//! Namespace binding and current Catalog reader/writer fences.
use super::failed;
use crate::storage::{LogicalSourceNamespace, StorageError};
use rusqlite::{Connection, functions::FunctionFlags};

pub(crate) const QUALIFIED_CATALOG_TABLES: &[&str] = &[
    "session_catalog",
    "session_catalog_sources",
    "session_slot_projections",
    "session_slots",
    "session_history_catalog",
    "session_history_projections",
    "session_history_slots",
    "session_history_slot_parts",
    "session_trace_headers",
    "session_trace_turns",
    "session_trace_edges",
    "session_trace_bookmarks",
    "session_overview",
    "session_descriptions",
    "session_search_coverage",
    "session_search_documents",
    "session_search_memberships",
    "session_search_counts",
    "session_catalog_changes",
    "session_catalog_invalidations",
    "catalog_backfill_sources",
];

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(crate) fn register(
    db: &Connection,
    source: &LogicalSourceNamespace,
) -> Result<(), StorageError> {
    crate::events::store::configured::bind_metadata_scope(db, source.as_str()).map_err(failed)?;
    let source = source.as_str().to_owned();
    db.create_scalar_function(
        "semon_namespace",
        0,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        move |_| Ok(source.clone()),
    )
    .map_err(failed)
}

/// Every unqualified native SELECT resolves a connection-local view with an
/// explicit namespace predicate. Indexed Catalog ranges use main plus the same
/// predicate, retaining namespace-leading covering indexes.
pub(crate) fn scope_readers(db: &Connection) -> Result<bool, StorageError> {
    for table in QUALIFIED_CATALOG_TABLES {
        let current: bool = if *table == "session_catalog_changes" {
            // Revisions order the shared stream globally; namespace owns each
            // row and leads the delivery index, rather than the revision PK.
            db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND type='TEXT' AND [notnull]=1) AND EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='revision' AND type='INTEGER' AND pk=1) AND EXISTS(SELECT 1 FROM pragma_index_info('session_catalog_change_order','main') WHERE seqno=0 AND name='namespace')", [table], |row|row.get(0)).map_err(failed)?
        } else {
            db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [table], |row| row.get(0)).map_err(failed)?
        };
        if !current {
            return Err(StorageError::NamespaceUpgradePending);
        }
        // Preserve the native positional row layout. Namespace is an adapter
        // qualifier, not a new native payload column.
        let mut statement = db
            .prepare(&format!("PRAGMA main.table_info({})", quote(table)))
            .map_err(failed)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?;
        let columns = names
            .iter()
            .filter(|name| name.as_str() != "namespace")
            .map(|name| quote(name))
            .collect::<Vec<_>>()
            .join(",");
        if columns.is_empty() {
            return Err(StorageError::NamespaceUpgradePending);
        }
        db.execute_batch(&format!("CREATE TEMP VIEW IF NOT EXISTS {} AS SELECT {columns} FROM main.{} WHERE namespace=semon_namespace();", quote(table), quote(table))).map_err(failed)?;
    }
    db.execute_batch("CREATE TEMP VIEW IF NOT EXISTS meta AS SELECT namespace,key,value FROM main.meta WHERE namespace=semon_meta_scope(key);").map_err(failed)?;
    Ok(true)
}

pub(super) fn guard(db: &Connection, table: &str) -> Result<(), StorageError> {
    for (operation, predicate) in [
        ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
        (
            "UPDATE",
            "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
        ),
        ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
    ] {
        db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {} BEFORE {operation} ON {} WHEN {predicate} BEGIN SELECT RAISE(ABORT,'namespace writer refused'); END;", quote(&format!("storage_catalog_guard_{table}_{operation}")), quote(table))).map_err(failed)?;
    }
    Ok(())
}

