//! Incremental capture of Claude Code JSONL transcripts into Semon's
//! two-region store.
//!
//! Mirrors `semon-codex`'s shape (incremental, cursor-based, a cursor that
//! only advances past complete lines, truncation resets to zero) but Claude
//! Code's own transcript shape forces two departures, both settled by the
//! decided key in issue #21 and by
//! `docs/design/trace-identity-and-occurrences.md`:
//!
//! - **One file is one session.** A Claude Code session can span many files
//!   — the principal transcript at `<projects-root>/<slug>/<sessionId>.jsonl`
//!   and zero or more subagent transcripts at
//!   `<projects-root>/<slug>/<sessionId>/subagents/agent-<agentId>.jsonl` —
//!   and subagent files carry no `parentUuid` edge back into the principal
//!   file at all (measured: 0 of 207). So `session` is derived from the
//!   *file's own path*, never from a line ordinal shared across files: the
//!   principal file's session is its own `<sessionId>`; a subagent file's is
//!   `<sessionId>/agent-<agentId>`. See [`file_identity`].
//! - **Classification, and therefore `sequence`, is per block, not per
//!   record.** An assistant record's `message.content` can carry `text`,
//!   `thinking`, and `tool_use` blocks together, so `sequence` is
//!   `line_ordinal * 1024 + block_index`, and `parent_sequence` follows the
//!   `parentUuid` DAG within one file rather than a flat "previous record"
//!   pointer. See [`normalize`] and [`process_record`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use sha2::{Digest, Sha256};

mod normalize;
mod state;

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
};

pub use normalize::{MAX_BLOCKS_PER_RECORD, ProjectedBlock, classify_record};
use semon_store::{
    NewOccurrence, NewRawCarrierRecord, RawBackfillLine, RepoSource, SemanticCore, StoreError,
    TraceStore,
};
use serde_json::Value;
pub use state::{CursorState, load_state, save_state};
use thiserror::Error;

/// The forensic carrier label written beside captured Claude Code records.
pub const CARRIER: &str = "claude";

/// Totals from replaying saved Claude cursors.
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
    record: Option<Value>,
    session: String,
    sequence: i64,
    timestamp: i64,
}

fn derive_line(line: &[u8], ordinal: u64, session: &str) -> DerivedLine {
    let record = serde_json::from_slice::<Value>(line)
        .ok()
        .filter(Value::is_object);
    let timestamp = semon_codex::parse_timestamp(
        record
            .as_ref()
            .and_then(|record| record.get("timestamp"))
            .unwrap_or(&Value::Null),
    );
    DerivedLine {
        record,
        session: session.to_owned(),
        sequence: sequence_for(ordinal, 0),
        timestamp,
    }
}

/// Replays tracked file prefixes without changing the cursor state.
pub fn backfill_raw(
    state: &CursorState,
    store: &mut TraceStore,
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
        let (session_id, agent_id) = file_identity(&path);
        let session = session_label(&session_id, agent_id.as_deref());
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
            let derived = derive_line(&bytes, ordinal, &session);
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

/// Returns the shared default local Semon database path, identical to
/// `semon-codex`'s: both carriers write into one store (see
/// `docs/design/carrier-neutral-trace-storage.md`).
pub use semon_codex::default_store_path;

/// Errors produced while normalizing or incrementally capturing Claude Code
/// transcript data.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// A filesystem operation failed.
    #[error("Claude adapter I/O error: {0}")]
    Io(#[from] io::Error),

    /// A cursor state document was invalid.
    #[error("invalid Claude cursor state: {0}")]
    State(String),

    /// A JSON document could not be encoded or decoded.
    #[error("Claude adapter JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The trace store rejected a capture.
    #[error("Claude capture failed: {0}")]
    Store(#[from] StoreError),

    /// A batch limit was zero and could never make progress.
    #[error("batch_size and max_batch_bytes must be greater than zero")]
    InvalidBatchLimit,

    /// One record projected a block at or past the per-record block limit
    /// (see [`MAX_BLOCKS_PER_RECORD`]); `sequence`'s encoding cannot
    /// represent it without colliding with the next line's blocks.
    #[error(
        "record projects a block at index {0}, at or past the {MAX_BLOCKS_PER_RECORD}-block-per-record limit"
    )]
    TooManyBlocks(usize),
}

