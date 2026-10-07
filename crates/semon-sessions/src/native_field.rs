//! Allowlisted native string recipes. Producer-only indexing may inspect a
//! complete source record; readers start at stored scalar checkpoints.
use crate::{json_string, slot_projection::Recipe};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const PRODUCER_RECORD_MAX: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeField {
    pub(crate) record_offset: u64,
    pub(crate) block: u32,
    pub(crate) kind: NativeFieldKind,
    pub(crate) pointer: String,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) checkpoints: Vec<u64>,
    pub(crate) native_event_id: Option<String>,
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
    path: &Path,
    offset: u64,
    block: u32,
    recipe: &Recipe,
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
    let mut file = crate::sealed::LogFile::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    BufReader::new(file)
        .take(PRODUCER_RECORD_MAX + 1)
        .read_until(b'\n', &mut bytes)
        .ok()?;
    if bytes.len() as u64 > PRODUCER_RECORD_MAX {
        return None;
    }
    let value = crate::tx::parse_native_record(&bytes)?;
    let mut kind = kind;
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
                    if serde_json::from_str::<serde_json::Value>(text)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("output")
                                .and_then(|value| value.as_str())
                                .map(str::to_owned)
                        })
                        .is_some()
                    {
                        return None;
                    }
                    (text, "/payload/output", NativeFieldKind::CodexResult)
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
                if content.get("type")?.as_str()? != "tool_result"
                    || !content.get("content")?.is_string()
                {
                    return None;
                }
                format!("/message/content/{block}/content")
            }
        }
        _ => return None,
    };
    let span = json_string::index_json_string_spans(&bytes)
        .ok()?
        .into_iter()
        .find(|span| span.json_pointer == pointer)?;
    let native_event_id = value
        .get("uuid")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty() && id.len() <= 4096)
        .map(str::to_owned);
    Some(NativeField {
        record_offset: offset,
        block,
        kind,
        pointer,
        start: span.start,
        end: span.end,
        checkpoints: span.checkpoints,
        native_event_id,
    })
}
