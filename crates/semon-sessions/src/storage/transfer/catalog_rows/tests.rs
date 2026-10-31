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

pub(crate) fn assert_catalog_categories_equal(
    original: &SnapshotManifest,
    reversed: &SnapshotManifest,
) {
    for kind in CatalogRowKind::ALL {
        let category = kind.category();
        assert_eq!(
            original
                .categories
                .iter()
                .find(|item| item.category == category)
                .unwrap(),
            reversed
                .categories
                .iter()
                .find(|item| item.category == category)
                .unwrap(),
            "exact Catalog category {category:?}",
        );
    }
}

pub(crate) fn assert_provider_transcript(
    config: &StoreConfig,
    options: &crate::Options,
    host: &dyn crate::SessionSourceReader,
    key: &str,
) {
    let mut options = options.clone();
    options.source = crate::storage::HostSource::new(
        config.namespace.clone(),
        config.expected_stream_epoch.clone().unwrap(),
    )
    .unwrap()
    .with_store(config.clone())
    .unwrap();
    let mut query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(String::new(), options)]),
    ));
    let body = query
        .call_selected_at(
            "local",
            key,
            "read_transcript",
            &serde_json::json!({"id":key,"after":0,"limit":1}),
            Some(host),
            crate::SessionSourceReadMode::ProviderOnly,
            1_790_000_000_000,
        )
        .unwrap();
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
fn current_transfer225_catalog_content_digest_survives_independent_capture() {
    let mut original = row();
    original.values[1] = "null".into();
    let capture = |snapshot_id, row: CatalogRowData| {
        let mut manifest = super::super::tests::manifest();
        manifest.snapshot_id = snapshot_id;
        let index = Category::ALL
            .iter()
            .position(|category| *category == row.kind.category())
            .unwrap();
        manifest.categories[index].rows = 1;
        manifest.seal().unwrap();
        let mut page = TransferPage {
            snapshot: manifest.identity(),
            sequence: 1,
            operation: snapshot_id,
            previous: [0; 32],
            category: row.kind.category(),
            records: fragments(manifest.identity(), row).unwrap(),
            digest: [0; 32],
        };
        page.seal().unwrap();
        let checkpoint = TransferCheckpoint::initial(&manifest)
            .advance(&manifest, &page)
            .unwrap();
        (page.digest, checkpoint.digests)
    };
    let first = capture([7; 32], original.clone());
    let second = capture([8; 32], original.clone());
    assert_ne!(first.0, second.0, "page binds its capture identity");
    assert_eq!(first.1, second.1, "content survives reverse capture");
    original.values[1] = "true".into();
    let changed = capture([8; 32], original);
    assert_ne!(second.1, changed.1, "category binds exact row bytes");
}

#[test]
fn current_transfer225_catalog_one_part_and_incomplete_manifest_refusal() {
    let mut small = row();
    small.values[1] = "null".into();
    let records = fragments([7; 32], small.clone()).unwrap();
    assert_eq!(records.len(), 1);
    let mut inbox = MemoryInbox::default();
    let complete = apply_fragment(&mut inbox, &records[0], &[7; 32])
        .unwrap()
        .unwrap();
    assert_eq!(complete.values, small.values);
    require_complete(&mut inbox, &[7; 32]).unwrap();

    let records = fragments([7; 32], row()).unwrap();
    apply_fragment(&mut inbox, &records[0], &[7; 32]).unwrap();
    let before = inbox.0.clone();
    let final_page = reported::ApplyPage {
        checkpoint: TransferCheckpoint {
            snapshot: [7; 32],
            sequence: 1,
            digest: [0; 32],
            rows: vec![0; Category::ALL.len()],
            digests: vec![[0; 32]; Category::ALL.len()],
        },
        records: vec![],
        replayed: false,
        first: false,
        complete: true,
    };
    assert!(reported::acknowledge_apply(&mut inbox, &final_page).is_err());
    assert_eq!(inbox.0, before);
    assert!(!inbox.0.contains_key(IMPORTED_GENERATION));
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