/// Cursor processing limits and paths for one Claude Code transcript file.
#[derive(Clone, Debug)]
pub struct ProcessOptions<'a> {
    /// Path at which cursor state is durably saved after each batch.
    pub state_path: &'a Path,
    /// Optional repository attribution override for every captured occurrence.
    pub repo_override: &'a str,
    /// Maximum complete records handled before saving the next cursor.
    pub batch_size: usize,
    /// Approximate maximum source bytes handled before saving the next cursor.
    pub max_batch_bytes: usize,
}

impl<'a> ProcessOptions<'a> {
    /// Creates options with the Codex adapter's original batch defaults.
    pub fn new(state_path: &'a Path) -> Self {
        Self {
            state_path,
            repo_override: "",
            batch_size: 100,
            max_batch_bytes: 1_000_000,
        }
    }
}

/// Returns this adapter's default durable cursor path.
pub fn default_state_path() -> PathBuf {
    if let Some(root) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("semon/claude-cursor.json");
    }
    home_dir().join(".local/state/semon/claude-cursor.json")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns every `.jsonl` transcript file under `projects_root`, sorted.
///
/// Recurses into `subagents/` directories: a Claude Code session can span
/// the principal transcript plus zero or more subagent transcripts (see the
/// module docs), and every one of them is a candidate file in its own right.
pub fn candidate_files(projects_root: &Path) -> Result<Vec<PathBuf>, AdapterError> {
    let mut files = Vec::new();
    if projects_root.exists() {
        collect_jsonl(projects_root, &mut files)?;
        files.sort();
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

/// Derives `(session_id, agent_id)` from a transcript file's own path.
///
/// `agent_id` is `Some` exactly when `path` matches
/// `.../<sessionId>/subagents/agent-<agentId>.jsonl`, in which case
/// `session_id` is the `<sessionId>` path component two levels up. Otherwise
/// `path` is treated as a principal transcript `.../<sessionId>.jsonl`, and
/// `session_id` is its file stem.
///
/// This is derived from the file's own path rather than from any in-record
/// field: the decided key (issue #21) keys occurrences on `session`, which
/// must be unique *by file*, and the path is the one thing guaranteed to
/// identify which file a line came from — a record's own `sessionId` field
/// does not distinguish a subagent file from its principal.
pub fn file_identity(path: &Path) -> (String, Option<String>) {
    let file_stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_owned();
    let parent = path.parent();
    let in_subagents_dir = parent
        .and_then(Path::file_name)
        .is_some_and(|name| name == "subagents");

    if in_subagents_dir && let Some(agent_id) = file_stem.strip_prefix("agent-") {
        let session_id = parent
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_owned();
        return (session_id, Some(agent_id.to_owned()));
    }

    (file_stem, None)
}

/// Renders the occurrence-region `session` value for a transcript file: the
/// bare session id for a principal transcript, `<sessionId>/agent-<id>` for
/// a subagent transcript. See issue #21's decided key.
fn session_label(session_id: &str, agent_id: Option<&str>) -> String {
    match agent_id {
        Some(agent_id) => format!("{session_id}/agent-{agent_id}"),
        None => session_id.to_owned(),
    }
}

/// Processes complete, not-yet-consumed lines and captures semantic records.
///
/// The returned count is the number of complete source records consumed,
/// not the number of occurrences captured — a line that projects zero, one,
/// or several blocks still advances the cursor by exactly one record.
pub fn process_file(
    path: &Path,
    state: &mut CursorState,
    store: &mut TraceStore,
    options: &ProcessOptions<'_>,
) -> Result<usize, AdapterError> {
    let source_key = resolved(path)?.to_string_lossy().into_owned();
    store.with_capture_source(CARRIER, &source_key, |store| {
        process_source_file(path, state, store, options)
    })
}

fn retained_prefix_lines(
    path: &Path,
    offset: u64,
    session: &str,
) -> Result<Vec<RawBackfillLine>, AdapterError> {
    let mut reader = BufReader::new(File::open(path)?.take(offset));
    let mut lines = Vec::new();
    let mut consumed = 0u64;
    while consumed < offset {
        let mut bytes = Vec::new();
        let count = reader.read_until(b'\n', &mut bytes)?;
        if count == 0 || !bytes.ends_with(b"\n") {
            return Err(AdapterError::State(
                "legacy cursor is not on a complete frame".into(),
            ));
        }
        consumed += count as u64;
        let derived = derive_line(&bytes, lines.len() as u64, session);
        lines.push(RawBackfillLine {
            bytes,
            session: derived.session,
            sequence: derived.sequence,
            timestamp: derived.timestamp,
        });
    }
    Ok(lines)
}

fn process_source_file(
    path: &Path,
    state: &mut CursorState,
    store: &mut TraceStore,
    options: &ProcessOptions<'_>,
) -> Result<usize, AdapterError> {
    if options.batch_size == 0 || options.max_batch_bytes == 0 {
        return Err(AdapterError::InvalidBatchLimit);
    }

    let key = resolved(path)?.to_string_lossy().into_owned();
    let size = fs::metadata(path)?.len();
    let mut saved = state.take_file(&key);
    let mut prefix = Sha256::new();
    if saved.offset <= size {
        let mut consumed = File::open(path)?.take(saved.offset);
        io::copy(&mut consumed, &mut prefix)?;
    }
    let prefix_matches = saved.offset == 0
        || saved.prefix_sha256.as_deref()
            == Some(format!("{:x}", prefix.clone().finalize()).as_str());
    let prefix_digest = format!("{:x}", prefix.clone().finalize());
    if saved.offset > 0
        && saved.offset <= size
        && prefix_matches
        && !store.has_capture_source_custody(CARRIER, &key)?
    {
        let lines = retained_prefix_lines(
            path,
            saved.offset,
            &session_label(&file_identity(path).0, file_identity(path).1.as_deref()),
        )?;
        store.adopt_retained_capture_prefix(CARRIER, &key, &lines, saved.offset, &prefix_digest)?;
    }
    let custody_matches =
        store.capture_source_cursor_matches(CARRIER, &key, saved.offset, &prefix_digest)?;
    if saved.offset > size || !prefix_matches || !custody_matches {
        store.reset_capture_source(CARRIER, &key)?;
        saved = state::FileCursor::default();
        prefix = Sha256::new();
        // The reset must reach disk even if the file below turns out to have
        // no complete line to consume right now (e.g. it was truncated
        // mid-line): otherwise the stale offset stays saved, and once the
        // file grows past it, the next run resumes from the stale offset and
        // silently skips the new prefix. An unchanged verified file never takes this branch and pays no
        // cursor write here. Older cursors without a digest replay once.
        state.put_file(key.clone(), saved.clone());
        save_state(options.state_path, state)?;
    }

    let (session_id, agent_id) = file_identity(path);
    let session = session_label(&session_id, agent_id.as_deref());

    let mut next_line_ordinal = saved.next_line_ordinal;
    // Rebuilt, never persisted — see `state`'s module docs. Only worth
    // paying for when there is new data to resume into; a fully consumed
    // file (`saved.offset == size`) needs no ancestor lookups at all, since
    // the read loop below will not process any line.
    let mut ancestor_sequence = if saved.offset > 0 && saved.offset < size {
        rebuild_ancestor_sequence(path, saved.offset)?
    } else {
        BTreeMap::new()
    };
    let mut consumed = 0;

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

            prefix.update(&line);
            batch_end = reader.stream_position()?;
            pending_bytes += line.len();
            batch_count += 1;

            // `line_ordinal` is this file's own zero-based line counter,
            // never a write-time or autoincrement counter, so a re-read of
            // the same bytes always regenerates the same `sequence` values
            // (see `docs/design/trace-identity-and-occurrences.md`).
            let this_line_ordinal = next_line_ordinal;
            next_line_ordinal += 1;

            let derived = derive_line(&line, this_line_ordinal, &session);
            if let Some(record) = derived.record {
                process_record(
                    store,
                    &record,
                    &line,
                    this_line_ordinal,
                    &derived.session,
                    derived.sequence,
                    derived.timestamp,
                    agent_id.as_deref(),
                    options.repo_override,
                    &mut ancestor_sequence,
                )?;
            } else {
                // A malformed or non-object complete line has no blocks,
                // but it is still retained once in the raw region. Block 0
                // is unused because no occurrence was projected.
                store.capture_raw_only(
                    CARRIER,
                    &line,
                    &derived.session,
                    derived.sequence,
                    derived.timestamp,
                )?;
            }
        }

        if batch_end == saved.offset {
            break;
        }

        saved.offset = batch_end;
        saved.prefix_sha256 = Some(format!("{:x}", prefix.clone().finalize()));
        saved.next_line_ordinal = next_line_ordinal;
        store.checkpoint_capture_source(
            CARRIER,
            &key,
            saved.offset,
            &format!("{:x}", prefix.clone().finalize()),
        )?;
        state.put_file(key.clone(), saved.clone());
        save_state(options.state_path, state)?;
        consumed += batch_count;
    }

    store.checkpoint_capture_source(
        CARRIER,
        &key,
        saved.offset,
        &format!("{:x}", prefix.clone().finalize()),
    )?;
    state.put_file(key, saved);
    Ok(consumed)
}

