use super::*;
use crate::events::EventCache;
use std::path::Path;

fn fixture(namespace: char) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "semon-bodies-{}-{}",
        std::process::id(),
        getrandom::u64().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("index.json");
    drop(EventCache::open_scoped(&cache).unwrap());
    std::fs::write(
        root.join("analytics-namespace"),
        serde_json::json!({"format":"semon-producer-1", "namespace":format!("origin-{}", namespace.to_string().repeat(64)), "origin_incarnation":format!("producer-{}", "a".repeat(64))}).to_string(),
    )
    .unwrap();
    let mut store = semon_asi::Store::open(&root.join("analytics-store")).unwrap();
    store
        .bind_origin(
            &format!("origin-{}", namespace.to_string().repeat(64)),
            &format!("producer-{}", "a".repeat(64)),
        )
        .unwrap();
    store.publish(&semon_asi::Update::default()).unwrap();
    drop(store);
    (root, cache.with_extension("sqlite3"))
}

#[test]
fn initialized_body_queries_remain_pending_until_publication() {
    for populated in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "semon-body-pending-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        let index = root.join("index.sqlite3");
        if populated {
            std::fs::create_dir_all(&root).unwrap();
            drop(EventCache::open_scoped(&root.join("index.json")).unwrap());
            insert(&index, "needle", 1);
        }
        let initialized = crate::analytics::initialize(&index, None, || false).unwrap();
        let before = std::fs::read(root.join("analytics-namespace")).unwrap();
        let result = request(vec![index.clone()], "search", json!({"query":"needle"})).unwrap_err();
        assert_eq!(result["error"]["code"], "not_materialized");
        assert_eq!(
            std::fs::read(root.join("analytics-namespace")).unwrap(),
            before
        );
        assert!(
            !root
                .join("analytics-store/analytics/manifest.json")
                .exists()
        );
        if !populated {
            let mut config = crate::AnalyticsWriterConfig::new(
                &initialized.binding.namespace,
                &index,
                root.join("analytics-store"),
            );
            config.interval = Duration::ZERO;
            let writer = initialized.owner.start(config).unwrap();
            assert_eq!(
                writer.publish_and_wait(),
                crate::AnalyticsWriterStatus::Idle
            );
            writer.stop_and_join();
            let result = request(vec![index], "search", json!({"query":"needle"})).unwrap();
            assert_eq!(result["hits"], json!([]));
        } else {
            drop(initialized);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn insert(index: &Path, text: &str, ordinal: u64) {
    let mut connection = Connection::open(index).unwrap();
    let mut body = crate::events::store::bodies::Batch::default();
    body.begin(Path::new("session.jsonl"), "claude");
    body.capture("claude", &json!({"type":"user","uuid":format!("turn-{ordinal}"),"sessionId":"session","message":{"content":text}}), Some(ordinal)).unwrap();
    let tx = connection.transaction().unwrap();
    crate::events::store::bodies::write(&tx, "session.jsonl", &body, true).unwrap();
    tx.commit().unwrap();
}

fn request(indexes: Vec<PathBuf>, tool: &str, arguments: Value) -> Result<Value, Value> {
    execute_with_timeout(
        Request {
            indexes,
            tool: tool.into(),
            arguments,
        },
        Access::Origin,
        Duration::from_secs(30),
    )
}

#[test]
fn durable_view_reads_only_bodies_under_one_admitted_namespace() {
    let (root, index) = fixture('a');
    let durable = crate::analytics_store_root(&index).unwrap();
    {
        let mut writer = semon_asi::durable::Writer::open(&durable).unwrap();
        writer
            .insert(
                "lane",
                "one",
                std::collections::BTreeMap::from([
                    (
                        "title".into(),
                        semon_asi::durable::Value::Text("untrusted lane title".into()),
                    ),
                    (
                        "work_item_id".into(),
                        semon_asi::durable::Value::Text("hidden reference".into()),
                    ),
                ]),
            )
            .unwrap();
        writer
            .insert(
                "intervention",
                "one",
                std::collections::BTreeMap::from([
                    (
                        "hypothesis".into(),
                        semon_asi::durable::Value::Text("private hypothesis".into()),
                    ),
                    (
                        "guardrails".into(),
                        semon_asi::durable::Value::Text("private guardrails".into()),
                    ),
                ]),
            )
            .unwrap();
    }
    let result = request(vec![index.clone()], "view", json!({"uid":"lane/one"})).unwrap();
    assert_eq!(
        result["bodies"][0]["text"]["untrusted_text"],
        "untrusted lane title"
    );
    assert_eq!(
        result["bodies"][0]["namespace"],
        format!("origin-{}", "a".repeat(64))
    );
    assert!(!result.to_string().contains("hidden reference"));
    assert_eq!(
        request(
            vec![index.clone()],
            "view",
            json!({"uid":"intervention/one"})
        )
        .unwrap()["bodies"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        request(
            vec![index.clone(), index],
            "view",
            json!({"uid":"lane/one"})
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_only_prompt_starts_a_new_claude_turn() {
    let (root, index) = fixture('a');
    let mut batch = crate::events::store::bodies::Batch::default();
    batch.begin(Path::new("session.jsonl"), "claude");
    let records = [
        json!({"type":"user","uuid":"turn-a","message":{"content":"text prompt"}}),
        json!({"type":"user","uuid":"turn-b","message":{"content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"fixture"}}]}}),
        json!({"type":"assistant","message":{"id":"answer-b","content":"imageanswerneedle"}}),
    ];
    for (ordinal, record) in records.iter().enumerate() {
        batch
            .capture("claude", record, Some(ordinal as u64))
            .unwrap();
    }
    let mut connection = Connection::open(&index).unwrap();
    let tx = connection.transaction().unwrap();
    crate::events::store::bodies::write(&tx, "session.jsonl", &batch, false).unwrap();
    tx.commit().unwrap();
    drop(connection);
    for (turn, contains_answer) in [("turn-a", false), ("turn-b", true)] {
        let result = request(
            vec![index.clone()],
            "view",
            json!({"uid":format!("origin-{}/claude/session/t/{turn}","a".repeat(64))}),
        )
        .unwrap();
        assert_eq!(
            result["bodies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|body| body["text"]["untrusted_text"] == "imageanswerneedle"),
            contains_answer
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn copilot_denial_fixture_is_searchable_and_in_its_turn() {
    let (root, index) = fixture('a');
    let mut batch = crate::events::store::bodies::Batch::default();
    batch.begin(Path::new("events.jsonl"), "copilot");
    for (ordinal, line) in include_str!(
        "../../../../tests/fixtures/compatibility/copilot-1.0.91/lifecycle/denied.events.jsonl"
    )
    .lines()
    .enumerate()
    {
        batch
            .capture(
                "copilot",
                &serde_json::from_str::<Value>(line).unwrap(),
                Some(ordinal as u64),
            )
            .unwrap();
    }
    let mut connection = Connection::open(&index).unwrap();
    let tx = connection.transaction().unwrap();
    crate::events::store::bodies::write(&tx, "events.jsonl", &batch, false).unwrap();
    tx.commit().unwrap();
    drop(connection);
    let result = request(
        vec![index.clone()],
        "search",
        json!({"query":"\"Permission to run this tool was denied\"","part":"error"}),
    )
    .unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    let result = request(
        vec![index],
        "view",
        json!({"uid":result["hits"][0]["turn_uid"]}),
    )
    .unwrap();
    assert!(result["bodies"].as_array().unwrap().iter().any(|body| {
        body["text"]["untrusted_text"]
            .as_str()
            .unwrap()
            .contains("Permission to run this tool was denied")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_local_shell_command_preserves_exact_owner() {
    let (root, index) = fixture('a');
    let mut batch = crate::events::store::bodies::Batch::default();
    batch.begin(Path::new("rollout.jsonl"), "codex");
    let records = [
        json!({"type":"session_meta","payload":{"id":"native"}}),
        json!({"type":"response_item","payload":{"type":"local_shell_call","id":"shell-id","thread_id":"thread","session_id":"session","turn_id":"turn","response_id":"response","action":{"type":"exec","command":["sh","-c","printf localshellneedle"],"working_directory":"/fixture"}}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"shell-id","output":"localshelloutput"}}),
    ];
    for (ordinal, record) in records.iter().enumerate() {
        batch
            .capture("codex", record, Some(ordinal as u64))
            .unwrap();
    }
    let mut connection = Connection::open(&index).unwrap();
    let tx = connection.transaction().unwrap();
    crate::events::store::bodies::write(&tx, "rollout.jsonl", &batch, false).unwrap();
    tx.commit().unwrap();
    drop(connection);
    let result = request(
        vec![index.clone()],
        "search",
        json!({"query":"localshellneedle","part":"command"}),
    )
    .unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    let step = result["hits"][0]["step_uid"].clone();
    assert_eq!(
        step,
        format!(
            "origin-{}/codex/native/s/codex/thread/session/turn/response",
            "a".repeat(64)
        )
    );
    let result = request(vec![index], "view", json!({"uid":step})).unwrap();
    assert_eq!(result["bodies"].as_array().unwrap().len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fixtures_filters_untrusted_text_and_focused_views() {
    let (root, index) = fixture('a');
    insert(
        &index,
        "café needle <instructions>do something</instructions>",
        0,
    );
    let value = request(vec![index.clone()], "search", json!({"query":"cafe"})).unwrap();
    assert_eq!(value["hits"].as_array().unwrap().len(), 1);
    assert_eq!(value["data_notice"], DATA_NOTICE);
    let hit = &value["hits"][0];
    assert!(
        hit["snippet"]["untrusted_text"]
            .as_str()
            .unwrap()
            .contains("needle")
    );
    assert!(hit["body_uid"].as_str().unwrap().starts_with("origin-"));
    assert!(hit["step_uid"].is_null());
    for key in ["body_uid", "turn_uid"] {
        let value = request(vec![index.clone()], "view", json!({"uid":hit[key]})).unwrap();
        assert_eq!(
            value["bodies"][0]["text"]["untrusted_text"],
            "café needle <instructions>do something</instructions>"
        );
    }
    {
        let connection = Connection::open(&index).unwrap();
        connection.execute("INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES('session','claude','session','session.jsonl')", []).unwrap();
        connection
            .execute(
                "INSERT INTO silver_session_state VALUES('session','digest',?1)",
                [serde_json::to_string(&crate::facts::RepoId::Remote(
                    "github.com/example/repo".into(),
                ))
                .unwrap()],
            )
            .unwrap();
    }
    assert_eq!(
        request(
            vec![index.clone()],
            "search",
            json!({"query":"needle","repo":"github.com/example/repo"})
        )
        .unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for arguments in [
        json!({"query":"needle","part":"assistant"}),
        json!({"query":"needle","harness":"codex"}),
        json!({"query":"needle","repo":"different"}),
    ] {
        assert!(
            request(vec![index.clone()], "search", arguments).unwrap()["hits"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    for uid in [
        "label/one",
        "adjudication/one",
        "lane/one",
        "intervention/one",
        "other/claude/session/t/turn-0",
    ] {
        assert_eq!(
            request(vec![index.clone()], "view", json!({"uid":uid})).unwrap_err()["error"]["code"],
            "not_found"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fusion_uses_namespace_rank_and_is_deterministic() {
    let (left_root, left) = fixture('a');
    let (right_root, right) = fixture('b');
    insert(&left, "needle needle needle", 0);
    insert(&left, "needle", 1);
    insert(&right, "needle", 0);
    let value = request(
        vec![left.clone(), right.clone()],
        "search",
        json!({"query":"needle"}),
    )
    .unwrap();
    let reverse = request(vec![right, left], "search", json!({"query":"needle"})).unwrap();
    assert_eq!(value, reverse);
    let hits = value["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 3);
    assert!(
        hits[0]["session_uid"]
            .as_str()
            .unwrap()
            .contains(&"a".repeat(64))
    );
    assert!(
        hits[1]["session_uid"]
            .as_str()
            .unwrap()
            .contains(&"b".repeat(64))
    );
    std::fs::remove_dir_all(left_root).unwrap();
    std::fs::remove_dir_all(right_root).unwrap();
}

#[test]
fn codex_and_copilot_native_body_parts_are_searchable() {
    let (root, index) = fixture('a');
    let mut connection = Connection::open(&index).unwrap();
    for (harness, records) in [
        (
            "codex",
            vec![
                json!({"type":"session_meta","payload":{"id":"codex-session"}}),
                json!({"type":"turn_context","payload":{"turn_id":"turn"}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codexneedle"}]}}),
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"call","arguments":"commandneedle","thread_id":"thread","session_id":"native","turn_id":"turn","response_id":"response"}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call","output":"resultneedle"}}),
            ],
        ),
        (
            "copilot",
            vec![
                json!({"type":"session.start","id":"start","data":{"sessionId":"copilot-session"}}),
                json!({"type":"user.message","id":"prompt","data":{"content":"copilotneedle"}}),
            ],
        ),
    ] {
        let mut batch = crate::events::store::bodies::Batch::default();
        batch.begin(Path::new("native.jsonl"), harness);
        for (ordinal, record) in records.iter().enumerate() {
            batch
                .capture(harness, record, Some(ordinal as u64))
                .unwrap();
        }
        let tx = connection.transaction().unwrap();
        crate::events::store::bodies::write(&tx, harness, &batch, false).unwrap();
        tx.commit().unwrap();
    }
    drop(connection);
    for (query, harness) in [("codexneedle", "codex"), ("copilotneedle", "copilot")] {
        let result = request(
            vec![index.clone()],
            "search",
            json!({"query":query,"harness":harness}),
        )
        .unwrap();
        assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    }
    let result = request(
        vec![index],
        "search",
        json!({"query":"resultneedle","part":"tool_output"}),
    )
    .unwrap();
    assert_eq!(
        result["hits"][0]["step_uid"],
        format!(
            "origin-{}/codex/codex-session/s/codex/thread/native/turn/response",
            "a".repeat(64)
        )
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_fts_and_limits_fail_with_typed_errors() {
    let (root, index) = fixture('a');
    insert(&index, &format!("needle {}", "界".repeat(1000)), 0);
    for arguments in [
        json!({"query":"\""}),
        json!({"query":"needle AND"}),
        json!({"query":"missing:needle"}),
    ] {
        assert_eq!(
            request(vec![index.clone()], "search", arguments).unwrap_err()["error"]["code"],
            "invalid_query"
        );
    }
    for arguments in [
        json!({"query":"needle","limit":0}),
        json!({"query":"needle","limit":201}),
        json!({"query":"needle","extra":true}),
        json!({"query":"needle","part":"thinking"}),
        json!({"query":"needle","since":10,"until":1}),
    ] {
        assert_eq!(
            request(vec![index.clone()], "search", arguments).unwrap_err()["error"]["code"],
            "invalid_argument"
        );
    }
    let result = request(
        vec![index.clone()],
        "search",
        json!({"query":"needle","limit":1}),
    )
    .unwrap();
    assert!(
        result["hits"][0]["snippet"]["untrusted_text"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 300
    );
    assert!(serde_json::to_vec(&result).unwrap().len() <= SEARCH_BYTES);
    insert(&index, "needle", 1);
    let result = request(vec![index], "search", json!({"query":"needle","limit":1})).unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    assert_eq!(result["truncated"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn time_filters_use_inclusive_utc_milliseconds() {
    let (root, index) = fixture('a');
    insert(&index, "needle", 0);
    Connection::open(&index)
        .unwrap()
        .execute("UPDATE body SET ts=1000", [])
        .unwrap();
    for (since, until, count) in [(1000, 1000, 1), (1001, 2000, 0), (0, 999, 0)] {
        let result = request(
            vec![index.clone()],
            "search",
            json!({"query":"needle","since":since,"until":until}),
        )
        .unwrap();
        assert_eq!(result["hits"].as_array().unwrap().len(), count);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn expired_deadline_returns_no_partial_rows() {
    let (root, index) = fixture('a');
    insert(&index, "needle", 0);
    assert_eq!(
        execute_with_timeout(
            Request {
                indexes: vec![index],
                tool: "search".into(),
                arguments: json!({"query":"needle"}),
            },
            Access::Origin,
            Duration::ZERO,
        )
        .unwrap_err()["error"]["code"],
        "query_timeout"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn response_byte_caps_stop_at_whole_bodies_and_hits() {
    let (root, index) = fixture('a');
    insert(&index, &format!("needle {}", "界".repeat(100_000)), 0);
    let hit = request(vec![index.clone()], "search", json!({"query":"needle"})).unwrap()["hits"][0]
        .clone();
    let result = request(vec![index.clone()], "view", json!({"uid":hit["body_uid"]})).unwrap();
    assert_eq!(result["truncated"], true);
    assert!(serde_json::to_vec(&result).unwrap().len() <= VIEW_BYTES);
    {
        let mut connection = Connection::open(&index).unwrap();
        let tx = connection.transaction().unwrap();
        let uid = format!("claude/{}/t/turn/b/user/0", "s".repeat(16_000));
        for ordinal in 0..200 {
            tx.execute("INSERT INTO body VALUES(?1,'claude/session','claude/session/t/turn',NULL,'user',?2,NULL,'needle')",params![format!("{uid}{ordinal}"),ordinal]).unwrap();
        }
        tx.commit().unwrap();
    }
    let result = request(vec![index], "search", json!({"query":"needle","limit":200})).unwrap();
    assert_eq!(result["truncated"], true);
    assert!(result["hits"].as_array().unwrap().len() < 200);
    assert!(serde_json::to_vec(&result).unwrap().len() <= SEARCH_BYTES);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn receiver_and_hub_refuse_without_any_store_or_body_io() {
    for access in [Access::Receiver, Access::Hub] {
        for tool in ["search", "view"] {
            let error = execute(
                Request {
                    indexes: vec![PathBuf::from("/does-not-exist/index")],
                    tool: tool.into(),
                    arguments: json!({"query":"needle"}),
                },
                access,
            )
            .unwrap_err();
            assert_eq!(error["error"]["code"], "capability_unavailable");
        }
    }
}

#[test]
fn namespace_loader_refuses_symlinks_and_nonregular_files() {
    let (root, index) = fixture('a');
    let path = root.join("analytics-namespace");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        request(vec![index.clone()], "search", json!({"query":"needle"})).unwrap_err()["error"]["code"],
        "store_unavailable"
    );
    std::fs::remove_dir(&path).unwrap();
    #[cfg(unix)]
    {
        let target = root.join("target");
        std::fs::write(&target, format!("origin-{}\n", "a".repeat(64))).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert_eq!(
            request(vec![index], "search", json!({"query":"needle"})).unwrap_err()["error"]["code"],
            "store_unavailable"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_receiver_bound_index_refuses_before_body_access() {
    let (root, index) = fixture('a');
    {
        let mut cache = EventCache::open_scoped(&index.with_extension("json")).unwrap();
        let namespace = crate::storage::Namespace::new("received-machine").unwrap();
        crate::storage::sqlite::bind(&mut cache, &namespace).unwrap();
    }
    let connection = Connection::open(&index).unwrap();
    connection
        .execute_batch("DROP TABLE body_fts; DROP TABLE body")
        .unwrap();
    drop(connection);
    let failure = request(vec![index], "search", json!({"query":"needle"})).unwrap_err();
    assert_eq!(failure["error"]["code"], "capability_unavailable");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespace_is_required_and_full_uid_must_match() {
    let (root, index) = fixture('a');
    insert(&index, "needle", 0);
    std::fs::remove_file(root.join("analytics-namespace")).unwrap();
    std::fs::remove_dir_all(root.join("analytics-store")).unwrap();
    assert_eq!(
        request(vec![index.clone()], "search", json!({"query":"needle"})).unwrap_err()["error"]["code"],
        "not_materialized"
    );
    std::fs::create_dir_all(root.join("analytics-store/analytics")).unwrap();
    std::fs::write(root.join("analytics-store/analytics/manifest.json"), "{}").unwrap();
    assert_eq!(
        request(vec![index], "search", json!({"query":"needle"})).unwrap_err()["error"]["code"],
        "store_unavailable"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_tool_owner_survives_restart_and_source_rewrite_is_atomic() {
    let (root, index) = fixture('a');
    let cache = index.with_extension("json");
    let source = root.join("session.jsonl");
    let records = [
        json!({"type":"user","uuid":"prompt","sessionId":"session","message":{"content":"question"}}),
        json!({"type":"assistant","uuid":"answer","sessionId":"session","message":{"id":"model","content":[{"type":"text","text":"assistantneedle"},{"type":"tool_use","id":"call","name":"Bash","input":{"command":"commandneedle"}}]}}),
    ];
    std::fs::write(
        &source,
        records
            .iter()
            .map(|record| format!("{record}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let scan = || {
        let mut cache = EventCache::open_scoped(&cache).unwrap();
        crate::events::scan_file(&source, "claude", &mut cache, &mut false).unwrap();
    };
    scan();
    use std::io::Write;
    let result = json!({"type":"user","sessionId":"session","message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"outputneedle","is_error":true}]}});
    std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(format!("{result}\n").as_bytes())
        .unwrap();
    scan();
    let result = request(
        vec![index.clone()],
        "search",
        json!({"query":"outputneedle","part":"error"}),
    )
    .unwrap();
    assert_eq!(
        result["hits"][0]["step_uid"],
        format!("origin-{}/claude/session/s/claude/model", "a".repeat(64))
    );
    let result = request(
        vec![index.clone()],
        "view",
        json!({"uid":result["hits"][0]["step_uid"]}),
    )
    .unwrap();
    assert_eq!(result["bodies"].as_array().unwrap().len(), 3);
    let connection = Connection::open(&index).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    connection.execute("DELETE FROM body", []).unwrap();
    assert_eq!(
        request(
            vec![index.clone()],
            "search",
            json!({"query":"assistantneedle"})
        )
        .unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tail_indexing_never_rewrites_old_bodies() {
    let (root, index) = fixture('a');
    let cache = index.with_extension("json");
    let source = root.join("session.jsonl");
    let line = |ordinal| {
        format!(
            "{}\n",
            json!({"type":"user","uuid":format!("turn-{ordinal}"),"sessionId":"session","message":{"content":"needle"}})
        )
    };
    std::fs::write(&source, (0..128).map(line).collect::<String>()).unwrap();
    let scan = || {
        let mut cache = EventCache::open_scoped(&cache).unwrap();
        crate::events::scan_file(&source, "claude", &mut cache, &mut false).unwrap();
    };
    scan();
    let connection = Connection::open(&index).unwrap();
    connection.execute_batch("CREATE TRIGGER old_body_update BEFORE UPDATE ON body BEGIN SELECT RAISE(ABORT,'old body update'); END; CREATE TRIGGER old_body_delete BEFORE DELETE ON body BEGIN SELECT RAISE(ABORT,'old body delete'); END;").unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(line(128).as_bytes())
        .unwrap();
    scan();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM body", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        129
    );
    drop(connection);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_native_call_id_never_infers_the_output_step() {
    let (root, index) = fixture('a');
    let mut batch = crate::events::store::bodies::Batch::default();
    batch.begin(Path::new("session.jsonl"), "claude");
    let records = [
        json!({"type":"user","uuid":"prompt","message":{"content":"question"}}),
        json!({"type":"assistant","message":{"id":"first","content":[{"type":"tool_use","id":"duplicate","input":{"command":"first"}}]}}),
        json!({"type":"assistant","message":{"id":"second","content":[{"type":"tool_use","id":"duplicate","input":{"command":"second"}}]}}),
        json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"duplicate","content":"ambiguousneedle"}]}}),
    ];
    for (ordinal, record) in records.iter().enumerate() {
        batch
            .capture("claude", record, Some(ordinal as u64))
            .unwrap();
    }
    let mut connection = Connection::open(&index).unwrap();
    let tx = connection.transaction().unwrap();
    crate::events::store::bodies::write(&tx, "session.jsonl", &batch, false).unwrap();
    tx.commit().unwrap();
    drop(connection);
    let result = request(vec![index], "search", json!({"query":"ambiguousneedle"})).unwrap();
    assert!(result["hits"][0]["step_uid"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn append_restart_and_rewrite_leave_no_stale_bodies() {
    let (root, index) = fixture('a');
    let cache = index.with_extension("json");
    let source = root.join("session.jsonl");
    let assert_cold = || {
        let (cold_root, cold_index) = fixture('b');
        let cold_source = cold_root.join("session.jsonl");
        std::fs::copy(&source, &cold_source).unwrap();
        let mut cold_cache = EventCache::open_scoped(&cold_index.with_extension("json")).unwrap();
        crate::events::scan_file(&cold_source, "claude", &mut cold_cache, &mut false).unwrap();
        let snapshot = |path: &Path| {
            let connection = Connection::open(path).unwrap();
            let mut statement = connection.prepare("SELECT json_array(body_uid,session_uid,turn_uid,step_uid,part,ord,ts,text) FROM body ORDER BY body_uid").unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let postings: i64 = connection.query_row("SELECT count(*) FROM body_fts WHERE body_fts MATCH 'oldneedle OR newneedle OR replacement'", [], |row| row.get(0)).unwrap();
            let contributions: i64 = connection
                .query_row("SELECT count(*) FROM body_source", [], |row| row.get(0))
                .unwrap();
            (rows, postings, contributions)
        };
        assert_eq!(snapshot(&index), snapshot(&cold_index));
        drop(cold_cache);
        std::fs::remove_dir_all(cold_root).unwrap();
    };
    let line = |role: &str, text: &str| {
        format!(
            "{}\n",
            json!({"type":role,"uuid":format!("{role}-uuid"),"sessionId":"session","message":{"id":"message","content":text}})
        )
    };
    std::fs::write(&source, line("user", "oldneedle")).unwrap();
    let scan = || {
        let mut cache = EventCache::open_scoped(&cache).unwrap();
        crate::events::scan_file(&source, "claude", &mut cache, &mut false).unwrap();
    };
    scan();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(line("assistant", "newneedle").as_bytes())
        .unwrap();
    scan();
    assert_cold();
    let value = request(vec![index.clone()], "search", json!({"query":"newneedle"})).unwrap();
    let hit = &value["hits"][0];
    assert_eq!(
        hit["turn_uid"],
        format!("origin-{}/claude/session/t/user-uuid", "a".repeat(64))
    );
    assert_eq!(
        hit["step_uid"],
        format!("origin-{}/claude/session/s/claude/message", "a".repeat(64))
    );
    let value = request(vec![index.clone()], "view", json!({"uid":hit["step_uid"]})).unwrap();
    assert_eq!(value["bodies"][0]["text"]["untrusted_text"], "newneedle");
    std::fs::write(&source, line("user", "replacement")).unwrap();
    scan();
    assert_cold();
    let value = request(
        vec![index],
        "search",
        json!({"query":"oldneedle OR newneedle"}),
    )
    .unwrap();
    assert!(value["hits"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
