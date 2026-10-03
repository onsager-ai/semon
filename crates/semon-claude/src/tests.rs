use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use semon_store::{
    AuthoredBy, LogFilter, NewOccurrence, NewRawCarrierRecord, OccurrenceSelector, RepoSource,
    TraceStore,
};
use serde_json::{Value, json};

use super::*;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Source-shaped baseline with reversed results for overlapping same-name
/// calls. Every partial frame is followed by a durable cursor/store restart.
#[test]
fn compatibility_blocks_cold_restart_and_retained_raw_parity() {
    let source = include_bytes!("../../../tests/fixtures/compatibility/v1/claude-blocks.jsonl");
    compatibility_capture_parity(source, "compat", 5);
}

#[test]
fn compatibility_native_claude_2_1_288_cold_restart_and_retained_raw_parity() {
    let source = include_bytes!(
        "../../../tests/fixtures/compatibility/claude-2.1.288/initial-transcript.jsonl"
    );
    compatibility_capture_parity(source, "native-claude-compat", 2);
}

fn compatibility_capture_parity(source: &[u8], session: &str, expected_occurrences: usize) {
    let root = TestDir::new();
    let path = root.path().join(format!("{session}.jsonl"));
    let cursor = root.path().join("cursor.json");
    let database = root.path().join("incremental.sqlite3");
    let mut cold = TraceStore::open_in_memory().unwrap();
    fs::write(&path, source).unwrap();
    process_file(
        &path,
        &mut CursorState::default(),
        &mut cold,
        &options(&cursor),
    )
    .unwrap();
    assert_eq!(
        cold.log(&LogFilter::default()).unwrap().len(),
        expected_occurrences,
        "known fixture messages and blocks must project"
    );
    fs::remove_file(&cursor).unwrap();
    let mut end = 0;
    for line in source.split_inclusive(|byte| *byte == b'\n') {
        for boundary in [end + line.len() / 2, end + line.len()] {
            fs::write(&path, &source[..boundary]).unwrap();
            let mut state = load_state(&cursor).unwrap();
            let mut store = TraceStore::open(&database).unwrap();
            process_file(&path, &mut state, &mut store, &options(&cursor)).unwrap();
        }
        end += line.len();
    }
    let incremental = TraceStore::open(&database).unwrap();
    assert_eq!(
        cold.log(&LogFilter::default()).unwrap(),
        incremental.log(&LogFilter::default()).unwrap()
    );
    let raw = incremental
        .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(session))
        .unwrap();
    let retained: Vec<u8> = raw
        .iter()
        .flat_map(|row| row.bytes().iter().copied())
        .collect();
    assert_eq!(
        retained, source,
        "unknown and malformed complete lines must survive"
    );
    fs::write(&path, retained).unwrap();
    let mut rebuilt = TraceStore::open_in_memory().unwrap();
    process_file(
        &path,
        &mut CursorState::default(),
        &mut rebuilt,
        &options(&cursor),
    )
    .unwrap();
    assert_eq!(
        cold.log(&LogFilter::default()).unwrap(),
        rebuilt.log(&LogFilter::default()).unwrap()
    );
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "semon-claude-test-{}-{sequence}",
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

fn write_jsonl(path: &Path, records: &[Value]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let contents = records
        .iter()
        .map(|record| format!("{}\n", serde_json::to_string(record).unwrap()))
        .collect::<String>();
    fs::write(path, contents).unwrap();
}

fn options(state: &Path) -> ProcessOptions<'_> {
    ProcessOptions::new(state)
}

fn user_string(cwd: &str, uuid: &str, parent_uuid: Option<&str>, text: &str) -> Value {
    json!({
        "type": "user",
        "uuid": uuid,
        "parentUuid": parent_uuid,
        "isSidechain": false,
        "cwd": cwd,
        "timestamp": "2026-09-20T00:00:00Z",
        "message": {"role": "user", "content": text},
    })
}

fn sidechain_user_string(cwd: &str, agent_id: &str, uuid: &str, text: &str) -> Value {
    json!({
        "type": "user",
        "uuid": uuid,
        "parentUuid": Value::Null,
        "isSidechain": true,
        "agentId": agent_id,
        "cwd": cwd,
        "timestamp": "2026-09-20T00:00:00Z",
        "message": {"role": "user", "content": text},
    })
}

