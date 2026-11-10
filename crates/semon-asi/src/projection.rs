//! Pure admitted-evidence projection into the ASI schema.
//!
//! This module does not know about SQLite, native logs, rules, or pricing
//! tables.  It only reshapes facts supplied by [`crate::input`] and validates
//! every resulting row against the current schema declaration.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    error::{Error, Result},
    input::{
        Activity, AdmittedSession, ContextBand, Price, PriceUnit, RateKind, Step, Trigger,
        UsageOwner, WasteKind,
    },
    schema::{self, Type},
};

/// One typed schema cell. `None` remains distinct from zero and an empty
/// string, so callers cannot accidentally turn an unrecorded value into one.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Text(Option<String>),
    Integer(Option<i64>),
    Float(Option<f64>),
    Boolean(Option<bool>),
    Timestamp(Option<i64>),
}

pub type Row = BTreeMap<&'static str, Cell>;

/// Whole rows for one admitted session, keyed by declared table name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectedSession {
    pub tables: BTreeMap<&'static str, Vec<Row>>,
}

/// Project a complete admitted session without classifying, repricing, native
/// reads, model calls, or export-policy evaluation.
pub fn project(session: &AdmittedSession) -> Result<ProjectedSession> {
    let mut output = ProjectedSession::default();
    let date_rows = dates(session);

    for step in &session.steps {
        let timestamp = step.timestamp;
        add(
            &mut output,
            "fact_step",
            step_row(session, step, timestamp)?,
        )?;
        for call in &step.tool_calls {
            add(
                &mut output,
                "fact_tool_call",
                row([
                    ("session_uid", text(&session.session_uid)),
                    ("step_uid", text(&step.step_uid)),
                    ("ord", int_u64(call.ord)?),
                    ("ts", time(timestamp)),
                    ("tool_key", text(&call.tool)),
                    ("target_path", text_opt(call.target_path.as_deref())),
                    ("command_tag", text_opt(call.command_tag.as_deref())),
                    ("ok", boolean(call.ok)),
                    ("exit_code", int_opt(call.exit_code.map(i64::from))),
                    ("tool_ms", int_opt_u64(call.tool_ms)?),
                ]),
            )?;
            add(
                &mut output,
                "dim_tool",
                row([
                    ("tool_key", text(&call.tool)),
                    ("display_name", text(&call.tool)),
                ]),
            )?;
        }
        add(
            &mut output,
            "dim_model",
            row([
                ("model_key", text(&step.model.key)),
                ("model", text(&step.model.name)),
                ("provider", text_opt(step.model.provider.as_deref())),
            ]),
        )?;
    }

    for turn in turns(session)? {
        add(&mut output, "fact_turn", turn)?;
    }
    for compact in &session.compactions {
        add(
            &mut output,
            "fact_compaction",
            row([
                ("compaction_uid", text(&compact.compaction_uid)),
                ("session_uid", text(&session.session_uid)),
                ("step_uid", text_opt(compact.step_uid.as_deref())),
                ("ts", time(compact.timestamp)),
            ]),
        )?;
    }

    add(&mut output, "fact_session", session_row(session)?)?;
    add(
        &mut output,
        "dim_machine",
        row([
            ("machine_key", text(&session.session.machine)),
            ("display_name", text(&session.session.machine)),
        ]),
    )?;
    add(
        &mut output,
        "dim_harness",
        row([
            ("harness_key", text(&session.session.harness)),
            ("display_name", text(&session.session.harness)),
        ]),
    )?;
    add(
        &mut output,
        "dim_session",
        row([
            ("session_uid", text(&session.session_uid)),
            ("machine_key", text(&session.session.machine)),
            ("harness_key", text(&session.session.harness)),
            ("role", text(role(session.session.role))),
            (
                "parent_session_uid",
                text_opt(session.session.parent_session_uid.as_deref()),
            ),
            (
                "parent_method",
                text(parent_method(session.session.parent_method)),
            ),
            (
                "lane_root_uid",
                text_opt(session.session.lane_root_uid.as_deref()),
            ),
            ("repo_key", text(&session.session.repo.key)),
            ("branch", text_opt(session.session.branch.as_deref())),
            ("status", text(status(session.session.status))),
            ("started_at", time(session.session.started_at)),
            ("ended_at", time(session.session.ended_at)),
        ]),
    )?;
    add(
        &mut output,
        "dim_repo",
        row([
            ("repo_key", text(&session.session.repo.key)),
            ("repo_id", text_opt(session.session.repo.repo_id.as_deref())),
            (
                "repo_path",
                text_opt(session.session.repo.local_path.as_deref()),
            ),
            ("display_name", text(&session.session.repo.display_name)),
        ]),
    )?;
    for date in date_rows {
        add(&mut output, "dim_date", date)?;
    }
    for price in &session.prices {
        add(&mut output, "dim_price", price_row(price)?)?;
    }
    deduplicate_dimensions(&mut output)?;
    Ok(output)
}

