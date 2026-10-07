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

/// Plain Claude text only. Structured bodies, native formatting wrappers and
/// arbitrary JSON keys are deliberately excluded from persisted metadata.
pub(crate) fn capture(
    source: &crate::model::summary::CatalogSource,
    offset: u64,
    block: u32,
    recipe: &Recipe,
) -> Option<NativeField> {
    let (offset, block) = match recipe {
        Recipe::U | Recipe::A | Recipe::Think => (offset, block),
        Recipe::Tool {
            reply: Some(reply),
            item: None,
            ..
        } => (reply.o, reply.b),
        _ => return None,
    };
    if block > 1024 {
        return None;
    }
    let bytes = read_source_record(source, offset)?;
    capture_from_verified_record(&bytes, offset, block, recipe)
}

/// Producer-only mapping from one already generation-verified original record.
/// The caller must bind these bytes and the source offset to an immutable
/// generation before publishing recipes. This helper supplies no authorization
/// or generation proof and must never be used as a request-time record parser.
pub(crate) fn capture_from_verified_record(
    bytes: &[u8],
    offset: u64,
    block: u32,
    recipe: &Recipe,
) -> Option<NativeField> {
    let kind = match recipe {
        Recipe::U | Recipe::A | Recipe::Think => NativeFieldKind::Text,
        Recipe::Tool {
            reply: Some(reply),
            item: None,
            ..
        } if reply.o == offset && reply.b == block => NativeFieldKind::ClaudeResult,
        _ => return None,
    };
    if block > 1024 || bytes.len() > 32 * 1024 * 1024 {
        return None;
    }
    let value = crate::tx::parse_native_record(bytes)?;
    let mut kind = kind;
    let mut layers = 1;
    let encoded_output = value
        .pointer("/payload/output")
        .and_then(|value| value.as_str())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok());
    let pointer = match recipe {
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
    let span = json_string::index_json_string_spans(bytes)
        .ok()?
        .into_iter()
        .find(|span| span.json_pointer == pointer)?;
    let span = if layers == 2 {
        json_string::index_nested_json_string_spans(bytes, &span)
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

/// Producer-only record read bound to the exact ledger observation, not merely
/// to a pathname. Refuse bytes changed between event parsing and projection.
pub(crate) fn read_source_record(
    source: &crate::model::summary::CatalogSource,
    offset: u64,
) -> Option<Vec<u8>> {
    if source.immutable_generation.is_some() {
        return None;
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
        && (Some(identity.0), Some(identity.1)) == (source.dev, source.ino)
        && metadata.len() == source.size
        && metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_nanos())
            == source.modified_ns
}

#[cfg(test)]
mod verified_record_tests {
    use super::*;

    #[test]
    fn verified_original_record_fields_preserve_offsets_and_native_identity() {
        let bytes = br#"{"type":"user","uuid":"native-event-1","message":{"role":"user","content":"original text"}}
"#;
        let field = capture_from_verified_record(bytes, 4096, 0, &Recipe::U).unwrap();
        assert_eq!(field.record_offset, 4096);
        assert_eq!(field.native_event_id.as_deref(), Some("native-event-1"));
        assert_eq!(
            &bytes[field.start as usize..field.end as usize],
            b"original text"
        );
        assert_eq!(field.layers, 1);
        assert!(capture_from_verified_record(bytes, 4096, 1025, &Recipe::U).is_none());
        assert!(capture_from_verified_record(b"not JSON", 4096, 0, &Recipe::U).is_none());
    }
}
