use super::*;
use crate::{
    events::{FileIndex, Ledger},
    storage::postgres::{
        PgCancellation,
        tests::{fixture, ns},
        tree_checkpoint::{
            Snapshot,
            tests::{Fixture, append_fixture, parser_fixture},
        },
        txn::TxnKind,
    },
};

// Tree's fixture reparses only the dimensions needed by its family tests.
// A connected file commit needs the complete production scan, including the
// auxiliary headers captured outside parse(). Finish all SQLite work before PG.
fn bundle_fixture(harness: &str) -> Fixture {
    if harness != "copilot" {
        return parser_fixture(harness);
    }
    // Tree's fixture unconditionally unwraps a Tree oracle. Copilot genuinely
    // has none: use real scan/ledger data and never pass an invented Tree input.
    let root = std::env::temp_dir().join(format!(
        "pg117-copilot-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let physical = root.join("one.jsonl");
    let value =
        serde_json::json!({"id":"start🦀","type":"session.start","data":{"sessionId":"s🦀"}});
    std::fs::write(&physical, format!("{value}\n")).unwrap();
    let cache_path = root.join("index.json");
    let mut cache = crate::events::EventCache::open(&cache_path).unwrap();
    let index = crate::events::scan_file(&physical, harness, &mut cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(crate::events::EventCache::path(&cache_path)).unwrap();
    let ledger = db.query_row("SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1", [physical.to_str().unwrap()], |r| Ok(Ledger {stat: crate::events::Stat {dev:r.get::<_,i64>(0)? as u64,ino:r.get::<_,i64>(1)? as u64,size:r.get::<_,i64>(2)? as u64,modified_ns:u128::from_be_bytes(r.get::<_,Vec<u8>>(3)?.try_into().unwrap()),changed_ns:r.get::<_,Option<Vec<u8>>>(4)?.map(|b|i128::from_be_bytes(b.try_into().unwrap()))},offset:r.get::<_,i64>(5)? as u64,prefix:r.get::<_,Vec<u8>>(6)?.try_into().unwrap(),tail:r.get::<_,Vec<u8>>(7)?.try_into().unwrap()})).unwrap();
    let path = format!("{}\0🦀", physical.to_str().unwrap());
    assert!(index.tree_summary.is_none());
    assert!(index.copilot.is_some(), "production Copilot identity");
    Fixture {
        root,
        path: path.clone(),
        ledger: ledger.clone(),
        oracle: Default::default(),
        sources: vec![(path, ledger, (*index).clone())],
        rows: Vec::new(),
    }
}

fn complete_fixture(mut data: Fixture, harness: &str) -> Fixture {
    // Use a fresh producer store: a warm Tree-only restore intentionally omits
    // parser accumulators such as line_count and is not a fresh ingest input.
    let cache_path = data.root.join(format!(
        "bundle-input-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut cache = crate::events::EventCache::open(&cache_path).unwrap();
    for (path, _, index) in &mut data.sources {
        let physical = path.strip_suffix("\0🦀").unwrap();
        let scanned = crate::events::scan_file(
            std::path::Path::new(physical),
            harness,
            &mut cache,
            &mut false,
        )
        .unwrap();
        // scan_file may commit and replace Tree with its warm read model
        // (events.rs load_tree), which omits parser accumulators. The fixture
        // already owns the actual production parse; keep that ingest input.
        let tree = index.tree_summary.take();
        let usage = index.native_usage_observations.take();
        *index = (*scanned).clone();
        index.tree_summary = tree;
        index.native_usage_observations = usage;
        if harness != "copilot" {
            assert!(index.tree_summary.is_some(), "production Tree input");
        }
        if harness == "codex" {
            assert!(
                index.codex_header.is_some(),
                "production Codex header input"
            );
        }
    }
    cache.flush_cold_writes().unwrap();
    drop(cache);
    data
}

fn apply(
    tx: &mut PgTxn<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    expected: Option<&Ledger>,
    changes: Option<&DirtyRows>,
) -> Result<(), StorageError> {
    let partial = stage(
        tx,
        Input {
            path,
            ledger,
            index,
            changes,
            expected,
            semantic_change: expected != Some(ledger),
        },
    )?;
    assert_eq!(partial.missing.len(), 4);
    Ok(())
}

// Every native row, including generations and auxiliary metadata, is compared
// byte-for-byte around rollback. No giant assertion payload enters CI logs.
fn snapshot(tx: &mut PgTxn<'_>) -> Result<Vec<(String, Vec<String>)>, StorageError> {
    let tables = tx.driver.query("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND (starts_with(table_name,'semon_pg_native_') OR table_name IN ('semon_pg_operations','semon_pg_source_state','semon_pg_source_members','semon_pg_source_invalidations','semon_pg_source_receipts','semon_pg_source_intents')) ORDER BY table_name", &[]).map_err(sqlstate::error)?;
    let mut result = Vec::new();
    for table in tables {
        let table: String = table.get(0);
        assert!(table.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        let rows = tx.driver.query(&format!("SELECT row_to_json(t.*)::text FROM {table} t WHERE namespace=$1 ORDER BY row_to_json(t.*)::text"), &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
        result.push((table, rows.iter().map(|r| r.get(0)).collect()));
    }
    Ok(result)
}

fn oracle_metadata(data: &Fixture) -> Vec<(String, String)> {
    let db = rusqlite::Connection::open(crate::events::EventCache::path(
        &data.root.join("index.json"),
    ))
    .unwrap();
    let path = data.root.join("one.jsonl").to_str().unwrap().to_owned();
    let mut result = Vec::new();
    for prefix in [
        "native_codex_header:",
        "native_claude_sidecar:",
        "native_parent_marker:",
    ] {
        let mut query = db.prepare("SELECT value FROM meta WHERE key=?1").unwrap();
        let rows = query
            .query_map([format!("{prefix}{path}")], |r| r.get::<_, String>(0))
            .unwrap();
        for value in rows {
            result.push((format!("{prefix}{}", data.path), value.unwrap()));
        }
    }
    result
}

#[test]
fn pg_native_bundle_real_parser_connected_initial_append_restart_and_metadata_oracle() {
    for harness in ["claude", "codex", "copilot"] {
        let data = complete_fixture(bundle_fixture(harness), harness);
        let metadata = oracle_metadata(&data);
        if harness == "codex" {
            assert!(!metadata.is_empty(), "nonempty independent header oracle");
        }
        let next = (harness != "copilot")
            .then(|| complete_fixture(append_fixture(&data, harness), harness));
        let (pool, factory) = fixture(|_| {});
        let namespace = ns("bundle\0🦀");
        let control = PgCancellation::default();
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            for (path, ledger, index) in &data.sources {
                apply(tx, path, ledger, index, None, None)?;
            }
            for (key, value) in &metadata {
                let rows = tx
                    .driver
                    .query(
                        "SELECT value FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2",
                        &[&tx.namespace.as_str().as_bytes(), &key.as_bytes()],
                    )
                    .map_err(sqlstate::error)?;
                assert_eq!(rows.len(), 1, "independent auxiliary metadata row exists");
                let actual: Vec<u8> = rows[0].get(0);
                assert!(
                    actual == value.as_bytes(),
                    "independent auxiliary metadata oracle"
                );
            }
            Ok(())
        })
        .unwrap();
        let bounds = pool.inner.limits;
        drop(pool);
        let pool = crate::storage::postgres::PgPool::open(factory.clone(), bounds).unwrap();
        if harness == "copilot" {
            // Production Copilot input has no Tree summary. Do not manufacture
            // one to satisfy another family's test fixture or readiness gate.
            pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                let count: i64 = tx
                    .driver
                    .query_one(
                        "SELECT count(*) FROM semon_pg_native_tree_headers WHERE namespace=$1",
                        &[&tx.namespace.as_str().as_bytes()],
                    )
                    .map_err(sqlstate::error)?
                    .get(0);
                assert_eq!(count, 0, "Copilot has no native Tree header");
                Ok(())
            })
            .unwrap();
            drop(pool);
            drop(factory);
            std::fs::remove_dir_all(data.root).unwrap();
            continue;
        }
        // Reopen before restoring the scalar/checkpoint through the PG reader.
        let restored = Snapshot::restore(
            pool.clone(),
            namespace.clone(),
            &data.path,
            &data.ledger,
            control.clone(),
        )
        .unwrap();
        assert_eq!(restored.line_count, data.oracle.line_count);
        assert_eq!(restored.tools, data.oracle.tools);
        if let Some(next) = next {
            let (_, _, index) = next
                .sources
                .iter()
                .find(|(path, _, _)| path == &next.path)
                .unwrap();
            let (_, _, old) = data
                .sources
                .iter()
                .find(|(path, _, _)| path == &data.path)
                .unwrap();
            let changes = DirtyRows::between(old, index);
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                apply(
                    tx,
                    &next.path,
                    &next.ledger,
                    index,
                    Some(&data.ledger),
                    Some(&changes),
                )
            })
            .unwrap();
            let restored = Snapshot::restore(
                pool.clone(),
                namespace.clone(),
                &next.path,
                &next.ledger,
                control.clone(),
            )
            .unwrap();
            assert_eq!(restored.line_count, next.oracle.line_count);
            assert_eq!(restored.tools, next.oracle.tools);
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                apply(tx, &data.path, &data.ledger, old, Some(&next.ledger), None)
            })
            .unwrap();
            let replaced = Snapshot::restore(
                pool.clone(),
                namespace.clone(),
                &data.path,
                &data.ledger,
                control.clone(),
            )
            .unwrap();
            assert_eq!(replaced.line_count, data.oracle.line_count);
            assert_eq!(replaced.tools, data.oracle.tools);
        }
        drop(pool);
        drop(factory);
        std::fs::remove_dir_all(data.root).unwrap();
    }
}

