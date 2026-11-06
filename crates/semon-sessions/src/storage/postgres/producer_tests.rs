use crate::{
    CatalogReadScope, Input, InputRoot,
    storage::{Namespace, postgres::PgProjectionStore},
};

struct NativeReader {
    path: std::path::PathBuf,
    source: crate::model::summary::CatalogSource,
    proof: [u8; 32],
}
impl crate::SessionSourceReader for NativeReader {
    fn read_range(&self, reference: &crate::SessionSourceRef, expected: Option<&str>, offset: u64, max: usize) -> std::io::Result<crate::SessionSourceRange> {
        use sha2::{Digest, Sha256};
        let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "selected fixture changed");
        let generation = format!("{:x}", Sha256::digest(self.source.prefix_sha256));
        let bytes = std::fs::read(&self.path)?;
        if reference.offset != self.source.offset || reference.prefix_sha256 != self.source.prefix_sha256
            || expected.is_some_and(|expected| expected != generation)
            || bytes.len() as u64 < reference.offset
            || <[u8; 32]>::from(Sha256::digest(&bytes[..reference.offset as usize])) != self.proof
            || offset >= reference.offset || max > crate::SESSION_SOURCE_CHUNK_MAX
        { return Err(invalid()); }
        let end = (offset as usize + max).min(reference.offset as usize);
        Ok(crate::SessionSourceRange { generation, length: reference.offset, offset,
            bytes: std::sync::Arc::from(&bytes[offset as usize..end]), cached: true })
    }
}

pub(super) async fn lifecycle(dsn: &str) {
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "pg-producer");
    let path =
        crate::storage::conformance::native(&root.0, "pg-produced", "synthetic producer prompt");
    let namespace = Namespace::new("native-producer").unwrap();
    let inputs = vec![Input {
        root: InputRoot::Claude,
        path: "projects/project/pg-produced.jsonl".into(),
    }];
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let start = std::time::Instant::now();
    let first = store
        .publish_sources(&namespace, &options, &inputs)
        .await
        .unwrap()
        .unwrap();
    let served = store.catalog_reply(&namespace, &options, "limit=10").await.unwrap();
    assert_eq!(served.status, 200);
    let served: serde_json::Value = serde_json::from_slice(&served.body).unwrap();
    assert_eq!(served["items"].as_array().unwrap().len(), 1);
    assert_eq!(served["items"][0]["harness"], "claude");
    let tx = store.begin(&namespace).await.unwrap();
    let page = tx
        .catalog_page(&namespace, CatalogReadScope::Current, None, 10)
        .await
        .unwrap();
    eprintln!(
        "postgres-shared-producer-catalog-first-page-us={}",
        start.elapsed().as_micros()
    );
    assert_eq!(page.generation, first);
    assert_eq!(page.rows.len(), 1);
    let row = &page.rows[0];
    let key = row.key.clone();
    assert_eq!(row.harness, "claude");
    assert_eq!(row.native_ids, ["pg-produced"]);
    let (recipe, total, slots) = tx
        .recipe_page(&namespace, &key, CatalogReadScope::Current, None, 0, 10)
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(slots.len(), 1);
    assert!(matches!(slots[0].recipe, crate::slot_projection::Recipe::U));
    assert_eq!(slots[0].file.as_ref().unwrap().path, path);
    let source = row.sources[0].clone();
    let proof = tx.proof_block(&namespace, path.to_str().unwrap(), tx.ledger(&namespace, path.to_str().unwrap()).await.unwrap().unwrap(), 0).await.unwrap().unwrap();
    tx.commit(&namespace).await.unwrap();
    let reader = NativeReader { path: path.clone(), source, proof };
    let selected = store.transcript_reply(&namespace, &options, &key, CatalogReadScope::Current, None, 0, 10, &reader).await.unwrap();
    assert_eq!(selected.status, 200);
    let selected: serde_json::Value = serde_json::from_slice(&selected.body).unwrap();
    assert_eq!(selected["entries"][0]["text"], "synthetic producer prompt");
    assert_eq!(selected["entries"][0]["freshness"]["state"], "cached");
    assert!(selected["entries"][0]["entry_id"].as_str().is_some());
    // Restart and append use the same native producer and persisted parser base.
    drop(store);
    std::io::Write::write_all(&mut std::fs::OpenOptions::new().append(true).open(&path).unwrap(), format!("{}\n", serde_json::json!({"type":"assistant","sessionId":"pg-produced","uuid":"answer","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"synthetic answer"}]}})).as_bytes()).unwrap();
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let next = store
        .publish_sources(&namespace, &options, &inputs)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first, next);
    let tx = store.begin(&namespace).await.unwrap();
    for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
        let (generation, total, slots) = tx
            .recipe_page(&namespace, &key, scope, None, 0, 10)
            .await
            .unwrap();
        assert_ne!(generation, recipe);
        assert_eq!(total, 2);
        assert_eq!(slots.len(), 2);
    }
    tx.commit(&namespace).await.unwrap();
    let foreign = Namespace::new("foreign-native-producer").unwrap();
    assert!(
        store
            .publish_sources(&foreign, &options, &inputs)
            .await
            .is_err()
    );
    let mut other = PgProjectionStore::new(super::connect(dsn).await, foreign.clone())
        .await
        .unwrap();
    let tx = other.begin(&foreign).await.unwrap();
    assert!(
        tx.catalog_row(&foreign, &key, CatalogReadScope::Current)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&foreign).await.unwrap();
    assert!(!crate::events::EventCache::path(&options.cache).exists());
    assert!(!options.cache.with_extension("sqlite3-wal").exists());
    assert!(!options.cache.with_extension("sqlite3-shm").exists());
}
