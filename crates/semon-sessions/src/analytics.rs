//! `/api/analytics`: what the Analytics page draws for the last 24 hours,
//! 7 days or 30 days, computed on the server.
//!
//! `/api/model` keeps only its own window (24 h by default), so a week or a
//! month can't be computed from it in the browser. A build reads every log
//! anyway and trims to the window at its end: before it trims, it keeps what
//! Analytics reads of each session active in the last [`KEEP_MS`]
//! ([`Activity`]: names, facets, times and totals, no text). A request
//! computes its range from those rows alone and builds nothing; [`Cache`]
//! keeps each answer until the model changes.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    events::RateLimits,
    model::{Handoff, Session, Transcript, Turn, fnv},
    viewer::decoded,
};

pub(crate) const DAY_MS: i64 = 86_400_000;
/// How far back a build keeps what Analytics reads: the longest range
/// (30 days) and the period before it, which its deltas compare with.
pub(crate) const KEEP_MS: i64 = 60 * DAY_MS;
/// The longest filter value a request may name, in bytes.
const VALUE_MAX: usize = 256;
/// A chart column's drill-in lists at most this many sessions (the most
/// busy, or the most expensive), and counts the rest.
pub(crate) const SLICE_MAX: usize = 100;
/// Each "top sessions" list's length.
const TOP: usize = 5;
/// Answers kept, one per range and filters.
const CACHE_MAX: usize = 16;
/// A kept answer is computed again once the model changed, but never sooner
/// than this after it was computed.
const SPACING: Duration = Duration::from_secs(1);
/// With nothing changed, time still moves the range and the waits: a kept
/// answer is computed again once it is this old.
const STALE_AFTER: Duration = Duration::from_secs(30);

/// What Analytics reads of one session, kept by each build for the sessions
/// active in the last [`KEEP_MS`] whatever the model's own window. Stubs
/// (the far end of a handoff these logs don't have) are left out: they
/// stand for a session, they didn't run here.
#[derive(Clone, Debug)]
pub(crate) struct Activity {
    pub(crate) name: String,
    pub(crate) harness: &'static str,
    pub(crate) repo: Option<String>,
    pub(crate) model: String,
    /// Working now: its last busy interval runs to the time of the answer.
    pub(crate) working: bool,
    pub(crate) start: i64,
    pub(crate) busy: Vec<(i64, i64)>,
    /// When each turn started (epoch ms).
    pub(crate) turns: Vec<i64>,
    /// Tool calls, and those that failed or never finished; `None` without a
    /// transcript.
    pub(crate) calls: Option<usize>,
    pub(crate) errors: Option<usize>,
    /// API-equivalent cost per UTC day: the day's start (epoch ms) and USD.
    pub(crate) cost_by_day: Vec<(i64, f64)>,
    pub(crate) unpriced_models: Vec<String>,
    pub(crate) rate_limits: Option<RateLimits>,
    /// When each of its messages to you that still waits was sent.
    pub(crate) waits: Vec<i64>,
}

/// A `YYYY-MM-DD` UTC day's start (epoch ms).
fn day_start(day: &str) -> Option<i64> {
    let bytes = day.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = day.get(..4)?.parse().ok()?;
    let month: i64 = day.get(5..7)?.parse().ok()?;
    let date: i64 = day.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&date) {
        return None;
    }
    Some(crate::days_from_civil(year, month, date) * DAY_MS)
}

/// What Analytics reads of the model's sessions before the model is trimmed
/// to its window: every session active since `now - KEEP_MS`, its busy
/// intervals, turns and cost days since then, and its waits on you.
pub(crate) fn activity(
    sessions: &BTreeMap<String, Session>,
    tx: &BTreeMap<String, Transcript>,
    turns: &[Turn],
    handoffs: &[Handoff],
    now: i64,
) -> BTreeMap<String, Activity> {
    let since = now.saturating_sub(KEEP_MS);
    let mut rows: BTreeMap<String, Activity> = sessions
        .iter()
        .filter(|(_, session)| {
            !session.stub && (session.last >= since || matches!(session.state, "work" | "wait"))
        })
        .map(|(id, session)| {
            let transcript = tx.get(id);
            let row = Activity {
                name: session.name.clone(),
                harness: session.harness,
                repo: session.repo.clone(),
                model: session.model.clone(),
                working: session.state == "work",
                start: session.start,
                busy: session
                    .busy
                    .iter()
                    .copied()
                    .filter(|(_, end)| *end >= since)
                    .collect(),
                turns: Vec::new(),
                calls: transcript.map(|transcript| transcript.calls),
                errors: transcript.map(|transcript| transcript.errors),
                cost_by_day: session
                    .cost
                    .by_day
                    .iter()
                    .filter_map(|(day, usd)| Some((day_start(day)?, *usd)))
                    .filter(|(day, _)| day + DAY_MS > since)
                    .collect(),
                unpriced_models: session.cost.unpriced_models.clone(),
                rate_limits: session.rate_limits.clone(),
                waits: Vec::new(),
            };
            (id.clone(), row)
        })
        .collect();
    let sent_at: BTreeMap<&str, i64> = handoffs
        .iter()
        .map(|handoff| (handoff.id.as_str(), handoff.at))
        .collect();
    for turn in turns {
        if let Some(row) = rows.get_mut(&turn.sid) {
            // As the page placed a turn: its own time, else its start
            // handoff's, else its session's start.
            let at = turn
                .at
                .or_else(|| {
                    turn.start
                        .as_deref()
                        .and_then(|id| sent_at.get(id).copied())
                })
                .unwrap_or(row.start);
            if at >= since {
                row.turns.push(at);
            }
        }
    }
    for handoff in handoffs {
        if handoff.kind == "toyou"
            && handoff.status == "wait"
            && let Some(row) = rows.get_mut(&handoff.from)
        {
            row.waits.push(handoff.at);
        }
    }
    // Collected and pushed without knowing their lengths: give back what
    // the growth left over, since these rows live as long as the model.
    for row in rows.values_mut() {
        row.busy.shrink_to_fit();
        row.turns.shrink_to_fit();
        row.cost_by_day.shrink_to_fit();
        row.waits.shrink_to_fit();
    }
    rows
}

