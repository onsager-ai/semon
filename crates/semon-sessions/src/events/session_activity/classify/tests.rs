use super::*;
use serde_json::json;

fn observed(name: &str, command: &str, output: Value, error: bool) -> Observation {
    let mut observation = call(
        name,
        &json!({"cmd":command, "command":command}),
        None,
        "call-digest".into(),
    );
    observation.envelopes = true;
    observation.formatter = name == "exec_command";
    finish(
        &mut observation,
        &output,
        error,
        Some(100),
        Some(7_200_100),
        "result-digest".into(),
    );
    observation
}

#[test]
fn waste_table_uses_explicit_outcomes_and_separate_empty_search() {
    let cases = [
        (
            "Agent",
            "",
            json!("done"),
            false,
            Some(WasteKind::OrchestrationWait),
            false,
        ),
        (
            "Task",
            "",
            json!("done"),
            false,
            Some(WasteKind::OrchestrationWait),
            false,
        ),
        (
            "wait_agent",
            "",
            json!("done"),
            false,
            Some(WasteKind::OrchestrationWait),
            false,
        ),
        (
            "Bash",
            "gh run watch 123",
            json!("done"),
            false,
            Some(WasteKind::OrchestrationWait),
            false,
        ),
        (
            "Bash",
            "gh pr checks --watch",
            json!("auth failed"),
            true,
            Some(WasteKind::OrchestrationWait),
            false,
        ),
        (
            "Bash",
            "sleep 5",
            json!(""),
            false,
            Some(WasteKind::Sleep),
            false,
        ),
        (
            "Bash",
            "rg needle file",
            json!({"exit_code":1,"output":""}),
            false,
            None,
            true,
        ),
        (
            "Bash",
            "cat absent",
            json!("cat: absent: No such file or directory"),
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Bash",
            "bad-command",
            json!({"metadata":{"exit_code":127},"output":"command not found"}),
            false,
            Some(WasteKind::MalformedCommand),
            false,
        ),
        (
            "Bash",
            "gh api user",
            json!({"exit_code":1,"output":"HTTP 503"}),
            false,
            Some(WasteKind::ExternalFailure),
            false,
        ),
        (
            "Bash",
            "gh api user",
            json!("rate limit"),
            true,
            Some(WasteKind::ExternalFailure),
            false,
        ),
        (
            "Bash",
            "gh api user",
            json!("authentication failed"),
            true,
            Some(WasteKind::ExternalFailure),
            false,
        ),
        (
            "Bash",
            "gh api user",
            json!("could not resolve host"),
            true,
            Some(WasteKind::ExternalFailure),
            false,
        ),
        (
            "Bash",
            "tool",
            json!("unclassified"),
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "cat file",
            json!({"exit_code":0,"output":"No such file or directory; HTTP 503; syntax error"}),
            false,
            None,
            false,
        ),
        (
            "Bash",
            "cat file",
            json!("No such file or directory"),
            false,
            None,
            false,
        ),
        (
            "Bash",
            "git status; cat absent",
            json!({"exit_code":1,"output":"cat: absent: No such file or directory"}),
            false,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Bash",
            "cat one; cat absent",
            json!({"exit_code":1,"output":"cat: absent: No such file or directory"}),
            false,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "git status; gh api user",
            json!({"exit_code":1,"output":"HTTP 503"}),
            false,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "git status; cat absent",
            json!({"exit_code":0,"output":"cat: absent: No such file or directory"}),
            false,
            None,
            false,
        ),
    ];
    for (name, command, output, error, waste, empty) in cases {
        let observation = observed(name, command, output, error);
        assert_eq!(observation.waste, waste, "{name} {command}");
        assert_eq!(observation.empty_search, empty, "{name} {command}");
        if waste == Some(WasteKind::OrchestrationWait) {
            assert_eq!(
                observation.wait,
                Some(WaitInterval {
                    started_at_ms: 100,
                    completed_at_ms: 7_200_100
                })
            );
        }
    }
    assert_eq!(WasteKind::PathNotFound.category(), WasteCategory::Tooling);
    assert_eq!(
        WasteKind::ExternalFailure.category(),
        WasteCategory::External
    );
    assert_eq!(WasteKind::Refill.category(), WasteCategory::Context);
    assert_eq!(WasteKind::RedundantRead.category(), WasteCategory::Unmapped);
}