#[test]
fn raw_backfill_restores_multiblock_and_subagent_prefixes() {
    let root = TestDir::new();
    let principal = root.path().join("session-backfill.jsonl");
    let subagent = root.path().join("session-backfill/subagents/agent-A.jsonl");
    write_jsonl(
        &principal,
        &[
            json!({"type":"system","timestamp":"2026-09-20T00:00:00Z"}),
            json!({"type":"assistant","uuid":"a1","parentUuid":null,"cwd":"/work/repo","timestamp":"2026-09-20T00:00:01Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"first"},{"type":"text","text":"second"}]}}),
            json!({"type":"system","timestamp":"2026-09-20T00:00:02Z"}),
        ],
    );
    write_jsonl(
        &subagent,
        &[
            json!({"type":"system","timestamp":"2026-09-20T00:00:03Z"}),
            sidechain_user_string("/work/repo", "A", "u1", "subagent request"),
        ],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut fresh = TraceStore::open(root.path().join("fresh.sqlite3")).unwrap();
    for path in [&principal, &subagent] {
        process_file(path, &mut state, &mut fresh, &options(&state_path)).unwrap();
    }
    let state_bytes = fs::read(&state_path).unwrap();
    let principal_session = "session-backfill";
    let subagent_session = "session-backfill/agent-A";
    let occurrences = fresh.log(&LogFilter::default()).unwrap();
    assert_eq!(occurrences.len(), 3);
    let mut old = TraceStore::open(root.path().join("old.sqlite3")).unwrap();
    for (path, session) in [
        (&principal, principal_session),
        (&subagent, subagent_session),
    ] {
        let source = fs::read(path).unwrap();
        for (ordinal, line) in source.split_inclusive(|byte| *byte == b'\n').enumerate() {
            let matched = occurrences
                .iter()
                .filter(|row| row.session() == session && row.sequence() / 1024 == ordinal as i64)
                .collect::<Vec<_>>();
            if matched.is_empty() {
                continue;
            }
            let blocks = matched
                .iter()
                .map(|row| {
                    (
                        row.semantic_core().clone(),
                        NewOccurrence {
                            session: row.session(),
                            sequence: row.sequence(),
                            timestamp: row.timestamp(),
                            repo: row.repo(),
                            repo_source: row.repo_source(),
                            parent_sequence: row.parent_sequence(),
                            agent: row.agent(),
                            authored_by: row.authored_by(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            old.capture_line(
                NewRawCarrierRecord::new(CARRIER, line),
                session,
                sequence_for(ordinal as u64, 0),
                matched[0].timestamp(),
                &blocks,
            )
            .unwrap();
        }
    }
    let baseline_traces = old.list_traces(None, 10).unwrap().len();
    let baseline_log = old.log(&LogFilter::default()).unwrap().len();
    let baseline_links = [principal_session, subagent_session]
        .iter()
        .map(|session| {
            old.fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(session))
                .unwrap()
                .iter()
                .map(|row| row.trace_ids().len())
                .sum::<usize>()
        })
        .sum::<usize>();
    let preview = backfill_raw(&state, &mut old, true).unwrap();
    assert_eq!(
        (
            preview.files_scanned,
            preview.lines_scanned,
            preview.rows_inserted,
            preview.rows_already_present
        ),
        (2, 5, 3, 2)
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&principal)
        .unwrap()
        .write_all(b"{\"type\":\"extra\"}\n")
        .unwrap();
    assert_eq!(
        backfill_raw(&state, &mut old, false).unwrap().rows_inserted,
        3
    );
    assert_eq!(
        backfill_raw(&state, &mut old, false).unwrap().rows_inserted,
        0
    );
    for session in [principal_session, subagent_session] {
        let expected = fresh
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(session))
            .unwrap();
        let actual = old
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(session))
            .unwrap();
        assert_eq!(actual.len(), expected.len());
        for row in &expected {
            let restored = actual
                .iter()
                .find(|candidate| candidate.sequence() == row.sequence())
                .unwrap();
            assert_eq!(
                (restored.bytes(), restored.session(), restored.timestamp()),
                (row.bytes(), row.session(), row.timestamp())
            );
        }
    }
    assert_eq!(old.list_traces(None, 10).unwrap().len(), baseline_traces);
    assert_eq!(old.log(&LogFilter::default()).unwrap().len(), baseline_log);
    let links = [principal_session, subagent_session]
        .iter()
        .map(|session| {
            old.fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(session))
                .unwrap()
                .iter()
                .map(|row| row.trace_ids().len())
                .sum::<usize>()
        })
        .sum::<usize>();
    assert_eq!(links, baseline_links);
    assert_eq!(fs::read(&state_path).unwrap(), state_bytes);
}

fn assistant_text(cwd: &str, uuid: &str, parent_uuid: Option<&str>, text: &str) -> Value {
    json!({
        "type": "assistant",
        "uuid": uuid,
        "parentUuid": parent_uuid,
        "isSidechain": false,
        "cwd": cwd,
        "timestamp": "2026-09-20T00:00:00Z",
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
    })
}

/// Test 1 from the spec: the collision case, directly. A synthetic principal
/// file and a synthetic subagent file for the same `sessionId`, both with
/// content on the same line numbers, must produce two occurrences with
/// distinct `session` values, neither overwriting the other.
#[test]
fn principal_and_subagent_files_do_not_collide_on_the_same_line_number() {
    let root = TestDir::new();
    let session_id = "session-collision";
    let principal_path = root.path().join(format!("{session_id}.jsonl"));
    let subagent_path = root
        .path()
        .join(session_id)
        .join("subagents/agent-agentA.jsonl");

    write_jsonl(
        &principal_path,
        &[user_string(
            "/work/repo",
            "p-uuid-0",
            None,
            "principal line zero",
        )],
    );
    write_jsonl(
        &subagent_path,
        &[sidechain_user_string(
            "/work/repo",
            "agentA",
            "s-uuid-0",
            "subagent line zero",
        )],
    );

    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();

    process_file(
        &principal_path,
        &mut state,
        &mut store,
        &options(&state_path),
    )
    .unwrap();
    process_file(
        &subagent_path,
        &mut state,
        &mut store,
        &options(&state_path),
    )
    .unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "both occurrences must be retained, not upserted over each other"
    );

    let principal_row = rows
        .iter()
        .find(|row| row.session() == session_id)
        .expect("principal occurrence missing");
    let subagent_row = rows
        .iter()
        .find(|row| row.session() == format!("{session_id}/agent-agentA"))
        .expect("subagent occurrence missing");

    assert_ne!(principal_row.session(), subagent_row.session());
    assert_eq!(principal_row.sequence(), 0);
    assert_eq!(subagent_row.sequence(), 0);
    assert_eq!(
        principal_row.semantic_core().value(),
        &json!({"kind": "intent", "content": "principal line zero"})
    );
    assert_eq!(
        subagent_row.semantic_core().value(),
        &json!({"kind": "intent", "content": "subagent line zero"})
    );
    assert_eq!(principal_row.authored_by(), AuthoredBy::Human);
    assert_eq!(subagent_row.authored_by(), AuthoredBy::Agent);
    assert_eq!(subagent_row.agent(), Some("agentA"));
    assert_eq!(principal_row.agent(), None);
}

#[test]
fn record_with_no_projected_blocks_writes_one_raw_only_row() {
    let root = TestDir::new();
    let path = root.path().join("session-raw-only.jsonl");
    write_jsonl(
        &path,
        &[json!({
            "type": "user",
            "uuid": "meta-uuid",
            "parentUuid": Value::Null,
            "isMeta": true,
            "cwd": "/work/repo",
            "timestamp": "2026-09-20T00:00:00Z",
            "message": {"role": "user", "content": "harness metadata"},
        })],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();

    assert_eq!(
        process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap(),
        1
    );
    assert!(store.log(&LogFilter::default()).unwrap().is_empty());
    assert!(store.list_traces(None, 10).unwrap().is_empty());

    let raw = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "session-raw-only",
        ))
        .unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].trace_ids(), &[]);
    assert!(
        std::str::from_utf8(raw[0].bytes())
            .unwrap()
            .contains("harness metadata")
    );
}