/// What `rows` hold on the heap and inline, in bytes: each key and row,
/// and what their strings and vectors allocated (their capacity). The
/// allocator's and the map's own overhead are not counted.
#[cfg(test)]
pub(crate) fn heap_bytes(rows: &BTreeMap<String, Activity>) -> usize {
    use std::mem::size_of;
    rows.iter()
        .map(|(id, row)| {
            size_of::<String>()
                + id.capacity()
                + size_of::<Activity>()
                + row.name.capacity()
                + row.repo.as_ref().map_or(0, String::capacity)
                + row.model.capacity()
                + row.busy.capacity() * size_of::<(i64, i64)>()
                + row.turns.capacity() * size_of::<i64>()
                + row.cost_by_day.capacity() * size_of::<(i64, f64)>()
                + row.unpriced_models.capacity() * size_of::<String>()
                + row
                    .unpriced_models
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>()
                + row.rate_limits.as_ref().map_or(0, |limits| {
                    limits.windows.capacity() * size_of::<crate::events::RateLimitWindow>()
                })
                + row.waits.capacity() * size_of::<i64>()
        })
        .sum()
}

/// One session as a core serves it: its id, the id of the machine it ran
/// on, and what Analytics reads of it.
#[derive(Clone, Copy)]
pub(crate) struct Row<'a> {
    pub(crate) id: &'a str,
    pub(crate) machine: &'a str,
    pub(crate) activity: &'a Activity,
}

/// Every machine's rows, in the order the machines are served. A session id
/// an earlier machine has is left out of a later one: it counts once.
pub(crate) fn rows<'a>(
    machines: impl IntoIterator<Item = (&'a str, &'a BTreeMap<String, Activity>)>,
) -> Vec<Row<'a>> {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for (machine, activity) in machines {
        for (id, row) in activity {
            if seen.insert(id.as_str()) {
                rows.push(Row {
                    id,
                    machine,
                    activity: row,
                });
            }
        }
    }
    rows
}

/// The ranges Analytics offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Range {
    Day,
    Week,
    Month,
}

impl Range {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "24h" => Some(Self::Day),
            "7d" => Some(Self::Week),
            "30d" => Some(Self::Month),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    fn days(self) -> i64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }

    /// The agent-hours chart's columns, and what each one covers.
    fn columns(self) -> (i64, &'static str) {
        match self {
            Self::Day => (24, "per hour"),
            Self::Week => (28, "per 6 hours"),
            Self::Month => (30, "per day"),
        }
    }
}

/// A request to `/api/analytics`: `range` (`24h`, `7d` or `30d`), and the
/// page's filters, each at most once: `repo` (empty: the sessions with no
/// repo), `machine`, `harness` and `model`. Anything else, a key named
/// twice, an empty value (but `repo`'s), a value that doesn't decode or one
/// over [`VALUE_MAX`] bytes is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) range: Range,
    /// `Some(None)`: the sessions with no repo.
    pub(crate) repo: Option<Option<String>>,
    pub(crate) machine: Option<String>,
    pub(crate) harness: Option<String>,
    pub(crate) model: Option<String>,
}

impl Request {
    pub(crate) fn parse(query: &str) -> Option<Self> {
        let mut seen = BTreeSet::new();
        let mut range = None;
        let mut request = Self {
            range: Range::Week,
            repo: None,
            machine: None,
            harness: None,
            model: None,
        };
        for part in query.split('&') {
            let (name, raw) = part.split_once('=')?;
            if !seen.insert(name) {
                return None;
            }
            let value = decoded(raw)?;
            if value.len() > VALUE_MAX {
                return None;
            }
            let nonempty = |value: String| (!value.is_empty()).then_some(value);
            match name {
                "range" => range = Some(Range::parse(&value)?),
                "repo" => request.repo = Some(nonempty(value)),
                "machine" => request.machine = Some(nonempty(value)?),
                "harness" => request.harness = Some(nonempty(value)?),
                "model" => request.model = Some(nonempty(value)?),
                _ => return None,
            }
        }
        request.range = range?;
        Some(request)
    }

    /// The cache's key: the range and the filters, unambiguously (no repo
    /// filter is `null`, the filter for no repo `[null]`).
    pub(crate) fn key(&self) -> String {
        serde_json::to_string(&json!([
            self.range.name(),
            self.repo.as_ref().map(|repo| [repo]),
            self.machine,
            self.harness,
            self.model
        ]))
        .expect("serializable key")
    }

    /// Whether each filter names a value some row has (over every row
    /// kept, not only the range's). A request that doesn't is answered, but
    /// its answer isn't kept: any value would otherwise be a new key.
    pub(crate) fn known(&self, rows: &[Row]) -> bool {
        let has = |test: &dyn Fn(&Row) -> bool| rows.iter().any(test);
        self.repo
            .as_ref()
            .is_none_or(|repo| has(&|row| row.activity.repo.as_ref() == repo.as_ref()))
            && self
                .machine
                .as_deref()
                .is_none_or(|machine| has(&|row| row.machine == machine))
            && self
                .harness
                .as_deref()
                .is_none_or(|harness| has(&|row| row.activity.harness == harness))
            && self
                .model
                .as_deref()
                .is_none_or(|model| has(&|row| row.activity.model == model))
    }

    fn matches(&self, row: &Row) -> bool {
        let activity = row.activity;
        self.repo
            .as_ref()
            .is_none_or(|repo| activity.repo.as_ref() == repo.as_ref())
            && self
                .machine
                .as_deref()
                .is_none_or(|machine| row.machine == machine)
            && self
                .harness
                .as_deref()
                .is_none_or(|harness| activity.harness == harness)
            && self
                .model
                .as_deref()
                .is_none_or(|model| activity.model == model)
    }
}

/// A row with its busy intervals as of the answer: a working session's last
/// one runs to now.
struct Live<'a> {
    row: Row<'a>,
    busy: Vec<(i64, i64)>,
}

impl Live<'_> {
    fn activity(&self) -> &Activity {
        self.row.activity
    }

    fn busy_in(&self, from: i64, to: i64) -> i64 {
        self.busy
            .iter()
            .map(|(a, b)| ((*b).min(to) - (*a).max(from)).max(0))
            .sum()
    }
}

