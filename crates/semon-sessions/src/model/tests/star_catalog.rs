//! Deliberately test-only: extract native observations independently of Catalog.
//! SQL chooses positions and metadata; fixture bodies never enter warehouse rows.
use std::{collections::BTreeMap, fs::File, io::BufRead, time::Instant};

use rusqlite::{Connection, params};

use super::*;

const SCHEMA: &str = include_str!("star_catalog/schema.sql");
const MAINTAIN: &str = include_str!("star_catalog/maintain.sql");
const INDEXES: &str = "
CREATE INDEX relation_target ON fact_relation_observation(kind,target,source);";
const LIST: &str = "SELECT id,owner,harness,parent,title_source,title_pos,fallback_title,
 model,status,input,cached,output,shared_input,shared_cached,shared_output
 FROM session_list ORDER BY id";
const DETAIL: &str = include_str!("star_catalog/detail.sql");

struct Warehouse {
    db: Connection,
    // Test analogue of the authorized native body reader, outside SQLite.
    titles: BTreeMap<(i64, i64), String>,
    marker_decided: BTreeMap<i64, bool>,
    maintenance: std::time::Duration,
    appends: u64,
}

#[derive(Debug)]
struct ListRow {
    id: String,
    owner: i64,
    parent: Option<String>,
    title: String,
    model: String,
    status: String,
    tokens: [u64; 3],
    shared: [u64; 3],
}