#[test]
fn three_block_line_has_one_raw_row_and_file_replay_reuses_it() {
    let root = TestDir::new();
    let path = root.path().join("session-three-blocks.jsonl");
    write_jsonl(
        &path,
        &[json!({
            "type": "assistant",
            "uuid": "a0",
            "parentUuid": Value::Null,
            "cwd": "/work/repo",
            "timestamp": "2026-09-20T00:00:00Z",
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "first"},
                {"type": "text", "text": "second"},
                {"type": "text", "text": "third"},
            ]},
        })],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    assert_eq!(
        process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap(),
        1
    );
    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(rows.len(), 3);
    let raw = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "session-three-blocks",
        ))
        .unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].trace_ids().len(), 3);

    let mut replay_state = CursorState::default();
    assert_eq!(
        process_file(&path, &mut replay_state, &mut store, &options(&state_path)).unwrap(),
        1
    );
    let replayed = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "session-three-blocks",
        ))
        .unwrap();
    assert_eq!(replayed, raw);
    assert_eq!(store.log(&LogFilter::default()).unwrap(), rows);
}

/// Test 2 from the spec: per-block sequence. An assistant record with `text`
/// and `tool_use` blocks projects two occurrences with sequences
/// `L * 1024 + 0` and `L * 1024 + 1`, and the second's parent is the first.
#[test]
fn assistant_record_with_two_blocks_gets_sequential_sequences_and_chained_parent() {
    let root = TestDir::new();
    let path = root.path().join("session-two-blocks.jsonl");
    write_jsonl(
        &path,
        &[json!({
            "type": "assistant",
            "uuid": "a-uuid-0",
            "parentUuid": Value::Null,
            "isSidechain": false,
            "cwd": "/work/repo",
            "timestamp": "2026-09-20T00:00:00Z",
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "doing it"},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}},
            ]},
        })],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();

    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let mut rows = store.log(&LogFilter::default()).unwrap();
    rows.sort_by_key(semon_store::OccurrenceRecord::sequence);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sequence(), 0); // line 0, block 0 => 0*1024+0
    assert_eq!(rows[1].sequence(), 1); // line 0, block 1 => 0*1024+1
    assert_eq!(rows[0].parent_sequence(), None);
    assert_eq!(rows[1].parent_sequence(), Some(rows[0].sequence()));
}

