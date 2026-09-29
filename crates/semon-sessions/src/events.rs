//! The per-file event index behind the session model.
//!
//! Every event is metadata and byte offsets only: its kind, line offset,
//! block, time and the ids that join it to other files. It never holds
//! message text, briefs, tool input or output, or answers (risk:secret).
//! The model reads text back from the source line by offset and keeps it in
//! memory only.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Tokens, field};

#[cfg(test)]
thread_local! {
    /// Reads of the persisted event cache on this thread.
    pub(crate) static CACHE_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The event cache sits beside V1's metadata cache and has its own version,
/// so `semon sessions`, `--watch` and `--json` never load or rewrite it.
/// v3: every assistant text is its own event (v2 collapsed adjacent ones).
/// v4: thinking blocks and Codex harness text are indexed apart, as `extras`.
/// v5: completed Codex items carry their exact code-mode call parent, if known.
/// v6: code-mode calls stop accepting item attribution at a turn boundary.
/// v7: tool ids index all tool events for exact legacy item matching.
/// v8: an item is an operation only when exactly one code-mode call owns it,
/// and a script error is read from the harness header alone.
/// v9: token usage is retained by model and Codex rate limits are indexed.
/// v10: assistant billing splits, timestamps and Codex token-count deltas.
/// v11: a Codex command that outlived its yield links its polls and its
/// completion to the script that started it.
/// v12: thinking extras and the thinking filters follow the viewer rewrite (#38).
/// v13: a plain Codex call links its `CommandExecution` item, which holds the
/// whole collected output (#52).
/// v14: compaction and interrupt signals are indexed apart, as `signals` (PR 1 of the dropped-signals plan).
const CACHE_VERSION: u32 = 14;

/// The four token categories the model serves for an exact model id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelTokens {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_write: u64,
    pub(crate) cache_read: u64,
}

impl ModelTokens {
    pub(crate) fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }
}

/// A Claude assistant message's token report. `tokens` preserves the legacy
/// triple's input/cache-read accounting; `model_tokens` keeps cache writes
/// separate for the model JSON.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct MessageUsage {
    pub(crate) model: Option<String>,
    pub(crate) tokens: Tokens,
    pub(crate) model_tokens: ModelTokens,
    pub(crate) billing: BillingUsage,
}

/// Per-message facts used only for price calculation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct BillingUsage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write_5m: u64,
    pub(crate) cache_write_1h: u64,
    pub(crate) web_search_requests: u64,
    pub(crate) speed: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) prompt_size: u64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) split_unknown: bool,
}

/// One Codex cumulative `token_count` event converted to the delta since the
/// preceding event in its file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CodexUsageEvent {
    pub(crate) model: String,
    pub(crate) tokens: ModelTokens,
    pub(crate) timestamp: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RateLimitWindow {
    pub(crate) minutes: u64,
    pub(crate) used_percent: f64,
    pub(crate) resets_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RateLimits {
    pub(crate) recorded_at: i64,
    pub(crate) windows: Vec<RateLimitWindow>,
}

/// One file's event index and the facts the model needs about it. Metadata
/// only (risk:secret).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FileIndex {
    #[serde(default)]
    pub(crate) events: Vec<Event>,
    /// Transcript-only items the model's rules never read: thinking blocks,
    /// harness text added before a Codex prompt, and completed Codex operations.
    /// Kept apart from `events` so they can't change how events collapse or
    /// turns split.
    #[serde(default)]
    pub(crate) extras: Vec<Event>,
    /// Harness signals indexed for analysis, never shown in the transcript.
    #[serde(default)]
    pub(crate) signals: Vec<Signal>,
    /// Tool calls still waiting for a result: call id -> event index.
    #[serde(default)]
    pub(crate) pending: BTreeMap<String, usize>,
    /// Tool call id -> all event indices with that exact id.
    #[serde(default)]
    tool_ids: BTreeMap<String, Vec<usize>>,
    /// Codex commands still running after their yield: the exec session id
    /// the yielded output named -> the event index of the script that
    /// started it. At most [`YIELDS_MAX`].
    #[serde(default)]
    pub(crate) yields: BTreeMap<String, usize>,
    /// Line timestamps clustered into busy intervals (epoch ms).
    #[serde(default)]
    pub(crate) busy: Vec<(i64, i64)>,
    #[serde(default)]
    pub(crate) links: Links,
    /// Claude `entrypoint` or Codex `originator`.
    #[serde(default)]
    pub(crate) entrypoint: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) agent_name: Option<String>,
    #[serde(default)]
    pub(crate) last_model: Option<String>,
    /// Codex: the last `task_complete` carried an error.
    #[serde(default)]
    pub(crate) failed: bool,
    #[serde(default)]
    pub(crate) first: Option<i64>,
    #[serde(default)]
    pub(crate) last: Option<i64>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) branch: Option<String>,
    #[serde(default)]
    usage_by_id: BTreeMap<String, MessageUsage>,
    #[serde(default)]
    codex_tokens: Tokens,
    #[serde(default)]
    codex_tokens_by_model: BTreeMap<String, ModelTokens>,
    #[serde(default)]
    codex_usage_events: Vec<CodexUsageEvent>,
    #[serde(default)]
    pub(crate) rate_limits: Option<RateLimits>,
}

/// A harness signal that isn't a transcript event: indexed for analysis only,
/// never shown in the transcript. Content-free like the rest of the cache:
/// `n` holds only a short enum-like tag, never log text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Signal {
    pub(crate) k: SignalKind,
    /// Byte offset of the line.
    pub(crate) o: u64,
    /// Timestamp (same unit and source as `Event::t`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) t: Option<i64>,
    /// `events.len()` when the signal was pushed: its place among the file's events.
    pub(crate) at: u32,
    /// A short tag: the compaction trigger, the interrupt kind or reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) n: Option<String>,
    /// A number the harness reported with it (tokens before a compaction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) v: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SignalKind {
    Compact,
    Interrupt,
}

impl FileIndex {
    /// Claude usage by message id, for merging across a lineage's files.
    pub(crate) fn usage(&self) -> &BTreeMap<String, MessageUsage> {
        &self.usage_by_id
    }

    pub(crate) fn codex_usage(&self) -> &BTreeMap<String, ModelTokens> {
        &self.codex_tokens_by_model
    }

    pub(crate) fn codex_usage_events(&self) -> &[CodexUsageEvent] {
        &self.codex_usage_events
    }

    pub(crate) fn tokens(&self, harness: &str) -> Tokens {
        if harness == "codex" {
            return self.codex_tokens.clone();
        }
        let mut total = Tokens::default();
        for usage in self.usage_by_id.values() {
            total.input += usage.tokens.input;
            total.cached_input += usage.tokens.cached_input;
            total.output += usage.tokens.output;
            total.total += usage.tokens.total;
        }
        total
    }
}

#[derive(Serialize, Deserialize)]
struct CachedFile {
    dev: u64,
    ino: u64,
    offset: u64,
    size: u64,
    modified_ns: u128,
    #[serde(with = "shared")]
    index: Arc<FileIndex>,
}

/// Serializes an `Arc` as its value: unchanged files are shared between the
/// cache and the model instead of copied on every build.
mod shared {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<T: Serialize, S: Serializer>(
        value: &Arc<T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().serialize(serializer)
    }

    pub(super) fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<T>, D::Error> {
        T::deserialize(deserializer).map(Arc::new)
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct EventCache {
    version: u32,
    files: BTreeMap<String, CachedFile>,
    #[serde(default)]
    reported_runs: BTreeMap<String, BTreeMap<i64, crate::facts::ReportedRunSnapshot>>,
    #[serde(default)]
    claude_json_stamp: Option<ReportedFileStamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReportedFileStamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified_ns: u128,
}

/// Only the named fields deserialize. Other account and project values are
/// consumed as ignored JSON and never enter the session model or facts.
#[derive(Deserialize)]
struct ClaudeJson {
    #[serde(default)]
    projects: BTreeMap<String, LastProjectRun>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastProjectRun {
    last_session_id: Option<String>,
    last_start_time: Option<i64>,
    last_cost: Option<f64>,
    last_duration: Option<u64>,
    last_api_duration: Option<u64>,
    last_tool_duration: Option<u64>,
    last_lines_added: Option<u64>,
    last_lines_removed: Option<u64>,
    last_model_usage: Option<BTreeMap<String, crate::facts::ReportedModelUsage>>,
}

impl EventCache {
    pub(crate) fn path(v1_cache: &Path) -> PathBuf {
        v1_cache.with_extension("events.json")
    }

    pub(crate) fn read(path: &Path) -> Self {
        #[cfg(test)]
        CACHE_READS.with(|reads| reads.set(reads.get() + 1));
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|cache| cache.version == CACHE_VERSION)
            .unwrap_or_else(|| Self {
                version: CACHE_VERSION,
                files: BTreeMap::new(),
                reported_runs: BTreeMap::new(),
                claude_json_stamp: None,
            })
    }

    pub(crate) fn save(&self, path: &Path) -> io::Result<()> {
        crate::save_json(path, self)
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub(crate) fn reported_runs(&self) -> impl Iterator<Item = &crate::facts::ReportedRunSnapshot> {
        self.reported_runs.values().flat_map(|runs| runs.values())
    }

    /// Reads only selected fields from the sibling state file when its
    /// identity, size or modified time changes. A missing/unreadable file is
    /// an empty input and leaves prior snapshots intact.
    pub(crate) fn refresh_reported_runs(&mut self, path: &Path, capture_at: i64, dirty: &mut bool) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos());
        #[cfg(unix)]
        let (dev, ino) = (metadata.dev(), metadata.ino());
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);
        let stamp = ReportedFileStamp {
            dev,
            ino,
            size: metadata.len(),
            modified_ns,
        };
        if self.claude_json_stamp.as_ref() == Some(&stamp) {
            return;
        }
        let Ok(file) = fs::File::open(path) else {
            return;
        };
        let Ok(root) = serde_json::from_reader::<_, ClaudeJson>(file) else {
            return;
        };
        self.claude_json_stamp = Some(stamp);
        *dirty = true;
        for project in root.projects.into_values() {
            let (Some(session_id), Some(start)) =
                (project.last_session_id, project.last_start_time)
            else {
                continue;
            };
            let snapshot = crate::facts::ReportedRunSnapshot {
                last_session_id: session_id.clone(),
                last_start_time: start,
                last_cost: project.last_cost,
                last_duration: project.last_duration,
                last_api_duration: project.last_api_duration,
                last_tool_duration: project.last_tool_duration,
                last_lines_added: project.last_lines_added,
                last_lines_removed: project.last_lines_removed,
                last_model_usage: project.last_model_usage.unwrap_or_default(),
                capture_at,
            };
            self.reported_runs
                .entry(session_id)
                .or_default()
                .insert(start, snapshot);
        }
    }

    /// Drops files that are gone, so the cache doesn't grow without bound.
    pub(crate) fn retain(&mut self, seen: &BTreeSet<String>, dirty: &mut bool) {
        let before = self.files.len();
        self.files.retain(|path, _| seen.contains(path));
        *dirty |= self.files.len() != before;
    }
}

