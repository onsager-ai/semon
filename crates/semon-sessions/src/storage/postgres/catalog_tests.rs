use crate::storage::{Namespace, postgres::PgProjectionStore};
use crate::{
    CatalogReadScope,
    model::summary::{CatalogLifecycle, CatalogRow, CatalogSource},
    slot_projection::{File, Projection, Publication, Recipe, SavedSlot},
};

async fn observation(store: &mut PgProjectionStore, namespace: &Namespace, path: &std::path::Path) -> (CatalogRow, Projection) {
    let tx = store.begin(namespace).await.unwrap();
    let (ledger, index) = tx.load_index(namespace, path.to_str().unwrap()).await.unwrap().unwrap();
    let source = CatalogSource {
        path: path.to_owned(), native_id: "pg-catalog".into(), dev: ledger.stat.dev,
        ino: ledger.stat.ino, size: ledger.stat.size, modified_ns: ledger.stat.modified_ns,
        changed_ns: ledger.stat.changed_ns, offset: ledger.offset,
        prefix_sha256: ledger.prefix, tail_sha256: ledger.tail, native_role: None,
    };
    let row = CatalogRow {
        overview: None, trace: None, query: None, lifecycle: CatalogLifecycle::Current,
        key: "pg-catalog".into(), name: "Synthetic catalog".into(), harness: "claude".into(),
        kind: "session".into(), presentation_kind: None, native_ids: vec!["pg-catalog".into()],
        parent: None, parent_source: None, repo: Some("project".into()), branch: None,
        model: String::new(), effort: None, start: Some(1), last: Some(index.events.len() as i64),
        tokens: [0.0; 3], cost: Default::default(), sources: vec![source],
    };
    let slots = index.events.iter().map(|event| SavedSlot {
        recipe: Recipe::U, file: Some(File { path: path.to_owned(), cwd: None }),
        offset: event.o, block: 0, t: None, turn: None, first: false,
        field: None, native_event_id: None, images: Vec::new(),
    }).collect();
    tx.commit(namespace).await.unwrap();
    (row, Projection { key: "pg-catalog".into(), slots, repair: false })
}

pub(super) async fn lifecycle(dsn: &str) {
    use std::io::Write;
    let root = crate::storage::conformance::root();
    let path = crate::storage::conformance::native(&root.0, "pg-catalog", "synthetic body");
    let namespace = Namespace::new("catalog-machine").unwrap();
    let mut store = PgProjectionStore::new(crate::storage::conformance::postgres::connect(dsn).await, namespace.clone()).await.unwrap();
    let start = std::time::Instant::now();
    let first = store.index_source(&namespace, &path, "claude").await.unwrap();
    let (row, projection) = observation(&mut store, &namespace, &path).await;
    let tx = store.begin(&namespace).await.unwrap();
    let generation = tx.publish_catalog(&namespace, &Publication { rows: std::slice::from_ref(&row), transcripts: Some(std::slice::from_ref(&projection)) }, None).await.unwrap().unwrap();
    tx.commit(&namespace).await.unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    let page = tx.catalog_page(&namespace, CatalogReadScope::Current, None, 10).await.unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.generation, generation);
    assert!(page.next.is_none());
    eprintln!("postgres-typed-catalog-cold-first-page-us={}", start.elapsed().as_micros());
    let (recipe_generation, total, slots) = tx.recipe_page(&namespace, &row.key, CatalogReadScope::Current, None, 0, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(serde_json::to_value(&slots).unwrap(), serde_json::to_value(&projection.slots).unwrap());
    tx.commit(&namespace).await.unwrap();
    // Append leaves the published selected prefix readable through a restart.
    writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", serde_json::json!({"type":"user","sessionId":"pg-catalog","uuid":"second","message":{"role":"user","content":"second synthetic body"}})).unwrap();
    let appended = store.index_source(&namespace, &path, "claude").await.unwrap();
    assert_eq!(first.epoch, appended.epoch);
    drop(store);
    store = PgProjectionStore::new(crate::storage::conformance::postgres::connect(dsn).await, namespace.clone()).await.unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    assert_eq!(tx.recipe_page(&namespace, &row.key, CatalogReadScope::Current, Some(&recipe_generation), 0, 10).await.unwrap().1, 1);
    tx.commit(&namespace).await.unwrap();
    let (next_row, next_projection) = observation(&mut store, &namespace, &path).await;
    let tx = store.begin(&namespace).await.unwrap();
    // Stale generation/source publication has no partial effect, and withdrawal
    // captures the last committed recipes while a newer component is prepared.
    assert!(tx.publish_catalog(&namespace, &Publication { rows: std::slice::from_ref(&next_row), transcripts: Some(std::slice::from_ref(&next_projection)) }, None).await.unwrap().is_none());
    assert!(!tx.withdraw_source(&namespace, path.to_str().unwrap(), first).await.unwrap());
    assert!(tx.withdraw_source(&namespace, path.to_str().unwrap(), appended).await.unwrap());
    tx.commit(&namespace).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    assert!(tx.catalog_row(&namespace, &row.key, CatalogReadScope::Current).await.unwrap().is_none());
    let history = tx.catalog_row(&namespace, &row.key, CatalogReadScope::RetainedHistory).await.unwrap().unwrap();
    assert_eq!(history.sources, row.sources);
    let (_, history_total, history_slots) = tx.recipe_page(&namespace, &row.key, CatalogReadScope::RetainedHistory, Some(&recipe_generation), 0, 10).await.unwrap();
    assert_eq!(history_total, total);
    assert_eq!(serde_json::to_value(&history_slots).unwrap(), serde_json::to_value(&slots).unwrap());
    assert!(tx.proof_block(&namespace, path.to_str().unwrap(), first, 0).await.unwrap().is_some());
    tx.commit(&namespace).await.unwrap();
    // Only Raw fixtures were created in this root: no SQLite comparison or
    // hidden EventCache is involved in this Catalog/recipe lifecycle.
    assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| !entry.unwrap().path().is_file()));
    let foreign = Namespace::new("foreign-catalog").unwrap();
    let mut store = PgProjectionStore::new(crate::storage::conformance::postgres::connect(dsn).await, foreign.clone()).await.unwrap();
    let tx = store.begin(&foreign).await.unwrap();
    assert!(tx.catalog_row(&foreign, &row.key, CatalogReadScope::RetainedHistory).await.unwrap().is_none());
    tx.commit(&foreign).await.unwrap();
}
