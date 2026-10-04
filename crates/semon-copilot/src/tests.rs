use super::*;
use semon_sessions::{Options, ViewerCore};
static FIXTURE: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "semon-copilot-{}-{}",
            std::process::id(),
            FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn opts(&self, home: &Path, cache: &str) -> Options {
        Options {
            claude_home: self.0.join("unused-claude"),
            claude_json: self.0.join("unused-settings"),
            codex_home: self.0.join("unused-codex"),
            copilot_home: home.to_owned(),
            proc_root: self.0.join("unused-proc"),
            cache: self.0.join(cache).join("view.json"),
            all: true,
            ..Default::default()
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(version: &str, name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/compatibility")
            .join(format!("copilot-{version}"))
            .join(name),
    )
    .unwrap()
}
fn install(home: &Path, bytes: &[u8]) -> PathBuf {
    let line = bytes.split(|byte| *byte == b'\n').next().unwrap();
    let record: Value = serde_json::from_slice(line).unwrap();
    let id = record["data"]["sessionId"].as_str().unwrap();
    let path = home.join("session-state").join(id).join("events.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}
fn model(options: &Options) -> Value {
    serde_json::from_str(&semon_sessions::model_json_at(options, 1_791_072_000_000).unwrap())
        .unwrap()
}
fn drain(c: &mut Collector) {
    while c.collect().unwrap() > 0 {}
}

#[test]
fn native_families_are_read_only_and_browsable_on_both_releases() {
    for version in ["1.0.90", "1.0.91"] {
        for name in [
            "headless-tools.events.jsonl",
            "lifecycle/resumed.events.jsonl",
            "lifecycle/denied.events.jsonl",
            "boundaries/interactive.events.jsonl",
            "boundaries/compaction.events.jsonl",
            "boundaries/cancel.events.jsonl",
            "subagent/task.events.jsonl",
        ] {
            let root = Temp::new();
            let home = root.0.join("native");
            let bytes = fixture(version, name);
            let path = install(&home, &bytes);
            let options = root.opts(&home, "index");
            let native_id = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            let json = model(&options);
            let session = &json["sessions"][native_id];
            assert_eq!(session["harness"], "copilot", "{version}/{name}");
            assert_eq!(session["copilot"]["version"], version);
            assert!(session["copilot"]["logical_parent"].is_null());
            assert!(session["copilot"]["approvals"].is_null());
            assert!(session.get("parent").is_none());
            assert!(session["cost"]["usd"].is_null());
            let core = ViewerCore::new(options.clone());
            let reply = core.respond("GET", "/api/tx", &format!("sid={native_id}"), None);
            assert_eq!(reply.status, 200, "{version}/{name}");
            let tx: Value = serde_json::from_slice(&reply.body).unwrap();
            assert!(!tx["entries"].as_array().unwrap().is_empty());
            let records: Vec<Value> = bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice(line).unwrap())
                .collect();
            let tools: Vec<_> = tx["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["k"] == "tool")
                .collect();
            for complete in records
                .iter()
                .filter(|record| record["type"] == "tool.execution_complete")
            {
                let call = &complete["data"]["toolCallId"];
                let tool = tools
                    .iter()
                    .find(|tool| tool["native"]["tool_call_id"] == *call)
                    .expect("exact native call in transcript");
                assert_eq!(tool["native"]["result_event_id"], complete["id"]);
                assert_eq!(
                    tool["native"]["protocol_success"],
                    complete["data"]["success"]
                );
                assert_eq!(
                    tool["native"]["shell_exit_code"],
                    complete["data"]["shellExecution"]["exitCode"]
                );
                if let Some(exit) = complete["data"]["shellExecution"]["exitCode"].as_i64() {
                    assert_eq!(
                        tool["ok"],
                        exit == 0,
                        "protocol success must not hide command failure"
                    );
                } else if complete["data"]["success"] == false {
                    assert_eq!(tool["ok"], false);
                }
            }
            let expected = records
                .iter()
                .rev()
                .find(|record| record["type"] == "session.shutdown")
                .and_then(|record| record["data"]["modelMetrics"].as_object())
                .map(|models| {
                    models
                        .values()
                        .map(|report| report["usage"]["inputTokens"].as_u64().unwrap())
                        .sum::<u64>()
                });
            assert_eq!(
                session["copilot"]["usage"]["tokens"]["input"].as_u64(),
                expected,
                "latest native snapshot only; no child or compaction addition"
            );

            let roots = semon_sessions::collect(&options).unwrap();
            assert_eq!(roots.len(), 1);
            assert_eq!(roots[0].harness, "copilot");
            assert_eq!(roots[0].copilot.as_ref().unwrap().version, version);
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn native_resume_cold_restart_and_retained_custody_rebuild_agree() {
    for version in ["1.0.90", "1.0.91"] {
        let root = Temp::new();
        let home = root.0.join("native");
        let initial = fixture(version, "lifecycle/initial.events.jsonl");
        let resumed = fixture(version, "lifecycle/resumed.events.jsonl");
        assert!(resumed.starts_with(&initial));
        let path = install(&home, &initial);
        let state = root.0.join("private/state.json");
        let db = root.0.join("private/store.db");
        let options = root.opts(&home, "incremental");
        let mut c = Collector::open(&home, &state, &db).unwrap();
        drain(&mut c);
        model(&options);
        drop(c);
        // Partial final record survives a collector and SQLite restart.
        let split = resumed.len()
            - resumed
                .split(|byte| *byte == b'\n')
                .rev()
                .nth(1)
                .unwrap()
                .len()
                / 2
            - 1;
        fs::write(&path, &resumed[..split]).unwrap();
        let mut c = Collector::open(&home, &state, &db).unwrap();
        drain(&mut c);
        model(&options);
        drop(c);
        fs::write(&path, &resumed).unwrap();
        let mut c = Collector::open(&home, &state, &db).unwrap();
        drain(&mut c);
        let warm = model(&options);
        let cold = model(&root.opts(&home, "cold"));
        assert_eq!(warm["sessions"], cold["sessions"]);
        let retained = c
            .store
            .fetch_current_capture_source_raw(CARRIER, &path.to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(retained, resumed);
        let replay = root.0.join("retained");
        install(&replay, &retained);
        assert_eq!(
            warm["sessions"],
            model(&root.opts(&replay, "replay"))["sessions"]
        );
        let id = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let usage = &warm["sessions"][id]["copilot"]["usage"]["tokens"];
        assert_eq!(usage["input"], 22);
        assert_eq!(usage["output"], 6);
        assert_eq!(usage["cached_input"], 4);
        assert_eq!(fs::read(&path).unwrap(), resumed);
        let cursor = fs::read(&state).unwrap();
        assert_eq!(c.collect().unwrap(), 0);
        assert_eq!(fs::read(&state).unwrap(), cursor);
        drop(c);
        let restarted = Collector::open(&home, &state, &db).unwrap();
        let mut rebuilt = Collector::open(
            &replay,
            &root.0.join("replay-state/state.json"),
            &root.0.join("replay-state/store.db"),
        )
        .unwrap();
        drain(&mut rebuilt);
        assert_eq!(c_log(&restarted), c_log(&rebuilt));
    }
}
fn c_log(c: &Collector) -> Vec<Value> {
    c.store.log(&semon_store::LogFilter::default()).unwrap().iter().map(|row|json!({"id":row.trace_id().as_str(),"session":row.session(),"sequence":row.sequence(),"parent":row.parent_sequence()})).collect()
}

#[test]
fn replacements_mutations_missing_sources_and_lost_cursor_preserve_raw_custody() {
    let root = Temp::new();
    let home = root.0.join("native");
    let original = fixture("1.0.91", "lifecycle/resumed.events.jsonl");
    let path = install(&home, &original);
    let state = root.0.join("private/state.json");
    let db = root.0.join("private/store.db");
    let mut c = Collector::open(&home, &state, &db).unwrap();
    drain(&mut c);
    // Equal-length interior mutation and longer replacement, keeping valid native ids.
    let mutated = String::from_utf8(original.clone())
        .unwrap()
        .replace("SEMON_SYNTHETIC_initial", "SEMON_SYNTHETIC_changed")
        .into_bytes();
    assert_ne!(mutated, original);
    assert_eq!(mutated.len(), original.len());
    fs::write(&path, &mutated).unwrap();
    drain(&mut c);
    assert_eq!(
        c.store
            .fetch_current_capture_source_raw(CARRIER, &path.to_string_lossy())
            .unwrap()
            .unwrap(),
        mutated
    );
    let mut longer = mutated.clone();
    longer.extend_from_slice(b"not JSON\n");
    let new = path.with_extension("replacement");
    fs::write(&new, &longer).unwrap();
    fs::rename(new, &path).unwrap();
    drain(&mut c);
    assert_eq!(
        c.store
            .fetch_current_capture_source_raw(CARRIER, &path.to_string_lossy())
            .unwrap()
            .unwrap(),
        longer
    );
    // Truncate to an unfinished frame, restart, and grow through the old offset.
    fs::write(&path, &original[..20]).unwrap();
    assert_eq!(c.collect().unwrap(), 0);
    drop(c);
    fs::write(&path, &original).unwrap();
    let mut c = Collector::open(&home, &state, &db).unwrap();
    drain(&mut c);
    assert_eq!(
        c.store
            .fetch_current_capture_source_raw(CARRIER, &path.to_string_lossy())
            .unwrap()
            .unwrap(),
        original
    );
    drop(c);
    fs::remove_file(&state).unwrap();
    let mut c = Collector::open(&home, &state, &db).unwrap();
    drain(&mut c);
    assert_eq!(
        c.store
            .fetch_current_capture_source_raw(CARRIER, &path.to_string_lossy())
            .unwrap()
            .unwrap(),
        original
    );
    let before = c_log(&c);
    fs::remove_file(&path).unwrap();
    assert_eq!(c.collect().unwrap(), 0);
    assert_eq!(before, c_log(&c));
    // Retired source generations remain retained; current evidence does not erase them.
    let raw = c
        .store
        .count_forensic_forget(semon_store::ForgetSelector::Before(i64::MAX))
        .unwrap();
    assert!(raw > original.split(|b| *b == b'\n').count() as u64);
}

#[test]
fn unknown_usage_and_ambiguous_calls_do_not_become_complete_work() {
    let root = Temp::new();
    let home = root.0.join("native");
    let original = fixture("1.0.91", "boundaries/cancel.events.jsonl");
    let path = install(&home, &original);
    let options = root.opts(&home, "index");
    let id = path
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let json = model(&options);
    assert!(json["sessions"][id]["copilot"]["usage"].is_null());
    let core = ViewerCore::new(options.clone());
    let tx: Value = serde_json::from_slice(
        &core
            .respond("GET", "/api/tx", &format!("sid={id}"), None)
            .body,
    )
    .unwrap();
    let pending = tx["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["k"] == "tool")
        .collect::<Vec<_>>();
    assert!(!pending.is_empty());
    for entry in pending {
        assert!(entry["ok"].is_null());
        assert!(entry.get("unfinished").is_none());
        assert!(entry["native"]["result_event_id"].is_null());
    }
    // An exact id repeated across two different requests is ambiguous, even after
    // one previously accepted result; neither proximity nor matching names wins.
    let start: Value =
        serde_json::from_slice(original.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let records = [
        start,
        json!({"id":"a1","parentId":null,"type":"assistant.message","data":{"content":"","toolRequests":[{"toolCallId":"same","name":"view","arguments":{"path":"one"}}]}}),
        json!({"id":"r1","parentId":"a1","type":"tool.execution_complete","data":{"toolCallId":"same","success":true,"result":{"content":"one"}}}),
        json!({"id":"a2","parentId":"r1","type":"assistant.message","data":{"content":"","toolRequests":[{"toolCallId":"same","name":"view","arguments":{"path":"two"}}]}}),
        json!({"id":"r2","parentId":"a2","type":"tool.execution_complete","data":{"toolCallId":"same","success":true,"result":{"content":"two"}}}),
    ];
    let text = records
        .iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
    fs::write(&path, text).unwrap();
    let core = ViewerCore::new(options);
    let tx: Value = serde_json::from_slice(
        &core
            .respond("GET", "/api/tx", &format!("sid={id}"), None)
            .body,
    )
    .unwrap();
    let tools = tx["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["k"] == "tool")
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 2);
    for tool in tools {
        assert!(tool["ok"].is_null());
        assert!(tool.get("unfinished").is_none());
        assert!(tool.get("out").is_none());
    }
}

#[test]
fn unsupported_versions_auxiliary_paths_and_links_are_not_inputs() {
    let root = Temp::new();
    let home = root.0.join("native");
    let original = fixture("1.0.91", "headless-tools.events.jsonl");
    let path = install(&home, &original);
    let options = root.opts(&home, "index");
    for name in [
        "session-state/id/workspace.yaml",
        "session-state/id/checkpoints/1.md",
        "session-store.db",
        "session-store.db-wal",
        "logs/events.jsonl",
        "session-state/id/rewind-snapshots/events.jsonl",
    ] {
        assert!(!semon_sessions::is_input_path("copilot", name));
    }
    fs::write(
        path.parent().unwrap().join("workspace.yaml"),
        "Bearer PRIVATE_WORKSPACE",
    )
    .unwrap();
    assert_eq!(candidate_files(&home).unwrap(), vec![path.clone()]);
    let unsupported = String::from_utf8(original.clone()).unwrap().replace(
        "\"copilotVersion\":\"1.0.91\"",
        "\"copilotVersion\":\"9.9.9\"",
    );
    fs::write(&path, &unsupported).unwrap();
    let error = semon_sessions::model_json_at(&options, 1_791_072_000_000).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported Copilot persisted format")
    );
    let mut c = Collector::open(
        &home,
        &root.0.join("private/state.json"),
        &root.0.join("private/store.db"),
    )
    .unwrap();
    assert!(c.collect().is_err());
    assert!(c_log(&c).is_empty());
    fs::write(&path, &original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(&path).unwrap();
        let secret = root.0.join("secret");
        fs::write(&secret, &original).unwrap();
        symlink(&secret, &path).unwrap();
        assert!(candidate_files(&home).unwrap().is_empty());
        assert!(semon_sessions::open_read_only_input(&path).is_err());
    }
    assert!(
        Collector::open(
            &home,
            &home.join("state.json"),
            &root.0.join("private/other.db")
        )
        .is_err()
    );
}

#[test]
fn independent_machines_keep_native_calls_separate_and_copied_ids_refuse_ownership() {
    let root = Temp::new();
    let a = root.0.join("a/copilot");
    let b = root.0.join("b/copilot");
    let first = fixture("1.0.90", "headless-tools.events.jsonl");
    let second = fixture("1.0.91", "headless-tools.events.jsonl");
    let ap = install(&a, &first);
    let bp = install(&b, &second);
    let id = |p: &Path| {
        p.parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    let ai = id(&ap);
    let bi = id(&bp);
    let opts_a = root.opts(&a, "index-a");
    let opts_b = root.opts(&b, "index-b");
    let core = ViewerCore::with_machines(vec![
        ("laptop".into(), opts_a.clone()),
        ("desktop".into(), opts_b.clone()),
    ]);
    for native in [&ai, &bi] {
        let reply = core.respond("GET", "/api/tx", &format!("sid={native}"), None);
        assert_eq!(reply.status, 200);
        let tx: Value = serde_json::from_slice(&reply.body).unwrap();
        let tools: Vec<_> = tx["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["k"] == "tool")
            .collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(
            tools
                .iter()
                .filter(|entry| entry["native"]["tool_call_id"] == "mock-call-1"
                    && entry["ok"] == false)
                .count(),
            1
        );
    }
    // A copied native session UUID on another machine is not proof of ownership.
    install(&b, &first);
    let conflicting =
        ViewerCore::with_machines(vec![("laptop".into(), opts_a), ("desktop".into(), opts_b)]);
    assert_eq!(
        conflicting
            .respond("GET", "/api/tx", &format!("sid={ai}"), None)
            .status,
        409
    );
    assert_eq!(fs::read(ap).unwrap(), first);
    assert_eq!(fs::read(bp).unwrap(), second);
}

#[test]
fn synthetic_collection_measures_idle_and_growing_sources() {
    let root = Temp::new();
    let home = root.0.join("native");
    let sample = fixture("1.0.91", "headless-tools.events.jsonl");
    let header: Value =
        serde_json::from_slice(sample.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let mut paths = Vec::new();
    for session in 0..4 {
        let mut start = header.clone();
        start["data"]["sessionId"] = json!(format!("synthetic-load-{session}"));
        let mut bytes = serde_json::to_vec(&start).unwrap();
        bytes.push(b'\n');
        for record in 0..250 {
            let event = json!({"id":format!("record-{record}"),"type":"user.message","parentId":null,"timestamp":"2026-10-03T12:00:00Z","data":{"content":"SEMON_SYNTHETIC_load"}});
            serde_json::to_writer(&mut bytes, &event).unwrap();
            bytes.push(b'\n');
        }
        paths.push(install(&home, &bytes));
    }
    let state = root.0.join("private/state.json");
    let db = root.0.join("private/store.db");
    let mut collector = Collector::open(&home, &state, &db).unwrap();
    let cold = std::time::Instant::now();
    drain(&mut collector);
    let cold_ms = cold.elapsed().as_secs_f64() * 1000.0;
    // Exercise Copilot source references across the public bounded paging API.
    let options = root.opts(&home, "paged-view");
    let core = ViewerCore::new(options.clone());
    for path in &paths {
        let id = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let mut before = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let query = format!(
                "sid={id}{}",
                before.map(|n| format!("&before={n}")).unwrap_or_default()
            );
            let reply = core.respond("GET", "/api/tx", &query, None);
            assert_eq!(reply.status, 200);
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            let entries = page["entries"].as_array().unwrap();
            assert!(!entries.is_empty() && entries.len() <= 200);
            for entry in entries.iter().filter(|entry| entry["k"] == "u") {
                assert!(seen.insert(entry["native"]["event_id"].as_str().unwrap().to_owned()));
            }
            let from = page["from"].as_u64().unwrap();
            if from == 0 {
                break;
            }
            before = Some(from);
        }
        assert_eq!(seen.len(), 250);
        let mut query = semon_sessions::Query::new(options.clone());
        let answer = query
            .call_at("get_session", &json!({"id":id}), 1_791_072_000_000)
            .unwrap();
        assert_eq!(answer["session"]["copilot"]["version"], "1.0.91");
        assert!(answer["session"]["copilot"]["usage"].is_null());
        assert!(answer["session"]["tokens"].is_null());
        let page = query
            .call_at(
                "read_transcript",
                &json!({"id":id,"limit":7}),
                1_791_072_000_000,
            )
            .unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), 7);
        let input = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_session","arguments":{"id":id}}})
        );
        let mut output = Vec::new();
        semon_sessions::serve_mcp(&mut query, input.as_bytes(), &mut output).unwrap();
        let reply: Value = serde_json::from_slice(&output).unwrap();
        let answer: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(answer["session"]["copilot"]["version"], "1.0.91");
    }
    let cursor = fs::read(&state).unwrap();
    let idle = std::time::Instant::now();
    for _ in 0..20 {
        assert_eq!(collector.collect().unwrap(), 0);
    }
    let idle_ms = idle.elapsed().as_secs_f64() * 1000.0 / 20.0;
    assert_eq!(fs::read(&state).unwrap(), cursor);
    for path in &paths {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{}", json!({"id":"appended-record","type":"user.message","parentId":"record-249","timestamp":"2026-10-03T12:01:00Z","data":{"content":"SEMON_SYNTHETIC_append"}})).unwrap();
    }
    let append = std::time::Instant::now();
    assert_eq!(collector.collect().unwrap(), 4);
    let append_ms = append.elapsed().as_secs_f64() * 1000.0;
    let expected = c_log(&collector);
    drop(collector);
    // Simulate durable records committed while the checkpoint still contains the
    // previous offset. Restart must reconcile custody before advancing it again.
    fs::write(&state, cursor).unwrap();
    let mut restarted = Collector::open(&home, &state, &db).unwrap();
    drain(&mut restarted);
    assert_eq!(c_log(&restarted), expected);
    println!(
        "synthetic saved-state measurement: 4 sessions / 1004 initial records; cold={cold_ms:.2}ms, idle_mean={idle_ms:.3}ms over 20 passes, four appends={append_ms:.2}ms; watch interval=2000ms; unchanged cursor preserved"
    );
}
