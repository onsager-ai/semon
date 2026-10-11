//! Allowlisted native string recipes. Producer-only indexing may inspect a
//! complete source record; readers start at stored scalar checkpoints.
use crate::{json_string, slot_projection::Recipe};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

const PRODUCER_RECORD_MAX: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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
pub(crate) fn capture_with_read(
    offset: u64,
    block: u32,
    recipe: &Recipe,
    read: impl FnOnce(u64) -> Option<Vec<u8>>,
) -> Option<NativeField> {
    let (offset, block, kind) = match recipe {
        Recipe::U | Recipe::A | Recipe::Think => (offset, block, NativeFieldKind::Text),
        Recipe::Tool {
            reply: Some(reply),
            item: None,
            ..
        } => (reply.o, reply.b, NativeFieldKind::ClaudeResult),
        _ => return None,
    };
    if block > 1024 {
        return None;
    }
    let bytes = read(offset)?;
    let value = crate::tx::parse_native_record(&bytes)?;
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
            if parts.len() != 1
                || block != 0
                || !matches!(
                    parts[0]["type"].as_str(),
                    Some("input_text" | "output_text" | "text")
                )
                || !parts[0]["text"].is_string()
            {
                return None;
            }
            "/payload/content/0/text".to_owned()
        }
        Recipe::U if value.pointer("/message/content")?.is_string() => {
            "/message/content".to_owned()
        }
        Recipe::U | Recipe::A => {
            if matches!(recipe, Recipe::U)
                && value.pointer("/message/content")?.as_array()?.len() != 1
            {
                return None;
            }
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
    let span = json_string::index_json_string_spans(&bytes)
        .ok()?
        .into_iter()
        .find(|span| span.json_pointer == pointer)?;
    let span = if layers == 2 {
        json_string::index_nested_json_string_spans(&bytes, &span)
            .ok()?
            .into_iter()
            .find(|span| span.json_pointer == "/output")?
    } else {
        span
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
