//! Allowlisted native string recipes. Producer-only indexing may inspect a
//! complete source record; readers start at stored scalar checkpoints.
use crate::{json_string, slot_projection::Recipe};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

const PRODUCER_RECORD_MAX: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeField {
    pub(crate) record_offset: u64,
    pub(crate) block: u32,
    pub(crate) kind: NativeFieldKind,
    pub(crate) layers: u8,
    pub(crate) pointer: String,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) checkpoints: Vec<u64>,
    pub(crate) native_event_id: Option<String>,
}

impl NativeField {
    pub(crate) fn scalar_slack(&self) -> usize {
        if self.layers == 2 {
            json_string::JSON_STRING_LAYERED_SCALAR_SLACK
        } else {
            11
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NativeFieldKind {
    Text,
    ClaudeResult,
    CodexResult,
    CodexResultObject,
}
impl NativeFieldKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            _ => "out",
        }
    }
}

/// Allowlisted plain native text. Structured bodies, native formatting wrappers
/// and arbitrary JSON keys are deliberately excluded from persisted metadata.
#[cfg(test)]
pub(crate) fn capture(
    source: &crate::model::summary::CatalogSource,
    offset: u64,
    block: u32,
    recipe: &Recipe,
    records: Option<&crate::query_source::Records<'_>>,
) -> Option<NativeField> {
    capture_with_read(offset, block, recipe, |at| {
        read_source_record(source, at, records)
    })
}
#[cfg(test)]
pub(crate) fn capture_with_read(
    offset: u64,
    block: u32,
    recipe: &Recipe,
    read: impl FnOnce(u64) -> Option<Vec<u8>>,
) -> Option<NativeField> {
    capture_with_record(offset, block, recipe, read).0
}

/// One qualified record supplies both scalar location and native identity/image
/// evidence. Its body is borrowed only during preparation, never persisted.
pub(crate) fn capture_with_record(
    offset: u64,
    block: u32,
    recipe: &Recipe,
    read: impl FnOnce(u64) -> Option<Vec<u8>>,
) -> (Option<NativeField>, Option<(u64, serde_json::Value)>) {
    let (offset, block, kind) = match recipe {
        Recipe::U | Recipe::A | Recipe::Think => (offset, block, NativeFieldKind::Text),
        Recipe::Tool {
            reply: Some(reply),
            item: None,
            ..
        } => (reply.o, reply.b, NativeFieldKind::ClaudeResult),
        _ => return (None, None),
    };
    if block > 1024 {
        return (None, None);
    }
    let Some(bytes) = read(offset) else {
        return (None, None);
    };
    let Some(value) = crate::tx::parse_native_record(&bytes) else {
        return (None, None);
    };
    let field = json_string::index_json_string_spans(&bytes)
        .ok()
        .and_then(|spans| capture_parsed(&bytes, &spans, &value, offset, block, kind, recipe));
    (field, Some((offset, value)))
}