#[test]
fn pg_native_bundle_namespace_retirement_and_drift_refusal() {
    let data = complete_fixture(parser_fixture("codex"), "codex");
    let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
    let namespace = ns("bundle-owner\0🦀");
    let foreign = ns("bundle-foreign\0🦀");
    let control = PgCancellation::default();
    let (_, ledger, index) = &data.sources[0];
    pool.transaction(&foreign, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, None, None)
    })
    .unwrap();
    let foreign_before = pool
        .transaction(&foreign, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let ns = tx.namespace.as_str().as_bytes().to_vec();
        let key = format!("archive_event_index:{}",data.path);
        tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3)", &[&ns,&key.as_bytes(),&b"prior archive".as_slice()]).map_err(sqlstate::error)?;
        tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3)", &[&ns,&data.path.as_bytes(),&b"prior mutation".as_slice()]).map_err(sqlstate::error)?;
        apply(tx,&data.path,ledger,index,None,None)?;
        let retired: bool = tx.driver.query_one("SELECT NOT EXISTS(SELECT 1 FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace=$1 AND source_path=$3)", &[&ns,&key.as_bytes(),&data.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        assert!(retired);
        tx.driver.execute("UPDATE semon_pg_native_usage_sources SET receipt=$2 WHERE namespace=$1", &[&ns,&b"drift".as_slice()]).map_err(sqlstate::error)?;
        Ok(())
    }).unwrap();
    let before = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, Some(ledger), None)
    });
    assert!(matches!(result, Err(StorageError::ScopeChanged)));
    assert!(
        before
            == pool
                .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
                .unwrap()
    );
    assert!(
        foreign_before
            == pool
                .transaction(&foreign, TxnKind::ReadSnapshot, &control, snapshot)
                .unwrap()
    );
    drop(pool);
    drop(factory);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_bundle_every_connected_boundary_rolls_back_and_stale_cas_refuses() {
    let data = complete_fixture(parser_fixture("codex"), "codex");
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("bundle-fault\0🦀");
    let control = PgCancellation::default();
    let (_, ledger, index) = &data.sources[0];
    let before = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    // Measure the actual connected path, then roll it back. This includes
    // every page blob, generation reference, header and Current-pointer effect.
    BOUNDARIES.with(|count| count.set(0));
    let probe: Result<(), StorageError> =
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            apply(tx, &data.path, ledger, index, None, None)?;
            Err(StorageError::Busy)
        });
    assert!(matches!(probe, Err(StorageError::Busy)));
    let boundaries = BOUNDARIES.with(|count| count.get());
    assert!((11..128).contains(&boundaries));
    for point in 1..=boundaries {
        FAIL_AFTER.with(|p| p.set(Some(point)));
        let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            apply(tx, &data.path, ledger, index, None, None)
        });
        FAIL_AFTER.with(|p| p.set(None));
        assert!(
            matches!(result, Err(StorageError::Unavailable)),
            "bundle boundary {point}"
        );
        let after = pool
            .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
            .unwrap();
        assert!(
            before == after,
            "whole connected bundle rollback at {point}"
        );
    }
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, None, None)
    })
    .unwrap();
    let stable = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, None, None)
    });
    assert!(matches!(result, Err(StorageError::Conflict)));
    assert!(
        stable
            == pool
                .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
                .unwrap()
    );
    drop(pool);
    drop(factory);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_bundle_copilot_cannot_bypass_existing_tree() {
    let codex = complete_fixture(parser_fixture("codex"), "codex");
    let copilot = complete_fixture(bundle_fixture("copilot"), "copilot");
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("bundle-tree-retirement\0🦀");
    let control = PgCancellation::default();
    let (_, ledger, index) = &codex.sources[0];
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &codex.path, ledger, index, None, None)
    })
    .unwrap();
    let before = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    let (_, replacement, index) = &copilot.sources[0];
    assert!(index.tree_summary.is_none());
    let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &codex.path, replacement, index, Some(ledger), None)
    });
    assert!(matches!(result, Err(StorageError::NamespaceUpgradePending)));
    let after = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    assert!(
        before == after,
        "Tree retirement refuses before any effects"
    );
    drop(pool);
    drop(factory);
    std::fs::remove_dir_all(codex.root).unwrap();
    std::fs::remove_dir_all(copilot.root).unwrap();
}

