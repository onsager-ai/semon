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

// Compare committed rows with the independent prototype projections. Native
// kinds, malformed positions and additional metadata stay in production rows.
fn assert_committed_facts(home: &Home, expected: &Connection, harness: &str) {
    let db = Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let sorted = |mut values: Vec<Vec<Sql>>| {
        values.sort_by_cached_key(|row| format!("{row:?}"));
        values
    };
    let queries: &[(&str, &str, &str)] = match harness {
        "claude" => &[
            ("record", "pos,kind,uuid,message_id,ts", "WHERE gap=0"),
            ("relation_observation", "pos,kind,target", ""),
            (
                "parent_observation",
                "pos,method,parent_harness,parent_id,call_id",
                "",
            ),
            (
                "step_observation",
                "pos,message_id,uuid,model,input,cached,output",
                "",
            ),
            ("step_billing", "pos,billing", ""),
            ("claude_call", "DISTINCT pos,block,call_id", ""),
            ("relay_send", "pos,block,call_id,target,ts", ""),
            ("relay_result", "pos,call_id,correlation,failed", ""),
            ("relay_receipt", "pos,correlation,name,pid,start,sock", ""),
        ],
        "codex" => &[
            ("record", "pos,kind,uuid,message_id,ts", "WHERE gap=0"),
            (
                "tool_call",
                "pos,call_id,tool,task_name,ts",
                "WHERE call_id IS NOT NULL",
            ),
            (
                "tool_result",
                "pos,call_id,ts",
                "WHERE kind='response_output'",
            ),
        ],
        _ => unreachable!(),
    };
    for &(table, projection, filter) in queries {
        let production = if harness == "codex" && table == "record" {
            "SELECT r.pos,CASE WHEN EXISTS(SELECT 1 FROM fact_prompt p WHERE p.source=r.source AND p.pos=r.pos) THEN 'prompt' ELSE r.kind END,r.uuid,r.message_id,r.ts FROM fact_record r WHERE gap=0".to_owned()
        } else {
            format!("SELECT {projection} FROM fact_{table} {filter}")
        };
        let prototype_projection = projection.strip_prefix("DISTINCT ").unwrap_or(projection);
        assert_eq!(
            sorted(rows(&db, &production)),
            sorted(rows(
                expected,
                &format!("SELECT {prototype_projection} FROM fact_{table}")
            )),
            "{harness} {table}"
        );
    }
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
        // Rows are a set per key: the parser's emission order within one
        // position need not match the SQL ORDER BY tie-break.
        let sorted = |mut rows: Vec<Vec<Sql>>| {
            rows.sort_by_cached_key(|row| format!("{row:?}"));
            rows
        };
        assert_eq!(
            sorted(facts.remove(table).unwrap_or_default()),
            sorted(expected),
            "{table}"
        );
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
        assistant("sender", ts(0, 1), vec![crate::model::tests::text("PRIVATE-ASSISTANT"), tool("send", "SendMessage", json!({"recipient":"worker [native]", "content":"PRIVATE-ARGUMENT"}))]),
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
        assert_committed_facts(&home, &expected.db, "claude");
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
    assert_committed_facts(&home, &expected.db, "claude");
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
        assert_committed_facts(&home, &expected.db, "codex");
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
    assert_committed_facts(&home, &expected.db, "codex");
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

// Deliberately rescans all active edges. The writer never uses this oracle.
fn assert_current_lineages(db: &Connection) {
    let expected = rows(db, "
        WITH RECURSIVE edges(a,b) AS (
            SELECT d.native_id,e.target FROM dim_session d JOIN fact_relation_observation e USING(source)
            WHERE d.lineage AND e.kind IN ('continued-in','session_id','bridge')
        ), reach(origin,node) AS (
            SELECT source,native_id FROM dim_session WHERE lineage
            UNION SELECT r.origin,e.b FROM reach r JOIN edges e ON e.a=r.node
            UNION SELECT r.origin,e.a FROM reach r JOIN edges e ON e.b=r.node
        ), member(origin,source) AS (
            SELECT r.origin,d.source FROM reach r JOIN dim_session d ON d.lineage AND d.native_id=r.node
            UNION SELECT source,source FROM dim_session
        ), ranked AS (
            SELECT m.origin,min(m.source) OVER(PARTITION BY m.origin) AS component,m.source,
                row_number() OVER(PARTITION BY m.origin ORDER BY d.first_ts IS NULL,d.first_ts,d.continued DESC,d.native_id,d.source) AS rank
            FROM member m JOIN dim_session d USING(source)
        ) SELECT origin,component,source FROM ranked WHERE rank=1 ORDER BY origin");
    assert_eq!(
        rows(
            db,
            "SELECT source,component,owner FROM dim_lineage JOIN membership USING(source) ORDER BY source"
        ),
        expected
    );
    assert_eq!(
        rows(
            db,
            "SELECT source FROM dim_session WHERE source NOT IN (SELECT source FROM warehouse_current)"
        ),
        Vec::<Vec<Sql>>::new()
    );
}

#[test]
fn warehouse_random_append_splits_and_replacement_agree_with_full_recompute() {
    for mut random in [19_u64, 352, 12981] {
        let home = Home::new();
        let inputs = [
            (
                "one",
                vec![
                    human("one", ts(4, 0), "a"),
                    json!({"type":"continued-in","continuedInSessionId":"two"}),
                ],
            ),
            (
                "two",
                vec![
                    human("two", ts(3, 0), "b"),
                    json!({"type":"bridge-session","bridgeSessionId":"bridge"}),
                ],
            ),
            (
                "three",
                vec![
                    human("three", ts(2, 0), "c"),
                    json!({"type":"bridge-session","bridgeSessionId":"bridge"}),
                    human("three", ts(0, 0), "out of order"),
                ],
            ),
            ("four", vec![human("four", ts(1, 0), "unrelated")]),
        ];
        let paths = inputs.each_ref().map(|(id, _)| home.top(id, &[]));
        let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
        let db = Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let mut offsets = [0; 4];
        let next = |random: &mut u64| {
            *random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            (*random >> 32) as usize
        };
        while offsets
            .iter()
            .zip(&inputs)
            .any(|(i, (_, records))| *i < records.len())
        {
            let i = next(&mut random) % inputs.len();
            let start = offsets[i];
            let records = &inputs[i].1;
            if start == records.len() {
                continue;
            }
            let end = (start + 1 + next(&mut random) % 2).min(records.len());
            for record in &records[start..end] {
                append(&paths[i], format!("{record}\n").as_bytes());
            }
            scan_file(&paths[i], "claude", &mut cache, &mut false).unwrap();
            offsets[i] = end;
            assert_current_lineages(&db);
        }
        assert_eq!(
            rows(&db, "SELECT count(*) FROM lineage_group"),
            vec![vec![Sql::Integer(2)]]
        );
        db.execute_batch("CREATE TRIGGER keep_unrelated BEFORE DELETE ON lineage_member WHEN OLD.source=(SELECT source FROM dim_session WHERE native_id='four') BEGIN SELECT RAISE(ABORT,'unrelated lineage changed'); END").unwrap();
        std::fs::write(&paths[1], format!("{}\n", inputs[1].1[0])).unwrap();
        scan_file(&paths[1], "claude", &mut cache, &mut false).unwrap();
        assert_current_lineages(&db);
        assert_eq!(
            rows(&db, "SELECT count(*) FROM lineage_group"),
            vec![vec![Sql::Integer(3)]]
        );
        assert_eq!(
            rows(
                &db,
                "SELECT incarnation FROM warehouse_source WHERE carrier LIKE '%/two.jsonl' ORDER BY incarnation"
            ),
            vec![vec![Sql::Integer(1)], vec![Sql::Integer(2)]]
        );
    }
}

#[test]
fn warehouse_spawn_placement_follows_late_parent_and_revokes_duplicate_calls() {
    let home = Home::new();
    let child = home.lines(
        "codex/sessions/rollout-child.jsonl",
        &codex_records("child", Some("parent"), Some("worker"), &[]),
    );
    let spawn = |id: &str| {
        codex_line(
            ts(0, 1),
            "response_item",
            json!({"type":"function_call","call_id":id,"name":"spawn_agent","arguments":"{\"task_name\":\"worker\"}"}),
        )
    };
    let parent = home.lines(
        "codex/sessions/rollout-parent.jsonl",
        &codex_records("parent", None, None, &[spawn("one")]),
    );
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    let db = Connection::open(EventCache::path(&home.options.cache)).unwrap();
    scan_file(&child, "codex", &mut cache, &mut false).unwrap();
    assert!(rows(&db, "SELECT child FROM spawn_placement").is_empty());
    scan_file(&parent, "codex", &mut cache, &mut false).unwrap();
    assert_eq!(
        rows(&db, "SELECT child FROM spawn_placement"),
        vec![vec![text("child")]]
    );
    append(&parent, format!("{}\n", spawn("two")).as_bytes());
    scan_file(&parent, "codex", &mut cache, &mut false).unwrap();
    assert!(rows(&db, "SELECT child FROM spawn_placement").is_empty());
}

#[test]
fn warehouse_verified_codex_relocation_keeps_fact_keys_through_append() {
    let home = Home::new();
    home.codex(
        "codex-move",
        json!({}),
        &[codex_user(ts(0, 1), "before move")],
    );
    let old = home
        .options
        .codex_home
        .join("sessions/2026/09/24/rollout-codex-move.jsonl");
    let new = home
        .options
        .codex_home
        .join("archived_sessions/rollout-codex-move.jsonl");
    let mut cache = EventCache::open(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut Texts::default(),
        NOW,
    )
    .unwrap();
    let db = Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let current = |path: &std::path::Path| {
        db.query_row(
        "SELECT s.source,s.incarnation,s.logical_source FROM warehouse_current c JOIN warehouse_source s USING(source) WHERE c.path=?1",
        [path.to_str().unwrap()], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)),
    ).unwrap()
    };
    let original = current(&old);
    let count = || rows(&db, "SELECT count(*) FROM fact_record");
    assert_eq!(count(), vec![vec![Sql::Integer(2)]]);
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::rename(&old, &new).unwrap();
    append(
        &new,
        format!("{}\n", codex_user(ts(0, 2), "after move")).as_bytes(),
    );
    scan_file(&new, "codex", &mut cache, &mut false).unwrap();
    assert_eq!(current(&new), original);
    assert_eq!(count(), vec![vec![Sql::Integer(3)]]);
    let selected = db
        .prepare(crate::events::store::warehouse::DETAIL)
        .unwrap()
        .query_map(["codex-move"], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(selected, vec![original.0; 3]);
    cache.retain(&BTreeSet::new(), &mut false);
    assert!(rows(&db, "SELECT * FROM warehouse_current").is_empty());
    assert!(rows(&db, "SELECT * FROM lineage_member").is_empty());
    assert!(rows(&db, "SELECT * FROM files").is_empty());
    assert_eq!(count(), vec![vec![Sql::Integer(3)]]);
}

pub(super) struct ProductionScale {
    dir: std::path::PathBuf,
    cache: EventCache,
}

impl ProductionScale {
    pub(super) fn new(dir: std::path::PathBuf) -> Self {
        std::fs::create_dir_all(&dir).unwrap();
        let mut cache = EventCache::open_scoped(&dir.join("cache.json")).unwrap();
        cache.enable_warehouse_scale_timing();
        cache.scan_byte_budget = Some(8 * 1024 * 1024);
        Self { dir, cache }
    }

    pub(super) fn admit(&mut self, name: &str, harness: &str, records: &[Value]) {
        let path = self.dir.join(format!("{name}.jsonl"));
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
        file.flush().unwrap();
        self.scan(&path, harness);
    }

    pub(super) fn flush(&mut self) {
        self.cache.flush_cold_writes().unwrap();
    }

    // Benchmark a receiver-qualified append. The synthetic writer owns the
    // files and records both stats around its exclusive append, as Raw does.
    fn scan(&mut self, path: &std::path::Path, harness: &str) {
        let size = std::fs::metadata(path).unwrap().len();
        loop {
            scan_file(path, harness, &mut self.cache, &mut false).unwrap();
            assert!(self.cache.persistent());
            if self.cache.revision(path).unwrap().2 == size {
                break;
            }
        }
    }

    pub(super) fn measure_appends(&mut self, case: &str, sources: usize) {
        use crate::events::{Stat, store::warehouse::SCALE_TIMING};
        use crate::source_change::AppendEvidence;
        self.flush();
        self.cache.scan_byte_budget = None;
        let db = Connection::open(EventCache::path(&self.dir.join("cache.json"))).unwrap();
        let paths = (1..=sources)
            .map(|source| self.dir.join(format!("append-{source}.jsonl")))
            .collect::<Vec<_>>();
        let before = self.cache.scan_durations();
        SCALE_TIMING.with(|timing| timing.set(Some((0, 0, 0))));
        let started = Instant::now();
        let mut samples = Vec::with_capacity(10_000);
        for i in 0..10_000 {
            let path = &paths[i % sources];
            let append_start = Instant::now();
            let stat = Stat::of(path).unwrap();
            for _ in 0..1 + i % 5 {
                append(path, b"{\"type\":\"compact_boundary\"}\n");
            }
            let evidence = AppendEvidence {
                generation: format!("{i:032x}"),
                before: stat,
                after: Stat::of(path).unwrap(),
            };
            db.execute(
                "INSERT INTO session_source_mutations VALUES(?1,?2)",
                params![path.to_str(), serde_json::to_string(&evidence).unwrap()],
            )
            .unwrap();
            self.scan(path, "claude");
            samples.push(append_start.elapsed().as_nanos());
        }
        let total = started.elapsed();
        let after = self.cache.scan_durations();
        let (fact_ns, maintenance_ns, admissions) =
            SCALE_TIMING.with(|timing| timing.take().unwrap());
        assert_eq!(admissions, 10_000);
        let first = &mut samples[..1000];
        first.sort_unstable();
        let (first_p50, first_p99) = (first[500], first[990]);
        let last = &mut samples[9000..];
        last.sort_unstable();
        let (last_p50, last_p99) = (last[500], last[990]);
        println!(
            "production append_case={case} count={admissions} total_ms={} parse_ms={} index_ms={} fact_write_ms={} maintenance_ms={} maintenance_mean_us={:.3} first_1000_p50_us={:.3} first_1000_p99_us={:.3} last_1000_p50_us={:.3} last_1000_p99_us={:.3}",
            total.as_millis(),
            (after.0 - before.0).as_millis(),
            (after.1 - before.1).as_millis(),
            fact_ns / 1_000_000,
            maintenance_ns / 1_000_000,
            maintenance_ns as f64 / admissions as f64 / 1000.0,
            first_p50 as f64 / 1000.0,
            first_p99 as f64 / 1000.0,
            last_p50 as f64 / 1000.0,
            last_p99 as f64 / 1000.0
        );
        assert!(
            last_p50 <= 3 * first_p50,
            "production {case}: last p50 {last_p50} ns > 3x first p50 {first_p50} ns"
        );
    }

    pub(super) fn ingest_corpus(home: &Home, corpus: &std::path::Path) {
        use crate::events::store::warehouse::SCALE_TIMING;
        let mut production = Self::new(home.root.join("production-corpus"));
        let started = Instant::now();
        SCALE_TIMING.with(|timing| timing.set(Some((0, 0, 0))));
        for (i, line) in std::io::BufReader::new(File::open(corpus).unwrap())
            .lines()
            .enumerate()
        {
            let envelope: Value = serde_json::from_str(&line.unwrap()).unwrap();
            production.admit(
                envelope["id"].as_str().unwrap(),
                envelope["harness"].as_str().unwrap(),
                envelope["records"].as_array().unwrap(),
            );
            if (i + 1) % 64 == 0 {
                production.flush();
            }
        }
        production.flush();
        let (parse, index) = production.cache.scan_durations();
        let (facts, maintenance, admissions) = SCALE_TIMING.with(|timing| timing.take().unwrap());
        assert_eq!(admissions, 20_000);
        println!(
            "production ingestion_ms={} sessions={admissions} parse_ms={} index_ms={} fact_write_ms={} maintenance_ms={} maintenance_mean_us={:.3}",
            started.elapsed().as_millis(),
            parse.as_millis(),
            index.as_millis(),
            facts / 1_000_000,
            maintenance / 1_000_000,
            maintenance as f64 / admissions as f64 / 1000.0
        );
        let db = Connection::open(EventCache::path(&production.dir.join("cache.json"))).unwrap();
        assert_eq!(
            rows(&db, "SELECT count(*) FROM dim_session"),
            vec![vec![Sql::Integer(20_000)]]
        );
    }

    pub(super) fn seed_long(&mut self, path: &std::path::Path) {
        std::fs::copy(path, self.dir.join("append-1.jsonl")).unwrap();
        self.scan(&self.dir.join("append-1.jsonl"), "claude");
    }
}
