use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use semon_store::{AuthoredBy, LogFilter, RepoSource, TraceStore};
use serde_json::{Value, json};

use super::*;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "semon-codex-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_session(root: &Path, records: &[Value]) -> PathBuf {
    let directory = root.join("sessions/2026/07");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("rollout-test.jsonl");
    let contents = records
        .iter()
        .map(|record| format!("{}\n", serde_json::to_string(record).unwrap()))
        .collect::<String>();
    fs::write(&path, contents).unwrap();
    path
}

fn options<'a>(state: &'a Path, history: &'a Path) -> ProcessOptions<'a> {
    ProcessOptions::new(state, history)
}

#[test]
fn state_path_resolution_uses_xdg_then_home() {
    assert_eq!(
        state_path_from(Some("/state-root".into()), PathBuf::from("/home/ignored")),
        PathBuf::from("/state-root/devlog/codex-tailer.json")
    );
    assert_eq!(
        state_path_from(None, PathBuf::from("/home/tester")),
        PathBuf::from("/home/tester/.local/state/devlog/codex-tailer.json")
    );
}

#[test]
fn load_state_returns_empty_state_when_file_is_absent() {
    let root = TestDir::new();

    let state = load_state(&root.path().join("missing.json")).unwrap();

    assert_eq!(state, CursorState::default());
}

/// An existing installation upgraded to this build still has its old
/// (version-1) cursor file on disk. That must reset to a fresh cursor
/// rather than error: the occurrence upsert on `(carrier, session,
/// sequence)` and content-addressed traces make a from-scratch replay safe,
/// so this is a one-time re-read cost, not a correctness risk.
#[test]
fn load_state_resets_to_a_fresh_cursor_for_an_older_version() {
    let root = TestDir::new();
    let path = root.path().join("state.json");
    fs::write(
        &path,
        br#"{"version":1,"files":{"some-file":{"offset":123}},"session_repos":{}}"#,
    )
    .unwrap();

    let state = load_state(&path).unwrap();

    assert_eq!(state, CursorState::default());
}

/// The asymmetric case: a version *newer* than this build understands must
/// still be a hard error, because a future build may have written shapes
/// this build cannot correctly interpret.
#[test]
fn load_state_errors_for_a_newer_version() {
    let root = TestDir::new();
    let path = root.path().join("state.json");
    fs::write(&path, br#"{"version":3,"files":{},"session_repos":{}}"#).unwrap();

    let error = load_state(&path).unwrap_err();

    assert!(matches!(error, AdapterError::State(_)));
}

#[test]
fn save_state_round_trips_through_load_state() {
    let root = TestDir::new();
    let path = root.path().join("nested/state.json");
    let state = CursorState::default();

    save_state(&path, &state).unwrap();

    assert_eq!(load_state(&path).unwrap(), state);
    assert!(path.is_file());
}

#[test]
fn normalizes_and_resumes_like_python_tailer() {
    let root = TestDir::new();
    let path = write_session(
        root.path(),
        &[
            json!({
                "timestamp": "2026-07-31T01:02:03Z",
                "type": "session_meta",
                "payload": {
                    "session_id": "session-1",
                    "cwd": "/work/devlog",
                    "git": {"repository_url": "git@github.com:onsager-ai/devlog.git"}
                }
            }),
            json!({
                "timestamp": "2026-07-31T01:02:04Z",
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "hello"}
            }),
            json!({
                "timestamp": "2026-07-31T01:02:05Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {"last_token_usage": {
                        "input_tokens": 10,
                        "output_tokens": 4,
                        "reasoning_output_tokens": 2
                    }}
                }
            }),
        ],
    );
    let history = root.path().join("history.jsonl");
    let state_path = root.path().join("state.json");
    let mut state = load_state(&state_path).unwrap();
    let mut store = TraceStore::open_in_memory().unwrap();

    assert_eq!(
        process_file(
            &path,
            &mut state,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap(),
        3
    );

    let mut reloaded = load_state(&state_path).unwrap();
    assert_eq!(
        process_file(
            &path,
            &mut reloaded,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap(),
        0
    );
    let traces = store.list_traces(None, 10).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0].semantic_core().value(),
        &json!({"kind": "intent", "content": "hello"})
    );
}

