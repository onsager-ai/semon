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
    let mut records = vec![human("sender", ts(0, 0), "PRIVATE-PROMPT")];
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
    expected.source(1, "claude", "sender", &records);
    let home = Home::new();
    let path = home.top("sender", &records);
    let mut cache = EventCache::open_scoped(&home.options.cache).unwrap();
    cache.warehouse_commits = Some(Vec::new());
    cache.scan_byte_budget = Some(1);
    let mut observations = Vec::new();
    for _ in &records {
        observations.extend(capture(&mut cache, &path, "claude"));
    }
    assert_claude_facts(&expected.db, &observations);
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
    let formatted = format!("{cold:?}");
    assert!(!formatted.contains("PRIVATE-"), "{formatted}");
    assert!(!formatted.contains("sha256"), "{formatted}");
}
