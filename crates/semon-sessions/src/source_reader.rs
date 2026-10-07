//! Provider-neutral access to native source bytes for parsed session views.
//! There is no HTTP endpoint, serialization or authorization grant here.
use crate::SessionSourceRef;
use std::{fmt, io, sync::Arc};

/// A single provider request never asks for more than this many source bytes.
pub const SESSION_SOURCE_CHUNK_MAX: usize = 128 * 1024;

/// A host-owned source reader. References are identity/provenance, not bearer
/// authority: implementations authorize the caller, machine, logical root/path
/// and native identity, and bind the consumed prefix to an independently
/// verified source generation before reading. Partial prefixes without that
/// proof are unavailable, rather than permission to read a different object.
///
/// Implementations enforce bounded I/O deadlines and cancellation according to
/// their host policy. `expected_generation`, when present, must match before
/// and after I/O. Reads may return fewer bytes than requested, but may not return
/// bytes outside the requested range. Async hosts run native rendering outside
/// their request runtime and bridge their existing scoped provider service.
/// Readers belong to one authorized request and are passed by reference to its
/// renderer. Never retain a reader or its request credentials in a shared
/// ViewerCore, workspace model, source cache or persistent projection. Shared
/// byte caches reauthorize every read, including cache hits.
/// Bytes feed Semon's native parsers; they must not become an ordinary trace or
/// raw HTTP response. Credential custody and archive access policy stay hosted.
pub trait SessionSourceReader: Send + Sync {
    fn read_range(
        &self,
        source: &SessionSourceRef,
        expected_generation: Option<&str>,
        offset: u64,
        max_bytes: usize,
    ) -> io::Result<SessionSourceRange>;
}

/// A bounded read from one independently bound native generation. This type
/// deliberately does not implement Serialize or print bytes through Debug.
#[derive(Clone)]
pub struct SessionSourceRange {
    pub generation: String,
    pub length: u64,
    pub offset: u64,
    pub bytes: Arc<[u8]>,
    pub cached: bool,
}

impl fmt::Debug for SessionSourceRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionSourceRange")
            .field("generation", &self.generation)
            .field("length", &self.length)
            .field("offset", &self.offset)
            .field("bytes_len", &self.bytes.len())
            .field("cached", &self.cached)
            .finish()
    }
}

/// Native line oracle for provider integration. Normal range rendering will
/// reuse this parser after the persisted slot projection is available; it must
/// not use a full selected FileIndex decode as its per-page fallback.
#[cfg(test)]
fn read_record(
    reader: &dyn SessionSourceReader,
    source: &SessionSourceRef,
    offset: u64,
) -> io::Result<serde_json::Value> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "native source generation or range changed",
        )
    };
    if offset >= source.offset {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    let mut generation = None::<String>;
    let mut length = None;
    let mut at = offset;
    loop {
        let remaining = (crate::tx::LINE_MAX + 1).saturating_sub(bytes.len() as u64);
        let max = SESSION_SOURCE_CHUNK_MAX
            .min(usize::try_from(source.offset - at).unwrap_or(usize::MAX))
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if max == 0 {
            return Err(invalid());
        }
        let part = reader.read_range(source, generation.as_deref(), at, max)?;
        if part.offset != at
            || part.bytes.len() > max
            || part.bytes.is_empty()
            || part.generation.is_empty()
            || part.generation.len() > 4096
            || part.length < source.offset
            || generation
                .as_ref()
                .is_some_and(|expected| *expected != part.generation)
            || length.is_some_and(|expected| expected != part.length)
        {
            return Err(invalid());
        }
        if generation.is_none() {
            generation = Some(part.generation);
            length = Some(part.length);
        }
        let end = part
            .bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|end| end + 1);
        let take = end.unwrap_or(part.bytes.len());
        bytes.extend_from_slice(&part.bytes[..take]);
        at += take as u64;
        if bytes.len() as u64 > crate::tx::LINE_MAX {
            return Err(invalid());
        }
        if end.is_some() {
            return crate::tx::parse_native_record(&bytes).ok_or_else(invalid);
        }
        if at == source.offset {
            // The consumed boundary consists of complete native lines only.
            return Err(invalid());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Reader {
        bytes: Arc<[u8]>,
        change_at: Option<usize>,
        calls: Mutex<Vec<(Option<String>, u64, usize)>>,
    }
    impl SessionSourceReader for Reader {
        fn read_range(
            &self,
            _source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<SessionSourceRange> {
            assert!(max <= SESSION_SOURCE_CHUNK_MAX);
            let mut calls = self.calls.lock().unwrap();
            let generation = if self.change_at.is_some_and(|at| calls.len() >= at) {
                "changed"
            } else {
                "verified"
            };
            calls.push((expected.map(str::to_owned), offset, max));
            let from = usize::try_from(offset).unwrap();
            let to = from.saturating_add(max).min(self.bytes.len());
            Ok(SessionSourceRange {
                generation: generation.into(),
                length: self.bytes.len() as u64,
                offset,
                bytes: Arc::from(&self.bytes[from..to]),
                cached: true,
            })
        }
    }
    fn source(length: usize) -> SessionSourceRef {
        SessionSourceRef {
            root: "claude".into(),
            path: "projects/project/session.jsonl".into(),
            native_id: "session".into(),
            offset: length as u64,
            prefix_sha256: [1; 32],
            tail_sha256: [2; 32],
            immutable_generation: None,
        }
    }
    #[test]
    fn provider_native_line_reads_bound_chunks_and_pin_generation() {
        let record = serde_json::json!({"type":"user","message":{"role":"user","content":"x".repeat(SESSION_SOURCE_CHUNK_MAX * 2)}});
        let bytes: Arc<[u8]> = (record.to_string() + "\n").into_bytes().into();
        let reference = source(bytes.len());
        let reader = Reader {
            bytes: bytes.clone(),
            change_at: None,
            calls: Mutex::default(),
        };
        assert_eq!(read_record(&reader, &reference, 0).unwrap(), record);
        let calls = reader.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, None);
        assert_eq!(calls[1].0.as_deref(), Some("verified"));
        assert_eq!(calls[2].0.as_deref(), Some("verified"));
        assert_eq!(calls[2].1 + calls[2].2 as u64, reference.offset);
        drop(calls);
        let replaced = Reader {
            bytes,
            change_at: Some(1),
            calls: Mutex::default(),
        };
        assert_eq!(
            read_record(&replaced, &reference, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(replaced.calls.lock().unwrap().len(), 2);
    }
    #[test]
    fn provider_native_line_never_reads_unconsumed_or_unterminated_bytes() {
        let bytes: Arc<[u8]> = b"{\"type\":\"user\"}\nsecret trailing partial"
            .to_vec()
            .into();
        let consumed = b"{\"type\":\"user\"}\n".len();
        let reader = Reader {
            bytes,
            change_at: None,
            calls: Mutex::default(),
        };
        let reference = source(consumed);
        assert_eq!(read_record(&reader, &reference, 0).unwrap()["type"], "user");
        assert_eq!(reader.calls.lock().unwrap()[0].2, consumed);
        assert!(read_record(&reader, &reference, reference.offset).is_err());
        let mut unfinished = reference;
        unfinished.offset -= 1;
        assert!(read_record(&reader, &unfinished, 0).is_err());
        assert!(
            !format!(
                "{:?}",
                reader.read_range(&unfinished, None, 0, consumed).unwrap()
            )
            .contains("secret")
        );
    }
}
