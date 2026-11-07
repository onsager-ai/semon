#[path = "postgres/catalog_tests.rs"]
mod catalog_tests;
#[path = "postgres/producer_tests.rs"]
mod producer_tests;
#[path = "postgres/raw_tests.rs"]
mod raw_tests;
#[path = "postgres/read_tests.rs"]
mod read_tests;
#[path = "postgres/runtime_tests.rs"]
mod runtime_tests;

use super::super::{
    Namespace, ProjectionTag, StorageError, TagCheck,
    postgres::{PgProjectionStore, schema},
};

pub(crate) async fn connect(dsn: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    client
}

fn assert_same_index(
    path: &std::path::Path,
    harness: &str,
    ledger: &crate::events::Ledger,
    index: &crate::events::FileIndex,
    sqlite: &mut crate::events::EventCache,
) {
    let native = crate::events::scan_file(path, harness, sqlite, &mut false).unwrap();
    assert_eq!(
        sqlite.revision(path).unwrap(),
        (
            ledger.stat.dev,
            ledger.stat.ino,
            ledger.offset,
            ledger.prefix,
            ledger.tail,
            ledger.stat.changed_ns
        )
    );
    assert_eq!(native.events, index.events);
    assert_eq!(native.extras, index.extras);
    assert_eq!(native.signals, index.signals);
    assert_eq!(native.tokens(harness), index.tokens(harness));
    assert_eq!(native.source_chunks, index.source_chunks);
}

