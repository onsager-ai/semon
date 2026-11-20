use super::*;
use crate::events::{
    EventCache, scan_file,
    warehouse::{Fact, Observation},
};
use rusqlite::types::Value as Sql;
use std::io::Write;

fn rows(db: &Connection, sql: &str) -> Vec<Vec<Sql>> {
    let mut statement = db.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn string(value: &Option<String>) -> Sql {
    value
        .as_ref()
        .map_or(Sql::Null, |value| Sql::Text(value.clone()))
}

fn time(value: Option<i64>) -> Sql {
    value.map_or(Sql::Null, Sql::Integer)
}

fn integer(value: u64) -> Sql {
    Sql::Integer(i64::try_from(value).unwrap())
}

fn text(value: &str) -> Sql {
    Sql::Text(value.to_owned())
}

// Compare the overlapping immutable rows. The prototype attaches replies to
// old events; for that table, join the reply to its immutable observation at
// the original call position before comparing all of the columns.
fn assert_claude_facts(database: &Connection, observations: &[Observation]) {
    let mut facts: BTreeMap<&str, Vec<Vec<Sql>>> = BTreeMap::new();
    for observation in observations {
        let pos = integer(observation.position.offset);
        let block = integer(observation.position.block.into());
        match &observation.fact {
            Fact::Record {
                kind,
                uuid,
                message_id,
                time: ts,
            } => facts.entry("record").or_default().push(vec![
                pos,
                string(kind),
                string(uuid),
                string(message_id),
                time(*ts),
            ]),
            Fact::Relation { kind, target } => facts
                .entry("relation_observation")
                .or_default()
                .push(vec![pos, text(kind), text(target)]),
            Fact::Parent {
                harness,
                id,
                call_id,
                ..
            } => facts.entry("parent_observation").or_default().push(vec![
                pos,
                text("marker"),
                text(harness),
                text(id),
                string(call_id),
            ]),
            Fact::ClaudeUsage {
                message_id,
                model,
                tokens,
                billing,
            } => {
                let uuid = observations
                    .iter()
                    .find_map(|item| match &item.fact {
                        Fact::Record { uuid, .. }
                            if item.position.offset == observation.position.offset =>
                        {
                            Some(string(uuid))
                        }
                        _ => None,
                    })
                    .unwrap();
                facts.entry("step_observation").or_default().push(vec![
                    pos.clone(),
                    text(message_id),
                    uuid,
                    string(model),
                    integer(tokens.input),
                    integer(tokens.cached_input),
                    integer(tokens.output),
                ]);
                facts.entry("step_billing").or_default().push(vec![
                    pos,
                    Sql::Text(serde_json::to_string(billing).unwrap()),
                ]);
            }
            Fact::ToolCall {
                id: Some(id),
                tool,
                target,
                time: ts,
                ..
            } => {
                facts.entry("claude_call").or_default().push(vec![
                    pos.clone(),
                    block.clone(),
                    text(id),
                ]);
                if tool.as_deref() == Some("SendMessage") {
                    facts.entry("relay_send").or_default().push(vec![
                        pos,
                        block,
                        text(id),
                        text(target.as_deref().unwrap_or("")),
                        time(*ts),
                    ]);
                }
            }
            Fact::ToolResult { id, reply, .. } => {
                facts.entry("relay_result").or_default().push(vec![
                    pos,
                    text(id),
                    string(&reply.m),
                    Sql::Integer(i64::from(reply.e || reply.f & (DENIED | SEND_FAILED) != 0)),
                ])
            }
            Fact::Event {
                kind,
                id,
                name,
                flags,
                time: ts,
                peer,
                ..
            } => {
                if *kind == Kind::Xsm {
                    facts.entry("relay_receipt").or_default().push(vec![
                        pos.clone(),
                        string(id),
                        Sql::Text(plain_name(name.as_deref().unwrap_or(""))),
                        peer.as_ref().and_then(|p| p.pid).map_or(Sql::Null, integer),
                        peer.as_ref().map_or(Sql::Null, |p| string(&p.start)),
                        peer.as_ref().map_or(Sql::Null, |p| string(&p.sock)),
                    ]);
                }
                let reply = observations.iter().find_map(|item| match &item.fact {
                    Fact::ToolResult {
                        call: Some(call),
                        reply,
                        ..
                    } if *kind == Kind::Tool && *call == observation.position => Some(reply),
                    _ => None,
                });
                facts.entry("subagent_event").or_default().push(vec![
                    pos,
                    block,
                    text(serde_json::to_value(kind).unwrap().as_str().unwrap()),
                    time(*ts),
                    string(id),
                    string(name),
                    integer((*flags).into()),
                    reply.map_or(Sql::Null, |reply| integer(reply.o)),
                    reply.map_or(Sql::Null, |reply| integer(reply.b.into())),
                    time(reply.and_then(|reply| reply.t)),
                    Sql::Integer(i64::from(reply.is_some_and(|r| r.e))),
                    integer(reply.map_or(0, |r| r.f.into())),
                    reply.map_or(Sql::Null, |reply| string(&reply.m)),
                    peer.as_ref().and_then(|p| p.pid).map_or(Sql::Null, integer),
                    peer.as_ref().map_or(Sql::Null, |p| string(&p.start)),
                    peer.as_ref().map_or(Sql::Null, |p| string(&p.sock)),
                ]);
            }
            _ => {}
        }
    }
    for (table, order) in [
        ("record", "pos"),
        ("relation_observation", "pos,kind"),
        ("parent_observation", "pos,method"),
        ("step_observation", "pos"),
        ("step_billing", "pos"),
        ("claude_call", "pos,block"),
        ("relay_send", "pos,block"),
        ("relay_result", "pos"),
        ("relay_receipt", "pos"),
        ("subagent_event", "event"),
    ] {
        // The prototype has a synthetic source column and (for events) a
        // process-local array index. Neither is part of the parser's key.
        let start = if table == "subagent_event" { 2 } else { 1 };
        let expected = rows(
            database,
            &format!("SELECT * FROM fact_{table} ORDER BY {order}"),
        )
        .into_iter()
        .map(|row| row.into_iter().skip(start).collect())
        .collect::<Vec<Vec<Sql>>>();
        assert_eq!(facts.remove(table).unwrap_or_default(), expected, "{table}");
    }
}

fn assert_private_bodies_absent(observations: &impl std::fmt::Debug, bodies: &[&str]) {
    use sha2::{Digest, Sha256};

    let formatted = format!("{observations:?}");
    assert!(!formatted.contains("PRIVATE-"), "{formatted}");
    for body in bodies {
        let digest = Sha256::digest(body.as_bytes());
        assert!(!formatted.contains(&format!("{digest:x}")), "{formatted}");
        assert!(
            !formatted.contains(&format!("{:?}", &digest[..])),
            "{formatted}"
        );
    }
}

fn capture(cache: &mut EventCache, path: &std::path::Path, harness: &str) -> Vec<Observation> {
    scan_file(path, harness, cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    std::mem::take(cache.warehouse_commits.as_mut().unwrap())
        .into_iter()
        .flat_map(|batch| {
            assert_eq!(batch.harness, harness);
            batch.observations
        })
        .collect()
}

fn append(path: &std::path::Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn claude_observations_match_prototype_at_admission() {
    let mut records = vec![user(
        "sender",
        ts(0, 0),
        "Semon-Parent: codex:parent:launch\nPRIVATE-PROMPT",
    )];
    records.extend(
        include_str!("../../../../tests/fixtures/session-activity/claude.jsonl")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()),
    );
    records.extend([
        json!({"type":"continued-in", "sessionId":"sender", "session_id":"previous", "continuedInSessionId":"next"}),
        json!({"type":"bridge-session", "bridgeSessionId":"bridge-id"}),
        assistant("sender", ts(0, 1), vec![text("PRIVATE-ASSISTANT"), tool("send", "SendMessage", json!({"recipient":"worker [native]", "content":"PRIVATE-ARGUMENT"}))]),
        peer("sender", ts(0, 2), "incoming", "peer [native]", 42, "PRIVATE-RELAY"),
    ]);
    let late = result(
        "sender",
        ts(0, 3),
        "send",
        "PRIVATE-RESULT",
        false,
        json!({"success":true,"msg_id":"outgoing","pin":{"id":"child"},"message":"PRIVATE-RESULT-METADATA"}),
    );
    let mut expected = Warehouse::new(std::path::Path::new(":memory:"));
    let home = Home::new();
    let path = home.top("sender", &records);
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    cache.scan_byte_budget = Some(1);
    let mut observations = Vec::new();
    for (ordinal, record) in records.iter().enumerate() {
        expected.source(1, "claude", "sender", std::slice::from_ref(record));
        let admitted = capture(&mut cache, &path, "claude");
        assert!(!admitted.is_empty());
        for (discriminator, observation) in admitted.iter().enumerate() {
            assert_eq!(observation.ordinal, Some(ordinal as u64));
            assert_eq!(observation.discriminator, discriminator);
        }
        observations.extend(admitted);
        assert_claude_facts(&expected.db, &observations);
    }
    let before = std::fs::metadata(&path).unwrap().len();
    append(&path, late.to_string().as_bytes());
    cache.scan_byte_budget = None;
    assert!(capture(&mut cache, &path, "claude").is_empty());
    append(&path, b"\n{malformed}\n");
    let suffix = capture(&mut cache, &path, "claude");
    assert!(
        suffix
            .iter()
            .any(|observation| matches!(observation.fact, Fact::Gap))
    );
    let reply = suffix
        .iter()
        .find_map(|observation| match &observation.fact {
            Fact::ToolResult {
                id,
                call: Some(call),
                reply,
                ..
            } if id == "send" => Some((observation, call, reply)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (reply.0.position.offset, reply.0.position.block),
        (before, 0)
    );
    assert_eq!(reply.1.block, 1);
    assert!(reply.1.offset < before);
    assert_eq!((reply.2.f, reply.2.m.as_deref()), (PIN, Some("outgoing")));
    assert!(
        matches!(&reply.0.fact, Fact::ToolResult {target: Some(target), ..} if target == "child")
    );
    observations.extend(suffix);
    expected.source(1, "claude", "sender", &[late]);
    assert_claude_facts(&expected.db, &observations);
    let compactions = observations
        .iter()
        .filter_map(|o| match &o.fact {
            Fact::Compact { pre_tokens, .. } => Some(*pre_tokens),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(compactions, [Some(155000), None]);
    let cold_home = Home::new();
    let cold_path = cold_home.write(
        "claude/projects/-work-proj/sender.jsonl",
        &std::fs::read_to_string(path).unwrap(),
    );
    let mut cold = EventCache::open_scoped(&cold_home.options.cache).unwrap();
    cold.scan_byte_budget = Some(u64::MAX);
    cold.warehouse_commits = Some(Vec::new());
    let cold = capture(&mut cold, &cold_path, "claude");
    assert_eq!(cold, observations);
    assert_private_bodies_absent(
        &cold,
        &[
            "PRIVATE-PROMPT",
            "PRIVATE-ASSISTANT",
            "PRIVATE-ARGUMENT",
            "PRIVATE-RELAY",
            "PRIVATE-RESULT",
        ],
    );
}

fn assert_codex_facts(database: &Connection, batch: &crate::events::warehouse::Batch) {
    let mut records = Vec::new();
    let mut calls = Vec::new();
    let mut results = Vec::new();
    let prompts = batch
        .observations
        .iter()
        .filter_map(|o| {
            matches!(o.fact, Fact::Event { kind: Kind::U, .. }).then_some(o.position.offset)
        })
        .collect::<BTreeSet<_>>();
    for observation in &batch.observations {
        let pos = integer(observation.position.offset);
        match &observation.fact {
            Fact::Record {
                kind,
                uuid,
                message_id,
                time: ts,
            } => {
                let kind = if prompts.contains(&observation.position.offset) {
                    text("prompt")
                } else {
                    string(kind)
                };
                records.push(vec![pos, kind, string(uuid), string(message_id), time(*ts)]);
            }
            Fact::ToolCall {
                id: Some(id),
                tool,
                task_name,
                time: ts,
                ..
            } => calls.push(vec![
                pos,
                text(id),
                string(tool),
                string(task_name),
                time(*ts),
            ]),
            Fact::ToolResult {
                kind: "response_output",
                id,
                reply,
                ..
            } => results.push(vec![pos, text(id), time(reply.t)]),
            _ => {}
        }
    }
    for (table, projected) in [
        ("record", records),
        ("tool_call", calls),
        ("tool_result", results),
    ] {
        let expected = rows(
            database,
            &format!("SELECT * FROM fact_{table} ORDER BY pos"),
        )
        .into_iter()
        .map(|row| row.into_iter().skip(1).collect())
        .collect::<Vec<Vec<Sql>>>();
        assert_eq!(projected, expected, "{table}");
    }
    let header = batch.header.as_ref().unwrap();
    assert!(header.complete);
    let meta = serde_json::to_value(&header.metadata).unwrap();
    let metadata = |key: &str| meta[key].as_str().map_or(Sql::Null, text);
    assert_eq!(
        vec![vec![
            text("codex"),
            text(&header.native_id),
            metadata("parent_thread"),
            metadata("nickname"),
            metadata("path")
        ]],
        rows(
            database,
            "SELECT harness,native_id,parent_id,nickname,agent_path FROM fact_session_observation"
        )
    );
}

#[test]
fn codex_observations_match_prototype_at_admission() {
    use crate::codex_lifecycle::TurnPhase;
    use crate::events::warehouse::Batch;

    let mut body = codex_spawn_fixture();
    body[1]["payload"]["arguments"] =
        json!("{\"task_name\":\"solo\",\"message\":\"PRIVATE-CODEX-ARGUMENT\"}");
    let native_usage = codex_line(
        ts(6, 5),
        "token_usage_record",
        json!({
            "thread_id":"root", "session_id":"session", "turn_id":"turn", "response_id":"response",
            "usage":{"input_tokens":7,"output_tokens":2,"total_tokens":9,"cached_input_tokens":3,
                     "cache_write_input_tokens":1,"reasoning_output_tokens":0}
        }),
    );
    body.extend([
        codex_line(
            ts(6, 2),
            "event_msg",
            json!({"type":"task_started", "turn_id":"turn"}),
        ),
        codex_line(ts(6, 3), "turn_context", json!({"model":"gpt-6-sol"})),
        codex_line(
            ts(6, 4),
            "event_msg",
            json!({"type":"token_count","info":{"total_token_usage":{
            "input_tokens":7,"output_tokens":2,"total_tokens":9,"cached_input_tokens":3}}}),
        ),
        native_usage.clone(),
        codex_line(
            ts(6, 6),
            "compacted",
            json!({"replacement_history":["PRIVATE-CODEX-COMPACTION"]}),
        ),
    ]);
    let records = codex_records("root", Some("parent"), Some("worker"), &body);
    let late = codex_line(
        ts(6, 7),
        "response_item",
        json!({"type":"function_call_output",
        "call_id":"only", "output":"PRIVATE-CODEX-OUTPUT"}),
    );
    let terminal = codex_line(
        ts(6, 8),
        "event_msg",
        json!({"type":"task_complete", "turn_id":"turn"}),
    );
    let home = Home::new();
    let path = home.lines("codex/sessions/rollout-root.jsonl", &records);
    let mut expected = Warehouse::new(std::path::Path::new(":memory:"));
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    cache.scan_byte_budget = Some(1);
    let mut batch = None::<Batch>;
    for record in &records {
        expected.source(1, "codex", "root", std::slice::from_ref(record));
        scan_file(&path, "codex", &mut cache, &mut false).unwrap();
        cache.flush_cold_writes().unwrap();
        let mut commits = std::mem::take(cache.warehouse_commits.as_mut().unwrap());
        assert_eq!(commits.len(), 1);
        let new = commits.pop().unwrap();
        if let Some(batch) = &mut batch {
            assert!(new.header.is_none());
            batch.observations.extend(new.observations);
        } else {
            batch = Some(new);
        }
        assert_codex_facts(&expected.db, batch.as_ref().unwrap());
    }
    let mut batch = batch.unwrap();
    let result_offset = std::fs::metadata(&path).unwrap().len();
    drop(cache);
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    append(&path, late.to_string().as_bytes());
    assert!(capture(&mut cache, &path, "codex").is_empty());
    append(
        &path,
        format!("\n{native_usage}\n{terminal}\n[invalid]\n").as_bytes(),
    );
    let suffix = capture(&mut cache, &path, "codex");
    let result = suffix
        .iter()
        .find_map(|o| match &o.fact {
            Fact::ToolResult {
                id,
                call: Some(call),
                reply,
                exit_code,
                ..
            } if id == "only" => Some((o.position.offset, call.offset, reply.f, *exit_code)),
            _ => None,
        })
        .unwrap();
    assert_eq!(result.0, result_offset);
    assert!(result.1 < result_offset);
    assert_eq!((result.2, result.3), (UNKNOWN, None));
    assert!(matches!(suffix.last().unwrap().fact, Fact::Gap));
    batch.observations.extend(suffix);
    expected.source(1, "codex", "root", &[late, native_usage, terminal]);
    assert_codex_facts(&expected.db, &batch);
    let accepted = batch
        .observations
        .iter()
        .filter_map(|o| match &o.fact {
            Fact::CodexUsage {
                tokens,
                model_tokens,
                accepted,
                ..
            } => Some((tokens.input, model_tokens.input, *accepted)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(accepted, [(7, 4, true), (7, 4, false)]);
    assert!(
        batch.observations.iter().any(
            |o| matches!(&o.fact, Fact::CodexTokenSnapshot { tokens, .. } if tokens.input == 7)
        )
    );
    assert!(
        batch
            .observations
            .iter()
            .any(|o| matches!(o.fact, Fact::Compact { .. }))
    );
    let turns = batch
        .observations
        .iter()
        .filter_map(|o| match &o.fact {
            Fact::CodexTurn { phase, .. } => Some(phase.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(turns, [TurnPhase::Active, TurnPhase::Completed]);
    let cold = Home::new();
    let cold_path = cold.write(
        "codex/sessions/rollout-root.jsonl",
        &std::fs::read_to_string(path).unwrap(),
    );
    let mut cache = EventCache::open_scoped(&cold.options.cache).unwrap();
    cache.scan_byte_budget = Some(u64::MAX);
    cache.warehouse_commits = Some(Vec::new());
    scan_file(&cold_path, "codex", &mut cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    let cold = cache.warehouse_commits.as_mut().unwrap().pop().unwrap();
    assert_eq!(cold.header, batch.header);
    assert_eq!(cold.observations, batch.observations);
    assert_private_bodies_absent(
        &cold,
        &[
            "PRIVATE-CODEX-ARGUMENT",
            "PRIVATE-CODEX-COMPACTION",
            "PRIVATE-CODEX-OUTPUT",
        ],
    );
}

#[test]
fn claude_sidecar_observations_follow_the_verified_sidecar_revision() {
    use crate::events::warehouse::Sidecar;

    let home = Home::new();
    let log = home.lines(
        "claude/projects/-work-proj/parent/subagents/agent-child.jsonl",
        &[human("child", ts(0, 0), "PRIVATE-SIDECAR-PROMPT")],
    );
    let metadata = log.with_extension("meta.json");
    std::fs::write(
        &metadata,
        r#"{"toolUseId":"launch","agentType":"worker","ignored":"PRIVATE-SIDECAR"}"#,
    )
    .unwrap();
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    scan_file(&log, "claude", &mut cache, &mut false).unwrap();
    let initial = cache.warehouse_commits.as_mut().unwrap().pop().unwrap();
    assert!(
        matches!(&initial.sidecar, Some(Sidecar::Present(meta)) if meta.tool_use_id.as_deref() == Some("launch"))
    );
    assert!(!format!("{:?}", initial.sidecar).contains("PRIVATE-"));

    std::fs::write(&metadata, r#"{"toolUseId":"replacement"}"#).unwrap();
    scan_file(&log, "claude", &mut cache, &mut false).unwrap();
    let changed = cache.warehouse_commits.as_mut().unwrap().pop().unwrap();
    assert!(changed.observations.is_empty());
    assert!(
        matches!(&changed.sidecar, Some(Sidecar::Present(meta)) if meta.tool_use_id.as_deref() == Some("replacement"))
    );

    std::fs::remove_file(&metadata).unwrap();
    scan_file(&log, "claude", &mut cache, &mut false).unwrap();
    let removed = cache.warehouse_commits.as_mut().unwrap().pop().unwrap();
    assert!(removed.observations.is_empty());
    assert_eq!(removed.sidecar, Some(Sidecar::Absent));
}

#[test]
fn codex_observes_relay_and_connector_metadata() {
    let fixture = codex_records(
        "root",
        None,
        None,
        &[
            codex_line(
                ts(0, 1),
                "response_item",
                json!({"type":"function_call","call_id":"send",
            "name":"collaboration.send_message","arguments":"{\"target\":\"root/worker\",\"message\":\"PRIVATE-RELAY-BODY\"}"}),
            ),
            codex_line(
                ts(0, 2),
                "response_item",
                json!({"type":"agent_message","author":"root/worker",
            "recipient":"root","content":"PRIVATE-AGENT-BODY"}),
            ),
            codex_line(
                ts(0, 3),
                "event_msg",
                json!({"type":"item_completed","item":{
            "id":"connector", "type":"McpToolCall", "server":"svc", "tool":"lookup",
            "status":"completed", "arguments":{"query":"PRIVATE-CONNECTOR-ARGS"},
            "result":{"isError":false,"content":"PRIVATE-CONNECTOR-RESULT"}}}),
            ),
            codex_line(
                ts(0, 4),
                "event_msg",
                json!({"type":"item_completed","item":{
            "id":"command", "type":"CommandExecution", "exit_code":2,
            "aggregated_output":"PRIVATE-COMMAND-OUTPUT"}}),
            ),
        ],
    );
    let home = Home::new();
    let path = home.lines("codex/sessions/rollout-root.jsonl", &fixture);
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    let observations = capture(&mut cache, &path, "codex");
    let any = |predicate: fn(&Fact) -> bool| observations.iter().any(|o| predicate(&o.fact));
    assert!(any(
        |f| matches!(f, Fact::ToolCall {id: Some(id), target: Some(target), ..}
        if id == "send" && target == "root/worker")
    ));
    assert!(any(
        |f| matches!(f, Fact::Event {kind: Kind::Agm, id: Some(id), target: Some(target), flags: crate::events::AGM_UP, ..}
        if id == "root/worker" && target == "root")
    ));
    assert!(any(
        |f| matches!(f, Fact::ToolCall {id: Some(id), tool: Some(tool), ..}
        if id == "connector" && tool == "svc.lookup")
    ));
    assert!(any(
        |f| matches!(f, Fact::ToolResult {id, kind: "connector", reply, ..}
        if id == "connector" && !reply.e && reply.f == 0)
    ));
    assert!(any(
        |f| matches!(f, Fact::ToolResult {id, call: None, exit_code: Some(2), reply, ..}
        if id == "command" && reply.e)
    ));
    assert!(!format!("{observations:?}").contains("PRIVATE-"));
}

#[test]
fn a_provisional_codex_header_is_independent_of_complete_record_admission() {
    let home = Home::new();
    let header = codex_records("native", Some("parent"), Some("worker"), &[]);
    let path = home.write(
        "codex/sessions/rollout-native.jsonl",
        &header[0].to_string(),
    );
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    scan_file(&path, "codex", &mut cache, &mut false).unwrap();
    let provisional = cache.warehouse_commits.as_mut().unwrap().pop().unwrap();
    assert!(provisional.observations.is_empty());
    assert!(
        provisional
            .header
            .is_some_and(|header| !header.complete && header.native_id == "native")
    );

    append(&path, b"\n");
    scan_file(&path, "codex", &mut cache, &mut false).unwrap();
    let complete = cache.warehouse_commits.as_mut().unwrap().pop().unwrap();
    assert!(
        complete
            .header
            .is_some_and(|header| header.complete && header.native_id == "native")
    );
    assert_eq!(complete.observations.len(), 1);
    assert_eq!(complete.observations[0].ordinal, Some(0));
}
