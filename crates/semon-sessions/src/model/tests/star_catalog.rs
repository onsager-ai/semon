//! Deliberately test-only: extract native observations independently of Catalog.
//! SQL chooses positions and metadata; fixture bodies never enter warehouse rows.
use std::{collections::BTreeMap, fs::File, io::BufRead, time::Instant};

use rusqlite::{Connection, params};

use super::*;

const SCHEMA: &str = include_str!("star_catalog/schema.sql");
const INDEXES: &str = "
CREATE INDEX relation_target ON fact_relation_observation(kind,target,source);
CREATE INDEX usage_identity ON fact_step_observation(message_id,uuid,source);
CREATE INDEX native_identity ON dim_session(harness,native_id);
CREATE INDEX child_name ON dim_session(parent_id,nickname);
CREATE INDEX call_name ON fact_tool_call(source,tool,task_name);
CREATE INDEX result_id ON fact_tool_result(source,call_id);";
const LIST: &str = "SELECT id,owner,harness,parent,title_source,title_pos,fallback_title,
 model,status,input,cached,output,shared_input,shared_cached,shared_output
 FROM session_list ORDER BY id";
const DETAIL: &str = "SELECT r.source,r.pos,r.kind FROM session_records r
 JOIN catalog_keys k ON k.owner=r.owner WHERE k.id=?1 ORDER BY r.ts,r.source,r.pos";

struct Warehouse {
    db: Connection,
    // Test analogue of the authorized native body reader, outside SQLite.
    titles: BTreeMap<(i64, i64), String>,
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
        Self {
            db,
            titles: BTreeMap::new(),
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
            "INSERT INTO fact_session_observation VALUES(?1,?2,?3,?4,?5,?6)",
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
        let mut bytes = 0;
        for record in records {
            let pos = bytes;
            bytes += serde_json::to_vec(record).unwrap().len() as i64 + 1;
            let kind = record["type"].as_str().unwrap();
            let uuid = record["uuid"].as_str();
            let message = &record["message"];
            let time = crate::events::record_time(record);
            tx.execute(
                "INSERT INTO fact_record VALUES(?1,?2,?3,?4,?5,?6)",
                params![source, pos, kind, uuid, message["id"].as_str(), time],
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
            "INSERT INTO index_advance VALUES(?1,?2)",
            params![source, bytes],
        )
        .unwrap();
        Self::recompute(&tx, source);
        tx.commit().unwrap();
    }

    fn recompute(db: &Connection, source: i64) {
        db.execute(
            "INSERT OR REPLACE INTO dim_session SELECT * FROM session_recompute WHERE source=?1",
            [source],
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
                Ok(ListRow {
                    id: row.get(0)?,
                    owner: row.get(1)?,
                    parent: row.get(3)?,
                    title,
                    model: pretty_model(&raw_model).unwrap(),
                    status: row.get(8)?,
                    tokens: [row.get(9)?, row.get(10)?, row.get(11)?],
                    shared: [row.get(12)?, row.get(13)?, row.get(14)?],
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
    // Re-derive a local dimension twice. No cross-key parent/owner is stored.
    Warehouse::recompute(&warehouse.db, 1);
    Warehouse::recompute(&warehouse.db, 1);
    assert_eq!(warehouse.list().len(), rows.len());
    println!(
        "star parity: {} rows; copy owner, parents, titles, models, status, exclusive/shared usage, unique spawn, transcript suppression",
        rows.len()
    );
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
    drop(warehouse);
    for indexed in [false, true] {
        let db = Connection::open(&path).unwrap();
        if indexed {
            db.execute_batch(INDEXES).unwrap();
            db.execute_batch("ANALYZE").unwrap();
        }
        drop(db);
        for (name, sql, selected) in [("list", LIST, false), ("detail", DETAIL, true)] {
            let db = Connection::open(&path).unwrap();
            // Keep primary-key indexes but disable automatic transient indexes.
            db.execute_batch("PRAGMA automatic_index=OFF").unwrap();
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
                println!(
                    "scale indexed={indexed} query={name} cache={temperature} rows={count} elapsed_ms={}",
                    start.elapsed().as_millis()
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
                println!(
                    "scale plan indexed={indexed} query={name}: {}",
                    row.unwrap()
                );
            }
        }
    }
}