#[test]
fn nested_wait_and_decoding_are_bounded_and_call_qualified() {
    let wait = json!({"type":"CollabAgentToolCall","tool":"wait","started_at_ms":10,"completed_at_ms":7_200_010});
    let wrapped = json!({"output":serde_json::to_string(&wait.to_string()).unwrap()});
    let observation = observed("exec_command", "unknown", wrapped, false);
    assert_eq!(observation.waste, Some(WasteKind::OrchestrationWait));
    assert_eq!(
        observation.wait,
        Some(WaitInterval {
            started_at_ms: 10,
            completed_at_ms: 7_200_010
        })
    );
    let observation = observed(
        "exec_command",
        "unknown",
        json!({"type":"CollabAgentToolCall","tool":"wait","started_at_ms":10}),
        false,
    );
    assert_eq!(observation.waste, Some(WasteKind::OrchestrationWait));
    // The tool reply can bound its wait even when the nested end is absent.
    assert_eq!(observation.wait.unwrap().completed_at_ms, 7_200_100);
    let mut pending = call("Agent", &Value::Null, None, "hash".into());
    assert!(pending.wait.is_none());
    finish(
        &mut pending,
        &json!("done"),
        false,
        Some(10),
        None,
        "hash".into(),
    );
    assert!(pending.wait.is_none());

    let native = "Wall time: 0.1000 seconds\nProcess exited with code 2\nOutput:\ncat: missing: No such file or directory";
    assert_eq!(
        observed("exec_command", "cat missing", json!(native), false).waste,
        Some(WasteKind::PathNotFound)
    );
    assert_eq!(
        observed("Bash", "cat missing", json!(native), false).waste,
        None
    );
    let blocks = json!([
        {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
        {"type":"input_text","text":json!({"metadata":{"exit_code":2},"output":"cat: missing: No such file or directory"}).to_string()}
    ]);
    assert_eq!(
        observed("exec_command", "cat missing", blocks, false).waste,
        Some(WasteKind::PathNotFound)
    );
    let decoy =
        json!({"exit_code":0,"output":json!({"exit_code":1,"output":"HTTP 503"}).to_string()});
    assert!(
        observed("exec_command", "cat file", decoy, false)
            .waste
            .is_none()
    );
    assert!(matches!(
        command(&["sed".into(), "-n".into(), "1,5p".into(), "file".into()]),
        Command::Read(_, _)
    ));
    assert_eq!(
        command(&[
            "sed".into(),
            "-n".into(),
            "1e touch file".into(),
            "file".into()
        ]),
        Command::Unknown
    );
    let mut too_deep = json!({"exit_code":1,"output":"HTTP 503"});
    for _ in 0..=MAX_DEPTH {
        too_deep = json!({"output":too_deep});
    }
    for invalid in [
        too_deep,
        json!("x".repeat(MAX_BYTES + 1)),
        json!({"arbitrary":{"exit_code":1,"output":"HTTP 503"}}),
    ] {
        let observation = observed("exec_command", "cat missing", invalid, false);
        assert!(observation.waste.is_none());
        assert!(!observation.completed);
    }
    let mut claude = call(
        "Bash",
        &json!({"command":"cat missing"}),
        None,
        "hash".into(),
    );
    finish(
        &mut claude,
        &json!({"exit_code":1,"output":"HTTP 503"})
            .to_string()
            .into(),
        false,
        None,
        None,
        "hash".into(),
    );
    assert!(
        claude.waste.is_none(),
        "successful Claude text is not an exec envelope"
    );
}

fn located(offset: u64, observation: Observation) -> LocatedObservation {
    LocatedObservation {
        position: SourcePosition {
            source: "synthetic".into(),
            offset,
        },
        timestamp: Some(offset as i64),
        result_offset: Some(offset + 1),
        observation,
    }
}

#[test]
fn read_proofs_respect_compaction_mutation_ranges_and_unknown_calls() {
    for (middle, compact, expected) in [
        (None, false, Some(WasteKind::RedundantRead)),
        (None, true, Some(WasteKind::Refill)),
        (Some("touch file"), false, None),
        (Some("unrecognized command"), false, None),
        (Some("touch file"), true, None),
        (Some("git status"), true, Some(WasteKind::Refill)),
    ] {
        let mut rows = vec![located(
            10,
            observed("Bash", "cat file", json!("body"), false),
        )];
        if let Some(command) = middle {
            rows.push(located(20, observed("Bash", command, json!("done"), false)));
        }
        rows.push(located(
            30,
            observed("Bash", "cat file", json!("body"), false),
        ));
        let compactions = if compact {
            vec![("synthetic".into(), 15)]
        } else {
            Vec::new()
        };
        classify_reads(&mut rows, &compactions);
        assert_eq!(
            rows.last().unwrap().observation.waste,
            expected,
            "{middle:?} {compact}"
        );
    }
    let mut rows = vec![
        located(10, observed("Bash", "cat file", json!("body"), false)),
        located(20, observed("Bash", "cat other", json!("body"), false)),
    ];
    classify_reads(&mut rows, &[]);
    assert!(rows[1].observation.waste.is_none());
    let mut rows = Vec::new();
    for (offset, limit) in [(10, 5), (20, 10), (30, 10)] {
        let mut observation = call(
            "Read",
            &json!({"file_path":"./file","offset":1,"limit":limit}),
            None,
            "hash".into(),
        );
        finish(
            &mut observation,
            &json!("body"),
            false,
            None,
            None,
            "hash".into(),
        );
        rows.push(located(offset, observation));
    }
    classify_reads(&mut rows, &[]);
    assert!(rows[1].observation.waste.is_none());
    assert_eq!(rows[2].observation.waste, Some(WasteKind::RedundantRead));
    let mut mutation = located(5, observed("Bash", "touch file", json!("done"), false));
    mutation.result_offset = Some(100);
    rows.insert(0, mutation);
    for row in &mut rows {
        if row.position.offset > 5 {
            row.observation.waste = None;
        }
    }
    classify_reads(&mut rows, &[]);
    assert!(
        rows.iter()
            .skip(1)
            .all(|row| row.observation.waste.is_none())
    );
}

#[test]
fn change_facts_require_recognized_pairs_and_sanitized_values() {
    let push = "From https://github.com/example/project\n abc1234..def5678 topic -> origin/topic\nTo https://github.com/example/project.git\n abc1234..def5678 topic -> topic\n * [new branch] other -> other\n abc1234..def5678 topic -> origin/decoy\nFrom https://github.com/example/another\n abc1234..def5678 decoy -> decoy";
    let cases = [
        (
            "git commit -m synthetic",
            "[topic abc1234] synthetic",
            vec![Change::Commit {
                branch: "topic".into(),
                sha: "abc1234".into(),
            }],
        ),
        (
            "git push origin topic",
            push,
            vec![
                Change::Push {
                    repository: "example/project".into(),
                    from: "topic".into(),
                    to: "topic".into(),
                },
                Change::Push {
                    repository: "example/project".into(),
                    from: "other".into(),
                    to: "other".into(),
                },
            ],
        ),
        (
            "gh pr create --title synthetic",
            "https://github.com/example/project/pull/42",
            vec![Change::PullRequest {
                repository: "example/project".into(),
                number: 42,
            }],
        ),
        (
            "cat file",
            "[topic abc1234] synthetic\nhttps://github.com/example/project/pull/42",
            vec![],
        ),
        ("git fetch origin", push, vec![]),
        (
            "git push",
            "To https://github.com/example/project?secret=oops\n abc1234..def5678 topic -> topic",
            vec![],
        ),
        (
            "gh pr create",
            "https://github.com/example/project/pull/42?secret=oops",
            vec![],
        ),
    ];
    for (command, output, expected) in cases {
        let observation = observed("Bash", command, json!(output), false);
        assert_eq!(observation.changes, expected, "{command}");
        let row = located(10, observation);
        for change in &row.observation.changes {
            let fact = fact(&row, change.clone());
            assert_eq!(fact, super::fact(&row, change.clone()));
            assert!(fact.step_id.is_none());
            assert_eq!(fact.position.offset, 11);
            assert_eq!(fact.timestamp, Some(10));
            let serialized = serde_json::to_string(&fact).unwrap();
            assert!(!serialized.contains("[topic"));
            assert!(!serialized.contains("secret"));
        }
    }
    assert!(
        observed("Bash", "git commit", json!("[topic abc1234] text"), true)
            .changes
            .is_empty()
    );
}

#[test]
fn scanner_capture_resolves_exact_ids_and_retains_no_bodies() {
    let mut index = FileIndex::default();
    let records = [
        json!({"type":"assistant","message":{"id":"message","role":"assistant","content":[{"type":"tool_use","id":"call","name":"Bash","input":{"command":"git commit -m private-body"}}]}}),
        json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":"[topic abc1234] private-body"}]}}),
    ];
    for (offset, record) in records.iter().enumerate() {
        crate::events::claude(&mut index, record, offset as u64 * 100);
        capture(
            &mut index,
            "claude",
            record,
            offset as u64 * 100,
            &serde_json::to_vec(record).unwrap(),
        );
    }
    let observation = index
        .events
        .iter()
        .find_map(|event| event.activity_observation.as_ref())
        .unwrap();
    assert_eq!(
        observation.step_id,
        Some(ActivityStepId::Claude("message".into()))
    );
    assert!(observation.result_digest.is_some());
    assert_eq!(observation.changes.len(), 1);
    let metadata = serde_json::to_string(observation).unwrap();
    assert!(!metadata.contains("private-body"));
    assert!(!metadata.contains("git commit"));
    assert!(
        !serde_json::to_string(&index.events)
            .unwrap()
            .contains("activity_observation")
    );
}

#[test]
fn codex_custom_exec_nested_wait_is_captured_without_command_guessing() {
    let mut index = FileIndex::default();
    let records = [
        json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"wait-call","name":"exec","input":"console.log(await tools.wait_agent({ids:['synthetic']}))"}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"wait-call","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":json!({"output":json!({"type":"CollabAgentToolCall","tool":"wait","started_at_ms":20,"completed_at_ms":7_200_020}).to_string()}).to_string()}]}}),
    ];
    for (offset, record) in records.iter().enumerate() {
        crate::events::codex(&mut index, record, offset as u64 * 100);
        capture(
            &mut index,
            "codex",
            record,
            offset as u64 * 100,
            &serde_json::to_vec(record).unwrap(),
        );
    }
    let observation = index.events[0].activity_observation.as_ref().unwrap();
    assert_eq!(observation.waste, Some(WasteKind::OrchestrationWait));
    assert_eq!(
        observation.wait.as_ref().unwrap().completed_at_ms,
        7_200_020
    );
    assert!(observation.step_id.is_none());
    assert_eq!(observation.commands, [Command::Unknown]);
}