fn step_row(session: &AdmittedSession, step: &Step, timestamp: Option<i64>) -> Result<Row> {
    Ok(row([
        ("step_uid", text(&step.step_uid)),
        ("turn_uid", text(&step.turn_uid)),
        ("session_uid", text(&session.session_uid)),
        ("model_key", text(&step.model.key)),
        ("repo_key", text(&session.session.repo.key)),
        ("date_key", int_opt(timestamp.map(date_key))),
        ("ts", time(timestamp)),
        ("input_tokens", int_u64(step.billing.input_tokens)?),
        ("output_tokens", int_u64(step.billing.output_tokens)?),
        (
            "cache_read_tokens",
            int_u64(step.billing.cache_read_tokens)?,
        ),
        (
            "cache_write_5m_tokens",
            int_u64(step.billing.cache_write_5m_tokens)?,
        ),
        (
            "cache_write_1h_tokens",
            int_u64(step.billing.cache_write_1h_tokens)?,
        ),
        (
            "reasoning_tokens",
            int_opt_u64(step.billing.reasoning_tokens)?,
        ),
        (
            "web_search_requests",
            int_u64(step.billing.web_search_requests)?,
        ),
        ("speed", text_opt(step.billing.speed.as_deref())),
        (
            "service_tier",
            text_opt(step.billing.service_tier.as_deref()),
        ),
        (
            "prompt_size_tokens",
            int_u64(step.billing.prompt_size_tokens)?,
        ),
        ("split_unknown", boolean(Some(step.billing.split_unknown))),
        ("usage_owner", text(usage_owner(step.usage_owner))),
        ("context_tokens", int_u64(step.context_tokens)?),
        ("context_band", text(context_band(step.context_band))),
        ("model_ms", int_opt_u64(step.model_ms)?),
        ("tool_ms", int_opt_u64(step.tool_ms)?),
        ("usd_est", float(step.cost.usd_est)),
        (
            "input_usd",
            float(step.cost.components.get("input").copied()),
        ),
        (
            "cache_read_usd",
            float(step.cost.components.get("cache_read").copied()),
        ),
        (
            "cache_write_usd",
            float(step.cost.components.get("cache_write").copied()),
        ),
        (
            "output_usd",
            float(step.cost.components.get("output").copied()),
        ),
        (
            "price_status",
            text(if step.cost.usd_est.is_some() {
                "priced"
            } else {
                "unpriced"
            }),
        ),
        ("price_version", text(&step.cost.price_version)),
        (
            "activity",
            text(step.activity.map(activity).unwrap_or("unknown")),
        ),
        ("waste_kind", text_opt(step.waste_kind.map(waste_kind))),
        ("trigger", text_opt(step.trigger.map(trigger))),
        ("ok", boolean(step.ok)),
    ]))
}