async fn pg_sanitized_native_indexes(dsn: &str) {
    let root = super::root();
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/compatibility");
    let namespace = Namespace::new("fixture-machine").unwrap();
    let mut store = PgProjectionStore::new(connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let mut sqlite =
        crate::events::EventCache::open_scoped(&root.0.join("comparison.json")).unwrap();
    for (harness, fixture) in [
        ("codex", "codex-native-a9b0815/parser-cases.jsonl"),
        ("copilot", "copilot-1.0.90/headless-tools.events.jsonl"),
    ] {
        let path = root.0.join(format!("{harness}.jsonl"));
        let bytes = std::fs::read(fixtures.join(fixture)).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        store
            .index_source(&namespace, &path, harness)
            .await
            .unwrap();
        let tx = store.begin(&namespace).await.unwrap();
        let (ledger, index) = tx
            .load_index(&namespace, path.to_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_same_index(&path, harness, &ledger, &index, &mut sqlite);
        tx.commit(&namespace).await.unwrap();
        // Restart and append a complete native line, retaining parser state.
        let line = bytes
            .split_inclusive(|byte| *byte == b'\n')
            .next_back()
            .unwrap();
        std::io::Write::write_all(
            &mut std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            line,
        )
        .unwrap();
        drop(store);
        store = PgProjectionStore::new(connect(dsn).await, namespace.clone())
            .await
            .unwrap();
        store
            .index_source(&namespace, &path, harness)
            .await
            .unwrap();
        let tx = store.begin(&namespace).await.unwrap();
        let (ledger, index) = tx
            .load_index(&namespace, path.to_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_same_index(&path, harness, &ledger, &index, &mut sqlite);
        tx.commit(&namespace).await.unwrap();
    }
}

async fn pg_native_index_lifecycle(dsn: &str) {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let root = super::root();
    let path = super::native(&root.0, "pg-native", &"private native body ".repeat(8192));
    let mut sqlite =
        crate::events::EventCache::open_scoped(&root.0.join("comparison.json")).unwrap();
    let namespace = Namespace::new("native-machine").unwrap();
    let mut store = PgProjectionStore::new(connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let first = store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    let unchanged = store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    assert_eq!(first, unchanged, "idle indexing preserves the generation");
    let name = path.to_str().unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    let (ledger, index) = tx.load_index(&namespace, name).await.unwrap().unwrap();
    assert_eq!(index.events.len(), 1);
    assert_same_index(&path, "claude", &ledger, &index, &mut sqlite);
    let first_hash = tx
        .proof_block(&namespace, name, first, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_hash,
        <[u8; 32]>::from(Sha256::digest(
            &std::fs::read(&path).unwrap()[..crate::source_chunks::SIZE]
        ))
    );
    tx.publish_tag(&namespace, "native-session", name, first)
        .await
        .unwrap();
    tx.commit(&namespace).await.unwrap();
    let appended = format!(
        "{}\n",
        serde_json::json!({"type":"assistant","sessionId":"pg-native","uuid":"answer","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"private answer"}]}})
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(appended.as_bytes())
        .unwrap();
    let next = store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    assert_eq!(first.epoch, next.epoch);
    assert!(next.offset > first.offset);
    let tx = store.begin(&namespace).await.unwrap();
    let (committed, normalized) = tx.load_index(&namespace, name).await.unwrap().unwrap();
    let mut replaced_ledger = committed.clone();
    replaced_ledger.stat.ino += 1;
    let uncommitted = tx
        .write_index(
            &namespace,
            name,
            Some(next),
            &replaced_ledger,
            &normalized,
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(uncommitted.epoch > next.epoch);
    drop(tx);
    let tx = store.begin(&namespace).await.unwrap();
    assert_eq!(tx.ledger(&namespace, name).await.unwrap(), Some(next));
    assert!(
        tx.proof_block(&namespace, name, uncommitted, 0)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        tx.write_index(&namespace, name, Some(first), &committed, &normalized, true)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&namespace).await.unwrap();
    drop(store);
    let mut store = PgProjectionStore::new(connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    let (after, index) = tx.load_index(&namespace, name).await.unwrap().unwrap();
    assert_eq!(index.events.len(), 2);
    assert_same_index(&path, "claude", &after, &index, &mut sqlite);
    assert!(after.offset > ledger.offset);
    assert_eq!(
        tx.check_tag(&namespace, "native-session", name)
            .await
            .unwrap(),
        Some(TagCheck::Advanced)
    );
    assert_eq!(
        tx.proof_block(&namespace, name, first, 0).await.unwrap(),
        Some(first_hash)
    );
    // The old consumed prefix's witness still qualifies those exact bytes.
    assert_eq!(
        first_hash,
        <[u8; 32]>::from(Sha256::digest(
            &std::fs::read(&path).unwrap()[..crate::source_chunks::SIZE]
        ))
    );
    tx.commit(&namespace).await.unwrap();
    let mut raw = connect(dsn).await;
    let raw_tx = raw.transaction().await.unwrap();
    raw_tx
        .query_one(
            "SELECT set_config('semon.namespace',$1,true)",
            &[&namespace.binding()],
        )
        .await
        .unwrap();
    let changed = raw_tx.execute(
        "UPDATE semon_projection.proof_blocks SET digest=decode(repeat('ff',32),'hex') WHERE namespace=$1 AND path=$2 AND ordinal=0",
        &[&namespace.binding(), &name],
    ).await.unwrap();
    assert!(changed > 0);
    raw_tx.commit().await.unwrap();
    let proof_tx = store.begin(&namespace).await.unwrap();
    assert!(
        proof_tx
            .proof_block(&namespace, name, first, 0)
            .await
            .is_err()
    );
    proof_tx.commit(&namespace).await.unwrap();
    let raw_tx = raw.transaction().await.unwrap();
    raw_tx
        .query_one(
            "SELECT set_config('semon.namespace',$1,true)",
            &[&namespace.binding()],
        )
        .await
        .unwrap();
    raw_tx.execute(
        "UPDATE semon_projection.proof_blocks SET digest=$3 WHERE namespace=$1 AND path=$2 AND ordinal=0",
        &[&namespace.binding(), &name, &&first_hash[..]],
    ).await.unwrap();
    raw_tx.commit().await.unwrap();
    let raw_tx = raw.transaction().await.unwrap();
    raw_tx
        .query_one(
            "SELECT set_config('semon.namespace',$1,true)",
            &[&namespace.binding()],
        )
        .await
        .unwrap();
    let stored: i64 = raw_tx
        .query_one(
            "SELECT count(*) FROM semon_projection.records WHERE record LIKE '%private%'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, 0, "normalized rows contain no native bodies");
    raw_tx.rollback().await.unwrap();
    std::fs::write(&path, format!("{}\n", serde_json::json!({"type":"user","sessionId":"pg-native","uuid":"replacement","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"replacement"}}))).unwrap();
    let replaced = store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    assert!(replaced.epoch > next.epoch);
    let tx = store.begin(&namespace).await.unwrap();
    let (replaced_ledger, index) = tx.load_index(&namespace, name).await.unwrap().unwrap();
    assert_eq!(index.events.len(), 1);
    assert_same_index(&path, "claude", &replaced_ledger, &index, &mut sqlite);
    assert!(matches!(
        tx.check_tag(&namespace, "native-session", name)
            .await
            .unwrap(),
        Some(TagCheck::Mismatch(_))
    ));
    assert_eq!(
        tx.proof_block(&namespace, name, first, 0).await.unwrap(),
        Some(first_hash)
    );
    tx.commit(&namespace).await.unwrap();
    // Real RLS, including normalized and proof tables, on a reused connection.
    let mut foreign = PgProjectionStore::new(
        connect(dsn).await,
        Namespace::new("foreign-native").unwrap(),
    )
    .await
    .unwrap();
    let foreign_namespace = Namespace::new("foreign-native").unwrap();
    let tx = foreign.begin(&foreign_namespace).await.unwrap();
    assert!(
        tx.load_index(&foreign_namespace, name)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        tx.proof_block(&foreign_namespace, name, first, 0)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&foreign_namespace).await.unwrap();
    let raw_tx = raw.transaction().await.unwrap();
    raw_tx
        .query_one(
            "SELECT set_config('semon.namespace',$1,true)",
            &[&foreign_namespace.binding()],
        )
        .await
        .unwrap();
    for table in ["files", "records", "proofs", "proof_blocks", "proof_nodes"] {
        let count: i64 = raw_tx
            .query_one(
                &format!("SELECT count(*) FROM semon_projection.{table}"),
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0);
    }
    raw_tx.rollback().await.unwrap();
    let allowed = std::collections::BTreeSet::from([
        "comparison.sqlite3".to_owned(),
        "comparison.sqlite3-wal".to_owned(),
        "comparison.sqlite3-shm".to_owned(),
    ]);
    let actual = std::fs::read_dir(&root.0)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            entry
                .file_type()
                .unwrap()
                .is_file()
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        actual.is_subset(&allowed),
        "only the explicit SQLite comparison may create session files: {actual:?}"
    );
}

/// Runs only in the service-container workflow. Missing DSNs fail explicitly.
#[tokio::test]
async fn pg_projection_rls_epochs_rollback_and_no_sqlite_files() {
    let admin =
        connect(&std::env::var("SEMON_PG_ADMIN_TEST_DSN").expect("CI PostgreSQL admin DSN")).await;
    admin.batch_execute(&schema()).await.unwrap();
    admin.batch_execute("GRANT USAGE ON SCHEMA semon_projection TO semon_app; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA semon_projection TO semon_app; REVOKE INSERT,UPDATE,DELETE ON semon_projection.metadata FROM semon_app").await.unwrap();
    let dsn = std::env::var("SEMON_PG_TEST_DSN").expect("CI restricted PostgreSQL DSN");
    let a = Namespace::new("machine-a").unwrap();
    let b = Namespace::new("machine-b").unwrap();
    let mut store_a = PgProjectionStore::new(connect(&dsn).await, a.clone())
        .await
        .unwrap();
    let mut store_b = PgProjectionStore::new(connect(&dsn).await, b.clone())
        .await
        .unwrap();
    assert!(matches!(
        store_a.begin(&b).await,
        Err(StorageError::WrongNamespace { .. })
    ));
    let tag = ProjectionTag {
        dev: 1,
        ino: 2,
        offset: 10,
        prefix_sha256: [3; 32],
        tail_sha256: [4; 32],
        epoch: 0,
        projector: crate::model::summary::CATALOG_VERSION,
    };
    let start = std::time::Instant::now();
    let tx = store_a.begin(&a).await.unwrap();
    let tag = tx
        .write_ledger(&a, "same/native/path", None, tag, false)
        .await
        .unwrap()
        .unwrap();
    tx.publish_tag(&a, "session", "same/native/path", tag)
        .await
        .unwrap();
    tx.commit(&a).await.unwrap();
    eprintln!(
        "postgres-projection-cold-transaction-us={}",
        start.elapsed().as_micros()
    );
    let tx = store_b.begin(&b).await.unwrap();
    assert_eq!(tx.ledger(&b, "same/native/path").await.unwrap(), None);
    let other = tx
        .write_ledger(
            &b,
            "same/native/path",
            None,
            ProjectionTag {
                prefix_sha256: [9; 32],
                ..tag
            },
            false,
        )
        .await
        .unwrap()
        .unwrap();
    tx.commit(&b).await.unwrap();
    assert_ne!(tag.prefix_sha256, other.prefix_sha256);
    let tx = store_a.begin(&a).await.unwrap();
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Current)
    );
    let next = tx
        .write_ledger(
            &a,
            "same/native/path",
            Some(tag),
            ProjectionTag { offset: 20, ..tag },
            true,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.epoch, tag.epoch);
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Advanced)
    );
    assert!(
        tx.write_ledger(&a, "same/native/path", Some(tag), tag, false)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&a).await.unwrap();
    let tx = store_a.begin(&a).await.unwrap();
    let rewritten = tx
        .write_ledger(
            &a,
            "same/native/path",
            Some(next),
            ProjectionTag { offset: 30, ..next },
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(rewritten.epoch > next.epoch);
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Mismatch("ledger_epoch"))
    );
    // Dropping an uncommitted transaction restores both ledger and epoch.
    drop(tx);
    let tx = store_a.begin(&a).await.unwrap();
    assert_eq!(tx.ledger(&a, "same/native/path").await.unwrap(), Some(next));
    tx.commit(&a).await.unwrap();
    // Test the actual SQL policy directly, without the Rust namespace check.
    let mut raw = connect(&dsn).await;
    let tx = raw.transaction().await.unwrap();
    tx.query_one("SELECT set_config('semon.namespace','machine-a',true)", &[])
        .await
        .unwrap();
    let visible: i64 = tx
        .query_one(
            "SELECT count(*) FROM semon_projection.ledgers WHERE namespace='machine-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 0);
    assert_eq!(
        tx.execute(
            "UPDATE semon_projection.ledgers SET consumed=0 WHERE namespace='machine-b'",
            &[]
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        tx.execute(
            "DELETE FROM semon_projection.ledgers WHERE namespace='machine-b'",
            &[]
        )
        .await
        .unwrap(),
        0
    );
    let refused = tx
        .execute(
            "INSERT INTO semon_projection.namespaces(namespace) VALUES('foreign')",
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.code(),
        Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
    );
    tx.rollback().await.unwrap();
    let leaked: i64 = raw
        .query_one("SELECT count(*) FROM semon_projection.ledgers", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        leaked, 0,
        "transaction-local namespace must not leak on reuse"
    );
    assert!(
        PgProjectionStore::new(
            connect(&std::env::var("SEMON_PG_ADMIN_TEST_DSN").unwrap()).await,
            a.clone()
        )
        .await
        .is_err()
    );
    let tx = store_a.begin(&a).await.unwrap();
    assert!(
        tx.write_ledger(
            &a,
            "wrong-version",
            None,
            ProjectionTag {
                projector: 0,
                ..tag
            },
            false
        )
        .await
        .is_err()
    );
    tx.commit(&a).await.unwrap();
    pg_native_index_lifecycle(&dsn).await;
    pg_sanitized_native_indexes(&dsn).await;
    catalog_tests::lifecycle(&dsn).await;
    producer_tests::lifecycle(&dsn).await;
    raw_tests::lifecycle(&dsn).await;
    read_tests::filters_and_continuation(&dsn).await;
    let runtime_dsn = dsn.clone();
    let runtime_probe = std::thread::spawn(move || runtime_tests::lifecycle(&runtime_dsn));
    while !runtime_probe.is_finished() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    runtime_probe.join().unwrap();
    // Retained normalized indexes are strict custody and obey SQL RLS even
    // when the application bypasses the Rust namespace capability.
    let tx = raw.transaction().await.unwrap();
    tx.query_one("SELECT set_config('semon.namespace','foreign-retention',true)", &[]).await.unwrap();
    assert_eq!(tx.query_one("SELECT count(*) FROM semon_projection.retained_indexes", &[]).await.unwrap().get::<_, i64>(0), 0);
    assert_eq!(tx.execute("DELETE FROM semon_projection.retained_indexes WHERE namespace='injected-runtime'", &[]).await.unwrap(), 0);
    assert!(tx.execute("INSERT INTO semon_projection.retained_indexes(namespace,path,ledger,index_digest,projection) VALUES('injected-runtime','foreign','{}',decode(repeat('00',32),'hex'),'{}')", &[]).await.is_err());
    tx.rollback().await.unwrap();
    admin.batch_execute("UPDATE semon_projection.units SET projector=0; UPDATE semon_projection.ledgers SET projector=0").await.unwrap();
    let tx = store_a.begin(&a).await.unwrap();
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Mismatch("projector_version"))
    );
    tx.commit(&a).await.unwrap();
    admin
        .execute(
            "UPDATE semon_projection.metadata SET stamp='unsupported'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        store_a.begin(&a).await,
        Err(StorageError::Unsupported(_))
    ));
    let root = std::env::var("SEMON_PG_EMPTY_ROOT").expect("CI empty working directory");
    assert!(
        std::fs::read_dir(root).unwrap().next().is_none(),
        "projection API must create no local session files"
    );
}
