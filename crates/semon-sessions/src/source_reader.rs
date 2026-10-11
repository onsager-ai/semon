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

/// Request-local I/O limits also bound providers that return tiny fragments.
/// The wrapper carries neither authorization nor a retained byte cache.
pub(crate) struct BudgetedReader<'a> {
    reader: &'a dyn SessionSourceReader,
    requests: std::sync::atomic::AtomicUsize,
}
impl<'a> BudgetedReader<'a> {
    pub(crate) fn remaining(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub(crate) fn new(reader: &'a dyn SessionSourceReader, requests: usize) -> Self {
        Self {
            reader,
            requests: std::sync::atomic::AtomicUsize::new(requests),
        }
    }
}
impl SessionSourceReader for BudgetedReader<'_> {
    fn read_range(
        &self,
        source: &SessionSourceRef,
        expected_generation: Option<&str>,
        offset: u64,
        max_bytes: usize,
    ) -> io::Result<SessionSourceRange> {
        use std::sync::atomic::Ordering;
        let mut left = self.requests.load(Ordering::Relaxed);
        loop {
            let next = left.checked_sub(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Native request read budget exhausted",
                )
            })?;
            match self.requests.compare_exchange_weak(
                left,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => left = current,
            }
        }
        self.reader
            .read_range(source, expected_generation, offset, max_bytes)
    }
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

/// Read and verify an exact indexed byte proof from one selected source.
pub(crate) fn read_evidence_bounded(
    reader: &dyn SessionSourceReader,
    source: &SessionSourceRef,
    evidence: &crate::native_record::Record,
    bytes_left: &mut usize,
) -> io::Result<Vec<u8>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "native evidence range changed");
    let end = evidence
        .end()
        .filter(|end| *end <= source.offset)
        .ok_or_else(invalid)?;
    let length = usize::try_from(end - evidence.offset).map_err(|_| invalid())?;
    if length > *bytes_left || length > 32 * 1024 * 1024 {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(length);
    let mut generation = None::<String>;
    let mut source_length = None;
    while bytes.len() < length {
        let at = evidence.offset + bytes.len() as u64;
        let max = SESSION_SOURCE_CHUNK_MAX.min(length - bytes.len());
        let part = reader.read_range(source, generation.as_deref(), at, max)?;
        if part.offset != at
            || part.bytes.is_empty()
            || part.bytes.len() > max
            || part.length < source.offset
            || part.generation.is_empty()
            || part.generation.len() > 4096
            || generation
                .as_ref()
                .is_some_and(|prior| prior != &part.generation)
            || source_length.is_some_and(|prior| prior != part.length)
        {
            return Err(invalid());
        }
        *bytes_left -= part.bytes.len();
        generation = Some(part.generation);
        source_length = Some(part.length);
        bytes.extend_from_slice(&part.bytes);
    }
    evidence.verify_bytes(&bytes)?;
    Ok(bytes)
}

/// Read one indexed native record through the same bounded range contract as
/// focused transcript reads. This never discovers sources or decodes an index.
pub(crate) fn read_native_record(
    reader: &dyn SessionSourceReader,
    source: &SessionSourceRef,
    expected_generation: &str,
    offset: u64,
) -> io::Result<(Option<serde_json::Value>, u64)> {
    let mut bytes_left = usize::MAX;
    read_bounded(
        reader,
        source,
        Some(expected_generation),
        offset,
        crate::tx::LINE_MAX as usize,
        &mut bytes_left,
    )
}

pub(crate) fn read_record_bounded(
    reader: &dyn SessionSourceReader,
    source: &SessionSourceRef,
    offset: u64,
    record_max: usize,
    bytes_left: &mut usize,
) -> io::Result<serde_json::Value> {
    read_bounded(reader, source, None, offset, record_max, bytes_left)?
        .0
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "native record exceeds its bound or is malformed",
            )
        })
}

fn read_bounded(
    reader: &dyn SessionSourceReader,
    source: &SessionSourceRef,
    expected: Option<&str>,
    offset: u64,
    record_max: usize,
    bytes_left: &mut usize,
) -> io::Result<(Option<serde_json::Value>, u64)> {
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
    let mut generation = expected.map(str::to_owned);
    let mut length = None;
    let mut at = offset;
    loop {
        let remaining = (record_max as u64 + 1).saturating_sub(bytes.len() as u64);
        let max = (8 * 1024)
            .min(usize::try_from(source.offset - at).unwrap_or(usize::MAX))
            .min(usize::try_from(remaining).unwrap_or(usize::MAX))
            .min(*bytes_left);
        if max == 0 {
            return Err(invalid());
        }
        let part = reader.read_range(source, generation.as_deref(), at, max)?;
        *bytes_left = bytes_left.saturating_sub(part.bytes.len());
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
        generation.get_or_insert(part.generation);
        length.get_or_insert(part.length);
        let end = part
            .bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|end| end + 1);
        let take = end.unwrap_or(part.bytes.len());
        bytes.extend_from_slice(&part.bytes[..take]);
        at += take as u64;
        if bytes.len() > record_max {
            return Ok((None, bytes.len() as u64));
        }
        if end.is_some() {
            return Ok((crate::tx::parse_native_record(&bytes), bytes.len() as u64));
        }
        if at == source.offset {
            return Err(invalid());
        }
    }
}

