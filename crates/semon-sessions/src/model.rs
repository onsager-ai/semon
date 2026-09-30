//! The session model behind `/api/model` and `--model-json` (#40 M1): the
//! mockup's `SESS` and `H` globals, the turn index and busy intervals, built
//! from the per-file event index.
//!
//! Semon never guesses a link. Every join below is on a logged id: a
//! `toolUseId`, a `msg_id`, a `session_id`, a `continued-in` or
//! `bridgeSessionId` record, a Codex marker or `parent_thread_id`. Where the
//! logs don't say, the other end is a stub that says so.
//!
//! Text (briefs, results, answers) is read back from the source line by
//! offset and held in memory only, never in the index cache.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::Serialize;
use serde_json::Value;

use crate::{
    Marker, Options, attachments, codex_meta, events,
    events::{
        ACK, ANSWERED, ASYNC, DENIED, Event, EventCache, FileIndex, Kind, PIN, SEND_FAILED, UNKNOWN,
    },
    facts::{MachineFacts, ReportedModelUsage, ReportedRunSnapshot},
    field, file_list, handoff, read_first_marker,
};

/// Version of the `/api/model` JSON shape. Embedders and clients compare it;
/// bump it only for a breaking change to that shape.
pub const MODEL_API: u32 = 1;

/// Briefs and results are capped as in the mockup data.
pub(crate) const MSG_MAX: usize = 4096;
const TRUNCATED: &str = "\n…(truncated)";
/// A tool call without a result counts as running for this long.
const LIVE_MS: i64 = 30 * 60 * 1000;
/// A single source line read back for text is bounded by this.
pub(crate) const MAX_LINE: u64 = 64 * 1024 * 1024;

// ---- Output shapes: the mockup's field names ---------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Session {
    pub(crate) name: String,
    pub(crate) harness: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<&'static str>,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) lane: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) stub: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) role: bool,
    pub(crate) machine: String,
    pub(crate) state: &'static str,
    pub(crate) model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<String>,
    pub(crate) tokens: [f64; 3],
    pub(crate) tokens_by_model: BTreeMap<String, events::ModelTokens>,
    pub(crate) cost: crate::pricing::Cost,
    pub(crate) reported_runs: Vec<ReportedRun>,
    pub(crate) cost_check: Vec<CostCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rate_limits: Option<events::RateLimits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    pub(crate) start: i64,
    pub(crate) last: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// A running tool: `[name, argument, age in seconds when served, start
    /// in epoch ms]`. A client that keeps a model across 304s computes the
    /// age from `activity[3]`.
    pub(crate) activity: Option<(String, String, i64, i64)>,
    pub(crate) busy: Vec<(i64, i64)>,
    /// The transcript's tool calls, and those that failed or never
    /// finished: the same numbers `/api/tx` reports, so a page needn't fetch
    /// a transcript to count. Absent for a session with no transcript.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) calls: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) errors: Option<usize>,
    /// The transcript's tool calls by tool name, counted from the same
    /// slots as `calls` (so the counts add up to it): at most
    /// [`TOOL_KINDS_MAX`] names, the rest in one `other` bucket, each name at
    /// most [`TOOL_NAME_MAX`] characters. Absent when there are none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tool_calls: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReportedRun {
    pub(crate) start: i64,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) api_ms: Option<u64>,
    pub(crate) tool_ms: Option<u64>,
    pub(crate) lines_added: Option<u64>,
    pub(crate) lines_removed: Option<u64>,
    pub(crate) by_model: BTreeMap<String, ReportedModelUsage>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CostCheck {
    pub(crate) start: i64,
    pub(crate) computed_usd: Option<f64>,
    pub(crate) reported_usd: Option<f64>,
    pub(crate) ok: Option<bool>,
}

/// One answered question: the values you picked or typed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Answer {
    pub(crate) question: String,
    pub(crate) values: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) multi: bool,
    /// A value that isn't one of the question's option labels.
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) free: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preview: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Handoff {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ask: Option<&'static str>,
    pub(crate) from: String,
    /// `null` for a failed send: it has no receiver.
    pub(crate) to: Option<String>,
    /// A failed send's recipient, as it was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    pub(crate) at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) done: Option<i64>,
    pub(crate) status: &'static str,
    pub(crate) brief: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<String>,
    /// One string per question, in the brief's order (the mockup's shape).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answer: Option<Vec<String>>,
    /// The same answers, structured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answers: Option<Vec<Answer>>,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) declined: bool,
    /// The other end isn't in these logs: a stub stands in for it.
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) unmatched: bool,
    /// The logs don't say which call spawned this run (several calls or
    /// runs share its task name): the spawn isn't placed on a call.
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) ambiguous: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct End {
    pub(crate) st: &'static str,
    pub(crate) why: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) h: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Turn {
    pub(crate) id: String,
    pub(crate) sid: String,
    pub(crate) start: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) u: bool,
    /// A `u` turn's prompt (not a handoff, so no brief carries it), capped
    /// like briefs: Home, search and traces show it without a transcript.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    pub(crate) at: Option<i64>,
    pub(crate) end: End,
    pub(crate) sent: Vec<String>,
    pub(crate) file: String,
    pub(crate) offset: u64,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) last: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Machine {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) up: bool,
    /// When an offline machine was last seen (epoch ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last: Option<i64>,
}

/// What the agent read surface (`semon query`, `semon mcp`) says about a
/// session beyond `/api/model`'s shape: facts the builder already holds but
/// the viewer doesn't serve. Metadata only, never served by `/api/model`, so
/// the model's JSON and version don't change with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SessionFacts {
    /// `session`, `subagent`, `codex-run` or `stub`.
    pub(crate) kind: &'static str,
    /// The session that spawned this one, by a logged id (a spawn handoff's
    /// `from`).
    pub(crate) parent: Option<String>,
    /// The process a record names for it: a Claude `sessions/<pid>.json`, or
    /// the holder of a Codex writer lock.
    pub(crate) pid: Option<u32>,
    /// Whether that process runs: `true` when its start time matches (or its
    /// writer lock is held), `false` when the record's process is gone,
    /// `None` when there is no process record to check.
    pub(crate) alive: Option<bool>,
    /// The status the Claude pid file recorded (`busy`, `idle`, `shell`, `waiting`, …).
    pub(crate) recorded_status: Option<String>,
    /// The live process's run ids: the entries of its environment named in
    /// [`crate::RUN_VARIABLES`], nothing else. Empty when it has none;
    /// `None` when there is no live process or its environment couldn't be
    /// read whole.
    pub(crate) run: Option<BTreeMap<String, String>>,
    /// The earliest and latest log line's time (epoch ms); `None` without one.
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
    /// Exact token counts; `None` for a stub, which has no logs.
    pub(crate) tokens: Option<crate::Tokens>,
    /// A log line names an earlier file of this session (its `session_id`)
    /// that the scan window didn't read: the turns in it are missing.
    pub(crate) turns_truncated: bool,
}

#[derive(Serialize)]
struct Rest<'a> {
    pricing: &'a crate::pricing::Pricing,
    handoffs: &'a [Handoff],
    turns: &'a [Turn],
    busy: &'a BTreeMap<String, Vec<(i64, i64)>>,
    /// Each transcript's growth mark, `<entries>.<bytes>.<running>`: its slot
    /// count, the length of the files its slots come from, and how many of
    /// its calls are running. The page tails a loaded transcript only when
    /// its mark moved.
    tx: &'a BTreeMap<&'a str, String>,
}

/// A built model. Everything but the sessions' running-tool age is
/// serialized once; `version` hashes the build and every source file's
/// length, so it changes when, and only when, the logs do.
pub(crate) struct Built {
    pub(crate) version: String,
    machine: String,
    /// `activity[2]` holds the running tool's start (epoch ms) until served.
    pub(crate) sessions: BTreeMap<String, Session>,
    /// `"handoffs":…,"turns":…,"busy":…,"tx":…}`
    rest: String,
    pub(crate) pids: Vec<u32>,
    /// `$HOME` of the machine the logs come from, for shortening paths.
    pub(crate) home: Option<String>,
    /// This machine's id, as sessions name it.
    pub(crate) machine_id: String,
    /// Every source file, by position: what transcript slots point into.
    pub(crate) files: Vec<SlotFile>,
    /// Each session's transcript index, for `/api/tx`: metadata and offsets
    /// only, the text is read back per page.
    pub(crate) tx: BTreeMap<String, Transcript>,
    /// Each session's facts for the agent read surface, by session key.
    pub(crate) facts: BTreeMap<String, SessionFacts>,
    /// Where the scan window started (epoch ms): files last modified before
    /// it weren't read. `None` when every file was.
    pub(crate) window_start: Option<i64>,
    /// What `/api/analytics` reads of each session active in the last
    /// [`crate::analytics::KEEP_MS`], taken before the model is trimmed to
    /// its window. Never served by `/api/model`: its JSON and version don't
    /// change with it.
    pub(crate) activity: BTreeMap<String, crate::analytics::Activity>,
    #[cfg(test)]
    pub(crate) handoffs: Vec<Handoff>,
    #[cfg(test)]
    pub(crate) turns: Vec<Turn>,
    /// Each session's assistant texts by turn: (turn number, offset, block).
    #[cfg(test)]
    pub(crate) texts: BTreeMap<String, Vec<(usize, u64, u32)>>,
}