fn page_oracle(data: &Fixture) -> (String, Option<String>, Vec<(i16, String, String)>) {
    use sha2::{Digest, Sha256};
    let db = rusqlite::Connection::open(crate::events::EventCache::path(
        &data.root.join("index.json"),
    ))
    .unwrap();
    let physical = data.path.strip_suffix("\0🦀").unwrap();
    let prefix = format!(
        "native_source_chunks:{:x}:",
        Sha256::digest(physical.as_bytes())
    );
    let launch = format!(
        "native_launch_calls:{:x}:",
        Sha256::digest(physical.as_bytes())
    );
    let header = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("{prefix}header")],
            |r| r.get(0),
        )
        .unwrap();
    use rusqlite::OptionalExtension;
    let launch_header = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("{launch}header")],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let mut pages = Vec::new();
    for (kind, prefix) in [
        (0i16, prefix.clone()),
        (1, launch),
        (2, format!("native_proof_tree:{prefix}:")),
    ] {
        let mut statement = db
            .prepare("SELECT key,value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")
            .unwrap();
        let rows = statement
            .query_map(rusqlite::params![&prefix, format!("{prefix}~")], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();
        for row in rows {
            let (key, value) = row.unwrap();
            if key != format!("{prefix}header") {
                pages.push((kind, key.strip_prefix(&prefix).unwrap().to_owned(), value));
            }
        }
    }
    (header, launch_header, pages)
}

