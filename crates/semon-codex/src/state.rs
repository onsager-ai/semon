use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{Map, Value};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{AdapterError, NormalizeContext};

const VERSION: i64 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable cursor and cross-file repository attribution state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorState {
    files: BTreeMap<String, FileCursor>,
    session_repos: BTreeMap<String, String>,
}

impl CursorState {
    pub(crate) fn take_file(&mut self, key: &str) -> FileCursor {
        self.files.remove(key).unwrap_or_default()
    }

    pub(crate) fn put_file(&mut self, key: String, cursor: FileCursor) {
        self.files.insert(key, cursor);
    }

    pub(crate) fn session_repos(&self) -> &BTreeMap<String, String> {
        &self.session_repos
    }

    pub(crate) fn remember_repo(&mut self, session_id: &str, repo: &str) {
        self.session_repos
            .insert(session_id.to_owned(), repo.to_owned());
    }

    fn from_value(value: Value) -> Result<Self, AdapterError> {
        let object = value
            .as_object()
            .ok_or_else(|| AdapterError::State("state document is not a JSON object".into()))?;
        let version = object.get("version").and_then(Value::as_i64);
        if version != Some(VERSION) {
            return Err(AdapterError::State(format!(
                "unsupported state version: {}",
                object.get("version").unwrap_or(&Value::Null)
            )));
        }
        let files = match object.get("files") {
            None => BTreeMap::new(),
            Some(Value::Object(files)) => files
                .iter()
                .map(|(key, value)| Ok((key.clone(), FileCursor::from_value(value)?)))
                .collect::<Result<_, AdapterError>>()?,
            Some(_) => return Err(AdapterError::State("files is not a JSON object".into())),
        };
        let session_repos = match object.get("session_repos") {
            None => BTreeMap::new(),
            Some(Value::Object(repos)) => repos
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_owned()))
                })
                .collect(),
            Some(_) => {
                return Err(AdapterError::State(
                    "session_repos is not a JSON object".into(),
                ));
            }
        };
        Ok(Self {
            files,
            session_repos,
        })
    }

    fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            (
                "files".into(),
                Value::Object(Map::from_iter(
                    self.files
                        .iter()
                        .map(|(key, value)| (key.clone(), value.to_value())),
                )),
            ),
            (
                "session_repos".into(),
                Value::Object(Map::from_iter(
                    self.session_repos
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone()))),
                )),
            ),
            ("version".into(), Value::Number(VERSION.into())),
        ]))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileCursor {
    pub(crate) offset: u64,
    session_id: String,
    repo: String,
    cwd: String,
    calls: Map<String, Value>,
    /// Latched once this file is seen to carry the `item_completed` item
    /// stream; never reset back to `false` (see `NormalizeContext`).
    has_item_stream: bool,
}

impl FileCursor {
    pub(crate) fn context(&self) -> NormalizeContext {
        NormalizeContext::from_parts(
            self.session_id.clone(),
            self.repo.clone(),
            self.cwd.clone(),
            self.calls.clone(),
            self.has_item_stream,
        )
    }

    pub(crate) fn update_context(&mut self, context: &NormalizeContext) {
        self.session_id.clone_from(&context.session_id);
        self.repo.clone_from(&context.repo);
        self.cwd.clone_from(&context.cwd);
        self.calls.clone_from(&context.calls);
        self.has_item_stream = context.has_item_stream;
    }

    fn from_value(value: &Value) -> Result<Self, AdapterError> {
        let object = value
            .as_object()
            .ok_or_else(|| AdapterError::State("file cursor is not a JSON object".into()))?;
        let offset = object.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let string = |key| {
            object
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let calls = object
            .get("calls")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let has_item_stream = object
            .get("has_item_stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Self {
            offset,
            session_id: string("session_id"),
            repo: string("repo"),
            cwd: string("cwd"),
            calls,
            has_item_stream,
        })
    }

    fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("calls".into(), sort_json(Value::Object(self.calls.clone()))),
            ("cwd".into(), Value::String(self.cwd.clone())),
            ("has_item_stream".into(), Value::Bool(self.has_item_stream)),
            ("offset".into(), Value::Number(self.offset.into())),
            ("repo".into(), Value::String(self.repo.clone())),
            ("session_id".into(), Value::String(self.session_id.clone())),
        ]))
    }
}

/// Loads cursor state, returning an empty version-one state when absent.
pub fn load_state(path: &Path) -> Result<CursorState, AdapterError> {
    match fs::read(path) {
        Ok(bytes) => CursorState::from_value(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CursorState::default()),
        Err(error) => Err(error.into()),
    }
}

/// Atomically saves cursor state beside the destination path.
pub fn save_state(path: &Path, state: &CursorState) -> Result<(), AdapterError> {
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
        let mut open_options = OpenOptions::new();
        open_options.write(true).create_new(true);
        #[cfg(unix)]
        open_options.mode(0o600);
        let mut file = open_options.open(&temporary)?;
        serde_json::to_writer(&mut file, &state.to_value())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok::<_, AdapterError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(Map::from_iter(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json(value))),
            ))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_json).collect()),
        value => value,
    }
}
