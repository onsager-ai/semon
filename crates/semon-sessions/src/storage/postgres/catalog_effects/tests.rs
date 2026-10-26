use super::*;
use crate::{
    Options,
    storage::{
        postgres::{
            PgCancellation,
            tests::{fixture, ns},
            txn::TxnKind,
        },
        sqlite::publication::{CurrentWriter, SlotWriter},
    },
};
use std::{fs, time::Duration};

const L1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const L2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const TABLES: &[&str] = &[
    "catalog_rows",
    "catalog_sources",
    "catalog_dependencies",
    "catalog_recipe_headers",
    "catalog_recipe_rows",
    "catalog_trace_headers",
    "catalog_trace_turns",
    "catalog_trace_edges",
    "catalog_trace_bookmarks",
];

struct Input {
    rows: Vec<CatalogRow>,
    projections: Vec<crate::slot_projection::Projection>,
    metadata: Vec<Vec<u8>>,
    slots: Vec<Vec<u8>>,
    trace: Vec<Vec<Vec<u8>>>,
}
impl Input {
    fn publication(&self) -> crate::slot_projection::Publication<'_> {
        crate::slot_projection::Publication {
            rows: &self.rows,
            transcripts: Some(&self.projections),
        }
    }
}

// Ordinary native parser/model FIRST, followed by the actual SQLite physical
// producers. Close and remove the fixture before any PG work starts. This is
// scoped producer parity, not complete backend no-SQLite instrumentation.
fn oracle() -> Input {
    let root = std::env::temp_dir().join(format!(
        "pg124-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let options = Options {
        claude_home: root.join("claude"),
        claude_json: root.join("claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join("index.json"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    fs::create_dir_all(root.join("proc")).unwrap();
    fs::write(root.join("proc/locks"), "").unwrap();
    let source = options.claude_home.join("projects/project/one.jsonl");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let lines = [
        serde_json::json!({"type":"user","sessionId":"one","uuid":"u1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized fixture"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"a1","parentUuid":"u1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:01Z","message":{"id":"m1","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized answer"}]}}),
    ];
    fs::write(
        source,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut cache = crate::events::EventCache::open_scoped(&options.cache).unwrap();
    let prepared =
        crate::model::prepare_sources(&options, &mut cache, &inputs, 1790812802000).unwrap();
    assert!(!prepared.rows.is_empty());
    assert!(!prepared.transcripts.is_empty());
    drop(cache);
    let db = rusqlite::Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let writer = CurrentWriter::unbound(&db);
    for row in &prepared.rows {
        writer
            .current(row, &serde_json::to_string(row).unwrap())
            .unwrap();
        catalog::publish_current_sources(&writer, row).unwrap();
        if let Some(trace) = &row.trace {
            crate::causal_trace::publish_with_store(
                &crate::storage::sqlite::trace::TraceWriter::unbound(&db),
                row,
                trace,
            )
            .unwrap();
        }
    }
    catalog::publish_slot_rows(
        &SlotWriter::unbound(&db),
        &prepared.rows,
        &prepared.transcripts,
    )
    .unwrap();
    db.execute_batch("COMMIT").unwrap();
    let read = |sql: &str| {
        db.prepare(sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap().into_bytes())
            .collect::<Vec<_>>()
    };
    let metadata = read("SELECT metadata FROM session_catalog ORDER BY session_key");
    let slots = read("SELECT metadata FROM session_slots ORDER BY session_key,slot");
    let trace = [
        "SELECT metadata FROM session_trace_headers ORDER BY session_key",
        "SELECT metadata FROM session_trace_turns ORDER BY session_key,turn_id",
        "SELECT metadata FROM session_trace_edges ORDER BY session_key,edge_id",
    ]
    .iter()
    .map(|sql| read(sql))
    .collect();
    assert!(!slots.is_empty());
    drop(db);
    fs::remove_dir_all(root).unwrap();
    Input {
        rows: prepared.rows,
        projections: prepared.transcripts,
        metadata,
        slots,
        trace,
    }
}

fn snapshot(tx: &mut PgTxn<'_>) -> Result<Vec<Vec<String>>, StorageError> {
    TABLES.iter().map(|table| {
        tx.driver.query(&format!("SELECT to_jsonb(t)::text FROM semon_pg_native_{table} t ORDER BY to_jsonb(t)::text"), &[]).map_err(sqlstate::error).map(|rows| rows.iter().map(|row| row.get(0)).collect())
    }).collect()
}

#[test]
fn pg_catalog124_real_sqlite_producer_recipe_parity_and_immutable_generations() {
    let data = oracle();
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("catalog124\0🦀");
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| stage(tx, L1, &data.publication()),
    )
    .unwrap();
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &PgCancellation::default(), |tx| {
        let rows = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_rows ORDER BY session_key", &[]).map_err(sqlstate::error)?;
        assert_eq!(rows.iter().map(|r| r.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), data.metadata);
        let slots = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_recipe_rows ORDER BY session_key,slot", &[]).map_err(sqlstate::error)?;
        assert_eq!(slots.iter().map(|r| r.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), data.slots);
        for (index, (table, order)) in [
            ("catalog_trace_headers", "session_key"),
            ("catalog_trace_turns", "session_key,turn_id"),
            ("catalog_trace_edges", "session_key,edge_id"),
        ].iter().enumerate() {
            let rows = tx.driver.query(&format!("SELECT metadata FROM semon_pg_native_{table} ORDER BY {order}"), &[]).map_err(sqlstate::error)?;
            assert_eq!(rows.iter().map(|row| row.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), data.trace[index]);
        }
        Ok(())
    }).unwrap();
    let before = pool
        .transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot,
        )
        .unwrap();
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| stage(tx, L1, &data.publication()),
    )
    .unwrap();
    assert_eq!(
        pool.transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot
        )
        .unwrap(),
        before
    );
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| stage(tx, L2, &data.publication()),
    )
    .unwrap();
    let reopened =
        crate::storage::postgres::PgPool::open(factory, crate::storage::postgres::tests::limits())
            .unwrap();
    reopened
        .transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| {
                for table in TABLES {
                    let count: i64 = tx
                        .driver
                        .query_one(
                            &format!(
                                "SELECT count(DISTINCT generation) FROM semon_pg_native_{table}"
                            ),
                            &[],
                        )
                        .map_err(sqlstate::error)?
                        .get(0);
                    if matches!(
                        *table,
                        "catalog_rows"
                            | "catalog_sources"
                            | "catalog_recipe_headers"
                            | "catalog_recipe_rows"
                    ) {
                        assert_eq!(count, 2);
                    }
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn pg_catalog124_all_measured_boundaries_rollback_and_changed_generation_refuses() {
    let mut data = oracle();
    let (pool, _) = fixture(|_| {});
    let namespace = ns("catalog124-rollback");
    BOUNDARIES.with(|n| n.set(0));
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| stage(tx, L1, &data.publication()),
    )
    .unwrap();
    let boundaries = BOUNDARIES.with(|n| n.get());
    assert!(boundaries >= 4);
    let before = pool
        .transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot,
        )
        .unwrap();
    for boundary in 1..=boundaries {
        FAIL_AFTER.with(|point| point.set(Some(boundary)));
        assert!(
            pool.transaction(
                &namespace,
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| stage(tx, L2, &data.publication())
            )
            .is_err()
        );
        FAIL_AFTER.with(|point| point.set(None));
        assert_eq!(
            pool.transaction(
                &namespace,
                TxnKind::ReadSnapshot,
                &PgCancellation::default(),
                snapshot
            )
            .unwrap(),
            before
        );
    }
    data.rows[0].name.push_str(" changed");
    assert!(matches!(
        pool.transaction(
            &namespace,
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| stage(tx, L1, &data.publication())
        ),
        Err(StorageError::ScopeChanged)
    ));
    assert_eq!(
        pool.transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot
        )
        .unwrap(),
        before
    );
}