#[cfg(test)]
fn read_record(
    reader: &dyn SessionSourceReader,
    source: &SessionSourceRef,
    offset: u64,
) -> io::Result<serde_json::Value> {
    read_native_record(reader, source, "verified", offset)?
        .0
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
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
        }
    }
    #[test]
    fn request_budget_never_wraps_or_reaches_the_provider_after_exhaustion() {
        let reader = Reader {
            bytes: Arc::from(b"{}\n".as_slice()),
            change_at: None,
            calls: Mutex::default(),
        };
        let bounded = BudgetedReader::new(&reader, 2);
        let source = source(3);
        for offset in 0..2 {
            bounded.read_range(&source, None, offset, 1).unwrap();
        }
        for _ in 0..10 {
            assert_eq!(
                bounded.read_range(&source, None, 2, 1).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
        assert_eq!(reader.calls.lock().unwrap().len(), 2);
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
        assert!(calls.len() > 1);
        assert_eq!(calls[0].0.as_deref(), Some("verified"));
        assert_eq!(calls[1].0.as_deref(), Some("verified"));
        assert_eq!(calls[2].0.as_deref(), Some("verified"));
        assert!(calls.iter().all(
            |call| call.0.as_deref() == Some("verified") && call.2 <= SESSION_SOURCE_CHUNK_MAX
        ));
        let last = calls.last().unwrap();
        assert_eq!(last.1 + last.2 as u64, reference.offset);
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
    #[test]
    fn native_preview_enforces_record_and_total_io_budgets() {
        let bytes: Arc<[u8]> = (serde_json::json!({"text":"x".repeat(10_000)}).to_string() + "\n")
            .into_bytes()
            .into();
        let reference = source(bytes.len());
        let reader = Reader {
            bytes,
            change_at: None,
            calls: Mutex::default(),
        };
        let mut budget = 9000;
        assert!(read_record_bounded(&reader, &reference, 0, 128 * 1024, &mut budget).is_err());
        assert_eq!(budget, 0);
        assert_eq!(
            reader
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.2)
                .sum::<usize>(),
            9000
        );
        let mut budget = 100_000;
        assert!(read_record_bounded(&reader, &reference, 0, 8192, &mut budget).is_err());
        assert_eq!(budget, 100_000 - 8193);
    }
    #[test]
    fn indexed_evidence_reads_only_selected_bytes_and_refuses_changed_proofs() {
        let raw = vec![b'x'; SESSION_SOURCE_CHUNK_MAX + 17];
        let proof = crate::native_record::Record::capture(&raw, 7);
        let bytes: Arc<[u8]> = [b"prefix!".as_slice(), raw.as_slice(), b"unrelated history"]
            .concat()
            .into();
        let reader = Reader {
            bytes: bytes.clone(),
            change_at: None,
            calls: Mutex::default(),
        };
        let source = source(bytes.len());
        let mut budget = raw.len();
        assert_eq!(
            read_evidence_bounded(&reader, &source, &proof, &mut budget).unwrap(),
            raw
        );
        assert_eq!(budget, 0);
        let calls = reader.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (None, 7, SESSION_SOURCE_CHUNK_MAX));
        assert_eq!(
            calls[1],
            (
                Some("verified".into()),
                7 + SESSION_SOURCE_CHUNK_MAX as u64,
                17
            )
        );
        drop(calls);
        let changed = Reader {
            bytes: bytes.clone(),
            change_at: Some(1),
            calls: Mutex::default(),
        };
        assert!(read_evidence_bounded(&changed, &source, &proof, &mut raw.len()).is_err());
        let mut rewritten = bytes.to_vec();
        rewritten[7] = b'y';
        let corrupt = Reader {
            bytes: rewritten.into(),
            change_at: None,
            calls: Mutex::default(),
        };
        assert!(read_evidence_bounded(&corrupt, &source, &proof, &mut raw.len()).is_err());
        let bounded = Reader {
            bytes,
            change_at: None,
            calls: Mutex::default(),
        };
        assert!(read_evidence_bounded(&bounded, &source, &proof, &mut (raw.len() - 1)).is_err());
        assert!(bounded.calls.lock().unwrap().is_empty());
    }
}