impl Built {
    /// The model as served at `now`: a running tool's `activity[2]` is its
    /// age in seconds, and a tool without a result for [`LIVE_MS`] is no
    /// longer shown as running.
    pub(crate) fn json(&self, now: i64) -> String {
        let mut sessions = self.sessions.clone();
        for session in sessions.values_mut() {
            session.activity = session
                .activity
                .take()
                .filter(|(_, _, _, start)| now - start < LIVE_MS)
                .map(|(name, arg, _, start)| (name, arg, (now - start).max(0) / 1000, start));
        }
        let sessions = serde_json::to_string(&sessions).expect("serializable sessions");
        format!(
            "{{\"api\":{},\"version\":\"{}\",\"now\":{now},\"machine\":{},\"sessions\":{sessions},{}",
            MODEL_API, self.version, self.machine, self.rest
        )
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Handoff {
    fn new(
        id: String,
        kind: &'static str,
        (from, to): (String, String),
        at: i64,
        status: &'static str,
        brief: &str,
    ) -> Self {
        Self {
            id,
            kind,
            ask: None,
            from,
            to: Some(to),
            target: None,
            at,
            done: None,
            status,
            brief: cap(brief, MSG_MAX),
            result: None,
            answer: None,
            answers: None,
            declined: false,
            unmatched: false,
            ambiguous: false,
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    // CI's browser suite serves a fixture whose clock must stand still. Only
    // a debug build with the `test-clock` feature reads this: without the
    // feature, or in a release build, the variable is ignored.
    #[cfg(all(feature = "test-clock", debug_assertions))]
    if let Some(now) = std::env::var("SEMON_TEST_NOW")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
    {
        return now;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| i64::try_from(time.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(crate) fn fnv(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn stable_id(prefix: &str, source: &str) -> String {
    format!("{prefix}{:016x}", fnv(source))
}

/// The prefix of an ask's id: your message to a session. No other handoff
/// id starts with it.
const ASK_PREFIX: &str = "a";

/// Whether `id` is an ask's: its transcript slot sits on your message's line.
pub(crate) fn is_ask(id: &str) -> bool {
    id.strip_prefix(ASK_PREFIX)
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Writes the model for `options` as JSON. Each file's change to the event
/// index is committed as it is read.
pub fn model_json(options: &Options) -> io::Result<String> {
    model_json_at(options, now_ms())
}

/// [`model_json`] at a fixed `now` (epoch ms), for fixtures.
#[doc(hidden)]
pub fn model_json_at(options: &Options, now: i64) -> io::Result<String> {
    let mut cache = EventCache::open(&options.cache);
    let mut dirty = false;
    if options.facts.is_none() {
        cache.refresh_reported_runs(&options.claude_json, now, &mut dirty);
    }
    let mut texts = Texts::default();
    let built = build(options, &mut cache, &mut dirty, &mut texts, now)?;
    Ok(built.json(now))
}

// ---- Text read back by offset, held in memory only ---------------------------------------

/// What the in-memory text cache may hold, in bytes.
const TEXT_BUDGET: usize = 32 * 1024 * 1024;
/// Every text read back is capped to this before it is kept.
const TEXT_MAX: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    path: PathBuf,
    generation: u64,
    offset: u64,
    block: u32,
    what: String,
}

/// Memoized reads of source lines, bounded by [`TEXT_BUDGET`] and evicted
/// least recently used first. A line at an offset stays valid while its file
/// only grows (the event cache's own rule); a replaced, truncated or
/// rewritten file starts a new generation, so stale text is never returned.
/// The most working directories whose repository [`Texts`] remembers.
const REPO_CACHE_MAX: usize = 512;

#[derive(Default)]
pub(crate) struct Texts {
    memo: HashMap<TextKey, (Option<String>, u64)>,
    bytes: usize,
    tick: u64,
    generations: HashMap<PathBuf, (Stamp, u64)>,
    next_generation: u64,
    repos: HashMap<String, Option<String>>,
    markers: HashMap<PathBuf, (Stamp, Option<Marker>)>,
    /// Each Codex file's `session_meta` and each subagent's `.meta.json`, by
    /// the file's stamp: a rebuild after one file grew re-reads neither for
    /// the files that didn't change.
    metas: HashMap<PathBuf, (Stamp, Option<Value>)>,
}

#[cfg(test)]
thread_local! {
    /// Metadata files read by [`Texts::meta`] on this thread: a rebuild reads
    /// only the changed ones.
    pub(crate) static META_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Builds on this thread.
    pub(crate) static BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Runs once right after the scan read the files: a test appends there,
    /// as a writer would while a build runs.
    pub(crate) static AFTER_SCAN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn read_line(path: &Path, offset: u64) -> Option<Value> {
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut reader = BufReader::new(file).take(MAX_LINE);
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

impl Texts {
    fn generation(&mut self, path: &Path, stamp: Stamp) -> u64 {
        if let Some((seen, number)) = self.generations.get_mut(path)
            && seen.dev == stamp.dev
            && seen.ino == stamp.ino
            && (stamp.size > seen.size
                || (stamp.size == seen.size && stamp.modified_ns == seen.modified_ns))
        {
            *seen = stamp;
            return *number;
        }
        self.next_generation += 1;
        self.generations
            .insert(path.to_owned(), (stamp, self.next_generation));
        self.next_generation
    }

    fn read(
        &mut self,
        file: &SourceFile,
        offset: u64,
        block: u32,
        what: &str,
        extract: impl FnOnce(&Value, usize) -> Option<String>,
    ) -> Option<String> {
        let key = TextKey {
            path: file.path.clone(),
            generation: self.generation(&file.path, file.stamp),
            offset,
            block,
            what: what.to_owned(),
        };
        self.tick += 1;
        if let Some((value, used)) = self.memo.get_mut(&key) {
            *used = self.tick;
            return value.clone();
        }
        let value = read_line(&file.path, offset)
            .and_then(|record| extract(&record, block as usize))
            .map(|text| {
                // `json:` values are built capped, and must stay whole.
                if what.starts_with("json:") {
                    text
                } else {
                    cap(&text, TEXT_MAX)
                }
            });
        self.bytes += value.as_ref().map_or(0, String::len) + key.what.len() + 96;
        self.memo.insert(key, (value.clone(), self.tick));
        if self.bytes > TEXT_BUDGET {
            self.evict();
        }
        value
    }

    /// Drops the least recently used texts down to three quarters of the
    /// budget.
    fn evict(&mut self) {
        let mut entries: Vec<(u64, TextKey)> = self
            .memo
            .iter()
            .map(|(key, (_, used))| (*used, key.clone()))
            .collect();
        entries.sort_by_key(|(used, _)| *used);
        for (_, key) in entries {
            if self.bytes <= TEXT_BUDGET / 4 * 3 {
                break;
            }
            if let Some((value, _)) = self.memo.remove(&key) {
                self.bytes = self
                    .bytes
                    .saturating_sub(value.map_or(0, |text| text.len()) + key.what.len() + 96);
            }
        }
    }

    fn repo(&mut self, cwd: &str) -> Option<String> {
        if let Some(repo) = self.repos.get(cwd) {
            return repo.clone();
        }
        let repo = repo_of(cwd, crate::facts::env_home().as_deref().map(Path::new));
        // Bounded: a full cache is dropped whole and refilled as the
        // working directories are met again.
        if self.repos.len() >= REPO_CACHE_MAX {
            self.repos.clear();
        }
        self.repos.insert(cwd.to_owned(), repo.clone());
        repo
    }

    /// A Codex run's marker, re-read only when the file changed.
    fn marker(&mut self, path: &Path, stamp: Stamp) -> Option<Marker> {
        if let Some((seen, marker)) = self.markers.get(path)
            && *seen == stamp
        {
            return marker.clone();
        }
        let marker = read_first_marker(path).ok().flatten();
        self.markers
            .insert(path.to_owned(), (stamp, marker.clone()));
        marker
    }

    /// A file's metadata, read by `read` only when its stamp changed. A Codex
    /// file that only grew keeps its first line, so a `session_meta` already
    /// read is kept (the event cache's rule); one not yet complete is read
    /// again. A read error isn't kept, so the file is tried again next time.
    fn meta(
        &mut self,
        path: &Path,
        grows: bool,
        read: impl FnOnce(&Path) -> io::Result<Option<Value>>,
    ) -> io::Result<Option<Value>> {
        let stamp = stamp_of(path);
        if let Some((seen, meta)) = self.metas.get_mut(path)
            && (*seen == stamp
                || (grows
                    && meta.is_some()
                    && seen.dev == stamp.dev
                    && seen.ino == stamp.ino
                    && stamp.size > seen.size))
        {
            *seen = stamp;
            return Ok(meta.clone());
        }
        #[cfg(test)]
        META_READS.with(|reads| reads.set(reads.get() + 1));
        let meta = read(path)?;
        self.metas.insert(path.to_owned(), (stamp, meta.clone()));
        Ok(meta)
    }
}

pub(crate) fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", text[..end].trim_end())
}

pub(crate) fn one_line(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= limit {
        return text;
    }
    let mut short: String = text.chars().take(limit.saturating_sub(1)).collect();
    short.push('…');
    short
}

pub(crate) fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match field(part, "type") {
                Some("text") => field(part, "text").map(str::to_owned),
                Some("image") => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A prompt's text: a Claude user record, a queued command or a Codex user
/// message, with reminders removed. Images it attaches aren't text: the
/// transcript shows them apart ([`attachments::refs`]), so the harness's
/// `[Image #N]` placeholders for them, and Codex's `<image>` frames, go too.
pub(crate) fn prompt_text(record: &Value) -> Option<String> {
    let images = attachments::parts(record).is_some_and(attachments::has_image);
    let placeholders = |text: String| {
        if images {
            attachments::strip_placeholders(&text)
        } else {
            text
        }
    };
    if let Some(attachment) = record.get("attachment") {
        let text = match attachment.get("prompt")? {
            Value::Array(parts) => parts
                .iter()
                .filter(|part| field(part, "type").is_none_or(|kind| kind == "text"))
                .filter_map(|part| field(part, "text"))
                .collect::<Vec<_>>()
                .join("\n"),
            other => content_text(other),
        };
        return Some(placeholders(events::clean_prompt(&text)));
    }
    if let Some(message) = record.get("message") {
        let text = match message.get("content")? {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|block| field(block, "type") == Some("text"))
                .filter_map(|block| field(block, "text"))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => return None,
        };
        return Some(placeholders(events::clean_prompt(&text)));
    }
    let payload = record.get("payload")?;
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| field(part, "text"))
        .filter(|text| !(images && attachments::is_image_tag(text)))
        .collect::<String>();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("Semon-Parent:") && !line.starts_with("Semon-Handoff:"))
        .collect();
    Some(placeholders(kept.join("\n").trim().to_owned()))
}

fn block_of(record: &Value, block: usize) -> Option<&Value> {
    record
        .get("message")?
        .get("content")?
        .as_array()?
        .get(block)
}

pub(crate) fn assistant_text(record: &Value, block: usize) -> Option<String> {
    if let Some(item) = block_of(record, block) {
        return field(item, "text").map(|text| text.trim().to_owned());
    }
    let payload = record.get("payload")?;
    Some(
        payload
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|part| field(part, "text"))
            .collect::<String>()
            .trim()
            .to_owned(),
    )
}

/// A tool call's input: a Claude `tool_use` block, or a Codex call's
/// arguments parsed as JSON.
pub(crate) fn tool_input(record: &Value, block: usize) -> Option<Value> {
    if let Some(item) = block_of(record, block) {
        return item.get("input").cloned();
    }
    let payload = record.get("payload")?;
    match payload.get("arguments").or_else(|| payload.get("input"))? {
        Value::String(text) => serde_json::from_str(text)
            .ok()
            .or_else(|| Some(Value::String(text.clone()))),
        other => Some(other.clone()),
    }
}

fn input_string(record: &Value, block: usize, keys: &[&str]) -> Option<String> {
    let input = tool_input(record, block)?;
    keys.iter()
        .find_map(|key| field(&input, key).map(str::to_owned))
}

fn origin_of(record: &Value) -> Option<&Value> {
    record
        .get("origin")
        .filter(|origin| origin.is_object())
        .or_else(|| {
            record
                .get("attachment")
                .and_then(|attachment| attachment.get("origin"))
                .filter(|origin| origin.is_object())
        })
}

/// The body of a relay or hand-back: `origin.body`, else the tag's text.
fn received_body(record: &Value, tag: &str, from: Option<&str>) -> Option<String> {
    if let Some(body) = origin_of(record).and_then(|origin| field(origin, "body")) {
        return Some(body.trim().to_owned());
    }
    let text = if let Some(payload) = record.get("payload") {
        payload
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| field(part, "text"))
                    .collect::<String>()
            })
            .unwrap_or_default()
    } else {
        record
            .get("attachment")
            .and_then(|attachment| attachment.get("prompt"))
            .or_else(|| {
                record
                    .get("message")
                    .and_then(|message| message.get("content"))
            })
            .map(content_text)
            .unwrap_or_default()
    };
    if record.get("payload").is_some() {
        return Some(text.trim().to_owned());
    }
    let close = format!("</{tag}>");
    for (attributes, end) in events::tags(&text, tag) {
        if from.is_some()
            && events::attributes(attributes)
                .get("from")
                .map(String::as_str)
                != from
        {
            continue;
        }
        let rest = &text[end..];
        let body = rest.find(&close).map_or(rest, |close| &rest[..close]);
        return Some(body.trim().to_owned());
    }
    None
}

fn notification_result(record: &Value, id: &str) -> Option<String> {
    let text = record
        .get("attachment")
        .and_then(|attachment| attachment.get("prompt"))
        .or_else(|| {
            record
                .get("message")
                .and_then(|message| message.get("content"))
        })
        .map(content_text)?;
    events::notifications(&text)
        .into_iter()
        .find(|(tool, _, _)| *tool == Some(id))
        .map(|(_, _, body)| {
            events::inner(body, "result")
                .or_else(|| events::inner(body, "summary"))
                .unwrap_or("")
                .to_owned()
        })
}

pub(crate) fn tool_result_text(record: &Value, block: usize) -> Option<String> {
    block_of(record, block)
        .and_then(|item| item.get("content"))
        .map(content_text)
}

pub(crate) use crate::repo::repo_of;

fn pretty_model(model: &str) -> Option<String> {
    if model.is_empty() || model.starts_with('<') {
        return None;
    }
    let mut model = model.strip_prefix("claude-").unwrap_or(model).to_owned();
    if let Some((head, date)) = model.rsplit_once('-')
        && date.len() == 8
        && date.bytes().all(|byte| byte.is_ascii_digit())
    {
        model = head.to_owned();
    }
    if let Some((head, minor)) = model.rsplit_once('-')
        && !minor.is_empty()
        && minor.bytes().all(|byte| byte.is_ascii_digit())
        && let Some((_, major)) = head.rsplit_once('-')
        && !major.is_empty()
        && major.bytes().all(|byte| byte.is_ascii_digit())
    {
        model = format!("{head}.{minor}");
    }
    Some(model)
}

/// A relay recipient or sender name without the harness's ` [..]` suffix.
fn plain_name(name: &str) -> String {
    name.split_once(" [")
        .map_or(name, |(head, _)| head)
        .trim()
        .to_owned()
}

/// This machine's hostname, as `/proc` (or `/etc/hostname`) gives it.
pub(crate) fn local_hostname(options: &Options) -> String {
    fs::read_to_string(options.proc_root.join("sys/kernel/hostname"))
        .or_else(|_| fs::read_to_string("/etc/hostname"))
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

// ---- Scan: every file's metadata summary --------------------------------------------------

#[derive(Default)]
struct AgentMeta {
    parent: String,
    tool_use_id: Option<String>,
    label: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
}

#[derive(Default)]
struct CodexMeta {
    parent_thread: Option<String>,
    nickname: Option<String>,
    path: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
}

enum Role {
    Top { slug: String },
    Agent(AgentMeta),
    Codex(CodexMeta),
}

struct SourceFile {
    path: PathBuf,
    stamp: Stamp,
    id: String,
    role: Role,
    summary: Arc<FileIndex>,
    first: Option<i64>,
    last: Option<i64>,
    /// The Codex `Semon-Parent` marker; never cached (it can carry a path).
    marker: Option<Marker>,
}

impl SourceFile {
    fn harness(&self) -> &'static str {
        if matches!(self.role, Role::Codex(_)) {
            "codex"
        } else {
            "claude"
        }
    }
}

pub(crate) struct PidFile {
    pub(crate) pid: u32,
    session: String,
    pub(crate) alive: bool,
    /// The process start time the file records.
    pub(crate) start: Option<u64>,
    status: Option<String>,
    /// What the process is waiting on you for, when it says so
    /// (`input needed`, `permission prompt`): the record's `waitingFor`.
    waiting_for: Option<String>,
    name: Option<String>,
}

/// A file's identity and version: text read back from it is valid while
/// the file only grows, as the event cache assumes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified_ns: u128,
}

fn stamp_of(path: &Path) -> Stamp {
    let Ok(meta) = fs::metadata(path) else {
        return Stamp::default();
    };
    #[cfg(unix)]
    let (dev, ino) = (meta.dev(), meta.ino());
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    Stamp {
        dev,
        ino,
        size: meta.len(),
        modified_ns: meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos()),
    }
}

pub(crate) fn pid_files(options: &Options, machine: &MachineFacts) -> Vec<PidFile> {
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir(options.claude_home.join("sessions")) else {
        return result;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Only `<pid>.json`: `*.key` files are never opened.
        let Some(pid) = name
            .strip_suffix(".json")
            .filter(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(record) = crate::read_regular(&entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object)
        else {
            continue;
        };
        let Some(session) = field(&record, "sessionId") else {
            continue;
        };
        let start = record
            .get("procStart")
            .and_then(Value::as_u64)
            .or_else(|| field(&record, "procStart").and_then(|value| value.parse().ok()));
        result.push(PidFile {
            pid,
            session: session.to_owned(),
            alive: start.is_some() && machine.proc_start(options, pid) == start,
            start,
            status: field(&record, "status").map(str::to_owned),
            waiting_for: field(&record, "waitingFor")
                .filter(|reason| !reason.is_empty())
                .map(str::to_owned),
            name: field(&record, "name").map(str::to_owned),
        });
    }
    result.sort_by_key(|pid| pid.pid);
    result
}

/// Every working directory the logs name, for [`crate::Facts::repos`]: each
/// file's own, and each subagent's and Codex run's metadata's. A superset of
/// the ones the builder looks a repository up for.
pub(crate) fn working_dirs(
    options: &Options,
    cache: &mut EventCache,
    dirty: &mut bool,
) -> io::Result<BTreeSet<String>> {
    let cutoff = scan_cutoff(options, now_ms());
    let (files, _) = scan(options, cache, dirty, &mut Texts::default(), cutoff)?;
    let mut cwds = BTreeSet::new();
    for file in &files {
        cwds.extend(file.summary.cwd.clone());
        match &file.role {
            Role::Agent(meta) => cwds.extend(meta.cwd.clone()),
            Role::Codex(meta) => cwds.extend(meta.cwd.clone()),
            Role::Top { .. } => {}
        }
    }
    Ok(cwds)
}

/// Where a scan window starts at `now` (epoch ms): `None` when every file
/// is read ([`Options::scan_window`] off, or `all`).
pub(crate) fn scan_cutoff(options: &Options, now: i64) -> Option<i64> {
    (options.scan_window && !options.all)
        .then(|| now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX)))
}

/// A file's modification time (epoch ms).
fn modified_ms(path: &Path) -> Option<i64> {
    let time = fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Every source file, with its event index. A file last modified before
/// `cutoff` isn't read at all: its cached index is kept (it counts as seen)
/// for builds that read every file. Also returns the ids of the top-level
/// Claude files so skipped.
fn scan(
    options: &Options,
    cache: &mut EventCache,
    dirty: &mut bool,
    texts: &mut Texts,
    cutoff: Option<i64>,
) -> io::Result<(Vec<SourceFile>, BTreeSet<String>)> {
    cache.begin_scan();
    let projects = options.claude_home.join("projects");
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut skipped = BTreeSet::new();
    let outside = |path: &Path| {
        cutoff.is_some_and(|cutoff| modified_ms(path).is_some_and(|modified| modified < cutoff))
    };
    let mut paths = Vec::new();
    file_list(&projects, &mut paths, "jsonl")?;
    for path in paths {
        if outside(&path) {
            seen.insert(path.to_string_lossy().into_owned());
            if path.parent().and_then(Path::parent) == Some(projects.as_path())
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                skipped.insert(stem.to_owned());
            }
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let parent = path.parent();
        let role = if parent.and_then(Path::parent) == Some(projects.as_path()) {
            Role::Top {
                slug: parent
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            }
        } else if parent
            .and_then(Path::file_name)
            .is_some_and(|name| name == "subagents")
            && stem.starts_with("agent-")
        {
            let meta = texts
                .meta(&path.with_extension("meta.json"), false, |meta| {
                    Ok(crate::read_regular(meta)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .filter(Value::is_object))
                })
                .ok()
                .flatten()
                .unwrap_or(Value::Null);
            Role::Agent(AgentMeta {
                parent: parent
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                tool_use_id: field(&meta, "toolUseId").map(str::to_owned),
                label: field(&meta, "description")
                    .or_else(|| field(&meta, "agentType"))
                    .map(str::to_owned),
                model: field(&meta, "model").map(str::to_owned),
                cwd: field(&meta, "worktreePath").map(str::to_owned),
                branch: field(&meta, "worktreeBranch").map(str::to_owned),
            })
        } else {
            continue;
        };
        let id = match &role {
            Role::Agent(_) => stem.strip_prefix("agent-").unwrap_or(stem),
            _ => stem,
        };
        // Stamped before it is read, so a line that lands during the read
        // counts toward the next version, not this one.
        let stamp = stamp_of(&path);
        let Ok(summary) = events::scan_file(&path, "claude", cache, dirty) else {
            continue;
        };
        seen.insert(path.to_string_lossy().into_owned());
        files.push(SourceFile {
            stamp,
            id: id.to_owned(),
            first: summary.first,
            last: summary.last,
            marker: None,
            path,
            role,
            summary,
        });
    }
    let mut paths = Vec::new();
    file_list(&options.codex_home.join("sessions"), &mut paths, "jsonl")?;
    for path in paths {
        if outside(&path) {
            seen.insert(path.to_string_lossy().into_owned());
            continue;
        }
        let Ok(meta) = texts.meta(&path, true, codex_meta) else {
            continue;
        };
        let Some(id) = meta
            .as_ref()
            .and_then(|meta| field(meta, "id"))
            .map(str::to_owned)
            .or_else(|| crate::codex_id_from_filename(&path))
        else {
            continue;
        };
        let stamp = stamp_of(&path);
        let Ok(summary) = events::scan_file(&path, "codex", cache, dirty) else {
            continue;
        };
        seen.insert(path.to_string_lossy().into_owned());
        let meta = meta.unwrap_or(Value::Null);
        files.push(SourceFile {
            marker: texts.marker(&path, stamp),
            stamp,
            id,
            first: summary.first,
            last: summary.last,
            role: Role::Codex(CodexMeta {
                parent_thread: field(&meta, "parent_thread_id").map(str::to_owned),
                nickname: field(&meta, "agent_nickname").map(str::to_owned),
                path: field(&meta, "agent_path").map(str::to_owned),
                cwd: field(&meta, "cwd").map(str::to_owned),
                branch: meta
                    .get("git")
                    .and_then(|git| field(git, "branch"))
                    .map(str::to_owned),
            }),
            path,
            summary,
        });
    }
    cache.retain(&seen, dirty);
    cache.end_scan();
    // Metadata of files that are gone is dropped with them.
    texts.metas.retain(|path, _| {
        let file = if path.extension().is_some_and(|ext| ext == "json") {
            path.with_extension("").with_extension("jsonl")
        } else {
            path.clone()
        };
        seen.contains(file.to_string_lossy().as_ref())
    });
    Ok((files, skipped))
}

// ---- Lineages: exact links only (M0, D4, D5) ---------------------------------------------

#[derive(Default)]
struct Unions {
    parent: HashMap<String, String>,
}

impl Unions {
    fn find(&mut self, key: &str) -> String {
        let mut root = key.to_owned();
        while let Some(next) = self.parent.get(&root) {
            if *next == root {
                break;
            }
            root = next.clone();
        }
        let mut current = key.to_owned();
        while let Some(next) = self.parent.get(&current).cloned() {
            if next == root {
                break;
            }
            self.parent.insert(current, root.clone());
            current = next;
        }
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            self.parent.insert(high, low);
        }
    }
}

