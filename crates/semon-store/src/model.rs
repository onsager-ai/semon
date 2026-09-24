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
    /// This is false for a repeat capture.
    pub fn canonical_inserted(&self) -> bool {
        self.canonical_inserted
    }

    /// Returns the forensic record's local row identifier, reused on replay.
    pub fn raw_record_id(&self) -> i64 {
        self.raw_record_id
    }
}

/// The basis recorded for one occurrence's `repo` attribution.
///
/// An inferred attribution and an overridden one are different kinds of fact,
/// and a failed inference is a third: this makes all three — plus the case
/// where nothing was ever attributed — first-class and countable rather than
/// folding them into "we have a repo string" / "we don't".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoSource {
    /// Inferred from the carrier's own `git.repository_url` (or equivalent).
    GitRemote,
    /// Inferred from the basename of the working directory.
    CwdBasename,
    /// Supplied explicitly, e.g. via `--repo` or `SEMON_REPO`.
    ExplicitOverride,
    /// No repository could be attributed.
    None,
}

impl RepoSource {
    /// Returns the stored/wire spelling of this basis.
    pub fn as_str(self) -> &'static str {
        match self {
            RepoSource::GitRemote => "git-remote",
            RepoSource::CwdBasename => "cwd-basename",
            RepoSource::ExplicitOverride => "explicit-override",
            RepoSource::None => "none",
        }
    }
}

impl fmt::Display for RepoSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RepoSource {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "git-remote" => Ok(RepoSource::GitRemote),
            "cwd-basename" => Ok(RepoSource::CwdBasename),
            "explicit-override" => Ok(RepoSource::ExplicitOverride),
            "none" => Ok(RepoSource::None),
            other => Err(StoreError::InvalidRepoSource(other.to_owned())),
        }
    }
}

/// Who or what produced the content behind one occurrence.
///
/// This is a stored classification, not a filter: an unrecognized shape
/// becomes a visible [`AuthoredBy::Unknown`] rather than silently defaulting
/// to [`AuthoredBy::Human`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoredBy {
    /// Authored directly by the human operator.
    Human,
    /// Authored by the agent (assistant turns, tool actions it took).
    Agent,
    /// Harness-injected framing, never the human's or the agent's words.
    Harness,
    /// A shape the adapter does not yet classify.
    Unknown,
}

impl AuthoredBy {
    /// Returns the stored/wire spelling of this classification.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthoredBy::Human => "human",
            AuthoredBy::Agent => "agent",
            AuthoredBy::Harness => "harness",
            AuthoredBy::Unknown => "unknown",
        }
    }
}

impl fmt::Display for AuthoredBy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AuthoredBy {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "human" => Ok(AuthoredBy::Human),
            "agent" => Ok(AuthoredBy::Agent),
            "harness" => Ok(AuthoredBy::Harness),
            "unknown" => Ok(AuthoredBy::Unknown),
            other => Err(StoreError::InvalidAuthoredBy(other.to_owned())),
        }
    }
}

/// Occurrence facts supplied alongside one capture.
///
/// `carrier` is not repeated here: it is taken from the accompanying
/// [`NewRawCarrierRecord`], so one capture call cannot disagree with itself
/// about which carrier produced it.
///
/// `sequence` must be deterministically derived from the source record's own
/// position (never a write-time or autoincrement counter): that determinism
/// is what makes the `UNIQUE (carrier, session, sequence)` upsert idempotent
/// under re-capture. `parent_sequence` must likewise be derived — the
/// sequence of the previous *projected* record in the same session, never a
/// locally tracked "last row written" value.
#[derive(Clone, Copy, Debug)]
pub struct NewOccurrence<'a> {
    /// The carrier's session identifier.
    pub session: &'a str,
    /// The zero-based ordinal of the source record within its session, as
    /// derived from the source (see the struct-level note on determinism).
    pub sequence: i64,
    /// Capture timestamp, nanoseconds since the Unix epoch.
    pub timestamp: i64,
    /// The attributed repository name, or empty when none was attributed.
    pub repo: &'a str,
    /// The basis for `repo`.
    pub repo_source: RepoSource,
    /// The previous projected occurrence's `sequence` in the same session,
    /// or `None` for the first occurrence in that session.
    pub parent_sequence: Option<i64>,
    /// The subagent identifier, when the carrier has one. Always `None` for
    /// carriers with no subagent concept (e.g. Codex).
    pub agent: Option<&'a str>,
    /// Who or what authored the content behind this occurrence.
    pub authored_by: AuthoredBy,
}

/// One row of the occurrence log: an occurrence joined to its trace.
///
/// Produced only by [`crate::TraceStore::log`], which reads `occurrences`
/// joined to `canonical_traces` and never reaches `raw_carrier_records`.
#[derive(Clone, Debug, PartialEq)]
pub struct OccurrenceRecord {
    pub(crate) trace_id: TraceId,
    pub(crate) carrier: String,
    pub(crate) session: String,
    pub(crate) sequence: i64,
    pub(crate) timestamp: i64,
    pub(crate) repo: String,
    pub(crate) repo_source: RepoSource,
    pub(crate) parent_sequence: Option<i64>,
    pub(crate) agent: Option<String>,
    pub(crate) authored_by: AuthoredBy,
    pub(crate) semantic_core: SemanticCore,
}

impl OccurrenceRecord {
    /// Returns the identity of the trace this occurrence is of.
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    /// Returns the producing carrier's label.
    pub fn carrier(&self) -> &str {
        &self.carrier
    }

    /// Returns the carrier's session identifier.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Returns this occurrence's sequence within `(carrier, session)`.
    pub fn sequence(&self) -> i64 {
        self.sequence
    }

    /// Returns the capture timestamp, nanoseconds since the Unix epoch.
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    /// Returns the attributed repository name, or empty when none.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Returns the basis for [`OccurrenceRecord::repo`].
    pub fn repo_source(&self) -> RepoSource {
        self.repo_source
    }

    /// Returns the previous projected occurrence's sequence in this session.
    pub fn parent_sequence(&self) -> Option<i64> {
        self.parent_sequence
    }

    /// Returns the subagent identifier, when the carrier has one.
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// Returns who or what authored the content behind this occurrence.
    pub fn authored_by(&self) -> AuthoredBy {
        self.authored_by
    }

    /// Returns the trace's carrier-neutral semantic core.
    pub fn semantic_core(&self) -> &SemanticCore {
        &self.semantic_core
    }
}

/// Filters accepted by [`crate::TraceStore::log`].
///
/// All fields are optional; an unset filter matches everything.
#[derive(Clone, Debug, Default)]
pub struct LogFilter {
    /// Restrict to occurrences attributed to this repository.
    pub repo: Option<String>,
    /// Restrict to occurrences with `timestamp` in `[start, end)`,
    /// nanoseconds since the Unix epoch.
    pub timestamp_range: Option<(i64, i64)>,
    /// Maximum number of rows to return.
    pub limit: Option<u32>,
}

/// A forensic carrier record returned only by the explicit raw-read API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawCarrierRecord {
    id: i64,
    trace_ids: Vec<TraceId>,
    carrier: String,
    bytes: Vec<u8>,
}

impl RawCarrierRecord {
    pub(crate) fn new(id: i64, trace_ids: Vec<TraceId>, carrier: String, bytes: Vec<u8>) -> Self {
        Self {
            id,
            trace_ids,
            carrier,
            bytes,
        }
    }

    /// Returns the local raw-record row identifier.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Returns the sorted trace identities projected by this source line.
    /// An unprojected line has no trace ids.
    pub fn trace_ids(&self) -> &[TraceId] {
        &self.trace_ids
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