fn trunc_records(include_second_turn: bool) -> Vec<Value> {
    let mut records = vec![
        user_string("/work/repo", "u0", None, "first request"),
        assistant_text("/work/repo", "a0", Some("u0"), "first reply"),
    ];
    if include_second_turn {
        records.push(user_string(
            "/work/repo",
            "u1",
            Some("a0"),
            "second request",
        ));
        records.push(assistant_text(
            "/work/repo",
            "a1",
            Some("u1"),
            "second reply",
        ));
    }
    records
}

/// Test 3 from the spec: truncate-and-recapture. Capture, truncate the file,
/// capture again: occurrence rows for the surviving lines must be identical
/// field for field to an independent from-scratch capture of exactly that
/// truncated content.
#[test]
fn truncate_and_recapture_regenerates_identical_occurrence_fields() {
    let full = trunc_records(true);
    let truncated = trunc_records(false);

    let root_a = TestDir::new();
    let path_a = root_a.path().join("trunc-session.jsonl");
    write_jsonl(&path_a, &full);
    let state_path_a = root_a.path().join("state.json");
    let mut state_a = CursorState::default();
    let mut store_a = TraceStore::open_in_memory().unwrap();
    assert_eq!(
        process_file(&path_a, &mut state_a, &mut store_a, &options(&state_path_a)).unwrap(),
        4
    );

    write_jsonl(&path_a, &truncated);
    assert_eq!(
        process_file(&path_a, &mut state_a, &mut store_a, &options(&state_path_a)).unwrap(),
        2,
        "truncation must reset the cursor and replay the surviving lines"
    );

    let root_b = TestDir::new();
    let path_b = root_b.path().join("trunc-session.jsonl");
    write_jsonl(&path_b, &truncated);
    let state_path_b = root_b.path().join("state.json");
    let mut state_b = CursorState::default();
    let mut store_b = TraceStore::open_in_memory().unwrap();
    assert_eq!(
        process_file(&path_b, &mut state_b, &mut store_b, &options(&state_path_b)).unwrap(),
        2
    );

    let mut rows_a = store_a.log(&LogFilter::default()).unwrap();
    rows_a.retain(|row| row.sequence() < 2 * MAX_BLOCKS_PER_RECORD as i64);
    let rows_b = store_b.log(&LogFilter::default()).unwrap();

    assert_eq!(rows_b.len(), 2);
    assert_eq!(
        rows_a, rows_b,
        "occurrence rows must match field for field after truncate-and-recapture"
    );
}

/// Test 4 from the spec: structural-only. A `tool_use` whose input carries a
/// sentinel string and an absolute path outside `cwd` must not leak either
/// into the projected trace; `path` becomes `"<external>"`.
#[test]
fn tool_use_projection_is_structural_only() {
    let root = TestDir::new();
    let path = root.path().join("session-structural.jsonl");
    let sentinel = "TOP-SECRET-SENTINEL-VALUE";
    write_jsonl(
        &path,
        &[json!({
            "type": "assistant",
            "uuid": "a-uuid-0",
            "parentUuid": Value::Null,
            "isSidechain": false,
            "cwd": "/work/repo",
            "timestamp": "2026-09-20T00:00:00Z",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {
                    "command": format!("echo {sentinel} > /etc/outside/leak.txt"),
                    "description": sentinel,
                }},
            ]},
        })],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].semantic_core().value(),
        &json!({"kind": "action", "action": "Bash", "path": null})
    );
    let rendered = rows[0].semantic_core().value().to_string();
    assert!(
        !rendered.contains(sentinel),
        "sentinel leaked into the trace"
    );
    assert!(
        !rendered.contains("/etc/outside"),
        "absolute path leaked into the trace"
    );

    // A file-path-carrying tool projects the path rule's "<external>" literal
    // for a path outside cwd, never the path itself. A separate session file
    // keeps this independent of the capture above (same-session, same-line
    // captures upsert on purpose — see the truncate-and-recapture test — and
    // this assertion is not exercising that).
    let path2 = root.path().join("session-structural-2.jsonl");
    write_jsonl(
        &path2,
        &[json!({
            "type": "assistant",
            "uuid": "a-uuid-1",
            "parentUuid": Value::Null,
            "isSidechain": false,
            "cwd": "/work/repo",
            "timestamp": "2026-09-20T00:00:00Z",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/etc/outside/leak.txt"}},
            ]},
        })],
    );
    let state_path2 = root.path().join("state2.json");
    let mut state2 = CursorState::default();
    process_file(&path2, &mut state2, &mut store, &options(&state_path2)).unwrap();
    let rows = store.log(&LogFilter::default()).unwrap();
    let read_row = rows
        .iter()
        .find(|row| row.semantic_core().value()["action"] == "Read")
        .unwrap();
    assert_eq!(read_row.semantic_core().value()["path"], "<external>");
    assert_eq!(
        rows.len(),
        2,
        "both captures must be retained (different sessions)"
    );
}

