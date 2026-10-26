use super::*;
use crate::{
    events::{EventCache, FileIndex},
    storage::{LogicalSourceNamespace, StoreBackend, StoreConfig, native_events::NativeEventStore},
};
use std::sync::atomic::{AtomicUsize, Ordering};

static SQLITE_OPENS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn deny_sqlite(
    _: *mut rusqlite::ffi::sqlite3,
    _: *mut *mut std::ffi::c_char,
    _: *const rusqlite::ffi::sqlite3_api_routines,
) -> std::ffi::c_int {
    SQLITE_OPENS.fetch_add(1, Ordering::SeqCst);
    rusqlite::ffi::SQLITE_AUTH
}
struct Guard;
impl Guard {
    fn new() -> Self {
        assert_eq!(
            unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(deny_sqlite)) },
            rusqlite::ffi::SQLITE_OK
        );
        std::thread::spawn(|| assert!(rusqlite::Connection::open_in_memory().is_err()))
            .join()
            .unwrap();
        assert_eq!(SQLITE_OPENS.swap(0, Ordering::SeqCst), 1);
        Self
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            rusqlite::ffi::sqlite3_cancel_auto_extension(Some(deny_sqlite));
        }
        assert_eq!(
            SQLITE_OPENS.load(Ordering::SeqCst),
            0,
            "PG phase attempted SQLite open"
        );
    }
}

fn fingerprint(index: &FileIndex) -> serde_json::Value {
    crate::events::native_decode::fingerprint(index)
}