#[test]
fn pg_native_pages_sqlite_bytes_retained_through_append_replacement_restart() {
    // SQLite's process-wide connection hook must not interfere with unrelated
    // parallel SQLite tests. Run this actual parity test in an isolated test
    // process, not a mock adapter or a counter on EventCache reads.
    const CHILD: &str = "SEMON_NATIVE122_SQLITE_GUARD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage::postgres::native_bundle::tests::pg_native_pages_sqlite_bytes_retained_through_append_replacement_restart", "--nocapture"])
            .env(CHILD, "1")
            .status().unwrap();
        assert!(status.success(), "isolated no-SQLite parity phase");
        return;
    }
    use crate::storage::postgres::{PgPool, native_pages, tests::limits};
    let data = complete_fixture(parser_fixture("codex"), "codex");
    let oracle = page_oracle(&data);
    let next = complete_fixture(append_fixture(&data, "codex"), "codex");
    let next_oracle = page_oracle(&next);
    // All SQLite work and independent oracle materialization precedes PG.
    let _no_sqlite = NoSqlite::new();
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("pages\0🦀");
    let control = PgCancellation::default();
    let (_, ledger, index) = &data.sources[0];
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, None, None)
    })
    .unwrap();
    let pinned = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            native_pages::current(tx, &data.path, ledger)
        })
        .unwrap();
    assert!(
        pinned.chunks_header == oracle.0 && pinned.launch_header == oracle.1,
        "SQLite header bytes"
    );
    let (_, new_ledger, new_index) = &next.sources[0];
    let changes = DirtyRows::between(index, new_index);
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(
            tx,
            &next.path,
            new_ledger,
            new_index,
            Some(ledger),
            Some(&changes),
        )
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let current = native_pages::current(tx, &next.path, new_ledger)?;
        assert_ne!(current.id, pinned.id);
        assert!(
            current.chunks_header == next_oracle.0 && current.launch_header == next_oracle.1,
            "append oracle bytes"
        );
        for (kind, key, expected) in &oracle.2 {
            assert!(
                pinned.page(tx, *kind, key)? == *expected,
                "retained SQLite page bytes"
            );
        }
        let chunks = index.source_chunks.as_ref().unwrap();
        assert_eq!(pinned.chunk_hash(tx, 0)?, chunks.hashes[0]);
        Ok(())
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, Some(new_ledger), None)
    })
    .unwrap();
    drop(pool);
    let reopened = PgPool::open(factory.clone(), limits()).unwrap();
    reopened
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let retained = native_pages::open(tx, &data.path, ledger, pinned.id.clone())?;
            for (kind, key, expected) in &oracle.2 {
                assert!(
                    retained.page(tx, *kind, key)? == *expected,
                    "restart immutable bytes"
                );
            }
            let later = native_pages::current(tx, &data.path, ledger)?;
            assert!(later.chunks_header == oracle.0, "replacement oracle");
            assert!(
                native_pages::open(tx, &data.path, new_ledger, retained.id.clone()).is_err(),
                "wrong ledger refused"
            );
            Ok(())
        })
        .unwrap();
    drop(reopened);
    drop(factory);
    std::fs::remove_dir_all(data.root).unwrap();
}