#[test]
fn running_and_background_results_do_not_prove_reads_or_changes() {
    let running = json!({"session_id":123,"output":"[topic abc1234] partial"});
    let observation = observed("exec_command", "git commit", running, false);
    assert!(!observation.completed);
    assert!(observation.changes.is_empty());
    let running =
        "Wall time: 0.1000 seconds\nProcess running with session ID 123\nOutput:\npartial";
    assert!(!observed("exec_command", "cat file", json!(running), false).completed);
    let mut observation = call(
        "Bash",
        &json!({"command":"cat file","run_in_background":true}),
        None,
        "hash".into(),
    );
    finish(
        &mut observation,
        &json!("partial"),
        false,
        None,
        None,
        "hash".into(),
    );
    assert!(!observation.completed);
    let oversized = call(
        "Read",
        &json!({"file_path":"file","pages": "1".repeat(129)}),
        None,
        "hash".into(),
    );
    assert_eq!(oversized.commands, [Command::Unknown]);
}

#[test]
fn git_global_options_and_all_explicit_server_errors_are_recognized() {
    let observation = observed(
        "Bash",
        "git -C work -c user.name=Example commit -m synthetic",
        json!("[topic abc1234] synthetic"),
        false,
    );
    assert_eq!(
        observation.changes,
        [Change::Commit {
            branch: "topic".into(),
            sha: "abc1234".into()
        }]
    );
    assert_eq!(
        observed("Bash", "gh api user", json!("HTTP 599"), true).waste,
        Some(WasteKind::ExternalFailure)
    );
}

