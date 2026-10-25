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
fn complete_fixture(mut data: Fixture, harness: &str) -> Fixture {
    let mut cache = crate::events::EventCache::open(&data.root.join("index.json")).unwrap();
    for (path, _, index) in &mut data.sources {
        let physical = path.strip_suffix("\0🦀").unwrap();
        let scanned = crate::events::scan_file(
            std::path::Path::new(physical),
            harness,
            &mut cache,
            &mut false,
        )
        .unwrap();
        *index = (*scanned).clone();
        assert!(index.tree_summary.is_some(), "production Tree input");
        if harness == "codex" {
            assert!(index.codex_header.is_some(), "production Codex header input");
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
        let data = complete_fixture(parser_fixture(harness), harness);
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
    // Ten actual bundle boundaries for this fixture, plus every nested
    // Core/Usage/Tree boundary remains covered by the inherited family tests.
    for point in 1..=10 {
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