fn session_row(session: &AdmittedSession) -> Result<Row> {
    let exclusive: Vec<_> = session
        .steps
        .iter()
        .filter(|step| step.usage_owner == UsageOwner::Exclusive)
        .collect();
    let token = |select: fn(&Step) -> u64| -> Result<Cell> {
        int_u64(sum_u64(exclusive.iter().copied().map(select))?)
    };
    let priced = sum_float(exclusive.iter().filter_map(|step| step.cost.usd_est));
    let all_priced = exclusive.iter().all(|step| step.cost.usd_est.is_some());
    let model_ms = complete_sum(session.steps.iter().map(|step| step.model_ms))?;
    let reasoning_tokens =
        complete_sum(exclusive.iter().map(|step| step.billing.reasoning_tokens))?;
    let failure_count = session
        .steps
        .iter()
        .filter(|step| step.ok == Some(false))
        .count();
    let wall_ms = match (session.session.started_at, session.session.ended_at) {
        (Some(start), Some(end)) if end >= start => Some((end - start) as u64),
        _ => None,
    };
    Ok(row([
        ("session_uid", text(&session.session_uid)),
        (
            "date_key",
            int_opt(session.session.started_at.map(date_key)),
        ),
        ("started_at", time(session.session.started_at)),
        ("ended_at", time(session.session.ended_at)),
        ("input_tokens", token(|step| step.billing.input_tokens)?),
        ("output_tokens", token(|step| step.billing.output_tokens)?),
        (
            "cache_read_tokens",
            token(|step| step.billing.cache_read_tokens)?,
        ),
        (
            "cache_write_5m_tokens",
            token(|step| step.billing.cache_write_5m_tokens)?,
        ),
        (
            "cache_write_1h_tokens",
            token(|step| step.billing.cache_write_1h_tokens)?,
        ),
        ("reasoning_tokens", int_opt_u64(reasoning_tokens)?),
        (
            "web_search_requests",
            token(|step| step.billing.web_search_requests)?,
        ),
        ("usd_est", float(all_priced.then_some(priced))),
        ("reported_usd", float(session.session.reported_usd)),
        ("wall_ms", int_opt_u64(wall_ms)?),
        ("model_ms", int_opt_u64(model_ms)?),
        ("step_count", int_u64(session.steps.len() as u64)?),
        ("failure_count", int_u64(failure_count as u64)?),
        ("usage_complete", boolean(Some(session.usage_complete))),
        ("source_status", text("complete")),
        ("computation_status", text("current")),
        (
            "exclusive_step_count",
            int_u64(session.versions.coverage.exclusive_step_count)?,
        ),
        (
            "unknown_step_count",
            int_u64(session.versions.coverage.unknown_step_count)?,
        ),
        (
            "unclassified_step_count",
            int_u64(session.versions.coverage.unclassified_step_count)?,
        ),
        (
            "taxonomy_version",
            int_opt(session.versions.taxonomy.map(i64::from)),
        ),
        (
            "transform_version",
            int_u64(u64::from(session.versions.transform))?,
        ),
        ("rules_version", int_u64(u64::from(session.versions.rules))?),
        ("price_version", text(&session.versions.price)),
    ]))
}

fn turns(session: &AdmittedSession) -> Result<Vec<Row>> {
    let mut turns = BTreeMap::<&str, (Option<i64>, Option<i64>, u64)>::new();
    for step in &session.steps {
        let entry = turns.entry(&step.turn_uid).or_insert((None, None, 0));
        entry.0 = min_time(entry.0, step.timestamp);
        entry.1 = max_time(entry.1, step.timestamp);
        entry.2 += 1;
    }
    turns
        .into_iter()
        .map(|(uid, (started, ended, count))| {
            Ok(row([
                ("turn_uid", text(uid)),
                ("session_uid", text(&session.session_uid)),
                ("initiator", text("unknown")),
                ("started_at", time(started)),
                ("ended_at", time(ended)),
                ("step_count", int_u64(count)?),
            ]))
        })
        .collect()
}

fn dates(session: &AdmittedSession) -> Vec<Row> {
    let mut days = BTreeSet::new();
    days.extend(
        session
            .session
            .started_at
            .map(|time| time.div_euclid(86_400_000)),
    );
    days.extend(
        session
            .steps
            .iter()
            .filter_map(|step| step.timestamp.map(|time| time.div_euclid(86_400_000))),
    );
    days.into_iter()
        .map(|day_number| {
            let (year, month, day) = civil(day_number);
            let weekday = (day_number + 3).rem_euclid(7) + 1;
            row([
                ("date_key", int(Some(year * 10_000 + month * 100 + day))),
                ("day_start", time(Some(day_number * 86_400_000))),
                ("year", int(Some(year))),
                ("month", int(Some(month))),
                ("day", int(Some(day))),
                ("iso_week", int(Some(iso_week(year, month, day, weekday)))),
                ("weekday", int(Some(weekday))),
            ])
        })
        .collect()
}