#[test]
fn relative_read_fingerprints_include_working_directory_without_storing_it() {
    let mut first = call(
        "exec_command",
        &json!({"cmd":"cat file","workdir":"/synthetic/one"}),
        None,
        "hash".into(),
    );
    let mut other = call(
        "exec_command",
        &json!({"cmd":"cat file","workdir":"/synthetic/two"}),
        None,
        "hash".into(),
    );
    finish(&mut first, &json!("body"), false, None, None, "hash".into());
    finish(&mut other, &json!("body"), false, None, None, "hash".into());
    assert_ne!(first.commands, other.commands);
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("/synthetic/one")
    );
    let mut rows = vec![located(10, first), located(20, other)];
    classify_reads(&mut rows, &[]);
    assert!(rows[1].observation.waste.is_none());
}

#[test]
fn literal_shell_wrappers_and_safe_change_compounds_table() {
    let pr = "https://github.com/example/project/pull/42";
    let commit = "[topic abc1234] synthetic";
    let cases = [
        (
            "gh pr create --title 'literal $title' --body 'synthetic'",
            pr,
            1,
        ),
        ("cd /synthetic && gh pr create --title synthetic", pr, 1),
        (
            "bash -lc 'cd /synthetic && gh pr create --body synthetic'",
            pr,
            1,
        ),
        (
            "git add file && git -c user.name=Example commit -m synthetic",
            commit,
            1,
        ),
        (
            "git commit -m synthetic",
            "[topic (root-commit) abc1234] synthetic",
            1,
        ),
        (
            "git add file && git commit -m synthetic && git push",
            "[topic abc1234] synthetic\nTo https://github.com/example/project.git\n abc1234..def5678 topic -> topic",
            2,
        ),
        ("cat file && gh pr create", pr, 0),
        ("printf decoy; gh pr create", pr, 0),
        ("gh pr create; gh pr create", pr, 0),
        ("bash -lc 'gh pr create; echo decoy'", pr, 0),
        ("bash -lc 'gh pr create $(unknown)'", pr, 0),
        ("gh pr create > file", pr, 0),
    ];
    for (command, output, count) in cases {
        for name in ["Bash", "exec_command", "functions.exec_command"] {
            let observation =
                observed(name, command, json!({"exit_code":0,"output":output}), false);
            assert_eq!(observation.changes.len(), count, "{name}: {command}");
        }
    }
}