static SQLITE_ATTEMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "C" fn reject_sqlite(
    _: *mut rusqlite::ffi::sqlite3,
    _: *mut *mut std::ffi::c_char,
    _: *const rusqlite::ffi::sqlite3_api_routines,
) -> std::ffi::c_int {
    SQLITE_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    rusqlite::ffi::SQLITE_AUTH
}

// Every rusqlite production open/flags/in-memory entrypoint reaches SQLite's
// connection initializer. All oracle connections are closed before this scope;
// the rejection also catches a swallowed failed fallback through its counter.
struct NoSqlite;
impl NoSqlite {
    fn new() -> Self {
        SQLITE_ATTEMPTS.store(0, std::sync::atomic::Ordering::SeqCst);
        let result = unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(reject_sqlite)) };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
        assert!(
            rusqlite::Connection::open_in_memory().is_err(),
            "guard must reject an actual SQLite connection"
        );
        assert_eq!(
            SQLITE_ATTEMPTS.swap(0, std::sync::atomic::Ordering::SeqCst),
            1
        );
        Self
    }
}
impl Drop for NoSqlite {
    fn drop(&mut self) {
        unsafe {
            rusqlite::ffi::sqlite3_cancel_auto_extension(Some(reject_sqlite));
        }
        assert_eq!(
            SQLITE_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "PG phase attempted SQLite access"
        );
    }
}

