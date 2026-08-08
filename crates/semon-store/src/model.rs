use std::{fmt, str::FromStr};

use serde_json::Value;

use crate::{StoreError, canonical};

/// A carrier-neutral semantic document.
///
/// This wrapper deliberately imposes no session, task, or decision shape.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticCore(Value);

impl SemanticCore {
    /// Parses a semantic core from JSON bytes.
    pub fn from_json_slice(json: &[u8]) -> Result<Self, StoreError> {
        Self::from_value(serde_json::from_slice(json)?)
    }

    /// Creates a semantic core from an already parsed JSON value.
    pub fn from_value(value: Value) -> Result<Self, StoreError> {
        canonical::canonicalize(&value)?;
        Ok(Self(value))
    }

    /// Returns the parsed semantic JSON value.
    pub fn value(&self) -> &Value {
        &self.0
    }

    /// Computes this semantic core's content identity.
    pub fn trace_id(&self) -> Result<TraceId, StoreError> {
        let canonical_json = self.canonical_json()?;
        Ok(canonical::content_hash(&canonical_json))
    }

    pub(crate) fn canonical_json(&self) -> Result<Vec<u8>, StoreError> {
        canonical::canonicalize(&self.0)
    }
}

/// A SHA-256 content identity rendered as 64 lowercase hexadecimal digits.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TraceId(String);

impl TraceId {
    pub(crate) fn from_digest(digest: String) -> Self {
        debug_assert!(is_valid_digest(&digest));
        Self(digest)
    }

    /// Returns the lowercase hexadecimal digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for TraceId {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if is_valid_digest(value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(StoreError::InvalidTraceId(value.to_owned()))
        }
    }
}

fn is_valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The semantic-only result of an ordinary lookup or list operation.
///
/// Raw records cannot be obtained from this type:
///
/// ```compile_fail
/// use semon_store::CanonicalTrace;
///
/// fn accidentally_read_raw(trace: &CanonicalTrace) {
///     let _ = trace.raw_bytes();
/// }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalTrace {
    pub(crate) id: TraceId,
    pub(crate) semantic_core: SemanticCore,
}

impl CanonicalTrace {
    /// Returns the trace's content identity.
    pub fn id(&self) -> &TraceId {
        &self.id
    }

    /// Returns the trace's carrier-neutral semantic core.
    pub fn semantic_core(&self) -> &SemanticCore {
        &self.semantic_core
    }
}

/// Opaque carrier bytes supplied with one capture.
#[derive(Clone, Copy, Debug)]
pub struct NewRawCarrierRecord<'a> {
    pub(crate) carrier: &'a str,
    pub(crate) bytes: &'a [u8],
}

impl<'a> NewRawCarrierRecord<'a> {
    /// Associates opaque bytes with the carrier that produced them.
    pub fn new(carrier: &'a str, bytes: &'a [u8]) -> Self {
        Self { carrier, bytes }
    }
}

/// The result of one atomic capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureResult {
    trace_id: TraceId,
    canonical_inserted: bool,
    raw_record_id: i64,
}

impl CaptureResult {
    pub(crate) fn new(trace_id: TraceId, canonical_inserted: bool, raw_record_id: i64) -> Self {
        Self {
            trace_id,
            canonical_inserted,
            raw_record_id,
        }
    }

    /// Returns the semantic content identity.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    /// Reports whether this capture created the canonical row.
    ///
    /// This is false for a repeat capture, while a new raw row is still added.
    pub fn canonical_inserted(&self) -> bool {
        self.canonical_inserted
    }

    /// Returns the newly appended forensic record's local row identifier.
    pub fn raw_record_id(&self) -> i64 {
        self.raw_record_id
    }
}

/// A forensic carrier record returned only by the explicit raw-read API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawCarrierRecord {
    id: i64,
    trace_id: TraceId,
    carrier: String,
    bytes: Vec<u8>,
}

impl RawCarrierRecord {
    pub(crate) fn new(id: i64, trace_id: TraceId, carrier: String, bytes: Vec<u8>) -> Self {
        Self {
            id,
            trace_id,
            carrier,
            bytes,
        }
    }

    /// Returns the local raw-record row identifier.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Returns the canonical trace identity this record archives.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    /// Returns the label of the producing carrier.
    pub fn carrier(&self) -> &str {
        &self.carrier
    }

    /// Returns the carrier bytes unchanged.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