#[test]
fn native_and_shell_read_selection_coverage_table() {
    let cases = [
        (
            "Read",
            json!({"file_path":"/synthetic/file","offset":1,"limit":5}),
            json!("body"),
        ),
        (
            "functions.Read",
            json!({"file_path":"/synthetic/file","pages":"1-2"}),
            json!([{"type":"text","text":"body"}]),
        ),
        (
            "Bash",
            json!({"command":"sed -n '1,5p' file"}),
            json!("body"),
        ),
        ("Bash", json!({"command":"cat file"}), json!("body")),
        (
            "exec_command",
            json!({"cmd":"bash -lc 'cat file'"}),
            json!({"exit_code":0,"output":"body"}),
        ),
        (
            "exec_command",
            json!({"cmd":"cd /synthetic && sed -n '1,5p' file"}),
            json!({"exit_code":0,"output":"body"}),
        ),
        (
            "exec_command",
            json!({"cmd":"rg --files src"}),
            json!({"exit_code":0,"output":"src/file"}),
        ),
        (
            "exec_command",
            json!({"cmd":"rg -n needle file"}),
            json!({"exit_code":0,"output":"1:needle"}),
        ),
        (
            "exec_command",
            json!({"cmd":"bash -lc 'rg -n needle file'"}),
            json!({"exit_code":0,"output":"1:needle"}),
        ),
        (
            "Grep",
            json!({"pattern":"needle","path":"/synthetic"}),
            json!("file:needle"),
        ),
    ];
    for (name, input, output) in cases {
        let mut rows = Vec::new();
        for offset in [10, 30] {
            let mut observation = call(name, &input, None, "hash".into());
            observation.envelopes = name.contains("exec_command");
            finish(&mut observation, &output, false, None, None, "hash".into());
            rows.push(located(offset, observation));
        }
        classify_reads(&mut rows, &[]);
        assert_eq!(
            rows[1].observation.waste,
            Some(WasteKind::RedundantRead),
            "{name} {input}"
        );
        rows[1].observation.waste = None;
        classify_reads(&mut rows, &[("synthetic".into(), 20)]);
        assert_eq!(
            rows[1].observation.waste,
            Some(WasteKind::Refill),
            "{name} {input}"
        );
    }
    for (name, command, output) in [
        (
            "exec_command",
            "rg needle file",
            json!({"exit_code":1,"output":""}),
        ),
        (
            "exec_command",
            "bash -lc 'rg needle file'",
            json!({"metadata":{"exit_code":1},"output":""}),
        ),
        (
            "exec_command",
            "cd /synthetic && grep needle file",
            json!({"exit_code":1,"output":""}),
        ),
        (
            "exec_command",
            "rg needle file",
            json!("Wall time: 0.1000 seconds\nProcess exited with code 1\nOutput:\n"),
        ),
    ] {
        let observation = observed(name, command, output, false);
        assert!(observation.empty_search, "{command}");
        assert!(observation.waste.is_none());
    }
    for (name, text) in [("Grep", "No matches found"), ("Glob", "No files found")] {
        let mut observation = call(name, &json!({"pattern":"needle"}), None, "hash".into());
        finish(
            &mut observation,
            &json!(text),
            false,
            None,
            None,
            "hash".into(),
        );
        assert!(observation.empty_search);
    }
}

