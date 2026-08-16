//! Incremental capture of Codex JSONL sessions into Semon's two-region store.
//!
//! Codex records are first normalized so the format archaeology from the
//! original tailer remains available. Capture then deliberately projects only
//! carrier-neutral work content into the semantic region. The original JSONL
//! line is passed unchanged to the store's forensic region in the same call.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod normalize;
mod state;

use std::{
    env,
    fs::{self, File},
    io::{self, BufRead, BufReader, Seek},
    path::{Path, PathBuf},
};

pub use normalize::{
    NormalizeContext, history_event, infer_success, json_text, message_text, normalize_record,
    parse_timestamp, repo_from_cwd, repo_from_url, token_usage,
};
use semon_store::{CaptureResult, NewRawCarrierRecord, SemanticCore, StoreError, TraceStore};
use serde_json::{Value, json};
pub use state::{CursorState, load_state, save_state};
use thiserror::Error;

/// The forensic carrier label written beside captured Codex records.
pub const CARRIER: &str = "codex";

/// Errors produced while normalizing or incrementally capturing Codex data.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// A filesystem operation failed.
    #[error("Codex adapter I/O error: {0}")]
    Io(#[from] io::Error),

    /// A cursor state document was invalid.
    #[error("invalid Codex cursor state: {0}")]
    State(String),

    /// A JSON document could not be encoded or decoded.
    #[error("Codex adapter JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The trace store rejected a capture.
    #[error("Codex capture failed: {0}")]
    Store(#[from] StoreError),

    /// A batch limit was zero and could never make progress.
    #[error("batch_size and max_batch_bytes must be greater than zero")]
    InvalidBatchLimit,
}

/// Cursor processing limits and paths for one Codex source file.
#[derive(Clone, Debug)]
pub struct ProcessOptions<'a> {
    /// Path at which cursor state is durably saved after each batch.
    pub state_path: &'a Path,
    /// The configured Codex history file, used to select history normalization.
    pub history_path: &'a Path,
    /// Optional repository attribution override for the normalized envelope.
    pub repo_override: &'a str,
    /// Maximum complete records handled before saving the next cursor.
    pub batch_size: usize,
    /// Approximate maximum source bytes handled before saving the next cursor.
    pub max_batch_bytes: usize,
}

impl<'a> ProcessOptions<'a> {
    /// Creates options with the original tailer's batch defaults.
    pub fn new(state_path: &'a Path, history_path: &'a Path) -> Self {
        Self {
            state_path,
            history_path,
            repo_override: "",
            batch_size: 100,
            max_batch_bytes: 1_000_000,
        }
    }
}

/// Returns the original tailer's default durable cursor path.
pub fn default_state_path() -> PathBuf {
    state_path_from(
        env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()),
        home_dir(),
    )
}

fn state_path_from(xdg_state_home: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
    if let Some(root) = xdg_state_home {
        return PathBuf::from(root).join("devlog/codex-tailer.json");
    }
    home.join(".local/state/devlog/codex-tailer.json")
}

/// Returns the default local Semon database path used by the capture binary.
pub fn default_store_path() -> PathBuf {
    if let Some(root) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("semon/traces.sqlite3");
    }
    home_dir().join(".local/share/semon/traces.sqlite3")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns session JSONL files recursively, sorted, followed by history.
pub fn candidate_files(sessions: &Path, history: &Path) -> Result<Vec<PathBuf>, AdapterError> {
    let mut files = Vec::new();
    if sessions.exists() {
        collect_jsonl(sessions, &mut files)?;
        files.sort();
    }
    if history.exists() {
        files.push(history.to_path_buf());
    }
    Ok(files)
}

