//! Allowlisted native string recipes. Producer-only indexing may inspect a
//! complete source record; readers start at stored scalar checkpoints.
use crate::{json_string, slot_projection::Recipe};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeField {
    pub(crate) pointer: String,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) checkpoints: Vec<u64>,
    pub(crate) native_event_id: Option<String>,
}

/// Plain Claude text only. Structured bodies, native formatting wrappers and
/// arbitrary JSON keys are deliberately excluded from persisted metadata.
pub(crate) fn capture(
    path: &Path,
    offset: u64,
    block: u32,
    recipe: &Recipe,
) -> Option<NativeField> {
    if !matches!(recipe, Recipe::U | Recipe::A | Recipe::Think) {
        return None;
    }
    let mut file = crate::sealed::LogFile::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    BufReader::new(file)
        .take(crate::tx::LINE_MAX + 1)
        .read_until(b'\n', &mut bytes)
        .ok()?;
    if bytes.len() as u64 > crate::tx::LINE_MAX {
        return None;
    }
    let value = crate::tx::parse_native_record(&bytes)?;
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
        _ => return None,
    };
    let span = json_string::index_json_string_spans(&bytes)
        .ok()?
        .into_iter()
        .find(|span| span.json_pointer == pointer)?;
    let native_event_id = value
        .get("uuid")
        .and_then(|value| value.as_str())
        .filter(|id| id.len() <= 4096)
        .map(str::to_owned);
    Some(NativeField {
        pointer,
        start: span.start,
        end: span.end,
        checkpoints: span.checkpoints,
        native_event_id,
    })
}
