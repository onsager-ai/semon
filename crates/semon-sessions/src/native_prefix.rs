//! Bounded native identity proof for an acknowledged mirrored prefix.
use serde::Deserialize;
use std::{
    io::{self, Read},
    path::Path,
};

/// Reads only the first complete Codex record, within the accepted prefix.
/// A partial, malformed, duplicate-field or oversized record is unavailable.
/// Callers must establish generation/ACK proof and protect the path from writes.
pub fn codex_native_id_prefix(path: &Path, accepted_bytes: u64) -> io::Result<Option<String>> {
    #[derive(Deserialize)]
    struct Record {
        #[serde(rename = "type")]
        kind: String,
        payload: Payload,
    }
    #[derive(Deserialize)]
    struct Payload {
        id: String,
    }
    let mut file = crate::sealed::LogFile::open(path)?;
    if accepted_bytes > file.metadata()?.len() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    let mut limited = (&mut file).take(accepted_bytes.min(64 * 1024));
    let mut buffer = [0; 1024];
    loop {
        let n = limited.read(&mut buffer)?;
        if n == 0 {
            return Ok(None);
        }
        if let Some(end) = buffer[..n].iter().position(|b| *b == b'\n') {
            bytes.extend_from_slice(&buffer[..end]);
            break;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
        return Ok(None);
    };
    if record.kind != "session_meta"
        || record.payload.id.is_empty()
        || record.payload.id.len() > 256
        || record
            .payload
            .id
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ')
    {
        return Ok(None);
    }
    Ok(Some(record.payload.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_first_record_requires_complete_unique_native_metadata() {
        let root = std::env::temp_dir().join(format!("semon-native-prefix-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("native.jsonl");
        for (record, expected) in [
            (
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"exact-native\"}}\n",
                Some("exact-native"),
            ),
            (
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"a\",\"id\":\"b\"}}\n",
                None,
            ),
            (
                "{\"type\":\"session_meta\",\"type\":\"event_msg\",\"payload\":{\"id\":\"a\"}}\n",
                None,
            ),
            (
                "{\"type\":\"event_msg\",\"payload\":{\"id\":\"a\"}}\n",
                None,
            ),
            (
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"\"}}\n",
                None,
            ),
            (
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"a\"}}",
                None,
            ),
        ] {
            std::fs::write(&path, record).unwrap();
            assert_eq!(
                codex_native_id_prefix(&path, record.len() as u64)
                    .unwrap()
                    .as_deref(),
                expected
            );
            assert_eq!(
                codex_native_id_prefix(&path, record.len().saturating_sub(1) as u64).unwrap(),
                None
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
