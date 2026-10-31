use super::*;
use crate::storage::transfer::reported::tests::{root, seed_catalog, sqlite, stage};

#[test]
fn current_transfer225_sqlite_large_catalog_restart_replay_and_final_rollback() {
    let root = root();
    let source = sqlite(&root, "large-catalog-source");
    let target = sqlite(&root, "large-catalog-target");
    let _ = seed_catalog(&source, &root);
    let value = serde_json::to_string(&"\0\\\"界".repeat(160_000)).unwrap();
    assert!(value.len() > transfer::MAX_PAGE_BYTES);
    let source_path = match &source.backend {
        crate::storage::StoreBackend::Sqlite { database } => database,
        #[cfg(feature = "postgres")]
        _ => unreachable!(),
    };
    let db = rusqlite::Connection::open(source_path).unwrap();
    db.execute(
        "INSERT INTO meta(namespace,key,value) VALUES(?1,'history_transfer:large-row',?2)",
        params![source.namespace.as_str(), value],
    ).unwrap();
    drop(db);
    let manifest = stage(&source, &target, &[177; 32]);
    let source_inbox = source.transfer_inbox();
    let count = source_inbox.checkpoint(&manifest).unwrap().sequence;
    let target_path = match &target.backend {
        crate::storage::StoreBackend::Sqlite { database } => database,
        #[cfg(feature = "postgres")]
        _ => unreachable!(),
    };
    let db = rusqlite::Connection::open(target_path).unwrap();
    let stored = |db: &rusqlite::Connection| {
        db.query_row(
            "SELECT value FROM meta WHERE namespace=?1 AND key='history_transfer:large-row'",
            [target.namespace.as_str()],
            |row| row.get::<_, String>(0),
        ).optional().unwrap()
    };
    let mut saw_partial = false;
    let mut saw_final = false;
    for sequence in 1..=count {
        let page = source_inbox.staged_page(&manifest, sequence).unwrap();
        assert!(page.encoded().unwrap().len() <= transfer::MAX_PAGE_BYTES);
        let fragments = page.records.iter().filter_map(|record| match record {
            TransferRecord::CatalogRow { kind: CatalogRowKind::HistoryProgress, part, parts, .. } => Some((*part, *parts)),
            _ => None,
        }).collect::<Vec<_>>();
        let final_row = fragments.iter().any(|(part, parts)| part + 1 == *parts);
        if final_row {
            let receipt = transfer::key(&manifest.identity(), &format!("applied:{sequence}"));
            db.execute_batch(&format!(
                "CREATE TRIGGER fail_catalog_final_receipt BEFORE INSERT ON meta WHEN NEW.key='{receipt}' BEGIN SELECT RAISE(ABORT,'injected final fragment receipt failure'); END"
            )).unwrap();
            assert!(target.transfer_inbox().apply_page(&manifest, sequence).is_err());
            assert!(stored(&db).is_none());
            let receipt_count: i64 = db.query_row(
                "SELECT count(*) FROM meta WHERE namespace=?1 AND key=?2",
                params![target.namespace.as_str(), receipt],
                |row| row.get(0),
            ).unwrap();
            assert_eq!(receipt_count, 0);
            db.execute_batch("DROP TRIGGER fail_catalog_final_receipt").unwrap();
            saw_final = true;
        }
        let checkpoint = target.transfer_inbox().apply_page(&manifest, sequence).unwrap();
        // New handles replay exact page receipts, including after restart and
        // after the final logical row was already inserted.
        assert_eq!(target.transfer_inbox().apply_page(&manifest, sequence).unwrap(), checkpoint);
        if !fragments.is_empty() && !final_row {
            assert!(stored(&db).is_none());
            saw_partial = true;
        }
        if final_row {
            assert_eq!(stored(&db).as_deref(), Some(value.as_str()));
        }
    }
    assert!(saw_partial && saw_final);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
