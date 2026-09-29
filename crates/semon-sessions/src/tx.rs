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
    events::{self, Reply, ScriptArg},
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

/// Cuts `text` to its last `limit` bytes on a character boundary; `true`
/// when cut.
fn clip_end(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (format!("…{}", text[start..].trim_start()), true)
}

/// A gap Codex left in a command's output, and how much it took out.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    n: u64,
    /// `tokens`, `chars`, `bytes` or `lines`.
    unit: &'static str,
    /// `lines` only: how many lines the output had.
    of: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Text(String),
    Gap(Gap),
}

/// Output Codex cut before the model saw it: what its warning header says of
/// the whole, and the text on both sides of the gap when the gap is found.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Cut {
    original_tokens: Option<u64>,
    lines: Option<u64>,
    /// Text, gap, text; empty when the header is there but no gap was found.
    parts: Vec<Part>,
}

/// The leading digits of `text` as a number, and what follows them.
fn digits(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    Some((text.get(..end)?.parse().ok()?, &text[end..]))
}

/// Codex's warning header, written at the start of an output it cut:
/// `Warning: truncated output (original token count: N)\n`, then
/// `Total output lines: M\n\n` (which older versions wrote alone). Returns
/// the text after the header, the two numbers, and whether there was one.
fn strip_header(text: &str) -> (&str, Option<u64>, Option<u64>, bool) {
    let (mut rest, mut tokens, mut lines, mut found) = (text, None, None, false);
    if let Some(after) = rest.strip_prefix("Warning: truncated output (original token count: ")
        && let Some((n, after)) = digits(after)
        && let Some(after) = after.strip_prefix(")\n")
    {
        (rest, tokens, found) = (after, Some(n), true);
    }
    if let Some(after) = rest.strip_prefix("Total output lines: ")
        && let Some((n, after)) = digits(after)
        && let Some(after) = after.strip_prefix('\n')
    {
        (rest, lines, found) = (after.strip_prefix('\n').unwrap_or(after), Some(n), true);
    }
    (rest, tokens, lines, found)
}

/// Where a gap sits in a text: the bytes the marker takes, and what it says.
struct Marker {
    start: usize,
    end: usize,
    gap: Gap,
}

/// Where a Codex text came from, which decides what in it is Codex's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Not Codex's: nothing is a cut.
    Plain,
    /// What the model saw: a `function_call_output`, a
    /// `custom_tool_call_output` or a poll's chunk. The token cut and the
    /// warning header are Codex's.
    Model,
    /// A `CommandExecution` item's `aggregated_output`, which only the 1 MiB
    /// collection cap touched. Anything else in it that looks like a cut is
    /// text the command printed.
    Collected,
}

/// Unified exec's frame around an output: `Chunk ID: …`, `Wall time: N
/// seconds`, then `Process exited with code N` or `Process running with
/// session ID N`, `Original token count: N`, and `Output:` on a line of its
/// own (codex-rs `core/src/tools/context.rs`, `ExecCommandToolOutput`). Codex's
/// warning header and cut come after it. Returns the text after the frame,
/// whether there was one, and the frame's `Original token count` if it has
/// one. A frame needs its `Wall time:` line.
fn strip_frame(text: &str) -> (&str, bool, Option<u64>) {
    let mut rest = text;
    let mut timed = false;
    let mut count = None;
    while let Some((line, after)) = rest.split_once('\n') {
        if line == "Output:" {
            return if timed {
                (after, true, count)
            } else {
                (text, false, None)
            };
        }
        if let Some(number) = line.strip_prefix("Original token count: ") {
            count = digits(number)
                .filter(|(_, rest)| rest.is_empty())
                .map(|(n, _)| n);
        } else if line.starts_with("Wall time: ") && line.ends_with(" seconds") {
            timed = true;
        } else if ![
            "Chunk ID: ",
            "Process exited with code ",
            "Process running with session ID ",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            break;
        }
        rest = after;
    }
    (text, false, None)
}

/// The gaps Codex left in `body`, in order, by the forms it writes them in:
///
/// - `…N tokens truncated…` or `…N chars truncated…`, in the middle of a line
///   as the model-facing cut writes it, so only after Codex's warning header
///   (`marked`), which says the text was cut;
/// - `\n... N bytes omitted ...\n`, the 1 MiB collection cap, in the middle;
/// - the same marker as the first line, which Codex writes when the cap fell
///   before a token cut (`ExecCommandToolOutput::truncated_output_with_policy`),
///   only where the text's start is Codex's (`anchored`: after its frame or
///   header);
/// - `[... omitted N of M lines ...]` on a line of its own, from Codex before
///   v0.56.
///
/// Only the collection cap's marker in the middle applies to text that was
/// only collected (`model` is false): the other forms are the token cut's.
///
/// Text that only looks like a marker (inside a line, or with no header for
/// the first form) is not one. Codex leaves at most one gap in the middle, so
/// of several matches the one nearest the middle is taken: it keeps the head
/// and the tail about equal.
fn find_gaps(body: &str, marked: bool, anchored: bool, model: bool) -> Vec<Marker> {
    let mut gaps = Vec::new();
    let mut from = 0;
    if anchored
        && let Some(rest) = body.strip_prefix("... ")
        && let Some((n, rest)) = digits(rest)
        && let Some(after) = rest.strip_prefix(" bytes omitted ...\n")
    {
        // After a warning line, Codex writes a blank line, which goes with
        // it. With no warning line the marker's own newline is all there is.
        let end = if marked {
            body.len() - after.strip_prefix('\n').unwrap_or(after).len()
        } else {
            body.len() - after.len()
        };
        gaps.push(Marker {
            start: 0,
            end,
            gap: Gap {
                n,
                unit: "bytes",
                of: None,
            },
        });
        from = end;
    }
    let mut found = Vec::new();
    if marked {
        for (at, _) in body.match_indices('…') {
            if at < from {
                continue;
            }
            let Some((n, rest)) = digits(&body[at + '…'.len_utf8()..]) else {
                continue;
            };
            for (word, unit) in [
                (" tokens truncated…", "tokens"),
                (" chars truncated…", "chars"),
            ] {
                if let Some(after) = rest.strip_prefix(word) {
                    let gap = Gap { n, unit, of: None };
                    found.push(Marker {
                        start: at,
                        end: body.len() - after.len(),
                        gap,
                    });
                }
            }
        }
    }
    for (at, _) in body.match_indices("\n... ") {
        if at < from {
            continue;
        }
        if let Some((n, rest)) = digits(&body[at + "\n... ".len()..])
            && let Some(after) = rest.strip_prefix(" bytes omitted ...\n")
        {
            let gap = Gap {
                n,
                unit: "bytes",
                of: None,
            };
            found.push(Marker {
                start: at,
                end: body.len() - after.len(),
                gap,
            });
        }
    }
    let mut line_start = 0;
    for line in body.split_inclusive('\n') {
        let start = line_start;
        line_start += line.len();
        if !model || start < from {
            continue;
        }
        if let Some(rest) = line.trim_end_matches('\n').strip_prefix("[... omitted ")
            && let Some((n, rest)) = digits(rest)
            && let Some(rest) = rest.strip_prefix(" of ")
            && let Some((of, rest)) = digits(rest)
            && rest == " lines ...]"
        {
            // The blank line Codex writes after the marker goes with it.
            let end = line_start + usize::from(body[line_start..].starts_with('\n'));
            let gap = Gap {
                n,
                unit: "lines",
                of: Some(of),
            };
            found.push(Marker { start, end, gap });
        }
    }
    gaps.extend(
        found
            .into_iter()
            .min_by_key(|marker| (marker.start + marker.end).abs_diff(body.len())),
    );
    gaps
}

/// A Codex output split at the gaps Codex cut, if it did: the text without
/// the warning header (the markers stay in it), and the cut. What is Codex's
/// depends on the [`Source`]: for `Collected` only the collection cap's
/// marker in the middle counts and nothing is stripped. A model-facing text
/// may sit in unified exec's frame, and a code-mode script's result may come
/// as JSON, read as its `output` first. `None` for text with no cut.
pub(crate) fn split_cut(text: &str, source: Source) -> (String, Option<Cut>) {
    let model = source == Source::Model;
    if source == Source::Plain {
        return (text.to_owned(), None);
    }
    let (after_frame, framed, frame_tokens) = if model {
        strip_frame(text)
    } else {
        (text, false, None)
    };
    let (mut rest, mut original_tokens, mut lines, mut marked) = if model {
        strip_header(after_frame)
    } else {
        (after_frame, None, None, false)
    };
    // Behind a frame, Codex's warning carries the count the frame does (both
    // are `ceil(bytes / 4)` of the same output, codex-rs `unified_exec/
    // process_manager.rs` and `tools/context.rs`). A warning line with any
    // other count, or a frame with none, is the command's own first line.
    if framed && marked && (original_tokens.is_none() || original_tokens != frame_tokens) {
        (rest, original_tokens, lines, marked) = (after_frame, None, None, false);
    }
    let mut unwrapped = None;
    if model
        && let Ok(value) = serde_json::from_str::<Value>(rest.trim())
        // A script's result carries these; any other JSON with an `output`
        // field is the command's own text.
        && (value.get("chunk_id").is_some() || value.get("wall_time_seconds").is_some())
        && let Some(output) = field(&value, "output")
    {
        // Codex's own cut of a result's output field has its header inside.
        let (inner, tokens, total, header) = strip_header(output);
        original_tokens = tokens.or(original_tokens);
        lines = total.or(lines);
        marked |= header;
        unwrapped = Some(inner.to_owned());
    }
    let body = unwrapped.as_deref().unwrap_or(rest);
    let markers = find_gaps(body, marked, model && (framed || marked), model);
    if !marked && markers.is_empty() {
        return (text.to_owned(), None);
    }
    let mut cut = Cut {
        original_tokens,
        lines,
        parts: Vec::new(),
    };
    let mut at = 0;
    for Marker { start, end, gap } in markers {
        cut.lines = cut.lines.or(gap.of);
        if start > at {
            cut.parts.push(Part::Text(body[at..start].to_owned()));
        }
        cut.parts.push(Part::Gap(gap));
        at = end;
    }
    if !cut.parts.is_empty() && at < body.len() {
        cut.parts.push(Part::Text(body[at..].to_owned()));
    }
    // What is shown as the output: the frame stays, the header does not.
    let plain = if unwrapped.is_some() {
        body.to_owned()
    } else {
        format!("{}{body}", &text[..text.len() - after_frame.len()])
    };
    (plain, Some(cut))
}

impl Cut {
    /// The `cut` field of an entry. The text on each side of the gap gets an
    /// equal share of `budget` bytes, the head cut at its end and the tail at
    /// its start; `true` when any was cut.
    fn json(&self, budget: usize) -> (Value, bool) {
        let texts = self
            .parts
            .iter()
            .filter(|part| matches!(part, Part::Text(_)))
            .count();
        let share = budget / texts.max(1);
        let last = self
            .parts
            .iter()
            .rposition(|part| matches!(part, Part::Text(_)));
        let mut clipped = false;
        let mut parts = Vec::new();
        for (index, part) in self.parts.iter().enumerate() {
            match part {
                Part::Text(text) => {
                    // The end of the last text is what matters; the others
                    // are read from their start.
                    let (text, cut) = if Some(index) == last && index > 0 {
                        clip_end(text, share)
                    } else {
                        clip(text, share)
                    };
                    clipped |= cut;
                    parts.push(json!({"text": text}));
                }
                Part::Gap(gap) => {
                    let mut fields = json!({"n": gap.n, "unit": gap.unit});
                    if let Some(of) = gap.of {
                        fields["of"] = json!(of);
                    }
                    parts.push(json!({"gap": fields}));
                }
            }
        }
        let mut cut = json!({"by": "codex"});
        if let Some(tokens) = self.original_tokens {
            cut["original_tokens"] = json!(tokens);
        }
        if let Some(lines) = self.lines {
            cut["lines"] = json!(lines);
        }
        if !parts.is_empty() {
            cut["parts"] = Value::Array(parts);
        }
        (cut, clipped)
    }
}

