//! Origin-local text projection. Keys are relative to the ASI namespace.
use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use semon_asi::input::encode_component;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const SCHEMA: &str = "
CREATE TABLE body (
 body_uid TEXT PRIMARY KEY, session_uid TEXT NOT NULL, turn_uid TEXT NOT NULL,
 step_uid TEXT, part TEXT NOT NULL CHECK(part IN ('user','assistant','command','tool_output','error')),
 ord INTEGER NOT NULL, ts INTEGER, text TEXT NOT NULL
) STRICT;
CREATE INDEX body_turn ON body(turn_uid,ord,body_uid);
CREATE INDEX body_step ON body(step_uid,ord,body_uid);
CREATE TABLE body_source (
 source_path TEXT NOT NULL, body_uid TEXT NOT NULL,
 PRIMARY KEY(source_path,body_uid)
) STRICT, WITHOUT ROWID;
CREATE INDEX body_source_uid ON body_source(body_uid);
CREATE VIRTUAL TABLE body_fts USING fts5(text,content='body',content_rowid='rowid',tokenize='unicode61 remove_diacritics 2');
CREATE TRIGGER body_insert AFTER INSERT ON body BEGIN
 INSERT INTO body_fts(rowid,text) VALUES(new.rowid,new.text);
END;
CREATE TRIGGER body_delete AFTER DELETE ON body BEGIN
 INSERT INTO body_fts(body_fts,rowid,text) VALUES('delete',old.rowid,old.text);
END;
";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    source: String,
    agent: bool,
    session: Option<String>,
    turn: Option<String>,
    record: u64,
    calls: BTreeMap<String, Option<Owner>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Owner {
    session: String,
    turn: String,
    step: Option<String>,
}

#[derive(Clone, Debug)]
struct Body {
    uid: String,
    owner: Owner,
    part: String,
    ord: i64,
    ts: Option<i64>,
    text: String,
}

/// Only the newly parsed text is resident; normal FileIndex loads read State only.
#[derive(Clone, Debug, Default)]
pub(crate) struct Batch {
    state: State,
    rows: Vec<Body>,
}

