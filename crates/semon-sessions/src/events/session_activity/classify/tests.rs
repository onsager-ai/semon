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

#[test]
fn unmatched_orchestration_result_keeps_indexed_call_and_reply_times() {
    let pending = || call("wait_agent", &json!({}), None, "call-digest".into());
    let mut observation = pending();
    observation.fill_wait_from_reply(Some(1_000), Some(4_000));
    assert_eq!(
        observation.wait,
        Some(WaitInterval {
            started_at_ms: 1_000,
            completed_at_ms: 4_000,
        })
    );
    assert_eq!(observation.waste, Some(WasteKind::OrchestrationWait));
    for (start, end) in [
        (Some(1_000), None),
        (None, Some(4_000)),
        (Some(4_000), Some(1_000)),
    ] {
        let mut observation = pending();
        observation.fill_wait_from_reply(start, end);
        assert_eq!(observation.wait, None);
    }
    // A decoded interval is never replaced, and non-waits never gain one.
    let mut observation = pending();
    observation.wait = Some(WaitInterval {
        started_at_ms: 10,
        completed_at_ms: 20,
    });
    observation.fill_wait_from_reply(Some(1_000), Some(4_000));
    assert_eq!(observation.wait.as_ref().unwrap().started_at_ms, 10);
    let mut observation = call("Read", &json!({"file_path": "/f"}), None, "d".into());
    observation.fill_wait_from_reply(Some(1_000), Some(4_000));
    assert_eq!(observation.wait, None);
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
    assert!(crate::events::activity_wait_source(
        "console.log(await tools.wait_agent({ids:['synthetic']}))"
    ));
    for script in [
        "console.log({type:'CollabAgentToolCall',tool:'wait'})",
        "const result = await tools.wait_agent({ids:['synthetic']}); text({type:'CollabAgentToolCall',tool:'wait'});",
        "console.log(await tools.wait_agent({ids:selectIds()}))",
        "console.log(await tools.wait_agent({ids:['synthetic']})); console.log('extra')",
    ] {
        assert!(!crate::events::activity_wait_source(script), "{script}");
    }
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

#[test]
fn native_output_projection_and_literal_pr_bodies_table() {
    let pr = "warning: synthetic\nCreating pull request for topic into main\nhttps://github.com/example/project/pull/42\n";
    for command in [
        "gh pr create --base main --title synthetic --body-file /synthetic/body",
        "gh --repo example/project pr create --base main --title synthetic --body-file /synthetic/body; gh pr checks --repo example/project",
        "cd /synthetic && gh pr create --base main --title synthetic --body-file /synthetic/body",
        "gh pr create --title synthetic --body-file - <<'BODY'\nLiteral $value and `text`\nBODY\n",
        "cd /synthetic\ngh pr create --title synthetic --body-file - <<'BODY'\nLiteral body\nBODY\n",
        "gh pr create --title synthetic --body \"$(cat <<'BODY'\nLiteral $value\nBODY\n)\"",
        "cat > /synthetic/body <<'BODY'\nLiteral body\nBODY\ngh pr create --title synthetic --body-file /synthetic/body",
    ] {
        for projected in [false, true] {
            let mut index = FileIndex::default();
            let script = format!(
                "const result = await tools.exec_command({{cmd:{},workdir:'/synthetic'}}); text(result.output);",
                serde_json::to_string(command).unwrap()
            );
            let records = if projected {
                vec![
                    json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"call","name":"exec","input":script}}),
                    json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":pr}]}}),
                ]
            } else {
                vec![
                    json!({"type":"assistant","message":{"id":"message","content":[{"type":"tool_use","id":"call","name":"Bash","input":{"command":command}}]}}),
                    json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call","content":pr}]}}),
                ]
            };
            for (n, record) in records.iter().enumerate() {
                if projected {
                    crate::events::codex(&mut index, record, n as u64 * 100);
                } else {
                    crate::events::claude(&mut index, record, n as u64 * 100);
                }
                capture(
                    &mut index,
                    if projected { "codex" } else { "claude" },
                    record,
                    n as u64 * 100,
                    &serde_json::to_vec(record).unwrap(),
                );
            }
            let observation = index
                .events
                .iter()
                .find_map(|event| event.activity_observation.as_ref())
                .unwrap();
            assert_eq!(
                observation.changes.len(),
                1,
                "projected={projected} {command}"
            );
            assert!(observation.completed);
        }
    }
    for script in [
        "const result = await tools.exec_command({cmd:'cat file'}); text(result.output);",
        "let result = await tools.exec_command({cmd:'cat file'}); text(result.output);",
    ] {
        assert!(crate::events::activity_output_source(script));
        assert!(!crate::events::result_source(script));
    }
    for script in [
        "const result = await tools.exec_command({cmd:'cat file'}); text(other.output);",
        "const result = await tools.exec_command({cmd:'cat file'}); result.output='forged'; text(result.output);",
        "const result = await tools.exec_command({cmd:'cat file'}); text(result.output || 'forged');",
        "const result = await tools.exec_command({cmd:'cat file'}); text(result.output); text('forged');",
    ] {
        assert!(!crate::events::activity_output_source(script), "{script}");
    }
}

