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

/// The event cache sits beside V1's metadata cache and has its own version,
/// so `semon sessions`, `--watch` and `--json` never load or rewrite it.
const CACHE_VERSION: u32 = 2;

/// One file's event index and the facts the model needs about it. Metadata
/// only (risk:secret).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FileIndex {
    #[serde(default)]
    pub(crate) events: Vec<Event>,
    /// Tool calls still waiting for a result: call id -> event index.
    #[serde(default)]
    pub(crate) pending: BTreeMap<String, usize>,
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
    usage_by_id: BTreeMap<String, Tokens>,
    #[serde(default)]
    codex_tokens: Tokens,
}

impl FileIndex {
    /// Claude usage by message id, for merging across a lineage's files.
    pub(crate) fn usage(&self) -> &BTreeMap<String, Tokens> {
        &self.usage_by_id
    }

    pub(crate) fn tokens(&self, harness: &str) -> Tokens {
        if harness == "codex" {
            return self.codex_tokens.clone();
        }
        let mut total = Tokens::default();
        for usage in self.usage_by_id.values() {
            total.input += usage.input;
            total.cached_input += usage.cached_input;
            total.output += usage.output;
            total.total += usage.total;
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
}

impl EventCache {
    pub(crate) fn path(v1_cache: &Path) -> PathBuf {
        v1_cache.with_extension("events.json")
    }

    pub(crate) fn read(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|cache| cache.version == CACHE_VERSION)
            .unwrap_or_else(|| Self {
                version: CACHE_VERSION,
                files: BTreeMap::new(),
            })
    }

    pub(crate) fn save(&self, path: &Path) -> io::Result<()> {
        crate::save_json(path, self)
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
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
    /// Assistant text. Consecutive text blocks collapse into the last one.
    A,
    /// A tool call, resolved in place when its result arrives.
    Tool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) r: Option<Reply>,
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
    if event.k == Kind::A
        && let Some(last) = summary.events.last_mut()
        && last.k == Kind::A
    {
        *last = event;
        return;
    }
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
    if text.is_empty()
        || text.starts_with('<')
        || text.starts_with('/')
        || text.starts_with("[Request interrupted")
    {
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
    }
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

fn tool_event(summary: &mut FileIndex, event: Event) {
    if let Some(id) = &event.id {
        summary.pending.insert(id.clone(), summary.events.len());
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
        let input = number(usage, "input_tokens")
            + number(usage, "cache_creation_input_tokens")
            + number(usage, "cache_read_input_tokens");
        let output = number(usage, "output_tokens");
        summary.usage_by_id.insert(
            id.to_owned(),
            Tokens {
                input,
                cached_input: number(usage, "cache_read_input_tokens"),
                output,
                reasoning_output: 0,
                total: input + output,
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
    let Some(message) = record.get("message") else {
        return;
    };
    if role == Some("assistant")
        && let Some(model) = field(message, "model")
    {
        summary.last_model = Some(model.to_owned());
    }
    if record.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
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

/// Codex rollout lines.
pub(crate) fn codex(summary: &mut FileIndex, record: &Value, offset: u64) {
    let time = record_time(record);
    activity(summary, time);
    let payload = &record["payload"];
    match field(record, "type") {
        Some("session_meta") => {
            summary.entrypoint = field(payload, "originator").map(str::to_owned);
            summary.cwd = field(payload, "cwd").map(str::to_owned);
            summary.branch = payload
                .get("git")
                .and_then(|git| field(git, "branch"))
                .map(str::to_owned);
        }
        Some("event_msg") if field(payload, "type") == Some("token_count") => {
            if let Some(usage) = payload
                .get("info")
                .and_then(|info| info.get("total_token_usage"))
            {
                summary.codex_tokens = Tokens {
                    input: number(usage, "input_tokens"),
                    cached_input: number(usage, "cached_input_tokens"),
                    output: number(usage, "output_tokens"),
                    reasoning_output: number(usage, "reasoning_output_tokens"),
                    total: number(usage, "total_tokens"),
                };
            }
        }
        // A finished command's structured exit code, when the rollout
        // records one for a call.
        Some("event_msg") if field(payload, "type") == Some("item_completed") => {
            let item = &payload["item"];
            if let (Some(id), Some(code)) = (
                field(item, "id"),
                item.get("exit_code").and_then(Value::as_i64),
            ) {
                exit_code(summary, id, code, offset, time);
            }
        }
        Some("turn_context") => {
            if let Some(model) = field(payload, "model") {
                summary.last_model = Some(model.to_owned());
            }
        }
        Some("event_msg") if field(payload, "type") == Some("task_complete") => {
            summary.failed = payload.get("error").is_some_and(|error| !error.is_null());
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
                    Some("user")
                        if !text.trim_start().starts_with('<') && !text.trim().is_empty() =>
                    {
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
            Some("function_call" | "custom_tool_call" | "local_shell_call") => tool_event(
                summary,
                Event {
                    k: Kind::Tool,
                    o: offset,
                    t: time,
                    id: field(payload, "call_id").map(str::to_owned),
                    n: Some(field(payload, "name").unwrap_or("shell").to_owned()),
                    ..Event::default()
                },
            ),
            Some("function_call_output" | "custom_tool_call_output") => {
                if let Some(id) = field(payload, "call_id") {
                    let output = payload.get("output");
                    let code = codex_exit(output);
                    let mut flags = if code.is_none() { UNKNOWN } else { 0 };
                    if acknowledgement(output) {
                        flags |= ACK;
                    }
                    resolve(summary, id, |_| Reply {
                        o: offset,
                        t: time,
                        e: code.is_some_and(|code| code != 0),
                        f: flags,
                        ..Reply::default()
                    });
                }
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
}
