//! Source-local explicit usage facts. Counts remain JSON u64 values: SQLite
//! signed INTEGER/SUM must never narrow or convert native token counters.
use super::*;
use crate::{events::CodexNativeUsage, native_record::Record, pricing::CostBuild};
pub(crate) mod algorithm;

#[cfg(all(test, feature = "postgres"))]
pub(crate) fn parser_fixture(path: &std::path::Path, harness: &str) -> FileIndex {
    let mut index = FileIndex::default();
    if harness == "codex" {
        index.native_usage_observations = Some(Vec::new());
    }
    parser_append_fixture(path, harness, &mut index, 0);
    index
}

#[cfg(all(test, feature = "postgres"))]
pub(crate) fn parser_append_fixture(
    path: &std::path::Path,
    harness: &str,
    index: &mut FileIndex,
    from: u64,
) {
    let mut file = crate::sealed::LogFile::open(path).unwrap();
    super::super::parse(
        &mut file,
        from,
        harness,
        index,
        None,
        None,
        &mut crate::native_timing::Measurements::default(),
    )
    .unwrap();
}

#[cfg(all(test, feature = "postgres"))]
pub(crate) fn sqlite_fixture(
    db: &mut Connection,
    path: &str,
    expected: Option<&Ledger>,
    ledger: &Ledger,
    append: bool,
    index: &FileIndex,
) -> i64 {
    let tx = db.transaction().unwrap();
    let id = begin_source(&tx, path, expected, ledger, append).unwrap();
    write(&tx, path, ledger, index).unwrap();
    tx.commit().unwrap();
    id
}

mod repair;
#[cfg(test)]
std::thread_local! {
    pub(crate) static RESTORED_DIMENSION_ROWS:std::cell::Cell<u64>=const {std::cell::Cell::new(0)};
}
pub(super) use repair::advance;
pub(super) use repair::pending as repair_pending;

