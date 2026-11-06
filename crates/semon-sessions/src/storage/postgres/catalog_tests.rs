use crate::storage::{Namespace, postgres::PgProjectionStore};
use crate::{
    CatalogReadScope,
    model::summary::{CatalogLifecycle, CatalogRow, CatalogSource},
    slot_projection::{File, Projection, Publication, Recipe, SavedSlot},
};

async fn observation(
    store: &mut PgProjectionStore,
    namespace: &Namespace,
    path: &std::path::Path,
) -> (CatalogRow, Projection) {
    let tx = store.begin(namespace).await.unwrap();
    let (ledger, index) = tx
        .load_index(namespace, path.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let source = CatalogSource {
        path: path.to_owned(),
        native_id: "pg-catalog".into(),
        dev: ledger.stat.dev,
        ino: ledger.stat.ino,
        size: ledger.stat.size,
        modified_ns: ledger.stat.modified_ns,
        changed_ns: ledger.stat.changed_ns,
        offset: ledger.offset,
        prefix_sha256: ledger.prefix,
        tail_sha256: ledger.tail,
        native_role: None,
    };
    let row = CatalogRow {
        overview: None,
        trace: None,
        query: None,
        lifecycle: CatalogLifecycle::Current,
        key: "pg-catalog".into(),
        name: "Synthetic catalog".into(),
        harness: "claude".into(),
        kind: "session".into(),
        presentation_kind: None,
        native_ids: vec!["pg-catalog".into()],
        parent: None,
        parent_source: None,
        repo: Some("project".into()),
        branch: None,
        model: String::new(),
        effort: None,
        start: Some(1),
        last: Some(index.events.len() as i64),
        tokens: [0.0; 3],
        cost: Default::default(),
        sources: vec![source],
    };
    let slots = index
        .events
        .iter()
        .map(|event| SavedSlot {
            recipe: Recipe::U,
            file: Some(File {
                path: path.to_owned(),
                cwd: None,
            }),
            offset: event.o,
            block: 0,
            t: None,
            turn: None,
            first: false,
            field: None,
            native_event_id: None,
            images: Vec::new(),
        })
        .collect();
    tx.commit(namespace).await.unwrap();
    (
        row,
        Projection {
            key: "pg-catalog".into(),
            slots,
            repair: false,
        },
    )
}

async fn replacement(dsn: &str) {
    let root = crate::storage::conformance::root();
    let path = crate::storage::conformance::native(&root.0, "pg-catalog", &"original synthetic ".repeat(1024));
    let namespace = Namespace::new("catalog-replacement").unwrap();
    let mut store = PgProjectionStore::new(crate::storage::conformance::postgres::connect(dsn).await, namespace.clone()).await.unwrap();
    let first = store.index_source(&namespace, &path, "claude").await.unwrap();
    let (row, projection) = observation(&mut store, &namespace, &path).await;
    let tx = store.begin(&namespace).await.unwrap();
    let generation = tx.publish_catalog(&namespace, &Publication { rows: std::slice::from_ref(&row), transcripts: Some(std::slice::from_ref(&projection)) }, None).await.unwrap().unwrap();
    let (recipes, _, _) = tx.recipe_page(&namespace, &row.key, CatalogReadScope::Current, None, 0, 10).await.unwrap();
    tx.commit(&namespace).await.unwrap();
    crate::storage::conformance::native(&root.0, "pg-catalog", "short replacement");
    let replaced = store.index_source(&namespace, &path, "claude").await.unwrap();
    assert!(replaced.offset < first.offset);
    assert!(replaced.epoch > first.epoch);
    let tx = store.begin(&namespace).await.unwrap();
    for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
        assert!(tx.catalog_row(&namespace, &row.key, scope).await.is_err());
    }
    tx.commit(&namespace).await.unwrap();
    let (next, next_projection) = observation(&mut store, &namespace, &path).await;
    let tx = store.begin(&namespace).await.unwrap();
    let next_generation = tx.publish_catalog(&namespace, &Publication { rows: std::slice::from_ref(&next), transcripts: Some(std::slice::from_ref(&next_projection)) }, Some(&generation)).await.unwrap().unwrap();
    assert_eq!(tx.catalog_row(&namespace, &row.key, CatalogReadScope::Current).await.unwrap().unwrap().sources, next.sources);
    assert_eq!(tx.catalog_row(&namespace, &row.key, CatalogReadScope::RetainedHistory).await.unwrap().unwrap().sources, row.sources);
    assert_eq!(tx.recipe_page(&namespace, &row.key, CatalogReadScope::RetainedHistory, Some(&recipes), 0, 10).await.unwrap().2[0].offset, projection.slots[0].offset);
    // A second outgoing observation needs composite History. Until that port is
    // implemented, refusal keeps both the original custody and Current intact.
    assert!(!tx.withdraw_source(&namespace, path.to_str().unwrap(), replaced).await.unwrap());
    assert_eq!(tx.catalog_generation(&namespace).await.unwrap(), Some(next_generation));
    assert_eq!(tx.ledger(&namespace, path.to_str().unwrap()).await.unwrap(), Some(replaced));
    tx.commit(&namespace).await.unwrap();
}

