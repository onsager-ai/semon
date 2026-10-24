//! Source-local explicit usage facts. Counts remain JSON u64 values: SQLite
//! signed INTEGER/SUM must never narrow or convert native token counters.
use super::*;
use crate::{events::CodexNativeUsage, native_record::Record, pricing::CostBuild};
use std::collections::BTreeSet;

mod repair;
#[cfg(test)]
std::thread_local! {
    pub(crate) static RESTORED_DIMENSION_ROWS:std::cell::Cell<u64>=const {std::cell::Cell::new(0)};
}
pub(super) use repair::advance;
pub(super) use repair::pending as repair_pending;

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
CREATE TABLE IF NOT EXISTS native_tree_repairs (
 source_path TEXT PRIMARY KEY, receipt TEXT NOT NULL, source_id INTEGER,
 cursor INTEGER NOT NULL DEFAULT 0, projection_id INTEGER, last_model TEXT,
 tree_root TEXT, state TEXT NOT NULL DEFAULT 'ready', last_attempt INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_tree_repair_queue ON native_tree_repairs(state,last_attempt,source_path);
";

/// Authored namespace form of the native usage family. The source id is local
/// to a namespace, so identical stores can independently allocate the same id.
pub(super) const NAMESPACED_DDL: &str = "
CREATE TABLE IF NOT EXISTS native_usage_sources (
 namespace TEXT NOT NULL, source_id INTEGER NOT NULL, source_path TEXT NOT NULL,
 receipt TEXT NOT NULL, checkpoint INTEGER NOT NULL, parser_version INTEGER NOT NULL,
 usage_ready INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_usage_source_path ON native_usage_sources(namespace,source_path,source_id);
CREATE TABLE IF NOT EXISTS native_usage_current (
 namespace TEXT NOT NULL, source_path TEXT NOT NULL, source_id INTEGER NOT NULL,
 PRIMARY KEY(namespace,source_path), UNIQUE(namespace,source_id),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_usage_facts (
 namespace TEXT NOT NULL, source_id INTEGER NOT NULL, native_offset INTEGER NOT NULL,
 identity TEXT NOT NULL, payload TEXT NOT NULL, record TEXT NOT NULL, outcome TEXT NOT NULL,
 model_outcome TEXT NOT NULL CHECK(model_outcome IN ('unavailable','accepted','duplicate','conflict')),
 CHECK(outcome IN ('accepted','duplicate','conflict')),
 PRIMARY KEY(namespace,source_id,native_offset),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_usage_identity_observations ON native_usage_facts(namespace,source_id,identity,native_offset);
CREATE TABLE IF NOT EXISTS native_usage_identities (
 namespace TEXT NOT NULL, source_id INTEGER NOT NULL,
 scope TEXT NOT NULL CHECK(scope IN ('tree','model')), identity TEXT NOT NULL,
 native_offset INTEGER NOT NULL,
 PRIMARY KEY(namespace,source_id,scope,identity),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_usage_dimensions (
 namespace TEXT NOT NULL, source_id INTEGER NOT NULL,
 kind TEXT NOT NULL, dimension TEXT NOT NULL, value TEXT NOT NULL,
 PRIMARY KEY(namespace,source_id,kind,dimension),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_repairs (
 namespace TEXT NOT NULL, source_path TEXT NOT NULL, receipt TEXT NOT NULL, source_id INTEGER,
 cursor INTEGER NOT NULL DEFAULT 0, projection_id INTEGER, last_model TEXT,
 tree_root TEXT, state TEXT NOT NULL DEFAULT 'ready', last_attempt INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,source_path),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_tree_repair_queue ON native_tree_repairs(namespace,state,last_attempt,source_path);
";

/// Select a complete authored statement by the usage family's own physical
/// schema. In particular, core files and meta may have independent layouts.
fn usage_sql<'a>(
    db: &Connection,
    legacy: &'a str,
    scoped: &'a str,
) -> rusqlite::Result<(bool, &'a str)> {
    let composite: bool = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_table_info('native_usage_sources') WHERE name='namespace' AND pk=1)")?.query_row([], |r| r.get(0))?;
    Ok((composite, if composite { scoped } else { legacy }))
}

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

pub(super) fn source_in(
    c: &Connection,
    namespace: &str,
    path: &str,
) -> rusqlite::Result<Option<(i64, String)>> {
    let (scoped, query) = usage_sql(
        c,
        "SELECT s.source_id,s.receipt FROM native_usage_current c JOIN native_usage_sources s USING(source_id) WHERE c.source_path=?1 AND s.parser_version=?2",
        "SELECT s.source_id,s.receipt FROM native_usage_current c JOIN native_usage_sources s ON s.namespace=c.namespace AND s.source_id=c.source_id WHERE c.namespace=?1 AND c.source_path=?2 AND s.parser_version=?3",
    )?;
    if scoped {
        c.query_row(query, params![namespace, path, CACHE_VERSION], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
    } else {
        c.query_row(query, params![path, CACHE_VERSION], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
    }
}

pub(super) fn normalized(
    c: &Connection,
    namespace: &str,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<bool> {
    let Some((id, receipt)) = source_in(c, namespace, path)? else {
        return Ok(false);
    };
    let (scoped, query) = usage_sql(
        c,
        "SELECT usage_ready FROM native_usage_sources WHERE source_id=?1",
        "SELECT usage_ready FROM native_usage_sources WHERE namespace=?1 AND source_id=?2",
    )?;
    let ready = if scoped {
        c.query_row(query, params![namespace, id], |r| r.get::<_, bool>(0))?
    } else {
        c.query_row(query, [id], |r| r.get::<_, bool>(0))?
    };
    Ok(receipt == header_ledger(ledger) && ready)
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Seen {
    pub(crate) tree: bool,
    pub(crate) model: bool,
}

pub(super) fn seen(
    c: &Connection,
    namespace: &str,
    path: &str,
    ledger: &Ledger,
    identity: &str,
) -> rusqlite::Result<Seen> {
    let Some((id, receipt)) = source_in(c, namespace, path)? else {
        return Ok(Seen::default());
    };
    if !normalized(c, namespace, path, ledger)? {
        return Ok(Seen::default());
    }
    if receipt != header_ledger(ledger) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let (scoped, query) = usage_sql(
        c,
        "SELECT EXISTS(SELECT 1 FROM native_usage_identities WHERE source_id=?1 AND scope='tree' AND identity=?2),EXISTS(SELECT 1 FROM native_usage_identities WHERE source_id=?1 AND scope='model' AND identity=?2)",
        "SELECT EXISTS(SELECT 1 FROM native_usage_identities WHERE namespace=?1 AND source_id=?2 AND scope='tree' AND identity=?3),EXISTS(SELECT 1 FROM native_usage_identities WHERE namespace=?1 AND source_id=?2 AND scope='model' AND identity=?3)",
    )?;
    if scoped {
        c.query_row(query, params![namespace, id, identity], |r| {
            Ok(Seen {
                tree: r.get(0)?,
                model: r.get(1)?,
            })
        })
    } else {
        c.query_row(query, params![id, identity], |r| {
            Ok(Seen {
                tree: r.get(0)?,
                model: r.get(1)?,
            })
        })
    }
}

pub(super) fn restore(
    c: &Connection,
    namespace: &str,
    path: &str,
    ledger: &Ledger,
    encoded: Option<String>,
) -> rusqlite::Result<Option<CodexNativeUsage>> {
    let Some(encoded) = encoded else {
        return Ok(None);
    };
    if !normalized(c, namespace, path, ledger)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let id = source_in(c, namespace, path)?
        .ok_or(rusqlite::Error::InvalidQuery)?
        .0;
    restore_root(c, namespace, id, &encoded).map(Some)
}

fn restore_root(
    c: &Connection,
    namespace: &str,
    id: i64,
    encoded: &str,
) -> rusqlite::Result<CodexNativeUsage> {
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
    let (scoped, query) = usage_sql(
        c,
        "SELECT kind,dimension,value FROM native_usage_dimensions WHERE source_id=?1 ORDER BY kind,dimension",
        "SELECT kind,dimension,value FROM native_usage_dimensions WHERE namespace=?1 AND source_id=?2 ORDER BY kind,dimension",
    )?;
    let mut statement = c.prepare(query)?;
    let mut rows = if scoped {
        statement.query(params![namespace, id])?
    } else {
        statement.query([id])?
    };
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
    namespace: &str,
    id: i64,
    kind: &str,
    key: &str,
    value: &T,
) -> rusqlite::Result<()> {
    let (scoped, query) = usage_sql(
        tx,
        "INSERT INTO native_usage_dimensions VALUES(?1,?2,?3,?4) ON CONFLICT(source_id,kind,dimension) DO UPDATE SET value=excluded.value",
        "INSERT INTO native_usage_dimensions(namespace,source_id,kind,dimension,value) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,source_id,kind,dimension) DO UPDATE SET value=excluded.value",
    )?;
    if scoped {
        tx.prepare_cached(query)?
            .execute(params![namespace, id, kind, key, json(value)?])?;
    } else {
        tx.prepare_cached(query)?
            .execute(params![id, kind, key, json(value)?])?;
    }
    Ok(())
}

pub(super) fn write(
    tx: &Transaction<'_>,
    namespace: &str,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
) -> rusqlite::Result<()> {
    let Some(_) = &index.native_usage_observations else {
        return Ok(());
    };
    let id = source_in(tx, namespace, path)?.ok_or(rusqlite::Error::InvalidQuery)?.0;
    write_observations(tx, namespace, id, index)?;
    let checkpoint = i64::try_from(ledger.offset).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let (scoped, query) = usage_sql(
        tx,
        "UPDATE native_usage_sources SET usage_ready=1,checkpoint=?2 WHERE source_id=?1",
        "UPDATE native_usage_sources SET usage_ready=1,checkpoint=?3 WHERE namespace=?1 AND source_id=?2",
    )?;
    if scoped { tx.execute(query, params![namespace,id,checkpoint])?; }
    else { tx.execute(query, params![id,checkpoint])?; }
    Ok(())
}

pub(super) fn begin_source(
    tx: &Transaction<'_>,
    namespace: &str,
    path: &str,
    expected: Option<&Ledger>,
    ledger: &Ledger,
    append: bool,
) -> rusqlite::Result<i64> {
    let (scoped, _) = usage_sql(tx, "", "")?;
    let id = match source_in(tx, namespace, path)? {
        Some((id, receipt))
            if (append || expected == Some(ledger))
                && expected.is_some_and(|old| receipt == header_ledger(old)) =>
        {
            id
        }
        _ => {
            let receipt = header_ledger(ledger);
            let id = if scoped {
                tx.query_row("INSERT INTO native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version) VALUES(?1,coalesce((SELECT max(source_id)+1 FROM native_usage_sources WHERE namespace=?1),1),?2,?3,0,?4) RETURNING source_id",params![namespace,path,receipt,CACHE_VERSION],|r|r.get(0))?
            } else {
                tx.execute("INSERT INTO native_usage_sources(source_path,receipt,checkpoint,parser_version) VALUES(?1,?2,0,?3)",params![path,receipt,CACHE_VERSION])?;
                tx.last_insert_rowid()
            };
            let (query_scoped, query) = usage_sql(
                tx,
                "INSERT INTO native_usage_current VALUES(?1,?2) ON CONFLICT(source_path) DO UPDATE SET source_id=excluded.source_id",
                "INSERT INTO native_usage_current(namespace,source_path,source_id) VALUES(?1,?2,?3) ON CONFLICT(namespace,source_path) DO UPDATE SET source_id=excluded.source_id",
            )?;
            if query_scoped { tx.execute(query, params![namespace,path,id])?; }
            else { tx.execute(query, params![path,id])?; }
            id
        }
    };
    let (query_scoped, query) = usage_sql(
        tx,
        "UPDATE native_usage_sources SET receipt=?2 WHERE source_id=?1",
        "UPDATE native_usage_sources SET receipt=?3 WHERE namespace=?1 AND source_id=?2",
    )?;
    if query_scoped { tx.execute(query, params![namespace,id,header_ledger(ledger)])?; }
    else { tx.execute(query, params![id,header_ledger(ledger)])?; }
    let (query_scoped, query) = usage_sql(
        tx,
        "UPDATE native_tree_repairs SET state='ready' WHERE source_path=?1 AND state LIKE 'blocked_%' AND (receipt<>?2 OR state='blocked_evidence')",
        "UPDATE native_tree_repairs SET state='ready' WHERE namespace=?1 AND source_path=?2 AND state LIKE 'blocked_%' AND (receipt<>?3 OR state='blocked_evidence')",
    )?;
    if query_scoped { tx.execute(query, params![namespace,path,header_ledger(ledger)])?; }
    else { tx.execute(query, params![path,header_ledger(ledger)])?; }
    Ok(id)
}

fn write_observations(tx: &Transaction<'_>, namespace: &str, id: i64, index: &FileIndex) -> rusqlite::Result<()> {
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
        let (query_scoped, query) = usage_sql(
            tx,
            "SELECT payload,record FROM native_usage_facts WHERE source_id=?1 AND native_offset=?2",
            "SELECT payload,record FROM native_usage_facts WHERE namespace=?1 AND source_id=?2 AND native_offset=?3",
        )?;
        let replay: Option<(String,String)> = if query_scoped {
            tx.query_row(query,params![namespace,id,offset],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
        } else {
            tx.query_row(query,params![id,offset],|r|Ok((r.get(0)?,r.get(1)?))).optional()?
        };
        if let Some(replay) = replay {
            if replay != (payload, evidence) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            continue;
        }
        let outcome = |scope: &str| -> rusqlite::Result<&str> {
            let (query_scoped, query) = usage_sql(
                tx,
                "SELECT f.payload FROM native_usage_identities i JOIN native_usage_facts f ON f.source_id=i.source_id AND f.native_offset=i.native_offset WHERE i.source_id=?1 AND i.scope=?2 AND i.identity=?3",
                "SELECT f.payload FROM native_usage_identities i JOIN native_usage_facts f ON f.namespace=i.namespace AND f.source_id=i.source_id AND f.native_offset=i.native_offset WHERE i.namespace=?1 AND i.source_id=?2 AND i.scope=?3 AND i.identity=?4",
            )?;
            let first: Option<String> = if query_scoped {
                tx.query_row(query,params![namespace,id,scope,observation.identity],|r|r.get(0)).optional()?
            } else {
                tx.query_row(query,params![id,scope,observation.identity],|r|r.get(0)).optional()?
            };
            let outcome = match first.as_deref() {
                None => "accepted",
                Some(first) if first == payload => "duplicate",
                Some(_) => "conflict",
            };
            if outcome == "accepted" {
                let (query_scoped, query) = usage_sql(
                    tx,
                    "INSERT INTO native_usage_identities VALUES(?1,?2,?3,?4)",
                    "INSERT INTO native_usage_identities(namespace,source_id,scope,identity,native_offset) VALUES(?1,?2,?3,?4,?5)",
                )?;
                if query_scoped { tx.prepare_cached(query)?.execute(params![namespace,id,scope,observation.identity,offset])?; }
                else { tx.prepare_cached(query)?.execute(params![id,scope,observation.identity,offset])?; }
            }
            Ok(outcome)
        };
        let tree_outcome = outcome("tree")?;
        let model_outcome = if observation.payload.model.is_some() {
            outcome("model")?
        } else {
            "unavailable"
        };
        let (query_scoped, query) = usage_sql(
            tx,
            "INSERT INTO native_usage_facts VALUES(?1,?2,?3,?4,?5,?6,?7)",
            "INSERT INTO native_usage_facts(namespace,source_id,native_offset,identity,payload,record,outcome,model_outcome) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        )?;
        if query_scoped { tx.prepare_cached(query)?.execute(params![namespace,id,offset,observation.identity,payload,evidence,tree_outcome,model_outcome])?; }
        else { tx.prepare_cached(query)?.execute(params![
                id,
                offset,
                observation.identity,
                payload,
                evidence,
                tree_outcome,
                model_outcome
            ])?; }
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
                put_dimension(tx, namespace, id, "model", &model, tokens)?;
            }
            if let Some(value) = cost.models.get(&model) {
                put_dimension(tx, namespace, id, "cost_model", &model, value)?;
            }
        }
        for day in days {
            if let Some(value) = cost.by_day.get(&day) {
                put_dimension(tx, namespace, id, "cost_day", &day, value)?;
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
    namespace: &str,
    old: &str,
    path: &str,
    prior: &Ledger,
    relocated: &Ledger,
) -> rusqlite::Result<bool> {
    let Some((id, receipt)) = source_in(tx, namespace, old)? else {
        return Ok(false);
    };
    let normalized_usage = normalized(tx, namespace, old, prior)?;
    if receipt != header_ledger(prior) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let (scoped, query) = usage_sql(
        tx,
        "UPDATE native_usage_current SET source_path=?2 WHERE source_path=?1",
        "UPDATE native_usage_current SET source_path=?3 WHERE namespace=?1 AND source_path=?2",
    )?;
    if scoped { tx.execute(query, params![namespace,old,path])?; }
    else { tx.execute(query, params![old,path])?; }
    let (scoped, query) = usage_sql(
        tx,
        "UPDATE native_usage_sources SET receipt=?2 WHERE source_id=?1",
        "UPDATE native_usage_sources SET receipt=?3 WHERE namespace=?1 AND source_id=?2",
    )?;
    if scoped { tx.execute(query, params![namespace,id,header_ledger(relocated)])?; }
    else { tx.execute(query, params![id,header_ledger(relocated)])?; }
    Ok(normalized_usage)
}

#[cfg(test)]
mod namespace_tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "semon-native-usage-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn ledger(offset: u64) -> Ledger {
        Ledger {
            stat: Stat {
                dev: 7,
                ino: 9,
                size: offset,
                modified_ns: 11,
                changed_ns: Some(13),
            },
            offset,
            prefix: [17; 32],
            tail: [19; 32],
        }
    }

    fn record(turn: &str) -> Value {
        serde_json::json!({
            "type": "token_usage_record",
            "payload": {
                "thread_id": "thread-shared",
                "session_id": "session-shared",
                "turn_id": turn,
                "response_id": format!("response-{turn}"),
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 3,
                    "total_tokens": 14,
                    "cached_input_tokens": 1,
                    "cache_write_input_tokens": 2,
                    "reasoning_output_tokens": 0
                }
            }
        })
    }

    fn usage_index(events: &[(u64, &str)]) -> FileIndex {
        let mut index = FileIndex {
            native_usage_observations: Some(Vec::new()),
            ..Default::default()
        };
        for (offset, turn) in events {
            let record = record(turn);
            let bytes = serde_json::to_vec(&record).unwrap();
            CodexNativeUsage::observe(
                &mut index.codex_native_usage,
                &record["payload"],
                "model-shared".into(),
                Some(1_800_000_000),
            );
            index
                .native_usage_observations
                .as_mut()
                .unwrap()
                .push(Observation::capture(&record, &bytes, *offset, Some("model-shared")).unwrap());
        }
        index
    }

    #[test]
    fn two_namespaces_isolate_same_usage_ids_across_append_replay_restart_and_rollback() {
        let path = fixture_path();
        let namespace_a = "fixture-source-a";
        let namespace_b = "fixture-source-b";
        let source_path = "/same/session.jsonl";
        let ledger_a = ledger(100);
        let ledger_b = ledger(100);
        let first = usage_index(&[(8, "same-turn")]);

        let mut db = Connection::open(&path).unwrap();
        db.pragma_update(None, "foreign_keys", true).unwrap();
        db.execute_batch(NAMESPACED_DDL).unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let id_a = begin_source(&tx, namespace_a, source_path, None, &ledger_a, false).unwrap();
        write(&tx, namespace_a, source_path, &ledger_a, &first).unwrap();
        tx.commit().unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let id_b = begin_source(&tx, namespace_b, source_path, None, &ledger_b, false).unwrap();
        write(&tx, namespace_b, source_path, &ledger_b, &first).unwrap();
        tx.commit().unwrap();
        assert_eq!(id_a, id_b);
        assert_eq!(id_a, 1);
        assert!(normalized(&db, namespace_a, source_path, &ledger_a).unwrap());
        assert!(normalized(&db, namespace_b, source_path, &ledger_b).unwrap());
        let first_identity = first.native_usage_observations.as_ref().unwrap()[0]
            .identity()
            .to_owned();
        let seen_a = seen(&db, namespace_a, source_path, &ledger_a, &first_identity).unwrap();
        let seen_b = seen(&db, namespace_b, source_path, &ledger_b, &first_identity).unwrap();
        assert!(seen_a.tree && seen_a.model);
        assert!(seen_b.tree && seen_b.model);

        let mut ledger_appended = ledger_a.clone();
        ledger_appended.offset = 200;
        ledger_appended.stat.size = 200;
        let appended = usage_index(&[(8, "same-turn"), (150, "second-turn")]);
        let appended_encoded = encoded(&appended).unwrap().unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let appended_id = begin_source(
            &tx,
            namespace_a,
            source_path,
            Some(&ledger_a),
            &ledger_appended,
            true,
        )
        .unwrap();
        write(&tx, namespace_a, source_path, &ledger_appended, &appended).unwrap();
        tx.commit().unwrap();
        assert_eq!(appended_id, id_a);

        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        begin_source(
            &tx,
            namespace_a,
            source_path,
            Some(&ledger_appended),
            &ledger_appended,
            true,
        )
        .unwrap();
        write(&tx, namespace_a, source_path, &ledger_appended, &appended).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_usage_facts WHERE namespace=?1", [namespace_a], |r| r.get::<_, i64>(0)).unwrap(),
            2
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_usage_facts WHERE namespace=?1", [namespace_b], |r| r.get::<_, i64>(0)).unwrap(),
            1
        );
        let restored = restore(
            &db,
            namespace_a,
            source_path,
            &ledger_appended,
            Some(appended_encoded.clone()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(restored.codex_usage().get("model-shared").unwrap().input, 20);
        let second_identity = appended.native_usage_observations.as_ref().unwrap()[1]
            .identity()
            .to_owned();
        let second_a = seen(&db, namespace_a, source_path, &ledger_appended, &second_identity)
            .unwrap();
        let second_b = seen(&db, namespace_b, source_path, &ledger_b, &second_identity).unwrap();
        assert!(second_a.tree && second_a.model);
        assert!(!second_b.tree && !second_b.model);

        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        begin_source(
            &tx,
            namespace_a,
            source_path,
            Some(&ledger_appended),
            &ledger_appended,
            true,
        )
        .unwrap();
        let mut failing = usage_index(&[(300, "third-turn")]);
        let changed = record("second-turn");
        let different_bytes = serde_json::to_vec_pretty(&changed).unwrap();
        failing.native_usage_observations.as_mut().unwrap().push(
            Observation::capture(&changed, &different_bytes, 150, Some("model-shared")).unwrap(),
        );
        assert!(write(&tx, namespace_a, source_path, &ledger_appended, &failing).is_err());
        tx.rollback().unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_usage_facts WHERE namespace=?1", [namespace_a], |r| r.get::<_, i64>(0)).unwrap(),
            2
        );
        drop(db);

        let db = Connection::open(&path).unwrap();
        let reopened = restore(
            &db,
            namespace_a,
            source_path,
            &ledger_appended,
            Some(appended_encoded),
        )
        .unwrap()
        .unwrap();
        assert_eq!(reopened.codex_usage().get("model-shared").unwrap().input, 20);
        assert!(normalized(&db, namespace_b, source_path, &ledger_b).unwrap());
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