fn in_range(at: i64, from: i64, to: i64) -> bool {
    at >= from && at < to
}

/// The whole UTC days a range ending at `to` counts cost on: as many as the
/// range has days, the last one the day `to` falls in (today, so far).
fn cost_span(to: i64, days: i64) -> (i64, i64) {
    let day = to.div_euclid(DAY_MS) * DAY_MS;
    (day - (days - 1) * DAY_MS, day + DAY_MS)
}

/// One session's cost on the whole days of `[from, to)`.
struct Cost {
    usd: Option<f64>,
    unpriced: Vec<String>,
    has_data: bool,
}

fn cost_in(activity: &Activity, (from, to): (i64, i64)) -> Cost {
    let days: Vec<f64> = activity
        .cost_by_day
        .iter()
        .filter(|(day, _)| *day >= from && day + DAY_MS <= to)
        .map(|(_, usd)| *usd)
        .collect();
    let has_data = !days.is_empty();
    let unpriced = if has_data {
        activity.unpriced_models.clone()
    } else {
        Vec::new()
    };
    Cost {
        usd: unpriced.is_empty().then(|| days.iter().sum()),
        unpriced,
        has_data,
    }
}

fn cost_json(usd: Option<f64>, unpriced: &BTreeSet<String>, has_data: bool) -> Value {
    json!({
        "usd": if unpriced.is_empty() { usd } else { None },
        "unpriced_models": unpriced,
        "has_data": has_data,
    })
}

/// The most sessions busy at one moment in `[from, to)`: each session's
/// intervals clipped and merged so it counts once, then a sweep over the
/// starts (+1) and ends (-1), ends first at a shared instant.
fn peak(rows: &[Live], from: i64, to: i64) -> usize {
    let mut events: Vec<(i64, i32)> = Vec::new();
    for row in rows {
        let mut intervals: Vec<(i64, i64)> = row
            .busy
            .iter()
            .map(|(a, b)| ((*a).max(from), (*b).min(to)))
            .filter(|(a, b)| a < b)
            .collect();
        intervals.sort_unstable();
        let mut current: Option<(i64, i64)> = None;
        for (a, b) in intervals {
            match &mut current {
                Some(merged) if a <= merged.1 => merged.1 = merged.1.max(b),
                _ => {
                    if let Some((start, end)) = current {
                        events.extend([(start, 1), (end, -1)]);
                    }
                    current = Some((a, b));
                }
            }
        }
        if let Some((start, end)) = current {
            events.extend([(start, 1), (end, -1)]);
        }
    }
    events.sort_unstable();
    let (mut now, mut best) = (0i64, 0i64);
    for (_, step) in events {
        now += i64::from(step);
        best = best.max(now);
    }
    usize::try_from(best).unwrap_or(0)
}

/// The headline figures for `[from, to)`, and each session's time waited
/// on you in it.
fn period(rows: &[Live], from: i64, to: i64, days: i64) -> (Value, BTreeMap<String, i64>) {
    let mut agent_ms = 0;
    let (mut started, mut turns, mut tools, mut errors) = (0usize, 0usize, 0usize, 0usize);
    let mut waits = Vec::new();
    let mut waited: BTreeMap<String, i64> = BTreeMap::new();
    let (mut usd, mut unpriced, mut has_data) = (0.0, BTreeSet::new(), false);
    let span = cost_span(to, days);
    for row in rows {
        let activity = row.activity();
        agent_ms += row.busy_in(from, to);
        turns += activity
            .turns
            .iter()
            .filter(|at| in_range(**at, from, to))
            .count();
        if in_range(activity.start, from, to) {
            started += 1;
            // The model has a session's totals, not when each call was
            // made: they count where the session started.
            let calls = activity.calls.unwrap_or(0);
            tools += calls;
            errors += activity.errors.unwrap_or(0).min(calls);
        }
        // A message to you still waiting has waited since it was sent.
        for sent in &activity.waits {
            if *sent < to {
                let ms = to - (*sent).max(from);
                if ms > 0 {
                    waits.push(ms);
                    *waited.entry(row.row.id.to_owned()).or_default() += ms;
                }
            }
        }
        let cost = cost_in(activity, span);
        if cost.has_data {
            has_data = true;
            usd += cost.usd.unwrap_or(0.0);
            unpriced.extend(cost.unpriced);
        }
    }
    waits.sort_unstable();
    let median = match waits.len() {
        0 => 0,
        n if n % 2 == 1 => waits[n / 2],
        n => (waits[n / 2 - 1] + waits[n / 2]) / 2,
    };
    let figures = json!({
        "agent_ms": agent_ms,
        "started": started,
        "turns": turns,
        "tools": tools,
        "errors": errors,
        "peak": peak(rows, from, to),
        "wait_ms": waits.iter().sum::<i64>(),
        "median_wait_ms": median,
        "longest_wait_ms": waits.last().copied().unwrap_or(0),
        "cost": cost_json(Some(usd), &unpriced, has_data),
    });
    (figures, waited)
}

/// One UTC day of the cost chart: each harness's cost, and the sessions
/// with a cost on it.
#[derive(Default)]
struct Day<'a> {
    claude: f64,
    codex: f64,
    sessions: Vec<(f64, &'a Live<'a>)>,
}

/// One breakdown row: what it groups by, and its sessions' busy time and
/// cost in the range.
#[derive(Default)]
struct Group {
    fields: Value,
    ms: i64,
    usd: f64,
    unpriced: BTreeSet<String>,
    sessions: usize,
}

/// `row` as of `now`: a working session's last busy interval runs to now.
fn live<'a>(row: &Row<'a>, now: i64) -> Live<'a> {
    let mut busy = row.activity.busy.clone();
    if row.activity.working
        && let Some(last) = busy.last_mut()
    {
        last.1 = last.1.max(now);
    }
    Live { row: *row, busy }
}

/// The first [`TOP`] of `items` as `{sid, ms}`, each named in `named`.
fn listed<'a>(items: &[(i64, &Live<'a>)], named: &mut BTreeSet<&'a str>) -> Vec<Value> {
    items
        .iter()
        .take(TOP)
        .map(|(ms, row)| {
            named.insert(row.row.id);
            json!({ "sid": row.row.id, "ms": ms })
        })
        .collect()
}