fn collect_jsonl(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), io::Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_jsonl(&path, files)?;
        } else if file_type.is_file() && path.extension().is_some_and(|value| value == "jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

/// Processes complete, not-yet-consumed lines and captures semantic records.
///
/// The returned count is the number of complete source records consumed, not
/// the number of canonical traces inserted. Metadata and token telemetry still
/// advance the cursor, matching the original incremental surface, but they do
/// not have reactivation value and therefore do not enter the trace store.
pub fn process_file(
    path: &Path,
    state: &mut CursorState,
    store: &mut TraceStore,
    options: &ProcessOptions<'_>,
) -> Result<usize, AdapterError> {
    if options.batch_size == 0 || options.max_batch_bytes == 0 {
        return Err(AdapterError::InvalidBatchLimit);
    }

    let key = resolved(path)?;
    let key = key.to_string_lossy().into_owned();
    let size = fs::metadata(path)?.len();
    let mut saved = state.take_file(&key);
    if saved.offset > size {
        saved = state::FileCursor::default();
    }
    let mut context = saved.context();
    let mut consumed = 0;
    let history_source = same_path(path, options.history_path)?;
    let mut reader = BufReader::new(File::open(path)?);
    reader.seek(io::SeekFrom::Start(saved.offset))?;

    loop {
        let mut batch_count = 0;
        let mut pending_bytes = 0;
        let mut batch_end = reader.stream_position()?;

        while batch_count < options.batch_size && pending_bytes < options.max_batch_bytes {
            let start = reader.stream_position()?;
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if !line.ends_with(b"\n") {
                reader.seek(io::SeekFrom::Start(start))?;
                break;
            }

            batch_end = reader.stream_position()?;
            pending_bytes += line.len();
            batch_count += 1;

            match serde_json::from_slice::<Value>(&line) {
                Ok(record) if record.is_object() => {
                    let event = if history_source {
                        history_event(&record, state.session_repos(), options.repo_override)
                    } else {
                        normalize_record(&record, &mut context, options.repo_override)
                    };
                    capture_event(store, &event, &line)?;
                }
                Ok(_) => {
                    // Python classified a non-object JSON value as a parse error.
                }
                Err(_) => {
                    // A malformed *complete* line is consumed just as before.
                    // It has no transferable semantics, so it is not captured.
                }
            }
        }

        if batch_end == saved.offset {
            break;
        }

        saved.offset = batch_end;
        saved.update_context(&context);
        if !context.session_id().is_empty() && !context.repo().is_empty() {
            state.remember_repo(context.session_id(), context.repo());
        }
        state.put_file(key.clone(), saved.clone());
        save_state(options.state_path, state)?;
        consumed += batch_count;
    }

    state.put_file(key, saved);
    Ok(consumed)
}

/// Captures one already-normalized event when it carries transferable work.
///
/// The projection intentionally does not inspect `extras`: that field contains
/// the complete Codex source record, including machine paths and format words.
/// It also omits session/repository/timestamp/token/tool framing fields. Text
/// authored as the work's intent or outcome is kept verbatim; redacting words
/// inside that content would alter the semantics rather than remove an
/// envelope.
pub fn capture_event(
    store: &mut TraceStore,
    normalized: &Value,
    original_record: &[u8],
) -> Result<Option<CaptureResult>, AdapterError> {
    let Some(core) = semantic_core(normalized)? else {
        return Ok(None);
    };
    Ok(Some(store.capture(
        &core,
        NewRawCarrierRecord::new(CARRIER, original_record),
    )?))
}

fn semantic_core(event: &Value) -> Result<Option<SemanticCore>, StoreError> {
    let kind = event.get("kind").and_then(Value::as_str).unwrap_or("");
    let (semantic_kind, field) = match kind {
        "user_prompt" | "history_entry" => ("intent", "prompt"),
        "assistant_response" | "turn_complete" => ("outcome", "response"),
        _ => return Ok(None),
    };
    let content = event.get(field).and_then(Value::as_str).unwrap_or("");
    if content.is_empty() {
        return Ok(None);
    }
    SemanticCore::from_value(json!({
        "kind": semantic_kind,
        "content": content,
    }))
    .map(Some)
}

fn same_path(first: &Path, second: &Path) -> Result<bool, io::Error> {
    Ok(resolved(first)? == resolved(second)?)
}

fn resolved(path: &Path) -> Result<PathBuf, io::Error> {
    if path.exists() {
        path.canonicalize()
    } else if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests;
