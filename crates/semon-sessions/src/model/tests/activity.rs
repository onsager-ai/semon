//! Synthetic activity publication through the complete native model producer.
use super::*;
use crate::events::{
    session_activity::{ActivityStepId, Ownership},
    store::session_activity::{self, Page},
};

fn activity(home: &Home, key: &str) -> Page {
    let (_, rows) = read_catalog(home);
    let row = rows.iter().find(|row| row.key == key).unwrap();
    let c = rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    session_activity::page(
        &tx,
        row,
        None,
        &crate::pricing::snapshot().fingerprint,
        0,
        256,
    )
    .unwrap()
    .unwrap()
}

#[test]
fn activity_claude_content_blocks_unrelated_ids_append_and_restart() {
    let home = Home::new();
    let first = uuid(
        assistant_usage(
            "first",
            ts(0, 1),
            "same-message",
            "claude-sonnet-4-6",
            json!({"input_tokens":11,"output_tokens":3}),
        ),
        "first-record",
    );
    let mut repeated = first.clone();
    repeated["message"]["content"] =
        json!([{"type":"text","text":"synthetic second content block"}]);
    home.top(
        "first",
        &[
            human("first", ts(0, 0), "synthetic prompt"),
            first,
            repeated,
        ],
    );
    home.top(
        "other",
        &[
            human("other", ts(0, 0), "synthetic prompt"),
            uuid(
                assistant_usage(
                    "other",
                    ts(0, 1),
                    "same-message",
                    "claude-sonnet-4-6",
                    json!({"input_tokens":17,"output_tokens":5}),
                ),
                "other-record",
            ),
        ],
    );
    home.build_query();
    let initial = activity(&home, "first");
    assert_eq!(initial.total, 1);
    assert_eq!(initial.steps[0].tokens.input, 11);
    assert_eq!(initial.steps[0].ownership, Ownership::Exclusive);
    assert_eq!(activity(&home, "other").steps[0].tokens.input, 17);
    home.build_query();
    assert_eq!(activity(&home, "first"), initial);
    append_records(
        &home,
        "first",
        &[uuid(
            assistant_usage(
                "first",
                ts(0, 2),
                "next-message",
                "claude-sonnet-4-6",
                json!({"input_tokens":13,"output_tokens":7}),
            ),
            "next-record",
        )],
    );
    home.build_query();
    let appended = activity(&home, "first");
    assert_eq!(appended.total, 2);
    assert_ne!(appended.generation, initial.generation);
    assert_eq!(appended.steps[0].elapsed_ms, Some(60_000));
    assert!(appended.steps[1].position.offset > appended.steps[0].position.offset);
    home.build_query();
    assert_eq!(activity(&home, "first"), appended);
    let c = rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    c.execute(
        "UPDATE session_activity_projections SET rules_version=0 WHERE session_key='first'",
        [],
    )
    .unwrap();
    home.build_query();
    assert_eq!(activity(&home, "first"), appended);
    let (_, rows) = read_catalog(&home);
    let json = serde_json::to_value(&rows[0]).unwrap();
    assert!(json.get("session_activity").is_none());
}

#[test]
fn activity_exact_copied_claude_records_remain_shared_observations() {
    let home = Home::new();
    for id in ["first", "second"] {
        home.top(
            id,
            &[
                human(id, ts(0, 0), "synthetic prompt"),
                uuid(
                    assistant_usage(
                        id,
                        ts(0, 1),
                        "copied-message",
                        "claude-sonnet-4-6",
                        json!({"input_tokens":19,"output_tokens":7}),
                    ),
                    "copied-record",
                ),
            ],
        );
    }
    home.build_query();
    let first = activity(&home, "first");
    let second = activity(&home, "second");
    assert_eq!(first.total, 1);
    assert_eq!(first.steps[0].ownership, second.steps[0].ownership);
    assert!(matches!(first.steps[0].ownership, Ownership::Shared(_)));
}

#[test]
fn activity_copy_resume_tool_calls_are_counted_once_in_step_facts() {
    let home = Home::new();
    let history = [
        uuid(human("old", ts(0, 0), "synthetic prompt"), "prompt-record"),
        uuid(
            assistant(
                "old",
                ts(0, 1),
                vec![tool("copied-call", "Bash", json!({"command":"git status"}))],
            ),
            "tool-record",
        ),
        uuid(
            result(
                "old",
                ts(0, 2),
                "copied-call",
                "synthetic output",
                false,
                json!({"exit_code":0,"output":"synthetic output"}),
            ),
            "result-record",
        ),
    ];
    let mut original = history.to_vec();
    original.push(json!({
        "type":"continued-in",
        "continuedInSessionId":"new",
        "sessionId":"old",
        "timestamp":ts(0, 3)
    }));
    home.top("old", &original);
    let mut resumed = history.to_vec();
    resumed.push(uuid(human("new", ts(0, 4), "after resume"), "next-prompt"));
    home.top("new", &resumed);
    home.build_query();

    let (_, rows) = read_catalog(&home);
    let row = rows
        .iter()
        .find(|row| row.harness == "claude" && row.sources.len() == 2)
        .unwrap();
    let page = activity(&home, &row.key);
    assert_eq!(page.total, 1);
    let c = rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    let facts = session_activity::step_facts(&c, row, &page.generation)
        .unwrap()
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].2.tool_calls.len(), 1);
    assert_eq!(facts[0].2.tool_ms, Some(60_000));
}

