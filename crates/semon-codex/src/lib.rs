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
    io::{self, BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
    str::FromStr,
};

pub use normalize::{
    NormalizeContext, apply_path_rule, history_event, infer_success, json_text, message_text,
    normalize_record, parse_timestamp, repo_from_cwd, repo_from_url, token_usage,
};
use semon_store::{
    AuthoredBy, CaptureResult, NewOccurrence, NewRawCarrierRecord, RawBackfillLine, RepoSource,
    SemanticCore, StoreError, TraceStore,
};
use serde_json::{Value, json};
pub use state::{CursorState, load_state, save_state};
use thiserror::Error;

/// The forensic carrier label written beside captured Codex records.
pub const CARRIER: &str = "codex";

/// Totals from replaying saved Codex cursors.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct BackfillReport {
    /// Files recorded in the cursor state.
    pub files_scanned: usize,
    /// Complete lines within saved offsets.
    pub lines_scanned: usize,
    /// Rows inserted, or rows that would be inserted in a dry run.
    pub rows_inserted: usize,
    /// Lines whose exact raw row already exists.
    pub rows_already_present: usize,
    /// Files absent from disk.
    pub missing: Vec<PathBuf>,
    /// Files shorter than their saved offsets.
    pub rewritten: Vec<PathBuf>,
    /// Files with conflicting raw bytes, paired with the first sequence.
    pub misaligned: Vec<(PathBuf, i64)>,
}

struct DerivedLine {
    event: Option<Value>,
    session: String,
    sequence: i64,
    timestamp: i64,
}

