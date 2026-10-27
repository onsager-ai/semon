use super::*;
use crate::{
    Options,
    storage::postgres::{
        PgCancellation,
        tests::{fixture, ns},
        txn::TxnKind,
    },
};
use std::{fs, time::Duration};

mod businessflow;

const L1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const L2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

#[test]
fn pg_catalog135_real_native_binding_before_effects_and_atomic_refusal() {
    fn whole_database(tx: &mut PgTxn<'_>) -> Result<Vec<(String, Vec<String>)>, StorageError> {
        let names = tx.driver.query("SELECT relname::text FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relkind='r' AND relname LIKE 'semon_pg_%' ORDER BY relname", &[]).map_err(sqlstate::error)?;
        names
            .iter()
            .map(|row| {
                let name: String = row.get(0);
                let values = tx
                    .driver
                    .query(
                        &format!(
                            "SELECT to_jsonb(t)::text FROM \"{name}\" t ORDER BY to_jsonb(t)::text"
                        ),
                        &[],
                    )
                    .map_err(sqlstate::error)?;
                Ok((name, values.iter().map(|row| row.get(0)).collect()))
            })
            .collect()
    }
    const NAME: &str = "storage::postgres::catalog_effects::tests::pg_catalog135_real_native_binding_before_effects_and_atomic_refusal";
    const CHILD: &str = "SEMON_CATALOG135_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    // Actual parser/model and ordinary SQLite producers finish FIRST. Keep only
    // this test's sanitized Raw fixture so PG ingests the identical observation.
    let data = build_oracle(true, false);
    use crate::storage::native_events::NativeEventStore;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static OPENS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn deny_sqlite(
        _: *mut rusqlite::ffi::sqlite3,
        _: *mut *mut std::ffi::c_char,
        _: *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::ffi::c_int {
        OPENS.fetch_add(1, Ordering::SeqCst);
        rusqlite::ffi::SQLITE_AUTH
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_cancel_auto_extension(Some(deny_sqlite));
            }
            assert_eq!(OPENS.load(Ordering::SeqCst), 0);
        }
    }
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(deny_sqlite)) },
        rusqlite::ffi::SQLITE_OK
    );
    std::thread::spawn(|| assert!(rusqlite::Connection::open_in_memory().is_err()))
        .join()
        .unwrap();
    assert_eq!(OPENS.swap(0, Ordering::SeqCst), 1);
    let _guard = Guard;
    let (pool, _) = fixture(|_| {});
    let namespace = ns("catalog135\0🦀");
    let control = PgCancellation::default();
    let mut pg =
        crate::events::EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone())
            .unwrap();
    for (at, source) in data.rows.iter().flat_map(|row| &row.sources).enumerate() {
        let receipt = pg
            .ingest(&format!("{:064x}", at + 1), &source.path, "claude")
            .unwrap();
        assert_eq!(receipt.consumed_bytes, source.offset);
    }
    drop(pg);
    let paths = data
        .rows
        .iter()
        .flat_map(|row| &row.sources)
        .map(|source| source.path.to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let fence = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            super::admission::capture(tx, &paths)
        })
        .unwrap();
    let before = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
        .unwrap();
    let mut bad = data.rows.clone();
    bad[0].sources[0].prefix_sha256[0] ^= 1;
    let wrong = crate::slot_projection::Publication {
        rows: &bad,
        transcripts: Some(&data.projections),
    };
    assert_eq!(
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            super::admission::stage_bound(tx, L1, &wrong, &fence).map(|_| ())
        })
        .err(),
        Some(StorageError::ScopeChanged)
    );
    assert!(
        before
            == pool
                .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
                .unwrap()
    );
    let refs = pool
        .transaction(&namespace, TxnKind::Write, &control, |tx| {
            super::admission::stage_bound(tx, L1, &data.publication(), &fence)
        })
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].path, data.rows[0].sources[0].path.to_str().unwrap());
    assert!(refs[0].usage_generation > 0);
    assert!(
        !refs[0].usage_ready,
        "Claude has no native Usage observations"
    );
    assert_eq!(refs[0].pages.0.len(), 64);
    super::snapshot::with_generation(&pool, &namespace, &control, L1, |view| {
        assert_eq!(
            serde_json::to_vec(&view.catalog(&data.rows[0].key, false)?.unwrap()).unwrap(),
            data.metadata[0]
        );
        Ok(())
    })
    .unwrap();
    // No head, ACK or readiness is fabricated by proof binding.
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let row = tx.driver.query_one("SELECT catalog_generation IS NULL, EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
        assert!(row.get::<_, bool>(0));
        assert!(row.get::<_, bool>(1));
        Ok(())
    }).unwrap();
    // Every measured Catalog boundary rolls back within this same connected
    // native-proof flow; compare all Catalog table rows, not just row counts.
    let before = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
        .unwrap();
    BOUNDARIES.with(|n| n.set(0));
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::admission::stage_bound(tx, L2, &data.publication(), &fence).map(|_| ())
    })
    .unwrap();
    let count = BOUNDARIES.with(|n| n.get());
    assert!(count > 0);
    // Use another generation: the successful L2 calibration remains immutable.
    let after = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
        .unwrap();
    assert!(before != after);
    let l3 = "3".repeat(64);
    for at in 1..=count {
        FAIL_AFTER.with(|n| n.set(Some(at)));
        assert_eq!(
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                super::admission::stage_bound(tx, &l3, &data.publication(), &fence).map(|_| ())
            })
            .err(),
            Some(StorageError::Unavailable)
        );
        FAIL_AFTER.with(|n| n.set(None));
        assert!(
            after
                == pool
                    .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
                    .unwrap()
        );
    }
    // Missing/current Usage authority and corrupt native header both refuse
    // before effects. Test-only mutations roll back together with the attempt.
    for sql in [
        "UPDATE semon_pg_native_usage_sources SET receipt=convert_to('corrupt','UTF8') WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "UPDATE semon_pg_native_page_generations SET chunks_header=convert_to('corrupt','UTF8') WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "DELETE FROM semon_pg_native_page_current WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
    ] {
        assert_eq!(
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                tx.driver
                    .execute(sql, &[&namespace.as_str().as_bytes()])
                    .map_err(sqlstate::error)?;
                super::admission::stage_bound(tx, &l3, &data.publication(), &fence).map(|_| ())
            })
            .err(),
            Some(StorageError::ScopeChanged)
        );
        assert!(
            after
                == pool
                    .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
                    .unwrap()
        );
    }
    for sql in [
        "UPDATE semon_pg_source_state SET catalog_generation=repeat('4',64) WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "UPDATE semon_pg_source_state SET revision=revision+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "UPDATE semon_pg_source_invalidations SET revision=revision+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
    ] {
        BOUNDARIES.with(|n| n.set(0));
        assert_eq!(
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                tx.driver
                    .execute(sql, &[&namespace.as_str().as_bytes()])
                    .map_err(sqlstate::error)?;
                super::admission::stage_bound(tx, &l3, &data.publication(), &fence).map(|_| ())
            })
            .err(),
            Some(StorageError::Conflict)
        );
        assert_eq!(BOUNDARIES.with(|n| n.get()), 0);
        assert!(
            after
                == pool
                    .transaction(&namespace, TxnKind::ReadSnapshot, &control, whole_database)
                    .unwrap()
        );
    }
    assert_eq!(
        pool.transaction(&ns("foreign135"), TxnKind::Write, &control, |tx| {
            super::admission::stage_bound(tx, L1, &data.publication(), &fence).map(|_| ())
        })
        .err(),
        Some(StorageError::ScopeChanged)
    );
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert_eq!(
        pool.transaction(&namespace, TxnKind::Write, &cancelled, |tx| {
            super::admission::stage_bound(tx, L1, &data.publication(), &fence).map(|_| ())
        })
        .err(),
        Some(StorageError::Busy)
    );
    fs::remove_dir_all(&data.root).unwrap();
}

