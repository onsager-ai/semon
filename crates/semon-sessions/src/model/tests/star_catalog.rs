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
    native: BTreeMap<i64, crate::events::FileIndex>,
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
        // Bind one effective price snapshot to this read adapter. The SQL fold
        // selects native billing observations; production pricing owns rates,
        // cache-write splits, service tiers and unknown-price semantics.
        let prices = crate::pricing::snapshot();
        db.create_scalar_function(
            "semon_step_usd",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            move |context| {
                let model = context.get::<Option<String>>(0)?;
                let Some(billing) = context.get::<Option<String>>(1)? else {
                    return Ok(None);
                };
                let billing = serde_json::from_str(&billing)
                    .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))?;
                let mut cost = crate::pricing::CostBuild::with_table(prices.clone());
                cost.message(&events::MessageUsage {
                    model,
                    billing,
                    ..Default::default()
                });
                Ok(cost.finish().usd)
            },
        )
        .unwrap();
        // Fixture snapshots have already qualified process identity/expiry. These
        // origin-local inputs are separate from log progress and chosen turns.
        db.execute_batch(
            "CREATE TEMP TABLE qualified_turn_runtime (
            source INTEGER PRIMARY KEY,state TEXT NOT NULL,waiting_for TEXT)",
        )
        .unwrap();
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
            native: BTreeMap::new(),
            maintenance: std::time::Duration::ZERO,
            appends: 0,
        }
    }

    fn source(&mut self, source: i64, harness: &str, id: &str, records: &[Value]) {
        // Use the native parser's append journal, including revisions to old
        // calls when their results arrive. Never re-project the whole session.
        let before_events = if harness == "claude" {
            let native = self.native.entry(source).or_default();
            native.dirty_rows = Some(crate::events::DirtyRows::default());
            native.events.len()
        } else {
            0
        };
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
                crate::events::claude(self.native.entry(source).or_default(), record, pos as u64);
                if kind == "assistant"
                    && message.get("usage").is_some()
                    && let Some(used) = message["id"]
                        .as_str()
                        .and_then(|id| self.native[&source].usage().get(id))
                {
                    tx.prepare_cached("INSERT INTO fact_step_billing VALUES(?1,?2,?3)")
                        .unwrap()
                        .execute(params![
                            source,
                            pos,
                            serde_json::to_string(&used.billing).unwrap()
                        ])
                        .unwrap();
                }
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
                                    plain_name(
                                        value["input"]["to"]
                                            .as_str()
                                            .or_else(|| value["input"]["recipient"].as_str())
                                            .unwrap_or("")
                                    ),
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
                if origin["kind"] == "peer"
                    && origin["senderTaskId"].is_null()
                    && origin["handback"] != true
                {
                    tx.execute(
                        "INSERT INTO fact_relay_receipt VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![
                            source,
                            pos,
                            origin["msg_id"].as_str(),
                            plain_name(origin["name"].as_str().unwrap_or("")),
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
        if let Some(native) = self.native.get_mut(&source) {
            let changed = native.dirty_rows.take().unwrap().events;
            // New events need not pass through event_mut. Old revised events
            // do; exclude the new suffix from that set to write each row once.
            for index in changed
                .into_iter()
                .filter(|index| *index < before_events)
                .chain(before_events..native.events.len())
            {
                let event = &native.events[index];
                let reply = event.r.as_ref();
                let peer = event.peer.as_ref();
                if event.k == Kind::Tool
                    && event.n.as_deref() == Some("SendMessage")
                    && let Some(reply) = reply
                {
                    tx.execute(
                        "UPDATE fact_relay_result SET failed=?3 WHERE source=?1 AND call_id=?2",
                        params![
                            source,
                            event.id,
                            reply.e || reply.f & (DENIED | SEND_FAILED) != 0
                        ],
                    )
                    .unwrap();
                }
                tx.execute(
                    "INSERT OR REPLACE INTO fact_subagent_event VALUES(
                     ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                    params![
                        source,
                        index as i64,
                        i64::try_from(event.o).unwrap(),
                        event.b,
                        serde_json::to_value(event.k).unwrap().as_str().unwrap(),
                        event.t,
                        event.id,
                        event.n,
                        event.script,
                        reply.map(|r| i64::try_from(r.o).unwrap()),
                        reply.map(|r| r.b),
                        reply.and_then(|r| r.t),
                        reply.is_some_and(|r| r.e),
                        reply.map_or(0, |r| r.f),
                        reply.and_then(|r| r.m.as_deref()),
                        peer.and_then(|p| p.pid)
                            .map(|pid| i64::try_from(pid).unwrap()),
                        peer.and_then(|p| p.start.as_deref()),
                        peer.and_then(|p| p.sock.as_deref())
                    ],
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
            .prepare_cached("SELECT pos FROM maintenance_cursor WHERE source=?1")
            .unwrap()
            .query_row([source], |r| r.get::<_, i64>(0))
            .optional()
            .unwrap()
            .unwrap_or(-1);
        let rank = || {
            db.prepare_cached("SELECT first_ts,continued FROM dim_session WHERE source=?1")
                .unwrap()
                .query_row([source], |r| {
                    Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?))
                })
                .optional()
                .unwrap()
        };
        let old_rank = rank();
        // Reuse the suffix-maintenance programs across append transactions.
        // Preparing the joins and upserts for every record costs more than
        // executing their indexed probes on small suffixes.
        for sql in MAINTAIN.split(';').filter(|sql| !sql.trim().is_empty()) {
            let mut statement = db.prepare_cached(sql).unwrap();
            for (name, value) in [(":source", source), (":after", after)] {
                if let Some(index) = statement.parameter_index(name).unwrap() {
                    statement.raw_bind_parameter(index, value).unwrap();
                }
            }
            statement.raw_execute().unwrap();
        }
        let new_relations: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fact_relation_observation
             WHERE source=?1 AND pos>?2 AND kind IN ('continued-in','session_id','bridge'))",
                params![source, after],
                |r| r.get(0),
            )
            .unwrap();
        if old_rank.is_none() || new_relations || old_rank != rank() {
            Self::maintain_lineage(db, source, after);
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

    fn preferred_owner(db: &Connection, left: i64, right: i64) -> i64 {
        db.query_row(
            "SELECT source FROM dim_session WHERE source IN (?1,?2)
             ORDER BY first_ts IS NULL,first_ts,continued DESC,native_id,source LIMIT 1",
            params![left, right],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn merge_groups(db: &Connection, left: i64, right: i64) {
        if left == right {
            return;
        }
        let group = |id| {
            db.query_row(
                "SELECT weight,minimum,owner FROM lineage_group WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        let (left_weight, left_min, left_owner) = group(left);
        let (right_weight, right_min, right_owner) = group(right);
        let owner = Self::preferred_owner(db, left_owner, right_owner);
        let (large, small) = if left_weight >= right_weight {
            (left, right)
        } else {
            (right, left)
        };
        db.execute(
            "UPDATE lineage_member SET component=?1 WHERE component=?2",
            params![large, small],
        )
        .unwrap();
        db.execute(
            "UPDATE lineage_node SET component=?1 WHERE component=?2",
            params![large, small],
        )
        .unwrap();
        // Remove the old unique minimum before assigning it to the surviving group.
        db.execute("DELETE FROM lineage_group WHERE id=?1", [small])
            .unwrap();
        db.execute(
            "UPDATE lineage_group SET weight=?2,minimum=?3,owner=?4 WHERE id=?1",
            params![
                large,
                left_weight + right_weight,
                left_min.min(right_min),
                owner
            ],
        )
        .unwrap();
    }

    fn maintain_lineage(db: &Connection, source: i64, after: i64) {
        let fresh = db
            .execute(
                "INSERT OR IGNORE INTO lineage_member VALUES(?1,?1)",
                [source],
            )
            .unwrap()
            != 0;
        if fresh {
            db.execute("INSERT INTO lineage_group VALUES(?1,1,?1,?1)", [source])
                .unwrap();
        }
        let (harness, native): (String, String) = db
            .query_row(
                "SELECT harness,native_id FROM dim_session WHERE source=?1",
                [source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut nodes = Vec::new();
        if harness == "claude" {
            if fresh {
                nodes.push(native);
            }
            nodes.extend(
                db.prepare(
                    "SELECT DISTINCT target FROM fact_relation_observation
                 WHERE source=?1 AND pos>?2 AND kind IN ('continued-in','session_id','bridge')",
                )
                .unwrap()
                .query_map(params![source, after], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap),
            );
        }
        for node in nodes {
            let component: i64 = db
                .query_row(
                    "SELECT component FROM lineage_member WHERE source=?1",
                    [source],
                    |r| r.get(0),
                )
                .unwrap();
            let inserted = db
                .execute(
                    "INSERT OR IGNORE INTO lineage_node VALUES(?1,?2)",
                    params![node, component],
                )
                .unwrap();
            if inserted != 0 {
                db.execute(
                    "UPDATE lineage_group SET weight=weight+1 WHERE id=?1",
                    [component],
                )
                .unwrap();
            } else {
                let other = db
                    .query_row(
                        "SELECT component FROM lineage_node WHERE node=?1",
                        [node],
                        |r| r.get(0),
                    )
                    .unwrap();
                Self::merge_groups(db, component, other);
            }
        }
        // Ranking inputs only improve on append: first_ts decreases (NULL last),
        // continued changes false -> true. Comparing this member is sufficient.
        let (component,owner): (i64,i64) = db.query_row(
            "SELECT g.id,g.owner FROM lineage_member m JOIN lineage_group g ON g.id=m.component WHERE m.source=?1",
            [source], |r| Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        let chosen = Self::preferred_owner(db, source, owner);
        if chosen != owner {
            db.execute(
                "UPDATE lineage_group SET owner=?2 WHERE id=?1",
                params![component, chosen],
            )
            .unwrap();
        }
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

// The source reader is deliberately outside SQLite. Event facts carry the
// source/record/block recipe, and only the authorized origin opens the body.
fn selected_record(files: &[SourceFile], at: Ref) -> Value {
    let line = fs::read_to_string(&files[at.0].path).unwrap();
    serde_json::from_str(line[event(files, at).o as usize..].lines().next().unwrap()).unwrap()
}

struct SelectedRelation {
    handoff: H,
    positions: Vec<Ref>,
    head: Option<usize>,
}

fn selected_claude_relations(
    warehouse: &Warehouse,
    files: &[SourceFile],
    source_files: &BTreeMap<i64, usize>,
    sessions: &BTreeMap<i64, usize>,
    rows: &[ListRow],
) -> (Vec<SelectedRelation>, BTreeSet<Ref>) {
    let observations: Vec<(i64, i64, u32, String, usize)> = warehouse
        .db
        .prepare("SELECT source,pos,block,kind,row_number() OVER (PARTITION BY source,pos,block,kind ORDER BY event)-1 FROM fact_subagent_event ORDER BY source,event")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                usize::try_from(r.get::<_, i64>(4)?).unwrap(),
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let selected: BTreeSet<_> = rows
        .iter()
        .flat_map(|row| warehouse.detail(&row.id))
        .map(|(source, pos, _)| (source_files[&source], pos as u64))
        .collect();
    let refs: BTreeSet<Ref> = observations
        .into_iter()
        .map(|(source, pos, block, kind, ordinal)| {
            let file = source_files[&source];
            let index = files[file]
                .summary
                .events
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.o == pos as u64
                        && e.b == block
                        && serde_json::to_value(e.k).unwrap().as_str() == Some(kind.as_str())
                })
                .nth(ordinal)
                .unwrap()
                .0;
            (file, index)
        })
        .filter(|at| selected.contains(&(at.0, event(files, *at).o)))
        .collect();
    let owner_of: BTreeMap<usize, usize> = warehouse
        .db
        .prepare("SELECT source,owner FROM bridge_session_owner")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .map(|r| {
            let (source, owner) = r.unwrap();
            (source_files[&source], sessions[&owner])
        })
        .collect();
    let events_of = |session| {
        refs.iter()
            .copied()
            .filter(|at| owner_of[&at.0] == session)
            .collect::<Vec<_>>()
    };
    let session_start = |session| {
        owner_of
            .iter()
            .filter(|(_, owner)| **owner == session)
            .filter_map(|(file, _)| files[*file].first)
            .min()
            .unwrap_or(0)
    };
    let session_last = |session| {
        owner_of
            .iter()
            .filter(|(_, owner)| **owner == session)
            .filter_map(|(file, _)| files[*file].last)
            .max()
            .unwrap_or(0)
    };
    let body = |at, key: &str| {
        let record = selected_record(files, at);
        input_string(&record, event(files, at).b as usize, &[key])
    };
    let receipt_bodies = |key: &str, received: &[Ref]| {
        received
            .iter()
            .enumerate()
            .map(|(index, at)| {
                let skip = received[..index]
                    .iter()
                    .filter(|other| {
                        other.0 == at.0 && event(files, **other).o == event(files, *at).o
                    })
                    .count();
                received_body(
                    &selected_record(files, *at),
                    "agent-message",
                    Some(key),
                    skip,
                )
            })
            .collect::<Vec<_>>()
    };
    let parents: Vec<(i64, i64, Option<String>)> = warehouse
        .db
        .prepare(
            "SELECT p.child,p.parent,p.call_id FROM parent_selection p
         JOIN fact_agent_observation a ON a.source=p.child ORDER BY p.child",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut relations = Vec::new();
    let mut handled = BTreeSet::new();
    let mut claimed_receipts = BTreeSet::new();
    for (child_source, parent_source, call_id) in parents {
        let child = sessions[&child_source];
        let parent = sessions[&parent_source];
        let key = &rows[child].id;
        let child_refs = events_of(child);
        let parent_refs = events_of(parent);
        let first_prompt = child_refs
            .iter()
            .copied()
            .find(|at| event(files, *at).k == Kind::U);
        let call = call_id.as_deref().and_then(|id| {
            parent_refs.iter().copied().find(|at| {
                event(files, *at).id.as_deref() == Some(id) && event(files, *at).k == Kind::Tool
            })
        });
        let mut received: Vec<Ref> = parent_refs
            .iter()
            .copied()
            .filter(|at| {
                let e = event(files, *at);
                e.k == Kind::Agm
                    && e.script & AGM_UP != 0
                    && [e.id.as_deref(), e.n.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|id| id == key || Some(id) == call_id.as_deref())
            })
            .collect();
        received.sort_by_key(|at| {
            let t = event(files, *at).t;
            (t.is_none(), t, *at)
        });
        let (finals, received): (Vec<_>, Vec<_>) = received
            .into_iter()
            .partition(|at| event(files, *at).script & AGM_FINAL != 0);
        let positions: Vec<_> = child_refs
            .iter()
            .enumerate()
            .filter_map(|(index, at)| {
                let e = event(files, *at);
                (e.k == Kind::Tool
                    && e.n.as_deref() == Some("SendMessage")
                    && !e.r.as_ref().is_some_and(|r| {
                        r.e || r.f & (DENIED | SEND_FAILED | PIN) != 0 || r.m.is_some()
                    })
                    && plain_name(
                        &body(*at, "to")
                            .or_else(|| body(*at, "recipient"))
                            .unwrap_or_default(),
                    ) == PARENT_ADDRESS)
                    .then_some(index)
            })
            .collect();
        let messages = positions
            .iter()
            .map(|index| {
                body(child_refs[*index], "message").or_else(|| body(child_refs[*index], "content"))
            })
            .collect::<Vec<_>>();
        let bodies = receipt_bodies(key, &received);
        let (claims, paired) = relay_match::pair_sends(
            files,
            &child_refs,
            &positions,
            &messages,
            &received,
            &bodies,
        );
        for (send, claim) in claims.iter().enumerate() {
            let at = child_refs[positions[send]];
            let e = event(files, at);
            let receipt = claim.map(|index| received[index]);
            // Pending sends can claim a receipt before the result is admitted;
            // the native builder draws that receipt independently until then.
            if e.r.is_none() {
                continue;
            }
            claimed_receipts.extend(receipt);
            handled.insert(at);
            let source = receipt
                .filter(|r| event(files, *r).script & AGM_PROGRESS != 0)
                .map(|r| format!("relay:recv:{}:{}", files[r.0].id, event(files, r).o))
                .unwrap_or_else(|| format!("relay:tool:{}", e.id.as_deref().unwrap()));
            let out = Handoff::new(
                stable_id("r", &source),
                "relay",
                (key.clone(), rows[parent].id.clone()),
                e.t.unwrap_or_else(|| session_last(child)),
                "done",
                messages[send].as_deref().unwrap_or(""),
            );
            relations.push(SelectedRelation {
                handoff: H {
                    out,
                    from: Some(child),
                    to: Some(parent),
                    previews: None,
                    preview_owners: BTreeMap::new(),
                    inline: false,
                },
                positions: std::iter::once(at).chain(receipt).collect(),
                head: None,
            });
        }
        let ends = parent_refs
            .iter()
            .copied()
            .filter(|at| {
                let e = event(files, *at);
                e.k == Kind::Tn
                    && e.id == call_id
                    && !matches!(e.n.as_deref(), None | Some("running" | "started"))
            })
            .collect::<Vec<_>>();
        let (runs, inline) =
            relay_match::claude_runs(files, &child_refs, first_prompt, &finals, &ends);
        let mut completion = None;
        if let Some(run) = runs.iter().rev().find(|run| run.ended()) {
            let done = [run.fin, run.tn, run.handback]
                .into_iter()
                .flatten()
                .filter_map(|at| event(files, at).t)
                .min();
            let failed = run.tn.is_some_and(|at| {
                !matches!(
                    event(files, at).n.as_deref(),
                    Some("completed" | "success" | "done")
                )
            });
            let result = if let Some(at) = run.fin {
                let index = finals.iter().position(|r| *r == at).unwrap();
                receipt_bodies(key, &finals)[index].clone()
            } else if let Some(at) = run.tn {
                notification_result(
                    &selected_record(files, at),
                    call_id.as_deref().unwrap_or(""),
                )
            } else {
                run.handback.and_then(|at| body(at, "message"))
            };
            completion = Some((result, done, failed));
        }
        if completion.is_none()
            && let Some(index) = (0..received.len())
                .rev()
                .find(|i| !paired[*i] && event(files, received[*i]).script & AGM_PROGRESS == 0)
        {
            completion = Some((
                bodies[index].clone(),
                event(files, received[index]).t,
                false,
            ));
        }
        if completion.is_none()
            && let Some(at) = call
            && let Some(reply) = event(files, at).r.as_ref()
            && reply.f & ASYNC == 0
        {
            completion = Some((
                {
                    let text = fs::read_to_string(&files[at.0].path).unwrap();
                    let record =
                        serde_json::from_str(text[reply.o as usize..].lines().next().unwrap())
                            .unwrap();
                    tool_result_text(&record, reply.b as usize)
                },
                reply.t,
                reply.e,
            ));
        }
        // Runtime observations name physical sources. Select the current root
        // through the parent/owner reads before qualifying a resumed native run.
        let mut root = parent;
        let mut visited = BTreeSet::new();
        while visited.insert(root) {
            let Some(next) = rows[root]
                .parent
                .as_ref()
                .and_then(|key| rows.iter().position(|row| &row.id == key))
            else {
                break;
            };
            root = next;
        }
        let running: bool = warehouse
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM qualified_turn_runtime r
             JOIN bridge_session_owner b ON b.source=r.source WHERE b.owner=?1)",
                [rows[root].owner],
                |r| r.get(0),
            )
            .unwrap();
        let resumed = runs.len() > 1 && runs.last().is_some_and(|run| !run.ended());
        let mut out = Handoff::new(
            stable_id("s", &format!("spawn:claude:{key}")),
            "spawn",
            (rows[parent].id.clone(), key.clone()),
            call.and_then(|at| event(files, at).t)
                .unwrap_or(files[source_files[&child_source]].first.unwrap()),
            if running && (resumed || completion.is_none()) {
                "work"
            } else if resumed {
                "done"
            } else if completion.as_ref().is_some_and(|c| c.2) {
                "err"
            } else {
                "done"
            },
            "",
        );
        out.done = completion.as_ref().and_then(|c| c.1);
        out.result = completion.and_then(|c| c.0);
        relations.push(SelectedRelation {
            handoff: H {
                out,
                from: Some(parent),
                to: Some(child),
                previews: None,
                preview_owners: BTreeMap::new(),
                inline: false,
            },
            positions: call.into_iter().chain(first_prompt).collect(),
            head: first_prompt.is_none().then_some(child),
        });
        let names = [
            Some(key.clone()),
            call.and_then(|at| body(at, "name")).map(|n| plain_name(&n)),
        ];
        let sends = parent_refs
            .iter()
            .copied()
            .filter(|at| {
                let e = event(files, *at);
                if e.k != Kind::Tool || e.n.as_deref() != Some("SendMessage") {
                    return false;
                }
                let Some(r) = e.r.as_ref() else {
                    return false;
                };
                if r.e || r.f & (DENIED | SEND_FAILED) != 0 || r.m.is_some() {
                    return false;
                }
                let record = if r.o == e.o {
                    selected_record(files, *at)
                } else {
                    let text = fs::read_to_string(&files[at.0].path).unwrap();
                    serde_json::from_str(text[r.o as usize..].lines().next().unwrap()).unwrap()
                };
                if let Some(id) = down_target(&record, r.b as usize).filter(|id| !id.is_empty()) {
                    return id == *key;
                }
                let target = plain_name(
                    &body(*at, "to")
                        .or_else(|| body(*at, "recipient"))
                        .unwrap_or_default(),
                );
                names.iter().flatten().any(|n| *n == target)
            })
            .collect::<Vec<_>>();
        handled.extend(sends.iter().copied());
        let downs = child_refs
            .iter()
            .copied()
            .filter(|at| {
                let e = event(files, *at);
                e.k == Kind::Agm && e.script & AGM_DOWN != 0
            })
            .collect::<Vec<_>>();
        let messages = sends
            .iter()
            .map(|at| body(*at, "message").or_else(|| body(*at, "content")))
            .collect::<Vec<_>>();
        let bodies = downs
            .iter()
            .map(|at| coordinator_body(&selected_record(files, *at)))
            .collect::<Vec<_>>();
        let claims = relay_match::pair_downs(files, &sends, &messages, &downs, &bodies);
        for (index, down) in downs.into_iter().enumerate() {
            let send = claims[index].map(|i| sends[i]);
            let brief = claims[index]
                .and_then(|i| messages[i].as_deref())
                .or(bodies[index].as_deref())
                .unwrap_or("");
            let out = Handoff::new(
                stable_id(
                    "r",
                    &format!("relay:down:{}:{}", files[down.0].id, event(files, down).o),
                ),
                "relay",
                (rows[parent].id.clone(), key.clone()),
                send.and_then(|at| event(files, at).t)
                    .or(event(files, down).t)
                    .unwrap_or_else(|| session_start(child)),
                "done",
                brief,
            );
            relations.push(SelectedRelation {
                handoff: H {
                    out,
                    from: Some(parent),
                    to: Some(child),
                    previews: None,
                    preview_owners: BTreeMap::new(),
                    inline: inline.contains(&down),
                },
                positions: send.into_iter().chain(Some(down)).collect(),
                head: None,
            });
        }
    }
    // A progress receipt stands alone until its send/result appears. The
    // sender identity is a real subagent only when role-qualified facts say so.
    for at in refs.iter().copied() {
        let e = event(files, at);
        if e.k != Kind::Agm || e.script & AGM_PROGRESS == 0 || claimed_receipts.contains(&at) {
            continue;
        }
        let from = [
            e.peer.as_ref().and_then(|p| p.sock.as_deref()),
            e.id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(|id| rows.iter().position(|row| row.id == id))
        .filter(|session| {
            source_files.iter().any(|(source, file)| {
                owner_of[file] == *session
                    && warehouse
                        .db
                        .query_row(
                            "SELECT count(*) FROM fact_agent_observation WHERE source=?1",
                            [source],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap()
                        > 0
            })
        });
        let to = owner_of[&at.0];
        let name = plain_name(e.n.as_deref().or(e.id.as_deref()).unwrap_or(""));
        let identity = match e.peer.as_ref() {
            Some(peer) if peer.pid.is_some() => format!(
                "pid:{}:{}",
                peer.pid.unwrap(),
                peer.start.as_deref().unwrap_or("")
            ),
            Some(peer) if peer.sock.is_some() => format!("sock:{}", peer.sock.as_deref().unwrap()),
            _ => format!("name:{name}"),
        };
        let mut out = Handoff::new(
            stable_id("r", &format!("relay:recv:{}:{}", files[at.0].id, e.o)),
            "relay",
            (
                from.map(|i| rows[i].id.clone())
                    .unwrap_or_else(|| stable_id("peer-", &identity)),
                rows[to].id.clone(),
            ),
            e.t.unwrap_or_else(|| session_start(to)),
            "done",
            received_body(&selected_record(files, at), "agent-message", None, 0)
                .as_deref()
                .unwrap_or(""),
        );
        out.unmatched = from.is_none();
        relations.push(SelectedRelation {
            handoff: H {
                out,
                from,
                to: Some(to),
                previews: None,
                preview_owners: BTreeMap::new(),
                inline: false,
            },
            positions: vec![at],
            head: None,
        });
    }
    (relations, handled)
}

// Origin-local fixture adapter: SQL selects source positions and relationships;
// the native reader supplies events/bodies at those positions. No Builder or
// Builder-produced handoff, entry, session, or turn is an input to this path.
fn selected_turns(warehouse: &Warehouse, home: &Home) -> BTreeMap<String, (String, Vec<Turn>)> {
    let mut cache = EventCache::open(&home.options.cache).unwrap();
    let (files, _, _) = scan(
        &home.options,
        &mut cache,
        &mut false,
        &mut Texts::default(),
        None,
        &MachineFacts::Local,
        None,
    )
    .unwrap();
    let rows = warehouse.list();
    let source_files: BTreeMap<i64, usize> = warehouse
        .db
        .prepare("SELECT source,harness,native_id FROM dim_session")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(|r| {
            let (source, harness, id) = r.unwrap();
            let file = files
                .iter()
                .position(|f| f.id == id && f.harness() == harness)
                .unwrap();
            (source, file)
        })
        .collect();
    let sessions: BTreeMap<i64, usize> =
        rows.iter().enumerate().map(|(i, r)| (r.owner, i)).collect();
    let selected: Vec<BTreeSet<(usize, u64)>> = rows
        .iter()
        .map(|row| {
            warehouse
                .detail(&row.id)
                .into_iter()
                .map(|(source, pos, _)| (source_files[&source], u64::try_from(pos).unwrap()))
                .collect()
        })
        .collect();
    let mut states = Vec::new();
    for row in &rows {
        let file = &files[source_files[&row.owner]];
        let state = if file.harness() == "codex" {
            file.summary
                .tree_summary
                .as_ref()
                .and_then(|s| s.codex_turn.owned(&file.id))
                .map_or("off", |turn| turn.state(false))
        } else {
            "done"
        };
        let runtime: Option<(String, Option<String>)> = warehouse
            .db
            .query_row(
                "SELECT r.state,r.waiting_for FROM qualified_turn_runtime r
                 JOIN bridge_session_owner b ON b.source=r.source WHERE b.owner=?1",
                [row.owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap();
        states.push(runtime.unwrap_or((state.into(), None)));
    }
    let (native_relations, native_handled) =
        selected_claude_relations(warehouse, &files, &source_files, &sessions, &rows);
    for relation in &native_relations {
        if relation.handoff.out.kind == "spawn" {
            states[relation.handoff.to.unwrap()].0 = relation.handoff.out.status.into();
        }
    }
    let mut handoffs = Vec::new();
    let mut placed = HashMap::<Ref, Vec<usize>>::new();
    let mut heads = BTreeMap::new();
    {
        let mut add = |out: Handoff, from, to, positions: Vec<Ref>| {
            let h = handoffs.len();
            handoffs.push(H {
                out,
                from,
                to,
                previews: None,
                preview_owners: BTreeMap::new(),
                inline: false,
            });
            for position in positions {
                placed.entry(position).or_default().push(h);
            }
            h
        };
        for (session, row) in rows.iter().enumerate() {
            for (file, source) in files.iter().enumerate() {
                for (i, found) in source.summary.events.iter().enumerate() {
                    if found.k == Kind::Ask && selected[session].contains(&(file, found.o)) {
                        let out = Handoff::new(
                            stable_id(ASK_PREFIX, &format!("ask:{}:{}", source.id, found.o)),
                            "ask",
                            ("you".into(), row.id.clone()),
                            found.t.unwrap(),
                            "done",
                            "",
                        );
                        add(out, None, Some(session), vec![(file, i)]);
                    }
                }
            }
        }
        // Parent selection and the revisable unique name/call placement are SQL
        // answers. An ambiguous spawn still starts the child but replaces no call.
        let spawns: Vec<(i64, i64, Option<i64>, Option<i64>)> = warehouse
            .db
            .prepare(
                "SELECT p.child,p.parent,b.pos,b.ts FROM parent_selection p
         JOIN dim_session d ON d.source=p.child AND d.harness='codex'
         LEFT JOIN bridge_spawn_placement b ON b.child=d.native_id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (child_source, parent_source, call, time) in spawns {
            let child = sessions[&child_source];
            let parent = sessions[&parent_source];
            let file = source_files[&child_source];
            let prompt = files[file]
                .summary
                .events
                .iter()
                .enumerate()
                .find(|(_, e)| e.k == Kind::U);
            let mut positions = Vec::new();
            if let Some(pos) = call {
                let parent_file = source_files[&parent_source];
                let i = files[parent_file]
                    .summary
                    .events
                    .iter()
                    .position(|e| e.o == pos as u64 && e.k == Kind::Tool)
                    .unwrap();
                positions.push((parent_file, i));
            }
            if let Some((i, _)) = prompt {
                positions.push((file, i));
            }
            let mut out = Handoff::new(
                stable_id("s", &format!("spawn:codex:{}", rows[child].id)),
                "spawn",
                (rows[parent].id.clone(), rows[child].id.clone()),
                time.unwrap_or(files[file].first.unwrap()),
                match states[child].0.as_str() {
                    "done" => "done",
                    "err" => "err",
                    "work" => "work",
                    _ => "off",
                },
                "",
            );
            out.ambiguous = call.is_none();
            let h = add(out, Some(parent), Some(child), positions);
            if prompt.is_none() {
                heads.insert(child, h);
            }
        }
        struct RelayRow {
            source: i64,
            pos: i64,
            call: String,
            sender: i64,
            receiver: Option<i64>,
            stub: Option<String>,
            failed: bool,
            correlation: Option<String>,
            time: Option<i64>,
        }
        let relays = warehouse.db.prepare(
        "SELECT source,pos,call_id,sender,receiver,stub,failed,correlation,ts FROM relay_endpoints"
    ).unwrap().query_map([], |r| Ok(RelayRow {
        source:r.get(0)?,pos:r.get(1)?,call:r.get(2)?,sender:r.get(3)?,receiver:r.get(4)?,
        stub:r.get(5)?,failed:r.get(6)?,correlation:r.get(7)?,time:r.get(8)?,
    })).unwrap().map(Result::unwrap).collect::<Vec<_>>();
        let mut joined = BTreeSet::new();
        for RelayRow {
            source,
            pos,
            call,
            sender,
            receiver,
            stub,
            failed,
            correlation,
            time,
        } in relays
        {
            let from = sessions[&sender];
            let to = receiver.map(|r| sessions[&r]);
            let file = source_files[&source];
            let i = files[file]
                .summary
                .events
                .iter()
                .position(|e| e.o == pos as u64 && e.id.as_deref() == Some(call.as_str()))
                .unwrap();
            if native_handled.contains(&(file, i))
                || event(&files, (file, i))
                    .r
                    .as_ref()
                    .is_some_and(|r| r.f & PIN != 0)
            {
                continue;
            }
            if correlation.is_none()
                && let Some(reply) = event(&files, (file, i)).r.as_ref()
            {
                let text = fs::read_to_string(&files[file].path).unwrap();
                let record =
                    serde_json::from_str(text[reply.o as usize..].lines().next().unwrap()).unwrap();
                if down_target(&record, reply.b as usize).is_some() {
                    continue;
                }
            }
            let mut positions = vec![(file, i)];
            if let Some(correlation) = &correlation {
                let receipt: Option<(i64, i64)> = warehouse
                    .db
                    .query_row(
                        "SELECT r.source,r.pos FROM fact_relay_receipt r
                 JOIN bridge_session_owner m USING(source) JOIN dim_session d ON d.source=m.owner
                 WHERE r.correlation=?1 ORDER BY d.native_id,r.pos LIMIT 1",
                        [correlation],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .unwrap();
                if let Some((source, pos)) = receipt {
                    joined.insert(correlation.clone());
                    let file = source_files[&source];
                    let i = files[file]
                        .summary
                        .events
                        .iter()
                        .position(|e| e.o == pos as u64 && e.k == Kind::Xsm)
                        .unwrap();
                    positions.push((file, i));
                }
            }
            let id_source =
                correlation.map_or_else(|| format!("relay:tool:{call}"), |c| format!("relay:{c}"));
            let out = Handoff::new(
                stable_id("r", &id_source),
                "relay",
                (
                    rows[from].id.clone(),
                    to.map(|i| rows[i].id.clone())
                        .or_else(|| stub.map(|s| stable_id("peer-", &s)))
                        .unwrap_or_default(),
                ),
                time.unwrap_or_else(|| {
                    files
                        .iter()
                        .enumerate()
                        .filter(|(file, _)| selected[from].iter().any(|(member, _)| member == file))
                        .filter_map(|(_, file)| file.last)
                        .max()
                        .unwrap_or(0)
                }),
                if failed { "err" } else { "done" },
                "",
            );
            add(out, Some(from), to, positions);
        }
        // Receipt-first evidence is an incoming relay even before a send is admitted.
        let receipts: Vec<(i64, i64, String)> = warehouse
        .db
        .prepare(
            "SELECT source,pos,correlation FROM fact_relay_receipt ORDER BY correlation,source,pos",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
        for (source, pos, correlation) in receipts {
            if !joined.insert(correlation.clone()) {
                continue;
            }
            let file = source_files[&source];
            let to: i64 = warehouse
                .db
                .query_row(
                    "SELECT owner FROM bridge_session_owner WHERE source=?1",
                    [source],
                    |r| r.get(0),
                )
                .unwrap();
            let to = sessions[&to];
            let i = files[file]
                .summary
                .events
                .iter()
                .position(|e| e.o == pos as u64 && e.k == Kind::Xsm)
                .unwrap();
            let out = Handoff::new(
                stable_id("r", &format!("relay:{correlation}")),
                "relay",
                ("fixture-sender-stub".into(), rows[to].id.clone()),
                files[file].summary.events[i].t.unwrap(),
                "done",
                "",
            );
            add(out, None, Some(to), vec![(file, i)]);
        }
    }
    let expected = home.build_query();
    for relation in &native_relations {
        if relation.handoff.out.kind != "relay" {
            continue;
        }
        let actual = &relation.handoff.out;
        let expected = expected
            .handoffs
            .iter()
            .find(|h| h.id == actual.id)
            .unwrap_or_else(|| panic!("unexpected native relay {}", actual.id));
        assert_eq!(
            (
                &actual.from,
                &actual.to,
                actual.at,
                actual.status,
                &actual.brief,
                actual.unmatched
            ),
            (
                &expected.from,
                &expected.to,
                expected.at,
                expected.status,
                &expected.brief,
                expected.unmatched
            ),
            "native relay {}",
            actual.id
        );
    }
    for relation in native_relations {
        let h = handoffs.len();
        handoffs.push(relation.handoff);
        for at in relation.positions {
            placed.entry(at).or_default().push(h);
        }
        if let Some(child) = relation.head {
            heads.insert(child, h);
        }
    }
    let mut actual = BTreeMap::new();
    for (session, row) in rows.iter().enumerate() {
        let mut members: Vec<_> = selected[session]
            .iter()
            .map(|(file, _)| *file)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        members.sort_by_key(|file| {
            (
                files[*file].first.unwrap_or(i64::MAX),
                files[*file].summary.links.continued_in.is_empty(),
                files[*file].id.clone(),
            )
        });
        let mut keyed = Vec::new();
        let mut native_refs = Vec::new();
        for (rank, file) in members.iter().copied().enumerate() {
            let mut time = files[file]
                .summary
                .events
                .iter()
                .find_map(|e| e.t)
                .unwrap_or(i64::MIN);
            for (i, found) in files[file].summary.events.iter().enumerate() {
                time = found.t.unwrap_or(time);
                if !selected[session].contains(&(file, found.o)) {
                    continue;
                }
                native_refs.push((
                    (if members.len() > 1 { time } else { 0 }, rank, i),
                    (file, i),
                ));
                let kinds = if let Some(h) = placed.get(&(file, i)) {
                    h.iter().map(|h| EntryKind::H(*h)).collect::<Vec<_>>()
                } else {
                    match found.k {
                        Kind::U => Some(EntryKind::U),
                        Kind::A => Some(EntryKind::A),
                        Kind::Gap => Some(EntryKind::Gap),
                        Kind::Tool if found.poll.is_some() && found.script & events::SENDS == 0 => {
                            None
                        }
                        Kind::Tool if found.n.as_deref() != Some("SubagentHandback") => {
                            Some(EntryKind::Tool(tool_state(found)))
                        }
                        _ => None,
                    }
                    .into_iter()
                    .collect()
                };
                for kind in kinds {
                    keyed.push((
                        (
                            if members.len() > 1 { time } else { 0 },
                            rank,
                            found.o,
                            found.b,
                        ),
                        Entry {
                            kind,
                            file,
                            offset: found.o,
                            pos: (found.o, found.b),
                            t: found.t,
                            at: Some((file, i)),
                        },
                    ));
                }
            }
        }
        keyed.sort_by_key(|(key, _)| *key);
        let mut entries: Vec<_> = keyed.into_iter().map(|(_, entry)| entry).collect();
        if let Some(h) = heads.get(&session) {
            entries.insert(
                0,
                Entry {
                    kind: EntryKind::H(*h),
                    file: members[0],
                    offset: 0,
                    pos: (0, 0),
                    t: None,
                    at: None,
                },
            );
        }
        native_refs.sort_by_key(|(key, _)| *key);
        let refs: Vec<Ref> = native_refs.into_iter().map(|(_, at)| at).collect();
        let context = turns::TurnContext {
            files: &files,
            handoffs: &handoffs,
            session,
            key: &row.id,
            state: &states[session].0,
            waiting_for: states[session].1.as_deref(),
        };
        let latest_reply = refs
            .iter()
            .copied()
            .rev()
            .find(|at| event(&files, *at).k == Kind::A);
        let latest_ask = refs
            .iter()
            .rposition(|at| event(&files, *at).k == Kind::Ask);
        let result = latest_reply.filter(|reply| {
            let eligible = if files[source_files[&row.owner]].harness() == "codex" {
                states[session].0 != "work" && row.parent.is_none()
                    && members.iter().all(|file| files[*file].marker.is_none()
                        && matches!(&files[*file].role, Role::Codex(meta) if meta.parent_thread.is_none()))
            } else {
                states[session].0 == "idle"
                    && latest_ask.is_none_or(|ask| ask < refs.iter().position(|at| at == reply).unwrap())
            };
            eligible && context.reply_started_by_you(&entries, &refs, *reply,
                files[source_files[&row.owner]].harness() == "codex" && row.parent.is_none(), |at| {
                    let found = event(&files, at);
                    let spawned_here = |id: &str| {
                        refs.iter().any(|call| {
                            let call = event(&files, *call);
                            call.k == Kind::Tool && call.id.as_deref() == Some(id)
                                && matches!(call.n.as_deref(), Some("Agent" | "Task"))
                        }) || rows.iter().any(|child| child.id == id
                            && child.parent.as_deref() == Some(row.id.as_str()))
                    };
                    match found.k {
                        Kind::Agm if found.script & AGM_DOWN != 0 => placed.get(&at)
                            .is_some_and(|hs| hs.iter().all(|h| handoffs[*h].inline)),
                        Kind::Tn => found.id.as_deref().is_some_and(spawned_here),
                        Kind::Agm if found.script & AGM_FINAL != 0 =>
                            [found.id.as_deref(), found.n.as_deref()].into_iter().flatten().any(spawned_here),
                        _ => false,
                    }
                })
        });
        let inferred = result.map(|at| {
            let found = event(&files, at);
            Handoff {
                ask: Some("result"),
                ..Handoff::new(
                    stable_id(
                        "q",
                        &format!("toyou:result:{}:{}:{}", files[at.0].id, found.o, found.b),
                    ),
                    "toyou",
                    (row.id.clone(), "you".into()),
                    found.t.unwrap_or_else(|| {
                        refs.iter()
                            .filter_map(|at| event(&files, *at).t)
                            .max()
                            .unwrap()
                    }),
                    "new",
                    &assistant_text(&selected_record(&files, at), found.b as usize)
                        .unwrap_or_default(),
                )
            }
        });
        let expected_results: Vec<_> = expected
            .handoffs
            .iter()
            .filter(|h| h.from == row.id && h.kind == "toyou" && h.ask == Some("result"))
            .collect();
        assert_eq!(
            serde_json::to_value(inferred.iter().collect::<Vec<_>>()).unwrap(),
            serde_json::to_value(expected_results).unwrap(),
            "{} inferred human result",
            row.id
        );
        // Claude draws the inferred result immediately after its assistant entry;
        // root Codex results are catalog handoffs without a transcript placement.
        if files[source_files[&row.owner]].harness() == "claude"
            && let Some(out) = inferred
        {
            let at = result.unwrap();
            let position = entries
                .iter()
                .rposition(|entry| entry.at == Some(at))
                .unwrap()
                + 1;
            let h = handoffs.len();
            handoffs.push(H {
                out,
                from: Some(session),
                to: None,
                previews: None,
                preview_owners: BTreeMap::new(),
                inline: false,
            });
            let previous = &entries[position - 1];
            let entry = Entry {
                kind: EntryKind::H(h),
                file: previous.file,
                offset: previous.offset,
                pos: previous.pos,
                t: previous.t,
                at: None,
            };
            entries.insert(position, entry);
        }
        let context = turns::TurnContext {
            files: &files,
            handoffs: &handoffs,
            session,
            key: &row.id,
            state: &states[session].0,
            waiting_for: states[session].1.as_deref(),
        };
        let (mut turns, _, prompts) = context.split(&entries);
        for (turn, at) in prompts {
            let line = fs::read_to_string(&files[at.0].path).unwrap();
            let record: Value = serde_json::from_str(
                line[files[at.0].summary.events[at.1].o as usize..]
                    .lines()
                    .next()
                    .unwrap(),
            )
            .unwrap();
            turns[turn].text = prompt_text(&record).map(|s| cap(&s, MSG_MAX));
        }
        actual.insert(row.id.clone(), (states[session].0.clone(), turns));
    }
    actual
}

fn assert_selected_turns(warehouse: &Warehouse, home: &Home) {
    let actual = selected_turns(warehouse, home);
    let expected = home.build_query();
    for (id, (state, turns)) in actual {
        assert_eq!(state, expected.sessions[&id].state, "{id} latest state");
        assert_eq!(
            serde_json::to_value(&turns).unwrap(),
            serde_json::to_value(turns_of(&expected, &id)).unwrap(),
            "{id} complete causal turns"
        );
    }
}

#[test]
fn star_catalog_subagent_body_time_and_turn_arrival_parity() {
    for reverse in [false, true] {
        for legacy in [false, true] {
            let home = Home::new();
            let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
            let mut parent = vec![
                human("lead", ts(20, 0), "work"),
                assistant(
                    "lead",
                    ts(20, 1),
                    vec![tool(
                        "spawn",
                        "Agent",
                        json!({"prompt":"work","name":"worker"}),
                    )],
                ),
                result(
                    "lead",
                    ts(20, 1),
                    "spawn",
                    "launched",
                    false,
                    json!({"status":"async_launched","agentId":"child"}),
                ),
            ];
            // Body pass skips a wrong receipt, then the time pass claims it
            // between neighbouring exact claims. Same-position tags exercise
            // receipt body indexing without storing any body in SQL.
            if legacy {
                parent.push(agm_line(
                    3,
                    &[("other", "not this child"), ("child", "first")],
                ));
                parent.push(agm_line(5, &[("child", "escaped second")]));
                parent.push(agm_line(7, &[("child", "third")]));
                parent.push(agm_line(9, &[("child", "final answer")]));
            } else {
                parent.push(agent_send("lead", ts(20, 3), "child", "first"));
                parent.push(agent_send("lead", ts(20, 5), "child", "escaped second"));
                parent.push(agent_send("lead", ts(20, 7), "child", "third"));
                parent.push(hand_back("lead", ts(20, 9), "child", "final answer"));
            }
            parent.push(notification(
                "lead",
                ts(20, 10),
                "spawn",
                "completed",
                "final answer",
            ));
            let mut child = vec![user("lead", ts(20, 1), "work")];
            child.extend(send_to_main("first", 2, "first", queued_reply()));
            child.extend(send_to_main("second", 4, "second", queued_reply()));
            child.push(child_says(5, "still working"));
            child.extend(send_to_main("third", 6, "third", queued_reply()));
            child.push(child_says(8, "final answer"));
            for source in if reverse { [2, 1] } else { [1, 2] } {
                if source == 1 {
                    home.top("lead", &parent);
                    warehouse.source(1, "claude", "lead", &parent);
                } else {
                    home.agent("lead", "child", "spawn", &child);
                    warehouse.source(2, "claude", "child", &child);
                    warehouse
                        .db
                        .execute(
                            "INSERT INTO fact_agent_observation VALUES(2,'lead','spawn')",
                            [],
                        )
                        .unwrap();
                }
                assert_selected_turns(&warehouse, &home);
            }
            let built = home.build_query();
            assert_eq!(
                built
                    .handoffs
                    .iter()
                    .filter(|h| h.kind == "relay" && h.from == "child")
                    .count(),
                3
            );
            assert_eq!(
                only(&built, "spawn", "lead", "child").result.as_deref(),
                Some("final answer")
            );
        }
    }
}

#[test]
fn star_catalog_subagent_inline_pin_resume_turn_arrival_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let mut parent = vec![
            human("lead", ts(20, 0), "work"),
            assistant(
                "lead",
                ts(20, 1),
                vec![tool(
                    "spawn",
                    "Agent",
                    json!({"prompt":"work","name":"worker"}),
                )],
            ),
            result(
                "lead",
                ts(20, 1),
                "spawn",
                "launched",
                false,
                json!({"status":"async_launched","agentId":"child"}),
            ),
        ];
        parent.extend(send_to_agent(
            "inline",
            2,
            "worker [native label]",
            "also this",
            json!({"success":true,"pin":{"id":"child"}}),
        ));
        parent.push(hand_back("lead", ts(20, 4), "child", "first result"));
        parent.push(notification(
            "lead",
            ts(20, 5),
            "spawn",
            "completed",
            "first result",
        ));
        parent.extend(send_to_agent(
            "resume",
            6,
            "child",
            "again",
            resumed("child"),
        ));
        parent.push(hand_back("lead", ts(20, 9), "child", "second result"));
        let child = vec![
            user("lead", ts(20, 1), "work"),
            coordinator("lead", ts(20, 3), "also this"),
            assistant(
                "lead",
                ts(20, 4),
                vec![tool(
                    "back",
                    "SubagentHandback",
                    json!({"message":"first result"}),
                )],
            ),
            coordinator("lead", ts(20, 7), "again"),
            assistant(
                "lead",
                ts(20, 8),
                vec![tool(
                    "back2",
                    "SubagentHandback",
                    json!({"message":"second result"}),
                )],
            ),
        ];
        for source in if reverse { [2, 1] } else { [1, 2] } {
            if source == 1 {
                home.top("lead", &parent);
                warehouse.source(1, "claude", "lead", &parent);
            } else {
                home.agent("lead", "child", "spawn", &child);
                warehouse.source(2, "claude", "child", &child);
                warehouse
                    .db
                    .execute(
                        "INSERT INTO fact_agent_observation VALUES(2,'lead','spawn')",
                        [],
                    )
                    .unwrap();
            }
            assert_selected_turns(&warehouse, &home);
        }
        assert_eq!(turns_of(&home.build_query(), "child").len(), 2);
    }
}

#[test]
fn star_catalog_ambiguous_missing_time_cross_lineage_relay_parity() {
    for reverse in [false, true] {
        for missing_time in [false, true] {
            let home = Home::new();
            let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
            let mut parent = vec![
                human("lead", ts(20, 0), "work"),
                assistant(
                    "lead",
                    ts(20, 1),
                    vec![tool(
                        "spawn",
                        "Agent",
                        json!({"prompt":"work","name":"worker"}),
                    )],
                ),
                result(
                    "lead",
                    ts(20, 1),
                    "spawn",
                    "launched",
                    false,
                    json!({"status":"async_launched","agentId":"child"}),
                ),
                agent_send("lead", ts(20, 3), "child", "same up"),
                agent_send("lead", ts(20, 5), "child", "same up"),
                agent_send("lead", ts(20, 7), "child", "escaped up"),
            ];
            // Repeated bodies must consume successive native occurrences. An
            // unmatched body between them uses the bounded second pass.
            let mut child = vec![user("lead", ts(20, 1), "work")];
            child.extend(send_to_main("up-1", 2, "same up", queued_reply()));
            child.extend(send_to_main("up-2", 4, "same up", queued_reply()));
            child.extend(send_to_main("up-3", 6, "different up", queued_reply()));
            child.push(child_says(8, "still working"));
            for (id, minute, message) in [
                ("down-1", 9, "same down"),
                ("down-2", 11, "same down"),
                ("down-3", 13, "different down"),
            ] {
                parent.extend(send_to_agent(
                    id,
                    minute,
                    "worker",
                    message,
                    json!({"success":true,"pin":{"id":"child"}}),
                ));
            }
            child.extend([
                coordinator("lead", ts(20, 10), "same down"),
                coordinator("lead", ts(20, 12), "same down"),
                coordinator("lead", ts(20, 14), "escaped down"),
            ]);
            if missing_time {
                // Body equality can pair an untimed receipt; the time pass
                // cannot. Down pairing permits a send with unknown time.
                parent[3].as_object_mut().unwrap().remove("timestamp");
                parent[5].as_object_mut().unwrap().remove("timestamp");
                parent[6].as_object_mut().unwrap().remove("timestamp");
                child[1].as_object_mut().unwrap().remove("timestamp");
                // Both the second down send and its receipt are untimed;
                // the rendered relay uses the child's start as its fallback.
                parent[8].as_object_mut().unwrap().remove("timestamp");
                child[9].as_object_mut().unwrap().remove("timestamp");
            }
            // The same sender's progress received outside its parent lineage
            // must remain a separate relay, never consume the parent's send.
            let other = vec![
                human("other", ts(20, 0), "unrelated work"),
                agent_send("other", ts(20, 3), "child", "same up"),
                agent_send("other", ts(20, 7), "child", "escaped up"),
            ];
            for source in if reverse { [3, 2, 1] } else { [1, 2, 3] } {
                match source {
                    1 => {
                        home.top("lead", &parent);
                        warehouse.source(1, "claude", "lead", &parent);
                    }
                    2 => {
                        home.agent("lead", "child", "spawn", &child);
                        warehouse.source(2, "claude", "child", &child);
                        warehouse
                            .db
                            .execute(
                                "INSERT INTO fact_agent_observation VALUES(2,'lead','spawn')",
                                [],
                            )
                            .unwrap();
                    }
                    _ => {
                        home.top("other", &other);
                        warehouse.source(3, "claude", "other", &other);
                    }
                }
                assert_selected_turns(&warehouse, &home);
            }
            let built = home.build_query();
            assert_eq!(
                built
                    .handoffs
                    .iter()
                    .filter(|h| h.kind == "relay"
                        && h.from == "child"
                        && h.to.as_deref() == Some("other"))
                    .count(),
                2
            );
            assert_eq!(
                built
                    .handoffs
                    .iter()
                    .filter(|h| h.kind == "relay"
                        && h.from == "lead"
                        && h.to.as_deref() == Some("child"))
                    .count(),
                3
            );
        }
    }
}

#[test]
fn star_catalog_native_event_append_writes() {
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    warehouse
        .db
        .execute_batch(
            "CREATE TEMP TABLE projected_event(event INTEGER);
         CREATE TEMP TRIGGER record_projection AFTER INSERT ON fact_subagent_event
         BEGIN INSERT INTO projected_event VALUES(new.event); END;",
        )
        .unwrap();
    let mut records = (0..1000)
        .map(|i| human("sender", ts(0, 0), &format!("prompt {i}")))
        .collect::<Vec<_>>();
    records.push(assistant(
        "sender",
        ts(0, 1),
        vec![tool(
            "send",
            "SendMessage",
            json!({"recipient":"worker","content":"hello"}),
        )],
    ));
    warehouse.source(1, "claude", "sender", &records);
    let call: i64 = warehouse
        .db
        .query_row(
            "SELECT event FROM fact_subagent_event WHERE id='send'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    warehouse
        .db
        .execute("DELETE FROM projected_event", [])
        .unwrap();
    warehouse.source(
        1,
        "claude",
        "sender",
        &[result(
            "sender",
            ts(0, 2),
            "send",
            "result",
            false,
            json!("permission denied"),
        )],
    );
    let projected: Vec<i64> = warehouse
        .db
        .prepare("SELECT event FROM projected_event ORDER BY event")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    // A result revises its old call; unrelated historical prompts stay untouched.
    assert_eq!(projected, vec![call]);
    let (flags, failed): (u8, bool) = warehouse
        .db
        .query_row(
            "SELECT e.reply_flags,r.failed FROM fact_subagent_event e
         JOIN fact_relay_result r ON r.source=e.source AND r.call_id=e.id WHERE e.id='send'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(flags, DENIED);
    assert!(failed);
    warehouse
        .db
        .execute("DELETE FROM projected_event", [])
        .unwrap();
    warehouse.source(
        1,
        "claude",
        "sender",
        &[human("sender", ts(0, 3), "next prompt")],
    );
    let projected: i64 = warehouse
        .db
        .query_row("SELECT count(*) FROM projected_event", [], |r| r.get(0))
        .unwrap();
    assert_eq!(projected, 1);
    let plan: String = warehouse.db.query_row(
        "EXPLAIN QUERY PLAN UPDATE fact_relay_result SET failed=?3 WHERE source=?1 AND call_id=?2",
        params![1, "send", true], |r| r.get(3),
    ).unwrap();
    assert!(
        plan.contains("relay_result_call (source=? AND call_id=?)"),
        "{plan}"
    );
}

#[test]
fn star_catalog_native_send_flags_and_normalized_names() {
    for reverse in [false, true] {
        for (outcome, error, flag) in [
            (json!("permission denied"), false, DENIED),
            (json!({"success":false}), false, SEND_FAILED),
            (json!({}), true, 0),
            (json!({"success":true}), false, 0),
        ] {
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
                        json!({"recipient":" worker [native] ","content":"message"}),
                    )],
                ),
                result("sender", ts(0, 2), "send", "result", error, outcome),
            ];
            let receiver = vec![peer(
                "receiver",
                ts(0, 3),
                "unclaimed",
                "remote [native]",
                42,
                "orphan",
            )];
            for source in if reverse { [2, 1] } else { [1, 2] } {
                let (id, records) = if source == 1 {
                    ("sender", &sender)
                } else {
                    ("receiver", &receiver)
                };
                home.top(id, records);
                warehouse.source(source, "claude", id, records);
                assert_selected_turns(&warehouse, &home);
                let expected = home.build_query();
                if source == 1 || !reverse {
                    let (flags, failed, target): (u8, bool, String) = warehouse
                        .db
                        .query_row(
                            "SELECT e.reply_flags,r.failed,s.target FROM fact_subagent_event e
                         JOIN fact_relay_send s ON s.source=e.source AND s.call_id=e.id
                         JOIN fact_relay_result r ON r.source=s.source AND r.call_id=s.call_id",
                            [],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .unwrap();
                    assert_eq!(flags, flag);
                    assert_eq!(failed, error || flag != 0);
                    assert_eq!(target, "worker");
                    let relay = expected
                        .handoffs
                        .iter()
                        .find(|h| h.kind == "relay" && h.from == "sender")
                        .unwrap();
                    let (receiver, stub, failed): (Option<i64>, Option<String>, bool) = warehouse
                        .db
                        .query_row(
                            "SELECT receiver,stub,failed FROM relay_endpoints",
                            [],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .unwrap();
                    assert_eq!(receiver, None);
                    assert_eq!(stub.as_deref(), (!failed).then_some("name:worker"));
                    assert_eq!(relay.status, if failed { "err" } else { "done" });
                    if !failed {
                        assert_eq!(
                            relay.to.as_deref(),
                            Some(stable_id("peer-", "name:worker").as_str())
                        );
                    }
                }
                if source == 2 || reverse {
                    let orphan = expected
                        .handoffs
                        .iter()
                        .find(|h| h.kind == "relay" && h.to.as_deref() == Some("receiver"))
                        .unwrap();
                    let (pid,start,sock):(Option<i64>,Option<String>,Option<String>)=warehouse.db.query_row(
                        "SELECT peer_pid,peer_start,peer_sock FROM fact_subagent_event WHERE kind='xsm'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                    let identity = if let Some(pid) = pid {
                        format!("pid:{pid}:{}", start.unwrap_or_default())
                    } else if let Some(sock) = sock {
                        format!("sock:{sock}")
                    } else {
                        "name:remote".into()
                    };
                    assert_eq!(orphan.from, stable_id("peer-", &identity));
                    assert!(orphan.unmatched);
                }
            }
        }
    }
}

#[test]
fn star_catalog_causal_turn_arrival_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let old = vec![
            uuid(human("old", ts(1, 0), "first"), "prompt"),
            uuid(assistant("old", ts(1, 1), vec![text("answer")]), "answer"),
            json!({"type":"continued-in","continuedInSessionId":"new"}),
        ];
        let new = vec![
            old[0].clone(),
            old[1].clone(),
            human("new", ts(2, 0), "second"),
            assistant(
                "new",
                ts(2, 1),
                vec![tool("late", "Read", json!({"file_path":"/x"}))],
            ),
            human("new", ts(3, 0), "third"),
        ];
        let mut records = BTreeMap::from([(1, old), (2, new)]);
        for source in if reverse { [2, 1] } else { [1, 2] } {
            let id = if source == 1 { "old" } else { "new" };
            home.top(id, &records[&source]);
            warehouse.source(source, "claude", id, &records[&source]);
            assert_selected_turns(&warehouse, &home);
        }
        // A late result attaches to the earlier call after a newer prompt;
        // its recorded clock is earlier than the most recently admitted record.
        let late = result("new", ts(2, 2), "late", "failed", true, json!({}));
        records.get_mut(&2).unwrap().push(late.clone());
        home.top("new", &records[&2]);
        warehouse.source(2, "claude", "new", &[late]);
        assert_selected_turns(&warehouse, &home);
        assert_eq!(
            turns_of(&home.build_query(), "old")[1].end.why,
            "failed_step"
        );
    }
}

#[test]
fn star_catalog_exact_relay_turn_arrival_parity() {
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
                    json!({"to":"worker","message":"hello"}),
                )],
            ),
        ];
        let receiver = vec![
            peer("receiver", ts(0, 3), "message", "sender", 111, "hello"),
            assistant("receiver", ts(0, 4), vec![text("received")]),
        ];
        let mut records = BTreeMap::from([(1, sender), (2, receiver)]);
        for source in if reverse { [2, 1] } else { [1, 2] } {
            let id = if source == 1 { "sender" } else { "receiver" };
            home.top(id, &records[&source]);
            warehouse.source(source, "claude", id, &records[&source]);
            assert_selected_turns(&warehouse, &home);
        }
        let late = result(
            "sender",
            ts(0, 2),
            "send",
            "sent",
            false,
            json!({"success":true,"msg_id":"message"}),
        );
        records.get_mut(&1).unwrap().push(late.clone());
        home.top("sender", &records[&1]);
        warehouse.source(1, "claude", "sender", &[late]);
        assert_selected_turns(&warehouse, &home);
        assert_eq!(
            turns_of(&home.build_query(), "sender")[0].end.why,
            "no_reply"
        );
    }
}

#[test]
fn star_catalog_spawn_turn_arrival_and_revocation_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let call = |id: &str| {
            codex_line(
                ts(1, 1),
                "response_item",
                json!({"type":"function_call","name":"spawn_agent","call_id":id,"arguments":"{\"task_name\":\"solo\"}"}),
            )
        };
        let parent = codex_records(
            "root",
            None,
            None,
            &[codex_user(ts(1, 0), "coordinate"), call("spawn")],
        );
        let child = codex_records(
            "child",
            Some("root"),
            Some("solo"),
            &[
                codex_line(
                    ts(1, 2),
                    "event_msg",
                    json!({"type":"task_started","turn_id":"t"}),
                ),
                codex_user(ts(1, 2), "work"),
                codex_reply(ts(1, 3), "done"),
                codex_line(
                    ts(1, 4),
                    "event_msg",
                    json!({"type":"task_complete","turn_id":"t"}),
                ),
            ],
        );
        let mut records = BTreeMap::from([(1, parent), (2, child)]);
        for source in if reverse { [2, 1] } else { [1, 2] } {
            let id = if source == 1 { "root" } else { "child" };
            home.lines(
                &format!("codex/sessions/rollout-{id}.jsonl"),
                &records[&source],
            );
            warehouse.source(source, "codex", id, &records[&source]);
            assert_selected_turns(&warehouse, &home);
        }
        assert_eq!(
            turns_of(&home.build_query(), "child")[0].end.why,
            "returned"
        );
        let duplicate = call("duplicate");
        records.get_mut(&1).unwrap().push(duplicate.clone());
        home.lines("codex/sessions/rollout-root.jsonl", &records[&1]);
        warehouse.source(1, "codex", "root", &[duplicate]);
        assert_selected_turns(&warehouse, &home);
        assert!(turns_of(&home.build_query(), "root")[0].sent.is_empty());
    }
}

#[test]
fn star_catalog_gap_turn_parity() {
    let home = Home::new();
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    let first = vec![
        human("gap", ts(1, 0), "first"),
        assistant("gap", ts(1, 1), vec![text("answer")]),
    ];
    let path = home.top("gap", &first);
    warehouse.source(1, "claude", "gap", &first);
    assert_selected_turns(&warehouse, &home);
    // The native parser records a gap for a malformed complete line. Keep its
    // source locator in the fact selection without treating it as a JSON body.
    let pos = fs::metadata(&path).unwrap().len();
    let gap = "{not json\n";
    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(gap);
    fs::write(&path, &body).unwrap();
    warehouse
        .db
        .execute(
            "INSERT INTO fact_record VALUES(1,?1,'gap',NULL,NULL,NULL)",
            [i64::try_from(pos).unwrap()],
        )
        .unwrap();
    warehouse
        .db
        .execute(
            "UPDATE index_advance SET complete_bytes=?1 WHERE source=1",
            [i64::try_from(pos + gap.len() as u64).unwrap()],
        )
        .unwrap();
    assert_selected_turns(&warehouse, &home);
    let after = assistant(
        "gap",
        ts(1, 2),
        vec![tool("never", "Read", json!({"file_path":"/x"}))],
    );
    body.push_str(&after.to_string());
    body.push('\n');
    fs::write(&path, body).unwrap();
    warehouse.source(1, "claude", "gap", &[after]);
    assert_selected_turns(&warehouse, &home);
    let built = home.build_query();
    let turns = turns_of(&built, "gap");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].end.why, "unfinished_step");
}

#[test]
fn star_catalog_human_result_relay_arrival_parity() {
    for reverse in [false, true] {
        for progress in [false, true] {
            for notification_only in [false, true] {
                let home = Home::new();
                let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
                let mut parent = vec![
                    human("lead", ts(20, 0), "delegate"),
                    assistant(
                        "lead",
                        ts(20, 1),
                        vec![tool("spawn", "Agent", json!({"prompt":"work"}))],
                    ),
                    result(
                        "lead",
                        ts(20, 1),
                        "spawn",
                        "launched",
                        false,
                        json!({"status":"async_launched","agentId":"child"}),
                    ),
                ];
                parent.push(if notification_only {
                    notification("lead", ts(20, 3), "spawn", "completed", "done")
                } else if progress {
                    agent_send("lead", ts(20, 3), "child", "update")
                } else {
                    hand_back("lead", ts(20, 3), "child", "done")
                });
                parent.push(assistant(
                    "lead",
                    ts(20, 4),
                    vec![text("Here is the answer.")],
                ));
                let mut child = vec![user("lead", ts(20, 1), "work")];
                if progress {
                    child.extend(send_to_main("up", 2, "update", queued_reply()));
                } else {
                    child.push(child_says(2, "done"));
                }
                home.live(101, "lead", "idle", json!({"startedAt":at(20, 2)}));
                // The observation is source-scoped even if the source has not arrived.
                warehouse
                    .db
                    .execute(
                        "INSERT INTO qualified_turn_runtime VALUES(1,'idle',NULL)",
                        [],
                    )
                    .unwrap();
                for source in if reverse { [2, 1] } else { [1, 2] } {
                    if source == 1 {
                        home.top("lead", &parent);
                        warehouse.source(1, "claude", "lead", &parent);
                    } else {
                        home.agent("lead", "child", "spawn", &child);
                        warehouse.source(2, "claude", "child", &child);
                        warehouse
                            .db
                            .execute(
                                "INSERT INTO fact_agent_observation VALUES(2,'lead','spawn')",
                                [],
                            )
                            .unwrap();
                    }
                    assert_selected_turns(&warehouse, &home);
                }
                let expected = home.build_query();
                assert_eq!(
                    expected
                        .handoffs
                        .iter()
                        .filter(|h| h.from == "lead" && h.ask == Some("result"))
                        .count(),
                    usize::from(!progress || notification_only)
                );
                // Appending a tool completion after the reply, with an earlier
                // native clock, must preserve the current attribution decision.
                parent.push(assistant(
                    "lead",
                    ts(20, 5),
                    vec![tool("late", "Read", json!({"file_path":"/x"}))],
                ));
                let call = parent.last().unwrap().clone();
                home.top("lead", &parent);
                warehouse.source(1, "claude", "lead", &[call]);
                assert_selected_turns(&warehouse, &home);
                let late = result("lead", ts(20, 2), "late", "failed", true, json!({}));
                parent.push(late.clone());
                home.top("lead", &parent);
                warehouse.source(1, "claude", "lead", &[late]);
                assert_selected_turns(&warehouse, &home);
            }
        }
    }
}

#[test]
fn star_catalog_resumed_live_runtime_arrival_parity() {
    for reverse in [false, true] {
        for status in ["busy", "waiting", "idle"] {
            let home = Home::new();
            let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
            let mut parent = vec![
                human("lead", ts(20, 0), "work"),
                assistant(
                    "lead",
                    ts(20, 1),
                    vec![tool("spawn", "Agent", json!({"prompt":"work"}))],
                ),
                result(
                    "lead",
                    ts(20, 1),
                    "spawn",
                    "launched",
                    false,
                    json!({"status":"async_launched","agentId":"child"}),
                ),
                hand_back("lead", ts(20, 4), "child", "first result"),
            ];
            parent.extend(send_to_agent(
                "resume",
                6,
                "child",
                "again",
                resumed("child"),
            ));
            let mut child = vec![
                user("lead", ts(20, 1), "work"),
                assistant(
                    "lead",
                    ts(20, 4),
                    vec![tool(
                        "back",
                        "SubagentHandback",
                        json!({"message":"first result"}),
                    )],
                ),
                coordinator("lead", ts(20, 7), "again"),
                assistant(
                    "lead",
                    ts(20, 8),
                    vec![tool("read", "Read", json!({"file_path":"/x"}))],
                ),
            ];
            let waiting = (status == "waiting").then_some("permission prompt");
            let state = match status {
                "busy" => "work",
                "waiting" => "wait",
                _ => "idle",
            };
            home.live(
                101,
                "lead",
                status,
                if let Some(waiting) = waiting {
                    json!({"startedAt":at(20, 5),"waitingFor":waiting})
                } else {
                    json!({"startedAt":at(20, 5)})
                },
            );
            warehouse
                .db
                .execute(
                    "INSERT INTO qualified_turn_runtime VALUES(1,?1,?2)",
                    params![state, waiting],
                )
                .unwrap();
            for source in if reverse { [2, 1] } else { [1, 2] } {
                if source == 1 {
                    home.top("lead", &parent);
                    warehouse.source(1, "claude", "lead", &parent);
                } else {
                    home.agent("lead", "child", "spawn", &child);
                    warehouse.source(2, "claude", "child", &child);
                    warehouse
                        .db
                        .execute(
                            "INSERT INTO fact_agent_observation VALUES(2,'lead','spawn')",
                            [],
                        )
                        .unwrap();
                }
                assert_selected_turns(&warehouse, &home);
            }
            assert_eq!(home.build_query().sessions["child"].state, "work");
            // The resumed process exits with the native run still open.
            fs::remove_file(home.root.join("claude/sessions/101.json")).unwrap();
            warehouse
                .db
                .execute("DELETE FROM qualified_turn_runtime", [])
                .unwrap();
            assert_selected_turns(&warehouse, &home);
            assert_eq!(home.build_query().sessions["child"].state, "done");
            // A new qualified process resumes that same source before a late,
            // earlier-clock completion of the pending tool is admitted.
            home.live(102, "lead", "busy", json!({"startedAt":at(20, 9)}));
            warehouse
                .db
                .execute(
                    "INSERT INTO qualified_turn_runtime VALUES(1,'work',NULL)",
                    [],
                )
                .unwrap();
            assert_selected_turns(&warehouse, &home);
            let late = result("lead", ts(20, 8), "read", "done", false, json!({}));
            child.push(late.clone());
            home.agent("lead", "child", "spawn", &child);
            warehouse.source(2, "claude", "child", &[late]);
            assert_selected_turns(&warehouse, &home);
            let back = assistant(
                "lead",
                ts(20, 10),
                vec![tool(
                    "back2",
                    "SubagentHandback",
                    json!({"message":"second result"}),
                )],
            );
            child.push(back.clone());
            home.agent("lead", "child", "spawn", &child);
            warehouse.source(2, "claude", "child", &[back]);
            assert_selected_turns(&warehouse, &home);
            assert_eq!(home.build_query().sessions["child"].state, "done");
        }
    }
}

#[cfg(unix)]
#[test]
fn star_catalog_resumed_live_yield_turn_parity() {
    let home = Home::new();
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    let lifecycle = |minute, kind: &str, id: &str| {
        codex_line(
            ts(minute, 0),
            "event_msg",
            json!({"type":kind,"turn_id":id}),
        )
    };
    let output = |minute, id: &str, done| {
        codex_line(
            ts(minute, 2),
            "response_item",
            json!({"type":"custom_tool_call_output","call_id":id,"output":[
                {"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type":"input_text","text":json!({"wall_time_seconds":1.0,"session_id":4242,
                    "exit_code":if done {Some(0)} else {None},"output":"listening\n"}).to_string()},
            ]}),
        )
    };
    let mut records = codex_records(
        "serving",
        None,
        None,
        &[
            lifecycle(1, "task_started", "first"),
            codex_user(ts(1, 0), "start"),
            codex_line(
                ts(1, 1),
                "response_item",
                json!({"type":"custom_tool_call","call_id":"start","name":"exec",
            "input":"const r = await tools.exec_command({cmd:\"serve\",yield_time_ms:1000});\ntext(JSON.stringify(r));"}),
            ),
            output(1, "start", false),
            lifecycle(2, "task_complete", "first"),
            lifecycle(3, "task_started", "second"),
            codex_user(ts(3, 0), "check on it"),
        ],
    );
    home.lines("codex/sessions/rollout-serving.jsonl", &records);
    warehouse.source(1, "codex", "serving", &records);
    hold_lock(&home, "serving");
    warehouse
        .db
        .execute(
            "INSERT INTO qualified_turn_runtime VALUES(1,'work',NULL)",
            [],
        )
        .unwrap();
    assert_selected_turns(&warehouse, &home);
    assert_eq!(
        turns_of(&home.build_query(), "serving")[0].end.why,
        "no_reply"
    );
    home.write("proc/locks", "");
    warehouse
        .db
        .execute("DELETE FROM qualified_turn_runtime", [])
        .unwrap();
    assert_selected_turns(&warehouse, &home);
    assert_eq!(
        turns_of(&home.build_query(), "serving")[0].end.why,
        "unfinished_step"
    );
    hold_lock(&home, "serving");
    warehouse
        .db
        .execute(
            "INSERT INTO qualified_turn_runtime VALUES(1,'work',NULL)",
            [],
        )
        .unwrap();
    assert_selected_turns(&warehouse, &home);
    // A later run completes the earlier yielded command at read time.
    let poll = codex_line(
        ts(3, 1),
        "response_item",
        json!({"type":"custom_tool_call","call_id":"poll","name":"exec",
        "input":"const r = await tools.write_stdin({session_id:4242,chars:\"\",yield_time_ms:30000});\ntext(JSON.stringify(r));"}),
    );
    let completion = output(3, "poll", true);
    records.extend([poll.clone(), completion.clone()]);
    home.lines("codex/sessions/rollout-serving.jsonl", &records);
    warehouse.source(1, "codex", "serving", &[poll, completion]);
    assert_selected_turns(&warehouse, &home);
    assert_ne!(
        turns_of(&home.build_query(), "serving")[0].end.why,
        "unfinished_step"
    );
}

#[test]
fn star_catalog_qualified_runtime_turn_state_parity() {
    let home = Home::new();
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    let records = vec![
        human("live", ts(1, 0), "work"),
        assistant(
            "live",
            ts(1, 1),
            vec![tool("read", "Read", json!({"file_path":"/x"}))],
        ),
    ];
    home.top("live", &records);
    warehouse.source(1, "claude", "live", &records);
    // These are independently qualified observations, analogous to the process
    // fixtures above. Acquisition/expiry is a production producer gate.
    for (status, extra, state, waiting) in [
        ("busy", json!({}), "work", None),
        (
            "waiting",
            json!({"waitingFor":"permission prompt"}),
            "wait",
            Some("permission prompt"),
        ),
        (
            "waiting",
            json!({"waitingFor":"input needed"}),
            "wait",
            Some("input needed"),
        ),
        ("idle", json!({}), "idle", None),
    ] {
        home.live(101, "live", status, extra);
        warehouse
            .db
            .execute(
                "INSERT OR REPLACE INTO qualified_turn_runtime VALUES(1,?1,?2)",
                params![state, waiting],
            )
            .unwrap();
        assert_selected_turns(&warehouse, &home);
    }
    fs::remove_file(home.root.join("claude/sessions/101.json")).unwrap();
    warehouse
        .db
        .execute("DELETE FROM qualified_turn_runtime", [])
        .unwrap();
    assert_selected_turns(&warehouse, &home);
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

// Fold the builder's per-owner evidence in its actual leaf-queue order. The
// family activity cost below independently checks that these token observations
// select the same native step as the production analytics fold.
fn assert_family_cost_parity(home: &Home, warehouse: &Warehouse) {
    let built = home.build_query();
    let costs = crate::causal_trace::family_costs(
        built
            .sessions
            .iter()
            .map(|(id, session)| (id.as_str(), session.parent.as_deref(), session.cost.usd)),
    );
    for row in warehouse.list() {
        let (own, family): (Option<f64>, Option<f64>) = warehouse.db.query_row(
            "SELECT s.usd,f.family_usd FROM session_cost s JOIN family_cost f ON f.root=s.owner WHERE s.owner=?1",
            [row.owner], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        for (label, actual, expected) in [
            ("session", own, built.sessions[&row.id].cost.usd),
            ("family", family, costs[row.id.as_str()]),
        ] {
            match (actual, expected) {
                (Some(actual), Some(expected)) => assert!(
                    (actual - expected).abs() < 1e-12,
                    "{} {label} USD: {actual} vs {expected}",
                    row.id,
                ),
                _ => assert_eq!(actual, expected, "{} {label} USD", row.id),
            }
        }
    }
}

fn assert_shared_family_parity(home: &Home, warehouse: &Warehouse) {
    assert_family_cost_parity(home, warehouse);
    use sha2::{Digest, Sha256};
    let built = home.build_query();
    let rows = warehouse.list();
    let mut pending: BTreeMap<_, usize> = built.sessions.keys().map(|id| (id, 0)).collect();
    let mut folds = BTreeMap::new();
    for (id, session) in &built.sessions {
        if let Some(count) = session.parent.as_ref().and_then(|id| pending.get_mut(id)) {
            *count += 1;
        }
        folds.insert(
            id,
            session
                .claude_usage
                .as_ref()
                .map_or_else(BTreeMap::new, |e| {
                    e.shared
                        .iter()
                        .map(|(key, tokens)| {
                            (
                                key.clone(),
                                [tokens.input, tokens.cached_input, tokens.output],
                            )
                        })
                        .collect()
                }),
        );
    }
    let mut ready: std::collections::VecDeque<_> = pending
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect();
    while let Some(id) = ready.pop_front() {
        let expected = folds.remove(id).unwrap();
        let owner = rows.iter().find(|row| &row.id == id).unwrap().owner;
        let mut actual = BTreeMap::new();
        let mut cost = 0.0;
        let mut unknown = 0;
        let mut statement = warehouse.db.prepare(
            "SELECT message_id,evidence_uuid,source,pos,input,cached,output FROM family_shared_usage WHERE root=?1",
        ).unwrap();
        let observations = statement
            .query_map([owner], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    [
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ],
                ))
            })
            .unwrap();
        for observation in observations {
            let (message, uuid, source, pos, tokens) = observation.unwrap();
            let key = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(message.clone(), uuid)).unwrap())
            );
            actual.insert(key, tokens.map(|n| u64::try_from(n).unwrap()));
            // Resolve the SQL-selected native position through the builder's
            // parsed source usage; no rates or billing rules are duplicated.
            let native: String = warehouse
                .db
                .query_row(
                    "SELECT native_id FROM dim_session WHERE source=?1",
                    [source],
                    |r| r.get(0),
                )
                .unwrap();
            let path = home
                .options
                .claude_home
                .join(format!("projects/-work-proj/{native}.jsonl"));
            let mut cache = EventCache::open(&home.options.cache).unwrap();
            let index = events::scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            let used = &index.usage()[&message];
            assert_eq!(used.offset, u64::try_from(pos).unwrap());
            let mut priced = crate::pricing::CostBuild::with_table(crate::pricing::snapshot());
            priced.message(used);
            if let Some(value) = priced.finish_step().list {
                cost += value;
            } else {
                unknown += 1;
            }
        }
        assert_eq!(actual, expected, "{id} shared tokens");
        let (sql_cost, sql_unknown): (f64, i64) = warehouse
            .db
            .query_row(
                "SELECT known,unknown_steps FROM family_shared_cost WHERE root=?1",
                [owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap()
            .unwrap_or((0.0, 0));
        assert!((sql_cost - cost).abs() < 1e-12, "{id} SQL shared price");
        assert_eq!(
            u64::try_from(sql_unknown).unwrap(),
            unknown,
            "{id} SQL shared coverage"
        );
        if let Some(family) = built.sessions[id]
            .activity_analytics
            .as_ref()
            .and_then(|a| a.family.as_ref())
        {
            let measured = &family.counts.totals.shared.list_cost_usd;
            assert!(
                (cost - measured.known).abs() < 1e-12,
                "{id} shared cost: {cost} vs {}",
                measured.known
            );
            assert_eq!(unknown, measured.unknown_steps, "{id} shared cost coverage");
        }
        // Retain the parent's own evidence before merging the child fold.
        if let Some(parent) = built.sessions[id]
            .parent
            .as_ref()
            .filter(|p| built.sessions.contains_key(*p))
        {
            let target = folds.get_mut(parent).unwrap();
            for (key, values) in expected {
                target.entry(key).or_insert(values);
            }
            let count = pending.get_mut(parent).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.push_back(parent);
            }
        }
    }
}