#[cfg(test)]
thread_local! {
    /// Lines parsed by [`scan_file`] on this thread: a warm build parses none.
    pub(crate) static PARSED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Indexes one file, resuming from the cached byte offset when the file only
/// grew, and sharing the cached index when it didn't change at all.
pub(crate) fn scan_file(
    path: &Path,
    harness: &str,
    cache: &mut EventCache,
    dirty: &mut bool,
) -> io::Result<Arc<FileIndex>> {
    let metadata = fs::metadata(path)?;
    let modified_ns = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    #[cfg(unix)]
    let (dev, ino) = (metadata.dev(), metadata.ino());
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    let key = path.to_string_lossy().into_owned();
    let mut index = FileIndex::default();
    let mut offset = 0;
    if let Some(entry) = cache.files.get(&key) {
        if entry.dev == dev
            && entry.ino == ino
            && entry.size == metadata.len()
            && entry.modified_ns == modified_ns
        {
            return Ok(Arc::clone(&entry.index));
        }
        if entry.dev == dev
            && entry.ino == ino
            && entry.offset <= metadata.len()
            && (metadata.len() > entry.size || entry.modified_ns == modified_ns)
        {
            offset = entry.offset;
        }
    }
    if let Some(entry) = cache.files.remove(&key)
        && offset > 0
    {
        index = Arc::unwrap_or_clone(entry.index);
    }
    *dirty = true;
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let length = reader.read_until(b'\n', &mut line)?;
        if length == 0 || !line.ends_with(b"\n") {
            break;
        }
        let line_offset = offset;
        offset += length as u64;
        #[cfg(test)]
        PARSED.with(|parsed| parsed.set(parsed.get() + 1));
        match serde_json::from_slice::<Value>(&line) {
            Ok(record) if record.is_object() => {
                if harness == "claude" {
                    claude(&mut index, &record, line_offset);
                } else {
                    codex(&mut index, &record, line_offset);
                }
            }
            _ => gap(&mut index, line_offset),
        }
    }
    let index = Arc::new(index);
    cache.files.insert(
        key,
        CachedFile {
            dev,
            ino,
            offset,
            size: metadata.len(),
            modified_ns,
            index: Arc::clone(&index),
        },
    );
    Ok(index)
}

/// Busy clustering: consecutive line timestamps no more than this far apart
/// form one interval.
pub(crate) const BUSY_GAP_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// Your message: a human-origin user record or queued command.
    Ask,
    /// A user prompt that isn't yours: a brief, an sdk-cli prompt, or a
    /// Codex user message.
    #[default]
    U,
    /// Assistant text: one event per text block, so back-to-back texts all
    /// stay (where the last reply matters, the model takes it explicitly).
    A,
    /// A tool call, resolved in place when its result arrives.
    Tool,
    /// A Codex command or file change completed by an `item_completed` event.
    Operation,
    /// A relay received (`origin.kind == "peer"`, or a
    /// `<cross-session-message>` block).
    Xsm,
    /// A subagent handing back (`origin.handback`, an `<agent-message>`
    /// block, or a Codex `agent_message`).
    Agm,
    /// A `<task-notification>` block.
    Tn,
    /// Lines the log lost: a run of malformed lines.
    Gap,
    /// A thinking block or Codex reasoning item (in `extras` only).
    Think,
    /// Harness text added before a Codex prompt, `n` its tag (in `extras`
    /// only).
    Harness,
}

/// Result flags on a tool call.
pub(crate) const ASYNC: u8 = 1;
pub(crate) const DENIED: u8 = 2;
pub(crate) const PIN: u8 = 4;
pub(crate) const ANSWERED: u8 = 8;
pub(crate) const SEND_FAILED: u8 = 16;
/// A Codex call's output is only an acknowledgement (`{"accepted": …}`), not
/// an answer.
pub(crate) const ACK: u8 = 32;
/// A Codex call finished without a structured exit code: its outcome is
/// unknown, neither success nor failure.
pub(crate) const UNKNOWN: u8 = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reply {
    /// Offset of the line holding the result, and its block.
    pub(crate) o: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) b: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) t: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) e: bool,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub(crate) f: u8,
    /// A SendMessage result's `msg_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) m: Option<String>,
}

/// A code-mode script calls `tools.write_stdin`...
pub(crate) const STDIN: u8 = 1;
/// ...with `chars` that aren't an empty string: it sends input.
pub(crate) const SENDS: u8 = 2;

/// Open yielded commands kept per file; past this the oldest is dropped.
const YIELDS_MAX: usize = 64;
/// Poll outputs kept per yielded command; later polls still fold in.
pub(crate) const POLLS_MAX: usize = 256;

/// A Codex command that outlived its `yield_time_ms`: the script that
/// started it answered with an exec session id and no exit code. Offsets
/// only, like the rest of the index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Yield {
    /// The output lines of the polls that folded into it, in order: at most
    /// [`POLLS_MAX`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) polls: Vec<u64>,
    /// More polls folded in than were kept.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) cut: bool,
    /// The `CommandExecution` item whose `process_id` is the session id:
    /// the process's end, its line and outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) done: Option<Reply>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Peer {
    /// `origin.from`: the sender's socket address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sock: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Event {
    pub(crate) k: Kind,
    /// Byte offset of the source line.
    pub(crate) o: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) b: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) t: Option<i64>,
    /// tool: the tool_use / call id. xsm: `msg_id`. agm: the agent id.
    /// tn: the tool-use id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    /// tool: the tool name. xsm: the sender's `origin.name`. tn: the status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) n: Option<String>,
    /// A Codex `custom_tool_call` named `exec`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) code_mode: bool,
    /// Still open for operation attribution within the current turn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) code_mode_open: bool,
    /// Operation: the exact index of its parent code-mode call, the only one
    /// open when the item completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<usize>,
    /// Code-mode call: what its source calls, as [`STDIN`] and [`SENDS`].
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub(crate) script: u8,
    /// Code-mode call: a command it started outlived its yield.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) y: Option<Box<Yield>>,
    /// Code-mode call: a poll of the yielded command the call at this event
    /// index started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) poll: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) r: Option<Reply>,
    /// A plain Codex call: the line of the `CommandExecution` item with its
    /// id, whose `aggregated_output` is the command's whole output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) item: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer: Option<Peer>,
    /// The Claude record's `uuid`: an id, used to find the lines a
    /// copy-resume duplicated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) u: Option<String>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The exact links between transcript files of one lineage (M0).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Links {
    /// `session_id` values naming another file: the lineage's first file.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) session_ids: BTreeSet<String>,
    /// `continued-in` records: the file this one was copied into.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) continued_in: BTreeSet<String>,
    /// `bridge-session` records: the Remote Control session (D5).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) bridges: BTreeSet<String>,
}

/// Parses an RFC 3339 timestamp to epoch milliseconds.
pub(crate) fn parse_ms(value: &str) -> Option<i64> {
    let seconds = i64::try_from(crate::parse_rfc3339(value)?).ok()?;
    let fraction = value.get(19..).and_then(|tail| tail.strip_prefix('.'));
    let millis = fraction
        .map(|digits| {
            let digits: String = digits
                .chars()
                .take_while(char::is_ascii_digit)
                .chain("000".chars())
                .take(3)
                .collect();
            digits.parse::<i64>().unwrap_or(0)
        })
        .unwrap_or(0);
    Some(seconds * 1000 + millis)
}

/// Adds one timestamp to a sorted interval list whose intervals are more than
/// [`BUSY_GAP_MS`] apart. Clustering per file and merging later is the same
/// as clustering every stamp of a lineage at once.
pub(crate) fn busy_add(busy: &mut Vec<(i64, i64)>, time: i64) {
    if let Some(last) = busy.last_mut()
        && time >= last.0
    {
        if time - last.1 <= BUSY_GAP_MS {
            last.1 = last.1.max(time);
            return;
        }
        busy.push((time, time));
        return;
    }
    busy.push((time, time));
    busy.sort_unstable();
    let merged = busy_merge(std::mem::take(busy));
    *busy = merged;
}

/// Merges sorted intervals that are no more than [`BUSY_GAP_MS`] apart.
pub(crate) fn busy_merge(mut intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    intervals.sort_unstable();
    let mut result: Vec<(i64, i64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = result.last_mut()
            && start - last.1 <= BUSY_GAP_MS
        {
            last.1 = last.1.max(end);
        } else {
            result.push((start, end));
        }
    }
    result
}

fn push(summary: &mut FileIndex, event: Event) {
    if event.k == Kind::Gap
        && summary
            .events
            .last()
            .is_some_and(|last| last.k == Kind::Gap)
    {
        return;
    }
    summary.events.push(event);
}

/// Compaction dedupe has two limits: an interrupt between lines of one
/// compaction counts it twice, while two compactions with no event between
/// them (such as `/compact` twice in a row) fold into one.
fn signal(
    summary: &mut FileIndex,
    k: SignalKind,
    o: u64,
    t: Option<i64>,
    n: Option<&str>,
    v: Option<u64>,
) {
    let n = n
        .filter(|tag| {
            tag.len() <= 32
                && tag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b' '))
        })
        .map(str::to_owned);
    let at = u32::try_from(summary.events.len()).unwrap_or(u32::MAX);
    if k == SignalKind::Compact
        && let Some(last) = summary.signals.last_mut()
        && last.k == SignalKind::Compact
        && last.at == at
    {
        if last.n.is_none() {
            last.n = n;
        }
        if last.v.is_none() {
            last.v = v;
        }
        if last.t.is_none() {
            last.t = t;
        }
        return;
    }
    summary.signals.push(Signal { k, o, t, at, n, v });
}

/// Adds a transcript-only marker. The viewer groups consecutive thinking
/// entries, retaining each time so a masked run can show its measured length.
fn extra(summary: &mut FileIndex, event: Event) {
    summary.extras.push(event);
}

pub(crate) fn gap(summary: &mut FileIndex, offset: u64) {
    push(
        summary,
        Event {
            k: Kind::Gap,
            o: offset,
            ..Event::default()
        },
    );
}

pub(crate) fn record_time(record: &Value) -> Option<i64> {
    field(record, "timestamp")
        .or_else(|| {
            record
                .get("payload")
                .and_then(|payload| field(payload, "timestamp"))
        })
        .and_then(parse_ms)
}

fn absolute_reset_ms(value: &Value) -> Option<i64> {
    if let Some(seconds) = value.as_i64() {
        return if seconds.unsigned_abs() >= 100_000_000_000 {
            Some(seconds)
        } else {
            seconds.checked_mul(1000)
        };
    }
    if let Some(seconds) = value.as_u64() {
        return if seconds >= 100_000_000_000 {
            i64::try_from(seconds).ok()
        } else {
            i64::try_from(seconds).ok()?.checked_mul(1000)
        };
    }
    if let Some(seconds) = value.as_f64() {
        if !seconds.is_finite() {
            return None;
        }
        let millis = if seconds.abs() >= 100_000_000_000.0 {
            seconds
        } else {
            seconds * 1000.0
        };
        return (millis >= i64::MIN as f64 && millis <= i64::MAX as f64)
            .then(|| millis.round() as i64);
    }
    value.as_str().and_then(parse_ms)
}

