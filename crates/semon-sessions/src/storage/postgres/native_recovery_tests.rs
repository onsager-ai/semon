use super::*;
use crate::{events::EventCache, storage::source_observations::{SourceIntent, SourceMutationReceipt, SourceObservationStore, SourceOperation}};

fn intent(namespace: &str, cursor: Option<String>) -> SourceIntent {
    SourceIntent::Recover {
        operation: SourceOperation::capture(namespace, "recover", &serde_json::to_vec(&cursor).unwrap()),
        cursor,
    }
}

#[test]
fn pg_native126_recovery136_sqlite_first_exact_pages_restart_and_faults() {
    const NAME: &str = "storage::postgres::native_recovery_tests::pg_native126_recovery136_sqlite_first_exact_pages_restart_and_faults";
    if std::env::var_os("SEMON_RECOVERY136_CHILD").is_none() {
        assert!(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", NAME, "--nocapture"]).env("SEMON_RECOVERY136_CHILD", "1").status().unwrap().success());
        return;
    }
    let root = std::env::temp_dir().join(format!("native-recovery136-{}", std::process::id()));
    let options = crate::Options {
        claude_home: root.join("claude"), claude_json: root.join("claude.json"),
        codex_home: root.join("codex"), copilot_home: root.join("copilot"),
        proc_root: root.join("proc"), cache: root.join("index.json"), all: true,
        since: std::time::Duration::from_secs(86400), session: None, facts: None, scan_window: false,
    };
    std::fs::create_dir_all(options.claude_home.join("projects/project")).unwrap();
    std::fs::create_dir_all(&options.proc_root).unwrap();
    std::fs::write(options.proc_root.join("locks"), "").unwrap();
    for i in 0..65 {
        let body = serde_json::json!({"type":"user","sessionId":format!("recovery136-{i:03}"),"uuid":format!("u-{i}"),"cwd":"/sanitized","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized fixture"}});
        std::fs::write(options.claude_home.join(format!("projects/project/{i:03}.jsonl")), format!("{body}\n")).unwrap();
    }
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut sqlite = EventCache::open_scoped(&options.cache).unwrap();
    let prepared = crate::model::prepare_sources(&options, &mut sqlite, &inputs, 1790812802000).unwrap();
    drop(sqlite);
    let mut db = rusqlite::Connection::open(EventCache::path(&options.cache)).unwrap();
    let tx = db.transaction().unwrap();
    let writer = crate::storage::sqlite::publication::CurrentWriter::unbound(&tx);
    use crate::storage::catalog::CurrentPublicationStore;
    for row in &prepared.rows {
        writer.current(row, &serde_json::to_string(row).unwrap()).unwrap();
        crate::storage::catalog::publish_current_sources(&writer, row).unwrap();
    }
    tx.commit().unwrap();
    crate::events::store::begin_catalog_recovery(&options, "recovery136 fixture").unwrap();
    let mut sqlite = EventCache::open_scoped(&options.cache).unwrap();
    let mut outcomes = Vec::new();
    for _ in 0..3 { outcomes.push(sqlite.discover_recovery_sources().unwrap()); }
    assert_eq!(outcomes, vec![false, false, true]);
    let expected = db.prepare("SELECT source_path FROM catalog_backfill_sources ORDER BY source_path").unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(expected.len(), 65);
    drop(sqlite); drop(db);
    let _guard = super::native_ingest_tests::Guard::new();
    let (pool, _) = tests::fixture(|_| {});
    let namespace = tests::ns("recovery136\0logical");
    let control = PgCancellation::default();
    let generation = "6".repeat(64);
    let mut pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    pool.transaction(&namespace, txn::TxnKind::Write, &control, |tx| {
        // Physical current-generation fixture only; this is not a Catalog
        // admission/publication test. The rows came from the real parser above.
        for rows in prepared.rows.chunks(64) {
            catalog_effects::stage(tx, &generation, &crate::slot_projection::Publication { rows, transcripts: None })?;
        }
        tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&namespace.as_str().as_bytes(), &generation]).map_err(sqlstate::error)?;
        native_keys::put_meta(tx, b"catalog_recovery_after", b"")
    }).unwrap();
    let mut journal = PgSourceJournal { pool: pool.clone(), namespace: namespace.clone(), control: control.clone() };
    let first = intent(namespace.as_str(), Some(String::new()));
    journal.persist_intent(&first).unwrap();
    let baseline = native_ingest_tests::batch_snapshot(&pool, &namespace, &control);
    native_recovery::BOUNDARIES.with(|v| v.set(0));
    assert_eq!(journal.recover(first.operation(), Some("")).unwrap(), (SourceMutationReceipt::Applied, false));
    let boundaries = native_recovery::BOUNDARIES.with(|v| v.get());
    assert!(boundaries > 64);
    let cursor = journal.recovery_cursor().unwrap();
    let second = intent(namespace.as_str(), cursor.clone());
    journal.persist_intent(&second).unwrap();
    let before = native_ingest_tests::batch_snapshot(&pool, &namespace, &control);
    for point in 1..=5 {
        native_recovery::FAIL_AFTER.with(|v| v.set(Some(point)));
        assert!(journal.recover(second.operation(), cursor.as_deref()).is_err());
        assert_eq!(native_ingest_tests::batch_snapshot(&pool, &namespace, &control), before);
    }
    native_recovery::FAIL_AFTER.with(|v| v.set(None));
    tests::LOSE_ACK.with(|v| v.set(true));
    assert_eq!(journal.recover(second.operation(), cursor.as_deref()), Err(StorageError::CommitUncertain));
    drop(journal); drop(pg);
    pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert!(pg.discover_recovery_sources().unwrap());
    let mut journal = PgSourceJournal { pool: pool.clone(), namespace: namespace.clone(), control: control.clone() };
    assert_eq!(journal.recover(first.operation(), Some("")).unwrap(), (SourceMutationReceipt::AlreadyApplied, false));
    assert_eq!(journal.recover(second.operation(), cursor.as_deref()).unwrap(), (SourceMutationReceipt::AlreadyApplied, false));
    let stale = intent(namespace.as_str(), Some(String::new()));
    journal.persist_intent(&stale).unwrap();
    assert_eq!(journal.recover(stale.operation(), Some("")).unwrap(), (SourceMutationReceipt::Conflict, false));
    let actual = pool.transaction(&namespace, txn::TxnKind::ReadSnapshot, &control, |tx| {
        let rows = tx.driver.query("SELECT source_path FROM semon_pg_source_members WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 ORDER BY source_path", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
        Ok(rows.iter().map(|r| String::from_utf8(r.get(0)).unwrap()).collect::<Vec<_>>())
    }).unwrap();
    assert_eq!(actual, expected);
    assert_ne!(native_ingest_tests::batch_snapshot(&pool, &namespace, &control), baseline);
}