/// Puts a step's output preview in `entry`: `out`, cut at [`PREVIEW_MAX`]
/// (`more` names it when cut), and for a Codex output that Codex cut, `cut`.
fn put_out(
    entry: &mut Map<String, Value>,
    more: &mut Vec<&'static str>,
    text: &str,
    source: Source,
) {
    let (plain, cut) = split_cut(text, source);
    let (shown, mut clipped) = clip(&plain, PREVIEW_MAX);
    entry.insert("out".into(), json!(shown));
    if let Some(cut) = cut {
        let (cut, cut_clipped) = cut.json(PREVIEW_MAX);
        entry.insert("cut".into(), cut);
        clipped |= cut_clipped;
    }
    if clipped {
        more.push("out");
    }
}

/// One part's whole text for "View all": `text`, whether semon cut it here
/// (`truncated`, or `cut_before`), and for a Codex output that Codex cut,
/// `cut`. A gap Codex made is never `truncated`.
fn text_json(text: &str, source: Source, cut_before: bool) -> String {
    let (plain, cut) = split_cut(text, source);
    let (shown, truncated) = clip(&plain, FULL_MAX);
    let mut fields = json!({"text": shown});
    if let Some(cut) = cut {
        // Each side may take up to the cap itself: whether semon cut the text
        // is `truncated`'s to say, from the text as a whole.
        let (cut, _) = cut.json(FULL_MAX * 2);
        fields["cut"] = cut;
    }
    fields["truncated"] = json!(truncated || cut_before);
    fields.to_string()
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
            Value::Array(parts) => {
                // One exec result a script printed: its command's output.
                if let Some(value) = events::yield_json(Some(output))
                    && let Some(text) = field(&value, "output")
                {
                    return Some(text.to_owned());
                }
                script_text(parts)
            }
            other => content_text(other),
        });
    }
    tool_result_text(record, block)
}

/// A code-mode script's output parts as text: what the script printed,
/// after the harness's "Script completed" header. A failure's header stays,
/// as it says what failed.
fn script_text(parts: &[Value]) -> String {
    let mut texts: Vec<String> = parts
        .iter()
        .filter_map(|part| match field(part, "type") {
            None | Some("text" | "input_text" | "output_text") => {
                field(part, "text").map(str::to_owned)
            }
            Some("image" | "input_image") => Some("[image]".to_owned()),
            _ => None,
        })
        .collect();
    if texts
        .first()
        .is_some_and(|header| header.trim_start().starts_with("Script completed"))
    {
        texts.remove(0);
    }
    texts.join("\n")
}

/// The output chunk a yielded command's script or poll printed.
fn chunk_of(record: &Value) -> Option<String> {
    let value = events::yield_json(record.get("payload")?.get("output"))?;
    field(&value, "output").map(str::to_owned)
}

/// A yielded command's output as its scripts and polls printed it, in
/// order, cut at `limit`; `true` when cut or when polls weren't kept.
fn chunks(
    lines: &mut Lines,
    path: &Path,
    first: Option<u64>,
    polls: &[u64],
    dropped: bool,
    limit: usize,
) -> (String, bool) {
    let mut text = String::new();
    for offset in first.into_iter().chain(polls.iter().copied()) {
        if text.len() > limit {
            break;
        }
        if let Some(chunk) = lines.get(path, offset).and_then(|record| chunk_of(&record)) {
            text.push_str(&chunk);
        }
    }
    let (mut text, cut) = clip(&text, limit);
    if dropped && !cut {
        text.push_str("\n…");
    }
    (text, cut || dropped)
}

/// The `CommandExecution` item on a line.
fn command_item(record: &Value) -> Option<&Value> {
    record
        .get("payload")?
        .get("item")
        .filter(|item| field(item, "type") == Some("CommandExecution"))
}

/// A `CommandExecution` item's command line: the shell's `-lc` argument,
/// else its argv joined.
fn item_command(item: &Value) -> Option<String> {
    let argv: Vec<&str> = item
        .get("command")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if argv.len() == 3 && argv[1] == "-lc" {
        Some(argv[2].to_owned())
    } else if !argv.is_empty() {
        Some(argv.join(" "))
    } else {
        field(item, "command").map(str::to_owned)
    }
}

/// A code-mode script's source, at its call's line.
fn script_at(lines: &mut Lines, path: &Path, offset: u64) -> Option<String> {
    let record = lines.get(path, offset)?;
    model::tool_input(&record, 0).and_then(|input| input.as_str().map(str::to_owned))
}

/// A yielded command's line: from the item that completed it, else from
/// the `cmd` its starting script passed.
fn yielded_command(
    lines: &mut Lines,
    path: &Path,
    start: u64,
    done: Option<u64>,
) -> Option<String> {
    done.and_then(|done| lines.get(path, done))
        .and_then(|record| command_item(&record).and_then(item_command))
        .or_else(|| {
            script_at(lines, path, start).and_then(|script| events::script_command(&script))
        })
}

/// The input a poll sent, as its script passed it.
fn sent_chars(lines: &mut Lines, path: &Path, offset: u64) -> Option<String> {
    match events::script_arg(&script_at(lines, path, offset)?, "write_stdin", "chars")? {
        ScriptArg::Literal(chars) => Some(chars),
        _ => None,
    }
}