/// Sessions by value, most first; ties by name, then id.
fn by_value<T: PartialOrd>(items: &mut [(T, &Live)]) {
    items.sort_by(|(a, x), (b, y)| {
        b.partial_cmp(a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.activity().name.cmp(&y.activity().name))
            .then_with(|| x.row.id.cmp(y.row.id))
    });
}

/// The answer to `request` over `rows` at `now`, for the model at `version`.
pub(crate) fn answer(rows: &[Row], request: &Request, now: i64, version: &str) -> Value {
    let days = request.range.days();
    let (from, to) = (now - days * DAY_MS, now);
    // The Codex allowance and the filters' values don't follow the filters.
    let allowance = rows
        .iter()
        .filter(|row| row.activity.harness == "codex")
        .filter_map(|row| row.activity.rate_limits.as_ref())
        .max_by_key(|limits| limits.recorded_at);
    let span = cost_span(to, days);
    // Only the rows that touch the range or the period before it are
    // walked: a start, busy time, a turn or a cost day in them, or working
    // or waiting on you now.
    let (before, cost_from) = (from - days * DAY_MS, cost_span(from, days).0);
    let touches = |row: &Activity| {
        row.working
            || !row.waits.is_empty()
            || in_range(row.start, before, to)
            || row.busy.iter().any(|(a, b)| *a < to && *b > before)
            || row.turns.iter().any(|at| in_range(*at, before, to))
            || row
                .cost_by_day
                .iter()
                .any(|(day, _)| *day >= cost_from && *day < span.1)
    };
    let all: Vec<Live> = rows
        .iter()
        .filter(|row| touches(row.activity))
        .map(|row| live(row, now))
        .collect();
    let active = |row: &Live| {
        in_range(row.activity().start, from, to)
            || row.busy_in(from, to) > 0
            || cost_in(row.activity(), span).has_data
    };
    let mut repos: BTreeSet<Option<&str>> = BTreeSet::new();
    let mut facets: [BTreeSet<&str>; 3] = Default::default();
    for row in all.iter().filter(|row| active(row)) {
        let activity = row.row.activity;
        repos.insert(activity.repo.as_deref());
        facets[0].insert(row.row.machine);
        facets[1].insert(activity.harness);
        facets[2].insert(&activity.model);
    }
    let rows: Vec<Live> = all
        .into_iter()
        .filter(|row| request.matches(&row.row))
        .collect();

    let (current, waited) = period(&rows, from, to, days);
    let (previous, _) = period(&rows, from - days * DAY_MS, from, days);
    let mut named: BTreeSet<&str> = BTreeSet::new();
    let longest_current = rows
        .iter()
        .flat_map(|row| {
            row.activity()
                .waits
                .iter()
                .map(move |sent| (row, (now - sent).max(0)))
        })
        .max_by(|(x, a), (y, b)| a.cmp(b).then_with(|| y.row.id.cmp(x.row.id)))
        .map(|(row, ms)| {
            named.insert(row.row.id);
            json!({ "sid": row.row.id, "ms": ms })
        });
    let calls_unknown = rows
        .iter()
        .filter(|row| {
            row.activity().calls.is_none()
                && (in_range(row.activity().start, from, to) || row.busy_in(from, to) > 0)
        })
        .count();

    // Agent-hours per column, stacked by harness, with the sessions busy in
    // each for its drill-in.
    let (count, unit) = request.range.columns();
    let width = (to - from) / count;
    let columns: Vec<Value> = (0..count)
        .map(|index| {
            let (a, b) = (from + index * width, from + (index + 1) * width);
            let (mut claude, mut codex) = (0, 0);
            let mut busy: Vec<(i64, &Live)> = Vec::new();
            for row in &rows {
                let ms = row.busy_in(a, b);
                if ms > 0 {
                    match row.activity().harness {
                        "claude" => claude += ms,
                        "codex" => codex += ms,
                        _ => {}
                    }
                    busy.push((ms, row));
                }
            }
            by_value(&mut busy);
            let more = busy.len().saturating_sub(SLICE_MAX);
            busy.truncate(SLICE_MAX);
            let sessions: Vec<Value> = busy
                .iter()
                .map(|(ms, row)| {
                    named.insert(row.row.id);
                    json!({ "sid": row.row.id, "ms": ms })
                })
                .collect();
            json!({
                "from": a, "to": b, "claude_ms": claude, "codex_ms": codex,
                "sessions": sessions, "more": more,
            })
        })
        .collect();

    // Cost per UTC day, stacked by harness: none for 24 h, since cost is
    // recorded per day and an hourly series would put a day in one hour.
    let cost_days = (days > 1).then(|| {
        let (first, _) = span;
        let mut unpriced = BTreeSet::new();
        let mut bins: Vec<Day> = (0..days).map(|_| Day::default()).collect();
        for row in &rows {
            for (day, usd) in &row.activity().cost_by_day {
                if *day < first || day + DAY_MS > span.1 {
                    continue;
                }
                unpriced.extend(row.activity().unpriced_models.iter().cloned());
                let bin = &mut bins[usize::try_from((day - first) / DAY_MS).unwrap_or(0)];
                match row.activity().harness {
                    "claude" => bin.claude += usd,
                    "codex" => bin.codex += usd,
                    _ => {}
                }
                bin.sessions.push((*usd, row));
            }
        }
        let bins: Vec<Value> = bins
            .into_iter()
            .enumerate()
            .map(|(index, bin)| {
                let Day {
                    claude,
                    codex,
                    mut sessions,
                } = bin;
                by_value(&mut sessions);
                let more = sessions.len().saturating_sub(SLICE_MAX);
                sessions.truncate(SLICE_MAX);
                let day = first + i64::try_from(index).unwrap_or(0) * DAY_MS;
                let sessions: Vec<Value> = sessions
                    .iter()
                    .map(|(usd, row)| {
                        named.insert(row.row.id);
                        json!({
                            "sid": row.row.id,
                            "usd": usd,
                            "unpriced_models": row.activity().unpriced_models,
                        })
                    })
                    .collect();
                json!({
                    "from": day, "to": day + DAY_MS, "claude_usd": claude, "codex_usd": codex,
                    "sessions": sessions, "more": more,
                })
            })
            .collect();
        json!({ "from": span.0, "to": span.1, "days": bins, "unpriced_models": unpriced })
    });

    // Busy time and cost by repo, machine, and harness and model: every
    // session busy, started or with a cost in the range.
    let breakdown = |group: fn(&Live) -> Value| {
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for row in &rows {
            let ms = row.busy_in(from, to);
            let cost = cost_in(row.activity(), span);
            if ms == 0 && !in_range(row.activity().start, from, to) && !cost.has_data {
                continue;
            }
            let fields = group(row);
            let entry = groups.entry(fields.to_string()).or_insert_with(|| Group {
                fields,
                ..Group::default()
            });
            entry.ms += ms;
            entry.sessions += 1;
            if cost.has_data {
                entry.usd += cost.usd.unwrap_or(0.0);
                entry.unpriced.extend(cost.unpriced);
            }
        }
        groups
            .into_values()
            .map(|group| {
                let mut fields = group.fields;
                fields["ms"] = json!(group.ms);
                fields["usd"] = json!(group.usd);
                fields["unpriced_models"] = json!(group.unpriced);
                fields["sessions"] = json!(group.sessions);
                fields
            })
            .collect::<Vec<_>>()
    };
    let by_repo = breakdown(|row| json!({ "repo": row.activity().repo }));
    let by_machine = breakdown(|row| json!({ "machine": row.row.machine }));
    let by_model = breakdown(
        |row| json!({ "harness": row.activity().harness, "model": row.activity().model }),
    );

    let mut busy: Vec<(i64, &Live)> = rows
        .iter()
        .map(|row| (row.busy_in(from, to), row))
        .filter(|(ms, _)| *ms > 0)
        .collect();
    by_value(&mut busy);
    let by_id: BTreeMap<&str, &Live> = rows.iter().map(|row| (row.row.id, row)).collect();
    let mut waits: Vec<(i64, &Live)> = waited
        .iter()
        .filter(|(_, ms)| **ms > 0)
        .filter_map(|(id, ms)| Some((*ms, *by_id.get(id.as_str())?)))
        .collect();
    by_value(&mut waits);
    let mut costs: Vec<(f64, &Live, Cost)> = rows
        .iter()
        .map(|row| (row, cost_in(row.activity(), span)))
        .filter(|(_, cost)| cost.has_data)
        .map(|(row, cost)| (cost.usd.unwrap_or(-1.0), row, cost))
        .collect();
    costs.sort_by(|(a, x, _), (b, y, _)| {
        b.partial_cmp(a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.activity().name.cmp(&y.activity().name))
            .then_with(|| x.row.id.cmp(y.row.id))
    });
    let top_busy = listed(&busy, &mut named);
    let top_waited = listed(&waits, &mut named);
    let top_cost: Vec<Value> = costs
        .iter()
        .take(TOP)
        .map(|(_, row, cost)| {
            named.insert(row.row.id);
            json!({ "sid": row.row.id, "usd": cost.usd, "unpriced_models": cost.unpriced })
        })
        .collect();
    let sessions: serde_json::Map<String, Value> = named
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|row| {
            (
                row.row.id.to_owned(),
                json!({ "name": row.activity().name, "harness": row.activity().harness }),
            )
        })
        .collect();

    let mut out = serde_json::Map::new();
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_owned(), value);
    };
    put("range", json!(request.range.name()));
    put("days", json!(days));
    put("version", json!(version));
    put("now", json!(now));
    put("from", json!(from));
    put("to", json!(to));
    put("current", current);
    put("previous", previous);
    put("longest_current_wait", json!(longest_current));
    put("calls_unknown", json!(calls_unknown));
    put("agents", json!({ "unit": unit, "columns": columns }));
    put("cost", json!(cost_days));
    put(
        "breakdown",
        json!({ "repo": by_repo, "machine": by_machine, "model": by_model }),
    );
    put(
        "top",
        json!({ "busy": top_busy, "waited": top_waited, "cost": top_cost }),
    );
    put("sessions", Value::Object(sessions));
    put("allowance", json!(allowance));
    put(
        "facets",
        json!({ "repo": repos, "machine": facets[0], "harness": facets[1], "model": facets[2] }),
    );
    Value::Object(out)
}