#[cfg(test)]
mod current_navigation_tests {
    use super::*;
    use crate::storage::{StoreBackend, StoreConfig};
    struct Fixture {
        db: Connection,
        config: StoreConfig,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "semon-current-navigation-{}-{}.sqlite3",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let ns = LogicalSourceNamespace::new("A").unwrap();
            let db = crate::events::store::current_search_test_connection(&path, &ns);
            let epoch = db.query_row("SELECT value FROM main.meta WHERE namespace='' AND key='catalog_changes_epoch'", [], |r| r.get(0)).unwrap();
            for key in ["same", "tail", "third"] {
                db.execute("INSERT INTO main.session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES('A',?1,10,'codex','{\"owner\":\"A\"}')", [key]).unwrap();
            }
            Self {
                db,
                config: StoreConfig {
                    backend: StoreBackend::Sqlite { database: path },
                    namespace: ns,
                    expected_stream_epoch: Some(epoch),
                },
            }
        }
        fn db(&self) -> &Connection {
            &self.db
        }
    }
    #[test]
    fn current_owner_navigation_pins_epoch_namespace_and_refuses_without_mutation() {
        use super::super::navigation::SqliteNavigation;
        use crate::events::store::configured::NativeOwnership;
        use crate::storage::{NativeReaderBinding, navigation::NativeNavigationStore};
        let f = Fixture::new();
        let db = f.db();
        for (key, edge) in [("d", "1"), ("b", "1"), ("a", "1"), ("c", "1"), ("a", "2")] {
            db.execute("INSERT INTO main.session_trace_bookmarks(namespace,target,session_key,edge_id) VALUES('A','target',?1,?2)", (key, edge)).unwrap();
        }
        db.execute("INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','same','current','codex','native-same','relative/same.jsonl')", []).unwrap();
        let owner = NativeOwnership::restore(&f.config).unwrap();
        let binding = NativeReaderBinding::Semantic(owner);
        let store = SqliteNavigation(binding.clone());
        let generation = store.catalog_generation().unwrap();
        let StoreBackend::Sqlite { database } = &f.config.backend;
        let wal = database.with_extension("sqlite3-wal");
        let state = || {
            [database.as_path(), wal.as_path()].map(|path| {
                (
                    std::fs::read(path).ok(),
                    std::fs::metadata(path).ok().and_then(|m| m.modified().ok()),
                )
            })
        };
        let before = state();
        let changes = db.total_changes();
        assert_eq!(
            store.bookmarked_target_keys("target").unwrap(),
            ["a", "b", "c"]
        );
        assert!(store.bookmarked_target_keys("missing").unwrap().is_empty());
        assert_eq!(
            store.bookmarked_target_keys(&"x".repeat(4097)),
            Err(StorageError::InvalidArguments)
        );
        let observation = store
            .native_resolution(
                "codex",
                "native-same",
                crate::CatalogReadScope::Current,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(observation.keys, ["same"]);
        assert_eq!(observation.metadata.as_deref(), Some("{\"owner\":\"A\"}"));
        assert_eq!(observation.generation.as_deref(), Some(generation.as_str()));
        assert!(matches!(
            store.native_resolution(
                "codex",
                "native-same",
                crate::CatalogReadScope::Current,
                Some(&"b".repeat(64))
            ),
            Err(StorageError::ScopeChanged)
        ));
        assert!(matches!(
            binding.validate(db, &LogicalSourceNamespace::new("foreign").unwrap()),
            Err(StorageError::NamespaceMismatch)
        ));
        let mut stale = f.config.clone();
        stale.expected_stream_epoch = Some("f".repeat(64));
        assert!(matches!(
            NativeOwnership::restore(&stale),
            Err(StorageError::ScopeChanged)
        ));
        let mut foreign = f.config.clone();
        foreign.namespace = LogicalSourceNamespace::new("foreign").unwrap();
        assert!(NativeOwnership::restore(&foreign).is_err());
        assert_eq!(db.total_changes(), changes);
        assert_eq!(state(), before);
    }

    #[test]
    fn qualified_navigation_uses_retained_metadata_fallback_bounds_and_indexes() {
        use super::super::navigation::SqliteNavigation;
        use crate::storage::{NativeReaderBinding, navigation::NativeNavigationStore};
        let f = Fixture::new();
        let generation = "a".repeat(64);
        let db = f.db();
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_version'),'catalog_version',?1)",
            [crate::model::summary::CATALOG_VERSION.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES(semon_meta_scope('catalog_generation'),'catalog_generation',?1)",
            [&generation],
        )
        .unwrap();

        let db = f.db();
        register(&db, &f.config.namespace).unwrap();
        db.execute(
            "INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','same','retained','codex','retained-history','retained/same')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES('A','same','{\"owner\":\"history\"}')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','tail','retained','codex','retained-fallback','retained/tail')",
            [],
        )
        .unwrap();
        let store = SqliteNavigation(NativeReaderBinding::Semantic(
            crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap(),
        ));
        let retained = store
            .native_resolution(
                "codex",
                "retained-history",
                crate::CatalogReadScope::RetainedHistory,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(retained.keys, ["same"]);
        assert_eq!(
            retained.metadata.as_deref(),
            Some("{\"owner\":\"history\"}")
        );
        let fallback = store
            .native_resolution(
                "codex",
                "retained-fallback",
                crate::CatalogReadScope::RetainedHistory,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(fallback.keys, ["tail"]);
        assert_eq!(fallback.metadata.as_deref(), Some("{\"owner\":\"A\"}"));

        let oversized = format!("\"{}\"", "x".repeat(1_048_577));
        db.execute(
            "UPDATE main.session_catalog SET metadata=?1 WHERE namespace='A' AND session_key='tail'",
            [&oversized],
        )
        .unwrap();
        db.execute(
            "INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','tail','current','codex','bounded-metadata','current/tail')",
            [],
        )
        .unwrap();
        let bounded = store
            .native_resolution(
                "codex",
                "bounded-metadata",
                crate::CatalogReadScope::Current,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(bounded.keys, ["tail"]);
        assert_eq!(bounded.metadata, None);

        for (key, path) in [
            ("same", "ambiguous/same"),
            ("tail", "ambiguous/tail"),
            ("third", "ambiguous/third"),
        ] {
            db.execute(
                "INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A',?1,'current','codex','ambiguous',?2)",
                (key, path),
            )
            .unwrap();
        }
        let ambiguous = store
            .native_resolution(
                "codex",
                "ambiguous",
                crate::CatalogReadScope::Current,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(ambiguous.keys, ["same", "tail"]);

        for (index, sql) in [
            "EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM main.session_catalog_sources INDEXED BY session_catalog_current_native WHERE namespace=semon_namespace() AND lifecycle='current' AND harness='codex' AND native_id='ambiguous' ORDER BY session_key LIMIT 2",
            "EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM main.session_catalog_sources INDEXED BY session_catalog_native WHERE namespace=semon_namespace() AND harness='codex' AND native_id='retained-history' ORDER BY session_key LIMIT 2",
        ]
        .iter()
        .enumerate()
        {
            let plan: String = db.query_row(sql, [], |row| row.get(3)).unwrap();
            let expected = [
                "COVERING INDEX session_catalog_current_native",
                "COVERING INDEX session_catalog_native",
            ][index];
            assert!(plan.contains(expected), "{plan}");
        }
    }
}