/// Reconstructs `ancestor_sequence` (see [`resolve_record`]) by replaying
/// `path` from byte zero up to `up_to_offset`, without capturing anything.
///
/// This is the read-only counterpart to never persisting the map: it is a
/// pure function of the file's own already-consumed bytes, so replaying
/// reproduces identical values to whatever an uninterrupted incremental run
/// would have computed — the same property that makes `sequence` itself
/// deterministic and safe to never assign at write time. Mirrors the same
/// kind of read-only, non-capturing prefix scan
/// `semon_codex::normalize`'s `file_uses_item_stream` already uses for a
/// different per-file fact.
///
/// Only called when `up_to_offset > 0`; a file with nothing consumed yet
/// starts with an empty map for free.
fn rebuild_ancestor_sequence(
    path: &Path,
    up_to_offset: u64,
) -> Result<BTreeMap<String, Option<i64>>, AdapterError> {
    let mut ancestor_sequence = BTreeMap::new();
    let mut reader = BufReader::new(File::open(path)?);
    let mut consumed = 0u64;
    let mut line_ordinal = 0u64;
    let mut line = Vec::new();

    while consumed < up_to_offset {
        line.clear();
        let bytes_read = reader.read_until(b'\n', &mut line)? as u64;
        if bytes_read == 0 {
            break;
        }
        consumed += bytes_read;
        // `up_to_offset` is always a byte offset `process_file` itself
        // saved, which only ever happens at a complete-line boundary — so
        // this defends against a corrupted or hand-edited cursor rather
        // than a case this adapter produces itself.
        if consumed > up_to_offset || !line.ends_with(b"\n") {
            break;
        }

        if let Ok(record) = serde_json::from_slice::<Value>(&line)
            && record.is_object()
        {
            resolve_record(&record, line_ordinal, &mut ancestor_sequence)?;
        }
        line_ordinal += 1;
    }

    Ok(ancestor_sequence)
}