#[test]
fn normalization_retains_repo_and_token_archaeology() {
    let mut context = NormalizeContext::default();
    let meta = normalize_record(
        &json!({
            "timestamp": "2026-07-31T01:02:03Z",
            "type": "session_meta",
            "payload": {
                "session_id": "session-1",
                "cwd": "/work/devlog",
                "git": {"repository_url": "git@github.com:onsager-ai/devlog.git"}
            }
        }),
        &mut context,
        "",
    );
    assert_eq!(meta["kind"], "session_meta");

    let prompt = normalize_record(
        &json!({
            "timestamp": "2026-07-31T01:02:04Z",
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "hello"}
        }),
        &mut context,
        "",
    );
    assert_eq!(prompt["kind"], "user_prompt");
    assert_eq!(prompt["prompt"], "hello");

    let usage = normalize_record(
        &json!({
            "timestamp": "2026-07-31T01:02:05Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"last_token_usage": {
                "input_tokens": 10,
                "output_tokens": 4,
                "reasoning_output_tokens": 2
            }}}
        }),
        &mut context,
        "",
    );
    assert_eq!(usage["repo"], "devlog");
    assert_eq!(usage["tokens_in"], 10);
    assert_eq!(usage["tokens_out"], 6);
}

#[test]
fn partial_line_waits_for_completion_like_python_tailer() {
    let root = TestDir::new();
    let sessions = root.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let path = sessions.join("rollout-test.jsonl");
    let record = json!({
        "timestamp": "2026-07-31T01:02:03Z",
        "type": "event_msg",
        "payload": {"type": "user_message", "message": "hello"}
    });
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let history = root.path().join("history.jsonl");
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();

    assert_eq!(
        process_file(
            &path,
            &mut state,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap(),
        0
    );
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert_eq!(
        process_file(
            &path,
            &mut state,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap(),
        1
    );
}

#[test]
fn malformed_and_non_object_complete_lines_are_retained_raw() {
    let root = TestDir::new();
    let directory = root.path().join("sessions/2026/07");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("rollout-non-object.jsonl");
    let meta = json!({
        "timestamp": "2026-07-31T01:02:03Z",
        "type": "session_meta",
        "payload": {"session_id": "raw-line-session", "cwd": "/work/repo"}
    });
    let contents = format!(
        "{}\n42\n{{malformed\n",
        serde_json::to_string(&meta).unwrap()
    );
    fs::write(&path, &contents).unwrap();

    let history = root.path().join("history.jsonl");
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();

    assert_eq!(
        process_file(
            &path,
            &mut state,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap(),
        3
    );
    assert!(store.log(&LogFilter::default()).unwrap().is_empty());

    let raw = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "raw-line-session",
        ))
        .unwrap();
    assert_eq!(raw.len(), 3);
    let retained = raw
        .iter()
        .flat_map(|record| record.bytes().iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(retained, contents.as_bytes());
}

#[test]
fn checked_in_fixture_captures_canonical_and_raw_regions() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sessions/2026/07/rollout-e2e.jsonl");
    let root = TestDir::new();
    let history = root.path().join("history.jsonl");
    let mut store = TraceStore::open_in_memory().unwrap();

    for pass in 0..2 {
        let state_path = root.path().join(format!("state-{pass}.json"));
        let mut state = CursorState::default();
        assert_eq!(
            process_file(
                &fixture,
                &mut state,
                &mut store,
                &options(&state_path, &history),
            )
            .unwrap(),
            3
        );
    }

    let traces = store.list_traces(None, 10).unwrap();
    assert_eq!(traces.len(), 1);
    let trace = store.fetch_trace(traces[0].id()).unwrap().unwrap();
    assert_eq!(
        trace.semantic_core().value(),
        &json!({"kind": "intent", "content": "devlog end-to-end test"})
    );
    let canonical_text = trace.semantic_core().value().to_string();
    for incidental in [
        "codex",
        "event_msg",
        "session_meta",
        "devlog-e2e",
        "/workspace/devlog",
        "input_tokens",
    ] {
        assert!(!canonical_text.contains(incidental), "found {incidental}");
    }

    let raw = store.fetch_raw_carrier_records(trace.id()).unwrap();
    assert_eq!(raw.len(), 2);
    assert!(raw.iter().all(|record| record.carrier() == CARRIER));
    assert_eq!(raw[0].bytes(), raw[1].bytes());
    assert!(
        std::str::from_utf8(raw[0].bytes())
            .unwrap()
            .contains("user_message")
    );
}

#[test]
fn item_stream_projects_actions_and_excludes_harness_boilerplate() {
    let root = TestDir::new();
    let path = write_session(
        root.path(),
        &[
            json!({
                "timestamp": "2026-09-19T00:00:00Z",
                "type": "session_meta",
                "payload": {
                    "session_id": "item-stream-session",
                    "cwd": "/work/repo",
                    "git": {"repository_url": "git@github.com:onsager-ai/repo.git"}
                }
            }),
            // Harness-injected framing that Codex itself labels `developer`.
            json!({
                "timestamp": "2026-09-19T00:00:01Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "<skills_instructions>...</skills_instructions>"}]
                }
            }),
            // A harness injection that exists ONLY in the legacy mirror
            // (role `user`, no item-stream counterpart at all) — this is
            // the shape of the real `<recommended_plugins>` contamination.
            json!({
                "timestamp": "2026-09-19T00:00:02Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "<recommended_plugins>...</recommended_plugins>"}]
                }
            }),
            // The real user turn, mirrored in both the legacy channel and
            // the item stream, in that file order (as Codex emits it).
            json!({
                "timestamp": "2026-09-19T00:00:03Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "please fix the bug"}]
                }
            }),
            json!({
                "timestamp": "2026-09-19T00:00:04Z",
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "UserMessage",
                        "content": [{"type": "text", "text": "please fix the bug"}]
                    }
                }
            }),
            json!({
                "timestamp": "2026-09-19T00:00:05Z",
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "CommandExecution",
                        "command": ["/bin/zsh", "-lc", "sed -n '1,20p' /work/repo/src/main.rs"],
                        "cwd": "file:///work/repo",
                        "parsed_cmd": [
                            {"type": "read", "cmd": "sed -n '1,20p' src/main.rs", "name": "main.rs", "path": "src/main.rs"}
                        ],
                        "status": "completed",
                        "stdout": "fn main() {}\n",
                        "stderr": "",
                        "exit_code": 0
                    }
                }
            }),
            json!({
                "timestamp": "2026-09-19T00:00:06Z",
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "FileChange",
                        "changes": {
                            "/work/repo/src/main.rs": {"type": "update", "content": "fn main() { fixed(); }\n"},
                            "/tmp/scratch-note.txt": {"type": "add", "content": "scratch\n"}
                        },
                        "status": "completed",
                        "stdout": "",
                        "stderr": ""
                    }
                }
            }),
            json!({
                "timestamp": "2026-09-19T00:00:07Z",
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {"type": "Reasoning", "summary_text": [], "raw_content": []}
                }
            }),
            json!({
                "timestamp": "2026-09-19T00:00:08Z",
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "AgentMessage",
                        "content": [{"type": "Text", "text": "fixed it"}],
                        "phase": "commentary"
                    }
                }
            }),
            json!({
                "timestamp": "2026-09-19T00:00:09Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "fixed it"}]
                }
            }),
        ],
    );
    let history = root.path().join("history.jsonl");
    let state_path = root.path().join("state.json");
    let mut state = load_state(&state_path).unwrap();
    let mut store = TraceStore::open_in_memory().unwrap();

    let consumed = process_file(
        &path,
        &mut state,
        &mut store,
        &options(&state_path, &history),
    )
    .unwrap();
    assert_eq!(consumed, 10, "every complete record advances the cursor");

    // Re-running from the saved cursor consumes nothing further.
    let mut reloaded = load_state(&state_path).unwrap();
    assert_eq!(
        process_file(
            &path,
            &mut reloaded,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap(),
        0
    );

    // This synthetic rollout intentionally covers metadata, both legacy
    // message roles, each projected item kind, and a non-projecting
    // reasoning item. Every complete source line has exactly one raw row,
    // in source order, whether or not it produced a trace.
    let raw = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "item-stream-session",
        ))
        .unwrap();
    assert_eq!(raw.len(), consumed);
    let retained = raw
        .iter()
        .flat_map(|record| record.bytes().iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(retained, fs::read(&path).unwrap());

    let traces = store
        .list_traces(None, 10)
        .unwrap()
        .into_iter()
        .map(|trace| trace.semantic_core().value().clone())
        .collect::<Vec<_>>();

    assert_eq!(
        traces.len(),
        4,
        "expected exactly intent + outcome + 2 action traces, got {traces:?}"
    );

    // No harness boilerplate of any kind projected: not the `developer`-role
    // framing, and not the `user`-role injection that exists only in the
    // legacy mirror once this file is known to carry the item stream.
    for boilerplate in ["skills_instructions", "recommended_plugins"] {
        assert!(
            !traces
                .iter()
                .any(|trace| trace.to_string().contains(boilerplate)),
            "found {boilerplate} in a captured trace"
        );
    }

    // The real turn is captured exactly once (from the item stream), not
    // twice (item stream + legacy mirror).
    let intent_count = traces
        .iter()
        .filter(|trace| trace["kind"] == "intent" && trace["content"] == "please fix the bug")
        .count();
    assert_eq!(intent_count, 1);
    let outcome_count = traces
        .iter()
        .filter(|trace| trace["kind"] == "outcome" && trace["content"] == "fixed it")
        .count();
    assert_eq!(outcome_count, 1);

    // CommandExecution and FileChange both project `action` traces, with
    // paths under cwd rewritten relative and paths outside cwd redacted.
    assert!(traces.contains(&json!({
        "kind": "action",
        "action": "read",
        "path": "src/main.rs",
        "exit_code": 0,
    })));
    assert!(traces.contains(&json!({
        "kind": "action",
        "action": "file_change",
        "changes": [
            {"path": "src/main.rs", "change": "modify"},
            {"path": "<external>", "change": "add"},
        ],
    })));

    // No absolute machine path, raw command string, or file content ever
    // reached the semantic region.
    let canonical_text = traces
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for incidental in [
        "/work/repo",
        "/tmp/scratch-note.txt",
        "sed -n",
        "fn main",
        "scratch\\n",
    ] {
        assert!(
            !canonical_text.contains(incidental),
            "found {incidental} in captured traces"
        );
    }
}