/// Groups top-level Claude files into lineages: `session_id`, `continued-in`
/// and the transcripts' own `bridge-session` records, nothing else. A pid
/// file's `bridgeSessionId` is not used: it disappears with the process, and
/// a lineage's key must not change when it does. Each lineage's files are ordered by
/// their first timestamp.
fn lineages(files: &[SourceFile]) -> Vec<Vec<usize>> {
    let mut unions = Unions::default();
    for file in files {
        let Role::Top { .. } = file.role else {
            continue;
        };
        for other in &file.summary.links.session_ids {
            unions.union(&file.id, other);
        }
        for other in &file.summary.links.continued_in {
            unions.union(&file.id, other);
        }
        for bridge in &file.summary.links.bridges {
            unions.union(&file.id, &format!("bridge:{bridge}"));
        }
    }
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (position, file) in files.iter().enumerate() {
        if matches!(file.role, Role::Top { .. }) {
            groups
                .entry(unions.find(&file.id))
                .or_default()
                .push(position);
        }
    }
    let mut result: Vec<Vec<usize>> = groups.into_values().collect();
    for group in &mut result {
        // A copy-resume's new file starts with the old file's lines, so on a
        // tie the file that was continued elsewhere comes first.
        group.sort_by_key(|file| {
            let file = &files[*file];
            (
                file.first.unwrap_or(i64::MAX),
                file.summary.links.continued_in.is_empty(),
                file.id.clone(),
            )
        });
    }
    result
}

// ---- The builder -------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessKind {
    Lineage,
    Agent,
    Codex,
    Stub,
}

struct Sess {
    key: String,
    kind: SessKind,
    files: Vec<usize>,
    alive: bool,
    busy: bool,
    /// The process is stopped on a dialog only you can answer.
    waiting: bool,
    names: BTreeSet<String>,
    refs: Vec<Ref>,
    parent: Option<usize>,
    /// Every pid file that names this lineage: (pid, alive, status, start).
    pid_files: Vec<(u32, bool, Option<String>, Option<u64>)>,
    /// The earliest and latest line's time, when any line has one.
    first: Option<i64>,
    last: Option<i64>,
    tokens: crate::Tokens,
    out: Session,
}

type Ref = (usize, usize);

struct H {
    out: Handoff,
    from: Option<usize>,
    to: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    In,
    Out,
}

struct Builder<'a> {
    files: &'a [SourceFile],
    texts: &'a mut Texts,
    now: i64,
    machine: String,
    facts: &'a MachineFacts,
    reported_runs: &'a [ReportedRunSnapshot],
    home: Option<String>,
    sessions: Vec<Sess>,
    of_file: Vec<usize>,
    by_key: HashMap<String, usize>,
    handoffs: Vec<H>,
    placed: HashMap<Ref, Vec<(usize, Place)>>,
    head: HashMap<usize, usize>,
    after: HashMap<Ref, usize>,
    tools: HashMap<String, Ref>,
    ids: HashMap<String, usize>,
    /// Events a copy-resume duplicated: skipped everywhere.
    copied: BTreeSet<Ref>,
}

fn event(files: &[SourceFile], at: Ref) -> &Event {
    &files[at.0].summary.events[at.1]
}

impl<'a> Builder<'a> {
    fn new(
        files: &'a [SourceFile],
        texts: &'a mut Texts,
        now: i64,
        machine: String,
        facts: &'a MachineFacts,
        reported_runs: &'a [ReportedRunSnapshot],
    ) -> Self {
        Self {
            files,
            texts,
            now,
            machine,
            home: facts.home(),
            facts,
            reported_runs,
            sessions: Vec::new(),
            of_file: vec![usize::MAX; files.len()],
            by_key: HashMap::new(),
            handoffs: Vec::new(),
            placed: HashMap::new(),
            head: HashMap::new(),
            after: HashMap::new(),
            tools: HashMap::new(),
            ids: HashMap::new(),
            copied: BTreeSet::new(),
        }
    }

    fn blank(&self, name: String, harness: &'static str) -> Session {
        Session {
            name,
            harness,
            kind: None,
            lane: false,
            stub: false,
            role: false,
            machine: self.machine.clone(),
            state: "done",
            model: "—".into(),
            effort: None,
            tokens: [0.0; 3],
            tokens_by_model: BTreeMap::new(),
            cost: crate::pricing::Cost::default(),
            reported_runs: Vec::new(),
            cost_check: Vec::new(),
            rate_limits: None,
            parent: None,
            repo: None,
            branch: None,
            start: 0,
            last: 0,
            activity: None,
            busy: Vec::new(),
            calls: None,
            errors: None,
            tool_calls: BTreeMap::new(),
        }
    }

    fn add_session(
        &mut self,
        key: String,
        kind: SessKind,
        files: Vec<usize>,
        out: Session,
    ) -> usize {
        let index = self.sessions.len();
        for file in &files {
            self.of_file[*file] = index;
        }
        self.by_key.insert(key.clone(), index);
        self.sessions.push(Sess {
            key,
            kind,
            files,
            alive: false,
            busy: false,
            waiting: false,
            names: BTreeSet::new(),
            refs: Vec::new(),
            parent: None,
            pid_files: Vec::new(),
            first: None,
            last: None,
            tokens: crate::Tokens::default(),
            out,
        });
        index
    }

    fn stub(&mut self, source: &str, name: &str) -> usize {
        let key = stable_id("peer-", source);
        if let Some(index) = self.by_key.get(&key) {
            return *index;
        }
        let mut out = self.blank(
            if name.is_empty() {
                "unknown".into()
            } else {
                name.to_owned()
            },
            "claude",
        );
        out.lane = true;
        out.stub = true;
        out.role = true;
        self.add_session(key, SessKind::Stub, Vec::new(), out)
    }

    fn text(
        &mut self,
        at: Ref,
        what: &str,
        extract: impl FnOnce(&Value, usize) -> Option<String>,
    ) -> Option<String> {
        let file = &self.files[at.0];
        let event = &file.summary.events[at.1];
        self.texts.read(file, event.o, event.b, what, extract)
    }

    fn result_text(
        &mut self,
        at: Ref,
        what: &str,
        extract: impl FnOnce(&Value, usize) -> Option<String>,
    ) -> Option<String> {
        let file = &self.files[at.0];
        let reply = file.summary.events[at.1].r.as_ref()?;
        self.texts.read(file, reply.o, reply.b, what, extract)
    }

    /// A session's events in order.
    fn events_of(&self, session: usize) -> Vec<Ref> {
        self.sessions[session].refs.clone()
    }

    /// Orders every session's events once. A lineage's files usually follow
    /// one another, but processes sharing a Remote Control session (D5) can
    /// write at the same time, so the files are merged by time; an event
    /// without a time keeps its place after the one before it.
    fn order_events(&mut self) {
        let copied = self.copied_events();
        for session in &mut self.sessions {
            let mut keyed = Vec::new();
            for (order, file) in session.files.iter().enumerate() {
                let events = &self.files[*file].summary.events;
                // Events before a file's first timestamp take that timestamp.
                let mut time = events.iter().find_map(|event| event.t).unwrap_or(i64::MIN);
                for (at, event) in events.iter().enumerate() {
                    time = event.t.unwrap_or(time);
                    if copied.contains(&(*file, at)) {
                        continue;
                    }
                    keyed.push(((time, order, at), (*file, at)));
                }
            }
            if session.files.len() > 1 {
                keyed.sort_by_key(|(key, _)| *key);
            }
            session.refs = keyed.into_iter().map(|(_, at)| at).collect();
        }
        self.copied = copied;
    }

    /// A copy-resume (`continued-in`) starts the new file with the old
    /// file's lines. Those copies are the new file's events whose record
    /// `uuid` (else tool id or `msg_id`) the old file already has. Only such
    /// pairs are compared: files of one bridge written at the same time never
    /// share a uuid, so nothing else is de-duplicated.
    fn copied_events(&self) -> BTreeSet<Ref> {
        let mut pairs = Vec::new();
        for session in &self.sessions {
            for source in &session.files {
                for target in &session.files {
                    if source != target
                        && self.files[*source]
                            .summary
                            .links
                            .continued_in
                            .contains(&self.files[*target].id)
                    {
                        pairs.push((*source, *target));
                    }
                }
            }
        }
        // From the index alone: each event carries its record's uuid.
        let key = |event: &Event| {
            event.u.clone().or_else(|| match event.k {
                Kind::Tool | Kind::Xsm => event.id.clone(),
                _ => None,
            })
        };
        let mut copied = BTreeSet::new();
        for (source, target) in pairs {
            let seen: BTreeSet<String> = self.files[source]
                .summary
                .events
                .iter()
                .filter_map(key)
                .collect();
            for (at, event) in self.files[target].summary.events.iter().enumerate() {
                if key(event).is_some_and(|key| seen.contains(&key)) {
                    copied.insert((target, at));
                }
            }
        }
        copied
    }

    /// Adds a handoff. Ids are derived from source ids and must be unique: a
    /// duplicate is a bug, caught here in debug builds and by the tests'
    /// invariant check, never silently merged.
    fn add_handoff(&mut self, handoff: Handoff, from: Option<usize>, to: Option<usize>) -> usize {
        debug_assert!(
            !self.ids.contains_key(&handoff.id),
            "duplicate handoff id {}",
            handoff.id
        );
        self.ids.insert(handoff.id.clone(), self.handoffs.len());
        self.handoffs.push(H {
            out: handoff,
            from,
            to,
        });
        self.handoffs.len() - 1
    }

    fn place(&mut self, at: Ref, handoff: usize, place: Place) {
        self.placed.entry(at).or_default().push((handoff, place));
    }

    fn root(&self, mut session: usize) -> usize {
        let mut seen = BTreeSet::new();
        while let Some(parent) = self.sessions[session].parent {
            if !seen.insert(session) {
                break;
            }
            session = parent;
        }
        session
    }

    /// Every session's facts for the agent read surface. `codex_process`
    /// gives a Codex run's writer-lock holder and whether it runs;
    /// `run_of` a live process's run ids, given the start time its record
    /// names, if any.
    fn session_facts(
        &self,
        skipped: &BTreeSet<String>,
        codex_process: impl Fn(&str) -> (Option<u32>, Option<bool>),
        run_of: impl Fn(u32, Option<u64>) -> Option<BTreeMap<String, String>>,
    ) -> BTreeMap<String, SessionFacts> {
        self.sessions
            .iter()
            .map(|session| {
                let (kind, pid, alive, recorded_status, run) = match session.kind {
                    SessKind::Lineage => {
                        let live = session.pid_files.iter().find(|(_, alive, _, _)| *alive);
                        match (live, session.pid_files.as_slice()) {
                            (Some((pid, _, status, start)), _) => (
                                "session",
                                Some(*pid),
                                Some(true),
                                status.clone(),
                                run_of(*pid, *start),
                            ),
                            // One pid file, and its process is gone.
                            (None, [(pid, _, status, _)]) => {
                                ("session", Some(*pid), Some(false), status.clone(), None)
                            }
                            // No process record: nothing to check.
                            (None, []) => ("session", None, None, None, None),
                            // Several, all gone: which one ran last isn't logged.
                            (None, _) => ("session", None, Some(false), None, None),
                        }
                    }
                    // A subagent runs in its parent's process: no record of
                    // its own, and no run of its own.
                    SessKind::Agent => ("subagent", None, None, None, None),
                    SessKind::Codex => {
                        let (pid, alive) = codex_process(&session.key);
                        let run = pid
                            .filter(|_| alive == Some(true))
                            .and_then(|pid| run_of(pid, None));
                        ("codex-run", pid, alive, None, run)
                    }
                    SessKind::Stub => ("stub", None, None, None, None),
                };
                let facts = SessionFacts {
                    kind,
                    parent: session
                        .parent
                        .map(|parent| self.sessions[parent].key.clone()),
                    pid,
                    alive,
                    recorded_status,
                    run,
                    first: session.first,
                    last: session.last,
                    tokens: (session.kind != SessKind::Stub).then(|| session.tokens.clone()),
                    turns_truncated: session.files.iter().any(|file| {
                        self.files[*file]
                            .summary
                            .links
                            .session_ids
                            .iter()
                            .any(|id| skipped.contains(id))
                    }),
                };
                (session.key.clone(), facts)
            })
            .collect()
    }

    // -- sessions --

    fn sessions(&mut self, lineages: Vec<Vec<usize>>, pids: &[PidFile], held: &BTreeSet<String>) {
        for group in lineages {
            let first = group[0];
            let key = self.files[first].id.clone();
            let out = self.blank(String::new(), "claude");
            let index = self.add_session(key, SessKind::Lineage, group, out);
            self.sessions[index].out.lane = true;
        }
        for (position, file) in self.files.iter().enumerate() {
            match &file.role {
                Role::Agent(meta) => {
                    let mut out = self.blank(
                        one_line(meta.label.as_deref().unwrap_or("Subagent"), 80),
                        "claude",
                    );
                    out.kind = Some("Subagent");
                    self.add_session(file.id.clone(), SessKind::Agent, vec![position], out);
                }
                Role::Codex(meta) => {
                    let mut out = self.blank(String::new(), "codex");
                    out.kind = Some("Codex run");
                    out.name = meta.nickname.clone().unwrap_or_default();
                    let index =
                        self.add_session(file.id.clone(), SessKind::Codex, vec![position], out);
                    self.sessions[index].alive = held.contains(&file.id);
                    self.sessions[index].busy = self.sessions[index].alive;
                }
                Role::Top { .. } => {}
            }
        }
        for pid in pids {
            let Some(&index) = self
                .files
                .iter()
                .position(|file| matches!(file.role, Role::Top { .. }) && file.id == pid.session)
                .map(|file| &self.of_file[file])
            else {
                continue;
            };
            let session = &mut self.sessions[index];
            session
                .pid_files
                .push((pid.pid, pid.alive, pid.status.clone(), pid.start));
            if let Some(name) = &pid.name {
                session.names.insert(plain_name(name));
            }
            if pid.alive {
                session.alive = true;
                // `shell` (a finished turn with background shells running)
                // is not busy here: it stays idle rather than working for as
                // long as a shell lives.
                session.busy = pid.status.as_deref() == Some("busy");
                session.waiting =
                    pid.status.as_deref() == Some("waiting") || pid.waiting_for.is_some();
                if let Some(name) = &pid.name {
                    session.out.name = name.clone();
                }
            }
        }
        self.order_events();
        for index in 0..self.sessions.len() {
            self.describe(index);
        }
    }

