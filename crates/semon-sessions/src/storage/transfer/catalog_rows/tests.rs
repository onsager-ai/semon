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
    assert!(apply_fragment(&mut inbox, &records[0], &[7; 32]).unwrap().is_none());
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
        assert!(apply_fragment(&mut restarted, record, &[7; 32]).unwrap().is_none());
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
    let TransferRecord::CatalogRow { key, parts, total_bytes, row_digest, .. } = &records[0] else {
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
    assert_eq!(resume_fragments(records.clone(), Some(&restored)).unwrap().len(), records.len() - 1);
    let mut changed = row();
    changed.values[1] = "{}".into();
    assert!(resume_fragments(fragments([7; 32], changed).unwrap(), Some(&restored)).is_err());
}