fn capture_parsed(
    bytes: &[u8],
    spans: &[json_string::JsonStringSpan],
    value: &serde_json::Value,
    offset: u64,
    block: u32,
    kind: NativeFieldKind,
    recipe: &Recipe,
) -> Option<NativeField> {
    let mut kind = kind;
    let mut layers = 1;
    let encoded_output = value
        .pointer("/payload/output")
        .and_then(|value| value.as_str())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok());
    let pointer = match recipe {
        Recipe::U | Recipe::A
            if value
                .pointer("/data/content")
                .is_some_and(serde_json::Value::is_string)
                && matches!(
                    value["type"].as_str(),
                    Some("user.message" | "assistant.message")
                ) =>
        {
            "/data/content".to_owned()
        }
        Recipe::U | Recipe::A
            if value
                .pointer("/payload/content")
                .is_some_and(serde_json::Value::is_array) =>
        {
            let parts = value.pointer("/payload/content")?.as_array()?;
            let mut text_parts = parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part["text"].is_string());
            let (index, part) = text_parts.next()?;
            if block != 0
                || text_parts.next().is_some()
                || !matches!(
                    part["type"].as_str(),
                    Some("input_text" | "output_text" | "text")
                )
            {
                return None;
            }
            format!("/payload/content/{index}/text")
        }
        Recipe::U if value.pointer("/message/content")?.is_string() => {
            "/message/content".to_owned()
        }
        Recipe::U => {
            let parts = value.pointer("/message/content")?.as_array()?;
            let mut text_parts = parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part["type"] == "text" && part["text"].is_string());
            let (index, _) = text_parts.next()?;
            if block != 0 || text_parts.next().is_some() {
                return None;
            }
            format!("/message/content/{index}/text")
        }
        Recipe::A => {
            let content = value.pointer(&format!("/message/content/{block}"))?;
            if content.get("type")?.as_str()? != "text" {
                return None;
            }
            format!("/message/content/{block}/text")
        }
        Recipe::Think => {
            let content = value.pointer(&format!("/message/content/{block}"))?;
            if content.get("type")?.as_str()? != "thinking" {
                return None;
            }
            format!("/message/content/{block}/thinking")
        }
        Recipe::Operation { .. } => {
            value.pointer("/payload/item/aggregated_output")?.as_str()?;
            "/payload/item/aggregated_output".to_owned()
        }
        Recipe::Tool { .. } => {
            if value
                .pointer("/payload/type")
                .and_then(|value| value.as_str())
                == Some("function_call_output")
            {
                let output = value.pointer("/payload/output")?;
                let (text, pointer, variant) = if let Some(text) = output.as_str() {
                    if let Some(inner) = encoded_output
                        .as_ref()
                        .and_then(|value| value.get("output"))
                        .and_then(|value| value.as_str())
                    {
                        layers = 2;
                        (inner, "/payload/output", NativeFieldKind::CodexResultObject)
                    } else {
                        (text, "/payload/output", NativeFieldKind::CodexResult)
                    }
                } else {
                    (
                        output.get("output")?.as_str()?,
                        "/payload/output/output",
                        NativeFieldKind::CodexResultObject,
                    )
                };
                let (plain, cut) = crate::tx::split_cut(text, crate::tx::Source::Model);
                if cut.is_some() || plain != text {
                    return None;
                }
                kind = variant;
                pointer.to_owned()
            } else {
                let content = value.pointer(&format!("/message/content/{block}"))?;
                if content.get("type")?.as_str()? != "tool_result" {
                    return None;
                }
                let body = content.get("content")?;
                if body.is_string() {
                    format!("/message/content/{block}/content")
                } else {
                    let parts = body.as_array()?;
                    if parts.len() != 1
                        || parts[0].get("type")?.as_str()? != "text"
                        || !parts[0].get("text")?.is_string()
                    {
                        return None;
                    }
                    format!("/message/content/{block}/content/0/text")
                }
            }
        }
        _ => return None,
    };
    let span = spans.iter().find(|span| span.json_pointer == pointer)?;
    let span = if layers == 2 {
        json_string::index_nested_json_string_spans(bytes, span)
            .ok()?
            .into_iter()
            .find(|span| span.json_pointer == "/output")?
    } else {
        span.clone()
    };
    let native_event_id = value
        .get("uuid")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty() && id.len() <= 4096)
        .map(str::to_owned);
    Some(NativeField {
        record_offset: offset,
        block,
        kind,
        layers,
        pointer,
        start: span.start,
        end: span.end,
        checkpoints: span.checkpoints,
        native_event_id,
    })
}

/// Body-free metadata for reconstructing bounded portions of a large record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SparseRecord {
    length: u64,
    fields: Vec<NativeField>,
}

pub(crate) struct SelectedField {
    pub(crate) block: u32,
    pub(crate) kind: NativeFieldKind,
    pub(crate) chunks: usize,
    pub(crate) text: Option<String>,
}

impl std::fmt::Debug for SelectedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectedField")
            .field("block", &self.block)
            .field("kind", &self.kind)
            .field("chunks", &self.chunks)
            .finish_non_exhaustive()
    }
}

