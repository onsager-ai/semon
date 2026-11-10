//! Stack-independent admitted SQLite input contract.
//!
//! The analytics store consumes this contract without knowing how native
//! sessions are indexed.  Implementors own their opaque checkpoints and must
//! return one coherent read snapshot: a change and the session evidence it
//! names can never come from different SQLite snapshots.

use std::collections::BTreeMap;

/// Reads one namespace of admitted activity evidence.
///
/// `checkpoint` is deliberately opaque to `semon-asi`.  An implementation
/// encodes its index incarnation and sequence there; when its incarnation
/// changes it returns a full namespace batch instead of interpreting a prior
/// sequence in the new index.
pub trait AnalyticsInput {
    type Error;

    fn read_admitted(&mut self, checkpoint: Option<&str>) -> Result<AdmittedBatch, Self::Error>;
}

/// A coherent incremental input read for one namespace.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedBatch {
    pub namespace: String,
    /// Implementation-defined incarnation and sequence checkpoint.
    pub checkpoint: String,
    /// Changed sessions, explicit removals, and availability observations in
    /// sequence order.  An admitted session with no steps is still `Admitted`.
    pub changes: Vec<SessionChange>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionChange {
    /// The whole current admitted generation for one session.
    Admitted(AdmittedSession),
    /// The index observed a change but has no currently admitted generation.
    /// Consumers retain the prior published facts.
    Pending { session_uid: String },
    /// The last admitted source cohort is unavailable.  This does not touch or
    /// remove the prior facts; it only records source availability.
    SourceMissing { session_uid: String },
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
    /// The exact historical price rows referenced by `steps`.  It is empty
    /// only when every step is unpriced; callers must not fill it from current
    /// rates.
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
    pub reasoning_tokens: u64,
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