/// Test 5 from the spec: authorship. The subagent task prompt (`isSidechain`)
/// is `agent`, not `human`.
#[test]
fn subagent_task_prompt_is_agent_authored() {
    let root = TestDir::new();
    let session_id = "session-authorship";
    let subagent_path = root
        .path()
        .join(session_id)
        .join("subagents/agent-agentB.jsonl");
    write_jsonl(
        &subagent_path,
        &[sidechain_user_string(
            "/work/repo",
            "agentB",
            "s-uuid-0",
            "run the subtask",
        )],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(
        &subagent_path,
        &mut state,
        &mut store,
        &options(&state_path),
    )
    .unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].authored_by(), AuthoredBy::Agent);
    assert_eq!(rows[0].agent(), Some("agentB"));
    assert_eq!(rows[0].session(), format!("{session_id}/agent-agentB"));
}

/// The ancestor walk must skip records that projected nothing at all (e.g. a
/// `thinking`-only assistant record) and still find the nearest real
/// projected ancestor, within the same file.
#[test]
fn parent_sequence_skips_non_projecting_ancestors() {
    let root = TestDir::new();
    let path = root.path().join("session-skip.jsonl");
    write_jsonl(
        &path,
        &[
            user_string("/work/repo", "u0", None, "first request"),
            json!({
                "type": "assistant",
                "uuid": "think0",
                "parentUuid": "u0",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "pondering..."},
                ]},
            }),
            assistant_text("/work/repo", "a0", Some("think0"), "final reply"),
        ],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let mut rows = store.log(&LogFilter::default()).unwrap();
    rows.sort_by_key(semon_store::OccurrenceRecord::sequence);
    assert_eq!(
        rows.len(),
        2,
        "the thinking-only record must not itself project"
    );
    assert_eq!(rows[0].sequence(), 0); // line 0 (user), block 0
    assert_eq!(rows[1].sequence(), 2 * MAX_BLOCKS_PER_RECORD as i64); // line 2 (assistant), block 0
    assert_eq!(
        rows[1].parent_sequence(),
        Some(rows[0].sequence()),
        "must skip the non-projecting thinking-only record and find the user prompt"
    );
}

/// A record whose `parentUuid` never resolves to a projected ancestor (a
/// genuine root) must record a `NULL` parent, not error or default to zero.
#[test]
fn parent_sequence_is_null_at_a_root() {
    let root = TestDir::new();
    let path = root.path().join("session-root.jsonl");
    write_jsonl(&path, &[user_string("/work/repo", "u0", None, "hello")]);
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].parent_sequence(), None);
}

/// Regression test for the parent-edge defect: a `{thinking, tool_use}`
/// record whose parent record itself projected. Before the fix, `tool_use`
/// sits at `block_index == 1`, so `block_index - 1 == 0` pointed at this
/// record's own `thinking` block — which never projects, so `line*1024+0`
/// names an occurrence that was never captured — instead of the parent
/// record's own last projected sequence.
#[test]
fn skipped_leading_block_does_not_break_the_parent_edge() {
    let root = TestDir::new();
    let path = root.path().join("session-skip-leading.jsonl");
    write_jsonl(
        &path,
        &[
            user_string("/work/repo", "u0", None, "first request"),
            json!({
                "type": "assistant",
                "uuid": "a0",
                "parentUuid": "u0",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "pondering..."},
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}},
                ]},
            }),
        ],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let mut rows = store.log(&LogFilter::default()).unwrap();
    rows.sort_by_key(semon_store::OccurrenceRecord::sequence);
    assert_eq!(rows.len(), 2, "the thinking block must not itself project");
    let user_row = &rows[0];
    let tool_use_row = &rows[1];
    assert_eq!(user_row.sequence(), 0); // line 0, block 0
    assert_eq!(tool_use_row.sequence(), MAX_BLOCKS_PER_RECORD as i64 + 1); // line 1, block 1
    assert_eq!(
        tool_use_row.parent_sequence(),
        Some(user_row.sequence()),
        "must point at the user prompt (the nearest real occurrence), not at \
         line*1024+0, which the skipped thinking block never occupied"
    );
}

/// Regression test for the parent-edge defect: `{text, thinking, tool_use}`
/// in one record. `tool_use`'s parent must be the *text* block's sequence
/// (the preceding *projected* block), not `line*1024+1`, which the skipped
/// `thinking` block in between never occupied.
#[test]
fn skipped_middle_block_parent_points_at_preceding_projected_block() {
    let root = TestDir::new();
    let path = root.path().join("session-skip-middle.jsonl");
    write_jsonl(
        &path,
        &[json!({
            "type": "assistant",
            "uuid": "a0",
            "parentUuid": Value::Null,
            "isSidechain": false,
            "cwd": "/work/repo",
            "timestamp": "2026-09-20T00:00:00Z",
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "let me check"},
                {"type": "thinking", "thinking": "pondering..."},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}},
            ]},
        })],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let mut rows = store.log(&LogFilter::default()).unwrap();
    rows.sort_by_key(semon_store::OccurrenceRecord::sequence);
    assert_eq!(rows.len(), 2, "the thinking block must not itself project");
    let text_row = &rows[0];
    let tool_use_row = &rows[1];
    assert_eq!(text_row.sequence(), 0); // line 0, block 0
    assert_eq!(tool_use_row.sequence(), 2); // line 0, block 2
    assert_eq!(
        tool_use_row.parent_sequence(),
        Some(text_row.sequence()),
        "must point at the text block, not at line*1024+1, which the \
         skipped thinking block never occupied"
    );
}

