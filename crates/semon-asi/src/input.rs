//! Stack-independent admitted SQLite input contract.
//!
//! The analytics store consumes this contract without knowing how native
//! sessions are indexed.  Implementors own their opaque checkpoints and must
//! return bounded resumable chunks. Each chunk is a coherent read snapshot:
//! a change and its evidence never come from different SQLite snapshots.
//! Only a completed inventory may reconcile absent namespace identities.

use std::collections::BTreeMap;

/// Version of the pure ASI projection. It is independent of the native Event
/// Index parser/cache version.
pub const ANALYTICS_TRANSFORM_VERSION: u32 = 1;

/// Shared repository key when no remote or origin-owned local identity is known.
pub const UNKNOWN_REPO_KEY: &str = "unknown";

/// Encode one session-UID component using the RFC 3986 unreserved set.
///
/// Unreserved ASCII bytes (`A-Za-z0-9-._~`) remain literal; every other UTF-8
/// byte becomes uppercase `%XX`. Components are joined with literal `/`.
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Caller-selected work per chunk. Both limits must be positive.
///
/// `max_sessions` bounds inventory keys, or incremental journal entries
/// including repeated entries for the same identity. A session is never split: the first session
/// exceeding `max_steps` is returned whole and alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputBudget {
    pub max_sessions: usize,
    pub max_steps: usize,
}

impl Default for InputBudget {
    fn default() -> Self {
        Self {
            max_sessions: 64,
            max_steps: 8_192,
        }
    }
}

/// Reads one namespace of admitted activity evidence, one bounded chunk at a time.
///
/// The adapter owns strict validation of the opaque checkpoint, including its
/// incarnation, inventory snapshot and cursor. A changed incarnation restarts
/// inventory. Changes during inventory must be replayed before reporting
/// completion. Only the final inventory chunk supplies the complete UID set.
pub trait AnalyticsInput {
    type Error;

    fn read_admitted(
        &mut self,
        checkpoint: Option<&str>,
        budget: InputBudget,
    ) -> Result<AdmittedBatch, Self::Error>;
}

/// A coherent bounded input read for one namespace.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedBatch {
    pub namespace: String,
    /// Adapter-owned next cursor; publish atomically with the chunk's facts.
    pub checkpoint: String,
    /// True when inventory and replay have caught up to this read's snapshot.
    /// A caller may yield between incomplete chunks and resume from its manifest.
    pub complete: bool,
    pub reconciliation: Reconciliation,
    /// Changes in journal sequence order. An admitted session with no steps is
    /// still `Admitted`; no session is admitted twice in a chunk.
    pub changes: Vec<SessionChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reconciliation {
    Incremental,
    /// Only the final inventory chunk reconciles absence. This is the complete
    /// current namespace UID inventory, not just the changes in that chunk.
    ReplaceNamespace {
        session_uids: std::collections::BTreeSet<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionChange {
    /// The whole current admitted generation for one session.
    Admitted(Box<AdmittedSession>),
    /// The index observed a change but has no currently admitted generation.
    /// Consumers retain the prior published facts.
    Pending { session_uid: String },
    /// The last admitted source cohort is unavailable.  This does not touch or
    /// remove the prior facts; it only records source availability.
    SourceMissing {
        session_uid: String,
        /// True when SQLite retains a complete admitted generation at the
        /// origin. It does not assert this analytics store has ever published
        /// that session: consumers only mark existing live rows missing.
        retained_at_origin: bool,
    },
    /// The source cohort is complete again.  The accompanying admitted change,
    /// if any, owns replacement rows.
    SourceRestored { session_uid: String },
    /// An explicit index removal.  This is the only deletion instruction.
    Removed { session_uid: String },
}

/// All evidence needed to project one session without native reads, model
/// calls, classification, or price calculation.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedSession {
    pub session_uid: String,
    pub source_witness: SourceWitness,
    pub versions: RecordedVersions,
    pub session: Session,
    pub steps: Vec<Step>,
    pub compactions: Vec<Compaction>,
    /// Every projected step carried a complete StepFacts usage observation.
    pub usage_complete: bool,
    /// The exact historical price rows referenced by `steps`. It is empty only
    /// when every step is unpriced; callers must not fill it from current rates.
    pub prices: Vec<Price>,
}

/// Complete source cohort provenance.  Paths deliberately remain local only;
/// governed export receives a different, policy-qualified projection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceWitness {
    pub generation: String,
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub native_id: String,
    pub local_path: String,
    pub consumed_offset: u64,
    pub prefix_sha256: String,
}

