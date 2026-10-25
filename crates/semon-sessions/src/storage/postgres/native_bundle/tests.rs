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
    let tables = tx.driver.query("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND starts_with(table_name,'semon_pg_native_') ORDER BY table_name", &[]).map_err(sqlstate::error)?;
    let mut result = Vec::new();
    for table in tables {
        let table: String = table.get(0);
        assert!(table.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        let rows = tx.driver.query(&format!("SELECT row_to_json(t)::text FROM {table} t WHERE namespace=$1 ORDER BY row_to_json(t)::text"), &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
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
        let data = parser_fixture(harness);
        let metadata = oracle_metadata(&data);
        let next = (harness != "copilot").then(|| append_fixture(&data, harness));
        let (pool, factory) = fixture(|_| {});
        let namespace = ns("bundle\0🦀");
        let control = PgCancellation::default();
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            for (path, ledger, index) in &data.sources {
                apply(tx, path, ledger, index, None, None)?;
            }
            for (key, value) in &metadata {
                let actual: Vec<u8> = tx
                    .driver
                    .query_one(
                        "SELECT value FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2",
                        &[&tx.namespace.as_str().as_bytes(), &key.as_bytes()],
                    )
                    .map_err(sqlstate::error)?
                    .get(0);
                assert!(
                    actual == value.as_bytes(),
                    "independent auxiliary metadata oracle"
                );
            }
            Ok(())
        })
        .unwrap();
        // Restoring the complete scalar/checkpoint uses the actual PG reader.
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
        }
        drop(pool);
        drop(factory);
        std::fs::remove_dir_all(data.root).unwrap();
    }
}

#[test]
fn pg_native_bundle_every_connected_boundary_rolls_back_and_stale_cas_refuses() {
    let data = parser_fixture("codex");
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
