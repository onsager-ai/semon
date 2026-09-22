use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{Map, Value};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::ZERO_CHAIN;

const VERSION: u64 = 3;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable sender state, keyed by the protocol's session and stream fields.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelayState {
    pub streams: BTreeMap<(String, String), StreamState>,
}

/// The contiguous acknowledgement position for one source stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamState {
    pub path: PathBuf,
    pub acked: Option<u64>,
    pub chain: [u8; 32],
    pub generation: u64,
    /// Lease epoch stamped on live and orphan frames from this source.
    pub epoch: u64,
    /// A fenced stream never sends to live history again.
    pub fenced: bool,
    /// Highest sequence retained in the orphan namespace.
    pub orphaned: Option<u64>,
    /// Byte immediately after the acknowledged line.
    pub offset: u64,
    /// Device and inode identify replacement between resident passes.
    pub device: u64,
    pub inode: u64,
    /// Byte at which the acknowledged line begins.
    pub last_line_start: Option<u64>,
    /// SHA-256 of the acknowledged line, including its newline.
    pub last_line_hash: Option<[u8; 32]>,
}

impl StreamState {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            acked: None,
            chain: ZERO_CHAIN,
            generation: 0,
            epoch: 0,
            fenced: false,
            orphaned: None,
            offset: 0,
            device: 0,
            inode: 0,
            last_line_start: None,
            last_line_hash: None,
        }
    }
}

/// Errors reading or replacing the sender state document.
#[derive(Debug, Error)]
pub enum StateError {
    #[error("cannot access relay state: {0}")]
    Io(#[from] io::Error),
    #[error("cannot decode relay state JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid relay state: {0}")]
    Invalid(String),
    #[error("state path is not valid UTF-8: {0:?}")]
    NonUtf8Path(PathBuf),
}

/// Loads sender state, returning an empty document when it does not exist.
pub fn load_state(path: &Path) -> Result<RelayState, StateError> {
    match fs::read(path) {
        Ok(bytes) => RelayState::from_value(&serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RelayState::default()),
        Err(error) => Err(error.into()),
    }
}

/// Atomically writes sender state with private filesystem permissions.
pub fn save_state(path: &Path, state: &RelayState) -> Result<(), StateError> {
    let bytes = serde_json::to_vec(&state.to_value()?)?;
    write_atomic(path, &bytes)?;
    Ok(())
}

impl RelayState {
    fn to_value(&self) -> Result<Value, StateError> {
        let streams = self
            .streams
            .iter()
            .map(|((session, stream), state)| {
                let path = state
                    .path
                    .to_str()
                    .ok_or_else(|| StateError::NonUtf8Path(state.path.clone()))?;
                Ok(Value::Object(Map::from_iter([
                    (
                        "acked".into(),
                        state.acked.map_or(Value::Null, |value| value.into()),
                    ),
                    ("chain".into(), Value::String(hex::encode(state.chain))),
                    ("device".into(), Value::Number(state.device.into())),
                    ("epoch".into(), Value::Number(state.epoch.into())),
                    ("fenced".into(), Value::Bool(state.fenced)),
                    ("generation".into(), Value::Number(state.generation.into())),
                    ("inode".into(), Value::Number(state.inode.into())),
                    (
                        "last_line_hash".into(),
                        state
                            .last_line_hash
                            .map_or(Value::Null, |hash| Value::String(hex::encode(hash))),
                    ),
                    (
                        "last_line_start".into(),
                        state
                            .last_line_start
                            .map_or(Value::Null, |value| value.into()),
                    ),
                    ("offset".into(), Value::Number(state.offset.into())),
                    (
                        "orphaned".into(),
                        state.orphaned.map_or(Value::Null, |value| value.into()),
                    ),
                    ("path".into(), Value::String(path.to_owned())),
                    ("session".into(), Value::String(session.clone())),
                    ("stream".into(), Value::String(stream.clone())),
                ])))
            })
            .collect::<Result<Vec<_>, StateError>>()?;
        Ok(Value::Object(Map::from_iter([
            ("streams".into(), Value::Array(streams)),
            ("version".into(), Value::Number(VERSION.into())),
        ])))
    }