#[test]
fn pg_native126_real_event_cache_sqlite_oracle_restart_and_exact_replay() {
    const CHILD: &str = "SEMON_NATIVE126_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","storage::postgres::native_ingest_tests::pg_native126_real_event_cache_sqlite_oracle_restart_and_exact_replay","--nocapture"]).env(CHILD,"1").status().unwrap();
        assert!(status.success(), "isolated actual PG native pipeline");
        return;
    }
    struct Case {
        root: std::path::PathBuf,
        path: std::path::PathBuf,
        harness: &'static str,
        body: Vec<u8>,
        oracle: serde_json::Value,
        consumed: u64,
        lifecycle: Vec<(Vec<u8>, bool, serde_json::Value, u64)>,
    }
    let mut cases = Vec::new();
    for harness in ["claude", "codex", "copilot"] {
        let (root, body) = if harness == "copilot" {
            let root =
                std::env::temp_dir().join(format!("native126-copilot-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            (root,format!("{}\n",serde_json::json!({"id":"start","type":"session.start","timestamp":"2026-10-26T00:00:00Z","data":{"sessionId":"copilot126","context":{"cwd":"/sanitized"}}})).into_bytes())
        } else {
            let data = tree_checkpoint::tests::parser_fixture(harness);
            let body = std::fs::read(data.root.join("one.jsonl")).unwrap();
            (data.root, body)
        };
        let path = root.join("raw126.jsonl");
        std::fs::write(&path, &body).unwrap();
        let config = StoreConfig {
            backend: StoreBackend::Sqlite {
                database: root.join("current126.sqlite3"),
            },
            namespace: tests::ns("native126"),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        crate::events::store::configured::create_semantic_store(&config).unwrap();
        // The private current producer owns its own composite admission. The
        // generic public factory remains fenced until full qualification.
        let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap();
        let mut sqlite = EventCache::open_semantic(cap).unwrap();
        let receipt = sqlite.ingest(&"1".repeat(64), &path, harness).unwrap();
        let oracle = crate::events::scan_file(&path, harness, &mut sqlite, &mut false).unwrap();
        let oracle = fingerprint(&oracle);
        let mut lifecycle = Vec::new();
        let first_line = body.iter().position(|b| *b == b'\n').unwrap() + 1;
        for (step, (bytes, replace)) in [
            ([body.as_slice(), b"\n"].concat(), false),
            ([body.as_slice(), b"\n{"].concat(), false),
            (body.clone(), true),
            (body[..first_line].to_vec(), false),
        ]
        .into_iter()
        .enumerate()
        {
            if replace {
                let replacement = root.join("replace130.jsonl");
                std::fs::write(&replacement, &bytes).unwrap();
                std::fs::rename(replacement, &path).unwrap();
            } else {
                std::fs::write(&path, &bytes).unwrap();
            }
            let receipt = sqlite
                .ingest(&format!("{:064x}", step + 16), &path, harness)
                .unwrap();
            let index = crate::events::scan_file(&path, harness, &mut sqlite, &mut false).unwrap();
            lifecycle.push((bytes, replace, fingerprint(&index), receipt.consumed_bytes));
        }
        drop(sqlite);
        std::fs::write(&path, &body).unwrap();
        cases.push(Case {
            root,
            path,
            harness,
            body,
            oracle,
            consumed: receipt.consumed_bytes,
            lifecycle,
        });
    }
    let batch_ids = (101..104)
        .map(|id| format!("{id:064x}"))
        .collect::<Vec<_>>();
    let requests = cases
        .iter()
        .zip(&batch_ids)
        .map(|(case, id)| (id.as_str(), case.path.as_path(), case.harness))
        .collect::<Vec<_>>();
    let config = StoreConfig {
        backend: StoreBackend::Sqlite {
            database: cases[0].root.join("batch133.sqlite3"),
        },
        namespace: tests::ns("native133"),
        expected_stream_epoch: Some("a".repeat(64)),
    };
    crate::events::store::configured::create_semantic_store(&config).unwrap();
    let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap();
    let mut sqlite = EventCache::open_semantic(cap).unwrap();
    let batch_oracle = sqlite.ingest_initial_batch(&requests).unwrap();
    let oracle_indexes = cases
        .iter()
        .map(|case| {
            fingerprint(
                &crate::events::scan_file(&case.path, case.harness, &mut sqlite, &mut false)
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    drop(sqlite);
    let _guard = Guard::new();
    {
        let (pool, _factory) = tests::fixture(|_| {});
        let control = PgCancellation::default();
        let probe = tests::ns("native133-probe");
        let mut pg = EventCache::open_pg_native(pool.clone(), probe, control.clone()).unwrap();
        native_bundle::inject_failure(None);
        let receipts = pg.ingest_initial_batch(&requests).unwrap();
        let boundaries = native_bundle::measured_boundaries();
        assert!(boundaries > 3, "must measure real native family effects");
        assert_eq!(
            receipts
                .iter()
                .map(|r| r.consumed_bytes)
                .collect::<Vec<_>>(),
            batch_oracle
                .iter()
                .map(|r| r.consumed_bytes)
                .collect::<Vec<_>>()
        );
        for (case, oracle) in cases.iter().zip(&oracle_indexes) {
            assert!(
                fingerprint(
                    &crate::events::scan_file(&case.path, case.harness, &mut pg, &mut false)
                        .unwrap()
                ) == *oracle
            );
        }
        let namespace = tests::ns("native133-fault");
        // First failure persists only prepared intents. Every later measured
        // family failure must restore this exact whole-schema snapshot.
        let mut pg =
            EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
        native_bundle::inject_failure(Some(1));
        assert!(pg.ingest_initial_batch(&requests).is_err());
        let baseline = batch_snapshot(&pool, &namespace, &control);
        for point in 1..=boundaries {
            let mut pg =
                EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone())
                    .unwrap();
            native_bundle::inject_failure(Some(point));
            assert!(pg.ingest_initial_batch(&requests).is_err(), "fault {point}");
            assert!(
                batch_snapshot(&pool, &namespace, &control) == baseline,
                "whole DB changed after fault {point}"
            );
        }
        native_bundle::inject_failure(None);
        let mut pg =
            EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
        pg.ingest_initial_batch(&requests).unwrap();
        drop(pg);
        // Reconcile durable individual receipts after restart without Raw I/O.
        for case in &cases {
            std::fs::rename(&case.path, case.root.join("batch133-missing.jsonl")).unwrap();
        }
        let mut pg =
            EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
        let replay = pg.ingest_initial_batch(&requests).unwrap();
        assert!(replay.iter().all(|r| r.replayed));
        assert_eq!(
            replay.iter().map(|r| r.consumed_bytes).collect::<Vec<_>>(),
            batch_oracle
                .iter()
                .map(|r| r.consumed_bytes)
                .collect::<Vec<_>>()
        );
        for case in &cases {
            std::fs::rename(case.root.join("batch133-missing.jsonl"), &case.path).unwrap();
        }
        let stale = tests::ns("native133-stale");
        let hook_pool = pool.clone();
        let hook_namespace = stale.clone();
        let hook_control = control.clone();
        let hook_path = cases[0].path.to_str().unwrap().to_owned();
        crate::events::BEFORE_COMMIT.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
            hook_pool.transaction(&hook_namespace, txn::TxnKind::Write, &hook_control, |tx| source_journal::invalidate(tx, &hook_path)).unwrap();
        })));
        let mut pg = EventCache::open_pg_native(pool.clone(), stale.clone(), control.clone()).unwrap();
        assert!(pg.ingest_initial_batch(&requests).is_err(), "stale Source claim");
        let rows: i64 = pool.transaction(&stale, txn::TxnKind::ReadSnapshot, &control, |tx| {
            Ok(tx.driver.query_one("SELECT count(*) FROM semon_pg_native_files WHERE namespace=$1", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0))
        }).unwrap();
        assert_eq!(rows, 0, "no partial cold batch after stale Source claim");
        for same_ids in [true, false] {
            let namespace = tests::ns(if same_ids { "native133-race-same" } else { "native133-race-different" });
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let handles = (0..2).map(|worker| {
                let pool = pool.clone();
                let namespace = namespace.clone();
                let control = control.clone();
                let barrier = barrier.clone();
                let owned = cases.iter().enumerate().map(|(n, case)| (format!("{:064x}", 201 + n + if same_ids { 0 } else { worker * 16 }), case.path.clone(), case.harness)).collect::<Vec<_>>();
                std::thread::spawn(move || {
                    let mut pg = EventCache::open_pg_native(pool, namespace, control).unwrap();
                    crate::events::BEFORE_COMMIT.with(|hook| *hook.borrow_mut() = Some(Box::new(move || { barrier.wait(); })));
                    let requests = owned.iter().map(|(id, path, harness)| (id.as_str(), path.as_path(), *harness)).collect::<Vec<_>>();
                    pg.ingest_initial_batch(&requests)
                })
            }).collect::<Vec<_>>();
            let outcomes = handles.into_iter().map(|handle| handle.join().unwrap()).collect::<Vec<_>>();
            assert!(outcomes.iter().any(Result::is_ok), "one batch writer must win");
            for outcome in outcomes.into_iter().flatten() {
                assert_eq!(outcome.len(), requests.len(), "no partial receipt batch");
            }
        }
    }
    for case in cases {
        let (pool, factory) = tests::fixture(|_| {});
        let namespace = tests::ns("native126");
        let control = PgCancellation::default();
        let mut pg =
            EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
        let id = "1".repeat(64);
        let receipt = pg.ingest(&id, &case.path, case.harness).unwrap();
        assert!(!receipt.replayed);
        assert_eq!(receipt.consumed_bytes, case.consumed);
        let index =
            crate::events::scan_file(&case.path, case.harness, &mut pg, &mut false).unwrap();
        assert!(
            fingerprint(&index) == case.oracle,
            "actual native metadata/usage/Tree/aux/proofs differ for {}",
            case.harness
        );
        drop(index);
        drop(pg);
        let mut restarted =
            EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
        let noop = restarted
            .ingest(&"2".repeat(64), &case.path, case.harness)
            .unwrap();
        assert_eq!(noop.consumed_bytes, receipt.consumed_bytes);
        // Metadata-only enumeration and discovery must not acquire a SQLite
        // coordinate. The process-wide hook remains active on every thread.
        restarted.begin_full_scan().unwrap();
        restarted
            .discover_catalog_sources(&crate::Options::default(), &[], false)
            .unwrap();
        for (step, (bytes, replace, oracle, consumed)) in case.lifecycle.iter().enumerate() {
            if *replace {
                let replacement = case.root.join("replace130.jsonl");
                std::fs::write(&replacement, bytes).unwrap();
                std::fs::rename(replacement, &case.path).unwrap();
            } else {
                std::fs::write(&case.path, bytes).unwrap();
            }
            let receipt = restarted
                .ingest(&format!("{:064x}", step + 16), &case.path, case.harness)
                .unwrap();
            assert_eq!(receipt.consumed_bytes, *consumed);
            let index =
                crate::events::scan_file(&case.path, case.harness, &mut restarted, &mut false)
                    .unwrap();
            assert!(
                fingerprint(&index) == *oracle,
                "native lifecycle step {step} for {}",
                case.harness
            );
        }
        assert!(restarted.ingest(&id, &case.path, "invalid").is_err());
        assert!(
            restarted
                .ingest(&id, &case.root.join("other"), case.harness)
                .is_err()
        );
        std::fs::write(
            &case.path,
            [case.body.as_slice(), b"incomplete tail"].concat(),
        )
        .unwrap();
        let replay = restarted.ingest(&id, &case.path, case.harness).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.consumed_bytes, receipt.consumed_bytes);
        std::fs::remove_file(&case.path).unwrap();
        drop(restarted);
        let mut recovered = EventCache::open_pg_native(pool, namespace, control).unwrap();
        let replay = recovered.ingest(&id, &case.path, case.harness).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.consumed_bytes, receipt.consumed_bytes);
        drop(recovered);
        drop(factory);
    }
}

fn batch_snapshot(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
) -> Vec<(String, Vec<String>)> {
    pool.transaction(namespace, txn::TxnKind::ReadSnapshot, control, |tx| {
        let tables = tx.driver.query("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND starts_with(table_name,'semon_pg_') ORDER BY table_name", &[]).map_err(sqlstate::error)?;
        let mut snapshot = Vec::new();
        for row in tables {
            let table: String = row.get(0);
            assert!(table.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
            let rows = tx.driver.query(&format!("SELECT row_to_json(t.*)::text FROM {table} t ORDER BY row_to_json(t.*)::text"), &[]).map_err(sqlstate::error)?;
            snapshot.push((table, rows.iter().map(|r| r.get(0)).collect()));
        }
        Ok(snapshot)
    }).unwrap()
}