/// Classifies one already-parsed record and updates `ancestor_sequence`
/// (this file's DAG-derivation state) for any later record that names this
/// one as its `parentUuid`. Shared between the real capturing path
/// ([`process_record`]) and the read-only replay
/// ([`rebuild_ancestor_sequence`]) that reconstructs the same map without
/// capturing anything — both must derive identical values from identical
/// bytes, so they share this one function rather than two copies of its
/// logic.
///
/// Returns the record's projected blocks and the resolved `parent_sequence`
/// for the *first projected* block of this record (`None` if the walk
/// reaches a root without finding a projected ancestor), per issue #21's
/// decided rule: a parent must always name an occurrence that actually
/// exists, so it follows position within `blocks` (the projected blocks, in
/// order), never the record's raw `block_index`, which skips over blocks
/// (`thinking`, harness content, …) that never became occurrences at all.
///
/// - for the *first* projected block of a record: the sequence of the last
///   projected block of the nearest ancestor reached by walking
///   `parentUuid`, within the same file (this is the value returned here);
/// - for every *later* projected block of a record: the sequence of the
///   *preceding projected block* of the same record — not `block_index -
///   1`, since a skipped block (e.g. `{thinking, tool_use}`, where
///   `thinking` never projects) would otherwise leave `block_index - 1`
///   naming an occurrence that was never captured. The caller computes this
///   case directly from `blocks`, by position, so only the first-block case
///   is returned here.
///
/// `ancestor_sequence` stores this resolved value per `uuid`, already
/// collapsed against that record's own parent (never a raw parent pointer),
/// so each lookup here is O(1): a record whose own blocks did not project
/// stores its *parent's* resolved value under its own `uuid`, which is what
/// lets a child several non-projecting records later still find the right
/// ancestor in one lookup. This is safe because a record's `parentUuid`
/// always names a record earlier in the same file (measured: Claude Code
/// session files are causally ordered), so by the time a record is
/// processed, its parent's entry already reflects its own resolution.
fn resolve_record(
    record: &Value,
    line_ordinal: u64,
    ancestor_sequence: &mut BTreeMap<String, Option<i64>>,
) -> Result<(Vec<ProjectedBlock>, Option<i64>), AdapterError> {
    let uuid = record
        .get("uuid")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let parent_uuid = record.get("parentUuid").and_then(Value::as_str);
    // Absent from the map (never seen, e.g. a cross-file or unknown parent)
    // and present-but-`None` (a resolved chain that never projected) both
    // mean the same thing here: no projected ancestor, i.e. a NULL parent.
    let ancestor_for_first = parent_uuid
        .and_then(|parent| ancestor_sequence.get(parent).copied())
        .flatten();

    let blocks = classify_record(record)?;

    if let Some(uuid) = uuid {
        // `blocks` is built by iterating the record's content array in
        // order and pushing only the blocks that project, so its last
        // element (if any) is the highest-`block_index` projected block —
        // exactly "this record's own last projected block".
        let resolved = blocks
            .last()
            .map(|block| sequence_for(line_ordinal, block.block_index))
            .or(ancestor_for_first);
        ancestor_sequence.insert(uuid, resolved);
    }

    Ok((blocks, ancestor_for_first))
}