    fn from_value(value: &Value) -> Result<Self, StateError> {
        let object = value
            .as_object()
            .ok_or_else(|| StateError::Invalid("document is not an object".into()))?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| StateError::Invalid("version is missing".into()))?;
        if !matches!(version, 1 | 2 | VERSION) {
            return Err(StateError::Invalid(format!(
                "unsupported version {version}; expected 1, 2, or {VERSION}"
            )));
        }
        let entries = object
            .get("streams")
            .and_then(Value::as_array)
            .ok_or_else(|| StateError::Invalid("streams is not an array".into()))?;
        let mut streams = BTreeMap::new();
        for value in entries {
            let entry = value
                .as_object()
                .ok_or_else(|| StateError::Invalid("stream entry is not an object".into()))?;
            let string = |field: &str| {
                entry
                    .get(field)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| StateError::Invalid(format!("stream {field} is missing")))
            };
            let session = string("session")?;
            let stream = string("stream")?;
            let chain = hex::decode(string("chain")?)
                .map_err(|error| StateError::Invalid(format!("invalid stream chain: {error}")))?;
            let chain_length = chain.len();
            let chain = chain.try_into().map_err(|_| {
                StateError::Invalid(format!(
                    "stream chain has {chain_length} bytes instead of 32"
                ))
            })?;
            let acked = match entry.get("acked") {
                Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or_else(|| {
                    StateError::Invalid("stream acked is not an unsigned integer".into())
                })?),
                None => return Err(StateError::Invalid("stream acked is missing".into())),
            };
            let generation = entry
                .get("generation")
                .and_then(Value::as_u64)
                .ok_or_else(|| StateError::Invalid("stream generation is missing".into()))?;
            let unsigned = |field: &str| {
                entry
                    .get(field)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| StateError::Invalid(format!("stream {field} is missing")))
            };
            let optional_unsigned = |field: &str| match entry.get(field) {
                Some(Value::Null) | None => Ok(None),
                Some(value) => value.as_u64().map(Some).ok_or_else(|| {
                    StateError::Invalid(format!("stream {field} is not an unsigned integer"))
                }),
            };
            let last_line_hash = match entry.get("last_line_hash") {
                Some(Value::String(encoded)) => {
                    let bytes = hex::decode(encoded).map_err(|error| {
                        StateError::Invalid(format!("invalid last line hash: {error}"))
                    })?;
                    let length = bytes.len();
                    Some(bytes.try_into().map_err(|_| {
                        StateError::Invalid(format!(
                            "last line hash has {length} bytes instead of 32"
                        ))
                    })?)
                }
                Some(Value::Null) | None => None,
                Some(_) => {
                    return Err(StateError::Invalid(
                        "stream last_line_hash is not a string or null".into(),
                    ));
                }
            };
            let state = StreamState {
                path: string("path")?.into(),
                acked,
                chain,
                generation,
                epoch: if version < 3 { 0 } else { unsigned("epoch")? },
                fenced: if version < 3 {
                    false
                } else {
                    entry
                        .get("fenced")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| StateError::Invalid("stream fenced is missing".into()))?
                },
                orphaned: if version < 3 {
                    None
                } else {
                    optional_unsigned("orphaned")?
                },
                offset: if version == 1 { 0 } else { unsigned("offset")? },
                device: if version == 1 { 0 } else { unsigned("device")? },
                inode: if version == 1 { 0 } else { unsigned("inode")? },
                last_line_start: if version == 1 {
                    None
                } else {
                    optional_unsigned("last_line_start")?
                },
                last_line_hash: if version == 1 { None } else { last_line_hash },
            };
            if streams.insert((session, stream), state).is_some() {
                return Err(StateError::Invalid("duplicate stream entry".into()));
            }
        }
        Ok(Self { streams })
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state");
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{name}.{}.{}", process::id(), sequence));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
