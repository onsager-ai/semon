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
    retained_prefix_pages(dsn).await;
}

async fn retained_prefix_pages(dsn: &str) {
    use std::io::Write;
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "retained-prefix");
    let path = crate::storage::conformance::native(&root.0, "prefix-page", "original selected prompt");
    let input = crate::Input { root: crate::InputRoot::Claude, path: "projects/project/prefix-page.jsonl".into() };
    let namespace = Namespace::new("lazy-retained-prefix").unwrap();
    #[cfg(target_os = "linux")]
    let watch = super::producer_tests::SqliteOpenWatch::new(&options);
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone()).await.unwrap();
    store.publish_sources(&namespace, &options, std::slice::from_ref(&input)).await.unwrap().unwrap();
    let before = store.transcript_query(&namespace, &options, "sid=prefix-page&after=0&limit=10", None).await.unwrap();
    assert_eq!(before.status, 200, "{}", String::from_utf8_lossy(&before.body));
    let before: serde_json::Value = serde_json::from_slice(&before.body).unwrap();
    let mut append = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(append, "{}", serde_json::json!({"type":"user","sessionId":"prefix-page","uuid":"later","timestamp":"2026-10-01T00:01:00Z","message":{"role":"user","content":"later appended prompt"}})).unwrap();
    append.sync_all().unwrap();
    store.index_source(&namespace, &path, "claude").await.unwrap();
    // Catalog still selects the old consumed prefix, while Index/proofs advanced.
    let after = store.transcript_query(&namespace, &options, "sid=prefix-page&after=0&limit=10", None).await.unwrap();
    assert_eq!(after.status, 200, "{}", String::from_utf8_lossy(&after.body));
    let after: serde_json::Value = serde_json::from_slice(&after.body).unwrap();
    assert_eq!(before["entries"], after["entries"]);
    assert_eq!(before["generation"], after["generation"]);
    assert!(after["observation"]["source_bytes"].as_u64().unwrap() > 0);
    let mut changed = std::fs::read(&path).unwrap();
    let at = changed.windows(b"original selected prompt".len()).position(|window| window == b"original selected prompt").unwrap();
    changed[at] = b'X';
    std::fs::write(&path, changed).unwrap();
    let refused = store.transcript_query(&namespace, &options, "sid=prefix-page&after=0&limit=10", None).await.unwrap();
    assert_eq!(refused.status, 409);
    #[cfg(target_os = "linux")]
    watch.assert_untouched(&options);
    assert!(!crate::events::EventCache::path(&options.cache).exists());
}
