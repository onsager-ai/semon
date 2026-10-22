//! Shared native transcript rendering, one session page at a time, in the
//! `TX` entry shape (`{k:"a"|"tool"|"bgend"|"think"|"h"|"u"|"harness"|"end", …}`).
//!
//! The Event Index keeps each session's transcript as offsets only (its slots);
//! the text of a page is read back from the source lines here, capped as in
//! the mockup data, and never kept.

use std::{
    collections::VecDeque,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    rc::Rc,
};

use serde_json::{Map, Value, json};

use crate::{
    attachments,
    events::{self, Reply, ScriptArg},
    field,
    model::{
        self, MSG_MAX, Shown, Slot, SlotFile, SlotKind, arg_summary, cap, content_text, one_line,
        patch_text, prompt_text, tool_input, tool_result_text,
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
/// A source line longer than this is not read back for a page (but for a
/// prompt's, see [`prompt_record`]).
pub(crate) const LINE_MAX: u64 = FULL_MAX as u64 + 1024 * 1024;

const SHELLS: [&str; 5] = ["Bash", "bash", "shell", "exec_command", "local_shell"];
const TITLED_TOOLS: [&str; 1] = ["Bash"];
const TITLE_MAX: usize = 160;
const EDITS: [&str; 5] = ["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"];
// Codex has recorded 1,597–7,375 ns for these commands, shorter than spawn overhead.
const CODEX_COMMAND_MIN_DURATION_NS: i64 = 1_000_000;

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

/// The record at `offset`, and how many bytes were read for it.
fn read_sized(path: &Path, offset: u64) -> (Option<Value>, u64) {
    let Ok(mut file) = crate::sealed::LogFile::open(path) else {
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
    (parse_native_record(&bytes), size)
}

/// Shared native JSON object parser for local and provider-backed source lines.
pub(crate) fn parse_native_record(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes).ok().filter(Value::is_object)
}

type NativeLineReader<'a> = dyn FnMut(&Path, u64) -> io::Result<(Option<Value>, u64)> + 'a;
type NativePromptReader<'a> = dyn FnMut(&SlotFile, u64) -> Option<(Value, u64)> + 'a;

/// Source lines read for one page: a line holding several blocks, or a call
/// and its result, is read once. Only the last few lines are kept.
#[derive(Default)]
pub(crate) struct Lines<'a> {
    provider: Option<&'a mut NativeLineReader<'a>>,
    prompt_provider: Option<&'a mut NativePromptReader<'a>>,
    pub(crate) failures: usize,
    recent: VecDeque<(LineKey, Option<Rc<Value>>)>,
    /// Bytes read from source files so far.
    pub(crate) bytes: u64,
}

/// The same bounded line cache can cross selected native session reads in one
/// search. It contains no provider or credentials and never escapes the call.
#[derive(Default)]
pub(crate) struct LineCache {
    recent: VecDeque<(LineKey, Option<Rc<Value>>)>,
    pub(crate) bytes: u64,
    failures: usize,
}

/// A source line: its file and byte offset.
type LineKey = (PathBuf, u64);

impl<'a> Lines<'a> {
    pub(crate) fn provider(reader: &'a mut NativeLineReader<'a>) -> Self {
        Self {
            provider: Some(reader),
            ..Self::default()
        }
    }

    pub(crate) fn provider_with_cache(
        reader: &'a mut NativeLineReader<'a>,
        cache: LineCache,
    ) -> Self {
        Self {
            provider: Some(reader),
            recent: cache.recent,
            bytes: cache.bytes,
            failures: cache.failures,
            ..Self::default()
        }
    }
    pub(crate) fn into_cache(self) -> LineCache {
        LineCache {
            recent: self.recent,
            bytes: self.bytes,
            failures: self.failures,
        }
    }

    pub(crate) fn provider_with_prompts(
        reader: &'a mut NativeLineReader<'a>,
        prompts: &'a mut NativePromptReader<'a>,
    ) -> Self {
        Self {
            prompt_provider: Some(prompts),
            ..Self::provider(reader)
        }
    }
    const KEEP: usize = 16;

    pub(crate) fn native_event_id(&self, path: &Path, offset: u64) -> Option<String> {
        let record = self
            .recent
            .iter()
            .find(|((seen, at), _)| seen == path && *at == offset)?
            .1
            .as_ref()?;
        ["uuid", "id"]
            .iter()
            .find_map(|key| record.get(key).and_then(Value::as_str))
            .filter(|id| !id.is_empty() && id.len() <= 4096)
            .map(str::to_owned)
    }

    fn get(&mut self, path: &Path, offset: u64) -> Option<Rc<Value>> {
        if let Some((_, record)) = self
            .recent
            .iter()
            .find(|((seen, at), _)| *at == offset && seen == path)
        {
            if record.is_none() && self.provider.is_some() {
                self.failures += 1;
            }
            return record.clone();
        }
        let (record, size) = if let Some(provider) = self.provider.as_mut() {
            match provider(path, offset) {
                Ok(result) => result,
                Err(_) => {
                    self.failures += 1;
                    (None, 0)
                }
            }
        } else {
            read_sized(path, offset)
        };
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

/// Display/cut handling uses the shared borrowed response-header decoder.
fn strip_frame(text: &str) -> (&str, bool, Option<u64>) {
    match crate::exec_response::decode_exec_response(text) {
        Some(frame) => (frame.body, true, frame.original_token_count),
        None => (text, false, None),
    }
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

#[cfg(test)]
fn text_value(text: &str, source: Source, cut_before: bool, limit: usize) -> Value {
    let (plain, cut) = split_cut(text, source);
    let (shown, truncated) = clip(&plain, limit);
    let mut fields = json!({"text": shown});
    if let Some(cut) = cut {
        // Each side may take up to the cap itself: whether semon cut the text
        // is `truncated`'s to say, from the text as a whole.
        let (cut, _) = cut.json(limit * 2);
        fields["cut"] = cut;
    }
    fields["truncated"] = json!(truncated || cut_before);
    fields
}

fn clip_within(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    let marker = "…";
    if limit < marker.len() {
        return (String::new(), true);
    }
    let mut end = limit - marker.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}{}", text[..end].trim_end(), marker), true)
}

fn selected_cut_metadata_bytes(cut: &Cut) -> usize {
    "codex".len()
        + cut
            .parts
            .iter()
            .map(|part| match part {
                Part::Gap(gap) => gap.unit.len(),
                Part::Text(_) => 0,
            })
            .sum::<usize>()
}

fn selected_cut(cut: &Cut, limit: usize) -> (Value, bool) {
    let mut value = json!({"by": "codex"});
    if let Some(tokens) = cut.original_tokens {
        value["original_tokens"] = json!(tokens);
    }
    if let Some(lines) = cut.lines {
        value["lines"] = json!(lines);
    }
    let mut remaining = limit.saturating_sub(selected_cut_metadata_bytes(cut));
    let mut truncated = false;
    let mut texts_left = cut
        .parts
        .iter()
        .filter(|part| matches!(part, Part::Text(_)))
        .count();
    let mut parts = Vec::new();
    for part in &cut.parts {
        match part {
            Part::Text(text) => {
                let share = remaining / texts_left.max(1);
                let (text, clipped) = clip_within(text, share);
                truncated |= clipped;
                remaining = remaining.saturating_sub(text.len());
                texts_left = texts_left.saturating_sub(1);
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
    if !parts.is_empty() {
        value["parts"] = Value::Array(parts);
    }
    (value, truncated)
}

fn selected_text_value(text: &str, source: Source, cut_before: bool, limit: usize) -> Value {
    let (plain, cut) = split_cut(text, source);
    // Preserve native gap evidence while reserving its string metadata inside
    // the same budget as flat text and duplicated structured text parts.
    let flat_limit = limit.saturating_sub(cut.as_ref().map_or(0, selected_cut_metadata_bytes));
    let (shown, mut truncated) = clip_within(&plain, flat_limit);
    let mut fields = json!({"text": shown});
    if let Some(cut) = cut {
        let remaining = limit.saturating_sub(string_value_bytes(&fields));
        let (cut, clipped) = selected_cut(&cut, remaining);
        fields["cut"] = cut;
        truncated |= clipped;
    }
    fields["truncated"] = json!(truncated || cut_before);
    fields
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

#[cfg(test)]
fn tool_name(record: &Value, block: usize) -> Option<String> {
    if field(record, "type") == Some("assistant.message") {
        return record
            .get("data")?
            .get("toolRequests")?
            .as_array()?
            .get(block)
            .and_then(|request| field(request, "name"))
            .map(str::to_owned);
    }
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
pub(crate) fn result_text(
    record: &Value,
    block: usize,
    code_result_source: bool,
) -> Option<String> {
    if field(record, "type") == Some("tool.execution_complete") {
        let data = record.get("data")?;
        return data
            .get("result")
            .and_then(|result| field(result, "content"))
            .or_else(|| data.get("error").and_then(|error| field(error, "message")))
            .map(str::to_owned);
    }
    if let Some(payload) = record.get("payload") {
        if let Some(item) = payload.get("item")
            && matches!(field(item, "type"), Some("McpToolCall" | "DynamicToolCall"))
        {
            let parts = item
                .pointer("/result/content")
                .or_else(|| item.get("content_items"));
            if let Some(parts) = parts.and_then(Value::as_array) {
                let text = parts
                    .iter()
                    .filter_map(|part| match field(part, "type") {
                        Some("text" | "input_text" | "output_text") => {
                            field(part, "text").map(str::to_owned)
                        }
                        Some("image" | "input_image") => Some("[image]".into()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    return Some(text);
                }
            }
            return item
                .get("error")
                .filter(|error| !error.is_null())
                .or_else(|| item.get("result"))
                .or(parts)
                .map(|error| {
                    error
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| error.to_string())
                });
        }
        let output = payload.get("output").or_else(|| payload.get("tools"))?;
        if field(payload, "type") == Some("tool_search_output") {
            return serde_json::to_string_pretty(output).ok();
        }
        let custom_output = field(payload, "type") == Some("custom_tool_call_output");
        if !custom_output || code_result_source {
            let structured = match output {
                Value::String(text) => serde_json::from_str::<Value>(text).ok(),
                other => Some(other.clone()),
            };
            if let Some(text) = structured.as_ref().and_then(|value| field(value, "output")) {
                return Some(text.to_owned());
            }
        }
        return Some(match output {
            Value::String(text) => text.clone(),
            Value::Array(_) => {
                // One exec result a script printed: its command's output.
                if code_result_source
                    && let Some(value) = events::yield_json(Some(output))
                    && let Some(text) = field(&value, "output")
                {
                    return Some(text.to_owned());
                }
                script_text(output)
            }
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        });
    }
    tool_result_text(record, block)
}

/// A code-mode script's output parts as text: what the script printed,
/// after the harness's "Script completed" header. A failure's header stays,
/// as it says what failed.
fn script_text(output: &Value) -> String {
    match events::output_content_view(output) {
        Some(events::OutputContentView::Text(text)) => text.to_owned(),
        Some(events::OutputContentView::Blocks(blocks)) => blocks
            .iter()
            .filter_map(|block| match block.content {
                events::OutputBlockContent::Text { header: true, .. } => None,
                events::OutputBlockContent::Text {
                    text,
                    header: false,
                } => Some(text.to_owned()),
                events::OutputBlockContent::Image => Some("[image]".to_owned()),
                events::OutputBlockContent::Unknown => {
                    Some("[unsupported output block]".to_owned())
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => serde_json::to_string_pretty(output).unwrap_or_default(),
    }
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

pub(crate) fn selected_image(
    lines: &mut Lines,
    slot: &Slot,
    image: &Value,
) -> Option<(&'static str, Vec<u8>)> {
    let file = slot.file.as_ref()?;
    if !matches!(&slot.kind, SlotKind::U | SlotKind::H(_)) {
        return None;
    }
    let offset = image["o"].as_u64()?;
    let block = usize::try_from(image["b"].as_u64()?).ok()?;
    let version = u64::from_str_radix(image["v"].as_str()?, 16).ok()?;
    let record = lines.get(&file.path, offset)?;
    attachments::image(&record, block, version)
}

/// Read one recorded script through the caller's qualified, budgeted source reader.
pub(crate) fn selected_script(lines: &mut Lines, slot: &Slot) -> Option<(String, bool)> {
    let file = slot.file.as_ref()?;
    let offset = match slot.kind {
        SlotKind::Operation {
            script_offset: Some(offset),
            ..
        } => offset,
        SlotKind::Yielded { .. } | SlotKind::Sent { .. } => slot.offset,
        _ => return None,
    };
    let mut script = script_at(lines, &file.path, offset)?;
    let truncated = script.len() > 64 * 1024;
    if truncated {
        let mut end = 64 * 1024;
        while !script.is_char_boundary(end) {
            end -= 1;
        }
        script.truncate(end);
    }
    Some((script, truncated))
}

/// Expand one structured tool result through the selected, budgeted reader.
/// Large plain strings use scalar checkpoints instead of this record path.
pub(crate) fn selected_output(lines: &mut Lines, slot: &Slot) -> Option<Value> {
    let file = slot.file.as_ref()?;
    let (text, source) = match &slot.kind {
        SlotKind::Tool {
            reply,
            item,
            result_source,
            ..
        } => tool_output(lines, &file.path, reply.as_ref(), *item, *result_source)?,
        SlotKind::Operation { kind, .. } if kind == "CommandExecution" => {
            let record = lines.get(&file.path, slot.offset)?;
            let output = record
                .pointer("/payload/item/aggregated_output")?
                .as_str()?;
            (output.to_owned(), Source::Collected)
        }
        _ => return None,
    };
    let mut selected = selected_text_value(&text, source, false, SELECTED_OUTPUT_MAX);
    if let SlotKind::Tool {
        reply: Some(reply), ..
    } = &slot.kind
        && let Some(record) = lines.get(&file.path, reply.o)
        && let Some(payload) = record.get("payload")
        && field(payload, "type") == Some("custom_tool_call_output")
        && let Some(output) = record.pointer("/payload/output")
        && let Some((blocks, truncated, omitted)) = selected_output_blocks(
            output,
            SELECTED_OUTPUT_MAX.saturating_sub(string_value_bytes(&selected)),
        )
    {
        selected["blocks"] = blocks;
        if truncated {
            selected["blocks_truncated"] = json!(true);
        }
        if omitted > 0 {
            selected["blocks_omitted"] = json!(omitted);
        }
    }
    Some(selected)
}

const SELECTED_OUTPUT_MAX: usize = 64 * 1024;
const OUTPUT_BLOCKS_MAX: usize = 128;
const OUTPUT_BLOCK_TYPE_MAX: usize = 128;

fn string_value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(values) => values.iter().map(string_value_bytes).sum(),
        Value::Object(values) => values.values().map(string_value_bytes).sum(),
        _ => 0,
    }
}

fn selected_output_blocks(output: &Value, budget: usize) -> Option<(Value, bool, usize)> {
    let events::OutputContentView::Blocks(parts) = events::output_content_view(output)? else {
        return None;
    };
    let omitted = parts.len().saturating_sub(OUTPUT_BLOCKS_MAX);
    let mut remaining = budget;
    let mut truncated = omitted > 0;
    let blocks = parts
        .iter()
        .take(OUTPUT_BLOCKS_MAX)
        .map(|part| {
            let (kind, kind_truncated) = part
                .kind
                .map(|kind| clip(kind, OUTPUT_BLOCK_TYPE_MAX))
                .unwrap_or_default();
            let mut block = json!({
                "ordinal": part.ordinal,
                "type": if part.kind.is_some() { json!(kind) } else { Value::Null },
            });
            if kind_truncated {
                block["type_truncated"] = json!(true);
                truncated = true;
            }
            match part.content {
                events::OutputBlockContent::Text { text, header } => {
                    block["role"] = json!(if header { "header" } else { "body" });
                    let (shown, cut) = if remaining == 0 {
                        (String::new(), !text.is_empty())
                    } else if text.len() > remaining {
                        let marker = "…";
                        if remaining < marker.len() {
                            (String::new(), true)
                        } else {
                            clip(text, remaining - marker.len())
                        }
                    } else {
                        clip(text, remaining)
                    };
                    remaining = remaining.saturating_sub(shown.len());
                    block["text"] = json!(shown);
                    if cut {
                        block["truncated"] = json!(true);
                        truncated = true;
                    }
                }
                events::OutputBlockContent::Image => {
                    block["role"] = json!("nontext");
                    block["fallback"] = json!("[image]");
                }
                events::OutputBlockContent::Unknown => {
                    block["role"] = json!("unknown");
                    block["fallback"] = json!("[unsupported output block]");
                }
            }
            block
        })
        .collect();
    Some((Value::Array(blocks), truncated, omitted))
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
            if let Some(t) = t {
                entry.insert("since".into(), json!(t));
            }
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

/// The summary for this particular task in a possibly batched notification.
fn background_summary(record: &Value, id: &str, status: &str) -> String {
    let text = record
        .get("attachment")
        .and_then(|attachment| attachment.get("prompt"))
        .or_else(|| {
            record
                .get("message")
                .and_then(|message| message.get("content"))
        })
        .map(content_text)
        .unwrap_or_default();
    events::notifications(&text)
        .into_iter()
        .find(|(tool, state, _)| *tool == Some(id) && *state == Some(status))
        .and_then(|(_, _, body)| events::inner(body, "summary"))
        .unwrap_or_default()
        .to_owned()
}

fn background_state(status: &str, exit: Option<i64>) -> &'static str {
    if matches!(status, "killed" | "stopped") {
        "killed"
    } else if status == "failed" || exit.is_some_and(|exit| exit != 0) {
        "failed"
    } else {
        "done"
    }
}

fn background_fields(
    lines: &mut Lines,
    bg: &model::Background,
    start: Option<i64>,
    now: i64,
) -> Value {
    let Some(end) = &bg.end else {
        return if bg.live {
            let mut value = json!({"state": "running", "secs": start.map_or_else(|| "—".to_owned(), |t| running(now - t))});
            if let Some(start) = start {
                value["since"] = json!(start);
            }
            value
        } else {
            json!({"state": "unknown"})
        };
    };
    let summary = lines
        .get(&end.file.path, end.offset)
        .map(|record| background_summary(&record, &bg.tid, &end.status))
        .unwrap_or_default();
    ended_fields(&summary, &end.status, start, end.t)
}

/// An ended background call's `bg`: on its step, and on its finish row, so a
/// viewer that loaded the step while it ran can update it from the row alone.
fn ended_fields(summary: &str, status: &str, start: Option<i64>, end: Option<i64>) -> Value {
    let exit = events::notification_exit(summary);
    let mut value = json!({
        "state": background_state(status, exit), "status": status,
        "secs": match (start, end) { (Some(start), Some(end)) => secs(end - start), _ => "—".to_owned() },
        "summary": clip(summary, PREVIEW_MAX).0,
    });
    if let Some(exit) = exit {
        value["exit"] = json!(exit);
    }
    value
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
        .or_else(|| {
            // This step does not carry the previous command's completion, so use the call start.
            match (slot.t, done.and_then(|done| done.t)) {
                (Some(start), Some(end)) => Some(format!("~{}", secs(end - start))),
                _ => None,
            }
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
        .and_then(|record| {
            let result_source = matches!(
                &slot.kind,
                SlotKind::Sent {
                    result_source: true,
                    ..
                }
            );
            (result_source.then(|| chunk_of(&record)).flatten())
                .or_else(|| result_text(&record, 0, result_source))
        })
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
    code_result_source: bool,
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
    let source = if matches!(
        record.pointer("/payload/item/type").and_then(Value::as_str),
        Some("McpToolCall" | "DynamicToolCall")
    ) {
        Source::Plain
    } else if record.get("payload").is_some() {
        Source::Model
    } else {
        Source::Plain
    };
    Some((
        result_text(&record, reply.b as usize, code_result_source)?,
        source,
    ))
}

fn think_text(record: &Value, block: usize) -> Option<String> {
    if let Some(payload) = record.get("payload") {
        if field(record, "type") != Some("response_item")
            || field(payload, "type") != Some("reasoning")
        {
            return None;
        }
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
    if TITLED_TOOLS.contains(&name)
        && let Some(title) = field(&input, "description")
            .map(|description| one_line(description, TITLE_MAX))
            .filter(|title| !title.is_empty())
    {
        entry.insert("title".into(), json!(title));
    }
    let mut more = Vec::new();
    if let Some(text) = tool_in(name, &input, &arg) {
        let (text, cut) = clip(&text, PREVIEW_MAX);
        entry.insert("in".into(), json!(text));
        if cut {
            more.push("in");
        }
    }
    match shown {
        Shown::Live => {
            entry.insert("live".into(), json!(true));
            if let Some(t) = slot.t {
                entry.insert("since".into(), json!(t));
            }
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
            let took = reply
                .and_then(|reply| lines.get(&file.path, reply.o))
                .and_then(|record| record.pointer("/payload/item").and_then(codex_duration))
                .unwrap_or_else(|| match (slot.t, reply.and_then(|reply| reply.t)) {
                    (Some(start), Some(end)) => secs(end - start),
                    _ => "—".to_owned(),
                });
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
            } else if let Some((text, source)) = tool_output(
                lines,
                &file.path,
                reply,
                item,
                matches!(
                    &slot.kind,
                    SlotKind::Tool {
                        result_source: true,
                        ..
                    }
                ),
            )
            .filter(|(text, _)| !text.trim().is_empty())
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
    if seconds == 0 && nanos < CODEX_COMMAND_MIN_DURATION_NS {
        return None;
    }
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
    let took = codex_duration(item).or_else(|| {
        if kind != "CommandExecution" {
            return None;
        }
        // The script may contain sequential or parallel commands. Its start
        // provides only an approximate span, which is marked in the display.
        let start = script_offset
            .and_then(|offset| lines.get(&file.path, offset))
            .and_then(|record| events::record_time(&record));
        match (start, slot.t) {
            (Some(start), Some(end)) => Some(format!("~{}", secs(end - start))),
            _ => None,
        }
    });
    entry.insert("secs".into(), json!(took.unwrap_or_else(|| "—".to_owned())));
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

pub(crate) struct RenderContext<'a> {
    pub(crate) home: Option<&'a str>,
    pub(crate) harness: Option<&'a str>,
    pub(crate) bounded: bool,
}

/// One slot as a `TX` entry; `None` for a slot with nothing to show.
pub(crate) fn render(
    context: &RenderContext<'_>,
    lines: &mut Lines,
    slots: &[Slot],
    slot: &Slot,
    index: usize,
    now: i64,
) -> Option<Value> {
    render_at(context, lines, slots, slot, index, index, now)
}

/// A selected window keeps local neighbor indices and original slot addresses
/// separate. All consumers retain the same native entry rendering rules.
pub(crate) fn render_at(
    context: &RenderContext<'_>,
    lines: &mut Lines,
    slots: &[Slot],
    slot: &Slot,
    index: usize,
    absolute: usize,
    now: i64,
) -> Option<Value> {
    let file = slot.file.as_deref();
    let mut record = || file.and_then(|file| lines.get(&file.path, slot.offset));
    let mut entry = match &slot.kind {
        // Your message is drawn from the model's ask, with the images its
        // line attaches.
        SlotKind::H(id) if model::is_ask(id) => {
            let record = prompt_record(lines, file, slot.offset);
            with_images(
                if context.bounded {
                    json!({"k":"h","id":id,"text":cap(&record.as_deref().and_then(prompt_text).unwrap_or_default(), MSG_MAX)})
                } else {
                    json!({"k": "h", "id": id})
                },
                record.as_deref(),
                file,
                slot.offset,
            )
        }
        SlotKind::H(id) => json!({"k": "h", "id": id}),
        SlotKind::U => {
            let record = prompt_record(lines, file, slot.offset);
            with_images(
                json!({"k": "u", "text": cap(&record.as_deref().and_then(prompt_text).unwrap_or_default(), MSG_MAX)}),
                record.as_deref(),
                file,
                slot.offset,
            )
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
            if context.harness == Some("claude")
                && let Some(secs) = thought_secs(slots, index)
            {
                entry["secs"] = json!(secs);
            }
            entry
        }
        SlotKind::Harness(label) => json!({"k": "harness", "label": label}),
        SlotKind::Signal(signal) => {
            json!({"k": "signal", "signal": signal, "at": slot.t, "turn":slot.turn,"slot":absolute})
        }
        SlotKind::Gap => {
            json!({"k": "end", "text": "Some entries not included: the log has unreadable lines here"})
        }
        SlotKind::NoActivity => json!({"k": "end", "text": "No activity in these logs"}),
        SlotKind::Returned { to, at, failed } => {
            json!({"k": "end", "ret": {"to": to, "at": at, "failed": failed}})
        }
        SlotKind::BgEnd {
            call,
            status,
            source,
            start,
        } => {
            let summary = record()
                .map(|record| background_summary(&record, call, status))
                .unwrap_or_default();
            let exit = events::notification_exit(&summary);
            let source_file = &source.0;
            let input = lines
                .get(&source_file.path, source.1)
                .and_then(|record| tool_input(&record, source.2 as usize))
                .unwrap_or(Value::Null);
            let label = field(&input, "description")
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    arg_summary("Bash", &input, source_file.cwd.as_deref(), context.home)
                });
            let mut entry = json!({
                "k": "bgend", "call": call, "state": background_state(status, exit), "label": clip(&label, PREVIEW_MAX).0,
                "bg": ended_fields(&summary, status, *start, slot.t),
            });
            if let Some(exit) = exit {
                entry["exit"] = json!(exit);
            }
            entry
        }
        SlotKind::Tool {
            shown,
            name,
            reply,
            item,
            bg,
            ..
        } => {
            let mut entry = tool_entry(
                lines,
                file?,
                slot,
                absolute,
                *shown,
                name,
                reply.as_ref(),
                *item,
                now,
                context.home,
            );
            if let Some(bg) = bg {
                entry["tid"] = json!(bg.tid);
                entry["bg"] = background_fields(lines, bg, slot.t, now);
            }
            entry
        }
        SlotKind::Operation {
            kind,
            ok,
            script_offset,
        } => operation_entry(lines, file?, slot, absolute, kind, *ok, *script_offset)?,
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
            absolute,
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
            ..
        } => sent_entry(
            lines,
            file?,
            slot,
            absolute,
            *shown,
            reply.as_ref(),
            *start,
            *done,
            now,
        ),
    };
    if let Some(record) = file.and_then(|file| lines.get(&file.path, slot.offset))
        && record.get("data").is_some()
        && record.get("parentId").is_some()
    {
        let data = &record["data"];
        let mut native = json!({"event_id":record["id"],"parent_event_id":record["parentId"]});
        for (source, target) in [
            ("messageId", "message_id"),
            ("originatingMessageId", "originating_message_id"),
            ("parentToolCallId", "parent_tool_call_id"),
            ("interactionId", "interaction_id"),
            ("turnId", "turn_id"),
            ("toolCallId", "tool_call_id"),
        ] {
            native[target] = data.get(source).cloned().unwrap_or(Value::Null);
        }
        if let Some(request) = data
            .get("toolRequests")
            .and_then(Value::as_array)
            .and_then(|requests| requests.get(slot.block as usize))
            && matches!(slot.kind, SlotKind::Tool { .. })
        {
            native["tool_call_id"] = request["toolCallId"].clone();
        }
        if let SlotKind::Tool {
            reply: Some(reply), ..
        } = &slot.kind
            && let Some(result) = file.and_then(|file| lines.get(&file.path, reply.o))
        {
            native["result_event_id"] = result["id"].clone();
            native["protocol_success"] = result["data"]["success"].clone();
            native["shell_exit_code"] = result["data"]["shellExecution"]["exitCode"].clone();
        }
        entry["native"] = native;
    }
    Some(entry)
}

/// A prompt's line as a page reads it or, when it is longer than a page reads
/// of one line ([`LINE_MAX`]), as the model reads it: its images can make it
/// that long, and its text and images still show.
fn prompt_record(lines: &mut Lines, file: Option<&SlotFile>, offset: u64) -> Option<Rc<Value>> {
    let file = file?;
    if let Some(provider) = lines.prompt_provider.as_mut() {
        let (record, bytes) = provider(file, offset)?;
        lines.bytes += bytes;
        return Some(Rc::new(record));
    }
    if lines.provider.is_some() {
        return lines.get(&file.path, offset);
    }
    let (record, bytes) = file.prompts.record(&file.path, offset)?;
    lines.bytes += bytes;
    Some(Rc::new(record))
}

/// `entry` with `img`: the images its prompt's line (at `offset`) attaches,
/// by reference ([`attachments::refs`], each with the line's offset `o`),
/// when there are any. Their bytes are served one at a time by
/// `/api/attachment?sid=&o=&b=`, never here.
fn with_images(
    mut entry: Value,
    record: Option<&Value>,
    file: Option<&SlotFile>,
    offset: u64,
) -> Value {
    let mut refs = file
        .and_then(|file| file.prompts.refs(offset))
        .unwrap_or_else(|| record.map(attachments::refs).unwrap_or_default());
    if !refs.is_empty() {
        for image in &mut refs {
            image["o"] = json!(offset);
        }
        entry["img"] = Value::Array(refs);
    }
    entry
}

pub(crate) fn thought_secs(slots: &[Slot], index: usize) -> Option<i64> {
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
pub(crate) fn slot_texts(lines: &mut Lines, slot: &Slot) -> Vec<(&'static str, String)> {
    let Some(file) = slot.file.as_deref() else {
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
            let code_result_source = matches!(
                &slot.kind,
                SlotKind::Tool {
                    result_source: true,
                    ..
                }
            );
            if let Some(input) = lines
                .get(&file.path, slot.offset)
                .and_then(|record| tool_input(&record, block))
            {
                let mut text = String::new();
                strings(&input, &mut text);
                texts.push(("in", text));
            }
            if let Some((text, _)) =
                tool_output(lines, &file.path, reply.as_ref(), *item, code_result_source)
            {
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

/// The native pager with request-owned source access. Its slots, rendering,
/// anchors and wire contract are shared with the existing local pager.
pub(crate) fn page_from_lines(
    context: &RenderContext<'_>,
    transcript: &model::Transcript,
    sid: &str,
    anchor: &Anchor,
    now: i64,
    limit: usize,
    lines: &mut Lines<'_>,
) -> io::Result<String> {
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
    // The background calls still running anywhere in the session: a viewer
    // that loaded one on an earlier page learns it stopped without its
    // process writing a line (and without fetching that page again).
    let bg_running: Vec<String> = slots
        .iter()
        .filter_map(|slot| match &slot.kind {
            SlotKind::Tool {
                bg:
                    Some(model::Background {
                        tid,
                        live: true,
                        end: None,
                    }),
                ..
            } => Some(tid.clone()),
            _ => None,
        })
        .collect();
    page_from_window(
        context,
        PageWindow {
            sid,
            slots,
            start: 0,
            total,
            from,
            forward,
            calls: transcript.calls,
            errors: transcript.errors,
            bg_running: &bg_running,
        },
        now,
        limit,
        lines,
    )
    .map(|page| page.to_string())
}

/// Original slot coordinates and exact totals for one request-owned window.
pub(crate) struct PageWindow<'a> {
    pub(crate) sid: &'a str,
    pub(crate) slots: &'a [Slot],
    pub(crate) start: usize,
    pub(crate) total: usize,
    pub(crate) from: usize,
    pub(crate) forward: bool,
    pub(crate) calls: usize,
    pub(crate) errors: usize,
    pub(crate) bg_running: &'a [String],
}

pub(crate) fn page_from_window(
    context: &RenderContext<'_>,
    window: PageWindow<'_>,
    now: i64,
    limit: usize,
    lines: &mut Lines<'_>,
) -> io::Result<Value> {
    let PageWindow {
        sid,
        slots,
        start,
        total,
        from,
        forward,
        calls,
        errors,
        bg_running,
    } = window;
    let limit = limit.clamp(1, PAGE_ENTRIES);
    let mut picked: Vec<(usize, Value)> = Vec::new();
    let mut bytes = 0;
    let (mut low, mut high) = (from, from);
    let mut take = |index: usize, picked: &mut Vec<(usize, Value)>| -> io::Result<bool> {
        let local = index.checked_sub(start).ok_or(io::ErrorKind::WouldBlock)?;
        let slot = slots.get(local).ok_or(io::ErrorKind::WouldBlock)?;
        // Thinking duration uses the preceding original slot. If paging reaches
        // the context row of a partial window, extend it before rendering.
        if index > 0 && local == 0 && matches!(slot.kind, SlotKind::Think) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let Some(entry) = render_at(context, lines, slots, slot, local, index, now) else {
            return Ok(true);
        };
        let size = entry.to_string().len() + 1;
        if !picked.is_empty() && (picked.len() >= limit || bytes + size > PAGE_BYTES) {
            return Ok(false);
        }
        bytes += size;
        picked.push((index, entry));
        Ok(true)
    };
    if forward {
        while high < total && take(high, &mut picked)? {
            high += 1;
        }
    } else {
        while low > 0 && take(low - 1, &mut picked)? {
            low -= 1;
        }
        picked.reverse();
    }
    let mut entries = Vec::with_capacity(picked.len());
    for (position, (index, mut entry)) in picked.into_iter().enumerate() {
        let slot = &slots[index - start];
        if let Some(turn) = &slot.turn
            && (slot.first || position == 0)
        {
            entry["turn"] = json!(turn);
        }
        entries.push(entry);
    }
    let mut page = json!({
        "sid": sid,
        "from": low,
        "to": high,
        "total": total,
        "calls": calls,
        "errors": errors,
        "entries": entries,
    });
    if !bg_running.is_empty() {
        page["bg_running"] = json!(bg_running);
    }
    Ok(page)
}

#[cfg(test)]
pub(crate) mod tests;
