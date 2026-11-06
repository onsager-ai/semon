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
