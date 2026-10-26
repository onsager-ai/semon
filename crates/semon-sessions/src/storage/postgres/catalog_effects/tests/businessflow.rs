//! The ordinary native model must run after PG ingest, not be copied from the
//! SQLite oracle, admitted publication and lease-bound Query bodies/fields.
use super::*;
use crate::events::EventCache;
use crate::storage::native_events::NativeEventStore;

fn producer_rows(rows: &[CatalogRow]) -> serde_json::Value {
    let mut value = serde_json::to_value(rows).unwrap();
    for row in value.as_array_mut().unwrap() {
        if let Some(query) = row
            .get_mut("query")
            .and_then(serde_json::Value::as_object_mut)
        {
            query.remove("runtime");
        }
    }
    value
}

#[test]
fn pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite() {
    const NAME: &str = "storage::postgres::catalog_effects::tests::businessflow::pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite";
    const CHILD: &str = "SEMON_VERTICAL140_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    // Producer parity precedes the process-wide rejection phase. No model
    // output from the oracle is supplied to the PG producer.
    let mut oracle = build_oracle(true, false);
    let namespace = ns("vertical140\0🦀");
    let source = crate::storage::HostSource::new(namespace.clone(), "a".repeat(64)).unwrap();
    let options = Options {
        source,
        claude_home: oracle.root.join("claude"),
        claude_json: oracle.root.join("claude.json"),
        codex_home: oracle.root.join("codex"),
        copilot_home: oracle.root.join("copilot"),
        proc_root: oracle.root.join("proc"),
        cache: oracle.root.join("never-open.sqlite3"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    // Use the ordinary harness contracts, including Copilot's UUID directory
    // and versioned session.start payload (model/tests.rs fixtures).
    let hostname = options.proc_root.join("sys/kernel/hostname");
    fs::create_dir_all(hostname.parent().unwrap()).unwrap();
    fs::write(&hostname, "synthetic-local-before207\n").unwrap();
    let local_authority =
        crate::model::relation_stage::LifecycleAuthority::capture(&options, &EventCache::default()).unwrap();
    assert!(local_authority.current().unwrap());
    fs::write(&hostname, "synthetic-local-after207\n").unwrap();
    assert!(!local_authority.current().unwrap());
    fs::write(&hostname, "synthetic-local-before207\n").unwrap();
    let codex_id = "01234567-89ab-4cde-8123-456789abcdef";
    let copilot_id = "11234567-89ab-4cde-8123-456789abcdef";
    let codex = options
        .codex_home
        .join(format!("sessions/2026/10/01/rollout-{codex_id}.jsonl"));
    let copilot = options
        .copilot_home
        .join(format!("session-state/{copilot_id}/events.jsonl"));
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::create_dir_all(copilot.parent().unwrap()).unwrap();
    fs::write(&codex, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":codex_id,"cwd":"/synthetic/project","originator":"codex_exec","thread_source":"user","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/synthetic/project"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized codex input"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(&copilot, [
        serde_json::json!({"id":"start","type":"session.start","timestamp":"2026-10-01T00:00:00Z","data":{"version":1,"copilotVersion":"1.0.91","sessionId":copilot_id,"context":{"cwd":"/synthetic/project"}}}),
        serde_json::json!({"id":"user","type":"user.message","timestamp":"2026-10-01T00:00:01Z","data":{"content":"sanitized copilot input"}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}",
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"related-spawn","arguments":"{\"task_name\":\"worker\",\"message\":\"go\"}"}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}",
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"related-spawn","output":"{\"task_name\":\"worker\"}"}})).unwrap();
    let child_id = "21234567-89ab-4cde-8123-456789abcdef";
    let child = options
        .codex_home
        .join(format!("sessions/2026/10/01/rollout-{child_id}.jsonl"));
    fs::write(&child, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":child_id,"cwd":"/synthetic/project","originator":"codex_exec","parent_thread_id":codex_id,"thread_source":"subagent","agent_path":"root/worker","agent_nickname":"worker","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized related codex input"}]}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sanitized related codex reply"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    let claude_parent = options.claude_home.join("projects/project/one.jsonl");
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related-agent-call","parentUuid":"a1","timestamp":"2026-10-01T00:00:01Z","message":{"id":"related-agent-call","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"claude-related-spawn","name":"Agent","input":{"description":"sanitized agent task","prompt":"sanitized agent prompt"}}]}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related-agent-result","parentUuid":"related-agent-call","timestamp":"2026-10-01T00:00:02Z","toolUseResult":{"status":"async_launched","agentId":"related154"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"claude-related-spawn","content":"launched","is_error":false}]}})).unwrap();
    let claude_agent = options
        .claude_home
        .join("projects/project/one/subagents/agent-related154.jsonl");
    fs::create_dir_all(claude_agent.parent().unwrap()).unwrap();
    fs::write(&claude_agent,[
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related-user","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized agent prompt"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related-answer","parentUuid":"related-user","timestamp":"2026-10-01T00:00:02Z","message":{"id":"related-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized agent answer"}]}}),
    ].iter().map(|line|format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(claude_agent.with_extension("meta.json"),serde_json::json!({"agentType":"general-purpose","description":"sanitized agent task","toolUseId":"claude-related-spawn"}).to_string()).unwrap();
    // A second connected subagent of the same parent; the catch-up below
    // withdraws both, so their new custodies exceed one bounded advance.
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related191-agent-call","parentUuid":"related-agent-result","timestamp":"2026-10-01T00:00:01Z","message":{"id":"related191-agent-call","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"claude-related191-spawn","name":"Agent","input":{"description":"sanitized agent task","prompt":"sanitized agent prompt"}}]}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related191-agent-result","parentUuid":"related191-agent-call","timestamp":"2026-10-01T00:00:03Z","toolUseResult":{"status":"async_launched","agentId":"related191"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"claude-related191-spawn","content":"launched","is_error":false}]}})).unwrap();
    let claude_agent191 = options
        .claude_home
        .join("projects/project/one/subagents/agent-related191.jsonl");
    fs::create_dir_all(claude_agent191.parent().unwrap()).unwrap();
    fs::write(&claude_agent191,[
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related191-user","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized agent prompt"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related191-answer","parentUuid":"related191-user","timestamp":"2026-10-01T00:00:02Z","message":{"id":"related191-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized agent answer"}]}}),
    ].into_iter().chain((0..75).flat_map(|turn| {
        // A long subagent: its withdrawn retained recipe spans more than two
        // 64-slot custody pages, so the catch-up below resumes several times.
        let parent = if turn == 0 { "related191-answer".to_owned() } else { format!("related191-answer{}", turn - 1) };
        [
            serde_json::json!({"type":"user","sessionId":"one","uuid":format!("related191-user{turn}"),"parentUuid":parent,"timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":format!("sanitized agent follow-up {turn}")}}),
            serde_json::json!({"type":"assistant","sessionId":"one","uuid":format!("related191-answer{turn}"),"parentUuid":format!("related191-user{turn}"),"timestamp":"2026-10-01T00:00:02Z","message":{"id":format!("related191-answer{turn}"),"role":"assistant","model":"claude-test","content":[{"type":"text","text":format!("sanitized agent answer {turn}")}]}}),
        ]
    })).map(|line|format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(claude_agent191.with_extension("meta.json"),serde_json::json!({"agentType":"general-purpose","description":"sanitized agent task","toolUseId":"claude-related191-spawn"}).to_string()).unwrap();
    // A real SendMessage result and peer receipt exercise the matched-sender
    // selector, rather than only proving that an absent receipt stays absent.
    for line in [
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"receipt203-send","parentUuid":"related191-agent-result","timestamp":"2026-10-01T00:00:04Z","message":{"id":"receipt203-send","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"receipt203-call","name":"SendMessage","input":{"to":"related154","message":"sanitized matched receipt203"}}]}}),
        serde_json::json!({"type":"user","sessionId":"one","uuid":"receipt203-result","parentUuid":"receipt203-send","timestamp":"2026-10-01T00:00:04Z","toolUseResult":{"success":true,"msg_id":"receipt203-message"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"receipt203-call","content":"sent","is_error":false}]}}),
    ] {
        writeln!(
            fs::OpenOptions::new()
                .append(true)
                .open(&claude_parent)
                .unwrap(),
            "{line}"
        )
        .unwrap();
    }
    writeln!(fs::OpenOptions::new().append(true).open(&claude_agent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"receipt203-peer","parentUuid":"related-answer","timestamp":"2026-10-01T00:00:05Z","origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/203.sock","name":"sanitized sender203","msg_id":"receipt203-message","body":"sanitized matched receipt203"},"message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/203.sock\" from-name=\"sanitized sender203\">sanitized matched receipt203</cross-session-message>"}})).unwrap();
    // Positive ClaudeName: the native custom-title record names the session.
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"custom-title","sessionId":"one","customTitle":"sanitized name175"})).unwrap();
    // Current upstream copy/resume semantics: the shared UUIDs retain one
    // native send, while the resumed file contributes its own new turn.
    let copied_records = [
        serde_json::json!({"type":"user","sessionId":"copy203-original","uuid":"copy203-user","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized copy203 input"}}),
        serde_json::json!({"type":"assistant","sessionId":"copy203-original","uuid":"copy203-send","parentUuid":"copy203-user","timestamp":"2026-10-01T00:00:01Z","message":{"id":"copy203-send","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"copy203-call","name":"SendMessage","input":{"to":"sanitized external203","message":"sanitized external203 request"}}]}}),
        serde_json::json!({"type":"user","sessionId":"copy203-original","uuid":"copy203-result","parentUuid":"copy203-send","timestamp":"2026-10-01T00:00:01Z","toolUseResult":{"success":true},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"copy203-call","content":"sent","is_error":false}]}}),
    ];
    let original = options
        .claude_home
        .join("projects/project/copy203-original.jsonl");
    let resumed = options
        .claude_home
        .join("projects/project/copy203-resumed.jsonl");
    let mut old = copied_records.to_vec();
    old.push(serde_json::json!({"type":"continued-in","continuedInSessionId":"copy203-resumed","sessionId":"copy203-original","timestamp":"2026-10-01T00:00:02Z"}));
    let mut copy = copied_records.to_vec();
    copy.push(serde_json::json!({"type":"user","sessionId":"copy203-resumed","uuid":"copy203-new","parentUuid":"copy203-result","timestamp":"2026-10-01T00:00:03Z","message":{"role":"user","content":"sanitized resumed203 input"}}));
    for (path, records) in [(original, old), (resumed, copy)] {
        fs::write(
            path,
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    // Larger indexed history with a fixed small receiver append below.
    let padding = format!(
        "{}\n",
        serde_json::json!({"type":"system","subtype":"turn_duration","sessionId":"one","padding":"x".repeat(8192)})
    );
    let mut history = fs::OpenOptions::new()
        .append(true)
        .open(&claude_parent)
        .unwrap();
    for _ in 0..256 {
        history.write_all(padding.as_bytes()).unwrap();
    }
    history.sync_all().unwrap();
    drop(history);
    let mut inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|input| {
            crate::native_evidence::catalog_input(&input.full_path(&options), &options).is_some()
        })
        .collect::<Vec<_>>();
    // The trusted append below targets this exact padded parent, independently
    // of the discovery order of copies, subagents and other harnesses.
    let append_input = inputs
        .iter()
        .position(|input| input.full_path(&options) == claude_parent)
        .expect("padded Claude parent is a discovered native input");
    inputs.swap(0, append_input);
    let mut sqlite_options = options.clone();
    sqlite_options.cache = oracle.root.join("oracle-all140");
    let mut sqlite =
        EventCache::open_scoped(&sqlite_options.cache, &sqlite_options.source).unwrap();
    let expected =
        crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812802000)
            .unwrap();
    assert_eq!(
        expected
            .rows
            .iter()
            .map(|row| row.harness.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["claude", "codex", "copilot"])
    );
    sqlite
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &expected.rows,
                transcripts: Some(&expected.transcripts),
            },
            expected.base_generation.as_deref(),
        )
        .expect("SQLite complete ordinary source publication");
    // Selected body/field reads consume the fully published ordinary Query
    // profile, including its completion and reader binding, not a partial head.
    let mut oracle_query =
        crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
            vec![("vertical140-oracle".into(), sqlite_options.clone())],
        )));
    oracle_query
        .call_at("list_sessions", &serde_json::json!({}), 1790812802000)
        .expect("SQLite fully published body/field oracle");
    let expected_bodies = expected
        .rows
        .iter()
        .map(|row| {
            let body = oracle_query
                .call_at(
                    "read_transcript",
                    &serde_json::json!({"id":row.key,"limit":100}),
                    1790812802000,
                )
                .unwrap();
            assert!(
                body.to_string().contains("sanitized"),
                "ordinary body missing for {} {} {:?}",
                row.harness,
                row.kind,
                row.native_ids
            );
            let session = oracle_query
                .call_at(
                    "get_session",
                    &serde_json::json!({"id":row.key}),
                    1790812802000,
                )
                .expect("SQLite ordinary session fields");
            (
                row.key.clone(),
                serde_json::json!({"body":body,"fields":session}),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    // Recent windows (options.all=false): the ordinary SQLite Query refresh
    // fully publishes a fresh oracle cache; its exact lists are saved before
    // the PG no-SQLite phase.
    let recent_windows = [
        Duration::from_secs(86400),
        Duration::from_secs(86401),
        Duration::from_secs(86400 * 365 * 100),
    ];
    // One day after the fixtures: the cutoffs fall between fixture times.
    let recent_now = 1790812802000 + 86_400_000;
    let recent_machine = "vertical140\0🦀";
    let mut closed_options = options.clone();
    closed_options.cache = oracle.root.join("oracle-query177");
    let oracle_recent = recent_windows
        .iter()
        .map(|since| {
            let mut recent_options = closed_options.clone();
            recent_options.all = false;
            recent_options.since = *since;
            let list = crate::Query::from_sources(std::sync::Arc::new(
                crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
            ))
            .call_at("list_sessions", &serde_json::json!({}), recent_now)
            .expect("SQLite recent-window list");
            (*since, list)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let oracle_recent_fields = recent_windows
        .iter()
        .map(|since| {
            let mut recent_options = closed_options.clone();
            recent_options.all = false;
            recent_options.since = *since;
            let mut query = crate::Query::from_sources(std::sync::Arc::new(
                crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
            ));
            let fields = expected
                .rows
                .iter()
                .map(|row| {
                    let fields = query.call_at(
                        "get_session",
                        &serde_json::json!({"id":row.key}),
                        recent_now,
                    );
                    (row.key.clone(), fields)
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            (*since, fields)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let included_key = &expected.rows.first().expect("ordinary owner fixture").key;
    let included_fields = oracle_recent_fields[&recent_windows[2]][included_key]
        .as_ref()
        .expect("all-history recent window includes the ordinary owner");
    assert!(included_fields["session"].is_object());
    assert_eq!(included_fields["session"]["id"], *included_key);
    let all_total = expected
        .rows
        .iter()
        .flat_map(|row| {
            row.query.as_ref().unwrap()["rows"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u64;
    assert_eq!(
        oracle_recent[&recent_windows[2]]["total"].as_u64(),
        Some(all_total)
    );
    // At least one window has Sessions on both sides of its cutoff.
    assert!(
        oracle_recent.values().any(|list| list["total"]
            .as_u64()
            .is_some_and(|total| total > 0 && total < all_total)),
        "recent-window fixture lacks a boundary: {:?}",
        oracle_recent
            .values()
            .map(|list| &list["total"])
            .collect::<Vec<_>>()
    );
    // Retained-history scope through the ordinary SQLite viewer page over the
    // fully published Query cache; exact ordered keys are saved before the PG
    // no-SQLite phase. Nothing is withdrawn yet, so it equals the current page.
    let mut history_options = closed_options.clone();
    history_options.all = false;
    history_options.since = recent_windows[2];
    let page_keys = |reply: crate::ViewerReply| -> Vec<String> {
        assert_eq!(reply.status, 200);
        let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let oracle_history = page_keys(crate::catalog::session_catalog_page(
        &history_options,
        recent_machine,
        "scope=retained_history&limit=50",
    ));
    let mut ordered = expected.rows.clone();
    ordered.sort_by(|a, b| {
        b.last
            .unwrap_or(0)
            .cmp(&a.last.unwrap_or(0))
            .then(a.key.cmp(&b.key))
    });
    assert_eq!(
        oracle_history,
        ordered
            .iter()
            .map(|row| row.key.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        oracle_history,
        page_keys(crate::catalog::session_catalog_page(
            &history_options,
            recent_machine,
            "limit=50",
        ))
    );
    // P3 oracle: the same connected withdrawal (both subagent Sources
    // disappear) through the ordinary SQLite Query refresh. Retained-only
    // rows, keyset pages and the harness filter are saved before the
    // no-SQLite phase; the Sources are restored byte-identical for PG ingest.
    let stable = |item: &serde_json::Value| {
        let mut picked = serde_json::Map::new();
        for field in [
            "key",
            "harness",
            "kind",
            "lifecycle",
            "name",
            "native_ids",
            "parent",
            "root",
            "repo",
            "branch",
            "model",
            "first",
            "last",
        ] {
            picked.insert(field.into(), item[field].clone());
        }
        serde_json::Value::Object(picked)
    };
    let mut withdraw_options = history_options.clone();
    withdraw_options.cache = oracle.root.join("oracle-withdraw201");
    let refresh = |options: &Options| {
        // Query's private listing completion does not finish Catalog/search
        // publication. Run the ordinary observer to completion before saving
        // the exact retained-search oracle, rather than forging its markers.
        let listing = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), options.clone())]),
        ))
        .call_at("list_sessions", &serde_json::json!({}), recent_now)
        .expect("SQLite withdrawal oracle list");
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        evidence.request_metadata_audit();
        let mut work = evidence.work.lock().unwrap();
        let mut complete = false;
        // Each advance retains its normal bounded work. Qualify completion
        // against the existing background budget, not a source-count-sized
        // number of calls through the producer's multiple durable phases.
        let started = std::time::Instant::now();
        let mut advances = 0;
        while started.elapsed() < Duration::from_secs(60) {
            advances += 1;
            if evidence.refresh_catalog(&mut work, recent_now).unwrap() {
                complete = true;
                break;
            }
            if advances % 32 == 0 {
                let cache = work.events.as_ref().unwrap();
                eprintln!(
                    "ordinary_observer advances={advances} qualified={} pending={} backfill={:?}",
                    cache.qualified_catalog().unwrap(),
                    cache.pending_catalog_sources(1).unwrap().len(),
                    cache.backfill_state().unwrap()
                );
            }
        }
        assert!(complete, "SQLite ordinary Catalog oracle did not finish");
        drop(work);
        listing
    };
    let before_withdrawal = refresh(&withdraw_options)["total"].as_u64().unwrap();
    let sqlite_before_current = page_keys(crate::catalog::session_catalog_page(
        &withdraw_options,
        recent_machine,
        "limit=50",
    ))
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    let sqlite_host = body_host(&options, &expected.rows);
    let parked = [
        (
            claude_agent.clone(),
            oracle.root.join("parked-agent180.jsonl"),
        ),
        (
            claude_agent191.clone(),
            oracle.root.join("parked-agent191.jsonl"),
        ),
    ];
    for (from, to) in &parked {
        fs::rename(from, to).unwrap();
    }
    let after_withdrawal = refresh(&withdraw_options)["total"].as_u64().unwrap();
    assert!(
        after_withdrawal < before_withdrawal,
        "SQLite withdrawal {before_withdrawal} -> {after_withdrawal}"
    );
    let sqlite_scoped = |scope: &str, limit: usize| {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let query = match &cursor {
                Some(cursor) => format!(
                    "{scope}limit={limit}&cursor={}",
                    crate::viewer::percent_encode(cursor)
                ),
                None => format!("{scope}limit={limit}"),
            };
            let reply =
                crate::catalog::session_catalog_page(&withdraw_options, recent_machine, &query);
            assert_eq!(
                reply.status,
                200,
                "{query} code={}",
                serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap()["error"]["code"]
            );
            let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            items.extend(page["items"].as_array().unwrap().iter().cloned());
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break items,
            }
        }
    };
    let sqlite_retained = sqlite_scoped("scope=retained_history&", 50);
    // Keyset pages over equal timestamps continue without gap or overlap.
    assert_eq!(
        sqlite_scoped("scope=retained_history&", 3)
            .iter()
            .map(stable)
            .collect::<Vec<_>>(),
        sqlite_retained.iter().map(stable).collect::<Vec<_>>()
    );
    let sqlite_current = sqlite_scoped("", 50)
        .iter()
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let oracle_withdrawn = sqlite_retained
        .iter()
        .filter(|item| !sqlite_current.contains(item["key"].as_str().unwrap()))
        .map(stable)
        .collect::<Vec<_>>();
    // Retained-history can select an original composite projection for a key
    // that also remains Current (the connected parent). Keep that full oracle
    // separately from the exact keys whose Current membership disappeared.
    let oracle_history_projection = sqlite_retained.iter().map(stable).collect::<Vec<_>>();
    assert!(
        oracle_withdrawn
            .iter()
            .all(|item| item["lifecycle"] == "retained")
    );
    // Query totals also count virtual relationship stubs, whose membership can
    // change when a sender disappears. Native Catalog custody is the exact
    // difference of the native Current key sets, not that Query total delta.
    assert_eq!(
        oracle_withdrawn
            .iter()
            .map(|row| row["key"].as_str().unwrap().to_owned())
            .collect::<std::collections::BTreeSet<_>>(),
        sqlite_before_current
            .difference(&sqlite_current)
            .cloned()
            .collect(),
        "SQLite retained-only rows"
    );
    assert!(
        oracle_withdrawn
            .iter()
            .all(|item| !sqlite_current.contains(item["key"].as_str().unwrap()))
    );
    let oracle_withdrawn_claude = sqlite_scoped("scope=retained_history&harness=claude&", 50)
        .iter()
        .filter(|item| !sqlite_current.contains(item["key"].as_str().unwrap()))
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        oracle_withdrawn_claude,
        oracle_withdrawn
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    let oracle_retained_reads = oracle_withdrawn
        .iter()
        .map(|row| {
            let key = row["key"].as_str().unwrap();
            let args = serde_json::json!({"id":key,"limit":100});
            let body = crate::query::transcript::read(
                &withdraw_options,
                key,
                &args,
                Some(&sqlite_host),
                crate::SessionSourceReadMode::LocalThenProvider,
                1790812802000,
            )
            .unwrap()
            .expect("SQLite retained body through preserved Raw custody");
            let fields = crate::query::transcript::session(
                &withdraw_options,
                key,
                &serde_json::json!({"id":key}),
                Some(&sqlite_host),
                crate::SessionSourceReadMode::LocalThenProvider,
                1790812802000,
            )
            .unwrap()
            .expect("SQLite retained fields through preserved Raw custody");
            (
                key.to_owned(),
                serde_json::json!({"body":body,"fields":fields}),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let oracle_retained_search = oracle_withdrawn
        .iter()
        .map(|row| {
            let key = row["key"].as_str().unwrap();
            let query = format!(
                "scope=retained_history&q={}&",
                crate::viewer::percent_encode(&key.to_lowercase())
            );
            let keys = sqlite_scoped(&query, 50)
                .iter()
                .map(|item| item["key"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            assert!(keys.contains(&key.to_owned()));
            (key.to_owned(), keys)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for (from, to) in &parked {
        fs::rename(to, from).unwrap();
    }
    // Rename preserves bytes but changes the physical ctime witness. Capture
    // the SQLite baseline from the restored Sources before NoSQLiteGuard, so
    // PG must match the same observation, including all proof fields.
    let expected =
        crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812802000)
            .unwrap();
    // The retained-row oracle consumes the same restored physical witnesses.
    // Its order remains the independently observed SQLite page order above.
    ordered = oracle_history
        .iter()
        .map(|key| {
            expected
                .rows
                .iter()
                .find(|row| &row.key == key)
                .unwrap()
                .clone()
        })
        .collect();
    oracle.rows = expected.rows;
    oracle.projections = expected.transcripts;
    drop(sqlite);
    let raw = inputs
        .iter()
        .map(|i| fs::read(i.full_path(&options)).unwrap())
        .collect::<Vec<_>>();
    let _guard = crate::storage::postgres::native_ingest_tests::Guard::new();
    let (pool, factory) = fixture(|limits| limits.max_connections = 1);
    let control = PgCancellation::default();
    let config = options
        .source
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    config.create_native_store().unwrap();
    assert_eq!(
        config.create_native_store().err(),
        Some(StorageError::Conflict)
    );
    let mut pg = EventCache::open_configured(&config).unwrap();
    for (at, input) in inputs.iter().enumerate() {
        pg.ingest(
            &format!("{:064x}", at + 1),
            &input.full_path(&options),
            input.root.as_str(),
        )
        .unwrap();
    }
    let prepared =
        crate::model::prepare_sources(&options, &mut pg, &inputs, 1790812802000).unwrap();
    assert_eq!(prepared.rows.len(), oracle.rows.len());
    assert_eq!(producer_rows(&prepared.rows), producer_rows(&oracle.rows));
    let copied = prepared
        .rows
        .iter()
        .find(|row| row.native_ids.iter().any(|id| id == "copy203-resumed"))
        .unwrap();
    assert!(copied.native_ids.iter().any(|id| id == "copy203-original"));
    assert_eq!(copied.sources.len(), 2);
    assert!(
        copied.query.as_ref().unwrap()["rows"]
            .as_object()
            .unwrap()
            .values()
            .any(|row| row["summary"]["kind"] == "stub"),
        "missing external recipient description"
    );
    assert!(
        serde_json::to_string(&prepared.rows)
            .unwrap()
            .contains("sanitized name175")
    );
    assert_eq!(prepared.transcripts.len(), oracle.projections.len());
    for (actual, expected) in prepared.transcripts.iter().zip(&oracle.projections) {
        assert_eq!(
            serde_json::to_value(&actual.slots).unwrap(),
            serde_json::to_value(&expected.slots).unwrap()
        );
    }
    let binding = pg.query_runtime_binding().unwrap().unwrap();
    let runtime = binding
        .witness(&crate::native_evidence::query_runtime_generation(&options).unwrap())
        .unwrap();
    for row in &prepared.rows {
        if let Some(query) = &row.query {
            assert_eq!(query["runtime"].as_str(), Some(runtime.as_str()));
        }
    }
    // Drive the SAME ordinary queued producer against durable PG staging.
    let paths = inputs
        .iter()
        .map(|input| input.full_path(&options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let stage_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    assert!(
        pg.begin_semantic_stage(
            &(0..65).map(|i| format!("oversize-{i}")).collect::<Vec<_>>(),
            &stage_runtime
        )
        .is_err()
    );
    assert_eq!(
        pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(),
        crate::events::Outcome::Written
    );
    let mut stages = Vec::new();
    let (published, semantic_batch, semantic_model) = loop {
        let batch = pg.semantic_batch(64).unwrap().unwrap();
        stages.push(batch.phase);
        // Reopen mid-stage: resume must preserve the exact phase and epoch.
        if batch.phase == crate::model::relation_stage::Phase::Identity {
            drop(pg);
            pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone())
                .unwrap();
            assert_eq!(
                pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(),
                crate::events::Outcome::Written
            );
            let resumed = pg.semantic_batch(64).unwrap().unwrap();
            assert_eq!(resumed.epoch, batch.epoch);
            assert_eq!(resumed.phase, batch.phase);
        }
        let model = pg
            .prepare_semantic_model_batch(&options, &inputs, &batch, 1790812802000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Relations {
            use crate::model::relation_stage::Selector;
            let selected = pg
                .semantic_select(
                    batch.epoch,
                    &Selector::ClaudeCall {
                        id: "claude-related-spawn".into(),
                    },
                )
                .unwrap();
            assert_eq!(selected.count, 1);
            assert!(
                selected
                    .endpoint
                    .unwrap()
                    .row
                    .native_ids
                    .iter()
                    .any(|id| id == "one")
            );
            assert!(selected.location.is_some());
            let received = pg
                .semantic_select(
                    batch.epoch,
                    &Selector::ClaudeReceipt {
                        id: "receipt203-message".into(),
                    },
                )
                .unwrap();
            assert_eq!(received.count, 1);
            // Native locations retain the physical SourceFile path, just as
            // Location::local requires when checking the consumed witness.
            assert_eq!(received.location.unwrap().source.path, claude_agent);
            assert_eq!(
                pg.semantic_select(
                    batch.epoch,
                    &Selector::ClaudeReceipt {
                        id: "absent-receipt".into()
                    }
                )
                .unwrap()
                .count,
                0
            );
            assert_eq!(
                pg.semantic_select(
                    batch.epoch,
                    &Selector::ClaudeName {
                        name: "absent-native-name".into()
                    }
                )
                .unwrap()
                .count,
                0
            );
        }
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let mut mismatched = batch.clone();
            mismatched
                .context
                .companions
                .push("altered full context".into());
            assert!(
                pg.publish_semantic_catalog(&mismatched, model.prepared())
                    .is_err()
            );
            let ack = pg
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            assert_eq!(
                pg.publish_semantic_catalog(&batch, model.prepared())
                    .unwrap(),
                ack
            );
            break (ack, batch, model);
        }
    };
    assert_eq!(stages.len(), 4);
    assert_eq!(
        producer_rows(&semantic_model.prepared().rows),
        producer_rows(&oracle.rows)
    );
    assert!(
        pg.note_catalog_audit(published.1.as_deref(), 1790812802000)
            .unwrap(),
        "committed publication closed its active stage before the finish ACK"
    );
    assert_eq!(
        pg.finish_semantic_stage(semantic_batch.epoch).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(published.0, crate::events::Outcome::Written);
    let generation = published.1.unwrap();
    assert!(!pg.note_catalog_audit(Some("stale"), 1790812802000).unwrap());
    assert!(
        pg.note_catalog_audit(Some(&generation), 1790812802000)
            .unwrap()
    );
    let component = pg
        .catalog_component_paths(
            &[inputs
                .iter()
                .find(|input| input.full_path(&options) == codex)
                .unwrap()
                .clone()],
            &[],
            &[("codex".into(), codex_id.into())],
            &options,
        )
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|input| input.full_path(&options))
        .collect::<Vec<_>>();
    assert!(component.contains(&codex));
    assert!(component.contains(&child));
    // A lost response retries the identical full request, even though its
    // expected head and acknowledged claims have advanced.
    assert_eq!(
        pg.publish_semantic_catalog(&semantic_batch, semantic_model.prepared())
            .unwrap()
            .1
            .as_deref(),
        Some(generation.as_str())
    );
    let mut competing = prepared.rows.clone();
    competing[0].name.push_str(" competing publisher");
    assert!(
        pg.publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &competing,
                transcripts: Some(&prepared.transcripts)
            },
            prepared.base_generation.as_deref()
        )
        .is_err()
    );
    let page = config.catalog_page(&options, "limit=20").unwrap();
    assert_eq!(page.status, 200);
    let page: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
    assert!(page.to_string().contains(&prepared.rows[0].key));
    // P3: retained history over the admitted PG generation equals the SQLite
    // oracle page: exact ordered keys and full producer rows, then a keyset
    // second page continues without overlap.
    let (history_rows, history_page, history_next) =
        super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            &generation,
            |snapshot| {
                let request =
                    crate::catalog::Request::parse("scope=retained_history&limit=50").unwrap();
                let rows = snapshot
                    .rows(&request)
                    .map_err(|_| StorageError::ScopeChanged)?;
                let page = crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &request,
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                let first = crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &crate::catalog::Request::parse("scope=retained_history&limit=1").unwrap(),
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                Ok((rows, page, first))
            },
        )
        .unwrap();
    assert_eq!(page_keys(history_page), oracle_history);
    assert_eq!(producer_rows(&history_rows), producer_rows(&ordered));
    assert!(
        history_rows
            .iter()
            .all(|row| row.lifecycle == crate::model::summary::CatalogLifecycle::Current)
    );
    let first: serde_json::Value = serde_json::from_slice(&history_next.body).unwrap();
    assert_eq!(
        first["items"][0]["key"].as_str(),
        oracle_history.first().map(String::as_str)
    );
    if oracle_history.len() > 1 {
        let cursor = first["next_cursor"]
            .as_str()
            .expect("retained history keyset cursor");
        let second = super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            &generation,
            |snapshot| {
                crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &crate::catalog::Request::parse(&format!(
                        "scope=retained_history&limit=50&cursor={}",
                        crate::viewer::percent_encode(cursor)
                    ))
                    .unwrap(),
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)
            },
        )
        .unwrap();
        assert_eq!(page_keys(second), oracle_history[1..].to_vec());
    }
    let query_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let long_recipe = prepared
        .transcripts
        .iter()
        .find(|row| row.key == "related191")
        .unwrap();
    assert_eq!(long_recipe.slots.len(), 152);
    assert!(
        crate::storage::query::QueryCatalogStore::has_search_slots(
            &query_store,
            &namespace,
            &long_recipe.key,
            None,
        )
        .unwrap()
    );
    assert!(
        !crate::storage::query::QueryCatalogStore::has_search_slots(
            &query_store,
            &namespace,
            &long_recipe.key,
            Some(i64::MAX),
        )
        .unwrap()
    );
    let mut query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(namespace.as_str().into(), options.clone())]),
    ));
    let query_list = query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812802000,
            &query_store,
        )
        .unwrap();
    assert_eq!(query_list["total"].as_u64(), Some(all_total));
    // Recent window (options.all=false): PG cohort equals the SQLite oracle.
    for since in recent_windows {
        let mut recent_options = options.clone();
        recent_options.all = false;
        recent_options.since = since;
        let recent_store = super::super::snapshot::query::QueryCatalog {
            pool: pool.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.clone(),
            options: recent_options.clone(),
        };
        let recent = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
        ))
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            recent_now,
            &recent_store,
        )
        .unwrap();
        let expected = &oracle_recent[&since];
        let ids = |list: &serde_json::Value| {
            list["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            recent["total"], expected["total"],
            "recent window {since:?}"
        );
        assert_eq!(ids(&recent), ids(expected), "recent window ids {since:?}");
        assert_eq!(
            recent["sessions"], expected["sessions"],
            "recent window rows {since:?}"
        );
    }
    super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            for row in &prepared.rows {
                let observation = snapshot
                    .transcript_observation(&row.key, &generation, false)
                    .map_err(|_| StorageError::Unavailable)?
                    .unwrap();
                assert!(observation["total"].as_u64().is_some());
                let details = snapshot
                    .selected_details(
                        &row.key,
                        &row.sources,
                        &mut |_| serde_json::json!({"observation":"unavailable"}),
                    )
                    .map_err(|_| StorageError::Unavailable)?;
                assert!(details.is_some());
            }
            Ok(())
        },
    )
    .unwrap();
    type BodySources = std::collections::BTreeMap<
        (String, String),
        (crate::SessionSourceRef, std::sync::Arc<[u8]>),
    >;
    struct BodyHost(BodySources);
    impl crate::SessionSourceReader for BodyHost {
        fn read_range(
            &self,
            reference: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            let (saved, bytes) = self
                .0
                .get(&(reference.root.clone(), reference.path.clone()))
                .ok_or_else(|| std::io::Error::other("host denied"))?;
            if saved != reference
                || expected.is_some_and(|pin| pin != "body-fixture")
                || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
            {
                return Err(std::io::Error::other("host denied"));
            }
            let start = usize::try_from(offset).map_err(std::io::Error::other)?;
            let end = bytes.len().min(start.saturating_add(max));
            Ok(crate::SessionSourceRange {
                generation: "body-fixture".into(),
                length: bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(
                    bytes
                        .get(start..end)
                        .ok_or_else(|| std::io::Error::other("host range"))?,
                ),
                cached: false,
            })
        }
    }
    fn body_host(options: &Options, rows: &[CatalogRow]) -> BodyHost {
        BodyHost(
            rows.iter()
                .flat_map(|row| {
                    row.sources.iter().map(|source| {
                        let path = crate::catalog::source_relative(&options, &row.harness, source)
                            .unwrap();
                        let reference = crate::SessionSourceRef {
                            root: row.harness.clone(),
                            path: path.clone(),
                            native_id: source.native_id.clone(),
                            offset: source.offset,
                            prefix_sha256: source.prefix_sha256,
                            tail_sha256: source.tail_sha256,
                        };
                        (
                            (row.harness.clone(), path),
                            (
                                reference,
                                std::sync::Arc::from(fs::read(&source.path).unwrap()),
                            ),
                        )
                    })
                })
                .collect(),
        )
    }
    let retained_host = body_host(&options, &prepared.rows);
    for since in recent_windows {
        let mut recent_options = options.clone();
        recent_options.all = false;
        recent_options.since = since;
        let store = super::super::snapshot::query::QueryCatalog {
            pool: pool.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.clone(),
            options: recent_options.clone(),
        };
        let mut query = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
        ));
        for (key, expected) in &oracle_recent_fields[&since] {
            let fields = query.call_bound_selected_at(
                "get_session",
                &serde_json::json!({"id":key}),
                recent_now,
                &store,
                &retained_host,
            );
            assert_eq!(
                &fields, expected,
                "recent-window selected fields {since:?} {key}"
            );
        }
    }
    let read_bodies = |query: &mut crate::Query,
                       store: &super::super::snapshot::query::QueryCatalog,
                       host: &BodyHost| {
        prepared
            .rows
            .iter()
            .map(|row| {
                let body = query
                    .call_bound_selected_at(
                        "read_transcript",
                        &serde_json::json!({"id":row.key,"limit":100}),
                        1790812802000,
                        store,
                        host,
                    )
                    .unwrap();
                let fields = query
                    .call_bound_selected_at(
                        "get_session",
                        &serde_json::json!({"id":row.key}),
                        1790812802000,
                        store,
                        host,
                    )
                    .unwrap();
                (
                    row.key.clone(),
                    serde_json::json!({"body":body,"fields":fields}),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    struct DeniedHost;
    impl crate::SessionSourceReader for DeniedHost {
        fn read_range(
            &self,
            _: &crate::SessionSourceRef,
            _: Option<&str>,
            _: u64,
            _: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            Err(std::io::Error::other("host denied"))
        }
    }
    assert!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":prepared.rows[0].key}),
                1790812802000,
                &query_store,
                &DeniedHost
            )
            .is_err(),
        "Catalog metadata must not grant Raw or return blank success"
    );

    // Only the request-owned host can grant fixture bytes. The PG reader adds
    // persisted native proof verification to the ordinary scalar-field parser.
    struct Host {
        source: crate::SessionSourceRef,
        bytes: std::sync::Arc<[u8]>,
    }
    impl crate::SessionSourceReader for Host {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            if source != &self.source
                || expected.is_some_and(|g| g != "authorized-fixture")
                || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
            {
                return Err(std::io::Error::other("fixture authority mismatch"));
            }
            let at = usize::try_from(offset).map_err(std::io::Error::other)?;
            let end = self.bytes.len().min(at.saturating_add(max));
            let bytes = self
                .bytes
                .get(at..end)
                .ok_or_else(|| std::io::Error::other("fixture range"))?;
            Ok(crate::SessionSourceRange {
                generation: "authorized-fixture".into(),
                length: self.bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(bytes),
                cached: false,
            })
        }
    }
    let (projection, slot) = prepared
        .transcripts
        .iter()
        .find_map(|projection| {
            projection
                .slots
                .iter()
                .find(|slot| {
                    matches!(slot.recipe, crate::slot_projection::Recipe::U)
                        && slot.field.is_some()
                        && slot.file.is_some()
                })
                .map(|slot| (projection, slot))
        })
        .expect("ordinary producer must capture a selected native user field");
    let selected = prepared
        .rows
        .iter()
        .find(|row| row.key == projection.key)
        .unwrap();
    let source = selected
        .sources
        .iter()
        .find(|source| source.path == slot.file.as_ref().unwrap().path)
        .unwrap();
    let input = inputs
        .iter()
        .find(|input| input.full_path(&options) == source.path)
        .unwrap();
    let reference = crate::SessionSourceRef {
        root: input.root.as_str().to_owned(),
        path: input.path.clone(),
        native_id: source.native_id.clone(),
        offset: source.offset,
        prefix_sha256: source.prefix_sha256,
        tail_sha256: source.tail_sha256,
    };
    let host = Host {
        source: reference.clone(),
        bytes: std::sync::Arc::from(fs::read(&source.path).unwrap()),
    };
    let field = slot.field.as_ref().unwrap();
    let expected_field =
        crate::native_field::preview(&host, &reference, field, "prompt", &mut (1024 * 1024))
            .unwrap();
    let read_selected = || {
        super::super::selected_reader::with_reader(
            &pool,
            &namespace,
            &control,
            &generation,
            source,
            &reference,
            &host,
            |reader| {
                crate::native_field::preview(
                    reader,
                    &reference,
                    field,
                    "prompt",
                    &mut (1024 * 1024),
                )
            },
        )
        .unwrap()
    };
    assert_eq!(read_selected(), expected_field);
    drop(pg);
    let mut resumed = EventCache::open_configured(&config).unwrap();
    let after = resumed.query_runtime_binding().unwrap().unwrap();
    assert_eq!(binding.admission, after.admission);
    assert_eq!(binding.stream_epoch, after.stream_epoch);
    assert_eq!(
        resumed.session_catalog_generation().as_deref(),
        Some(generation.as_str())
    );
    let repeated =
        crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812802000).unwrap();
    assert_eq!(
        serde_json::to_value(&repeated.rows).unwrap(),
        serde_json::to_value(&prepared.rows).unwrap()
    );
    for (input, original) in inputs.iter().zip(&raw) {
        assert_eq!(&fs::read(input.full_path(&options)).unwrap(), original);
    }
    let restarted_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let mut restarted_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(namespace.as_str().into(), options.clone())]),
    ));
    assert_eq!(
        read_bodies(&mut restarted_query, &restarted_store, &retained_host),
        expected_bodies
    );
    use std::io::Write;
    let path = inputs[0].full_path(&options);
    let append = format!(
        "{}\n",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"u2","parentUuid":"a1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized continuation"}})
    );
    let raw_store = || {
        crate::storage::postgres::index_store::open(
            pool.clone(),
            namespace.clone(),
            control.clone(),
        )
        .unwrap()
        .0
    };
    assert!(
        raw[0].len() > 2 * 1024 * 1024,
        "trusted append fixture must have a large old prefix"
    );
    let io_before = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get);
    let hash_before = crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get);
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        Some("0123456789abcdef0123456789abcdef"),
        raw[0].len() as u64,
        raw_store(),
        None,
    )
    .unwrap();
    let raw_intent = writer.intent();
    assert!(
        raw_intent.continuation.is_some(),
        "native durable continuation must qualify trusted append"
    );
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert!(
            crate::storage::postgres::native_keys::read_meta(
                tx,
                format!("raw-write-intent:{}", raw_intent.operation).as_bytes(),
                131072
            )?
            .is_some()
        );
        Ok(())
    })
    .unwrap();
    drop(writer); // Fault: durable SQL intent, no Raw bytes yet.
    assert_eq!(fs::read(&path).unwrap(), raw[0]);
    let mut mismatch = raw_intent.clone();
    mismatch.from += 1;
    assert!(
        crate::SessionSourceWrite::begin_with_store(
            &options,
            &inputs[0],
            None,
            0,
            raw_store(),
            Some(mismatch)
        )
        .is_err()
    );
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(raw_intent.clone()),
    )
    .unwrap();
    let mut raw_file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    raw_file.write_all(append.as_bytes()).unwrap();
    raw_file.sync_all().unwrap();
    drop(raw_file);
    drop(writer); // Fault: durable bytes, completion transaction not attempted.
    let written = raw[0].len() as u64 + append.len() as u64;
    assert_eq!(
        resumed
            .ingest(&format!("{:064x}", 158), &path, "claude")
            .err(),
        Some(StorageError::Conflict),
        "unacknowledged Raw bytes cannot enter native SQL"
    );
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(raw_intent.clone()),
    )
    .unwrap();
    writer.complete(written).unwrap();
    // Fault: SQL committed but the receiver lost the ACK. The exact request
    // reconciles before head/ledger CAS and reproduces the same completion.
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(raw_intent.clone()),
    )
    .unwrap();
    writer.complete(written).unwrap();
    let read_bytes = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get) - io_before;
    let hash_bytes = crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get) - hash_before;
    assert!(
        read_bytes < 512 * 1024,
        "trusted SourceWrite read {read_bytes} bytes"
    );
    assert!(
        hash_bytes < 512 * 1024,
        "trusted SourceWrite hashed {hash_bytes} bytes"
    );
    let c = raw_intent.continuation.as_ref().unwrap();
    let mut source = crate::sealed::LogFile::open(&path).unwrap();
    assert_eq!(
        crate::source_chunks::continue_prefix(
            &mut source,
            &c.state,
            c.offset,
            c.prefix,
            written,
            || Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancel"
            ))
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::Interrupted
    );
    eprintln!(
        "sourcewrite160 old_bytes={} delta={} read_bytes={read_bytes} hash_bytes={hash_bytes} history_io_under_sql_lock=0",
        raw[0].len(),
        append.len()
    );
    let mut stale = raw_intent;
    stale.operation.push_str("-stale");
    stale.head = Some("stale-head".into());
    assert!(
        crate::SessionSourceWrite::begin_with_store(
            &options,
            &inputs[0],
            None,
            0,
            raw_store(),
            Some(stale)
        )
        .is_err()
    );
    let receipt = resumed
        .ingest(&format!("{:064x}", 128), &path, "claude")
        .unwrap();
    let continued =
        crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812803000).unwrap();
    assert_eq!(continued.rows.len(), prepared.rows.len());
    assert_eq!(continued.rows[0].key, prepared.rows[0].key);
    assert_eq!(
        receipt.consumed_bytes,
        raw[0].len() as u64 + append.len() as u64
    );
    assert_eq!(
        continued
            .rows
            .iter()
            .flat_map(|row| &row.sources)
            .find(|source| source.path == path)
            .unwrap()
            .offset,
        receipt.consumed_bytes
    );
    let append_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    resumed
        .begin_semantic_stage(&paths, &append_runtime)
        .unwrap();
    let continued_generation = loop {
        let batch = resumed.semantic_batch(64).unwrap().unwrap();
        let model = resumed
            .prepare_semantic_model_batch(&options, &inputs, &batch, 1790812803000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let ack = resumed
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            resumed.finish_semantic_stage(batch.epoch).unwrap();
            break ack.1.unwrap();
        }
    };
    let next_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: continued_generation.clone(),
        options: options.clone(),
    };
    let next_host = body_host(&options, &continued.rows);
    let next_bodies = read_bodies(&mut restarted_query, &next_store, &next_host);
    assert!(
        next_bodies
            .values()
            .any(|body| body.to_string().contains("sanitized continuation"))
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old authorized body must survive successor publication"
    );
    assert_ne!(continued_generation, generation);
    assert_eq!(
        read_selected(),
        expected_field,
        "retained selected proof must survive a newer publication"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        [raw[0].as_slice(), append.as_bytes()].concat()
    );
    assert!(!options.cache.exists());
    let (mut cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let input_path = options
        .claude_home
        .join("projects/p/source.meta.json")
        .to_string_lossy()
        .into_owned();
    let mut facts_options = options.clone();
    facts_options.facts = Some(oracle.root.join("facts-dispatch-only"));
    cache
        .resume_runtime_input(&facts_options, &input_path)
        .unwrap();
    assert!(!options.cache.exists());
    let input_revision = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::super::super::source_journal::invalidate(tx,&input_path)?;
        Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(),&input_path.as_bytes()]).map_err(sqlstate::error)?.get::<_,i64>(0))
    }).unwrap();
    assert_eq!(
        cache
            .resolve_catalog_input(&input_path, input_revision + 1, None, None)
            .unwrap(),
        crate::events::Outcome::Conflict
    );
    assert_eq!(
        cache
            .resolve_catalog_input(
                &input_path,
                input_revision,
                Some(("codex", codex_id)),
                Some(codex.to_str().unwrap())
            )
            .unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        cache
            .resolve_catalog_input(
                &input_path,
                input_revision,
                Some(("codex", codex_id)),
                Some(codex.to_str().unwrap())
            )
            .unwrap(),
        crate::events::Outcome::Written,
        "exact lost-ACK replay"
    );
    assert_eq!(
        cache
            .resolve_catalog_input(&input_path, input_revision, Some(("codex", child_id)), None)
            .unwrap(),
        crate::events::Outcome::Conflict,
        "a changed full request cannot replay the ACK"
    );
    let key = "opaque\0🦀".repeat(512);
    cache
        .save_session_descriptions(&[(key.clone(), 1, "inputs".into(), "metadata".into())])
        .unwrap();
    assert_eq!(
        cache
            .session_description(&key, 1, "inputs")
            .unwrap()
            .as_deref(),
        Some("metadata")
    );
    assert!(
        cache
            .session_description(&key, 2, "inputs")
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .session_description(&key, 1, "other")
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .save_session_descriptions(&[(
                key.clone(),
                1,
                "inputs".into(),
                "x".repeat(1024 * 1024)
            )])
            .is_err()
    );
    assert_eq!(
        cache
            .session_description(&key, 1, "inputs")
            .unwrap()
            .as_deref(),
        Some("metadata")
    );
    // Disappearance withdraws Current membership, without consuming the host's
    // independently retained bytes or any older Catalog/native proof.
    let missing = copilot.to_str().unwrap().to_owned();
    let missing_ledger = cache.ledger(&missing).unwrap().unwrap();
    let retained_copy = oracle.root.join("retained-copilot156.jsonl");
    fs::rename(&copilot, &retained_copy).unwrap();
    let removal = vec![(missing.clone(), Some(missing_ledger.clone()))];
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written
    );
    let withdrawn_generation = cache.session_catalog_generation().unwrap().unwrap();
    assert_ne!(withdrawn_generation, continued_generation);
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written,
        "lost removal ACK"
    );
    assert_eq!(
        cache.session_catalog_generation().unwrap().as_deref(),
        Some(withdrawn_generation.as_str())
    );
    let mut stale = missing_ledger;
    stale.offset += 1;
    assert_eq!(
        cache
            .remove_files(&[(missing.clone(), Some(stale))])
            .unwrap(),
        crate::events::Outcome::Conflict
    );
    let withdrawn_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: withdrawn_generation.clone(),
        options: options.clone(),
    };
    let current = query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812802000,
            &withdrawn_store,
        )
        .unwrap();
    assert_eq!(
        current["total"].as_u64(),
        Some((continued.rows.len() - 1) as u64)
    );
    let removed_key = continued
        .rows
        .iter()
        .find(|r| r.harness == "copilot")
        .unwrap()
        .key
        .clone();
    assert!(!current.to_string().contains(&removed_key));
    assert!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":removed_key,"limit":100}),
                1790812802000,
                &withdrawn_store,
                &retained_host
            )
            .is_err()
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    drop(cache);
    let (mut cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "withdrawal preserves authorized old body after reopen"
    );
    assert_eq!(
        fs::read(&retained_copy).unwrap(),
        retained_host
            .0
            .values()
            .find(|(r, _)| r.root == "copilot")
            .unwrap()
            .1
            .as_ref()
    );
    let archived = options
        .codex_home
        .join(format!("archived_sessions/rollout-{codex_id}.jsonl"));
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    let codex_bytes = fs::read(&codex).unwrap();
    fs::rename(&codex, &archived).unwrap();
    let moved_stat = crate::events::Stat::of(&archived).unwrap();
    let mut moved_file = crate::sealed::LogFile::open(&archived).unwrap();
    let mut timing = crate::native_timing::Measurements::new(false);
    assert_eq!(
        cache
            .relocate_codex(
                archived.to_str().unwrap(),
                &moved_stat,
                codex_id,
                &mut moved_file,
                &mut timing
            )
            .unwrap()
            .as_deref(),
        codex.to_str()
    );
    assert_eq!(
        cache
            .relocate_codex(
                archived.to_str().unwrap(),
                &moved_stat,
                codex_id,
                &mut moved_file,
                &mut timing
            )
            .unwrap()
            .as_deref(),
        codex.to_str(),
        "exact relocation ACK replay"
    );
    assert_eq!(fs::read(&archived).unwrap(), codex_bytes);
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old authorized body survives native relocation"
    );
    drop(cache);
    let moved_inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|i| {
            crate::native_evidence::catalog_input(&i.full_path(&options), &options).is_some()
        })
        .collect::<Vec<_>>();
    let moved_paths = moved_inputs
        .iter()
        .map(|i| i.full_path(&options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut moved_cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let moved_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    moved_cache
        .begin_semantic_stage(&moved_paths, &moved_runtime)
        .unwrap();
    let (moved_generation, moved_rows) = loop {
        let batch = moved_cache.semantic_batch(64).unwrap().unwrap();
        let model = moved_cache
            .prepare_semantic_model_batch(&options, &moved_inputs, &batch, 1790812803000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let rows = model.prepared().rows.clone();
            let ack = moved_cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            moved_cache.finish_semantic_stage(batch.epoch).unwrap();
            break (ack.1.unwrap(), rows);
        }
    };
    assert_eq!(
        moved_rows
            .iter()
            .filter(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
            .count(),
        1
    );
    let moved_owner = moved_rows
        .iter()
        .find(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
        .unwrap();
    assert_eq!(
        moved_owner.key,
        continued
            .rows
            .iter()
            .find(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
            .unwrap()
            .key
    );
    let moved_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: moved_generation,
        options: options.clone(),
    };
    let moved_host = body_host(&options, &moved_rows);
    assert_eq!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":moved_owner.key,"limit":100}),
                1790812802000,
                &moved_store,
                &moved_host
            )
            .unwrap(),
        expected_bodies[&moved_owner.key]["body"]
    );
    drop(moved_cache);
    let mut moved_cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&archived).unwrap(),"{}",serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:03Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sanitized relocation continuation"}]}})).unwrap();
    moved_cache
        .ingest(&format!("{:064x}", 156), &archived, "codex")
        .unwrap();
    let appended =
        crate::model::prepare_sources(&options, &mut moved_cache, &moved_inputs, 1790812804000)
            .unwrap();
    assert!(appended.rows.iter().any(|r| r.key == moved_owner.key));
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    let (_cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let collision = ns("vertical140\0🦀other");
    let (other, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        collision.clone(),
        control.clone(),
    )
    .unwrap();
    assert!(
        other
            .session_description(&key, 1, "inputs")
            .unwrap()
            .is_none()
    );
    let other_binding = other.query_runtime_binding().unwrap().unwrap();
    assert_ne!(binding.stream_epoch, other_binding.stream_epoch);
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(matches!(
        crate::storage::postgres::index_store::open(pool.clone(), collision, cancelled),
        Err(StorageError::Busy)
    ));
    let facts_path = oracle.root.join("positive-facts158.json");
    let mut recorded = crate::Facts {
        version: 2,
        hostname: "source-write-before158".into(),
        ..Default::default()
    };
    crate::save_json_file(&facts_path, &recorded).unwrap();
    let mut facts_options = options.clone();
    facts_options.facts = Some(facts_path.clone());
    let before_runtime = crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
    // A selected read cannot reuse a publication under different runtime
    // Facts, even when its immutable Raw recipes and host custody still match.
    let changed_runtime_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: facts_options.clone(),
    };
    assert!(
        crate::storage::query::QueryCatalogStore::selected_body(
            &changed_runtime_store,
            &namespace,
            &prepared.rows[0].key,
            &serde_json::json!({"limit":1}),
            1790812802000,
            &retained_host,
        )
        .is_err()
    );
    let from = fs::metadata(&path).unwrap().len();
    let writer = crate::SessionSourceWrite::begin_with_store(
        &facts_options,
        &inputs[0],
        None,
        from,
        raw_store(),
        None,
    )
    .unwrap();
    let facts_intent = writer.intent();
    let extra = format!(
        "{}\n",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"positive-facts158","parentUuid":"u2","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:04Z","message":{"role":"user","content":"positive Facts source continuation"}})
    );
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(extra.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    writer.complete(from + extra.len() as u64).unwrap();
    let replay = crate::SessionSourceWrite::begin_with_store(
        &facts_options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(facts_intent),
    )
    .unwrap();
    replay.complete(from + extra.len() as u64).unwrap();
    let pid_input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "sessions/4242.json".into(),
    };
    fs::create_dir_all(options.claude_home.join("sessions")).unwrap();
    let pid_write = crate::SessionSourceWrite::begin_with_store(
        &facts_options,
        &pid_input,
        None,
        0,
        raw_store(),
        None,
    )
    .unwrap();
    let pid_bytes=serde_json::to_vec(&serde_json::json!({"sessionId":"one","procStart":1234,"startedAt":1790812800000i64,"status":"waiting","waitingFor":"input needed"})).unwrap();
    fs::write(pid_input.full_path(&facts_options), &pid_bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(pid_input.full_path(&facts_options))
        .unwrap()
        .sync_all()
        .unwrap();
    pid_write.complete(pid_bytes.len() as u64).unwrap();
    recorded.hostname = "source-write-after158".into();
    recorded.proc_starts.insert(4242, 1234);
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    let facts_request = facts_writer.intent();
    drop(facts_writer); // Intent/no Facts bytes.
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        Some(facts_request.clone()),
    )
    .unwrap();
    crate::save_json_file(&facts_path, &recorded).unwrap();
    drop(facts_writer); // Bytes/no SQL completion.
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        Some(facts_request.clone()),
    )
    .unwrap();
    facts_writer.complete().unwrap();
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        Some(facts_request.clone()),
    )
    .unwrap();
    facts_writer.complete().unwrap(); // Lost ACK.
    let mut stale_facts = facts_request.clone();
    stale_facts.operation.push_str("-stale");
    stale_facts.head = Some("stale".into());
    assert!(
        crate::SessionFactsWrite::begin_with_store(
            &facts_options,
            &recorded,
            true,
            raw_store(),
            Some(stale_facts)
        )
        .is_err()
    );
    let mut wrong = facts_request;
    wrong.before.hostname.push_str("-wrong");
    assert!(
        crate::SessionFactsWrite::begin_with_store(
            &facts_options,
            &recorded,
            true,
            raw_store(),
            Some(wrong)
        )
        .is_err()
    );
    assert!(
        crate::model::pid_files(
            &facts_options,
            &crate::facts::MachineFacts::Recorded(Box::new(recorded.clone()))
        )
        .iter()
        .any(|p| p.pid == 4242 && p.session == "one" && p.alive)
    );
    let runtime = crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
    assert_ne!(before_runtime, runtime);
    moved_cache
        .ingest(&format!("{:064x}", 159), &path, "claude")
        .unwrap();
    moved_cache
        .begin_semantic_stage(
            &moved_inputs
                .iter()
                .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            &runtime,
        )
        .unwrap();
    let (facts_generation, facts_rows) = loop {
        let batch = moved_cache.semantic_batch(64).unwrap().unwrap();
        let model = moved_cache
            .prepare_semantic_model_batch(&facts_options, &moved_inputs, &batch, 1790812805000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            assert!(
                serde_json::to_string(&model.prepared().rows)
                    .unwrap()
                    .contains("source-write-after158")
            );
            let ack = moved_cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            moved_cache.finish_semantic_stage(batch.epoch).unwrap();
            break (ack.1.unwrap(), model.prepared().rows.clone());
        }
    };
    assert_eq!(facts_generation.len(), 64);
    // The existing Facts-write scene also exercises the bounded authority
    // under the process-wide SQLite guard, including actual header values.
    let facts_authority = {
        let limits = crate::selected_read::SelectedReadControl::default().with_limits(256, 16 * 1024);
        let _scope = crate::selected_read::Scope::enter(&limits);
        let authority = moved_cache.facts_authority(&facts_path).unwrap();
        assert!(authority.facts_known());
        assert_eq!(authority.hostname().as_deref(), Some("source-write-after158"));
        assert_eq!(authority.codex_current("sessions/unselected.jsonl").unwrap(), None);
        authority.validate().unwrap();
        authority
    };
    assert!(other.facts_authority(&facts_path).is_err(), "namespace cannot borrow another Facts head");
    let binding = crate::storage::NativeReaderBinding::Admitted(config.clone());
    let navigation = crate::storage::navigation_store(binding.clone());
    assert_eq!(navigation.catalog_generation().unwrap(), facts_generation);
    let bookmark = binding.bookmark_identity(&facts_options, namespace.as_str(), &facts_rows[0].key).unwrap().unwrap();
    assert_eq!(bookmark.catalog_key, facts_rows[0].key);
    assert_eq!(bookmark.generation, facts_generation);

    let facts_store =
        crate::storage::configured_query_catalog(&facts_options, config.clone()).unwrap();
    let mut facts_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_configured_machines(vec![(
            namespace.as_str().into(),
            facts_options.clone(),
            config.clone(),
        )])
        .unwrap(),
    ));
    let list = facts_query
        .call_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            1790812805000,
        )
        .unwrap();
    let bound_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812805000,
            facts_store.as_ref(),
        )
        .unwrap();
    assert_eq!(list, bound_list);
    assert!(list.to_string().contains("source-write-after158"));
    let facts_host = body_host(&facts_options, &facts_rows);
    for row in &facts_rows {
        for tool in ["read_transcript", "get_session"] {
            let args = serde_json::json!({"id":row.key});
            let expected = facts_query
                .call_bound_selected_at(
                    tool,
                    &args,
                    1790812805000,
                    facts_store.as_ref(),
                    &facts_host,
                )
                .unwrap();
            let actual = facts_query
                .call_selected_at(
                    namespace.as_str(),
                    &row.key,
                    tool,
                    &args,
                    Some(&facts_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    1790812805000,
                )
                .unwrap();
            assert_eq!(
                actual, expected,
                "configured ordinary Facts selected {tool} {}",
                row.key
            );
        }
    }

    assert_eq!(
        fs::read(&path).unwrap(),
        [raw[0].as_slice(), append.as_bytes(), extra.as_bytes()].concat()
    );
    assert!(!options.cache.exists());
    // (a) Ordinary incremental stages grow Current beyond 64 owners. Each
    // stage names only its changed paths; unmentioned owners carry forward.
    let stage_publish = |cache: &mut EventCache, inputs: &[crate::Input], at: i64| {
        let paths = inputs
            .iter()
            .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        cache.begin_semantic_stage(&paths, &runtime).unwrap();
        loop {
            let batch = cache.semantic_batch(64).unwrap().unwrap();
            let model = cache
                .prepare_semantic_model_batch(&facts_options, inputs, &batch, at)
                .unwrap();
            if batch.phase == crate::model::relation_stage::Phase::Publication {
                // Each bounded custody advance commits as a Deferred checkpoint;
                // the qualified head is unchanged until the final CAS publishes.
                let head = cache.session_catalog_generation().unwrap();
                let mut deferred = 0;
                let ack = loop {
                    let ack = cache
                        .publish_semantic_catalog(&batch, model.prepared())
                        .unwrap();
                    if ack.0 != crate::events::Outcome::Deferred {
                        break ack;
                    }
                    assert_eq!(ack.1, None);
                    assert_eq!(cache.session_catalog_generation().unwrap(), head);
                    deferred += 1;
                    assert!(deferred < 64, "custody makes bounded progress");
                };
                assert_eq!(ack.0, crate::events::Outcome::Written);
                cache.finish_semantic_stage(batch.epoch).unwrap();
                break (ack.1.unwrap(), batch, model, deferred);
            }
        }
    };
    let bulk = (0..70)
        .map(|i| {
            let input = crate::Input {
                root: crate::InputRoot::Claude,
                path: format!("projects/project/bulk169-{i:03}.jsonl"),
            };
            let body = serde_json::json!({"type":"user","sessionId":format!("bulk169-{i:03}"),"uuid":format!("bulk-{i}"),"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized bulk input"}});
            fs::write(input.full_path(&facts_options), format!("{body}\n")).unwrap();
            input
        })
        .collect::<Vec<_>>();
    for (at, input) in bulk.iter().enumerate() {
        moved_cache
            .ingest(
                &format!("{:064x}", 1000 + at),
                &input.full_path(&facts_options),
                "claude",
            )
            .unwrap();
    }
    let mut bulk_generation = String::new();
    for chunk in bulk.chunks(30) {
        bulk_generation = stage_publish(&mut moved_cache, chunk, 1790812806000).0;
    }
    let store_at = |generation: &str| super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.to_owned(),
        options: facts_options.clone(),
    };
    let total = list["total"].as_u64().unwrap() + 70;
    assert!(total > 64);
    let bulk_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812806000,
            &store_at(&bulk_generation),
        )
        .unwrap();
    assert_eq!(bulk_list["total"].as_u64(), Some(total));
    assert!(bulk_list.to_string().contains("source-write-after158"));
    let bulk_page = super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &bulk_generation,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                &facts_options,
                "",
                &crate::catalog::Request::parse("limit=20").unwrap(),
                snapshot,
            )
            .map_err(|_| StorageError::ScopeChanged)
        },
    )
    .unwrap();
    assert_eq!(bulk_page.status, 200);
    // (b) A small partial update republishes one owner; every unrelated
    // Current row, Source and proof stays byte-identical, as do Facts bytes.
    let facts_bytes = fs::read(&facts_path).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(bulk[5].full_path(&facts_options)).unwrap(),"{}",serde_json::json!({"type":"user","sessionId":"bulk169-005","uuid":"bulk-5b","parentUuid":"bulk-5","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized bulk continuation"}})).unwrap();
    moved_cache
        .ingest(
            &format!("{:064x}", 1100),
            &bulk[5].full_path(&facts_options),
            "claude",
        )
        .unwrap();
    let (partial_generation, partial_batch, partial_model, _) =
        stage_publish(&mut moved_cache, &bulk[5..6], 1790812807000);
    assert_ne!(partial_generation, bulk_generation);
    // Unrelated means every owner and proof not reading `path`.
    let unrelated = |a: &str, b: &str, path: &str| {
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            let mut differences = Vec::new();
            let owner = "session_key NOT IN (SELECT session_key FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation IN ($2,$3) AND source_path=$4)";
            for (table, columns, owner) in [
                ("catalog_rows", "session_key,metadata", owner),
                (
                    "catalog_sources",
                    "session_key,source_path,metadata,native_digest",
                    owner,
                ),
                (
                    "catalog_proofs",
                    "source_path,page_generation,usage_generation,usage_ready",
                    "source_path<>$4",
                ),
            ] {
                let count: i64 = tx.driver.query_one(&format!("SELECT count(*) FROM (((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {owner}) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND {owner})) UNION ALL ((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND {owner}) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {owner}))) d"), &[&ns, &a.as_bytes(), &b.as_bytes(), &path.as_bytes()]).map_err(|_| StorageError::Unavailable)?.get(0);
                differences.push(count);
            }
            Ok(differences)
        })
        .unwrap()
    };
    let changed = bulk[5].full_path(&facts_options);
    assert_eq!(
        unrelated(
            &bulk_generation,
            &partial_generation,
            changed.to_str().unwrap()
        ),
        vec![0, 0, 0]
    );
    assert_eq!(fs::read(&facts_path).unwrap(), facts_bytes);
    let partial_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812807000,
            &store_at(&partial_generation),
        )
        .unwrap();
    assert_eq!(partial_list["total"].as_u64(), Some(total));
    assert!(partial_list.to_string().contains("source-write-after158"));
    // (c) An isolated Source withdraws by bounded pages. A mixed or connected
    // owner's Source is withdrawn last, below, through the semantic catch-up.
    let (mut store, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let agent = claude_agent.to_str().unwrap().to_owned();
    let agent_removal = vec![(agent.clone(), store.ledger(&agent).unwrap())];
    let gone = bulk[1].full_path(&facts_options);
    let gone_path = gone.to_str().unwrap().to_owned();
    let gone_removal = vec![(gone_path.clone(), store.ledger(&gone_path).unwrap())];
    fs::rename(&gone, oracle.root.join("retained-bulk169.jsonl")).unwrap();
    assert_eq!(
        store.remove_files(&gone_removal).unwrap(),
        crate::events::Outcome::Written
    );
    let withdrawn = store.session_catalog_generation().unwrap().unwrap();
    assert_ne!(withdrawn, partial_generation);
    assert_eq!(
        unrelated(&partial_generation, &withdrawn, &gone_path),
        vec![0, 0, 0],
        "withdrawal of an isolated owner keeps unrelated Current"
    );
    // A successor publication, then exact old requests replay their own
    // recorded generations without republishing or moving the head.
    writeln!(fs::OpenOptions::new().append(true).open(bulk[2].full_path(&facts_options)).unwrap(),"{}",serde_json::json!({"type":"user","sessionId":"bulk169-002","uuid":"bulk-2b","parentUuid":"bulk-2","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized successor"}})).unwrap();
    moved_cache
        .ingest(
            &format!("{:064x}", 1101),
            &bulk[2].full_path(&facts_options),
            "claude",
        )
        .unwrap();
    let successor = stage_publish(&mut moved_cache, &bulk[2..3], 1790812808000).0;
    assert_ne!(successor, withdrawn);
    assert_eq!(
        moved_cache
            .publish_semantic_catalog(&partial_batch, partial_model.prepared())
            .unwrap()
            .1
            .as_deref(),
        Some(partial_generation.as_str())
    );
    assert_eq!(
        store.remove_files(&gone_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(successor.as_str())
    );
    let successor_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812808000,
            &store_at(&successor),
        )
        .unwrap();
    assert_eq!(successor_list["total"].as_u64(), Some(total - 1));
    // (d) Restart: reopened stores read the newest Current and the old
    // selected bodies exactly as before.
    drop(store);
    drop(moved_cache);
    let reopened =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        reopened.session_catalog_generation().as_deref(),
        Some(successor.as_str())
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old selected bodies after incremental publication and restart"
    );
    // Fresh Pool: new connections run the cold binding audit, then read the
    // committed head, every older committed generation's cohort (retained
    // history merged as in SQLite) and the old selected bodies.
    drop(reopened);
    let started = std::time::Instant::now();
    let cold = crate::storage::postgres::PgPool::new(
        factory.clone(),
        crate::storage::postgres::tests::limits(),
    )
    .unwrap();
    let cold_cache =
        EventCache::open_pg_native(cold.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        cold_cache.session_catalog_generation().as_deref(),
        Some(successor.as_str())
    );
    let cold_store =
        |generation: &str, options: &Options| super::super::snapshot::query::QueryCatalog {
            pool: cold.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.to_owned(),
            options: options.clone(),
        };
    let first = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812809000,
            &cold_store(&successor, &facts_options),
        )
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "cold first page"
    );
    assert_eq!(first["total"].as_u64(), Some(total - 1));
    for (at, expected) in [
        (&bulk_generation, total),
        (&partial_generation, total),
        (&withdrawn, total - 1),
    ] {
        let listed = facts_query
            .call_bound_catalog_at(
                "list_sessions",
                &serde_json::json!({}),
                1790812809000,
                &cold_store(at, &facts_options),
            )
            .unwrap();
        assert_eq!(listed["total"].as_u64(), Some(expected), "older cohort");
    }
    assert_eq!(
        read_bodies(
            &mut query,
            &cold_store(&generation, &options),
            &retained_host
        ),
        expected_bodies,
        "old selected bodies through a fresh Pool"
    );
    // Codex active/archive membership: a Facts write selecting the archived
    // rollout completes, replays its lost ACK and keeps the exact Facts bytes.
    let mut archived_facts = recorded.clone();
    archived_facts.codex_rollouts = Some(std::collections::BTreeSet::from([format!(
        "archived_sessions/rollout-{codex_id}.jsonl"
    )]));
    let membership = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &archived_facts,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    let membership_request = membership.intent();
    crate::save_json_file(&facts_path, &archived_facts).unwrap();
    membership.complete().unwrap();
    crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &archived_facts,
        true,
        raw_store(),
        Some(membership_request),
    )
    .unwrap()
    .complete()
    .unwrap();
    assert_eq!(crate::read_facts(&facts_path).unwrap(), archived_facts);
    assert!(facts_authority.validate().is_err(), "exact canonical replacement invalidates the old authority");
    moved_cache.prepare_facts_authority(&facts_options).unwrap();
    let selected = moved_cache.facts_authority(&facts_path).unwrap();
    assert_eq!(selected.codex_current(&format!("archived_sessions/rollout-{codex_id}.jsonl")).unwrap(), Some(true));
    assert_eq!(selected.codex_current("sessions/unselected.jsonl").unwrap(), Some(false));

    // A global Facts change invalidates every Current Source (> 64 here) by
    // bounded pages instead of refusing, and replays its lost ACK.
    let mut renamed = archived_facts.clone();
    renamed.hostname = "source-write-after174".into();
    let global = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &renamed,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    let global_request = global.intent();
    crate::save_json_file(&facts_path, &renamed).unwrap();
    global.complete().unwrap();
    crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &renamed,
        true,
        raw_store(),
        Some(global_request),
    )
    .unwrap()
    .complete()
    .unwrap();
    assert_eq!(crate::read_facts(&facts_path).unwrap(), renamed);
    // (e) A connected Source withdrawal commits only its Index intent and the
    // `removed` invalidation. The qualified Current generation stays until the
    // ordinary semantic catch-up publishes; an exact lost-ACK replay is applied.
    let (mut store, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let qualified = store.session_catalog_generation().unwrap();
    fs::rename(&claude_agent, oracle.root.join("retained-agent180.jsonl")).unwrap();
    assert_eq!(
        store.remove_files(&agent_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    let reason: Vec<u8> = pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let row = tx.driver.query_one("SELECT reason FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(), &agent.as_bytes()]).map_err(sqlstate::error)?;
        Ok(row.get::<_, String>(0).into_bytes())
    }).unwrap();
    assert_eq!(reason, b"removed");
    assert_eq!(
        store.remove_files(&agent_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    // The second connected subagent is withdrawn the same way; both removed
    // sessions need new retained custody in one catch-up publication.
    let agent191 = claude_agent191.to_str().unwrap().to_owned();
    let agent191_removal = vec![(agent191.clone(), store.ledger(&agent191).unwrap())];
    fs::rename(
        &claude_agent191,
        oracle.root.join("retained-agent191.jsonl"),
    )
    .unwrap();
    assert_eq!(
        store.remove_files(&agent191_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    // The ordinary semantic catch-up claims the `removed` invalidations and
    // republishes the connected parent without the withdrawn Sources.
    let mut catchup =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let connected = [
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/one.jsonl".into(),
        },
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/one/subagents/agent-related154.jsonl".into(),
        },
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/one/subagents/agent-related191.jsonl".into(),
        },
    ];
    let paths = connected
        .iter()
        .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    catchup.begin_semantic_stage(&paths, &runtime).unwrap();
    let (batch, model) = loop {
        let batch = catchup.semantic_batch(64).unwrap().unwrap();
        let model = catchup
            .prepare_semantic_model_batch(&facts_options, &connected, &batch, 1790812809000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            break (batch, model);
        }
    };
    // Two new custodies need more than one bounded advance. The first
    // checkpoint's ACK is lost with the process; a restarted producer resumes
    // the same operation from the durable checkpoint, not from scratch.
    assert_eq!(
        catchup
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap(),
        (crate::events::Outcome::Deferred, None)
    );
    assert_eq!(
        catchup.session_catalog_generation().unwrap(),
        qualified.as_deref().unwrap()
    );
    drop(catchup);
    let mut catchup =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let mut deferred = 1;
    let ack = loop {
        let ack = catchup
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        if ack.0 != crate::events::Outcome::Deferred {
            break ack;
        }
        assert_eq!(ack.1, None);
        assert_eq!(
            catchup.session_catalog_generation().unwrap(),
            qualified.as_deref().unwrap()
        );
        deferred += 1;
        assert!(deferred < 64, "custody makes bounded progress");
    };
    assert_eq!(ack.0, crate::events::Outcome::Written);
    // The long withdrawn recipe (>150 slots) cannot fit one 64-slot page.
    assert!(deferred >= 2, "long recipe resumed by pages: {deferred}");
    // A lost Written ACK replays the exact operation and its generation.
    assert_eq!(
        catchup
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap(),
        ack
    );
    catchup.finish_semantic_stage(batch.epoch).unwrap();
    let caught = ack.1.unwrap();
    assert_eq!(catchup.session_catalog_generation().unwrap(), caught);
    assert_ne!(Some(caught.clone()), qualified);
    let parent = claude_parent.to_str().unwrap().to_owned();
    let counts: Vec<i64> = pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        let mut counts = Vec::new();
        for (generation, path) in [(caught.as_str(), &agent), (caught.as_str(), &parent), (qualified.as_deref().unwrap(), &agent)] {
            counts.push(tx.driver.query_one("SELECT count(*) FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND source_path=$3", &[&ns, &generation.as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?.get::<_, i64>(0));
        }
        Ok(counts)
    }).unwrap();
    assert_eq!(counts[0], 0, "withdrawn Source left the successor Current");
    assert!(
        counts[1] > 0 && counts[2] > 0,
        "parent republished; qualified generation retained"
    );
    // P3: the successor's retained history is its Current rows plus every
    // session the withdrawal removed, served from the owner headers as
    // Retained. Whole scopes follow the viewer keyset cursor (bulk169 made
    // the generation larger than one page).
    let scoped_items = |generation: &str, scope: &str| {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let query = match &cursor {
                Some(cursor) => format!(
                    "{scope}limit=50&cursor={}",
                    crate::viewer::percent_encode(cursor)
                ),
                None => format!("{scope}limit=50"),
            };
            let reply = super::super::snapshot::with_admitted_generation(
                &pool,
                &namespace,
                &control,
                generation,
                |snapshot| {
                    crate::catalog::read_page_snapshot(
                        &options,
                        recent_machine,
                        &crate::catalog::Request::parse(&query).unwrap(),
                        snapshot,
                    )
                    .map_err(|_| StorageError::ScopeChanged)
                },
            )
            .unwrap();
            assert_eq!(reply.status, 200);
            let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            items.extend(page["items"].as_array().unwrap().iter().cloned());
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break items,
            }
        }
    };
    let scoped = |generation: &str, scope: &str| {
        let mut items = Vec::new();
        for item in scoped_items(generation, scope) {
            items.push((
                item["key"].as_str().unwrap().to_owned(),
                item["lifecycle"] == "retained",
            ));
        }
        items
    };
    let before = scoped(qualified.as_deref().unwrap(), "");
    let before_history = scoped(qualified.as_deref().unwrap(), "scope=retained_history&");
    let current = scoped(&caught, "");
    let mut retained = scoped(&caught, "scope=retained_history&");
    let current_keys = current
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = before
        .iter()
        .filter(|(key, _)| !current_keys.contains(key))
        .map(|(key, _)| key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        !removed.is_empty(),
        "connected withdrawal removed no Current session"
    );
    assert!(current.iter().chain(&before).all(|(_, retained)| !retained));
    // Membership and lifecycle are exact; removed rows come from custody.
    let mut expected_retained = before_history
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    for (key, _) in &current {
        expected_retained.entry(key.clone()).or_insert(false);
    }
    for key in &removed {
        expected_retained.insert(key.clone(), true);
    }
    // The parent keeps Current membership while its composite history wins in
    // retained scope. Check the exact SQLite projection, including that case.
    for row in &oracle_history_projection {
        if row["harness"] == "claude" {
            expected_retained.insert(
                row["key"].as_str().unwrap().to_owned(),
                row["lifecycle"] == "retained",
            );
        }
    }
    let mut expected_retained = expected_retained.into_iter().collect::<Vec<_>>();
    expected_retained.sort();
    retained.sort();
    assert_eq!(retained, expected_retained);
    // The same metadata-only resolver serves retained native aliases after
    // their Current row disappears. Expected generation fencing stays active.
    super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &caught,
        |snapshot| {
            use crate::storage::navigation::NativeNavigationStore;
            assert_eq!(snapshot.catalog_generation()?, caught);
            for row in prepared
                .rows
                .iter()
                .filter(|row| removed.contains(&row.key))
            {
                for native in &row.native_ids {
                    let current = snapshot.native_resolution(
                        &row.harness,
                        native,
                        crate::CatalogReadScope::Current,
                        Some(&caught),
                    )?;
                    assert!(current.keys.is_empty());
                    let retained = snapshot.native_resolution(
                        &row.harness,
                        native,
                        crate::CatalogReadScope::RetainedHistory,
                        Some(&caught),
                    )?;
                    assert_eq!(retained.keys, vec![row.key.clone()]);
                    let resolved: CatalogRow =
                        serde_json::from_str(retained.metadata.as_deref().unwrap()).unwrap();
                    assert_eq!(resolved.key, row.key);
                }
            }
            assert!(
                snapshot
                    .native_resolution(
                        "claude",
                        "one",
                        crate::CatalogReadScope::Current,
                        Some(&generation)
                    )
                    .is_err()
            );
            Ok(())
        },
    )
    .unwrap();
    // Owner-side filters: the withdrawn Claude sessions stay Retained under
    // the harness filter and are absent under another harness.
    let claude = scoped(&caught, "scope=retained_history&harness=claude&");
    let codex = scoped(&caught, "scope=retained_history&harness=codex&");
    for key in &removed {
        assert!(
            claude.contains(&(key.clone(), true)),
            "{key} under harness=claude"
        );
        assert!(codex.iter().all(|(other, _)| other != key));
    }
    assert!(claude.len() + codex.len() <= retained.len());
    // The SQLite pre-Guard replay of the same withdrawal: retained-only rows
    // in keyset order and under the harness filter are exactly the oracle's.
    let withdrawn_rows = |generation: &str| {
        scoped_items(generation, "scope=retained_history&")
            .iter()
            .filter(|item| removed.contains(item["key"].as_str().unwrap()))
            .map(stable)
            .collect::<Vec<_>>()
    };
    assert_eq!(withdrawn_rows(&caught), oracle_withdrawn);
    assert_eq!(
        claude
            .iter()
            .filter(|(key, _)| removed.contains(key))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
        oracle_withdrawn_claude
    );
    assert_eq!(
        scoped_items(&caught, "scope=retained_history&harness=claude&")
            .iter()
            .filter(|item| sqlite_before_current.contains(item["key"].as_str().unwrap()))
            .map(stable)
            .collect::<Vec<_>>(),
        oracle_history_projection
            .iter()
            .filter(|item| item["harness"] == "claude")
            .cloned()
            .collect::<Vec<_>>()
    );
    // P3 search: metadata search over the published PG projection. A key
    // query finds its Current session; a removed key is absent from the
    // successor's Current search but found in the qualified generation.
    let search = |generation: &str, scope: &str, key: &str| {
        let query = format!(
            "{scope}limit=50&q={}",
            crate::viewer::percent_encode(&key.to_lowercase())
        );
        let reply = super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            generation,
            |snapshot| {
                crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &crate::catalog::Request::parse(&query).unwrap(),
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)
            },
        )
        .unwrap();
        assert_eq!(reply.status, 200, "{query}");
        let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let kept = &current[0].0;
    assert!(
        search(&caught, "", kept).contains(kept),
        "Current search {kept}"
    );
    let gone = removed.iter().next().unwrap();
    assert!(
        !search(&caught, "", gone).contains(gone),
        "{gone} withdrawn"
    );
    let old = qualified.as_deref().unwrap();
    assert!(
        search(old, "", gone).contains(gone),
        "{gone} before withdrawal"
    );
    // Retained scope finds the withdrawn key through its carried document.
    assert!(
        search(&caught, "scope=retained_history&", gone).contains(gone),
        "{gone} retained search"
    );
    assert!(search(&caught, "scope=retained_history&", kept).contains(kept));
    for key in &removed {
        assert_eq!(
            search(&caught, "scope=retained_history&", key),
            oracle_retained_search[key]
        );
    }
    // Retained-only selected body: on the successor a withdrawn key has no
    // Current row, so its body is read through the proof-owned retained
    // recipe from Raw bytes and equals the SQLite ordinary body exactly.
    let caught_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: caught.clone(),
        options: options.clone(),
    };
    for key in &removed {
        let body = query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":key,"limit":100}),
                1790812802000,
                &caught_store,
                &retained_host,
            )
            .unwrap();
        assert_eq!(body, expected_bodies[key]["body"], "retained body {key}");
        assert_eq!(
            body, oracle_retained_reads[key]["body"],
            "SQLite withdrawal body parity {key}"
        );
        assert!(body.to_string().contains("sanitized agent answer"));
        let fields = query
            .call_bound_selected_at(
                "get_session",
                &serde_json::json!({"id":key}),
                1790812802000,
                &caught_store,
                &retained_host,
            )
            .unwrap();
        assert_eq!(
            fields, oracle_retained_reads[key]["fields"],
            "SQLite withdrawal fields parity {key}"
        );
    }
    // Exact old OpID after its successor replays without moving the head.
    assert_eq!(
        store.remove_files(&agent_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(caught.as_str())
    );
    // Restart continues from the successor; the readers' retained proofs
    // still serve the old selected bodies exactly.
    drop(catchup);
    let mut reopened =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        reopened.session_catalog_generation().as_deref(),
        Some(caught.as_str())
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old selected bodies after a connected withdrawal and restart"
    );
    assert_eq!(
        withdrawn_rows(&caught),
        oracle_withdrawn,
        "retained-only rows after restart"
    );
    // Zero-row publication: Facts no longer list the archived rollout, so
    // its catch-up batch prepares no row. The recorded batch runtime is the
    // lifecycle authority that retires the owner into retained history.
    let mut emptied = renamed.clone();
    emptied.codex_rollouts = Some(std::collections::BTreeSet::new());
    let emptied_write = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &emptied,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    crate::save_json_file(&facts_path, &emptied).unwrap();
    emptied_write.complete().unwrap();
    let rollout = [crate::Input {
        root: crate::InputRoot::Codex,
        path: format!("archived_sessions/rollout-{codex_id}.jsonl"),
    }];
    let (retired, retired_batch, retired_model, _) =
        stage_publish(&mut reopened, &rollout, 1790812810000);
    assert!(retired_model.prepared().rows.is_empty(), "zero-row batch");
    assert_eq!(retired_batch.runtime, runtime);
    assert_ne!(retired, caught);
    let codex_keys = prepared
        .rows
        .iter()
        .filter(|row| row.harness == "codex")
        .map(|row| row.key.clone())
        .collect::<Vec<_>>();
    assert!(!codex_keys.is_empty());
    let after = scoped(&retired, "");
    let kept_after = scoped(&retired, "scope=retained_history&");
    for key in &codex_keys {
        assert!(after.iter().all(|(other, _)| other != key), "{key} retired");
        assert!(
            kept_after.contains(&(key.clone(), true)),
            "{key} retained after zero-row publication"
        );
    }
    // Every other Current session is unchanged by the zero-row publication.
    assert_eq!(
        after.len() + codex_keys.len(),
        current.len(),
        "zero-row publication retires only the rollout owner"
    );
    // The withdrawn subagents stay Retained, unchanged, in the new generation.
    let still = withdrawn_rows(&retired);
    assert!(
        oracle_withdrawn.iter().all(|row| still.contains(row)),
        "withdrawn rows after zero-row publication"
    );
}
