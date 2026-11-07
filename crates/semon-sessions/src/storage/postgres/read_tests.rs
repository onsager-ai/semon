use crate::{
    CatalogReadScope,
    storage::{Namespace, postgres::PgProjectionStore},
};

pub(super) async fn filters_and_continuation(dsn: &str) {
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "pg-pages");
    let namespace = Namespace::new("paged-native-producer").unwrap();
    let mut inputs = Vec::new();
    for index in 0..3 {
        let id = format!("page-{index}");
        crate::storage::conformance::native(&root.0, &id, "paged synthetic source");
        inputs.push(crate::Input {
            root: crate::InputRoot::Claude,
            path: format!("projects/project/{id}.jsonl"),
        });
    }
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    store
        .publish_sources(&namespace, &options, &inputs)
        .await
        .unwrap()
        .unwrap();
    for query in [
        "limit=1",
        "q=page&harness=claude&limit=1",
        "scope=retained_history&q=page&limit=1",
    ] {
        let mut seen = std::collections::BTreeSet::new();
        let mut current = query.to_owned();
        for _ in 0..3 {
            let reply = store
                .catalog_reply(&namespace, &options, &current)
                .await
                .unwrap();
            assert_eq!(reply.status, 200);
            let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(body["completeness"]["state"], "partial");
            assert_eq!(body["items"].as_array().unwrap().len(), 1);
            assert!(seen.insert(body["items"][0]["key"].as_str().unwrap().to_owned()));
            let Some(cursor) = body["next_cursor"].as_str() else {
                break;
            };
            current = format!("{query}&cursor={}", crate::viewer::percent_encode(cursor));
            let wrong_scope = format!(
                "harness=codex&limit=1&cursor={}",
                crate::viewer::percent_encode(cursor)
            );
            assert_eq!(
                store
                    .catalog_reply(&namespace, &options, &wrong_scope)
                    .await
                    .unwrap()
                    .status,
                400
            );
        }
        assert_eq!(seen.len(), 3, "{query}");
    }
    let first = store
        .catalog_reply(&namespace, &options, "limit=1")
        .await
        .unwrap();
    let first: serde_json::Value = serde_json::from_slice(&first.body).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    let path = inputs[0].full_path(&options);
    // Index replacement invalidates the previously published source tag.
    crate::storage::conformance::native(&root.0, "page-0", "replacement source with new bytes");
    store
        .index_source(&namespace, &path, "claude")
        .await
        .unwrap();
    assert!(
        store
            .catalog_reply(&namespace, &options, "limit=10")
            .await
            .is_err()
    );
    store
        .publish_sources(&namespace, &options, &inputs)
        .await
        .unwrap()
        .unwrap();
    let stale = format!("limit=1&cursor={}", crate::viewer::percent_encode(cursor));
    assert_eq!(
        store
            .catalog_reply(&namespace, &options, &stale)
            .await
            .unwrap()
            .status,
        409
    );
    let tx = store.begin(&namespace).await.unwrap();
    assert_eq!(
        tx.catalog_page(&namespace, CatalogReadScope::Current, None, 10)
            .await
            .unwrap()
            .rows
            .len(),
        3
    );
    tx.commit(&namespace).await.unwrap();
    assert!(!crate::events::EventCache::path(&options.cache).exists());
}