#[test]
fn helper_variants_and_call_correlation_match_python_behavior() {
    assert_eq!(
        parse_timestamp(&json!("2026-07-31T01:02:03Z")),
        1_785_459_723_000_000_000
    );
    assert_eq!(
        parse_timestamp(&json!("2026-07-31T09:02:03+08:00")),
        1_785_459_723_000_000_000
    );
    assert_eq!(
        message_text(&json!(["one", {"text": "two"}, 3])),
        "one\ntwo"
    );
    assert_eq!(
        repo_from_url(&json!("git@github.com:onsager-ai/semon.git")),
        "semon"
    );
    assert_eq!(repo_from_cwd(&json!("/work/semon/")), "semon");
    assert!(!infer_success(&json!({"metadata": {"exitCode": 7}}), true));
    assert!(infer_success(&json!(r#"{"status":"completed"}"#), false));

    let mut context = NormalizeContext::default();
    let call = normalize_record(
        &json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "call_id": "call-1",
                "name": "shell",
                "namespace": "local",
                "arguments": {"command": "true"}
            }
        }),
        &mut context,
        "",
    );
    assert_eq!(call["tool"], "local.shell");
    let result = normalize_record(
        &json!({
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": {"exit_code": 0}
            }
        }),
        &mut context,
        "",
    );
    assert_eq!(result["tool"], "local.shell");
    assert_eq!(result["tool_input"], r#"{"command":"true"}"#);
    assert_eq!(result["tool_output"], r#"{"exit_code":0}"#);
}

fn trunc_session_records(include_second_turn: bool) -> Vec<Value> {
    let mut items = vec![
        json!({
            "timestamp": "2026-09-19T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "session_id": "trunc-session",
                "cwd": "/work/repo",
                "git": {"repository_url": "git@github.com:onsager-ai/repo.git"}
            }
        }),
        json!({
            "timestamp": "2026-09-19T00:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "item": {"type": "UserMessage", "content": [{"type": "text", "text": "first request"}]}
            }
        }),
        json!({
            "timestamp": "2026-09-19T00:00:02Z",
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "item": {"type": "AgentMessage", "content": [{"type": "Text", "text": "first reply"}]}
            }
        }),
    ];
    if include_second_turn {
        items.push(json!({
            "timestamp": "2026-09-19T00:00:03Z",
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "item": {"type": "UserMessage", "content": [{"type": "text", "text": "second request"}]}
            }
        }));
        items.push(json!({
            "timestamp": "2026-09-19T00:00:04Z",
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "item": {"type": "AgentMessage", "content": [{"type": "Text", "text": "second reply"}]}
            }
        }));
    }
    items
}