impl SparseRecord {
    pub(crate) fn capture(value: &serde_json::Value, raw: &[u8], offset: u64) -> Option<Self> {
        use crate::{events::Reply, field};
        if raw.len() <= json_string::JSON_STRING_CHUNK_DECODED_MAX {
            return None;
        }
        let recipe = |block| Recipe::Tool {
            outcome: crate::slot_projection::Outcome::Ok,
            name: String::new(),
            reply: Some(Reply {
                b: block,
                ..Reply::default()
            }),
            result_source: false,
            item: None,
            background: None,
        };
        let mut candidates = Vec::new();
        let parts = || {
            value
                .pointer("/message/content")
                .and_then(serde_json::Value::as_array)
        };
        match field(value, "type") {
            Some("user")
                if parts().is_some_and(|parts| {
                    parts
                        .iter()
                        .any(|part| field(part, "type") == Some("tool_result"))
                }) =>
            {
                for (block, part) in parts()?.iter().take(1025).enumerate() {
                    if field(part, "type") == Some("tool_result") {
                        candidates.push((
                            block as u32,
                            NativeFieldKind::ClaudeResult,
                            recipe(block as u32),
                        ));
                    }
                }
            }
            Some("user" | "user.message") => candidates.push((0, NativeFieldKind::Text, Recipe::U)),
            Some("assistant.message") => candidates.push((0, NativeFieldKind::Text, Recipe::A)),
            Some("assistant") => {
                for (block, part) in parts()?.iter().take(1025).enumerate() {
                    let recipe = match field(part, "type") {
                        Some("text") => Recipe::A,
                        Some("thinking") => Recipe::Think,
                        _ => continue,
                    };
                    candidates.push((block as u32, NativeFieldKind::Text, recipe));
                }
            }
            Some("event_msg")
                if value
                    .pointer("/payload/type")
                    .and_then(serde_json::Value::as_str)
                    == Some("item_completed")
                    && value
                        .pointer("/payload/item/type")
                        .and_then(serde_json::Value::as_str)
                        == Some("CommandExecution") =>
            {
                candidates.push((
                    0,
                    NativeFieldKind::CodexResult,
                    Recipe::Operation {
                        kind: "CommandExecution".to_owned(),
                        ok: None,
                        script_offset: None,
                    },
                ));
            }
            Some("response_item") => {
                let payload = value.get("payload")?;
                match (field(payload, "type"), field(payload, "role")) {
                    (Some("message"), Some("user")) => {
                        candidates.push((0, NativeFieldKind::Text, Recipe::U))
                    }
                    (Some("message"), Some("assistant")) => {
                        candidates.push((0, NativeFieldKind::Text, Recipe::A))
                    }
                    (Some("function_call_output"), _) => {
                        candidates.push((0, NativeFieldKind::ClaudeResult, recipe(0)))
                    }
                    _ => (),
                }
            }
            _ => (),
        }
        if candidates.is_empty() {
            return None;
        }
        let spans = json_string::index_json_string_spans(raw).ok()?;
        let mut fields: Vec<_> = candidates
            .into_iter()
            .filter_map(|(block, kind, recipe)| {
                capture_parsed(raw, &spans, value, offset, block, kind, &recipe)
            })
            .filter(|field| {
                field.end - field.start > json_string::JSON_STRING_CHUNK_DECODED_MAX as u64
            })
            .collect();
        fields.sort_by_key(|field| field.start);
        (!fields.is_empty()).then_some(Self {
            length: raw.len() as u64,
            fields,
        })
    }

