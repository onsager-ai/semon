//! Runtime adoption probe. This deliberately uses a private injected backend;
//! it does not qualify a selectable complete PostgreSQL mode.
pub(super) fn lifecycle(dsn: &str) {
    use crate::{
        native_evidence::{NativeEvidence, SessionSources},
        storage::{Namespace, runtime::RuntimeStore},
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "pg-runtime");
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/compatibility");
    let path = options
        .codex_home
        .join("sessions/2026/09/24/rollout-parser-cases.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(
        fixtures.join("codex-native-a9b0815/parser-cases.jsonl"),
        &path,
    )
    .unwrap();
    #[cfg(target_os = "linux")]
    let watch = super::producer_tests::SqliteOpenWatch::new(&options);
    let namespace = Namespace::new("injected-runtime").unwrap();
    let storage = RuntimeStore::postgres(&options, namespace.clone(), dsn).unwrap();
    let evidence = NativeEvidence::with_storage(options.clone(), storage);
    let sources = Arc::new(SessionSources::from_evidence(vec![(
        namespace.to_string(),
        evidence.clone(),
    )]));
    let core = crate::ViewerCore::from_sources(sources.clone());
    let started = std::time::Instant::now();
    let page = core.respond(
        "GET",
        "/api/sessions",
        "machine=injected-runtime&limit=60",
        None,
    );
    assert_eq!(page.status, 200, "{}", String::from_utf8_lossy(&page.body));
    eprintln!(
        "postgres-viewercore-injected-first-page latency_us={} response_bytes={}",
        started.elapsed().as_micros(),
        page.body.len()
    );
    let body: Value = serde_json::from_slice(&page.body).unwrap();
    let key = body["items"][0]["key"].as_str().unwrap();
    let transcript = core.respond(
        "GET",
        "/api/session-transcript",
        &format!(
            "machine=injected-runtime&sid={}&after=0&limit=60",
            crate::viewer::percent_encode(key)
        ),
        None,
    );
    assert_eq!(
        transcript.status,
        200,
        "{}",
        String::from_utf8_lossy(&transcript.body)
    );
    let transcript: Value = serde_json::from_slice(&transcript.body).unwrap();
    assert!(!transcript["entries"].as_array().unwrap().is_empty());
    let mut query = crate::Query::from_sources(sources.clone());
    let answer = query.call("list_sessions", &json!({"limit":100})).unwrap();
    assert!(!answer["sessions"].as_array().unwrap().is_empty());
    assert_eq!(
        query
            .call_catalog(
                "list_sessions",
                &json!({}),
                None,
                crate::SessionSourceReadMode::ProviderOnly,
            )
            .unwrap_err()
            .code,
        "capability_unavailable"
    );
    assert_eq!(
        query
            .call_selected(
                "injected-runtime",
                key,
                "get_session",
                &json!({"id":key}),
                None,
                crate::SessionSourceReadMode::ProviderOnly,
            )
            .unwrap_err()
            .code,
        "capability_unavailable"
    );
    // A foreign source key cannot use this app-owned capability, even if the
    // same native paths exist. Lower-level conformance proves forced-RLS SQL.
    let foreign = evidence.storage.page(
        "foreign-runtime",
        &crate::catalog::Request::parse("limit=60").unwrap(),
        None,
    );
    assert_eq!(foreign.status, 503);
    assert_eq!(
        core.respond(
            "GET",
            "/api/sessions",
            "machine=foreign-runtime&limit=60",
            None
        )
        .status,
        404
    );
    core.close();
    sources.close();
    #[cfg(target_os = "linux")]
    watch.assert_untouched(&options);
    assert!(!crate::events::EventCache::path(&options.cache).exists());
    // Connection errors remain errors; the private injection never falls back.
    assert!(
        RuntimeStore::postgres(
            &options,
            namespace,
            "host=127.0.0.1 port=1 user=unavailable connect_timeout=1"
        )
        .is_err()
    );
    assert!(!crate::events::EventCache::path(&options.cache).exists());
}
