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
    env, fs,
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
    Marker, Options, codex_meta, events,
    events::{
        ACK, ANSWERED, ASYNC, DENIED, Event, EventCache, FileIndex, Kind, PIN, SEND_FAILED, UNKNOWN,
    },
    field, file_list, lock_identity, lock_pid, matches_handoff, proc_start, read_first_marker,
};

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
    pub(crate) tokens: [f64; 3],
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
}

#[derive(Serialize)]
struct Rest<'a> {
    handoffs: &'a [Handoff],
    turns: &'a [Turn],
    busy: &'a BTreeMap<String, Vec<(i64, i64)>>,
}

/// A built model. Everything but the sessions' running-tool age is
/// serialized once; `version` hashes the build, so it only changes when the
/// logs do.
pub(crate) struct Built {
    pub(crate) version: String,
    machine: String,
    /// `activity[2]` holds the running tool's start (epoch ms) until served.
    pub(crate) sessions: BTreeMap<String, Session>,
    /// `"handoffs":…,"turns":…,"busy":…}`
    rest: String,
    pub(crate) pids: Vec<u32>,
    /// This machine's id, as sessions name it.
    pub(crate) machine_id: String,
    /// Every source file, by position: what transcript slots point into.
    pub(crate) files: Vec<SlotFile>,
    /// Each session's transcript index, for `/api/tx`: metadata and offsets
    /// only, the text is read back per page.
    pub(crate) tx: BTreeMap<String, Transcript>,
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
            "{{\"version\":\"{}\",\"now\":{now},\"machine\":{},\"sessions\":{sessions},{}",
            self.version, self.machine, self.rest
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
    if let Some(now) = env::var("SEMON_TEST_NOW")
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

/// Writes the model for `options` as JSON, and saves the metadata cache.
pub fn model_json(options: &Options) -> io::Result<String> {
    model_json_at(options, now_ms())
}

/// [`model_json`] at a fixed `now` (epoch ms), for fixtures.
#[doc(hidden)]
pub fn model_json_at(options: &Options, now: i64) -> io::Result<String> {
    let path = EventCache::path(&options.cache);
    let mut cache = EventCache::read(&path);
    let mut dirty = false;
    let mut texts = Texts::default();
    let built = build(options, &mut cache, &mut dirty, &mut texts, now)?;
    if dirty {
        cache.save(&path)?;
    }
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
#[derive(Default)]
pub(crate) struct Texts {
    memo: HashMap<TextKey, (Option<String>, u64)>,
    bytes: usize,
    tick: u64,
    generations: HashMap<PathBuf, (Stamp, u64)>,
    next_generation: u64,
    handoff_tools: HashMap<String, (u64, Option<String>)>,
    repos: HashMap<String, Option<String>>,
    markers: HashMap<PathBuf, (Stamp, Option<Marker>)>,
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
        let repo = repo_of(cwd);
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
/// message, with reminders removed.
pub(crate) fn prompt_text(record: &Value) -> Option<String> {
    if let Some(attachment) = record.get("attachment") {
        return attachment
            .get("prompt")
            .map(content_text)
            .map(|text| events::clean_prompt(&text));
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
        return Some(events::clean_prompt(&text));
    }
    let payload = record.get("payload")?;
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| field(part, "text"))
        .collect::<String>();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("Semon-Parent:") && !line.starts_with("Semon-Handoff:"))
        .collect();
    Some(kept.join("\n").trim().to_owned())
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

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn repo_of(cwd: &str) -> Option<String> {
    if let Some((before, _)) = cwd.split_once("/.claude/worktrees/") {
        return Path::new(before)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }
    let home = home();
    let mut path = Some(Path::new(cwd));
    while let Some(current) = path {
        if current == Path::new("/") || home.as_deref() == Some(current) {
            break;
        }
        if current.join(".git").exists() {
            return current
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
        }
        path = current.parent();
    }
    None
}

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

fn hostname(options: &Options) -> String {
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

struct PidFile {
    pid: u32,
    session: String,
    alive: bool,
    status: Option<String>,
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

fn pid_files(options: &Options) -> Vec<PidFile> {
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
        let Some(record) = fs::read(entry.path())
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
            alive: start.is_some() && proc_start(&options.proc_root, pid) == start,
            status: field(&record, "status").map(str::to_owned),
            name: field(&record, "name").map(str::to_owned),
        });
    }
    result.sort_by_key(|pid| pid.pid);
    result
}

fn held_codex(options: &Options) -> BTreeSet<String> {
    let mut held = BTreeSet::new();
    #[cfg(unix)]
    if let (Ok(locks), Ok(entries)) = (
        fs::read_to_string(options.proc_root.join("locks")),
        fs::read_dir(options.codex_home.join("thread-writer-locks")),
    ) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".lock")) else {
                continue;
            };
            if let Ok((major, minor, ino)) = lock_identity(&entry.path())
                && lock_pid(&locks, major, minor, ino).is_some()
            {
                held.insert(id.to_owned());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = options;
    held
}