fn derive_line(
    line: &[u8],
    ordinal: u64,
    context: &mut NormalizeContext,
    history_source: bool,
    session_repos: &std::collections::BTreeMap<String, (String, RepoSource)>,
    repo_override: &str,
) -> DerivedLine {
    let event = serde_json::from_slice::<Value>(line)
        .ok()
        .filter(Value::is_object)
        .map(|record| {
            if history_source {
                history_event(&record, session_repos, repo_override)
            } else {
                normalize_record(&record, context, repo_override)
            }
        });
    let session = event
        .as_ref()
        .and_then(|event| event.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| context.session_id())
        .to_owned();
    let timestamp = event
        .as_ref()
        .and_then(|event| event.get("ts"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| parse_timestamp(&Value::Null));
    DerivedLine {
        event,
        session,
        sequence: ordinal as i64,
        timestamp,
    }
}

/// Replays tracked file prefixes without changing the cursor state.
pub fn backfill_raw(
    state: &CursorState,
    store: &mut TraceStore,
    history_path: &Path,
    repo_override: &str,
    dry_run: bool,
) -> Result<BackfillReport, AdapterError> {
    let mut report = BackfillReport::default();
    for (key, saved) in state.files() {
        report.files_scanned += 1;
        let path = PathBuf::from(key);
        let size = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.missing.push(path);
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if size < saved.offset {
            report.rewritten.push(path);
            continue;
        }
        let history_source = same_path(&path, history_path)?;
        let mut context = NormalizeContext::default();
        if !history_source {
            context.has_item_stream = saved.context().has_item_stream
                || file_uses_item_stream_prefix(&path, saved.offset)?;
        }
        let mut reader = BufReader::new(File::open(&path)?.take(saved.offset));
        let mut lines = Vec::new();
        let mut ordinal = 0;
        let mut consumed = 0;
        while consumed < saved.offset {
            let mut bytes = Vec::new();
            let count = reader.read_until(b'\n', &mut bytes)?;
            if count == 0 || !bytes.ends_with(b"\n") {
                return Err(AdapterError::State(format!(
                    "cursor is not at a complete line in {}",
                    path.display()
                )));
            }
            consumed += count as u64;
            let derived = derive_line(
                &bytes,
                ordinal,
                &mut context,
                history_source,
                state.session_repos(),
                repo_override,
            );
            lines.push(RawBackfillLine {
                bytes,
                session: derived.session,
                sequence: derived.sequence,
                timestamp: derived.timestamp,
            });
            ordinal += 1;
        }
        report.lines_scanned += lines.len();
        let result = store.backfill_raw_lines(CARRIER, &lines, dry_run)?;
        report.rows_already_present += result.already_present;
        if let Some(sequence) = result.misaligned_sequence {
            report.misaligned.push((path, sequence));
        } else {
            report.rows_inserted += result.inserted;
        }
    }
    Ok(report)
}

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
/// advance the cursor and are retained in the raw region, but they have no
/// reactivation value and therefore create no trace or occurrence.
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
    // Local, per-batch-committed mirrors of the occurrence derivation state
    // that lives in `saved` between calls (see `FileCursor::next_line_ordinal`
    // and `FileCursor::last_projected_sequence`). Mirrored the same way
    // `context` is: read from `saved` once, threaded through every line in
    // every batch, and written back into `saved` only at each batch boundary
    // alongside `saved.update_context(&context)`.
    let mut next_line_ordinal = saved.next_line_ordinal;
    let mut last_projected_sequence = saved.last_projected_sequence.clone();
    let mut consumed = 0;
    let history_source = same_path(path, options.history_path)?;
    // Whether this file carries the `item_completed` item stream decides
    // whether the legacy `response_item`/`message` mirror still projects
    // (see `NormalizeContext::has_item_stream`). That decision has to be
    // known for the *whole* file before any of its lines are normalized:
    // Codex writes a harness-injected mirror message (role `user`, no item
    // stream counterpart at all, e.g. `<recommended_plugins>`) before the
    // first `item_completed` line of the same session, so a flag latched
    // only once that first line is reached would miss it. A history file
    // never carries this stream and is skipped; once latched true the flag
    // is never rechecked, and once a file is fully consumed there is no new
    // line left for the flag to change the outcome of, so neither case pays
    // for a rescan.
    if !history_source && !context.has_item_stream && saved.offset < size {
        context.has_item_stream = file_uses_item_stream(path)?;
    }
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

            // `sequence` is the zero-based ordinal of this source line within
            // its session file — never a write-time or autoincrement
            // counter — so a re-read of the same bytes always regenerates
            // the same value. It is assigned to every complete line, whether
            // or not that line goes on to project an occurrence.
            let this_line_ordinal = next_line_ordinal;
            next_line_ordinal += 1;

            let derived = derive_line(
                &line,
                this_line_ordinal,
                &mut context,
                history_source,
                state.session_repos(),
                options.repo_override,
            );
            match derived.event {
                Some(event) => {
                    let session = derived.session;
                    let repo = event
                        .get("repo")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let repo_source = event
                        .get("repo_source")
                        .and_then(Value::as_str)
                        .and_then(|value| RepoSource::from_str(value).ok())
                        .unwrap_or(RepoSource::None);
                    let timestamp = derived.timestamp;
                    // The previous *projected* record in this session, never
                    // a locally tracked "last row written" value: this map
                    // is itself fully re-derivable by replaying the file
                    // from its start (see `FileCursor::last_projected_sequence`).
                    let parent_sequence = last_projected_sequence.get(&session).copied();
                    let occurrence = NewOccurrence {
                        session: &session,
                        sequence: derived.sequence,
                        timestamp,
                        repo: &repo,
                        repo_source,
                        parent_sequence,
                        agent: None,
                        authored_by: authored_by_for_kind(
                            event.get("kind").and_then(Value::as_str).unwrap_or(""),
                        ),
                    };
                    if capture_event(store, &event, &line, occurrence)?.is_some() {
                        last_projected_sequence.insert(session, this_line_ordinal as i64);
                    }
                }
                None => {
                    store.capture_raw_only(
                        CARRIER,
                        &line,
                        &derived.session,
                        derived.sequence,
                        derived.timestamp,
                    )?;
                }
            }
        }

        if batch_end == saved.offset {
            break;
        }

        saved.offset = batch_end;
        saved.update_context(&context);
        saved.next_line_ordinal = next_line_ordinal;
        saved.last_projected_sequence = last_projected_sequence.clone();
        if !context.session_id().is_empty() && !context.repo().is_empty() {
            state.remember_repo(context.session_id(), context.repo(), context.repo_source());
        }
        state.put_file(key.clone(), saved.clone());
        save_state(options.state_path, state)?;
        consumed += batch_count;
    }

    state.put_file(key, saved);
    Ok(consumed)
}