#[test]
fn pg_catalog131_borrowed_snapshot_real_producer_mvcc_and_source_witness() {
    let data = build_oracle(false, false);
    let (pool, _) = fixture(|b| b.max_connections = 2);
    let namespace = ns("catalog131\0🦀");
    let control = PgCancellation::default();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        stage(tx, L1, &data.publication())
    })
    .unwrap();
    let key = &data.rows[0].key;
    super::snapshot::with_generation(&pool, &namespace, &control, L1, |view| {
        let (row, header, slots) = view.selected(key, false, 0, 64)?.unwrap();
        assert_eq!(serde_json::to_vec(&row).unwrap(), data.metadata[0]);
        assert_eq!(slots.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>(), data.slots);
        assert_eq!(header.3 as usize, slots.len());
        // A second connection commits while the callback holds its actual SQL
        // snapshot. Test-only corruption distinguishes MVCC from fresh reads.
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            stage(tx, L2, &data.publication())?;
            tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET metadata=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&namespace.as_str().as_bytes(), &L1.as_bytes(), &b"corrupt".as_slice()]).map_err(sqlstate::error)?;
            Ok(())
        })?;
        assert_eq!(serde_json::to_vec(&view.catalog(key, false)?.unwrap()).unwrap(), data.metadata[0]);
        assert!(view.catalog(key, true)?.is_none());
        Ok(())
    }).unwrap();
    assert_eq!(
        super::snapshot::with_generation(&pool, &namespace, &control, L1, |view| view
            .catalog(key, false))
        .err(),
        Some(StorageError::ScopeChanged)
    );
    super::snapshot::with_generation(&pool, &namespace, &control, L2, |view| {
        assert!(view.selected(key, false, 0, 1)?.is_some());
        assert_eq!(
            view.recipe(key, false, -1, 1).err(),
            Some(StorageError::InvalidArguments)
        );
        Ok(())
    })
    .unwrap();
    super::snapshot::with_generation(&pool, &ns("foreign\0🦀"), &control, L2, |view| {
        assert!(view.catalog(key, false)?.is_none());
        Ok(())
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        tx.driver.execute("UPDATE semon_pg_native_catalog_recipe_headers SET source_generation=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&namespace.as_str().as_bytes(), &L2.as_bytes(), &L1.as_bytes()]).map_err(sqlstate::error)?;
        Ok(())
    }).unwrap();
    assert_eq!(
        super::snapshot::with_generation(&pool, &namespace, &control, L2, |view| view
            .selected(key, false, 0, 1))
        .err(),
        Some(StorageError::ScopeChanged)
    );
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert_eq!(
        super::snapshot::with_generation(&pool, &namespace, &cancelled, L2, |_| Ok(())).err(),
        Some(StorageError::Busy)
    );
}
const TABLES: &[&str] = &[
    "catalog_search_documents",
    "catalog_search_memberships",
    "catalog_search_counts",
    "catalog_history_owners",
    "catalog_history_headers",
    "catalog_history_slots",
    "catalog_history_sources",
    "catalog_history_progress",
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
    root: std::path::PathBuf,
    rows: Vec<CatalogRow>,
    projections: Vec<crate::slot_projection::Projection>,
    metadata: Vec<Vec<u8>>,
    slots: Vec<Vec<u8>>,
    trace: Vec<Vec<Vec<u8>>>,
    search: Vec<Vec<u8>>,
    original: Option<Vec<Vec<u8>>>,
    original_slots: Option<Vec<Vec<u8>>>,
    composite: Option<Vec<Vec<u8>>>,
    composite_slots: Option<Vec<Vec<u8>>>,
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
    build_oracle(false, true)
}