#[test]
fn request_blobs_do_not_duplicate_tools_or_break_exact_claude_ownership() {
    for reverse in [false, true] {
        let mut index = FileIndex::default();
        let call = json!({"type":"tool_use","id":"call","name":"Bash","input":{"command":"cd /synthetic && gh pr create --title synthetic"}});
        let assistant = json!({"type":"assistant","message":{"id":"message","role":"assistant","content":[call.clone()]}});
        let blob =
            json!({"type":"api-request-blob","message":{"role":"assistant","content":[call]}});
        let mut records = if reverse {
            vec![blob, assistant]
        } else {
            vec![assistant, blob]
        };
        records.push(json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":"https://github.com/example/project/pull/42"}]}}));
        for (n, record) in records.iter().enumerate() {
            crate::events::claude(&mut index, record, n as u64 * 100);
            capture(
                &mut index,
                "claude",
                record,
                n as u64 * 100,
                &serde_json::to_vec(record).unwrap(),
            );
        }
        let events: Vec<_> = index
            .events
            .iter()
            .filter(|event| event.k == Kind::Tool)
            .collect();
        assert_eq!(events.len(), 1);
        let observation = events[0].activity_observation.as_ref().unwrap();
        assert_eq!(
            observation.step_id,
            Some(ActivityStepId::Claude("message".into()))
        );
        assert!(observation.completed);
        assert_eq!(observation.changes.len(), 1);
    }
}

#[test]
fn codex_capture_requires_complete_explicit_identity_and_preserves_script_workdir() {
    for (identity, expected) in [
        (json!({"turn_id":"turn"}), None),
        (
            json!({"thread_id":"thread","session_id":"session","turn_id":"turn","response_id":"response"}),
            Some(ActivityStepId::Codex([
                "thread".into(),
                "session".into(),
                "turn".into(),
                "response".into(),
            ])),
        ),
    ] {
        let mut index = FileIndex::default();
        let mut payload = json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"text(await tools.exec_command({cmd:'cat file',workdir:'/synthetic'}))"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(identity.as_object().unwrap().clone());
        let records = [
            json!({"type":"response_item","payload":payload}),
            json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":"{\"exit_code\":0,\"output\":\"body\"}"}]}}),
        ];
        for (n, record) in records.iter().enumerate() {
            crate::events::codex(&mut index, record, n as u64 * 100);
            capture(
                &mut index,
                "codex",
                record,
                n as u64 * 100,
                &serde_json::to_vec(record).unwrap(),
            );
        }
        let observation = index.events[0].activity_observation.as_ref().unwrap();
        assert_eq!(observation.step_id, expected);
        assert!(observation.completed);
        let mut other = call(
            "exec_command",
            &json!({"cmd":"cat file","workdir":"/other"}),
            None,
            "hash".into(),
        );
        bind_source_context(&mut other, None);
        assert_ne!(observation.commands, other.commands);
        assert!(
            !serde_json::to_string(observation)
                .unwrap()
                .contains("/synthetic")
        );
    }
}

