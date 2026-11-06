use crate::{
    CatalogReadScope, Input, InputRoot,
    storage::{Namespace, postgres::PgProjectionStore},
};

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
    tx.commit(&namespace).await.unwrap();
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