    /// Keep the native record structure and a bounded prefix of each selected
    /// scalar. Requested chunks use the exact source checkpoints, independent
    /// of the number of preceding bytes.
    pub(crate) fn read(
        &self,
        reader: &dyn crate::SessionSourceReader,
        source: &crate::SessionSourceRef,
        offset: u64,
        (preview, chunk): (usize, Option<usize>),
        omitted: impl Iterator<Item = std::ops::Range<u64>>,
        bytes_left: &mut usize,
    ) -> std::io::Result<(serde_json::Value, Vec<SelectedField>)> {
        use crate::source_reader::append_committed_range;
        let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
        let end = offset
            .checked_add(self.length)
            .filter(|end| *end <= source.offset)
            .ok_or_else(invalid)?;
        let mut omitted = omitted.peekable();
        let mut append =
            |mut start: u64, end: u64, native: &mut Vec<u8>, bytes_left: &mut usize| {
                while let Some(range) = omitted.next_if(|range| range.start < end) {
                    if range.start < start || range.end > end {
                        return Err(invalid());
                    }
                    append_committed_range(
                        reader,
                        source,
                        offset + start..offset + range.start,
                        native,
                        bytes_left,
                    )?;
                    start = range.end;
                }
                append_committed_range(
                    reader,
                    source,
                    offset + start..offset + end,
                    native,
                    bytes_left,
                )
            };
        let mut native = Vec::new();
        let mut selected = Vec::new();
        let mut cursor = 0;
        for field in &self.fields {
            if field.record_offset != offset
                || field.start < cursor
                || field.end > self.length
                || field.checkpoints.first() != Some(&field.start)
                || field.checkpoints.last() != Some(&field.end)
                || !field.checkpoints.windows(2).all(|pair| {
                    pair[0] < pair[1]
                        && pair[1] - pair[0]
                            <= json_string::JSON_STRING_CHECKPOINT_BYTES
                                + field.scalar_slack() as u64
                })
            {
                return Err(invalid());
            }
            append(cursor, field.start, &mut native, bytes_left)?;
            let mut decoded = 0;
            let mut text = None;
            for (index, pair) in field.checkpoints.windows(2).enumerate() {
                if decoded > preview && chunk != Some(index) {
                    continue;
                }
                let mut raw = Vec::new();
                append_committed_range(
                    reader,
                    source,
                    offset + pair[0]..offset + pair[1],
                    &mut raw,
                    bytes_left,
                )?;
                let slice = json_string::decode_json_string_layered_chunk(
                    &raw,
                    field.layers,
                    true,
                    json_string::JSON_STRING_CHUNK_DECODED_MAX,
                )?;
                if !slice.complete || slice.consumed != raw.len() {
                    return Err(invalid());
                }
                if decoded <= preview {
                    decoded += slice.text.len();
                    native.extend_from_slice(&raw);
                }
                if chunk == Some(index) {
                    text = Some(slice.text);
                }
            }
            selected.push(SelectedField {
                block: field.block,
                kind: field.kind,
                chunks: field.checkpoints.len().saturating_sub(1),
                text,
            });
            cursor = field.end;
        }
        append(cursor, end - offset, &mut native, bytes_left)?;
        if omitted.next().is_some() {
            return Err(invalid());
        }
        let value = crate::tx::parse_native_record(&native).ok_or_else(invalid)?;
        Ok((value, selected))
    }
}

/// A small, qualified string preview reuses the selected transcript's scalar
/// descriptor. It never reads a complete large native record.
pub(crate) fn preview(
    reader: &dyn crate::SessionSourceReader,
    source: &crate::SessionSourceRef,
    field: &NativeField,
    kind: &str,
    bytes_left: &mut usize,
) -> std::io::Result<String> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Native preview generation or field changed",
        )
    };
    let start = field
        .record_offset
        .checked_add(field.start)
        .ok_or_else(invalid)?;
    let end = field
        .record_offset
        .checked_add(field.end)
        .ok_or_else(invalid)?;
    if field.block > 1024
        || field.checkpoints.first() != Some(&field.start)
        || field.checkpoints.last() != Some(&field.end)
        || start > end
        || end > source.offset
    {
        return Err(invalid());
    }
    if start == end {
        return Ok(String::new());
    }
    let mut at = start;
    let mut text = String::new();
    let mut generation = None::<String>;
    let mut length = None;
    while at < end {
        let max = usize::try_from(end - at)
            .unwrap_or(usize::MAX)
            .min(8192 + field.scalar_slack())
            .min(*bytes_left);
        if max == 0 || text.len() >= 128 * 1024 {
            return Err(invalid());
        }
        let part = reader.read_range(source, generation.as_deref(), at, max)?;
        if part.offset != at
            || part.bytes.is_empty()
            || part.bytes.len() > max
            || part.length < source.offset
            || part.generation.is_empty()
            || part.generation.len() > 4096
            || generation
                .as_ref()
                .is_some_and(|value| value != &part.generation)
            || length.is_some_and(|value| value != part.length)
        {
            return Err(invalid());
        }
        *bytes_left -= part.bytes.len();
        generation = Some(part.generation);
        length = Some(part.length);
        let chunk = json_string::decode_json_string_layered_chunk(
            &part.bytes,
            field.layers,
            at + part.bytes.len() as u64 == end,
            (128 * 1024 - text.len()).min(8192),
        )?;
        if chunk.consumed == 0 {
            return Err(invalid());
        }
        at += chunk.consumed as u64;
        text.push_str(&chunk.text);
        let record = preview_record(field, &text, kind).ok_or_else(invalid)?;
        let visible = match kind {
            "prompt" | "trace_prompt" => crate::model::prompt_text(&record),
            "assistant" => crate::model::assistant_text(&record, field.block as usize),
            "tool_result" => crate::model::tool_result_text(&record, field.block as usize),
            _ => None,
        }
        .ok_or_else(invalid)?;
        // One byte beyond the display cap distinguishes a complete value from
        // a capped prefix and preserves the established truncation marker.
        if visible.len() > crate::model::MSG_MAX || at == end {
            return Ok(text);
        }
    }
    Ok(text)
}