fn build_oracle(keep_raw: bool, history: bool) -> Input {
    let root = std::env::temp_dir().join(format!(
        "pg124-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let options = Options {
        source: crate::storage::HostSource::new(ns("catalog-effects-oracle"), "c".repeat(64))
            .unwrap(),
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
        &source,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut cache =
        crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let prepared =
        crate::model::prepare_sources(&options, &mut cache, &inputs, 1790812802000).unwrap();
    assert!(!prepared.rows.is_empty());
    assert!(!prepared.transcripts.is_empty());
    // Exercise the production publisher role, namespace and source/claim CAS.
    let (outcome, generation) = cache.publish_partial_catalog_fenced(&prepared).unwrap();
    assert_eq!(outcome, crate::events::Outcome::Written);
    let config = options.source.config(&options.cache).unwrap();
    let owner = crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
    let connection = crate::storage::sqlite::source_journal::SourceJournal::owned(owner)
        .connection()
        .unwrap();
    let db = &connection;
    let read = |sql: &str| {
        db.prepare(sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap().into_bytes())
            .collect::<Vec<_>>()
    };
    let metadata = read(
        "SELECT metadata FROM session_catalog WHERE namespace=semon_namespace() ORDER BY session_key",
    );
    let slots = read(
        "SELECT metadata FROM session_slots WHERE namespace=semon_namespace() ORDER BY session_key,slot",
    );
    let search = read(
        "SELECT json_array(read_scope,session_key,json(fields),complete) FROM session_search_documents WHERE namespace=semon_namespace() ORDER BY read_scope,session_key",
    );
    let trace = [
        "SELECT metadata FROM session_trace_headers WHERE namespace=semon_namespace() ORDER BY session_key",
        "SELECT metadata FROM session_trace_turns WHERE namespace=semon_namespace() ORDER BY session_key,turn_id",
        "SELECT metadata FROM session_trace_edges WHERE namespace=semon_namespace() ORDER BY session_key,edge_id",
    ]
    .iter()
    .map(|sql| read(sql))
    .collect();
    // The vertical and native-binding scenarios consume only Current. Keep
    // the separately asserted custody scenario in oracle(), so a custody
    // constructor refusal does not prevent qualification of ordinary ingest.
    if !history {
        assert!(!slots.is_empty());
        drop(cache);
        drop(connection);
        if !keep_raw {
            fs::remove_dir_all(&root).unwrap();
        }
        return Input {
            root,
            rows: prepared.rows,
            projections: prepared.transcripts,
            metadata,
            slots,
            trace,
            search,
            original: None,
            original_slots: None,
            composite: None,
            composite_slots: None,
        };
    }
    let mut missing = prepared.rows.clone();
    for row in &mut missing {
        row.sources.clear();
    }
    let missing_recipes = missing
        .iter()
        .map(|row| crate::slot_projection::Projection {
            key: row.key.clone(),
            slots: vec![],
            repair: false,
        })
        .collect::<Vec<_>>();
    // Original withdrawal and same-key missing-source composition are distinct
    // lifecycles. Seed both with the same real producer before Raw moves.
    let mut original_options = options.clone();
    original_options.cache = root.join("original-index.json");
    let mut original_cache = crate::events::EventCache::open_scoped(
        &original_options.cache,
        &original_options.source,
    )
    .unwrap();
    let original_prepared = crate::model::prepare_sources(
        &original_options,
        &mut original_cache,
        &inputs,
        1790812802000,
    )
    .unwrap();
    let (_, original_generation) = original_cache
        .publish_partial_catalog_fenced(&original_prepared)
        .unwrap();
    // Retain actual Raw custody while the ordinary removal writer records
    // withdrawal. A row with cleared sources alone cannot authorize removing
    // the still-live ledger from a complete publication.
    let archived = root.join("retained-one.jsonl");
    fs::rename(&source, &archived).unwrap();
    cache
        .retain(&std::collections::BTreeSet::new(), &mut false)
        .unwrap();
    original_cache
        .retain(&std::collections::BTreeSet::new(), &mut false)
        .unwrap();
    let withdrawn_generation = original_cache
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &[],
                transcripts: Some(&[]),
            },
            original_generation.as_deref(),
        )
        .unwrap();
    assert!(withdrawn_generation.is_some());
    // Missing-source replacement and original custody share the ordinary
    // publication lifecycle. Never reopen a physical writer without its role.
    let missing_generation = cache
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &missing,
                transcripts: Some(&missing_recipes),
            },
            generation.as_deref(),
        )
        .unwrap();
    assert!(missing_generation.is_some());
    let composite = read(
        "SELECT metadata FROM session_history_catalog WHERE namespace=semon_namespace() ORDER BY session_key",
    );
    let composite_slots = read(
        "SELECT metadata FROM session_history_slots WHERE namespace=semon_namespace() ORDER BY session_key,slot",
    );
    fs::rename(&archived, &source).unwrap();
    // Returning Raw has a new ctime: obtain its real producer witness rather
    // than publishing the stale pre-withdrawal Source identity.
    let restored =
        crate::model::prepare_sources(&options, &mut cache, &inputs, 1790812802000).unwrap();
    let restored_generation = cache
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &restored.rows,
                transcripts: Some(&restored.transcripts),
            },
            missing_generation.as_deref(),
        )
        .unwrap();
    assert!(restored_generation.is_some());
    let restored_original = crate::model::prepare_sources(
        &original_options,
        &mut original_cache,
        &inputs,
        1790812802000,
    )
    .unwrap();
    assert!(original_cache
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &restored_original.rows,
                transcripts: Some(&restored_original.transcripts),
            },
            withdrawn_generation.as_deref(),
        )
        .unwrap()
        .is_some());
    let original_config = original_options.source.config(&original_options.cache).unwrap();
    let original_owner = crate::events::store::configured::NativeOwnership::admit(&original_config).unwrap();
    let original_db = crate::storage::sqlite::source_journal::SourceJournal::owned(original_owner)
        .connection()
        .unwrap();
    let original = original_db.prepare("SELECT metadata FROM session_history_catalog WHERE namespace=semon_namespace() ORDER BY session_key").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap()
        .map(|r| r.unwrap().into_bytes()).collect::<Vec<_>>();
    let original_slots = original_db.prepare("SELECT metadata FROM session_history_slots WHERE namespace=semon_namespace() ORDER BY session_key,slot").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap()
        .map(|r| r.unwrap().into_bytes()).collect::<Vec<_>>();
    let retained_metadata = original_prepared.rows.iter().map(|row| {
        let mut row = row.clone();
        row.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        serde_json::to_vec(&row).unwrap()
    }).collect::<Vec<_>>();
    assert_eq!(
        original, retained_metadata,
        "original custody keeps the published row"
    );
    assert_eq!(
        original_slots, slots,
        "original custody keeps the complete recipe"
    );
    assert!(!slots.is_empty());
    drop(cache);
    drop(original_cache);
    drop(original_db);
    drop(connection);
    if !keep_raw {
        fs::remove_dir_all(&root).unwrap();
    }
    Input {
        root,
        rows: prepared.rows,
        projections: prepared.transcripts,
        metadata,
        slots,
        trace,
        search,
        original: Some(original),
        original_slots: Some(original_slots),
        composite: Some(composite),
        composite_slots: Some(composite_slots),
    }
}

