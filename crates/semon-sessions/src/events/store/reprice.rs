//! Resumable cost-only producer. It never opens native bodies, changes source
//! membership, deletes observations, or advances parser/publication witnesses.
use super::*;
use crate::pricing::{Cost, CostBuild, EffectiveTable};
use std::sync::Arc;

const WORK: &str = "pricing_recompute_v1";
const BATCH: usize = 128;
const MAX_FACTS: usize = 4096;
const MAX_JSON: usize = 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Work {
    fingerprint: String,
    fetched_at: u64,
    phase: u8,
    source_after: i64,
    session_after: String,
    job: Option<SourceJob>,
    finished_at: i64,
}
#[derive(Serialize, Deserialize)]
struct SourceJob {
    id: i64,
    receipt: String,
    offset: i64,
    cost: CostBuild,
}

/// One bounded step. Each transaction holds at most one source fact batch or
/// one qualified session's metadata. Durable cursors resume after process exit.
pub(crate) fn step(path: &Path, table: Arc<EffectiveTable>) -> rusqlite::Result<bool> {
    step_inner(path, table, true)
}

fn step_inner(
    path: &Path,
    table: Arc<EffectiveTable>,
    check_current: bool,
) -> rusqlite::Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(50))?;
    let (schema, parser) = versions(&connection)?;
    if schema != SCHEMA_VERSION || parser != Some(CACHE_VERSION) {
        return Ok(false);
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let encoded: Option<String>=tx.query_row("SELECT CASE WHEN octet_length(value)<=1048576 THEN value ELSE NULL END FROM meta WHERE key=?1",[WORK],|r|r.get(0)).optional()?;
    let mut work: Work = encoded
        .as_deref()
        .map(|s| from_json(0, s))
        .transpose()?
        .unwrap_or_default();
    // A second process using an older last-known-good table may not roll a
    // newer process's completed/partial work back to its older fetch.
    if work.fetched_at > table.fetched_at {
        return Ok(false);
    }
    if work.fingerprint != table.fingerprint {
        work = Work {
            fingerprint: table.fingerprint.clone(),
            fetched_at: table.fetched_at,
            ..Work::default()
        };
    }
    if work.phase == 3 {
        if crate::model::now_ms().saturating_sub(work.finished_at) < 30_000 {
            return Ok(false);
        }
        work.phase = 0;
        work.source_after = 0;
        work.session_after.clear();
    }
    tx.execute_batch("SAVEPOINT pricing_owner")?;
    let result = match work.phase {
        0 => source_step(&tx, &mut work, &table),
        1 => catalog_step(&tx, &mut work, &table),
        _ => family_step(&tx, &mut work),
    };
    match result {
        Ok(()) => tx.execute_batch("RELEASE pricing_owner")?,
        Err(
            rusqlite::Error::InvalidQuery
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::ToSqlConversionFailure(..)
            | rusqlite::Error::QueryReturnedNoRows,
        ) => {
            // Unsupported retained evidence must not block unrelated owners.
            // Roll back this owner's partial writes and retain its old stamp.
            tx.execute_batch("ROLLBACK TO pricing_owner; RELEASE pricing_owner")?;
            work.job = None;
        }
        Err(error) => return Err(error),
    }
    // A stale job never publishes after the process has swapped its table.
    if check_current && crate::pricing::snapshot().fingerprint != table.fingerprint {
        return Ok(false);
    }
    let mut value = json(&work)?;
    if value.len() > MAX_JSON {
        // Keep the old source cost for an over-budget accumulator, then
        // advance to unrelated sources on the next attempt.
        work.job = None;
        value = json(&work)?;
    }
    tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![WORK,value])?;
    tx.commit()?;
    Ok(true)
}