#[test]
fn multiple_literal_exec_results_retain_failure_diagnostics_without_success_proof() {
    let script = "text(await tools.exec_command({cmd:'cat absent'})); text(await tools.exec_command({cmd:'cat present'}));";
    assert!(crate::events::activity_results_source(script));
    assert!(!crate::events::activity_results_source(
        "text({exit_code:1,output:'cat: absent: No such file or directory'});"
    ));
    for (message, expected) in [
        (
            "cat: absent: No such file or directory",
            WasteKind::PathNotFound,
        ),
        ("HTTP 503", WasteKind::ExternalFailure),
        ("syntax error", WasteKind::MalformedCommand),
        ("failure", WasteKind::OtherFailure),
    ] {
        let mut observation = call("exec_command", &Value::Null, None, "hash".into());
        observation.failure_results = true;
        finish(
            &mut observation,
            &json!([
                {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type":"input_text","text":json!({"exit_code":1,"output":message}).to_string()},
                {"type":"input_text","text":json!({"exit_code":0,"output":"body"}).to_string()}
            ]),
            false,
            None,
            None,
            "result".into(),
        );
        assert_eq!(observation.waste, Some(expected));
        assert!(observation.settled);
        assert!(!observation.completed);
        assert!(observation.changes.is_empty());
    }
}

#[test]
fn terminal_ambiguous_code_mode_does_not_suppress_all_later_reads() {
    let mut index = FileIndex::default();
    let records = [
        json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"parallel","name":"exec","input":"const results = await Promise.all([tools.exec_command({cmd:'cat one'}),tools.exec_command({cmd:'cat two'})]); for (const result of results) text(JSON.stringify({output:result.output,exit_code:result.exit_code}));"}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"parallel","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":"{\"index\":0,\"exit_code\":0,\"output\":\"one\"}"},{"type":"input_text","text":"{\"index\":1,\"exit_code\":0,\"output\":\"two\"}"}]}}),
    ];
    for (n, record) in records.iter().enumerate() {
        crate::events::codex(&mut index, record, n as u64 * 10);
        capture(
            &mut index,
            "codex",
            record,
            n as u64 * 10,
            &serde_json::to_vec(record).unwrap(),
        );
    }
    let observation = index.events[0].activity_observation.clone().unwrap();
    assert!(observation.settled);
    assert!(observation.changes.is_empty());
    let mut rows = vec![
        located(10, observation),
        located(30, observed("Bash", "cat file", json!("body"), false)),
        located(50, observed("Bash", "cat file", json!("body"), false)),
    ];
    classify_reads(&mut rows, &[]);
    assert!(rows[1].observation.waste.is_none());
    assert_eq!(rows[2].observation.waste, Some(WasteKind::RedundantRead));
}

