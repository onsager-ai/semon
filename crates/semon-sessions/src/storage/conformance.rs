//! Reusable metadata contract checks. PG must implement this same fixture seam;
//! native fixture/claim/proof parity is a separate, still-required #478 suite.
use super::*;

pub(super) trait MetadataFixture {
    fn store(&self) -> Box<dyn SessionMetadataStore>;
    fn namespace(&self) -> LogicalSourceNamespace;
    fn publish(&mut self, key: &str);
    fn restart(&mut self);
}

pub(super) fn metadata_conformance(fixture: &mut impl MetadataFixture) {
    let ns = fixture.namespace();
    let other = LogicalSourceNamespace::new("another-authorized-source").unwrap();
    fixture.publish("same-native-id-and-relative-path");
    let store = fixture.store();
    assert!(matches!(store.read_snapshot(&other), Err(StorageError::NamespaceMismatch)));
    let mut snapshot = store.read_snapshot(&ns).unwrap();
    let request = CatalogChangesRequest {
        scope: CatalogReadScope::Current, stream_epoch: None, after: 0, limit: 256,
    };
    let first = snapshot.catalog_changes(&ns, request).unwrap();
    assert_eq!(first.changes.len(), 1);
    assert_eq!(first.source_key, ns.as_str());
    assert_eq!(snapshot.catalog_changes(&other, request), Err(StorageError::NamespaceMismatch));
    fixture.publish("second");
    // A concurrent publication cannot move the admitted snapshot.
    assert_eq!(snapshot.catalog_changes(&ns, request).unwrap(), first);
    let fresh = fixture.store().read_snapshot(&ns).unwrap().catalog_changes(&ns, request).unwrap();
    assert_eq!(fresh.changes.len(), 2);
    assert_eq!(fresh.stream_epoch, first.stream_epoch);
    let compact = store.compact_catalog_changes(&ns, CatalogReadScope::Current, &first.stream_epoch, fresh.head, 1).unwrap();
    assert_eq!(compact.removed, 1);
    // GC/compaction cannot revoke a previously admitted reader.
    assert_eq!(snapshot.catalog_changes(&ns, request).unwrap(), first);
    drop(snapshot);
    fixture.restart();
    let store = fixture.store();
    assert_eq!(store.read_snapshot(&ns).unwrap().catalog_changes(&ns, request), Err(StorageError::ScopeChanged));
    let resumed = store.read_snapshot(&ns).unwrap().catalog_changes(&ns, CatalogChangesRequest {
        stream_epoch: Some(&first.stream_epoch), after: compact.compacted_through, ..request
    }).unwrap();
    assert_eq!(resumed.changes.len(), 1);
    assert_eq!(resumed.changes[0].catalog_key, "second");
    assert_eq!(store.compact_catalog_changes(&other, CatalogReadScope::Current, &first.stream_epoch, fresh.head, 1), Err(StorageError::NamespaceMismatch));
    assert_eq!(store.compact_catalog_changes(&ns, CatalogReadScope::Current, &"0".repeat(64), fresh.head, 1), Err(StorageError::ScopeChanged));
}