#[test]
fn activity_codex_uses_only_accepted_native_response_facts_across_restart() {
    let home = Home::new();
    let payload = json!({"thread_id":"native","session_id":"native","turn_id":"turn","response_id":"response","usage":{"input_tokens":31,"output_tokens":7,"total_tokens":38,"cached_input_tokens":3,"cache_write_input_tokens":2,"reasoning_output_tokens":0}});
    let accepted = codex_line(ts(0, 1), "token_usage_record", payload.clone());
    let mut conflict = payload.clone();
    conflict["usage"]["input_tokens"] = json!(99);
    home.codex(
        "native",
        json!({}),
        &[
            codex_line(ts(0, 0), "turn_context", json!({"model":"gpt-test"})),
            accepted.clone(),
            accepted.clone(),
            codex_line(ts(0, 1), "token_usage_record", conflict),
        ],
    );
    home.build_query();
    let initial = activity(&home, "native");
    assert_eq!(initial.total, 1);
    assert_eq!(initial.steps[0].tokens.input, 28);
    assert_eq!(initial.steps[0].tokens.cache_read, 3);
    assert!(matches!(initial.steps[0].id, ActivityStepId::Codex(_)));
    home.build_query();
    assert_eq!(activity(&home, "native"), initial);
    let path = home
        .root
        .join("codex/sessions/2026/09/24/rollout-native.jsonl");
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{accepted}").unwrap();
    drop(file);
    home.build_query();
    let appended = activity(&home, "native");
    assert_eq!(appended.steps, initial.steps);
    assert_ne!(
        appended.generation, initial.generation,
        "consumed source generation advances even for duplicate facts"
    );
    let c = rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    let outcomes: Vec<String> = c
        .prepare("SELECT model_outcome FROM native_usage_facts ORDER BY native_offset")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(outcomes, ["accepted", "duplicate", "conflict", "duplicate"]);
}

#[test]
fn activity_request_blob_replay_preserves_change_fact_and_read_step_coverage() {
    for (name, input, output, change) in [
        (
            "Bash",
            json!({"command":"cd /synthetic && gh pr create --title synthetic --body-file /synthetic/body"}),
            "https://github.com/example/project/pull/42",
            true,
        ),
        (
            "Bash",
            json!({"command":"git add file && git commit -m synthetic"}),
            "[topic abc1234] synthetic",
            true,
        ),
        (
            "Read",
            json!({"file_path":"/synthetic/file","offset":1,"limit":5}),
            "synthetic body",
            false,
        ),
    ] {
        let home = Home::new();
        let mut records = vec![human("native", ts(0, 0), "Synthetic prompt")];
        for minute in 1..=2 {
            let call =
                json!({"type":"tool_use","id":format!("call-{minute}"),"name":name,"input":input});
            let mut assistant = assistant_usage(
                "native",
                ts(0, minute),
                &format!("message-{minute}"),
                "claude-sonnet-4-6",
                json!({"input_tokens":10,"output_tokens":2}),
            );
            assistant["message"]["content"] = json!([call.clone()]);
            records.push(assistant);
            records.push(json!({"type":"api-request-blob","sessionId":"native","timestamp":ts(0,minute),"message":{"role":"assistant","content":[call]}}));
            records.push(json!({"type":"user","sessionId":"native","timestamp":ts(0,minute),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("call-{minute}"),"content":output}]}}));
        }
        home.top("native", &records);
        home.build_query();
        let page = activity(&home, "native");
        assert_eq!(page.total, 2);
        if change {
            let (_, rows) = read_catalog(&home);
            let db =
                rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
            let facts = session_activity::items(
                &db,
                &rows[0],
                None,
                &crate::pricing::snapshot().fingerprint,
                true,
                None,
                256,
            )
            .unwrap()
            .unwrap();
            assert_eq!(facts.items.len(), 2, "{name} {input}");
            assert!(facts.items.iter().all(|fact| !fact["step_id"].is_null()));
            assert!(
                facts
                    .items
                    .iter()
                    .all(|fact| fact.get("target_path").is_none())
            );
            let steps = session_activity::items(
                &db,
                &rows[0],
                None,
                &crate::pricing::snapshot().fingerprint,
                false,
                None,
                256,
            )
            .unwrap()
            .unwrap();
            assert!(
                steps
                    .items
                    .iter()
                    .all(|step| step.get("facts").is_none() && step.get("target_path").is_none())
            );
        } else {
            assert_eq!(
                page.steps[1].waste,
                Some(crate::events::session_activity::classify::WasteKind::RedundantRead)
            );
        }
        home.build_query();
        assert_eq!(activity(&home, "native"), page);
    }
}
