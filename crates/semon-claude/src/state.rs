//! Durable, per-file incremental-capture cursor state.
//!
//! Mirrors semon-codex's atomic save/load shape (`crates/semon-codex/src/state.rs`)
//! but a fresh implementation rather than a shared one: Claude Code's cursor
//! carries no per-record data at all — just an offset and a line counter per
//! file, exactly like a flat carrier would. The DAG-ancestor map that
//! `parent_sequence`'s block-0 case needs (see
//! `crate::rebuild_ancestor_sequence`) is deliberately **not** part of this
//! durable state.
//!
//! It used to be: an early version stored one `ancestor_sequence` entry per
//! record `uuid` in this file, rewritten to disk after every 100-record
//! batch. Measured against the full real corpus, that produced a cursor file
//! that grew to megabytes and roughly 65x write amplification relative to
//! the store itself, because the whole ever-growing map was serialized on
//! every batch even though nothing in it changes size-wise once a file stops
//! growing. The map's own defining property — it is fully re-derivable by
//! replaying the file from byte zero — is exactly what makes it safe to
//! never persist: `crate::rebuild_ancestor_sequence` reconstructs it,
//! read-only, from the file itself whenever there is new data to resume
//! into, and pays nothing when a file has no new data.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde_json::{Map, Value};

use crate::AdapterError;

/// State document version. A version newer than this build understands is a
/// hard error (a future build may have written cursor semantics this build
/// cannot correctly interpret); anything older, missing, or unparsable
/// resets to a fresh cursor and replays every unconsumed file from offset
/// zero, which is safe because occurrences upsert on
/// `(carrier, session, sequence)` and canonical traces are content-addressed
/// (see `crates/semon-codex/src/state.rs`, which documents the identical
/// argument for that carrier).
const VERSION: i64 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable cursor state across every tracked Claude Code transcript file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorState {
    files: std::collections::BTreeMap<String, FileCursor>,
}

impl CursorState {
    pub(crate) fn take_file(&mut self, key: &str) -> FileCursor {
        self.files.remove(key).unwrap_or_default()
    }

    pub(crate) fn put_file(&mut self, key: String, cursor: FileCursor) {
        self.files.insert(key, cursor);
    }

    fn from_value(value: Value) -> Result<Self, AdapterError> {
        let object = value
            .as_object()
            .ok_or_else(|| AdapterError::State("state document is not a JSON object".into()))?;
        let version = object.get("version").and_then(Value::as_i64);
        match version {
            Some(version) if version > VERSION => {
                return Err(AdapterError::State(format!(
                    "unsupported state version: {version} (this build understands up to {VERSION})"
                )));
            }
            Some(VERSION) => {}
            _ => return Ok(Self::default()),
        }

        let files = match object.get("files") {
            None => std::collections::BTreeMap::new(),
            Some(Value::Object(files)) => files
                .iter()
                .map(|(key, value)| Ok((key.clone(), FileCursor::from_value(value)?)))
                .collect::<Result<_, AdapterError>>()?,
            Some(_) => return Err(AdapterError::State("files is not a JSON object".into())),
        };
        Ok(Self { files })
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
            ("version".into(), Value::Number(VERSION.into())),
        ]))
    }
}

/// Cursor state for one tracked transcript file: just an offset and a line
/// counter, deliberately as small as Codex's own per-file cursor. See the
/// module docs for why the DAG-ancestor map is not stored here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileCursor {
    /// Byte offset of the next unread line.
    pub(crate) offset: u64,
    /// The zero-based ordinal to assign to the next source line read from
    /// this file — this file's own line counter, not a write-time counter,
    /// so replaying the file from scratch (as happens on truncation)
    /// regenerates identical ordinals for identical source lines. This is
    /// the `line_ordinal` half of `sequence = line_ordinal * 1024 +
    /// block_index`.
    pub(crate) next_line_ordinal: u64,
}

impl FileCursor {
    fn from_value(value: &Value) -> Result<Self, AdapterError> {
        let object = value
            .as_object()
            .ok_or_else(|| AdapterError::State("file cursor is not a JSON object".into()))?;
        let offset = object.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let next_line_ordinal = object
            .get("next_line_ordinal")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Ok(Self {
            offset,
            next_line_ordinal,
        })
    }

    fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            (
                "next_line_ordinal".into(),
                Value::Number(self.next_line_ordinal.into()),
            ),
            ("offset".into(), Value::Number(self.offset.into())),
        ]))
    }
}

/// Loads cursor state, returning an empty state when absent.
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
