//! `/api/tx`: one session's transcript, a page at a time, in the mockup's
//! `TX` entry shape (`{k:"a"|"tool"|"think"|"h"|"u"|"harness"|"end", …}`).
//!
//! The model keeps each session's transcript as offsets only (its slots);
//! the text of a page is read back from the source lines here, capped as in
//! the mockup data, and never kept.

use std::{
    collections::VecDeque,
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    rc::Rc,
};

use serde_json::{Map, Value, json};

use crate::{
    field,
    model::{
        self, Built, MSG_MAX, Shown, Slot, SlotFile, SlotKind, arg_summary, cap, content_text,
        one_line, patch_text, prompt_text, tool_input, tool_result_text,
    },
};

/// A page holds at most this many entries...
pub(crate) const PAGE_ENTRIES: usize = 200;
/// ...and at most this many bytes of JSON.
pub(crate) const PAGE_BYTES: usize = 2 * 1024 * 1024;
/// Tool previews (input, output, diff) are cut here, as in the mockup data.
pub(crate) const PREVIEW_MAX: usize = 1536;
/// "View all" reads up to this much of one call.
pub(crate) const FULL_MAX: usize = 8 * 1024 * 1024;
/// A source line longer than this is not read back.
const LINE_MAX: u64 = FULL_MAX as u64 + 1024 * 1024;

const SHELLS: [&str; 4] = ["Bash", "shell", "exec_command", "local_shell"];
const EDITS: [&str; 5] = ["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"];

/// Which page of a transcript to return.
pub(crate) enum Anchor {
    /// The last page.
    Last,
    /// The page before this slot index.
    Before(usize),
    /// The page from this slot index on.
    After(usize),
    /// The page starting at this turn's first entry.
    Turn(String),
}

impl Anchor {
    /// The page `before`, `after` or `turn` names; with none of them, the
    /// last page. `None` when more than one is given: they are exclusive.
    pub(crate) fn of(
        before: Option<usize>,
        after: Option<usize>,
        turn: Option<String>,
    ) -> Option<Self> {
        match (before, after, turn) {
            (None, None, None) => Some(Self::Last),
            (Some(before), None, None) => Some(Self::Before(before)),
            (None, Some(after), None) => Some(Self::After(after)),
            (None, None, Some(turn)) => Some(Self::Turn(turn)),
            _ => None,
        }
    }
}

pub(crate) fn read_record(path: &Path, offset: u64) -> Option<Value> {
    read_sized(path, offset).0
}

/// The record at `offset`, and how many bytes were read for it.
fn read_sized(path: &Path, offset: u64) -> (Option<Value>, u64) {
    let Ok(mut file) = fs::File::open(path) else {
        return (None, 0);
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (None, 0);
    }
    let mut reader = BufReader::new(file).take(LINE_MAX + 1);
    let mut bytes = Vec::new();
    let read = reader.read_until(b'\n', &mut bytes);
    let size = bytes.len() as u64;
    if read.is_err() || size > LINE_MAX {
        return (None, size);
    }
    (
        serde_json::from_slice(&bytes).ok().filter(Value::is_object),
        size,
    )
}

/// Source lines read for one page: a line holding several blocks, or a call
/// and its result, is read once. Only the last few lines are kept.
#[derive(Default)]
pub(crate) struct Lines {
    recent: VecDeque<(LineKey, Option<Rc<Value>>)>,
    /// Bytes read from source files so far.
    pub(crate) bytes: u64,
}

/// A source line: its file and byte offset.
type LineKey = (PathBuf, u64);

impl Lines {
    const KEEP: usize = 16;

    fn get(&mut self, path: &Path, offset: u64) -> Option<Rc<Value>> {
        if let Some((_, record)) = self
            .recent
            .iter()
            .find(|((seen, at), _)| *at == offset && seen == path)
        {
            return record.clone();
        }
        let (record, size) = read_sized(path, offset);
        self.bytes += size;
        let record = record.map(Rc::new);
        if self.recent.len() == Self::KEEP {
            self.recent.pop_front();
        }
        self.recent
            .push_back(((path.to_owned(), offset), record.clone()));
        record
    }
}

/// Cuts `text` to `limit` bytes on a character boundary; `true` when cut.
pub(crate) fn clip(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", text[..end].trim_end()), true)
}

/// A step's duration: tenths of a second under a minute, then minutes.
fn secs(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m {}s", ms / 60_000, ms / 1000 % 60)
    }
}

/// How long a running step has run.
fn running(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn tool_name(record: &Value, block: usize) -> Option<String> {
    record
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.get(block))
        .and_then(|item| field(item, "name"))
        .or_else(|| {
            record
                .get("payload")
                .and_then(|payload| field(payload, "name"))
        })
        .map(str::to_owned)
}