#[test]
fn pg_catalog124_long_opaque_keys_namespaces_and_precancel() {
    let mut data = oracle();
    let old = data.rows[0].key.clone();
    let long = format!("{}\0🦀", "opaque".repeat(2000));
    data.rows[0].key = long.clone();
    for projection in &mut data.projections {
        if projection.key == old {
            projection.key = long.clone();
        }
    }
    let (pool, _) = fixture(|_| {});
    for name in ["a\0中", "b\0中"] {
        pool.transaction(
            &ns(name),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| stage(tx, L1, &data.publication()),
        )
        .unwrap();
    }
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(
        pool.transaction(&ns("c"), TxnKind::Write, &cancelled, |tx| stage(
            tx,
            L1,
            &data.publication()
        ))
        .is_err()
    );
    pool.transaction(
        &ns("c"),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            assert!(snapshot(tx)?.iter().all(Vec::is_empty));
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn pg_catalog124_injected_source_role_rls_and_namespace_reuse() {
    let data = oracle();
    let (pool, factory) = fixture(|_| {});
    let source = crate::storage::postgres::tests::source_factory(&factory);
    let source_pool =
        crate::storage::postgres::PgPool::open(source, crate::storage::postgres::tests::limits())
            .unwrap();
    for name in ["tenant-a\0中", "tenant-b\0中"] {
        source_pool
            .transaction(
                &ns(name),
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| stage(tx, L1, &data.publication()),
            )
            .unwrap();
    }
    let a = source_pool
        .transaction(
            &ns("tenant-a\0中"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot,
        )
        .unwrap();
    let b = source_pool
        .transaction(
            &ns("tenant-b\0中"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot,
        )
        .unwrap();
    assert_eq!(
        a.iter().map(Vec::len).collect::<Vec<_>>(),
        b.iter().map(Vec::len).collect::<Vec<_>>()
    );
    source_pool
        .transaction(
            &ns("tenant-a\0中"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| {
                for table in TABLES {
                    let rows = tx
                        .driver
                        .query(
                            &format!(
                                "SELECT generation FROM semon_pg_native_{table} WHERE namespace=$1"
                            ),
                            &[&"tenant-b\0中".as_bytes()],
                        )
                        .map_err(sqlstate::error)?;
                    assert!(rows.is_empty());
                }
                Ok(())
            },
        )
        .unwrap();
    assert!(
        source_pool
            .transaction(
                &ns("tenant-c"),
                TxnKind::ReadSnapshot,
                &PgCancellation::default(),
                snapshot
            )
            .unwrap()
            .iter()
            .all(Vec::is_empty)
    );
    drop(pool);
}
