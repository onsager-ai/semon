use super::*;
use crate::storage::transfer::reported::tests::{root, seed_catalog, sqlite, stage};

#[test]
fn current_transfer225_unsupported_catalog_custody_preserves_capture_cursor_and_row() {
    let root = root();
    let source = sqlite(&root, "unsupported-custody-source");
    let _ = seed_catalog(&source, &root);
    let length = catalog_rows::MAX_CATALOG_ROW_BYTES as i64 + 1;
    super::super::fixture_transaction(&source, |db| {
        db.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(?1,'history_original_header:unsupported',CAST(zeroblob(?2) AS TEXT))",
            params![source.namespace.as_str(), length],
        ).unwrap();
        Ok(())
    }).unwrap();
    let db = rusqlite::Connection::open(source.backend.sqlite_database().unwrap()).unwrap();
    let operation = [182; 32];
    let cursor_key = transfer::key(&operation, "capture");
    let mut previous: Option<String> = None;
    loop {
        match source.transfer_inbox().capture(&operation) {
            Ok(transfer::CaptureProgress::Capturing { .. }) => {
                previous = Some(
                    db.query_row(
                        "SELECT value FROM meta WHERE namespace=?1 AND key=?2",
                        params![source.namespace.as_str(), cursor_key],
                        |row| row.get(0),
                    )
                    .unwrap(),
                );
            }
            Err(StorageError::HistoryRetentionBounds) => break,
            other => panic!("unsupported custody must stop explicitly: {other:?}"),
        }
    }
    let unchanged: String = db
        .query_row(
            "SELECT value FROM meta WHERE namespace=?1 AND key=?2",
            params![source.namespace.as_str(), cursor_key],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(Some(unchanged), previous);
    let retained: i64 = db.query_row(
        "SELECT octet_length(value) FROM meta WHERE namespace=?1 AND key='history_original_header:unsupported'",
        [source.namespace.as_str()], |row| row.get(0),
    ).unwrap();
    assert_eq!(retained, length);
    assert_eq!(
        source.transfer_inbox().capture(&operation),
        Err(StorageError::HistoryRetentionBounds)
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_sqlite_real_large_presentation_reverse_without_raw() {
    use crate::storage::transfer::catalog_rows::tests::assert_provider_transcript;
    use crate::storage::transfer::reported::tests::{
        apply,
        selected_receipts::{AuthorizedOriginals, originals},
    };
    let root = root();
    let source = sqlite(&root, "real-large-source");
    let target = sqlite(&root, "real-large-target");
    let reverse = sqlite(&root, "real-large-reverse");
    let (options, key, id) = seed_large_presentation(&source, &root);
    let mut host = AuthorizedOriginals::default();
    originals(&source, &options, &mut host);
    assert_provider_transcript(&source, &options, &host, &key);
    let manifest = stage(&source, &target, &[178; 32]);
    let mut presentation_parts = 0;
    for sequence in 1..=source
        .transfer_inbox()
        .checkpoint(&manifest)
        .unwrap()
        .sequence
    {
        let page = source
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap();
        assert!(page.encoded().unwrap().len() <= transfer::MAX_PAGE_BYTES);
        for record in page.records {
            if let TransferRecord::CatalogRow {
                kind: CatalogRowKind::Presentation,
                key,
                parts,
                ..
            } = record
                && key[0].as_str() == Some(id.as_str())
            {
                assert!(parts > 1);
                presentation_parts += 1;
            }
        }
    }
    assert!(presentation_parts > 1);
    std::fs::remove_dir_all(&options.claude_home).unwrap();
    apply(&target, &manifest);
    target
        .transfer_inbox()
        .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
        .unwrap();
    assert_provider_transcript(&target, &options, &host, &key);
    let reverse_manifest = stage(&target, &reverse, &[179; 32]);
    apply(&reverse, &reverse_manifest);
    reverse
        .transfer_inbox()
        .admit_catalog_from(&target.transfer_inbox(), &reverse_manifest, &options)
        .unwrap();
    assert_provider_transcript(&reverse, &options, &host, &key);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_sqlite_large_catalog_restart_replay_and_final_rollback() {
    let root = root();
    let source = sqlite(&root, "large-catalog-source");
    let target = sqlite(&root, "large-catalog-target");
    let _ = seed_catalog(&source, &root);
    let value = serde_json::to_string(&"\0\\\"界".repeat(160_000)).unwrap();
    assert!(value.len() > transfer::MAX_PAGE_BYTES);
    // Payload-only custody fixtures still use the adapter's restored owner and
    // ordinary guarded transaction; no publication or proof authority is minted.
    let escaped = serde_json::to_string(&"\\".repeat(250_000)).unwrap();
    assert!(escaped.len() < transfer::MAX_PAGE_BYTES);
    super::super::fixture_transaction(&source, |db| {
        db.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(?1,'history_transfer:large-row',?2)",
            params![source.namespace.as_str(), value],
        )
        .unwrap();
        // Its raw cells fit one page, but escaped JSON and wire byte arrays span
        // several pages. The following row must wait for this row's final part.
        for (key, metadata) in [("00-escaped", escaped.as_str()), ("01-following", "null")] {
            db.execute(
                "INSERT INTO meta(namespace,key,value) VALUES(?1,?2,?3)",
                params![
                    source.namespace.as_str(),
                    format!("history_transfer:{key}"),
                    metadata
                ],
            )
            .unwrap();
        }
        Ok(())
    })
    .unwrap();
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
        )
        .optional()
        .unwrap()
    };
    let mut saw_partial = false;
    let mut saw_final = false;
    let mut escaped_partial_pages = 0;
    for sequence in 1..=count {
        let page = source_inbox.staged_page(&manifest, sequence).unwrap();
        assert!(page.encoded().unwrap().len() <= transfer::MAX_PAGE_BYTES);
        let fragments = page
            .records
            .iter()
            .filter_map(|record| match record {
                TransferRecord::CatalogRow {
                    kind: CatalogRowKind::HistoryProgress,
                    part,
                    parts,
                    ..
                } => Some((*part, *parts)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let final_row = fragments.iter().any(|(part, parts)| part + 1 == *parts);
        let final_large_row = page.records.iter().any(|record| matches!(record,
            TransferRecord::CatalogRow { kind: CatalogRowKind::HistoryProgress, key, part, parts, .. }
                if key[0].as_str() == Some("large-row") && part + 1 == *parts
        ));
        let escaped_partial = page.records.iter().any(|record| matches!(record,
            TransferRecord::CatalogRow { kind: CatalogRowKind::HistoryProgress, key, part, parts, .. }
                if key[0].as_str() == Some("00-escaped") && part + 1 < *parts
        ));
        if final_large_row {
            let receipt = transfer::key(&manifest.identity(), &format!("applied:{sequence}"));
            db.execute_batch(&format!(
                "CREATE TRIGGER fail_catalog_final_receipt BEFORE INSERT ON meta WHEN NEW.key='{receipt}' BEGIN SELECT RAISE(ABORT,'injected final fragment receipt failure'); END"
            )).unwrap();
            assert!(
                target
                    .transfer_inbox()
                    .apply_page(&manifest, sequence)
                    .is_err()
            );
            assert!(stored(&db).is_none());
            let receipt_count: i64 = db
                .query_row(
                    "SELECT count(*) FROM meta WHERE namespace=?1 AND key=?2",
                    params![target.namespace.as_str(), receipt],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(receipt_count, 0);
            db.execute_batch("DROP TRIGGER fail_catalog_final_receipt")
                .unwrap();
            saw_final = true;
        }
        let checkpoint = target
            .transfer_inbox()
            .apply_page(&manifest, sequence)
            .unwrap();
        // New handles replay exact page receipts, including after restart and
        // after the final logical row was already inserted.
        assert_eq!(
            target
                .transfer_inbox()
                .apply_page(&manifest, sequence)
                .unwrap(),
            checkpoint
        );
        if !fragments.is_empty() && !final_row {
            assert!(stored(&db).is_none());
            saw_partial = true;
        }
        if final_large_row {
            assert_eq!(stored(&db).as_deref(), Some(value.as_str()));
        }
        escaped_partial_pages += usize::from(escaped_partial);
    }
    assert!(saw_partial && saw_final);
    assert!(escaped_partial_pages > 1);
    let stored_escaped: String = db
        .query_row(
            "SELECT value FROM meta WHERE namespace=?1 AND key='history_transfer:00-escaped'",
            [target.namespace.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_escaped, escaped);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn seed_large_presentation(
    source: &StoreConfig,
    root: &std::path::Path,
) -> (crate::Options, String, String) {
    use crate::storage::native_events::NativeEventStore;
    use crate::storage::transfer::reported::tests::{
        begin_active_catalog, cache, finish_catalog, seed_catalog,
    };
    let (options, _) = seed_catalog(source, root);
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/sanitized/catalog.jsonl".into(),
    };
    let path = input.full_path(&options);
    // Claude's native IDs are strings (the ordinary fixture already uses u0,
    // rather than UUIDv4). Quotes stress the additional context JSON nesting
    // without adding Raw body bytes to the immutable presentation.
    let ids = (0..80)
        .map(|i| format!("u{i}-{}", "\"".repeat(1152)))
        .collect::<Vec<_>>();
    let body = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            format!(
                "{}\n",
                serde_json::json!({
                    "type":"user", "sessionId":"catalog", "uuid":id,
                    "parentUuid":i.checked_sub(1).map(|previous| &ids[previous]),
                    "timestamp":"2026-10-01T00:00:00Z", "cwd":"/sanitized",
                    "message":{"role":"user", "content":format!("sanitized {i}")}
                })
            )
        })
        .collect::<String>();
    std::fs::write(&path, body).unwrap();
    let mut cache = cache(source);
    cache.ingest(&"9".repeat(64), &path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
    begin_active_catalog(
        &mut cache,
        &[path.to_str().unwrap().to_owned()],
        "transfer-large-real-model",
        true,
    );
    let rows = finish_catalog(&mut cache, &options, std::slice::from_ref(&input));
    assert_eq!(rows.len(), 1);
    let db = rusqlite::Connection::open(source.backend.sqlite_database().unwrap()).unwrap();
    let (id, metadata, slots, presentation): (String, String, String, String) = db.query_row(
        "SELECT g.id,g.metadata,g.slots,p.metadata FROM storage_proof_refs f JOIN storage_proof_generations g USING(namespace,id,session_key) JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current'",
        rusqlite::params![source.namespace.as_str(), rows[0].key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    eprintln!(
        "large_catalog_real metadata={} slots={} presentation={}",
        metadata.len(),
        slots.len(),
        presentation.len()
    );
    assert!(metadata.len() <= transfer::MAX_PAGE_BYTES);
    assert!(slots.len() <= transfer::MAX_PAGE_BYTES);
    assert!(presentation.len() > transfer::MAX_PAGE_BYTES);
    assert!(presentation.len() <= 8 * transfer::MAX_PAGE_BYTES);
    (options, rows[0].key.clone(), id)
}