/// Per-session recorded versions and conservative coverage.  These are facts
/// of the admitted generation, not writer-wide refresh targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedVersions {
    pub transform: u32,
    pub rules: u32,
    pub price: String,
    pub taxonomy: Option<u32>,
    pub coverage: Coverage,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub session_count: u64,
    pub step_count: u64,
    pub exclusive_step_count: u64,
    pub unknown_step_count: u64,
    pub unclassified_step_count: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub machine: String,
    pub harness: String,
    pub native_session_id: String,
    pub role: Role,
    pub parent_session_uid: Option<String>,
    pub parent_method: ParentMethod,
    pub lane_root_uid: Option<String>,
    pub repo: Repo,
    pub branch: Option<String>,
    pub status: Status,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub reported_usd: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Manager,
    Worker,
    Reviewer,
    Solo,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentMethod {
    Native,
    Workspace,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    Ended,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub key: String,
    pub repo_id: Option<String>,
    /// The path identity the key is bound to; identical for every session with
    /// the same key; None for remote and unknown.
    pub local_path: Option<String>,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub step_uid: String,
    pub turn_uid: String,
    pub model: Model,
    pub timestamp: Option<i64>,
    pub billing: BillingUsage,
    pub usage_owner: UsageOwner,
    pub context_tokens: u64,
    pub context_band: ContextBand,
    pub model_ms: Option<u64>,
    pub tool_ms: Option<u64>,
    pub cost: RecordedCost,
    pub activity: Option<Activity>,
    pub waste_kind: Option<WasteKind>,
    pub trigger: Option<Trigger>,
    pub ok: Option<bool>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub key: String,
    pub name: String,
    pub provider: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BillingUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub web_search_requests: u64,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
    pub prompt_size_tokens: u64,
    pub split_unknown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageOwner {
    Exclusive,
    Copied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextBand {
    Under50k,
    From50kTo150k,
    Over150k,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordedCost {
    pub usd_est: Option<f64>,
    pub price_version: String,
    /// Components are recorded independently by the source producer.  The
    /// schema currently exposes their total, but retaining them here prevents
    /// a later projection from recalculating or losing their provenance.
    pub components: BTreeMap<String, f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Coordinate,
    Wait,
    Integrate,
    Verify,
    Recover,
    Implement,
    Explore,
    Setup,
    Report,
    Think,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasteKind {
    Wait,
    FailedCall,
    Retry,
    RedundantRead,
    ContextRefill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    ToolError,
    EmptyResult,
    TestOrCiFailure,
    ExternalError,
    Compaction,
    Polling,
    WorkerResult,
    InstructionChange,
    MissingInput,
    NoneVisible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub ord: u64,
    pub tool: String,
    pub target_path: Option<String>,
    pub command_tag: Option<String>,
    pub ok: Option<bool>,
    pub exit_code: Option<i32>,
    pub tool_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compaction {
    pub compaction_uid: String,
    pub step_uid: Option<String>,
    pub timestamp: Option<i64>,
}

/// One immutable rate in the exact table used by an admitted generation.
#[derive(Clone, Debug, PartialEq)]
pub struct Price {
    pub price_version: String,
    pub provider: String,
    pub model: String,
    pub rate_kind: RateKind,
    pub tier: Option<String>,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
    pub threshold_tokens: Option<u64>,
    pub unit: PriceUnit,
    pub usd_per_unit: f64,
    pub valid_from: i64,
    pub valid_to: Option<i64>,
    pub learned_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateKind {
    Input,
    Output,
    CacheRead,
    CacheWrite5m,
    CacheWrite1h,
    Reasoning,
    WebSearchRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceUnit {
    Mtok,
    Request,
}

#[cfg(test)]
mod tests {
    use super::encode_component;

    #[test]
    fn uid_component_encoding_preserves_historical_adapter_output() {
        for (input, expected) in [
            ("", ""),
            ("/", "%2F"),
            ("%", "%25"),
            (" ", "%20"),
            ("~", "~"),
            ("Aaz09-._~", "Aaz09-._~"),
            ("a/b", "a%2Fb"),
            ("a%2Fb", "a%252Fb"),
            ("机器😀", "%E6%9C%BA%E5%99%A8%F0%9F%98%80"),
        ] {
            assert_eq!(encode_component(input), expected, "{input:?}");
        }
    }
}