fn command(input: &Value) -> Option<String> {
    match input.get("command").or_else(|| input.get("cmd"))? {
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// What a call was asked, when its one-line summary doesn't already say it
/// all: a shell command longer than its summary, or a structured input.
/// Edits show a diff instead.
pub(crate) fn tool_in(name: &str, input: &Value, arg: &str) -> Option<String> {
    if SHELLS.contains(&name) {
        let command = command(input)?;
        // Shown when the summary (one line, cut at 160) isn't the command.
        return (command.trim() != arg).then_some(command);
    }
    if EDITS.contains(&name) {
        return None;
    }
    match input {
        Value::Null => None,
        Value::Object(fields) if fields.len() <= 1 => None,
        Value::String(text) if one_line(text, 160) == arg => None,
        Value::String(text) => Some(text.clone()),
        other => serde_json::to_string_pretty(other).ok(),
    }
}

/// An edit as diff rows (`["ctx"|"del"|"add", line]`): the lines both
/// sides share at the start and end are context.
fn edit_rows(old: &str, new: &str, rows: &mut Vec<(&'static str, String)>) {
    let old: Vec<&str> = old.split('\n').collect();
    let new: Vec<&str> = new.split('\n').collect();
    let mut head = 0;
    while head < old.len() && head < new.len() && old[head] == new[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < old.len() - head
        && tail < new.len() - head
        && old[old.len() - 1 - tail] == new[new.len() - 1 - tail]
    {
        tail += 1;
    }
    rows.extend(old[..head].iter().map(|line| ("ctx", format!(" {line}"))));
    rows.extend(
        old[head..old.len() - tail]
            .iter()
            .map(|line| ("del", format!("-{line}"))),
    );
    rows.extend(
        new[head..new.len() - tail]
            .iter()
            .map(|line| ("add", format!("+{line}"))),
    );
    rows.extend(
        old[old.len() - tail..]
            .iter()
            .map(|line| ("ctx", format!(" {line}"))),
    );
}

/// An edit's rows, cut at `budget` bytes; `true` when cut.
pub(crate) fn diff_rows(name: &str, input: &Value, budget: usize) -> Option<(Value, bool)> {
    let mut rows = Vec::new();
    match name {
        "Edit" => edit_rows(
            field(input, "old_string")?,
            field(input, "new_string").unwrap_or(""),
            &mut rows,
        ),
        "MultiEdit" => {
            for edit in input.get("edits")?.as_array()? {
                edit_rows(
                    field(edit, "old_string").unwrap_or(""),
                    field(edit, "new_string").unwrap_or(""),
                    &mut rows,
                );
            }
        }
        "apply_patch" => {
            for line in patch_text(input)?.lines() {
                let kind = match line.as_bytes().first() {
                    _ if line.starts_with("***") || line.starts_with("@@") => continue,
                    Some(b'+') => "add",
                    Some(b'-') => "del",
                    Some(b' ') => "ctx",
                    _ => continue,
                };
                rows.push((kind, line.to_owned()));
            }
        }
        _ => return None,
    }
    if rows.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut used = 0;
    let mut cut = false;
    for (kind, line) in rows {
        if used >= budget {
            cut = true;
            out.push(json!(["ctx", "…"]));
            break;
        }
        let (line, clipped) = clip(&line, budget - used);
        used += line.len() + 1;
        cut |= clipped;
        out.push(json!([kind, line]));
    }
    Some((Value::Array(out), cut))
}

/// A tool result's text: a Claude `tool_result` block, or a Codex call's
/// output (its `output` field when the output is structured).
pub(crate) fn result_text(record: &Value, block: usize) -> Option<String> {
    if let Some(payload) = record.get("payload") {
        let output = payload.get("output")?;
        let structured = match output {
            Value::String(text) => serde_json::from_str::<Value>(text).ok(),
            other => Some(other.clone()),
        };
        if let Some(text) = structured.as_ref().and_then(|value| field(value, "output")) {
            return Some(text.to_owned());
        }
        return Some(match output {
            Value::String(text) => text.clone(),
            other => content_text(other),
        });
    }
    tool_result_text(record, block)
}

fn think_text(record: &Value, block: usize) -> Option<String> {
    if let Some(payload) = record.get("payload") {
        let text = payload
            .get("summary")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| field(part, "text"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        return Some(text);
    }
    let item = record
        .get("message")?
        .get("content")?
        .as_array()?
        .get(block)?;
    field(item, "thinking").map(str::to_owned)
}

#[allow(clippy::too_many_arguments)]
fn tool_entry(
    lines: &mut Lines,
    file: &SlotFile,
    slot: &Slot,
    index: usize,
    shown: Shown,
    name: &str,
    reply: Option<&crate::events::Reply>,
    now: i64,
    home: Option<&str>,
) -> Value {
    let record = lines.get(&file.path, slot.offset);
    let input = record
        .as_ref()
        .and_then(|record| tool_input(record, slot.block as usize))
        .unwrap_or(Value::Null);
    let arg = arg_summary(name, &input, file.cwd.as_deref(), home);
    let mut entry = Map::new();
    entry.insert("k".into(), json!("tool"));
    entry.insert("name".into(), json!(name));
    entry.insert("arg".into(), json!(arg));
    let mut more = Vec::new();
    if let Some(text) = tool_in(name, &input, &arg) {
        let (text, cut) = clip(&text, PREVIEW_MAX);
        entry.insert("in".into(), json!(text));
        if cut {
            more.push("in");
        }
    }
    let mut result = || {
        reply.and_then(|reply| {
            lines
                .get(&file.path, reply.o)
                .and_then(|record| result_text(&record, reply.b as usize))
        })
    };
    match shown {
        Shown::Live => {
            entry.insert("live".into(), json!(true));
            entry.insert(
                "secs".into(),
                json!(slot.t.map_or_else(|| "—".to_owned(), |t| running(now - t))),
            );
        }
        Shown::Unfinished => {
            entry.insert("ok".into(), json!(false));
            entry.insert("unfinished".into(), json!(true));
            entry.insert("secs".into(), json!("—"));
        }
        Shown::Ok | Shown::Err | Shown::Unknown => {
            entry.insert(
                "ok".into(),
                match shown {
                    Shown::Ok => json!(true),
                    Shown::Err => json!(false),
                    _ => Value::Null,
                },
            );
            let took = match (slot.t, reply.and_then(|reply| reply.t)) {
                (Some(start), Some(end)) => secs(end - start),
                _ => "—".to_owned(),
            };
            entry.insert("secs".into(), json!(took));
            let diff = if shown == Shown::Err {
                None
            } else {
                diff_rows(name, &input, PREVIEW_MAX)
            };
            if let Some((rows, cut)) = diff {
                entry.insert("diff".into(), rows);
                if cut {
                    more.push("diff");
                }
            } else if let Some(text) = result().filter(|text| !text.trim().is_empty()) {
                let (text, cut) = clip(&text, PREVIEW_MAX);
                entry.insert("out".into(), json!(text));
                if cut {
                    more.push("out");
                }
            }
        }
    }
    if !more.is_empty() {
        entry.insert("more".into(), json!(more));
    }
    // View all asks for this slot's parts by its index; the server finds the
    // file and offsets itself.
    entry.insert("slot".into(), json!(index));
    Value::Object(entry)
}

/// One slot as a `TX` entry; `None` for a slot with nothing to show.
fn render(built: &Built, lines: &mut Lines, slot: &Slot, index: usize, now: i64) -> Option<Value> {
    let file = slot.file.and_then(|file| built.files.get(file));
    let mut record = || file.and_then(|file| lines.get(&file.path, slot.offset));
    let entry = match &slot.kind {
        SlotKind::H(id) => json!({"k": "h", "id": id}),
        SlotKind::U => {
            json!({"k": "u", "text": cap(&record().and_then(|record| prompt_text(&record)).unwrap_or_default(), MSG_MAX)})
        }
        SlotKind::A => {
            json!({"k": "a", "text": cap(&record().and_then(|record| model::assistant_text(&record, slot.block as usize)).unwrap_or_default(), MSG_MAX)})
        }
        SlotKind::Think => {
            let text = record()
                .and_then(|record| think_text(&record, slot.block as usize))
                .unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            json!({"k": "think", "text": cap(text.trim(), MSG_MAX)})
        }
        SlotKind::Harness(label) => json!({"k": "harness", "label": label}),
        SlotKind::Gap => {
            json!({"k": "end", "text": "Some entries not included: the log has unreadable lines here"})
        }
        SlotKind::NoActivity => json!({"k": "end", "text": "No activity in these logs"}),
        SlotKind::Returned { to, at, failed } => {
            json!({"k": "end", "ret": {"to": to, "at": at, "failed": failed}})
        }
        SlotKind::Tool { shown, name, reply } => tool_entry(
            lines,
            file?,
            slot,
            index,
            *shown,
            name,
            reply.as_ref(),
            now,
            built.home.as_deref(),
        ),
    };
    Some(entry)
}

/// Every string a value holds, one per line: a tool input's text, whatever
/// its shape.
fn strings(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(fields) => fields.values().for_each(|item| strings(item, out)),
        _ => {}
    }
}

/// A slot's text, whole, read back from its source line for search: the
/// same parts a page shows (`text`, or a call's `in` and `out`), uncapped.
/// A handoff's text is the model's (its brief, result and answer), so a
/// handoff slot has none here; neither do markers.
pub(crate) fn slot_texts(
    built: &Built,
    lines: &mut Lines,
    slot: &Slot,
) -> Vec<(&'static str, String)> {
    let Some(file) = slot.file.and_then(|file| built.files.get(file)) else {
        return Vec::new();
    };
    let block = slot.block as usize;
    let mut texts = Vec::new();
    let text = match &slot.kind {
        SlotKind::U => lines
            .get(&file.path, slot.offset)
            .and_then(|record| prompt_text(&record)),
        SlotKind::A => lines
            .get(&file.path, slot.offset)
            .and_then(|record| model::assistant_text(&record, block)),
        SlotKind::Think => lines
            .get(&file.path, slot.offset)
            .and_then(|record| think_text(&record, block)),
        SlotKind::Tool { reply, .. } => {
            if let Some(input) = lines
                .get(&file.path, slot.offset)
                .and_then(|record| tool_input(&record, block))
            {
                let mut text = String::new();
                strings(&input, &mut text);
                texts.push(("in", text));
            }
            if let Some(reply) = reply
                && let Some(text) = lines
                    .get(&file.path, reply.o)
                    .and_then(|record| result_text(&record, reply.b as usize))
            {
                texts.push(("out", text));
            }
            None
        }
        _ => None,
    };
    texts.extend(text.map(|text| ("text", text)));
    texts
}

/// A page of `sid`'s transcript as JSON. Each entry that starts a turn, and
/// the page's first entry, carries its turn's id as `turn`.
pub(crate) fn page(built: &Built, sid: &str, anchor: &Anchor, now: i64) -> io::Result<String> {
    page_limited(built, sid, anchor, now, PAGE_ENTRIES)
}

/// [`page`] with at most `limit` entries (1 to [`PAGE_ENTRIES`]); the byte
/// bound is the same.
pub(crate) fn page_limited(
    built: &Built,
    sid: &str,
    anchor: &Anchor,
    now: i64,
    limit: usize,
) -> io::Result<String> {
    let limit = limit.clamp(1, PAGE_ENTRIES);
    let transcript = built.tx.get(sid).ok_or(io::ErrorKind::NotFound)?;
    let slots = &transcript.slots;
    let total = slots.len();
    let first_of = |turn: &str| {
        slots
            .iter()
            .position(|slot| slot.first && slot.turn.as_deref() == Some(turn))
    };
    let (forward, from) = match anchor {
        Anchor::Last => (false, total),
        Anchor::Before(before) => (false, (*before).min(total)),
        Anchor::After(after) => (true, (*after).min(total)),
        Anchor::Turn(turn) => (true, first_of(turn).ok_or(io::ErrorKind::NotFound)?),
    };
    let mut lines = Lines::default();
    let mut picked: Vec<(usize, Value)> = Vec::new();
    let mut bytes = 0;
    let (mut low, mut high) = (from, from);
    let mut take = |index: usize, picked: &mut Vec<(usize, Value)>| -> bool {
        let Some(entry) = render(built, &mut lines, &slots[index], index, now) else {
            return true;
        };
        let size = entry.to_string().len() + 1;
        if !picked.is_empty() && (picked.len() >= limit || bytes + size > PAGE_BYTES) {
            return false;
        }
        bytes += size;
        picked.push((index, entry));
        true
    };
    if forward {
        while high < total && take(high, &mut picked) {
            high += 1;
        }
    } else {
        while low > 0 && take(low - 1, &mut picked) {
            low -= 1;
        }
        picked.reverse();
    }
    let mut entries = Vec::with_capacity(picked.len());
    for (position, (index, mut entry)) in picked.into_iter().enumerate() {
        let slot = &slots[index];
        if let Some(turn) = &slot.turn
            && (slot.first || position == 0)
        {
            entry["turn"] = json!(turn);
        }
        entries.push(entry);
    }
    Ok(json!({
        "sid": sid,
        "from": low,
        "to": high,
        "total": total,
        "calls": transcript.calls,
        "errors": transcript.errors,
        "entries": entries,
    })
    .to_string())
}

/// The whole of one part of a session's tool call, by its slot, for "View
/// all": what it was asked (`in`), what came back (`out`) or its diff.
pub(crate) fn full_slot(built: &Built, sid: &str, index: usize, part: &str) -> io::Result<String> {
    let slot = built
        .tx
        .get(sid)
        .and_then(|transcript| transcript.slots.get(index))
        .ok_or(io::ErrorKind::NotFound)?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "not a tool call");
    let SlotKind::Tool { reply, .. } = &slot.kind else {
        return Err(invalid());
    };
    let file = slot
        .file
        .and_then(|file| built.files.get(file))
        .ok_or_else(invalid)?;
    match part {
        "out" => {
            let reply = reply.as_ref().ok_or_else(invalid)?;
            full(&file.path, reply.o, reply.b as usize, part)
        }
        "in" | "diff" => full(&file.path, slot.offset, slot.block as usize, part),
        _ => Err(invalid()),
    }
}

/// The whole of one part of a tool call at a line: what it was asked (`in`),
/// what came back (`out`, the result's line), or its diff.
fn full(path: &Path, offset: u64, block: usize, part: &str) -> io::Result<String> {
    let record = read_record(path, offset)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entry"))?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "entry is not expandable");
    match part {
        "in" => {
            let name = tool_name(&record, block).ok_or_else(invalid)?;
            let input = tool_input(&record, block).ok_or_else(invalid)?;
            let text = if SHELLS.contains(&name.as_str()) {
                command(&input).ok_or_else(invalid)?
            } else {
                match input {
                    Value::String(text) => text,
                    other => serde_json::to_string_pretty(&other).unwrap_or_default(),
                }
            };
            let (text, truncated) = clip(&text, FULL_MAX);
            Ok(json!({"text": text, "truncated": truncated}).to_string())
        }
        "out" => {
            let text = result_text(&record, block).ok_or_else(invalid)?;
            let (text, truncated) = clip(&text, FULL_MAX);
            Ok(json!({"text": text, "truncated": truncated}).to_string())
        }
        "diff" => {
            let name = tool_name(&record, block).ok_or_else(invalid)?;
            let input = tool_input(&record, block).ok_or_else(invalid)?;
            let (rows, truncated) = diff_rows(&name, &input, FULL_MAX).ok_or_else(invalid)?;
            Ok(json!({"diff": rows, "truncated": truncated}).to_string())
        }
        _ => Err(invalid()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use serde_json::{Value, json};

    use super::*;
    use crate::{
        Options,
        events::EventCache,
        model::{Texts, build},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    /// 2026-09-24T00:00:00Z.
    const BASE: i64 = 1_790_208_000_000;

    fn ts(hour: i64, minute: i64, millis: i64) -> String {
        let at = BASE + (hour * 60 + minute) * 60_000 + millis;
        let seconds = at / 1000 % 60;
        format!(
            "2026-09-24T{hour:02}:{minute:02}:{seconds:02}.{:03}Z",
            at % 1000
        )
    }

    struct Home {
        root: PathBuf,
        options: Options,
    }

    impl Home {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "semon-tx-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let options = Options {
                claude_home: root.join("claude"),
                codex_home: root.join("codex"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: Duration::from_secs(86400),
                session: None,
                facts: None,
            };
            let home = Self { root, options };
            home.write("proc/locks", "");
            home.write("proc/sys/kernel/hostname", "testbox\n");
            home
        }

        fn write(&self, relative: &str, content: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            path
        }

        fn lines(&self, relative: &str, records: &[Value]) -> PathBuf {
            let text: String = records.iter().map(|record| format!("{record}\n")).collect();
            self.write(relative, &text)
        }

        fn built(&self, now: i64) -> Built {
            let mut cache = EventCache::default();
            build(
                &self.options,
                &mut cache,
                &mut false,
                &mut Texts::default(),
                now,
            )
            .unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn claude(sid: &str, time: String, kind: &str, extra: Value) -> Value {
        let mut record =
            json!({"type": kind, "timestamp": time, "sessionId": sid, "cwd": "/work/proj"});
        for (key, value) in extra.as_object().unwrap() {
            record[key] = value.clone();
        }
        record
    }

    fn said(sid: &str, time: String, blocks: Value) -> Value {
        claude(
            sid,
            time,
            "assistant",
            json!({"message": {"role": "assistant", "model": "claude-opus-5-5", "content": blocks}}),
        )
    }

    fn result(sid: &str, time: String, id: &str, content: &str, error: bool) -> Value {
        claude(
            sid,
            time,
            "user",
            json!({"message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": content, "is_error": error}]}}),
        )
    }

    fn ask(sid: &str, time: String, text: &str) -> Value {
        claude(
            sid,
            time,
            "user",
            json!({"origin": {"kind": "human"}, "message": {"role": "user", "content": text}}),
        )
    }

    fn page_of(built: &Built, sid: &str, anchor: &Anchor) -> Value {
        serde_json::from_str(&page(built, sid, anchor, BASE + 86_400_000).unwrap()).unwrap()
    }

    #[test]
    fn a_page_has_the_mockup_tx_shape() {
        let home = Home::new();
        let long = format!("{}\nend", "line\n".repeat(PREVIEW_MAX));
        home.lines(
            "claude/projects/-work-proj/lane.jsonl",
            &[
                ask("lane", ts(1, 0, 0), "Fix the <script>alert(1)</script> test"),
                said(
                    "lane",
                    ts(1, 0, 900),
                    json!([{"type": "thinking", "thinking": "Where does it overflow?", "signature": "s"}]),
                ),
                said(
                    "lane",
                    ts(1, 1, 0),
                    json!([{"type": "text", "text": "Running it."}]),
                ),
                said(
                    "lane",
                    ts(1, 1, 0),
                    json!([{"type": "tool_use", "id": "b1", "name": "Bash", "input": {"command": "cd /work/proj && cargo test"}}]),
                ),
                result("lane", ts(1, 1, 1500), "b1", &long, true),
                said(
                    "lane",
                    ts(1, 2, 0),
                    json!([{"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "/work/proj/src/a.rs", "old_string": "fn a() {\n    old();", "new_string": "fn a() {\n    new();"}}]),
                ),
                result("lane", ts(1, 2, 0), "e1", "updated", false),
                said(
                    "lane",
                    ts(1, 3, 0),
                    json!([{"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "/work/proj/src/b.rs"}}]),
                ),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "lane", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        let kinds: Vec<&str> = entries
            .iter()
            .map(|entry| entry["k"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["h", "think", "a", "tool", "tool", "tool"]);
        assert!(
            entries[0]["turn"].is_string(),
            "the page's first entry names its turn"
        );
        assert_eq!(entries[1]["text"], "Where does it overflow?");
        let bash = &entries[3];
        assert_eq!(bash["arg"], "cd /work/proj && cargo test");
        assert_eq!(bash["ok"], false);
        assert_eq!(bash["secs"], "1.5s");
        assert!(
            bash.get("in").is_none(),
            "a command its summary already shows has no input preview"
        );
        let out = bash["out"].as_str().unwrap();
        assert!(out.len() <= PREVIEW_MAX + '…'.len_utf8() && out.ends_with('…'));
        assert_eq!(bash["more"], json!(["out"]));
        assert_eq!(bash["slot"], 3);
        let edit = &entries[4];
        assert_eq!(edit["arg"], "src/a.rs");
        assert_eq!(
            edit["diff"],
            json!([
                ["ctx", " fn a() {"],
                ["del", "-    old();"],
                ["add", "+    new();"]
            ])
        );
        let read = &entries[5];
        assert_eq!(read["ok"], false);
        assert_eq!(read["unfinished"], true);
        assert_eq!(read["secs"], "—");
        assert_eq!(
            (
                page["from"].as_u64(),
                page["to"].as_u64(),
                page["total"].as_u64()
            ),
            (Some(0), Some(6), Some(6))
        );
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(3), Some(2))
        );

        // View all reads each part in full, by the slot.
        let full_of = |slot: usize, part: &str| -> Value {
            serde_json::from_str(&full_slot(&built, "lane", slot, part).unwrap()).unwrap()
        };
        assert_eq!(full_of(3, "out")["text"], long.as_str());
        assert_eq!(full_of(3, "out")["truncated"], false);
        assert_eq!(full_of(3, "in")["text"], "cd /work/proj && cargo test");
        assert_eq!(full_of(4, "diff")["diff"][2], json!(["add", "+    new();"]));
        assert!(full_slot(&built, "lane", 3, "nope").is_err());
        assert!(
            full_slot(&built, "lane", 2, "in").is_err(),
            "not a tool call"
        );
        assert!(full_slot(&built, "lane", 5, "out").is_err(), "no result");
        assert!(full_slot(&built, "lane", 99, "in").is_err());
        assert!(full_slot(&built, "nobody", 0, "in").is_err());
    }

    #[test]
    fn pages_are_bounded_and_page_back_without_gaps() {
        let home = Home::new();
        let mut records = Vec::new();
        for number in 0..450 {
            if number % 50 == 0 {
                records.push(ask(
                    "many",
                    ts(2, number / 60, (number % 60) * 1000),
                    &format!("ask {number}"),
                ));
            }
            records.push(said(
                "many",
                ts(2, number / 60, (number % 60) * 1000 + 1),
                json!([{"type": "tool_use", "id": format!("t{number}"), "name": "Grep", "input": {"pattern": format!("p{number}")}}]),
            ));
            records.push(result(
                "many",
                ts(2, number / 60, (number % 60) * 1000 + 2),
                &format!("t{number}"),
                "hit",
                false,
            ));
        }
        home.lines("claude/projects/-work-proj/many.jsonl", &records);
        let built = home.built(BASE + 86_400_000);
        let total = built.tx["many"].slots.len();
        let mut seen = Vec::new();
        let mut anchor = Anchor::Last;
        loop {
            let page = page_of(&built, "many", &anchor);
            let entries = page["entries"].as_array().unwrap();
            assert!(!entries.is_empty() && entries.len() <= PAGE_ENTRIES);
            assert!(page.to_string().len() <= PAGE_BYTES + 1024);
            assert!(entries[0]["turn"].is_string());
            let from = page["from"].as_u64().unwrap() as usize;
            let to = page["to"].as_u64().unwrap() as usize;
            assert_eq!(to - from, entries.len());
            seen.splice(0..0, from..to);
            if from == 0 {
                break;
            }
            anchor = Anchor::Before(from);
        }
        assert_eq!(seen, (0..total).collect::<Vec<_>>());
        // A turn's page starts at its first entry, and later pages follow on.
        let turn = built.tx["many"].slots[250..]
            .iter()
            .find(|slot| slot.first)
            .and_then(|slot| slot.turn.clone())
            .unwrap();
        let page = page_of(&built, "many", &Anchor::Turn(turn.clone()));
        assert_eq!(page["entries"][0]["turn"], turn.as_str());
        let next = page_of(
            &built,
            "many",
            &Anchor::After(page["to"].as_u64().unwrap() as usize),
        );
        assert_eq!(next["from"], page["to"]);
        assert_eq!(
            super::page(&built, "many", &Anchor::Turn("missing".into()), 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(super::page(&built, "nobody", &Anchor::Last, 0).is_err());
    }

    #[test]
    fn codex_harness_text_and_a_failed_return() {
        let home = Home::new();
        home.lines(
            "claude/projects/-work-proj/lead.jsonl",
            &[
                ask("lead", ts(3, 0, 0), "Hand it to Codex"),
                said(
                    "lead",
                    ts(3, 1, 0),
                    json!([{"type": "tool_use", "id": "bash-1", "name": "Bash", "input": {"command": "codex exec < /tmp/p.md"}}]),
                ),
            ],
        );
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-run.jsonl",
            &[
                codex(ts(3, 1, 0), "session_meta", json!({"id": "run", "cwd": "/work/proj"})),
                codex(ts(3, 1, 0), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Semon-Parent: claude:lead:bash-1\nImplement it"}]})),
                codex(ts(3, 1, 1), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<recommended_plugins>x</recommended_plugins>"}]})),
                codex(ts(3, 1, 2), "response_item", json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": "Plan it"}]})),
                codex(ts(3, 2, 0), "response_item", json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "c1", "input": "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n*** End Patch"})),
                codex(ts(3, 2, 0), "response_item", json!({"type": "custom_tool_call_output", "call_id": "c1", "output": "{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"})),
                codex(ts(3, 3, 0), "response_item", json!({"type": "function_call", "name": "shell", "call_id": "c2", "arguments": "{\"command\":[\"make\"]}"})),
                codex(ts(3, 3, 500), "response_item", json!({"type": "function_call_output", "call_id": "c2", "output": "no status"})),
                codex(ts(3, 4, 0), "response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done"}]})),
                codex(ts(3, 4, 0), "event_msg", json!({"type": "task_complete", "error": {"message": "failed"}})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "run", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        let kinds: Vec<&str> = entries
            .iter()
            .map(|entry| entry["k"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["h", "harness", "think", "tool", "tool", "a", "end"]);
        assert_eq!(entries[1]["label"], "recommended_plugins");
        assert_eq!(entries[2]["text"], "Plan it");
        assert_eq!(entries[3]["arg"], "src/a.rs");
        assert_eq!(
            entries[3]["diff"],
            json!([["del", "-old"], ["add", "+new"]])
        );
        assert_eq!(
            entries[4]["ok"],
            Value::Null,
            "no exit status: neither ok nor failed"
        );
        assert_eq!(entries[4]["out"], "no status");
        assert_eq!(entries[6]["ret"]["to"], "lead");
        assert_eq!(entries[6]["ret"]["failed"], true);
    }

    #[test]
    fn every_field_passes_content_as_data() {
        let home = Home::new();
        let payload =
            "<img src=x onerror=alert(1)><script>alert(2)</script><iframe src=javascript:alert(3)>";
        // A session key comes from a file name, which can't hold a slash.
        let key = "<img src=x onerror=alert(4)>";
        home.write(
            "proc/1/stat",
            &format!("1 (claude) {}\n", ["0"; 19].join(" ") + " 7"),
        );
        home.write(
            "claude/sessions/1.json",
            &json!({"pid": 1, "sessionId": key, "procStart": 7, "status": "busy", "name": payload})
                .to_string(),
        );
        home.lines(
            &format!("claude/projects/-work-proj/{key}.jsonl"),
            &[
                claude(key, ts(4, 0, 0), "user", json!({"origin": {"kind": "human"}, "gitBranch": payload, "message": {"role": "user", "content": payload}})),
                claude(key, ts(4, 0, 1), "user", json!({"message": {"role": "user", "content": format!("note {payload}")}})),
                said(key, ts(4, 0, 2), json!([{"type": "thinking", "thinking": payload, "signature": "s"}])),
                said(key, ts(4, 1, 0), json!([{"type": "text", "text": payload}])),
                said(key, ts(4, 2, 0), json!([{"type": "tool_use", "id": "x1", "name": payload, "input": {"a": payload, "b": payload}}])),
                result(key, ts(4, 2, 1), "x1", payload, false),
                said(key, ts(4, 3, 0), json!([{"type": "tool_use", "id": "x2", "name": "Edit", "input": {"file_path": "/work/proj/a.rs", "old_string": payload, "new_string": format!("{payload}!")}}])),
                result(key, ts(4, 3, 1), "x2", "ok", false),
                said(key, ts(4, 4, 0), json!([{"type": "tool_use", "id": "x3", "name": "SendMessage", "input": {"to": payload, "message": payload}}])),
                claude(key, ts(4, 4, 1), "user", json!({"toolUseResult": {"success": false}, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x3", "content": "no such peer"}]}})),
                said(key, ts(4, 5, 0), json!([{"type": "tool_use", "id": "x4", "name": "AskUserQuestion", "input": {"questions": [{"question": payload, "header": "h", "multiSelect": false, "options": [{"label": payload}, {"label": "b"}]}]}}])),
                claude(key, ts(4, 5, 1), "user", json!({"toolUseResult": {"answers": {payload.to_string(): payload}}, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x4", "content": "answered"}]}})),
                said(key, ts(4, 6, 0), json!([{"type": "tool_use", "id": "x5", "name": "Agent", "input": {"description": payload, "prompt": payload}}])),
                result(key, ts(4, 7, 0), "x5", payload, false),
                said(key, ts(4, 8, 0), json!([{"type": "tool_use", "id": "x6", "name": "Bash", "input": {"command": payload}}])),
            ],
        );
        home.lines(
            &format!("claude/projects/-work-proj/{key}/subagents/agent-sub.jsonl"),
            &[
                claude(
                    "sub",
                    ts(4, 6, 0),
                    "user",
                    json!({"message": {"role": "user", "content": format!("brief {payload}")}}),
                ),
                said(
                    "sub",
                    ts(4, 6, 30),
                    json!([{"type": "text", "text": payload}]),
                ),
            ],
        );
        home.write(
            &format!("claude/projects/-work-proj/{key}/subagents/agent-sub.meta.json"),
            &json!({"description": payload, "toolUseId": "x5"}).to_string(),
        );
        home.lines(
            "codex/sessions/2026/09/24/rollout-cx.jsonl",
            &[
                json!({"timestamp": ts(4, 0, 0), "type": "session_meta", "payload": {"id": "cx", "cwd": "/work/proj", "git": {"branch": payload}}}),
                json!({"timestamp": ts(4, 0, 1), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": payload}]}}),
                json!({"timestamp": ts(4, 0, 2), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": format!("<recommended_plugins>{payload}</recommended_plugins>")}]}}),
                json!({"timestamp": ts(4, 0, 3), "type": "response_item", "payload": {"type": "reasoning", "summary": [{"type": "summary_text", "text": payload}]}}),
            ],
        );
        let now = BASE + 4 * 3_600_000 + 9 * 60_000;
        let built = home.built(now);
        let entries = |sid: &str| {
            page_of(&built, sid, &Anchor::Last)["entries"]
                .as_array()
                .unwrap()
                .clone()
        };

        let lane = entries(key);
        let find = |kind: &str| {
            lane.iter()
                .filter(|entry| entry["k"] == kind)
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(find("u")[0]["text"], format!("note {payload}"));
        assert_eq!(find("think")[0]["text"], payload);
        assert_eq!(find("a")[0]["text"], payload);
        let tools = find("tool");
        assert_eq!(tools[0]["name"], payload);
        assert_eq!(tools[0]["arg"], payload);
        assert!(tools[0]["in"].as_str().unwrap().contains(payload));
        assert_eq!(tools[0]["out"], payload);
        assert_eq!(
            tools[1]["diff"],
            json!([
                ["del", format!("-{payload}")],
                ["add", format!("+{payload}!")]
            ])
        );
        assert_eq!(tools[2]["arg"], payload, "a running tool's summary");

        let sub = entries("sub");
        assert_eq!(sub.last().unwrap()["ret"]["to"], key);
        let codex = entries("cx");
        assert_eq!(
            codex[0]["text"], payload,
            "a Codex prompt that starts with < is a prompt"
        );
        assert_eq!(
            codex[1]["label"], "recommended_plugins",
            "a harness label is only a known tag"
        );
        assert_eq!(codex[2]["text"], payload);

        // The failed send's addressee has its own transcript: its send.
        let unsent = entries(&format!("unsent:{payload}"));
        assert_eq!(unsent[0]["text"], "No activity in these logs");
        assert_eq!(unsent[1]["k"], "h");

        // Everything /api/model says, as data.
        let model: Value = serde_json::from_str(&built.json(now)).unwrap();
        let session = &model["sessions"][key];
        assert_eq!(session["name"], payload);
        // A subagent's name is its description on one line, cut at 80.
        assert!(
            model["sessions"]["sub"]["name"]
                .as_str()
                .unwrap()
                .starts_with("<img src=x onerror=alert(1)><script>")
        );
        assert_eq!(
            model["sessions"]["cx"]["branch"],
            Value::Null,
            "no repo, no branch"
        );
        assert_eq!(session["activity"][1], payload);
        let handoff = |kind: &str| {
            model["handoffs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|handoff| handoff["kind"] == kind)
                .unwrap()
                .clone()
        };
        assert_eq!(handoff("ask")["brief"], payload);
        let relay = handoff("relay");
        assert_eq!(
            (relay["target"].as_str(), relay["brief"].as_str()),
            (Some(payload), Some(payload))
        );
        let question = handoff("toyou");
        assert_eq!(
            (question["brief"].as_str(), question["answer"][0].as_str()),
            (Some(payload), Some(payload))
        );
        let spawn = handoff("spawn");
        assert_eq!(
            (spawn["brief"].as_str(), spawn["result"].as_str()),
            (Some(payload), Some(payload))
        );
    }

    #[test]
    fn a_command_longer_than_its_summary_shows_as_input() {
        let long = format!("cd /work/proj && {}", "cargo test ".repeat(20));
        let input = json!({"command": long});
        let arg = arg_summary("Bash", &input, None, None);
        assert!(arg.ends_with('…'));
        assert_eq!(tool_in("Bash", &input, &arg), Some(long));
        let multi = json!({"command": "set -e\nmake"});
        assert_eq!(
            tool_in("Bash", &multi, &arg_summary("Bash", &multi, None, None)).as_deref(),
            Some("set -e\nmake")
        );
        let short = json!({"command": "ls -la"});
        assert_eq!(tool_in("Bash", &short, "ls -la"), None);
    }

    /// Prints every session's last page of a fixture home written by
    /// `tests/ui/fixture.mjs`, to check the fixture without a browser.
    #[test]
    #[ignore = "reads SEMON_SAMPLE_HOME, written by tests/ui/fixture.mjs"]
    fn print_sample_tx() {
        let root = PathBuf::from(env::var("SEMON_SAMPLE_HOME").unwrap());
        let now: i64 = env::var("SEMON_SAMPLE_NOW").unwrap().parse().unwrap();
        let options = Options {
            claude_home: root.join("claude"),
            codex_home: root.join("codex"),
            proc_root: root.join("proc"),
            cache: root.join("tx-index.json"),
            all: false,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
        };
        let mut cache = EventCache::default();
        let built = build(&options, &mut cache, &mut false, &mut Texts::default(), now).unwrap();
        let mut all = serde_json::Map::new();
        for sid in built.tx.keys() {
            let page: Value =
                serde_json::from_str(&page(&built, sid, &Anchor::Last, now).unwrap()).unwrap();
            all.insert(sid.clone(), page);
        }
        println!("{}", Value::Object(all));
    }
}