/// An answer's `ETag`: a hash of it without the time it was computed at
/// (`now`, and the range's and each agent column's ends, which follow it),
/// so an answer that didn't change keeps its tag. `version` is in it.
pub(crate) fn etag(answer: &Value) -> String {
    let mut stable = answer.clone();
    if let Some(fields) = stable.as_object_mut() {
        for key in ["now", "from", "to"] {
            fields.remove(key);
        }
    }
    if let Some(columns) = stable["agents"]["columns"].as_array_mut() {
        for column in columns.iter_mut().filter_map(Value::as_object_mut) {
            column.remove("from");
            column.remove("to");
        }
    }
    format!("\"a{:016x}\"", fnv(&stable.to_string()))
}

/// A kept answer: its `ETag` and its body.
#[derive(Clone)]
pub(crate) struct Kept {
    pub(crate) etag: String,
    pub(crate) body: Arc<Vec<u8>>,
}

struct Entry {
    key: String,
    version: String,
    at: Instant,
    kept: Kept,
}

/// The answers a core keeps, at most [`CACHE_MAX`], one per range and
/// filters. A kept answer serves until the model it came from changed (and
/// [`SPACING`] has passed), or until it is [`STALE_AFTER`] old. The caller
/// computes a new one without holding the cache.
#[derive(Default)]
pub(crate) struct Cache {
    entries: Vec<Entry>,
}

impl Cache {
    /// The kept answer for `key` that may still answer for the model at
    /// `version`.
    pub(crate) fn kept(&self, key: &str, version: &str) -> Option<Kept> {
        self.entries
            .iter()
            .find(|entry| {
                entry.key == key
                    && (entry.at.elapsed() < SPACING
                        || (entry.version == version && entry.at.elapsed() < STALE_AFTER))
            })
            .map(|entry| entry.kept.clone())
    }