#[test]
fn star_catalog_family_dollar_arrival_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let inputs = [
            (1, "root", None),
            (2, "child", Some("root")),
            (3, "late-leaf", Some("child")),
            (4, "empty", Some("root")),
        ];
        for index in if reverse { [3, 2, 1, 0] } else { [0, 1, 2, 3] } {
            let (source, id, parent) = inputs[index];
            let prompt =
                parent.map_or("Work".into(), |p| format!("Semon-Parent: claude:{p}\nWork"));
            let mut records = vec![human(id, ts(0, source), &prompt)];
            if id != "empty" {
                records.push(assistant_usage(
                    id, ts(0, 10), &format!("message-{id}"), "claude-sonnet-4-6",
                    json!({"input_tokens":100,"cache_read_input_tokens":30,
                        "cache_creation_input_tokens":40,
                        "cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":30},
                        "output_tokens":20}),
                ));
            }
            home.top(id, &records);
            warehouse.source(source, "claude", id, &records);
            assert_family_cost_parity(&home, &warehouse);
        }
        let total: f64 = warehouse
            .db
            .query_row("SELECT family_usd FROM family_cost WHERE root=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(total > 0.0);
        // A later native report replaces the invocation price, rather than
        // adding a second charge. Unknown rates then poison every ancestor.
        for model in ["claude-sonnet-4-6", "unknown-fixture-model"] {
            let records = [assistant_usage(
                "late-leaf",
                ts(0, 15),
                "message-late-leaf",
                model,
                json!({"input_tokens":250,"output_tokens":50}),
            )];
            append_records(&home, "late-leaf", &records);
            warehouse.source(3, "claude", "late-leaf", &records);
            assert_family_cost_parity(&home, &warehouse);
        }
        assert_eq!(
            warehouse
                .db
                .query_row("SELECT family_usd FROM family_cost WHERE root=1", [], |r| r
                    .get::<_, Option<f64>>(0),)
                .unwrap(),
            None
        );
    }
}