const SCHEMA: &str = "
CREATE TABLE native_usage_sources (
 namespace TEXT NOT NULL,source_id INTEGER NOT NULL,source_path TEXT NOT NULL,
 receipt TEXT NOT NULL,checkpoint INTEGER NOT NULL,parser_version INTEGER NOT NULL,
 usage_ready INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX native_usage_source_path ON native_usage_sources(namespace,source_path,source_id);
CREATE TABLE native_usage_current (
 namespace TEXT NOT NULL,source_path TEXT NOT NULL,source_id INTEGER NOT NULL,
 PRIMARY KEY(namespace,source_path),UNIQUE(namespace,source_id),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE native_usage_facts (
 namespace TEXT NOT NULL,source_id INTEGER NOT NULL,native_offset INTEGER NOT NULL,
 identity TEXT NOT NULL,payload TEXT NOT NULL,record TEXT NOT NULL,outcome TEXT NOT NULL,
 model_outcome TEXT NOT NULL CHECK(model_outcome IN ('unavailable','accepted','duplicate','conflict')),
 CHECK(outcome IN ('accepted','duplicate','conflict')),
 PRIMARY KEY(namespace,source_id,native_offset),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX native_usage_identity_observations ON native_usage_facts(namespace,source_id,identity,native_offset);
CREATE TABLE native_usage_identities (
 namespace TEXT NOT NULL,source_id INTEGER NOT NULL,scope TEXT NOT NULL CHECK(scope IN ('tree','model')),
 identity TEXT NOT NULL,native_offset INTEGER NOT NULL,PRIMARY KEY(namespace,source_id,scope,identity),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE native_usage_dimensions (
 namespace TEXT NOT NULL,source_id INTEGER NOT NULL,kind TEXT NOT NULL,dimension TEXT NOT NULL,value TEXT NOT NULL,
 PRIMARY KEY(namespace,source_id,kind,dimension),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
";

const REPAIR_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_tree_repairs (
 namespace TEXT NOT NULL,source_path TEXT NOT NULL,receipt TEXT NOT NULL,source_id INTEGER,
 cursor INTEGER NOT NULL DEFAULT 0,projection_id INTEGER,last_model TEXT,
 tree_root TEXT,state TEXT NOT NULL DEFAULT 'ready',last_attempt INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,source_path),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_tree_repair_queue ON native_tree_repairs(namespace,state,last_attempt,source_path);
";

pub(crate) fn schema() -> [&'static str; 2] {
    [SCHEMA, REPAIR_SCHEMA]
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

pub(super) fn encoded(index: &FileIndex) -> rusqlite::Result<Option<String>> {
    super::super::file_rows::encoded_native_usage(index)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

pub(super) fn source(c: &Connection, path: &str) -> rusqlite::Result<Option<(i64, String)>> {
    c.query_row("SELECT s.source_id,s.receipt FROM native_usage_current c JOIN native_usage_sources s USING(namespace,source_id) WHERE c.namespace=semon_namespace() AND c.source_path=?1 AND s.parser_version=?2", params![path,CACHE_VERSION], |r| Ok((r.get(0)?,r.get(1)?))).optional()
}

pub(super) fn normalized(c: &Connection, path: &str, ledger: &Ledger) -> rusqlite::Result<bool> {
    let Some((id, receipt)) = source(c, path)? else {
        return Ok(false);
    };
    Ok(receipt == header_ledger(ledger)
        && c.query_row(
            "SELECT usage_ready FROM native_usage_sources WHERE namespace=semon_namespace() AND source_id=?1",
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
    c.query_row("SELECT EXISTS(SELECT 1 FROM native_usage_identities WHERE namespace=semon_namespace() AND source_id=?1 AND scope='tree' AND identity=?2),EXISTS(SELECT 1 FROM native_usage_identities WHERE namespace=semon_namespace() AND source_id=?1 AND scope='model' AND identity=?2)",params![id,identity],|r|Ok(Seen { tree:r.get(0)?,model:r.get(1)? }))
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
        return Err(rusqlite::Error::InvalidQuery);
    }
    let id = source(c, path)?.ok_or(rusqlite::Error::InvalidQuery)?.0;
    restore_root(c, id, &encoded).map(Some)
}

fn restore_root(c: &Connection, id: i64, encoded: &str) -> rusqlite::Result<CodexNativeUsage> {
    let mut native = crate::events::native_decode::usage_root(encoded)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let mut statement = c.prepare("SELECT kind,dimension,value FROM native_usage_dimensions WHERE namespace=semon_namespace() AND source_id=?1 ORDER BY kind,dimension")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        #[cfg(test)]
        RESTORED_DIMENSION_ROWS.with(|n| n.set(n.get() + 1));
        crate::selected_read::row(row, 128)?;
        let kind: String = row.get(0)?;
        let key: String = row.get(1)?;
        let value: String = row.get(2)?;
        crate::events::native_decode::usage_dimension(&mut native, &kind, key, &value)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    crate::events::native_decode::finish_usage(&mut native)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(native)
}

// Preserve physical SQLite errors through the backend-neutral state machine.
struct SqliteEffects<'a, 'tx> {
    tx: &'a Transaction<'tx>,
    error: Option<rusqlite::Error>,
}

impl SqliteEffects<'_, '_> {
    fn result<T>(&mut self, result: rusqlite::Result<T>) -> Result<T, StorageError> {
        result.map_err(|error| {
            self.error = Some(error);
            StorageError::Unavailable
        })
    }
}

fn with_effects<T>(
    tx: &Transaction<'_>,
    f: impl FnOnce(&mut SqliteEffects<'_, '_>) -> Result<T, StorageError>,
) -> rusqlite::Result<T> {
    let mut effects = SqliteEffects { tx, error: None };
    f(&mut effects).map_err(|_| effects.error.unwrap_or(rusqlite::Error::InvalidQuery))
}

impl algorithm::Effects for SqliteEffects<'_, '_> {
    fn current(&mut self, path: &str) -> Result<Option<(i64, String)>, StorageError> {
        self.result(source(self.tx, path))
    }
    fn create(&mut self, path: &str, receipt: &str) -> Result<i64, StorageError> {
        let tx = self.tx;
        let result = (|| {
            let id = tx.query_row("INSERT INTO native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version) VALUES(semon_namespace(),(SELECT coalesce(max(source_id),0)+1 FROM native_usage_sources WHERE namespace=semon_namespace()),?1,?2,0,?3) RETURNING source_id", params![path,receipt,CACHE_VERSION], |r| r.get::<_, i64>(0))?;
            tx.execute("INSERT INTO native_usage_current VALUES(semon_namespace(),?1,?2) ON CONFLICT(namespace,source_path) DO UPDATE SET source_id=excluded.source_id",params![path,id])?;
            Ok(id)
        })();
        self.result(result)
    }
    fn receipt(&mut self, id: i64, receipt: &str) -> Result<(), StorageError> {
        let tx = self.tx;
        let result = (|| {
            tx.execute("UPDATE native_usage_sources SET receipt=?2 WHERE namespace=semon_namespace() AND source_id=?1",params![id,receipt])?;
            Ok(())
        })();
        self.result(result)
    }
    fn unblock(&mut self, path: &str, receipt: &str) -> Result<(), StorageError> {
        let tx = self.tx;
        let result = (|| {
            tx.execute("UPDATE native_tree_repairs SET state='ready' WHERE namespace=semon_namespace() AND source_path=?1 AND state LIKE 'blocked_%' AND (receipt<>?2 OR state='blocked_evidence')",params![path,receipt])?;
            Ok(())
        })();
        self.result(result)
    }
    fn fact(&mut self, id: i64, offset: u64) -> Result<Option<(String, String)>, StorageError> {
        let tx = self.tx;
        let result = (|| {
            let offset = i64::try_from(offset).map_err(|_| rusqlite::Error::InvalidQuery)?;
            tx.query_row("SELECT payload,record FROM native_usage_facts WHERE namespace=semon_namespace() AND source_id=?1 AND native_offset=?2",params![id,offset],|r|Ok((r.get(0)?,r.get(1)?))).optional()
        })();
        self.result(result)
    }
    fn first(
        &mut self,
        id: i64,
        scope: &str,
        identity: &str,
    ) -> Result<Option<String>, StorageError> {
        let tx = self.tx;
        let result = (|| {
            tx.query_row("SELECT f.payload FROM native_usage_identities i JOIN native_usage_facts f ON f.namespace=i.namespace AND f.source_id=i.source_id AND f.native_offset=i.native_offset WHERE i.namespace=semon_namespace() AND i.source_id=?1 AND i.scope=?2 AND i.identity=?3",params![id,scope,identity],|r|r.get(0)).optional()
        })();
        self.result(result)
    }
    fn identity(
        &mut self,
        id: i64,
        scope: &str,
        identity: &str,
        offset: u64,
    ) -> Result<(), StorageError> {
        let tx = self.tx;
        let result = (|| {
            let offset = i64::try_from(offset).map_err(|_| rusqlite::Error::InvalidQuery)?;
            tx.prepare_cached("INSERT INTO native_usage_identities VALUES(semon_namespace(),?1,?2,?3,?4)")?
            .execute(params![id, scope, identity, offset])?;
            Ok(())
        })();
        self.result(result)
    }
    fn insert(&mut self, fact: algorithm::Fact<'_>) -> Result<(), StorageError> {
        let tx = self.tx;
        let result = (|| {
            let offset = i64::try_from(fact.offset).map_err(|_| rusqlite::Error::InvalidQuery)?;
            tx.prepare_cached("INSERT INTO native_usage_facts VALUES(semon_namespace(),?1,?2,?3,?4,?5,?6,?7)")?
            .execute(params![
                fact.source,
                offset,
                fact.identity,
                fact.payload,
                fact.record,
                fact.tree,
                fact.model
            ])?;
            Ok(())
        })();
        self.result(result)
    }
    fn dimension(
        &mut self,
        id: i64,
        kind: &str,
        key: &str,
        value: &str,
    ) -> Result<(), StorageError> {
        let tx = self.tx;
        let result = (|| {
            tx.prepare_cached("INSERT INTO native_usage_dimensions VALUES(semon_namespace(),?1,?2,?3,?4) ON CONFLICT(namespace,source_id,kind,dimension) DO UPDATE SET value=excluded.value")?.execute(params![id,kind,key,value])?;
            Ok(())
        })();
        self.result(result)
    }
    fn ready(&mut self, id: i64, checkpoint: u64) -> Result<(), StorageError> {
        let tx = self.tx;
        let result = (|| {
            let checkpoint =
                i64::try_from(checkpoint).map_err(|_| rusqlite::Error::InvalidQuery)?;
            tx.execute("UPDATE native_usage_sources SET usage_ready=1,checkpoint=?2 WHERE namespace=semon_namespace() AND source_id=?1",params![id,checkpoint])?;
            Ok(())
        })();
        self.result(result)
    }
}

pub(super) fn write(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
) -> rusqlite::Result<()> {
    with_effects(tx, |effects| algorithm::write(effects, path, ledger, index))
}

pub(super) fn begin_source(
    tx: &Transaction<'_>,
    path: &str,
    expected: Option<&Ledger>,
    ledger: &Ledger,
    append: bool,
) -> rusqlite::Result<i64> {
    with_effects(tx, |effects| {
        algorithm::begin(effects, path, expected, ledger, append)
    })
}

#[cfg(test)]
fn put_dimension<T: Serialize>(
    tx: &Transaction<'_>,
    id: i64,
    kind: &str,
    key: &str,
    value: &T,
) -> rusqlite::Result<()> {
    use algorithm::Effects;
    let value = json(value)?;
    with_effects(tx, |effects| effects.dimension(id, kind, key, &value))
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
        "UPDATE native_usage_current SET source_path=?2 WHERE namespace=semon_namespace() AND source_path=?1",
        params![old, path],
    )?;
    tx.execute(
        "UPDATE native_usage_sources SET receipt=?2 WHERE namespace=semon_namespace() AND source_id=?1",
        params![id, header_ledger(relocated)],
    )?;
    Ok(normalized_usage)
}

#[cfg(test)]
mod namespace_tests {
    use super::*;
    use rusqlite::functions::FunctionFlags;

    #[test]
    fn private_usage_same_ids_paths_observations_and_rollback_are_scoped() {
        // Exercise usage and repair queue isolation. Tree/pricing and retained
        // readers still prevent public multi-source qualification.
        let mut db = Connection::open_in_memory().unwrap();
        for ddl in schema() {
            db.execute_batch(ddl).unwrap();
        }
        db.pragma_update(None, "foreign_keys", true).unwrap();
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
        let state = |db: &Connection, namespace: &str| {
            let mut state = Vec::<Vec<rusqlite::types::Value>>::new();
            for table in [
                "native_usage_sources",
                "native_usage_current",
                "native_usage_facts",
                "native_usage_identities",
                "native_usage_dimensions",
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
        };
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: 64,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: 64,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let path = "same/relative.jsonl";
        for namespace in ["a", "b"] {
            bind(&db, namespace);
            let tx = db.transaction().unwrap();
            assert_eq!(begin_source(&tx, path, None, &ledger, false).unwrap(), 1);
            let index = FileIndex {
                native_usage_observations: Some(vec![Observation {
                    identity: "same-identity".into(),
                    payload: Payload {
                        model: Some(namespace.into()),
                        timestamp: None,
                        tokens: Tokens::default(),
                        model_tokens: ModelTokens::default(),
                    },
                    record: Record::capture(b"sanitized observation", 0),
                }]),
                ..FileIndex::default()
            };
            write(&tx, path, &ledger, &index).unwrap();
            tx.execute("INSERT INTO native_tree_repairs(namespace,source_path,receipt) VALUES(semon_namespace(),?1,?2)",
                params![path,header_ledger(&ledger)]).unwrap();
            put_dimension(&tx, 1, "model", "same-model", &namespace).unwrap();
            // The original observation at the same offset reconciles exactly.
            write(&tx, path, &ledger, &index).unwrap();
            tx.commit().unwrap();
            assert!(normalized(&db, path, &ledger).unwrap());
            let seen = seen(&db, path, &ledger, "same-identity").unwrap();
            assert!(seen.tree && seen.model);
        }
        let b = state(&db, "b");
        assert!(repair_pending(&db).unwrap());
        bind(&db, "a");
        db.execute("UPDATE native_tree_repairs SET state='done' WHERE namespace=semon_namespace() AND source_path=?1", [path]).unwrap();
        assert!(!repair_pending(&db).unwrap());
        bind(&db, "b");
        assert!(repair_pending(&db).unwrap());
        bind(&db, "a");
        let a = state(&db, "a");
        {
            let tx = db.transaction().unwrap();
            put_dimension(&tx, 1, "model", "same-model", &"changed").unwrap();
            assert!(relocate(&tx, path, "new-a", &ledger, &ledger).unwrap());
            assert!(source(&tx, path).unwrap().is_none());
            assert_eq!(source(&tx, "new-a").unwrap().unwrap().0, 1);
            assert_eq!(state(&tx, "b"), b);
        }
        assert_eq!(state(&db, "a"), a);
        assert_eq!(state(&db, "b"), b);
        let tx = db.transaction().unwrap();
        // A new incarnation keeps the prior non-rebuildable observations.
        assert_eq!(begin_source(&tx, path, None, &ledger, false).unwrap(), 2);
        tx.commit().unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM native_usage_facts WHERE namespace='a' AND source_id=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(state(&db, "b"), b);
        bind(&db, "b");
        assert_eq!(source(&db, path).unwrap().unwrap().0, 1);
    }
}

#[cfg(test)]
mod file_namespace_tests {
    use super::*;
    use std::path::PathBuf;

    // Exercise the production bound-selector API, including after reopening.
    fn bind(db: &Connection, namespace: &str) {
        let namespace = namespace.to_owned();
        db.create_scalar_function(
            "semon_namespace",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| Ok(namespace.clone()),
        )
        .unwrap();
    }
    fn begin_source(
        tx: &Transaction<'_>,
        namespace: &str,
        path: &str,
        expected: Option<&Ledger>,
        ledger: &Ledger,
        append: bool,
    ) -> rusqlite::Result<i64> {
        bind(tx, namespace);
        super::begin_source(tx, path, expected, ledger, append)
    }
    fn write(
        tx: &Transaction<'_>,
        namespace: &str,
        path: &str,
        ledger: &Ledger,
        index: &FileIndex,
    ) -> rusqlite::Result<()> {
        bind(tx, namespace);
        super::write(tx, path, ledger, index)
    }
    fn normalized(
        db: &Connection,
        namespace: &str,
        path: &str,
        ledger: &Ledger,
    ) -> rusqlite::Result<bool> {
        bind(db, namespace);
        super::normalized(db, path, ledger)
    }
    fn seen(
        db: &Connection,
        namespace: &str,
        path: &str,
        ledger: &Ledger,
        identity: &str,
    ) -> rusqlite::Result<Seen> {
        bind(db, namespace);
        super::seen(db, path, ledger, identity)
    }
    fn restore(
        db: &Connection,
        namespace: &str,
        path: &str,
        ledger: &Ledger,
        encoded: Option<String>,
    ) -> rusqlite::Result<Option<CodexNativeUsage>> {
        bind(db, namespace);
        super::restore(db, path, ledger, encoded)
    }

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
            index.native_usage_observations.as_mut().unwrap().push(
                Observation::capture(&record, &bytes, *offset, Some("model-shared")).unwrap(),
            );
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
        for ddl in schema() {
            db.execute_batch(ddl).unwrap();
        }
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
            db.query_row(
                "SELECT count(*) FROM native_usage_facts WHERE namespace=?1",
                [namespace_a],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM native_usage_facts WHERE namespace=?1",
                [namespace_b],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
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
        assert_eq!(restored.by_model.get("model-shared").unwrap().input, 20);
        let second_identity = appended.native_usage_observations.as_ref().unwrap()[1]
            .identity()
            .to_owned();
        let second_a = seen(
            &db,
            namespace_a,
            source_path,
            &ledger_appended,
            &second_identity,
        )
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
            db.query_row(
                "SELECT count(*) FROM native_usage_facts WHERE namespace=?1",
                [namespace_a],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
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
        assert_eq!(reopened.by_model.get("model-shared").unwrap().input, 20);
        assert!(normalized(&db, namespace_b, source_path, &ledger_b).unwrap());
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