/// Captures one already-parsed transcript record's projected blocks,
/// deriving each one's `parent_sequence` per issue #21's decided rule (see
/// [`resolve_record`]): the first projected block gets the ancestor's
/// sequence, and every later projected block gets the *preceding projected
/// block's* sequence — by position in `blocks`, never by raw `block_index`,
/// since a skipped (non-projecting) block must never be named as a parent.
#[allow(clippy::too_many_arguments)]
fn process_record(
    store: &mut TraceStore,
    record: &Value,
    raw_line: &[u8],
    line_ordinal: u64,
    session: &str,
    raw_sequence: i64,
    timestamp: i64,
    agent_id: Option<&str>,
    repo_override: &str,
    ancestor_sequence: &mut BTreeMap<String, Option<i64>>,
) -> Result<(), AdapterError> {
    let (blocks, ancestor_for_first) = resolve_record(record, line_ordinal, ancestor_sequence)?;
    let (repo, repo_source) = record_repo(record, repo_override);
    let mut captures = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        let sequence = sequence_for(line_ordinal, block.block_index);
        let parent_sequence = if index == 0 {
            ancestor_for_first
        } else {
            Some(sequence_for(line_ordinal, blocks[index - 1].block_index))
        };
        let core = SemanticCore::from_value(block.semantic_core.clone())?;
        let occurrence = NewOccurrence {
            session,
            sequence,
            timestamp,
            repo: &repo,
            repo_source,
            parent_sequence,
            agent: agent_id,
            authored_by: block.authored_by,
        };
        captures.push((core, occurrence));
    }

    store.capture_line(
        NewRawCarrierRecord::new(CARRIER, raw_line),
        session,
        raw_sequence,
        timestamp,
        &captures,
    )?;

    Ok(())
}

fn sequence_for(line_ordinal: u64, block_index: u32) -> i64 {
    (line_ordinal as i64) * (MAX_BLOCKS_PER_RECORD as i64) + i64::from(block_index)
}

/// Derives `(repo, repo_source)` for one record: `--repo`/`SEMON_REPO`
/// always wins, otherwise the basename of the record's own `cwd`, otherwise
/// none. Claude Code records carry no git remote URL, so
/// [`RepoSource::GitRemote`] never occurs for this carrier — see issue #21.
fn record_repo(record: &Value, repo_override: &str) -> (String, RepoSource) {
    if !repo_override.is_empty() {
        return (repo_override.to_owned(), RepoSource::ExplicitOverride);
    }
    let repo = semon_codex::repo_from_cwd(record.get("cwd").unwrap_or(&Value::Null));
    if repo.is_empty() {
        (String::new(), RepoSource::None)
    } else {
        (repo, RepoSource::CwdBasename)
    }
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