fn snapshot(tx: &mut PgTxn<'_>) -> Result<Vec<Vec<String>>, StorageError> {
    TABLES.iter().map(|table| {
        tx.driver.query(&format!("SELECT to_jsonb(t)::text FROM semon_pg_native_{table} t ORDER BY to_jsonb(t)::text"), &[]).map_err(sqlstate::error).map(|rows| rows.iter().map(|row| row.get(0)).collect())
    }).collect()
}

#[test]
fn pg_catalog127_missing_raw_composite_shared_oracle_and_namespace_owners() {
    let data = oracle();
    let (pool, _) = fixture(|_| {});
    let mut missing = data.rows.clone();
    for row in &mut missing {
        row.sources.clear();
    }
    let recipes = missing
        .iter()
        .map(|row| crate::slot_projection::Projection {
            key: row.key.clone(),
            slots: vec![],
            repair: false,
        })
        .collect::<Vec<_>>();
    for name in ["history127-a", "history127-b"] {
        let namespace = ns(name);
        pool.transaction(
            &namespace,
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| stage(tx, L1, &data.publication()),
        )
        .unwrap();
        pool.transaction(
            &namespace,
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                assert!(history::preserve(
                    tx,
                    L2,
                    L1,
                    &crate::slot_projection::Publication {
                        rows: &missing,
                        transcripts: Some(&recipes)
                    },
                    &[]
                )?);
                Ok(())
            },
        )
        .unwrap();
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &PgCancellation::default(), |tx| {
            let rows = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_history_headers ORDER BY session_key", &[]).map_err(sqlstate::error)?;
            assert_eq!(rows.iter().map(|row| row.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), *data.composite.as_ref().expect("custody oracle"));
            let rows = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_history_slots ORDER BY session_key,slot", &[]).map_err(sqlstate::error)?;
            assert_eq!(rows.iter().map(|row| row.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), *data.composite_slots.as_ref().expect("custody oracle"));
            Ok(())
        }).unwrap();
        super::snapshot::with_generation(
            &pool,
            &namespace,
            &PgCancellation::default(),
            L2,
            |view| {
                for (index, current) in missing.iter().enumerate() {
                    let retained = view
                        .catalog(&current.key, true)?
                        .ok_or(StorageError::ScopeChanged)?;
                    assert_eq!(
                        serde_json::to_vec(&retained).unwrap(),
                        data.composite.as_ref().expect("custody oracle")[index]
                    );
                    let (_, slots) = view
                        .recipe(&current.key, true, 0, 64)?
                        .ok_or(StorageError::ScopeChanged)?;
                    assert_eq!(
                        slots
                            .iter()
                            .map(|s| s.as_bytes().to_vec())
                            .collect::<Vec<_>>(),
                        *data.composite_slots.as_ref().expect("custody oracle")
                    );
                }
                Ok(())
            },
        )
        .unwrap();
    }
}