fn rate_limits(info: &Value, recorded_at: i64) -> Option<RateLimits> {
    let data = info.get("rate_limits")?;
    let mut by_minutes = BTreeMap::new();
    for name in ["primary", "secondary"] {
        let window = data.get(name)?;
        let minutes = window.get("window_minutes")?.as_u64()?;
        let used_percent = window.get("used_percent")?.as_f64()?;
        let resets_at = if let Some(seconds) = window.get("resets_in_seconds") {
            let seconds = seconds.as_f64()?;
            if !seconds.is_finite() {
                continue;
            }
            let millis = seconds * 1000.0;
            if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
                continue;
            }
            recorded_at.checked_add(millis.round() as i64)?
        } else {
            absolute_reset_ms(window.get("resets_at")?)?
        };
        by_minutes.insert(
            minutes,
            RateLimitWindow {
                minutes,
                used_percent,
                resets_at,
            },
        );
    }
    (!by_minutes.is_empty()).then_some(RateLimits {
        recorded_at,
        windows: by_minutes.into_values().collect(),
    })
}

fn strip_reminders(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<system-reminder>") {
        output.push_str(&rest[..start]);
        match rest[start..].find("</system-reminder>") {
            Some(end) => rest = &rest[start + end + "</system-reminder>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    output.push_str(rest);
    output.trim().to_owned()
}

pub(crate) fn clean_prompt(text: &str) -> String {
    strip_reminders(text)
}

/// `/compact` as a whole command, typed or as a command tag.
fn is_compact_command(text: &str) -> bool {
    text.strip_prefix("/compact")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        || text.contains("<command-name>/compact</command-name>")
}

/// `name="value"` attributes of an opening tag, as the harness writes them.
pub(crate) fn attributes(tag: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut rest = tag;
    while let Some(equals) = rest.find("=\"") {
        // Byte offsets from `char_indices`, so a multi-byte character in the
        // tag can't split a slice.
        let start = rest[..equals]
            .char_indices()
            .rev()
            .find(|(_, character)| !(character.is_ascii_alphanumeric() || *character == '-'))
            .map_or(0, |(at, character)| at + character.len_utf8());
        let name = rest[start..equals].to_owned();
        let value_start = equals + 2;
        let Some(length) = rest[value_start..].find('"') else {
            break;
        };
        if !name.is_empty() {
            result.insert(name, rest[value_start..value_start + length].to_owned());
        }
        rest = &rest[value_start + length + 1..];
    }
    result
}

/// The opening-tag attribute strings of every `<name ...>` block in `text`.
pub(crate) fn tags<'a>(text: &'a str, name: &str) -> Vec<(&'a str, usize)> {
    let open = format!("<{name}");
    let mut result = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(&open) {
        let start = from + found + open.len();
        let next = text[start..].chars().next();
        if !matches!(next, Some(' ' | '>' | '\n' | '\t')) {
            from = start;
            continue;
        }
        let Some(close) = text[start..].find('>') else {
            break;
        };
        result.push((&text[start..start + close], start + close + 1));
        from = start + close + 1;
    }
    result
}

/// The text inside `<name>…</name>`, first occurrence.
pub(crate) fn inner<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

/// Task-notification blocks: `(tool-use id, status, body)`.
pub(crate) fn notifications(text: &str) -> Vec<(Option<&str>, Option<&str>, &str)> {
    let mut result = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<task-notification>") {
        let body_start = start + "<task-notification>".len();
        let Some(length) = rest[body_start..].find("</task-notification>") else {
            break;
        };
        let body = &rest[body_start..body_start + length];
        result.push((inner(body, "tool-use-id"), inner(body, "status"), body));
        rest = &rest[body_start + length..];
    }
    result
}

fn peer_event(origin: &Value, offset: u64, time: Option<i64>) -> Event {
    if origin.get("handback").and_then(Value::as_bool) == Some(true) {
        // `senderTaskId` first; `from` is kept too when it differs, and the
        // model matches either against the agent id or the spawning call.
        let task = field(origin, "senderTaskId");
        let from = field(origin, "from");
        return Event {
            k: Kind::Agm,
            o: offset,
            t: time,
            id: task.or(from).map(str::to_owned),
            n: from
                .filter(|from| task.is_some_and(|task| task != *from))
                .map(str::to_owned),
            ..Event::default()
        };
    }
    Event {
        k: Kind::Xsm,
        o: offset,
        t: time,
        id: field(origin, "msg_id").map(str::to_owned),
        n: field(origin, "name").map(str::to_owned),
        peer: Some(Peer {
            sock: field(origin, "from").map(str::to_owned),
            pid: origin.get("verifiedPeerPid").and_then(Value::as_u64),
            start: origin
                .get("verifiedPeerProcStart")
                .and_then(|value| match value {
                    Value::String(text) => Some(text.clone()),
                    Value::Number(number) => Some(number.to_string()),
                    _ => None,
                }),
        }),
        ..Event::default()
    }
}

/// Classifies one user-side prompt (a user record's text or a queued
/// command's prompt) into events.
fn prompt_events(
    summary: &mut FileIndex,
    text: &str,
    origin: Option<&Value>,
    meta: bool,
    offset: u64,
    time: Option<i64>,
) {
    match origin.and_then(|origin| field(origin, "kind")) {
        Some("peer") => {
            push(
                summary,
                peer_event(origin.expect("peer origin"), offset, time),
            );
            return;
        }
        Some("human") => {
            let text = strip_reminders(text);
            if !meta && !text.is_empty() && !is_compact_command(&text) {
                push(
                    summary,
                    Event {
                        k: Kind::Ask,
                        o: offset,
                        t: time,
                        ..Event::default()
                    },
                );
            }
            return;
        }
        _ => {}
    }
    let mut found = false;
    for (tag, _) in tags(text, "cross-session-message") {
        let attributes = attributes(tag);
        push(
            summary,
            Event {
                k: Kind::Xsm,
                o: offset,
                t: time,
                n: attributes.get("from-name").cloned(),
                peer: Some(Peer {
                    sock: attributes.get("from").cloned(),
                    ..Peer::default()
                }),
                ..Event::default()
            },
        );
        found = true;
    }
    for (tag, _) in tags(text, "agent-message") {
        push(
            summary,
            Event {
                k: Kind::Agm,
                o: offset,
                t: time,
                id: attributes(tag).get("from").cloned(),
                ..Event::default()
            },
        );
        found = true;
    }
    for (id, status, _) in notifications(text) {
        push(
            summary,
            Event {
                k: Kind::Tn,
                o: offset,
                t: time,
                id: id.map(str::to_owned),
                n: status.map(str::to_owned),
                ..Event::default()
            },
        );
        found = true;
    }
    if found || meta {
        return;
    }
    let text = strip_reminders(text);
    if text.starts_with("[Request interrupted") {
        let kind = if text.starts_with("[Request interrupted by user for tool use") {
            "tool"
        } else {
            "user"
        };
        signal(
            summary,
            SignalKind::Interrupt,
            offset,
            time,
            Some(kind),
            None,
        );
        return;
    }
    if text.is_empty() || text.starts_with('<') || text.starts_with('/') {
        return;
    }
    push(
        summary,
        Event {
            k: Kind::U,
            o: offset,
            t: time,
            ..Event::default()
        },
    );
}

