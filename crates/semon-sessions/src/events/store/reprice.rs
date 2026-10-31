//! Resumable cost-only producer. It never opens native bodies, changes source
//! membership, deletes observations, or advances parser/publication witnesses.
use super::*;
use crate::pricing::{Cost, CostBuild, EffectiveTable};
use crate::storage::sqlite::proof_generations::{
    CatalogObservationOwner, ObservationPreparationError, prepare_repriced_observation,
};
use std::sync::Arc;

const WORK: &str = "pricing_recompute_v2";
const BATCH: usize = 128;
const MAX_FACTS: usize = 4096;
const MAX_JSON: usize = 1024 * 1024;
const TARGET: &str = "pricing_recompute_target";
const COMPLETED: &str = "pricing_recompute_completed";

#[cfg(test)]
std::thread_local! {
    static REPRICE_PAYLOAD_DECODES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn decode_json<T: serde::de::DeserializeOwned>(column: usize, value: &str) -> rusqlite::Result<T> {
    #[cfg(test)]
    REPRICE_PAYLOAD_DECODES.with(|count| count.set(count.get() + 1));
    from_json(column, value)
}

const COMPOSITE_QUEUE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pricing_recompute_pending (
    namespace TEXT NOT NULL,
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('source','session','family')),
    owner_key TEXT NOT NULL,
    PRIMARY KEY(namespace,owner_kind,owner_key)
) STRICT, WITHOUT ROWID;
";

pub(super) fn queue_schema() -> &'static str {
    COMPOSITE_QUEUE_SCHEMA
}

const CATALOG_QUEUE_11: &str = "
CREATE INDEX IF NOT EXISTS session_catalog_pricing_fingerprint
    ON session_catalog(namespace,session_key,pricing_fingerprint);
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_insert
AFTER INSERT ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'session',NEW.session_key FROM session_catalog c,meta m WHERE c.namespace=NEW.namespace AND c.session_key=NEW.session_key AND m.namespace='' AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_update
AFTER UPDATE OF metadata ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'session',NEW.session_key FROM session_catalog c,meta m WHERE c.namespace=NEW.namespace AND c.session_key=NEW.session_key AND m.namespace='' AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
";

pub(super) fn catalog_schema() -> &'static str {
    CATALOG_QUEUE_11
}

const COMPOSITE_FILE_QUEUE_1: &str = "
CREATE INDEX IF NOT EXISTS files_codex_pricing_fingerprint
    ON files(namespace,file_id,codex_pricing_fingerprint)
    WHERE codex_native_usage IS NOT NULL;
CREATE TRIGGER IF NOT EXISTS pricing_pending_file_insert
AFTER INSERT ON files
WHEN NEW.codex_native_usage IS NOT NULL
BEGIN
    UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'source',NEW.path FROM files f,meta m WHERE f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.namespace='' AND m.key='pricing_recompute_target'
      AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_file_update
AFTER UPDATE OF codex_native_usage ON files
BEGIN
    UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'source',NEW.path FROM files f,meta m WHERE NEW.codex_native_usage IS NOT NULL AND f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.namespace='' AND m.key='pricing_recompute_target'
      AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_native_usage_ready
AFTER UPDATE OF usage_ready ON native_usage_sources
WHEN NEW.usage_ready=1
BEGIN
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'source',NEW.source_path FROM files f,meta m WHERE f.namespace=NEW.namespace AND f.path=NEW.source_path AND m.namespace='' AND m.key='pricing_recompute_target'
      AND f.codex_native_usage IS NOT NULL AND f.codex_pricing_fingerprint IS NOT m.value;
END;
";

pub(super) fn file_schema() -> &'static str {
    COMPOSITE_FILE_QUEUE_1
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Work {
    target_fingerprint: String,
    completed_fingerprint: String,
    fetched_at: u64,
    phase: u8,
    file_after: i64,
    session_after: String,
    job: Option<SourceJob>,
    pending_owner: Option<PendingOwner>,
}
#[derive(Serialize, Deserialize)]
struct PendingOwner {
    kind: String,
    key: String,
}
#[derive(Serialize, Deserialize)]
struct SourceJob {
    id: i64,
    file_id: i64,
    receipt: String,
    offset: i64,
    cost: CostBuild,
}

/// One bounded step. Each transaction holds at most one source fact batch or
/// one qualified session's metadata. Durable cursors resume after process exit.
pub(crate) fn step(
    owner: &configured::NativeOwnership,
    table: Arc<EffectiveTable>,
) -> rusqlite::Result<bool> {
    step_inner(owner, table, true)
}

#[cfg(test)]
pub(crate) fn drain_for_store_test(
    owner: &configured::NativeOwnership,
    table: Arc<EffectiveTable>,
) -> rusqlite::Result<()> {
    for _ in 0..64 {
        if !step_inner(owner, Arc::clone(&table), false)? {
            return Ok(());
        }
    }
    Err(rusqlite::Error::InvalidQuery)
}