#[test]
fn repeated_or_conflicting_native_call_ids_still_refuse_attribution() {
    for name in ["Bash", "Read"] {
        let mut index = FileIndex::default();
        let input = if name == "Bash" {
            json!({"command":"git commit -m synthetic"})
        } else {
            json!({"file_path":"file"})
        };
        let records = [
            json!({"type":"assistant","message":{"id":"first","content":[{"type":"tool_use","id":"repeated","name":name,"input":input}]}}),
            json!({"type":"assistant","message":{"id":"second","content":[{"type":"tool_use","id":"repeated","name":name,"input":input}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"repeated","content":"[topic abc1234] synthetic"}]}}),
        ];
        for (n, record) in records.iter().enumerate() {
            crate::events::claude(&mut index, record, n as u64 * 100);
            capture(
                &mut index,
                "claude",
                record,
                n as u64 * 100,
                &serde_json::to_vec(record).unwrap(),
            );
        }
        assert!(
            index
                .events
                .iter()
                .filter_map(|event| event.activity_observation.as_ref())
                .all(|observation| !observation.completed && observation.changes.is_empty())
        );
    }
}

#[test]
fn terminal_failed_mutations_end_overlap_barriers_without_preserving_old_reads() {
    for (output, error) in [
        (json!({"exit_code":1,"output":"failure"}), false),
        (json!("failure"), true),
    ] {
        let mut mutation = call(
            "exec_command",
            &json!({"cmd":"unknown-command"}),
            None,
            "hash".into(),
        );
        mutation.envelopes = true;
        finish(&mut mutation, &output, error, None, None, "hash".into());
        assert!(mutation.settled);
        assert!(!mutation.completed);
        let mut rows = vec![
            located(10, observed("Bash", "cat file", json!("body"), false)),
            located(20, mutation),
            located(30, observed("Bash", "cat file", json!("body"), false)),
            located(40, observed("Bash", "cat file", json!("body"), false)),
        ];
        classify_reads(&mut rows, &[]);
        assert!(
            rows[2].observation.waste.is_none(),
            "failed mutation invalidates the earlier read"
        );
        assert_eq!(rows[3].observation.waste, Some(WasteKind::RedundantRead));
    }
}