/// A step's state fields: running, unfinished, or done with its outcome and
/// how long it took.
fn state_fields(
    entry: &mut Map<String, Value>,
    shown: Shown,
    took: String,
    t: Option<i64>,
    now: i64,
) {
    match shown {
        Shown::Live => {
            entry.insert("live".into(), json!(true));
            entry.insert(
                "secs".into(),
                json!(t.map_or_else(|| "—".to_owned(), |t| running(now - t))),
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
            entry.insert("secs".into(), json!(took));
        }
    }
}

/// A command that outlived its yield, as one `exec_command` step: its line,
/// then its output as the polls printed it, and once it completed, its exit
/// code, duration and whole output from the completing item.
#[allow(clippy::too_many_arguments)]
fn yielded_entry(
    lines: &mut Lines,
    file: &SlotFile,
    slot: &Slot,
    index: usize,
    shown: Shown,
    first: Option<u64>,
    polls: &[u64],
    dropped: bool,
    done: Option<&Reply>,
    now: i64,
) -> Value {
    let item = done
        .and_then(|done| lines.get(&file.path, done.o))
        .and_then(|record| command_item(&record).cloned());
    let command = item
        .as_ref()
        .and_then(item_command)
        .or_else(|| {
            script_at(lines, &file.path, slot.offset)
                .and_then(|script| events::script_command(&script))
        })
        .unwrap_or_default();
    let arg = one_line(&command, 160);
    let mut entry = Map::new();
    let mut more = Vec::new();
    entry.insert("k".into(), json!("tool"));
    entry.insert("name".into(), json!("exec_command"));
    entry.insert("arg".into(), json!(arg));
    if command.trim() != arg {
        let (text, cut) = clip(&command, PREVIEW_MAX);
        entry.insert("in".into(), json!(text));
        if cut {
            more.push("in");
        }
    }
    let took = item
        .as_ref()
        .and_then(codex_duration)
        .or_else(|| match (slot.t, done.and_then(|done| done.t)) {
            (Some(start), Some(end)) => Some(secs(end - start)),
            _ => None,
        })
        .unwrap_or_else(|| "—".to_owned());
    state_fields(&mut entry, shown, took, slot.t, now);
    if let Some(item) = &item {
        if let Some(exit) = item.get("exit_code").and_then(Value::as_i64) {
            entry.insert("exit".into(), json!(exit));
        }
        if let Some(cwd) = field(item, "cwd") {
            entry.insert("cwd".into(), json!(codex_path(cwd, file.cwd.as_deref())));
        }
    }
    // The item's whole collected output, else what the polls printed.
    let (output, polls_cut, source) = match item
        .as_ref()
        .and_then(|item| field(item, "aggregated_output"))
    {
        Some(output) => (output.to_owned(), false, Source::Collected),
        None => {
            let (output, cut) = chunks(lines, &file.path, first, polls, dropped, PREVIEW_MAX);
            (output, cut, Source::Model)
        }
    };
    if !output.is_empty() {
        put_out(&mut entry, &mut more, &output, source);
        if polls_cut && !more.contains(&"out") {
            more.push("out");
        }
    }
    if !more.is_empty() {
        entry.insert("more".into(), json!(more));
    }
    entry.insert("script".into(), json!(slot.offset));
    entry.insert("slot".into(), json!(index));
    Value::Object(entry)
}

/// A poll that sent input to a yielded command: the command it went to,
/// the input, and what the command printed during that poll.
#[allow(clippy::too_many_arguments)]
fn sent_entry(
    lines: &mut Lines,
    file: &SlotFile,
    slot: &Slot,
    index: usize,
    shown: Shown,
    reply: Option<&Reply>,
    start: u64,
    done: Option<u64>,
    now: i64,
) -> Value {
    let command = yielded_command(lines, &file.path, start, done).unwrap_or_default();
    let mut entry = Map::new();
    let mut more = Vec::new();
    entry.insert("k".into(), json!("tool"));
    entry.insert("name".into(), json!("write_stdin"));
    entry.insert("arg".into(), json!(one_line(&command, 160)));
    if let Some(chars) = sent_chars(lines, &file.path, slot.offset) {
        let (text, cut) = clip(&chars, PREVIEW_MAX);
        entry.insert("in".into(), json!(text));
        if cut {
            more.push("in");
        }
    }
    let took = match (slot.t, reply.and_then(|reply| reply.t)) {
        (Some(start), Some(end)) => secs(end - start),
        _ => "—".to_owned(),
    };
    state_fields(&mut entry, shown, took, slot.t, now);
    if let Some(output) = reply
        .and_then(|reply| lines.get(&file.path, reply.o))
        .and_then(|record| chunk_of(&record).or_else(|| result_text(&record, 0)))
        .filter(|output| !output.trim().is_empty())
    {
        put_out(&mut entry, &mut more, &output, Source::Model);
    }
    if !more.is_empty() {
        entry.insert("more".into(), json!(more));
    }
    entry.insert("script".into(), json!(slot.offset));
    entry.insert("slot".into(), json!(index));
    Value::Object(entry)
}

/// A plain call's output, and where it is from: the whole output its
/// `CommandExecution` item collected when the logs have that item, else the
/// call's result, which is what the model saw.
fn tool_output(
    lines: &mut Lines,
    path: &Path,
    reply: Option<&Reply>,
    item: Option<u64>,
) -> Option<(String, Source)> {
    if let Some(output) = item
        .and_then(|item| lines.get(path, item))
        .and_then(|record| {
            command_item(&record)
                .and_then(|item| field(item, "aggregated_output"))
                .filter(|output| !output.is_empty())
                .map(str::to_owned)
        })
    {
        return Some((output, Source::Collected));
    }
    let reply = reply?;
    let record = lines.get(path, reply.o)?;
    let source = if record.get("payload").is_some() {
        Source::Model
    } else {
        Source::Plain
    };
    Some((result_text(&record, reply.b as usize)?, source))
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
    item: Option<u64>,
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
    let mut result = || tool_output(lines, &file.path, reply, item);
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
            } else if let Some((text, source)) =
                result().filter(|(text, _)| !text.trim().is_empty())
            {
                put_out(&mut entry, &mut more, &text, source);
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

fn codex_path(path: &str, cwd: Option<&str>) -> String {
    let path = path.strip_prefix("file://").unwrap_or(path);
    if let Some(cwd) = cwd {
        let base = if cwd == "/" {
            "/"
        } else {
            cwd.trim_end_matches('/')
        };
        if path == base {
            return ".".to_owned();
        }
        return model::relative(path, Some(cwd), None);
    }
    path.to_owned()
}

fn codex_duration(item: &Value) -> Option<String> {
    let duration = item.get("duration")?;
    let seconds = duration.get("secs")?.as_i64()?;
    let nanos = duration.get("nanos").and_then(Value::as_i64).unwrap_or(0);
    Some(secs(
        seconds
            .saturating_mul(1000)
            .saturating_add(nanos / 1_000_000),
    ))
}

fn codex_diff_rows(diff: &str, limit: usize) -> (Value, bool, usize) {
    let mut rows = Vec::new();
    let mut used = 0;
    let mut cut = false;
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if (!in_hunk && (line.starts_with("--- ") || line.starts_with("+++ ")))
            || line.starts_with("\\ No newline")
        {
            continue;
        }
        let Some((kind, text)) = line
            .strip_prefix('+')
            .map(|text| ("add", format!("+{text}")))
            .or_else(|| {
                line.strip_prefix('-')
                    .map(|text| ("del", format!("-{text}")))
            })
            .or_else(|| {
                line.strip_prefix(' ')
                    .map(|text| ("ctx", format!(" {text}")))
            })
        else {
            continue;
        };
        if used >= limit {
            rows.push(json!(["ctx", "…"]));
            cut = true;
            break;
        }
        let (text, clipped) = clip(&text, limit - used);
        used += text.len() + 1;
        cut |= clipped;
        rows.push(json!([kind, text]));
        if clipped {
            break;
        }
    }
    (Value::Array(rows), cut, used)
}

fn operation_entry(
    lines: &mut Lines,
    file: &SlotFile,
    slot: &Slot,
    index: usize,
    kind: &str,
    ok: Option<bool>,
    script_offset: Option<u64>,
) -> Option<Value> {
    let record = lines.get(&file.path, slot.offset)?;
    let item = record.get("payload")?.get("item")?;
    let mut entry = Map::new();
    let mut more = Vec::new();
    entry.insert("k".into(), json!("tool"));
    entry.insert(
        "name".into(),
        json!(match kind {
            "CommandExecution" => "exec_command",
            "FileChange" => "apply_patch",
            _ => return None,
        }),
    );
    entry.insert("ok".into(), json!(ok));
    entry.insert(
        "secs".into(),
        json!(codex_duration(item).unwrap_or_else(|| "—".to_owned())),
    );
    entry.insert("slot".into(), json!(index));
    if let Some(script_offset) = script_offset {
        entry.insert("script".into(), json!(script_offset));
    }
    match kind {
        "CommandExecution" => {
            let command = item_command(item).unwrap_or_default();
            let arg = one_line(&command, 160);
            let needs_input = command.trim() != arg.as_str();
            entry.insert("arg".into(), json!(arg));
            if needs_input {
                let (input, cut) = clip(&command, PREVIEW_MAX);
                entry.insert("in".into(), json!(input));
                if cut {
                    more.push("in");
                }
            }
            if let Some(exit) = item.get("exit_code").and_then(Value::as_i64) {
                entry.insert("exit".into(), json!(exit));
            }
            if let Some(cwd) = field(item, "cwd") {
                entry.insert("cwd".into(), json!(codex_path(cwd, file.cwd.as_deref())));
            }
            if let Some(output) = field(item, "aggregated_output")
                && !output.is_empty()
            {
                put_out(&mut entry, &mut more, output, Source::Collected);
            }
        }
        "FileChange" => {
            let mut changes = Vec::new();
            let mut paths = Vec::new();
            let mut remaining = PREVIEW_MAX;
            let mut cut = false;
            if let Some(fields) = item.get("changes").and_then(Value::as_object) {
                for (path, change) in fields {
                    let path = codex_path(path, file.cwd.as_deref());
                    paths.push(path.clone());
                    let mut rendered = Map::new();
                    rendered.insert("path".into(), json!(path));
                    if let Some(move_path) = field(change, "move_path") {
                        rendered.insert(
                            "move".into(),
                            json!(codex_path(move_path, file.cwd.as_deref())),
                        );
                    }
                    let (rows, clipped, used) = field(change, "unified_diff")
                        .map(|diff| codex_diff_rows(diff, remaining))
                        .unwrap_or_else(|| (json!([]), false, 0));
                    remaining = remaining.saturating_sub(used);
                    cut |= clipped;
                    rendered.insert("diff".into(), rows);
                    changes.push(Value::Object(rendered));
                }
            }
            entry.insert("arg".into(), json!(model::one_line(&paths.join(", "), 160)));
            entry.insert("changes".into(), Value::Array(changes));
            if cut {
                more.push("diff");
            }
        }
        _ => return None,
    }
    if !more.is_empty() {
        entry.insert("more".into(), json!(more));
    }
    Some(Value::Object(entry))
}

/// One slot as a `TX` entry; `None` for a slot with nothing to show.
fn render(
    built: &Built,
    lines: &mut Lines,
    sid: &str,
    slots: &[Slot],
    slot: &Slot,
    index: usize,
    now: i64,
) -> Option<Value> {
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
            let mut entry = json!({"k": "think"});
            if !text.trim().is_empty() {
                entry["text"] = json!(cap(text.trim(), MSG_MAX));
            }
            if built
                .sessions
                .get(sid)
                .is_some_and(|session| session.harness == "claude")
                && let Some(secs) = thought_secs(slots, index)
            {
                entry["secs"] = json!(secs);
            }
            entry
        }
        SlotKind::Harness(label) => json!({"k": "harness", "label": label}),
        SlotKind::Gap => {
            json!({"k": "end", "text": "Some entries not included: the log has unreadable lines here"})
        }
        SlotKind::NoActivity => json!({"k": "end", "text": "No activity in these logs"}),
        SlotKind::Returned { to, at, failed } => {
            json!({"k": "end", "ret": {"to": to, "at": at, "failed": failed}})
        }
        SlotKind::Tool {
            shown,
            name,
            reply,
            item,
        } => tool_entry(
            lines,
            file?,
            slot,
            index,
            *shown,
            name,
            reply.as_ref(),
            *item,
            now,
            built.home.as_deref(),
        ),
        SlotKind::Operation {
            kind,
            ok,
            script_offset,
        } => operation_entry(lines, file?, slot, index, kind, *ok, *script_offset)?,
        SlotKind::Yielded {
            shown,
            first,
            polls,
            cut,
            done,
        } => yielded_entry(
            lines,
            file?,
            slot,
            index,
            *shown,
            *first,
            polls,
            *cut,
            done.as_ref(),
            now,
        ),
        SlotKind::Sent {
            shown,
            reply,
            start,
            done,
        } => sent_entry(
            lines,
            file?,
            slot,
            index,
            *shown,
            reply.as_ref(),
            *start,
            *done,
            now,
        ),
    };
    Some(entry)
}