/// Test 2 from the occurrence-region spec: capture a file, truncate it, and
/// capture again. The occurrence rows for the surviving lines must be
/// identical *field for field* to a from-scratch capture of exactly that
/// truncated content — not merely unchanged in count, which is the failure
/// the `(carrier, session, sequence)` natural key exists to prevent.
#[test]
fn truncate_and_recapture_regenerates_identical_occurrence_fields() {
    let full_records = trunc_session_records(true);
    let truncated_records = trunc_session_records(false);

    // Store A: a full capture, then the file shrinks on disk and is
    // recaptured with the SAME (non-fresh) cursor state and store. This is
    // exactly the `saved.offset > size` reset path in `process_file`.
    let root_a = TestDir::new();
    let path_a = write_session(root_a.path(), &full_records);
    let history_a = root_a.path().join("history.jsonl");
    let state_path_a = root_a.path().join("state.json");
    let mut state_a = load_state(&state_path_a).unwrap();
    let mut store_a = TraceStore::open_in_memory().unwrap();
    assert_eq!(
        process_file(
            &path_a,
            &mut state_a,
            &mut store_a,
            &options(&state_path_a, &history_a),
        )
        .unwrap(),
        5
    );

    // Truncate: overwrite the same file with only its first three lines.
    write_session(root_a.path(), &truncated_records);
    assert_eq!(
        process_file(
            &path_a,
            &mut state_a,
            &mut store_a,
            &options(&state_path_a, &history_a),
        )
        .unwrap(),
        3,
        "truncation must reset the cursor and replay the surviving lines"
    );

    // Store B: an independent, from-scratch capture of exactly the
    // truncated content, for comparison.
    let root_b = TestDir::new();
    let path_b = write_session(root_b.path(), &truncated_records);
    let history_b = root_b.path().join("history.jsonl");
    let state_path_b = root_b.path().join("state.json");
    let mut state_b = load_state(&state_path_b).unwrap();
    let mut store_b = TraceStore::open_in_memory().unwrap();
    assert_eq!(
        process_file(
            &path_b,
            &mut state_b,
            &mut store_b,
            &options(&state_path_b, &history_b),
        )
        .unwrap(),
        3
    );

    let mut rows_a = store_a.log(&LogFilter::default()).unwrap();
    rows_a.retain(|row| row.session() == "trunc-session" && row.sequence() < 3);
    let rows_b = store_b.log(&LogFilter::default()).unwrap();

    assert_eq!(rows_b.len(), 2, "only the two item_completed lines project");
    assert_eq!(
        rows_a, rows_b,
        "occurrence rows must match field for field after truncate-and-recapture"
    );
    assert_eq!(rows_b[0].parent_sequence(), None);
    assert_eq!(rows_b[1].parent_sequence(), Some(rows_b[0].sequence()));
}