/// General invariant for the parent-edge defect: over a small multi-record
/// fixture mixing skipped-leading and skipped-middle shapes, every non-NULL
/// `parent_sequence` must name an occurrence that actually exists in the
/// same session.
#[test]
fn every_parent_sequence_names_an_existing_occurrence_in_the_same_session() {
    let root = TestDir::new();
    let session = "session-invariant";
    let path = root.path().join(format!("{session}.jsonl"));
    write_jsonl(
        &path,
        &[
            user_string("/work/repo", "u0", None, "first request"),
            json!({
                "type": "assistant",
                "uuid": "a0",
                "parentUuid": "u0",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "pondering..."},
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}},
                ]},
            }),
            json!({
                "type": "user",
                "uuid": "u1",
                "parentUuid": "a0",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": []},
                ]},
            }),
            json!({
                "type": "assistant",
                "uuid": "a1",
                "parentUuid": "u1",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "let me check"},
                    {"type": "thinking", "thinking": "pondering more..."},
                    {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "echo bye"}},
                ]},
            }),
        ],
    );
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    let session_rows: Vec<_> = rows.iter().filter(|row| row.session() == session).collect();
    let sequences: std::collections::BTreeSet<i64> =
        session_rows.iter().map(|row| row.sequence()).collect();
    assert!(!sequences.is_empty());
    for row in &session_rows {
        if let Some(parent) = row.parent_sequence() {
            assert!(
                sequences.contains(&parent),
                "parent_sequence {parent} for occurrence {} names no \
                 existing occurrence in session {session}",
                row.sequence()
            );
        }
    }
}

#[test]
fn repo_source_is_cwd_basename_unless_overridden() {
    let root = TestDir::new();
    let path = root.path().join("session-repo.jsonl");
    write_jsonl(&path, &[user_string("/work/my-repo", "u0", None, "hello")]);
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(rows[0].repo(), "my-repo");
    assert_eq!(rows[0].repo_source(), RepoSource::CwdBasename);

    // An explicit override always wins, independent of `cwd`.
    let path2 = root.path().join("session-repo-2.jsonl");
    write_jsonl(
        &path2,
        &[user_string("/work/my-repo", "u1", None, "hello again")],
    );
    let state_path2 = root.path().join("state2.json");
    let mut state2 = CursorState::default();
    let mut overridden = options(&state_path2);
    overridden.repo_override = "explicit-name";
    process_file(&path2, &mut state2, &mut store, &overridden).unwrap();
    let rows = store.log(&LogFilter::default()).unwrap();
    let overridden_row = rows
        .iter()
        .find(|row| row.repo() == "explicit-name")
        .unwrap();
    assert_eq!(overridden_row.repo_source(), RepoSource::ExplicitOverride);
}

#[test]
fn candidate_files_recurses_into_subagents() {
    let root = TestDir::new();
    let session_id = "session-candidates";
    let principal = root.path().join(format!("{session_id}.jsonl"));
    let subagent = root.path().join(session_id).join("subagents/agent-x.jsonl");
    write_jsonl(&principal, &[user_string("/work/repo", "u0", None, "hi")]);
    write_jsonl(
        &subagent,
        &[sidechain_user_string("/work/repo", "x", "s0", "hi")],
    );
    // A non-jsonl sibling must be ignored.
    fs::write(
        root.path()
            .join(session_id)
            .join("subagents/agent-x.meta.json"),
        b"{}",
    )
    .unwrap();

    let files = candidate_files(root.path()).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.contains(&principal));
    assert!(files.contains(&subagent));
}

#[test]
fn file_identity_distinguishes_principal_and_subagent_paths() {
    let principal = Path::new("/root/slug/session-a.jsonl");
    assert_eq!(file_identity(principal), ("session-a".to_owned(), None));

    let subagent = Path::new("/root/slug/session-a/subagents/agent-xyz.jsonl");
    assert_eq!(
        file_identity(subagent),
        ("session-a".to_owned(), Some("xyz".to_owned()))
    );
}