fn scan(
    options: &Options,
    cache: &mut EventCache,
    dirty: &mut bool,
    texts: &mut Texts,
) -> io::Result<Vec<SourceFile>> {
    let projects = options.claude_home.join("projects");
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();
    file_list(&projects, &mut paths, "jsonl")?;
    for path in paths {
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
            let meta = fs::read(path.with_extension("meta.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .filter(Value::is_object)
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
        let Ok(summary) = events::scan_file(&path, "claude", cache, dirty) else {
            continue;
        };
        seen.insert(path.to_string_lossy().into_owned());
        files.push(SourceFile {
            stamp: stamp_of(&path),
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
        let Ok(meta) = codex_meta(&path) else {
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
        let Ok(summary) = events::scan_file(&path, "codex", cache, dirty) else {
            continue;
        };
        seen.insert(path.to_string_lossy().into_owned());
        let meta = meta.unwrap_or(Value::Null);
        let stamp = stamp_of(&path);
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
    Ok(files)
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
    names: BTreeSet<String>,
    refs: Vec<Ref>,
    parent: Option<usize>,
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
    sessions: Vec<Sess>,
    of_file: Vec<usize>,
    by_key: HashMap<String, usize>,
    handoffs: Vec<H>,
    placed: HashMap<Ref, (usize, Place)>,
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
    fn new(files: &'a [SourceFile], texts: &'a mut Texts, now: i64, machine: String) -> Self {
        Self {
            files,
            texts,
            now,
            machine,
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
            tokens: [0.0; 3],
            repo: None,
            branch: None,
            start: 0,
            last: 0,
            activity: None,
            busy: Vec::new(),
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
            names: BTreeSet::new(),
            refs: Vec::new(),
            parent: None,
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
        self.placed.entry(at).or_insert((handoff, place));
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
            if let Some(name) = &pid.name {
                session.names.insert(plain_name(name));
            }
            if pid.alive {
                session.alive = true;
                session.busy = pid.status.as_deref() == Some("busy");
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

    /// Name, model, tokens, repo, branch, start, last and busy.
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
        let mut start = i64::MAX;
        let mut last = 0;
        let mut names = BTreeSet::new();
        let mut title = None;
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
            tokens.input += used.input;
            tokens.cached_input += used.cached_input;
            tokens.output += used.output;
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
        let repo = cwd.as_deref().and_then(|cwd| self.texts.repo(cwd));
        let session = &mut self.sessions[index];
        session.names.extend(names);
        let out = &mut session.out;
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
        let Some(parent_file) = self
            .files
            .iter()
            .position(|file| matches!(file.role, Role::Top { .. }) && file.id == marker.claude_id)
        else {
            return (None, None, false);
        };
        let spawner = self.of_file[parent_file];
        let tool = match (&marker.tool_id, &marker.handoff) {
            (Some(id), _) => Some(id.clone()),
            (None, Some(prompt)) => self.handoff_tool(&file.id, spawner, prompt),
            _ => None,
        };
        let call = tool.and_then(|id| self.find_in(spawner, &id));
        (Some(spawner), call, false)
    }

    /// The parent's Bash or Skill call that launched a `Semon-Handoff`
    /// prompt, memoized per run until the parent's files grow.
    fn handoff_tool(&mut self, run: &str, spawner: usize, prompt: &str) -> Option<String> {
        let paths: Vec<PathBuf> = self.sessions[spawner]
            .files
            .iter()
            .map(|file| self.files[*file].path.clone())
            .collect();
        let size: u64 = paths
            .iter()
            .map(|path| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
            .sum();
        if let Some((seen, found)) = self.texts.handoff_tools.get(run)
            && (found.is_some() || *seen == size)
        {
            return found.clone();
        }
        let found = paths
            .iter()
            .find_map(|path| matches_handoff(path, prompt).ok().flatten())
            .map(|tool| tool.id);
        self.texts
            .handoff_tools
            .insert(run.to_owned(), (size, found.clone()));
        found
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
                    stable_id("a", &source),
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
                        let answered = found.r.as_ref().is_some_and(|reply| reply.f & ACK == 0);
                        let followed = refs[position + 1..]
                            .iter()
                            .any(|later| event(self.files, *later).k == Kind::U);
                        if self.sessions[index].alive && !answered && !followed {
                            handoff.status = "wait";
                        }
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

    /// Live top-level sessions: working, waiting on you (an open question or
    /// a result), or idle.
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
            if session.busy {
                self.sessions[index].out.state = "work";
                continue;
            }
            let open = self.handoffs.iter().any(|handoff| {
                handoff.from == Some(index)
                    && handoff.out.kind == "toyou"
                    && handoff.out.status == "wait"
            });
            if open {
                self.sessions[index].out.state = "wait";
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
                            "wait",
                            text.as_deref().unwrap_or(""),
                        )
                    };
                    let handoff = self.add_handoff(handoff, Some(index), None);
                    self.after.insert(at, handoff);
                    self.sessions[index].out.state = "wait";
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
            let name = found.n.clone().unwrap_or_else(|| "tool".into());
            let cwd = self.files[at.0].summary.cwd.clone();
            let summary_name = name.clone();
            let arg = self
                .text(at, "arg", move |record, block| {
                    tool_input(record, block)
                        .map(|input| arg_summary(&summary_name, &input, cwd.as_deref()))
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
        for file in &session.files {
            extras.insert(*file, self.files[*file].summary.extras.iter().collect());
        }
        let mut slots = Vec::with_capacity(entries.len());
        let mut owner: Option<usize> = None;
        let mut started = BTreeSet::new();
        let turn_id = |turn: Option<usize>| turn.map(|turn| turns[turn].id.clone());
        let extra_slot = |event: &Event, file: usize, owner: Option<usize>| {
            let kind = match event.k {
                Kind::Harness => SlotKind::Harness(event.n.clone().unwrap_or_default()),
                _ => SlotKind::Think,
            };
            let mut slot = Slot::new(kind, Some(file), event.o, event.b, event.t);
            slot.turn = turn_id(owner);
            slot
        };
        for (position, entry) in entries.iter().enumerate() {
            if let Some(queue) = extras.get_mut(&entry.file) {
                let at = entry.at.map_or((entry.offset, 0), |at| {
                    let found = event(self.files, at);
                    (found.o, found.b)
                });
                while queue.front().is_some_and(|extra| (extra.o, extra.b) < at) {
                    let extra = queue.pop_front().expect("front");
                    slots.push(extra_slot(extra, entry.file, owner));
                }
            }
            owner = owners[position];
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
                    SlotKind::Tool {
                        shown: match state {
                            ToolState::Ok => Shown::Ok,
                            ToolState::Err => Shown::Err,
                            ToolState::Unknown => Shown::Unknown,
                            ToolState::Pending if running => Shown::Live,
                            ToolState::Pending => Shown::Unfinished,
                        },
                        name: found
                            .and_then(|found| found.n.clone())
                            .unwrap_or_else(|| "tool".into()),
                        reply: found.and_then(|found| found.r.clone()),
                    }
                }
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
            slots.push(extra_slot(extra, file, owner));
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

    fn entries(&self, index: usize) -> Vec<Entry> {
        let mut entries = Vec::new();
        let refs = self.events_of(index);
        if let Some(handoff) = self.head.get(&index) {
            let (file, offset) = refs
                .first()
                .map(|at| (at.0, event(self.files, *at).o))
                .unwrap_or((self.sessions[index].files[0], 0));
            entries.push(Entry {
                kind: EntryKind::H(*handoff),
                file,
                offset,
                t: None,
                at: None,
            });
        }
        for at in refs {
            let found = event(self.files, at);
            let kind = if let Some((handoff, _)) = self.placed.get(&at) {
                Some(EntryKind::H(*handoff))
            } else {
                match found.k {
                    Kind::U => Some(EntryKind::U),
                    Kind::A => Some(EntryKind::A),
                    Kind::Gap => Some(EntryKind::Gap),
                    Kind::Tool if found.n.as_deref() != Some("SubagentHandback") => {
                        Some(EntryKind::Tool(match &found.r {
                            Some(reply) if reply.e => ToolState::Err,
                            Some(reply) if reply.f & UNKNOWN != 0 => ToolState::Unknown,
                            Some(_) => ToolState::Ok,
                            None => ToolState::Pending,
                        }))
                    }
                    _ => None,
                }
            };
            if let Some(kind) = kind {
                entries.push(Entry {
                    kind,
                    file: at.0,
                    offset: found.o,
                    t: found.t,
                    at: Some(at),
                });
            }
            if let Some(handoff) = self.after.get(&at) {
                entries.push(Entry {
                    kind: EntryKind::H(*handoff),
                    file: at.0,
                    offset: found.o,
                    t: found.t,
                    at: None,
                });
            }
        }
        entries
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
                EntryKind::A | EntryKind::Tool(_) => true,
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
        match content.last().map(|entry| entry.kind) {
            Some(EntryKind::Tool(ToolState::Err)) => {
                return Some(End {
                    st: "err",
                    why: "failed_step",
                    h: None,
                });
            }
            // Reached only outside the running last turn of a working session.
            Some(EntryKind::Tool(ToolState::Pending)) => {
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
}

/// A session's transcript index and its totals.
#[derive(Clone, Debug, Default)]
pub(crate) struct Transcript {
    pub(crate) slots: Vec<Slot>,
    /// Tool calls, and those that failed or never finished.
    pub(crate) calls: usize,
    pub(crate) errors: usize,
}

impl Transcript {
    fn from_slots(slots: Vec<Slot>) -> Self {
        let mut calls = 0;
        let mut errors = 0;
        for slot in &slots {
            if let SlotKind::Tool { shown, .. } = &slot.kind {
                calls += 1;
                if matches!(shown, Shown::Err | Shown::Unfinished) {
                    errors += 1;
                }
            }
        }
        Self {
            slots,
            calls,
            errors,
        }
    }
}

struct Entry {
    kind: EntryKind,
    file: usize,
    offset: u64,
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

pub(crate) fn relative(path: &str, cwd: Option<&str>) -> String {
    if let Some(cwd) = cwd
        && let Some(rest) = path.strip_prefix(&format!("{}/", cwd.trim_end_matches('/')))
    {
        return rest.to_owned();
    }
    if let Some(home) = home().and_then(|home| home.to_str().map(str::to_owned))
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_owned()
}

/// The one-line argument the mockup shows for a running tool.
pub(crate) fn arg_summary(name: &str, input: &Value, cwd: Option<&str>) -> String {
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
            .map(|path| relative(&path, cwd)),
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
                .map(|path| relative(&path, cwd))
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
/// both ends, so every file is indexed whatever the window. The window only
/// trims what is returned.
pub(crate) fn build(
    options: &Options,
    cache: &mut EventCache,
    dirty: &mut bool,
    texts: &mut Texts,
    now: i64,
) -> io::Result<Built> {
    let files = scan(options, cache, dirty, texts)?;
    let pids = pid_files(options);
    let held = held_codex(options);
    let machine = hostname(options);
    let groups = lineages(&files);
    let mut builder = Builder::new(&files, texts, now, machine.clone());
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

    let mut handoffs: Vec<Handoff> = builder
        .handoffs
        .iter()
        .map(|handoff| handoff.out.clone())
        .collect();
    let mut sessions: BTreeMap<String, Session> = builder
        .sessions
        .iter()
        .map(|session| (session.key.clone(), session.out.clone()))
        .collect();
    if !options.all {
        let cutoff = now - i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX);
        handoffs.retain(|handoff| {
            handoff.at >= cutoff
                || handoff.done.is_some_and(|done| done >= cutoff)
                || matches!(handoff.status, "work" | "wait")
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
        tx.retain(|key, _| keep.contains(key));
        let kept: BTreeSet<&str> = handoffs.iter().map(|handoff| handoff.id.as_str()).collect();
        turns.retain(|turn| {
            sessions.contains_key(&turn.sid)
                && (turn.last
                    || turn.at.is_some_and(|at| at >= cutoff)
                    || turn
                        .start
                        .as_deref()
                        .is_some_and(|start| kept.contains(start)))
        });
        for session in sessions.values_mut() {
            session.busy.retain(|interval| interval.1 >= cutoff);
            if let Some(first) = session.busy.first_mut() {
                first.0 = first.0.max(cutoff);
            }
        }
    }
    handoffs.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
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
    let busy = BTreeMap::from([(machine.clone(), all_busy)]);
    let machine_id = machine.clone();
    let machine = Machine {
        id: machine.clone(),
        name: machine,
        up: true,
    };
    let machine = serde_json::to_string(&machine)?;
    let rest = serde_json::to_string(&Rest {
        handoffs: &handoffs,
        turns: &turns,
        busy: &busy,
    })?;
    let version = format!(
        "{:016x}",
        fnv(&format!(
            "{machine}{}{rest}",
            serde_json::to_string(&sessions)?
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
        machine_id,
        files: slot_files,
        tx,
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