    /// Keeps `kept` for `key`, in place of any older answer for it; the
    /// oldest answer goes when the cache is full.
    pub(crate) fn keep(&mut self, key: &str, version: &str, kept: Kept) {
        self.entries.retain(|entry| entry.key != key);
        if self.entries.len() >= CACHE_MAX
            && let Some(oldest) =
                (0..self.entries.len()).min_by_key(|index| self.entries[*index].at)
        {
            self.entries.remove(oldest);
        }
        self.entries.push(Entry {
            key: key.to_owned(),
            version: version.to_owned(),
            at: Instant::now(),
            kept,
        });
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000_000;
    const HOUR: i64 = 3_600_000;

    fn session(name: &str, harness: &'static str, repo: Option<&str>) -> Activity {
        Activity {
            name: name.into(),
            harness,
            repo: repo.map(str::to_owned),
            model: "Opus 5.5".into(),
            working: false,
            start: 0,
            busy: Vec::new(),
            turns: Vec::new(),
            calls: Some(0),
            errors: Some(0),
            cost_by_day: Vec::new(),
            unpriced_models: Vec::new(),
            rate_limits: None,
            waits: Vec::new(),
        }
    }

    fn day(at: i64) -> i64 {
        at.div_euclid(DAY_MS) * DAY_MS
    }

    fn get(query: &str) -> Request {
        Request::parse(query).unwrap_or_else(|| panic!("{query} is refused"))
    }

    #[test]
    fn only_a_known_range_and_each_filter_once_are_accepted() {
        assert_eq!(get("range=24h").range, Range::Day);
        assert_eq!(get("range=7d").range, Range::Week);
        assert_eq!(get("range=30d").range, Range::Month);
        let filtered = get("range=7d&repo=harbor&machine=laptop&harness=codex&model=GPT%205");
        assert_eq!(filtered.repo, Some(Some("harbor".into())));
        assert_eq!(filtered.model.as_deref(), Some("GPT 5"));
        assert_eq!(get("repo=&range=30d").repo, Some(None));
        let long = "x".repeat(VALUE_MAX);
        assert!(Request::parse(&format!("range=7d&repo={long}")).is_some());
        for query in [
            "",
            "range",
            "range=",
            "range=1d",
            "range=7D",
            "range=7d&range=7d",
            "range=7d&range=30d",
            "range=7d&",
            "&range=7d",
            "range=7d&t=token",
            "range=7d&since=x",
            "range=7d&machine=",
            "range=7d&harness=",
            "range=7d&model=",
            "range=7d&repo=a&repo=b",
            "range=7d&repo=%zz",
            "range=7d&repo=%",
            "range=7d&repo=%ff",
            "repo=harbor",
            &format!("range=7d&repo={long}x"),
        ] {
            assert_eq!(Request::parse(query), None, "{query:?}");
        }
        // One key for one request, whatever the order.
        assert_eq!(
            get("range=7d&repo=a&model=m").key(),
            get("model=m&repo=a&range=7d").key()
        );
        assert_ne!(get("range=7d&repo=").key(), get("range=7d").key());
    }

    #[test]
    fn a_day_parses_to_its_utc_start() {
        assert_eq!(day_start("1970-01-02"), Some(DAY_MS));
        assert_eq!(day_start("2026-09-28"), Some(1_790_553_600_000));
        for bad in ["2026-9-28", "2026-13-01", "20260928", "2026-09-28T"] {
            assert_eq!(day_start(bad), None, "{bad}");
        }
    }

    fn fixture() -> BTreeMap<String, Activity> {
        let mut rows = BTreeMap::new();
        // Today: two hours busy, one turn, three calls (one failed), $4 today.
        let mut today = session("today", "claude", Some("harbor"));
        today.start = NOW - 3 * HOUR;
        today.busy = vec![(NOW - 3 * HOUR, NOW - HOUR)];
        today.turns = vec![NOW - 3 * HOUR];
        (today.calls, today.errors) = (Some(3), Some(1));
        today.cost_by_day = vec![(day(NOW), 4.0)];
        rows.insert("today".into(), today);
        // Five days ago, which a 24 h model leaves out: an hour busy, $2.
        let mut older = session("older", "codex", None);
        older.start = NOW - 5 * DAY_MS;
        older.busy = vec![(NOW - 5 * DAY_MS, NOW - 5 * DAY_MS + HOUR)];
        older.turns = vec![NOW - 5 * DAY_MS, NOW - 5 * DAY_MS + HOUR / 2];
        (older.calls, older.errors) = (Some(2), Some(0));
        older.cost_by_day = vec![(day(NOW - 5 * DAY_MS), 2.0)];
        rows.insert("older".into(), older);
        // Twenty days ago, and a working session waiting on you for an hour.
        let mut month = session("month", "claude", Some("harbor"));
        month.start = NOW - 20 * DAY_MS;
        month.busy = vec![(NOW - 20 * DAY_MS, NOW - 20 * DAY_MS + 3 * HOUR)];
        month.cost_by_day = vec![(day(NOW - 20 * DAY_MS), 10.0)];
        rows.insert("month".into(), month);
        // Ten days ago: the week before the last one.
        let mut before = session("before", "claude", Some("atlas"));
        before.start = NOW - 10 * DAY_MS;
        before.busy = vec![(NOW - 10 * DAY_MS, NOW - 10 * DAY_MS + 4 * HOUR)];
        before.cost_by_day = vec![(day(NOW - 10 * DAY_MS), 1.0)];
        rows.insert("before".into(), before);
        let mut waiting = session("waiting", "claude", Some("atlas"));
        waiting.start = NOW - 2 * HOUR;
        waiting.working = true;
        waiting.busy = vec![(NOW - 2 * HOUR, NOW - HOUR)];
        waiting.waits = vec![NOW - HOUR];
        rows.insert("waiting".into(), waiting);
        rows
    }

    fn at(query: &str, activity: &BTreeMap<String, Activity>) -> Value {
        answer(&rows([("laptop", activity)]), &get(query), NOW, "v1")
    }

    #[test]
    fn a_week_counts_the_session_from_five_days_ago_that_a_day_leaves_out() {
        let activity = fixture();
        let day = at("range=24h", &activity);
        let week = at("range=7d", &activity);
        let month = at("range=30d", &activity);
        // Started: today and waiting in 24 h; with older in 7 d; and month in 30 d.
        assert_eq!(day["current"]["started"], 2);
        assert_eq!(week["current"]["started"], 3);
        assert_eq!(month["current"]["started"], 5);
        // Busy: 2 h today, and waiting's hour plus the hour to now it is working.
        assert_eq!(day["current"]["agent_ms"], 4 * HOUR);
        assert_eq!(week["current"]["agent_ms"], 5 * HOUR);
        assert_eq!(month["current"]["agent_ms"], 12 * HOUR);
        assert_eq!(day["current"]["turns"], 1);
        assert_eq!(week["current"]["turns"], 3);
        assert_eq!(week["current"]["tools"], 5);
        assert_eq!(week["current"]["errors"], 1);
        assert_eq!(day["current"]["cost"]["usd"], 4.0);
        assert_eq!(week["current"]["cost"]["usd"], 6.0);
        assert_eq!(month["current"]["cost"]["usd"], 17.0);
        assert!(day["cost"].is_null());
        assert_eq!(week["cost"]["days"].as_array().unwrap().len(), 7);
        assert_eq!(month["cost"]["days"].as_array().unwrap().len(), 30);
        let spent: f64 = week["cost"]["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bin| bin["claude_usd"].as_f64().unwrap() + bin["codex_usd"].as_f64().unwrap())
            .sum();
        assert_eq!(spent, 6.0);
        // The chart's columns: 24, 28 and 30, adding up to the headline.
        for (result, count) in [(&day, 24), (&week, 28), (&month, 30)] {
            let columns = result["agents"]["columns"].as_array().unwrap();
            assert_eq!(columns.len(), count);
            let busy: i64 = columns
                .iter()
                .map(|c| c["claude_ms"].as_i64().unwrap() + c["codex_ms"].as_i64().unwrap())
                .sum();
            assert_eq!(busy, result["current"]["agent_ms"].as_i64().unwrap());
        }
        // The older session is in the week's lists and named for them.
        let listed: Vec<&str> = week["top"]["busy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["sid"].as_str().unwrap())
            .collect();
        assert_eq!(listed, ["today", "waiting", "older"]);
        assert_eq!(week["sessions"]["older"]["name"], "older");
        assert!(day["sessions"].get("older").is_none());
        let repos: Vec<&Value> = week["breakdown"]["repo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| &group["repo"])
            .collect();
        // Grouped, not ordered: the page sorts by its measure.
        assert_eq!(repos, [&json!("atlas"), &json!("harbor"), &Value::Null]);
        assert_eq!(week["facets"]["harness"], json!(["claude", "codex"]));
        assert_eq!(day["facets"]["harness"], json!(["claude"]));
    }

    #[test]
    fn the_previous_period_is_the_range_before_it() {
        let activity = fixture();
        let week = at("range=7d", &activity);
        // 7 to 14 days ago: before's four hours, its $1 and its start.
        assert_eq!(week["previous"]["agent_ms"], 4 * HOUR);
        assert_eq!(week["previous"]["started"], 1);
        assert_eq!(week["previous"]["cost"]["usd"], 1.0);
        let month = at("range=30d", &activity);
        assert_eq!(month["previous"]["agent_ms"], 0);
        let day = at("range=24h", &activity);
        assert_eq!(day["previous"]["agent_ms"], 0);
        assert_eq!(day["previous"]["started"], 0);
    }

    #[test]
    fn waits_peaks_and_filters() {
        let activity = fixture();
        let day = at("range=24h", &activity);
        assert_eq!(day["current"]["wait_ms"], HOUR);
        assert_eq!(day["current"]["median_wait_ms"], HOUR);
        assert_eq!(
            day["longest_current_wait"],
            json!({ "sid": "waiting", "ms": HOUR })
        );
        assert_eq!(
            day["top"]["waited"],
            json!([{ "sid": "waiting", "ms": HOUR }])
        );
        // today and waiting overlap from 2 h ago to 1 h ago.
        assert_eq!(day["current"]["peak"], 2);
        let harbor = at("range=30d&repo=harbor", &activity);
        assert_eq!(harbor["current"]["started"], 2);
        assert_eq!(harbor["current"]["cost"]["usd"], 14.0);
        assert!(harbor["longest_current_wait"].is_null());
        // The filters' values stay all of the range's.
        assert_eq!(harbor["facets"]["repo"], json!([null, "atlas", "harbor"]));
        let none = at("range=30d&repo=", &activity);
        assert_eq!(none["current"]["started"], 1);
        let codex = at("range=30d&harness=codex&machine=laptop", &activity);
        assert_eq!(codex["current"]["started"], 1);
        assert_eq!(
            at("range=30d&machine=desktop", &activity)["current"]["started"],
            0
        );
        assert_eq!(
            at("range=30d&model=Other", &activity)["current"]["started"],
            0
        );
    }

    /// Cost is recorded per UTC day. A session with $10 yesterday and $3
    /// today: 24 h is today's UTC day alone ($3, against yesterday's $10),
    /// with no hourly series; 7 d counts both, in two daily columns.
    #[test]
    fn a_day_counts_todays_utc_day_and_compares_with_yesterday() {
        let mut activity = BTreeMap::new();
        let mut two = session("two days", "claude", None);
        two.start = NOW - 30 * HOUR;
        two.cost_by_day = vec![(day(NOW) - DAY_MS, 10.0), (day(NOW), 3.0)];
        activity.insert("two".to_owned(), two);
        let today = at("range=24h", &activity);
        assert_eq!(today["current"]["cost"]["usd"], 3.0);
        assert_eq!(today["previous"]["cost"]["usd"], 10.0);
        assert!(today["cost"].is_null());
        let week = at("range=7d", &activity);
        assert_eq!(week["current"]["cost"]["usd"], 13.0);
        let columns: Vec<f64> = week["cost"]["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bin| bin["claude_usd"].as_f64().unwrap())
            .collect();
        assert_eq!(columns, [0.0, 0.0, 0.0, 0.0, 0.0, 10.0, 3.0]);
        assert_eq!(week["cost"]["days"][6]["from"], day(NOW));
        assert_eq!(week["cost"]["days"][6]["sessions"][0]["sid"], "two");
        assert_eq!(week["previous"]["cost"]["has_data"], false);
    }

    #[test]
    fn an_unpriced_model_leaves_the_cost_unknown() {
        let mut activity = fixture();
        activity.get_mut("older").unwrap().unpriced_models = vec!["mystery".into()];
        let week = at("range=7d", &activity);
        assert!(week["current"]["cost"]["usd"].is_null());
        assert_eq!(
            week["current"]["cost"]["unpriced_models"],
            json!(["mystery"])
        );
        assert_eq!(week["cost"]["unpriced_models"], json!(["mystery"]));
        // Today alone has a price.
        assert_eq!(at("range=24h", &activity)["current"]["cost"]["usd"], 4.0);
    }

    #[test]
    fn a_session_two_machines_both_have_counts_once_on_the_first() {
        let first = fixture();
        let second = fixture();
        let rows = rows([("laptop", &first), ("desktop", &second)]);
        assert_eq!(rows.len(), first.len());
        assert!(rows.iter().all(|row| row.machine == "laptop"));
        let mut other = BTreeMap::new();
        other.insert("far".to_owned(), {
            let mut far = session("far", "codex", Some("atlas"));
            far.start = NOW - HOUR;
            far.busy = vec![(NOW - HOUR, NOW)];
            far
        });
        let rows = super::rows([("laptop", &first), ("desktop", &other)]);
        let both = answer(&rows, &get("range=24h"), NOW, "u1");
        assert_eq!(both["current"]["started"], 3);
        assert_eq!(both["facets"]["machine"], json!(["desktop", "laptop"]));
        let desktop = answer(&rows, &get("range=24h&machine=desktop"), NOW, "u1");
        assert_eq!(desktop["current"]["agent_ms"], HOUR);
    }

    #[test]
    fn a_column_lists_at_most_slice_max_sessions_and_counts_the_rest() {
        let mut activity = BTreeMap::new();
        for index in 0..SLICE_MAX + 7 {
            let mut row = session(&format!("s{index:03}"), "claude", None);
            row.start = NOW - HOUR;
            row.busy = vec![(NOW - HOUR, NOW - HOUR / 2)];
            activity.insert(format!("s{index:03}"), row);
        }
        let day = at("range=24h", &activity);
        let column = &day["agents"]["columns"][23];
        assert_eq!(column["sessions"].as_array().unwrap().len(), SLICE_MAX);
        assert_eq!(column["more"], 7);
        assert_eq!(day["top"]["busy"].as_array().unwrap().len(), TOP);
    }

    #[test]
    fn the_cache_keeps_an_answer_per_key_until_its_version_changes() {
        let mut cache = Cache::default();
        let answer = |n: i64| Kept {
            etag: format!("\"{n}\""),
            body: Arc::new(n.to_string().into_bytes()),
        };
        assert!(cache.kept("a", "v1").is_none());
        cache.keep("a", "v1", answer(1));
        assert_eq!(cache.kept("a", "v1").unwrap().etag, "\"1\"");
        // Within SPACING a new version is answered from the kept one.
        assert_eq!(cache.kept("a", "v2").unwrap().etag, "\"1\"");
        assert!(cache.kept("b", "v1").is_none());
        cache.keep("a", "v2", answer(2));
        assert_eq!(cache.kept("a", "v2").unwrap().etag, "\"2\"");
        assert_eq!(cache.len(), 1);
        for key in 0..CACHE_MAX + 3 {
            cache.keep(&key.to_string(), "v1", answer(3));
        }
        assert_eq!(cache.len(), CACHE_MAX);
    }

    #[test]
    fn the_etag_ignores_when_the_answer_was_computed() {
        let activity = fixture();
        let first = at("range=7d", &activity);
        // The same figures computed a little later: only the times differ.
        let mut later = first.clone();
        later["now"] = json!(NOW + 30_000);
        later["from"] = json!(NOW + 30_000 - 7 * DAY_MS);
        later["to"] = json!(NOW + 30_000);
        for column in later["agents"]["columns"].as_array_mut().unwrap() {
            column["from"] = json!(column["from"].as_i64().unwrap() + 30_000);
            column["to"] = json!(column["to"].as_i64().unwrap() + 30_000);
        }
        assert_ne!(first, later);
        assert_eq!(etag(&first), etag(&later));
        // A figure, or the model it came from, changes it.
        let mut busier = later.clone();
        busier["current"]["agent_ms"] = json!(1);
        assert_ne!(etag(&first), etag(&busier));
        let mut moved = later;
        moved["version"] = json!("v2");
        assert_ne!(etag(&first), etag(&moved));
    }

    #[test]
    fn a_filter_no_row_has_is_not_known() {
        let activity = fixture();
        let rows = rows([("laptop", &activity)]);
        for query in [
            "range=7d",
            "range=7d&repo=harbor",
            "range=7d&repo=",
            "range=7d&machine=laptop&harness=codex&model=Opus%205.5",
        ] {
            assert!(get(query).known(&rows), "{query}");
        }
        for query in [
            "range=7d&repo=nowhere",
            "range=7d&machine=desktop",
            "range=7d&harness=other",
            "range=7d&model=Other",
        ] {
            assert!(!get(query).known(&rows), "{query}");
        }
    }

    #[test]
    fn rows_outside_the_range_and_the_one_before_are_not_walked() {
        let mut activity = fixture();
        // Fifty days ago: outside 7 d and the week before, inside 30 d's before.
        let mut old = session("old", "claude", Some("ledger"));
        old.start = NOW - 50 * DAY_MS;
        old.busy = vec![(NOW - 50 * DAY_MS, NOW - 50 * DAY_MS + HOUR)];
        activity.insert("old".into(), old);
        let week = at("range=7d", &activity);
        assert_eq!(week["facets"]["repo"], json!([null, "atlas", "harbor"]));
        assert_eq!(week["previous"]["agent_ms"], 4 * HOUR);
        let month = at("range=30d", &activity);
        assert_eq!(month["previous"]["agent_ms"], HOUR);
        assert_eq!(month["previous"]["started"], 1);
    }
}