/// Re-running from the saved cursor over unmodified content must not
/// duplicate occurrences (the upsert on the natural key, and the cursor's
/// own "nothing left to read" behavior, both apply here).
#[test]
fn recapturing_the_same_content_upserts_rather_than_duplicates() {
    let root = TestDir::new();
    let path = root.path().join("session-recapture.jsonl");
    write_jsonl(&path, &trunc_records(false));
    let state_path = root.path().join("state.json");
    let mut store = TraceStore::open_in_memory().unwrap();

    for _ in 0..2 {
        let mut state = CursorState::default();
        process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();
    }

    let rows = store.log(&LogFilter::default()).unwrap();
    assert_eq!(rows.len(), 2, "the upsert must not duplicate rows");
}

/// The DAG-ancestor map is deliberately never persisted (see `state`'s
/// module docs). This exercises the rebuild path directly: capture a
/// prefix whose only projecting record is followed by a non-projecting one
/// (so the *last* consumed line resolves to an ancestor further back than
/// itself), persist the cursor, append more lines that depend on that
/// resolution via `parentUuid`, and process again from the saved (nonzero,
/// not-yet-caught-up) offset. The appended record's `parent_sequence` must
/// still resolve correctly, proving the rebuild reconstructs the same
/// ancestor chain an uninterrupted run would have kept in memory.
#[test]
fn resuming_a_growing_file_rebuilds_the_ancestor_chain_from_disk() {
    let root = TestDir::new();
    let path = root.path().join("session-resume.jsonl");
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();

    // First pass: a user prompt (projects), then a thinking-only assistant
    // record (does not project) as the last consumed line — so the cursor
    // is left at an offset whose *own* last line resolves to an ancestor
    // one record further back.
    write_jsonl(
        &path,
        &[
            user_string("/work/repo", "u0", None, "first request"),
            json!({
                "type": "assistant",
                "uuid": "think0",
                "parentUuid": "u0",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "pondering..."},
                ]},
            }),
        ],
    );
    assert_eq!(
        process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap(),
        2
    );

    // Second pass: append a record whose parentUuid names the
    // non-projecting `think0`, then re-process from the saved (nonzero)
    // offset. The cursor carries no ancestor map across this call — it must
    // be rebuilt from the file itself.
    write_jsonl(
        &path,
        &[
            user_string("/work/repo", "u0", None, "first request"),
            json!({
                "type": "assistant",
                "uuid": "think0",
                "parentUuid": "u0",
                "isSidechain": false,
                "cwd": "/work/repo",
                "timestamp": "2026-09-20T00:00:00Z",
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "pondering..."},
                ]},
            }),
            assistant_text("/work/repo", "a0", Some("think0"), "final reply"),
        ],
    );
    assert_eq!(
        process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap(),
        1,
        "only the newly appended line should be consumed"
    );

    let mut rows = store.log(&LogFilter::default()).unwrap();
    rows.sort_by_key(semon_store::OccurrenceRecord::sequence);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sequence(), 0); // line 0 (user), block 0
    assert_eq!(rows[1].sequence(), 2 * MAX_BLOCKS_PER_RECORD as i64); // line 2 (assistant), block 0
    assert_eq!(
        rows[1].parent_sequence(),
        Some(rows[0].sequence()),
        "the rebuilt ancestor map must skip the non-projecting thinking-only \
         record and find the user prompt, exactly as an uninterrupted run would"
    );
}

/// The cursor's on-disk JSON must not grow with the number of records in a
/// file — only with the number of files tracked. This is the regression
/// test for the write-amplification defect: an earlier version stored one
/// `ancestor_sequence` entry per record `uuid`, so the saved document grew
/// without bound as a session grew.
#[test]
fn saved_cursor_state_carries_no_per_record_data() {
    let root = TestDir::new();
    let path = root.path().join("session-cursor-size.jsonl");
    // Enough records that a per-record cursor entry would be easy to spot.
    let mut records = Vec::new();
    for index in 0..50 {
        let uuid = format!("u{index}");
        let parent_uuid = (index > 0).then(|| format!("u{}", index - 1));
        let text = format!("request {index}");
        records.push(user_string(
            "/work/repo",
            &uuid,
            parent_uuid.as_deref(),
            &text,
        ));
    }
    write_jsonl(&path, &records);
    let state_path = root.path().join("state.json");
    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap();

    let saved_bytes = fs::read(&state_path).unwrap();
    let saved: Value = serde_json::from_slice(&saved_bytes).unwrap();
    let file_entry = saved["files"].as_object().unwrap().values().next().unwrap();
    let keys: std::collections::BTreeSet<&str> = file_entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from(["offset", "next_line_ordinal", "prefix_sha256"]),
        "the per-file cursor must carry only an offset, a line counter and a constant-size digest, \
         never a per-record ancestor map"
    );
    assert!(
        saved_bytes.len() < 500,
        "cursor file grew suspiciously large ({} bytes) for 50 records \
         with no per-record data expected",
        saved_bytes.len()
    );
}

