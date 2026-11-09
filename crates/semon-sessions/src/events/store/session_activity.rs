//! Source-qualified activity facts and generation-bound SQLite publication.
//! Native bodies never enter these rows. Pricing inputs retain only typed usage.
use super::*;
use crate::events::session_activity::{
    self as activity, ActivityStep, ActivityStepId, SessionActivity, SourcePosition, Usage,
    UsageStep,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_activity_projections (
 session_key TEXT PRIMARY KEY, generation TEXT NOT NULL,
 source_generation TEXT NOT NULL, rules_version INTEGER NOT NULL,
 pricing_fingerprint TEXT NOT NULL, total INTEGER NOT NULL,
 compactions INTEGER NOT NULL, thrash INTEGER NOT NULL, summary TEXT NOT NULL,
 preview TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS session_activity_rules ON session_activity_projections(rules_version,session_key);
CREATE INDEX IF NOT EXISTS session_activity_pricing ON session_activity_projections(pricing_fingerprint,session_key);
CREATE TABLE IF NOT EXISTS session_activity_sources (
 session_key TEXT PRIMARY KEY, metadata TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS session_activity_steps (
 session_key TEXT NOT NULL, generation TEXT NOT NULL, ordinal INTEGER NOT NULL,
 pricing_fingerprint TEXT NOT NULL, metadata TEXT NOT NULL,
 waste_kind TEXT, waste_category TEXT, empty_search INTEGER NOT NULL,
 PRIMARY KEY(session_key,generation,ordinal)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_activity_compactions (
 session_key TEXT NOT NULL, generation TEXT NOT NULL, ordinal INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(session_key,generation,ordinal)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_activity_facts (
 session_key TEXT NOT NULL, generation TEXT NOT NULL, identity TEXT NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(session_key,generation,identity)
) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    pub(crate) activity: SessionActivity,
    // Kept outside Catalog JSON and keyed by the same exact native step identity.
    inputs: BTreeMap<activity::ActivityStepId, Usage>,
    facts: BTreeMap<activity::ActivityStepId, activity::StepFacts>,
}

impl Projection {
    pub(crate) fn derive(
        key: String,
        usage: Vec<UsageStep>,
        shared: &crate::claude_usage::Shared,
        signals: Vec<(String, Signal)>,
        observations: Vec<activity::classify::LocatedObservation>,
        input_events: Vec<activity::TimedInput>,
        table: Arc<crate::pricing::EffectiveTable>,
    ) -> Self {
        let mut inputs = BTreeMap::new();
        let mut turn_ids = BTreeMap::new();
        for row in &usage {
            match &row.id {
                activity::ActivityStepId::Claude(_) => {
                    inputs.insert(row.id.clone(), row.usage.clone());
                    turn_ids.insert(row.id.clone(), row.turn_id.clone());
                }
                activity::ActivityStepId::Codex(_) => {
                    inputs
                        .entry(row.id.clone())
                        .or_insert_with(|| row.usage.clone());
                    turn_ids
                        .entry(row.id.clone())
                        .or_insert_with(|| row.turn_id.clone());
                }
            }
        }
        let mut activity = activity::derive(key, usage, shared, signals, table);
        let observations = activity::apply_observations(&mut activity, observations);
        let facts = derive_facts(&activity, &inputs, &turn_ids, &observations, &input_events);
        Self {
            activity,
            inputs,
            facts,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SavedStep {
    step: ActivityStep,
    usage: Usage,
    facts: activity::StepFacts,
}

fn billing(usage: &Usage) -> crate::events::BillingUsage {
    match usage {
        Usage::Claude(usage) => usage.billing.clone(),
        Usage::Codex(event) => crate::events::BillingUsage {
            input: event.tokens.input,
            output: event.tokens.output,
            cache_read: event.tokens.cache_read,
            prompt_size: event.tokens.input.saturating_add(event.tokens.cache_read),
            timestamp: event.timestamp,
            ..Default::default()
        },
    }
}

/// Model time is measured only from the latest preceding user prompt or tool
/// result with a timestamp in the same physical source to the step's native
/// usage record. Absent or reversed timestamps remain unknown.
fn model_ms(
    step: &ActivityStep,
    observations: &[activity::classify::LocatedObservation],
    input_events: &[activity::TimedInput],
) -> Option<u64> {
    observations
        .iter()
        .filter(|row| {
            row.position.source == step.position.source
                && row.position.offset < step.position.offset
        })
        .filter_map(|row| match row.result_offset {
            Some(offset) if offset < step.position.offset => row.result_timestamp,
            _ => None,
        })
        .chain(
            input_events
                .iter()
                .filter(|input| {
                    input.position.source == step.position.source
                        && input.position.offset < step.position.offset
                })
                .filter_map(|input| input.timestamp),
        )
        .max()
        .zip(step.timestamp)
        .and_then(|(start, end)| (end >= start).then_some((end - start) as u64))
}

fn derive_facts(
    activity: &SessionActivity,
    inputs: &BTreeMap<ActivityStepId, Usage>,
    turn_ids: &BTreeMap<ActivityStepId, Option<String>>,
    observations: &[activity::classify::LocatedObservation],
    input_events: &[activity::TimedInput],
) -> BTreeMap<ActivityStepId, activity::StepFacts> {
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
    let mut calls: Vec<Vec<activity::ToolCallFact>> = vec![Vec::new(); activity.steps.len()];
    for row in observations {
        let Some(tool) = &row.tool else {
            continue;
        };
        let Some(owner) = activity::observation_owner(&exact, &sources, row) else {
            continue;
        };
        let tool_ms = row
            .timestamp
            .zip(row.result_timestamp)
            .and_then(|(start, end)| (end >= start).then_some((end - start) as u64));
        let ord = calls[owner].len() as u64;
        calls[owner].push(activity::ToolCallFact {
            ord,
            tool: tool.tool.clone(),
            target_path: tool.target_path.clone(),
            command_tag: row.observation.command_tag(),
            ok: row.observation.ok(),
            exit_code: row.observation.exit_code(),
            tool_ms,
        });
    }
    activity
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let usage = inputs.get(&step.id).expect("step has normalized usage");
            let tool_ms = calls[index]
                .iter()
                .filter_map(|call| call.tool_ms)
                .reduce(u64::saturating_add);
            let turn_id = turn_ids.get(&step.id).cloned().flatten();
            let usage_owner = match &step.ownership {
                activity::Ownership::Exclusive => activity::UsageOwner::Exclusive,
                activity::Ownership::Shared(_) => activity::UsageOwner::Copied,
            };
            (
                step.id.clone(),
                activity::StepFacts {
                    turn_id,
                    usage_owner,
                    billing: billing(usage),
                    model_ms: model_ms(step, observations, input_events),
                    tool_ms,
                    tool_calls: std::mem::take(&mut calls[index]),
                },
            )
        })
        .collect()
}

/// Only model-accepted observations from the exact committed native receipt.
/// The payload is the normalized typed payload, not a copied native record.
pub(super) fn accepted_codex(
    c: &Connection,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Vec<UsageStep>> {
    if !native_usage::normalized(c, path, ledger)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let id = native_usage::source(c, path)?
        .ok_or(rusqlite::Error::InvalidQuery)?
        .0;
    let mut statement = c.prepare("SELECT native_offset,identity,payload FROM native_usage_facts WHERE source_id=?1 AND model_outcome='accepted' ORDER BY native_offset")?;
    let mut rows = statement.query([id])?;
    let mut steps = Vec::new();
    while let Some(row) = rows.next()? {
        crate::selected_read::row(row, 256)?;
        let identity: [String; 4] = from_json(1, &row.get::<_, String>(1)?)?;
        let payload: native_usage::Payload = from_json(2, &row.get::<_, String>(2)?)?;
        steps.push(UsageStep::normalized_codex(
            identity,
            SourcePosition {
                source: path.into(),
                offset: uint(row.get(0)?),
            },
            CodexUsageEvent {
                model: payload.model.ok_or(rusqlite::Error::InvalidQuery)?,
                tokens: payload.model_tokens,
                timestamp: payload.timestamp,
            },
        ));
    }
    Ok(steps)
}

/// A focused activity derivation publishes its own physical-source projection.
/// It never replaces Catalog ownership, relationships or family summaries.
pub(crate) fn publish_selected(
    options: &crate::Options,
    row: &crate::model::summary::CatalogRow,
    namespace: Option<&crate::storage::Namespace>,
) -> io::Result<()> {
    let projection = row
        .session_activity
        .as_ref()
        .ok_or_else(|| io::Error::other("Selected activity is unavailable"))?;
    let mut db = Connection::open(crate::events::EventCache::path(&options.cache))
        .map_err(io::Error::other)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    if let Some(namespace) = namespace {
        crate::storage::sqlite::bind_connection(&tx, namespace).map_err(io::Error::other)?;
    }
    if !current(&tx).map_err(io::Error::other)? {
        return Err(io::Error::other("Selected activity index changed"));
    }
    if row.sources.is_empty()
        || row.sources.len() > 64
        || !super::selected_roles_current(&tx, &row.sources).map_err(io::Error::other)?
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Selected activity source generation changed",
        ));
    }
    // The Catalog fields of `row` are not published here, so the analytics
    // digest stays as the last Catalog publication left it.
    if write_activity(&tx, row, projection).map_err(io::Error::other)? {
        log_change(&tx, &row.key).map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)
}

/// Publish a session's activity together with the Catalog fields analytics
/// reads: the caller is the transaction that publishes those fields.
pub(super) fn publish(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    projection: &Projection,
) -> rusqlite::Result<()> {
    let rewritten = write_activity(tx, row, projection)?;
    note_session_state(tx, row, rewritten)
}

/// Write the activity rows. `false` when they already were this generation.
fn write_activity(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    projection: &Projection,
) -> rusqlite::Result<bool> {
    let activity = &projection.activity;
    if activity.session_key != row.key || activity.rules_version != activity::ACTIVITY_RULES_VERSION
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let sources = crate::retention::source_generation(&row.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    // Include exact ownership and derivation output. Unrelated Catalog owners
    // can advance without invalidating this session's generation.
    let generation = format!(
        "{:x}",
        Sha256::digest(json(&(&sources, activity))?.as_bytes())
    );
    let unchanged: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_activity_projections WHERE session_key=?1 AND generation=?2 AND source_generation=?3 AND rules_version=?4 AND pricing_fingerprint=?5)",params![row.key,generation,sources,activity.rules_version,activity.pricing_fingerprint],|r|r.get(0))?;
    if unchanged {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM session_activity_steps WHERE session_key=?1",
        [&row.key],
    )?;
    tx.execute(
        "DELETE FROM session_activity_compactions WHERE session_key=?1",
        [&row.key],
    )?;
    tx.execute(
        "DELETE FROM session_activity_facts WHERE session_key=?1",
        [&row.key],
    )?;
    let mut put =
        tx.prepare("INSERT INTO session_activity_steps VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")?;
    for (ordinal, step) in activity.steps.iter().enumerate() {
        let saved = SavedStep {
            step: step.clone(),
            usage: projection
                .inputs
                .get(&step.id)
                .ok_or(rusqlite::Error::InvalidQuery)?
                .clone(),
            facts: projection
                .facts
                .get(&step.id)
                .ok_or(rusqlite::Error::InvalidQuery)?
                .clone(),
        };
        put.execute(params![
            row.key,
            generation,
            ordinal as i64,
            activity.pricing_fingerprint,
            json(&saved)?,
            step.waste.map(|kind| tag(&kind)).transpose()?,
            step.waste_category
                .map(|category| tag(&category))
                .transpose()?,
            step.empty_search,
        ])?;
    }
    let mut put = tx.prepare("INSERT INTO session_activity_compactions VALUES(?1,?2,?3,?4)")?;
    for (ordinal, compact) in activity.compactions.iter().enumerate() {
        put.execute(params![row.key, generation, ordinal as i64, json(compact)?])?;
    }
    let mut put = tx.prepare("INSERT INTO session_activity_facts VALUES(?1,?2,?3,?4)")?;
    for fact in &activity.facts {
        put.execute(params![row.key, generation, fact.identity, json(fact)?])?;
    }
    tx.execute("INSERT INTO session_activity_sources VALUES(?1,?2) ON CONFLICT(session_key) DO UPDATE SET metadata=excluded.metadata", params![row.key,json(row)?])?;
    tx.execute("INSERT INTO session_activity_projections VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,rules_version=excluded.rules_version,pricing_fingerprint=excluded.pricing_fingerprint,total=excluded.total,compactions=excluded.compactions,thrash=excluded.thrash,summary=excluded.summary,preview=excluded.preview",params![row.key,generation,sources,activity.rules_version,activity.pricing_fingerprint,activity.steps.len() as i64,activity.compactions.len() as i64,activity.aggregate.thrash,json(&activity::analytics::Analytics::of(activity))?,json(&activity::analytics::Preview::of(&activity.steps))?])?;
    Ok(true)
}

/// The catalog fields analytics reads besides activity, as one digest.
fn analytics_digest(row: &crate::model::summary::CatalogRow) -> rusqlite::Result<String> {
    let state = row.overview.as_ref().map(|overview| overview.status());
    Ok(format!(
        "{:x}",
        Sha256::digest(
            json(&(
                row.lifecycle,
                &row.kind,
                &row.presentation_kind,
                &row.parent,
                &row.parent_source,
                &row.repo,
                &row.repo_id,
                &row.branch,
                row.start,
                row.last,
                state,
                (
                    &row.name,
                    &row.harness,
                    &row.native_ids,
                    &row.model,
                    &row.effort,
                    &row.tokens,
                    &row.cost,
                ),
                row.overview
                    .as_ref()
                    .map(|overview| overview.analytics_inputs()),
            ))?
            .as_bytes()
        )
    ))
}

/// Log a change for a query-ready session when its activity was rewritten
/// (`rewritten`) or its analytics digest differs from the last one logged.
fn note_session_state(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    rewritten: bool,
) -> rusqlite::Result<()> {
    let digest = analytics_digest(row)?;
    let stored: Option<String> = tx
        .query_row(
            "SELECT digest FROM silver_session_state WHERE session_key=?1",
            [&row.key],
            |r| r.get(0),
        )
        .optional()?;
    if rewritten || stored.as_deref() != Some(digest.as_str()) {
        tx.execute(
            "INSERT INTO silver_session_state(session_key,digest,repo_id) VALUES(?1,?2,?3) ON CONFLICT(session_key) DO UPDATE SET digest=excluded.digest,repo_id=excluded.repo_id",
            params![row.key, digest, row.repo_id.as_ref().map(json).transpose()?],
        )?;
        log_change(tx, &row.key)?;
    }
    Ok(())
}

fn log_change(tx: &Transaction<'_>, key: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
        [key],
    )?;
    Ok(())
}

/// Mark sessions leaving the current catalog: a `changed` row for each that
/// had query-ready activity, so analytics can see its sources are missing, and
/// a digest that no republication matches, so a returning session is logged
/// again. Their activity and retained history are untouched (`removed` is for
/// projections that are dropped). `key` limits the scope to one session, by
/// its index. Runs before the lifecycle is set to `retained`, so a repeated
/// publication logs nothing.
pub(super) fn withdraw(tx: &Transaction<'_>, key: Option<&str>) -> rusqlite::Result<()> {
    let (log, mark) = withdraw_sql(key.is_some());
    tx.execute(&log, rusqlite::params_from_iter(key))?;
    tx.execute(&mark, rusqlite::params_from_iter(key))?;
    Ok(())
}

fn withdraw_sql(exact: bool) -> (String, String) {
    const ALL: &str = "FROM session_catalog WHERE lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)";
    const ONE: &str = "FROM session_catalog WHERE session_key=?1 AND lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)";
    let gone = if exact { ONE } else { ALL };
    (
        format!(
            "INSERT INTO silver_change(session_key,kind) SELECT session_key,'changed' {gone} AND session_key IN (SELECT session_key FROM session_activity_projections) ORDER BY session_key"
        ),
        format!(
            "UPDATE silver_session_state SET digest='retained' WHERE session_key IN (SELECT session_key {gone})"
        ),
    )
}

pub(super) fn remove_mismatched(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
) -> rusqlite::Result<()> {
    let sources = crate::retention::source_generation(&row.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let removed = tx.execute(
        "DELETE FROM session_activity_projections WHERE session_key=?1 AND source_generation<>?2",
        params![row.key, sources],
    )?;
    if removed > 0 {
        tx.execute(
            "INSERT INTO silver_change(session_key,kind) VALUES(?1,'removed')",
            [&row.key],
        )?;
        tx.execute(
            "DELETE FROM silver_session_state WHERE session_key=?1",
            [&row.key],
        )?;
        tx.execute(
            "DELETE FROM session_activity_facts WHERE session_key=?1",
            [&row.key],
        )?;
        tx.execute(
            "DELETE FROM session_activity_steps WHERE session_key=?1",
            [&row.key],
        )?;
        tx.execute(
            "DELETE FROM session_activity_compactions WHERE session_key=?1",
            [&row.key],
        )?;
    } else {
        let ready: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_activity_projections WHERE session_key=?1)",
            [&row.key],
            |r| r.get(0),
        )?;
        if ready {
            note_session_state(tx, row, false)?;
        }
    }
    Ok(())
}

pub(super) fn repairs(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = c.prepare("SELECT s.source_path FROM session_catalog_sources s WHERE s.lifecycle='current' AND s.session_key IN (SELECT c.session_key FROM session_catalog c LEFT JOIN session_activity_projections a USING(session_key) WHERE c.lifecycle='current' AND (c.harness='claude' OR (c.harness='codex' AND EXISTS(SELECT 1 FROM session_catalog_sources cs JOIN files f ON f.path=cs.source_path WHERE cs.session_key=c.session_key AND f.codex_native_usage IS NOT NULL))) AND (a.session_key IS NULL OR a.rules_version<>?1) LIMIT 64) LIMIT 64")?;
    statement
        .query_map([activity::ACTIVITY_RULES_VERSION], |r| r.get(0))?
        .collect()
}

/// Internal bounded reader. Selection and pages must share the caller's SQLite
/// snapshot. Query and MCP use the sanitized items reader below.
#[derive(Debug, PartialEq)]
pub(crate) struct Page {
    pub(crate) generation: String,
    pub(crate) total: u64,
    pub(crate) steps: Vec<ActivityStep>,
}

pub(crate) fn page(
    c: &Connection,
    row: &crate::model::summary::CatalogRow,
    expected_generation: Option<&str>,
    pricing_fingerprint: &str,
    start: u64,
    limit: u32,
) -> rusqlite::Result<Option<Page>> {
    let sources = crate::retention::source_generation(&row.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let header: Option<(String, i64)> = c.query_row("SELECT generation,total FROM session_activity_projections WHERE session_key=?1 AND source_generation=?2 AND rules_version=?3 AND pricing_fingerprint=?4",params![row.key,sources,activity::ACTIVITY_RULES_VERSION,pricing_fingerprint],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((generation, total)) = header else {
        return Ok(None);
    };
    if expected_generation.is_some_and(|expected| expected != generation) {
        return Ok(None);
    }
    let mut statement = c.prepare("SELECT ordinal,metadata,pricing_fingerprint FROM session_activity_steps WHERE session_key=?1 AND generation=?2 AND ordinal>=?3 ORDER BY ordinal LIMIT ?4")?;
    let mut rows = statement.query(params![
        row.key,
        generation,
        i64::try_from(start).map_err(|_| rusqlite::Error::InvalidQuery)?,
        limit.min(256)
    ])?;
    let mut steps = Vec::new();
    while let Some(record) = rows.next()? {
        crate::selected_read::row(record, 256)?;
        if u64::try_from(record.get::<_, i64>(0)?).ok() != Some(start + steps.len() as u64)
            || record.get::<_, String>(2)? != pricing_fingerprint
        {
            return Ok(None);
        }
        let saved: SavedStep = from_json(1, &record.get::<_, String>(1)?)?;
        steps.push(saved.step);
    }
    if steps.len() as u64
        != (total as u64)
            .saturating_sub(start)
            .min(u64::from(limit.min(256)))
    {
        return Ok(None);
    }
    Ok(Some(Page {
        generation,
        total: total as u64,
        steps,
    }))
}

/// Read all persisted StepFacts under the same source, rules, price and
/// generation admission as activity pages. This crate-internal reader is for
/// the analytics writer; public query and mirror surfaces must use `items`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "kept crate-internal for the analytics writer")
)]
pub(crate) fn step_facts(
    c: &Connection,
    row: &crate::model::summary::CatalogRow,
    generation: &str,
) -> rusqlite::Result<Option<Vec<(u64, ActivityStepId, activity::StepFacts)>>> {
    let price = crate::pricing::snapshot();
    let Some(admitted) = page(c, row, Some(generation), &price.fingerprint, 0, 0)? else {
        return Ok(None);
    };
    let mut statement = c.prepare(
        "SELECT ordinal,metadata,pricing_fingerprint FROM session_activity_steps WHERE session_key=?1 AND generation=?2 ORDER BY ordinal",
    )?;
    let mut records = statement.query(params![row.key, generation])?;
    let mut facts = Vec::new();
    while let Some(record) = records.next()? {
        crate::selected_read::row(record, 256)?;
        if record.get::<_, String>(2)? != price.fingerprint {
            return Ok(None);
        }
        let ordinal =
            u64::try_from(record.get::<_, i64>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if ordinal != facts.len() as u64 {
            return Ok(None);
        }
        let saved: SavedStep = from_json(1, &record.get::<_, String>(1)?)?;
        facts.push((ordinal, saved.step.id, saved.facts));
    }
    if facts.len() as u64 != admitted.total {
        return Ok(None);
    }
    Ok(Some(facts))
}

/// Read one persisted step and its native usage under the page admission fence.
pub(crate) fn step_at(
    c: &Connection,
    row: &crate::model::summary::CatalogRow,
    generation: &str,
    ordinal: u64,
) -> rusqlite::Result<Option<(ActivityStep, Usage)>> {
    let price = crate::pricing::snapshot();
    let Some(admitted) = page(c, row, Some(generation), &price.fingerprint, ordinal, 1)? else {
        return Ok(None);
    };
    if admitted.steps.is_empty() {
        return Ok(None);
    }
    let metadata: String = c.query_row(
        "SELECT metadata FROM session_activity_steps WHERE session_key=?1 AND generation=?2 AND ordinal=?3",
        params![row.key, generation, i64::try_from(ordinal).map_err(|_| rusqlite::Error::InvalidQuery)?],
        |record| record.get(0),
    )?;
    let saved: SavedStep = from_json(0, &metadata)?;
    Ok(Some((saved.step, saved.usage)))
}

/// The bounded timeline stored with the projection, under the same source,
/// rules, price and generation admission as the step pages.
pub(crate) fn preview(
    c: &Connection,
    row: &crate::model::summary::CatalogRow,
    expected: Option<&str>,
    price: &str,
) -> rusqlite::Result<Option<(String, activity::analytics::Preview)>> {
    let sources = crate::retention::source_generation(&row.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let stored: Option<(String, String)> = c.query_row("SELECT generation,preview FROM session_activity_projections WHERE session_key=?1 AND source_generation=?2 AND rules_version=?3 AND pricing_fingerprint=?4",params![row.key,sources,activity::ACTIVITY_RULES_VERSION,price],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((generation, encoded)) = stored else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| expected != generation) {
        return Ok(None);
    }
    Ok(Some((generation, from_json(1, &encoded)?)))
}

pub(crate) struct Items {
    pub(crate) generation: String,
    pub(crate) items: Vec<serde_json::Value>,
    pub(crate) next_cursor: Option<String>,
}

/// Steps retain producer ordinal order; facts use their stable identity key.
/// Both streams share the selected source/rules/price/generation admission.
#[allow(clippy::too_many_arguments)]
pub(crate) fn items(
    c: &Connection,
    row: &crate::model::summary::CatalogRow,
    expected: Option<&str>,
    price: &str,
    facts: bool,
    cursor: Option<&str>,
    limit: u32,
) -> rusqlite::Result<Option<Items>> {
    let start = if facts {
        0
    } else {
        cursor
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?
            .unwrap_or(0)
    };
    let Some(header) = page(
        c,
        row,
        expected,
        price,
        if facts { 0 } else { start },
        if facts { 0 } else { limit },
    )?
    else {
        return Ok(None);
    };
    let limit = limit.min(256);
    let mut items = Vec::new();
    let next_cursor = if facts {
        let mut statement = c.prepare("SELECT identity,metadata FROM session_activity_facts WHERE session_key=?1 AND generation=?2 AND identity>?3 ORDER BY identity LIMIT ?4")?;
        let mut records = statement.query(params![
            row.key,
            header.generation,
            cursor.unwrap_or(""),
            limit + 1
        ])?;
        let mut last = None;
        let mut more = false;
        while let Some(record) = records.next()? {
            crate::selected_read::row(record, 256)?;
            if items.len() == limit as usize {
                more = true;
                break;
            }
            let fact: activity::classify::ChangeFact = from_json(1, &record.get::<_, String>(1)?)?;
            last = Some(record.get::<_, String>(0)?);
            items.push(serde_json::json!({"identity":fact.identity,"timestamp":fact.timestamp,"step_id":fact.step_id,"change":fact.change}));
        }
        if more { last } else { None }
    } else {
        for step in header.steps {
            let mut item = serde_json::to_value(step).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let object = item.as_object_mut().expect("typed activity step");
            object.remove("position");
            object.remove("call_position");
            items.push(item);
        }
        if start > header.total {
            return Ok(None);
        }
        let next = start + items.len() as u64;
        (next < header.total).then(|| next.to_string())
    };
    Ok(Some(Items {
        generation: header.generation,
        items,
        next_cursor,
    }))
}

/// Fold already-published compact summaries, never native bodies. Match every
/// member's selected source vector and price. Oversize or incomplete families
/// stay explicit unknown, matching the bounded family-cost producer.
fn family(
    c: &Connection,
    key: &str,
    price: &str,
) -> rusqlite::Result<Option<activity::analytics::Family>> {
    let mut statement = c.prepare("WITH RECURSIVE family(id) AS (SELECT ?1 UNION SELECT c.session_key FROM session_catalog c JOIN family f ON c.parent_key=f.id LIMIT 257) SELECT c.session_key,CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END,a.generation,a.source_generation,a.rules_version,a.pricing_fingerprint,a.summary FROM family JOIN session_catalog c ON c.session_key=id LEFT JOIN session_activity_projections a ON a.session_key=id ORDER BY c.session_key")?;
    let mut rows = statement.query([key])?;
    let mut fold: Option<activity::analytics::Fold> = None;
    let mut members = 0;
    let mut shared_count = 0;
    let mut member_ids = BTreeSet::new();
    let mut root_parent = None;
    let mut bytes = 0;
    while let Some(record) = rows.next()? {
        crate::selected_read::row(record, 256)?;
        members += 1;
        if members > 256 {
            return Ok(None);
        }
        let Some(encoded) = record.get::<_, Option<String>>(1)? else {
            return Ok(None);
        };
        bytes += encoded.len();
        if bytes > 4 * 1024 * 1024 {
            return Ok(None);
        }
        let row: crate::model::summary::CatalogRow = from_json(1, &encoded)?;
        member_ids.insert(row.key.clone());
        if row.key == key {
            root_parent = row.parent.clone();
        }
        let source: Option<String> = record.get(3)?;
        if source.as_deref()
            != Some(
                &crate::retention::source_generation(&row.sources)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            )
            || record.get::<_, Option<u32>>(4)? != Some(activity::ACTIVITY_RULES_VERSION)
            || record.get::<_, Option<String>>(5)?.as_deref() != Some(price)
        {
            return Ok(None);
        }
        let summary: activity::analytics::Analytics = from_json(6, &record.get::<_, String>(6)?)?;
        let generation: String = record.get(2)?;
        let mut shared = Vec::new();
        if summary.counts.totals.shared.steps > 0 {
            let mut steps = c.prepare("SELECT metadata FROM session_activity_steps WHERE session_key=?1 AND generation=?2 AND json_type(metadata,'$.step.ownership.Shared')='text' ORDER BY ordinal LIMIT 4097")?;
            let mut records = steps.query(params![row.key, generation])?;
            while let Some(record) = records.next()? {
                shared_count += 1;
                if shared_count > 4096 {
                    return Ok(None);
                }
                crate::selected_read::row(record, 256)?;
                let encoded = record.get::<_, String>(0)?;
                bytes += encoded.len();
                if bytes > 4 * 1024 * 1024 {
                    return Ok(None);
                }
                let saved: SavedStep = from_json(0, &encoded)?;
                shared.push(saved.step);
            }
            if shared.len() as u64 != summary.counts.totals.shared.steps {
                return Ok(None);
            }
        }
        let next = activity::analytics::Fold::summary(&summary, shared);
        if let Some(fold) = &mut fold {
            fold.merge(next);
        } else {
            fold = Some(next);
        }
    }
    if root_parent.is_some_and(|parent| member_ids.contains(&parent)) {
        return Ok(None);
    }
    Ok(fold.filter(|_| members > 1).map(|fold| fold.family()))
}

fn save_summary(
    tx: &Transaction<'_>,
    row: &mut crate::model::summary::CatalogRow,
    table: &str,
) -> rusqlite::Result<()> {
    if let Some(query) = &mut row.query {
        query["rows"][&row.key]["summary"]["activity_analytics"] =
            serde_json::to_value(&row.activity_analytics)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    tx.execute(
        &format!("UPDATE {table} SET metadata=?2 WHERE session_key=?1"),
        params![row.key, json(row)?],
    )?;
    // Summary-only changes leave ordered recipes unchanged, but their Catalog
    // observation witness must advance in the same transaction.
    let identity = crate::query_projection::identity(row);
    if let Some(header) = crate::query_projection::header(tx, &identity)? {
        let mut header: serde_json::Value = from_json(0, &header)?;
        header["query_generation"] = serde_json::Value::String(format!(
            "{:x}",
            Sha256::digest(json(&row.query)?.as_bytes())
        ));
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![
                crate::query_projection::prefix("header", &identity),
                json(&header)?
            ],
        )?;
    }
    Ok(())
}

pub(super) fn refresh_summaries(tx: &Transaction<'_>, key: &str) -> rusqlite::Result<()> {
    // Isolated owners already carry their freshly derived compact summary.
    // Only lineage needs the stored fold for separately published members.
    let related: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 AND parent_key IS NOT NULL) OR EXISTS(SELECT 1 FROM session_catalog WHERE parent_key=?1)",
        [key],
        |row| row.get(0),
    )?;
    if !related {
        return Ok(());
    }
    refresh_at_price(tx, key, &crate::pricing::snapshot().fingerprint)
}

fn refresh_at_price(tx: &Transaction<'_>, key: &str, price: &str) -> rusqlite::Result<()> {
    let mut owners = tx.prepare("WITH RECURSIVE parents(id) AS (SELECT ?1 UNION SELECT c.parent_key FROM session_catalog c JOIN parents p ON c.session_key=p.id WHERE c.parent_key IS NOT NULL LIMIT 257) SELECT c.metadata FROM parents JOIN session_catalog c ON c.session_key=id ORDER BY c.session_key")?;
    let encoded = owners
        .query_map([key], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for encoded in encoded {
        let mut row: crate::model::summary::CatalogRow = from_json(0, &encoded)?;
        let source = crate::retention::source_generation(&row.sources)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let summary: Option<String> = tx.query_row("SELECT summary FROM session_activity_projections WHERE session_key=?1 AND source_generation=?2 AND rules_version=?3 AND pricing_fingerprint=?4", params![row.key, source, activity::ACTIVITY_RULES_VERSION, price], |r|r.get(0)).optional()?;
        let summary = summary
            .map(|text| from_json::<activity::analytics::Analytics>(0, &text))
            .transpose()?;
        row.activity_analytics = summary;
        if let Some(summary) = &mut row.activity_analytics {
            summary.family = family(tx, &row.key, price)?;
        }
        if json(&row)? != encoded {
            save_summary(tx, &mut row, "session_catalog")?;
            if let Some(history) = crate::history_projection::catalog(tx, &row.key)? {
                let mut history: crate::model::summary::CatalogRow = from_json(0, &history)?;
                if history.sources == row.sources {
                    history.activity_analytics = row.activity_analytics.clone();
                    save_summary(tx, &mut history, "session_history_catalog")?;
                }
            }
        }
    }
    Ok(())
}

/// Reprice at most 128 step rows per producer transaction, including retained
/// projections whose native sources are no longer available. Readers reject a
/// partially repriced page; only the final batch advances the header stamp.
pub(super) fn reprice_step(
    tx: &Transaction<'_>,
    table: &Arc<crate::pricing::EffectiveTable>,
) -> rusqlite::Result<bool> {
    let owner: Option<(String,String)> = tx.query_row("SELECT session_key,generation FROM session_activity_projections WHERE pricing_fingerprint<>?1 ORDER BY pricing_fingerprint,session_key LIMIT 1",[&table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((key, generation)) = owner else {
        return Ok(false);
    };
    let mut statement = tx.prepare("SELECT ordinal,metadata FROM session_activity_steps WHERE session_key=?1 AND generation=?2 AND pricing_fingerprint<>?3 ORDER BY ordinal LIMIT 128")?;
    let mut rows = statement.query(params![key, generation, table.fingerprint])?;
    let mut changed = 0;
    while let Some(row) = rows.next()? {
        let ordinal: i64 = row.get(0)?;
        let mut saved: SavedStep = from_json(1, &row.get::<_, String>(1)?)?;
        let mut cost = crate::pricing::CostBuild::with_table(table.clone());
        match &saved.usage {
            Usage::Claude(usage) => cost.message(usage),
            Usage::Codex(event) => cost.codex(event),
        }
        let cost = cost.finish_step();
        saved.step.list_cost_usd = cost.list;
        saved.step.fresh_input_usd = cost.fresh_input;
        saved.step.cache_read_usd = cost.cache_read;
        saved.step.cache_write_usd = cost.cache_write;
        saved.step.output_usd = cost.output;
        tx.execute("UPDATE session_activity_steps SET pricing_fingerprint=?4,metadata=?5 WHERE session_key=?1 AND generation=?2 AND ordinal=?3",params![key,generation,ordinal,table.fingerprint,json(&saved)?])?;
        changed += 1;
    }
    if changed < 128 {
        // The last batch has repriced every step. Price-dependent summary values
        // (totals, waste and activity costs, coverage) and the preview come from
        // the stored steps; readers refuse the summary until the stamp advances.
        let encoded: String = tx.query_row(
            "SELECT summary FROM session_activity_projections WHERE session_key=?1",
            [&key],
            |r| r.get(0),
        )?;
        let mut summary: activity::analytics::Analytics = from_json(0, &encoded)?;
        let mut statement = tx.prepare("SELECT metadata FROM session_activity_steps WHERE session_key=?1 AND generation=?2 ORDER BY ordinal")?;
        let steps = statement
            .query_map(params![key, generation], |row| {
                Ok(from_json::<SavedStep>(0, &row.get::<_, String>(0)?)?.step)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        summary.reprice(&steps);
        let preview = activity::analytics::Preview::of(&steps);
        let next = format!(
            "{:x}",
            Sha256::digest(json(&(&generation, &table.fingerprint))?.as_bytes())
        );
        tx.execute(
            "UPDATE session_activity_steps SET generation=?2 WHERE session_key=?1",
            params![key, next],
        )?;
        tx.execute(
            "UPDATE session_activity_compactions SET generation=?2 WHERE session_key=?1",
            params![key, next],
        )?;
        tx.execute(
            "UPDATE session_activity_facts SET generation=?2 WHERE session_key=?1",
            params![key, next],
        )?;
        summary.pricing_fingerprint = table.fingerprint.clone();
        tx.execute("UPDATE session_activity_projections SET pricing_fingerprint=?2,generation=?3,summary=?4,preview=?5 WHERE session_key=?1",params![key,table.fingerprint,next,json(&summary)?,json(&preview)?])?;
        refresh_at_price(tx, &key, &table.fingerprint)?;
        tx.execute(
            "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
            [&key],
        )?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
