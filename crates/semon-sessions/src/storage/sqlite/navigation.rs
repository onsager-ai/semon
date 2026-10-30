//! SQLite snapshots for native aliases and metadata-only bookmark targets.
use super::failed;
use crate::{
    CatalogReadScope,
    storage::{
        NativeReaderBinding, StorageError,
        navigation::{NativeNavigationStore, NativeResolutionObservation},
    },
};
use rusqlite::{Connection, OptionalExtension};

const CURRENT_NATIVE_SQL: &str = "SELECT DISTINCT session_key FROM main.session_catalog_sources INDEXED BY session_catalog_current_native WHERE namespace=semon_namespace() AND lifecycle='current' AND harness=?1 AND native_id=?2 ORDER BY session_key LIMIT 2";

pub(crate) struct SqliteNavigation(pub(crate) NativeReaderBinding);
impl NativeNavigationStore for SqliteNavigation {
    fn catalog_generation(&self) -> Result<String, StorageError> {
        let db = self.0.snapshot(&self.0.config().namespace)?;
        db.query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |row| row.get(0),
        )
        .map_err(failed)
    }

    fn bookmarked_target_keys(&self, target: &str) -> Result<Vec<String>, StorageError> {
        if target.len() > 4096 {
            return Err(StorageError::InvalidArguments);
        }
        let db = self.0.snapshot(&self.0.config().namespace)?;
        bookmark_keys(&db, target)
    }
    fn native_resolution(
        &self,
        harness: &str,
        native_id: &str,
        scope: CatalogReadScope,
        expected_generation: Option<&str>,
    ) -> Result<NativeResolutionObservation, StorageError> {
        let db = self.0.snapshot(&self.0.config().namespace)?;
        let observation = read_observation(&db, harness, native_id, scope, expected_generation)?;
        if matches!(self.0, NativeReaderBinding::Admitted(_))
            && observation.qualified_head().is_none()
        {
            return Err(StorageError::Unavailable);
        }
        Ok(observation)
    }
}
fn bookmark_keys(db: &Connection, target: &str) -> Result<Vec<String>, StorageError> {
    let sql = "SELECT DISTINCT session_key FROM main.session_trace_bookmarks WHERE namespace=semon_namespace() AND target=?1 ORDER BY session_key LIMIT 3";
    db.prepare(sql)
        .map_err(failed)?
        .query_map([target], |row| row.get(0))
        .map_err(failed)?
        .collect::<rusqlite::Result<_>>()
        .map_err(failed)
}
fn read_observation(
    db: &Connection,
    harness: &str,
    native: &str,
    scope: CatalogReadScope,
    expected: Option<&str>,
) -> Result<NativeResolutionObservation, StorageError> {
    let meta = |key: &str| {
        db.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(failed)
    };
    let mut observation = NativeResolutionObservation {
        version: meta("catalog_version")?,
        generation: meta("catalog_generation")?,
        completeness: meta("catalog_completeness")?,
        ..Default::default()
    };
    if expected.is_some_and(|expected| observation.generation.as_deref() != Some(expected)) {
        return Err(StorageError::ScopeChanged);
    }
    if observation.qualified_head().is_none() {
        return Ok(observation);
    }
    let sql = CURRENT_NATIVE_SQL;
    let lookup = |sql: &str| {
        db.prepare(sql)
            .map_err(failed)?
            .query_map((harness, native), |row| row.get(0))
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<String>>>()
            .map_err(failed)
    };
    observation.keys = lookup(sql)?;
    let current = !observation.keys.is_empty();
    if !current && scope == CatalogReadScope::RetainedHistory {
        observation.keys = lookup(
            "SELECT DISTINCT session_key FROM main.session_catalog_sources INDEXED BY session_catalog_native WHERE namespace=semon_namespace() AND harness=?1 AND native_id=?2 ORDER BY session_key LIMIT 2",
        )?;
    }
    if let [key] = observation.keys.as_slice() {
        let metadata = |table: &str| -> Result<Option<Option<String>>, StorageError> {
            db.query_row(&format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1"), [key], |row| row.get(0)).optional().map_err(failed)
        };
        observation.metadata = if current {
            metadata("session_catalog")?
        } else {
            match metadata("session_history_catalog")? {
                None => metadata("session_catalog")?,
                present => present,
            }
        }
        .flatten();
    }
    Ok(observation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::summary::CATALOG_VERSION;
    use crate::native_resolution::{CatalogNativeResolution, CatalogNativeResolutionState};

    fn resolve(
        db: &mut Connection,
        result: &mut CatalogNativeResolution,
        options: Option<&crate::Options>,
    ) -> Result<(), StorageError> {
        let tx = db.transaction().map_err(failed)?;
        let observation = read_observation(
            &tx,
            &result.harness,
            &result.native_id,
            result.read_scope,
            None,
        )?;
        crate::native_resolution::apply_observation(result, options, &observation)
            .map_err(|_| StorageError::Unavailable)?;
        tx.commit().map_err(failed)
    }
    fn fixture() -> Connection {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "semon-navigation-current-{}-{}.sqlite3",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        crate::events::store::current_search_test_connection(
            &path,
            &crate::storage::LogicalSourceNamespace::new("navigation-current").unwrap(),
        )
    }
    use serde_json::json;
    #[test]
    fn generation_refusal_precedes_binding_sql_and_does_not_mutate() {
        let db = fixture();
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_version'),'catalog_version',?1)",
            [CATALOG_VERSION.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_generation'),'catalog_generation',?1)",
            ["a".repeat(64)],
        )
        .unwrap();
        let before = db.total_changes();
        assert!(matches!(
            read_observation(
                &db,
                "codex",
                "native",
                CatalogReadScope::Current,
                Some(&"b".repeat(64))
            ),
            Err(StorageError::ScopeChanged)
        ));
        assert_eq!(db.total_changes(), before);
    }

    #[test]
    fn bookmarked_targets_are_distinct_ordered_and_bounded_for_current_store() {
        let db = fixture();
        for (key, edge) in [("d", "1"), ("b", "1"), ("a", "1"), ("c", "1"), ("a", "2")] {
            db.execute(
                "INSERT INTO main.session_trace_bookmarks VALUES(semon_namespace(),'target',?1,?2)",
                (key, edge),
            )
            .unwrap();
        }
        assert_eq!(bookmark_keys(&db, "target").unwrap(), ["a", "b", "c"]);
        assert!(bookmark_keys(&db, "missing").unwrap().is_empty());
    }

    #[test]
    fn native_alias_is_exact_bounded_and_preserves_ambiguity_and_provisional_reads() {
        let mut db = fixture();
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_version'),'catalog_version',?1)",
            [CATALOG_VERSION.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_generation'),'catalog_generation',?1)",
            ["a".repeat(64)],
        )
        .unwrap();
        let metadata=json!({"key":"canonical","name":"example","harness":"codex","kind":"session","native_ids":["native"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"","effort":null,"start":null,"last":null,"tokens":[0,0,0],"cost":{"usd":null,"unpriced_models":[],"split_unknown_messages":0,"by_model":{},"by_day":{}},"sources":[{"path":"missing.jsonl","native_id":"native","dev":1,"ino":1,"size":0,"modified_ns":0,"offset":0,"prefix_sha256":vec![0;32],"tail_sha256":vec![0;32]}]}).to_string();
        db.execute(
            "INSERT INTO main.session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(semon_namespace(),'canonical',0,'codex',?1)",
            [&metadata],
        )
        .unwrap();
        db.execute(
            "INSERT INTO main.session_history_catalog VALUES(semon_namespace(),'canonical',?1)",
            [&metadata],
        )
        .unwrap();
        db.execute(
            "INSERT INTO main.session_catalog_sources(namespace,session_key,harness,native_id,lifecycle,source_path) VALUES(semon_namespace(),'canonical','codex','native','current','fixture/canonical')",
            [],
        )
        .unwrap();
        for key in ["old-owner", "other"] {
            db.execute("INSERT INTO main.session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(semon_namespace(),?1,0,'codex','{}')", [key]).unwrap();
        }
        let mut r = CatalogNativeResolution {
            state: CatalogNativeResolutionState::Unavailable,
            source_key: "exact-source".into(),
            harness: "codex".into(),
            native_id: "native".into(),
            read_scope: CatalogReadScope::Current,
            catalog_key: None,
            generation: None,
        };
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Resolved);
        assert_eq!(r.catalog_key.as_deref(), Some("canonical"));
        db.execute("DELETE FROM main.session_history_catalog", [])
            .unwrap();
        r.read_scope = CatalogReadScope::RetainedHistory;
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Resolved);
        assert_eq!(r.catalog_key.as_deref(), Some("canonical"));
        db.execute(
            "INSERT INTO main.session_catalog_sources(namespace,session_key,harness,native_id,lifecycle,source_path) VALUES(semon_namespace(),'old-owner','codex','native','retained','fixture/old-owner')",
            [],
        )
        .unwrap();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Resolved);
        db.execute(
            "DELETE FROM main.session_catalog_sources WHERE session_key='old-owner'",
            [],
        )
        .unwrap();
        r.read_scope = CatalogReadScope::Current;
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_completeness'),'catalog_completeness','partial')",
            [],
        )
        .unwrap();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Provisional);
        db.execute(
            "INSERT INTO main.session_catalog_sources(namespace,session_key,harness,native_id,lifecycle,source_path) VALUES(semon_namespace(),'other','codex','native','current','fixture/other')",
            [],
        )
        .unwrap();
        r.catalog_key = None;
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Ambiguous);
        assert_eq!(r.catalog_key, None);
        r.harness = "claude".into();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Pending);
        db.execute(
            "DELETE FROM main.session_catalog_sources WHERE session_key='other'",
            [],
        )
        .unwrap();
        db.execute(
            "UPDATE main.session_catalog_sources SET lifecycle='retained'",
            [],
        )
        .unwrap();
        r.harness = "codex".into();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Pending);
        r.read_scope = CatalogReadScope::RetainedHistory;
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Provisional);
        let plan: String = db
            .query_row(
                &format!("EXPLAIN QUERY PLAN {CURRENT_NATIVE_SQL}"),
                ("codex", "native"),
                |row| row.get(3),
            )
            .unwrap();
        assert!(plan.contains("COVERING INDEX"), "{plan}");
    }
}