fn text_parts(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| field(part, "type").is_none_or(|kind| kind == "text"))
            .filter_map(|part| field(part, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn result_flags(name: Option<&str>, result: Option<&Value>) -> (u8, Option<String>) {
    let mut flags = 0;
    let mut msg_id = None;
    match name {
        Some("Agent" | "Task") => {
            if result.and_then(|value| field(value, "status")) == Some("async_launched") {
                flags |= ASYNC;
            }
        }
        Some("SendMessage") => match result {
            Some(Value::String(_)) => flags |= DENIED,
            Some(value @ Value::Object(_)) => {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    flags |= SEND_FAILED;
                }
                if value.get("pin").is_some() || value.get("resumedAgentId").is_some() {
                    flags |= PIN;
                }
                msg_id = field(value, "msg_id").map(str::to_owned);
            }
            _ => {}
        },
        Some("AskUserQuestion") => match result {
            Some(Value::String(_)) => flags |= DENIED,
            Some(value) if value.get("answers").is_some_and(Value::is_object) => {
                flags |= ANSWERED;
            }
            _ => {}
        },
        _ => {}
    }
    (flags, msg_id)
}

fn resolve(summary: &mut FileIndex, id: &str, reply: impl FnOnce(Option<&str>) -> Reply) {
    let Some(index) = summary.pending.remove(id) else {
        return;
    };
    if let Some(event) = summary.events.get_mut(index)
        && event.k == Kind::Tool
    {
        event.r = Some(reply(event.n.as_deref()));
        event.code_mode_open = false;
    }
}

/// Stops unresolved code-mode calls from claiming items in a later turn.
fn close_code_mode(summary: &mut FileIndex) {
    for index in summary.pending.values().copied().collect::<Vec<_>>() {
        if let Some(event) = summary.events.get_mut(index)
            && event.k == Kind::Tool
            && event.code_mode
        {
            event.code_mode_open = false;
        }
    }
}

fn non_code_mode_tool(summary: &FileIndex, id: &str) -> bool {
    summary.tool_ids.get(id).is_some_and(|indices| {
        indices.iter().any(|index| {
            summary
                .events
                .get(*index)
                .is_some_and(|event| event.k == Kind::Tool && !event.code_mode)
        })
    })
}

/// The one code-mode call open for item attribution, if exactly one is.
fn open_code_mode_call(summary: &FileIndex) -> Option<usize> {
    let mut open = summary.pending.values().copied().filter(|index| {
        summary
            .events
            .get(*index)
            .is_some_and(|event| event.k == Kind::Tool && event.code_mode && event.code_mode_open)
    });
    let first = open.next()?;
    open.next().is_none().then_some(first)
}

/// A code-mode script failed: the harness's header, the output's first
/// element, says so. Text in later elements is the script's own output and is
/// never read as a failure.
fn script_error(output: Option<&Value>) -> bool {
    output
        .and_then(Value::as_array)
        .and_then(|elements| elements.first())
        .and_then(|header| field(header, "text"))
        .is_some_and(|text| text.trim_start().starts_with("Script error"))
}

/// What a code-mode script gives one key of a `tools` call's object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScriptArg {
    /// The object has no such key.
    Absent,
    /// A plain string literal, unescaped.
    Literal(String),
    /// Anything else: a variable, an expression, a template with `${…}`, or
    /// a call whose argument isn't an object literal this reader can follow.
    Other,
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The JavaScript string literal opening at `start` (a quote): its text,
/// unescaped, or `None` for a template with `${…}`; and where it ends. An
/// unterminated literal ends at the end of `text`.
fn read_literal(text: &str, start: usize) -> (Option<String>, usize) {
    let mut chars = text[start..].char_indices();
    let Some((_, quote)) = chars.next() else {
        return (None, text.len());
    };
    let mut value = String::new();
    let mut plain = true;
    // Hex digits only: `from_str_radix` would also take a leading `+`.
    let hex = |digits: &str| {
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok()
    };
    while let Some((at, c)) = chars.next() {
        match c {
            _ if c == quote => return (plain.then_some(value), start + at + c.len_utf8()),
            '$' if quote == '`' && text[start + at..].starts_with("${") => plain = false,
            '\\' => {
                let Some((_, escaped)) = chars.next() else {
                    break;
                };
                let rest = chars.as_str();
                let code = match escaped {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'b' => Some('\u{8}'),
                    'f' => Some('\u{c}'),
                    'v' => Some('\u{b}'),
                    '0' => Some('\0'),
                    '\n' => None,
                    'x' => {
                        let code = rest.get(..2).and_then(hex).and_then(char::from_u32);
                        if code.is_some() {
                            chars.nth(1);
                        }
                        code.or(Some('x'))
                    }
                    'u' if rest.starts_with('{') => {
                        // `{`, at most six digits and `}`: never a scan
                        // of the rest of the script.
                        let close = rest
                            .char_indices()
                            .take(8)
                            .find(|(_, c)| *c == '}')
                            .map(|(close, _)| close);
                        let code = close
                            .and_then(|close| hex(&rest[1..close]))
                            .map(|code| char::from_u32(code).unwrap_or('\u{fffd}'));
                        if let (Some(close), Some(_)) = (close, code) {
                            chars.nth(close);
                        }
                        code.or(Some('u'))
                    }
                    'u' => {
                        let code = rest
                            .get(..4)
                            .and_then(hex)
                            .map(|code| char::from_u32(code).unwrap_or('\u{fffd}'));
                        if code.is_some() {
                            chars.nth(3);
                        }
                        code.or(Some('u'))
                    }
                    other => Some(other),
                };
                value.extend(code);
            }
            _ => value.push(c),
        }
    }
    (None, text.len())
}

/// The value at `start`, after a key's colon.
fn script_value(text: &str, start: usize) -> ScriptArg {
    let rest = text[start..].trim_start();
    if !rest.starts_with(['"', '\'', '`']) {
        return ScriptArg::Other;
    }
    match read_literal(text, text.len() - rest.len()).0 {
        Some(value) => ScriptArg::Literal(value),
        None => ScriptArg::Other,
    }
}

/// Where the source's first call to `tool` opens its arguments: `tool` as a
/// whole identifier followed by `(`, outside comments and string literals.
fn call_open(script: &str, tool: &str) -> Option<usize> {
    if tool.is_empty() {
        return None;
    }
    let mut at = 0;
    while let Some(c) = script[at..].chars().next() {
        match c {
            '"' | '\'' | '`' => {
                at = read_literal(script, at).1;
                continue;
            }
            '/' if script[at..].starts_with("//") => {
                at = script[at..].find('\n').map_or(script.len(), |end| at + end);
                continue;
            }
            '/' if script[at..].starts_with("/*") => {
                at = script[at + 2..]
                    .find("*/")
                    .map_or(script.len(), |end| at + end + 4);
                continue;
            }
            _ if ident_char(c) => {
                let end = script[at..]
                    .find(|c: char| !ident_char(c))
                    .map_or(script.len(), |end| at + end);
                let word = &script[at..end];
                at = end;
                let rest = script[at..].trim_start();
                if word == tool && rest.starts_with('(') {
                    return Some(script.len() - rest.len() + 1);
                }
                continue;
            }
            _ => {}
        }
        at += c.len_utf8();
    }
    None
}

/// What a code-mode script gives `key` in its first call to `tool`
/// (`tools.<tool>({key: …})`); `None` when the source never calls it. Read
/// only for what the structured output doesn't carry: a yielded command's
/// line before it completes, and the input a poll sends.
pub(crate) fn script_arg(script: &str, tool: &str, key: &str) -> Option<ScriptArg> {
    let open = call_open(script, tool)?;
    let text = &script[open..];
    if !text.trim_start().starts_with('{') {
        return Some(ScriptArg::Other);
    }
    // Depth 0 is the argument list, 1 the object literal.
    let mut depth = 0usize;
    let mut expect_key = false;
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        match c {
            '"' | '\'' | '`' => {
                let (literal, end) = read_literal(text, at);
                at = end;
                let rest = text[at..].trim_start();
                if depth == 1 && expect_key && rest.starts_with(':') {
                    if literal.as_deref() == Some(key) {
                        return Some(script_value(text, text.len() - rest.len() + 1));
                    }
                    expect_key = false;
                }
                continue;
            }
            '/' if text[at..].starts_with("//") => {
                at = text[at..].find('\n').map_or(text.len(), |end| at + end);
                continue;
            }
            '/' if text[at..].starts_with("/*") => {
                at = text[at + 2..]
                    .find("*/")
                    .map_or(text.len(), |end| at + end + 4);
                continue;
            }
            '(' | '[' | '{' => {
                depth += 1;
                expect_key = depth == 1 && c == '{';
            }
            ')' | ']' | '}' => {
                if depth == 0 {
                    return Some(ScriptArg::Absent);
                }
                depth -= 1;
                expect_key = false;
            }
            ',' if depth == 1 => expect_key = true,
            _ if depth == 1 && expect_key && ident_char(c) => {
                let end = text[at..]
                    .find(|c: char| !ident_char(c))
                    .map_or(text.len(), |end| at + end);
                let word = &text[at..end];
                at = end;
                let rest = text[at..].trim_start();
                if word == key {
                    return Some(if rest.starts_with(':') {
                        script_value(text, text.len() - rest.len() + 1)
                    } else {
                        // Shorthand `{chars}`: a variable.
                        ScriptArg::Other
                    });
                }
                expect_key = false;
                continue;
            }
            _ if depth == 1 && !c.is_whitespace() => expect_key = false,
            _ => {}
        }
        at += c.len_utf8();
    }
    // The source ends inside the call: it can't be read.
    Some(ScriptArg::Other)
}

/// The command line a code-mode script starts with `tools.exec_command`.
pub(crate) fn script_command(script: &str) -> Option<String> {
    match script_arg(script, "exec_command", "cmd")? {
        ScriptArg::Literal(command) if !command.trim().is_empty() => Some(command),
        _ => None,
    }
}

/// [`STDIN`] and [`SENDS`] for a code-mode script's source. `chars` that
/// can't be read count as input sent, so a poll is never hidden on a guess.
/// A script that starts a command is never a poll, even one that goes on to
/// poll what it started.
fn script_flags(script: &str) -> u8 {
    if script_arg(script, "exec_command", "cmd").is_some() {
        return 0;
    }
    match script_arg(script, "write_stdin", "chars") {
        None => 0,
        Some(ScriptArg::Absent) => STDIN,
        Some(ScriptArg::Literal(chars)) if chars.is_empty() => STDIN,
        Some(_) => STDIN | SENDS,
    }
}

/// A code-mode script's output that is one exec result: the harness header
/// and the script's text, a JSON object with the command's `output` and
/// `wall_time_seconds`, as `text(JSON.stringify(r))` prints it.
pub(crate) fn yield_json(output: Option<&Value>) -> Option<Value> {
    let text = match output? {
        Value::String(text) => text.as_str(),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|part| field(part, "text"))
                .collect();
            match texts.as_slice() {
                [header, body] if header.trim_start().starts_with("Script completed") => *body,
                [body] => *body,
                _ => return None,
            }
        }
        _ => return None,
    };
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    (value.get("output").is_some_and(Value::is_string)
        && value.get("wall_time_seconds").is_some_and(Value::is_number))
    .then_some(value)
}

/// An exec session id, from a yielded output's `session_id` or an item's
/// `process_id`: a number or a short string.
fn exec_session(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(number) => number.as_u64().map(|number| number.to_string()),
        Value::String(text) if !text.is_empty() && text.len() <= 64 => Some(text.clone()),
        _ => None,
    }
}

/// The exec session a code-mode script's output leaves running: the
/// command outlived its yield, so there is a session id and no exit code.
fn yielded_session(output: Option<&Value>) -> Option<String> {
    let value = yield_json(output)?;
    if value.get("exit_code").is_some_and(|code| !code.is_null()) {
        return None;
    }
    exec_session(value.get("session_id"))
}

/// Links a code-mode call whose output just arrived to a yielded command:
/// as the script that started it, or as one of its polls. A script with
/// operations of its own, or one that failed, stays a step of its own, but
/// the chunk it printed of an open command is still that command's output.
fn fold_yield(summary: &mut FileIndex, index: usize, output: Option<&Value>, offset: u64) {
    let Some(event) = summary.events.get(index) else {
        return;
    };
    let (call_offset, flags, linked) = (event.o, event.script, event.poll);
    let failed = event.r.as_ref().is_some_and(|reply| reply.e);
    let operations = summary
        .extras
        .iter()
        .rev()
        .take_while(|extra| extra.o > call_offset)
        .any(|extra| extra.k == Kind::Operation && extra.parent == Some(index));
    let session = yielded_session(output);
    // A poll of an open command, by the session id its output names.
    let known = session
        .as_ref()
        .filter(|_| flags & STDIN != 0)
        .and_then(|session| summary.yields.get(session).copied())
        .filter(|start| *start < index);
    if operations || failed {
        summary.events[index].poll = None;
        if let Some(start) = known {
            push_poll(summary, start, offset);
        }
        return;
    }
    // The command's completion arrived while this poll ran: its last poll.
    let ended_here = linked.filter(|start| {
        summary
            .events
            .get(*start)
            .and_then(|found| found.y.as_ref())
            .and_then(|yielded| yielded.done.as_ref())
            .is_some_and(|done| done.o > call_offset)
    });
    let start = match session {
        None => ended_here,
        Some(_) if known.is_some() => known,
        Some(_) if ended_here.is_some() => ended_here,
        // A command these logs haven't seen yet: a new start, whether this
        // script started it or polls one started where these logs can't
        // see.
        Some(session) => {
            if summary.yields.len() >= YIELDS_MAX
                && !summary.yields.contains_key(&session)
                && let Some(oldest) = summary
                    .yields
                    .iter()
                    .min_by_key(|(_, start)| **start)
                    .map(|(session, _)| session.clone())
            {
                summary.yields.remove(&oldest);
            }
            summary.yields.insert(session, index);
            summary.events[index].poll = None;
            summary.events[index].y = Some(Box::default());
            return;
        }
    };
    let Some(start) = start.filter(|start| *start < index) else {
        summary.events[index].poll = None;
        return;
    };
    summary.events[index].poll = Some(start);
    push_poll(summary, start, offset);
}

/// Adds a poll's output line to the yielded command it polled, up to
/// [`POLLS_MAX`]; past that the command's output is marked cut.
fn push_poll(summary: &mut FileIndex, start: usize, offset: u64) {
    if let Some(found) = summary.events.get_mut(start)
        && let Some(yielded) = found.y.as_mut()
    {
        if yielded.polls.len() < POLLS_MAX {
            yielded.polls.push(offset);
        } else {
            yielded.cut = true;
        }
    }
}

/// The yielded command a poll that hasn't answered yet is taken to poll:
/// the only one still open. Its output settles it.
fn open_poll(summary: &FileIndex, flags: u8) -> Option<usize> {
    if flags & STDIN == 0 || summary.yields.len() != 1 {
        return None;
    }
    summary.yields.values().next().copied()
}

fn exit_code(summary: &mut FileIndex, id: &str, code: i64, offset: u64, time: Option<i64>) {
    if summary.pending.contains_key(id) {
        resolve(summary, id, |_| Reply {
            o: offset,
            t: time,
            e: code != 0,
            ..Reply::default()
        });
        return;
    }
    // A later structured exit code settles an unknown outcome.
    if let Some(event) = summary
        .events
        .iter_mut()
        .rev()
        .take(512)
        .find(|event| event.k == Kind::Tool && event.id.as_deref() == Some(id))
        && let Some(reply) = &mut event.r
    {
        reply.e = code != 0;
        reply.f &= !UNKNOWN;
    }
}