fn price_row(price: &Price) -> Result<Row> {
    Ok(row([
        ("price_version", text(&price.price_version)),
        ("provider", text(&price.provider)),
        ("model", text(&price.model)),
        ("rate_kind", text(rate_kind(price.rate_kind))),
        ("tier", text_opt(price.tier.as_deref())),
        ("speed", text_opt(price.speed.as_deref())),
        ("service_tier", text_opt(price.service_tier.as_deref())),
        ("threshold_tokens", int_opt_u64(price.threshold_tokens)?),
        ("unit", text(price_unit(price.unit))),
        ("usd_per_unit", float(Some(price.usd_per_unit))),
        ("valid_from", time(Some(price.valid_from))),
        ("valid_to", time(price.valid_to)),
        ("learned_at", time(Some(price.learned_at))),
    ]))
}

fn add(output: &mut ProjectedSession, table: &'static str, row: Row) -> Result<()> {
    validate(table, &row)?;
    output.tables.entry(table).or_default().push(row);
    Ok(())
}

fn deduplicate_dimensions(output: &mut ProjectedSession) -> Result<()> {
    for table in schema::TABLES {
        if table.kind != schema::Kind::Dimension {
            continue;
        }
        let Some(rows) = output.tables.get_mut(table.name) else {
            continue;
        };
        let mut unique = Vec::with_capacity(rows.len());
        for row in rows.drain(..) {
            let duplicate = if table.name == "dim_price" {
                unique.iter().position(|old: &Row| old == &row)
            } else {
                let key = row
                    .get(table.columns[0].name)
                    .ok_or_else(|| Error::Invalid(format!("missing {} key", table.name)))?;
                unique
                    .iter()
                    .position(|old: &Row| old.get(table.columns[0].name) == Some(key))
            };
            match duplicate {
                Some(index) if unique[index] == row => {}
                Some(_) => {
                    return Err(Error::Invalid(format!(
                        "conflicting {} dimension",
                        table.name
                    )));
                }
                None => unique.push(row),
            }
        }
        *rows = unique;
    }
    Ok(())
}

fn validate(table_name: &str, row: &Row) -> Result<()> {
    let table = schema::table(table_name)
        .ok_or_else(|| Error::Invalid(format!("unknown projection table {table_name}")))?;
    if row.len() != table.columns.len() {
        return Err(Error::Invalid(format!("incomplete row for {table_name}")));
    }
    for column in table.columns {
        let value = row
            .get(column.name)
            .ok_or_else(|| Error::Invalid(format!("missing {}.{}", table_name, column.name)))?;
        let (kind, null) = match value {
            Cell::Text(value) => (Type::Text, value.is_none()),
            Cell::Integer(value) => (Type::Integer, value.is_none()),
            Cell::Float(value) => (Type::Float, value.is_none()),
            Cell::Boolean(value) => (Type::Boolean, value.is_none()),
            Cell::Timestamp(value) => (Type::Timestamp, value.is_none()),
        };
        if kind != column.data_type || (null && !column.nullable) {
            return Err(Error::Invalid(format!(
                "invalid {}.{}",
                table_name, column.name
            )));
        }
        if let Cell::Text(Some(value)) = value
            && !column.values.is_empty()
            && !column.values.contains(&value.as_str())
        {
            return Err(Error::Invalid(format!(
                "invalid enum {table_name}.{}",
                column.name
            )));
        }
    }
    Ok(())
}