#[test]
fn pg_catalog127_original_custody_real_producer_restart_and_atomic_boundaries() {
    let data = oracle();
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("catalog127-original");
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| stage(tx, L1, &data.publication()),
    )
    .unwrap();
    let preserve = |tx: &mut PgTxn<'_>| {
        let store = history::History {
            effects: Effects {
                tx: RefCell::new(tx),
                generation: L2,
            },
            previous: L1,
        };
        for row in &data.rows {
            assert!(catalog::preserve_original(&store, row)?);
        }
        Ok(())
    };
    let before = pool
        .transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot,
        )
        .unwrap();
    BOUNDARIES.with(|count| count.set(0));
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            preserve(tx)?;
            Err::<(), _>(StorageError::Unavailable)
        },
    )
    .unwrap_err();
    let boundaries = BOUNDARIES.with(|count| count.get());
    assert!(boundaries > 0);
    for point in 1..=boundaries {
        FAIL_AFTER.with(|fail| fail.set(Some(point)));
        assert!(
            pool.transaction(
                &namespace,
                TxnKind::Write,
                &PgCancellation::default(),
                preserve
            )
            .is_err()
        );
        FAIL_AFTER.with(|fail| fail.set(None));
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
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        preserve,
    )
    .unwrap();
    let reopened =
        crate::storage::postgres::PgPool::open(factory, crate::storage::postgres::tests::limits())
            .unwrap();
    reopened.transaction(&namespace, TxnKind::ReadSnapshot, &PgCancellation::default(), |tx| {
        let rows = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_history_headers ORDER BY session_key", &[]).map_err(sqlstate::error)?;
        assert_eq!(rows.iter().map(|row| row.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), *data.original.as_ref().expect("custody oracle"));
        let rows = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_history_slots ORDER BY session_key,slot", &[]).map_err(sqlstate::error)?;
        assert_eq!(rows.iter().map(|row| row.get::<_, Vec<u8>>(0)).collect::<Vec<_>>(), *data.original_slots.as_ref().expect("custody oracle"));
        Ok(())
    }).unwrap();
}

