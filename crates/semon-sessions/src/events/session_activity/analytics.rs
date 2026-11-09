//! Compact producer-owned summaries. Detail and source positions stay out of
//! Catalog/query summaries; exact shared keys exist only during family folding.
use std::collections::{BTreeMap, VecDeque};

use super::*;
use classify::{Activity, Change, WasteCategory, WasteKind};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ChangeCounts {
    pub(crate) commits: u64,
    pub(crate) pushes: u64,
    pub(crate) pull_requests: u64,
    pub(crate) without_exact_step: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct OwnedMeasured {
    pub(crate) observed: Measured<u64>,
    pub(crate) exclusive: Measured<u64>,
    pub(crate) shared: Measured<u64>,
}

/// Each token component of exclusive steps priced independently; the exclusive
/// list cost in `Counts::totals` is what these usually sum to. Kept to the
/// exclusive view and out of the context bands so Catalog rows stay small.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Components {
    pub(crate) fresh_input_usd: Measured<f64>,
    pub(crate) cache_read_usd: Measured<f64>,
    pub(crate) cache_write_usd: Measured<f64>,
    pub(crate) output_usd: Measured<f64>,
}

impl Components {
    fn add(&mut self, step: &ActivityStep) {
        self.fresh_input_usd.add(step.fresh_input_usd);
        self.cache_read_usd.add(step.cache_read_usd);
        self.cache_write_usd.add(step.cache_write_usd);
        self.output_usd.add(step.output_usd);
    }

