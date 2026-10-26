use super::*;
use crate::storage::StorageError;
use crate::{
    events::EventCache,
    storage::source_observations::{
        SourceIntent, SourceMutationReceipt, SourceObservationStore, SourceOperation,
    },
};

fn intent(namespace: &str, cursor: Option<String>) -> SourceIntent {
    SourceIntent::Recover {
        operation: SourceOperation::capture(
            namespace,
            "recover",
            &serde_json::to_vec(&cursor).unwrap(),
        ),
        cursor,
    }
}

#[test]
fn pg_native126_recovery136_sqlite_first_exact_pages_restart_and_faults() {
    const NAME: &str = "storage::postgres::native_recovery_tests::pg_native126_recovery136_sqlite_first_exact_pages_restart_and_faults";
    if std::env::var_os("SEMON_RECOVERY136_CHILD").is_none() {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env("SEMON_RECOVERY136_CHILD", "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    let root = std::env::temp_dir().join(format!("native-recovery136-{}", std::process::id()));
    let namespace = tests::ns("recovery136\0logical");
    let options = crate::Options {
        source: crate::storage::HostSource::new(namespace.clone(), "3".repeat(64)).unwrap(),
        claude_home: root.join("claude"),
        claude_json: root.join("claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join("index.json"),
        all: true,
        since: std::time::Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    std::fs::create_dir_all(options.claude_home.join("projects/project")).unwrap();
    std::fs::create_dir_all(&options.proc_root).unwrap();
    std::fs::write(options.proc_root.join("locks"), "").unwrap();
    for i in 0..65 {
        let body = serde_json::json!({"type":"user","sessionId":format!("recovery136-{i:03}"),"uuid":format!("u-{i}"),"cwd":"/sanitized","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized fixture"}});
        std::fs::write(
            options
                .claude_home
                .join(format!("projects/project/{i:03}.jsonl")),
            format!("{body}\n"),
        )
        .unwrap();
    }
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut sqlite = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let mut prepared_rows = Vec::new();
    for batch in inputs.chunks(64) {
        prepared_rows.extend(
            crate::model::prepare_sources(&options, &mut sqlite, batch, 1790812802000)
                .unwrap()
                .rows,
        );
    }
    drop(sqlite);
    let config = options.source.config(&options.cache).unwrap();
    let admission = crate::storage::sqlite::semantic::AdmittedSource::native(
        crate::events::store::configured::NativeOwnership::admit(&config).unwrap(),
    );
    let mut db = rusqlite::Connection::open(EventCache::path(&options.cache)).unwrap();
    let tx = db.transaction().unwrap();
    let writer = crate::storage::sqlite::publication::CurrentWriter::admitted(&tx, &admission)
        .unwrap();
    use crate::storage::catalog::CurrentPublicationStore;
    for row in &prepared_rows {
        writer
            .current(row, &serde_json::to_string(row).unwrap())
            .unwrap();
        crate::storage::catalog::publish_current_sources(&writer, row).unwrap();
    }
    tx.commit().unwrap();
    crate::events::store::begin_catalog_recovery(&options, "recovery136 fixture").unwrap();
    let mut sqlite = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let mut outcomes = Vec::new();
    for _ in 0..3 {
        outcomes.push(sqlite.discover_recovery_sources().unwrap());
    }
    assert_eq!(outcomes, vec![false, false, true]);
    let expected = db
        .prepare("SELECT source_path FROM catalog_backfill_sources ORDER BY source_path")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(expected.len(), 65);
    drop(sqlite);
    drop(db);
    let _guard = super::native_ingest_tests::Guard::new();
    let (pool, _) = tests::fixture(|_| {});
    let control = PgCancellation::default();
    let generation = "6".repeat(64);
    let mut pg =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    pool.transaction(&namespace, txn::TxnKind::Write, &control, |tx| {
        // Physical recovery fixture, not a publication: 65 Sources exercise
        // two recovery pages. The admitted publisher is bounded to 64 Sources;
        // repeating it would conflict with immutable generation search counts.
        // Preserve the real parser/model rows and seed only recovery's inputs.
        let ns = namespace.as_str().as_bytes();
        for row in &prepared_rows {
            let metadata = serde_json::to_vec(row).map_err(|_| StorageError::Unavailable)?;
            tx.driver.execute("INSERT INTO semon_pg_native_catalog_rows(namespace,generation,key_digest,session_key,metadata,last_ms,harness,repo,model) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,$6,$7,$8)", &[&ns, &generation.as_bytes(), &row.key.as_bytes(), &metadata, &row.last.unwrap_or(0), &row.harness.as_bytes(), &row.repo.as_deref().unwrap_or("").as_bytes(), &row.model.as_bytes()]).map_err(sqlstate::error)?;
            for source in &row.sources {
                let path = source.path.to_str().ok_or(StorageError::InvalidArguments)?.as_bytes();
                let metadata = serde_json::to_vec(source).map_err(|_| StorageError::Unavailable)?;
                tx.driver.execute("INSERT INTO semon_pg_native_catalog_sources(namespace,generation,key_digest,session_key,path_digest,source_path,metadata,native_digest) VALUES($1,$2,sha256($3::bytea),$3,sha256($4::bytea),$4,$5,sha256($6::bytea))", &[&ns, &generation.as_bytes(), &row.key.as_bytes(), &path, &metadata, &source.native_id.as_bytes()]).map_err(sqlstate::error)?;
            }
        }
        tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&namespace.as_str().as_bytes(), &generation]).map_err(sqlstate::error)?;
        native_keys::put_meta(tx, b"catalog_recovery_after", b"")
    }).unwrap();
    let mut journal = PgSourceJournal {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
    };
    let first = intent(namespace.as_str(), Some(String::new()));
    journal.persist_intent(&first).unwrap();
    let baseline = native_ingest_tests::batch_snapshot(&pool, &namespace, &control);
    native_recovery::BOUNDARIES.with(|v| v.set(0));
    assert_eq!(
        journal.recover(first.operation(), Some("")).unwrap(),
        (SourceMutationReceipt::Applied, false)
    );
    let boundaries = native_recovery::BOUNDARIES.with(|v| v.get());
    assert!(boundaries > 64);
    let cursor = journal.recovery_cursor().unwrap();
    let second = intent(namespace.as_str(), cursor.clone());
    journal.persist_intent(&second).unwrap();
    let before = native_ingest_tests::batch_snapshot(&pool, &namespace, &control);
    for point in 1..=5 {
        native_recovery::FAIL_AFTER.with(|v| v.set(Some(point)));
        assert!(
            journal
                .recover(second.operation(), cursor.as_deref())
                .is_err()
        );
        assert_eq!(
            native_ingest_tests::batch_snapshot(&pool, &namespace, &control),
            before
        );
    }
    native_recovery::FAIL_AFTER.with(|v| v.set(None));
    tests::LOSE_ACK.with(|v| v.set(true));
    assert_eq!(
        journal.recover(second.operation(), cursor.as_deref()),
        Err(StorageError::CommitUncertain)
    );
    drop(journal);
    drop(pg);
    pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert!(pg.discover_recovery_sources().unwrap());
    let mut journal = PgSourceJournal {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
    };
    assert_eq!(
        journal.recover(first.operation(), Some("")).unwrap(),
        (SourceMutationReceipt::AlreadyApplied, false)
    );
    assert_eq!(
        journal
            .recover(second.operation(), cursor.as_deref())
            .unwrap(),
        (SourceMutationReceipt::AlreadyApplied, false)
    );
    let stale = intent(namespace.as_str(), Some(String::new()));
    journal.persist_intent(&stale).unwrap();
    assert_eq!(
        journal.recover(stale.operation(), Some("")).unwrap(),
        (SourceMutationReceipt::Conflict, false)
    );
    let actual = pool.transaction(&namespace, txn::TxnKind::ReadSnapshot, &control, |tx| {
        let rows = tx.driver.query("SELECT source_path FROM semon_pg_source_members WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 ORDER BY source_path", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
        Ok(rows.iter().map(|r| String::from_utf8(r.get(0)).unwrap()).collect::<Vec<_>>())
    }).unwrap();
    assert_eq!(actual, expected);
    assert_ne!(
        native_ingest_tests::batch_snapshot(&pool, &namespace, &control),
        baseline
    );
    // A partial publication over the same >64 owner Current carries every
    // unmentioned owner byte-identical by bounded pages. An unmentioned owner
    // of a claimed Source is connected and refuses; its transaction rolls back.
    let successor = "7".repeat(64);
    let mentioned = vec![{
        use sha2::Digest;
        sha2::Sha256::digest(b"absent mentioned owner").to_vec()
    }];
    let claimed = prepared_rows[1].sources[0]
        .path
        .to_str()
        .unwrap()
        .as_bytes()
        .to_vec();
    pool.transaction(&namespace, txn::TxnKind::Write, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT namespace,generation,path_digest,source_path,$3,0,false FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 ON CONFLICT DO NOTHING", &[&ns, &generation.as_bytes(), &"8".repeat(64).as_bytes()]).map_err(sqlstate::error)?;
        Ok(())
    })
    .unwrap();
    let refused = pool.transaction(&namespace, txn::TxnKind::Write, &control, |tx| {
        super::catalog_effects::carry::unmentioned(
            tx,
            &successor,
            &generation,
            &mentioned,
            std::slice::from_ref(&claimed),
        )
    });
    assert!(matches!(
        refused,
        Err(StorageError::NamespaceUpgradePending)
    ));
    let carried = pool.transaction(&namespace, txn::TxnKind::Write, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        let carried = super::catalog_effects::carry::unmentioned(
            tx,
            &successor,
            &generation,
            &mentioned,
            &[],
        )?;
        let mut differences = Vec::new();
        for (table, columns) in [
            (
                "catalog_rows",
                "key_digest,session_key,metadata,last_ms,harness,repo,model",
            ),
            (
                "catalog_sources",
                "key_digest,session_key,path_digest,source_path,metadata,native_digest",
            ),
            (
                "catalog_proofs",
                "path_digest,source_path,page_generation,usage_generation,usage_ready",
            ),
        ] {
            let count: i64 = tx.driver.query_one(&format!("SELECT count(*) FROM (((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3)) UNION ALL ((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2))) d"), &[&ns, &generation.as_bytes(), &successor.as_bytes()]).map_err(sqlstate::error)?.get(0);
            differences.push((table, count));
        }
        Ok((carried, differences))
    })
    .unwrap();
    assert_eq!(carried.0, prepared_rows.len() as u64);
    assert!(carried.0 > 64);
    assert_eq!(
        carried.1,
        vec![
            ("catalog_rows", 0),
            ("catalog_sources", 0),
            ("catalog_proofs", 0),
        ]
    );
}
