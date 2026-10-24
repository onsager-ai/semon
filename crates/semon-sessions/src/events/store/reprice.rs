//! Resumable cost-only producer. It never opens native bodies, changes source
//! membership, deletes observations, or advances parser/publication witnesses.
use super::*;
use crate::pricing::{Cost, CostBuild, EffectiveTable};
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

pub(super) const SCHEMA: &str = "
CREATE INDEX IF NOT EXISTS session_catalog_pricing_fingerprint
    ON session_catalog(session_key,pricing_fingerprint);
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_insert
AFTER INSERT ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
    SELECT 'session',NEW.session_key FROM session_catalog c,meta m WHERE c.session_key=NEW.session_key AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_update
AFTER UPDATE OF metadata ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
    SELECT 'session',NEW.session_key FROM session_catalog c,meta m WHERE c.session_key=NEW.session_key AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
";

pub(super) const FILE_SCHEMA: &str = "
CREATE INDEX IF NOT EXISTS files_codex_pricing_fingerprint
    ON files(file_id,codex_pricing_fingerprint)
    WHERE codex_native_usage IS NOT NULL;
CREATE TRIGGER IF NOT EXISTS pricing_pending_file_insert
AFTER INSERT ON files
WHEN NEW.codex_native_usage IS NOT NULL
BEGIN
    UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END WHERE file_id=NEW.file_id;
    INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
    SELECT 'source',NEW.path FROM files f,meta m WHERE f.file_id=NEW.file_id AND m.key='pricing_recompute_target'
      AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_file_update
AFTER UPDATE OF codex_native_usage ON files
BEGIN
    UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END WHERE file_id=NEW.file_id;
    INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
    SELECT 'source',NEW.path FROM files f,meta m WHERE NEW.codex_native_usage IS NOT NULL AND f.file_id=NEW.file_id AND m.key='pricing_recompute_target'
      AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_native_usage_ready
AFTER UPDATE OF usage_ready ON native_usage_sources
WHEN NEW.usage_ready=1
BEGIN
    INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
    SELECT 'source',NEW.source_path FROM files f,meta m WHERE f.path=NEW.source_path AND m.key='pricing_recompute_target'
      AND f.codex_native_usage IS NOT NULL AND f.codex_pricing_fingerprint IS NOT m.value;
END;
";

const QUEUE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pricing_recompute_pending (
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('source','session','family')),
    owner_key TEXT NOT NULL,
    PRIMARY KEY(owner_kind,owner_key)
) STRICT, WITHOUT ROWID;
";
const COMPOSITE_QUEUE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pricing_recompute_pending (
    namespace TEXT NOT NULL,
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('source','session','family')),
    owner_key TEXT NOT NULL,
    PRIMARY KEY(namespace,owner_kind,owner_key)
) STRICT, WITHOUT ROWID;
";

pub(super) fn queue_schema(composite: bool) -> &'static str {
    if composite {
        COMPOSITE_QUEUE_SCHEMA
    } else {
        QUEUE_SCHEMA
    }
}

fn queue_sql<'a>(c: &Connection, legacy: &'a str, scoped: &'a str) -> rusqlite::Result<&'a str> {
    let composite: bool = c.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_table_info('pricing_recompute_pending') WHERE name='namespace' AND pk=1)")?.query_row([], |r| r.get(0))?;
    Ok(if composite { scoped } else { legacy })
}