impl Warehouse {
    fn new(path: &std::path::Path) -> Self {
        let db = Connection::open(path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(include_str!("star_catalog/parents.sql"))
            .unwrap();
        db.execute_batch(include_str!("star_catalog/relays.sql"))
            .unwrap();
        db.execute_batch(include_str!("star_catalog/family.sql"))
            .unwrap();
        db.execute_batch(include_str!("star_catalog/shared_family.sql"))
            .unwrap();
        Self {
            db,
            titles: BTreeMap::new(),
            marker_decided: BTreeMap::new(),
            maintenance: std::time::Duration::ZERO,
            appends: 0,
        }
    }

    fn source(&mut self, source: i64, harness: &str, id: &str, records: &[Value]) {
        let tx = self.db.transaction().unwrap();
        let meta = records
            .iter()
            .find(|v| v["type"] == "session_meta")
            .map(|v| &v["payload"]);
        let attribute = |key: &str| meta.and_then(|v| v[key].as_str());
        tx.execute(
            "INSERT OR IGNORE INTO fact_session_observation VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                source,
                harness,
                id,
                attribute("parent_thread_id"),
                attribute("agent_nickname"),
                attribute("agent_path")
            ],
        )
        .unwrap();
        let mut bytes = tx
            .query_row(
                "SELECT complete_bytes FROM index_advance WHERE source=?1",
                [source],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .unwrap()
            .unwrap_or(0);
        let mut marker_decided = self.marker_decided.get(&source).copied().unwrap_or(false);
        for record in records {
            let pos = bytes;
            bytes += serde_json::to_vec(record).unwrap().len() as i64 + 1;
            let kind = record["type"].as_str().unwrap();
            let uuid = record["uuid"].as_str();
            let message = &record["message"];
            let time = crate::events::record_time(record);
            if !marker_decided {
                if crate::is_agent_output(record) {
                    marker_decided = true;
                } else if let Some(marker) = crate::user_text(record).and_then(crate::parse_marker)
                {
                    marker_decided = true;
                    tx.execute(
                        "INSERT INTO fact_parent_observation VALUES(?1,?2,'marker',?3,?4,?5)",
                        params![
                            source,
                            pos,
                            marker.harness,
                            marker.parent_id,
                            marker.tool_id
                        ],
                    )
                    .unwrap();
                }
            }
            let prompt = if harness == "codex" && record["payload"]["role"] == "user" {
                prompt_text(record).filter(|text| !text.trim().is_empty())
            } else {
                None
            };
            let record_kind = if prompt.is_some() { "prompt" } else { kind };
            if let Some(prompt) = prompt {
                self.titles.insert((source, pos), one_line(&prompt, 80));
            }
            tx.execute(
                "INSERT INTO fact_record VALUES(?1,?2,?3,?4,?5,?6)",
                params![source, pos, record_kind, uuid, message["id"].as_str(), time],
            )
            .unwrap();
            if kind == "custom-title" {
                self.titles.insert(
                    (source, pos),
                    record["customTitle"].as_str().unwrap().to_owned(),
                );
            }
            for (field, relation) in [
                ("continuedInSessionId", "continued-in"),
                ("session_id", "session_id"),
            ] {
                if let Some(target) = record[field].as_str() {
                    tx.execute(
                        "INSERT INTO fact_relation_observation VALUES(?1,?2,?3,?4)",
                        params![source, pos, relation, target],
                    )
                    .unwrap();
                }
            }
            if kind == "bridge-session"
                && let Some(bridge) = record["bridgeSessionId"].as_str()
            {
                tx.execute(
                    "INSERT INTO fact_relation_observation VALUES(?1,?2,'bridge',?3)",
                    params![source, pos, format!("bridge:{bridge}")],
                )
                .unwrap();
            }
            if kind == "assistant"
                && let Some(usage) = message.get("usage")
            {
                let number = |key: &str| usage[key].as_i64().unwrap_or(0);
                let cached = number("cache_read_input_tokens");
                let input = number("input_tokens") + number("cache_creation_input_tokens") + cached;
                tx.execute(
                    "INSERT INTO fact_step_observation VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        source,
                        pos,
                        message["id"].as_str().unwrap(),
                        uuid,
                        message["model"].as_str(),
                        input,
                        cached,
                        number("output_tokens")
                    ],
                )
                .unwrap();
            }
            if harness == "claude" {
                if let Some(content) = message["content"].as_array() {
                    for (block, value) in content.iter().enumerate() {
                        if value["type"] == "tool_use" {
                            tx.execute(
                                "INSERT INTO fact_claude_call VALUES(?1,?2,?3,?4)",
                                params![source, pos, block as i64, value["id"].as_str().unwrap()],
                            )
                            .unwrap();
                        }
                        if value["type"] == "tool_use" && value["name"] == "SendMessage" {
                            tx.execute(
                                "INSERT INTO fact_relay_send VALUES(?1,?2,?3,?4,?5,?6)",
                                params![
                                    source,
                                    pos,
                                    block as i64,
                                    value["id"].as_str().unwrap(),
                                    value["input"]["to"].as_str().unwrap(),
                                    time
                                ],
                            )
                            .unwrap();
                        }
                        if value["type"] == "tool_result" {
                            let result = &record["toolUseResult"];
                            tx.execute(
                                "INSERT INTO fact_relay_result VALUES(?1,?2,?3,?4,?5)",
                                params![
                                    source,
                                    pos,
                                    value["tool_use_id"].as_str().unwrap(),
                                    result["msg_id"].as_str(),
                                    value["is_error"] == true || result["success"] == false
                                ],
                            )
                            .unwrap();
                        }
                    }
                }
                let origin = &record["origin"];
                if origin["kind"] == "peer" {
                    tx.execute(
                        "INSERT INTO fact_relay_receipt VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![
                            source,
                            pos,
                            origin["msg_id"].as_str(),
                            origin["name"].as_str().unwrap_or(""),
                            origin["verifiedPeerPid"].as_i64(),
                            origin["verifiedPeerProcStart"].as_str(),
                            origin["from"].as_str()
                        ],
                    )
                    .unwrap();
                }
            }
            let payload = &record["payload"];
            if payload["type"] == "function_call" {
                let arguments: Value =
                    serde_json::from_str(payload["arguments"].as_str().unwrap()).unwrap();
                tx.execute(
                    "INSERT INTO fact_tool_call VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        source,
                        pos,
                        payload["call_id"].as_str().unwrap(),
                        payload["name"].as_str().unwrap(),
                        arguments["task_name"].as_str(),
                        time
                    ],
                )
                .unwrap();
            }
            if payload["type"] == "function_call_output" {
                tx.execute(
                    "INSERT INTO fact_tool_result VALUES(?1,?2,?3,?4)",
                    params![source, pos, payload["call_id"].as_str().unwrap(), time],
                )
                .unwrap();
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO index_advance VALUES(?1,?2)",
            params![source, bytes],
        )
        .unwrap();
        self.marker_decided.insert(source, marker_decided);
        let maintenance = Instant::now();
        Self::recompute(&tx, source);
        self.maintenance += maintenance.elapsed();
        self.appends += 1;
        tx.commit().unwrap();
    }

    // Capture qualified observations, including epochs that become retained
    // evidence after the process exits. No chosen parent is written here.
    fn process(
        &self,
        epoch: i64,
        source: i64,
        pid: i64,
        qualified: bool,
        ancestors: &[i64],
        environment: Option<(&str, &str)>,
    ) {
        self.db
            .execute(
                "INSERT INTO fact_process_observation VALUES(?1,?2,?3,NULL,?4)",
                params![epoch, source, pid, qualified],
            )
            .unwrap();
        for (ordinal, ancestor) in ancestors.iter().enumerate() {
            self.db
                .execute(
                    "INSERT INTO fact_ancestor_observation VALUES(?1,?2,?3,?4)",
                    params![epoch, source, ordinal as i64, ancestor],
                )
                .unwrap();
        }
        if let Some((harness, parent)) = environment {
            self.db
                .execute(
                    "INSERT INTO fact_environment_observation VALUES(?1,?2,?3,?4,NULL)",
                    params![epoch, source, harness, parent],
                )
                .unwrap();
        }
    }

    fn marker_handoff(&self, child: i64, records: &BTreeMap<i64, Vec<Value>>) -> Option<String> {
        let (parent, call): (i64, Option<String>) = self
            .db
            .query_row(
                "SELECT parent,call_id FROM parent_selection WHERE child=?1",
                [child],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap()?;
        if call.is_some() {
            return call;
        }
        let marker = records[&child]
            .iter()
            .find_map(|r| crate::user_text(r).and_then(crate::parse_marker))?;
        let prompt = marker.handoff?;
        let start = records[&child]
            .iter()
            .filter_map(crate::events::record_time)
            .min();
        let mut matcher = crate::handoff::Matcher::new(&prompt, Some("/work/proj"), start);
        for record in &records[&parent] {
            if let Some(content) = record["message"]["content"].as_array() {
                for block in content {
                    if block["type"] != "tool_use" {
                        continue;
                    }
                    let call = crate::handoff::Call {
                        tool: crate::ToolCall {
                            id: block["id"].as_str().unwrap().into(),
                            name: block["name"].as_str().unwrap().into(),
                        },
                        time: crate::events::record_time(record),
                        input: block["input"].to_string(),
                    };
                    if let Some(tool) = matcher.consider(&call) {
                        return Some(tool.id);
                    }
                }
            }
        }
        matcher.finish().map(|tool| tool.id)
    }

    fn recompute(db: &Connection, source: i64) {
        let after = db
            .query_row(
                "SELECT pos FROM maintenance_cursor WHERE source=?1",
                [source],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .unwrap()
            .unwrap_or(-1);
        for sql in MAINTAIN.split(';').filter(|sql| !sql.trim().is_empty()) {
            let mut statement = db.prepare(sql).unwrap();
            for (name, value) in [(":source", source), (":after", after)] {
                if let Some(index) = statement.parameter_index(name).unwrap() {
                    statement.raw_bind_parameter(index, value).unwrap();
                }
            }
            statement.raw_execute().unwrap();
        }
        db.execute(
            "INSERT OR REPLACE INTO maintenance_cursor
             SELECT ?1,max(pos) FROM (
               SELECT ?2 AS pos UNION ALL
               SELECT max(pos) FROM fact_record WHERE source=?1 UNION ALL
               SELECT max(pos) FROM fact_relation_observation WHERE source=?1 UNION ALL
               SELECT max(pos) FROM fact_step_observation WHERE source=?1 UNION ALL
               SELECT max(pos) FROM fact_tool_call WHERE source=?1
             )",
            params![source, after],
        )
        .unwrap();
    }

    fn list(&self) -> Vec<ListRow> {
        self.db
            .prepare(LIST)
            .unwrap()
            .query_map([], |row| {
                let title_source: Option<i64> = row.get(4)?;
                let title_pos: Option<i64> = row.get(5)?;
                let fallback: String = row.get(6)?;
                let title = title_source
                    .zip(title_pos)
                    .map(|position| self.titles[&position].clone())
                    .unwrap_or_else(|| fallback.rsplit('/').next().unwrap().to_owned());
                let raw_model: String = row.get(7)?;
                let tokens = |column: usize| -> rusqlite::Result<u64> {
                    let value: i64 = row.get(column)?;
                    Ok(u64::try_from(value).expect("fixture token totals are nonnegative"))
                };
                Ok(ListRow {
                    id: row.get(0)?,
                    owner: row.get(1)?,
                    parent: row.get(3)?,
                    title,
                    model: pretty_model(&raw_model).unwrap(),
                    status: row.get(8)?,
                    tokens: [tokens(9)?, tokens(10)?, tokens(11)?],
                    shared: [tokens(12)?, tokens(13)?, tokens(14)?],
                })
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn detail(&self, id: &str) -> Vec<(i64, i64, String)> {
        self.db
            .prepare(DETAIL)
            .unwrap()
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

fn codex_records(
    id: &str,
    parent: Option<&str>,
    name: Option<&str>,
    records: &[Value],
) -> Vec<Value> {
    let mut out = vec![json!({"type":"session_meta","timestamp":ts(0,0),"payload":{
        "id":id,"cwd":"/work/proj","originator":"codex_exec",
        "thread_source":if parent.is_some(){"subagent"}else{"user"},
        "parent_thread_id":parent,"agent_nickname":name,
        "agent_path":name.map(|n| format!("root/{n}"))}})];
    out.extend_from_slice(records);
    out
}

#[test]
fn star_catalog_fixture_parity() {
    let home = Home::new();
    let mut warehouse = Warehouse::new(&home.root.join("star.sqlite3"));
    let usage = include_str!("../../../tests/fixtures/session-activity/claude.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let title = |name: &str| json!({"type":"custom-title","customTitle":name});
    // A real /clear copy: timestamps and UUIDs remain those of the old source.
    let mut old = vec![title("Before clear")];
    old.extend(usage.clone());
    old.push(json!({"type":"continued-in","continuedInSessionId":"new","sessionId":"old"}));
    let mut new = vec![title("After clear")];
    new.extend(usage.clone());
    new.push(uuid(human("new", ts(4, 0), "after clear"), "new-prompt"));
    // Same persisted UUID+message across distinct owners has unknown attribution.
    let shared = vec![usage[0].clone(), usage[1].clone(), title("Shared copy")];
    for (source, id, records) in [(1, "old", old), (2, "new", new), (3, "copy", shared)] {
        home.top(id, &records);
        warehouse.source(source, "claude", id, &records);
    }
    let collision = vec![title("Claude collision"), human("same", ts(5, 0), "hi")];
    home.top("same", &collision);
    warehouse.source(4, "claude", "same", &collision);
    for (source, id, parent, name, body) in [
        (5, "same", None, None, vec![codex_user(ts(5, 0), "hi")]),
        (
            6,
            "root",
            None,
            None,
            vec![
                codex_user(ts(6, 0), "coordinate"),
                codex_line(
                    ts(6, 1),
                    "response_item",
                    json!({"type":"function_call","name":"spawn_agent","call_id":"only","arguments":"{\"task_name\":\"solo\"}"}),
                ),
            ],
        ),
        (
            7,
            "child",
            Some("root"),
            Some("solo"),
            vec![codex_user(ts(6, 2), "work")],
        ),
    ] {
        let records = codex_records(id, parent, name, &body);
        home.lines(&format!("codex/sessions/rollout-{id}.jsonl"), &records);
        warehouse.source(source, "codex", id, &records);
    }
    warehouse.db.execute_batch(INDEXES).unwrap();
    let built = home.build_query();
    let rows = warehouse.list();
    assert_eq!(
        rows.iter().map(|r| &r.id).collect::<Vec<_>>(),
        built.sessions.keys().collect::<Vec<_>>()
    );
    for row in &rows {
        let expected = &built.sessions[&row.id];
        assert_eq!(row.parent, expected.parent, "{} parent", row.id);
        assert_eq!(row.title, expected.name, "{} title", row.id);
        assert_eq!(row.model, expected.model, "{} model", row.id);
        assert_eq!(row.status, expected.state, "{} status", row.id);
        let exact = built.facts[&row.id].tokens.as_ref().unwrap();
        assert_eq!(
            row.tokens,
            [exact.input, exact.cached_input, exact.output],
            "{} exact usage",
            row.id
        );
        let million = |v: u64| (v as f64 / 1e6 * 1000.0).round() / 1000.0;
        assert_eq!(
            [
                million(row.tokens[0] - row.tokens[1]),
                million(row.tokens[1]),
                million(row.tokens[2])
            ],
            expected.tokens,
            "{} exclusive usage",
            row.id
        );
        let expected_shared = expected.claude_usage.as_ref().map_or([0; 3], |e| {
            e.shared.values().fold([0; 3], |mut total, t| {
                total[0] += t.input;
                total[1] += t.cached_input;
                total[2] += t.output;
                total
            })
        });
        assert_eq!(row.shared, expected_shared, "{} shared usage", row.id);
        if let Some(evidence) = &expected.claude_usage {
            assert!(evidence.shared_owner.is_none());
            assert!(evidence.fresh.is_none());
        }
        assert_eq!(warehouse.list().len(), rows.len());
    }
    assert_eq!(rows.iter().find(|r| r.id == "old").unwrap().owner, 1);
    assert!(!rows.iter().any(|r| r.id == "new"));
    let detail = warehouse.detail("old");
    // Both source members remain addressable; copied assistant records appear once.
    assert!(detail.iter().any(|r| r.0 == 2));
    assert_eq!(detail.iter().filter(|r| r.2 == "assistant").count(), 4);
    let placement: i64 = warehouse
        .db
        .query_row(
            "SELECT ts FROM spawn_placement WHERE child='child'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(placement, only(&built, "spawn", "root", "child").at);
    assert_eq!(placement, at(6, 1));
    // Replacing the affected component twice must be idempotent.
    Warehouse::recompute(&warehouse.db, 1);
    Warehouse::recompute(&warehouse.db, 1);
    assert_eq!(warehouse.list().len(), rows.len());
    println!(
        "star parity: {} rows; copy owner, parents, titles, models, status, exclusive/shared usage, unique spawn, transcript suppression",
        rows.len()
    );
}

#[test]
fn star_catalog_parent_precedence_parity() {
    for (method, native) in [
        ("marker", Some("native")),
        ("marker", None),
        ("environment", Some("native")),
        ("environment", None),
        ("ancestry", None),
        ("ancestry", Some("native")),
    ] {
        for reverse in [false, true] {
            let home = Home::new();
            let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
            let prompt = if method == "marker" {
                "Semon-Parent: codex:parent:launch\nWork"
            } else {
                "Work"
            };
            let inputs = [
                (
                    1,
                    "parent",
                    codex_records("parent", None, None, &[codex_user(ts(0, 0), "Parent")]),
                ),
                (
                    2,
                    "native",
                    codex_records("native", None, None, &[codex_user(ts(0, 0), "Native")]),
                ),
                (
                    3,
                    "child",
                    codex_records("child", native, None, &[codex_user(ts(0, 1), prompt)]),
                ),
            ];
            let order = if reverse { [2, 1, 0] } else { [0, 1, 2] };
            for index in order {
                let (source, id, records) = &inputs[index];
                home.lines(&format!("codex/sessions/rollout-{id}.jsonl"), records);
                warehouse.source(*source, "codex", id, records);
            }
            let mut facts = crate::Facts {
                version: crate::FACTS_VERSION,
                hostname: "machine".into(),
                codex_locks: BTreeMap::from([("parent".into(), 10), ("child".into(), 20)]),
                ..Default::default()
            };
            if method == "ancestry" {
                facts.process_ancestors.insert(20, vec![10]);
            }
            if method == "environment" {
                facts.runs.insert(
                    20,
                    BTreeMap::from([("SEMON_PARENT".into(), "codex:parent:launch".into())]),
                );
            }
            warehouse.process(1, 1, 10, true, &[], None);
            warehouse.process(
                1,
                3,
                20,
                true,
                if method == "ancestry" { &[10] } else { &[] },
                (method == "environment").then_some(("codex", "parent")),
            );
            let path = home.root.join("facts.json");
            crate::write_facts(&path, &facts).unwrap();
            let options = Options {
                facts: Some(path.clone()),
                ..home.options.clone()
            };
            let expected = home.build_at(&options, NOW);
            let parent = || {
                warehouse.db.query_row(
                "SELECT p.native_id FROM parent_selection selected JOIN dim_session p ON p.source=selected.parent WHERE selected.child=3",[],|r|r.get::<_,String>(0)).unwrap()
            };
            assert_eq!(
                Some(parent()),
                expected.sessions["child"].parent,
                "{method} reverse={reverse}"
            );
            // Previously observed process evidence survives loss of the process.
            facts.codex_locks.clear();
            facts.runs.clear();
            facts.process_ancestors.clear();
            crate::write_facts(&path, &facts).unwrap();
            let retained = home.build_at(&options, NOW + 1);
            assert_eq!(
                Some(parent()),
                retained.sessions["child"].parent,
                "{method} retained"
            );
        }
    }
    let home = Home::new();
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    for (source, id, body) in [
        (1, "a", "Semon-Parent: codex:b\nWork"),
        (2, "b", "Semon-Parent: codex:a\nWork"),
        (3, "stopped", "Work"),
    ] {
        let mut records = codex_records(id, None, None, &[codex_user(ts(0, 0), body)]);
        if id == "stopped" {
            records.push(codex_reply(ts(0, 1), "already answered"));
            records.push(codex_user(ts(0, 2), "Semon-Parent: codex:a\nToo late"));
        }
        home.lines(&format!("codex/sessions/rollout-{id}.jsonl"), &records);
        warehouse.source(source, "codex", id, &records);
    }
    let expected = home.build_query();
    for row in warehouse.list() {
        assert_eq!(
            row.parent, expected.sessions[&row.id].parent,
            "{} cycle/first output",
            row.id
        );
    }
}

#[test]
fn star_catalog_storage_parent_parity() {
    for (storage_call, other_call, expected_parent) in [
        (true, true, "storage"),
        (false, true, "other"),
        (false, false, "storage"),
    ] {
        for reverse in [false, true] {
            let home = Home::new();
            let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
            let records = |id: &str, has_call: bool| {
                let mut out = vec![human(id, ts(0, 0), "work")];
                if has_call {
                    out.push(assistant(
                        id,
                        ts(0, 1),
                        vec![tool("call", "Agent", json!({"prompt":"work"}))],
                    ));
                }
                out
            };
            let inputs = [
                (1, "storage", records("storage", storage_call)),
                (2, "other", records("other", other_call)),
                (
                    3,
                    "agent-child",
                    vec![human("agent-child", ts(0, 2), "work")],
                ),
            ];
            for index in if reverse { [2, 1, 0] } else { [0, 1, 2] } {
                let (source, id, body) = &inputs[index];
                if *source == 3 {
                    home.agent("storage", "agent-child", "call", body);
                } else {
                    home.top(id, body);
                }
                warehouse.source(*source, "claude", id, body);
                if *source == 3 {
                    warehouse
                        .db
                        .execute(
                            "INSERT INTO fact_agent_observation VALUES(3,'storage','call')",
                            [],
                        )
                        .unwrap();
                }
            }
            let built = home.build_query();
            let actual: String = warehouse.db.query_row("SELECT d.native_id FROM parent_selection p JOIN dim_session d ON d.source=p.parent WHERE child=3",[],|r|r.get(0)).unwrap();
            assert_eq!(actual, expected_parent);
            assert_eq!(
                built.sessions["agent-child"].parent.as_deref(),
                Some(actual.as_str())
            );
        }
    }
}

#[test]
fn star_catalog_marker_eof_parity() {
    for newline in [false, true] {
        let home = Home::new();
        let parent = vec![human("parent", ts(0, 0), "parent")];
        home.top("parent", &parent);
        home.codex("child", json!({}), &[]);
        let path = home
            .options
            .codex_home
            .join("sessions/2026/09/24/rollout-child.jsonl");
        let marker = codex_user(ts(0, 1), "Semon-Parent: claude:parent\nWork");
        let bytes = marker.to_string();
        let split = bytes.len() / 2;
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{}", &bytes[..split]).unwrap();
        assert!(
            native_marker_query(&home).sessions["child"]
                .parent
                .is_none()
        );
        assert!(serde_json::from_slice::<Value>(&bytes.as_bytes()[..split]).is_err());
        write!(file, "{}", &bytes[split..]).unwrap();
        if newline {
            writeln!(file).unwrap();
        }
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        warehouse.source(1, "claude", "parent", &parent);
        warehouse.source(
            2,
            "codex",
            "child",
            &codex_records("child", None, None, &[marker]),
        );
        let built = native_marker_query(&home);
        let parent = warehouse
            .list()
            .into_iter()
            .find(|r| r.id == "child")
            .unwrap()
            .parent;
        assert_eq!(parent, built.sessions["child"].parent);
        assert_eq!(parent.as_deref(), Some("parent"));
    }
}

#[test]
fn star_catalog_marker_handoff_parity() {
    let home = Home::new();
    let parent = vec![
        human("parent", ts(0, 0), "launch"),
        assistant(
            "parent",
            ts(0, 1),
            vec![tool(
                "launch",
                "Bash",
                json!({"command":"codex exec --prompt /work/proj/task.txt"}),
            )],
        ),
    ];
    let child = codex_records(
        "child",
        None,
        None,
        &[codex_user(
            ts(0, 2),
            "Semon-Parent: claude:parent\nSemon-Handoff: /work/proj/task.txt\nWork",
        )],
    );
    home.top("parent", &parent);
    home.lines("codex/sessions/rollout-child.jsonl", &child);
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    warehouse.source(2, "codex", "child", &child);
    assert_eq!(
        warehouse
            .db
            .query_row("SELECT count(*) FROM parent_selection", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    warehouse.source(1, "claude", "parent", &parent);
    let expected = home.build_query();
    assert_eq!(expected.sessions["child"].parent.as_deref(), Some("parent"));
    let calls = BTreeMap::from([(1, parent), (2, child)]);
    assert_eq!(warehouse.marker_handoff(2, &calls), Some("launch".into()));
    assert_eq!(only(&expected, "spawn", "parent", "child").at, at(0, 1));
}

#[test]
fn star_catalog_relay_endpoint_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let sender = vec![
            human("sender", ts(0, 0), "send"),
            assistant(
                "sender",
                ts(0, 1),
                vec![tool(
                    "send",
                    "SendMessage",
                    json!({"to":"worker","message":"hello","type":"message"}),
                )],
            ),
            result(
                "sender",
                ts(0, 2),
                "send",
                "sent",
                false,
                json!({"success":true,"msg_id":"message"}),
            ),
        ];
        let receiver = vec![peer(
            "receiver",
            ts(0, 3),
            "message",
            "sender",
            111,
            "hello",
        )];
        let inputs = [(1, "sender", sender), (2, "receiver", receiver)];
        let order = if reverse { [1, 0] } else { [0, 1] };
        for index in order {
            let (source, id, records) = &inputs[index];
            home.top(id, records);
            warehouse.source(*source, "claude", id, records);
            let expected = home.build_query();
            if index == 0 {
                let actual: (i64, Option<i64>, Option<String>, bool) = warehouse
                    .db
                    .query_row(
                        "SELECT sender,receiver,stub,failed FROM relay_endpoints",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .unwrap();
                let relay = expected
                    .handoffs
                    .iter()
                    .find(|h| h.kind == "relay")
                    .unwrap();
                assert_eq!(relay.from, "sender");
                assert!(!actual.3);
                if reverse {
                    assert_eq!(actual.1, Some(2));
                    assert_eq!(relay.to.as_deref(), Some("receiver"));
                } else {
                    assert_eq!(actual.2.as_deref(), Some("name:worker"));
                    assert_eq!(
                        relay.to.as_deref(),
                        Some(stable_id("peer-", "name:worker").as_str())
                    );
                    assert!(relay.unmatched);
                }
            }
        }
        let expected = home.build_query();
        let relay = only(&expected, "relay", "sender", "receiver");
        let receiver: i64 = warehouse
            .db
            .query_row("SELECT receiver FROM relay_endpoints", [], |r| r.get(0))
            .unwrap();
        assert_eq!(receiver, 2);
        assert_eq!(relay.at, at(0, 1));
        assert!(!relay.unmatched);
        // A late duplicate receipt must not steal the first deterministic endpoint.
        // Native source order breaks duplicate receipt ties, not timestamp.
        let duplicate = vec![peer("z-later", ts(0, 4), "message", "sender", 111, "hello")];
        home.top("z-later", &duplicate);
        warehouse.source(3, "claude", "z-later", &duplicate);
        let late = home.build_query();
        assert_eq!(
            warehouse
                .db
                .query_row("SELECT receiver FROM relay_endpoints", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(only(&late, "relay", "sender", "receiver").id, relay.id);
    }
}

#[test]
fn star_catalog_family_usage_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let inputs = [
            (1, "root", None, 10),
            (2, "child", Some("root"), 20),
            (3, "grandchild", Some("child"), 30),
        ];
        let order = if reverse { [2, 1, 0] } else { [0, 1, 2] };
        for index in order {
            let (source, id, parent, tokens) = inputs[index];
            let mut body = vec![human(id, ts(0, source), "Work")];
            if let Some(parent) = parent {
                body[0] = human(
                    id,
                    ts(0, source),
                    &format!("Semon-Parent: claude:{parent}\nWork"),
                );
            }
            body.push(uuid(assistant_usage(id,ts(0,10+source),&format!("m-{id}"),"claude-sonnet-4-6",
                json!({"input_tokens":tokens,"cache_read_input_tokens":2,"output_tokens":tokens/2})),&format!("u-{id}")));
            home.top(id, &body);
            warehouse.source(source, "claude", id, &body);
            let expected = home.build_query();
            for row in warehouse.list() {
                let mut pending = vec![row.id.clone()];
                let mut visited = std::collections::BTreeSet::new();
                let mut totals = [0_u64; 3];
                while let Some(id) = pending.pop() {
                    if !visited.insert(id.clone()) {
                        continue;
                    }
                    let usage = expected.facts[&id].tokens.as_ref().unwrap();
                    totals[0] += usage.input;
                    totals[1] += usage.cached_input;
                    totals[2] += usage.output;
                    pending.extend(
                        expected
                            .sessions
                            .iter()
                            .filter(|(_, s)| s.parent.as_deref() == Some(&id))
                            .map(|(id, _)| id.clone()),
                    );
                }
                let actual: [u64; 3] = warehouse
                    .db
                    .query_row(
                        "SELECT input,cached,output FROM family_usage WHERE root=?1",
                        [row.owner],
                        |r| {
                            Ok([
                                r.get::<_, i64>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, i64>(2)?,
                            ])
                        },
                    )
                    .map(|v| v.map(|n| u64::try_from(n).unwrap()))
                    .unwrap();
                assert_eq!(actual, totals, "{} reverse={reverse}", row.id);
            }
        }
        assert_eq!(
            warehouse
                .db
                .query_row("SELECT input FROM family_usage WHERE root=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            66
        );
    }
}

#[test]
fn star_catalog_shared_family_parity() {
    use sha2::{Digest, Sha256};
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let inputs = [
            (1, "root", None, vec!["u1"]),
            (2, "middle", Some("root"), vec!["u1", "u2"]),
            (3, "leaf", Some("middle"), vec!["u2"]),
        ];
        for index in if reverse { [2, 1, 0] } else { [0, 1, 2] } {
            let (source, id, parent, uuids) = &inputs[index];
            let prompt =
                parent.map_or("Work".into(), |p| format!("Semon-Parent: claude:{p}\nWork"));
            let mut records = vec![human(id, ts(0, *source), &prompt)];
            for (n, uuid_id) in uuids.iter().enumerate() {
                records.push(uuid(
                    assistant_usage(
                        id,
                        ts(0, 10 + n as i64),
                        "shared-message",
                        "claude-sonnet-4-6",
                        json!({"input_tokens":10,"output_tokens":5}),
                    ),
                    uuid_id,
                ));
            }
            home.top(id, &records);
            warehouse.source(*source, "claude", id, &records);
            let built = home.build_query();
            let mut expected = BTreeMap::<String, [u64; 3]>::new();
            for session in built.sessions.values() {
                if let Some(evidence) = &session.claude_usage {
                    for (key, tokens) in &evidence.shared {
                        expected.entry(key.clone()).or_insert([
                            tokens.input,
                            tokens.cached_input,
                            tokens.output,
                        ]);
                    }
                }
            }
            let actual=warehouse.db.prepare("SELECT message_id,evidence_uuid,input,cached,output FROM family_shared_usage WHERE root=1").unwrap()
                .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,[r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?]))).unwrap()
                .map(|row| {let (message,uuid,tokens)=row.unwrap();
                    (format!("{:x}",Sha256::digest(serde_json::to_vec(&(message,uuid)).unwrap())),tokens.map(|v|u64::try_from(v).unwrap()))}).collect::<BTreeMap<_,_>>();
            if built.sessions.contains_key("root") {
                assert_eq!(actual, expected, "reverse={reverse} source={source}");
            }
        }
        assert_eq!(
            warehouse
                .db
                .query_row(
                    "SELECT count(*) FROM family_shared_usage WHERE root=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            warehouse
                .db
                .query_row(
                    "SELECT input FROM family_shared_usage WHERE root=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            10
        );
    }
}

#[test]
fn star_catalog_placeholder_lineage_parity() {
    let home = Home::new();
    let inputs = [
        (
            1,
            "bridge-a",
            vec![
                json!({"type":"bridge-session","bridgeSessionId":"remote"}),
                uuid(human("bridge-a", ts(0, 0), "same text"), "equal"),
            ],
        ),
        (
            2,
            "bridge-b",
            vec![
                json!({"type":"bridge-session","bridgeSessionId":"remote"}),
                uuid(human("bridge-b", ts(0, 1), "same text"), "equal"),
            ],
        ),
        (
            3,
            "alias-a",
            vec![
                json!({"type":"user","timestamp":ts(1,0),"session_id":"missing","message":{"role":"user","content":"one"}}),
            ],
        ),
        (
            4,
            "alias-b",
            vec![
                json!({"type":"user","timestamp":ts(1,1),"session_id":"missing","message":{"role":"user","content":"two"}}),
            ],
        ),
        (
            5,
            "missing",
            vec![human("missing", ts(1, 2), "late source")],
        ),
    ];
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    // Connect through absent endpoints first, then admit the physical endpoint.
    for (source, id, records) in &inputs {
        home.top(id, records);
        warehouse.source(*source, "claude", id, records);
        let built = home.build_query();
        let rows = warehouse.list();
        assert_eq!(
            rows.iter().map(|r| &r.id).collect::<Vec<_>>(),
            built.sessions.keys().collect::<Vec<_>>()
        );
        for row in rows {
            assert_eq!(row.title, built.sessions[&row.id].name, "{} title", row.id);
            assert_eq!(
                row.parent, built.sessions[&row.id].parent,
                "{} parent",
                row.id
            );
        }
    }
    let built = home.build_query();
    assert_eq!(turns_of(&built, "bridge-a").len(), 2);
    assert_eq!(
        warehouse
            .detail("bridge-a")
            .iter()
            .filter(|r| r.2 == "user")
            .count(),
        2,
        "bridge peers must not suppress equal UUIDs"
    );
    assert_eq!(turns_of(&built, "alias-a").len(), 3);
    assert_eq!(
        warehouse
            .detail("alias-a")
            .iter()
            .filter(|r| r.2 == "user")
            .count(),
        3
    );
    let mut reversed = Warehouse::new(std::path::Path::new(":memory:"));
    for (source, id, records) in inputs.iter().rev() {
        reversed.source(*source, "claude", id, records);
    }
    assert_eq!(warehouse.detail("alias-a"), reversed.detail("alias-a"));
    assert_eq!(warehouse.detail("bridge-a"), reversed.detail("bridge-a"));
}

#[test]
fn star_catalog_detail_key_resolution() {
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    for (source, harness, id) in [
        (1, "claude", "same"),
        (2, "codex", "same"),
        (3, "claude", "claude:same"),
        (4, "claude", ":claude:same"),
        (5, "codex", "with:colon"),
    ] {
        warehouse.source(source, harness, id, &[human(id, ts(0, 0), "hello")]);
    }
    let mut statement = warehouse.db.prepare("SELECT id FROM catalog_keys").unwrap();
    let keys = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    for id in keys {
        let expected = warehouse.db
            .prepare("SELECT r.source,r.pos,r.kind FROM session_records r JOIN catalog_keys k ON k.owner=r.owner WHERE k.id=?1 ORDER BY r.ts,r.source,r.pos")
            .unwrap()
            .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap().map(Result::unwrap).collect::<Vec<(i64,i64,String)>>();
        assert_eq!(warehouse.detail(&id), expected, "{id}");
        assert_eq!(expected.len(), 1);
    }
    assert!(warehouse.detail("missing").is_empty());
}

#[test]
fn star_catalog_out_of_order_dimensions() {
    let parent = codex_records(
        "parent",
        None,
        None,
        &[codex_line(
            ts(0, 1),
            "response_item",
            json!({"type":"function_call","name":"spawn_agent","call_id":"spawn","arguments":"{\"task_name\":\"solo\"}"}),
        )],
    );
    let child = codex_records(
        "child",
        Some("parent"),
        Some("solo"),
        &[codex_user(ts(0, 2), "work")],
    );
    let old = vec![
        uuid(human("old", ts(0, 0), "before"), "before"),
        json!({"type":"continued-in","continuedInSessionId":"new"}),
    ];
    let new = vec![uuid(human("new", ts(0, 3), "after"), "after")];
    let inputs = [
        (1, "codex", "parent", parent),
        (2, "codex", "child", child),
        (3, "claude", "old", old),
        (4, "claude", "new", new),
    ];
    let load = |order: &[usize]| {
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        for &index in order {
            let (source, harness, id, records) = &inputs[index];
            warehouse.source(*source, harness, id, records);
        }
        warehouse
    };
    let ordered = load(&[0, 1, 2, 3]);
    let late_parent = load(&[1, 3, 0, 2]);
    // Compare every persisted derived column, not only the final list shape.
    for sql in [
        "SELECT * FROM dim_session ORDER BY source",
        "SELECT * FROM dim_lineage ORDER BY source",
        "SELECT * FROM bridge_session_owner ORDER BY source",
        "SELECT * FROM bridge_spawn_placement ORDER BY child",
        LIST,
    ] {
        let snapshot = |warehouse: &Warehouse| {
            let mut statement = warehouse.db.prepare(sql).unwrap();
            let columns = statement.column_count();
            statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(&ordered), snapshot(&late_parent), "{sql}");
    }
    assert_eq!(ordered.detail("old"), late_parent.detail("old"));
    assert_eq!(
        late_parent
            .list()
            .iter()
            .find(|row| row.id == "child")
            .unwrap()
            .parent,
        Some("parent".to_owned())
    );
    let placement: i64 = late_parent
        .db
        .query_row("SELECT count(*) FROM spawn_placement", [], |row| row.get(0))
        .unwrap();
    assert_eq!(placement, 1);
    let mut late_parent = late_parent;
    let sibling = codex_records("sibling", Some("parent"), Some("solo"), &[]);
    late_parent.source(5, "codex", "sibling", &sibling);
    let placements: i64 = late_parent
        .db
        .query_row("SELECT count(*) FROM spawn_placement", [], |row| row.get(0))
        .unwrap();
    assert_eq!(placements, 0, "ambiguous child name revokes placement");

    late_parent.source(6, "claude", "other", &[]);
    // An appended edge merges existing components; rollback restores both bridges.
    {
        let tx = late_parent.db.transaction().unwrap();
        tx.execute(
            "INSERT INTO fact_relation_observation VALUES(4,999,'session_id','other')",
            [],
        )
        .unwrap();
        Warehouse::recompute(&tx, 4);
        let members: i64 = tx
            .query_row(
                "SELECT count(*) FROM bridge_session_owner WHERE owner=3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(members, 3);
    }
    let members: i64 = late_parent
        .db
        .query_row(
            "SELECT count(*) FROM bridge_session_owner WHERE owner=3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(members, 2);
}

#[test]
fn star_catalog_transaction_and_late_result() {
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    let records = codex_records(
        "root",
        None,
        None,
        &[codex_line(
            ts(0, 1),
            "response_item",
            json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{}"}),
        )],
    );
    warehouse.source(1, "codex", "root", &records);
    let original: i64 = warehouse
        .db
        .query_row("SELECT pos FROM fact_tool_call", [], |r| r.get(0))
        .unwrap();
    let pending: Option<i64> = warehouse
        .db
        .query_row("SELECT result_pos FROM tool_answers", [], |r| r.get(0))
        .unwrap();
    assert!(pending.is_none());
    {
        let tx = warehouse.db.transaction().unwrap();
        tx.execute("INSERT INTO fact_tool_result VALUES(1,999,'call',123)", [])
            .unwrap();
        tx.execute(
            "UPDATE index_advance SET complete_bytes=1000 WHERE source=1",
            [],
        )
        .unwrap();
        // Simulated crash before commit: both fact and Index advancement roll back.
    }
    let count: i64 = warehouse
        .db
        .query_row("SELECT count(*) FROM fact_tool_result", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let before: i64 = warehouse
        .db
        .query_row("SELECT complete_bytes FROM index_advance", [], |r| r.get(0))
        .unwrap();
    assert!(before < 1000);
    let tx = warehouse.db.transaction().unwrap();
    tx.execute("INSERT INTO fact_tool_result VALUES(1,999,'call',123)", [])
        .unwrap();
    tx.execute(
        "UPDATE index_advance SET complete_bytes=1000 WHERE source=1",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    let answer: i64 = warehouse
        .db
        .query_row("SELECT result_pos FROM tool_answers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(answer, 999);
    let unchanged: i64 = warehouse
        .db
        .query_row("SELECT pos FROM fact_tool_call", [], |r| r.get(0))
        .unwrap();
    assert_eq!(unchanged, original);
}

/// CI only: writes real synthetic JSONL, extracts it through the same loader,
/// then times actual list and detail SQL. No full builder over the scale corpus.
#[test]
#[ignore = "manual 1 GB / 20k session warehouse scale experiment"]
fn star_catalog_scale() {
    let home = Home::new();
    let path = home.root.join("scale.sqlite3");
    let corpus = home.root.join("corpus.jsonl");
    let mut file = std::io::BufWriter::new(File::create(&corpus).unwrap());
    let padding = "s".repeat(50_000);
    let generation = Instant::now();
    for i in 0..20_000 {
        let id = format!("session-{i:05}");
        let parent = (i > 0 && i < 1000).then(|| format!("session-{:05}", i - 1));
        let name = if i % 10 == 0 { "popular" } else { "worker" };
        let body = vec![
            json!({"type":"custom-title","customTitle":format!("Synthetic {i}")}),
            codex_user(ts(0, 1), &padding),
            codex_line(
                ts(0, 2),
                "response_item",
                json!({"type":"function_call","name":"spawn_agent","call_id":format!("call-{i}"),"arguments":format!("{{\"task_name\":\"{name}\"}}") }),
            ),
        ];
        let (harness, records) = if i >= 19_800 {
            let mut records = vec![
                human(&id, ts(0, 1), &padding),
                uuid(
                    assistant_usage(
                        &id,
                        ts(0, 2),
                        &format!("message-{i}"),
                        "synthetic-model",
                        json!({"input_tokens":1000,"output_tokens":100}),
                    ),
                    &format!("uuid-{i}"),
                ),
            ];
            if i % 2 == 0 {
                records.push(json!({"type":"continued-in","continuedInSessionId":format!("session-{:05}",i+1)}));
            }
            ("claude", records)
        } else {
            (
                "codex",
                codex_records(&id, parent.as_deref(), Some(name), &body),
            )
        };
        writeln!(
            file,
            "{}",
            json!({"source":i+1,"id":id,"harness":harness,"records":records})
        )
        .unwrap();
    }
    file.flush().unwrap();
    drop(file);
    let bytes = fs::metadata(&corpus).unwrap().len();
    assert!((1_000_000_000..1_100_000_000).contains(&bytes), "{bytes}");
    println!(
        "scale corpus bytes={bytes} sessions=20000 generation_ms={}",
        generation.elapsed().as_millis()
    );
    let mut warehouse = Warehouse::new(&path);
    let ingest = Instant::now();
    // One bounded native-session envelope at a time, never a 1 GB in-memory Value.
    for line in std::io::BufReader::new(File::open(&corpus).unwrap()).lines() {
        let envelope: Value = serde_json::from_str(&line.unwrap()).unwrap();
        warehouse.source(
            envelope["source"].as_i64().unwrap(),
            envelope["harness"].as_str().unwrap(),
            envelope["id"].as_str().unwrap(),
            envelope["records"].as_array().unwrap(),
        );
    }
    let count: i64 = warehouse
        .db
        .query_row("SELECT count(*) FROM dim_session", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 20_000);
    println!(
        "scale ingestion_ms={} sqlite_bytes={}",
        ingest.elapsed().as_millis(),
        fs::metadata(&path).unwrap().len()
    );
    println!(
        "scale dimension_maintenance_ms={} appends={} mean_append_us={}",
        warehouse.maintenance.as_millis(),
        warehouse.appends,
        warehouse.maintenance.as_micros() / u128::from(warehouse.appends)
    );
    drop(warehouse);
    // Production always has these indexes; an unindexed run with automatic
    // indexes disabled is a nested-scan worst case that never finished in CI.
    {
        let indexed = true;
        let db = Connection::open(&path).unwrap();
        if indexed {
            db.execute_batch(INDEXES).unwrap();
            db.execute_batch("ANALYZE").unwrap();
        }
        drop(db);
        for (name, sql, selected) in [("list", LIST, false), ("detail", DETAIL, true)] {
            let db = Connection::open(&path).unwrap();
            // Keep SQLite's default automatic indexes for materialized grouped
            // sets (for example catalog keys). Persistent fact/dimension indexes
            // are still explicit; the plans report both kinds.
            for temperature in ["fresh_connection", "warm"] {
                let start = Instant::now();
                let mut statement = db.prepare(sql).unwrap();
                let mut query = statement
                    .query(rusqlite::params_from_iter(
                        selected.then_some("session-00999"),
                    ))
                    .unwrap();
                let mut count = 0;
                while query.next().unwrap().is_some() {
                    count += 1;
                }
                assert_eq!(count, if selected { 4 } else { 19_900 });
                let elapsed = start.elapsed();
                println!(
                    "scale indexed={indexed} query={name} cache={temperature} rows={count} elapsed_ms={}",
                    elapsed.as_millis()
                );
                let ceiling_ms = if selected { 500 } else { 5_000 };
                assert!(
                    elapsed.as_millis() < ceiling_ms,
                    "{name} exceeds {ceiling_ms} ms: {elapsed:?}"
                );
            }
            let mut plan = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            for row in plan
                .query_map(
                    rusqlite::params_from_iter(selected.then_some("session-00999")),
                    |r| r.get::<_, String>(3),
                )
                .unwrap()
            {
                let step = row.unwrap();
                println!("scale plan indexed={indexed} query={name}: {step}");
                assert!(!step.contains("CORRELATED SCALAR SUBQUERY"), "{step}");
                // The only recursive reads are key encoding and the parent
                // graph, never a lineage closure over facts.
                assert!(
                    !step.contains("CO-ROUTINE reach") && !step.contains("MATERIALIZE reach"),
                    "recursive lineage read: {step}"
                );
                if selected {
                    assert!(
                        !step.starts_with("SCAN d ")
                            && !step.starts_with("SCAN r ")
                            && step != "SCAN l",
                        "detail scans unrelated session rows: {step}"
                    );
                    assert!(
                        !step.contains("MATERIALIZE reserved")
                            && !step.contains("MATERIALIZE catalog_keys"),
                        "detail materializes global keys: {step}"
                    );
                }
            }
        }
    }
}
