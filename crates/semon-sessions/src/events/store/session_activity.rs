//! Source-qualified activity facts and generation-bound SQLite publication.
//! Native bodies never enter these rows. Pricing inputs retain only typed usage.
use super::*;
use crate::events::session_activity::{
    self as activity, ActivityStep, SessionActivity, SourcePosition, Usage, UsageStep,
};
use std::sync::Arc;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_activity_projections (
 session_key TEXT PRIMARY KEY, generation TEXT NOT NULL,
 source_generation TEXT NOT NULL, rules_version INTEGER NOT NULL,
 pricing_fingerprint TEXT NOT NULL, total INTEGER NOT NULL,
 compactions INTEGER NOT NULL, thrash INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS session_activity_rules ON session_activity_projections(rules_version,session_key);
CREATE INDEX IF NOT EXISTS session_activity_pricing ON session_activity_projections(pricing_fingerprint,session_key);
CREATE TABLE IF NOT EXISTS session_activity_steps (
 session_key TEXT NOT NULL, generation TEXT NOT NULL, ordinal INTEGER NOT NULL,
 pricing_fingerprint TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(session_key,generation,ordinal)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_activity_compactions (
 session_key TEXT NOT NULL, generation TEXT NOT NULL, ordinal INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(session_key,generation,ordinal)
) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    pub(crate) activity: SessionActivity,
    // Kept outside Catalog JSON and keyed by the same exact native step identity.
    inputs: BTreeMap<activity::ActivityStepId, Usage>,
}

impl Projection {
    pub(crate) fn derive(
        key: String,
        usage: Vec<UsageStep>,
        shared: &crate::claude_usage::Shared,
        signals: Vec<(String, Signal)>,
        table: Arc<crate::pricing::EffectiveTable>,
    ) -> Self {
        let mut inputs = BTreeMap::new();
        for row in &usage {
            match &row.id {
                activity::ActivityStepId::Claude(_) => {
                    inputs.insert(row.id.clone(), row.usage.clone());
                }
                activity::ActivityStepId::Codex(_) => {
                    inputs
                        .entry(row.id.clone())
                        .or_insert_with(|| row.usage.clone());
                }
            }
        }
        Self {
            activity: activity::derive(key, usage, shared, signals, table),
            inputs,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SavedStep {
    step: ActivityStep,
    usage: Usage,
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

pub(super) fn publish(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    projection: &Projection,
) -> rusqlite::Result<()> {
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
        return Ok(());
    }
    tx.execute(
        "DELETE FROM session_activity_steps WHERE session_key=?1",
        [&row.key],
    )?;
    tx.execute(
        "DELETE FROM session_activity_compactions WHERE session_key=?1",
        [&row.key],
    )?;
    let mut put = tx.prepare("INSERT INTO session_activity_steps VALUES(?1,?2,?3,?4,?5)")?;
    for (ordinal, step) in activity.steps.iter().enumerate() {
        let saved = SavedStep {
            step: step.clone(),
            usage: projection
                .inputs
                .get(&step.id)
                .ok_or(rusqlite::Error::InvalidQuery)?
                .clone(),
        };
        put.execute(params![
            row.key,
            generation,
            ordinal as i64,
            activity.pricing_fingerprint,
            json(&saved)?
        ])?;
    }
    let mut put = tx.prepare("INSERT INTO session_activity_compactions VALUES(?1,?2,?3,?4)")?;
    for (ordinal, compact) in activity.compactions.iter().enumerate() {
        put.execute(params![row.key, generation, ordinal as i64, json(compact)?])?;
    }
    tx.execute("INSERT INTO session_activity_projections VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,rules_version=excluded.rules_version,pricing_fingerprint=excluded.pricing_fingerprint,total=excluded.total,compactions=excluded.compactions,thrash=excluded.thrash",params![row.key,generation,sources,activity.rules_version,activity.pricing_fingerprint,activity.steps.len() as i64,activity.compactions.len() as i64,activity.aggregate.thrash])?;
    Ok(())
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
            "DELETE FROM session_activity_steps WHERE session_key=?1",
            [&row.key],
        )?;
        tx.execute(
            "DELETE FROM session_activity_compactions WHERE session_key=?1",
            [&row.key],
        )?;
    }
    Ok(())
}

pub(super) fn repairs(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = c.prepare("SELECT s.source_path FROM session_catalog_sources s WHERE s.lifecycle='current' AND s.session_key IN (SELECT c.session_key FROM session_catalog c LEFT JOIN session_activity_projections a USING(session_key) WHERE c.lifecycle='current' AND c.harness IN ('claude','codex') AND (a.session_key IS NULL OR a.rules_version<>?1) LIMIT 64) LIMIT 64")?;
    statement
        .query_map([activity::ACTIVITY_RULES_VERSION], |r| r.get(0))?
        .collect()
}

/// Internal bounded reader. Selection and pages must share the caller's SQLite
/// snapshot. No operation is exported through Query or MCP in this slice.
#[derive(Debug, PartialEq)]
pub(crate) struct Page {
    pub(crate) generation: String,
    pub(crate) total: u64,
    pub(crate) steps: Vec<ActivityStep>,
}

#[allow(
    dead_code,
    reason = "Internal page contract; public selected reads follow separately"
)]
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
        (saved.step.list_cost_usd, saved.step.carry_cost_usd) =
            cost.finish_step(saved.step.tokens.cache_read);
        tx.execute("UPDATE session_activity_steps SET pricing_fingerprint=?4,metadata=?5 WHERE session_key=?1 AND generation=?2 AND ordinal=?3",params![key,generation,ordinal,table.fingerprint,json(&saved)?])?;
        changed += 1;
    }
    if changed < 128 {
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
        tx.execute("UPDATE session_activity_projections SET pricing_fingerprint=?2,generation=?3 WHERE session_key=?1",params![key,table.fingerprint,next])?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