const CATALOG_QUEUE_00: &str = "
CREATE INDEX IF NOT EXISTS session_catalog_pricing_fingerprint
    ON session_catalog(session_key,pricing_fingerprint);
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_insert
AFTER INSERT ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT semon_namespace(),'session',NEW.session_key FROM session_catalog c,meta m WHERE c.session_key=NEW.session_key AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_update
AFTER UPDATE OF metadata ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT semon_namespace(),'session',NEW.session_key FROM session_catalog c,meta m WHERE c.session_key=NEW.session_key AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
";
const CATALOG_QUEUE_01: &str = "
CREATE INDEX IF NOT EXISTS session_catalog_pricing_fingerprint
    ON session_catalog(session_key,pricing_fingerprint);
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_insert
AFTER INSERT ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT semon_namespace(),'session',NEW.session_key FROM session_catalog c,meta m WHERE c.session_key=NEW.session_key AND m.namespace='' AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_update
AFTER UPDATE OF metadata ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT semon_namespace(),'session',NEW.session_key FROM session_catalog c,meta m WHERE c.session_key=NEW.session_key AND m.namespace='' AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
";
const CATALOG_QUEUE_10: &str = "
CREATE INDEX IF NOT EXISTS session_catalog_pricing_fingerprint
    ON session_catalog(namespace,session_key,pricing_fingerprint);
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_insert
AFTER INSERT ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'session',NEW.session_key FROM session_catalog c,meta m WHERE c.namespace=NEW.namespace AND c.session_key=NEW.session_key AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_catalog_update
AFTER UPDATE OF metadata ON session_catalog
BEGIN
    UPDATE session_catalog SET pricing_fingerprint=CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND session_key=NEW.session_key;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'session',NEW.session_key FROM session_catalog c,meta m WHERE c.namespace=NEW.namespace AND c.session_key=NEW.session_key AND m.key='pricing_recompute_target'
      AND c.pricing_fingerprint IS NOT m.value;
END;
";
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

pub(super) fn catalog_schema(
    queue_composite: bool,
    catalog_composite: bool,
    meta_composite: bool,
) -> &'static str {
    if !queue_composite {
        return SCHEMA;
    }
    match (catalog_composite, meta_composite) {
        (false, false) => CATALOG_QUEUE_00,
        (false, true) => CATALOG_QUEUE_01,
        (true, false) => CATALOG_QUEUE_10,
        (true, true) => CATALOG_QUEUE_11,
    }
}

const COMPOSITE_FILE_QUEUE_0: &str = "
CREATE INDEX IF NOT EXISTS files_codex_pricing_fingerprint
    ON files(namespace,file_id,codex_pricing_fingerprint)
    WHERE codex_native_usage IS NOT NULL;
CREATE TRIGGER IF NOT EXISTS pricing_pending_file_insert
AFTER INSERT ON files
WHEN NEW.codex_native_usage IS NOT NULL
BEGIN
    UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'source',NEW.path FROM files f,meta m WHERE f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.key='pricing_recompute_target'
      AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_file_update
AFTER UPDATE OF codex_native_usage ON files
BEGIN
    UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'source',NEW.path FROM files f,meta m WHERE NEW.codex_native_usage IS NOT NULL AND f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.key='pricing_recompute_target'
      AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER IF NOT EXISTS pricing_pending_native_usage_ready
AFTER UPDATE OF usage_ready ON native_usage_sources
WHEN NEW.usage_ready=1
BEGIN
    INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key)
    SELECT NEW.namespace,'source',NEW.source_path FROM files f,meta m WHERE f.namespace=NEW.namespace AND f.path=NEW.source_path AND m.key='pricing_recompute_target'
      AND f.codex_native_usage IS NOT NULL AND f.codex_pricing_fingerprint IS NOT m.value;
END;
";
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