pub(super) async fn lifecycle(dsn: &str) {
    replacement(dsn).await;
    use std::io::Write;
    let root = crate::storage::conformance::root();
    let path = crate::storage::conformance::native(&root.0, "pg-catalog", "synthetic body");
    let namespace = Namespace::new("catalog-machine").unwrap();
    let mut store = PgProjectionStore::new(
        crate::storage::conformance::postgres::connect(dsn).await,
        namespace.clone(),
    )
    .await
    .unwrap();
    let start = std::time::Instant::now();
    let first = store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    let (row, projection) = observation(&mut store, &namespace, &path).await;
    let tx = store.begin(&namespace).await.unwrap();
    let generation = tx
        .publish_catalog(
            &namespace,
            &Publication {
                rows: std::slice::from_ref(&row),
                transcripts: Some(std::slice::from_ref(&projection)),
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
    tx.commit(&namespace).await.unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    let page = tx
        .catalog_page(&namespace, CatalogReadScope::Current, None, 10)
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.generation, generation);
    assert!(page.next.is_none());
    let found = tx
        .search_catalog(&namespace, CatalogReadScope::Current, "SYNTHETIC", None, 10)
        .await
        .unwrap();
    assert_eq!(found.rows.len(), 1);
    assert_eq!(found.candidates, 1);
    assert!(!found.partial);
    assert!(found.next.is_none());
    assert!(
        tx.search_catalog(
            &namespace,
            CatalogReadScope::Current,
            "private body",
            None,
            10
        )
        .await
        .unwrap()
        .rows
        .is_empty()
    );
    eprintln!(
        "postgres-typed-catalog-cold-first-page-us={}",
        start.elapsed().as_micros()
    );
    let (recipe_generation, total, slots) = tx
        .recipe_page(&namespace, &row.key, CatalogReadScope::Current, None, 0, 10)
        .await
        .unwrap();
    assert_eq!(total, 1);
    let facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "reported-machine".into(),
        ..Default::default()
    };
    assert!(tx.write_facts(&namespace, None, 1, &facts).await.unwrap());
    assert!(!tx.write_facts(&namespace, None, 2, &facts).await.unwrap());
    assert!(
        tx.write_checkpoint(&namespace, "source-claim", None, 1, "prepared")
            .await
            .unwrap()
    );
    let changes = tx
        .catalog_changes(&namespace, CatalogReadScope::Current, None, 0, 10)
        .await
        .unwrap();
    assert!(changes.caught_up);
    assert_eq!(changes.changes.len(), 1);
    let stream_epoch = changes.stream_epoch;
    // Force a late search-document refusal after Catalog and recipe writes.
    // Committing the surrounding transaction must retain the old publication.
    let mut oversized = row.clone();
    oversized.name = "x".repeat(9000);
    assert!(tx.publish_catalog(&namespace, &Publication { rows: std::slice::from_ref(&oversized), transcripts: Some(std::slice::from_ref(&projection)) }, Some(&generation)).await.is_err());
    assert_eq!(tx.catalog_generation(&namespace).await.unwrap(), Some(generation.clone()));
    assert_eq!(tx.catalog_row(&namespace, &row.key, CatalogReadScope::Current).await.unwrap().unwrap().name, row.name);
    assert_eq!(tx.catalog_changes(&namespace, CatalogReadScope::Current, Some(&stream_epoch), 0, 10).await.unwrap().head, 1);
    // The same typed publication and recipes exercise the ordinary SQLite
    // publisher; no assertion is weakened to accommodate PostgreSQL.
    let comparison_root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&comparison_root.0, "comparison");
    let mut sqlite = crate::events::EventCache::open_scoped(&options.cache).unwrap();
    crate::events::scan_file(&path, "claude", &mut sqlite, &mut false).unwrap();
    let sqlite_publication = Publication { rows: std::slice::from_ref(&row), transcripts: Some(std::slice::from_ref(&projection)) };
    assert!(sqlite.publish_catalog_projection_fenced(&sqlite_publication, None).unwrap().is_some());
    let db = rusqlite::Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
    let metadata: String = db.query_row("SELECT metadata FROM session_catalog WHERE session_key=?1", [&row.key], |row| row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&metadata).unwrap(), serde_json::to_value(&page.rows[0]).unwrap());
    let sqlite_slots = sqlite.session_slots(&row, sqlite.session_catalog_generation().as_deref(), projection.slots.len()).unwrap();
    assert_eq!(serde_json::to_value(sqlite_slots).unwrap(), serde_json::to_value(&slots).unwrap());
    assert_eq!(
        serde_json::to_value(&slots).unwrap(),
        serde_json::to_value(&projection.slots).unwrap()
    );
    tx.commit(&namespace).await.unwrap();
    // Append leaves the published selected prefix readable through a restart.
    writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", serde_json::json!({"type":"user","sessionId":"pg-catalog","uuid":"second","message":{"role":"user","content":"second synthetic body"}})).unwrap();
    let appended = store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    assert_eq!(first.epoch, appended.epoch);
    drop(store);
    store = PgProjectionStore::new(
        crate::storage::conformance::postgres::connect(dsn).await,
        namespace.clone(),
    )
    .await
    .unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    assert_eq!(
        tx.reported_facts(&namespace).await.unwrap(),
        Some((1, facts.clone()))
    );
    assert_eq!(
        tx.checkpoint(&namespace, "source-claim").await.unwrap(),
        Some((1, "prepared".into()))
    );
    assert!(
        !tx.write_checkpoint(&namespace, "source-claim", None, 2, "stale")
            .await
            .unwrap()
    );
    assert!(
        tx.write_checkpoint(&namespace, "source-claim", Some(1), 2, "uncommitted")
            .await
            .unwrap()
    );
    drop(tx);
    let tx = store.begin(&namespace).await.unwrap();
    assert_eq!(
        tx.checkpoint(&namespace, "source-claim").await.unwrap(),
        Some((1, "prepared".into()))
    );
    assert_eq!(
        tx.recipe_page(
            &namespace,
            &row.key,
            CatalogReadScope::Current,
            Some(&recipe_generation),
            0,
            10
        )
        .await
        .unwrap()
        .1,
        1
    );
    tx.commit(&namespace).await.unwrap();
    let (next_row, next_projection) = observation(&mut store, &namespace, &path).await;
    let tx = store.begin(&namespace).await.unwrap();
    // Stale generation/source publication has no partial effect, and withdrawal
    // captures the last committed recipes while a newer component is prepared.
    assert!(
        tx.publish_catalog(
            &namespace,
            &Publication {
                rows: std::slice::from_ref(&next_row),
                transcripts: Some(std::slice::from_ref(&next_projection))
            },
            None
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        !tx.withdraw_source(&namespace, path.to_str().unwrap(), first)
            .await
            .unwrap()
    );
    assert!(
        tx.withdraw_source(&namespace, path.to_str().unwrap(), appended)
            .await
            .unwrap()
    );
    tx.commit(&namespace).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    assert!(
        tx.catalog_row(&namespace, &row.key, CatalogReadScope::Current)
            .await
            .unwrap()
            .is_none()
    );
    let history = tx
        .catalog_row(&namespace, &row.key, CatalogReadScope::RetainedHistory)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(history.sources, row.sources);
    assert_eq!(
        tx.search_catalog(
            &namespace,
            CatalogReadScope::RetainedHistory,
            "synthetic",
            None,
            10
        )
        .await
        .unwrap()
        .rows
        .len(),
        1
    );
    assert!(
        tx.search_catalog(&namespace, CatalogReadScope::Current, "synthetic", None, 10)
            .await
            .unwrap()
            .rows
            .is_empty()
    );
    let (_, history_total, history_slots) = tx
        .recipe_page(
            &namespace,
            &row.key,
            CatalogReadScope::RetainedHistory,
            Some(&recipe_generation),
            0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(history_total, total);
    assert_eq!(
        serde_json::to_value(&history_slots).unwrap(),
        serde_json::to_value(&slots).unwrap()
    );
    let changes = tx
        .catalog_changes(
            &namespace,
            CatalogReadScope::Current,
            Some(&stream_epoch),
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(changes.changes.len(), 1);
    assert_eq!(changes.head, 2);
    assert!(changes.caught_up);
    assert!(
        tx.catalog_changes(
            &namespace,
            CatalogReadScope::Current,
            Some("foreign epoch"),
            1,
            10
        )
        .await
        .is_err()
    );
    assert!(
        tx.proof_block(&namespace, path.to_str().unwrap(), first, 0)
            .await
            .unwrap()
            .is_some()
    );
    tx.commit(&namespace).await.unwrap();
    // Only Raw fixtures were created in this root: no SQLite comparison or
    // hidden EventCache is involved in this Catalog/recipe lifecycle.
    assert!(
        std::fs::read_dir(&root.0)
            .unwrap()
            .all(|entry| !entry.unwrap().path().is_file())
    );
    let foreign = Namespace::new("foreign-catalog").unwrap();
    let mut store = PgProjectionStore::new(
        crate::storage::conformance::postgres::connect(dsn).await,
        foreign.clone(),
    )
    .await
    .unwrap();
    let tx = store.begin(&foreign).await.unwrap();
    assert!(tx.reported_facts(&foreign).await.unwrap().is_none());
    assert!(
        tx.checkpoint(&foreign, "source-claim")
            .await
            .unwrap()
            .is_none()
    );
    assert!(tx.write_facts(&namespace, None, 1, &facts).await.is_err());
    assert!(
        tx.catalog_row(&foreign, &row.key, CatalogReadScope::RetainedHistory)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&foreign).await.unwrap();
    let mut raw = crate::storage::conformance::postgres::connect(dsn).await;
    for table in ["catalog", "catalog_heads", "recipe_heads", "recipes", "history_catalog", "history_recipe_heads", "history_recipes", "search_postings", "reported_facts", "checkpoints", "changes", "change_heads"] {
        let tx = raw.transaction().await.unwrap();
        tx.query_one("SELECT set_config('semon.namespace',$1,true)", &[&foreign.binding()]).await.unwrap();
        let visible: i64 = tx.query_one(&format!("SELECT count(*) FROM semon_projection.{table}"), &[]).await.unwrap().get(0);
        assert_eq!(visible, 0, "foreign rows visible in {table}");
        assert_eq!(tx.execute(&format!("DELETE FROM semon_projection.{table} WHERE namespace=$1"), &[&namespace.binding()]).await.unwrap(), 0);
        tx.rollback().await.unwrap();
    }
    let tx = raw.transaction().await.unwrap();
    tx.query_one("SELECT set_config('semon.namespace',$1,true)", &[&foreign.binding()]).await.unwrap();
    let refused = tx.execute("INSERT INTO semon_projection.checkpoints(namespace,checkpoint_key,revision,metadata) VALUES($1,'foreign-insert',1,'denied')", &[&namespace.binding()]).await.unwrap_err();
    assert_eq!(refused.code(), Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE));
    tx.rollback().await.unwrap();
}