    /// Name, model, effort, tokens, repo, branch, start, last and busy.
    fn describe(&mut self, index: usize) {
        let files: Vec<&SourceFile> = self.sessions[index]
            .files
            .iter()
            .map(|file| &self.files[*file])
            .collect();
        let Some(last_file) = files.last() else {
            return;
        };
        let harness = last_file.harness();
        let mut tokens = crate::Tokens::default();
        let mut busy = Vec::new();
        let mut model = None;
        let effort = files.iter().rev().find_map(|file| {
            file.summary
                .signals
                .iter()
                .filter(|signal| signal.k == events::SignalKind::Effort)
                .max_by_key(|signal| signal.o)
                .and_then(|signal| signal.n.as_deref())
        });
        let effort = effort
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let mut start = i64::MAX;
        let mut last = 0;
        let mut names = BTreeSet::new();
        let mut title = None;
        let mut tokens_by_model = BTreeMap::new();
        let mut rate_limits: Option<events::RateLimits> = None;
        // Claude usage is merged by message id across the lineage's files, so
        // a copy-resume's copied messages count once.
        let mut usage = BTreeMap::new();
        for file in &files {
            if harness == "claude" {
                usage.extend(
                    file.summary
                        .usage()
                        .iter()
                        .map(|(id, used)| (id.clone(), used.clone())),
                );
            } else {
                let used = file.summary.tokens(harness);
                tokens.input += used.input;
                tokens.cached_input += used.cached_input;
                tokens.output += used.output;
                for (model, usage) in file.summary.codex_usage() {
                    tokens_by_model
                        .entry(model.clone())
                        .or_insert_with(events::ModelTokens::default)
                        .add(usage);
                }
                if let Some(latest) = &file.summary.rate_limits
                    && rate_limits
                        .as_ref()
                        .is_none_or(|current| latest.recorded_at >= current.recorded_at)
                {
                    rate_limits = Some(latest.clone());
                }
            }
            busy.extend(file.summary.busy.iter().copied());
            if let Some(found) = &file.summary.last_model {
                model = Some(found.clone());
            }
            start = start.min(file.first.unwrap_or(i64::MAX));
            last = last.max(file.last.unwrap_or(0));
            for name in [&file.summary.title, &file.summary.agent_name]
                .into_iter()
                .flatten()
            {
                names.insert(plain_name(name));
            }
            if file.summary.title.is_some() {
                title.clone_from(&file.summary.title);
            } else if title.is_none() && file.summary.agent_name.is_some() {
                title.clone_from(&file.summary.agent_name);
            }
        }
        for used in usage.values() {
            tokens.input += used.tokens.input;
            tokens.cached_input += used.tokens.cached_input;
            tokens.output += used.tokens.output;
            // A message that names no model still counts, under the key the
            // cost table gives it, so the rows always sum to `tokens`.
            let model =
                crate::pricing::normalize_model_id(used.model.as_deref().unwrap_or("unknown"))
                    .to_owned();
            tokens_by_model
                .entry(model)
                .or_insert_with(events::ModelTokens::default)
                .add(&used.model_tokens);
        }
        let million = |value: u64| (value as f64 / 1e6 * 1000.0).round() / 1000.0;
        let (cwd, branch, fallback_model, fallback_name) = match &last_file.role {
            Role::Top { slug } => (
                last_file.summary.cwd.clone(),
                last_file.summary.branch.clone(),
                None,
                slug.rsplit('-').next().unwrap_or(slug).to_owned(),
            ),
            Role::Agent(meta) => (
                meta.cwd.clone().or_else(|| last_file.summary.cwd.clone()),
                meta.branch
                    .clone()
                    .or_else(|| last_file.summary.branch.clone()),
                meta.model.clone(),
                String::new(),
            ),
            Role::Codex(meta) => (
                meta.cwd.clone().or_else(|| last_file.summary.cwd.clone()),
                meta.branch
                    .clone()
                    .or_else(|| last_file.summary.branch.clone()),
                Some("codex".to_owned()),
                String::new(),
            ),
        };
        let repo = cwd.as_deref().and_then(|cwd| {
            self.facts
                .recorded_repo(cwd)
                .unwrap_or_else(|| self.texts.repo(cwd))
        });
        let billing_messages: Vec<events::MessageUsage> = usage.values().cloned().collect();
        let codex_events: Vec<events::CodexUsageEvent> = if harness == "codex" {
            files
                .iter()
                .flat_map(|file| file.summary.codex_usage_events().iter().cloned())
                .collect()
        } else {
            Vec::new()
        };
        let cost = crate::pricing::calculate_cost(&billing_messages, &codex_events);
        let mut reports: Vec<&ReportedRunSnapshot> = if harness == "claude" {
            self.reported_runs
                .iter()
                .filter(|run| {
                    files
                        .iter()
                        .any(|file| file.id.as_str() == run.last_session_id.as_str())
                })
                .collect()
        } else {
            Vec::new()
        };
        reports.sort_by_key(|run| run.last_start_time);
        let mut reported_runs = Vec::with_capacity(reports.len());
        let mut cost_check = Vec::with_capacity(reports.len());
        for run in reports {
            let bounded_messages: Vec<events::MessageUsage> = billing_messages
                .iter()
                .filter(|message| {
                    message.billing.timestamp.is_some_and(|timestamp| {
                        timestamp >= run.last_start_time && timestamp <= run.capture_at
                    })
                })
                .cloned()
                .collect();
            let computed = crate::pricing::calculate_cost(&bounded_messages, &[]).usd;
            reported_runs.push(ReportedRun {
                start: run.last_start_time,
                cost_usd: run.last_cost,
                duration_ms: run.last_duration,
                api_ms: run.last_api_duration,
                tool_ms: run.last_tool_duration,
                lines_added: run.last_lines_added,
                lines_removed: run.last_lines_removed,
                by_model: run.last_model_usage.clone(),
            });
            cost_check.push(CostCheck {
                start: run.last_start_time,
                computed_usd: computed,
                reported_usd: run.last_cost,
                ok: crate::pricing::cost_check_ok(computed, run.last_cost),
            });
        }
        let session = &mut self.sessions[index];
        session.names.extend(names);
        session.tokens = tokens.clone();
        session.first = (start != i64::MAX).then_some(start);
        session.last = (last != 0).then_some(last);
        let out = &mut session.out;
        out.tokens_by_model = tokens_by_model;
        out.cost = cost;
        out.reported_runs = reported_runs;
        out.cost_check = cost_check;
        out.rate_limits = rate_limits;
        out.tokens = [
            million(tokens.input.saturating_sub(tokens.cached_input)),
            million(tokens.cached_input),
            million(tokens.output),
        ];
        out.model = model
            .as_deref()
            .and_then(pretty_model)
            .or_else(|| fallback_model.as_deref().and_then(pretty_model))
            .unwrap_or_else(|| "—".into());
        out.effort = effort;
        out.start = if start == i64::MAX { self.now } else { start };
        out.last = if last == 0 { out.start } else { last };
        out.busy = events::busy_merge(busy);
        out.role = repo.is_none();
        out.branch = repo.as_ref().and(branch);
        out.repo = repo;
        if out.name.is_empty() {
            out.name = match session.kind {
                SessKind::Lineage => title.unwrap_or(fallback_name),
                _ => out
                    .branch
                    .clone()
                    .or_else(|| out.repo.clone())
                    .unwrap_or_else(|| "Codex run".into()),
            };
        }
    }

    fn index_tools(&mut self) {
        for (position, file) in self.files.iter().enumerate() {
            for (at, event) in file.summary.events.iter().enumerate() {
                if self.copied.contains(&(position, at)) {
                    continue;
                }
                if event.k == Kind::Tool
                    && let Some(id) = &event.id
                {
                    self.tools.entry(id.clone()).or_insert((position, at));
                }
            }
        }
    }

    fn find_in(&self, session: usize, id: &str) -> Option<Ref> {
        self.tools
            .get(id)
            .copied()
            .filter(|at| self.of_file[at.0] == session)
    }

    // -- spawns --

