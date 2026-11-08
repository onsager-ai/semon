//! Compact producer-owned summaries. Detail and source positions stay out of
//! Catalog/query summaries; exact shared keys exist only during family folding.
use std::collections::{BTreeMap, VecDeque};

use super::*;
use classify::{Change, WasteCategory, WasteKind};

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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Counts {
    pub(crate) totals: OwnershipTotals,
    pub(crate) by_band: BTreeMap<ContextBand, OwnershipTotals>,
    pub(crate) compactions: u64,
    pub(crate) waste_by_kind: BTreeMap<WasteKind, u64>,
    pub(crate) waste_by_category: BTreeMap<WasteCategory, u64>,
    pub(crate) waste_cost_by_kind: BTreeMap<WasteKind, WasteTotals>,
    pub(crate) waste_cost_by_category: BTreeMap<WasteCategory, WasteTotals>,
    pub(crate) wait_time_ms: OwnedMeasured,
    pub(crate) waits_without_interval: OwnedCounts,
    pub(crate) empty_search_steps: u64,
    pub(crate) change_facts: ChangeCounts,
    pub(crate) exclusive_cost_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Analytics {
    pub(crate) rules_version: u32,
    pub(crate) pricing_fingerprint: String,
    #[serde(flatten)]
    pub(crate) counts: Counts,
    pub(crate) levers_not_computed: Vec<LeverNotComputed>,
    pub(crate) thrash: bool,
    pub(crate) family: Option<Family>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Family {
    #[serde(flatten)]
    pub(crate) counts: Counts,
    pub(crate) member_sessions: u64,
    pub(crate) thrashed_sessions: u64,
    pub(crate) levers_not_computed: Vec<LeverNotComputed>,
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
    fn of(activity: &SessionActivity) -> Self {
        let mut counts = Self {
            totals: activity.aggregate.totals.clone(),
            by_band: activity.aggregate.by_band.clone(),
            compactions: activity.aggregate.compactions,
            exclusive_cost_complete: activity.aggregate.exclusive_cost_complete,
            ..Self::default()
        };
        let mut wait_intervals: [Vec<(i64, i64)>; 3] = Default::default();
        for step in &activity.steps {
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
            if let Some(kind) = step.waste
                && matches!(&step.ownership, Ownership::Exclusive)
            {
                add_waste(&mut counts.waste_cost_by_kind, kind, step);
                add_waste(&mut counts.waste_cost_by_category, kind.category(), step);
            }
            counts.empty_search_steps += u64::from(step.empty_search);
        }
        counts.wait_time_ms.exclusive.known = interval_union_ms(&wait_intervals[0]);
        counts.wait_time_ms.shared.known = interval_union_ms(&wait_intervals[1]);
        counts.wait_time_ms.observed.known = interval_union_ms(&wait_intervals[2]);
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

impl Analytics {
    pub(crate) fn of(activity: &SessionActivity) -> Self {
        Self {
            rules_version: activity.rules_version,
            pricing_fingerprint: activity.pricing_fingerprint.clone(),
            counts: Counts::of(activity),
            levers_not_computed: vec![LeverNotComputed::TaskSpecification],
            thrash: activity.aggregate.thrash,
            family: None,
        }
    }
}

fn measured<T: std::ops::AddAssign + Copy>(a: &mut Measured<T>, b: &Measured<T>) {
    a.known += b.known;
    a.unknown_steps += b.unknown_steps;
}
fn totals(a: &mut StepTotals, b: &StepTotals) {
    a.steps += b.steps;
    measured(&mut a.list_cost_usd, &b.list_cost_usd);
    measured(&mut a.carry_cost_usd, &b.carry_cost_usd);
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
        counts.by_band.clear();
        let mut shared = BTreeMap::new();
        for step in &activity.steps {
            match &step.ownership {
                Ownership::Exclusive => {
                    counts.totals.add(step);
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
        owned_measured(&mut self.counts.wait_time_ms, &child.counts.wait_time_ms);
        owned_counts(
            &mut self.counts.waits_without_interval,
            &child.counts.waits_without_interval,
        );
        self.counts.empty_search_steps += child.counts.empty_search_steps;
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
            levers_not_computed: vec![LeverNotComputed::TaskSpecification],
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