pub(super) fn file_schema(composite: bool, meta_composite: bool) -> &'static str {
    if !composite {
        FILE_SCHEMA
    } else if meta_composite {
        COMPOSITE_FILE_QUEUE_1
    } else {
        COMPOSITE_FILE_QUEUE_0
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Work {
    #[serde(alias = "fingerprint")]
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
pub(crate) fn step(path: &Path, table: Arc<EffectiveTable>) -> rusqlite::Result<bool> {
    step_inner(path, table, true)
}

fn idle(path: &Path, fingerprint: &str) -> rusqlite::Result<bool> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // This Options producer has no configured namespace authority. Keep the
    // admission fence ahead of even its optional metadata-only fast path.
    if crate::storage::sqlite::namespace::is_bound(&connection)? {
        return Ok(false);
    }
    let (target, completed, pending): (Option<String>, Option<String>, bool) = connection
        .query_row(
            progress_sql(&connection)?,
            params![TARGET, COMPLETED],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    Ok(target.as_deref() == Some(fingerprint)
        && completed.as_deref() == Some(fingerprint)
        && !pending)
}

fn progress_sql(c: &Connection) -> rusqlite::Result<&'static str> {
    queue_sql(
        c,
        configured::metadata_sql(
            c,
            "SELECT (SELECT value FROM meta WHERE key=?1),(SELECT value FROM meta WHERE key=?2),EXISTS(SELECT 1 FROM pricing_recompute_pending)",
            "SELECT (SELECT value FROM meta WHERE namespace='' AND key=?1),(SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?2),EXISTS(SELECT 1 FROM pricing_recompute_pending)",
        )?,
        configured::metadata_sql(
            c,
            "SELECT (SELECT value FROM meta WHERE key=?1),(SELECT value FROM meta WHERE key=?2),EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace())",
            "SELECT (SELECT value FROM meta WHERE namespace='' AND key=?1),(SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?2),EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace())",
        )?,
    )
}

fn step_inner(
    path: &Path,
    table: Arc<EffectiveTable>,
    check_current: bool,
) -> rusqlite::Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    // The completed check is a single read-only metadata query. It never opens
    // an IMMEDIATE transaction or decodes the work cursor or owner payloads.
    if idle(path, &table.fingerprint)? {
        return Ok(false);
    }
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(50))?;
    if !current(&connection)? {
        return Ok(false);
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (current_target, current_completed, has_pending): (Option<String>, Option<String>, bool) =
        tx.query_row(progress_sql(&tx)?, params![TARGET, COMPLETED], |row| {
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
    if work.target_fingerprint.is_empty() && work.phase == 3 {
        // Migrate the old completed Work encoding without repeating its pass.
        work.target_fingerprint = current_target
            .clone()
            .unwrap_or_else(|| table.fingerprint.clone());
    }
    if work.phase == 3 && work.completed_fingerprint.is_empty() {
        work.completed_fingerprint = work.target_fingerprint.clone();
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
    tx.execute_batch("SAVEPOINT pricing_owner")?;
    let full_pass = work.completed_fingerprint != table.fingerprint;
    let migrated_completion = !full_pass
        && work.phase == 3
        && current_completed.as_deref() != Some(table.fingerprint.as_str());
    let result = if full_pass {
        match work.phase {
            0 => source_step(&tx, &mut work, &table, None).map(|_| true),
            1 => catalog_step(&tx, &mut work, &table, None).map(|_| true),
            _ => family_step(&tx, &mut work).map(|_| true),
        }
    } else {
        pending_step(&tx, &mut work, &table)
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
                    queue_sql(&tx, "DELETE FROM pricing_recompute_pending WHERE owner_kind=?1 AND owner_key=?2", "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind=?1 AND owner_key=?2")?,
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
                queue_sql(&tx, "DELETE FROM pricing_recompute_pending WHERE owner_kind=?1 AND owner_key=?2", "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind=?1 AND owner_key=?2")?,
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
            tx.query_row(native_usage::sql(tx, "SELECT s.source_id,s.receipt,f.file_id,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM native_usage_sources s JOIN native_usage_current n USING(source_id) JOIN files f ON f.path=n.source_path WHERE s.source_id=?1 AND s.parser_version=?2 AND s.usage_ready=1 AND f.codex_pricing_fingerprint IS NOT ?3", "SELECT s.source_id,s.receipt,f.file_id,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM native_usage_sources s JOIN native_usage_current n USING(namespace,source_id) JOIN files f ON f.namespace=n.namespace AND f.path=n.source_path WHERE s.namespace=semon_namespace() AND s.source_id=?1 AND s.parser_version=?2 AND s.usage_ready=1 AND f.codex_pricing_fingerprint IS NOT ?3")?,params![id,CACHE_VERSION,table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
        } else {
            tx.query_row(native_usage::sql(tx, "SELECT s.source_id,s.receipt,f.file_id,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM files f INDEXED BY files_codex_pricing_fingerprint JOIN native_usage_current n ON n.source_path=f.path JOIN native_usage_sources s USING(source_id) WHERE f.codex_native_usage IS NOT NULL AND f.file_id>?1 AND s.parser_version=?2 AND s.usage_ready=1 AND f.codex_pricing_fingerprint IS NOT ?3 ORDER BY f.file_id LIMIT 1", "SELECT s.source_id,s.receipt,f.file_id,CASE WHEN octet_length(f.codex_native_usage)<=1048576 THEN f.codex_native_usage ELSE NULL END FROM files f INDEXED BY files_codex_pricing_fingerprint JOIN native_usage_current n ON n.namespace=f.namespace AND n.source_path=f.path JOIN native_usage_sources s ON s.namespace=n.namespace AND s.source_id=n.source_id WHERE f.namespace=semon_namespace() AND f.codex_native_usage IS NOT NULL AND f.file_id>?1 AND s.parser_version=?2 AND s.usage_ready=1 AND f.codex_pricing_fingerprint IS NOT ?3 ORDER BY f.file_id LIMIT 1")?,params![work.file_after,CACHE_VERSION,table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
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
            native_usage::sql(tx, "UPDATE files SET codex_pricing_fingerprint=?2 WHERE path=(SELECT source_path FROM native_usage_current WHERE source_id=?1)", "UPDATE files SET codex_pricing_fingerprint=?2 WHERE namespace=semon_namespace() AND path=(SELECT source_path FROM native_usage_current WHERE namespace=semon_namespace() AND source_id=?1)")?,
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
    let current: Option<(String,String)>=tx.query_row(native_usage::sql(tx, "SELECT s.receipt,f.codex_native_usage FROM native_usage_sources s JOIN native_usage_current n USING(source_id) JOIN files f ON f.path=n.source_path WHERE s.source_id=?1 AND octet_length(f.codex_native_usage)<=1048576", "SELECT s.receipt,f.codex_native_usage FROM native_usage_sources s JOIN native_usage_current n USING(namespace,source_id) JOIN files f ON f.namespace=n.namespace AND f.path=n.source_path WHERE s.namespace=semon_namespace() AND s.source_id=?1 AND octet_length(f.codex_native_usage)<=1048576")?,[job.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
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
    let mut statement=tx.prepare(native_usage::sql(tx, "SELECT native_offset,CASE WHEN octet_length(payload)<=8192 THEN payload ELSE NULL END FROM native_usage_facts WHERE source_id=?1 AND native_offset>?2 AND model_outcome='accepted' ORDER BY native_offset LIMIT ?3", "SELECT native_offset,CASE WHEN octet_length(payload)<=8192 THEN payload ELSE NULL END FROM native_usage_facts WHERE namespace=semon_namespace() AND source_id=?1 AND native_offset>?2 AND model_outcome='accepted' ORDER BY native_offset LIMIT ?3")?)?;
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
    let mut models=tx.prepare(native_usage::sql(tx, "SELECT dimension,CASE WHEN octet_length(value)<=8192 THEN value ELSE NULL END FROM native_usage_dimensions WHERE source_id=?1 AND kind='model' ORDER BY dimension LIMIT 4097", "SELECT dimension,CASE WHEN octet_length(value)<=8192 THEN value ELSE NULL END FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 AND kind='model' ORDER BY dimension LIMIT 4097")?)?;
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
    tx.execute(native_usage::sql(tx, "DELETE FROM native_usage_dimensions WHERE source_id=?1 AND kind IN ('cost_model','cost_day')", "DELETE FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 AND kind IN ('cost_model','cost_day')")?,[job.id])?;
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
    tx.execute(native_usage::sql(tx, "UPDATE files SET codex_native_usage=?2 WHERE path=(SELECT source_path FROM native_usage_current WHERE source_id=?1)", "UPDATE files SET codex_native_usage=?2 WHERE namespace=semon_namespace() AND path=(SELECT source_path FROM native_usage_current WHERE namespace=semon_namespace() AND source_id=?1)")?,params![job.id,json(&root)?])?;
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
        native_usage::sql(
            tx,
            "INSERT INTO native_usage_dimensions VALUES(?1,?2,?3,?4)",
            "INSERT INTO native_usage_dimensions VALUES(semon_namespace(),?1,?2,?3,?4)",
        )?,
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
            let mut statement=tx.prepare(&format!("SELECT {USAGE_COLUMNS} FROM usage WHERE {}file_id=?1 AND octet_length(record_ids)<=65536 AND length(COALESCE(model,''))<=512 LIMIT ?2", copied_usage_sql(tx, "", "namespace=semon_namespace() AND ")?))?;
            let total: i64 = tx.query_row(
                copied_usage_sql(tx, "SELECT COUNT(*) FROM (SELECT 1 FROM usage WHERE file_id=?1 LIMIT ?2)", "SELECT COUNT(*) FROM (SELECT 1 FROM usage WHERE namespace=semon_namespace() AND file_id=?1 LIMIT ?2)")?,
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
            let encoded:Option<String>=tx.query_row(configured::sql(tx, "SELECT CASE WHEN octet_length(codex_native_usage)<=1048576 THEN codex_native_usage ELSE NULL END FROM files WHERE file_id=?1", "SELECT CASE WHEN octet_length(codex_native_usage)<=1048576 THEN codex_native_usage ELSE NULL END FROM files WHERE namespace=semon_namespace() AND file_id=?1")?,[file_id],|r|r.get(0))?;
            if let Some(encoded) = encoded {
                if let Some((id, _)) = native_usage::source(tx, path)? {
                    let count:i64=tx.query_row(native_usage::sql(tx, "SELECT COUNT(*) FROM (SELECT 1 FROM native_usage_dimensions WHERE source_id=?1 LIMIT 4097)", "SELECT COUNT(*) FROM (SELECT 1 FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 LIMIT 4097)")?,[id],|r|r.get(0))?;
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
                let mut statement=tx.prepare(copied_usage_sql(tx, "SELECT model,input,output,cache_write,cache_read,t FROM codex_usage WHERE file_id=?1 ORDER BY seq LIMIT ?2", "SELECT model,input,output,cache_write,cache_read,t FROM codex_usage WHERE namespace=semon_namespace() AND file_id=?1 ORDER BY seq LIMIT ?2")?)?;
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
    pending_key: Option<&str>,
) -> rusqlite::Result<bool> {
    let next: Option<(String, Option<String>)> = if let Some(key) = pending_key {
        tx.query_row("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1 AND pricing_fingerprint IS NOT ?2",params![key,table.fingerprint],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
    } else {
        tx.query_row("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog INDEXED BY session_catalog_pricing_fingerprint WHERE pricing_fingerprint IS NOT ?1 AND session_key>?2 ORDER BY session_key LIMIT 1",params![table.fingerprint,work.session_after],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
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
    tx.execute(
        "UPDATE session_catalog SET pricing_fingerprint=?2 WHERE session_key=?1",
        params![key, row.cost.pricing_fingerprint],
    )?;
    if row.cost.pricing_fingerprint == table.fingerprint {
        return Ok(false);
    }
    let Some(cost) = retained_cost(tx, &row, table)? else {
        return Ok(false);
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
        let mut history: crate::model::summary::CatalogRow = decode_json(0, &encoded)?;
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
                "UPDATE session_descriptions SET description=?2 WHERE session_key=?1",
                params![key, json(&value)?],
            )?;
        }
    }
    let overview:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(summary)<=1048576 THEN summary ELSE NULL END FROM session_overview WHERE session_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
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
                "UPDATE session_overview SET summary=?2 WHERE session_key=?1",
                params![key, json(&value)?],
            )?;
            tx.execute(metadata_increment(tx)?, ["overview_generation"])?;
        }
    }
    let trace:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation FROM session_trace_headers WHERE session_key=?1 AND source_generation=?2",params![key,crate::retention::source_generation(&row.sources).map_err(|_|rusqlite::Error::InvalidQuery)?],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
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
            "UPDATE session_trace_headers SET metadata=?2,generation=?3 WHERE session_key=?1",
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
    work: &mut Work,
    table: &Arc<EffectiveTable>,
) -> rusqlite::Result<bool> {
    if let Some(owner) = work.pending_owner.as_ref() {
        let still_pending: bool = tx.query_row(
            queue_sql(tx, "SELECT EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE owner_kind=?1 AND owner_key=?2)", "SELECT EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind=?1 AND owner_key=?2)")?,
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
                queue_sql(tx, "SELECT owner_kind,owner_key FROM pricing_recompute_pending ORDER BY owner_kind,owner_key LIMIT 1", "SELECT owner_kind,owner_key FROM pricing_recompute_pending WHERE namespace=semon_namespace() ORDER BY owner_kind,owner_key LIMIT 1")?,
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
                    native_usage::sql(tx, "SELECT s.source_id FROM native_usage_current c JOIN native_usage_sources s USING(source_id) WHERE c.source_path=?1 AND s.parser_version=?2 AND s.usage_ready=1", "SELECT s.source_id FROM native_usage_current c JOIN native_usage_sources s USING(namespace,source_id) WHERE c.namespace=semon_namespace() AND c.source_path=?1 AND s.parser_version=?2 AND s.usage_ready=1")?,
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
                    queue_sql(tx, "DELETE FROM pricing_recompute_pending WHERE owner_kind='source' AND owner_key=?1", "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='source' AND owner_key=?1")?,
                    [&key],
                )?;
                work.pending_owner = None;
            }
            // A non-final fact batch still made bounded progress and must
            // persist its offset while retaining the queue row.
            Ok(true)
        }
        "session" => {
            let changed = catalog_step(tx, work, table, Some(&key))?;
            if changed {
                tx.execute(
                    queue_sql(tx, "INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key) SELECT 'family',?1", "INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key) SELECT semon_namespace(),'family',?1")?,
                    [&key],
                )?;
            }
            tx.execute(
                queue_sql(tx, "DELETE FROM pricing_recompute_pending WHERE owner_kind='session' AND owner_key=?1", "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='session' AND owner_key=?1")?,
                [&key],
            )?;
            work.pending_owner = None;
            Ok(true)
        }
        "family" => {
            family_owner(tx, &key, &table.fingerprint)?;
            let parent: Option<String> = tx
                .query_row(
                    "SELECT parent_key FROM session_catalog WHERE session_key=?1",
                    [&key],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if let Some(parent) = parent {
                tx.execute(
                    queue_sql(tx, "INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key) SELECT 'family',?1 WHERE EXISTS(SELECT 1 FROM session_trace_headers WHERE session_key=?1)", "INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key) SELECT semon_namespace(),'family',?1 WHERE EXISTS(SELECT 1 FROM session_trace_headers WHERE session_key=?1)")?,
                    [&parent],
                )?;
            }
            tx.execute(
                queue_sql(tx, "DELETE FROM pricing_recompute_pending WHERE owner_kind='family' AND owner_key=?1", "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='family' AND owner_key=?1")?,
                [&key],
            )?;
            work.pending_owner = None;
            Ok(true)
        }
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn family_step(tx: &Transaction<'_>, work: &mut Work) -> rusqlite::Result<()> {
    let key:Option<String>=tx.query_row("SELECT session_key FROM session_trace_headers WHERE session_key>?1 ORDER BY session_key LIMIT 1",[&work.session_after],|r|r.get(0)).optional()?;
    let Some(key) = key else {
        work.phase = 3;
        return Ok(());
    };
    work.session_after = key.clone();
    family_owner(tx, &key, &work.target_fingerprint)
}

fn family_owner(tx: &Transaction<'_>, key: &str, fingerprint: &str) -> rusqlite::Result<()> {
    let witness:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END,t.source_generation FROM session_catalog c JOIN session_trace_headers t USING(session_key) WHERE session_key=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
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
    let mut statement=tx.prepare("WITH RECURSIVE family(id) AS (SELECT ?1 UNION SELECT c.session_key FROM session_catalog c JOIN family f ON c.parent_key=f.id LIMIT 257) SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_extract(metadata,'$.cost.usd') END,CASE WHEN octet_length(metadata)<=1048576 THEN json_extract(metadata,'$.cost.pricing_fingerprint') END FROM family JOIN session_catalog ON session_key=id")?;
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
    let prior:Option<(String,String)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation FROM session_trace_headers WHERE session_key=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
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

    #[test]
    fn native_usage_copied_layout_is_independent_of_core_and_native_layout() {
        for copied_composite in [false, true] {
            let mut db = Connection::open_in_memory().unwrap();
            db.execute_batch(configured::CORE_SCHEMA).unwrap();
            db.execute_batch(metadata_schema(false)).unwrap();
            for ddl in native_usage::schema(true) {
                db.execute_batch(ddl).unwrap();
            }
            db.execute_batch(copied_usage_schema(copied_composite))
                .unwrap();
            db.execute_batch(tree_state::schema(copied_composite))
                .unwrap();
            bind_fixture(&db, "a");
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
        // Catalog and metadata layouts selected independently of the core.
        for meta_composite in [false, true] {
            for catalog_composite in [false, true] {
                let root = root();
                let database = root.join("families.sqlite3");
                let mut db = Connection::open(&database).unwrap();
                db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
                    .unwrap();
                db.execute_batch(configured::CORE_SCHEMA).unwrap();
                db.execute_batch(metadata_schema(meta_composite)).unwrap();
                db.execute_batch(copied_usage_schema(true)).unwrap();
                for ddl in native_usage::schema(true) {
                    db.execute_batch(ddl).unwrap();
                }
                db.execute_batch(tree_state::schema(true)).unwrap();
                db.execute_batch(queue_schema(true)).unwrap();
                db.execute_batch(if catalog_composite {
                    "CREATE TABLE session_catalog(namespace TEXT NOT NULL,session_key TEXT NOT NULL,metadata TEXT NOT NULL,pricing_fingerprint TEXT,PRIMARY KEY(namespace,session_key)) STRICT, WITHOUT ROWID;"
                } else {
                    "CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY,metadata TEXT NOT NULL,pricing_fingerprint TEXT) STRICT;"
                }).unwrap();
                db.execute_batch(catalog_schema(true, catalog_composite, meta_composite))
                    .unwrap();
                db.execute_batch(file_schema(true, meta_composite)).unwrap();
                bind_fixture(&db, "a");
                db.execute(native_metadata_put(&db).unwrap(), params![TARGET, "target"])
                    .unwrap();
                if meta_composite {
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
                            native_usage::begin_source(&tx, path, None, &ledger, false).unwrap(),
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
                    tx.execute(native_usage::repair_sql(&tx,
                        "INSERT INTO native_tree_repairs(source_path,receipt) VALUES(?1,?2)",
                        "INSERT INTO native_tree_repairs(namespace,source_path,receipt) VALUES(semon_namespace(),?1,?2)").unwrap(), params!["same/source.jsonl", header_ledger(&ledger)]).unwrap();
                    let (records, tokens, _) = tree_state::shared_usage(&tx, 1, 1).unwrap();
                    assert_eq!(records["same-message"].len(), 1);
                    assert_eq!(tokens["same-message"].input, amount);
                    assert_eq!(tx.query_row(queue_sql(&tx,
                        "SELECT count(*) FROM pricing_recompute_pending WHERE owner_kind='source'",
                        "SELECT count(*) FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='source'").unwrap(), [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                    let key = if catalog_composite {
                        "same-session".to_owned()
                    } else {
                        format!("session-{namespace}")
                    };
                    if catalog_composite {
                        tx.execute("INSERT INTO session_catalog(namespace,session_key,metadata) VALUES(semon_namespace(),?1,?2)", params![key, r#"{"cost":{"pricing_fingerprint":"stale"}}"#]).unwrap();
                    } else {
                        tx.execute(
                            "INSERT INTO session_catalog(session_key,metadata) VALUES(?1,?2)",
                            params![key, r#"{"cost":{"pricing_fingerprint":"stale"}}"#],
                        )
                        .unwrap();
                    }
                    assert!(tx.query_row(queue_sql(&tx,
                        "SELECT EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE owner_kind='session' AND owner_key=?1)",
                        "SELECT EXISTS(SELECT 1 FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='session' AND owner_key=?1)").unwrap(), [key], |r| r.get::<_, bool>(0)).unwrap());
                    tx.commit().unwrap();
                }
                bind_fixture(&db, "a");
                if meta_composite {
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
                if catalog_composite {
                    db.execute(
                        "DELETE FROM pricing_recompute_pending WHERE owner_kind='session'",
                        [],
                    )
                    .unwrap();
                    db.execute(
                        "UPDATE session_catalog SET metadata=metadata WHERE namespace='b'",
                        [],
                    )
                    .unwrap();
                    assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace='a' AND owner_kind='session'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
                    assert_eq!(db.query_row("SELECT count(*) FROM pricing_recompute_pending WHERE namespace='b' AND owner_kind='session'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                }
                let b = usage_family_state(&db, "b");
                let a = usage_family_state(&db, "a");
                bind_fixture(&db, "a");
                {
                    let tx = db.transaction().unwrap();
                    let target = table("openai", "gpt-6.1-sol", 7.0, 1);
                    assert!(source_step(&tx, &mut Work::default(), &target, Some(1)).unwrap());
                    clear(&tx, 1).unwrap();
                    assert_eq!(tree_state::shared_usage(&tx, 2, 2).unwrap().0.len(), 0);
                    assert_eq!(usage_family_state(&tx, "b"), b);
                    tx.rollback().unwrap();
                }
                assert_eq!(usage_family_state(&db, "a"), a);
                assert_eq!(usage_family_state(&db, "b"), b);
                drop(db);
                let mut db = Connection::open(&database).unwrap();
                db.pragma_update(None, "foreign_keys", true).unwrap();
                bind_fixture(&db, "a");
                assert!(native_usage::repair_pending(&db).unwrap());
                let tx = db.transaction().unwrap();
                let target = table("openai", "gpt-6.1-sol", 7.0, 1);
                let mut work = Work::default();
                // Keep only the source owner to exercise actual pending selection
                // and deletion without invoking the unqualified Catalog producer.
                tx.execute(queue_sql(&tx,
                    "DELETE FROM pricing_recompute_pending WHERE owner_kind='session'",
                    "DELETE FROM pricing_recompute_pending WHERE namespace=semon_namespace() AND owner_kind='session'").unwrap(), []).unwrap();
                assert!(pending_step(&tx, &mut work, &target).unwrap());
                assert!(!pending_step(&tx, &mut work, &target).unwrap());
                clear(&tx, 1).unwrap();
                assert!(tree_state::shared_usage(&tx, 2, 2).unwrap().0.is_empty());
                assert_eq!(usage_family_state(&tx, "b"), b);
                tx.commit().unwrap();
                assert_eq!(usage_family_state(&db, "b"), b);
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

    #[test]
    fn native_usage_composite_repricing_isolates_colliding_source_ids_and_rollback() {
        use rusqlite::functions::FunctionFlags;
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE files (
            namespace TEXT NOT NULL,file_id INTEGER NOT NULL,path TEXT NOT NULL,
            codex_native_usage TEXT,codex_pricing_fingerprint TEXT,
            PRIMARY KEY(namespace,file_id),UNIQUE(namespace,path));
            CREATE INDEX files_codex_pricing_fingerprint ON files(namespace,file_id,codex_pricing_fingerprint) WHERE codex_native_usage IS NOT NULL;").unwrap();
        for ddl in native_usage::schema(true) {
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
        let (_store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
        let table = table("anthropic", "claude-opus-5-5", 8.0, 1);
        drain(&path, &table);
        let before: (i64, String) = Connection::open(&path)
            .unwrap()
            .query_row("SELECT rowid,value FROM meta WHERE key=?1", [WORK], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let decoded = REPRICE_PAYLOAD_DECODES.with(std::cell::Cell::get);
        crate::pricing::runtime::register_index(path.clone());
        crate::pricing::runtime::register_index(path.clone());
        for _ in 0..8 {
            assert!(!step_inner(&path, Arc::clone(&table), false).unwrap());
        }
        let after: (i64, String) = Connection::open(&path)
            .unwrap()
            .query_row("SELECT rowid,value FROM meta WHERE key=?1", [WORK], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
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
        let (mut store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
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

        assert!(step_inner(&path, Arc::clone(&current), false).unwrap());
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
            "INSERT INTO native_usage_identities VALUES(?1,'tree','append-129',129)",
            [p_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_identities VALUES(?1,'model','append-129',129)",
            [p_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_facts VALUES(?1,129,'append-129',?2,?3,'accepted','accepted')",
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
            let progressed = step_inner(&path, Arc::clone(&current), false).unwrap();
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
        let (mut store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
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
        assert!(step_inner(&path, Arc::clone(&current), false).unwrap());
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
            "INSERT INTO native_usage_identities VALUES(?1,'tree','replacement-0',0)",
            [replacement_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_identities VALUES(?1,'model','replacement-0',0)",
            [replacement_id],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_facts VALUES(?1,0,'replacement-0',?2,?3,'accepted','accepted')",
            params![replacement_id, replacement_payload.to_string(), replacement_record.to_string()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_usage_dimensions VALUES(?1,'model','gpt-6.1-sol',?2)",
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

        assert!(step_inner(&path, Arc::clone(&current), false).unwrap());
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
            let progressed = step_inner(&path, Arc::clone(&current), false).unwrap();
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
        let (mut store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
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
        let (mut store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
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
        let (mut store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
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
        assert_eq!(parser_version(&c).unwrap(), Some(CACHE_VERSION));
        drop(c);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_changed_retained_facts_keep_the_previous_price_fingerprint() {
        let root = root();
        let path = root.join("index.sqlite3");
        let (store, _) = SqliteStore::attempt(&path).unwrap_or_else(|e| panic!("{e}"));
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
