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
    let foreign_root = crate::storage::conformance::root();
    let foreign_options = crate::storage::conformance::source(&foreign_root.0, "foreign-runtime");
    crate::storage::conformance::native(
        &foreign_root.0,
        "foreign-session",
        "foreign namespace source",
    );
    let foreign_store = RuntimeStore::postgres(
        &foreign_options,
        Namespace::new("foreign-runtime").unwrap(),
        dsn,
    )
    .unwrap();
    foreign_store
        .publish_sources(&[crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/foreign-session.jsonl".into(),
        }])
        .unwrap();
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
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["key"] != "foreign-session")
    );
    let key = body["items"][0]["key"].as_str().unwrap();
    assert!(evidence.storage.work.lock().unwrap().events.is_none());
    assert_eq!(
        evidence.storage.generation().unwrap().as_deref(),
        body["generation"].as_str()
    );
    let resolution = core.respond(
        "GET",
        "/api/session-resolve",
        &format!(
            "machine=injected-runtime&harness=codex&native_id={}",
            crate::viewer::percent_encode(body["items"][0]["native_ids"][0].as_str().unwrap()),
        ),
        None,
    );
    assert_eq!(
        resolution.status,
        200,
        "{}",
        String::from_utf8_lossy(&resolution.body)
    );
    let resolution: Value = serde_json::from_slice(&resolution.body).unwrap();
    assert_eq!(resolution["resolution"]["state"], "provisional");
    assert_eq!(resolution["resolution"]["catalog_key"], key);
    assert!(
        evidence
            .storage
            .resolve_native(
                "foreign-runtime",
                "codex",
                key,
                crate::CatalogReadScope::Current
            )
            .is_err()
    );
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
    assert_ne!(transcript["content_observation"]["state"], "incomplete");
    assert!(transcript["observation"]["source_bytes"].as_u64().unwrap() > 0);
    assert_eq!(transcript["observation"]["incomplete_entries"], 0);
    assert!(
        transcript["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["out"]
                    .as_str()
                    .is_some_and(|text| text.contains("direct"))
            })
    );
    for route in [
        "/api/tree",
        "/api/viewer/home",
        "/api/viewer/analytics",
        "/api/session-annotations",
    ] {
        assert_eq!(core.respond("GET", route, "", None).status, 503, "{route}");
    }
    let mut query = crate::Query::from_sources(sources.clone());
    let answer = query.call("list_sessions", &json!({"limit":100})).unwrap();
    assert!(!answer["sessions"].as_array().unwrap().is_empty());
    let catalog = query
        .call_catalog(
            "list_sessions",
            &json!({}),
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        )
        .unwrap();
    assert_eq!(catalog["sessions"], answer["sessions"]);
    query
        .call_catalog(
            "stalls",
            &json!({"idle_minutes":1}),
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        )
        .unwrap();
    for tool in ["get_session", "read_transcript"] {
        let args = if tool == "read_transcript" {
            json!({"id":key,"limit":10})
        } else {
            json!({"id":key})
        };
        let selected = query
            .call_selected(
                "injected-runtime",
                key,
                tool,
                &args,
                None,
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
            )
            .unwrap();
        assert!(selected.get("error").is_none(), "{selected}");
    }
    assert_eq!(
        query
            .call_selected(
                "injected-runtime",
                key,
                "get_session_activity",
                &json!({"id":key}),
                None,
                crate::SessionSourceReadMode::LocalOrPinnedProvider
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
    assert!(
        evidence
            .storage
            .publish_for(
                &Namespace::new("foreign-runtime").unwrap(),
                &[crate::Input {
                    root: crate::InputRoot::Claude,
                    path: "projects/project/foreign-session.jsonl".into(),
                }],
            )
            .is_err()
    );
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
    // Runtime custody writes use the admitted owner, never a second SQLite
    // connection. An interrupted intent keeps Current unavailable on restart.
    let input = crate::Input {
        root: crate::InputRoot::Codex,
        path: "sessions/2026/09/24/rollout-parser-cases.jsonl".into(),
    };
    assert!(
        crate::SessionSourceWrite::begin_in(&sources, "foreign-runtime", &input, None, 0).is_err()
    );
    let length = std::fs::metadata(&path).unwrap().len();
    let interrupted =
        crate::SessionSourceWrite::begin_in(&sources, "injected-runtime", &input, None, length)
            .unwrap();
    assert!(
        evidence
            .storage
            .publish_discovered(std::slice::from_ref(&input))
            .is_err()
    );
    assert!(crate::collect_in(&sources, "injected-runtime").is_err());
    let pending = core.respond(
        "GET",
        "/api/sessions",
        "machine=injected-runtime&limit=60",
        None,
    );
    assert_eq!(pending.status, 503);
    drop(interrupted);
    let reopened =
        RuntimeStore::postgres(&options, Namespace::new("injected-runtime").unwrap(), dsn).unwrap();
    assert_eq!(
        reopened
            .page(
                "injected-runtime",
                &crate::catalog::Request::parse("limit=60").unwrap(),
                None
            )
            .status,
        503
    );
    drop(reopened);
    let write = crate::SessionSourceWrite::begin_in(
        &sources,
        "injected-runtime",
        &input,
        Some(&"a".repeat(32)),
        length,
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .sync_all()
        .unwrap();
    write.complete(length).unwrap();
    evidence
        .storage
        .publish_discovered(std::slice::from_ref(&input))
        .unwrap();
    let wanted = crate::read_facts(options.facts.as_ref().unwrap()).unwrap();
    let facts_write =
        crate::SessionFactsWrite::begin_in(&sources, "injected-runtime", &wanted).unwrap();
    assert_eq!(
        core.respond(
            "GET",
            "/api/sessions",
            "machine=injected-runtime&limit=60",
            None
        )
        .status,
        503
    );
    facts_write.complete().unwrap();
    evidence
        .storage
        .publish_discovered(std::slice::from_ref(&input))
        .unwrap();
    assert_eq!(
        core.respond(
            "GET",
            "/api/sessions",
            "machine=injected-runtime&limit=60",
            None
        )
        .status,
        200
    );
    assert!(
        !crate::collect_in(&sources, "injected-runtime")
            .unwrap()
            .is_empty()
    );
    assert!(crate::local_facts_in(&sources, "injected-runtime").is_err());
    let local_options = crate::Options {
        facts: None,
        ..options.clone()
    };
    let local_storage = RuntimeStore::postgres(
        &local_options,
        Namespace::new("local-collection").unwrap(),
        dsn,
    )
    .unwrap();
    let local_sources = SessionSources::from_evidence(vec![(
        "local-collection".into(),
        NativeEvidence::with_storage(local_options, local_storage),
    )]);
    let collected = crate::local_facts_in(&local_sources, "local-collection").unwrap();
    assert_eq!(collected.version, crate::FACTS_VERSION);
    assert!(
        collected
            .codex_rollouts
            .as_ref()
            .unwrap()
            .contains(&input.path)
    );
    assert!(crate::FactsSource::new_in(&sources, "foreign-runtime").is_err());
    // Sidecar and process-record intents use PostgreSQL and conservatively
    // fence every Current owner until completion, including across restart.
    for (relative, bytes) in [
        ("projects/project/subagents/agent-runtime.meta.json", br#"{"agentType":"worker"}"#.as_slice()),
        ("sessions/4242.json", br#"{"sessionId":"runtime","pid":4242}"#.as_slice()),
    ] {
        let runtime_input = crate::Input { root: crate::InputRoot::Claude, path: relative.into() };
        let write = crate::SessionSourceWrite::begin_in(&sources, "injected-runtime", &runtime_input, None, 0).unwrap();
        assert!(crate::collect_in(&sources, "injected-runtime").is_err());
        assert_eq!(evidence.storage.page("injected-runtime", &crate::catalog::Request::parse("limit=60").unwrap(), None).status, 503);
        let full = runtime_input.full_path(&options);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, bytes).unwrap();
        std::fs::File::open(&full).unwrap().sync_all().unwrap();
        write.complete(bytes.len() as u64).unwrap();
        assert!(evidence.storage.pending_sources().unwrap().contains(&input));
        evidence.storage.publish_discovered(std::slice::from_ref(&input)).unwrap();
    }
    evidence.storage.discover_sources(std::slice::from_ref(&input), true, true).unwrap();
    assert!(crate::source_projection_ready_in(&sources, "injected-runtime", "codex", &input.path).unwrap().is_none());
    evidence.storage.publish_discovered(std::slice::from_ref(&input)).unwrap();
    let proof = crate::session_source_proof_in(&sources, "injected-runtime", "codex", &input.path).unwrap().unwrap();
    let ready = crate::source_projection_ready_in(&sources, "injected-runtime", "codex", &input.path).unwrap().unwrap();
    crate::retain_source_event_index_in(&sources, "injected-runtime", &proof, &ready).unwrap();
    assert!(crate::source_event_index_retained_in(&sources, "injected-runtime", &proof, &ready).unwrap());
    assert!(crate::session_source_proof_in(&sources, "foreign-runtime", "codex", &input.path).is_err());
    assert!(crate::source_projection_ready_in(&sources, "foreign-runtime", "codex", &input.path).is_err());
    let empty_input = crate::Input {
        root: crate::InputRoot::Codex,
        path: "sessions/2026/09/24/empty.jsonl".into(),
    };
    std::fs::write(empty_input.full_path(&options), b"").unwrap();
    evidence
        .storage
        .discover_sources(std::slice::from_ref(&empty_input), false, false)
        .unwrap();
    evidence
        .storage
        .publish_discovered(std::slice::from_ref(&empty_input))
        .unwrap();
    assert!(
        !evidence
            .storage
            .pending_sources()
            .unwrap()
            .contains(&empty_input)
    );
    std::fs::remove_file(empty_input.full_path(&options)).unwrap();
    // Reconcile observed removal into retained custody and a valid empty page.
    std::fs::remove_file(&path).unwrap();
    {
        let mut work = evidence.storage.work.lock().unwrap();
        while !evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap()
        {}
    }
    let empty = evidence.storage.page(
        "injected-runtime",
        &crate::catalog::Request::parse("limit=60").unwrap(),
        None,
    );
    assert_eq!(
        empty.status,
        200,
        "{}",
        String::from_utf8_lossy(&empty.body)
    );
    let empty: Value = serde_json::from_slice(&empty.body).unwrap();
    assert!(empty["items"].as_array().unwrap().is_empty());
    let history = evidence.storage.page(
        "injected-runtime",
        &crate::catalog::Request::parse(&format!(
            "sid={}&scope=retained_history",
            crate::viewer::percent_encode(key)
        ))
        .unwrap(),
        None,
    );
    assert_eq!(
        history.status,
        200,
        "{}",
        String::from_utf8_lossy(&history.body)
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