/// Regression test for the truncation-reset defect: a truncation reset
/// detected at the top of `process_file` must reach disk even when the
/// truncated file has no complete line to consume yet. Otherwise the stale
/// pre-truncation offset stays saved, and once the file later grows past
/// it, the next run resumes from the stale offset and silently skips the
/// new prefix.
#[test]
fn truncation_reset_persists_even_without_a_complete_line_to_consume() {
    let root = TestDir::new();
    let path = root.path().join("session-truncate-partial.jsonl");
    let state_path = root.path().join("state.json");

    write_jsonl(&path, &trunc_records(true));
    let key = path.canonicalize().unwrap().to_string_lossy().into_owned();

    let mut state = CursorState::default();
    let mut store = TraceStore::open_in_memory().unwrap();
    assert_eq!(
        process_file(&path, &mut state, &mut store, &options(&state_path)).unwrap(),
        4
    );

    // Truncate to a partial line, shorter than the first newline, so the
    // next run's batch loop cannot consume even one complete record.
    let full_bytes = fs::read(&path).unwrap();
    let first_newline = full_bytes.iter().position(|&b| b == b'\n').unwrap();
    fs::write(&path, &full_bytes[..first_newline / 2]).unwrap();

    // A genuinely separate run: load the cursor fresh from disk, exactly as
    // `main.rs` does at startup.
    let mut resumed = load_state(&state_path).unwrap();
    assert_eq!(
        process_file(&path, &mut resumed, &mut store, &options(&state_path)).unwrap(),
        0,
        "the partial line is not a complete record"
    );

    let mut reloaded = load_state(&state_path).unwrap();
    let cursor = reloaded.take_file(&key);
    assert_eq!(
        cursor.offset, 0,
        "the truncation reset must reach disk even though no batch was consumed"
    );
    assert_eq!(cursor.next_line_ordinal, 0);

    // Grow the file back past the original content: if the reset was
    // persisted, this must recapture everything from the start rather than
    // resuming from the stale pre-truncation offset and skipping the
    // regrown prefix.
    write_jsonl(&path, &trunc_records(true));
    let mut regrown = load_state(&state_path).unwrap();
    assert_eq!(
        process_file(&path, &mut regrown, &mut store, &options(&state_path)).unwrap(),
        4,
        "must recapture from the start once the file grows back, not skip \
         the prefix a stale offset would have hidden"
    );
}

#[test]
fn consumed_prefix_replacement_replays_after_restart() {
    let fixture = include_str!("../../../tests/fixtures/compatibility/v1/claude-blocks.jsonl");
    for (padding, replacement) in [
        (0, "compatibility example"),
        (0, "compatibility expanded replacement example"),
        (8192, "compatibility example"),
    ] {
        let root = TestDir::new();
        let path = root.path().join("compat.jsonl");
        let cursor = root.path().join("cursor.json");
        let database = root.path().join("capture.sqlite3");
        let padding = format!(
            "{{\"type\":\"future_padding\",\"data\":\"{}\"}}\n",
            "x".repeat(padding)
        );
        let original = format!("{padding}{fixture}{padding}");
        let changed = original.replace("compatibility request", replacement);
        assert_ne!(original, changed);
        fs::write(&path, &original).unwrap();
        {
            let mut store = TraceStore::open(&database).unwrap();
            process_file(
                &path,
                &mut CursorState::default(),
                &mut store,
                &options(&cursor),
            )
            .unwrap();
        }
        // Old installed cursors have no digest: replay once without duplicating.
        let mut document: Value = serde_json::from_slice(&fs::read(&cursor).unwrap()).unwrap();
        for value in document["files"].as_object_mut().unwrap().values_mut() {
            value.as_object_mut().unwrap().remove("prefix_sha256");
        }
        fs::write(&cursor, serde_json::to_vec(&document).unwrap()).unwrap();
        {
            let mut state = load_state(&cursor).unwrap();
            let mut store = TraceStore::open(&database).unwrap();
            assert!(process_file(&path, &mut state, &mut store, &options(&cursor)).unwrap() > 0);
            assert_eq!(
                process_file(&path, &mut state, &mut store, &options(&cursor)).unwrap(),
                0
            );
        }
        fs::write(&path, &changed).unwrap();
        let mut state = load_state(&cursor).unwrap();
        let mut store = TraceStore::open(&database).unwrap();
        assert!(process_file(&path, &mut state, &mut store, &options(&cursor)).unwrap() > 0);
        let mut cold = TraceStore::open_in_memory().unwrap();
        process_file(
            &path,
            &mut CursorState::default(),
            &mut cold,
            &options(&cursor),
        )
        .unwrap();
        assert_eq!(
            store.log(&LogFilter::default()).unwrap(),
            cold.log(&LogFilter::default()).unwrap()
        );
        let retained = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("compat"))
            .unwrap();
        let old_line = original
            .lines()
            .find(|line| line.contains("compatibility request"))
            .unwrap();
        assert!(
            retained
                .iter()
                .any(|row| row.bytes() == format!("{old_line}\n").as_bytes()),
            "replacement must preserve previous forensic bytes"
        );
    }
}
