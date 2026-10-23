//! Source-local explicit usage facts. Counts remain JSON u64 values: SQLite
//! signed INTEGER/SUM must never narrow or convert native token counters.
use super::*;
use crate::{events::CodexNativeUsage, native_record::Record, pricing::CostBuild};
use std::collections::BTreeSet;

mod migration;
#[cfg(test)]
std::thread_local! {
    pub(crate) static RESTORED_DIMENSION_ROWS:std::cell::Cell<u64>=const {std::cell::Cell::new(0)};
}
pub(super) use migration::advance;
pub(super) use migration::pending as migration_pending;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_usage_sources (
 source_id INTEGER PRIMARY KEY AUTOINCREMENT, source_path TEXT NOT NULL,
 receipt TEXT NOT NULL, checkpoint INTEGER NOT NULL, parser_version INTEGER NOT NULL,
 usage_ready INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX IF NOT EXISTS native_usage_source_path ON native_usage_sources(source_path,source_id);
CREATE TABLE IF NOT EXISTS native_usage_current (
 source_path TEXT PRIMARY KEY, source_id INTEGER NOT NULL UNIQUE
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_usage_facts (
 source_id INTEGER NOT NULL, native_offset INTEGER NOT NULL, identity TEXT NOT NULL,
 payload TEXT NOT NULL, record TEXT NOT NULL, outcome TEXT NOT NULL,
 model_outcome TEXT NOT NULL CHECK(model_outcome IN ('unavailable','accepted','duplicate','conflict')),
 CHECK(outcome IN ('accepted','duplicate','conflict')),
 PRIMARY KEY(source_id,native_offset)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_usage_identity_observations ON native_usage_facts(source_id,identity,native_offset);
CREATE TABLE IF NOT EXISTS native_usage_identities (
 source_id INTEGER NOT NULL, scope TEXT NOT NULL CHECK(scope IN ('tree','model')),
 identity TEXT NOT NULL, native_offset INTEGER NOT NULL,
 PRIMARY KEY(source_id,scope,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_usage_dimensions (
 source_id INTEGER NOT NULL, kind TEXT NOT NULL, dimension TEXT NOT NULL, value TEXT NOT NULL,
 PRIMARY KEY(source_id,kind,dimension)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_usage_migrations (
 source_path TEXT PRIMARY KEY, receipt TEXT NOT NULL, source_id INTEGER,
 cursor INTEGER NOT NULL DEFAULT 0, projection_id INTEGER, usage_needed INTEGER NOT NULL DEFAULT 1, last_model TEXT, native_root TEXT,
 tree_root TEXT, state TEXT NOT NULL DEFAULT 'ready', last_attempt INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_usage_migration_queue ON native_usage_migrations(state,last_attempt,source_path);
";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Payload {
    // Option preserves the existing model consumer's unavailable-model rule.
    pub(super) model: Option<String>,
    pub(super) timestamp: Option<i64>,
    tokens: Tokens,
    pub(super) model_tokens: ModelTokens,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Observation {
    identity: String,
    payload: Payload,
    record: Record,
}

impl Observation {
    pub(crate) fn capture(
        record: &Value,
        bytes: &[u8],
        offset: u64,
        model: Option<&str>,
    ) -> Option<Self> {
        if crate::field(record, "type") != Some("token_usage_record") {
            return None;
        }
        let (identity, tokens, model_tokens) = CodexNativeUsage::decode(&record["payload"])?;
        Some(Self {
            identity,
            payload: Payload {
                model: model.map(str::to_owned),
                timestamp: super::super::record_time(record),
                tokens,
                model_tokens,
            },
            record: Record::capture(bytes, offset),
        })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
}

#[derive(Serialize, Deserialize)]
struct Root {
    projection_version: u32,
    tokens: Tokens,
    cost: CostBuild,
}

pub(super) fn encoded(index: &FileIndex) -> rusqlite::Result<Option<String>> {
    let Some(native) = &index.codex_native_usage else {
        return Ok(None);
    };
    if index.native_usage_observations.is_none() {
        return json(native).map(Some);
    }
    let mut cost = index.codex_cost();
    cost.models.clear();
    cost.by_day.clear();
    cost.unpriced_models.clear();
    json(&Root {
        projection_version: 1,
        tokens: native.tokens(),
        cost,
    })
    .map(Some)
}

pub(super) fn source(c: &Connection, path: &str) -> rusqlite::Result<Option<(i64, String)>> {
    c.query_row("SELECT s.source_id,s.receipt FROM native_usage_current c JOIN native_usage_sources s USING(source_id) WHERE c.source_path=?1 AND s.parser_version=?2", params![path,CACHE_VERSION], |r| Ok((r.get(0)?,r.get(1)?))).optional()
}

pub(super) fn normalized(c: &Connection, path: &str, ledger: &Ledger) -> rusqlite::Result<bool> {
    let Some((id, receipt)) = source(c, path)? else {
        return Ok(false);
    };
    Ok(receipt == header_ledger(ledger)
        && c.query_row(
            "SELECT usage_ready FROM native_usage_sources WHERE source_id=?1",
            [id],
            |r| r.get::<_, bool>(0),
        )?)
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Seen {
    pub(crate) tree: bool,
    pub(crate) model: bool,
}

pub(super) fn seen(
    c: &Connection,
    path: &str,
    ledger: &Ledger,
    identity: &str,
) -> rusqlite::Result<Seen> {
    let Some((id, receipt)) = source(c, path)? else {
        return Ok(Seen::default());
    };
    if !normalized(c, path, ledger)? {
        return Ok(Seen::default());
    }
    if receipt != header_ledger(ledger) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    c.query_row("SELECT EXISTS(SELECT 1 FROM native_usage_identities WHERE source_id=?1 AND scope='tree' AND identity=?2),EXISTS(SELECT 1 FROM native_usage_identities WHERE source_id=?1 AND scope='model' AND identity=?2)",params![id,identity],|r|Ok(Seen { tree:r.get(0)?,model:r.get(1)? }))
}

pub(super) fn restore(
    c: &Connection,
    path: &str,
    ledger: &Ledger,
    encoded: Option<String>,
) -> rusqlite::Result<Option<CodexNativeUsage>> {
    let Some(encoded) = encoded else {
        return Ok(None);
    };
    if !normalized(c, path, ledger)? {
        return from_json(26, &encoded).map(Some);
    }
    let id = source(c, path)?.ok_or(rusqlite::Error::InvalidQuery)?.0;
    restore_root(c, id, &encoded).map(Some)
}

fn restore_root(c: &Connection, id: i64, encoded: &str) -> rusqlite::Result<CodexNativeUsage> {
    let root: Root = from_json(26, encoded)?;
    if root.projection_version != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut native = CodexNativeUsage {
        tokens: root.tokens,
        cost: Some(root.cost),
        ..Default::default()
    };
    let cost = native.cost.as_mut().expect("cost restored");
    let mut statement = c.prepare("SELECT kind,dimension,value FROM native_usage_dimensions WHERE source_id=?1 ORDER BY kind,dimension")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        #[cfg(test)]
        RESTORED_DIMENSION_ROWS.with(|n| n.set(n.get() + 1));
        crate::selected_read::row(row, 128)?;
        let kind: String = row.get(0)?;
        let key: String = row.get(1)?;
        let value: String = row.get(2)?;
        match kind.as_str() {
            "model" => {
                native.by_model.insert(key, from_json(2, &value)?);
            }
            "cost_model" => {
                cost.models.insert(key, from_json(2, &value)?);
            }
            "cost_day" => {
                cost.by_day.insert(key, from_json(2, &value)?);
            }
            _ => return Err(rusqlite::Error::InvalidQuery),
        }
    }
    cost.unpriced_models = cost
        .models
        .iter()
        .filter(|(_, model)| model.unpriced)
        .map(|(name, _)| name.clone())
        .collect();
    Ok(native)
}

fn put_dimension<T: Serialize>(
    tx: &Transaction<'_>,
    id: i64,
    kind: &str,
    key: &str,
    value: &T,
) -> rusqlite::Result<()> {
    tx.prepare_cached("INSERT INTO native_usage_dimensions VALUES(?1,?2,?3,?4) ON CONFLICT(source_id,kind,dimension) DO UPDATE SET value=excluded.value")?.execute(params![id,kind,key,json(value)?])?;
    Ok(())
}

pub(super) fn write(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
) -> rusqlite::Result<()> {
    let Some(_) = &index.native_usage_observations else {
        return Ok(());
    };
    let id = source(tx, path)?.ok_or(rusqlite::Error::InvalidQuery)?.0;
    write_observations(tx, id, index)?;
    tx.execute(
        "UPDATE native_usage_sources SET usage_ready=1,checkpoint=?2 WHERE source_id=?1",
        params![
            id,
            i64::try_from(ledger.offset).map_err(|_| rusqlite::Error::InvalidQuery)?
        ],
    )?;
    Ok(())
}

pub(super) fn begin_source(
    tx: &Transaction<'_>,
    path: &str,
    expected: Option<&Ledger>,
    ledger: &Ledger,
    append: bool,
) -> rusqlite::Result<i64> {
    let id = match source(tx, path)? {
        Some((id, receipt))
            if (append || expected == Some(ledger))
                && expected.is_some_and(|old| receipt == header_ledger(old)) =>
        {
            id
        }
        _ => {
            tx.execute("INSERT INTO native_usage_sources(source_path,receipt,checkpoint,parser_version) VALUES(?1,?2,0,?3)",params![path,header_ledger(ledger),CACHE_VERSION])?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO native_usage_current VALUES(?1,?2) ON CONFLICT(source_path) DO UPDATE SET source_id=excluded.source_id",params![path,id])?;
            id
        }
    };
    tx.execute(
        "UPDATE native_usage_sources SET receipt=?2 WHERE source_id=?1",
        params![id, header_ledger(ledger),],
    )?;
    tx.execute("UPDATE native_usage_migrations SET state='ready' WHERE source_path=?1 AND state LIKE 'blocked_%' AND (receipt<>?2 OR state='blocked_evidence')",params![path,header_ledger(ledger)])?;
    Ok(id)
}

fn write_observations(tx: &Transaction<'_>, id: i64, index: &FileIndex) -> rusqlite::Result<()> {
    let observations = index
        .native_usage_observations
        .as_deref()
        .unwrap_or_default();
    let mut models = BTreeSet::new();
    let mut days = BTreeSet::new();
    for observation in observations {
        let offset =
            i64::try_from(observation.record.offset).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let payload = json(&observation.payload)?;
        let evidence = json(&observation.record)?;
        let replay: Option<(String,String)> = tx.query_row("SELECT payload,record FROM native_usage_facts WHERE source_id=?1 AND native_offset=?2",params![id,offset],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some(replay) = replay {
            if replay != (payload, evidence) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            continue;
        }
        let outcome = |scope: &str| -> rusqlite::Result<&str> {
            let first: Option<String> = tx.query_row("SELECT f.payload FROM native_usage_identities i JOIN native_usage_facts f ON f.source_id=i.source_id AND f.native_offset=i.native_offset WHERE i.source_id=?1 AND i.scope=?2 AND i.identity=?3",params![id,scope,observation.identity],|r|r.get(0)).optional()?;
            let outcome = match first.as_deref() {
                None => "accepted",
                Some(first) if first == payload => "duplicate",
                Some(_) => "conflict",
            };
            if outcome == "accepted" {
                tx.prepare_cached("INSERT INTO native_usage_identities VALUES(?1,?2,?3,?4)")?
                    .execute(params![id, scope, observation.identity, offset])?;
            }
            Ok(outcome)
        };
        let tree_outcome = outcome("tree")?;
        let model_outcome = if observation.payload.model.is_some() {
            outcome("model")?
        } else {
            "unavailable"
        };
        tx.prepare_cached("INSERT INTO native_usage_facts VALUES(?1,?2,?3,?4,?5,?6,?7)")?
            .execute(params![
                id,
                offset,
                observation.identity,
                payload,
                evidence,
                tree_outcome,
                model_outcome
            ])?;
        if model_outcome == "accepted"
            && let Some(model) = &observation.payload.model
        {
            models.insert(model.clone());
            if let Some(time) = observation.payload.timestamp {
                days.insert(crate::pricing::utc_day(time));
            }
        }
    }
    if let Some(native) = &index.codex_native_usage {
        let cost = index.codex_cost();
        for model in models {
            if let Some(tokens) = native.by_model.get(&model) {
                put_dimension(tx, id, "model", &model, tokens)?;
            }
            if let Some(value) = cost.models.get(&model) {
                put_dimension(tx, id, "cost_model", &model, value)?;
            }
        }
        for day in days {
            if let Some(value) = cost.by_day.get(&day) {
                put_dimension(tx, id, "cost_day", &day, value)?;
            }
        }
    }
    Ok(())
}

/// Drop transient batch identities after a successful checkpoint. Complete
/// history is never needed to restore a normalized source's append accumulator.
pub(crate) fn checkpoint(index: &mut FileIndex) {
    if index.native_usage_observations.is_none() {
        return;
    }
    if let Some(native) = &mut index.codex_native_usage {
        if native.cost.is_none() {
            let mut cost = CostBuild::default();
            for event in &native.events {
                cost.codex(event);
            }
            native.cost = Some(cost);
        }
        native.seen.clear();
        native.events.clear();
    }
    if let Some(tree) = &mut index.tree_summary
        && let Some(native) = &mut tree.codex_native_usage
    {
        *native = native.compact();
    }
    if let Some(observations) = &mut index.native_usage_observations {
        observations.clear();
    }
}

pub(super) fn relocate(
    tx: &Transaction<'_>,
    old: &str,
    path: &str,
    prior: &Ledger,
    relocated: &Ledger,
) -> rusqlite::Result<bool> {
    let Some((id, receipt)) = source(tx, old)? else {
        return Ok(false);
    };
    let normalized_usage = normalized(tx, old, prior)?;
    if receipt != header_ledger(prior) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    tx.execute(
        "UPDATE native_usage_current SET source_path=?2 WHERE source_path=?1",
        params![old, path],
    )?;
    tx.execute(
        "UPDATE native_usage_sources SET receipt=?2 WHERE source_id=?1",
        params![id, header_ledger(relocated)],
    )?;
    Ok(normalized_usage)
}
