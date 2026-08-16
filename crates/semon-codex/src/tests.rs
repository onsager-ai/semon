use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use semon_store::TraceStore;
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