/// Minimal records feed the existing native text parsers. No caller-defined
/// pointer becomes a field read; descriptors use the producer's allowlist.
pub(crate) fn preview_record(
    field: &NativeField,
    text: &str,
    kind: &str,
) -> Option<serde_json::Value> {
    use serde_json::{Value, json};
    match field.kind {
        NativeFieldKind::Text => {
            if field.pointer == "/message/content" {
                return Some(json!({"message":{"content":text}}));
            }
            if field.pointer == "/data/content" {
                return Some(
                    json!({"type":if kind=="assistant" {"assistant.message"} else {"user.message"},"data":{"content":text}}),
                );
            }
            if field.pointer == "/payload/content/0/text" {
                return Some(json!({"payload":{"content":[{"type":"input_text","text":text}]}}));
            }
            let member = if field.pointer == format!("/message/content/{}/text", field.block) {
                "text"
            } else if field.pointer == format!("/message/content/{}/thinking", field.block) {
                "thinking"
            } else {
                return None;
            };
            let mut blocks = vec![Value::Null; field.block as usize + 1];
            let mut part = json!({"type":member});
            part[member] = json!(text);
            blocks[field.block as usize] = part;
            Some(json!({"message":{"content":blocks}}))
        }
        NativeFieldKind::ClaudeResult => {
            let mut blocks = vec![Value::Null; field.block as usize + 1];
            blocks[field.block as usize] = json!({"type":"tool_result","content":text});
            Some(json!({"message":{"content":blocks}}))
        }
        NativeFieldKind::CodexResult => {
            Some(json!({"payload":{"type":"function_call_output","output":text}}))
        }
        NativeFieldKind::CodexResultObject => {
            Some(json!({"payload":{"type":"function_call_output","output":{"output":text}}}))
        }
    }
}

/// Producer-only record read bound to the exact ledger observation, not merely
/// to a pathname. Refuse bytes changed between event parsing and projection.
pub(crate) fn read_source_record(
    source: &crate::model::summary::CatalogSource,
    offset: u64,
    records: Option<&crate::query_source::Records<'_>>,
) -> Option<Vec<u8>> {
    if let Some(records) = records {
        return records
            .record_bytes(&source.path, offset, PRODUCER_RECORD_MAX)
            .ok()
            .flatten();
    }
    let remaining = source.offset.checked_sub(offset)?;
    if remaining == 0 {
        return None;
    }
    let mut file = crate::sealed::LogFile::open(&source.path).ok()?;
    if !source_matches(source, &file.metadata().ok()?) {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    BufReader::new(&mut file)
        .take(remaining.min(PRODUCER_RECORD_MAX + 1))
        .read_until(b'\n', &mut bytes)
        .ok()?;
    if bytes.len() as u64 > PRODUCER_RECORD_MAX
        || bytes.last() != Some(&b'\n')
        || !source_matches(source, &file.metadata().ok()?)
    {
        return None;
    }
    Some(bytes)
}
fn source_matches(
    source: &crate::model::summary::CatalogSource,
    metadata: &std::fs::Metadata,
) -> bool {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = (0, 0);
    source.changed_ns.is_some()
        && source.changed_ns == crate::events::change_time_ns(metadata)
        && identity == (source.dev, source.ino)
        && metadata.len() == source.size
        && metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_nanos())
            == Some(source.modified_ns)
}