impl Batch {
    pub(crate) fn begin(&mut self, path: &std::path::Path, harness: &str) {
        self.rows.clear();
        self.state.source = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .into();
        self.state.agent = harness == "claude" && crate::native_sidecar::agent_path(path);
        if harness == "claude" && !self.state.agent && self.state.session.is_none() {
            self.state.session = Some(format!("claude/{}", encode_component(&self.state.source)));
        }
        if self.state.agent && self.state.session.is_none() {
            self.state.session = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("agent-"))
                .map(|id| format!("claude/{}", encode_component(id)));
        }
    }

    pub(crate) fn checkpoint(&mut self) {
        self.rows.clear();
    }

    pub(crate) fn capture(
        &mut self,
        harness: &str,
        record: &Value,
        ordinal: Option<u64>,
    ) -> std::io::Result<()> {
        let ordinal = ordinal.unwrap_or(self.state.record);
        self.state.record = ordinal.saturating_add(1);
        let ts = crate::events::record_time(record);
        // Received Claude copies retain their record's native session ID even
        // when the containing continuation file has a different name.
        let source = if harness == "claude" {
            crate::field(record, "sessionId").unwrap_or(&self.state.source)
        } else {
            &self.state.source
        }
        .to_owned();
        let anchor = || format!("@{source}:{ordinal}");
        let mut parts: Vec<(&str, String, Option<String>)> = Vec::new();
        let mut step = None;
        match harness {
            "claude" => {
                if self.state.session.is_none()
                    && !self.state.agent
                    && let Some(id) = crate::field(record, "sessionId")
                {
                    self.state.session = Some(format!("claude/{}", encode_component(id)));
                }
                let message = &record["message"];
                let role = crate::field(record, "type");
                let content = &message["content"];
                let has_prompt = content.is_string()
                    || content.as_array().is_some_and(|blocks| {
                        blocks
                            .iter()
                            .any(|b| crate::field(b, "type") == Some("text"))
                    })
                    || crate::attachments::parts(record).is_some_and(crate::attachments::has_image);
                if role == Some("user") && has_prompt {
                    self.state.turn = Some(
                        crate::field(record, "uuid")
                            .map(str::to_owned)
                            .unwrap_or_else(anchor),
                    );
                    self.state.calls.clear();
                }
                if role == Some("assistant") {
                    step = crate::field(message, "id")
                        .map(|id| format!("s/claude/{}", encode_component(id)));
                }
                if let Some(text) = content.as_str()
                    && let Some(part) = match role {
                        Some("user") => Some("user"),
                        Some("assistant") => Some("assistant"),
                        _ => None,
                    }
                {
                    parts.push((part, text.into(), None));
                }
                for block in content.as_array().into_iter().flatten() {
                    match crate::field(block, "type") {
                        Some("text") if matches!(role, Some("user" | "assistant")) => {
                            if let Some(text) = crate::field(block, "text") {
                                parts.push((
                                    if role == Some("user") {
                                        "user"
                                    } else {
                                        "assistant"
                                    },
                                    text.into(),
                                    None,
                                ));
                            }
                        }
                        Some("tool_use") if role == Some("assistant") => {
                            let input = &block["input"];
                            let text = input
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| input.to_string());
                            parts.push((
                                "command",
                                text,
                                crate::field(block, "id").map(str::to_owned),
                            ));
                        }
                        Some("tool_result") => {
                            let text = text_content(&block["content"]);
                            let call = crate::field(block, "tool_use_id").map(str::to_owned);
                            parts.push((
                                if block["is_error"] == true {
                                    "error"
                                } else {
                                    "tool_output"
                                },
                                text,
                                call,
                            ));
                        }
                        _ => {}
                    }
                }
            }
            "codex" => {
                let payload = &record["payload"];
                if crate::field(record, "type") == Some("session_meta") {
                    self.state.session = crate::field(payload, "id")
                        .map(|id| format!("codex/{}", encode_component(id)));
                }
                if let Some(turn) = crate::field(payload, "turn_id") {
                    if self.state.turn.as_deref() != Some(turn) {
                        self.state.calls.clear();
                    }
                    self.state.turn = Some(turn.into());
                }
                let ids = ["thread_id", "session_id", "turn_id", "response_id"]
                    .map(|key| crate::field(payload, key));
                if let [Some(thread), Some(session), Some(turn), Some(response)] = ids {
                    step = Some(format!(
                        "s/codex/{}/{}/{}/{}",
                        encode_component(thread),
                        encode_component(session),
                        encode_component(turn),
                        encode_component(response)
                    ));
                }
                if crate::field(record, "type") == Some("response_item") {
                    match crate::field(payload, "type") {
                        Some("message") => {
                            let part = match crate::field(payload, "role") {
                                Some("user") => Some("user"),
                                Some("assistant") => Some("assistant"),
                                _ => None,
                            };
                            if let Some(part) = part {
                                parts.push((part, text_content(&payload["content"]), None));
                            }
                        }
                        Some("function_call" | "custom_tool_call" | "local_shell_call") => {
                            let text = payload
                                .get("arguments")
                                .or_else(|| payload.get("input"))
                                .or_else(|| payload.get("action"))
                                .map(text_content)
                                .unwrap_or_default();
                            parts.push((
                                "command",
                                text,
                                crate::field(payload, "call_id")
                                    .or_else(|| crate::field(payload, "id"))
                                    .map(str::to_owned),
                            ));
                        }
                        Some("function_call_output" | "custom_tool_call_output") => {
                            let result_call = crate::field(payload, "call_id").map(str::to_owned);
                            parts.push((
                                if payload["is_error"] == true {
                                    "error"
                                } else {
                                    "tool_output"
                                },
                                text_content(&payload["output"]),
                                result_call.clone(),
                            ));
                        }
                        _ => {}
                    }
                } else if crate::field(record, "type") == Some("event_msg")
                    && crate::field(payload, "type") == Some("error")
                {
                    parts.push(("error", text_content(&payload["message"]), None));
                }
            }
            "copilot" => {
                let data = &record["data"];
                match crate::field(record, "type") {
                    Some("session.start") => {
                        self.state.session = crate::field(data, "sessionId")
                            .map(|id| format!("copilot/{}", encode_component(id)))
                    }
                    Some("user.message") => {
                        self.state.turn = Some(
                            crate::field(record, "id")
                                .map(str::to_owned)
                                .unwrap_or_else(anchor),
                        );
                        self.state.calls.clear();
                        parts.push(("user", text_content(&data["content"]), None));
                    }
                    Some("assistant.message") => {
                        parts.push(("assistant", text_content(&data["content"]), None));
                        for call in data["toolRequests"].as_array().into_iter().flatten() {
                            parts.push((
                                "command",
                                text_content(&call["arguments"]),
                                crate::field(call, "toolCallId").map(str::to_owned),
                            ));
                        }
                    }
                    Some("tool.execution_complete") => {
                        let result_call = crate::field(data, "toolCallId").map(str::to_owned);
                        parts.push((
                            if data["success"] == false {
                                "error"
                            } else {
                                "tool_output"
                            },
                            if data["success"] == false {
                                data.get("error")
                                    .and_then(|error| crate::field(error, "message"))
                                    .map(str::to_owned)
                                    .filter(|message| !message.is_empty())
                                    .unwrap_or_else(|| text_content(&data["result"]))
                            } else {
                                text_content(&data["result"])
                            },
                            result_call.clone(),
                        ));
                    }
                    _ => {}
                }
            }
            _ => return Ok(()),
        }
        let Some(session) = self.state.session.clone() else {
            return Ok(());
        };
        let turn = self
            .state
            .turn
            .clone()
            .or_else(|| {
                if harness == "claude" && crate::field(record, "type") == Some("assistant") {
                    crate::field(&record["message"], "id").map(str::to_owned)
                } else {
                    None
                }
            })
            .unwrap_or_else(anchor);
        let owner = Owner {
            turn: format!("{session}/t/{}", encode_component(&turn)),
            step: step.map(|step| format!("{session}/{step}")),
            session,
        };
        for (block, (part, text, call)) in parts.into_iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            let is_output = matches!(part, "tool_output" | "error");
            let selected = if is_output {
                call.as_ref()
                    .and_then(|id| self.state.calls.get(id))
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| Owner {
                        step: None,
                        ..owner.clone()
                    })
            } else {
                owner.clone()
            };
            if !is_output && let Some(call) = call {
                self.state
                    .calls
                    .entry(call)
                    .and_modify(|owner| *owner = None)
                    .or_insert_with(|| Some(selected.clone()));
            }
            // Physical record order and block order are stable under append. The
            // checked arithmetic refuses unrepresentable sources rather than aliasing.
            let ord = (block < 1_048_576)
                .then_some(ordinal)
                .and_then(|ordinal| ordinal.checked_mul(1_048_576))
                .and_then(|ord| ord.checked_add(block as u64))
                .and_then(|ord| i64::try_from(ord).ok())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "body ordinal exceeds supported range",
                    )
                })?;
            let parent = selected.step.as_ref().unwrap_or(&selected.turn);
            self.rows.push(Body {
                uid: format!("{parent}/b/{part}/{ord}"),
                owner: selected,
                part: part.into(),
                ord,
                ts,
                text,
            });
        }
        Ok(())
    }
}