/// Links the plain call `id` to the item at `offset` that completed it.
fn link_item(summary: &mut FileIndex, id: &str, offset: u64) {
    let Some(index) = summary.tool_ids.get(id).and_then(|indices| {
        indices.iter().rev().copied().find(|index| {
            summary
                .events
                .get(*index)
                .is_some_and(|event| event.k == Kind::Tool && !event.code_mode)
        })
    }) else {
        return;
    };
    summary.events[index].item = Some(offset);
}

fn tool_event(summary: &mut FileIndex, event: Event) {
    if let Some(id) = &event.id {
        let index = summary.events.len();
        summary.tool_ids.entry(id.clone()).or_default().push(index);
        summary.pending.insert(id.clone(), index);
    }
    summary.events.push(event);
}

/// Claude Code lines: `offset` is the line's byte offset.
fn activity(summary: &mut FileIndex, time: Option<i64>) {
    if let Some(time) = time {
        busy_add(&mut summary.busy, time);
        // Earliest and latest, not first and last line: lines aren't always
        // written in time order.
        summary.first = Some(summary.first.map_or(time, |first| first.min(time)));
        summary.last = Some(summary.last.map_or(time, |last| last.max(time)));
    }
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn claude(summary: &mut FileIndex, record: &Value, offset: u64) {
    claude_events(summary, record, offset);
    if let Some(uuid) = field(record, "uuid") {
        for event in summary
            .events
            .iter_mut()
            .rev()
            .take_while(|event| event.o == offset)
        {
            event.u = Some(uuid.to_owned());
        }
    }
}

fn claude_events(summary: &mut FileIndex, record: &Value, offset: u64) {
    let time = record_time(record);
    activity(summary, time);
    if let Some(cwd) = field(record, "cwd") {
        summary.cwd = Some(cwd.to_owned());
    }
    if let Some(branch) = field(record, "gitBranch") {
        summary.branch = Some(branch.to_owned());
    }
    // Tokens de-duplicated by message id, as V1 counts them.
    if let Some(message) = record.get("message")
        && let (Some(id), Some(usage)) = (field(message, "id"), message.get("usage"))
    {
        let input = number(usage, "input_tokens");
        let cache_write = number(usage, "cache_creation_input_tokens");
        let cache_read = number(usage, "cache_read_input_tokens");
        let output = number(usage, "output_tokens");
        let split = usage
            .get("cache_creation")
            .and_then(Value::as_object)
            .and_then(|cache_creation| {
                let five = cache_creation
                    .get("ephemeral_5m_input_tokens")
                    .and_then(Value::as_u64)?;
                let one = cache_creation
                    .get("ephemeral_1h_input_tokens")
                    .and_then(Value::as_u64)?;
                (five.saturating_add(one) == cache_write).then_some((five, one))
            });
        let (cache_write_5m, cache_write_1h, split_unknown) = split
            .map(|(five, one)| (five, one, false))
            .unwrap_or((cache_write, 0, true));
        let web_search_requests = usage
            .get("server_tool_use")
            .and_then(|tools| tools.get("web_search_requests"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let timestamp = time;
        summary.usage_by_id.insert(
            id.to_owned(),
            MessageUsage {
                model: field(message, "model").map(str::to_owned),
                tokens: Tokens {
                    input: input + cache_write + cache_read,
                    cached_input: cache_read,
                    output,
                    reasoning_output: 0,
                    total: input + cache_write + cache_read + output,
                },
                model_tokens: ModelTokens {
                    input,
                    output,
                    cache_write,
                    cache_read,
                },
                billing: BillingUsage {
                    input,
                    output,
                    cache_read,
                    cache_write_5m,
                    cache_write_1h,
                    web_search_requests,
                    speed: field(usage, "speed").map(str::to_owned),
                    service_tier: field(usage, "service_tier").map(str::to_owned),
                    prompt_size: input
                        .saturating_add(cache_read)
                        .saturating_add(cache_write_5m)
                        .saturating_add(cache_write_1h),
                    timestamp,
                    split_unknown,
                },
            },
        );
    }
    if let Some(id) = field(record, "session_id")
        && Some(id) != field(record, "sessionId")
    {
        summary.links.session_ids.insert(id.to_owned());
    }
    match field(record, "type") {
        Some("continued-in") => {
            if let Some(id) = field(record, "continuedInSessionId") {
                summary.links.continued_in.insert(id.to_owned());
            }
            return;
        }
        Some("bridge-session") => {
            if let Some(id) = field(record, "bridgeSessionId") {
                summary.links.bridges.insert(id.to_owned());
            }
            return;
        }
        Some("custom-title") => {
            summary.title = field(record, "customTitle").map(str::to_owned);
            return;
        }
        Some("agent-name") => {
            summary.agent_name = field(record, "agentName").map(str::to_owned);
            return;
        }
        Some("attachment") => {
            if let Some(attachment) = record.get("attachment")
                && field(attachment, "type") == Some("queued_command")
            {
                let text = attachment.get("prompt").map(text_parts).unwrap_or_default();
                let meta = attachment.get("isMeta").and_then(Value::as_bool) == Some(true)
                    || record.get("isMeta").and_then(Value::as_bool) == Some(true);
                prompt_events(
                    summary,
                    &text,
                    attachment.get("origin").filter(|origin| origin.is_object()),
                    meta,
                    offset,
                    time,
                );
            }
            return;
        }
        _ => {}
    }
    if summary.entrypoint.is_none() {
        summary.entrypoint = field(record, "entrypoint").map(str::to_owned);
    }
    let role = field(record, "type");
    if role == Some("system") && field(record, "subtype") == Some("compact_boundary") {
        let metadata = record.get("compactMetadata");
        signal(
            summary,
            SignalKind::Compact,
            offset,
            time,
            metadata.and_then(|metadata| field(metadata, "trigger")),
            metadata
                .and_then(|metadata| metadata.get("preTokens"))
                .and_then(Value::as_u64),
        );
    }
    let Some(message) = record.get("message") else {
        return;
    };
    if role == Some("assistant")
        && let Some(model) = field(message, "model")
    {
        summary.last_model = Some(model.to_owned());
    }
    if record.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
        signal(summary, SignalKind::Compact, offset, time, None, None);
        return;
    }
    let tool_result = record.get("toolUseResult");
    let mut texts = Vec::new();
    match message.get("content") {
        Some(Value::String(text)) => texts.push(text.as_str()),
        Some(Value::Array(blocks)) => {
            for (block, item) in blocks.iter().enumerate() {
                let block = u32::try_from(block).unwrap_or(u32::MAX);
                match field(item, "type") {
                    Some("text") => {
                        let text = field(item, "text").unwrap_or("");
                        if role == Some("assistant") {
                            if !text.trim().is_empty() {
                                push(
                                    summary,
                                    Event {
                                        k: Kind::A,
                                        o: offset,
                                        b: block,
                                        t: time,
                                        ..Event::default()
                                    },
                                );
                            }
                        } else {
                            texts.push(text);
                        }
                    }
                    // Retain empty blocks too: Claude often redacts the text, but
                    // the viewer still shows a masked thought marker.
                    Some("thinking") if role == Some("assistant") => {
                        extra(
                            summary,
                            Event {
                                k: Kind::Think,
                                o: offset,
                                b: block,
                                t: time,
                                ..Event::default()
                            },
                        );
                    }
                    Some("tool_use") => tool_event(
                        summary,
                        Event {
                            k: Kind::Tool,
                            o: offset,
                            b: block,
                            t: time,
                            id: field(item, "id").map(str::to_owned),
                            n: field(item, "name").map(str::to_owned),
                            ..Event::default()
                        },
                    ),
                    Some("tool_result") => {
                        if let Some(id) = field(item, "tool_use_id") {
                            let error = item.get("is_error").and_then(Value::as_bool) == Some(true);
                            resolve(summary, id, |name| {
                                let (flags, msg_id) = result_flags(name, tool_result);
                                Reply {
                                    o: offset,
                                    b: block,
                                    t: time,
                                    e: error,
                                    f: flags,
                                    m: msg_id,
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if role == Some("user") && !texts.is_empty() {
        let text = texts.join("\n");
        let meta = record.get("isMeta").and_then(Value::as_bool) == Some(true);
        prompt_events(
            summary,
            &text,
            record.get("origin").filter(|origin| origin.is_object()),
            meta,
            offset,
            time,
        );
    }
}

/// A Codex call's structured exit code: `metadata.exit_code` of its output.
/// Output text is never read for it, so `rg exit_code` or a printed
/// "Exit code: 1" can't mark a call failed; without the field it is unknown.
/// An output that only acknowledges the call, as `request_user_input_async`
/// answers at once.
fn acknowledgement(output: Option<&Value>) -> bool {
    let parsed;
    let value = match output {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(value) => {
                parsed = value;
                &parsed
            }
            Err(_) => return false,
        },
        Some(other) => other,
        None => return false,
    };
    value
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key("accepted"))
}

fn codex_exit(output: Option<&Value>) -> Option<i64> {
    let parsed;
    let value = match output? {
        Value::String(text) => {
            parsed = serde_json::from_str::<Value>(text).ok()?;
            &parsed
        }
        other => other,
    };
    value.get("metadata")?.get("exit_code")?.as_i64()
}

/// Text Codex adds before a prompt, by its tag. Any other text is a prompt,
/// even one that starts with `<`.
const HARNESS_TAGS: [&str; 5] = [
    "environment_context",
    "user_instructions",
    "recommended_plugins",
    "user_shell_command",
    "turn_aborted",
];

fn harness_tag(text: &str) -> Option<&'static str> {
    let rest = text.trim_start().strip_prefix('<')?;
    HARNESS_TAGS.into_iter().find(|tag| {
        rest.strip_prefix(tag)
            .is_some_and(|after| after.starts_with(['>', ' ', '\n']))
    })
}

/// Codex rollout lines.
pub(crate) fn codex(summary: &mut FileIndex, record: &Value, offset: u64) {
    let time = record_time(record);
    activity(summary, time);
    let payload = &record["payload"];
    match field(record, "type") {
        Some("compacted") => signal(summary, SignalKind::Compact, offset, time, None, None),
        Some("session_meta") => {
            summary.entrypoint = field(payload, "originator").map(str::to_owned);
            summary.cwd = field(payload, "cwd").map(str::to_owned);
            summary.branch = payload
                .get("git")
                .and_then(|git| field(git, "branch"))
                .map(str::to_owned);
        }
        Some("event_msg") if field(payload, "type") == Some("token_count") => {
            if let Some(info) = payload.get("info") {
                if let Some(usage) = info.get("total_token_usage") {
                    let current = Tokens {
                        input: number(usage, "input_tokens"),
                        cached_input: number(usage, "cached_input_tokens"),
                        output: number(usage, "output_tokens"),
                        reasoning_output: number(usage, "reasoning_output_tokens"),
                        total: number(usage, "total_tokens"),
                    };
                    if let Some(model) = &summary.last_model {
                        let input_delta = current.input.saturating_sub(summary.codex_tokens.input);
                        let cache_read = current
                            .cached_input
                            .saturating_sub(summary.codex_tokens.cached_input);
                        let counts = ModelTokens {
                            input: input_delta.saturating_sub(cache_read),
                            output: current.output.saturating_sub(summary.codex_tokens.output),
                            cache_write: 0,
                            cache_read,
                        };
                        summary
                            .codex_tokens_by_model
                            .entry(model.clone())
                            .or_default()
                            .add(&counts);
                        summary.codex_usage_events.push(CodexUsageEvent {
                            model: model.clone(),
                            tokens: counts,
                            timestamp: time,
                        });
                    }
                    summary.codex_tokens = current;
                }
                if let Some(time) = time
                    && let Some(latest) = rate_limits(info, time)
                    && summary
                        .rate_limits
                        .as_ref()
                        .is_none_or(|previous| latest.recorded_at >= previous.recorded_at)
                {
                    summary.rate_limits = Some(latest);
                }
            }
        }
        Some("event_msg") if field(payload, "type") == Some("context_compacted") => {
            signal(summary, SignalKind::Compact, offset, time, None, None);
        }
        // A finished command's structured exit code, and the completed
        // operations emitted by code-mode `exec`.
        Some("event_msg") if field(payload, "type") == Some("item_completed") => {
            let item = &payload["item"];
            if field(item, "type") == Some("ContextCompaction") {
                signal(summary, SignalKind::Compact, offset, time, None, None);
            }
            let operation = matches!(field(item, "type"), Some("CommandExecution" | "FileChange"));
            let legacy_tool = field(item, "id").is_some_and(|id| non_code_mode_tool(summary, id));
            let open = open_code_mode_call(summary);
            let code = item.get("exit_code").and_then(Value::as_i64);
            let failed =
                code.is_some_and(|code| code != 0) || field(item, "status") == Some("failed");
            let known = code.is_some()
                || field(item, "status") == Some("failed")
                || field(item, "type") == Some("FileChange");
            // A yielded command's end: its `process_id` is the session id
            // the starting script answered with. It arrives inside a poll,
            // or while no script runs. Inside a script that isn't a poll the
            // id was used again: that script's own command, and the old
            // process is gone.
            let yielded = if field(item, "type") == Some("CommandExecution") && !legacy_tool {
                exec_session(item.get("process_id")).and_then(|session| {
                    let start = summary.yields.remove(&session)?;
                    let own = open.is_some_and(|open| {
                        summary
                            .events
                            .get(open)
                            .is_some_and(|event| event.script & STDIN == 0)
                    });
                    (!own).then_some(start)
                })
            } else {
                None
            };
            if let Some(start) = yielded {
                if let Some(found) = summary.events.get_mut(start)
                    && let Some(started) = found.y.as_mut()
                {
                    started.done = Some(Reply {
                        o: offset,
                        t: time,
                        e: failed,
                        f: if known { 0 } else { UNKNOWN },
                        ..Reply::default()
                    });
                }
                if let Some(poll) = open.and_then(|open| summary.events.get_mut(open)) {
                    poll.poll = Some(start);
                }
            }
            // An item is a step of its own only when exactly one code-mode
            // call is open to own it. Otherwise it is a plain call's item, or
            // its owner is ambiguous: its wrapper, or the plain call, is the
            // step, and the item adds nothing but the exit code below.
            let parent = if operation && !legacy_tool && yielded.is_none() {
                open
            } else {
                None
            };
            if let Some(parent) = parent {
                extra(
                    summary,
                    Event {
                        k: Kind::Operation,
                        o: offset,
                        t: time,
                        id: field(item, "id").map(str::to_owned),
                        n: field(item, "type").map(str::to_owned),
                        parent: Some(parent),
                        r: Some(Reply {
                            o: offset,
                            t: time,
                            e: failed,
                            f: if known { 0 } else { UNKNOWN },
                            ..Reply::default()
                        }),
                        ..Event::default()
                    },
                );
            }
            if legacy_tool
                && field(item, "type") == Some("CommandExecution")
                && let Some(id) = field(item, "id")
            {
                link_item(summary, id, offset);
            }
            if let (Some(id), Some(code)) = (
                field(item, "id"),
                item.get("exit_code").and_then(Value::as_i64),
            ) {
                let code_mode_call = summary
                    .pending
                    .get(id)
                    .and_then(|index| summary.events.get(*index))
                    .is_some_and(|event| event.code_mode);
                if !code_mode_call {
                    exit_code(summary, id, code, offset, time);
                }
            }
        }
        Some("turn_context") => {
            if let Some(model) = field(payload, "model") {
                summary.last_model = Some(model.to_owned());
            }
        }
        Some("event_msg") if field(payload, "type") == Some("task_complete") => {
            close_code_mode(summary);
            summary.failed = payload.get("error").is_some_and(|error| !error.is_null());
        }
        Some("event_msg") if field(payload, "type") == Some("turn_aborted") => {
            signal(
                summary,
                SignalKind::Interrupt,
                offset,
                time,
                field(payload, "reason"),
                None,
            );
            close_code_mode(summary);
        }
        Some("response_item") => match field(payload, "type") {
            Some("message") => {
                let text = payload
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| field(part, "text"))
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                match field(payload, "role") {
                    Some("user") => {
                        close_code_mode(summary);
                        if let Some(tag) = harness_tag(&text) {
                            extra(
                                summary,
                                Event {
                                    k: Kind::Harness,
                                    o: offset,
                                    t: time,
                                    n: Some(tag.to_owned()),
                                    ..Event::default()
                                },
                            );
                        } else if !text.trim().is_empty() {
                            push(
                                summary,
                                Event {
                                    k: Kind::U,
                                    o: offset,
                                    t: time,
                                    ..Event::default()
                                },
                            );
                        }
                    }
                    Some("assistant") if !text.trim().is_empty() => push(
                        summary,
                        Event {
                            k: Kind::A,
                            o: offset,
                            t: time,
                            ..Event::default()
                        },
                    ),
                    _ => {}
                }
            }
            Some("function_call" | "custom_tool_call" | "local_shell_call") => {
                let code_mode = field(payload, "type") == Some("custom_tool_call")
                    && field(payload, "name") == Some("exec");
                let script = if code_mode {
                    field(payload, "input").map_or(0, script_flags)
                } else {
                    0
                };
                let poll = open_poll(summary, script);
                tool_event(
                    summary,
                    Event {
                        k: Kind::Tool,
                        o: offset,
                        t: time,
                        id: field(payload, "call_id").map(str::to_owned),
                        n: Some(field(payload, "name").unwrap_or("shell").to_owned()),
                        code_mode,
                        code_mode_open: code_mode,
                        script,
                        poll,
                        ..Event::default()
                    },
                );
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                if let Some(id) = field(payload, "call_id") {
                    let output = payload.get("output");
                    let code = codex_exit(output);
                    let mut flags = if code.is_none() { UNKNOWN } else { 0 };
                    if acknowledgement(output) {
                        flags |= ACK;
                    }
                    let code_mode_index = summary.pending.get(id).copied().filter(|index| {
                        summary
                            .events
                            .get(*index)
                            .is_some_and(|event| event.code_mode)
                    });
                    let code_mode_call = code_mode_index.is_some();
                    let script_output = field(payload, "type") == Some("custom_tool_call_output");
                    if script_output || !code_mode_call {
                        resolve(summary, id, |_| Reply {
                            o: offset,
                            t: time,
                            e: code.is_some_and(|code| code != 0)
                                || (code_mode_call && script_error(output)),
                            f: flags,
                            ..Reply::default()
                        });
                    }
                    if let Some(index) = code_mode_index
                        && script_output
                    {
                        fold_yield(summary, index, output, offset);
                    }
                }
            }
            Some("reasoning")
                if payload
                    .get("summary")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| field(part, "text").is_some())
                    }) =>
            {
                extra(
                    summary,
                    Event {
                        k: Kind::Think,
                        o: offset,
                        t: time,
                        ..Event::default()
                    },
                );
            }
            Some("agent_message") => push(
                summary,
                Event {
                    k: Kind::Agm,
                    o: offset,
                    t: time,
                    ..Event::default()
                },
            ),
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_clusters_within_five_minutes_in_any_order() {
        let mut busy = Vec::new();
        for time in [0, 60_000, 300_000 + 60_000, 1_000_000, 700_000, 2_000_000] {
            busy_add(&mut busy, time);
        }
        // Sorted: gaps of 1, 5, 5.7, 5 and 16.7 minutes.
        assert_eq!(
            busy,
            [(0, 360_000), (700_000, 1_000_000), (2_000_000, 2_000_000)]
        );
        assert_eq!(
            busy_merge(vec![(10, 20), (0, 5), (400_000, 500_000)]),
            [(0, 20), (400_000, 500_000)]
        );
    }

    #[test]
    fn timestamps_keep_milliseconds() {
        assert_eq!(
            parse_ms("2026-09-24T00:00:00.123Z"),
            Some(1_790_208_000_123)
        );
        assert_eq!(parse_ms("2026-09-24T00:00:00Z"), Some(1_790_208_000_000));
        assert_eq!(
            parse_ms("2026-09-24T08:00:00.5+08:00"),
            Some(1_790_208_000_500)
        );
    }

    #[test]
    fn attributes_and_notifications_parse() {
        let text = "<cross-session-message from=\"uds:/x.sock\" from-name=\"peer-a1\">hi</cross-session-message>";
        let found = tags(text, "cross-session-message");
        assert_eq!(found.len(), 1);
        assert_eq!(attributes(found[0].0)["from-name"], "peer-a1");
        let notes = notifications(
            "<task-notification><tool-use-id>t1</tool-use-id><status>completed</status><result>ok</result></task-notification>",
        );
        assert_eq!(notes[0].0, Some("t1"));
        assert_eq!(notes[0].1, Some("completed"));
        assert_eq!(inner(notes[0].2, "result"), Some("ok"));
    }
    #[test]
    fn non_ascii_attributes_never_split_a_character() {
        for tag in [
            " 名前=\"x\" from=\"y\"",
            " fröm=\"ü\" from-name=\"名前\"",
            " ü=\"\" from=\"z\"",
        ] {
            let parsed = attributes(tag);
            assert!(parsed.values().all(|value| !value.contains('"')));
        }
        assert_eq!(attributes(" 名前=\"x\" from=\"y\"")["from"], "y");
        assert_eq!(attributes(" x名=\"1\"").get("名"), None);
        assert_eq!(
            attributes(" fröm=\"ü\" from-name=\"名前\"")["from-name"],
            "名前"
        );
    }

    /// Each call's outcome: `Some(true)` failed, `Some(false)` succeeded,
    /// `None` unknown.
    fn codex_reply(records: &[Value]) -> Vec<Option<bool>> {
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }
        index
            .events
            .iter()
            .filter(|event| event.k == Kind::Tool)
            .map(|event| {
                let reply = event.r.as_ref().expect("resolved");
                (reply.f & UNKNOWN == 0).then_some(reply.e)
            })
            .collect()
    }

    fn call(id: &str) -> Value {
        serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":id,"arguments":"{}"}})
    }

    fn output(id: &str, output: Value) -> Value {
        serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":id,"output":output}})
    }

    #[test]
    fn codex_failure_comes_only_from_a_structured_exit_code() {
        let replies = codex_reply(&[
            call("rg"),
            output(
                "rg",
                serde_json::json!(
                    "{\"output\":\"src/a.rs:3: exit_code: 1\",\"metadata\":{\"exit_code\":0}}"
                ),
            ),
            call("text"),
            output(
                "text",
                serde_json::json!("Exit code: 1\nWall time: 0.2 seconds\nOutput:\nboom"),
            ),
            call("failed"),
            output(
                "failed",
                serde_json::json!({"output":"x","metadata":{"exit_code":2}}),
            ),
            call("item"),
            output("item", serde_json::json!("Process exited with code 1")),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"item","exit_code":1}}}),
        ]);
        assert_eq!(replies, [Some(false), None, Some(true), Some(true)]);
    }

    #[test]
    fn an_acknowledgement_is_not_an_answer() {
        let mut index = FileIndex::default();
        for (offset, record) in [
            call("ask"),
            output("ask", serde_json::json!("{\"accepted\":true}")),
            call("real"),
            output(
                "real",
                serde_json::json!("{\"accepted\":true,\"answer\":\"yes\"}"),
            ),
        ]
        .iter()
        .enumerate()
        {
            codex(&mut index, record, offset as u64);
        }
        let flags: Vec<bool> = index
            .events
            .iter()
            .map(|event| event.r.as_ref().unwrap().f & ACK != 0)
            .collect();
        assert_eq!(flags, [true, false]);
    }

    fn texts(records: &[Value]) -> Vec<(u64, u32)> {
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64 * 100);
        }
        index
            .events
            .iter()
            .filter(|event| event.k == Kind::A)
            .map(|event| (event.o, event.b))
            .collect()
    }

    fn said(id: &str, blocks: &[&str]) -> Value {
        let content: Vec<Value> = blocks
            .iter()
            .map(|text| serde_json::json!({"type":"text","text":text}))
            .collect();
        serde_json::json!({"type":"assistant","uuid":format!("u-{id}"),"message":{"id":id,"role":"assistant","content":content}})
    }

    #[test]
    fn claude_compact_boundary_and_summary_make_one_signal() {
        let records = [
            serde_json::json!({
                "type":"system",
                "subtype":"compact_boundary",
                "compactMetadata":{"trigger":"auto","preTokens":155000}
            }),
            serde_json::json!({
                "type":"user",
                "isCompactSummary":true,
                "message":{"role":"user","content":[{"type":"text","text":"summary"}]}
            }),
        ];
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
        assert_eq!(index.signals[0].o, 0);
        assert_eq!(index.signals[0].at, 0);
        assert_eq!(index.signals[0].n.as_deref(), Some("auto"));
        assert_eq!(index.signals[0].v, Some(155000));
        assert!(index.events.is_empty());
    }

    #[test]
    fn claude_compact_summary_from_old_logs_makes_a_signal() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({
                "type":"user",
                "isCompactSummary":true,
                "message":{"role":"user","content":[{"type":"text","text":"summary"}]}
            }),
            12,
        );

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
        assert_eq!(index.signals[0].o, 12);
        assert_eq!(index.signals[0].n, None);
        assert_eq!(index.signals[0].v, None);
    }

    #[test]
    fn claude_compactions_separated_by_a_prompt_make_two_signals() {
        let records = [
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":155000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":[]}}),
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"continue"}]}}),
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"manual","preTokens":140000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":[]}}),
        ];
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 2);
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| signal.at)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(index.signals[0].n.as_deref(), Some("auto"));
        assert_eq!(index.signals[1].n.as_deref(), Some("manual"));
    }

    #[test]
    fn claude_interrupts_are_signals_without_user_events() {
        for (text, expected) in [
            ("[Request interrupted by user]", "user"),
            ("[Request interrupted by user for tool use]", "tool"),
        ] {
            let mut index = FileIndex::default();
            claude(
                &mut index,
                &serde_json::json!({
                    "type":"user",
                    "message":{"role":"user","content":[{"type":"text","text":"keep this prompt"}]}
                }),
                10,
            );
            let events_before_interrupt = index.events.clone();
            claude(
                &mut index,
                &serde_json::json!({
                    "type":"user",
                    "message":{"role":"user","content":[{"type":"text","text":text}]}
                }),
                20,
            );

            assert_eq!(index.events, events_before_interrupt);
            assert_eq!(index.signals.len(), 1);
            assert_eq!(index.signals[0].k, SignalKind::Interrupt);
            assert_eq!(index.signals[0].o, 20);
            assert_eq!(index.signals[0].n.as_deref(), Some(expected));
        }
    }

    #[test]
    fn codex_compaction_records_dedupe_and_context_compaction_items_index() {
        let records = [
            serde_json::json!({"type":"compacted"}),
            serde_json::json!({"type":"event_msg","payload":{"type":"context_compacted"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"ContextCompaction"}}}),
        ];
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
        assert_eq!(index.signals[0].at, 0);
        assert!(index.events.is_empty());
    }

    fn assert_one_codex_compact(record: Value) {
        let mut index = FileIndex::default();
        codex(&mut index, &record, 0);

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
    }

    #[test]
    fn lone_codex_compacted_record_indexes_one_compact_signal() {
        assert_one_codex_compact(serde_json::json!({"type":"compacted"}));
    }

    #[test]
    fn lone_codex_context_compacted_event_indexes_one_compact_signal() {
        assert_one_codex_compact(
            serde_json::json!({"type":"event_msg","payload":{"type":"context_compacted"}}),
        );
    }

    #[test]
    fn lone_codex_context_compaction_item_indexes_one_compact_signal() {
        assert_one_codex_compact(serde_json::json!({
            "type":"event_msg",
            "payload":{"type":"item_completed","item":{"type":"ContextCompaction"}}
        }));
    }

    #[test]
    fn codex_turn_aborts_keep_only_safe_reason_tags() {
        let long_reason = "x".repeat(33);
        let records = [
            (
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}),
                Some("interrupted"),
            ),
            (
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"bad\nreason"}}),
                None,
            ),
            (
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":long_reason}}),
                None,
            ),
        ];
        let mut index = FileIndex::default();
        for (offset, (record, _)) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 3);
        assert!(
            index
                .signals
                .iter()
                .all(|signal| signal.k == SignalKind::Interrupt)
        );
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| signal.n.as_deref())
                .collect::<Vec<_>>(),
            records.map(|(_, expected)| expected)
        );
    }

    #[test]
    fn signals_round_trip_through_the_cache_and_v13_is_stale() {
        let root = std::env::temp_dir().join(format!(
            "semon-signals-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let index = FileIndex {
            signals: vec![Signal {
                k: SignalKind::Compact,
                o: 42,
                t: Some(1_790_208_000_123),
                at: 2,
                n: Some("auto".to_owned()),
                v: Some(155000),
            }],
            ..FileIndex::default()
        };
        let mut cache = EventCache {
            version: CACHE_VERSION,
            files: BTreeMap::from([(
                "session.jsonl".to_owned(),
                CachedFile {
                    dev: 1,
                    ino: 2,
                    offset: 3,
                    size: 3,
                    modified_ns: 4,
                    index: Arc::new(index.clone()),
                },
            )]),
            ..EventCache::default()
        };
        let path = root.join("events.json");
        cache.save(&path).unwrap();
        let reopened = EventCache::read(&path);
        let cached_index = &reopened.files.get("session.jsonl").unwrap().index;
        let serialized_index = serde_json::to_value(&index).unwrap();
        assert_eq!(serialized_index["signals"][0]["k"], "compact");
        assert_eq!(
            serialized_index,
            serde_json::to_value(cached_index.as_ref()).unwrap()
        );

        let stale_path = root.join("stale.events.json");
        cache.version = 13;
        cache.save(&stale_path).unwrap();
        let stale = EventCache::read(&stale_path);
        assert_eq!(stale.version, CACHE_VERSION);
        assert!(stale.files.is_empty());
        assert!(!stale.files.contains_key("session.jsonl"));

        let current_path = root.join("current.events.json");
        cache.version = 14;
        cache.save(&current_path).unwrap();
        let current = EventCache::read(&current_path);
        assert_eq!(current.version, 14);
        let retained_index = &current.files.get("session.jsonl").unwrap().index;
        assert_eq!(
            serde_json::to_value(retained_index.as_ref()).unwrap(),
            serialized_index
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_assistant_text_is_its_own_event() {
        // Two text blocks in one message.
        assert_eq!(texts(&[said("m1", &["first", "second"])]), [(0, 0), (0, 1)]);
        // Two consecutive messages, and one message streamed over two lines
        // (Claude Code writes a line per block, with the same message id).
        assert_eq!(
            texts(&[
                said("m1", &["one"]),
                said("m2", &["two"]),
                said("m3", &["three"]),
                said("m3", &["four"]),
            ]),
            [(0, 0), (100, 0), (200, 0), (300, 0)]
        );
    }

    #[test]
    fn claude_events_carry_their_record_uuid() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"assistant","uuid":"u-1","message":{"role":"assistant","content":[
                {"type":"text","text":"hi"},{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}),
            0,
        );
        assert!(
            index
                .events
                .iter()
                .all(|event| event.u.as_deref() == Some("u-1"))
        );
        assert_eq!(index.events.len(), 2);
    }

    #[test]
    fn assistant_billing_facts_split_cache_writes_and_keep_thinking_in_output_once() {
        let mut index = FileIndex::default();
        let record = serde_json::json!({
            "type":"assistant",
            "timestamp":"2026-09-28T12:00:00.000Z",
            "message":{
                "id":"billing-1",
                "model":"claude-opus-5",
                "usage":{
                    "input_tokens":2,
                    "cache_creation_input_tokens":15,
                    "cache_read_input_tokens":20,
                    "output_tokens":7,
                    "output_tokens_details":{"thinking_tokens":6},
                    "server_tool_use":{"web_search_requests":3},
                    "service_tier":"standard",
                    "speed":"fast",
                    "cache_creation":{"ephemeral_1h_input_tokens":10,"ephemeral_5m_input_tokens":5}
                }
            }
        });
        claude(&mut index, &record, 0);
        let fact = index.usage().get("billing-1").unwrap();
        assert_eq!(fact.model_tokens.output, 7);
        assert_eq!(fact.billing.cache_write_5m, 5);
        assert_eq!(fact.billing.cache_write_1h, 10);
        assert_eq!(fact.billing.prompt_size, 37);
        assert_eq!(fact.billing.web_search_requests, 3);
        assert_eq!(fact.billing.speed.as_deref(), Some("fast"));
        assert_eq!(fact.billing.service_tier.as_deref(), Some("standard"));
        assert!(!fact.billing.split_unknown);

        let fallback = serde_json::json!({
            "type":"assistant",
            "timestamp":"2026-09-28T12:01:00.000Z",
            "message":{"id":"billing-2","model":"claude-opus-5","usage":{
                "input_tokens":1,"cache_creation_input_tokens":9,"cache_read_input_tokens":0,"output_tokens":2
            }}
        });
        claude(&mut index, &fallback, 1);
        let fact = index.usage().get("billing-2").unwrap();
        assert_eq!(fact.billing.cache_write_5m, 9);
        assert_eq!(fact.billing.cache_write_1h, 0);
        assert!(fact.billing.split_unknown);
    }

    #[test]
    fn claude_state_reader_keeps_allowlisted_snapshots_across_overwrites() {
        let root = std::env::temp_dir().join(format!(
            "semon-reported-runs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let claude_json = root.join(".claude.json");
        let cache_path = EventCache::path(&root.join("index.json"));
        let secrets = ["fixture-secret@example.invalid", "fixture-account-uuid"];
        let first = serde_json::json!({
            "oauthAccount":{"emailAddress":secrets[0],"accountUuid":secrets[1]},
            "projects":{"/private/project/path":{
                "lastSessionId":"run-one","lastStartTime":1000,"lastCost":1.25,
                "lastDuration":2000,"lastAPIDuration":1500,"lastToolDuration":300,
                "lastLinesAdded":4,"lastLinesRemoved":2,"projectSecret":"unlisted-project-value",
                "lastModelUsage":{"claude-opus-5[1m]":{
                    "inputTokens":10,"outputTokens":5,"thinkingTokens":3,
                    "cacheReadInputTokens":20,"cacheCreationInputTokens":4,
                    "webSearchRequests":1,"costUSD":1.25,"accountSecret":"unlisted-model-value"
                }}
            }},
            "unlistedRootSecret":"unlisted-root-value"
        });
        fs::write(&claude_json, first.to_string()).unwrap();
        let mut cache = EventCache::read(&cache_path);
        let mut dirty = false;
        cache.refresh_reported_runs(&claude_json, 2000, &mut dirty);
        assert!(dirty);
        cache.save(&cache_path).unwrap();
        cache = EventCache::read(&cache_path);
        assert_eq!(cache.reported_runs().count(), 1);
        let serialized = serde_json::to_string(&cache).unwrap();
        for secret in secrets.into_iter().chain([
            "unlisted-project-value",
            "unlisted-model-value",
            "unlisted-root-value",
            "/private/project/path",
        ]) {
            assert!(!serialized.contains(secret));
        }
        let snapshot = cache.reported_runs().next().unwrap();
        assert_eq!(
            snapshot.last_model_usage["claude-opus-5[1m]"].thinking_tokens,
            3
        );

        let second = serde_json::json!({
            "projects":{"/private/project/path":{
                "lastSessionId":"run-two","lastStartTime":3000,"lastCost":2.0,
                "lastDuration":4000,"lastAPIDuration":3000,"lastToolDuration":600,
                "lastLinesAdded":8,"lastLinesRemoved":1,"lastModelUsage":{}
            }},
            "oauthAccount":{"emailAddress":secrets[0],"accountUuid":secrets[1]}
        });
        fs::write(&claude_json, second.to_string()).unwrap();
        cache.refresh_reported_runs(&claude_json, 5000, &mut dirty);
        assert_eq!(cache.reported_runs().count(), 2);
        cache.save(&cache_path).unwrap();
        let reopened = EventCache::read(&cache_path);
        let ids: BTreeSet<_> = reopened
            .reported_runs()
            .map(|snapshot| snapshot.last_session_id.as_str())
            .collect();
        assert_eq!(ids, ["run-one", "run-two"].into_iter().collect());
        let serialized = serde_json::to_string(&reopened).unwrap();
        assert!(!serialized.contains(secrets[0]));
        assert!(!serialized.contains(secrets[1]));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_unreadable_claude_state_is_an_empty_input() {
        let root = std::env::temp_dir().join(format!(
            "semon-reported-runs-missing-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let cache_path = EventCache::path(&root.join("index.json"));
        let mut cache = EventCache::read(&cache_path);
        let mut dirty = false;
        cache.refresh_reported_runs(&root.join("missing.json"), 1000, &mut dirty);
        assert!(cache.reported_runs().next().is_none());
        fs::create_dir(root.join("unreadable.json")).unwrap();
        cache.refresh_reported_runs(&root.join("unreadable.json"), 2000, &mut dirty);
        assert!(cache.reported_runs().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_hand_back_keeps_its_sender_task_id_and_from() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","origin":{"kind":"peer","handback":true,"from":"lead","senderTaskId":"a1","body":"done"},"message":{"role":"user","content":"x"}}),
            0,
        );
        assert_eq!(index.events[0].id.as_deref(), Some("a1"));
        assert_eq!(index.events[0].n.as_deref(), Some("lead"));
    }

    #[test]
    fn only_a_whole_compact_command_is_skipped() {
        for (text, skipped) in [
            ("/compact", true),
            ("/compact keep tests", true),
            ("/compactor", false),
            ("<command-name>/compact</command-name>", true),
        ] {
            assert_eq!(is_compact_command(text), skipped, "{text}");
        }
    }

    #[test]
    fn a_script_argument_is_read_only_from_a_plain_literal() {
        let literal = |text: &str| Some(ScriptArg::Literal(text.to_owned()));
        for (script, tool, key, expected) in [
            (
                "await tools.write_stdin({session_id:1,chars:\"\"})",
                "write_stdin",
                "chars",
                literal(""),
            ),
            (
                "tools.write_stdin({session_id: 1, \"chars\": 'y\\n'})",
                "write_stdin",
                "chars",
                literal("y\n"),
            ),
            (
                "tools.write_stdin({session_id:1})",
                "write_stdin",
                "chars",
                Some(ScriptArg::Absent),
            ),
            (
                "tools.write_stdin({session_id:1, chars: input})",
                "write_stdin",
                "chars",
                Some(ScriptArg::Other),
            ),
            (
                "tools.write_stdin({chars})",
                "write_stdin",
                "chars",
                Some(ScriptArg::Other),
            ),
            (
                "tools.write_stdin(args)",
                "write_stdin",
                "chars",
                Some(ScriptArg::Other),
            ),
            (
                "tools.exec_command({cmd:\"echo \\\"hi\\\" \\u0041\\x42\\u{43}\", workdir:\"/w\"})",
                "exec_command",
                "cmd",
                literal("echo \"hi\" ABC"),
            ),
            (
                "tools.exec_command({env: {cmd: \"inner\"}, // cmd: \"comment\"\n cmd: `outer`})",
                "exec_command",
                "cmd",
                literal("outer"),
            ),
            (
                "tools.exec_command({cmd:`ls ${dir}`})",
                "exec_command",
                "cmd",
                Some(ScriptArg::Other),
            ),
            (
                "tools.exec_command({cmd:\"unterminated",
                "exec_command",
                "cmd",
                Some(ScriptArg::Other),
            ),
            ("my_exec_command({cmd:\"x\"})", "exec_command", "cmd", None),
            ("text('no call')", "exec_command", "cmd", None),
            // Calls in comments and strings aren't calls.
            (
                "// tools.write_stdin({chars:\"x\"})\ntext('tools.write_stdin({})')",
                "write_stdin",
                "chars",
                None,
            ),
            (
                "/* tools.exec_command({cmd:\"a\"}) */ tools.exec_command({cmd:\"b\"})",
                "exec_command",
                "cmd",
                literal("b"),
            ),
            ("tools.exec_command({cmd:\"x\"})", "", "cmd", None),
            // Non-ASCII text, escapes and identifiers.
            (
                "tools.exec_command({ключ: 1, cmd: \"café \\u{1F600} ü\"})",
                "exec_command",
                "cmd",
                literal("café 😀 ü"),
            ),
            (
                "tools.exec_command({cmd: \"\\xé\\u{é}\\u00é\"})",
                "exec_command",
                "cmd",
                literal("xéu{é}u00é"),
            ),
            (
                "tools.exec_command({cmd: \"\\x+1\\u{+41}\\u+041\"})",
                "exec_command",
                "cmd",
                literal("x+1u{+41}u+041"),
            ),
            (
                "tools.exec_command({cmd: \"\\u{41\" + \"}\"})",
                "exec_command",
                "cmd",
                literal("u{41"),
            ),
        ] {
            assert_eq!(script_arg(script, tool, key), expected, "{script}");
        }
        assert_eq!(
            script_flags("tools.write_stdin({session_id:1,chars:\"\"})"),
            STDIN
        );
        assert_eq!(
            script_flags("tools.write_stdin({session_id:1,chars:\"q\"})"),
            STDIN | SENDS
        );
        assert_eq!(
            script_flags("tools.write_stdin({session_id:1,chars:keys})"),
            STDIN | SENDS
        );
        assert_eq!(script_flags("tools.exec_command({cmd:\"ls\"})"), 0);
        // A script that starts a command and then polls it starts it.
        assert_eq!(
            script_flags(
                "let r = await tools.exec_command({cmd:\"ls\"}); r = await tools.write_stdin({session_id: r.session_id, chars: \"\"});"
            ),
            0
        );
    }

    fn exec_line(id: &str, script: &str) -> Value {
        serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":id,"name":"exec","input":script}})
    }

    fn yielded_line(id: &str, session: u64) -> Value {
        let result = serde_json::json!({"wall_time_seconds":1.0,"session_id":session,"output":""});
        serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":id,"output":[
            {"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},
            {"type":"input_text","text":result.to_string()},
        ]}})
    }

    /// Feeds lines at increasing offsets, as a file would.
    fn feed(summary: &mut FileIndex, next: &mut u64, lines: &[Value]) {
        for line in lines {
            *next += 100;
            codex(summary, line, *next);
        }
    }

    #[test]
    fn open_yields_are_bounded_and_the_oldest_goes_first() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        for session in 1..=YIELDS_MAX as u64 + 1 {
            let id = format!("start-{session}");
            feed(
                &mut summary,
                &mut next,
                &[
                    exec_line(&id, "await tools.exec_command({cmd:\"serve\"})"),
                    yielded_line(&id, session),
                ],
            );
        }
        assert_eq!(summary.yields.len(), YIELDS_MAX);
        assert!(!summary.yields.contains_key("1"), "the oldest was dropped");
        assert!(summary.yields.contains_key("2"));
        assert!(summary.yields.contains_key(&(YIELDS_MAX + 1).to_string()));
        // The dropped command keeps its step: it just never completes.
        assert!(summary.events[0].y.is_some());
        assert!(summary.events.iter().all(|event| event.y.is_some()));
    }

    #[test]
    fn polls_past_the_bound_still_fold_in_and_mark_the_output_cut() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("start", "await tools.exec_command({cmd:\"serve\"})"),
                yielded_line("start", 9),
            ],
        );
        let poll = "await tools.write_stdin({session_id:9,chars:\"\"})";
        for count in 1..=POLLS_MAX + 1 {
            let id = format!("poll-{count}");
            feed(
                &mut summary,
                &mut next,
                &[exec_line(&id, poll), yielded_line(&id, 9)],
            );
            let yielded = summary.events[0].y.as_ref().unwrap();
            assert_eq!(yielded.polls.len(), count.min(POLLS_MAX), "{count}");
            assert_eq!(yielded.cut, count > POLLS_MAX, "{count}");
            assert_eq!(summary.events.last().unwrap().poll, Some(0), "{count}");
        }
    }

    #[test]
    fn a_yielded_output_is_a_session_id_without_an_exit_code() {
        use serde_json::json;
        let output = |body: Value| {
            json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": body.to_string()},
            ])
        };
        let running = output(json!({"wall_time_seconds": 1.0, "session_id": 7, "output": "a"}));
        assert_eq!(yielded_session(Some(&running)).as_deref(), Some("7"));
        let ended = output(
            json!({"wall_time_seconds": 1.0, "session_id": 7, "exit_code": 0, "output": "a"}),
        );
        assert_eq!(yielded_session(Some(&ended)), None);
        let printed = output(json!({"session_id": 7}));
        assert_eq!(yielded_session(Some(&printed)), None);
        let sixty_four =
            output(json!({"wall_time_seconds": 1.0, "session_id": "x".repeat(64), "output": ""}));
        assert_eq!(yielded_session(Some(&sixty_four)), Some("x".repeat(64)));
        let long =
            output(json!({"wall_time_seconds": 1.0, "session_id": "x".repeat(65), "output": ""}));
        assert_eq!(yielded_session(Some(&long)), None);
        assert_eq!(yielded_session(Some(&json!("done"))), None);
    }
}