#[test]
fn pg_catalog127_history_changed_witness_and_oversize_refuse_without_effects() {
    let data = build_oracle(false, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("catalog127-history-refuse");
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| stage(tx, L1, &data.publication()),
    )
    .unwrap();
    let before = pool
        .transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            snapshot,
        )
        .unwrap();
    for mode in 0..2 {
        assert!(
            pool.transaction(
                &namespace,
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| {
                    let store = history::History {
                        effects: Effects {
                            tx: RefCell::new(tx),
                            generation: L2,
                        },
                        previous: L1,
                    };
                    use catalog::{CompositeHistoryStore, OriginalHistoryStore};
                    if mode == 0 {
                        store.reset_transfer(&data.rows[0].key)
                    } else {
                        store.composed(
                            &data.rows[0].key,
                            L1,
                            L1,
                            "{}",
                            &["x".repeat(MAX + 1)],
                            false,
                        )
                    }
                }
            )
            .is_err()
        );
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
}

#[test]
fn pg_catalog124_real_sqlite_producer_recipe_parity_and_immutable_generations() {
    let data = build_oracle(false, false);
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
        let search = tx.driver.query("SELECT scope,session_key,fields,complete FROM semon_pg_native_catalog_search_documents ORDER BY scope,session_key", &[]).map_err(sqlstate::error)?;
        let search = search.iter().map(|row| {
            let text = |index| String::from_utf8(row.get::<_, Vec<u8>>(index)).unwrap();
            serde_json::to_vec(&serde_json::json!([text(0), text(1), serde_json::from_str::<serde_json::Value>(&text(2)).unwrap(), i32::from(row.get::<_, bool>(3))])).unwrap()
        }).collect::<Vec<_>>();
        assert_eq!(search, data.search);
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
                for table in TABLES
                    .iter()
                    .filter(|table| !table.starts_with("catalog_history_"))
                {
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
    let data = build_oracle(false, false);
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