fn owner_error(error: crate::storage::StorageError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn preparation_error(error: ObservationPreparationError) -> rusqlite::Error {
    match error {
        ObservationPreparationError::Evidence(error) => owner_error(error),
        ObservationPreparationError::Sql(error) => error,
    }
}

fn idle(connection: &Connection, fingerprint: &str) -> rusqlite::Result<bool> {
    let (target, completed, pending): (Option<String>, Option<String>, bool) = connection
        .query_row(PROGRESS_SQL, params![TARGET, COMPLETED], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    Ok(target.as_deref() == Some(fingerprint)
        && completed.as_deref() == Some(fingerprint)
        && !pending)
}

const PROGRESS_SQL: &str = "SELECT (SELECT value FROM meta WHERE namespace='' AND key=?1),(SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?2),EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace())";

fn step_inner(
    owner: &configured::NativeOwnership,
    table: Arc<EffectiveTable>,
    check_current: bool,
) -> rusqlite::Result<bool> {
    let mut connection =
        crate::storage::sqlite::semantic::pricing_connection(owner).map_err(owner_error)?;
    owner.validate_ready(&connection).map_err(owner_error)?;
    // A completed owner needs only metadata reads and never decodes payloads.
    if idle(&connection, &table.fingerprint)? {
        return Ok(false);
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    owner.validate_ready(&tx).map_err(owner_error)?;
    let (current_target, current_completed, has_pending): (Option<String>, Option<String>, bool) =
        tx.query_row(PROGRESS_SQL, params![TARGET, COMPLETED], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if current_target.as_deref() == Some(table.fingerprint.as_str())
        && current_completed.as_deref() == Some(table.fingerprint.as_str())
        && !has_pending
    {
        return Ok(false);
    }
    let encoded: Option<String> = tx
        .query_row(native_metadata_value(&tx)?, [WORK], |r| r.get(0))
        .optional()?;
    let mut work: Work = encoded
        .as_deref()
        .map(|s| decode_json(0, s))
        .transpose()?
        .unwrap_or_default();
    // A second process using an older last-known-good table may not roll a
    // newer process's completed/partial work back to its older fetch.
    if work.fetched_at > table.fetched_at {
        return Ok(false);
    }
    if work.target_fingerprint != table.fingerprint {
        work = Work {
            target_fingerprint: table.fingerprint.clone(),
            fetched_at: table.fetched_at,
            ..Work::default()
        };
    }
    if current_target.as_deref() != Some(table.fingerprint.as_str()) {
        tx.execute(
            native_metadata_put(&tx)?,
            params![TARGET, table.fingerprint],
        )?;
    }
    if work.phase == 3
        && work.completed_fingerprint == table.fingerprint
        && current_completed.as_deref() != Some(table.fingerprint.as_str())
    {
        tx.execute(
            native_metadata_put(&tx)?,
            params![COMPLETED, table.fingerprint],
        )?;
    }
    let binding = crate::storage::QueryRuntimeBinding {
        namespace: owner.config().namespace.clone(),
        admission: owner.admission(),
        stream_epoch: owner
            .config()
            .expected_stream_epoch
            .clone()
            .ok_or(rusqlite::Error::InvalidQuery)?,
    };
    tx.execute_batch("SAVEPOINT pricing_owner")?;
    let full_pass = work.completed_fingerprint != table.fingerprint;
    let migrated_completion = !full_pass
        && work.phase == 3
        && current_completed.as_deref() != Some(table.fingerprint.as_str());
    let result = if full_pass {
        match work.phase {
            0 => source_step(&tx, &mut work, &table, None).map(|_| true),
            1 => catalog_step(&tx, &binding, &mut work, &table, None).map(|_| true),
            _ => family_step(&tx, &mut work).map(|_| true),
        }
    } else {
        pending_step(&tx, &binding, &mut work, &table)
    };
    let mut made_progress = migrated_completion;
    match result {
        Ok(progress) => {
            made_progress |= progress;
            tx.execute_batch("RELEASE pricing_owner")?;
        }
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
            made_progress = true;
            if let Some(owner) = work.pending_owner.take() {
                tx.execute(
                    "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind=?1 AND owner_key=?2",
                    params![owner.kind, owner.key],
                )?;
            }
        }
        Err(error) => return Err(error),
    }
    // A stale job never publishes after the process has swapped its table.
    if check_current && crate::pricing::snapshot().fingerprint != table.fingerprint {
        return Ok(false);
    }
    if !full_pass && !made_progress {
        return Ok(false);
    }
    if full_pass && work.phase == 3 {
        work.completed_fingerprint = table.fingerprint.clone();
        tx.execute(
            native_metadata_put(&tx)?,
            params![COMPLETED, table.fingerprint],
        )?;
    }
    let mut value = json(&work)?;
    if value.len() > MAX_JSON {
        // Keep the old source cost for an over-budget accumulator, then
        // advance to unrelated sources on the next attempt.
        work.job = None;
        if let Some(owner) = work.pending_owner.take() {
            tx.execute(
                "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind=?1 AND owner_key=?2",
                params![owner.kind, owner.key],
            )?;
        }
        value = json(&work)?;
    }
    tx.execute(native_metadata_put(&tx)?, params![WORK, value])?;
    tx.commit()?;
    Ok(true)
}

fn source_step(
    tx: &Transaction<'_>,
    work: &mut Work,
    table: &Arc<EffectiveTable>,
    pending_id: Option<i64>,
) -> rusqlite::Result<bool> {
    if pending_id.is_some_and(|id| work.job.as_ref().is_some_and(|job| job.id != id)) {
        // The pending owner now resolves to another retained source generation.
        // Discard the partial accumulator but keep the queue row for a fresh job.
        work.job = None;
        return Ok(false);
    }
    if work.job.is_none() {
        let next: Option<(i64, String, i64, Option<String>)> = if let Some(id) = pending_id {
            tx.query_row("SELECT s.source_id,s.receipt,f.file_id,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM native_usage_sources s JOIN native_usage_current n USING(namespace,source_id) JOIN files f ON f.namespace=n.namespace AND f.path=n.source_path WHERE s.namespace=semon_namespace() AND s.source_id=?1 AND s.parser_version=?2 AND s.usage_ready=1 AND f.codex_pricing_fingerprint IS NOT ?3",params![id,CACHE_VERSION,table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
        } else {
            tx.query_row("SELECT s.source_id,s.receipt,f.file_id,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM files f INDEXED BY files_codex_pricing_fingerprint JOIN native_usage_current n ON n.namespace=f.namespace AND n.source_path=f.path JOIN native_usage_sources s ON s.namespace=n.namespace AND s.source_id=n.source_id WHERE f.namespace=semon_namespace() AND f.codex_native_usage IS NOT NULL AND f.file_id>?1 AND s.parser_version=?2 AND s.usage_ready=1 AND f.codex_pricing_fingerprint IS NOT ?3 ORDER BY f.file_id LIMIT 1",params![work.file_after,CACHE_VERSION,table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
        };
        let Some((id, receipt, file_id, encoded)) = next else {
            if pending_id.is_none() {
                work.phase = 1;
                work.session_after.clear();
            }
            return Ok(true);
        };
        if pending_id.is_none() {
            work.file_after = file_id;
        }
        let Some(encoded) = encoded else {
            return Ok(true);
        };
        let root: Value = decode_json(0, &encoded)?;
        let fingerprint = root["cost"]["pricing_fingerprint"].as_str();
        tx.execute(
            "UPDATE files SET codex_pricing_fingerprint=?2 WHERE namespace=semon_namespace() AND path=(SELECT source_path FROM native_usage_current WHERE namespace=semon_namespace() AND source_id=?1)",
            params![id, fingerprint],
        )?;
        if root["projection_version"] != 1 {
            return Ok(true);
        }
        work.job = Some(SourceJob {
            id,
            file_id,
            receipt,
            offset: -1,
            cost: CostBuild::with_table(Arc::clone(table)),
        });
    }
    let job = work.job.as_mut().expect("source selected");
    job.cost.bind_table(Arc::clone(table));
    let current: Option<(String,String)>=tx.query_row("SELECT s.receipt,f.codex_native_usage FROM native_usage_sources s JOIN native_usage_current n USING(namespace,source_id) JOIN files f ON f.namespace=n.namespace AND f.path=n.source_path WHERE s.namespace=semon_namespace() AND s.source_id=?1 AND octet_length(f.codex_native_usage)<=1048576",[job.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((receipt, encoded)) = current else {
        work.job = None;
        return Ok(pending_id.is_none());
    };
    if receipt != job.receipt {
        if pending_id.is_none() {
            work.file_after = job.file_id.saturating_sub(1);
        }
        work.job = None;
        return Ok(pending_id.is_none());
    }
    let mut statement=tx.prepare("SELECT native_offset,CASE WHEN octet_length(payload)<=8192 THEN payload ELSE NULL END FROM native_usage_facts WHERE namespace=semon_namespace() AND source_id=?1 AND native_offset>?2 AND model_outcome='accepted' ORDER BY native_offset LIMIT ?3")?;
    let rows = statement.query_map(params![job.id, job.offset, BATCH as i64], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut count = 0;
    for row in rows {
        let (offset, payload) = row?;
        let payload: native_usage::Payload = decode_json(1, &payload)?;
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
        return Ok(false);
    }
    let mut root: Value = decode_json(0, &encoded)?;
    if root["projection_version"] != 1 {
        work.job = None;
        return Ok(true);
    }
    // A source with no complete model facts must not become a free source.
    if job.cost.models.is_empty() && root["tokens"]["total"].as_u64().is_some_and(|n| n > 0) {
        work.job = None;
        return Ok(true);
    }
    let mut expected = BTreeMap::new();
    let mut models=tx.prepare("SELECT dimension,CASE WHEN octet_length(value)<=8192 THEN value ELSE NULL END FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 AND kind='model' ORDER BY dimension LIMIT 4097")?;
    for row in models.query_map([job.id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (name, value) = row?;
        let tokens: ModelTokens = decode_json(1, &value)?;
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
        return Ok(true);
    }
    // Replace all derived dimensions together. The native totals/receipts and
    // every original fact remain byte-for-byte unchanged.
    tx.execute("DELETE FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 AND kind IN ('cost_model','cost_day')",[job.id])?;
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
    tx.execute("UPDATE files SET codex_native_usage=?2 WHERE namespace=semon_namespace() AND path=(SELECT source_path FROM native_usage_current WHERE namespace=semon_namespace() AND source_id=?1)",params![job.id,json(&root)?])?;
    work.job = None;
    Ok(true)
}

fn put_dimension<T: Serialize>(
    tx: &Transaction<'_>,
    id: i64,
    kind: &str,
    key: &str,
    value: &T,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO native_usage_dimensions VALUES(semon_namespace(),?1,?2,?3,?4)",
        params![id, kind, key, json(value)?],
    )?;
    Ok(())
}
fn sql_json(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

/// Reprice only exact retained source generations and complete retained token
/// facts. Over-budget or incomplete owners keep their older fingerprint.
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
            let mut statement=tx.prepare(&format!("SELECT {USAGE_COLUMNS} FROM usage WHERE {}file_id=?1 AND octet_length(record_ids)<=65536 AND length(COALESCE(model,''))<=512 LIMIT ?2", "namespace=semon_namespace() AND "))?;
            let total: i64 = tx.query_row(
                "SELECT COUNT(*) FROM (SELECT 1 FROM usage WHERE namespace=semon_namespace() AND file_id=?1 LIMIT ?2)",
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
                if let Some(prior) = messages.get(&id)
                    && (prior.model != message.model || prior.billing != message.billing)
                {
                    return Ok(None);
                }
                messages.insert(id, message);
            }
            if loaded != total as usize {
                return Ok(None);
            }
            facts += loaded;
        } else {
            let encoded:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(codex_native_usage)<=1048576 THEN codex_native_usage ELSE NULL END FROM files WHERE namespace=semon_namespace() AND file_id=?1",[file_id],|r|r.get(0))?;
            if let Some(encoded) = encoded {
                if let Some((id, _)) = native_usage::source(tx, path)? {
                    let count:i64=tx.query_row("SELECT COUNT(*) FROM (SELECT 1 FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 LIMIT 4097)",[id],|r|r.get(0))?;
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
                let mut statement=tx.prepare("SELECT model,input,output,cache_write,cache_read,t FROM codex_usage WHERE namespace=semon_namespace() AND file_id=?1 ORDER BY seq LIMIT ?2")?;
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
    binding: &crate::storage::QueryRuntimeBinding,
    work: &mut Work,
    table: &Arc<EffectiveTable>,
    pending_key: Option<&str>,
) -> rusqlite::Result<bool> {
    let next: Option<(String, Option<String>)> = if let Some(key) = pending_key {
        tx.query_row("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE namespace=semon_namespace() AND session_key=?1 AND pricing_fingerprint IS NOT ?2",params![key,table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
    } else {
        tx.query_row("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog INDEXED BY session_catalog_pricing_fingerprint WHERE namespace=semon_namespace() AND pricing_fingerprint IS NOT ?1 AND session_key>?2 ORDER BY session_key LIMIT 1",params![table.fingerprint,work.session_after],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
    };
    let Some((key, encoded)) = next else {
        if pending_key.is_none() {
            work.phase = 2;
            work.session_after.clear();
        }
        return Ok(false);
    };
    if pending_key.is_none() {
        work.session_after = key.clone();
    }
    let Some(encoded) = encoded else {
        return Ok(false);
    };
    let mut row: crate::model::summary::CatalogRow = decode_json(0, &encoded)?;
    if row.cost.pricing_fingerprint == table.fingerprint {
        tx.execute(
            "UPDATE session_catalog SET pricing_fingerprint=?2 WHERE namespace=semon_namespace() AND session_key=?1",
            params![key, table.fingerprint],
        )?;
        return Ok(false);
    }
    let Some(cost) = retained_cost(tx, &row, table)? else {
        return Ok(false);
    };
    let original = row.clone();
    let old = row.cost.clone();
    row.cost = cost.clone();
    if let Some(query) = &mut row.query {
        replace_costs(query, &old, &cost)?;
    }
    prepare_repriced_observation(
        tx,
        binding,
        CatalogObservationOwner::Catalog,
        &original,
        &cost,
        row.query.as_ref(),
    )
    .map_err(preparation_error)?
    .publish(tx)?;
    // Same-generation history publications can share the recomputation. An
    // older/different history cohort remains at its original price fingerprint.
    if let Some(encoded) = crate::history_projection::catalog(tx, &key)? {
        let mut history: crate::model::summary::CatalogRow = decode_json(0, &encoded)?;
        if history.sources == row.sources {
            let original_history = history.clone();
            history.cost = cost.clone();
            if let Some(query) = &mut history.query {
                replace_costs(query, &old, &cost)?;
            }
            match prepare_repriced_observation(
                tx,
                binding,
                CatalogObservationOwner::History,
                &original_history,
                &cost,
                history.query.as_ref(),
            ) {
                Ok(prepared) => prepared.publish(tx)?,
                Err(ObservationPreparationError::Evidence(
                    crate::storage::StorageError::ScopeChanged
                    | crate::storage::StorageError::Unavailable
                    | crate::storage::StorageError::HistoryRetentionBounds,
                )) => {}
                Err(error) => return Err(preparation_error(error)),
            }
        }
    }
    let description:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(description)<=1048576 THEN description ELSE NULL END FROM session_descriptions WHERE namespace=semon_namespace() AND session_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
    if let Some(encoded) = description {
        let mut value: Value = decode_json(0, &encoded)?;
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
                "UPDATE session_descriptions SET description=?2 WHERE namespace=semon_namespace() AND session_key=?1",
                params![key, json(&value)?],
            )?;
        }
    }
    let overview:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(summary)<=1048576 THEN summary ELSE NULL END FROM session_overview WHERE namespace=semon_namespace() AND session_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
    let mut content_generation = None;
    if let Some(encoded) = overview {
        let mut value: Value = decode_json(0, &encoded)?;
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
                        // The base Activity series stores numeric partial
                        // amounts and carries unknown status separately. If a
                        // retained day has no recomputed fact, keep its prior
                        // value rather than manufacturing a free zero.
                        if let Some(amount) = cost.by_day.get(&crate::pricing::utc_day(time)) {
                            day[1] = serde_json::to_value(amount).map_err(sql_json)?;
                        }
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
                "UPDATE session_overview SET summary=?2 WHERE namespace=semon_namespace() AND session_key=?1",
                params![key, json(&value)?],
            )?;
            tx.execute(metadata_increment(tx)?, ["overview_generation"])?;
        }
    }
    let trace:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation FROM session_trace_headers WHERE namespace=semon_namespace() AND session_key=?1 AND source_generation=?2",params![key,crate::retention::source_generation(&row.sources).map_err(|_|rusqlite::Error::InvalidQuery)?],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((encoded, generation)) = trace {
        let mut value: Value = decode_json(0, &encoded)?;
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
            "UPDATE session_trace_headers SET metadata=?2,generation=?3 WHERE namespace=semon_namespace() AND session_key=?1",
            params![key, json(&value)?, generation],
        )?;
    }
    Ok(true)
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

fn pending_step(
    tx: &Transaction<'_>,
    binding: &crate::storage::QueryRuntimeBinding,
    work: &mut Work,
    table: &Arc<EffectiveTable>,
) -> rusqlite::Result<bool> {
    if let Some(owner) = work.pending_owner.as_ref() {
        let still_pending: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind=?1 AND owner_key=?2)",
            params![owner.kind, owner.key],
            |row| row.get(0),
        )?;
        if !still_pending {
            work.pending_owner = None;
            work.job = None;
        }
    }
    if work.pending_owner.is_none() {
        work.pending_owner = tx
            .query_row(
                "SELECT owner_kind,owner_key FROM pricing_recompute_pending WHERE namespace=semon_namespace() ORDER BY owner_kind,owner_key LIMIT 1",
                [],
                |row| Ok(PendingOwner { kind: row.get(0)?, key: row.get(1)? }),
            )
            .optional()?;
    }
    let Some(owner) = work.pending_owner.as_ref() else {
        return Ok(false);
    };
    let kind = owner.kind.clone();
    let key = owner.key.clone();
    match kind.as_str() {
        "source" => {
            let id: Option<i64> = tx
                .query_row(
                    "SELECT s.source_id FROM native_usage_current c JOIN native_usage_sources s USING(namespace,source_id) WHERE c.namespace=semon_namespace() AND c.source_path=?1 AND s.parser_version=?2 AND s.usage_ready=1",
                    params![key, CACHE_VERSION],
                    |row| row.get(0),
                )
                .optional()?;
            let done = if let Some(id) = id {
                source_step(tx, work, table, Some(id))?
            } else {
                // The source is not ready/current yet; pricing_pending_native_usage_ready
                // re-enqueues it when retained usage becomes ready.
                true
            };
            if done {
                tx.execute(
                    "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='source' AND owner_key=?1",
                    [&key],
                )?;
                work.pending_owner = None;
            }
            // A non-final fact batch still made bounded progress and must
            // persist its offset while retaining the queue row.
            Ok(true)
        }
        "session" => {
            let changed = catalog_step(tx, binding, work, table, Some(&key))?;
            if changed {
                tx.execute(
                    "INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key) SELECT semon_namespace(),'family',?1",
                    [&key],
                )?;
            }
            tx.execute(
                "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='session' AND owner_key=?1",
                [&key],
            )?;
            work.pending_owner = None;
            Ok(true)
        }
        "family" => {
            family_owner(tx, &key, &table.fingerprint)?;
            let parent: Option<String> = tx
                .query_row(
                    "SELECT parent_key FROM session_catalog WHERE namespace=semon_namespace() AND session_key=?1",
                    [&key],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if let Some(parent) = parent {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_trace_headers WHERE namespace=semon_namespace() AND session_key=?1)",
                    [&parent],
                    |r| r.get(0),
                )?;
                if exists {
                    tx.execute(
                        "INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key) SELECT semon_namespace(),'family',?1",
                        [&parent],
                    )?;
                }
            }
            tx.execute(
                "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='family' AND owner_key=?1",
                [&key],
            )?;
            work.pending_owner = None;
            Ok(true)
        }
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn family_step(tx: &Transaction<'_>, work: &mut Work) -> rusqlite::Result<()> {
    let key:Option<String>=tx.query_row("SELECT session_key FROM session_trace_headers WHERE namespace=semon_namespace() AND session_key>?1 ORDER BY session_key LIMIT 1",[&work.session_after],|r|r.get(0)).optional()?;
    let Some(key) = key else {
        work.phase = 3;
        return Ok(());
    };
    work.session_after = key.clone();
    family_owner(tx, &key, &work.target_fingerprint)
}

fn family_owner(tx: &Transaction<'_>, key: &str, fingerprint: &str) -> rusqlite::Result<()> {
    let witness:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END,t.source_generation FROM session_catalog c JOIN session_trace_headers t USING(namespace,session_key) WHERE c.namespace=semon_namespace() AND session_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((encoded, generation)) = witness else {
        return Ok(());
    };
    let owner: crate::model::summary::CatalogRow = decode_json(0, &encoded)?;
    if crate::retention::source_generation(&owner.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?
        != generation
    {
        return Ok(());
    }
    let mut statement=tx.prepare("WITH RECURSIVE family(id) AS (SELECT ?1 UNION SELECT c.session_key FROM session_catalog c JOIN family f ON c.parent_key=f.id WHERE c.namespace=semon_namespace() LIMIT 257) SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_extract(metadata,'$.cost.usd') END,CASE WHEN octet_length(metadata)<=1048576 THEN json_extract(metadata,'$.cost.pricing_fingerprint') END FROM family JOIN session_catalog ON session_key=id WHERE namespace=semon_namespace()")?;
    let mut rows = statement.query([&key])?;
    let mut count = 0;
    let mut usd = Some(0.0);
    while let Some(row) = rows.next()? {
        count += 1;
        let amount: Option<f64> = row.get(0)?;
        let owner_fingerprint: Option<String> = row.get(1)?;
        usd = usd
            .zip(amount)
            .filter(|_| owner_fingerprint.as_deref() == Some(fingerprint))
            .map(|(a, b)| a + b);
    }
    if count > 256 {
        usd = None
    }
    let prior:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation FROM session_trace_headers WHERE namespace=semon_namespace() AND session_key=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((encoded, version)) = prior {
        let mut metadata: Value = decode_json(0, &encoded)?;
        let amount = serde_json::to_value(usd).map_err(sql_json)?;
        if metadata["family_usd"] != amount {
            metadata["family_usd"] = amount;
            let generation = format!(
                "{:x}",
                Sha256::digest(json(&(&version, &metadata))?.as_bytes())
            );
            tx.execute(
                "UPDATE session_trace_headers SET metadata=?2,generation=?3 WHERE namespace=semon_namespace() AND session_key=?1",
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
    fn current_store(
        path: &Path,
        namespace: &crate::storage::LogicalSourceNamespace,
    ) -> SqliteStore {
        let mut store = crate::events::store::current_test_store(path, namespace);
        store.connection =
            crate::storage::sqlite::semantic::pricing_connection(&store.owner).unwrap();
        store
    }
    fn publish(store: &SqliteStore, row: &CatalogRow) {
        let c = &store.connection;
        let generation = crate::retention::source_generation(&row.sources).unwrap();
        c.execute(
            "INSERT INTO session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(semon_namespace(),?1,1,?2,?3)",
            params![row.key, row.harness, json(row).unwrap()],
        )
        .unwrap();
        crate::storage::sqlite::proof_generations::native_catalog_observation_grant(
            c,
            store.owner.config().namespace.as_str(),
            &row.key,
            store.owner.admission(),
            store
                .owner
                .config()
                .expected_stream_epoch
                .as_deref()
                .unwrap(),
            &json(row).unwrap(),
            None,
        )
        .unwrap();
        c.execute(
            "INSERT INTO session_descriptions(namespace,session_key,version,inputs,description) VALUES(semon_namespace(),?1,2,'fixture',?2)",
            params![
                row.key,
                json(&serde_json::json!({"cost":row.cost})).unwrap()
            ],
        )
        .unwrap();
        c.execute("INSERT INTO session_overview(namespace,session_key,last_ms,state,summary) VALUES(semon_namespace(),?1,1,'off',?2)",params![row.key,json(&serde_json::json!({"source_generation":generation,"home":{"cost":row.cost.usd},"activity":null})).unwrap()]).unwrap();
        c.execute(
            "INSERT INTO session_trace_headers(namespace,session_key,generation,source_generation,metadata) VALUES(semon_namespace(),?1,'before',?2,?3)",
            params![
                row.key,
                generation,
                json(&serde_json::json!({"cost":row.cost,"details":{"cost":row.cost}})).unwrap()
            ],
        )
        .unwrap();
    }
    fn owner(path: &Path) -> configured::NativeOwnership {
        configured::NativeOwnership::restore(&crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: path.to_owned(),
            },
            namespace: crate::events::store::fixture_namespace(),
            expected_stream_epoch: Some("a".repeat(64)),
        })
        .unwrap()
    }
    fn drain(path: &Path, table: &Arc<EffectiveTable>) {
        for _ in 0..64 {
            if !step_inner(&owner(path), Arc::clone(table), false).unwrap() {
                return;
            }
        }
        panic!("bounded fixture did not finish")
    }

    fn codex_index(model: &str, facts: u64) -> FileIndex {
        let mut index = FileIndex {
            native_usage_observations: Some(Vec::new()),
            ..Default::default()
        };
        for seq in 0..facts {
            let record = serde_json::json!({"type":"token_usage_record","timestamp":"1970-01-01T00:00:00Z","payload":{"thread_id":"thread","session_id":"session","turn_id":format!("turn-{seq}"),"response_id":format!("response-{seq}"),"usage":{"input_tokens":100,"output_tokens":0,"total_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}});
            CodexNativeUsage::observe(
                &mut index.codex_native_usage,
                &record["payload"],
                model.to_owned(),
                Some(0),
            );
            let bytes = serde_json::to_vec(&record).unwrap();
            index.native_usage_observations.as_mut().unwrap().push(
                native_usage::Observation::capture(&record, &bytes, seq, Some(model)).unwrap(),
            );
        }
        index
    }

    fn mark_file_stale(
        connection: &Connection,
        path: &str,
        fingerprint: &str,
        total_tokens: Option<u64>,
    ) {
        let encoded: String = connection
            .query_row(
                "SELECT codex_native_usage FROM files WHERE path=?1",
                [path],
                |row| row.get(0),
            )
            .unwrap();
        let mut root: Value = serde_json::from_str(&encoded).unwrap();
        root["cost"]["pricing_fingerprint"] = Value::String(fingerprint.to_owned());
        if let Some(total) = total_tokens {
            root["tokens"]["total"] = Value::from(total);
        }
        connection
            .execute(
                "UPDATE files SET codex_native_usage=?2 WHERE path=?1",
                params![path, json(&root).unwrap()],
            )
            .unwrap();
    }

    fn source_facts(
        connection: &Connection,
        id: i64,
    ) -> Vec<(i64, String, String, String, String)> {
        let mut statement = connection
            .prepare(
                "SELECT native_offset,identity,payload,record,outcome||':'||model_outcome FROM native_usage_facts WHERE source_id=?1 ORDER BY native_offset",
            )
            .unwrap();
        statement
            .query_map([id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn pending_source_count(connection: &Connection, path: &str) -> i64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM pricing_recompute_pending WHERE owner_kind='source' AND owner_key=?1",
                [path],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn bind_fixture(db: &Connection, namespace: &str) {
        configured::bind_metadata_scope(db, namespace).unwrap();
        let namespace = namespace.to_owned();
        db.create_scalar_function(
            "semon_namespace",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| Ok(namespace.clone()),
        )
        .unwrap();
    }

    fn usage_family_state(db: &Connection, namespace: &str) -> Vec<Vec<rusqlite::types::Value>> {
        let mut state = Vec::new();
        for table in [
            "files",
            "events",
            "signals",
            "usage_identity",
            "usage",
            "codex_usage",
            "native_usage_sources",
            "native_usage_current",
            "native_usage_facts",
            "native_usage_identities",
            "native_usage_dimensions",
            "native_tree_repairs",
            "native_tree_generations",
            "native_tree_usage_work",
            "native_tree_codex_work",
            "native_tree_headers",
            "native_tree_models",
            "native_tree_labels",
            "native_tree_tools",
            "native_tree_usage_models",
            "native_tree_copied_identities",
            "pricing_recompute_pending",
        ] {
            let mut statement = db
                .prepare(&format!(
                    "SELECT * FROM {table} WHERE namespace=?1 ORDER BY 1,2,3"
                ))
                .unwrap();
            let columns = statement.column_count();
            state.extend(
                statement
                    .query_map([namespace], |r| {
                        (0..columns)
                            .map(|i| r.get(i))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap(),
            );
        }
        state
    }

    fn catalog_family_state(db: &Connection) -> Vec<(String, String, String)> {
        db.prepare("SELECT session_key,metadata,coalesce(pricing_fingerprint,'') FROM session_catalog ORDER BY session_key,metadata")
            .unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    fn trace_family_state(db: &Connection) -> Vec<(String, String, String)> {
        db.prepare("SELECT session_key,generation,metadata FROM session_trace_headers ORDER BY session_key,metadata")
            .unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    fn foreign_trace_state(db: &Connection) -> Vec<(String, String, String)> {
        db.prepare("SELECT session_key,generation,metadata FROM session_trace_headers WHERE namespace='b' ORDER BY session_key")
            .unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    fn foreign_change_state(db: &Connection) -> Vec<(i64, String, String)> {
        db.prepare("SELECT revision,session_key,read_scope FROM main.session_catalog_changes WHERE namespace='b' ORDER BY revision").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    #[test]
    fn native_usage_authored_assembly_preserves_factories_and_complete_dependencies() {
        // Both production factories now author the same current contract.
        let original = authored_storage_ddl();
        let configured_original = original;
        assert_eq!(authored_storage_ddl(), original);
        let full_objects = |db: &Connection| {
            db.prepare("SELECT type,name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name").unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))).unwrap()
                .collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        let configured_reference = Connection::open_in_memory().unwrap();
        tree_state::register(&configured_reference).unwrap();
        for ddl in configured_original {
            configured_reference.execute_batch(ddl).unwrap();
        }
        let directory = root();
        let config = crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: directory.join("configured.sqlite"),
            },
            namespace: crate::storage::LogicalSourceNamespace::new("a").unwrap(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        let database = config.backend.sqlite_database().unwrap();
        let configured_actual = Connection::open(database).unwrap();
        assert_eq!(
            full_objects(&configured_actual),
            full_objects(&configured_reference),
            "actual factory columns/indexes/triggers must match authored CONFIGURED DDL"
        );
        drop(configured_actual);
        fs::remove_dir_all(directory).unwrap();
        let reference = Connection::open_in_memory().unwrap();
        tree_state::register(&reference).unwrap();
        for ddl in original {
            reference.execute_batch(ddl).unwrap();
        }
        let objects = |db: &Connection| {
            db.prepare("SELECT type,name FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name").unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap()
                .collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        // Every table, index and trigger from all 22 dependencies is required,
        // including history, overview, support, slots, search and semantic stage.
        let required_objects = objects(&reference);
        for _trace in [true] {
            let db = Connection::open_in_memory().unwrap();
            bind_fixture(&db, "a");
            db.pragma_update(None, "foreign_keys", true).unwrap();
            for ddl in authored_storage_ddl() {
                db.execute_batch(ddl).unwrap();
            }
            assert_eq!(objects(&db), required_objects);
            assert!(current_objects_present(&db).unwrap());
            // A matching object name cannot conceal a malformed
            // index, trigger body or table in this current profile.
            for drift in [
                "DROP INDEX files_cwd; CREATE INDEX files_cwd ON files(path);",
                "DROP TRIGGER catalog_changes_delete; CREATE TRIGGER catalog_changes_delete AFTER DELETE ON session_catalog BEGIN SELECT 1; END;",
                "ALTER TABLE native_usage_sources ADD COLUMN unexpected TEXT;",
            ] {
                let tx = db.unchecked_transaction().unwrap();
                tx.execute_batch(drift).unwrap();
                let before = tx.total_changes();
                assert!(!current_objects_present(&tx).unwrap(), "{drift}");
                assert_eq!(tx.total_changes(), before);
                tx.rollback().unwrap();
                assert!(current_objects_present(&db).unwrap());
            }
            assert_eq!(
                db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                0
            );
            validate_copied_usage(&db).unwrap();
            for table in ["usage_identity", "usage", "codex_usage"] {
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM pragma_foreign_key_list(?1)",
                        [table],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    2
                );
            }
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM meta WHERE key GLOB 'catalog_changes_floor_*'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0,
                "DDL cannot enroll a Source"
            );
            db.execute_batch(crate::storage::sqlite::namespace::REGISTRY_SCHEMA)
                .unwrap();
            db.execute("INSERT INTO storage_namespaces VALUES('a',1,0,1,0)", [])
                .unwrap();
            let tx = db.unchecked_transaction().unwrap();
            crate::storage::sqlite::changes::initialize_source_floors(
                &tx,
                &crate::storage::LogicalSourceNamespace::new("a").unwrap(),
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(db.query_row("SELECT count(*) FROM meta WHERE namespace='a' AND key GLOB 'catalog_changes_floor_*'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
            // Catalog triggers use the row's owner, even when
            // the executing connection is bound to Source A.
            db.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES('b','same-session',1,'codex','{}')", []).unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog_changes WHERE namespace='b'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                2
            );
            db.execute(
                "UPDATE session_catalog SET metadata='{\"changed\":true}' WHERE namespace='b'",
                [],
            )
            .unwrap();
            db.execute("DELETE FROM session_catalog WHERE namespace='b'", [])
                .unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog_changes WHERE namespace='b'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                6
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog_changes WHERE namespace='a'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
    }

    fn assembled_metadata_fixture(db: &Connection) {
        bind_fixture(db, "a");
        // This fixture models an existing current store, including its journal
        // contract. Admission must not switch a caller's journal mode for it.
        db.pragma_update(None, "journal_mode", "WAL").unwrap();
        for ddl in authored_storage_ddl() {
            db.execute_batch(ddl).unwrap();
        }
        db.execute(
            "INSERT INTO meta VALUES('','cache_version',?1)",
            [CACHE_VERSION.to_string()],
        )
        .unwrap();
        db.pragma_update(None, "user_version", SCHEMA_VERSION)
            .unwrap();
    }

    #[test]
    fn native_usage_metadata_current_admission_preserves_floors_and_fences_warm_writers() {
        use crate::storage::sqlite::changes::initialize_source_floors;
        use crate::storage::{LogicalSourceNamespace, StorageError, StoreBackend, StoreConfig};
        let directory = root();
        let database = directory.join("metadata.sqlite3");
        let namespace = LogicalSourceNamespace::new("a").unwrap();
        let mut store = current_store(&database, &namespace);
        let config = StoreConfig {
            backend: StoreBackend::Sqlite {
                database: database.clone(),
            },
            namespace: namespace.clone(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        let warm = Connection::open(&database).unwrap();
        let first = config.admit_namespace().unwrap();
        assert_eq!(config.admit_namespace().unwrap(), first);
        assert_eq!(config.clone().admit_namespace().unwrap(), first);
        assert_eq!(warm.query_row("SELECT count(*) FROM meta WHERE namespace='a' AND key GLOB 'catalog_changes_floor_*' AND value='0'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        {
            let tx = store.connection.transaction().unwrap();
            tx.execute(
                "DELETE FROM main.meta WHERE namespace='a' AND key='catalog_changes_floor_current'",
                [],
            )
            .unwrap();
            tx.execute("UPDATE main.meta SET value='corrupt' WHERE namespace='a' AND key='catalog_changes_floor_retained_history'", []).unwrap();
            // Validation must not install the first missing floor when the
            // second floor is malformed. Rollback restores both original floors.
            assert_eq!(
                initialize_source_floors(&tx, &namespace),
                Err(StorageError::NamespaceUpgradePending)
            );
            assert_eq!(tx.query_row("SELECT count(*) FROM main.meta WHERE namespace='a' AND key='catalog_changes_floor_current'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            tx.rollback().unwrap();
        }
        assert_eq!(warm.query_row("SELECT count(*) FROM meta WHERE namespace='a' AND key GLOB 'catalog_changes_floor_*' AND value='0'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        let other = StoreConfig {
            namespace: LogicalSourceNamespace::new("b").unwrap(),
            ..config.clone()
        };
        assert_eq!(
            other.admit_namespace(),
            Err(StorageError::MultiSourceUnqualified)
        );
        assert_eq!(
            warm.query_row("SELECT count(*) FROM meta WHERE namespace='b'", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        assert!(warm.execute("UPDATE meta SET value='99' WHERE namespace='a' AND key='catalog_changes_floor_current'", []).is_err());
        assert_eq!(
            warm.query_row(
                "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "a".repeat(64)
        );
        drop(warm);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_usage_metadata_source_keys_and_floors_isolate_restart_and_rollback() {
        use crate::storage::sqlite::changes::initialize_source_floors;
        use crate::storage::{LogicalSourceNamespace, StorageError};
        let directory = root();
        let database = directory.join("private-metadata.sqlite");
        let mut db = Connection::open(&database).unwrap();
        assembled_metadata_fixture(&db);
        db.execute_batch(crate::storage::sqlite::namespace::REGISTRY_SCHEMA)
            .unwrap();
        let a = LogicalSourceNamespace::new("a").unwrap();
        let b = LogicalSourceNamespace::new("b").unwrap();
        {
            let tx = db.transaction().unwrap();
            assert_eq!(
                initialize_source_floors(&tx, &a),
                Err(StorageError::NamespaceUnadmitted)
            );
            tx.execute("INSERT INTO storage_namespaces VALUES('a',1,0,1,0)", [])
                .unwrap();
            initialize_source_floors(&tx, &a).unwrap();
            tx.execute(
                native_metadata_put(&tx).unwrap(),
                params!["catalog_changes_floor_current", "17"],
            )
            .unwrap();
            initialize_source_floors(&tx, &a).unwrap();
            tx.commit().unwrap();
        }
        bind_fixture(&db, "b");
        {
            // Private lifecycle fixture: public second admission above refuses.
            let tx = db.transaction().unwrap();
            tx.execute("INSERT INTO storage_namespaces VALUES('b',2,0,1,0)", [])
                .unwrap();
            initialize_source_floors(&tx, &b).unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM meta WHERE namespace='b'", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        {
            let tx = db.transaction().unwrap();
            tx.execute("INSERT INTO storage_namespaces VALUES('b',2,0,1,0)", [])
                .unwrap();
            initialize_source_floors(&tx, &b).unwrap();
            tx.commit().unwrap();
        }
        for namespace in ["a", "b"] {
            bind_fixture(&db, namespace);
            for key in [
                "catalog_generation",
                "catalog_recovery_after",
                "slot_projection_catalog_generation",
                "search_generation_current",
                "claude_json_stamp",
                "slot_upgrade_selected",
            ] {
                db.execute(native_metadata_put(&db).unwrap(), params![key, namespace])
                    .unwrap();
            }
        }
        for namespace in ["a", "b"] {
            bind_fixture(&db, namespace);
            let tx = db.transaction().unwrap();
            let index = codex_index("gpt-6.1-sol", 3);
            let changes = crate::events::DirtyRows::between(&FileIndex::default(), &index);
            write_file(
                &tx,
                "same.jsonl",
                &ledger(),
                &changes,
                &index,
                &mut crate::native_timing::Measurements::default(),
            )
            .unwrap();
            tx.commit().unwrap();
        }
        bind_fixture(&db, "b");
        db.execute(
            native_metadata_put(&db).unwrap(),
            params!["archive_event_index:same.jsonl", "foreign-poison"],
        )
        .unwrap();
        let foreign_family = usage_family_state(&db, "b");
        bind_fixture(&db, "a");
        assert_eq!(
            current_file_count(&db, &BTreeMap::new()).unwrap(),
            1,
            "foreign archive metadata must not affect A's file count"
        );
        let b_state = || {
            let db = Connection::open(&database).unwrap();
            db.prepare("SELECT key,value FROM meta WHERE namespace='b' ORDER BY key")
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let before = b_state();
        bind_fixture(&db, "a");
        {
            let tx = db.transaction().unwrap();
            assert_eq!(
                initialize_source_floors(&tx, &b),
                Err(StorageError::NamespaceMismatch)
            );
            tx.execute(
                native_metadata_put(&tx).unwrap(),
                params!["catalog_generation", "rolled-back"],
            )
            .unwrap();
            tx.execute(
                native_metadata_delete(&tx).unwrap(),
                ["catalog_recovery_after"],
            )
            .unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(b_state(), before);
        assert_eq!(usage_family_state(&db, "b"), foreign_family);
        {
            let tx = db.transaction().unwrap();
            let id: i64 = tx
                .query_row(
                    "SELECT file_id FROM files WHERE namespace='a' AND path='same.jsonl'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            clear(&tx, id).unwrap();
            let index = codex_index("gpt-6.1-sol", 5);
            let changes = crate::events::DirtyRows::between(&FileIndex::default(), &index);
            let mut appended = ledger();
            appended.offset += 1;
            appended.stat.size += 1;
            write_file(
                &tx,
                "same.jsonl",
                &appended,
                &changes,
                &index,
                &mut crate::native_timing::Measurements::default(),
            )
            .unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(b_state(), before);
        assert_eq!(usage_family_state(&db, "b"), foreign_family);
        {
            let tx = db.transaction().unwrap();
            let id: i64 = tx
                .query_row(
                    "SELECT file_id FROM files WHERE namespace='a' AND path='same.jsonl'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            clear(&tx, id).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(b_state(), before);
        assert_eq!(usage_family_state(&db, "b"), foreign_family);
        let changes = db.total_changes();
        assert!(
            db.execute(
                native_metadata_put(&db).unwrap(),
                params!["unknown-metadata-owner", "bad"]
            )
            .is_err()
        );
        assert_eq!(db.total_changes(), changes);
        assert!(configured::bind_metadata_scope(&db, "").is_err());
        assert!(configured::bind_metadata_scope(&db, &"x".repeat(4097)).is_err());
        drop(db);
        let db = Connection::open(&database).unwrap();
        bind_fixture(&db, "a");
        let tx = db.unchecked_transaction().unwrap();
        initialize_source_floors(&tx, &a).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            db.query_row(
                native_metadata_value(&db).unwrap(),
                ["catalog_changes_floor_current"],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "17"
        );
        assert_eq!(
            db.query_row(
                "SELECT value FROM main.meta WHERE namespace='a' AND key='catalog_generation'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "a"
        );
        assert_eq!(b_state(), before);
        drop(db);
        let db = Connection::open(&database).unwrap();
        db.execute_batch("DROP TABLE meta; CREATE TABLE meta(namespace TEXT NOT NULL,key TEXT PRIMARY KEY,value TEXT NOT NULL) STRICT;").unwrap();
        assert!(
            configured::validate_statement_layout(&db).is_err(),
            "a namespace column without the composite key is corrupt, not singleton"
        );
        drop(db);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_usage_metadata_proof_reference_owners_refuse_invalid_history_before_gc() {
        let mut db = Connection::open_in_memory().unwrap();
        assembled_metadata_fixture(&db);
        let mut old = ledger();
        old.offset = crate::source_chunks::SIZE as u64;
        old.stat.size = old.offset;
        let chunks = crate::source_chunks::Chunks {
            offset: old.offset,
            hashes: vec![[7; 32]],
            resume: None,
        };
        for namespace in ["a", "b"] {
            bind_fixture(&db, namespace);
            let tx = db.transaction().unwrap();
            // Published proof custody binds the current retained usage source
            // receipt, as it does when the ordinary Index producer writes it.
            native_usage::begin_source(&tx, "same.jsonl", None, &old, false).unwrap();
            chunk_proofs::write(&tx, "same.jsonl", &old, Some(&chunks)).unwrap();
            let metadata = if namespace == "a" {
                json(&row("same.jsonl", &old, Cost::default(), "codex")).unwrap()
            } else {
                "{}".into()
            };
            tx.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(?1,'owner',0,'codex',?2)", params![namespace,metadata]).unwrap();
            tx.execute("INSERT INTO session_catalog_sources(namespace,session_key,harness,native_id,source_path) VALUES(?1,'owner','codex','native','same.jsonl')", [namespace]).unwrap();
            tx.commit().unwrap();
        }
        let foreign = || {
            db.prepare("SELECT key,value FROM meta WHERE namespace='b' ORDER BY key")
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let before = foreign();
        bind_fixture(&db, "a");
        {
            let tx = db.unchecked_transaction().unwrap();
            // B's same-key malformed owner must never be decoded. The real
            // preservation helper copies A's referenced proof before clearing.
            chunk_proofs::clear(&tx, "same.jsonl").unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(foreign(), before);
        let manifest = format!(
            "native_published_chunks_manifest:{}",
            crate::source_carrier::source_hash("same.jsonl")
        );
        let saved: String = db
            .query_row(native_metadata_value(&db).unwrap(), [&manifest], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&saved).unwrap().len(),
            1
        );
        let retained_before: i64 = db.query_row("SELECT count(*) FROM meta WHERE namespace='a' AND key GLOB 'native_published_chunks:*'", [], |r| r.get(0)).unwrap();
        assert!(retained_before > 0);
        db.execute(
            "INSERT INTO session_history_catalog(namespace,session_key,metadata) VALUES('a','owner','{}')",
            [],
        )
        .unwrap();
        let changes = db.total_changes();
        {
            let tx = db.unchecked_transaction().unwrap();
            assert!(
                chunk_proofs::clear(&tx, "same.jsonl").is_err(),
                "malformed current history cannot authorize proof collection"
            );
            tx.rollback().unwrap();
        }
        assert_eq!(
            db.total_changes(),
            changes,
            "refuse before proof copy or GC writes"
        );
        assert_eq!(foreign(), before);
        assert_eq!(db.query_row("SELECT count(*) FROM meta WHERE namespace='a' AND key GLOB 'native_published_chunks:*'", [], |r| r.get::<_, i64>(0)).unwrap(), retained_before);
    }

    #[test]
    fn native_usage_copied_family_rejects_mixed_shapes_before_payload_dml() {
        {
            let mut db = Connection::open_in_memory().unwrap();
            tree_state::register(&db).unwrap();
            db.execute_batch(configured::CORE_SCHEMA).unwrap();
            db.execute_batch(metadata_schema()).unwrap();
            for ddl in native_usage::schema() {
                db.execute_batch(ddl).unwrap();
            }
            db.execute_batch(copied_usage_schema()).unwrap();
            db.execute_batch(tree_state::schema()).unwrap();
            bind_fixture(&db, "a");
            validate_copied_usage(&db).unwrap();
            {
                let tx = db.transaction().unwrap();
                tx.execute_batch(
                    "DROP TABLE usage_identity; DROP TABLE usage; DROP TABLE codex_usage;",
                )
                .unwrap();
                tx.execute_batch(copied_usage_schema()).unwrap();
                validate_copied_usage(&tx).unwrap();
                tx.rollback().unwrap();
                validate_copied_usage(&db).unwrap();
            }
            // Invalidate an already cached decision with a partial family DDL.
            db.execute_batch("DROP TABLE codex_usage; CREATE TABLE codex_usage(file_id INTEGER NOT NULL,seq INTEGER NOT NULL,PRIMARY KEY(file_id,seq)) STRICT, WITHOUT ROWID;").unwrap();
            let tx = db.transaction().unwrap();
            let before: i64 = tx
                .query_row("SELECT total_changes()", [], |r| r.get(0))
                .unwrap();
            let index = FileIndex::default();
            let changes = crate::events::DirtyRows::between(&FileIndex::default(), &index);
            assert!(matches!(
                write_file(
                    &tx,
                    "source",
                    &ledger(),
                    &changes,
                    &index,
                    &mut crate::native_timing::Measurements::default()
                ),
                Err(rusqlite::Error::InvalidQuery)
            ));
            assert!(matches!(clear(&tx, 1), Err(rusqlite::Error::InvalidQuery)));
            assert_eq!(
                tx.query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                before
            );
            assert_eq!(
                tx.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0
            );
            tx.rollback().unwrap();
            db.execute_batch("DROP TABLE codex_usage;").unwrap();
            assert!(matches!(
                validate_copied_usage(&db),
                Err(rusqlite::Error::InvalidQuery)
            ));
        }
    }

    #[test]
    fn native_usage_copied_family_rejects_corrupt_constraints_before_payload_dml() {
        for corrupt in [
            "DROP INDEX usage_identity_peers",
            "DROP INDEX usage_identity_peers; CREATE INDEX usage_identity_peers ON usage_identity(namespace,message_id,record_uuid,file_id) WHERE file_id>0",
            "ALTER TABLE files RENAME TO foreign_files",
        ] {
            let mut db = Connection::open_in_memory().unwrap();
            bind_fixture(&db, "a");
            for ddl in authored_storage_ddl() {
                db.execute_batch(ddl).unwrap();
            }
            // Cache the admitted shape, then invalidate a real authored schema.
            validate_copied_usage(&db).unwrap();
            db.execute_batch(corrupt).unwrap();
            let tx = db.transaction().unwrap();
            let before: i64 = tx
                .query_row("SELECT total_changes()", [], |r| r.get(0))
                .unwrap();
            let index = FileIndex::default();
            let changes = crate::events::DirtyRows::between(&FileIndex::default(), &index);
            assert!(matches!(
                write_file(
                    &tx,
                    "source",
                    &ledger(),
                    &changes,
                    &index,
                    &mut crate::native_timing::Measurements::default()
                ),
                Err(rusqlite::Error::InvalidQuery)
            ));
            assert!(matches!(clear(&tx, 1), Err(rusqlite::Error::InvalidQuery)));
            assert_eq!(
                tx.query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                before
            );
            tx.rollback().unwrap();
        }
    }

    #[test]
    fn native_usage_copied_layout_is_independent_of_core_and_native_layout() {
        {
            let mut db = Connection::open_in_memory().unwrap();
            bind_fixture(&db, "a");
            for ddl in authored_storage_ddl() {
                db.execute_batch(ddl).unwrap();
            }
            let mut index = FileIndex::default();
            index.usage_by_id.insert(
                "message".into(),
                MessageUsage {
                    tokens: Tokens {
                        input: 31,
                        total: 31,
                        ..Default::default()
                    },
                    record_ids: ["record".to_owned()].into_iter().collect(),
                    ..Default::default()
                },
            );
            index.codex_usage_events.push(CodexUsageEvent {
                model: "fixture".into(),
                tokens: ModelTokens {
                    input: 41,
                    ..Default::default()
                },
                timestamp: None,
            });
            let tx = db.transaction().unwrap();
            let changes = crate::events::DirtyRows::between(&FileIndex::default(), &index);
            write_file(
                &tx,
                "source",
                &ledger(),
                &changes,
                &index,
                &mut crate::native_timing::Measurements::default(),
            )
            .unwrap();
            tx.commit().unwrap();
            let mut readers = Readers::new(&db).unwrap();
            assert_eq!(
                readers
                    .usage
                    .query([1])
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .get::<_, i64>(2)
                    .unwrap(),
                31
            );
            assert_eq!(
                readers
                    .codex_usage
                    .query([1])
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .get::<_, i64>(2)
                    .unwrap(),
                41
            );
            drop(readers);
            let tx = db.transaction().unwrap();
            clear(&tx, 1).unwrap();
            tx.commit().unwrap();
            for table in [
                "usage",
                "codex_usage",
                "usage_identity",
                "native_tree_copied_identities",
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
        }
    }

    #[test]
    fn native_usage_copied_tree_pricing_collision_restart_and_rollback() {
        // Assemble the complete authored usage/Tree/repair families, with
        // all families following the same current namespace contract.
        {
            {
                {
                    let root = root();
                    let database = root.join("families.sqlite3");
                    let mut db = Connection::open(&database).unwrap();
                    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
                        .unwrap();
                    bind_fixture(&db, "a");
                    for ddl in authored_storage_ddl() {
                        db.execute_batch(ddl).unwrap();
                    }
                    db.execute(native_metadata_put(&db).unwrap(), params![TARGET, "target"])
                        .unwrap();
                    {
                        // A source-owned poison target must not replace the store target.
                        db.execute("INSERT INTO meta VALUES('a',?1,'stale')", [TARGET])
                            .unwrap();
                    }
                    let ledger = ledger();
                    for (namespace, amount) in [("a", 11), ("b", 23)] {
                        bind_fixture(&db, namespace);
                        let tx = db.transaction().unwrap();
                        for (ordinal, path) in ["same/source.jsonl", "same/peer.jsonl"]
                            .into_iter()
                            .enumerate()
                        {
                            let mut index = if ordinal == 0 {
                                codex_index("gpt-6.1-sol", 3)
                            } else {
                                FileIndex {
                                    native_usage_observations: Some(Vec::new()),
                                    ..Default::default()
                                }
                            };
                            index.usage_by_id.insert(
                                "same-message".into(),
                                MessageUsage {
                                    model: Some(namespace.into()),
                                    tokens: Tokens {
                                        input: amount,
                                        total: amount,
                                        ..Default::default()
                                    },
                                    record_ids: ["same-record".to_owned()].into_iter().collect(),
                                    ..Default::default()
                                },
                            );
                            index.codex_usage_events.push(CodexUsageEvent {
                                model: namespace.into(),
                                tokens: ModelTokens {
                                    input: amount,
                                    ..Default::default()
                                },
                                timestamp: None,
                            });
                            index.tree_summary = Some(crate::native_tree::Summary::default());
                            assert_eq!(
                                native_usage::begin_source(&tx, path, None, &ledger, false)
                                    .unwrap(),
                                ordinal as i64 + 1
                            );
                            let changes =
                                crate::events::DirtyRows::between(&FileIndex::default(), &index);
                            write_file(
                                &tx,
                                path,
                                &ledger,
                                &changes,
                                &index,
                                &mut crate::native_timing::Measurements::default(),
                            )
                            .unwrap();
                            native_usage::write(&tx, path, &ledger, &index).unwrap();
                        }
                        tx.execute("INSERT INTO native_tree_repairs(namespace,source_path,receipt) VALUES(semon_namespace(),?1,?2)", params!["same/source.jsonl", header_ledger(&ledger)]).unwrap();
                        let (records, tokens, _) = tree_state::shared_usage(&tx, 1, 1).unwrap();
                        assert_eq!(records["same-message"].len(), 1);
                        assert_eq!(tokens["same-message"].input, amount);
                        assert_eq!(tx.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='source'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                        let key = { "same-session".to_owned() };
                        let mut owner = row(
                            "same/source.jsonl",
                            &ledger,
                            Cost {
                                usd: Some(amount as f64),
                                pricing_fingerprint: "family-price".into(),
                                ..Cost::default()
                            },
                            "codex",
                        );
                        owner.key = key.clone();
                        let encoded = json(&owner).unwrap();
                        {
                            tx.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(semon_namespace(),?1,1,'codex',?2)", params![key, encoded]).unwrap();
                        }
                        tx.execute("INSERT INTO session_trace_headers(namespace,session_key,generation,source_generation,metadata) VALUES(semon_namespace(),?1,'before',?2,'{\"family_usd\":null}')",
                            params![key, crate::retention::source_generation(&owner.sources).unwrap()]).unwrap();
                        // Equal child IDs and parent IDs must remain in their Source family.
                        let child = { "same-child".to_owned() };
                        owner.key = child.clone();
                        owner.parent = Some(key.clone());
                        let child_metadata = json(&owner).unwrap();
                        tx.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,parent_key,metadata) VALUES(semon_namespace(),?1,1,'codex',?2,?3)",
                            params![child, key, child_metadata]).unwrap();
                        tx.execute("INSERT INTO session_trace_headers(namespace,session_key,generation,source_generation,metadata) VALUES(semon_namespace(),?1,'before',?2,'{\"family_usd\":null}')",
                            params![child, crate::retention::source_generation(&owner.sources).unwrap()]).unwrap();
                        assert!(tx.query_row("SELECT EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='session' AND owner_key=?1)", [key], |r| r.get::<_, bool>(0)).unwrap());
                        tx.commit().unwrap();
                    }
                    bind_fixture(&db, "a");
                    {
                        let stamp: String = db.query_row("SELECT codex_pricing_fingerprint FROM files WHERE namespace='a' AND file_id=1", [], |r| r.get(0)).unwrap();
                        db.execute(native_metadata_put(&db).unwrap(), params![TARGET, stamp])
                            .unwrap();
                        db.execute(
                            "DELETE FROM pricing_recompute_pending WHERE owner_kind='source'",
                            [],
                        )
                        .unwrap();
                        db.execute("UPDATE files SET codex_native_usage=codex_native_usage WHERE namespace='a' AND file_id=1", []).unwrap();
                        assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE owner_kind='source'", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "a source-owned poison target must not enqueue a currently priced file");
                        db.execute(native_metadata_put(&db).unwrap(), params![TARGET, "target"])
                            .unwrap();
                    }
                    // Trigger ownership comes from the changed row, not the current
                    // connection binding. These are private DDL fixtures, without
                    // public writer guards or multi-source admission.
                    db.execute(
                        "DELETE FROM pricing_recompute_pending WHERE owner_kind='source'",
                        [],
                    )
                    .unwrap();
                    db.execute("UPDATE files SET codex_native_usage=codex_native_usage WHERE namespace='b' AND file_id=1", []).unwrap();
                    assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace='a' AND owner_kind='source'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
                    assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace='b' AND owner_kind='source'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                    db.execute("UPDATE files SET codex_native_usage=codex_native_usage WHERE namespace='a' AND file_id=1", []).unwrap();
                    {
                        db.execute(
                            "DELETE FROM pricing_recompute_pending WHERE owner_kind='session'",
                            [],
                        )
                        .unwrap();
                        db.execute(
                            "UPDATE session_catalog SET metadata=metadata WHERE namespace='b' AND session_key=?1",
                            [{ "same-session" }],
                        )
                        .unwrap();
                        assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace='a' AND owner_kind='session'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
                        assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace='b' AND owner_kind='session'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                    }
                    let b = usage_family_state(&db, "b");
                    let a = usage_family_state(&db, "a");
                    let catalog_before = catalog_family_state(&db);
                    let trace_before = trace_family_state(&db);
                    let family_key = { "same-session" };
                    let b_trace = foreign_trace_state(&db);
                    let b_changes = foreign_change_state(&db);
                    bind_fixture(&db, "a");
                    {
                        let tx = db.transaction().unwrap();
                        let target = table("openai", "gpt-6.1-sol", 7.0, 1);
                        {
                            let before: i64 = tx
                                .query_row("SELECT total_changes()", [], |r| r.get(0))
                                .unwrap();
                            let decodes = REPRICE_PAYLOAD_DECODES.with(|count| count.get());
                            // The current Catalog consumer qualifies this
                            // namespace's exact retained facts. These facts are
                            // not at the target price yet, so it preserves the
                            // owner without a write rather than refusing every
                            // Catalog operation before reading its evidence.
                            assert!(
                                !catalog_step(
                                    &tx,
                                    &crate::storage::QueryRuntimeBinding {
                                        namespace: crate::storage::LogicalSourceNamespace::new("a")
                                            .unwrap(),
                                        admission: 1,
                                        stream_epoch: "a".repeat(64),
                                    },
                                    &mut Work::default(),
                                    &target,
                                    Some(family_key),
                                )
                                .unwrap()
                            );
                            assert_eq!(
                                tx.query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
                                    .unwrap(),
                                before
                            );
                            assert!(REPRICE_PAYLOAD_DECODES.with(|count| count.get()) > decodes);
                        }
                        assert!(source_step(&tx, &mut Work::default(), &target, Some(1)).unwrap());
                        let mut appended = FileIndex::default();
                        appended.usage_by_id.insert(
                            "same-message".into(),
                            MessageUsage {
                                tokens: Tokens {
                                    input: 37,
                                    total: 37,
                                    ..Tokens::default()
                                },
                                record_ids: ["same-record".to_owned()].into_iter().collect(),
                                ..MessageUsage::default()
                            },
                        );
                        let mut appended_ledger = ledger.clone();
                        appended_ledger.offset += 1;
                        appended_ledger.stat.size += 1;
                        let delta =
                            crate::events::DirtyRows::between(&FileIndex::default(), &appended);
                        write_file(
                            &tx,
                            "same/source.jsonl",
                            &appended_ledger,
                            &delta,
                            &appended,
                            &mut crate::native_timing::Measurements::default(),
                        )
                        .unwrap();
                        assert_eq!(
                            tree_state::shared_usage(&tx, 1, 1).unwrap().1["same-message"].input,
                            37
                        );
                        family_owner(&tx, family_key, "family-price").unwrap();
                        let total: f64 = tx.query_row("SELECT json_extract(metadata,'$.family_usd') FROM session_trace_headers WHERE namespace=semon_namespace() AND session_key=?1", [family_key], |r| r.get(0)).unwrap();
                        assert_eq!(total, 22.0);
                        clear(&tx, 1).unwrap();
                        assert_eq!(tree_state::shared_usage(&tx, 2, 2).unwrap().0.len(), 0);
                        assert_eq!(usage_family_state(&tx, "b"), b);
                        assert_eq!(foreign_change_state(&tx), b_changes);
                        tx.rollback().unwrap();
                    }
                    assert_eq!(usage_family_state(&db, "a"), a);
                    assert_eq!(usage_family_state(&db, "b"), b);
                    assert_eq!(catalog_family_state(&db), catalog_before);
                    assert_eq!(trace_family_state(&db), trace_before);
                    assert_eq!(foreign_change_state(&db), b_changes);
                    drop(db);
                    let mut db = Connection::open(&database).unwrap();
                    db.pragma_update(None, "foreign_keys", true).unwrap();
                    bind_fixture(&db, "a");
                    assert!(native_usage::repair_pending(&db).unwrap());
                    let tx = db.transaction().unwrap();
                    let target = table("openai", "gpt-6.1-sol", 7.0, 1);
                    let mut work = Work::default();
                    let binding = crate::storage::QueryRuntimeBinding {
                        namespace: crate::storage::LogicalSourceNamespace::new("a").unwrap(),
                        admission: 1,
                        stream_epoch: "a".repeat(64),
                    };
                    // Keep only the source owner to exercise actual pending selection
                    // and deletion without invoking the unqualified Catalog producer.
                    tx.execute("DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='session'", []).unwrap();
                    assert!(pending_step(&tx, &binding, &mut work, &target).unwrap());
                    assert!(!pending_step(&tx, &binding, &mut work, &target).unwrap());
                    let child_key = { "same-child" };
                    tx.execute("INSERT INTO pricing_recompute_pending(namespace,owner_kind,owner_key) VALUES(semon_namespace(),'family',?1)", [child_key]).unwrap();
                    let mut family_target = (*target).clone();
                    family_target.fingerprint = "family-price".into();
                    let family_target = Arc::new(family_target);
                    assert!(pending_step(&tx, &binding, &mut work, &family_target).unwrap());
                    assert!(pending_step(&tx, &binding, &mut work, &family_target).unwrap());
                    assert!(!pending_step(&tx, &binding, &mut work, &family_target).unwrap());
                    work = Work {
                        target_fingerprint: "family-price".into(),
                        ..Work::default()
                    };
                    family_step(&tx, &mut work).unwrap();
                    assert_eq!(foreign_trace_state(&tx), b_trace);
                    assert_eq!(foreign_change_state(&tx), b_changes);
                    assert_eq!(catalog_family_state(&tx), catalog_before);
                    clear(&tx, 1).unwrap();
                    assert!(tree_state::shared_usage(&tx, 2, 2).unwrap().0.is_empty());
                    assert_eq!(usage_family_state(&tx, "b"), b);
                    tx.commit().unwrap();
                    assert_eq!(usage_family_state(&db, "b"), b);
                    let target: String = db.query_row("SELECT value FROM meta WHERE namespace='' AND key='pricing_recompute_target'", [], |r| r.get(0)).unwrap();
                    assert_eq!(target, "target");
                    bind_fixture(&db, "b");
                    assert_eq!(
                        tree_state::shared_usage(&db, 1, 1).unwrap().1["same-message"].input,
                        23
                    );
                    let mut readers = Readers::new(&db).unwrap();
                    assert!(readers.usage.query([1]).unwrap().next().unwrap().is_some());
                    assert!(
                        readers
                            .codex_usage
                            .query([1])
                            .unwrap()
                            .next()
                            .unwrap()
                            .is_some()
                    );
                    drop(readers);
                    drop(db);
                    fs::remove_dir_all(root).unwrap();
                }
            }
        }
    }

    #[test]
    fn native_usage_composite_repricing_isolates_colliding_source_ids_and_rollback() {
        use rusqlite::functions::FunctionFlags;
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE files (
            namespace TEXT NOT NULL,file_id INTEGER NOT NULL,path TEXT NOT NULL,
            codex_native_usage TEXT,codex_pricing_fingerprint TEXT,
            PRIMARY KEY(namespace,file_id),UNIQUE(namespace,path));
            CREATE INDEX files_codex_pricing_fingerprint ON files(namespace,file_id,codex_pricing_fingerprint) WHERE codex_native_usage IS NOT NULL;").unwrap();
        for ddl in native_usage::schema() {
            db.execute_batch(ddl).unwrap();
        }
        let bind = |db: &Connection, namespace: &str| {
            let namespace = namespace.to_owned();
            db.create_scalar_function(
                "semon_namespace",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| Ok(namespace.clone()),
            )
            .unwrap();
        };
        let ledger = ledger();
        let path = "same/relative.jsonl";
        for (namespace, count) in [("a", 3), ("b", 5)] {
            bind(&db, namespace);
            let tx = db.transaction().unwrap();
            let index = codex_index("gpt-6.1-sol", count);
            assert_eq!(
                native_usage::begin_source(&tx, path, None, &ledger, false).unwrap(),
                1
            );
            native_usage::write(&tx, path, &ledger, &index).unwrap();
            let mut root: Value =
                serde_json::from_str(&native_usage::encoded(&index).unwrap().unwrap()).unwrap();
            root["cost"]["pricing_fingerprint"] = Value::String("stale".into());
            tx.execute(
                "INSERT INTO files VALUES(semon_namespace(),1,?1,?2,'stale')",
                params![path, json(&root).unwrap()],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let state = |db: &Connection, namespace: &str| -> (String, String, String) {
            (
                db.query_row("SELECT codex_native_usage FROM files WHERE namespace=?1",[namespace],|r|r.get(0)).unwrap(),
                db.query_row("SELECT json_group_array(json_array(kind,dimension,value)) FROM (SELECT kind,dimension,value FROM native_usage_dimensions WHERE namespace=?1 ORDER BY kind,dimension)",[namespace],|r|r.get(0)).unwrap(),
                db.query_row("SELECT json_group_array(json_array(native_offset,payload,record,outcome,model_outcome)) FROM (SELECT * FROM native_usage_facts WHERE namespace=?1 ORDER BY native_offset)",[namespace],|r|r.get(0)).unwrap()
            )
        };
        let b = state(&db, "b");
        let a_facts = state(&db, "a").2;
        let target = table("openai", "gpt-6.1-sol", 7.0, 1);
        bind(&db, "a");
        let tx = db.transaction().unwrap();
        assert!(source_step(&tx, &mut Work::default(), &target, Some(1)).unwrap());
        assert_eq!(state(&tx, "b"), b);
        assert_eq!(state(&tx, "a").2, a_facts);
        let root: Value = serde_json::from_str(&state(&tx, "a").0).unwrap();
        assert_eq!(root["cost"]["pricing_fingerprint"], target.fingerprint);
        tx.commit().unwrap();
        let a = state(&db, "a");
        let tx = db.transaction().unwrap();
        let other = table("openai", "gpt-6.1-sol", 9.0, 2);
        assert!(source_step(&tx, &mut Work::default(), &other, Some(1)).unwrap());
        tx.rollback().unwrap();
        assert_eq!(state(&db, "a"), a);
        assert_eq!(state(&db, "b"), b);
    }

    #[test]
    fn completed_fingerprint_stays_idle_without_writes_or_payload_decodes() {
        let root = root();
        let path = root.join("index.sqlite3");
        let _store = current_store(&path, &crate::events::store::fixture_namespace());
        let table = table("anthropic", "claude-opus-5-5", 8.0, 1);
        drain(&path, &table);
        // Current meta is WITHOUT ROWID. Keep one observer connection so its
        // data_version also detects any writes committed by the step connections.
        let observer = Connection::open(&path).unwrap();
        let before: (i64, String) = observer
            .query_row(
                "SELECT (SELECT data_version FROM pragma_data_version),value FROM meta WHERE namespace='fixture-source' AND key=?1",
                [WORK],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let decoded = REPRICE_PAYLOAD_DECODES.with(std::cell::Cell::get);
        crate::pricing::runtime::register_index(owner(&path));
        crate::pricing::runtime::register_index(owner(&path));
        for _ in 0..8 {
            assert!(!step_inner(&owner(&path), Arc::clone(&table), false).unwrap());
        }
        let after: (i64, String) = observer
            .query_row(
                "SELECT (SELECT data_version FROM pragma_data_version),value FROM meta WHERE namespace='fixture-source' AND key=?1",
                [WORK],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(after, before);
        assert_eq!(REPRICE_PAYLOAD_DECODES.with(std::cell::Cell::get), decoded);
        assert_eq!(
            Connection::open(&path)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM pricing_recompute_pending",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
        drop(_store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_source_append_restarts_partial_batch_and_prices_all_retained_facts() {
        let root = root();
        let path = root.join("index.sqlite3");
        let mut store = current_store(&path, &crate::events::store::fixture_namespace());
        let p = "/not-present/pending-append.jsonl";
        let unrelated = "/not-present/already-current.jsonl";
        let p_ledger = ledger();
        let unrelated_ledger = ledger();
        store
            .write_one(p, None, None, &p_ledger, &codex_index("gpt-6.1-sol", 129))
            .unwrap();
        store
            .write_one(
                unrelated,
                None,
                None,
                &unrelated_ledger,
                &codex_index("gpt-6.1-sol", 1),
            )
            .unwrap();
        let current = table("openai", "gpt-6.1-sol", 7.0, 1);
        let old = table("openai", "gpt-6.1-sol", 4.0, 0);
        drain(&path, &current);
        let p_id: i64 = store
            .connection
            .query_row(
                "SELECT source_id FROM native_usage_current WHERE source_path=?1",
                [p],
                |row| row.get(0),
            )
            .unwrap();
        let q_id: i64 = store
            .connection
            .query_row(
                "SELECT source_id FROM native_usage_current WHERE source_path=?1",
                [unrelated],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_usage: String = store
            .connection
            .query_row(
                "SELECT codex_native_usage FROM files WHERE path=?1",
                [unrelated],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_stamp: String = store
            .connection
            .query_row(
                "SELECT codex_pricing_fingerprint FROM files WHERE path=?1",
                [unrelated],
                |row| row.get(0),
            )
            .unwrap();
        mark_file_stale(&store.connection, p, &old.fingerprint, None);
        assert_eq!(pending_source_count(&store.connection, p), 1);
        REPRICE_PAYLOAD_DECODES.with(|count| count.set(0));

        assert!(step_inner(&owner(&path), Arc::clone(&current), false).unwrap());
        assert_eq!(pending_source_count(&store.connection, p), 1);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT json_extract(value,'$.job.offset') FROM meta WHERE key=?1",
                    [WORK],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            127
        );

        let mut appended_ledger = p_ledger.clone();
        appended_ledger.stat.size += 100;
        appended_ledger.offset += 100;
        appended_ledger.prefix = [3; 32];
        appended_ledger.tail = [4; 32];
        let original_receipt: String = store
            .connection
            .query_row(
                "SELECT receipt FROM native_usage_sources WHERE source_id=?1",
                [p_id],
                |row| row.get(0),
            )
            .unwrap();
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            native_usage::begin_source(&tx, p, Some(&p_ledger), &appended_ledger, true).unwrap(),
            p_id
        );
        let appended_payload = serde_json::json!({
            "model":"gpt-6.1-sol",
            "timestamp":0,
            "tokens":{"input":100,"cached_input":0,"output":0,"reasoning_output":0,"total":100},
            "model_tokens":{"input":100,"output":0,"cache_write":0,"cache_read":0}
        });
        let appended_record =
            serde_json::json!({"type":"token_usage_record","payload":{"response_id":"append-129"}});
        tx.execute(
            "INSERT INTO native_usage_identities VALUES(semon_namespace(),?1,'tree','append-129',129)",
            [p_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_identities VALUES(semon_namespace(),?1,'model','append-129',129)",
            [p_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_facts VALUES(semon_namespace(),?1,129,'append-129',?2,?3,'accepted','accepted')",
            params![p_id, appended_payload.to_string(), appended_record.to_string()],
        )
        .unwrap();
        tx.execute(
            "UPDATE native_usage_dimensions SET value=json_set(value,'$.input',json_extract(value,'$.input')+100) WHERE source_id=?1 AND kind='model' AND dimension='gpt-6.1-sol'",
            [p_id],
        )
        .unwrap();
        mark_file_stale(&tx, p, &old.fingerprint, Some(13_000));
        tx.commit().unwrap();
        let writer_facts = source_facts(&store.connection, p_id);
        let writer_receipt: String = store
            .connection
            .query_row(
                "SELECT receipt FROM native_usage_sources WHERE source_id=?1",
                [p_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(writer_facts.len(), 130);
        assert_ne!(writer_receipt, original_receipt);
        assert_eq!(pending_source_count(&store.connection, p), 1);

        let mut completed = false;
        for _ in 0..30 {
            let progressed = step_inner(&owner(&path), Arc::clone(&current), false).unwrap();
            if pending_source_count(&store.connection, p) == 0 {
                let stamp: String = store
                    .connection
                    .query_row(
                        "SELECT codex_pricing_fingerprint FROM files WHERE path=?1",
                        [p],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(stamp, current.fingerprint);
                completed = true;
                break;
            }
            assert!(progressed);
        }
        assert!(
            completed,
            "pending append was not completed within 30 steps"
        );
        let amount: f64 = store
            .connection
            .query_row(
                "SELECT json_extract(value,'$.usd') FROM native_usage_dimensions WHERE source_id=?1 AND kind='cost_model' AND dimension='gpt-6.1-sol'",
                [p_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!((amount - 0.091).abs() < 1e-12);
        let day_amount: f64 = store
            .connection
            .query_row(
                "SELECT CAST(value AS REAL) FROM native_usage_dimensions WHERE source_id=?1 AND kind='cost_day'",
                [p_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!((day_amount - 0.091).abs() < 1e-12);
        assert_eq!(source_facts(&store.connection, p_id), writer_facts);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT receipt FROM native_usage_sources WHERE source_id=?1",
                    [p_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            writer_receipt
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT codex_native_usage FROM files WHERE path=?1",
                    [unrelated],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            unrelated_usage
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT codex_pricing_fingerprint FROM files WHERE path=?1",
                    [unrelated],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            unrelated_stamp
        );
        assert_eq!(
            REPRICE_PAYLOAD_DECODES.with(std::cell::Cell::get),
            266,
            "pending repricing must not decode the unrelated current owner"
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM native_usage_facts WHERE source_id=?1",
                    [q_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_source_current_id_replacement_discards_old_partial_job() {
        let root = root();
        let path = root.join("index.sqlite3");
        let mut store = current_store(&path, &crate::events::store::fixture_namespace());
        let native = "/not-present/pending-replacement.jsonl";
        let original_ledger = ledger();
        store
            .write_one(
                native,
                None,
                None,
                &original_ledger,
                &codex_index("gpt-6.1-sol", 129),
            )
            .unwrap();
        let current = table("openai", "gpt-6.1-sol", 7.0, 1);
        let old = table("openai", "gpt-6.1-sol", 4.0, 0);
        drain(&path, &current);
        let old_id: i64 = store
            .connection
            .query_row(
                "SELECT source_id FROM native_usage_current WHERE source_path=?1",
                [native],
                |row| row.get(0),
            )
            .unwrap();
        let old_facts = source_facts(&store.connection, old_id);
        mark_file_stale(&store.connection, native, &old.fingerprint, None);
        assert_eq!(pending_source_count(&store.connection, native), 1);
        assert!(step_inner(&owner(&path), Arc::clone(&current), false).unwrap());
        assert_eq!(pending_source_count(&store.connection, native), 1);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT json_extract(value,'$.job.offset') FROM meta WHERE key=?1",
                    [WORK],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            127
        );

        let mut replacement_ledger = original_ledger.clone();
        replacement_ledger.stat.size += 200;
        replacement_ledger.offset += 200;
        replacement_ledger.prefix = [5; 32];
        replacement_ledger.tail = [6; 32];
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let replacement_id =
            native_usage::begin_source(&tx, native, None, &replacement_ledger, false).unwrap();
        assert_ne!(replacement_id, old_id);
        let replacement_payload = serde_json::json!({
            "model":"gpt-6.1-sol",
            "timestamp":0,
            "tokens":{"input":300,"cached_input":0,"output":0,"reasoning_output":0,"total":300},
            "model_tokens":{"input":300,"output":0,"cache_write":0,"cache_read":0}
        });
        let replacement_record = serde_json::json!({"type":"token_usage_record","payload":{"response_id":"replacement-0"}});
        tx.execute(
            "INSERT INTO native_usage_identities VALUES(semon_namespace(),?1,'tree','replacement-0',0)",
            [replacement_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_identities VALUES(semon_namespace(),?1,'model','replacement-0',0)",
            [replacement_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_facts VALUES(semon_namespace(),?1,0,'replacement-0',?2,?3,'accepted','accepted')",
            params![replacement_id, replacement_payload.to_string(), replacement_record.to_string()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_dimensions VALUES(semon_namespace(),?1,'model','gpt-6.1-sol',?2)",
            params![
                replacement_id,
                json(&serde_json::json!({"input":300,"output":0,"cache_write":0,"cache_read":0}))
                    .unwrap()
            ],
        )
        .unwrap();
        mark_file_stale(&tx, native, &old.fingerprint, Some(300));
        tx.execute(
            "UPDATE native_usage_sources SET usage_ready=1,checkpoint=?2 WHERE source_id=?1",
            params![replacement_id, replacement_ledger.offset as i64],
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(pending_source_count(&store.connection, native), 1);
        let replacement_facts = source_facts(&store.connection, replacement_id);

        assert!(step_inner(&owner(&path), Arc::clone(&current), false).unwrap());
        assert_eq!(pending_source_count(&store.connection, native), 1);
        assert!(
            store
                .connection
                .query_row(
                    "SELECT json_extract(value,'$.job') IS NULL FROM meta WHERE key=?1",
                    [WORK],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap(),
            "a replacement ID must discard the old partial accumulator"
        );
        let stale_stamp: String = store
            .connection
            .query_row(
                "SELECT codex_pricing_fingerprint FROM files WHERE path=?1",
                [native],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_stamp, old.fingerprint);

        let mut completed = false;
        for _ in 0..30 {
            let progressed = step_inner(&owner(&path), Arc::clone(&current), false).unwrap();
            if pending_source_count(&store.connection, native) == 0 {
                let stamp: String = store
                    .connection
                    .query_row(
                        "SELECT codex_pricing_fingerprint FROM files WHERE path=?1",
                        [native],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(stamp, current.fingerprint);
                completed = true;
                break;
            }
            assert!(progressed);
        }
        assert!(
            completed,
            "replacement source was not completed within 30 steps"
        );
        let amount: f64 = store
            .connection
            .query_row(
                "SELECT json_extract(value,'$.usd') FROM native_usage_dimensions WHERE source_id=?1 AND kind='cost_model' AND dimension='gpt-6.1-sol'",
                [replacement_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!((amount - 0.0021).abs() < 1e-12);
        assert_eq!(source_facts(&store.connection, old_id), old_facts);
        assert_eq!(
            source_facts(&store.connection, replacement_id),
            replacement_facts
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT source_id FROM native_usage_current WHERE source_path=?1",
                    [native],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            replacement_id
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_stale_retained_owner_is_enqueued_and_repriced_without_rescanning_others() {
        let root = root();
        let path = root.join("index.sqlite3");
        let mut store = current_store(&path, &crate::events::store::fixture_namespace());
        let ledger = ledger();
        let native = "/not-present/new-retained-claude.jsonl";
        let usage = MessageUsage {
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
        index.usage_by_id.insert("message".into(), usage.clone());
        store
            .write_one(native, None, None, &ledger, &index)
            .unwrap();

        let old = table("anthropic", "claude-opus-5-5", 4.0, 0);
        let current = table("anthropic", "claude-opus-5-5", 8.0, 1);
        let mut old_cost = CostBuild::with_table(Arc::clone(&old));
        old_cost.message(&usage);
        let stale_row = row(native, &ledger, old_cost.finish(), "claude");
        drain(&path, &current);
        let current_work: String = store
            .connection
            .query_row("SELECT value FROM meta WHERE key=?1", [WORK], |row| {
                row.get(0)
            })
            .unwrap();

        let mut unrelated_cost = CostBuild::with_table(Arc::clone(&current));
        unrelated_cost.message(&usage);
        let mut unrelated = row(
            "/not-present/unrelated.jsonl",
            &ledger,
            unrelated_cost.finish(),
            "claude",
        );
        unrelated.key = "unrelated".into();
        publish(&store, &unrelated);
        publish(&store, &stale_row);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM pricing_recompute_pending",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            1
        );
        drop(store);

        drain(&path, &current);
        let connection = Connection::open(&path).unwrap();
        let updated: f64 = connection
            .query_row(
                "SELECT json_extract(metadata,'$.cost.usd') FROM session_catalog WHERE session_key='owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let untouched: f64 = connection
            .query_row(
                "SELECT json_extract(metadata,'$.cost.usd') FROM session_catalog WHERE session_key='unrelated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated, 8.0);
        assert_eq!(untouched, 8.0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM pricing_recompute_pending",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
        let after_work: String = connection
            .query_row("SELECT value FROM meta WHERE key=?1", [WORK], |row| {
                row.get(0)
            })
            .unwrap();
        let before_work: Work = from_json(0, &current_work).unwrap();
        let after_work: Work = from_json(0, &after_work).unwrap();
        assert_eq!(after_work.session_after, before_work.session_after);
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_saved_session_overview_and_summary_reprice_without_native_files_across_restart() {
        let root = root();
        let path = root.join("index.sqlite3");
        let mut store = current_store(&path, &crate::events::store::fixture_namespace());
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
        assert_eq!(parser_version(&c).unwrap(), Some(CACHE_VERSION));
        assert_eq!(CACHE_VERSION, 29);
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
        let mut store = current_store(&path, &crate::events::store::fixture_namespace());
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
        assert!(step_inner(&owner(&path), Arc::clone(&table), false).unwrap());
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
        assert_eq!(parser_version(&c).unwrap(), Some(CACHE_VERSION));
        drop(c);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_changed_retained_facts_keep_the_previous_price_fingerprint() {
        let root = root();
        let path = root.join("index.sqlite3");
        let store = current_store(&path, &crate::events::store::fixture_namespace());
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