#[test]
fn star_catalog_shared_family_parity() {
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
            assert_shared_family_parity(&home, &warehouse);
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
fn star_catalog_divergent_shared_family_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        // Root has no shared observation initially. a wins before a-tail
        // arrives; that late child delays a's fold, so leaf z then wins.
        // Source numbers deliberately disagree with both keys and tree order.
        let inputs = [
            (4, "root", None, None),
            (3, "a", Some("root"), Some((11, 2, 3))),
            (1, "z", Some("root"), Some((23, 5, 7))),
            (2, "a-tail", Some("a"), Some((79, 13, 19))),
        ];
        let observation = |id: &str, input, cached, output| {
            uuid(
                assistant_usage(
                    id,
                    ts(0, 20),
                    "divergent",
                    "claude-sonnet-4-6",
                    json!({"input_tokens":input,"cache_read_input_tokens":cached,"output_tokens":output}),
                ),
                "same-uuid",
            )
        };
        for index in if reverse { [3, 2, 1, 0] } else { [0, 1, 2, 3] } {
            let (source, id, parent, tokens) = inputs[index];
            let prompt =
                parent.map_or("Work".into(), |p| format!("Semon-Parent: claude:{p}\nWork"));
            let mut records = vec![human(id, ts(0, source), &prompt)];
            if let Some((input, cached, output)) = tokens {
                records.push(observation(id, input, cached, output));
            }
            home.top(id, &records);
            warehouse.source(source, "claude", id, &records);
            assert_shared_family_parity(&home, &warehouse);
            if !reverse && index == 2 {
                assert_eq!(
                    warehouse
                        .db
                        .query_row(
                            "SELECT input FROM family_shared_usage WHERE root=4",
                            [],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap(),
                    13
                );
            }
        }
        assert_eq!(
            warehouse
                .db
                .query_row(
                    "SELECT input FROM family_shared_usage WHERE root=4",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
            28
        );
        // The winning owner's later native report replaces all categories
        // together, including cost. The other owner's larger values cannot win.
        for (source, id, input, cached, output) in [
            (1, "z", 31, 7, 11),
            (3, "a", 101, 17, 23),
            (4, "root", 43, 11, 13),
        ] {
            let records = [observation(id, input, cached, output)];
            append_records(&home, id, &records);
            warehouse.source(source, "claude", id, &records);
            assert_shared_family_parity(&home, &warehouse);
        }
        // A late parent observation takes precedence over every descendant,
        // even though a has the first native ID and z the first source ID.
        assert_eq!(
            warehouse
                .db
                .query_row(
                    "SELECT input,cached,output FROM family_shared_usage WHERE root=4",
                    [],
                    |r| Ok([
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?
                    ]),
                )
                .unwrap(),
            [54, 11, 13]
        );
        let mut unpriced = observation("root", 43, 11, 13);
        unpriced["message"]["model"] = json!("unknown-fixture-model");
        append_records(&home, "root", &[unpriced.clone()]);
        warehouse.source(4, "claude", "root", &[unpriced]);
        assert_shared_family_parity(&home, &warehouse);
        assert_eq!(
            home.build_query().sessions["root"]
                .activity_analytics
                .as_ref()
                .unwrap()
                .family
                .as_ref()
                .unwrap()
                .counts
                .totals
                .shared
                .list_cost_usd
                .unknown_steps,
            1
        );
    }
}

#[test]
fn star_catalog_shared_family_queue_tie_parity() {
    for reverse in [false, true] {
        let home = Home::new();
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        // Both subtrees have height one. b is ready before a because its
        // deepest leaf x precedes y, despite a preceding b in catalog order.
        let inputs = [
            (1, "root", None, None),
            (2, "a", Some("root"), Some(11)),
            (3, "b", Some("root"), Some(23)),
            (4, "y", Some("a"), Some(37)),
            (5, "x", Some("b"), Some(51)),
        ];
        for index in if reverse {
            [4, 3, 2, 1, 0]
        } else {
            [0, 1, 2, 3, 4]
        } {
            let (source, id, parent, tokens) = inputs[index];
            let prompt =
                parent.map_or("Work".into(), |p| format!("Semon-Parent: claude:{p}\nWork"));
            let mut records = vec![human(id, ts(0, source), &prompt)];
            if let Some(tokens) = tokens {
                records.push(uuid(
                    assistant_usage(
                        id,
                        ts(0, 20),
                        "shared-message",
                        "claude-sonnet-4-6",
                        json!({"input_tokens":tokens,"output_tokens":tokens/2}),
                    ),
                    "same-uuid",
                ));
            }
            home.top(id, &records);
            warehouse.source(source, "claude", id, &records);
            assert_shared_family_parity(&home, &warehouse);
        }
        assert_eq!(
            warehouse
                .db
                .query_row(
                    "SELECT input FROM family_shared_usage WHERE root=1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
            23
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

// Rebuild every derived answer from all facts independently of incremental state.
fn assert_full_recompute(warehouse: &Warehouse) {
    let snapshot = |sql: &str| {
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
    let oracle = include_str!("star_catalog/full_recompute.sql")
        .split(';')
        .filter(|sql| !sql.trim().is_empty());
    for (expected, actual) in oracle.zip([
        "SELECT * FROM dim_session ORDER BY source",
        "SELECT * FROM dim_lineage ORDER BY source",
        "SELECT * FROM bridge_session_owner ORDER BY source",
        "SELECT * FROM bridge_spawn_placement ORDER BY child",
    ]) {
        assert_eq!(snapshot(actual), snapshot(expected), "{actual}");
    }
}

#[test]
fn star_catalog_list_does_not_price_or_rewrite_billing() {
    let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    warehouse.source(
        1,
        "claude",
        "priced",
        &[assistant_usage(
            "priced",
            ts(0, 1),
            "message",
            "claude-sonnet-4-5",
            json!({"input_tokens":1000,"output_tokens":100}),
        )],
    );
    warehouse
        .db
        .execute_batch(
            "CREATE TEMP TRIGGER billing_insert BEFORE INSERT ON fact_step_billing
         BEGIN SELECT RAISE(ABORT,'non-usage append inserted billing'); END;
         CREATE TEMP TRIGGER billing_update BEFORE UPDATE ON fact_step_billing
         BEGIN SELECT RAISE(ABORT,'non-usage append updated billing'); END;
         CREATE TEMP TRIGGER billing_delete BEFORE DELETE ON fact_step_billing
         BEGIN SELECT RAISE(ABORT,'non-usage append deleted billing'); END;",
        )
        .unwrap();
    warehouse.source(1, "claude", "priced", &[json!({"type":"compact_boundary"})]);
    warehouse
        .db
        .create_scalar_function(
            "semon_step_usd",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| -> rusqlite::Result<Option<f64>> { panic!("list must not invoke dollar pricing") },
        )
        .unwrap();
    let rows = warehouse.list();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tokens, [1000, 0, 100]);
}

#[test]
fn star_catalog_incremental_maintenance_plans() {
    let warehouse = Warehouse::new(std::path::Path::new(":memory:"));
    for sql in MAINTAIN.split(';').filter(|sql| !sql.trim().is_empty()) {
        let mut statement = warehouse
            .db
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        for name in [":source", ":after"] {
            if let Some(index) = statement.parameter_index(name).unwrap() {
                statement.raw_bind_parameter(index, 1_i64).unwrap();
            }
        }
        let mut rows = statement.raw_query();
        while let Some(row) = rows.next().unwrap() {
            let step: String = row.get(3).unwrap();
            assert!(!step.contains("SCAN fact_"), "{step}");
            assert!(
                !step.starts_with("SCAN c USING INDEX spawn_call_name")
                    && !step.starts_with("SCAN child "),
                "{step}"
            );
            if step.contains("SEARCH fact_record")
                || step.contains("SEARCH fact_step_observation")
                || step.contains("SEARCH fact_relation_observation")
                || step.contains("SEARCH fact_tool_call")
            {
                assert!(step.contains("pos>?"), "suffix scan lost: {step}");
            }
        }
    }
}

#[test]
fn star_catalog_incremental_random_splits_and_order() {
    let inputs = [
        (
            1,
            "claude",
            "a",
            vec![
                human("a", ts(0, 5), "start"),
                json!({"type":"bridge-session","bridgeSessionId":"pending"}),
                json!({"type":"custom-title","customTitle":"first"}),
                assistant_usage("a", ts(0, 6), "msg", "model-one", json!({"input_tokens":1})),
                json!({"type":"compact_boundary"}),
                json!({"type":"continued-in","continuedInSessionId":"b"}),
                // Late observation with an earlier timestamp changes the owner.
                human("a", ts(0, 0), "late first"),
                json!({"type":"custom-title","customTitle":"latest"}),
            ],
        ),
        (
            2,
            "claude",
            "b",
            vec![
                human("b", ts(0, 2), "b"),
                json!({"type":"continued-in","continuedInSessionId":"c"}),
            ],
        ),
        (
            3,
            "claude",
            "c",
            vec![
                human("c", ts(0, 3), "c"),
                json!({"type":"bridge-session","bridgeSessionId":"other"}),
            ],
        ),
        (
            4,
            "claude",
            "d",
            vec![
                human("d", ts(0, 1), "d"),
                json!({"type":"bridge-session","bridgeSessionId":"other"}),
                // Join two already populated groups, whose owners differ.
                json!({"type":"bridge-session","bridgeSessionId":"pending"}),
            ],
        ),
        (
            5,
            "codex",
            "parent",
            codex_records(
                "parent",
                None,
                None,
                &[
                    codex_line(
                        ts(0, 1),
                        "response_item",
                        json!({"type":"function_call","name":"spawn_agent",
                "call_id":"spawn","arguments":"{\"task_name\":\"solo\"}"}),
                    ),
                    codex_line(
                        ts(0, 2),
                        "response_item",
                        json!({"type":"function_call","name":"spawn_agent",
                "call_id":"spawn-again","arguments":"{\"task_name\":\"solo\"}"}),
                    ),
                ],
            ),
        ),
        (
            6,
            "codex",
            "child",
            codex_records(
                "child",
                Some("parent"),
                Some("solo"),
                &[
                    codex_user(ts(0, 3), "first prompt"),
                    codex_user(ts(0, 4), "second prompt"),
                ],
            ),
        ),
        (
            7,
            "claude",
            "pending",
            vec![human("pending", ts(0, 4), "physical pending target")],
        ),
    ];
    for seed in 1..=32_u64 {
        let mut random = seed;
        let mut next = || {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            (random >> 32) as usize
        };
        let mut warehouse = Warehouse::new(std::path::Path::new(":memory:"));
        let mut offsets = [0; 7];
        while offsets
            .iter()
            .zip(&inputs)
            .any(|(offset, input)| *offset < input.3.len())
        {
            let i = next() % inputs.len();
            let (source, harness, id, records) = &inputs[i];
            let start = offsets[i];
            if start == records.len() {
                continue;
            }
            let end = (start + 1 + next() % 3).min(records.len());
            warehouse.source(*source, harness, id, &records[start..end]);
            offsets[i] = end;
            assert_full_recompute(&warehouse);
        }
        // Replaying maintenance is idempotent, without replaying native facts.
        for source in 1..=7 {
            Warehouse::recompute(&warehouse.db, source);
        }
        assert_full_recompute(&warehouse);
    }
}

fn append_distribution(name: &str, warehouse: &mut Warehouse, sources: usize, home: &Home) {
    let mut files = (1..=sources)
        .map(|source| {
            std::io::BufWriter::new(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(home.root.join(format!("{name}-{source}.jsonl")))
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    let mut samples = Vec::with_capacity(10_000);
    println!("scale progress phase=append_{name} elapsed_ms=0 rows=0");
    for i in 0..10_000 {
        let source = 1 + (i % sources) as i64;
        let records = (0..1 + i % 5)
            .map(|_| json!({"type":"compact_boundary"}))
            .collect::<Vec<_>>();
        let append = Instant::now();
        let file = &mut files[(source - 1) as usize];
        for record in &records {
            writeln!(file, "{record}").unwrap();
        }
        file.flush().unwrap();
        warehouse.source(source, "claude", &format!("append-{source}"), &records);
        samples.push(append.elapsed().as_nanos());
        if (i + 1) % 1000 == 0 {
            println!(
                "scale progress phase=append_{name} elapsed_ms={} rows={}",
                started.elapsed().as_millis(),
                i + 1
            );
        }
    }
    let total = started.elapsed();
    let mut first = samples[..1000].to_vec();
    let mut last = samples[9000..].to_vec();
    first.sort_unstable();
    last.sort_unstable();
    println!(
        "scale append_case={name} count={} total_ms={} first_1000_p50_us={:.3} first_1000_p99_us={:.3} last_1000_p50_us={:.3} last_1000_p99_us={:.3}",
        samples.len(),
        total.as_millis(),
        first[500] as f64 / 1000.0,
        first[990] as f64 / 1000.0,
        last[500] as f64 / 1000.0,
        last[990] as f64 / 1000.0
    );
    assert!(
        last[500] <= 3 * first[500],
        "{name}: last p50 {} ns > 3x first p50 {} ns",
        last[500],
        first[500]
    );
}

fn star_catalog_append_scale(home: &Home) {
    // A: generate/admit one million native records in bounded 1,000-row chunks.
    // The file is evidence of the physical JSONL shape; no million-Value buffer.
    let generation = Instant::now();
    let mut long = Warehouse::new(&home.root.join("long-session.sqlite3"));
    let mut file = std::io::BufWriter::new(File::create(home.root.join("A-1.jsonl")).unwrap());
    println!("scale progress phase=generate_A elapsed_ms=0 rows=0");
    for chunk in 0..1000 {
        let records = (0..1000).map(|j| {
            let i = chunk*1000+j;
            if i%10_000 == 9999 { json!({"type":"compact_boundary"}) }
            else { json!({"type":"user","uuid":format!("record-{i}"),"message":{"role":"user","content":"synthetic"}}) }
        }).collect::<Vec<_>>();
        for record in &records {
            writeln!(file, "{record}").unwrap();
        }
        long.source(1, "claude", "append-1", &records);
        if (chunk + 1) % 100 == 0 {
            println!(
                "scale progress phase=generate_A elapsed_ms={} rows={}",
                generation.elapsed().as_millis(),
                (chunk + 1) * 1000
            );
        }
    }
    file.flush().unwrap();
    let count: i64 = long
        .db
        .query_row("SELECT count(*) FROM fact_record", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1_000_000);
    println!(
        "scale append_case=A generation_ms={} records={count}",
        generation.elapsed().as_millis()
    );
    drop(file);
    append_distribution("A", &mut long, 1, home);
    drop(long);
    // B: a 50-file continuation chain, append only to the newest file (source 1).
    let generation = Instant::now();
    println!("scale progress phase=generate_B elapsed_ms=0 rows=0");
    let mut chain = Warehouse::new(&home.root.join("long-chain.sqlite3"));
    for source in (1..=50).rev() {
        let mut records = vec![human(&format!("append-{source}"), ts(0, 1), "chain")];
        if source > 1 {
            records.push(
                json!({"type":"continued-in","continuedInSessionId":format!("append-{}",source-1)}),
            );
        }
        let mut file = std::io::BufWriter::new(
            File::create(home.root.join(format!("B-{source}.jsonl"))).unwrap(),
        );
        for record in &records {
            writeln!(file, "{record}").unwrap();
        }
        file.flush().unwrap();
        chain.source(source, "claude", &format!("append-{source}"), &records);
    }
    println!(
        "scale progress phase=generate_B elapsed_ms={} rows=50",
        generation.elapsed().as_millis()
    );
    append_distribution("B", &mut chain, 1, home);
    // C: 300 physical files in one bridge group; spread appends across all files.
    let generation = Instant::now();
    println!("scale progress phase=generate_C elapsed_ms=0 rows=0");
    let mut wide = Warehouse::new(&home.root.join("wide-group.sqlite3"));
    for source in 1..=300 {
        let records = [
            human(&format!("append-{source}"), ts(0, 1), "wide"),
            json!({"type":"bridge-session","bridgeSessionId":"wide-group"}),
        ];
        let mut file = std::io::BufWriter::new(
            File::create(home.root.join(format!("C-{source}.jsonl"))).unwrap(),
        );
        for record in &records {
            writeln!(file, "{record}").unwrap();
        }
        file.flush().unwrap();
        wide.source(source, "claude", &format!("append-{source}"), &records);
    }
    println!(
        "scale progress phase=generate_C elapsed_ms={} rows=300",
        generation.elapsed().as_millis()
    );
    append_distribution("C", &mut wide, 300, home);
}

/// CI only: writes real synthetic JSONL, extracts it through the same loader,
/// then times actual list and detail SQL. No full builder over the scale corpus.
#[test]
#[ignore = "manual 1 GB / 20k session warehouse scale experiment"]
fn star_catalog_scale() {
    let home = Home::new();
    star_catalog_append_scale(&home);
    let path = home.root.join("scale.sqlite3");
    let corpus = home.root.join("corpus.jsonl");
    let mut file = std::io::BufWriter::new(File::create(&corpus).unwrap());
    let padding = "s".repeat(50_000);
    let generation = Instant::now();
    println!("scale progress phase=corpus elapsed_ms=0 rows=0");
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
    println!("scale progress phase=ingest elapsed_ms=0 rows=0");
    // One bounded native-session envelope at a time, never a 1 GB in-memory Value.
    for (i, line) in std::io::BufReader::new(File::open(&corpus).unwrap())
        .lines()
        .enumerate()
    {
        let envelope: Value = serde_json::from_str(&line.unwrap()).unwrap();
        warehouse.source(
            envelope["source"].as_i64().unwrap(),
            envelope["harness"].as_str().unwrap(),
            envelope["id"].as_str().unwrap(),
            envelope["records"].as_array().unwrap(),
        );
        if (i + 1) % 1000 == 0 {
            println!(
                "scale progress phase=ingest elapsed_ms={} rows={}",
                ingest.elapsed().as_millis(),
                i + 1
            );
        }
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
            let start = Instant::now();
            println!("scale progress phase=index elapsed_ms=0 rows=20000");
            db.execute_batch(INDEXES).unwrap();
            db.execute_batch("ANALYZE").unwrap();
            println!(
                "scale progress phase=index elapsed_ms={} rows=20000",
                start.elapsed().as_millis()
            );
        }
        drop(db);
        for (name, sql, selected) in [("list", LIST, false), ("detail", DETAIL, true)] {
            let db = Connection::open(&path).unwrap();
            // Keep SQLite's default automatic indexes for materialized grouped
            // sets (for example catalog keys). Persistent fact/dimension indexes
            // are still explicit; the plans report both kinds.
            for temperature in ["fresh_connection", "warm"] {
                println!("scale progress phase={name}_{temperature} elapsed_ms=0 rows=0");
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