fn source_step(
    tx: &Transaction<'_>,
    work: &mut Work,
    table: &Arc<EffectiveTable>,
) -> rusqlite::Result<()> {
    if work.job.is_none() {
        let next: Option<(i64,String,Option<String>)>=tx.query_row("SELECT s.source_id,s.receipt,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM native_usage_sources s JOIN native_usage_current n USING(source_id) JOIN files f ON f.path=n.source_path WHERE s.source_id>?1 AND s.parser_version=?2 AND s.usage_ready=1 ORDER BY s.source_id LIMIT 1",params![work.source_after,CACHE_VERSION],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((id, receipt, encoded)) = next else {
            work.phase = 1;
            work.session_after.clear();
            return Ok(());
        };
        work.source_after = id;
        let Some(encoded) = encoded else {
            return Ok(());
        };
        let root: Value = from_json(0, &encoded)?;
        if root["projection_version"] != 1
            || root["cost"]["pricing_fingerprint"] == table.fingerprint
        {
            return Ok(());
        }
        work.job = Some(SourceJob {
            id,
            receipt,
            offset: -1,
            cost: CostBuild::with_table(Arc::clone(table)),
        });
    }
    let job = work.job.as_mut().expect("source selected");
    job.cost.bind_table(Arc::clone(table));
    let current: Option<(String,String)>=tx.query_row("SELECT s.receipt,f.codex_native_usage FROM native_usage_sources s JOIN native_usage_current n USING(source_id) JOIN files f ON f.path=n.source_path WHERE s.source_id=?1 AND octet_length(f.codex_native_usage)<=1048576",[job.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((receipt, encoded)) = current else {
        work.job = None;
        return Ok(());
    };
    if receipt != job.receipt {
        work.source_after = job.id - 1;
        work.job = None;
        return Ok(());
    }
    let mut statement=tx.prepare("SELECT native_offset,CASE WHEN octet_length(payload)<=8192 THEN payload ELSE NULL END FROM native_usage_facts WHERE source_id=?1 AND native_offset>?2 AND model_outcome='accepted' ORDER BY native_offset LIMIT ?3")?;
    let rows = statement.query_map(params![job.id, job.offset, BATCH as i64], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut count = 0;
    for row in rows {
        let (offset, payload) = row?;
        let payload: native_usage::Payload = from_json(1, &payload)?;
        if let Some(model) = payload.model {
            job.cost.codex(&CodexUsageEvent {
                model,
                tokens: payload.model_tokens,
                timestamp: payload.timestamp,
            });
        }
        job.offset = offset;
        count += 1;
    }
    if count == BATCH {
        return Ok(());
    }
    let mut root: Value = from_json(0, &encoded)?;
    if root["projection_version"] != 1 {
        work.job = None;
        return Ok(());
    }
    // A source with no complete model facts must not become a free source.
    if job.cost.models.is_empty() && root["tokens"]["total"].as_u64().is_some_and(|n| n > 0) {
        work.job = None;
        return Ok(());
    }
    let mut expected = BTreeMap::new();
    let mut models=tx.prepare("SELECT dimension,CASE WHEN octet_length(value)<=8192 THEN value ELSE NULL END FROM native_usage_dimensions WHERE source_id=?1 AND kind='model' ORDER BY dimension LIMIT 4097")?;
    for row in models.query_map([job.id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (name, value) = row?;
        let tokens: ModelTokens = from_json(1, &value)?;
        expected.insert(name, tokens);
    }
    let computed = job.cost.clone().finish();
    if expected.len() > MAX_FACTS
        || expected.len() != computed.by_model.len()
        || expected.iter().any(|(name, tokens)| {
            computed.by_model.get(name).is_none_or(|cost| {
                cost.tokens.input != tokens.input
                    || cost.tokens.output != tokens.output
                    || cost.tokens.cache_read != tokens.cache_read
            })
        })
    {
        work.job = None;
        return Ok(());
    }
    // Replace all derived dimensions together. The native totals/receipts and
    // every original fact remain byte-for-byte unchanged.
    tx.execute("DELETE FROM native_usage_dimensions WHERE source_id=?1 AND kind IN ('cost_model','cost_day')",[job.id])?;
    for (model, value) in &job.cost.models {
        put_dimension(tx, job.id, "cost_model", model, value)?;
    }
    for (day, value) in &job.cost.by_day {
        put_dimension(tx, job.id, "cost_day", day, value)?;
    }
    let mut cost = job.cost.clone();
    cost.models.clear();
    cost.by_day.clear();
    cost.unpriced_models.clear();
    root["cost"] = serde_json::to_value(cost).map_err(sql_json)?;
    tx.execute("UPDATE files SET codex_native_usage=?2 WHERE path=(SELECT source_path FROM native_usage_current WHERE source_id=?1)",params![job.id,json(&root)?])?;
    work.job = None;
    Ok(())
}

fn put_dimension<T: Serialize>(
    tx: &Transaction<'_>,
    id: i64,
    kind: &str,
    key: &str,
    value: &T,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO native_usage_dimensions VALUES(?1,?2,?3,?4)",
        params![id, kind, key, json(value)?],
    )?;
    Ok(())
}
fn sql_json(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

/// Reprice only exact retained source generations and complete retained token
/// facts. Over-budget/legacy/incomplete owners keep their older fingerprint.
fn retained_cost(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    table: &Arc<EffectiveTable>,
) -> rusqlite::Result<Option<Cost>> {
    if row.sources.is_empty()
        || row.sources.len() > 64
        || !matches!(row.harness.as_str(), "claude" | "codex")
    {
        return Ok(None);
    }
    let mut build = CostBuild::with_table(Arc::clone(table));
    let mut messages: BTreeMap<String, MessageUsage> = BTreeMap::new();
    let mut facts = 0;
    for source in &row.sources {
        let Some(path) = source.path.to_str() else {
            return Ok(None);
        };
        let current = ledger_row(tx, path)?;
        let Some((file_id, ledger)) = current else {
            return Ok(None);
        };
        if ledger.offset != source.offset
            || ledger.prefix != source.prefix_sha256
            || ledger.tail != source.tail_sha256
            || ledger.stat.dev != source.dev
            || ledger.stat.ino != source.ino
            || ledger.stat.size != source.size
            || ledger.stat.modified_ns != source.modified_ns
            || ledger.stat.changed_ns != source.changed_ns
        {
            return Ok(None);
        }
        if row.harness == "claude" {
            let mut statement=tx.prepare(&format!("SELECT {USAGE_COLUMNS} FROM usage WHERE file_id=?1 AND octet_length(record_ids)<=65536 AND length(COALESCE(model,''))<=512 LIMIT ?2"))?;
            let total: i64 = tx.query_row(
                "SELECT COUNT(*) FROM (SELECT 1 FROM usage WHERE file_id=?1 LIMIT ?2)",
                params![file_id, (MAX_FACTS + 1) as i64],
                |r| r.get(0),
            )?;
            if total as usize + facts > MAX_FACTS {
                return Ok(None);
            }
            let rows = statement.query_map(params![file_id, (MAX_FACTS + 1) as i64], read_usage)?;
            let mut loaded = 0;
            for result in rows {
                let (id, message) = result?;
                loaded += 1;
                if let Some(prior) = messages.get(&id) {
                    if prior.model != message.model || prior.billing != message.billing {
                        return Ok(None);
                    }
                }
                messages.insert(id, message);
            }
            if loaded != total as usize {
                return Ok(None);
            }
            facts += loaded;
        } else {
            let encoded:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(codex_native_usage)<=1048576 THEN codex_native_usage ELSE NULL END FROM files WHERE file_id=?1",[file_id],|r|r.get(0))?;
            if let Some(encoded) = encoded {
                if let Some((id, _)) = native_usage::source(tx, path)? {
                    let count:i64=tx.query_row("SELECT COUNT(*) FROM (SELECT 1 FROM native_usage_dimensions WHERE source_id=?1 LIMIT 4097)",[id],|r|r.get(0))?;
                    if count > MAX_FACTS as i64 {
                        return Ok(None);
                    }
                }
                let native = native_usage::restore(tx, path, &ledger, Some(encoded))?
                    .ok_or(rusqlite::Error::InvalidQuery)?;
                let index = FileIndex {
                    codex_native_usage: Some(native),
                    ..FileIndex::default()
                };
                let cost = index.codex_cost();
                if cost.pricing_fingerprint != table.fingerprint {
                    return Ok(None);
                }
                build.merge(&cost);
            } else {
                let mut statement=tx.prepare("SELECT model,input,output,cache_write,cache_read,t FROM codex_usage WHERE file_id=?1 ORDER BY seq LIMIT ?2")?;
                let mut rows = statement.query(params![file_id, (MAX_FACTS + 1) as i64])?;
                while let Some(r) = rows.next()? {
                    facts += 1;
                    if facts > MAX_FACTS {
                        return Ok(None);
                    }
                    build.codex(&CodexUsageEvent {
                        model: r.get(0)?,
                        tokens: ModelTokens {
                            input: uint(r.get(1)?),
                            output: uint(r.get(2)?),
                            cache_write: uint(r.get(3)?),
                            cache_read: uint(r.get(4)?),
                        },
                        timestamp: r.get(5)?,
                    });
                }
            }
        }
    }
    for message in messages.values() {
        build.message(message)
    }
    let mut cost = build.finish();
    // Existing ownership/copy exclusions must be exactly reproducible. Token
    // disagreement is unavailable evidence, never permission to bill a copy.
    if cost.by_model.len() != row.cost.by_model.len()
        || cost.by_model.iter().any(|(model, value)| {
            row.cost
                .by_model
                .get(model)
                .is_none_or(|old| old.tokens != value.tokens)
        })
    {
        return Ok(None);
    }
    if row.cost.usd.is_none() && row.cost.unpriced_models.is_empty() {
        cost.usd = None
    }
    Ok(Some(cost))
}

fn catalog_step(
    tx: &Transaction<'_>,
    work: &mut Work,
    table: &Arc<EffectiveTable>,
) -> rusqlite::Result<()> {
    let next:Option<(String,Option<String>)>=tx.query_row("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key>?1 ORDER BY session_key LIMIT 1",[&work.session_after],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((key, encoded)) = next else {
        work.phase = 2;
        work.session_after.clear();
        return Ok(());
    };
    work.session_after = key.clone();
    let Some(encoded) = encoded else {
        return Ok(());
    };
    let mut row: crate::model::summary::CatalogRow = from_json(0, &encoded)?;
    if row.cost.pricing_fingerprint == table.fingerprint {
        return Ok(());
    }
    let Some(cost) = retained_cost(tx, &row, table)? else {
        return Ok(());
    };
    let old = row.cost.clone();
    row.cost = cost.clone();
    if let Some(query) = &mut row.query {
        replace_costs(query, &old, &cost)?;
    }
    tx.execute(
        "UPDATE session_catalog SET metadata=?2 WHERE session_key=?1",
        params![key, json(&row)?],
    )?;
    // Same-generation history publications can share the recomputation. An
    // older/different history cohort remains at its original price fingerprint.
    if let Some(encoded) = crate::history_projection::catalog(tx, &key)? {
        let mut history: crate::model::summary::CatalogRow = from_json(0, &encoded)?;
        if history.sources == row.sources {
            history.cost = cost.clone();
            if let Some(query) = &mut history.query {
                replace_costs(query, &old, &cost)?;
            }
            tx.execute(
                "UPDATE session_history_catalog SET metadata=?2 WHERE session_key=?1",
                params![key, json(&history)?],
            )?;
        }
    }
    let description:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(description)<=1048576 THEN description ELSE NULL END FROM session_descriptions WHERE session_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
    if let Some(encoded) = description {
        let mut value: Value = from_json(0, &encoded)?;
        // Only replace a summary with the same exact retained source cohort.
        // Otherwise its next normal description build resolves the new table.
        if serde_json::from_value::<Cost>(value["cost"].clone())
            .ok()
            .is_some_and(|found| {
                found.usd == old.usd
                    && found.by_day == old.by_day
                    && found.by_model.len() == old.by_model.len()
                    && found
                        .by_model
                        .iter()
                        .all(|(id, m)| old.by_model.get(id).is_some_and(|o| o.tokens == m.tokens))
            })
        {
            replace_costs(&mut value, &old, &cost)?;
            clear_cost_checks(&mut value);
            tx.execute(
                "UPDATE session_descriptions SET description=?2 WHERE session_key=?1",
                params![key, json(&value)?],
            )?;
        }
    }
    let overview:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(summary)<=1048576 THEN summary ELSE NULL END FROM session_overview WHERE session_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
    let mut content_generation = None;
    if let Some(encoded) = overview {
        let mut value: Value = from_json(0, &encoded)?;
        if value["source_generation"]
            == crate::retention::source_generation(&row.sources)
                .map_err(|_| rusqlite::Error::InvalidQuery)?
        {
            value["home"]["cost"] = serde_json::to_value(cost.usd).map_err(sql_json)?;
            if let Some(days) = value
                .pointer_mut("/activity/cost_by_day")
                .and_then(Value::as_array_mut)
            {
                // Preserve the activity's original retained window; do not
                // manufacture days outside the captured Analytics cohort.
                for day in days {
                    if let Some(time) = day[0].as_i64() {
                        day[1] = serde_json::to_value(
                            cost.by_day
                                .get(&crate::pricing::utc_day(time))
                                .copied()
                                .unwrap_or(0.0),
                        )
                        .map_err(sql_json)?;
                    }
                }
            }
            if let Some(activity) = value.get_mut("activity").filter(|v| v.is_object()) {
                activity["unpriced_models"] =
                    serde_json::to_value(&cost.unpriced_models).map_err(sql_json)?;
            }
            let summary: crate::overview::Summary =
                serde_json::from_value(value.clone()).map_err(sql_json)?;
            content_generation = Some(crate::overview::content_generation(&summary));
            tx.execute(
                "UPDATE session_overview SET summary=?2 WHERE session_key=?1",
                params![key, json(&value)?],
            )?;
            tx.execute("INSERT INTO meta(key,value) VALUES('overview_generation','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)",[])?;
        }
    }
    let trace:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation FROM session_trace_headers WHERE session_key=?1 AND source_generation=?2",params![key,crate::retention::source_generation(&row.sources).map_err(|_|rusqlite::Error::InvalidQuery)?],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((encoded, generation)) = trace {
        let mut value: Value = from_json(0, &encoded)?;
        replace_costs(&mut value, &old, &cost)?;
        clear_cost_checks(&mut value);
        if let Some(generation) = content_generation {
            value["content_generation"] = Value::String(generation);
        }
        // Family totals are recomputed in a separate bounded pass after all
        // own costs have been updated, never mix old and new rates silently.
        value["family_usd"] = Value::Null;
        let generation = format!(
            "{:x}",
            Sha256::digest(json(&(&generation, &value))?.as_bytes())
        );
        tx.execute(
            "UPDATE session_trace_headers SET metadata=?2,generation=?3 WHERE session_key=?1",
            params![key, json(&value)?, generation],
        )?;
    }
    Ok(())
}

fn replace_costs(value: &mut Value, old: &Cost, cost: &Cost) -> rusqlite::Result<()> {
    if let Some(object) = value.as_object_mut() {
        for (key, value) in object {
            if key == "cost" && value.is_object() && value.get("by_model").is_some() {
                let found: Cost = serde_json::from_value(value.clone()).map_err(sql_json)?;
                if found.by_model.len() == old.by_model.len()
                    && found
                        .by_model
                        .iter()
                        .all(|(id, m)| old.by_model.get(id).is_some_and(|o| m.tokens == o.tokens))
                {
                    *value = serde_json::to_value(cost).map_err(sql_json)?;
                }
            } else {
                replace_costs(value, old, cost)?;
            }
        }
    } else if let Some(array) = value.as_array_mut() {
        for value in array {
            replace_costs(value, old, cost)?;
        }
    }
    Ok(())
}
fn clear_cost_checks(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for (key, value) in object {
            if key == "cost_check" {
                // A prior reported/computed comparison is no longer current.
                if let Some(checks) = value.as_array_mut() {
                    for check in checks {
                        check["computed_usd"] = Value::Null;
                        check["ok"] = Value::Null;
                    }
                }
            } else {
                clear_cost_checks(value);
            }
        }
    }
}

fn family_step(tx: &Transaction<'_>, work: &mut Work) -> rusqlite::Result<()> {
    let key:Option<String>=tx.query_row("SELECT session_key FROM session_trace_headers WHERE session_key>?1 ORDER BY session_key LIMIT 1",[&work.session_after],|r|r.get(0)).optional()?;
    let Some(key) = key else {
        work.phase = 3;
        work.finished_at = crate::model::now_ms();
        return Ok(());
    };
    work.session_after = key.clone();
    let witness:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END,t.source_generation FROM session_catalog c JOIN session_trace_headers t USING(session_key) WHERE session_key=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((encoded, generation)) = witness else {
        return Ok(());
    };
    let owner: crate::model::summary::CatalogRow = from_json(0, &encoded)?;
    if crate::retention::source_generation(&owner.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?
        != generation
    {
        return Ok(());
    }
    let mut statement=tx.prepare("WITH RECURSIVE family(id) AS (SELECT ?1 UNION SELECT c.session_key FROM session_catalog c JOIN family f ON c.parent_key=f.id LIMIT 257) SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_extract(metadata,'$.cost.usd') END,CASE WHEN octet_length(metadata)<=1048576 THEN json_extract(metadata,'$.cost.pricing_fingerprint') END FROM family JOIN session_catalog ON session_key=id")?;
    let mut rows = statement.query([&key])?;
    let mut count = 0;
    let mut usd = Some(0.0);
    while let Some(row) = rows.next()? {
        count += 1;
        let amount: Option<f64> = row.get(0)?;
        let fingerprint: Option<String> = row.get(1)?;
        usd = usd
            .zip(amount)
            .filter(|_| fingerprint.as_deref() == Some(work.fingerprint.as_str()))
            .map(|(a, b)| a + b);
    }
    if count > 256 {
        usd = None
    }
    let prior:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation FROM session_trace_headers WHERE session_key=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((encoded, version)) = prior {
        let mut metadata: Value = from_json(0, &encoded)?;
        let amount = serde_json::to_value(usd).map_err(sql_json)?;
        if metadata["family_usd"] != amount {
            metadata["family_usd"] = amount;
            let generation = format!(
                "{:x}",
                Sha256::digest(json(&(&version, &metadata))?.as_bytes())
            );
            tx.execute(
                "UPDATE session_trace_headers SET metadata=?2,generation=?3 WHERE session_key=?1",
                params![key, json(&metadata)?, generation],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::CodexNativeUsage,
        model::summary::{CatalogLifecycle, CatalogRow, CatalogSource},
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "semon-reprice-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn table(provider: &str, id: &str, rate: f64, at: u64) -> Arc<EffectiveTable> {
        let mut fixture: Value =
            serde_json::from_slice(include_bytes!("../../pricing/fixtures/models-dev.json"))
                .unwrap();
        fixture[provider]["models"][id]["cost"]["input"] = Value::from(rate);
        Arc::new(
            crate::pricing::runtime::adapt(&serde_json::to_vec(&fixture).unwrap(), at).unwrap(),
        )
    }
    fn ledger() -> Ledger {
        Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: 10_000,
                modified_ns: 5,
                changed_ns: Some(6),
            },
            offset: 10_000,
            prefix: [1; 32],
            tail: [2; 32],
        }
    }
    fn source(path: &str, ledger: &Ledger) -> CatalogSource {
        CatalogSource {
            path: PathBuf::from(path),
            native_id: "native".into(),
            dev: ledger.stat.dev,
            ino: ledger.stat.ino,
            size: ledger.stat.size,
            modified_ns: ledger.stat.modified_ns,
            changed_ns: ledger.stat.changed_ns,
            offset: ledger.offset,
            prefix_sha256: ledger.prefix,
            tail_sha256: ledger.tail,
            native_role: None,
        }
    }
    fn row(path: &str, ledger: &Ledger, cost: Cost, harness: &str) -> CatalogRow {
        CatalogRow {
            overview: None,
            trace: None,
            query: None,
            lifecycle: CatalogLifecycle::Current,
            key: "owner".into(),
            name: "owner".into(),
            harness: harness.into(),
            kind: "session".into(),
            presentation_kind: None,
            native_ids: vec!["native".into()],
            parent: None,
            parent_source: None,
            repo: None,
            branch: None,
            model: "fixture".into(),
            effort: None,
            start: Some(0),
            last: Some(1),
            tokens: [1.0, 0.0, 0.0],
            cost,
            sources: vec![source(path, ledger)],
        }
    }
    fn publish(store: &SqliteStore, row: &CatalogRow) {
        let c = &store.connection;
        let generation = crate::retention::source_generation(&row.sources).unwrap();
        c.execute(
            "INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,1,?2,?3)",
            params![row.key, row.harness, json(row).unwrap()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO session_descriptions VALUES(?1,2,'fixture',?2)",
            params![
                row.key,
                json(&serde_json::json!({"cost":row.cost})).unwrap()
            ],
        )
        .unwrap();
        c.execute("INSERT INTO session_overview VALUES(?1,1,'off',?2)",params![row.key,json(&serde_json::json!({"source_generation":generation,"home":{"cost":row.cost.usd},"activity":null})).unwrap()]).unwrap();
        c.execute(
            "INSERT INTO session_trace_headers VALUES(?1,'before',?2,?3)",
            params![
                row.key,
                generation,
                json(&serde_json::json!({"cost":row.cost,"details":{"cost":row.cost}})).unwrap()
            ],
        )
        .unwrap();
    }
    fn drain(path: &Path, table: &Arc<EffectiveTable>) {
        for _ in 0..64 {
            if !step_inner(path, Arc::clone(table), false).unwrap() {
                return;
            }
        }
        panic!("bounded fixture did not finish")
    }

    #[test]
    fn unchanged_saved_session_overview_and_summary_reprice_without_native_files_across_restart() {
        let root = root();
        let path = root.join("index.sqlite3");
        let (mut store, _) = SqliteStore::attempt(&path, None).unwrap_or_else(|e| panic!("{e}"));
        let ledger = ledger();
        let native = "/not-present/retained-claude.jsonl";
        let used = MessageUsage {
            model: Some("claude-opus-5-5".into()),
            billing: BillingUsage {
                input: 1_000_000,
                prompt_size: 1_000_000,
                timestamp: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut index = FileIndex::default();
        index.usage_by_id.insert("message".into(), used.clone());
        store
            .write_one(native, None, None, &ledger, &index)
            .unwrap();
        let old = crate::pricing::calculate_cost(&[used], &[]);
        assert_eq!(old.usd, Some(4.0));
        publish(&store, &row(native, &ledger, old, "claude"));
        let before: String = store
            .connection
            .query_row("SELECT group_concat(billing_input) FROM usage", [], |r| {
                r.get(0)
            })
            .unwrap();
        drop(store);
        let first = table("anthropic", "claude-opus-5-5", 8.0, 1);
        // Opening a new SQLite connection on each step is a process-restart
        // simulation. The original native file has never existed.
        drain(&path, &first);
        let c = Connection::open(&path).unwrap();
        let cost: f64 = c
            .query_row(
                "SELECT json_extract(metadata,'$.cost.usd') FROM session_catalog",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cost, 8.0);
        let summary: f64 = c
            .query_row(
                "SELECT json_extract(description,'$.cost.usd') FROM session_descriptions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, 8.0);
        let overview: f64 = c
            .query_row(
                "SELECT json_extract(summary,'$.home.cost') FROM session_overview",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(overview, 8.0);
        assert_eq!(
            c.query_row("SELECT group_concat(billing_input) FROM usage", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            before
        );
        assert_eq!(versions(&c).unwrap().1, Some(CACHE_VERSION));
        assert_eq!(CACHE_VERSION, 27);
        drop(c);
        let second = table("anthropic", "claude-opus-5-5", 12.0, 2);
        drain(&path, &second);
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT json_extract(summary,'$.home.cost') FROM session_overview",
                [],
                |r| r.get::<_, f64>(0)
            )
            .unwrap(),
            12.0
        );
        assert_eq!(
            c.query_row(
                "SELECT json_extract(metadata,'$.cost.pricing_fingerprint') FROM session_catalog",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            second.fingerprint
        );
        drop(c);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalized_codex_cost_dimensions_resume_after_a_fact_batch() {
        let root = root();
        let path = root.join("index.sqlite3");
        let (mut store, _) = SqliteStore::attempt(&path, None).unwrap_or_else(|e| panic!("{e}"));
        let ledger = ledger();
        let native = "/not-present/retained-codex.jsonl";
        let mut index = FileIndex {
            native_usage_observations: Some(Vec::new()),
            ..Default::default()
        };
        for seq in 0..129 {
            let record = serde_json::json!({"type":"token_usage_record","timestamp":"1970-01-01T00:00:00Z","payload":{"thread_id":"thread","session_id":"session","turn_id":format!("turn-{seq}"),"response_id":format!("response-{seq}"),"usage":{"input_tokens":100,"output_tokens":0,"total_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}});
            CodexNativeUsage::observe(
                &mut index.codex_native_usage,
                &record["payload"],
                "gpt-6.1-sol".into(),
                Some(0),
            );
            let bytes = serde_json::to_vec(&record).unwrap();
            index.native_usage_observations.as_mut().unwrap().push(
                native_usage::Observation::capture(&record, &bytes, seq, Some("gpt-6.1-sol"))
                    .unwrap(),
            );
        }
        let old = index.codex_cost().finish();
        publish(&store, &row(native, &ledger, old, "codex"));
        store
            .write_one(native, None, None, &ledger, &index)
            .unwrap();
        let facts: String = store
            .connection
            .query_row(
                "SELECT group_concat(payload) FROM native_usage_facts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(store);
        let table = table("openai", "gpt-6.1-sol", 7.0, 1);
        assert!(step_inner(&path, Arc::clone(&table), false).unwrap());
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT json_extract(value,'$.job.offset') FROM meta WHERE key=?1",
                [WORK],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            127
        );
        drop(c);
        drain(&path, &table);
        let c = Connection::open(&path).unwrap();
        let amount:f64=c.query_row("SELECT json_extract(value,'$.usd') FROM native_usage_dimensions WHERE kind='cost_model'",[],|r|r.get(0)).unwrap();
        assert!((amount - 0.0903).abs() < 1e-12);
        let day: f64 = c
            .query_row(
                "SELECT CAST(value AS REAL) FROM native_usage_dimensions WHERE kind='cost_day'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((day - amount).abs() < 1e-12);
        let overview: f64 = c
            .query_row(
                "SELECT json_extract(summary,'$.home.cost') FROM session_overview",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((overview - amount).abs() < 1e-12);
        assert_eq!(
            c.query_row(
                "SELECT group_concat(payload) FROM native_usage_facts",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            facts
        );
        assert_eq!(versions(&c).unwrap().1, Some(27));
        drop(c);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_changed_retained_facts_keep_the_previous_price_fingerprint() {
        let root = root();
        let path = root.join("index.sqlite3");
        let (store, _) = SqliteStore::attempt(&path, None).unwrap_or_else(|e| panic!("{e}"));
        let cost = crate::pricing::Cost::default();
        let old = cost.pricing_fingerprint.clone();
        publish(&store, &row("/missing/native", &ledger(), cost, "claude"));
        drop(store);
        drain(&path, &table("anthropic", "claude-opus-5-5", 8.0, 1));
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT json_extract(metadata,'$.cost.pricing_fingerprint') FROM session_catalog",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            old
        );
        drop(c);
        fs::remove_dir_all(root).unwrap();
    }
}