fn text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| crate::field(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn key(path: &str) -> String {
    format!("body_state:{path}")
}

pub(super) fn load(connection: &Connection, path: &str) -> rusqlite::Result<Batch> {
    let state: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key=?1", [key(path)], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(Batch {
        state: state
            .map(|state| serde_json::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?
            .unwrap_or_default(),
        rows: Vec::new(),
    })
}

pub(super) fn clear(connection: &Connection, path: &str) -> rusqlite::Result<()> {
    connection.execute("DELETE FROM body WHERE body_uid IN (SELECT body_uid FROM body_source WHERE source_path=?1) AND NOT EXISTS (SELECT 1 FROM body_source other WHERE other.body_uid=body.body_uid AND other.source_path<>?1)", [path])?;
    connection.execute("DELETE FROM body_source WHERE source_path=?1", [path])?;
    connection.execute("DELETE FROM meta WHERE key=?1", [key(path)])?;
    Ok(())
}

/// Catalog establishes the native lineage owner. Only an actual ownership
/// change rekeys that source's rows; ordinary publication never rescans text.
pub(super) fn bind_owner(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
) -> rusqlite::Result<()> {
    let Some(native) = row.owner_native_id() else {
        return Ok(());
    };
    let session = format!(
        "{}/{}",
        encode_component(&row.harness),
        encode_component(native)
    );
    for source in &row.sources {
        let path = source.path.to_str().ok_or(rusqlite::Error::InvalidQuery)?;
        let mut batch = load(tx, path)?;
        let Some(old) = batch.state.session.clone().filter(|old| old != &session) else {
            continue;
        };
        let replace = |value: &str| {
            value
                .strip_prefix(&format!("{old}/"))
                .map(|suffix| format!("{session}/{suffix}"))
                .unwrap_or_else(|| value.to_owned())
        };
        let rows = tx.prepare("SELECT b.body_uid,b.turn_uid,b.step_uid,b.part,b.ord,b.ts,b.text FROM body b JOIN body_source s USING(body_uid) WHERE s.source_path=?1")?.query_map([path], |record| {
            let uid: String = record.get(0)?;
            let turn: String = record.get(1)?;
            let step: Option<String> = record.get(2)?;
            Ok(Body { uid: replace(&uid), owner: Owner { session: session.clone(), turn: replace(&turn), step: step.map(|step| replace(&step)) }, part: record.get(3)?, ord: record.get(4)?, ts: record.get(5)?, text: record.get(6)? })
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        batch.state.session = Some(session.clone());
        for owner in batch.state.calls.values_mut().flatten() {
            owner.session = session.clone();
            owner.turn = replace(&owner.turn);
            owner.step = owner.step.as_ref().map(|step| replace(step));
        }
        batch.rows = rows;
        write(tx, path, &batch, false)?;
    }
    Ok(())
}

pub(crate) fn write(
    tx: &Transaction<'_>,
    path: &str,
    batch: &Batch,
    append: bool,
) -> rusqlite::Result<()> {
    if batch.rows.is_empty() && batch.state.source.is_empty() {
        if !append {
            clear(tx, path)?;
        }
        return Ok(());
    }
    let mut batch = batch.clone();
    if append
        && let Some(session) = load(tx, path)?.state.session
        && let Some(old) = batch.state.session.clone().filter(|old| old != &session)
    {
        let replace = |value: &str| {
            value
                .strip_prefix(&format!("{old}/"))
                .map(|suffix| format!("{session}/{suffix}"))
                .unwrap_or_else(|| value.to_owned())
        };
        batch.state.session = Some(session.clone());
        for owner in batch.state.calls.values_mut().flatten() {
            owner.session = session.clone();
            owner.turn = replace(&owner.turn);
            owner.step = owner.step.as_ref().map(|step| replace(step));
        }
        for body in &mut batch.rows {
            body.uid = replace(&body.uid);
            body.owner.session = session.clone();
            body.owner.turn = replace(&body.owner.turn);
            body.owner.step = body.owner.step.as_ref().map(|step| replace(step));
        }
    }
    if !append {
        clear(tx, path)?;
    }
    for body in &batch.rows {
        let inserted = tx.execute("INSERT INTO body(body_uid,session_uid,turn_uid,step_uid,part,ord,ts,text) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(body_uid) DO NOTHING", params![body.uid, body.owner.session, body.owner.turn, body.owner.step, body.part, body.ord, body.ts, body.text])?;
        if inserted == 0 {
            let agrees: bool = tx.query_row("SELECT session_uid=?2 AND turn_uid=?3 AND step_uid IS ?4 AND part=?5 AND ord=?6 AND ts IS ?7 AND text=?8 FROM body WHERE body_uid=?1", params![body.uid, body.owner.session, body.owner.turn, body.owner.step, body.part, body.ord, body.ts, body.text], |row| row.get(0))?;
            if !agrees {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO body_source VALUES(?1,?2)",
            params![path, body.uid],
        )?;
    }
    tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key(path),serde_json::to_string(&batch.state).map_err(|_| rusqlite::Error::InvalidQuery)?])?;
    Ok(())
}