#[test]
fn code_mode_printed_output_repeats_native_shell_reads() {
    for command in [
        "cat file",
        "sed -n '1,20p' file",
        "nl -ba file | sed -n '1,20p'",
        "bash -lc 'rg needle file'",
    ] {
        let mut index = FileIndex::default();
        for n in 0..2 {
            let script = format!(
                "const result = await tools.exec_command({{cmd:{},workdir:'/synthetic'}}); text(result.output);",
                serde_json::to_string(command).unwrap()
            );
            let records = [
                json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":format!("call-{n}"),"name":"exec","input":script}}),
                json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":format!("call-{n}"),"output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":"synthetic output"}]}}),
            ];
            for (at, record) in records.iter().enumerate() {
                let offset = (n * 200 + at * 100) as u64;
                crate::events::codex(&mut index, record, offset);
                capture(
                    &mut index,
                    "codex",
                    record,
                    offset,
                    &serde_json::to_vec(record).unwrap(),
                );
            }
        }
        let mut rows = index
            .events
            .iter()
            .filter_map(|event| {
                event
                    .activity_observation
                    .clone()
                    .map(|observation| LocatedObservation {
                        position: super::SourcePosition {
                            source: "synthetic".into(),
                            offset: event.o,
                        },
                        timestamp: event.t,
                        result_offset: event.r.as_ref().map(|reply| reply.o),
                        observation,
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        classify_reads(&mut rows, &[]);
        assert_eq!(
            rows[1].observation.waste,
            Some(WasteKind::RedundantRead),
            "{command}"
        );
    }
}

#[test]
fn selected_text_pipelines_are_reads_but_arbitrary_filters_remain_unknown() {
    for command in [
        "nl -ba file | sed -n '1,20p'",
        "cat file | head -n 20",
        "rg needle file | head -n 20",
    ] {
        let mut rows = vec![
            located(10, observed("Bash", command, json!("body"), false)),
            located(30, observed("Bash", command, json!("body"), false)),
        ];
        classify_reads(&mut rows, &[]);
        assert_eq!(
            rows[1].observation.waste,
            Some(WasteKind::RedundantRead),
            "{command}"
        );
    }
    for command in [
        "cat file | sh",
        "cat file | head; touch file",
        "gh pr create | cat",
        "cat file <<EOF\nbody\nEOF",
    ] {
        let observation = observed("Bash", command, json!("body"), false);
        assert!(
            observation
                .commands
                .iter()
                .any(|command| matches!(command, Command::Unknown | Command::Ambiguous)),
            "{command}"
        );
    }
}

#[test]
fn directory_changes_cannot_reuse_bare_or_relative_read_selections() {
    for commands in [
        ["cat file", "cd /other", "cat file"],
        ["cat file", "cd /other && cat file", "cat file"],
        [
            "cd relative && cat file",
            "git status",
            "cd relative && cat file",
        ],
    ] {
        let mut rows: Vec<_> = commands
            .iter()
            .enumerate()
            .map(|(n, command)| {
                located(
                    10 + n as u64 * 10,
                    observed("Bash", command, json!("body"), false),
                )
            })
            .collect();
        classify_reads(&mut rows, &[]);
        assert!(
            rows.last().unwrap().observation.waste.is_none(),
            "{commands:?}"
        );
    }
}

fn claude_result(name: &str, command: &str, content: Value, error: bool) -> Observation {
    let mut observation = call(
        name,
        &json!({"command":command, "file_path":"/synthetic/file"}),
        None,
        "call-digest".into(),
    );
    observation.claude_bash = name == "Bash";
    finish(
        &mut observation,
        &content,
        error,
        None,
        None,
        "result-digest".into(),
    );
    observation
}

#[test]
fn claude_bash_exit_header_drives_typed_failures_and_verify_signals() {
    // (tool, command, content, is_error, waste, verify_failure)
    let cases = [
        // gh pr checks: 8 is pending, 1 is a verification outcome.
        (
            "Bash",
            "gh pr checks 12",
            "Exit code 8\npending",
            true,
            Some(WasteKind::ChecksPending),
            false,
        ),
        (
            "Bash",
            "gh pr checks 12",
            "Exit code 1\nfailing",
            true,
            None,
            true,
        ),
        // Only the header counts: later or bare body lines never set the exit.
        (
            "Bash",
            "gh pr checks 12",
            "Exit code 1\nExit code 8",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "gh pr checks 12",
            "Exit code 1\nExit code 0\nbody",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "gh pr checks 12",
            "boom\nExit code 8",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "gh pr checks 12",
            "Exit code: 8\nboom",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        // A successful call's output cannot forge a header.
        (
            "Bash",
            "cargo test",
            "Exit code 1\nprinted by the program",
            false,
            None,
            false,
        ),
        // Timeouts and termination.
        (
            "Bash",
            "timeout 5 tool",
            "Exit code 124\n",
            true,
            Some(WasteKind::Timeout),
            false,
        ),
        (
            "Bash",
            "tool",
            "Exit code 143\n",
            true,
            Some(WasteKind::Timeout),
            false,
        ),
        (
            "Bash",
            "cargo test",
            "Exit code 143\n",
            true,
            Some(WasteKind::Timeout),
            false,
        ),
        // Verification failures are signals, not waste.
        (
            "Bash",
            "cargo test --locked",
            "Exit code 101\nfailures",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "cargo clippy --all-targets",
            "Exit code 101\nwarning",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "cargo fmt --all --check",
            "Exit code 1\nDiff",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "cd /synthetic && cargo build 2>&1",
            "Exit code 101\nerror",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "npm --prefix ui run typecheck",
            "Exit code 2\nerror",
            true,
            None,
            true,
        ),
        ("Bash", "npm test", "Exit code 1\nfail", true, None, true),
        ("Bash", "pytest -q", "Exit code 1\nfail", true, None, true),
        (
            "Bash",
            "go test ./...",
            "Exit code 1\nfail",
            true,
            None,
            true,
        ),
        (
            "Bash",
            "gh run view 5 --log-failed",
            "Exit code 1\nlog",
            true,
            None,
            true,
        ),
        // Not verification: cargo fmt without --check, a missing tool, an
        // unrecognized wrapper, a compound with other work, a failed cd.
        (
            "Bash",
            "cargo fmt --all",
            "Exit code 1\nerror",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "cargo test",
            "Exit code 127\ncommand not found",
            true,
            Some(WasteKind::MalformedCommand),
            false,
        ),
        (
            "Bash",
            "git add . && cargo test",
            "Exit code 1\nerror",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "cd /missing && cargo test",
            "Exit code 1\ncd: /missing: No such file or directory",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        // A pipeline's status belongs to its last stage.
        (
            "Bash",
            "cargo test 2>&1 | tail -5",
            "ok",
            false,
            None,
            false,
        ),
        (
            "Bash",
            "cargo test 2>&1 | grep ok",
            "Exit code 1\n",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        // Harmless redirects keep single-command attribution.
        (
            "Bash",
            "cat absent 2>&1",
            "Exit code 1\ncat: absent: No such file or directory",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Bash",
            "cat absent 2> /dev/null",
            "Exit code 1\ncat: absent: No such file or directory",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        // Compound diagnostics: one allowlisted program that appears once.
        (
            "Bash",
            "cat a | grep b",
            "Exit code 1\ncat: a: No such file or directory",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Bash",
            "ls dir && git status",
            "Exit code 2\nls: cannot access 'dir': No such file or directory",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Bash",
            "find . -name x | head -3",
            "Exit code 1\nfind: 'dir': No such file or directory",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Bash",
            "cat a; ls b",
            "Exit code 1\ncat: a: No such file or directory\nls: b: No such file or directory",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "cat a | cat b",
            "Exit code 1\ncat: b: No such file or directory",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "tool | cat",
            "Exit code 1\ntool: No such file or directory",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        // Merge conflicts need a failing git integration command.
        (
            "Bash",
            "git merge topic",
            "Exit code 1\nCONFLICT (content): Merge conflict in f",
            true,
            Some(WasteKind::MergeConflict),
            false,
        ),
        (
            "Bash",
            "git fetch && git rebase origin/main",
            "Exit code 1\nCONFLICT (content): Merge conflict in f",
            true,
            Some(WasteKind::MergeConflict),
            false,
        ),
        (
            "Bash",
            "git cherry-pick abc",
            "Exit code 1\nCONFLICT (modify/delete): f",
            true,
            Some(WasteKind::MergeConflict),
            false,
        ),
        (
            "Bash",
            "git pull",
            "Exit code 1\nCONFLICT (content): f",
            true,
            Some(WasteKind::MergeConflict),
            false,
        ),
        (
            "Bash",
            "git merge topic",
            "Exit code 1\nerror: dirty tree",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "git merge topic",
            "Already up to date.\nCONFLICT (content): printed only",
            false,
            None,
            false,
        ),
        (
            "Bash",
            "git status",
            "Exit code 1\nCONFLICT (content): f",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        // Claude's native tool wrapper.
        (
            "Edit",
            "",
            "<tool_use_error>String to replace not found</tool_use_error>",
            true,
            Some(WasteKind::ToolInputRejected),
            false,
        ),
        (
            "Write",
            "",
            "<tool_use_error>File has not been read yet</tool_use_error>",
            true,
            Some(WasteKind::ToolInputRejected),
            false,
        ),
        (
            "MultiEdit",
            "",
            "<tool_use_error>bad edit</tool_use_error>",
            true,
            Some(WasteKind::ToolInputRejected),
            false,
        ),
        (
            "NotebookEdit",
            "",
            "<tool_use_error>bad cell</tool_use_error>",
            true,
            Some(WasteKind::ToolInputRejected),
            false,
        ),
        (
            "Read",
            "",
            "<tool_use_error>File does not exist.</tool_use_error>",
            true,
            Some(WasteKind::PathNotFound),
            false,
        ),
        (
            "Read",
            "",
            "<tool_use_error>File too large</tool_use_error>",
            true,
            Some(WasteKind::ToolInputRejected),
            false,
        ),
        (
            "Edit",
            "",
            "<tool_use_error>File does not exist.</tool_use_error>",
            true,
            Some(WasteKind::ToolInputRejected),
            false,
        ),
        (
            "Read",
            "",
            "plain failure",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
        (
            "Bash",
            "tool",
            "<tool_use_error>nope</tool_use_error>",
            true,
            Some(WasteKind::OtherFailure),
            false,
        ),
    ];
    for (name, command, content, error, waste, verify) in cases {
        let observation = claude_result(name, command, json!(content), error);
        assert_eq!(observation.waste, waste, "{name} {command} {content}");
        assert_eq!(
            observation.verify_failure, verify,
            "{name} {command} {content}"
        );
        assert!(!observation.silent_exit);
    }
    // Codex's colon form is not Claude's header, even for a Claude-named tool.
    let observation = claude_result("Bash", "gh pr checks 1", json!("Exit code: 8"), true);
    assert_eq!(observation.waste, Some(WasteKind::OtherFailure));
    // Content blocks carry the same header.
    let observation = claude_result(
        "Bash",
        "gh pr checks 1",
        json!([{"type":"text","text":"Exit code 8\npending"}]),
        true,
    );
    assert_eq!(observation.waste, Some(WasteKind::ChecksPending));
    // The header is Claude-only: another harness's Bash-shaped tool is unchanged.
    let mut other = call(
        "bash",
        &json!({"command":"gh pr checks 1"}),
        None,
        "call".into(),
    );
    finish(
        &mut other,
        &json!("Exit code 8\npending"),
        true,
        None,
        None,
        "result".into(),
    );
    assert_eq!(other.waste, Some(WasteKind::OtherFailure));
}

#[test]
fn new_waste_kinds_have_their_categories() {
    assert_eq!(
        WasteKind::ChecksPending.category(),
        WasteCategory::Orchestration
    );
    assert_eq!(WasteKind::Timeout.category(), WasteCategory::Tooling);
    assert_eq!(
        WasteKind::ToolInputRejected.category(),
        WasteCategory::Tooling
    );
    assert_eq!(WasteKind::MergeConflict.category(), WasteCategory::Unmapped);
}

#[test]
fn general_pipelines_and_redirects_stay_unproven_reads() {
    // A redirect to a real file writes it: unattributed, never harmless.
    for command in [
        "echo hi > out.txt",
        "cat a >> out.txt",
        "cat a 2>err.txt",
        "cat a >/dev/nullx",
        "cat a > /dev/null extra > out.txt",
        "cat a >&2",
        "cat a &",
        "cat a |& tee out",
        "cat a | ",
        "| cat a",
        "cat < a",
        "cat a 2>",
    ] {
        let observation = claude_result("Bash", command, json!(""), false);
        assert_eq!(observation.commands, [Command::Ambiguous], "{command}");
    }
    // Harmless redirects and general pipelines supply failure evidence only.
    for command in [
        "cat a 2>&1",
        "cat a >/dev/null",
        "cat a 2>/dev/null",
        "cat a &>/dev/null",
        "cat a 1>&2",
        "cat a > /dev/null",
        "cat a | tr a b | wc -l",
        "cat a | sort",
    ] {
        let observation = claude_result("Bash", command, json!(""), false);
        assert_eq!(observation.commands, [Command::Unknown], "{command}");
    }
    // A quoted operator is a plain argument.
    let observation = claude_result("Bash", "rg '>' file", json!(""), false);
    assert!(matches!(observation.commands[..], [Command::Search(_, _)]));
    // Expansions, subshells and heredocs remain unattributed.
    for command in ["cat $(x) | head", "(cat a) | head", "cat a | { head; }"] {
        let observation = claude_result("Bash", command, json!(""), false);
        assert_eq!(observation.commands, [Command::Ambiguous], "{command}");
    }
    // The bounded read pipeline still proves a repeated read.
    let mut rows = vec![
        located(
            10,
            observed("Bash", "cat file | head -n 20", json!("a"), false),
        ),
        located(
            30,
            observed("Bash", "cat file | head -n 20", json!("a"), false),
        ),
        located(50, observed("Bash", "cat file 2>&1", json!("a"), false)),
        located(70, observed("Bash", "cat file 2>&1", json!("a"), false)),
    ];
    classify_reads(&mut rows, &[]);
    assert_eq!(rows[1].observation.waste, Some(WasteKind::RedundantRead));
    // An unproven redirect or pipeline breaks read history instead.
    assert!(rows[2].observation.waste.is_none());
    assert!(rows[3].observation.waste.is_none());
}

#[test]
fn unattributed_silent_exit_is_a_signal_not_waste() {
    let parts = |first: Value| {
        json!([
            {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
            {"type":"input_text","text":first.to_string()},
            {"type":"input_text","text":json!({"exit_code":0,"output":"body"}).to_string()}
        ])
    };
    let run = |command: Option<&str>, output: Value| {
        let mut observation = match command {
            Some(command) => call("exec_command", &json!({"cmd":command}), None, "hash".into()),
            None => call("exec_command", &Value::Null, None, "hash".into()),
        };
        observation.failure_results = true;
        finish(
            &mut observation,
            &output,
            false,
            None,
            None,
            "result".into(),
        );
        observation
    };
    let silent = run(None, parts(json!({"exit_code":1,"output":""})));
    assert!(silent.silent_exit);
    assert_eq!(silent.waste, None);
    // Output, another exit code, or an attributed command is not silent.
    let noisy = run(None, parts(json!({"exit_code":1,"output":"failure"})));
    assert!(!noisy.silent_exit);
    assert_eq!(noisy.waste, Some(WasteKind::OtherFailure));
    let other = run(None, parts(json!({"exit_code":2,"output":""})));
    assert!(!other.silent_exit);
    assert_eq!(other.waste, Some(WasteKind::OtherFailure));
    let attributed = run(Some("tool"), parts(json!({"exit_code":1,"output":""})));
    assert!(!attributed.silent_exit);
    assert_eq!(attributed.waste, Some(WasteKind::OtherFailure));
    // A silent part never hides a real failure in the same batch.
    let mixed = run(
        None,
        json!([
            {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
            {"type":"input_text","text":json!({"exit_code":1,"output":""}).to_string()},
            {"type":"input_text","text":json!({"exit_code":1,"output":"HTTP 503"}).to_string()}
        ]),
    );
    assert!(mixed.silent_exit);
    assert_eq!(mixed.waste, Some(WasteKind::ExternalFailure));
}

#[test]
fn activity_label_follows_first_matching_precedence() {
    let shell = |command: &str| observed("Bash", command, json!("ok"), false).activity();
    for (command, expected) in [
        ("sleep 5", Some(Activity::Wait)),
        ("gh run watch 123", Some(Activity::Wait)),
        ("git add .", Some(Activity::Integrate)),
        ("git commit -m synthetic", Some(Activity::Integrate)),
        ("git push origin topic", Some(Activity::Integrate)),
        ("git merge main", Some(Activity::Integrate)),
        ("gh pr create --title t --body b", Some(Activity::Integrate)),
        ("cargo test", Some(Activity::Verify)),
        ("cd /work && cargo test", Some(Activity::Verify)),
        ("gh pr checks 12", Some(Activity::Verify)),
        ("touch file", Some(Activity::Implement)),
        ("cat file", Some(Activity::Explore)),
        ("rg needle", Some(Activity::Explore)),
        ("git status", Some(Activity::Explore)),
        ("cd /work", Some(Activity::Setup)),
        ("unrecognized command", None),
        // The earlier rule in precedence order wins within one call.
        ("git add . && cargo test", Some(Activity::Integrate)),
        ("sleep 1 && git commit -m synthetic", Some(Activity::Wait)),
        ("cargo test && touch file", Some(Activity::Verify)),
        ("touch file && cat file", Some(Activity::Implement)),
        ("cd /work && cat file", Some(Activity::Explore)),
        ("cat file && unrecognized command", Some(Activity::Explore)),
        // Rules version 8: read-only and CI commands of the gh CLI.
        ("gh run view 123", Some(Activity::Verify)),
        ("gh run view 123 --log-failed", Some(Activity::Verify)),
        ("gh run list --limit 5", Some(Activity::Verify)),
        ("gh --repo example/project run watch 123", Some(Activity::Wait)),
        ("gh run rerun 123", Some(Activity::Verify)),
        ("gh workflow run rust.yml --ref topic", Some(Activity::Verify)),
        ("gh pr view 12", Some(Activity::Explore)),
        ("gh pr diff 12", Some(Activity::Explore)),
        ("gh pr list", Some(Activity::Explore)),
        ("gh pr status", Some(Activity::Explore)),
        ("gh issue view 12", Some(Activity::Explore)),
        ("gh issue list", Some(Activity::Explore)),
        ("gh api repos/example/project", None),
        ("gh workflow list", None),
    ] {
        assert_eq!(shell(command), expected, "{command}");
    }
    // Ambiguous shell text proves nothing.
    assert_eq!(shell("echo $(date)"), None);
    // Checks that are still pending are waiting; settled checks verify.
    let pending = claude_result(
        "Bash",
        "gh pr checks 12",
        json!("Exit code 8\npending"),
        true,
    );
    assert_eq!(pending.activity(), Some(Activity::Wait));
    let failing = claude_result("Bash", "gh pr checks 12", json!("Exit code 1\nfail"), true);
    assert_eq!(failing.activity(), Some(Activity::Verify));
    for (name, expected) in [
        ("Edit", Some(Activity::Implement)),
        ("Write", Some(Activity::Implement)),
        ("MultiEdit", Some(Activity::Implement)),
        ("NotebookEdit", Some(Activity::Implement)),
        ("Read", Some(Activity::Explore)),
        ("Grep", Some(Activity::Explore)),
        ("Glob", Some(Activity::Explore)),
        ("Agent", Some(Activity::Coordinate)),
        ("Task", Some(Activity::Coordinate)),
        ("SendMessage", Some(Activity::Coordinate)),
        ("spawn_agent", Some(Activity::Coordinate)),
        ("send_input", Some(Activity::Coordinate)),
        ("wait_agent", Some(Activity::Wait)),
        ("followup_task", Some(Activity::Coordinate)),
        ("list_agents", Some(Activity::Coordinate)),
        ("interrupt_agent", Some(Activity::Coordinate)),
        ("sleep", Some(Activity::Wait)),
        ("Monitor", Some(Activity::Wait)),
        ("WebFetch", Some(Activity::Explore)),
        ("WebSearch", Some(Activity::Explore)),
        ("web_search", Some(Activity::Explore)),
        ("_web_run", Some(Activity::Explore)),
        ("ToolSearch", Some(Activity::Explore)),
        ("Skill", Some(Activity::Explore)),
        ("AskUserQuestion", Some(Activity::Report)),
        ("SubagentHandback", Some(Activity::Report)),
        ("UnlistedTool", None),
    ] {
        let input = json!({"file_path":"/f","pattern":"p"});
        assert_eq!(
            call(name, &input, None, "d".into()).activity(),
            expected,
            "{name}"
        );
    }
    // Waiting on an agent is not coordination, though both are orchestration.
    let waited = call("wait_agent", &Value::Null, None, "d".into());
    let spawned = call("Agent", &Value::Null, None, "d".into());
    assert!(waited.orchestration && spawned.orchestration);
    assert_eq!(waited.agent, Some(AgentCall::Wait));
    assert_eq!(spawned.agent, Some(AgentCall::Coordinate));
}

fn repeat_rows(calls: Vec<Observation>) -> Vec<LocatedObservation> {
    let mut rows: Vec<_> = calls
        .into_iter()
        .enumerate()
        .map(|(at, observation)| located(10 + 20 * at as u64, observation))
        .collect();
    classify_retries(&mut rows);
    rows
}

fn failing_test() -> Observation {
    claude_result("Bash", "cargo test", json!("Exit code 101\nfailed"), true)
}

fn passing_test() -> Observation {
    claude_result("Bash", "cargo test", json!("ok"), false)
}

fn unknown_failure() -> Observation {
    claude_result("Bash", "curl synthetic", json!("Exit code 2\nboom"), true)
}

fn unknown_success() -> Observation {
    claude_result("Bash", "curl synthetic", json!("fine"), false)
}

#[test]
fn flaky_rerun_needs_a_failed_verification_that_now_passes() {
    let rows = repeat_rows(vec![failing_test(), passing_test()]);
    assert!(rows[0].observation.verify_failure);
    assert_eq!(rows[0].observation.waste, None);
    assert_eq!(rows[1].observation.waste, Some(WasteKind::FlakyRerun));
    assert_eq!(WasteKind::FlakyRerun.category(), WasteCategory::External);
    // Read-only calls and sleeps between the two do not change state.
    let rows = repeat_rows(vec![
        failing_test(),
        claude_result("Bash", "cat file", json!("body"), false),
        claude_result("Bash", "sleep 30", json!("ok"), false),
        passing_test(),
    ]);
    assert_eq!(rows[3].observation.waste, Some(WasteKind::FlakyRerun));
    // A verification that fails again is a retry, not a flaky rerun.
    let rows = repeat_rows(vec![failing_test(), failing_test(), passing_test()]);
    assert_eq!(rows[1].observation.waste, Some(WasteKind::Retry));
    assert!(rows[1].observation.verify_failure);
    assert_eq!(rows[2].observation.waste, Some(WasteKind::FlakyRerun));
}

#[test]
fn retry_follows_any_failed_call_and_its_category_is_unmapped() {
    let rows = repeat_rows(vec![unknown_failure(), unknown_success()]);
    assert_eq!(rows[0].observation.waste, Some(WasteKind::OtherFailure));
    assert_eq!(rows[1].observation.waste, Some(WasteKind::Retry));
    assert_eq!(WasteKind::Retry.category(), WasteCategory::Unmapped);
    // A failed repeat gives way to the repeat marker.
    let rows = repeat_rows(vec![
        unknown_failure(),
        unknown_failure(),
        unknown_success(),
    ]);
    assert_eq!(rows[1].observation.waste, Some(WasteKind::Retry));
    assert_eq!(rows[2].observation.waste, Some(WasteKind::Retry));
    // A repeat of a call that succeeded is not a retry.
    let rows = repeat_rows(vec![unknown_success(), unknown_success()]);
    assert!(rows.iter().all(|row| row.observation.waste.is_none()));
    // A different command or a different source is not a repeat.
    let rows = repeat_rows(vec![
        unknown_failure(),
        claude_result("Bash", "curl other", json!("fine"), false),
    ]);
    assert!(rows[1].observation.waste.is_none());
    let mut rows = vec![
        located(10, unknown_failure()),
        located(30, unknown_success()),
    ];
    rows[1].position.source = "z-other".into();
    classify_retries(&mut rows);
    assert!(rows[1].observation.waste.is_none());
    // The repeat must start after the failed result is recorded.
    let mut rows = vec![
        located(10, unknown_failure()),
        located(15, unknown_success()),
    ];
    rows[0].result_offset = Some(20);
    classify_retries(&mut rows);
    assert!(rows[1].observation.waste.is_none());
}

#[test]
fn an_intervening_state_change_breaks_the_repeat_but_the_repeat_itself_does_not() {
    for (between, expected) in [
        ("touch file", None),
        ("cargo clippy", None),
        ("unrecognized command", None),
        ("git commit -m synthetic", None),
        ("git status", Some(WasteKind::FlakyRerun)),
        ("rg needle", Some(WasteKind::FlakyRerun)),
    ] {
        let rows = repeat_rows(vec![
            failing_test(),
            claude_result("Bash", between, json!("ok"), false),
            passing_test(),
        ]);
        assert_eq!(rows[2].observation.waste, expected, "{between}");
    }
    // The change must be between the calls: an earlier one does not matter.
    let rows = repeat_rows(vec![
        claude_result("Bash", "touch file", json!("ok"), false),
        failing_test(),
        passing_test(),
    ]);
    assert_eq!(rows[2].observation.waste, Some(WasteKind::FlakyRerun));
    // A native edit between the calls changes state too.
    let mut edit = call("Edit", &json!({"file_path":"/f"}), None, "d".into());
    finish(&mut edit, &json!("ok"), false, None, None, "r".into());
    let rows = repeat_rows(vec![failing_test(), edit, passing_test()]);
    assert_eq!(rows[2].observation.waste, None);
}

#[test]
fn tool_and_gh_labels_keep_state_change_semantics() {
    let tool = |name: &str| call(name, &json!({"query": "q"}), None, "d".into());
    // Read-only tools and calls are no longer barriers.
    for name in [
        "WebFetch",
        "WebSearch",
        "web_search",
        "_web_run",
        "ToolSearch",
        "Skill",
        "AskUserQuestion",
        "SubagentHandback",
        "sleep",
    ] {
        assert!(!tool(name).changes_state(), "{name}");
    }
    // Tools that can start processes or act on other agents stay barriers.
    for name in ["Monitor", "followup_task", "list_agents", "interrupt_agent"] {
        assert!(tool(name).changes_state(), "{name}");
    }
    for (command, barrier) in [
        ("gh run view 123", false),
        ("gh run list", false),
        ("gh pr view 12", false),
        ("gh issue list", false),
        ("gh run rerun 123", true),
        ("gh workflow run rust.yml", true),
    ] {
        let shell = call("Bash", &json!({"command": command}), None, "d".into());
        assert_eq!(shell.changes_state(), barrier, "{command}");
    }
    // A Codex sleep is the same Sleep waste as a shell sleep; Monitor is not.
    assert_eq!(observed("sleep", "", json!("ok"), false).waste, Some(WasteKind::Sleep));
    assert_eq!(observed("Monitor", "", json!("ok"), false).waste, None);
}

fn captured_exec(script: &str) -> Observation {
    let mut index = FileIndex::default();
    let records = [
        json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"call","name":"exec","input":script}}),
        json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":"printed"}]}}),
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
    index.events[0].activity_observation.clone().unwrap()
}

#[test]
fn code_mode_exec_literal_commands_label_without_proving() {
    let labelled = captured_exec(
        "await tools.exec_command({cmd:'cat file'}); await tools.exec_command({cmd:'cargo test'});",
    );
    assert_eq!(labelled.activity(), Some(Activity::Verify));
    assert_eq!(labelled.commands, [Command::Unknown]);
    assert!(labelled.changes_state());
    // The retry fingerprint still ignores the undecoded script.
    assert_eq!(labelled.fingerprint, None);
    // A non-literal command, or a session write, leaves the script unlabelled.
    for script in [
        "await tools.exec_command({cmd:name}); await tools.exec_command({cmd:'cat file'});",
        "await tools.exec_command({cmd:'cat file'}); await tools.exec_command({cmd:`cat ${x}`});",
        "await tools.exec_command({cmd:'cat file'}); await tools.write_stdin({session_id:1});",
        "for (const c of list) { await tools.exec_command({cmd:c}); }",
    ] {
        let unlabelled = captured_exec(script);
        assert_eq!(unlabelled.activity(), None, "{script}");
        assert!(unlabelled.label_commands.is_empty(), "{script}");
    }
    // Label evidence never feeds read proofs: a repeat of a read the script
    // printed is not redundant, and the script still clears earlier reads.
    let mut rows = vec![
        located(10, observed("Bash", "cat file", json!("ok"), false)),
        located(
            30,
            captured_exec(
                "await tools.exec_command({cmd:'cat file'}); await tools.exec_command({cmd:'ls'});",
            ),
        ),
        located(50, observed("Bash", "cat file", json!("ok"), false)),
    ];
    classify_reads(&mut rows, &[]);
    assert!(rows.iter().all(|row| row.observation.waste.is_none()));
}
