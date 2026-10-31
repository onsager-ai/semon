use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(crate) struct MemoryInbox(BTreeMap<String, Vec<u8>>);

impl InboxTransaction for MemoryInbox {
    fn get(&mut self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, StorageError> {
        match self.0.get(key) {
            Some(value) if value.len() > limit => Err(StorageError::ScopeChanged),
            value => Ok(value.cloned()),
        }
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.0.insert(key.into(), value.to_vec());
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), StorageError> {
        self.0.remove(key);
        Ok(())
    }
}

pub(crate) fn seed_large_presentation(
    source: &StoreConfig,
    root: &std::path::Path,
) -> (crate::Options, String, String) {
    use crate::storage::transfer::reported::tests::{cache, finish_catalog, seed_catalog};
    let (options, _) = seed_catalog(source, root);
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/sanitized/catalog.jsonl".into(),
    };
    let path = input.full_path(&options);
    // Claude's native IDs are strings (the ordinary fixture already uses u0,
    // rather than UUIDv4). Quotes stress the additional context JSON nesting
    // without adding Raw body bytes to the immutable presentation.
    let ids = (0..80).map(|i| format!("u{i}-{}", "\"".repeat(1152))).collect::<Vec<_>>();
    let body = ids.iter().enumerate().map(|(i, id)| format!("{}\n", serde_json::json!({
        "type":"user", "sessionId":"catalog", "uuid":id,
        "parentUuid":i.checked_sub(1).map(|previous| &ids[previous]),
        "timestamp":"2026-10-01T00:00:00Z", "cwd":"/sanitized",
        "message":{"role":"user", "content":format!("sanitized {i}")}
    }))).collect::<String>();
    std::fs::write(&path, body).unwrap();
    let mut cache = cache(source);
    cache.ingest(&"9".repeat(64), &path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
    assert_eq!(cache.begin_semantic_stage_with_journal(
        &[path.to_str().unwrap().to_owned()], "transfer-large-real-model", true,
    ).unwrap(), crate::events::Outcome::Written);
    let rows = finish_catalog(&mut cache, &options, std::slice::from_ref(&input));
    assert_eq!(rows.len(), 1);
    let db = rusqlite::Connection::open(source.backend.sqlite_database().unwrap()).unwrap();
    let (id, metadata, slots, presentation): (String, String, String, String) = db.query_row(
        "SELECT g.id,g.metadata,g.slots,p.metadata FROM storage_proof_refs f JOIN storage_proof_generations g USING(namespace,id,session_key) JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current'",
        rusqlite::params![source.namespace.as_str(), rows[0].key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    eprintln!("large_catalog_real metadata={} slots={} presentation={}", metadata.len(), slots.len(), presentation.len());
    assert!(metadata.len() <= MAX_PAGE_BYTES);
    assert!(slots.len() <= MAX_PAGE_BYTES);
    assert!(presentation.len() > MAX_PAGE_BYTES);
    assert!(presentation.len() <= 8 * MAX_PAGE_BYTES);
    (options, rows[0].key.clone(), id)
}

pub(crate) fn assert_provider_transcript(
    config: &StoreConfig,
    options: &crate::Options,
    host: &crate::storage::transfer::reported::tests::selected_receipts::AuthorizedOriginals,
    key: &str,
) {
    let mut options = options.clone();
    options.source = crate::storage::HostSource::new(
        config.namespace.clone(), config.expected_stream_epoch.clone().unwrap(),
    ).unwrap().with_store(config.clone()).unwrap();
    let mut query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(String::new(), options)]),
    ));
    let body = query.call_selected_at(
        "local", key, "read_transcript", &serde_json::json!({"id":key,"after":0,"limit":1}),
        Some(host), crate::SessionSourceReadMode::ProviderOnly, 1_790_000_000_000,
    ).unwrap();
    assert!(body.to_string().contains("sanitized 0"));
}

