//! Pure session activity derivation over already qualified native facts.
//!
//! This module does not read sources or publish a projection. The producer must
//! select one canonical session's complete source cohort, merge Claude usage in
//! the existing model order, resolve exact copied-record ownership, and supply
//! only receipt-qualified Codex facts and normalized signals. Source positions
//! break timestamp ties; they are provenance, never authorization.

use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use super::CodexNativeUsage;
use super::{BillingUsage, CodexUsageEvent, MessageUsage, ModelTokens, Signal, SignalKind};
use crate::{claude_usage::Shared, pricing};

pub(crate) mod analytics;
pub(crate) mod classify;

pub(crate) const ACTIVITY_RULES_VERSION: u32 = 11;
const HOUR_MS: u64 = 3_600_000;

/// Claude IDs are scoped by `SessionActivity::session_key`. Codex identity is
/// the complete native (thread, session, turn, response) tuple, not response alone.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum ActivityStepId {
    Claude(String),
    Codex([String; 4]),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct SourcePosition {
    pub(crate) source: String,
    pub(crate) offset: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct TimedInput {
    pub(crate) position: SourcePosition,
    pub(crate) timestamp: Option<i64>,
}

/// Native tool evidence without an activity classifier observation.
#[derive(Clone, Debug)]
pub(crate) struct UnclassifiedTool {
    pub(crate) position: SourcePosition,
    pub(crate) timestamp: Option<i64>,
    pub(crate) result_timestamp: Option<i64>,
    pub(crate) result_offset: Option<u64>,
    pub(crate) tool: Option<String>,
    pub(crate) ok: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum ContextBand {
    Under50k,
    From50kTo150k,
    Over150k,
}

impl ContextBand {
    fn for_tokens(tokens: u64) -> Self {
        match tokens {
            0..50_000 => Self::Under50k,
            50_000..=150_000 => Self::From50kTo150k,
            _ => Self::Over150k,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Ownership {
    Exclusive,
    /// The exact key returned by `claude_usage::shared`; no original is inferred.
    Shared(String),
}

/// Stable ownership vocabulary for downstream billing facts. Shared Claude
/// usage is copied evidence, never another billable invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum UsageOwner {
    Exclusive,
    Copied,
}

/// The classifier's bounded, closed command vocabulary. This is deliberately
/// separate from the activity category: a tool call can be a known `Read` even
/// when its enclosing model step has several kinds of activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CommandTag {
    Read,
    Search,
    Setup,
    Stage,
    BodyWrite,
    Inspect,
    Commit,
    Push,
    PullRequest,
    PrChecks,
    Watch,
    Sleep,
    Wait,
    CiStatus,
    CiStart,
    Report,
    Verify,
    Merge,
    Mutation,
    Ambiguous,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolCallFact {
    pub(crate) ord: u64,
    pub(crate) tool: String,
    /// A raw local path for a native file tool only. It never enters the
    /// activity API, mirrors, or source-independent event observation.
    pub(crate) target_path: Option<String>,
    pub(crate) command_tag: CommandTag,
    pub(crate) ok: Option<bool>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) tool_ms: Option<u64>,
}

/// Inputs for one analytics `fact_step` row. The surrounding saved step is
/// keyed by its `ActivityStepId`; this payload intentionally carries no body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StepFacts {
    pub(crate) turn_id: Option<String>,
    pub(crate) record_ordinal: Option<u64>,
    pub(crate) usage_owner: UsageOwner,
    pub(crate) billing: BillingUsage,
    pub(crate) model_ms: Option<u64>,
    pub(crate) tool_ms: Option<u64>,
    pub(crate) tool_calls: Vec<ToolCallFact>,
}

/// A normalized usage row, before session ordering and pricing. Keep these
/// constructors at the boundary so legacy cumulative Codex counters cannot be
/// mistaken for native response facts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct UsageStep {
    pub(crate) id: ActivityStepId,
    pub(crate) position: SourcePosition,
    /// Exact native turn identity when the source records one.
    pub(crate) turn_id: Option<String>,
    pub(crate) usage: Usage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Usage {
    Claude(MessageUsage),
    Codex(CodexUsageEvent),
}

impl UsageStep {
    pub(crate) fn claude(
        message_id: String,
        position: SourcePosition,
        usage: MessageUsage,
    ) -> Self {
        Self {
            id: ActivityStepId::Claude(message_id),
            position,
            turn_id: None,
            usage: Usage::Claude(usage),
        }
    }

    pub(crate) fn normalized_codex(
        identity: [String; 4],
        position: SourcePosition,
        event: CodexUsageEvent,
    ) -> Self {
        Self {
            turn_id: Some(identity[2].clone()),
            id: ActivityStepId::Codex(identity),
            position,
            usage: Usage::Codex(event),
        }
    }

    /// `model_outcome` comes from the existing native fact table, after source
    /// receipt qualification. This filter does not establish that qualification.
    #[cfg(test)]
    pub(crate) fn codex(
        payload: &serde_json::Value,
        model_outcome: &str,
        position: SourcePosition,
        model: String,
        timestamp: Option<i64>,
    ) -> Option<Self> {
        if model_outcome != "accepted" {
            return None;
        }
        let (identity, _, tokens) = CodexNativeUsage::decode(payload)?;
        let identity: [String; 4] =
            serde_json::from_str(&identity).expect("native decoder emits a complete string tuple");
        Some(Self {
            turn_id: Some(identity[2].clone()),
            id: ActivityStepId::Codex(identity),
            position,
            usage: Usage::Codex(CodexUsageEvent {
                model,
                tokens,
                timestamp,
            }),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ActivityStep {
    pub(crate) id: ActivityStepId,
    pub(crate) position: SourcePosition,
    pub(crate) call_position: Option<SourcePosition>,
    pub(crate) ownership: Ownership,
    pub(crate) model: Option<String>,
    pub(crate) tokens: ModelTokens,
    pub(crate) context_tokens: u64,
    pub(crate) band: ContextBand,
    /// Sum of the four token components below plus any web search, known only
    /// when every priced category is.
    pub(crate) list_cost_usd: Option<f64>,
    /// Each component is priced independently, so one can stay known when
    /// another rate is missing. No tokens is a known zero. The cache-read
    /// component is the carry cost of the step's context.
    pub(crate) fresh_input_usd: Option<f64>,
    pub(crate) cache_read_usd: Option<f64>,
    pub(crate) cache_write_usd: Option<f64>,
    pub(crate) output_usd: Option<f64>,
    pub(crate) timestamp: Option<i64>,
    pub(crate) elapsed_ms: Option<u64>,
    pub(crate) compactions: u64,
    pub(crate) waste: Option<classify::WasteKind>,
    pub(crate) waste_category: Option<classify::WasteCategory>,
    pub(crate) waits: Vec<classify::WaitInterval>,
    pub(crate) empty_search: bool,
    pub(crate) verify_failure: bool,
    pub(crate) silent_exit: bool,
    /// None when no call proves an activity: unknown or ambiguous commands only,
    /// or no tool call at all. Report and think are not derived, because the
    /// step facts carry no assistant-text evidence.
    pub(crate) activity: Option<classify::Activity>,
    /// Set only for steps with waste or a verify failure.
    pub(crate) trigger: Option<Trigger>,
}

/// What immediately preceded a wasteful or failing step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Trigger {
    ToolError,
    EmptyResult,
    TestOrCiFailure,
    ExternalError,
    Compaction,
    Polling,
    WorkerResult,
    NoneVisible,
}

impl Trigger {
    /// Refill follows a compaction. Otherwise the nearest preceding step of the
    /// same native source (one agent's transcript) supplies the evidence, in order.
    fn of(steps: &[ActivityStep], index: usize) -> Option<Self> {
        use classify::{Activity, WasteKind};
        let step = &steps[index];
        if step.waste.is_none() && !step.verify_failure {
            return None;
        }
        if step.waste == Some(WasteKind::Refill) {
            return Some(Self::Compaction);
        }
        let Some(previous) = steps[..index]
            .iter()
            .rev()
            .find(|previous| previous.position.source == step.position.source)
        else {
            return Some(Self::NoneVisible);
        };
        let waste = previous.waste;
        let external = waste == Some(WasteKind::ExternalFailure);
        let failed = waste.is_some_and(WasteKind::is_failure) && !external;
        let trigger = if failed {
            Self::ToolError
        } else if previous.empty_search {
            Self::EmptyResult
        } else if previous.verify_failure {
            Self::TestOrCiFailure
        } else if external {
            Self::ExternalError
        } else if previous.activity == Some(Activity::Wait)
            || matches!(waste, Some(WasteKind::ChecksPending | WasteKind::Sleep))
        {
            Self::Polling
        } else if previous.activity == Some(Activity::Coordinate)
            || waste == Some(WasteKind::OrchestrationWait)
        {
            Self::WorkerResult
        } else {
            Self::NoneVisible
        };
        Some(trigger)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Compaction {
    pub(crate) position: SourcePosition,
    pub(crate) native_record: Option<super::NativeRecordIdentity>,
    pub(crate) timestamp: Option<i64>,
    pub(crate) step_id: Option<ActivityStepId>,
}

/// A partial sum preserves both the measured amount and its missing coverage.
/// In particular the final step does not invent a zero duration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measured<T> {
    pub(crate) known: T,
    pub(crate) unknown_steps: u64,
}

impl<T: std::ops::AddAssign> Measured<T> {
    fn add(&mut self, value: Option<T>) {
        if let Some(value) = value {
            self.known += value;
        } else {
            self.unknown_steps += 1;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct StepTotals {
    pub(crate) steps: u64,
    pub(crate) list_cost_usd: Measured<f64>,
    pub(crate) elapsed_ms: Measured<u64>,
}

impl StepTotals {
    fn add(&mut self, step: &ActivityStep) {
        self.steps += 1;
        self.list_cost_usd.add(step.list_cost_usd);
        self.elapsed_ms.add(step.elapsed_ms);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct OwnershipTotals {
    pub(crate) observed: StepTotals,
    pub(crate) exclusive: StepTotals,
    pub(crate) shared: StepTotals,
}

impl OwnershipTotals {
    fn add(&mut self, step: &ActivityStep) {
        self.observed.add(step);
        match &step.ownership {
            Ownership::Exclusive => self.exclusive.add(step),
            Ownership::Shared(_) => self.shared.add(step),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionActivityAggregate {
    pub(crate) totals: OwnershipTotals,
    pub(crate) by_band: BTreeMap<ContextBand, OwnershipTotals>,
    pub(crate) compactions: u64,
    pub(crate) thrash: bool,
    pub(crate) thrash_windows: Vec<[ActivityStepId; 3]>,
    /// Shared observations leave original billing ownership unresolved, even
    /// when all exclusively attributed steps can be priced.
    pub(crate) exclusive_cost_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionActivity {
    pub(crate) session_key: String,
    pub(crate) rules_version: u32,
    pub(crate) pricing_fingerprint: String,
    pub(crate) steps: Vec<ActivityStep>,
    pub(crate) compactions: Vec<Compaction>,
    pub(crate) aggregate: SessionActivityAggregate,
    pub(crate) facts: Vec<classify::ChangeFact>,
}

/// Tool observations can join a step only through exact native identity or a
/// same-source following usage observation (or the trailing prior one). Facts retain
/// only exact identities, never the interval association used for step waste.
pub(crate) fn apply_observations(
    activity: &mut SessionActivity,
    mut observations: Vec<classify::LocatedObservation>,
) -> Vec<classify::LocatedObservation> {
    let compactions: Vec<_> = activity
        .compactions
        .iter()
        .map(|compact| (compact.position.source.clone(), compact.position.offset))
        .collect();
    classify::classify_reads(&mut observations, &compactions);
    classify::classify_retries(&mut observations);
    let mut exact = BTreeMap::new();
    let mut sources: BTreeMap<String, Vec<(u64, usize)>> = BTreeMap::new();
    for (index, step) in activity.steps.iter().enumerate() {
        exact.insert(step.id.clone(), index);
        sources
            .entry(step.position.source.clone())
            .or_default()
            .push((step.position.offset, index));
    }
    for offsets in sources.values_mut() {
        offsets.sort_unstable();
    }
    for row in &mut observations {
        if row
            .observation
            .step_id
            .as_ref()
            .is_some_and(|id| !exact.contains_key(id))
        {
            row.observation.step_id = None;
        }
        activity.facts.extend(
            row.observation
                .changes
                .iter()
                .cloned()
                .map(|change| classify::fact(row, change)),
        );
        let owner = observation_owner(&exact, &sources, row);
        if let Some(owner) = owner {
            let step = &mut activity.steps[owner];
            if matches!(step.id, ActivityStepId::Codex(_))
                && row.observation.step_id.as_ref() == Some(&step.id)
                && step
                    .call_position
                    .as_ref()
                    .is_none_or(|old| row.position < *old)
            {
                step.call_position = Some(row.position.clone());
            }
            step.empty_search |= row.observation.empty_search;
            step.verify_failure |= row.observation.verify_failure;
            step.silent_exit |= row.observation.silent_exit;
            step.activity = step
                .activity
                .into_iter()
                .chain(row.observation.activity())
                .min();
            if let Some(wait) = &row.observation.wait {
                step.waits.push(wait.clone());
            }
            if let Some(waste) = row.observation.waste {
                let rank = |kind| match kind {
                    classify::WasteKind::OrchestrationWait | classify::WasteKind::ChecksPending => {
                        0
                    }
                    classify::WasteKind::PathNotFound
                    | classify::WasteKind::MalformedCommand
                    | classify::WasteKind::ToolInputRejected
                    | classify::WasteKind::Timeout
                    | classify::WasteKind::ExternalFailure
                    | classify::WasteKind::OtherFailure
                    | classify::WasteKind::MergeConflict
                    | classify::WasteKind::Retry
                    | classify::WasteKind::FlakyRerun
                    | classify::WasteKind::Sleep => 1,
                    classify::WasteKind::Refill => 2,
                    classify::WasteKind::RedundantRead => 3,
                };
                if step.waste.is_none_or(|old| rank(waste) < rank(old)) {
                    step.waste = Some(waste);
                    step.waste_category = Some(waste.category());
                }
            }
        }
    }
    let triggers: Vec<_> = (0..activity.steps.len())
        .map(|index| Trigger::of(&activity.steps, index))
        .collect();
    for (step, trigger) in activity.steps.iter_mut().zip(triggers) {
        step.trigger = trigger;
    }
    activity
        .facts
        .sort_by(|a, b| (&a.position, &a.identity).cmp(&(&b.position, &b.identity)));
    activity.facts.dedup_by(|a, b| a.identity == b.identity);
    observations
}

pub(crate) fn observation_owner(
    exact: &BTreeMap<ActivityStepId, usize>,
    sources: &BTreeMap<String, Vec<(u64, usize)>>,
    row: &classify::LocatedObservation,
) -> Option<usize> {
    position_owner(
        exact,
        sources,
        &row.position,
        row.observation.step_id.as_ref(),
    )
}

pub(crate) fn position_owner(
    exact: &BTreeMap<ActivityStepId, usize>,
    sources: &BTreeMap<String, Vec<(u64, usize)>>,
    position: &SourcePosition,
    step_id: Option<&ActivityStepId>,
) -> Option<usize> {
    if let Some(id) = step_id {
        exact.get(id).copied()
    } else {
        sources.get(&position.source).and_then(|offsets| {
            let next = offsets.partition_point(|(offset, _)| *offset < position.offset);
            // Response usage follows its calls. This association never becomes
            // an exact response identity on a change fact.
            offsets
                .get(next)
                .or_else(|| offsets.last())
                .map(|(_, index)| *index)
        })
    }
}

/// Use the model's existing Claude merge order: the last normalized row for a
/// message ID wins. Only accepted Codex facts may enter; repeated accepted
/// identities (e.g. selected copies) retain the first observation. Duplicate or
/// conflicting fact rows are filtered by `UsageStep::codex`, never re-totaled.
pub(crate) fn derive(
    session_key: String,
    usage: impl IntoIterator<Item = UsageStep>,
    shared: &Shared,
    signals: impl IntoIterator<Item = (String, Signal)>,
    table: Arc<pricing::EffectiveTable>,
) -> SessionActivity {
    let mut rows = BTreeMap::new();
    for row in usage {
        match &row.id {
            ActivityStepId::Claude(_) => {
                rows.insert(row.id.clone(), row);
            }
            ActivityStepId::Codex(_) => {
                rows.entry(row.id.clone()).or_insert(row);
            }
        }
    }
    let mut steps: Vec<_> = rows
        .into_values()
        .map(|row| {
            let mut cost = pricing::CostBuild::with_table(table.clone());
            let (model, tokens, timestamp) = match &row.usage {
                Usage::Claude(usage) => {
                    cost.message(usage);
                    (
                        usage.model.clone(),
                        usage.model_tokens.clone(),
                        usage.billing.timestamp,
                    )
                }
                Usage::Codex(event) => {
                    cost.codex(event);
                    (
                        Some(event.model.clone()),
                        event.tokens.clone(),
                        event.timestamp,
                    )
                }
            };
            let cost = cost.finish_step();
            let context_tokens = tokens
                .input
                .saturating_add(tokens.cache_read)
                .saturating_add(tokens.cache_write);
            let ownership = match &row.id {
                ActivityStepId::Claude(id) => shared
                    .get(id)
                    .map_or(Ownership::Exclusive, |key| Ownership::Shared(key.clone())),
                ActivityStepId::Codex(_) => Ownership::Exclusive,
            };
            ActivityStep {
                id: row.id,
                position: row.position,
                call_position: None,
                ownership,
                model,
                tokens,
                context_tokens,
                band: ContextBand::for_tokens(context_tokens),
                list_cost_usd: cost.list,
                fresh_input_usd: cost.fresh_input,
                cache_read_usd: cost.cache_read,
                cache_write_usd: cost.cache_write,
                output_usd: cost.output,
                timestamp,
                elapsed_ms: None,
                compactions: 0,
                waste: None,
                waste_category: None,
                waits: Vec::new(),
                empty_search: false,
                verify_failure: false,
                silent_exit: false,
                activity: None,
                trigger: None,
            }
        })
        .collect();
    // Unknown times sort after known times, by exact source position. They do
    // not acquire a synthetic timestamp or a duration across the missing value.
    steps.sort_by(|a, b| {
        (a.timestamp.is_none(), a.timestamp, &a.position, &a.id).cmp(&(
            b.timestamp.is_none(),
            b.timestamp,
            &b.position,
            &b.id,
        ))
    });
    for index in 0..steps.len().saturating_sub(1) {
        if let (Some(at), Some(next)) = (steps[index].timestamp, steps[index + 1].timestamp) {
            steps[index].elapsed_ms = Some((next.saturating_sub(at).max(0) as u64).min(HOUR_MS));
        }
    }
    let mut compactions: Vec<_> = signals
        .into_iter()
        .filter(|(_, signal)| signal.k == SignalKind::Compact)
        .map(|(source, signal)| Compaction {
            native_record: signal.native_record,
            position: SourcePosition {
                source,
                offset: signal.o,
            },
            timestamp: signal.t,
            step_id: None,
        })
        .collect();
    compactions.sort_by(|a, b| a.position.cmp(&b.position));
    // Signals have already passed native compaction dedupe. Only suppress a
    // repeated observation of that exact normalized source/offset here.
    compactions.dedup_by(|a, b| a.position == b.position);
    let timestamped = steps.partition_point(|step| step.timestamp.is_some());
    let mut source_steps: BTreeMap<String, Vec<(u64, usize)>> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate() {
        source_steps
            .entry(step.position.source.clone())
            .or_default()
            .push((step.position.offset, index));
    }
    for offsets in source_steps.values_mut() {
        offsets.sort_unstable();
    }
    for compact in &mut compactions {
        let owner = if let Some(at) = compact.timestamp {
            let next = steps[..timestamped].partition_point(|step| {
                (step.timestamp.expect("timestamped prefix"), &step.position)
                    < (at, &compact.position)
            });
            if next < timestamped {
                Some(next)
            } else {
                timestamped.checked_sub(1)
            }
        } else {
            // Without time, only offsets within the same physical source prove
            // following/preceding order. Cross-source placement stays unknown.
            source_steps
                .get(&compact.position.source)
                .and_then(|offsets| {
                    let next =
                        offsets.partition_point(|(offset, _)| *offset < compact.position.offset);
                    offsets
                        .get(next)
                        .or_else(|| offsets.last())
                        .map(|(_, index)| *index)
                })
        };
        if let Some(owner) = owner {
            steps[owner].compactions += 1;
            compact.step_id = Some(steps[owner].id.clone());
        }
    }
    let mut aggregate = SessionActivityAggregate {
        compactions: compactions.len() as u64,
        ..Default::default()
    };
    for step in &steps {
        aggregate.totals.add(step);
        aggregate.by_band.entry(step.band).or_default().add(step);
    }
    for window in steps.windows(3) {
        if window.iter().map(|step| step.compactions).sum::<u64>() >= 3 {
            aggregate
                .thrash_windows
                .push(std::array::from_fn(|index| window[index].id.clone()));
        }
    }
    aggregate.thrash = !aggregate.thrash_windows.is_empty();
    aggregate.exclusive_cost_complete = aggregate.totals.shared.steps == 0
        && aggregate.totals.exclusive.list_cost_usd.unknown_steps == 0;
    SessionActivity {
        session_key,
        rules_version: ACTIVITY_RULES_VERSION,
        pricing_fingerprint: table.fingerprint.clone(),
        steps,
        compactions,
        aggregate,
        facts: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