    fn merge(&mut self, other: &Self) {
        measured(&mut self.fresh_input_usd, &other.fresh_input_usd);
        measured(&mut self.cache_read_usd, &other.cache_read_usd);
        measured(&mut self.cache_write_usd, &other.cache_write_usd);
        measured(&mut self.output_usd, &other.output_usd);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OwnedCounts {
    pub(crate) observed: u64,
    pub(crate) exclusive: u64,
    pub(crate) shared: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WasteTotals {
    pub(crate) steps: u64,
    pub(crate) elapsed_ms: Measured<u64>,
    pub(crate) list_cost_usd: Measured<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum LeverNotComputed {
    TaskSpecification,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum TriggerNotComputed {
    InstructionChange,
    MissingInput,
}

/// At most this many bins summarize the ordered exclusive steps.
const PREVIEW_BINS: usize = 64;

/// Exclusive steps by how completely their token components are priced. Only
/// components that carry tokens count: a step is `full` when all of them are
/// priced (including a step with no tokens), `unpriced` when none is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PricingCoverage {
    pub(crate) full_steps: u64,
    pub(crate) partial_steps: u64,
    pub(crate) unpriced_steps: u64,
}

/// Exclusive steps by whether their `elapsed_ms` is known.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TimingCoverage {
    pub(crate) known_steps: u64,
    pub(crate) unknown_steps: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClassifierCoverage {
    pub(crate) rules_version: u32,
    pub(crate) levers_not_computed: Vec<LeverNotComputed>,
    pub(crate) activities_not_computed: Vec<Activity>,
    pub(crate) triggers_not_computed: Vec<TriggerNotComputed>,
    /// Exclusive steps with no activity label: `by_activity` steps plus this
    /// equal the exclusive step total.
    pub(crate) unlabelled_activity_steps: u64,
}

/// What the other summary values did not or could not measure. The measured
/// wait union is `wait_time_ms`, which does not depend on step pages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Coverage {
    pub(crate) pricing: PricingCoverage,
    pub(crate) timing: TimingCoverage,
    pub(crate) classifier: ClassifierCoverage,
}

/// One bin of the timeline preview. Step numbers are the producer ordinals
/// `get_session_activity` pages by; shared steps are not binned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PreviewBin {
    pub(crate) first_step: u64,
    pub(crate) last_step: u64,
    pub(crate) max_context_tokens: u64,
    pub(crate) last_context_tokens: u64,
    pub(crate) list_cost_usd: Measured<f64>,
    pub(crate) waste_by_category: BTreeMap<WasteCategory, u64>,
    pub(crate) compactions: u64,
}

/// The ordered exclusive steps in equal-count bins of ceil(n/64) steps, so at
/// most 64 bins whatever the session length. Derived with the summary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Preview {
    pub(crate) steps_total: u64,
    pub(crate) binned_steps: u64,
    pub(crate) bins: Vec<PreviewBin>,
}

impl Preview {
    pub(crate) fn of(steps: &[ActivityStep]) -> Self {
        let exclusive: Vec<_> = steps
            .iter()
            .enumerate()
            .filter(|(_, step)| matches!(step.ownership, Ownership::Exclusive))
            .collect();
        let bins = exclusive
            .chunks(exclusive.len().div_ceil(PREVIEW_BINS).max(1))
            .map(|chunk| {
                let mut bin = PreviewBin {
                    first_step: chunk[0].0 as u64,
                    last_step: chunk[chunk.len() - 1].0 as u64,
                    ..PreviewBin::default()
                };
                for (_, step) in chunk {
                    bin.max_context_tokens = bin.max_context_tokens.max(step.context_tokens);
                    bin.last_context_tokens = step.context_tokens;
                    bin.list_cost_usd.add(step.list_cost_usd);
                    if let Some(kind) = step.waste {
                        *bin.waste_by_category.entry(kind.category()).or_default() += 1;
                    }
                    bin.compactions += step.compactions;
                }
                bin
            })
            .collect();
        Self {
            steps_total: steps.len() as u64,
            binned_steps: exclusive.len() as u64,
            bins,
        }
    }
}

const ACTIVITIES_NOT_COMPUTED: [Activity; 1] = [Activity::Recover];
const TRIGGERS_NOT_COMPUTED: [TriggerNotComputed; 2] = [
    TriggerNotComputed::InstructionChange,
    TriggerNotComputed::MissingInput,
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Counts {
    pub(crate) totals: OwnershipTotals,
    pub(crate) component_cost_usd: Components,
    pub(crate) by_band: BTreeMap<ContextBand, OwnershipTotals>,
    pub(crate) compactions: u64,
    pub(crate) waste_by_kind: BTreeMap<WasteKind, u64>,
    pub(crate) waste_by_category: BTreeMap<WasteCategory, u64>,
    pub(crate) waste_cost_by_kind: BTreeMap<WasteKind, WasteTotals>,
    pub(crate) waste_cost_by_category: BTreeMap<WasteCategory, WasteTotals>,
    /// Exclusive labelled steps, like waste cost.
    pub(crate) by_activity: BTreeMap<Activity, WasteTotals>,
    /// Exclusive steps with waste or a verify failure, by their trigger.
    pub(crate) by_trigger: BTreeMap<Trigger, WasteTotals>,
    pub(crate) coverage: Coverage,
    pub(crate) wait_time_ms: OwnedMeasured,
    pub(crate) waits_without_interval: OwnedCounts,
    pub(crate) empty_search_steps: u64,
    pub(crate) verify_failure_steps: u64,
    pub(crate) silent_exit_steps: u64,
    pub(crate) change_facts: ChangeCounts,
    pub(crate) exclusive_cost_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Analytics {
    pub(crate) pricing_fingerprint: String,
    #[serde(flatten)]
    pub(crate) counts: Counts,
    pub(crate) thrash: bool,
    pub(crate) family: Option<Family>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Family {
    #[serde(flatten)]
    pub(crate) counts: Counts,
    pub(crate) member_sessions: u64,
    pub(crate) thrashed_sessions: u64,
    /// Sum of capped physical-session step durations, including shared
    /// observations. Overlapping sessions can exceed family wall-clock time.
    pub(crate) total_step_time_ms: Measured<u64>,
}

pub(super) fn interval_union_ms(intervals: &[(i64, i64)]) -> u64 {
    let mut intervals = intervals.to_vec();
    intervals.sort_unstable();
    let mut total = 0i128;
    let mut current: Option<(i64, i64)> = None;
    for (start, end) in intervals {
        match current {
            Some((current_start, current_end)) if start <= current_end => {
                current = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                total += i128::from(current_end) - i128::from(current_start);
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((start, end)) = current {
        total += i128::from(end) - i128::from(start);
    }
    u64::try_from(total).unwrap_or(u64::MAX)
}

fn add_waste<K: Ord>(map: &mut BTreeMap<K, WasteTotals>, key: K, step: &ActivityStep) {
    let totals = map.entry(key).or_default();
    totals.steps += 1;
    totals.elapsed_ms.add(step.elapsed_ms);
    totals.list_cost_usd.add(step.list_cost_usd);
}

impl OwnedCounts {
    fn add(&mut self, ownership: &Ownership) {
        self.observed += 1;
        match ownership {
            Ownership::Exclusive => self.exclusive += 1,
            Ownership::Shared(_) => self.shared += 1,
        }
    }
}

impl OwnedMeasured {
    fn add_missing(&mut self, ownership: &Ownership) {
        self.observed.unknown_steps += 1;
        match ownership {
            Ownership::Exclusive => self.exclusive.unknown_steps += 1,
            Ownership::Shared(_) => self.shared.unknown_steps += 1,
        }
    }
}

impl Counts {
    /// Every value derived from steps alone, shared by first derivation and
    /// repricing.
    fn of_steps(steps: &[ActivityStep]) -> Self {
        let mut counts = Self {
            coverage: Coverage {
                classifier: ClassifierCoverage {
                    rules_version: ACTIVITY_RULES_VERSION,
                    levers_not_computed: vec![LeverNotComputed::TaskSpecification],
                    activities_not_computed: ACTIVITIES_NOT_COMPUTED.to_vec(),
                    triggers_not_computed: TRIGGERS_NOT_COMPUTED.to_vec(),
                    unlabelled_activity_steps: 0,
                },
                ..Coverage::default()
            },
            ..Self::default()
        };
        let mut wait_intervals: [Vec<(i64, i64)>; 3] = Default::default();
        for step in steps {
            counts.totals.add(step);
            counts.by_band.entry(step.band).or_default().add(step);
            if let Some(kind) = step.waste {
                *counts.waste_by_kind.entry(kind).or_default() += 1;
                *counts.waste_by_category.entry(kind.category()).or_default() += 1;
            }
            if step.waste == Some(WasteKind::OrchestrationWait) && step.waits.is_empty() {
                counts.waits_without_interval.add(&step.ownership);
                counts.wait_time_ms.add_missing(&step.ownership);
            }
            for wait in &step.waits {
                match &step.ownership {
                    Ownership::Exclusive => {
                        wait_intervals[0].push((wait.started_at_ms, wait.completed_at_ms))
                    }
                    Ownership::Shared(_) => {
                        wait_intervals[1].push((wait.started_at_ms, wait.completed_at_ms))
                    }
                }
                wait_intervals[2].push((wait.started_at_ms, wait.completed_at_ms));
            }
            if matches!(&step.ownership, Ownership::Exclusive) {
                if let Some(kind) = step.waste {
                    add_waste(&mut counts.waste_cost_by_kind, kind, step);
                    add_waste(&mut counts.waste_cost_by_category, kind.category(), step);
                }
                match step.activity {
                    Some(activity) => add_waste(&mut counts.by_activity, activity, step),
                    None => counts.coverage.classifier.unlabelled_activity_steps += 1,
                }
                if let Some(trigger) = step.trigger {
                    add_waste(&mut counts.by_trigger, trigger, step);
                }
                counts.coverage.add_exclusive(step);
                counts.component_cost_usd.add(step);
            }
            counts.empty_search_steps += u64::from(step.empty_search);
            counts.verify_failure_steps += u64::from(step.verify_failure);
            counts.silent_exit_steps += u64::from(step.silent_exit);
        }
        counts.wait_time_ms.exclusive.known = interval_union_ms(&wait_intervals[0]);
        counts.wait_time_ms.shared.known = interval_union_ms(&wait_intervals[1]);
        counts.wait_time_ms.observed.known = interval_union_ms(&wait_intervals[2]);
        counts.exclusive_cost_complete = counts.totals.shared.steps == 0
            && counts.totals.exclusive.list_cost_usd.unknown_steps == 0;
        counts
    }

    fn of(activity: &SessionActivity) -> Self {
        let mut counts = Self::of_steps(&activity.steps);
        counts.compactions = activity.aggregate.compactions;
        for fact in &activity.facts {
            match fact.change {
                Change::Commit { .. } => counts.change_facts.commits += 1,
                Change::Push { .. } => counts.change_facts.pushes += 1,
                Change::PullRequest { .. } => counts.change_facts.pull_requests += 1,
            }
            counts.change_facts.without_exact_step += u64::from(fact.step_id.is_none());
        }
        counts
    }
}

impl Coverage {
    fn add_exclusive(&mut self, step: &ActivityStep) {
        let tokens = &step.tokens;
        let (mut priced, mut unpriced) = (0, 0);
        for (tokens, usd) in [
            (tokens.input, step.fresh_input_usd),
            (tokens.cache_read, step.cache_read_usd),
            (tokens.cache_write, step.cache_write_usd),
            (tokens.output, step.output_usd),
        ] {
            if tokens > 0 {
                if usd.is_some() {
                    priced += 1;
                } else {
                    unpriced += 1;
                }
            }
        }
        match (priced, unpriced) {
            (_, 0) => self.pricing.full_steps += 1,
            (0, _) => self.pricing.unpriced_steps += 1,
            _ => self.pricing.partial_steps += 1,
        }
        if step.elapsed_ms.is_some() {
            self.timing.known_steps += 1;
        } else {
            self.timing.unknown_steps += 1;
        }
    }

    fn merge(&mut self, other: &Self) {
        self.pricing.full_steps += other.pricing.full_steps;
        self.pricing.partial_steps += other.pricing.partial_steps;
        self.pricing.unpriced_steps += other.pricing.unpriced_steps;
        self.timing.known_steps += other.timing.known_steps;
        self.timing.unknown_steps += other.timing.unknown_steps;
        self.classifier.unlabelled_activity_steps += other.classifier.unlabelled_activity_steps;
    }
}

impl Analytics {
    pub(crate) fn of(activity: &SessionActivity) -> Self {
        Self {
            pricing_fingerprint: activity.pricing_fingerprint.clone(),
            counts: Counts::of(activity),
            thrash: activity.aggregate.thrash,
            family: None,
        }
    }

    /// Prices change no other fact: recompute every step-derived value from
    /// the repriced steps and keep the compaction and change-fact counts.
    pub(crate) fn reprice(&mut self, steps: &[ActivityStep]) {
        self.counts = Counts {
            compactions: self.counts.compactions,
            change_facts: self.counts.change_facts.clone(),
            ..Counts::of_steps(steps)
        };
    }
}

fn measured<T: std::ops::AddAssign + Copy>(a: &mut Measured<T>, b: &Measured<T>) {
    a.known += b.known;
    a.unknown_steps += b.unknown_steps;
}
fn totals(a: &mut StepTotals, b: &StepTotals) {
    a.steps += b.steps;
    measured(&mut a.list_cost_usd, &b.list_cost_usd);
    measured(&mut a.elapsed_ms, &b.elapsed_ms);
}
fn ownership(a: &mut OwnershipTotals, b: &OwnershipTotals) {
    totals(&mut a.observed, &b.observed);
    totals(&mut a.exclusive, &b.exclusive);
    totals(&mut a.shared, &b.shared);
}
fn owned_measured(a: &mut OwnedMeasured, b: &OwnedMeasured) {
    measured(&mut a.observed, &b.observed);
    measured(&mut a.exclusive, &b.exclusive);
    measured(&mut a.shared, &b.shared);
}
fn owned_counts(a: &mut OwnedCounts, b: &OwnedCounts) {
    a.observed += b.observed;
    a.exclusive += b.exclusive;
    a.shared += b.shared;
}
fn waste_totals(a: &mut WasteTotals, b: &WasteTotals) {
    a.steps += b.steps;
    measured(&mut a.elapsed_ms, &b.elapsed_ms);
    measured(&mut a.list_cost_usd, &b.list_cost_usd);
}
fn waste_map<K: Ord>(a: &mut BTreeMap<K, WasteTotals>, b: BTreeMap<K, WasteTotals>) {
    for (key, value) in b {
        waste_totals(a.entry(key).or_default(), &value);
    }
}

pub(crate) struct Fold {
    counts: Counts,
    shared: BTreeMap<String, ActivityStep>,
    members: u64,
    thrashed: u64,
    time: Measured<u64>,
}
impl Fold {
    pub(crate) fn summary(analytics: &Analytics, shared: Vec<ActivityStep>) -> Self {
        let mut counts = analytics.counts.clone();
        counts.totals.observed = counts.totals.exclusive.clone();
        counts.totals.shared = StepTotals::default();
        for band in counts.by_band.values_mut() {
            band.observed = band.exclusive.clone();
            band.shared = StepTotals::default();
        }
        Self {
            counts,
            shared: shared
                .into_iter()
                .filter_map(|step| {
                    let Ownership::Shared(key) = &step.ownership else {
                        return None;
                    };
                    Some((key.clone(), step))
                })
                .collect(),
            members: 1,
            thrashed: u64::from(analytics.thrash),
            time: analytics.counts.totals.observed.elapsed_ms.clone(),
        }
    }
    fn of(activity: &SessionActivity) -> Self {
        let mut counts = Counts::of(activity);
        // Sum exclusive steps now; add each exact shared key once at the end.
        counts.totals = OwnershipTotals::default();
        counts.component_cost_usd = Components::default();
        counts.by_band.clear();
        let mut shared = BTreeMap::new();
        for step in &activity.steps {
            match &step.ownership {
                Ownership::Exclusive => {
                    counts.totals.add(step);
                    counts.component_cost_usd.add(step);
                    counts.by_band.entry(step.band).or_default().add(step);
                }
                Ownership::Shared(key) => {
                    shared.entry(key.clone()).or_insert_with(|| step.clone());
                }
            }
        }
        Self {
            counts,
            shared,
            members: 1,
            thrashed: u64::from(activity.aggregate.thrash),
            time: activity.aggregate.totals.observed.elapsed_ms.clone(),
        }
    }
    pub(crate) fn merge(&mut self, child: Self) {
        ownership(&mut self.counts.totals, &child.counts.totals);
        self.counts
            .component_cost_usd
            .merge(&child.counts.component_cost_usd);
        for (band, values) in child.counts.by_band {
            ownership(self.counts.by_band.entry(band).or_default(), &values);
        }
        self.counts.compactions += child.counts.compactions;
        for (kind, n) in child.counts.waste_by_kind {
            *self.counts.waste_by_kind.entry(kind).or_default() += n;
        }
        for (category, n) in child.counts.waste_by_category {
            *self.counts.waste_by_category.entry(category).or_default() += n;
        }
        waste_map(
            &mut self.counts.waste_cost_by_kind,
            child.counts.waste_cost_by_kind,
        );
        waste_map(
            &mut self.counts.waste_cost_by_category,
            child.counts.waste_cost_by_category,
        );
        waste_map(&mut self.counts.by_activity, child.counts.by_activity);
        waste_map(&mut self.counts.by_trigger, child.counts.by_trigger);
        self.counts.coverage.merge(&child.counts.coverage);
        owned_measured(&mut self.counts.wait_time_ms, &child.counts.wait_time_ms);
        owned_counts(
            &mut self.counts.waits_without_interval,
            &child.counts.waits_without_interval,
        );
        self.counts.empty_search_steps += child.counts.empty_search_steps;
        self.counts.verify_failure_steps += child.counts.verify_failure_steps;
        self.counts.silent_exit_steps += child.counts.silent_exit_steps;
        self.counts.change_facts.commits += child.counts.change_facts.commits;
        self.counts.change_facts.pushes += child.counts.change_facts.pushes;
        self.counts.change_facts.pull_requests += child.counts.change_facts.pull_requests;
        self.counts.change_facts.without_exact_step += child.counts.change_facts.without_exact_step;
        self.counts.exclusive_cost_complete &= child.counts.exclusive_cost_complete;
        for (key, step) in child.shared {
            self.shared.entry(key).or_insert(step);
        }
        self.members += child.members;
        self.thrashed += child.thrashed;
        measured(&mut self.time, &child.time);
    }
    pub(crate) fn family(&self) -> Family {
        let mut counts = self.counts.clone();
        for step in self.shared.values() {
            counts.totals.add(step);
            counts.by_band.entry(step.band).or_default().add(step);
        }
        Family {
            counts,
            member_sessions: self.members,
            thrashed_sessions: self.thrashed,
            total_step_time_ms: self.time.clone(),
        }
    }
}

/// Follow the producer's exact prepared parent chain, like family_costs. A
/// missing member projection or cyclic chain leaves the family unknown.
pub(crate) fn families<'a>(
    rows: impl Iterator<Item = (&'a str, Option<&'a str>, Option<&'a SessionActivity>)>,
) -> BTreeMap<&'a str, Family> {
    let rows: BTreeMap<_, _> = rows
        .map(|(id, parent, activity)| (id, (parent, activity)))
        .collect();
    let mut folds: BTreeMap<_, _> = rows
        .iter()
        .map(|(&id, (_, a))| (id, a.map(Fold::of)))
        .collect();
    let mut pending: BTreeMap<_, usize> = rows.keys().map(|&id| (id, 0)).collect();
    for (parent, _) in rows.values() {
        if let Some(n) = parent.and_then(|id| pending.get_mut(id)) {
            *n += 1;
        }
    }
    let mut ready: VecDeque<_> = pending
        .iter()
        .filter_map(|(&id, &n)| (n == 0).then_some(id))
        .collect();
    let mut result = BTreeMap::new();
    while let Some(id) = ready.pop_front() {
        let fold = folds.remove(id).flatten();
        if let Some(fold) = &fold
            && fold.members > 1
        {
            result.insert(id, fold.family());
        }
        if let Some(parent) = rows[id].0.filter(|parent| rows.contains_key(parent)) {
            let parent_fold = folds.get_mut(parent).expect("registered parent");
            match (parent_fold.as_mut(), fold) {
                (Some(parent), Some(child)) => parent.merge(child),
                _ => *parent_fold = None,
            }
            let n = pending.get_mut(parent).expect("registered parent");
            *n -= 1;
            if *n == 0 {
                ready.push_back(parent);
            }
        }
    }
    result
}