#[test]
fn pg_native_pages_tamper_namespace_and_missing_generation_refuse() {
    use crate::storage::postgres::native_pages;
    let data = complete_fixture(parser_fixture("codex"), "codex");
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("proof-owner\0🦀");
    let foreign = ns("proof-foreign\0🦀");
    let control = PgCancellation::default();
    let (_, ledger, index) = &data.sources[0];
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, None, None)
    })
    .unwrap();
    let pinned = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            native_pages::current(tx, &data.path, ledger)
        })
        .unwrap();
    pool.transaction(&foreign, TxnKind::ReadSnapshot, &control, |tx| {
        assert!(pinned.chunk_hash(tx, 0).is_err());
        assert!(native_pages::open(tx, &data.path, ledger, pinned.id.clone()).is_err());
        Ok(())
    })
    .unwrap();
    let result: Result<(), StorageError> =
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver
                .execute(
                    "UPDATE semon_pg_native_page_blobs SET payload=$2 WHERE namespace=$1",
                    &[&tx.namespace.as_str().as_bytes(), &b"tampered".as_slice()],
                )
                .map_err(sqlstate::error)?;
            assert!(
                pinned.chunk_hash(tx, 0).is_err(),
                "tampered old page refuses"
            );
            Err(StorageError::Busy)
        });
    assert!(matches!(result, Err(StorageError::Busy)));
    let result: Result<(), StorageError> =
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver
                .execute(
                    "DELETE FROM semon_pg_native_page_generations WHERE namespace=$1",
                    &[&tx.namespace.as_str().as_bytes()],
                )
                .map_err(sqlstate::error)?;
            assert!(
                native_pages::open(tx, &data.path, ledger, pinned.id.clone()).is_err(),
                "missing authority refuses"
            );
            Err(StorageError::Busy)
        });
    assert!(matches!(result, Err(StorageError::Busy)));
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(
            pinned.chunk_hash(tx, 0)?,
            index.source_chunks.as_ref().unwrap().hashes[0]
        );
        Ok(())
    })
    .unwrap();
    drop(pool);
    drop(factory);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_pages_launch_257_and_eof_match_production_sqlite() {
    use crate::storage::postgres::native_pages;
    let data = bundle_fixture("copilot");
    let physical = data.root.join("launch.jsonl");
    let mut body = String::new();
    for at in 0..258 {
        let value = serde_json::json!({"message":{"content":[{"type":"tool_use","id":format!("launch-{at}"),"name":"Bash","input":{"command":"codex exec fixture"}}]}});
        body.push_str(&value.to_string());
        if at < 257 {
            body.push('\n');
        }
    }
    std::fs::write(&physical, body.as_bytes()).unwrap();
    let cache_path = data.root.join("launch-index.json");
    let mut cache = crate::events::EventCache::open(&cache_path).unwrap();
    let scanned = crate::events::scan_file(&physical, "claude", &mut cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(crate::events::EventCache::path(&cache_path)).unwrap();
    let ledger: Ledger = db.query_row("SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1", [physical.to_str().unwrap()], |r| Ok(Ledger {stat: crate::events::Stat {dev:r.get::<_,i64>(0)? as u64,ino:r.get::<_,i64>(1)? as u64,size:r.get::<_,i64>(2)? as u64,modified_ns:u128::from_be_bytes(r.get::<_,Vec<u8>>(3)?.try_into().unwrap()),changed_ns:r.get::<_,Option<Vec<u8>>>(4)?.map(|b|i128::from_be_bytes(b.try_into().unwrap()))},offset:r.get::<_,i64>(5)? as u64,prefix:r.get::<_,Vec<u8>>(6)?.try_into().unwrap(),tail:r.get::<_,Vec<u8>>(7)?.try_into().unwrap()})).unwrap();
    use sha2::{Digest, Sha256};
    let prefix = format!(
        "native_launch_calls:{:x}:",
        Sha256::digest(physical.to_str().unwrap().as_bytes())
    );
    let mut statement = db
        .prepare("SELECT key,value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")
        .unwrap();
    let oracle = statement
        .query_map(rusqlite::params![&prefix, format!("{prefix}~")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(oracle.len() >= 3, "two production launch pages and header");
    let expected = scanned.handoff_index.as_ref().unwrap().clone();
    assert_eq!(expected.complete.len(), 257);
    assert_eq!(expected.eof.len(), 1);
    drop(statement);
    drop(db);
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("launch-pages\0🦀");
    let control = PgCancellation::default();
    let (_, old_ledger, old_index) = &data.sources[0];
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, old_ledger, old_index, None, None)
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let source = algorithm::begin(&mut Usage::new(tx)?, "launch\0🦀", None, &ledger, false)?;
        algorithm::write(&mut Usage::new(tx)?, "launch\0🦀", &ledger, &scanned)?;
        let prepared = native_pages::prepare(&ledger, &scanned)?;
        let id = native_pages::write(tx, source, "launch\0🦀", &ledger, prepared)?;
        let pinned = native_pages::open(tx, "launch\0🦀", &ledger, id)?;
        for (key, value) in &oracle {
            let suffix = key.strip_prefix(&prefix).unwrap();
            if suffix == "header" {
                assert!(pinned.launch_header.as_ref() == Some(value));
            } else {
                assert!(
                    pinned.page(tx, 1, suffix)? == *value,
                    "production launch page bytes"
                );
            }
        }
        assert_eq!(pinned.launch(tx)?, expected);
        Ok(())
    })
    .unwrap();
    drop(pool);
    drop(factory);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_bundle_relocation_advance_matches_sqlite_and_rolls_back() {
    // Exercise advancement, not Raw/Catalog move qualification. The prior
    // evidence is a supplement; the next bytes come from the real SQLite
    // producer's append path before the PG phase begins.
    let data = complete_fixture(parser_fixture("codex"), "codex");
    let (_, ledger, index) = &data.sources[0];
    let physical = data.path.strip_suffix("\0🦀").unwrap();
    let old = "old\0🦀";
    let key = format!("native_catalog_relocation:{old}");
    let evidence = serde_json::to_string(&shared::CatalogRelocation {
        version: 1,
        new_path: physical.to_owned(),
        native_id: "qualified-native".into(),
        prior: ledger.clone(),
        catalog_before: None,
        after: ledger.clone(),
    })
    .unwrap();
    let db_path = crate::events::EventCache::path(&data.root.join("index.json"));
    {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute("INSERT INTO catalog_relocation_candidates(old_path,new_path,native_id) VALUES(?1,?2,?3)", rusqlite::params![old, physical, "qualified-native"]).unwrap();
        db.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2)",
            rusqlite::params![key, evidence],
        )
        .unwrap();
    }
    let next = complete_fixture(append_fixture(&data, "codex"), "codex");
    let oracle: String = {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
            .unwrap()
    };
    assert!(
        oracle != evidence,
        "actual SQLite producer advanced evidence"
    );
    let (pool, factory) = fixture(|_| {});
    let namespace = ns("relocation-advance\0🦀");
    let control = PgCancellation::default();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(tx, &data.path, ledger, index, None, None)?;
        let ns = tx.namespace.as_str().as_bytes();
        tx.driver.execute("INSERT INTO semon_pg_native_relocation_candidates(namespace,old_path,new_path) VALUES($1,$2,$3)", &[&ns,&old.as_bytes(),&data.path.as_bytes()]).map_err(sqlstate::error)?;
        tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3)", &[&ns,&key.as_bytes(),&evidence.as_bytes()]).map_err(sqlstate::error)?;
        Ok(())
    }).unwrap();
    let stable = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
        .unwrap();
    let (_, next_ledger, next_index) = &next.sources[0];
    let changes = DirtyRows::between(index, next_index);
    FAIL_AFTER.with(|point| point.set(Some(1)));
    let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(
            tx,
            &data.path,
            next_ledger,
            next_index,
            Some(ledger),
            Some(&changes),
        )
    });
    FAIL_AFTER.with(|point| point.set(None));
    assert!(matches!(result, Err(StorageError::Unavailable)));
    assert!(
        stable
            == pool
                .transaction(&namespace, TxnKind::ReadSnapshot, &control, snapshot)
                .unwrap(),
        "evidence and every native family roll back together"
    );
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        apply(
            tx,
            &data.path,
            next_ledger,
            next_index,
            Some(ledger),
            Some(&changes),
        )?;
        let row = tx
            .driver
            .query_one(
                "SELECT value FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2",
                &[&tx.namespace.as_str().as_bytes(), &key.as_bytes()],
            )
            .map_err(sqlstate::error)?;
        assert!(
            row.get::<_, Vec<u8>>(0) == oracle.as_bytes(),
            "independent SQLite advancement bytes"
        );
        Ok(())
    })
    .unwrap();
    drop(pool);
    drop(factory);
    std::fs::remove_dir_all(data.root).unwrap();
}