/// Test 3 from the occurrence-region spec: a second capture of the same
/// source position upserts rather than duplicating.
#[test]
fn recapturing_the_same_line_upserts_the_occurrence() {
    let root = TestDir::new();
    let path = write_session(root.path(), &trunc_session_records(false));
    let history = root.path().join("history.jsonl");
    let state_path = root.path().join("state.json");
    let mut store = TraceStore::open_in_memory().unwrap();

    // Two independent full passes over the same unmodified file, each with
    // its own fresh cursor state, so both fully replay from the start.
    for _ in 0..2 {
        let mut state = CursorState::default();
        process_file(
            &path,
            &mut state,
            &mut store,
            &options(&state_path, &history),
        )
        .unwrap();
    }

    let rows = store.log(&LogFilter::default()).unwrap();
    let projected = rows
        .iter()
        .filter(|row| row.session() == "trunc-session")
        .count();
    assert_eq!(projected, 2, "the upsert must not duplicate rows");
}

/// Test 4 from the occurrence-region spec: every `authored_by` classification
/// this adapter derives is reachable, and Codex's occurrences always carry a
/// `None` `agent` (no subagent concept).
#[test]
fn authored_by_classification_covers_human_and_agent_kinds() {
    let root = TestDir::new();
    let path = write_session(root.path(), &trunc_session_records(true));
    let history = root.path().join("history.jsonl");
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(
        &path,
        &mut state,
        &mut store,
        &options(&state_path, &history),
    )
    .unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    assert!(rows.iter().all(|row| row.agent().is_none()));
    assert!(
        rows.iter()
            .any(|row| row.authored_by() == AuthoredBy::Human)
    );
    assert!(
        rows.iter()
            .any(|row| row.authored_by() == AuthoredBy::Agent)
    );

    // The one occurrence with `repo_source` git-remote confirms the basis
    // this adapter attaches when `session_meta` carries a git remote URL.
    assert!(
        rows.iter()
            .all(|row| row.repo_source() == RepoSource::GitRemote)
    );
}

#[test]
fn candidate_files_are_sorted_with_history_last() {
    let root = TestDir::new();
    let sessions = root.path().join("sessions");
    fs::create_dir_all(sessions.join("nested")).unwrap();
    fs::write(sessions.join("z.jsonl"), b"").unwrap();
    fs::write(sessions.join("nested/a.jsonl"), b"").unwrap();
    fs::write(sessions.join("ignored.txt"), b"").unwrap();
    let history = root.path().join("history.jsonl");
    fs::write(&history, b"").unwrap();

    let files = candidate_files(&sessions, &history).unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files.last(), Some(&history));
    assert!(files[0] < files[1]);
}