    fn claude_spawns(&mut self) {
        let mut notifications = HashMap::<String, Ref>::new();
        let mut handbacks = HashMap::<String, Ref>::new();
        for (position, file) in self.files.iter().enumerate() {
            for (at, event) in file.summary.events.iter().enumerate() {
                if self.copied.contains(&(position, at)) {
                    continue;
                }
                match (event.k, &event.id) {
                    (Kind::Tn, Some(id))
                        if !matches!(event.n.as_deref(), None | Some("running" | "started")) =>
                    {
                        notifications.insert(id.clone(), (position, at));
                    }
                    // A hand-back names its sender by `senderTaskId` (the
                    // agent id or the spawning call's id) and by `from`.
                    (Kind::Agm, Some(id)) if file.harness() == "claude" => {
                        handbacks.insert(id.clone(), (position, at));
                        if let Some(from) = &event.n {
                            handbacks.entry(from.clone()).or_insert((position, at));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut spawns = Vec::new();
        for (position, file) in self.files.iter().enumerate() {
            let Role::Agent(meta) = &file.role else {
                continue;
            };
            let child = self.of_file[position];
            let call = meta
                .tool_use_id
                .as_deref()
                .and_then(|id| self.tools.get(id).copied());
            let spawner = call.map(|at| self.of_file[at.0]).or_else(|| {
                self.files
                    .iter()
                    .position(|file| {
                        matches!(file.role, Role::Top { .. }) && file.id == meta.parent
                    })
                    .map(|file| self.of_file[file])
            });
            spawns.push((child, spawner, call, meta.tool_use_id.clone()));
        }
        for (child, spawner, _, _) in &spawns {
            self.sessions[*child].parent = *spawner;
            if spawner.is_none() {
                self.sessions[*child].out.lane = true;
            }
        }
        for (child, spawner, call, tool_use_id) in spawns {
            let Some(spawner) = spawner else {
                continue;
            };
            let child_events = self.events_of(child);
            let first_prompt = child_events
                .iter()
                .copied()
                .find(|at| event(self.files, *at).k == Kind::U);
            let mut brief = call.and_then(|at| {
                self.text(at, "prompt", |record, block| {
                    input_string(record, block, &["prompt"])
                })
            });
            if brief.is_none()
                && let Some(at) = first_prompt
            {
                brief = self.text(at, "user", |record, _| prompt_text(record));
            }
            let at = call
                .and_then(|at| event(self.files, at).t)
                .unwrap_or(self.sessions[child].out.start);
            let key = self.sessions[child].key.clone();
            let mut result = None;
            if let Some(found) = tool_use_id
                .as_deref()
                .and_then(|id| notifications.get(id))
                .copied()
            {
                let id = tool_use_id.clone().unwrap_or_default();
                let status = event(self.files, found).n.clone().unwrap_or_default();
                let text = self.text(found, &format!("tn:{id}"), |record, _| {
                    notification_result(record, &id)
                });
                let failed = !matches!(status.as_str(), "completed" | "success" | "done");
                result = Some((text, event(self.files, found).t, failed));
            }
            if result.is_none()
                && let Some(found) = handbacks
                    .get(&key)
                    .or_else(|| tool_use_id.as_deref().and_then(|id| handbacks.get(id)))
                    .copied()
            {
                let from = key.clone();
                let text = self.text(found, &format!("agm:{from}"), |record, _| {
                    // The sending agent's own body only: a batched record's
                    // first `<agent-message>` may be another agent's.
                    received_body(record, "agent-message", Some(&from))
                });
                result = Some((text, event(self.files, found).t, false));
            }
            if result.is_none()
                && let Some(at) = call
                && let Some(reply) = event(self.files, at).r.clone()
                && reply.f & ASYNC == 0
            {
                let text = self.result_text(at, "result", tool_result_text);
                result = Some((text, reply.t, reply.e));
            }
            if result.is_none()
                && let Some(at) = child_events.iter().rev().copied().find(|at| {
                    let event = event(self.files, *at);
                    event.k == Kind::Tool && event.n.as_deref() == Some("SubagentHandback")
                })
            {
                let text = self.text(at, "handback", |record, block| {
                    input_string(record, block, &["message"])
                });
                result = Some((text, event(self.files, at).t, false));
            }
            let root = self.root(spawner);
            let running = self.sessions[root].alive;
            let (status, state) = match &result {
                Some((_, _, true)) => ("err", "err"),
                Some(_) => ("done", "done"),
                None if running => ("work", "work"),
                None => ("done", "done"),
            };
            self.sessions[child].out.state = state;
            self.sessions[child].busy = state == "work";
            self.sessions[child].alive = state == "work";
            let handoff = Handoff {
                done: result.as_ref().and_then(|result| result.1),
                result: result.map(|(text, _, _)| cap(text.as_deref().unwrap_or(""), MSG_MAX)),
                ..Handoff::new(
                    stable_id("s", &format!("spawn:claude:{key}")),
                    "spawn",
                    (self.sessions[spawner].key.clone(), key),
                    at,
                    status,
                    brief.as_deref().unwrap_or("(brief not in these logs)"),
                )
            };
            let handoff = self.add_handoff(handoff, Some(spawner), Some(child));
            if let Some(at) = call {
                self.place(at, handoff, Place::Out);
            }
            match first_prompt {
                Some(at) => self.place(at, handoff, Place::In),
                None => {
                    self.head.insert(child, handoff);
                }
            }
        }
    }

    /// A Codex run's spawner, the call that launched it when that is
    /// exact, and whether the call is ambiguous.
    fn codex_spawner(&mut self, position: usize) -> (Option<usize>, Option<Ref>, bool) {
        let file = &self.files[position];
        let Role::Codex(meta) = &file.role else {
            return (None, None, false);
        };
        if let Some(parent) = &meta.parent_thread {
            let Some(&spawner) = self.by_key.get(parent) else {
                return (None, None, false);
            };
            let names: Vec<String> = [
                meta.path
                    .as_deref()
                    .and_then(|path| path.rsplit('/').next()),
                meta.nickname.as_deref(),
            ]
            .into_iter()
            .flatten()
            .map(str::to_owned)
            .collect();
            // Neither the child's rollout nor `spawn_agent`'s output (only
            // `{task_name}`) names the other side. So a call is placed only
            // when it is the spawner's one `spawn_agent` call for this task
            // name and this child is the one run of that name it spawned.
            // Otherwise the spawn is marked ambiguous and left unplaced.
            let calls: Vec<Ref> = self
                .events_of(spawner)
                .into_iter()
                .filter(|at| {
                    let event = event(self.files, *at);
                    event.k == Kind::Tool && event.n.as_deref() == Some("spawn_agent")
                })
                .collect();
            let mut matching = Vec::new();
            for at in calls {
                let task = self.text(at, "task_name", |record, block| {
                    input_string(record, block, &["task_name"])
                });
                if let Some(task) = task.filter(|task| names.contains(task)) {
                    matching.push((at, task));
                }
            }
            let siblings = |task: &str| {
                self.files
                    .iter()
                    .filter(|file| {
                        matches!(&file.role, Role::Codex(other)
                            if other.parent_thread.as_deref() == Some(parent.as_str())
                                && (other.nickname.as_deref() == Some(task)
                                    || other.path.as_deref().and_then(|path| path.rsplit('/').next()) == Some(task)))
                    })
                    .count()
            };
            return match matching.as_slice() {
                [] => (Some(spawner), None, false),
                [(at, task)] if siblings(task) == 1 => (Some(spawner), Some(*at), false),
                _ => (Some(spawner), None, true),
            };
        }
        let Some(marker) = file.marker.clone() else {
            return (None, None, false);
        };
        let Some(parent_file) = self.files.iter().position(|file| {
            matches!(file.role, Role::Top { .. } | Role::Agent(_))
                && file.harness() == "claude"
                && file.id == marker.claude_id
        }) else {
            return (None, None, false);
        };
        let spawner = self.of_file[parent_file];
        let tool = match (&marker.tool_id, &marker.handoff) {
            (Some(id), _) => Some(id.clone()),
            (None, Some(prompt)) => {
                self.handoff_tool(spawner, prompt, meta.cwd.as_deref(), file.first)
            }
            _ => None,
        };
        let call = tool.and_then(|id| self.find_in(spawner, &id));
        (Some(spawner), call, false)
    }

    /// Match through the same incremental parent scan used by the tree.
    fn handoff_tool(
        &self,
        spawner: usize,
        prompt: &str,
        cwd: Option<&str>,
        start: Option<i64>,
    ) -> Option<String> {
        let paths: Vec<PathBuf> = self.sessions[spawner]
            .files
            .iter()
            .map(|file| self.files[*file].path.clone())
            .collect();
        handoff::find(&paths, prompt, cwd, start).map(|tool| tool.id)
    }

    fn codex_spawns(&mut self) {
        let mut spawns = Vec::new();
        let mut runs: Vec<usize> = (0..self.files.len())
            .filter(|position| matches!(self.files[*position].role, Role::Codex(_)))
            .collect();
        runs.sort_by_key(|position| (self.files[*position].first, *position));
        for position in runs {
            let (spawner, call, ambiguous) = self.codex_spawner(position);
            spawns.push((self.of_file[position], spawner, call, ambiguous));
        }
        for (child, spawner, _, _) in &spawns {
            self.sessions[*child].parent = *spawner;
            if spawner.is_none() {
                self.sessions[*child].out.lane = true;
            }
        }
        for (child, spawner, call, ambiguous) in spawns {
            let failed = self.files[self.sessions[child].files[0]].summary.failed;
            let state = if self.sessions[child].alive {
                "work"
            } else if failed {
                "err"
            } else {
                "done"
            };
            self.sessions[child].out.state = state;
            let Some(spawner) = spawner else {
                continue;
            };
            let child_events = self.events_of(child);
            let first_prompt = child_events
                .iter()
                .copied()
                .find(|at| event(self.files, *at).k == Kind::U);
            let brief =
                first_prompt.and_then(|at| self.text(at, "user", |record, _| prompt_text(record)));
            let key = self.sessions[child].key.clone();
            let mut result = None;
            if let Role::Codex(meta) = &self.files[self.sessions[child].files[0]].role
                && let Some(path) = meta.path.clone()
            {
                let candidates: Vec<Ref> = self
                    .events_of(spawner)
                    .into_iter()
                    .chain(child_events.iter().copied())
                    .filter(|at| event(self.files, *at).k == Kind::Agm)
                    .collect();
                for at in candidates {
                    let author = self.text(at, "author", |record, _| {
                        field(&record["payload"], "author").map(str::to_owned)
                    });
                    if author.as_deref() == Some(path.as_str()) {
                        let text = self.text(at, "agent_message", |record, _| {
                            received_body(record, "agent-message", None)
                        });
                        result = Some((text, event(self.files, at).t));
                    }
                }
            }
            if result.is_none()
                && state != "work"
                && let Some(at) = child_events
                    .iter()
                    .rev()
                    .copied()
                    .find(|at| event(self.files, *at).k == Kind::A)
            {
                let text = self.text(at, "a", assistant_text);
                result = Some((text, Some(self.sessions[child].out.last)));
            }
            let handoff = Handoff {
                done: result.as_ref().and_then(|result| result.1),
                result: result.map(|(text, _)| cap(text.as_deref().unwrap_or(""), MSG_MAX)),
                ambiguous,
                ..Handoff::new(
                    stable_id("s", &format!("spawn:codex:{key}")),
                    "spawn",
                    (self.sessions[spawner].key.clone(), key),
                    call.and_then(|at| event(self.files, at).t)
                        .unwrap_or(self.sessions[child].out.start),
                    state,
                    brief.as_deref().unwrap_or("(brief not in these logs)"),
                )
            };
            let handoff = self.add_handoff(handoff, Some(spawner), Some(child));
            if let Some(at) = call {
                self.place(at, handoff, Place::Out);
            }
            match first_prompt {
                Some(at) => self.place(at, handoff, Place::In),
                None => {
                    self.head.insert(child, handoff);
                }
            }
        }
    }

    // -- relays --

    fn relays(&mut self) {
        let mut received = HashMap::<String, Vec<Ref>>::new();
        let mut unkeyed = Vec::new();
        for (position, file) in self.files.iter().enumerate() {
            if file.harness() != "claude" {
                continue;
            }
            for (at, event) in file.summary.events.iter().enumerate() {
                if self.copied.contains(&(position, at)) {
                    continue;
                }
                if event.k != Kind::Xsm {
                    continue;
                }
                match &event.id {
                    Some(id) => received.entry(id.clone()).or_default().push((position, at)),
                    None => unkeyed.push((position, at)),
                }
            }
        }
        let mut sends = Vec::new();
        for (position, file) in self.files.iter().enumerate() {
            if file.harness() != "claude" {
                continue;
            }
            for (at, event) in file.summary.events.iter().enumerate() {
                if self.copied.contains(&(position, at)) {
                    continue;
                }
                if event.k == Kind::Tool
                    && event.n.as_deref() == Some("SendMessage")
                    && let Some(reply) = &event.r
                    && reply.f & PIN == 0
                {
                    sends.push(((position, at), reply.clone()));
                }
            }
        }
        // D3's fallback: names learned only from exact joins and records.
        let mut names = HashMap::<String, BTreeSet<usize>>::new();
        for (index, session) in self.sessions.iter().enumerate() {
            if session.kind == SessKind::Lineage {
                for name in &session.names {
                    names.entry(name.clone()).or_default().insert(index);
                }
            }
        }
        let mut joined = BTreeSet::new();
        for (at, reply) in &sends {
            if let Some(id) = &reply.m
                && let Some(receiver) = received.get(id).and_then(|found| found.first())
                && let Some(name) = event(self.files, *receiver).n.as_deref()
            {
                let sender = self.root(self.of_file[at.0]);
                if self.sessions[sender].kind == SessKind::Lineage {
                    names.entry(plain_name(name)).or_default().insert(sender);
                }
            }
        }
        for (at, reply) in sends {
            let from = self.of_file[at.0];
            let tool_id = event(self.files, at).id.clone().unwrap_or_default();
            let message = self.text(at, "message", |record, block| {
                input_string(record, block, &["message", "content"])
            });
            let mut unmatched = false;
            let failed = reply.e || reply.f & (DENIED | SEND_FAILED) != 0;
            let source = match &reply.m {
                Some(id) => format!("relay:{id}"),
                None => format!("relay:tool:{tool_id}"),
            };
            let joined_at = reply
                .m
                .as_ref()
                .and_then(|id| received.get(id))
                .and_then(|found| found.first())
                .copied();
            let mut target = None;
            let (to, receiver) = if let Some(receiver) = joined_at {
                joined.extend(reply.m.clone());
                (Some(self.of_file[receiver.0]), Some(receiver))
            } else if failed {
                // A failed or denied send reached no one: no receiver.
                target = Some(self.send_target(at));
                (None, None)
            } else if reply.m.is_some() {
                let name = plain_name(&self.send_target(at));
                unmatched = true;
                (Some(self.stub(&format!("name:{name}"), &name)), None)
            } else {
                let name = plain_name(&self.send_target(at));
                let unique = names
                    .get(&name)
                    .filter(|found| found.len() == 1)
                    .and_then(|found| found.first().copied());
                if unique.is_none() {
                    unmatched = true;
                }
                (
                    Some(unique.unwrap_or_else(|| self.stub(&format!("name:{name}"), &name))),
                    None,
                )
            };
            let handoff = Handoff {
                to: to.map(|to| self.sessions[to].key.clone()),
                target,
                unmatched,
                ..Handoff::new(
                    stable_id("r", &source),
                    "relay",
                    (self.sessions[from].key.clone(), String::new()),
                    event(self.files, at)
                        .t
                        .unwrap_or(self.sessions[from].out.last),
                    if failed { "err" } else { "done" },
                    message.as_deref().unwrap_or(""),
                )
            };
            let handoff = self.add_handoff(handoff, Some(from), to);
            self.place(at, handoff, Place::Out);
            if let Some(receiver) = receiver {
                self.place(receiver, handoff, Place::In);
            }
        }
        // Received with no send in these logs: the sender is a stub keyed by
        // its process, or the sending subagent that `origin.from` names.
        let mut orphans: Vec<(String, Ref)> = received
            .iter()
            .filter(|(id, _)| !joined.contains(*id))
            .filter_map(|(id, found)| found.first().map(|at| (format!("relay:{id}"), *at)))
            .collect();
        orphans.extend(unkeyed.into_iter().map(|at| {
            let file = &self.files[at.0];
            (
                format!("relay:recv:{}:{}", file.id, event(self.files, at).o),
                at,
            )
        }));
        orphans.sort_by(|a, b| a.0.cmp(&b.0));
        for (source, at) in orphans {
            let to = self.of_file[at.0];
            let found = event(self.files, at).clone();
            let peer = found.peer.clone().unwrap_or_default();
            let name = found.n.as_deref().map(plain_name).unwrap_or_default();
            // A subagent's message to another subagent carries no msg_id;
            // its `origin.from` is the sending agent's id. Anything else is a
            // stub: a socket belongs to a process, not to whichever session
            // that process writes now, so a live pid file proves nothing.
            let from = peer
                .sock
                .as_deref()
                .and_then(|from| self.by_key.get(from).copied())
                .filter(|from| self.sessions[*from].kind == SessKind::Agent);
            let from = from.unwrap_or_else(|| {
                let identity = match (peer.pid, &peer.start, &peer.sock) {
                    (Some(pid), start, _) => {
                        format!("pid:{pid}:{}", start.clone().unwrap_or_default())
                    }
                    (None, _, Some(sock)) => format!("sock:{sock}"),
                    _ => format!("name:{name}"),
                };
                self.stub(&identity, &name)
            });
            let body = self.text(at, "body", |record, _| {
                received_body(record, "cross-session-message", None)
            });
            let handoff = Handoff {
                unmatched: self.sessions[from].kind == SessKind::Stub,
                ..Handoff::new(
                    stable_id("r", &source),
                    "relay",
                    (
                        self.sessions[from].key.clone(),
                        self.sessions[to].key.clone(),
                    ),
                    found.t.unwrap_or(self.sessions[to].out.start),
                    "done",
                    body.as_deref().unwrap_or(""),
                )
            };
            let handoff = self.add_handoff(handoff, Some(from), Some(to));
            self.place(at, handoff, Place::In);
        }
    }

    fn send_target(&mut self, at: Ref) -> String {
        self.text(at, "to", |record, block| {
            input_string(record, block, &["to", "recipient"])
        })
        .unwrap_or_default()
    }

    fn codex_relays(&mut self) {
        let mut sends = Vec::new();
        for (position, file) in self.files.iter().enumerate() {
            if file.harness() != "codex" {
                continue;
            }
            for (at, event) in file.summary.events.iter().enumerate() {
                if self.copied.contains(&(position, at)) {
                    continue;
                }
                if event.k == Kind::Tool
                    && matches!(event.n.as_deref(), Some("send_message" | "followup_task"))
                {
                    sends.push((position, at));
                }
            }
        }
        for at in sends {
            let from = self.of_file[at.0];
            let tree = self.root(from);
            let target = self
                .text(at, "target", |record, block| {
                    input_string(record, block, &["target"])
                })
                .unwrap_or_default();
            let message = self.text(at, "message", |record, block| {
                input_string(record, block, &["message"])
            });
            let matches: Vec<usize> = (0..self.sessions.len())
                .filter(|index| {
                    let session = &self.sessions[*index];
                    session.kind == SessKind::Codex
                        && self.root(*index) == tree
                        && matches!(
                            &self.files[session.files[0]].role,
                            Role::Codex(meta) if meta.path.as_deref() == Some(target.as_str())
                                || meta.nickname.as_deref() == Some(target.as_str())
                        )
                })
                .collect();
            let reply = event(self.files, at).r.clone();
            let failed = reply.as_ref().is_some_and(|reply| reply.e);
            let (to, unmatched) = match matches.as_slice() {
                [only] => (*only, false),
                _ => (self.stub(&format!("codex:{target}"), &target), true),
            };
            let call = event(self.files, at).id.clone().unwrap_or_default();
            let handoff = Handoff {
                unmatched,
                ..Handoff::new(
                    stable_id("r", &format!("relay:codex:{call}")),
                    "relay",
                    (
                        self.sessions[from].key.clone(),
                        self.sessions[to].key.clone(),
                    ),
                    event(self.files, at)
                        .t
                        .unwrap_or(self.sessions[from].out.last),
                    if failed { "err" } else { "done" },
                    message.as_deref().unwrap_or(""),
                )
            };
            let handoff = self.add_handoff(handoff, Some(from), Some(to));
            self.place(at, handoff, Place::Out);
        }
    }

    // -- your messages and messages to you --

    fn asks(&mut self) {
        for index in 0..self.sessions.len() {
            if self.sessions[index].kind != SessKind::Lineage {
                continue;
            }
            for at in self.events_of(index) {
                if event(self.files, at).k != Kind::Ask {
                    continue;
                }
                let text = self.text(at, "user", |record, _| prompt_text(record));
                let file = &self.files[at.0];
                let source = format!("ask:{}:{}", file.id, event(self.files, at).o);
                let handoff = Handoff::new(
                    stable_id(ASK_PREFIX, &source),
                    "ask",
                    ("you".into(), self.sessions[index].key.clone()),
                    event(self.files, at)
                        .t
                        .unwrap_or(self.sessions[index].out.start),
                    "done",
                    text.as_deref().unwrap_or(""),
                );
                let handoff = self.add_handoff(handoff, None, Some(index));
                self.place(at, handoff, Place::In);
            }
        }
    }

    fn questions(&mut self) {
        for index in 0..self.sessions.len() {
            if matches!(self.sessions[index].kind, SessKind::Stub) {
                continue;
            }
            let refs = self.events_of(index);
            for (position, at) in refs.iter().copied().enumerate() {
                let found = event(self.files, at).clone();
                if found.k != Kind::Tool {
                    continue;
                }
                let codex = matches!(
                    found.n.as_deref(),
                    Some("request_user_input" | "request_user_input_async")
                );
                if found.n.as_deref() != Some("AskUserQuestion") && !codex {
                    continue;
                }
                let questions: Vec<Question> = self
                    .text(at, "json:questions", |record, block| {
                        tool_input(record, block).map(|input| compact_questions(&input).to_string())
                    })
                    .and_then(|input| serde_json::from_str::<Value>(&input).ok())
                    .map(|input| questions_of(&input))
                    .unwrap_or_default();
                let brief = questions
                    .iter()
                    .map(|question| question.question.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let source = found
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", self.files[at.0].id, found.o));
                let mut handoff = Handoff {
                    ask: Some("question"),
                    ..Handoff::new(
                        stable_id("q", &format!("toyou:{source}")),
                        "toyou",
                        (self.sessions[index].key.clone(), "you".into()),
                        found.t.unwrap_or(self.sessions[index].out.last),
                        "done",
                        if brief.is_empty() {
                            "(question)"
                        } else {
                            &brief
                        },
                    )
                };
                match &found.r {
                    // Codex keeps no answer record. A question waits on you
                    // while its run is alive and nothing has followed it: no
                    // output for the call beyond an acknowledgement, and no
                    // later user message.
                    _ if codex => {
                        let reply = found.r.as_ref().filter(|reply| reply.f & ACK == 0);
                        let following = refs[position + 1..]
                            .iter()
                            .map(|later| event(self.files, *later))
                            .find(|later| later.k == Kind::U);
                        if self.sessions[index].alive && reply.is_none() && following.is_none() {
                            handoff.status = "wait";
                        }
                        // It was answered when its output says so, else when
                        // your next message came.
                        handoff.done = reply
                            .and_then(|reply| reply.t)
                            .or_else(|| following.and_then(|later| later.t));
                    }
                    Some(reply) => {
                        handoff.done = reply.t;
                        handoff.declined = reply.e || reply.f & DENIED != 0;
                        if !handoff.declined {
                            let raw = self.result_text(at, "json:answers", |record, block| {
                                Some(compact_answers(record, block).to_string())
                            });
                            let raw = raw
                                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                                .unwrap_or(Value::Null);
                            let answers = if reply.f & ANSWERED != 0 {
                                structured_answers(&questions, &raw["tur"])
                            } else {
                                text_answers(&questions, field(&raw, "text").unwrap_or(""))
                            };
                            handoff.answer = Some(
                                answers
                                    .iter()
                                    .map(|answer| cap(&answer.values.join(", "), MSG_MAX))
                                    .collect(),
                            );
                            handoff.answers = Some(answers);
                        } else {
                            handoff.answer = Some(Vec::new());
                        }
                    }
                    None => {
                        let root = self.root(index);
                        if self.sessions[root].alive {
                            handoff.status = "wait";
                        }
                    }
                }
                let handoff = self.add_handoff(handoff, Some(index), None);
                self.place(at, handoff, Place::Out);
            }
        }
    }

    /// Live top-level sessions: waiting on you (an open question or
    /// decision, or a dialog the process reports), working, or idle.
    fn lineage_states(&mut self) {
        for index in 0..self.sessions.len() {
            let session = &self.sessions[index];
            if session.kind != SessKind::Lineage {
                continue;
            }
            if !session.alive {
                self.sessions[index].out.state = "done";
                continue;
            }
            // Waiting on you outranks working: a session asking a question
            // or stopped on a permission prompt is still mid-turn, so its
            // process reads as busy (or as `waiting`, for a dialog).
            let open = session.waiting
                || self.handoffs.iter().any(|handoff| {
                    handoff.from == Some(index)
                        && handoff.out.kind == "toyou"
                        && matches!(handoff.out.ask, Some("question" | "decision"))
                        && handoff.out.status == "wait"
                });
            if open {
                self.sessions[index].out.state = "wait";
                continue;
            }
            if session.busy {
                self.sessions[index].out.state = "work";
                continue;
            }
            let refs = self.events_of(index);
            let last_a = refs
                .iter()
                .rposition(|at| event(self.files, *at).k == Kind::A);
            let last_ask = refs
                .iter()
                .rposition(|at| event(self.files, *at).k == Kind::Ask);
            match last_a {
                Some(position) if last_ask.is_none_or(|ask| ask < position) => {
                    let at = refs[position];
                    let text = self.text(at, "a", assistant_text);
                    let file = &self.files[at.0];
                    let found = event(self.files, at);
                    if self.reply_started_by_you(index, at) {
                        let handoff = Handoff {
                            ask: Some("result"),
                            ..Handoff::new(
                                stable_id(
                                    "q",
                                    &format!("toyou:result:{}:{}:{}", file.id, found.o, found.b),
                                ),
                                "toyou",
                                (self.sessions[index].key.clone(), "you".into()),
                                found.t.unwrap_or(self.sessions[index].out.last),
                                "new",
                                text.as_deref().unwrap_or(""),
                            )
                        };
                        let handoff = self.add_handoff(handoff, Some(index), None);
                        self.after.insert(at, handoff);
                    }
                    self.sessions[index].out.state = "idle";
                }
                _ => self.sessions[index].out.state = "idle",
            }
        }
        // The latest message you sent a working session is still in progress.
        for index in 0..self.sessions.len() {
            if self.sessions[index].out.state != "work" {
                continue;
            }
            if let Some(latest) = (0..self.handoffs.len())
                .filter(|handoff| {
                    self.handoffs[*handoff].to == Some(index)
                        && self.handoffs[*handoff].out.kind == "ask"
                })
                .max_by_key(|handoff| self.handoffs[*handoff].out.at)
            {
                self.handoffs[latest].out.status = "work";
            }
        }
    }

    /// A result belongs to a reply only when the existing turn grouping says
    /// it began with your message. Some incoming events aren't transcript
    /// entries; if one falls between that message and the reply, its turn
    /// start is ambiguous and no result is inferred.
    fn reply_started_by_you(&self, index: usize, reply: Ref) -> bool {
        let entries = self.entries(index);
        let Some(reply_entry) = entries
            .iter()
            .position(|entry| entry.at == Some(reply) && entry.kind == EntryKind::A)
        else {
            return false;
        };
        let groups = self.groups(index, &entries);
        let Some(group) = groups
            .iter()
            .find(|group| group.entries.contains(&reply_entry))
        else {
            return false;
        };
        let Some(start) = group
            .start
            .filter(|start| self.handoffs[*start].out.kind == "ask")
        else {
            return false;
        };
        let Some(start_at) = group
            .entries
            .iter()
            .find_map(|position| entries[*position].at)
        else {
            return false;
        };
        let refs = &self.sessions[index].refs;
        let Some(start_position) = refs.iter().position(|at| *at == start_at) else {
            return false;
        };
        let Some(reply_position) = refs.iter().position(|at| *at == reply) else {
            return false;
        };
        if start_position > reply_position {
            return false;
        }
        !refs[start_position..=reply_position].iter().any(|at| {
            let event = event(self.files, *at);
            matches!(event.k, Kind::Xsm | Kind::Agm | Kind::Tn)
                || event.k == Kind::Tool && event.n.as_deref() == Some("SubagentHandback")
        }) && self.handoffs[start].out.from == "you"
    }

    fn activity(&mut self) {
        for index in 0..self.sessions.len() {
            if self.sessions[index].out.state != "work" {
                continue;
            }
            // Only the last turn can still be running a tool.
            let entries = self.entries(index);
            let Some(at) = self.groups(index, &entries).last().and_then(|turn| {
                turn.entries.iter().rev().find_map(|position| {
                    let entry = &entries[*position];
                    matches!(entry.kind, EntryKind::Tool(ToolState::Pending))
                        .then_some(entry.at)
                        .flatten()
                })
            }) else {
                continue;
            };
            let found = event(self.files, at).clone();
            // The start time: the age and the 30-minute expiry are applied
            // when the model is served, so they never freeze between builds.
            let Some(time) = found.t else {
                continue;
            };
            // A command that outlived its yield runs on after its script.
            if found.y.is_some() {
                let arg = self
                    .text(at, "yield:cmd", |record, block| {
                        tool_input(record, block)
                            .and_then(|input| events::script_command(input.as_str()?))
                            .map(|command| one_line(&command, 160))
                    })
                    .unwrap_or_default();
                self.sessions[index].out.activity = Some(("exec_command".into(), arg, 0, time));
                continue;
            }
            let name = found.n.clone().unwrap_or_else(|| "tool".into());
            let cwd = self.files[at.0].summary.cwd.clone();
            let summary_name = name.clone();
            let home = self.home.clone();
            let arg = self
                .text(at, "arg", move |record, block| {
                    tool_input(record, block).map(|input| {
                        arg_summary(&summary_name, &input, cwd.as_deref(), home.as_deref())
                    })
                })
                .unwrap_or_default();
            // The age is filled when served, so `version` depends on the
            // logs alone.
            self.sessions[index].out.activity = Some((name, arg, 0, time));
        }
    }

    // -- turns --

    #[cfg(test)]
    fn texts_by_turn(&self) -> BTreeMap<String, Vec<(usize, u64, u32)>> {
        let mut result = BTreeMap::new();
        for (index, session) in self.sessions.iter().enumerate() {
            if session.kind == SessKind::Stub {
                continue;
            }
            let entries = self.entries(index);
            let texts: Vec<(usize, u64, u32)> = self
                .groups(index, &entries)
                .iter()
                .enumerate()
                .flat_map(|(number, turn)| {
                    turn.entries
                        .iter()
                        .map(|position| &entries[*position])
                        .filter(|entry| matches!(entry.kind, EntryKind::A))
                        .filter_map(|entry| entry.at)
                        .map(move |at| (number, event(self.files, at).o, event(self.files, at).b))
                        .collect::<Vec<_>>()
                })
                .collect();
            result.insert(session.key.clone(), texts);
        }
        result
    }

    /// Every session's turns, and its transcript index: the entries the
    /// turns were split from, with the transcript-only extras merged in.
    fn turns(&mut self) -> (Vec<Turn>, BTreeMap<String, Transcript>) {
        let mut turns = Vec::new();
        let mut transcripts = BTreeMap::new();
        for index in 0..self.sessions.len() {
            if self.sessions[index].kind == SessKind::Stub {
                transcripts.insert(
                    self.sessions[index].key.clone(),
                    self.stub_transcript(index),
                );
                continue;
            }
            let entries = self.entries(index);
            let (mut split, owners, prompts) = self.split(index, &entries);
            for (turn, at) in prompts {
                split[turn].text = self
                    .text(at, "user", |record, _| prompt_text(record))
                    .map(|text| cap(&text, MSG_MAX));
            }
            let slots = self.slots(index, &entries, &owners, &split);
            transcripts.insert(self.sessions[index].key.clone(), slots);
            turns.extend(split);
        }
        (turns, transcripts)
    }

    /// A stub has no transcript of its own: it shows the handoffs that name
    /// it, after a note that its activity isn't in these logs.
    fn stub_transcript(&self, index: usize) -> Transcript {
        let mut named: Vec<&H> = self
            .handoffs
            .iter()
            .filter(|handoff| handoff.from == Some(index) || handoff.to == Some(index))
            .collect();
        named.sort_by_key(|handoff| handoff.out.at);
        let mut slots = vec![Slot::new(SlotKind::NoActivity, None, 0, 0, None)];
        slots.extend(named.into_iter().map(|handoff| {
            Slot::new(
                SlotKind::H(handoff.out.id.clone()),
                None,
                0,
                0,
                Some(handoff.out.at),
            )
        }));
        Transcript::from_slots(slots)
    }

    /// The transcript index of one session: its entries in order, each
    /// thinking or harness marker after the entry before it in its file, and
    /// a return line when a spawned run handed back.
    fn slots(
        &self,
        index: usize,
        entries: &[Entry],
        owners: &[Option<usize>],
        turns: &[Turn],
    ) -> Transcript {
        let session = &self.sessions[index];
        let mut extras: BTreeMap<usize, std::collections::VecDeque<&Event>> = BTreeMap::new();
        let mut operation_parents = BTreeSet::new();
        for file in &session.files {
            for extra in &self.files[*file].summary.extras {
                if extra.k == Kind::Operation
                    && let Some(parent) = extra.parent
                {
                    operation_parents.insert((*file, parent));
                }
            }
        }
        for file in &session.files {
            extras.insert(*file, self.files[*file].summary.extras.iter().collect());
        }
        let mut slots = Vec::with_capacity(entries.len());
        let mut owner: Option<usize> = None;
        let mut started = BTreeSet::new();
        let turn_id = |turn: Option<usize>| turn.map(|turn| turns[turn].id.clone());
        let extra_slot =
            |event: &Event, file: usize, owner: Option<usize>, started: &mut BTreeSet<usize>| {
                let kind = match event.k {
                    Kind::Harness => SlotKind::Harness(event.n.clone().unwrap_or_default()),
                    Kind::Operation => {
                        let ok = event
                            .r
                            .as_ref()
                            .and_then(|reply| (reply.f & events::UNKNOWN == 0).then_some(!reply.e));
                        let script_offset = event
                            .parent
                            .and_then(|parent| self.files[file].summary.events.get(parent))
                            .filter(|parent| parent.k == Kind::Tool && parent.code_mode)
                            .map(|parent| parent.o);
                        SlotKind::Operation {
                            kind: event.n.clone().unwrap_or_default(),
                            ok,
                            script_offset,
                        }
                    }
                    _ => SlotKind::Think,
                };
                let mut slot = Slot::new(kind, Some(file), event.o, event.b, event.t);
                slot.turn = turn_id(owner);
                if event.k == Kind::Operation
                    && let Some(owner) = owner
                {
                    slot.first = started.insert(owner);
                }
                slot
            };
        for (position, entry) in entries.iter().enumerate() {
            if let Some(queue) = extras.get_mut(&entry.file) {
                let at = entry.pos;
                while queue.front().is_some_and(|extra| (extra.o, extra.b) < at) {
                    let extra = queue.pop_front().expect("front");
                    slots.push(extra_slot(extra, entry.file, owner, &mut started));
                }
            }
            owner = owners[position];
            if matches!(entry.kind, EntryKind::Operation(_)) {
                let operation = extras.get_mut(&entry.file).and_then(|queue| {
                    let position = queue
                        .iter()
                        .position(|extra| extra.k == Kind::Operation && extra.o == entry.offset)?;
                    queue.remove(position)
                });
                if let Some(operation) = operation {
                    slots.push(extra_slot(operation, entry.file, owner, &mut started));
                }
                continue;
            }
            // A code-mode call is drawn as its operations, unless it failed
            // or never finished: then it is also that step, after them.
            if matches!(
                entry.kind,
                EntryKind::Tool(ToolState::Ok | ToolState::Unknown)
            ) && entry.at.is_some_and(|at| operation_parents.contains(&at))
            {
                continue;
            }
            let block = entry.at.map_or(0, |at| event(self.files, at).b);
            let kind = match entry.kind {
                EntryKind::H(handoff) => SlotKind::H(self.handoffs[handoff].out.id.clone()),
                EntryKind::U => SlotKind::U,
                EntryKind::A => SlotKind::A,
                EntryKind::Gap => SlotKind::Gap,
                EntryKind::Tool(state) => {
                    let found = entry.at.map(|at| event(self.files, at));
                    let running = session.out.state == "work"
                        && owner.is_some_and(|turn| turn + 1 == turns.len() && turns[turn].last);
                    let shown = match state {
                        ToolState::Ok => Shown::Ok,
                        ToolState::Err => Shown::Err,
                        ToolState::Unknown => Shown::Unknown,
                        ToolState::Pending if running => Shown::Live,
                        ToolState::Pending => Shown::Unfinished,
                    };
                    let started = found
                        .and_then(|found| found.poll)
                        .and_then(|start| self.files[entry.file].summary.events.get(start))
                        .filter(|start| start.y.is_some());
                    let yielded = found.and_then(|found| found.y.as_deref());
                    match (found, yielded, started, entry.at) {
                        (Some(found), Some(yielded), _, _) => SlotKind::Yielded {
                            // Running while its session works and its
                            // session id is open, whichever turn started
                            // it: its polls may come in a later turn.
                            shown: match (state, entry.at) {
                                (ToolState::Pending, Some(at)) if self.live_yield(index, at) => {
                                    Shown::Live
                                }
                                (ToolState::Pending, _) => Shown::Unfinished,
                                _ => shown,
                            },
                            first: found.r.as_ref().map(|reply| reply.o),
                            polls: yielded.polls.clone(),
                            cut: yielded.cut,
                            done: yielded.done.clone(),
                        },
                        (Some(found), None, Some(started), Some(at))
                            if !operation_parents.contains(&at) =>
                        {
                            SlotKind::Sent {
                                shown,
                                reply: found.r.clone(),
                                start: started.o,
                                done: started
                                    .y
                                    .as_ref()
                                    .and_then(|yielded| yielded.done.as_ref())
                                    .map(|done| done.o),
                            }
                        }
                        _ => SlotKind::Tool {
                            shown,
                            name: found
                                .and_then(|found| found.n.clone())
                                .unwrap_or_else(|| "tool".into()),
                            reply: found.and_then(|found| found.r.clone()),
                            item: found.and_then(|found| found.item),
                        },
                    }
                }
                EntryKind::Operation(_) => unreachable!("operation slots are read from extras"),
            };
            let mut slot = Slot::new(kind, Some(entry.file), entry.offset, block, entry.t);
            slot.turn = turn_id(owner);
            slot.first = owner.is_some_and(|turn| started.insert(turn));
            slots.push(slot);
        }
        // Markers after the last entry, from every file, in time order.
        let mut rest: Vec<(usize, &Event)> = extras
            .into_iter()
            .flat_map(|(file, queue)| queue.into_iter().map(move |extra| (file, extra)))
            .collect();
        rest.sort_by_key(|(file, extra)| (extra.t, *file, extra.o, extra.b));
        for (file, extra) in rest {
            slots.push(extra_slot(extra, file, owner, &mut started));
        }
        if let Some(spawn) = self.handoffs.iter().find(|handoff| {
            handoff.to == Some(index)
                && handoff.out.kind == "spawn"
                && handoff.out.done.is_some()
                && matches!(handoff.out.status, "done" | "err")
        }) && let Some(parent) = spawn.from
        {
            let mut slot = Slot::new(
                SlotKind::Returned {
                    to: self.sessions[parent].key.clone(),
                    at: spawn.out.done,
                    failed: spawn.out.status == "err",
                },
                None,
                0,
                0,
                spawn.out.done,
            );
            slot.turn = turn_id(owner);
            slots.push(slot);
        }
        Transcript::from_slots(slots)
    }

    /// A session's entries in transcript order: its events, each handoff
    /// placed after the event it follows, and its Codex operations. Each
    /// entry's sort key is computed once, where it is made.
    fn entries(&self, index: usize) -> Vec<Entry> {
        let refs = self.events_of(index);
        let session_files = &self.sessions[index].files;
        let several = session_files.len() > 1;
        // Each file's event times; an event without one takes the time before
        // it, and events before a file's first time take that time.
        let mut event_times: HashMap<usize, Vec<i64>> = HashMap::new();
        for file in session_files {
            let events = &self.files[*file].summary.events;
            let mut time = events.iter().find_map(|event| event.t).unwrap_or(i64::MIN);
            let times = events
                .iter()
                .map(|found| {
                    time = found.t.unwrap_or(time);
                    time
                })
                .collect();
            event_times.insert(*file, times);
        }
        // The time of the last event at or before `offset` in `file`: events
        // are in line order, so a binary search finds it.
        let time_at = |file: usize, offset: u64| {
            let events = &self.files[file].summary.events;
            let before = events.partition_point(|found| found.o <= offset);
            event_times
                .get(&file)
                .and_then(|times| times.get(before.saturating_sub(1)))
                .copied()
                .unwrap_or(i64::MIN)
        };
        let file_order: HashMap<usize, usize> = session_files
            .iter()
            .enumerate()
            .map(|(order, file)| (*file, order))
            .collect();
        // Several files merge by time; within a file, line and block order.
        // Phase 2 puts a handoff after the event it follows.
        let key = |time: i64, file: usize, pos: (u64, u32), phase: u8| {
            (
                if several { time } else { 0 },
                *file_order.get(&file).unwrap_or(&usize::MAX),
                pos.0,
                pos.1,
                phase,
            )
        };
        // Code-mode calls drawn as their operations, and where the last of
        // each call's operations sits.
        let mut last_operation: HashMap<Ref, (i64, (u64, u32))> = HashMap::new();
        for file in session_files {
            for extra in &self.files[*file].summary.extras {
                if extra.k == Kind::Operation
                    && let Some(parent) = extra.parent
                {
                    let time = extra.t.unwrap_or_else(|| time_at(*file, extra.o));
                    last_operation.insert((*file, parent), (time, (extra.o, extra.b)));
                }
            }
        }
        let mut keyed = Vec::new();
        if let Some(handoff) = self.head.get(&index) {
            let (file, offset) = refs
                .first()
                .map(|at| (at.0, event(self.files, *at).o))
                .unwrap_or((self.sessions[index].files[0], 0));
            keyed.push((
                (i64::MIN, 0, 0, 0, 0),
                Entry {
                    kind: EntryKind::H(*handoff),
                    file,
                    offset,
                    pos: (offset, 0),
                    t: None,
                    at: None,
                },
            ));
        }
        for at in refs {
            let found = event(self.files, at);
            let time = event_times[&at.0][at.1];
            let kinds: Vec<EntryKind> = if let Some(placements) = self.placed.get(&at) {
                placements
                    .iter()
                    .map(|(handoff, _)| EntryKind::H(*handoff))
                    .collect()
            } else {
                match found.k {
                    Kind::U => Some(EntryKind::U),
                    Kind::A => Some(EntryKind::A),
                    Kind::Gap => Some(EntryKind::Gap),
                    // A poll of a yielded command folds into the step the
                    // script that started it is, unless it sent input.
                    Kind::Tool
                        if found.poll.is_some()
                            && found.script & events::SENDS == 0
                            && !last_operation.contains_key(&at) =>
                    {
                        None
                    }
                    Kind::Tool if found.n.as_deref() != Some("SubagentHandback") => {
                        Some(EntryKind::Tool(tool_state(found)))
                    }
                    _ => None,
                }
                .into_iter()
                .collect()
            };
            for kind in kinds {
                // A code-mode call drawn as its operations that failed, or
                // that never finished, is still a step: after them, where its
                // failure arrived or after the last of them.
                let (time, pos, phase) = match (&kind, &found.r, last_operation.get(&at)) {
                    (EntryKind::Tool(ToolState::Err), Some(reply), Some(_)) => (
                        reply.t.unwrap_or_else(|| time_at(at.0, reply.o)),
                        (reply.o, reply.b),
                        1,
                    ),
                    (EntryKind::Tool(ToolState::Pending), None, Some((time, pos))) => {
                        (*time, *pos, 2)
                    }
                    _ => (time, (found.o, found.b), 1),
                };
                keyed.push((
                    key(time, at.0, pos, phase),
                    Entry {
                        kind,
                        file: at.0,
                        offset: found.o,
                        pos,
                        t: found.t,
                        at: Some(at),
                    },
                ));
            }
            if let Some(handoff) = self.after.get(&at) {
                // The event's own place, so the handoff follows it even when
                // it is a later block of its line.
                keyed.push((
                    key(time, at.0, (found.o, found.b), 2),
                    Entry {
                        kind: EntryKind::H(*handoff),
                        file: at.0,
                        offset: found.o,
                        pos: (found.o, found.b),
                        t: found.t,
                        at: None,
                    },
                ));
            }
        }
        for file in session_files {
            for extra in &self.files[*file].summary.extras {
                if extra.k != Kind::Operation {
                    continue;
                }
                let state = match extra.r.as_ref() {
                    Some(reply) if reply.e => ToolState::Err,
                    Some(reply) if reply.f & UNKNOWN != 0 => ToolState::Unknown,
                    Some(_) => ToolState::Ok,
                    None => ToolState::Pending,
                };
                let time = extra.t.unwrap_or_else(|| time_at(*file, extra.o));
                keyed.push((
                    key(time, *file, (extra.o, extra.b), 1),
                    Entry {
                        kind: EntryKind::Operation(state),
                        file: *file,
                        offset: extra.o,
                        pos: (extra.o, extra.b),
                        t: extra.t,
                        at: None,
                    },
                ));
            }
        }
        // Stable, and linear when the entries are already in order (a
        // session without operations). It isn't skipped for one file: a
        // Codex file's operations come from its extras and must interleave.
        keyed.sort_by_key(|(key, _)| *key);
        keyed.into_iter().map(|(_, entry)| entry).collect()
    }

    /// The mockup's turn rule: a turn starts at each incoming entry (your
    /// message, a relay, a brief, or a user prompt) and a gap ends one.
    fn groups(&self, index: usize, entries: &[Entry]) -> Vec<Group> {
        let mut started = BTreeSet::new();
        let mut open: Vec<Group> = Vec::new();
        let mut current: Option<usize> = None;
        for (position, entry) in entries.iter().enumerate() {
            if matches!(entry.kind, EntryKind::Gap) {
                current = None;
                continue;
            }
            let incoming = match entry.kind {
                EntryKind::H(handoff) => {
                    let found = &self.handoffs[handoff];
                    found.to == Some(index)
                        && matches!(found.out.kind, "ask" | "relay" | "spawn")
                        && !started.contains(&handoff)
                }
                EntryKind::U => true,
                _ => false,
            };
            if incoming || current.is_none() {
                let start = match entry.kind {
                    EntryKind::H(handoff) if incoming => {
                        started.insert(handoff);
                        Some(handoff)
                    }
                    _ => None,
                };
                open.push(Group {
                    start,
                    u: incoming && start.is_none(),
                    file: entry.file,
                    offset: entry.offset,
                    at: start
                        .map(|handoff| self.handoffs[handoff].out.at)
                        .or(entry.t),
                    entries: Vec::new(),
                });
                current = Some(open.len() - 1);
            }
            open[current.expect("open turn")].entries.push(position);
        }
        open
    }

    /// Returns the turns, each entry's turn (by position in the returned
    /// list; `None` for entries of a turn with nothing in it), and the prompt
    /// event of each `u` turn.
    #[allow(clippy::type_complexity)]
    fn split(
        &self,
        index: usize,
        entries: &[Entry],
    ) -> (Vec<Turn>, Vec<Option<usize>>, Vec<(usize, Ref)>) {
        let sid = &self.sessions[index].key;
        let open = self.groups(index, entries);
        let count = open.len();
        let mut turns = Vec::new();
        let mut owners = vec![None; entries.len()];
        let mut prompts = Vec::new();
        for (number, turn) in open.into_iter().enumerate() {
            let last = number + 1 == count;
            let sent: Vec<usize> = turn
                .entries
                .iter()
                .filter_map(|position| match entries[*position].kind {
                    EntryKind::H(handoff)
                        if Some(handoff) != turn.start
                            && self.handoffs[handoff].from == Some(index) =>
                    {
                        Some(handoff)
                    }
                    _ => None,
                })
                .collect();
            let Some(end) = self.end(
                index,
                &turn.entries,
                entries,
                turn.start,
                turn.u,
                &sent,
                last,
            ) else {
                continue;
            };
            for position in &turn.entries {
                owners[*position] = Some(turns.len());
            }
            if turn.u
                && let Some(at) = turn
                    .entries
                    .first()
                    .and_then(|position| entries[*position].at)
            {
                prompts.push((turns.len(), at));
            }
            turns.push(Turn {
                id: match turn.start {
                    Some(handoff) => self.handoffs[handoff].out.id.clone(),
                    None => format!("{sid}:{}:{}", self.files[turn.file].id, turn.offset),
                },
                sid: sid.clone(),
                start: turn
                    .start
                    .map(|handoff| self.handoffs[handoff].out.id.clone()),
                u: turn.u,
                text: None,
                at: turn.at,
                end,
                sent: sent
                    .iter()
                    .map(|handoff| self.handoffs[*handoff].out.id.clone())
                    .collect(),
                file: self.files[turn.file].id.clone(),
                offset: turn.offset,
                last,
            });
        }
        (turns, owners, prompts)
    }

    /// How a turn ended, as the mockup's `turnEnd`. `None` for a turn with
    /// nothing in it.
    #[allow(clippy::too_many_arguments)]
    fn end(
        &self,
        index: usize,
        members: &[usize],
        entries: &[Entry],
        start: Option<usize>,
        u: bool,
        sent: &[usize],
        last: bool,
    ) -> Option<End> {
        let content: Vec<&Entry> = members
            .iter()
            .map(|position| &entries[*position])
            .filter(|entry| match entry.kind {
                EntryKind::A | EntryKind::Tool(_) | EntryKind::Operation(_) => true,
                EntryKind::H(handoff) => sent.contains(&handoff),
                _ => false,
            })
            .collect();
        let has_h = members
            .iter()
            .any(|position| matches!(entries[*position].kind, EntryKind::H(_)));
        if start.is_none() && !u && content.is_empty() && !has_h {
            return None;
        }
        if last && self.sessions[index].out.state == "work" {
            return Some(End {
                st: "work",
                why: "working",
                h: None,
            });
        }
        if let Some(handoff) = sent
            .iter()
            .rev()
            .find(|handoff| self.handoffs[**handoff].out.kind == "toyou")
        {
            let found = &self.handoffs[*handoff].out;
            return Some(End {
                st: if found.status == "wait" {
                    "wait"
                } else {
                    "done"
                },
                why: "toyou",
                h: Some(found.id.clone()),
            });
        }
        let start = start.map(|handoff| &self.handoffs[handoff].out);
        if start.is_some_and(|start| start.status == "err") {
            return Some(End {
                st: "err",
                why: "failed",
                h: None,
            });
        }
        let last_content = content.last();
        match last_content.map(|entry| entry.kind) {
            Some(EntryKind::Operation(ToolState::Err)) => {
                return Some(End {
                    st: "err",
                    why: "failed_step",
                    h: None,
                });
            }
            Some(EntryKind::Tool(ToolState::Err)) => {
                return Some(End {
                    st: "err",
                    why: "failed_step",
                    h: None,
                });
            }
            // Reached only outside the running last turn of a working
            // session, or for a yielded command still running there.
            Some(EntryKind::Tool(ToolState::Pending))
                if !last_content
                    .and_then(|entry| entry.at)
                    .is_some_and(|at| self.live_yield(index, at)) =>
            {
                return Some(End {
                    st: "err",
                    why: "unfinished_step",
                    h: None,
                });
            }
            Some(EntryKind::H(handoff)) if self.handoffs[handoff].out.status == "err" => {
                return Some(End {
                    st: "err",
                    why: "handoff_failed",
                    h: Some(self.handoffs[handoff].out.id.clone()),
                });
            }
            _ => {}
        }
        if start.is_some_and(|start| start.kind == "spawn" && start.status == "done") {
            return Some(End {
                st: "done",
                why: "returned",
                h: None,
            });
        }
        if content
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::A))
        {
            return Some(End {
                st: "done",
                why: "replied",
                h: None,
            });
        }
        Some(End {
            st: "idle",
            why: "no_reply",
            h: None,
        })
    }
}

impl Builder<'_> {
    /// A yielded command still running: no completion yet, its session id
    /// still open in its file, and its session working.
    fn live_yield(&self, index: usize, at: Ref) -> bool {
        let found = event(self.files, at);
        self.sessions[index].out.state == "work"
            && found
                .y
                .as_ref()
                .is_some_and(|yielded| yielded.done.is_none())
            && self.files[at.0]
                .summary
                .yields
                .values()
                .any(|start| *start == at.1)
    }
}

/// A tool call's state from its event: a yielded command's from the item
/// that completed it, and a poll that sent input's from its script alone
/// (its command's exit is the command's step).
fn tool_state(found: &Event) -> ToolState {
    let reply = match &found.y {
        Some(yielded) => yielded.done.as_ref(),
        None => found.r.as_ref(),
    };
    match reply {
        Some(reply) if reply.e => ToolState::Err,
        Some(_) if found.poll.is_some() => ToolState::Ok,
        Some(reply) if reply.f & UNKNOWN != 0 => ToolState::Unknown,
        Some(_) => ToolState::Ok,
        None => ToolState::Pending,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolState {
    Ok,
    Err,
    /// A Codex call with no structured exit code: neither ok nor failed.
    Unknown,
    /// No result yet. Only the last turn of a working session can still be
    /// running it; anywhere else it is a step that never finished.
    Pending,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    H(usize),
    U,
    A,
    Tool(ToolState),
    Operation(ToolState),
    Gap,
}

/// How a tool call is drawn: a pending call is live only in the last turn
/// of a working session, as the model's turn ends and activity read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    Ok,
    Err,
    /// A Codex call with no exit status: neither ok nor failed.
    Unknown,
    Live,
    Unfinished,
}

/// A source file a transcript slot points into.
#[derive(Clone, Debug)]
pub(crate) struct SlotFile {
    pub(crate) path: PathBuf,
    pub(crate) cwd: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum SlotKind {
    H(String),
    U,
    A,
    Tool {
        shown: Shown,
        name: String,
        reply: Option<events::Reply>,
        /// A plain Codex call's `CommandExecution` item line, when the logs
        /// have one: its output is the whole copy.
        item: Option<u64>,
    },
    /// A Codex code-mode command or file change, drawn in place of its
    /// wrapper when exactly one code-mode call owns it.
    Operation {
        kind: String,
        ok: Option<bool>,
        script_offset: Option<u64>,
    },
    /// A Codex command that outlived its yield, drawn as one step: the
    /// script that started it (the slot's line), the polls that folded into
    /// it and the item that completed it.
    Yielded {
        shown: Shown,
        /// The starting script's output line: the first output chunk.
        first: Option<u64>,
        /// The polls' output lines, in order.
        polls: Vec<u64>,
        /// More polls folded in than were kept.
        cut: bool,
        /// The `CommandExecution` item that completed it.
        done: Option<events::Reply>,
    },
    /// A poll that sent input to a yielded command: a step of its own.
    Sent {
        shown: Shown,
        reply: Option<events::Reply>,
        /// The line of the script that started the command, and of the item
        /// that completed it.
        start: u64,
        done: Option<u64>,
    },
    Gap,
    Think,
    Harness(String),
    /// A spawned run's return to the session that started it.
    Returned {
        to: String,
        at: Option<i64>,
        failed: bool,
    },
    /// A stub's transcript: nothing of its own in these logs.
    NoActivity,
}

/// One transcript entry, as offsets: `/api/tx` reads its text per page.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    pub(crate) kind: SlotKind,
    pub(crate) file: Option<usize>,
    pub(crate) offset: u64,
    pub(crate) block: u32,
    pub(crate) t: Option<i64>,
    /// The turn it belongs to, when that turn is in the model.
    pub(crate) turn: Option<String>,
    /// The first entry of its turn.
    pub(crate) first: bool,
}

impl Slot {
    fn new(kind: SlotKind, file: Option<usize>, offset: u64, block: u32, t: Option<i64>) -> Self {
        Self {
            kind,
            file,
            offset,
            block,
            t,
            turn: None,
            first: false,
        }
    }

    /// Whether this slot is a tool call and, if it is, whether it failed or
    /// never finished: exactly what a transcript's `calls` and `errors`
    /// count, so the errors badge and `/api/tx?errors=1` agree.
    pub(crate) fn failed_call(&self) -> Option<bool> {
        match &self.kind {
            SlotKind::Tool { shown, .. }
            | SlotKind::Yielded { shown, .. }
            | SlotKind::Sent { shown, .. } => Some(matches!(shown, Shown::Err | Shown::Unfinished)),
            SlotKind::Operation { ok, .. } => Some(*ok == Some(false)),
            _ => None,
        }
    }

    /// The tool a call slot is a call of, for counting calls by tool: a
    /// Claude call's name; a Codex command, poll or file change by the tool
    /// that ran it. `Some` exactly when [`Slot::failed_call`] is.
    pub(crate) fn call_name(&self) -> Option<&str> {
        match &self.kind {
            SlotKind::Tool { name, .. } => Some(name),
            SlotKind::Yielded { .. } => Some("exec_command"),
            SlotKind::Sent { .. } => Some("write_stdin"),
            SlotKind::Operation { kind, .. } => Some(if kind == "CommandExecution" {
                "exec_command"
            } else {
                "apply_patch"
            }),
            _ => None,
        }
    }
}

/// The most tool names a session's `tool_calls` keeps; the calls of the rest
/// are counted together under [`TOOL_OTHER`].
pub(crate) const TOOL_KINDS_MAX: usize = 12;
/// The longest tool name `tool_calls` keeps, in characters.
pub(crate) const TOOL_NAME_MAX: usize = 64;
/// The bucket for the calls of the tools past [`TOOL_KINDS_MAX`].
pub(crate) const TOOL_OTHER: &str = "other";

/// A tool name fit to keep: at most [`TOOL_NAME_MAX`] characters. A longer
/// name (an MCP tool's) is cut and ends in `#` and a hash of the whole name,
/// so two names that share a long prefix stay different.
pub(crate) fn tool_label(name: &str) -> String {
    if name.chars().count() <= TOOL_NAME_MAX {
        return name.to_owned();
    }
    // 8 hex digits and the `#` make up the last 9 characters.
    let head: String = name.chars().take(TOOL_NAME_MAX - 9).collect();
    format!("{head}#{}", events::short_hash(name))
}

/// The calls by tool, bounded: the [`TOOL_KINDS_MAX`] tools with the most
/// calls (ties by name) and the rest added up under [`TOOL_OTHER`], so the
/// counts still add up to the calls.
pub(crate) fn capped_tool_counts(counts: BTreeMap<String, usize>) -> BTreeMap<String, usize> {
    if counts.len() <= TOOL_KINDS_MAX {
        return counts;
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let rest = ranked.split_off(TOOL_KINDS_MAX);
    let mut kept: BTreeMap<String, usize> = ranked.into_iter().collect();
    for (_, calls) in rest {
        *kept.entry(TOOL_OTHER.to_owned()).or_default() += calls;
    }
    kept
}

/// A session's transcript index and its totals.
#[derive(Clone, Debug, Default)]
pub(crate) struct Transcript {
    pub(crate) slots: Vec<Slot>,
    /// Tool calls, and those that failed or never finished.
    pub(crate) calls: usize,
    pub(crate) errors: usize,
    /// The calls by tool name, bounded (see [`capped_tool_counts`]); they add
    /// up to `calls`.
    pub(crate) tools: BTreeMap<String, usize>,
}

impl Transcript {
    fn from_slots(slots: Vec<Slot>) -> Self {
        let mut calls = 0;
        let mut errors = 0;
        let mut tools = BTreeMap::<String, usize>::new();
        for slot in &slots {
            let Some(failed) = slot.failed_call() else {
                continue;
            };
            calls += 1;
            errors += usize::from(failed);
            *tools
                .entry(tool_label(slot.call_name().unwrap_or("tool")))
                .or_default() += 1;
        }
        let tools = capped_tool_counts(tools);
        Self {
            slots,
            calls,
            errors,
            tools,
        }
    }
}

struct Entry {
    kind: EntryKind,
    file: usize,
    offset: u64,
    /// Where it sits in its file (line offset, block): its event's place, the
    /// event a handoff follows, or where a failed code-mode call's failure
    /// arrived.
    pos: (u64, u32),
    t: Option<i64>,
    at: Option<Ref>,
}

/// One turn's entries, before its end is known.
struct Group {
    start: Option<usize>,
    u: bool,
    file: usize,
    offset: u64,
    at: Option<i64>,
    entries: Vec<usize>,
}

// ---- Questions and answers ---------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Question {
    question: String,
    labels: Vec<String>,
    multi: bool,
}

/// The question fields the model uses. Questions, labels and answers are
/// all capped at the same generous [`TEXT_MAX`], so matching (a chosen label,
/// the text-only anchors) compares like with like; output is capped to
/// [`MSG_MAX`] only after matching.
fn compact_questions(input: &Value) -> Value {
    let questions: Vec<Value> = input
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| {
            questions
                .iter()
                .map(|question| {
                    let labels: Vec<Value> = question
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|options| {
                            options
                                .iter()
                                .filter_map(|option| {
                                    field(option, "label").or_else(|| option.as_str())
                                })
                                .map(|label| Value::String(cap(label, TEXT_MAX)))
                                .collect()
                        })
                        .unwrap_or_default();
                    serde_json::json!({
                        "question": cap(field(question, "question")
                            .or_else(|| field(question, "title"))
                            .unwrap_or(""), TEXT_MAX),
                        "multiSelect": question.get("multiSelect").and_then(Value::as_bool) == Some(true),
                        "options": labels,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({ "questions": questions })
}

/// A question's result, reduced to capped answers, previews and text.
fn compact_answers(record: &Value, block: usize) -> Value {
    let result = record.get("toolUseResult").unwrap_or(&Value::Null);
    let capped = |value: &Value| match value {
        Value::String(text) => Value::String(cap(text, TEXT_MAX)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| Value::String(cap(item.as_str().unwrap_or(""), TEXT_MAX)))
                .collect(),
        ),
        _ => Value::Null,
    };
    let answers: serde_json::Map<String, Value> = result
        .get("answers")
        .and_then(Value::as_object)
        .map(|answers| {
            answers
                .iter()
                .map(|(question, answer)| (cap(question, TEXT_MAX), capped(answer)))
                .collect()
        })
        .unwrap_or_default();
    let annotations: serde_json::Map<String, Value> = result
        .get("annotations")
        .and_then(Value::as_object)
        .map(|annotations| {
            annotations
                .iter()
                .filter_map(|(question, annotation)| {
                    let preview = field(annotation, "preview")?;
                    Some((
                        cap(question, TEXT_MAX),
                        serde_json::json!({ "preview": cap(preview, TEXT_MAX) }),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "tur": { "answers": answers, "annotations": annotations },
        "text": tool_result_text(record, block).map(|text| cap(&text, 4 * TEXT_MAX)),
    })
}

fn questions_of(input: &Value) -> Vec<Question> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| {
            questions
                .iter()
                .map(|question| Question {
                    question: field(question, "question")
                        .or_else(|| field(question, "title"))
                        .unwrap_or("")
                        .to_owned(),
                    labels: question
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|options| {
                            options
                                .iter()
                                .filter_map(|option| {
                                    field(option, "label").or_else(|| option.as_str())
                                })
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                    multi: question.get("multiSelect").and_then(Value::as_bool) == Some(true),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Splits a multi-select answer written as one string greedily on the known
/// option labels. Whatever isn't a label is kept as free text.
fn split_labels(value: &str, labels: &[String]) -> Vec<String> {
    let mut sorted: Vec<&String> = labels.iter().filter(|label| !label.is_empty()).collect();
    sorted.sort_by_key(|label| std::cmp::Reverse(label.len()));
    let mut result = Vec::new();
    let mut rest = value.trim();
    'outer: while !rest.is_empty() {
        for label in &sorted {
            if let Some(after) = rest.strip_prefix(label.as_str())
                && (after.is_empty() || after.starts_with(','))
            {
                result.push((*label).clone());
                rest = after.trim_start_matches(',').trim_start();
                continue 'outer;
            }
        }
        result.push(rest.to_owned());
        break;
    }
    result
}

fn answer_for(question: &Question, value: &Value, preview: Option<String>) -> Answer {
    let values: Vec<String> = match value {
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map_or_else(|| item.to_string(), str::to_owned)
            })
            .collect(),
        Value::String(text) if question.multi && !question.labels.contains(text) => {
            split_labels(text, &question.labels)
        }
        Value::String(text) => vec![text.clone()],
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    };
    Answer {
        question: cap(&question.question, MSG_MAX),
        free: values.iter().any(|value| !question.labels.contains(value)),
        multi: question.multi || matches!(value, Value::Array(_)),
        values: values.iter().map(|value| cap(value, MSG_MAX)).collect(),
        preview: preview.map(|preview| cap(&preview, MSG_MAX)),
    }
}

/// `toolUseResult.answers`, ordered by `input.questions`.
fn structured_answers(questions: &[Question], result: &Value) -> Vec<Answer> {
    let answers = &result["answers"];
    let annotations = &result["annotations"];
    questions
        .iter()
        .map(|question| {
            let preview = annotations
                .get(&question.question)
                .and_then(|annotation| field(annotation, "preview"))
                .map(str::to_owned);
            answer_for(
                question,
                answers.get(&question.question).unwrap_or(&Value::Null),
                preview,
            )
        })
        .collect()
}

/// The result text, only when the structured field is missing: anchored on
/// the known question strings, so quotes inside a question or an answer
/// don't break it.
fn text_answers(questions: &[Question], text: &str) -> Vec<Answer> {
    let anchors: Vec<Option<(usize, usize)>> = {
        let mut from = 0;
        questions
            .iter()
            .map(|question| {
                let anchor = format!("\"{}\"=\"", question.question);
                let found = text[from..]
                    .find(&anchor)
                    .map(|at| (from + at, from + at + anchor.len()));
                if let Some((_, end)) = found {
                    from = end;
                }
                found
            })
            .collect()
    };
    let mut result = Vec::new();
    for (position, question) in questions.iter().enumerate() {
        let Some((_, value_start)) = anchors[position] else {
            result.push(answer_for(question, &Value::Null, None));
            continue;
        };
        let value_end = anchors[position + 1..]
            .iter()
            .flatten()
            .next()
            .map(|(start, _)| text[..*start].trim_end().trim_end_matches(',').len())
            .unwrap_or_else(|| {
                ["\". You can now continue", "\". Read the answers carefully"]
                    .iter()
                    .find_map(|end| {
                        text[value_start..]
                            .rfind(end)
                            .map(|at| value_start + at + 1)
                    })
                    .or_else(|| text.rfind('"').map(|at| at + 1))
                    .unwrap_or(text.len())
            });
        let segment = text
            .get(value_start..value_end.max(value_start))
            .unwrap_or("")
            .trim_end();
        let segment = segment.strip_suffix('"').unwrap_or(segment);
        let (value, preview) = match segment.split_once("\" selected preview:\n") {
            Some((value, preview)) => (value, Some(preview.to_owned())),
            None => (segment, None),
        };
        let value = if question.multi {
            Value::Array(
                split_labels(value, &question.labels)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            )
        } else {
            Value::String(value.to_owned())
        };
        result.push(answer_for(question, &value, preview));
    }
    result
}

/// A Codex `apply_patch` call's patch: its raw input, or an `input` or
/// `patch` argument.
pub(crate) fn patch_text(input: &Value) -> Option<String> {
    match input {
        Value::String(text) => Some(text.clone()),
        _ => field(input, "input")
            .or_else(|| field(input, "patch"))
            .map(str::to_owned),
    }
}

/// The files a patch touches, in order.
pub(crate) fn patch_files(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(|path| path.trim().to_owned())
        .collect()
}

pub(crate) fn relative(path: &str, cwd: Option<&str>, home: Option<&str>) -> String {
    if let Some(cwd) = cwd
        && let Some(rest) = path.strip_prefix(&format!("{}/", cwd.trim_end_matches('/')))
    {
        return rest.to_owned();
    }
    if let Some(home) = home
        && let Some(rest) = path.strip_prefix(home)
    {
        return format!("~{rest}");
    }
    path.to_owned()
}

/// The one-line argument the mockup shows for a running tool.
pub(crate) fn arg_summary(
    name: &str,
    input: &Value,
    cwd: Option<&str>,
    home: Option<&str>,
) -> String {
    let get = |key: &str| field(input, key).map(str::to_owned);
    let summary = match name {
        "Bash" | "shell" | "exec_command" | "local_shell" => {
            match input.get("command").or_else(|| input.get("cmd")) {
                Some(Value::Array(parts)) => Some(
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                Some(Value::String(command)) => Some(command.clone()),
                _ => None,
            }
        }
        "Read" | "Edit" | "Write" | "NotebookEdit" | "MultiEdit" => get("file_path")
            .or_else(|| get("notebook_path"))
            .map(|path| relative(&path, cwd, home)),
        "Grep" | "Glob" => get("pattern"),
        "WebFetch" => get("url"),
        "WebSearch" | "ToolSearch" => get("query"),
        "Agent" | "Task" => get("description"),
        "SendMessage" => get("to"),
        "Skill" => get("skill"),
        "Monitor" => get("description").or_else(|| get("command")),
        "apply_patch" => patch_text(input).and_then(|patch| {
            patch_files(&patch)
                .into_iter()
                .next()
                .map(|path| relative(&path, cwd, home))
        }),
        _ => None,
    };
    let summary = summary.filter(|text| !text.trim().is_empty()).or_else(|| {
        input.as_object().and_then(|object| {
            object
                .values()
                .find_map(|value| value.as_str().filter(|text| !text.trim().is_empty()))
                .map(str::to_owned)
        })
    });
    one_line(summary.as_deref().unwrap_or(""), 160)
}

// ---- Build -------------------------------------------------------------------------------

/// Builds the model from the whole of `~/.claude` and `~/.codex`: links need
/// both ends, so every file is indexed whatever the window, and the window
/// only trims what is returned. With [`Options::scan_window`], files last
/// modified before the window aren't read at all.
pub(crate) fn build(
    options: &Options,
    cache: &mut EventCache,
    dirty: &mut bool,
    texts: &mut Texts,
    now: i64,
) -> io::Result<Built> {
    let window_start = scan_cutoff(options, now);
    let (files, skipped) = scan(options, cache, dirty, texts, window_start)?;
    #[cfg(test)]
    {
        BUILDS.with(|builds| builds.set(builds.get() + 1));
        AFTER_SCAN.with(|hook| hook.borrow_mut().take().map(|hook| hook()));
    }
    let facts = MachineFacts::of(options);
    let pids = pid_files(options, &facts);
    let lock_pids = facts.codex_lock_pids(options);
    let held: BTreeSet<String> = lock_pids
        .iter()
        .flat_map(|locks| locks.keys().cloned())
        .collect();
    let machine = facts.hostname(options);
    let home = facts.home();
    let offline_since = facts.offline_since();
    let groups = lineages(&files);
    let reported_runs: Vec<ReportedRunSnapshot> = match &facts {
        MachineFacts::Local => cache.reported_runs().cloned().collect(),
        MachineFacts::Recorded(facts) => facts.reported_runs.clone(),
    };
    let mut builder = Builder::new(&files, texts, now, machine.clone(), &facts, &reported_runs);
    builder.sessions(groups, &pids, &held);
    builder.index_tools();
    builder.claude_spawns();
    builder.codex_spawns();
    builder.relays();
    builder.codex_relays();
    builder.asks();
    builder.questions();
    builder.lineage_states();
    builder.activity();
    #[cfg(test)]
    let texts = builder.texts_by_turn();
    let (mut turns, mut tx) = builder.turns();

    // Stubs span the handoffs that name them.
    for handoff in &builder.handoffs {
        for end in [handoff.from, handoff.to].into_iter().flatten() {
            let session = &mut builder.sessions[end];
            if session.kind == SessKind::Stub {
                let at = handoff.out.at;
                session.out.start = if session.out.start == 0 {
                    at
                } else {
                    session.out.start.min(at)
                };
                session.out.last = session.out.last.max(at);
            }
        }
    }
    let run_of = |pid, start| facts.run(options, pid, start);
    let session_facts = builder.session_facts(
        &skipped,
        |id| match lock_pids.as_ref() {
            Some(locks) => match locks.get(id) {
                Some(pid) => (Some(*pid), Some(true)),
                // A lock file no process holds: its writer is gone. Without a
                // lock file there is no process record.
                None => (
                    None,
                    facts
                        .codex_lock_file(options, id)
                        .filter(|exists| *exists)
                        .map(|_| false),
                ),
            },
            None => (None, None),
        },
        run_of,
    );

    let mut handoffs: Vec<Handoff> = builder
        .handoffs
        .iter()
        .map(|handoff| handoff.out.clone())
        .collect();
    let handoff_by_id: HashMap<&str, &Handoff> = builder
        .handoffs
        .iter()
        .map(|handoff| (handoff.out.id.as_str(), &handoff.out))
        .collect();
    let mut sessions: BTreeMap<String, Session> = builder
        .sessions
        .iter()
        .map(|session| (session.key.clone(), session.out.clone()))
        .collect();
    // Analytics reads a month and the month before it, whatever the model's
    // window: taken from every session before the window trims them.
    let activity = crate::analytics::activity(&sessions, &tx, &turns, &handoffs, now);
    // A scan window already chose the files; what it read is returned
    // whole, so an answer is never trimmed inside a session.
    if !options.all && !options.scan_window {
        let cutoff = now - i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX);
        handoffs.retain(|handoff| {
            handoff.at >= cutoff
                || handoff.done.is_some_and(|done| done >= cutoff)
                || matches!(handoff.status, "work" | "wait" | "new")
        });
        let mut keep: BTreeSet<String> = sessions
            .iter()
            .filter(|(_, session)| {
                session.last >= cutoff || matches!(session.state, "work" | "wait")
            })
            .map(|(key, _)| key.clone())
            .collect();
        for handoff in &handoffs {
            keep.insert(handoff.from.clone());
            keep.extend(handoff.to.clone());
        }
        sessions.retain(|key, _| keep.contains(key));
        let mut kept: BTreeSet<String> =
            handoffs.iter().map(|handoff| handoff.id.clone()).collect();
        turns.retain(|turn| {
            sessions.contains_key(&turn.sid)
                && (turn.last
                    || turn.at.is_some_and(|at| at >= cutoff)
                    || turn
                        .start
                        .as_deref()
                        .is_some_and(|start| kept.contains(start)))
        });
        // A retained turn names its incoming and outgoing handoffs even
        // when those handoffs fall outside the time window. Keep those exact
        // links so the turn remains self-contained.
        for turn in &turns {
            for id in turn.start.iter().chain(&turn.sent) {
                if kept.insert(id.clone())
                    && let Some(handoff) = handoff_by_id.get(id.as_str())
                {
                    handoffs.push((*handoff).clone());
                    for endpoint in std::iter::once(&handoff.from).chain(handoff.to.iter()) {
                        if !sessions.contains_key(endpoint)
                            && let Some(session) = builder
                                .sessions
                                .iter()
                                .find(|session| &session.key == endpoint)
                        {
                            sessions.insert(endpoint.clone(), session.out.clone());
                        }
                    }
                }
            }
        }
        tx.retain(|key, _| sessions.contains_key(key));
        for session in sessions.values_mut() {
            session.busy.retain(|interval| interval.1 >= cutoff);
            if let Some(first) = session.busy.first_mut() {
                first.0 = first.0.max(cutoff);
            }
        }
    }
    handoffs.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    // Match the approved mockup's parentOf rule against the same handoffs
    // and session flags the client receives.
    for (sid, session) in &mut sessions {
        session.parent = handoffs
            .iter()
            .find(|handoff| {
                (handoff.kind == "spawn" || handoff.kind == "relay")
                    && handoff.to.as_deref() == Some(sid.as_str())
                    && handoff.from.as_str() != sid.as_str()
                    && (handoff.kind == "spawn" || session.kind == Some("Relayed") || !session.lane)
            })
            .map(|handoff| handoff.from.clone());
    }
    let order: HashMap<&str, usize> = {
        let mut keys: Vec<(&str, i64)> = sessions
            .iter()
            .map(|(key, session)| (key.as_str(), session.start))
            .collect();
        keys.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));
        keys.into_iter()
            .enumerate()
            .map(|(position, (key, _))| (key, position))
            .collect()
    };
    turns.sort_by_key(|turn| order.get(turn.sid.as_str()).copied().unwrap_or(usize::MAX));
    let all_busy = events::busy_merge(
        sessions
            .values()
            .flat_map(|session| session.busy.iter().copied())
            .collect(),
    );
    // A failed send reached no one. The page draws a stub for the addressee
    // as written, `unsent:<target>`; its transcript lists those sends.
    let mut unsent: BTreeMap<String, Vec<&Handoff>> = BTreeMap::new();
    for handoff in &handoffs {
        if handoff.to.is_none()
            && let Some(target) = &handoff.target
        {
            unsent
                .entry(format!("unsent:{target}"))
                .or_default()
                .push(handoff);
        }
    }
    for (key, sends) in unsent {
        let mut slots = vec![Slot::new(SlotKind::NoActivity, None, 0, 0, None)];
        slots.extend(sends.into_iter().map(|handoff| {
            Slot::new(
                SlotKind::H(handoff.id.clone()),
                None,
                0,
                0,
                Some(handoff.at),
            )
        }));
        tx.insert(key, Transcript::from_slots(slots));
    }
    // Each session's totals come from its transcript index, built above from
    // the same slots `/api/tx` counts.
    for (key, session) in &mut sessions {
        if let Some(transcript) = tx.get(key) {
            session.calls = Some(transcript.calls);
            session.errors = Some(transcript.errors);
            session.tool_calls = transcript.tools.clone();
        }
    }
    let busy = BTreeMap::from([(machine.clone(), all_busy)]);
    let marks: BTreeMap<&str, String> = tx
        .iter()
        .map(|(sid, transcript)| {
            let sources: BTreeSet<usize> = transcript
                .slots
                .iter()
                .filter_map(|slot| slot.file)
                .collect();
            let bytes: u64 = sources
                .iter()
                .filter_map(|file| files.get(*file))
                .map(|file| file.stamp.size)
                .sum();
            // A call stops running when its process dies, which writes no
            // line: the running count moves the mark all the same.
            let running = transcript
                .slots
                .iter()
                .filter(|slot| {
                    matches!(
                        slot.kind,
                        SlotKind::Tool {
                            shown: Shown::Live,
                            ..
                        } | SlotKind::Yielded {
                            shown: Shown::Live,
                            ..
                        } | SlotKind::Sent {
                            shown: Shown::Live,
                            ..
                        }
                    )
                })
                .count();
            (
                sid.as_str(),
                format!("{}.{bytes}.{running}", transcript.slots.len()),
            )
        })
        .collect();
    let machine_id = machine.clone();
    let machine = Machine {
        id: machine.clone(),
        name: machine,
        up: offline_since.is_none(),
        last: offline_since,
    };
    let machine = serde_json::to_string(&machine)?;
    let pricing = crate::pricing::table();
    let rest = serde_json::to_string(&Rest {
        pricing: &pricing,
        handoffs: &handoffs,
        turns: &turns,
        busy: &busy,
        tx: &marks,
    })?;
    // The version also covers every source file's length, so any appended
    // line gives a new version, even one whose entry changes nothing else in
    // the model (the same timestamp as the line before it). Lengths, not
    // paths or times, keep it the same for the same content.
    let lengths: Vec<String> = files
        .iter()
        .map(|file| file.stamp.size.to_string())
        .collect();
    let version = format!(
        "{:016x}",
        fnv(&format!(
            "{machine}{}{rest}|{}",
            serde_json::to_string(&sessions)?,
            lengths.join(",")
        ))
    );
    let pids = pids.iter().map(|pid| pid.pid).collect();
    let slot_files = files
        .iter()
        .map(|file| SlotFile {
            path: file.path.clone(),
            cwd: file.summary.cwd.clone(),
        })
        .collect();
    Ok(Built {
        version,
        machine,
        sessions,
        rest: rest[1..].to_owned(),
        pids,
        home,
        machine_id,
        files: slot_files,
        tx,
        facts: session_facts,
        window_start,
        activity,
        #[cfg(test)]
        handoffs,
        #[cfg(test)]
        turns,
        #[cfg(test)]
        texts,
    })
}

#[cfg(test)]
mod tests;