fn row<const N: usize>(values: [(&'static str, Cell); N]) -> Row {
    values.into_iter().collect()
}
fn text(value: &str) -> Cell {
    Cell::Text(Some(value.into()))
}
fn text_opt(value: Option<&str>) -> Cell {
    Cell::Text(value.map(Into::into))
}
fn int(value: Option<i64>) -> Cell {
    Cell::Integer(value)
}
fn int_opt(value: Option<i64>) -> Cell {
    Cell::Integer(value)
}
fn float(value: Option<f64>) -> Cell {
    Cell::Float(value)
}
fn boolean(value: Option<bool>) -> Cell {
    Cell::Boolean(value)
}
fn time(value: Option<i64>) -> Cell {
    Cell::Timestamp(value)
}
fn int_u64(value: u64) -> Result<Cell> {
    int_opt_u64(Some(value))
}
fn int_opt_u64(value: Option<u64>) -> Result<Cell> {
    value
        .map(|value| i64::try_from(value).map_err(|_| Error::Invalid("integer overflow".into())))
        .transpose()
        .map(Cell::Integer)
}

fn sum_u64(mut values: impl Iterator<Item = u64>) -> Result<u64> {
    values.try_fold(0_u64, |sum, value| {
        sum.checked_add(value)
            .ok_or_else(|| Error::Invalid("integer overflow".into()))
    })
}
fn sum_float(values: impl Iterator<Item = f64>) -> f64 {
    values.sum()
}
fn complete_sum(values: impl Iterator<Item = Option<u64>>) -> Result<Option<u64>> {
    let mut sum = 0_u64;
    for value in values {
        let Some(value) = value else {
            return Ok(None);
        };
        sum = sum
            .checked_add(value)
            .ok_or_else(|| Error::Invalid("integer overflow".into()))?;
    }
    Ok(Some(sum))
}
fn min_time(current: Option<i64>, value: Option<i64>) -> Option<i64> {
    current.into_iter().chain(value).min()
}
fn max_time(current: Option<i64>, value: Option<i64>) -> Option<i64> {
    current.into_iter().chain(value).max()
}

fn date_key(timestamp: i64) -> i64 {
    let (year, month, day) = civil(timestamp.div_euclid(86_400_000));
    year * 10_000 + month * 100 + day
}
fn civil(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}
fn iso_week(year: i64, month: i64, day: i64, weekday: i64) -> i64 {
    let ordinal = ordinal(year, month, day);
    let week = (ordinal - weekday + 10).div_euclid(7);
    if week < 1 {
        iso_weeks(year - 1)
    } else if week > iso_weeks(year) {
        1
    } else {
        week
    }
}
fn ordinal(year: i64, month: i64, day: i64) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let before = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    before[(month - 1) as usize] + day + i64::from(leap && month > 2)
}
fn iso_weeks(year: i64) -> i64 {
    let weekday = (days(year, 1, 1) + 3).rem_euclid(7) + 1;
    if weekday == 4 || (weekday == 3 && ordinal(year, 12, 31) == 366) {
        53
    } else {
        52
    }
}
fn days(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn usage_owner(value: UsageOwner) -> &'static str {
    match value {
        UsageOwner::Exclusive => "exclusive",
        UsageOwner::Copied => "copied",
    }
}
fn context_band(value: ContextBand) -> &'static str {
    match value {
        ContextBand::Under50k => "under_50k",
        ContextBand::From50kTo150k => "50k_to_150k",
        ContextBand::Over150k => "over_150k",
    }
}
fn activity(value: Activity) -> &'static str {
    match value {
        Activity::Coordinate => "coordinate",
        Activity::Wait => "wait",
        Activity::Integrate => "integrate",
        Activity::Verify => "verify",
        Activity::Recover => "recover",
        Activity::Implement => "implement",
        Activity::Explore => "explore",
        Activity::Setup => "setup",
        Activity::Report => "report",
        Activity::Think => "think",
    }
}
fn waste_kind(value: WasteKind) -> &'static str {
    match value {
        WasteKind::Wait => "wait",
        WasteKind::FailedCall => "failed_call",
        WasteKind::Retry => "retry",
        WasteKind::RedundantRead => "redundant_read",
        WasteKind::ContextRefill => "context_refill",
    }
}
fn trigger(value: Trigger) -> &'static str {
    match value {
        Trigger::ToolError => "tool_error",
        Trigger::EmptyResult => "empty_result",
        Trigger::TestOrCiFailure => "test_or_ci_failure",
        Trigger::ExternalError => "external_error",
        Trigger::Compaction => "compaction",
        Trigger::Polling => "polling",
        Trigger::WorkerResult => "worker_result",
        Trigger::InstructionChange => "instruction_change",
        Trigger::MissingInput => "missing_input",
        Trigger::NoneVisible => "none_visible",
    }
}
fn role(value: crate::input::Role) -> &'static str {
    match value {
        crate::input::Role::Manager => "manager",
        crate::input::Role::Worker => "worker",
        crate::input::Role::Reviewer => "reviewer",
        crate::input::Role::Solo => "solo",
        crate::input::Role::Unknown => "unknown",
    }
}
fn parent_method(value: crate::input::ParentMethod) -> &'static str {
    match value {
        crate::input::ParentMethod::Native => "native",
        crate::input::ParentMethod::Workspace => "workspace",
        crate::input::ParentMethod::None => "none",
    }
}
fn status(value: crate::input::Status) -> &'static str {
    match value {
        crate::input::Status::Active => "active",
        crate::input::Status::Ended => "ended",
        crate::input::Status::Unknown => "unknown",
    }
}
fn rate_kind(value: RateKind) -> &'static str {
    match value {
        RateKind::Input => "input",
        RateKind::Output => "output",
        RateKind::CacheRead => "cache_read",
        RateKind::CacheWrite5m => "cache_write_5m",
        RateKind::CacheWrite1h => "cache_write_1h",
        RateKind::Reasoning => "reasoning",
        RateKind::WebSearchRequest => "web_search_request",
    }
}
fn price_unit(value: PriceUnit) -> &'static str {
    match value {
        PriceUnit::Mtok => "mtok",
        PriceUnit::Request => "request",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::input::{
        AdmittedSession, BillingUsage, Coverage, Model, ParentMethod, RecordedCost,
        RecordedVersions, Repo, Role, Session, SourceWitness, Status, Step, UsageOwner,
    };

    fn session(steps: Vec<Step>) -> AdmittedSession {
        AdmittedSession {
            session_uid: "machine/codex/session".into(),
            source_witness: SourceWitness {
                generation: "complete".into(),
                sources: Vec::new(),
            },
            versions: RecordedVersions {
                transform: 40,
                rules: 10,
                price: "historical-price".into(),
                taxonomy: None,
                coverage: Coverage::default(),
            },
            session: Session {
                machine: "machine".into(),
                harness: "codex".into(),
                native_session_id: "session".into(),
                role: Role::Solo,
                parent_session_uid: None,
                parent_method: ParentMethod::None,
                lane_root_uid: None,
                repo: Repo {
                    key: "repo".into(),
                    repo_id: Some("github.com/example/repo".into()),
                    local_path: None,
                    display_name: "example/repo".into(),
                },
                branch: None,
                status: Status::Ended,
                started_at: Some(1_700_000_000_000),
                ended_at: Some(1_700_000_001_000),
                reported_usd: None,
            },
            steps,
            compactions: Vec::new(),
            usage_complete: true,
            prices: vec![Price {
                price_version: "historical-price".into(),
                provider: "openai".into(),
                model: "gpt-test".into(),
                rate_kind: RateKind::Input,
                tier: None,
                speed: None,
                service_tier: None,
                threshold_tokens: None,
                unit: PriceUnit::Mtok,
                usd_per_unit: 1.0,
                valid_from: 1_700_000_000_000,
                valid_to: None,
                learned_at: 1_700_000_000_000,
            }],
        }
    }

    fn step(owner: UsageOwner, input: u64, timestamp: Option<i64>) -> Step {
        Step {
            step_uid: format!("machine/codex/session/s/{input}"),
            turn_uid: "machine/codex/session/t/turn".into(),
            model: Model {
                key: "gpt-test".into(),
                name: "gpt-test".into(),
                provider: Some("openai".into()),
            },
            timestamp,
            billing: BillingUsage {
                input_tokens: input,
                ..BillingUsage::default()
            },
            usage_owner: owner,
            context_tokens: input,
            context_band: ContextBand::Under50k,
            model_ms: Some(10),
            tool_ms: None,
            cost: RecordedCost {
                usd_est: Some(input as f64),
                price_version: "historical-price".into(),
                components: BTreeMap::new(),
            },
            activity: None,
            waste_kind: None,
            trigger: None,
            ok: None,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn projection_keeps_copied_usage_out_of_session_totals() {
        let projection = project(&session(vec![
            step(UsageOwner::Exclusive, 3, Some(1_700_000_000_000)),
            step(UsageOwner::Copied, 99, Some(1_700_000_000_100)),
        ]))
        .unwrap();
        let aggregate = &projection.tables["fact_session"][0];
        assert_eq!(aggregate["input_tokens"], Cell::Integer(Some(3)));
        assert_eq!(aggregate["usd_est"], Cell::Float(Some(3.0)));
        assert_eq!(
            projection.tables["fact_step"][1]["usage_owner"],
            Cell::Text(Some("copied".into()))
        );
        assert_eq!(
            projection.tables["dim_price"][0]["price_version"],
            Cell::Text(Some("historical-price".into()))
        );
    }

    #[test]
    fn unknown_activity_and_time_stay_explicit() {
        let projection = project(&session(vec![step(UsageOwner::Exclusive, 0, None)])).unwrap();
        let step = &projection.tables["fact_step"][0];
        assert_eq!(step["activity"], Cell::Text(Some("unknown".into())));
        assert_eq!(step["ts"], Cell::Timestamp(None));
        assert_eq!(step["date_key"], Cell::Integer(None));
    }
}