fn row() -> CatalogRowData {
    CatalogRowData {
        generation: "a".repeat(64),
        kind: CatalogRowKind::HistoryProgress,
        values: vec![
            "native-key".into(),
            serde_json::to_string(&"\0\\\"界".repeat(MAX_PAGE_BYTES / 8))
                .unwrap()
                .into(),
        ],
    }
}

#[test]
fn current_transfer225_catalog_fragment_order_identity_digest_and_restart() {
    let original = row();
    let records = fragments([7; 32], original.clone()).unwrap();
    assert!(records.len() > 1);
    let mut inbox = MemoryInbox::default();
    assert!(apply_fragment(&mut inbox, &records[1], &[7; 32]).is_err());
    assert!(apply_fragment(&mut inbox, &records[0], &[8; 32]).is_err());
    assert!(
        apply_fragment(&mut inbox, &records[0], &[7; 32])
            .unwrap()
            .is_none()
    );
    assert!(require_complete(&mut inbox, &[7; 32]).is_err());
    // An exact page receipt is responsible for replay; bypassing that receipt
    // and replaying an already consumed fragment is refused.
    assert!(apply_fragment(&mut inbox, &records[0], &[7; 32]).is_err());
    let mut changed = records[1].clone();
    if let TransferRecord::CatalogRow { key, .. } = &mut changed {
        key[0] = "different-key".into();
    }
    assert!(apply_fragment(&mut inbox, &changed, &[7; 32]).is_err());
    let mut changed = records[1].clone();
    if let TransferRecord::CatalogRow { generation, .. } = &mut changed {
        *generation = "b".repeat(64);
    }
    assert!(apply_fragment(&mut inbox, &changed, &[7; 32]).is_err());
    let mut changed = records[1].clone();
    if let TransferRecord::CatalogRow { kind, .. } = &mut changed {
        *kind = CatalogRowKind::HistoryOriginal;
    }
    assert!(apply_fragment(&mut inbox, &changed, &[7; 32]).is_err());
    // Clone only the durable metadata to model a new receiver handle.
    let mut restarted = MemoryInbox(inbox.0.clone());
    for record in &records[1..records.len() - 1] {
        assert!(
            apply_fragment(&mut restarted, record, &[7; 32])
                .unwrap()
                .is_none()
        );
    }
    let before_final = restarted.clone();
    let mut tampered = records.last().unwrap().clone();
    if let TransferRecord::CatalogRow { bytes, .. } = &mut tampered {
        bytes[0] ^= 1;
    }
    assert!(apply_fragment(&mut restarted, &tampered, &[7; 32]).is_err());
    restarted = before_final;
    let result = apply_fragment(&mut restarted, records.last().unwrap(), &[7; 32])
        .unwrap()
        .unwrap();
    assert_eq!(result.values, original.values);
    assert!(restarted.0.is_empty());
    require_complete(&mut restarted, &[7; 32]).unwrap();
}

#[test]
fn current_transfer225_catalog_declared_bounds_and_capture_resume_witness() {
    let records = fragments([7; 32], row()).unwrap();
    let mut oversized = records[0].clone();
    if let TransferRecord::CatalogRow { total_bytes, .. } = &mut oversized {
        *total_bytes = MAX_CATALOG_ROW_BYTES as u64 + 1;
    }
    let mut inbox = MemoryInbox::default();
    assert!(apply_fragment(&mut inbox, &oversized, &[7; 32]).is_err());
    assert!(inbox.0.is_empty());
    let TransferRecord::CatalogRow {
        key,
        parts,
        total_bytes,
        row_digest,
        ..
    } = &records[0]
    else {
        unreachable!()
    };
    let cursor = CaptureCursor {
        key: key.clone(),
        next_part: 1,
        parts: *parts,
        total_bytes: *total_bytes,
        row_digest: *row_digest,
    };
    let restored = capture_cursor(Some(&serde_json::to_string(&cursor).unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(
        resume_fragments(records.clone(), Some(&restored))
            .unwrap()
            .len(),
        records.len() - 1
    );
    let mut changed = row();
    changed.values[1] = "{}".into();
    assert!(resume_fragments(fragments([7; 32], changed).unwrap(), Some(&restored)).is_err());
}