fn thought_secs(slots: &[Slot], index: usize) -> Option<i64> {
    let before = slots.get(index.checked_sub(1)?)?.t?;
    let at = slots.get(index)?.t?;
    let elapsed = at.checked_sub(before)?;
    (elapsed >= 0).then(|| elapsed.saturating_add(500) / 1000)
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
        SlotKind::Tool { reply, item, .. } => {
            if let Some(input) = lines
                .get(&file.path, slot.offset)
                .and_then(|record| tool_input(&record, block))
            {
                let mut text = String::new();
                strings(&input, &mut text);
                texts.push(("in", text));
            }
            if let Some((text, _)) = tool_output(lines, &file.path, reply.as_ref(), *item) {
                texts.push(("out", text));
            }
            None
        }
        SlotKind::Yielded {
            first,
            polls,
            cut,
            done,
            ..
        } => {
            let done = done.as_ref().map(|done| done.o);
            if let Some(command) = yielded_command(lines, &file.path, slot.offset, done) {
                texts.push(("in", command));
            }
            let output = done
                .and_then(|done| lines.get(&file.path, done))
                .and_then(|record| {
                    command_item(&record)
                        .and_then(|item| field(item, "aggregated_output"))
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| chunks(lines, &file.path, *first, polls, *cut, FULL_MAX).0);
            texts.push(("out", output));
            None
        }
        SlotKind::Sent { reply, .. } => {
            if let Some(chars) = sent_chars(lines, &file.path, slot.offset) {
                texts.push(("in", chars));
            }
            if let Some(output) = reply
                .as_ref()
                .and_then(|reply| lines.get(&file.path, reply.o))
                .and_then(|record| chunk_of(&record))
            {
                texts.push(("out", output));
            }
            None
        }
        SlotKind::Operation { kind, .. } => {
            if let Some(item) = lines.get(&file.path, slot.offset).and_then(|record| {
                record
                    .get("payload")
                    .and_then(|payload| payload.get("item"))
                    .cloned()
            }) {
                match kind.as_str() {
                    "CommandExecution" => {
                        if let Some(command) = item.get("command") {
                            let mut text = String::new();
                            strings(command, &mut text);
                            texts.push(("in", text));
                        }
                        if let Some(output) = field(&item, "aggregated_output") {
                            texts.push(("out", output.to_owned()));
                        }
                    }
                    "FileChange" => {
                        if let Some(changes) = item.get("changes").and_then(Value::as_object) {
                            for (path, change) in changes {
                                texts.push(("in", path.clone()));
                                if let Some(diff) = field(change, "unified_diff") {
                                    texts.push(("out", diff.to_owned()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
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
        let Some(entry) = render(built, &mut lines, sid, slots, &slots[index], index, now) else {
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
    let file = slot
        .file
        .and_then(|file| built.files.get(file))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a tool call"))?;
    if part == "script"
        && let SlotKind::Operation {
            script_offset: Some(offset),
            ..
        } = &slot.kind
    {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "entry is not expandable");
        let record = read_record(&file.path, *offset).ok_or_else(invalid)?;
        let script = model::tool_input(&record, 0)
            .and_then(|input| input.as_str().map(str::to_owned))
            .ok_or_else(invalid)?;
        let (text, truncated) = clip(&script, FULL_MAX);
        return Ok(json!({"text": text, "truncated": truncated}).to_string());
    }
    if part == "in"
        && let SlotKind::Operation { kind, .. } = &slot.kind
        && kind == "CommandExecution"
    {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "entry is not expandable");
        let record = read_record(&file.path, slot.offset).ok_or_else(invalid)?;
        let item = record
            .get("payload")
            .and_then(|payload| payload.get("item"))
            .ok_or_else(invalid)?;
        let command = item_command(item).unwrap_or_default();
        let (text, truncated) = clip(&command, FULL_MAX);
        return Ok(json!({"text": text, "truncated": truncated}).to_string());
    }
    if let SlotKind::Yielded { .. } | SlotKind::Sent { .. } = &slot.kind {
        return full_yielded(file, slot, part);
    }
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "not a tool call");
    let SlotKind::Tool { reply, item, .. } = &slot.kind else {
        if part == "out"
            && let SlotKind::Operation { kind, .. } = &slot.kind
            && kind == "CommandExecution"
        {
            let record = read_record(&file.path, slot.offset).ok_or_else(invalid)?;
            let output = record
                .get("payload")
                .and_then(|payload| payload.get("item"))
                .and_then(|item| field(item, "aggregated_output"))
                .ok_or_else(invalid)?;
            return Ok(text_json(output, Source::Collected, false));
        }
        if part == "diff"
            && let SlotKind::Operation { kind, .. } = &slot.kind
            && kind == "FileChange"
        {
            let record = read_record(&file.path, slot.offset).ok_or_else(invalid)?;
            let item = record
                .get("payload")
                .and_then(|payload| payload.get("item"))
                .ok_or_else(invalid)?;
            let mut remaining = FULL_MAX;
            let mut truncated = false;
            let mut changes = Vec::new();
            if let Some(fields) = item.get("changes").and_then(Value::as_object) {
                for (path, change) in fields {
                    let (diff, cut, used) = field(change, "unified_diff")
                        .map(|diff| codex_diff_rows(diff, remaining))
                        .unwrap_or_else(|| (json!([]), false, 0));
                    remaining = remaining.saturating_sub(used);
                    truncated |= cut;
                    changes.push(json!({
                        "path": codex_path(path, file.cwd.as_deref()),
                        "move": field(change, "move_path").map(|path| codex_path(path, file.cwd.as_deref())),
                        "diff": diff,
                    }));
                }
            }
            return Ok(json!({"changes": changes, "truncated": truncated}).to_string());
        }
        return Err(invalid());
    };
    match part {
        "out" => {
            let (text, source) =
                tool_output(&mut Lines::default(), &file.path, reply.as_ref(), *item)
                    .ok_or_else(invalid)?;
            Ok(text_json(&text, source, false))
        }
        "in" | "diff" => full(&file.path, slot.offset, slot.block as usize, part),
        _ => Err(invalid()),
    }
}

/// The whole of one part of a yielded command's step or a poll's: its
/// script, its command line or input (`in`), or its output (`out`).
fn full_yielded(file: &SlotFile, slot: &Slot, part: &str) -> io::Result<String> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "entry is not expandable");
    let mut lines = Lines::default();
    let path = &file.path;
    // What the item collected, or what the model was shown of a poll.
    let mut source = Source::Model;
    let text = match (&slot.kind, part) {
        (_, "script") => script_at(&mut lines, path, slot.offset),
        (SlotKind::Yielded { done, .. }, "in") => yielded_command(
            &mut lines,
            path,
            slot.offset,
            done.as_ref().map(|done| done.o),
        ),
        (
            SlotKind::Yielded {
                first,
                polls,
                cut,
                done,
                ..
            },
            "out",
        ) => {
            let whole = done
                .as_ref()
                .and_then(|done| lines.get(path, done.o))
                .and_then(|record| {
                    command_item(&record)
                        .and_then(|item| field(item, "aggregated_output"))
                        .map(str::to_owned)
                });
            if whole.is_none() {
                let (text, truncated) = chunks(&mut lines, path, *first, polls, *cut, FULL_MAX);
                return Ok(text_json(&text, Source::Model, truncated));
            }
            source = Source::Collected;
            whole
        }
        (SlotKind::Sent { .. }, "in") => sent_chars(&mut lines, path, slot.offset),
        (SlotKind::Sent { reply, .. }, "out") => reply
            .as_ref()
            .and_then(|reply| lines.get(path, reply.o))
            .and_then(|record| chunk_of(&record).or_else(|| result_text(&record, 0))),
        _ => None,
    }
    .ok_or_else(invalid)?;
    if part == "out" {
        return Ok(text_json(&text, source, false));
    }
    let (text, truncated) = clip(&text, FULL_MAX);
    Ok(json!({"text": text, "truncated": truncated}).to_string())
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
            Ok(text_json(
                &text,
                if record.get("payload").is_some() {
                    Source::Model
                } else {
                    Source::Plain
                },
                false,
            ))
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
                claude_json: root.join(".claude.json"),
                codex_home: root.join("codex"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: Duration::from_secs(86400),
                session: None,
                facts: None,
                scan_window: false,
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
    fn code_mode_exec_renders_its_completed_operations() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        let script = "const r = await Promise.allSettled([\ntools.exec_command({cmd:\"git status\"}),\ntools.exec_command({cmd:\"sed -n '1,9p' a.rs\"}),\n]);";
        home.lines(
            "codex/sessions/2026/09/24/rollout-ops.jsonl",
            &[
                codex(
                    ts(4, 0, 0),
                    "session_meta",
                    json!({"id":"ops","cwd":"/work/proj"}),
                ),
                codex(
                    ts(4, 1, 0),
                    "response_item",
                    json!({"type":"custom_tool_call","call_id":"call_A","name":"exec","status":"completed","input":script}),
                ),
                codex(
                    ts(4, 2, 0),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-1","command":["/bin/zsh","-lc","git status"],"cwd":"file:///work/proj","exit_code":0,"duration":{"secs":1,"nanos":200000000},"aggregated_output":"## main\n"}}),
                ),
                codex(
                    ts(4, 2, 1),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-2","command":["/bin/zsh","-lc","sed -n '1,9p' a.rs"],"cwd":"file:///work/proj/src","exit_code":0,"duration":{"secs":0,"nanos":400000000},"aggregated_output":"a\n"}}),
                ),
                codex(
                    ts(4, 2, 2),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"FileChange","id":"exec-3","changes":{"/work/proj/src/a.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\n-a\n+b\n","move_path":null}}}}),
                ),
                codex(
                    ts(4, 3, 0),
                    "response_item",
                    json!({"type":"custom_tool_call_output","call_id":"call_A","output":[{"type":"input_text","text":"Script completed"}]}),
                ),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "ops", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry["k"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["tool", "tool", "tool"]
        );
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(3), Some(0))
        );
        assert_eq!(entries[0]["arg"], "git status");
        assert_eq!(entries[0]["cwd"], ".");
        assert_eq!(entries[0]["secs"], "1.2s");
        assert_eq!(entries[0]["exit"], 0);
        assert_eq!(entries[0]["out"], "## main\n");
        assert_eq!(entries[1]["arg"], "sed -n '1,9p' a.rs");
        assert_eq!(entries[1]["cwd"], "src");
        assert_eq!(entries[2]["arg"], "src/a.rs");
        assert_eq!(
            entries[2]["changes"][0]["diff"],
            json!([["del", "-a"], ["add", "+b"]])
        );
        assert!(entries[0]["script"].is_number());
        let script_view: Value =
            serde_json::from_str(&full_slot(&built, "ops", 0, "script").unwrap()).unwrap();
        assert_eq!(script_view["text"], script);
        assert_eq!(script_view["truncated"], false);
    }

    #[test]
    fn code_mode_command_longer_than_summary_has_a_full_command_preview() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        let command = format!("printf {}", "x".repeat(PREVIEW_MAX + 32));
        let script = "tools.exec_command({cmd: 'long command'})";
        home.lines(
            "codex/sessions/2026/09/24/rollout-long-command.jsonl",
            &[
                codex(
                    ts(4, 0, 0),
                    "session_meta",
                    json!({"id":"long-command","cwd":"/work/proj"}),
                ),
                codex(
                    ts(4, 1, 0),
                    "response_item",
                    json!({"type":"custom_tool_call","call_id":"call","name":"exec","status":"completed","input":script}),
                ),
                codex(
                    ts(4, 2, 0),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc",command.clone()],"exit_code":0,"aggregated_output":"done\n"}}),
                ),
                codex(
                    ts(4, 3, 0),
                    "response_item",
                    json!({"type":"custom_tool_call_output","call_id":"call","output":"done"}),
                ),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "long-command", &Anchor::Last);
        let entry = &page["entries"][0];
        assert!(entry["arg"].as_str().unwrap().ends_with('…'));
        assert!(entry["in"].as_str().unwrap().starts_with("printf "));
        assert!(entry["in"].as_str().unwrap().ends_with('…'));
        assert!(entry["more"].as_array().unwrap().contains(&json!("in")));
        let slot = entry["slot"].as_u64().unwrap() as usize;
        let full: Value =
            serde_json::from_str(&full_slot(&built, "long-command", slot, "in").unwrap()).unwrap();
        assert_eq!(full["text"], command);
        assert_eq!(full["truncated"], false);
    }

    #[test]
    fn operation_output_can_be_opened_in_full() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        let output = format!("{}tail", "x".repeat(PREVIEW_MAX + 32));
        home.lines(
            "codex/sessions/2026/09/24/rollout-long-operation.jsonl",
            &[
                codex(ts(4, 0, 0), "session_meta", json!({"id":"long-operation","cwd":"/work/proj"})),
                codex(ts(4, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"tools.exec_command({cmd:'long'})"})),
                codex(ts(4, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","long"],"exit_code":0,"aggregated_output":output}})),
                codex(ts(4, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":"done"})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "long-operation", &Anchor::Last);
        let slot = page["entries"][0]["slot"].as_u64().unwrap() as usize;
        assert!(
            page["entries"][0]["more"]
                .as_array()
                .unwrap()
                .contains(&json!("out"))
        );
        let full: Value =
            serde_json::from_str(&full_slot(&built, "long-operation", slot, "out").unwrap())
                .unwrap();
        assert_eq!(full["text"], output);
        assert_eq!(full["truncated"], false);
    }

    fn model(text: &str) -> (String, Option<Cut>) {
        split_cut(text, Source::Model)
    }

    fn text(part: &str) -> Part {
        Part::Text(part.to_owned())
    }

    fn gap(n: u64, unit: &'static str, of: Option<u64>) -> Part {
        Part::Gap(Gap { n, unit, of })
    }

    #[test]
    fn a_token_cut_is_split_at_its_marker_and_its_header_stripped() {
        let (plain, cut) = model(
            "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\nline 2…19500 tokens truncated…line 899\nline 900\n",
        );
        assert_eq!(
            plain,
            "line 1\nline 2…19500 tokens truncated…line 899\nline 900\n"
        );
        assert_eq!(
            cut,
            Some(Cut {
                original_tokens: Some(24000),
                lines: Some(900),
                parts: vec![
                    text("line 1\nline 2"),
                    gap(19500, "tokens", None),
                    text("line 899\nline 900\n")
                ],
            })
        );
    }

    #[test]
    fn a_char_cut_is_split_at_its_marker() {
        let (_, cut) = model(
            "Warning: truncated output (original token count: 9000)\nTotal output lines: 40\n\nhead…12345 chars truncated…tail",
        );
        let cut = cut.unwrap();
        assert_eq!(
            cut.parts,
            vec![text("head"), gap(12345, "chars", None), text("tail")]
        );
        assert_eq!((cut.original_tokens, cut.lines), (Some(9000), Some(40)));
    }

    #[test]
    fn the_collection_cap_marker_is_split_with_no_header() {
        let (plain, cut) = model("head\n... 1048576 bytes omitted ...\ntail\n");
        assert_eq!(plain, "head\n... 1048576 bytes omitted ...\ntail\n");
        assert_eq!(
            cut,
            Some(Cut {
                original_tokens: None,
                lines: None,
                parts: vec![text("head"), gap(1_048_576, "bytes", None), text("tail\n")],
            })
        );
    }

    #[test]
    fn the_old_line_marker_is_split_and_gives_the_line_count() {
        let (_, cut) = model("a\nb\n[... omitted 3 of 9 lines ...]\n\nc\nd\n");
        assert_eq!(
            cut,
            Some(Cut {
                original_tokens: None,
                lines: Some(9),
                parts: vec![text("a\nb\n"), gap(3, "lines", Some(9)), text("c\nd\n")],
            })
        );
    }

    #[test]
    fn text_that_only_looks_like_a_marker_is_not_split() {
        for output in [
            // The token marker is written mid-line, so only a header vouches for it.
            "echo says …5 tokens truncated… and goes on\n",
            "…5 chars truncated…",
            "see ... 5 bytes omitted ... here\n",
            "no newline after\n... 5 bytes omitted ...",
            "  [... omitted 3 of 9 lines ...]\n",
            "a [... omitted 3 of 9 lines ...] b\n",
            "[... omitted 3 of 9 lines ...] trailing\n",
            "Total output lines are 900\nplain\n",
            "Warning: truncated output is what Codex prints\n",
        ] {
            let (plain, cut) = model(output);
            assert_eq!((plain.as_str(), cut), (output, None), "{output:?}");
        }
    }

    #[test]
    fn a_header_with_no_marker_keeps_the_cut_but_no_gap() {
        let (plain, cut) = model(
            "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nonly what is left\n",
        );
        assert_eq!(plain, "only what is left\n");
        assert_eq!(
            cut,
            Some(Cut {
                original_tokens: Some(24000),
                lines: Some(900),
                parts: Vec::new(),
            })
        );
    }

    #[test]
    fn a_previewed_cut_keeps_the_head_and_the_end_of_the_tail() {
        let long = "x".repeat(PREVIEW_MAX);
        let (_, cut) = model(&format!(
            "Warning: truncated output (original token count: 1)\n{long}HEAD…9 tokens truncated…{long}TAIL"
        ));
        let (json, clipped) = cut.unwrap().json(PREVIEW_MAX);
        assert!(clipped);
        let parts = json["parts"].as_array().unwrap();
        assert!(parts[0]["text"].as_str().unwrap().ends_with('…'));
        assert_eq!(parts[1], json!({"gap": {"n": 9, "unit": "tokens"}}));
        let tail = parts[2]["text"].as_str().unwrap();
        assert!(tail.starts_with('…') && tail.ends_with("TAIL"));
        assert!(
            parts
                .iter()
                .all(|part| part.to_string().len() < PREVIEW_MAX)
        );
    }

    fn codex_line(time: String, kind: &str, payload: Value) -> Value {
        json!({"timestamp": time, "type": kind, "payload": payload})
    }

    #[test]
    fn a_plain_call_with_a_cut_output_says_where_and_keeps_the_text() {
        let home = Home::new();
        let output = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
        home.lines(
            "codex/sessions/2026/09/24/rollout-plain-cut.jsonl",
            &[
                codex_line(ts(6, 0, 0), "session_meta", json!({"id":"plain-cut","cwd":"/work/proj"})),
                codex_line(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"cargo test\"}"})),
                codex_line(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"call","output":output})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "plain-cut", &Anchor::Last);
        let entry = &page["entries"][0];
        assert_eq!(entry["out"], "line 1\n…19500 tokens truncated…\nline 900\n");
        assert_eq!(
            entry["cut"],
            json!({"by":"codex","original_tokens":24000,"lines":900,"parts":[
                {"text":"line 1\n"},
                {"gap":{"n":19500,"unit":"tokens"}},
                {"text":"\nline 900\n"}
            ]})
        );
        assert!(entry.get("more").is_none());
        let slot = entry["slot"].as_u64().unwrap() as usize;
        let full: Value =
            serde_json::from_str(&full_slot(&built, "plain-cut", slot, "out").unwrap()).unwrap();
        assert_eq!(full["text"], entry["out"]);
        assert_eq!(full["cut"], entry["cut"]);
        assert_eq!(full["truncated"], false);
    }

    #[test]
    fn a_plain_call_shows_its_items_whole_output_instead_of_the_cut_one() {
        let home = Home::new();
        let cut = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
        home.lines(
            "codex/sessions/2026/09/24/rollout-plain-item.jsonl",
            &[
                codex_line(ts(6, 0, 0), "session_meta", json!({"id":"plain-item","cwd":"/work/proj"})),
                codex_line(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"cargo test\"}"})),
                codex_line(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"call","output":cut})),
                codex_line(ts(6, 1, 750), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"call","command":["/bin/zsh","-lc","cargo test"],"exit_code":0,"aggregated_output":"line 1\nline 2\nline 900\n"}})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "plain-item", &Anchor::Last);
        let entry = &page["entries"][0];
        assert_eq!(entry["out"], "line 1\nline 2\nline 900\n");
        assert!(entry.get("cut").is_none());
        let slot = entry["slot"].as_u64().unwrap() as usize;
        let full: Value =
            serde_json::from_str(&full_slot(&built, "plain-item", slot, "out").unwrap()).unwrap();
        assert_eq!(full["text"], "line 1\nline 2\nline 900\n");
        assert!(full.get("cut").is_none());
    }

    #[test]
    fn an_operations_collection_cap_is_shown_as_a_gap_in_the_preview_and_in_full() {
        let home = Home::new();
        let head = "h".repeat(PREVIEW_MAX);
        let output = format!("{head}\n... 1048576 bytes omitted ...\nthe end\n");
        home.lines(
            "codex/sessions/2026/09/24/rollout-capped.jsonl",
            &[
                codex_line(ts(6, 0, 0), "session_meta", json!({"id":"capped","cwd":"/work/proj"})),
                codex_line(ts(6, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"tools.exec_command({cmd:'big'})"})),
                codex_line(ts(6, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","big"],"exit_code":0,"aggregated_output":output}})),
                codex_line(ts(6, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":"done"})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "capped", &Anchor::Last);
        let entry = &page["entries"][0];
        let parts = entry["cut"]["parts"].as_array().unwrap();
        assert_eq!(parts[1], json!({"gap": {"n": 1_048_576, "unit": "bytes"}}));
        assert_eq!(parts[2], json!({"text": "the end\n"}));
        assert!(entry["cut"].get("original_tokens").is_none());
        assert!(entry["more"].as_array().unwrap().contains(&json!("out")));
        let slot = entry["slot"].as_u64().unwrap() as usize;
        let full: Value =
            serde_json::from_str(&full_slot(&built, "capped", slot, "out").unwrap()).unwrap();
        assert_eq!(full["text"], output);
        assert_eq!(full["cut"]["parts"][0], json!({"text": head}));
        assert_eq!(full["truncated"], false);
    }

    #[test]
    fn a_code_mode_result_cut_in_its_json_falls_back_to_its_text_without_the_header() {
        let home = Home::new();
        let header =
            "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\n";
        // Whole JSON around the cut: the script's output field is read.
        let whole = format!(
            "{header}{{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"line 1\\nline 2…19500 tokens truncated…line 899\\n\"}}"
        );
        // Cut mid-string and never closed: the text is shown as it is.
        let broken = format!(
            "{header}{{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"line 1\\nline 2…19500 tokens truncated…lin"
        );
        let result = |call: &str, body: &str| json!({"type":"custom_tool_call_output","call_id":call,"output":[{"type":"input_text","text":"Script completed\nWall time 1.0s\nOutput:\n"},{"type":"input_text","text":body}]});
        home.lines(
            "codex/sessions/2026/09/24/rollout-code-cut.jsonl",
            &[
                codex_line(ts(6, 0, 0), "session_meta", json!({"id":"code-cut","cwd":"/work/proj"})),
                codex_line(ts(6, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"one","name":"exec","input":"text('one')"})),
                codex_line(ts(6, 1, 500), "response_item", result("one", &whole)),
                codex_line(ts(6, 2, 0), "response_item", json!({"type":"custom_tool_call","call_id":"two","name":"exec","input":"text('two')"})),
                codex_line(ts(6, 2, 500), "response_item", result("two", &broken)),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "code-cut", &Anchor::Last);
        let entries: Vec<&Value> = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["k"] == "tool")
            .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0]["out"],
            "line 1\nline 2…19500 tokens truncated…line 899\n"
        );
        assert_eq!(
            entries[0]["cut"]["parts"][1],
            json!({"gap": {"n": 19500, "unit": "tokens"}})
        );
        assert_eq!(entries[0]["cut"]["original_tokens"], 24000);
        let out = entries[1]["out"].as_str().unwrap();
        assert!(out.starts_with("{\"chunk_id\""), "{out}");
        assert!(!out.contains("Warning: truncated output"));
        assert_eq!(
            entries[1]["cut"]["parts"][1],
            json!({"gap": {"n": 19500, "unit": "tokens"}})
        );
        assert_eq!(entries[1]["cut"]["lines"], 900);
    }

    #[test]
    fn collected_output_is_never_read_as_the_model_facing_cut() {
        // A command that printed a rollout's cut output.
        let printed = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…line 900\n[... omitted 3 of 9 lines ...]\n";
        assert_eq!(
            split_cut(printed, Source::Collected),
            (printed.to_owned(), None)
        );
        // The collection cap's own marker still splits it, and the printed
        // header stays in the text.
        let capped = format!("{printed}head\n... 2048 bytes omitted ...\ntail\n");
        let (plain, cut) = split_cut(&capped, Source::Collected);
        assert_eq!(plain, capped);
        let cut = cut.unwrap();
        assert_eq!((cut.original_tokens, cut.lines), (None, None));
        assert_eq!(cut.parts.len(), 3);
        assert_eq!(cut.parts[1], gap(2048, "bytes", None));
        assert!(
            matches!(&cut.parts[0], Part::Text(head) if head.starts_with("Warning: truncated") && head.ends_with("head"))
        );
        // No marker at the first line either: that form is the model's.
        let first = "... 5 bytes omitted ...\nwhat a command printed\n";
        assert_eq!(
            split_cut(first, Source::Collected),
            (first.to_owned(), None)
        );
    }

    #[test]
    fn unified_execs_frame_comes_before_the_warning_header() {
        // codex-rs core/src/tools/context.rs: `{frame}\n{formatted_truncate_text(..)}`.
        let frame = "Chunk ID: 3f9a1c\nWall time: 4.2100 seconds\nProcess exited with code 0\nOriginal token count: 24000\nOutput:\n";
        let raw = format!(
            "{frame}Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…line 900\n"
        );
        let (plain, cut) = model(&raw);
        assert_eq!(
            plain,
            format!("{frame}line 1\n…19500 tokens truncated…line 900\n")
        );
        assert_eq!(
            cut,
            Some(Cut {
                original_tokens: Some(24000),
                lines: Some(900),
                parts: vec![
                    text("line 1\n"),
                    gap(19500, "tokens", None),
                    text("line 900\n")
                ],
            })
        );
        // No cut: the frame alone is ordinary output, unchanged.
        let plain_run = format!("{frame}hello\n");
        assert_eq!(model(&plain_run), (plain_run.clone(), None));
        // A frame doesn't vouch for a token marker a command printed.
        let echoed = format!("{frame}echo …5 tokens truncated…\n");
        assert_eq!(model(&echoed), (echoed.clone(), None));
        // A frame needs its wall time.
        let bare = "Chunk ID: x\nOutput:\nWarning: truncated output (original token count: 1)\n\nx";
        assert_eq!(model(bare), (bare.to_owned(), None));
    }

    #[test]
    fn a_cap_marker_first_then_a_token_cut_are_two_gaps() {
        // `truncated_output_with_policy`: the warning line, the omission
        // notice, a blank line, then the token-cut text.
        let raw = "Chunk ID: a\nWall time: 1.0000 seconds\nOriginal token count: 5000000\nOutput:\nWarning: truncated output (original token count: 5000000)\n... 1048576 bytes omitted ...\n\nhead…9000 tokens truncated…tail";
        let cut = model(raw).1.unwrap();
        assert_eq!(cut.original_tokens, Some(5_000_000));
        assert_eq!(
            cut.parts,
            vec![
                gap(1_048_576, "bytes", None),
                text("head"),
                gap(9000, "tokens", None),
                text("tail")
            ]
        );
        // The same notice with no token cut, as `format!("{marker}\n{text}")`.
        let notice =
            "Chunk ID: a\nWall time: 1.0000 seconds\nOutput:\n... 77 bytes omitted ...\nrest\n";
        assert_eq!(
            model(notice).1.unwrap().parts,
            vec![gap(77, "bytes", None), text("rest\n")]
        );
    }

    #[test]
    fn a_header_inside_a_script_results_output_field_is_found() {
        let result = "{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"Warning: truncated output (original token count: 7)\\nTotal output lines: 9\\n\\nab…3 tokens truncated…cd\"}";
        let (plain, cut) = model(result);
        assert_eq!(plain, "ab…3 tokens truncated…cd");
        let cut = cut.unwrap();
        assert_eq!((cut.original_tokens, cut.lines), (Some(7), Some(9)));
        assert_eq!(
            cut.parts,
            vec![text("ab"), gap(3, "tokens", None), text("cd")]
        );
        // A result with nothing cut is left as it is.
        let whole = "{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"fine\"}";
        assert_eq!(model(whole), (whole.to_owned(), None));
    }

    #[test]
    fn a_multibyte_character_at_the_previews_split_is_kept_whole_or_dropped() {
        // Each side gets 768 bytes: 1 + 3 * 255 = 766 is the last boundary
        // of the head at or before 768, and the tail starts at a boundary
        // at or after len - 768.
        let head = format!("a{}", "€".repeat(400));
        let tail = format!("{}z", "€".repeat(400));
        let (_, cut) = model(&format!(
            "Warning: truncated output (original token count: 1)\n{head}…9 tokens truncated…{tail}"
        ));
        let (json, clipped) = cut.unwrap().json(PREVIEW_MAX);
        assert!(clipped);
        let parts = json["parts"].as_array().unwrap();
        let shown_head = parts[0]["text"]
            .as_str()
            .unwrap()
            .strip_suffix('…')
            .unwrap();
        assert!(
            head.starts_with(shown_head) && shown_head.len() == 766,
            "{}",
            shown_head.len()
        );
        let shown_tail = parts[2]["text"]
            .as_str()
            .unwrap()
            .strip_prefix('…')
            .unwrap();
        assert!(tail.ends_with(shown_tail) && shown_tail.ends_with('z'));
        assert!(
            shown_tail.len() <= 768 && shown_tail.len() >= 766,
            "{}",
            shown_tail.len()
        );
    }

    #[test]
    fn view_all_never_says_semon_cut_a_gap_codex_made() {
        // Each side over half the cap, the whole under it.
        let side = FULL_MAX / 2 + 1024;
        let text = format!(
            "{}\n... 9 bytes omitted ...\n{}",
            "a".repeat(side),
            "b".repeat(1024)
        );
        let value: Value =
            serde_json::from_str(&text_json(&text, Source::Collected, false)).unwrap();
        assert_eq!(value["truncated"], false);
        assert_eq!(
            value["cut"]["parts"][0]["text"].as_str().unwrap().len(),
            side
        );
    }

    #[test]
    fn an_item_that_printed_a_cut_output_shows_it_unchanged() {
        let home = Home::new();
        let printed = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
        home.lines(
            "codex/sessions/2026/09/24/rollout-printed.jsonl",
            &[
                codex_line(ts(6, 0, 0), "session_meta", json!({"id":"printed","cwd":"/work/proj"})),
                codex_line(ts(6, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"tools.exec_command({cmd:'jq'})"})),
                codex_line(ts(6, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","jq"],"exit_code":0,"aggregated_output":printed}})),
                codex_line(ts(6, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":"done"})),
                codex_line(ts(6, 4, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"plain","arguments":"{\"cmd\":\"jq\"}"})),
                codex_line(ts(6, 4, 500), "response_item", json!({"type":"function_call_output","call_id":"plain","output":"model saw this"})),
                codex_line(ts(6, 4, 750), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"plain","command":["/bin/zsh","-lc","jq"],"exit_code":0,"aggregated_output":printed}})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "printed", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        for entry in entries {
            assert_eq!(entry["out"], printed);
            assert!(entry.get("cut").is_none(), "{entry}");
            let slot = entry["slot"].as_u64().unwrap() as usize;
            let full: Value =
                serde_json::from_str(&full_slot(&built, "printed", slot, "out").unwrap()).unwrap();
            assert_eq!(full["text"], printed);
            assert!(full.get("cut").is_none());
        }
    }

    #[test]
    fn a_unified_exec_call_cut_by_codex_is_found_after_its_frame() {
        let home = Home::new();
        let output = "Chunk ID: 3f9a1c\nWall time: 4.2100 seconds\nProcess exited with code 0\nOriginal token count: 24000\nOutput:\nWarning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
        home.lines(
            "codex/sessions/2026/09/24/rollout-framed.jsonl",
            &[
                codex_line(ts(6, 0, 0), "session_meta", json!({"id":"framed","cwd":"/work/proj"})),
                codex_line(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"cargo test\"}"})),
                codex_line(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"call","output":output})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "framed", &Anchor::Last);
        let entry = &page["entries"][0];
        assert_eq!(
            entry["cut"],
            json!({"by":"codex","original_tokens":24000,"lines":900,"parts":[
                {"text":"line 1\n"},
                {"gap":{"n":19500,"unit":"tokens"}},
                {"text":"\nline 900\n"}
            ]})
        );
        assert!(
            !entry["out"]
                .as_str()
                .unwrap()
                .contains("Warning: truncated")
        );
    }

    #[test]
    fn a_printed_warning_line_behind_a_frame_is_kept_unless_its_count_is_the_frames() {
        // A command printed a rollout's cut output; Codex cut nothing.
        let printed = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…line 900\n";
        let with = |count: &str| {
            format!(
                "Chunk ID: a\nWall time: 1.0000 seconds\nProcess exited with code 0\n{count}Output:\n{printed}"
            )
        };
        // The frame's count is the whole output's, not the printed one's.
        let other = with("Original token count: 12\n");
        assert_eq!(model(&other), (other.clone(), None));
        // A frame with no count (an intercepted patch's) can't vouch for it.
        let none = with("");
        assert_eq!(model(&none), (none.clone(), None));
        // Equal counts are Codex's own header.
        let real = with("Original token count: 24000\n");
        let (plain, cut) = model(&real);
        assert!(!plain.contains("Warning: truncated output"));
        assert_eq!(cut.unwrap().original_tokens, Some(24000));
        // With no frame at all (legacy and code-mode forms) the header is
        // Codex's wherever it starts the text.
        assert_eq!(model(printed).1.unwrap().original_tokens, Some(24000));
    }

    #[test]
    fn a_leading_cap_marker_takes_only_its_own_newline_without_a_warning_line() {
        // `format!("{marker}\n{text}")`: a kept output that starts blank.
        let raw =
            "Chunk ID: a\nWall time: 1.0000 seconds\nOutput:\n... 5 bytes omitted ...\n\nkept\n";
        assert_eq!(
            model(raw).1.unwrap().parts,
            vec![gap(5, "bytes", None), text("\nkept\n")]
        );
        // After a warning line the blank line is Codex's separator.
        let warned = "Chunk ID: a\nWall time: 1.0000 seconds\nOriginal token count: 9\nOutput:\nWarning: truncated output (original token count: 9)\n... 5 bytes omitted ...\n\nkept\n";
        assert_eq!(
            model(warned).1.unwrap().parts,
            vec![gap(5, "bytes", None), text("kept\n")]
        );
    }

    #[test]
    fn codex_diff_keeps_deleted_lines_that_look_like_headers() {
        let (rows, cut, _) = codex_diff_rows("@@ -1 +0,0 @@\n--- note\n", FULL_MAX);
        assert_eq!(rows, json!([["del", "--- note"]]));
        assert!(!cut);
    }

    #[test]
    fn code_mode_exec_errors_and_ambiguous_items_are_not_steps() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        let mut records = vec![codex(
            ts(5, 0, 0),
            "session_meta",
            json!({"id":"overlap","cwd":"/work/proj"}),
        )];
        for (id, script, minute) in [
            ("first", "const first = true;", 1),
            ("second", "const second = true;", 2),
        ] {
            records.push(codex(
                ts(5, minute, 0),
                "response_item",
                json!({"type":"custom_tool_call","call_id":id,"name":"exec","input":script}),
            ));
        }
        records.push(codex(
            ts(5, 3, 0),
            "event_msg",
            json!({"type":"item_completed","item":{"type":"CommandExecution","id":"unowned","command":["/bin/zsh","-lc","pwd"],"exit_code":0,"duration":{"secs":0,"nanos":100000000},"aggregated_output":"/work/proj\n"}}),
        ));
        records.push(codex(
            ts(5, 4, 0),
            "response_item",
            json!({"type":"custom_tool_call_output","call_id":"second","output":"done"}),
        ));
        records.push(codex(
            ts(5, 5, 0),
            "event_msg",
            json!({"type":"item_completed","item":{"type":"FileChange","id":"owned","changes":{"/work/proj/a.rs":{"unified_diff":"@@ -1 +1 @@\n-old\n+new\n"}}}}),
        ));
        records.push(codex(
            ts(5, 6, 0),
            "response_item",
            json!({"type":"custom_tool_call_output","call_id":"first","output":"done"}),
        ));
        records.push(codex(
            ts(5, 7, 0),
            "response_item",
            json!({"type":"custom_tool_call","call_id":"failed","name":"exec","input":"tools.exec_command({cmd:'false'})"}),
        ));
        records.push(codex(
            ts(5, 8, 0),
            "event_msg",
            json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-failed","command":["/bin/zsh","-lc","false"],"exit_code":1,"duration":{"secs":0,"nanos":200000000},"aggregated_output":"failed\n"}}),
        ));
        records.push(codex(
            ts(5, 9, 0),
            "response_item",
            json!({"type":"custom_tool_call_output","call_id":"failed","output":"done"}),
        ));
        records.push(codex(
            ts(5, 10, 0),
            "response_item",
            json!({"type":"custom_tool_call","call_id":"empty","name":"exec","input":"const empty = true;"}),
        ));
        records.push(codex(
            ts(5, 11, 0),
            "response_item",
            json!({"type":"custom_tool_call_output","call_id":"empty","output":"done"}),
        ));
        home.lines("codex/sessions/2026/09/24/rollout-overlap.jsonl", &records);
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "overlap", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        // The item that completed while two calls were open has no exact
        // owner: it is no step of its own, so it can't repeat its wrapper.
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(4), Some(1))
        );
        assert!(!entries.iter().any(|entry| entry["arg"] == "pwd"));
        let owned = entries.iter().find(|entry| entry["arg"] == "a.rs").unwrap();
        assert!(owned["script"].is_number());
        let owned_script: Value = serde_json::from_str(
            &full_slot(
                &built,
                "overlap",
                owned["slot"].as_u64().unwrap() as usize,
                "script",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(owned_script["text"], "const first = true;");
        let failed = entries
            .iter()
            .find(|entry| entry["arg"] == "false")
            .unwrap();
        assert_eq!(failed["ok"], false);
        assert_eq!(failed["exit"], 1);
        assert_eq!(failed["secs"], "0.2s");
        assert_eq!(failed["out"], "failed\n");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry["name"] == "exec")
                .count(),
            2
        );
        assert_eq!(entries.iter().filter(|entry| entry["arg"] == "").count(), 2);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry["in"] == "const second = true;")
                .unwrap()["name"],
            "exec"
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry["in"] == "const empty = true;")
                .unwrap()["name"],
            "exec"
        );
    }

    #[test]
    fn direct_function_call_exec_command_keeps_its_existing_entry() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-old-exec.jsonl",
            &[
                codex(
                    ts(6, 0, 0),
                    "session_meta",
                    json!({"id":"old-exec","cwd":"/work/proj"}),
                ),
                codex(
                    ts(6, 1, 0),
                    "response_item",
                    json!({"type":"function_call","name":"exec_command","call_id":"old-call","arguments":"{\"cmd\":\"git status\"}"}),
                ),
                codex(
                    ts(6, 1, 500),
                    "response_item",
                    json!({"type":"function_call_output","call_id":"old-call","output":"{\"output\":\"## main\\n\",\"metadata\":{\"exit_code\":0}}"}),
                ),
                codex(
                    ts(6, 1, 750),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"CommandExecution","id":"old-call","command":["/bin/zsh","-lc","git status"],"exit_code":1,"aggregated_output":"failed"}}),
                ),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "old-exec", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "exec_command");
        assert_eq!(entries[0]["arg"], "git status");
        // The item that completed the call holds its whole output (#52).
        assert_eq!(entries[0]["out"], "failed");
        assert_eq!(entries[0]["ok"], false);
        assert_eq!(entries[0]["secs"], "0.5s");
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(1), Some(1))
        );
    }

    #[test]
    fn direct_function_call_apply_patch_keeps_one_entry_with_file_change_item() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-old-patch.jsonl",
            &[
                codex(ts(6, 0, 0), "session_meta", json!({"id":"old-patch","cwd":"/work/proj"})),
                codex(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"apply_patch","call_id":"patch-call","arguments":"*** Begin Patch\\n*** Update File: a.rs\\n@@ -1 +1 @@\\n-old\\n+new\\n*** End Patch"})),
                codex(ts(6, 1, 250), "event_msg", json!({"type":"item_completed","item":{"type":"FileChange","id":"patch-call","changes":{"/work/proj/a.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\\n-old\\n+new\\n"}}}})),
                codex(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"patch-call","output":"{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "old-patch", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "apply_patch");
        assert_eq!(entries[0]["ok"], true);
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(1), Some(0))
        );
    }

    #[test]
    fn an_aborted_code_mode_call_does_not_claim_the_next_turns_items() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-interrupted.jsonl",
            &[
                codex(ts(7, 0, 0), "session_meta", json!({"id":"interrupted","cwd":"/work/proj"})),
                codex(ts(7, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"old","name":"exec","input":"const old = true;"})),
                codex(ts(7, 2, 0), "event_msg", json!({"type":"turn_aborted"})),
                codex(ts(7, 3, 0), "response_item", json!({"type":"custom_tool_call","call_id":"new","name":"exec","input":"const next = true;"})),
                codex(ts(7, 4, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item-new","command":["/bin/zsh","-lc","next"],"exit_code":0,"aggregated_output":"ok"}})),
                codex(ts(7, 5, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"new","output":"done"})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "interrupted", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(2), Some(1))
        );
        let operation = entries.iter().find(|entry| entry["arg"] == "next").unwrap();
        assert_eq!(operation["name"], "exec_command");
        let script: Value = serde_json::from_str(
            &full_slot(
                &built,
                "interrupted",
                operation["slot"].as_u64().unwrap() as usize,
                "script",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(script["text"], "const next = true;");
    }

    #[test]
    fn code_mode_wrapper_script_error_is_a_failed_tool() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-script-error.jsonl",
            &[
                codex(ts(8, 0, 0), "session_meta", json!({"id":"script-error","cwd":"/work/proj"})),
                codex(ts(8, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"throw new Error('boom')"})),
                codex(ts(8, 2, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script error: boom"}]})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "script-error", &Anchor::Last);
        let entry = &page["entries"][0];
        assert_eq!(entry["name"], "exec");
        assert_eq!(entry["ok"], false);
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(1), Some(1))
        );
    }

    #[test]
    fn an_item_with_no_open_code_mode_call_is_not_a_step() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-unowned.jsonl",
            &[
                codex(ts(9, 0, 0), "session_meta", json!({"id":"unowned","cwd":"/work/proj"})),
                codex(ts(9, 1, 0), "response_item", json!({"type":"function_call","name":"apply_patch","call_id":"patch-call","arguments":"*** Begin Patch\\n*** Update File: a.rs\\n@@ -1 +1 @@\\n-old\\n+new\\n*** End Patch"})),
                codex(ts(9, 1, 250), "event_msg", json!({"type":"item_completed","item":{"type":"FileChange","id":"other-id","changes":{"/work/proj/a.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\\n-old\\n+new\\n"}}}})),
                codex(ts(9, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"patch-call","output":"{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"})),
                codex(ts(9, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"stray","command":["/bin/zsh","-lc","pwd"],"exit_code":1,"aggregated_output":"/work/proj\n"}})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "unowned", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "apply_patch");
        assert_eq!(entries[0]["ok"], true);
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(1), Some(0))
        );
    }

    #[test]
    fn a_script_error_after_its_operations_is_a_failed_step() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-late-error.jsonl",
            &[
                codex(ts(9, 0, 0), "session_meta", json!({"id":"late-error","cwd":"/work/proj"})),
                codex(ts(9, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"await tools.exec_command({cmd:'ls'}); throw new Error('boom')"})),
                codex(ts(9, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","ls"],"exit_code":0,"aggregated_output":"a.rs\n"}})),
                codex(ts(9, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script error: boom"}]})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "late-error", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["arg"], "ls");
        assert_eq!(entries[0]["ok"], true);
        assert_eq!(entries[1]["name"], "exec");
        assert_eq!(entries[1]["ok"], false);
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(2), Some(1))
        );
    }

    #[test]
    fn a_script_error_is_read_from_the_harness_header_only() {
        let home = Home::new();
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-printed-error.jsonl",
            &[
                codex(ts(9, 0, 0), "session_meta", json!({"id":"printed-error","cwd":"/work/proj"})),
                codex(ts(9, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"text('Script error: printed')"})),
                codex(ts(9, 2, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script completed"},{"type":"input_text","text":"Script error: printed"}]})),
                codex(ts(9, 3, 0), "response_item", json!({"type":"custom_tool_call","call_id":"plain","name":"exec","input":"text('x')"})),
                codex(ts(9, 4, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"plain","output":"Script error: a plain string is no header"})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "printed-error", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry["ok"] != false));
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(2), Some(0))
        );
    }

    fn script_call(id: &str, script: &str) -> Value {
        json!({"type":"custom_tool_call","call_id":id,"name":"exec","status":"completed","input":script})
    }

    /// A script's output that printed one exec result, as
    /// `text(JSON.stringify(r))` does.
    fn exec_result(id: &str, session: Option<u64>, exit: Option<i64>, output: &str) -> Value {
        let mut result = json!({"chunk_id":"c0ffee","wall_time_seconds":1.0,"original_token_count":1,"output":output});
        if let Some(session) = session {
            result["session_id"] = json!(session);
        }
        if let Some(exit) = exit {
            result["exit_code"] = json!(exit);
        }
        json!({"type":"custom_tool_call_output","call_id":id,"output":[
            {"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},
            {"type":"input_text","text":result.to_string()},
        ]})
    }

    const WATCH: &str = "const r = await tools.exec_command({cmd:\"make watch\",workdir:\"/work/proj\",yield_time_ms:1000});\ntext(JSON.stringify(r));\n";

    fn poll(session: u64, chars: &str) -> String {
        format!(
            "const r = await tools.write_stdin({{session_id:{session},chars:{},yield_time_ms:1000}});\ntext(JSON.stringify(r));\n",
            json!(chars)
        )
    }

    /// `make watch` outlives its yield as session 4242 and prints "one";
    /// three polls with no input print "two", nothing and "three".
    fn yielded(sid: &str) -> Vec<Value> {
        let mut records = vec![
            codex_line(
                ts(10, 0, 0),
                "session_meta",
                json!({"id":sid,"cwd":"/work/proj"}),
            ),
            codex_line(ts(10, 1, 0), "response_item", script_call("start", WATCH)),
            codex_line(
                ts(10, 1, 1000),
                "response_item",
                exec_result("start", Some(4242), None, "one\n"),
            ),
        ];
        for (minute, output) in [(2, "two\n"), (3, ""), (4, "three\n")] {
            let id = format!("poll-{minute}");
            records.push(codex_line(
                ts(10, minute, 0),
                "response_item",
                script_call(&id, &poll(4242, "")),
            ));
            records.push(codex_line(
                ts(10, minute, 1000),
                "response_item",
                exec_result(&id, Some(4242), None, output),
            ));
        }
        records
    }

    /// The last poll: the command exits while it waits, so its completion
    /// arrives inside it and its output has an exit code and no session.
    fn completed(records: &mut Vec<Value>, chars: &str, aggregated: &str) {
        records.push(codex_line(
            ts(10, 5, 0),
            "response_item",
            script_call("poll-last", &poll(4242, chars)),
        ));
        records.push(codex_line(
            ts(10, 5, 500),
            "event_msg",
            json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-watch","process_id":"4242","command":["/bin/zsh","-lc","make watch"],"cwd":"file:///work/proj","status":"completed","exit_code":0,"duration":{"secs":5,"nanos":0},"aggregated_output":aggregated}}),
        ));
        records.push(codex_line(
            ts(10, 5, 1000),
            "response_item",
            exec_result("poll-last", None, Some(0), "four\n"),
        ));
    }

    fn tools(page: &Value) -> Vec<Value> {
        page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["k"] == "tool")
            .cloned()
            .collect()
    }

    #[test]
    fn a_yielded_command_and_its_polls_are_one_step() {
        let home = Home::new();
        let mut records = yielded("watch");
        completed(&mut records, "", "one\ntwo\nthree\nfour\n");
        records.push(codex_line(
            ts(10, 6, 0),
            "response_item",
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"It passed."}]}),
        ));
        home.lines("codex/sessions/2026/09/24/rollout-watch.jsonl", &records);
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "watch", &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 1, "{steps:?}");
        let step = &steps[0];
        assert_eq!(step["name"], "exec_command");
        assert_eq!(step["arg"], "make watch");
        assert_eq!(step["ok"], true);
        assert_eq!(step["exit"], 0);
        assert_eq!(step["secs"], "5.0s");
        assert_eq!(step["cwd"], ".");
        assert_eq!(step["out"], "one\ntwo\nthree\nfour\n");
        assert!(step.get("in").is_none());
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(1), Some(0))
        );
        let slot = step["slot"].as_u64().unwrap() as usize;
        let script: Value =
            serde_json::from_str(&full_slot(&built, "watch", slot, "script").unwrap()).unwrap();
        assert_eq!(script["text"], WATCH);
        let out: Value =
            serde_json::from_str(&full_slot(&built, "watch", slot, "out").unwrap()).unwrap();
        assert_eq!(out["text"], "one\ntwo\nthree\nfour\n");
        assert_eq!(out["truncated"], false);
    }

    #[test]
    fn a_yielded_command_that_never_completes_is_one_unfinished_step() {
        let endings: [(&str, Vec<Value>); 4] = [
            ("ends", Vec::new()),
            (
                "aborted",
                vec![codex_line(
                    ts(10, 5, 0),
                    "event_msg",
                    json!({"type":"turn_aborted"}),
                )],
            ),
            (
                "complete",
                vec![codex_line(
                    ts(10, 5, 0),
                    "event_msg",
                    json!({"type":"task_complete"}),
                )],
            ),
            (
                "mid-poll",
                vec![
                    codex_line(
                        ts(10, 5, 0),
                        "response_item",
                        script_call("poll-5", &poll(4242, "")),
                    ),
                    codex_line(ts(10, 5, 1000), "event_msg", json!({"type":"turn_aborted"})),
                ],
            ),
        ];
        for (sid, ending) in endings {
            let home = Home::new();
            let mut records = yielded(sid);
            records.extend(ending);
            home.lines(
                &format!("codex/sessions/2026/09/24/rollout-{sid}.jsonl"),
                &records,
            );
            let built = home.built(BASE + 86_400_000);
            let page = page_of(&built, sid, &Anchor::Last);
            let steps = tools(&page);
            assert_eq!(steps.len(), 1, "{sid}: {steps:?}");
            let step = &steps[0];
            assert_eq!(step["name"], "exec_command", "{sid}");
            assert_eq!(step["arg"], "make watch", "{sid}");
            assert_eq!(step["unfinished"], true, "{sid}");
            assert_eq!(step["ok"], false, "{sid}");
            assert_eq!(step["out"], "one\ntwo\nthree\n", "{sid}");
            assert_eq!(
                (page["calls"].as_u64(), page["errors"].as_u64()),
                (Some(1), Some(1)),
                "{sid}"
            );
        }
    }

    #[test]
    fn a_poll_that_sends_input_is_a_step_of_its_own() {
        let home = Home::new();
        let mut records = vec![
            codex_line(
                ts(10, 0, 0),
                "session_meta",
                json!({"id":"sent","cwd":"/work/proj"}),
            ),
            codex_line(ts(10, 1, 0), "response_item", script_call("start", WATCH)),
            codex_line(
                ts(10, 1, 1000),
                "response_item",
                exec_result("start", Some(4242), None, "one\n"),
            ),
            codex_line(
                ts(10, 2, 0),
                "response_item",
                script_call("answer", &poll(4242, "y\n")),
            ),
            codex_line(
                ts(10, 2, 1500),
                "response_item",
                exec_result("answer", Some(4242), None, "ok\n"),
            ),
        ];
        completed(&mut records, "", "one\nok\n");
        home.lines("codex/sessions/2026/09/24/rollout-sent.jsonl", &records);
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "sent", &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert_eq!(steps[0]["name"], "exec_command");
        assert_eq!(steps[0]["out"], "one\nok\n");
        assert_eq!(steps[0]["exit"], 0);
        assert_eq!(steps[1]["name"], "write_stdin");
        assert_eq!(steps[1]["arg"], "make watch");
        assert_eq!(steps[1]["in"], "y\n");
        assert_eq!(steps[1]["out"], "ok\n");
        assert_eq!(steps[1]["ok"], true);
        assert_eq!(steps[1]["secs"], "1.5s");
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(2), Some(0))
        );
    }

    #[test]
    fn a_script_with_no_operations_shows_its_text_output() {
        let home = Home::new();
        home.lines(
            "codex/sessions/2026/09/24/rollout-printed.jsonl",
            &[
                codex_line(ts(10, 0, 0), "session_meta", json!({"id":"printed","cwd":"/work/proj"})),
                codex_line(ts(10, 1, 0), "response_item", script_call("plain", "text('hello')")),
                codex_line(
                    ts(10, 1, 500),
                    "response_item",
                    json!({"type":"custom_tool_call_output","call_id":"plain","output":[
                        {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
                        {"type":"input_text","text":"hello"},
                    ]}),
                ),
                // A poll of a command these logs never saw start: that
                // command is a step from here on.
                codex_line(ts(10, 2, 0), "response_item", script_call("orphan", &poll(777, ""))),
                codex_line(
                    ts(10, 2, 500),
                    "response_item",
                    exec_result("orphan", Some(777), None, "still running\n"),
                ),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "printed", &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert_eq!(steps[0]["name"], "exec");
        assert_eq!(steps[0]["out"], "hello");
        assert_eq!(steps[1]["name"], "exec_command");
        assert_eq!(steps[1]["unfinished"], true);
        assert_eq!(steps[1]["out"], "still running\n");
    }

    #[test]
    fn an_interrupted_code_mode_call_shows_unfinished_after_its_operations() {
        let home = Home::new();
        home.lines(
            "codex/sessions/2026/09/24/rollout-stopped.jsonl",
            &[
                codex_line(ts(11, 0, 0), "session_meta", json!({"id":"stopped","cwd":"/work/proj"})),
                codex_line(
                    ts(11, 1, 0),
                    "response_item",
                    script_call("call", "await tools.exec_command({cmd:'ls'});\nawait tools.exec_command({cmd:'pwd'});\nawait tools.exec_command({cmd:'sleep 600'});"),
                ),
                codex_line(
                    ts(11, 2, 0),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"CommandExecution","id":"ls","command":["/bin/zsh","-lc","ls"],"exit_code":0,"aggregated_output":"a.rs\n"}}),
                ),
                codex_line(
                    ts(11, 3, 0),
                    "event_msg",
                    json!({"type":"item_completed","item":{"type":"CommandExecution","id":"pwd","command":["/bin/zsh","-lc","pwd"],"exit_code":0,"aggregated_output":"/work/proj\n"}}),
                ),
                codex_line(ts(11, 4, 0), "event_msg", json!({"type":"turn_aborted"})),
            ],
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "stopped", &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 3, "{steps:?}");
        assert_eq!(steps[0]["arg"], "ls");
        assert_eq!(steps[1]["arg"], "pwd");
        assert_eq!(steps[2]["name"], "exec");
        assert_eq!(steps[2]["unfinished"], true);
        assert_eq!(steps[2]["ok"], false);
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(3), Some(1))
        );
        let turn = built
            .turns
            .iter()
            .rfind(|turn| turn.sid == "stopped")
            .unwrap();
        assert_eq!((turn.end.st, turn.end.why), ("err", "unfinished_step"));
    }

    #[test]
    fn a_completion_that_matches_no_poll_is_not_the_yielded_step() {
        let home = Home::new();
        // Session id 4242 again, for a command the next script starts and
        // finishes itself: the id was used again, the old process is gone.
        let mut records = yielded("reused");
        records.extend([
            codex_line(
                ts(10, 6, 0),
                "response_item",
                script_call("date", "const r = await tools.exec_command({cmd:\"date\"});\ntext(JSON.stringify(r));"),
            ),
            codex_line(
                ts(10, 6, 500),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-date","process_id":"4242","command":["/bin/zsh","-lc","date"],"exit_code":0,"aggregated_output":"Thu\n"}}),
            ),
            codex_line(
                ts(10, 6, 1000),
                "response_item",
                exec_result("date", None, Some(0), "Thu\n"),
            ),
        ]);
        home.lines("codex/sessions/2026/09/24/rollout-reused.jsonl", &records);
        // A poll during which some other process completes: that item is
        // the poll's own operation, and the poll isn't folded, but what it
        // printed of 4242 is 4242's.
        let mut stray = yielded("stray");
        stray.extend([
            codex_line(ts(10, 6, 0), "response_item", script_call("poll-6", &poll(4242, ""))),
            codex_line(
                ts(10, 6, 500),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-other","process_id":"999","command":["/bin/zsh","-lc","echo other"],"exit_code":0,"aggregated_output":"other\n"}}),
            ),
            codex_line(
                ts(10, 6, 1000),
                "response_item",
                exec_result("poll-6", Some(4242), None, "five\n"),
            ),
        ]);
        home.lines("codex/sessions/2026/09/24/rollout-stray.jsonl", &stray);
        let built = home.built(BASE + 86_400_000);
        // The stray poll's own chunk is still the command's output.
        for (sid, other, out) in [
            ("reused", "date", "one\ntwo\nthree\n"),
            ("stray", "echo other", "one\ntwo\nthree\nfive\n"),
        ] {
            let page = page_of(&built, sid, &Anchor::Last);
            let steps = tools(&page);
            assert_eq!(steps.len(), 2, "{sid}: {steps:?}");
            assert_eq!(steps[0]["arg"], "make watch", "{sid}");
            assert_eq!(steps[0]["unfinished"], true, "{sid}");
            assert_eq!(steps[0]["out"], out, "{sid}");
            assert_eq!(steps[1]["name"], "exec_command", "{sid}");
            assert_eq!(steps[1]["arg"], other, "{sid}");
            assert_eq!(steps[1]["ok"], true, "{sid}");
        }
    }

    #[test]
    fn a_script_that_starts_and_polls_its_command_is_its_start() {
        let home = Home::new();
        // It starts the command and polls it in a loop, and the command
        // still outlives the script. A comment names another call.
        let script = "let r = await tools.exec_command({cmd:\"make watch\",yield_time_ms:1000});\n// then tools.write_stdin({session_id, chars:\"q\"}) to quit\nwhile (r.session_id && Date.now() < end) {\n  r = await tools.write_stdin({session_id: r.session_id, chars: \"\"});\n}\ntext(JSON.stringify(r));\n";
        let mut records = vec![
            codex_line(
                ts(10, 0, 0),
                "session_meta",
                json!({"id":"looped","cwd":"/work/proj"}),
            ),
            codex_line(ts(10, 1, 0), "response_item", script_call("start", script)),
            codex_line(
                ts(10, 1, 1000),
                "response_item",
                exec_result("start", Some(4242), None, "one\n"),
            ),
            codex_line(
                ts(10, 2, 0),
                "response_item",
                script_call("poll-2", &poll(4242, "")),
            ),
            codex_line(
                ts(10, 2, 1000),
                "response_item",
                exec_result("poll-2", Some(4242), None, "two\n"),
            ),
        ];
        completed(&mut records, "", "one\ntwo\nfour\n");
        home.lines("codex/sessions/2026/09/24/rollout-looped.jsonl", &records);
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "looped", &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(steps[0]["name"], "exec_command");
        assert_eq!(steps[0]["arg"], "make watch");
        assert_eq!(steps[0]["exit"], 0);
        assert_eq!(steps[0]["out"], "one\ntwo\nfour\n");
    }

    #[test]
    fn polls_past_the_bound_fold_in_and_mark_the_output_cut() {
        let home = Home::new();
        let mut records = vec![
            codex_line(
                ts(12, 0, 0),
                "session_meta",
                json!({"id":"many","cwd":"/work/proj"}),
            ),
            codex_line(ts(12, 1, 0), "response_item", script_call("start", WATCH)),
            codex_line(
                ts(12, 1, 10),
                "response_item",
                exec_result("start", Some(4242), None, "one\n"),
            ),
        ];
        for poll_index in 0..=crate::events::POLLS_MAX {
            let id = format!("poll-{poll_index}");
            let at = 20 + poll_index as i64 * 20;
            records.push(codex_line(
                ts(12, 2, at),
                "response_item",
                script_call(&id, &poll(4242, "")),
            ));
            records.push(codex_line(
                ts(12, 2, at + 10),
                "response_item",
                exec_result(&id, Some(4242), None, ""),
            ));
        }
        home.lines("codex/sessions/2026/09/24/rollout-many.jsonl", &records);
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "many", &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 1, "{} steps", steps.len());
        assert_eq!(steps[0]["out"], "one\n\n…");
        assert!(steps[0]["more"].as_array().unwrap().contains(&json!("out")));
        let slot = steps[0]["slot"].as_u64().unwrap() as usize;
        let out: Value =
            serde_json::from_str(&full_slot(&built, "many", slot, "out").unwrap()).unwrap();
        assert_eq!(out["text"], "one\n\n…");
        assert_eq!(out["truncated"], true);
    }

    #[test]
    fn a_character_across_the_output_bound_is_never_split() {
        let home = Home::new();
        for (sid, head) in [("wide", PREVIEW_MAX - 1), ("huge", FULL_MAX - 1)] {
            home.lines(
                &format!("codex/sessions/2026/09/24/rollout-{sid}.jsonl"),
                &[
                    codex_line(
                        ts(13, 0, 0),
                        "session_meta",
                        json!({"id":sid,"cwd":"/work/proj"}),
                    ),
                    codex_line(ts(13, 1, 0), "response_item", script_call("start", WATCH)),
                    codex_line(
                        ts(13, 1, 10),
                        "response_item",
                        exec_result("start", Some(4242), None, &"a".repeat(head)),
                    ),
                    codex_line(
                        ts(13, 2, 0),
                        "response_item",
                        script_call("poll", &poll(4242, "")),
                    ),
                    codex_line(
                        ts(13, 2, 10),
                        "response_item",
                        exec_result("poll", Some(4242), None, "é tail"),
                    ),
                ],
            );
        }
        let built = home.built(BASE + 86_400_000);
        let out_of = |sid: &str| {
            let page = page_of(&built, sid, &Anchor::Last);
            let step = tools(&page).remove(0);
            let slot = step["slot"].as_u64().unwrap() as usize;
            let full: Value =
                serde_json::from_str(&full_slot(&built, sid, slot, "out").unwrap()).unwrap();
            (step, full)
        };
        // The two bytes of "é" straddle PREVIEW_MAX: the preview stops
        // before it; the whole output is under FULL_MAX.
        let (step, full) = out_of("wide");
        assert_eq!(step["out"], format!("{}…", "a".repeat(PREVIEW_MAX - 1)));
        assert!(step["more"].as_array().unwrap().contains(&json!("out")));
        assert_eq!(
            full["text"],
            format!("{}é tail", "a".repeat(PREVIEW_MAX - 1))
        );
        assert_eq!(full["truncated"], false);
        // They straddle FULL_MAX: "View all" stops before it.
        let (step, full) = out_of("huge");
        assert_eq!(step["out"], format!("{}…", "a".repeat(PREVIEW_MAX)));
        assert_eq!(full["text"], format!("{}…", "a".repeat(FULL_MAX - 1)));
        assert_eq!(full["truncated"], true);
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
            claude_json: root.join(".claude.json"),
            codex_home: root.join("codex"),
            proc_root: root.join("proc"),
            cache: root.join("tx-index.json"),
            all: false,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
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