/// Captures one already-normalized event.
///
/// The projection intentionally does not inspect `extras`: that field contains
/// the complete Codex source record, including machine paths and format words.
/// It also omits session/repository/timestamp/token/tool framing fields. Text
/// authored as the work's intent or outcome is kept verbatim; redacting words
/// inside that content would alter the semantics rather than remove an
/// envelope. An event with no semantic core is retained only in the raw
/// region and returns `None`.
pub fn capture_event(
    store: &mut TraceStore,
    normalized: &Value,
    original_record: &[u8],
    occurrence: NewOccurrence<'_>,
) -> Result<Option<CaptureResult>, AdapterError> {
    let Some(core) = semantic_core(normalized)? else {
        store.capture_raw_only(
            CARRIER,
            original_record,
            occurrence.session,
            occurrence.sequence,
            occurrence.timestamp,
        )?;
        return Ok(None);
    };
    Ok(Some(store.capture(
        &core,
        NewRawCarrierRecord::new(CARRIER, original_record),
        occurrence,
    )?))
}

fn semantic_core(event: &Value) -> Result<Option<SemanticCore>, StoreError> {
    let kind = event.get("kind").and_then(Value::as_str).unwrap_or("");
    match kind {
        "user_prompt" | "history_entry" => text_core("intent", event, "prompt"),
        "assistant_response" | "turn_complete" => text_core("outcome", event, "response"),
        "item_user_message" => text_core("intent", event, "content"),
        "item_agent_message" => text_core("outcome", event, "content"),
        "item_command_execution" => command_execution_core(event),
        "item_file_change" => file_change_core(event),
        _ => Ok(None),
    }
}

/// Maps a normalized event's `kind` to who or what authored it, per the
/// occurrence region's `authored_by` vocabulary. Only `kind`s that
/// [`semantic_core`] actually projects matter here; anything else is
/// classified but discarded along with the rest of the occurrence facts
/// when [`semantic_core`] returns `None`.
///
/// `agent` is always `None` for Codex, which has no subagent concept — the
/// column exists for a future carrier (e.g. the Claude adapter) that does.
fn authored_by_for_kind(kind: &str) -> AuthoredBy {
    match kind {
        "item_user_message" | "user_prompt" | "history_entry" => AuthoredBy::Human,
        "item_agent_message"
        | "assistant_response"
        | "turn_complete"
        | "item_command_execution"
        | "item_file_change" => AuthoredBy::Agent,
        _ => AuthoredBy::Unknown,
    }
}

fn text_core(
    semantic_kind: &str,
    event: &Value,
    field: &str,
) -> Result<Option<SemanticCore>, StoreError> {
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

// `action` is a new third `kind` value alongside `intent` and `outcome`. It
// needs no `semon-store` change: the semantic core is unconstrained JSON.
//
// Unlike the text kinds above, an action core is captured even when it is
// near-contentless (an unparsed command projects `action: "unknown"`,
// `path: null`). That cost — roughly half of observed commands, per issue
// #9's census — was accepted deliberately: the alternative is silently
// dropping half of all command activity.
fn command_execution_core(event: &Value) -> Result<Option<SemanticCore>, StoreError> {
    let action = event
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let path = event.get("path").cloned().unwrap_or(Value::Null);
    let exit_code = event.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
    SemanticCore::from_value(json!({
        "kind": "action",
        "action": action,
        "path": path,
        "exit_code": exit_code,
    }))
    .map(Some)
}

fn file_change_core(event: &Value) -> Result<Option<SemanticCore>, StoreError> {
    let changes = event
        .get("changes")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    if changes.as_array().is_none_or(Vec::is_empty) {
        return Ok(None);
    }
    SemanticCore::from_value(json!({
        "kind": "action",
        "action": "file_change",
        "changes": changes,
    }))
    .map(Some)
}

/// Scans a session file from its start for the first `event_msg`/
/// `item_completed` record, without holding more than one line in memory.
///
/// This intentionally reads independently of any saved cursor offset: the
/// question is whether the *file* carries the item stream at all, not
/// whether the unconsumed remainder does.
fn file_uses_item_stream(path: &Path) -> Result<bool, AdapterError> {
    file_uses_item_stream_prefix(path, u64::MAX)
}

fn file_uses_item_stream_prefix(path: &Path, limit: u64) -> Result<bool, AdapterError> {
    let mut reader = BufReader::new(File::open(path)?.take(limit));
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(false);
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some(object) = record.as_object() else {
            continue;
        };
        let is_item_completed = object.get("type").and_then(Value::as_str) == Some("event_msg")
            && object
                .get("payload")
                .and_then(Value::as_object)
                .and_then(|payload| payload.get("type"))
                .and_then(Value::as_str)
                == Some("item_completed");
        if is_item_completed {
            return Ok(true);
        }
    }
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
