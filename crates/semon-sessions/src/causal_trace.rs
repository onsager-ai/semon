//! Source-qualified causal metadata. The producer publishes relationships;
//! readers follow one requested turn, never a workspace model or native body.
use crate::{
    Options,
    events::EventCache,
    model::{Handoff, Session, Turn},
    viewer::ViewerReply,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_trace_headers(session_key TEXT PRIMARY KEY, generation TEXT NOT NULL, source_generation TEXT NOT NULL, metadata TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS session_trace_turns(session_key TEXT NOT NULL,turn_id TEXT NOT NULL,start_id TEXT,metadata TEXT NOT NULL,PRIMARY KEY(session_key,turn_id)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_trace_starts ON session_trace_turns(session_key,start_id,turn_id);
CREATE TABLE IF NOT EXISTS session_trace_edges(session_key TEXT NOT NULL,edge_id TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(session_key,edge_id)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_trace_from ON session_trace_edges(CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END,session_key);
CREATE INDEX IF NOT EXISTS session_trace_to ON session_trace_edges(CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END,session_key);
CREATE INDEX IF NOT EXISTS session_trace_query_at ON session_trace_edges(json_extract(metadata,'$.at'),session_key,edge_id);
CREATE INDEX IF NOT EXISTS session_trace_query_done ON session_trace_edges(json_extract(metadata,'$.done'),session_key,edge_id);
CREATE INDEX IF NOT EXISTS session_trace_query_status ON session_trace_edges(json_extract(metadata,'$.status'),session_key,edge_id);
CREATE INDEX IF NOT EXISTS session_trace_turn_query_at ON session_trace_turns(session_key,json_extract(metadata,'$.at'),turn_id);
CREATE INDEX IF NOT EXISTS session_trace_turn_query_last ON session_trace_turns(session_key,turn_id) WHERE json_extract(metadata,'$.last')=1;
CREATE TABLE IF NOT EXISTS session_trace_bookmarks(target TEXT NOT NULL,session_key TEXT NOT NULL,edge_id TEXT NOT NULL,PRIMARY KEY(target,session_key,edge_id)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_trace_bookmark_owner ON session_trace_bookmarks(session_key);
";

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    session: Value,
    turns: Vec<Value>,
    edges: Vec<Value>,
    bookmarks: Vec<(String, String)>,
}
impl Projection {
    pub(crate) fn endpoints(&self) -> impl Iterator<Item = &str> {
        self.edges.iter().flat_map(|edge| {
            ["from", "to"]
                .into_iter()
                .filter_map(move |field| edge[field].as_str().filter(|key| *key != "you"))
        })
    }
    pub(crate) fn closed_over(&self, keys: &BTreeSet<&str>) -> bool {
        self.bookmarks.is_empty()
            && self.edges.iter().all(|edge| {
                ["from", "to"].into_iter().all(|field| {
                    edge[field].is_null()
                        || edge[field]
                            .as_str()
                            .is_some_and(|key| key == "you" || keys.contains(key))
                })
            })
    }
    pub(crate) fn relationships_incomplete(&mut self) {
        self.session["relationships_incomplete"] = json!(true);
    }
    pub(crate) fn capture(
        key: &str,
        session: &Session,
        turns: &[&Turn],
        edges: &[&Handoff],
        owners: &BTreeMap<String, Value>,
        cwd: Option<&str>,
        transcript: Option<&crate::model::Transcript>,
    ) -> Self {
        let details = json!({
            "state": session.state,
            "activity": session.activity.as_ref().map(|(tool, _, _, since)| json!({"tool": tool, "since": since})),
            "cwd": cwd,
            "turns": turns.iter().filter(|turn| turn.sid == key).count(),
            "calls": transcript.map(|value| value.calls),
            "errors": transcript.map(|value| value.errors),
            "tool_calls": transcript.map(|value| &value.tools).unwrap_or(&session.tool_calls),
            "signals": session.signals,
            "cost": session.cost,
            "copied_usage": session.claude_usage.is_some(),
            "claude_usage": session.claude_usage,
            "copilot": session.copilot.as_ref().map(|evidence| json!({
                "version": evidence.version, "schema": evidence.schema,
                "usage": evidence.usage, "usage_source": evidence.usage_source,
                "lifecycle": evidence.lifecycle, "logical_parent": evidence.logical_parent,
                "approvals": evidence.approvals,
            })),
            "reported_runs": session.reported_runs,
            "cost_check": session.cost_check,
            "tokens_by_model": session.tokens_by_model,
            "rate_limits": session.rate_limits,
            "lane": session.lane,
            "role": session.role,
            "waiting_since": session.waiting_since,
        });
        Self {
            session: json!({"name":session.name,"harness":session.harness,"kind":session.kind,"state":session.state,"model":session.model,"start":session.start,"last":session.last,"tokens":session.tokens,"cost":session.cost,"busy":session.busy,"wait_edges":session.wait_edges,"wait_edges_truncated":session.wait_edges_truncated,"details":details}),
            turns: turns.iter().filter(|turn|turn.sid==key).map(|turn|json!({"id":turn.id,"sid":turn.sid,"start":turn.start,"u":turn.u,"at":turn.at,"end":turn.end,"sent":turn.sent,"last":turn.last})).collect(),
            edges: edges.iter().filter(|edge|edge.from==key || edge.to.as_deref()==Some(key)).map(|edge|json!({"id":edge.id,"kind":edge.kind,"ask":edge.ask,"from":edge.from,"to":edge.to,"at":edge.at,"done":edge.done,"status":edge.status,"declined":edge.declined,"unmatched":edge.unmatched,"ambiguous":edge.ambiguous,"brief_session":owners[&edge.id]["brief_session"],"result_session":owners[&edge.id]["result_session"]})).collect(),
            bookmarks: edges.iter().filter(|edge| edge.from == key && edge.to.is_none()).filter_map(|edge| edge.target.as_ref().map(|target| (target.clone(), edge.id.clone()))).collect(),
        }
    }
    pub(crate) fn with_family_cost(mut self, usd: Option<f64>) -> Self {
        self.session["family_usd"] = json!(usd);
        self
    }
}

/// The producer already owns the machine's descriptions. Roll descendant costs
/// up once without making a reader traverse every family. Cycles stay unknown.
pub(crate) fn family_costs<'a>(
    rows: impl Iterator<Item = (&'a str, Option<&'a str>, Option<f64>)>,
) -> BTreeMap<&'a str, Option<f64>> {
    let rows: BTreeMap<_, _> = rows.map(|(id, parent, usd)| (id, (parent, usd))).collect();
    let mut costs: BTreeMap<_, _> = rows.iter().map(|(&id, &(_, usd))| (id, usd)).collect();
    let mut pending: BTreeMap<_, usize> = rows.keys().map(|&id| (id, 0)).collect();
    for (parent, _) in rows.values() {
        if let Some(count) = parent.and_then(|parent| pending.get_mut(parent)) {
            *count += 1;
        }
    }
    let mut ready: VecDeque<_> = pending
        .iter()
        .filter_map(|(&id, &count)| (count == 0).then_some(id))
        .collect();
    while let Some(id) = ready.pop_front() {
        if let Some(parent) = rows[id].0.filter(|parent| rows.contains_key(parent)) {
            costs.insert(parent, costs[parent].zip(costs[id]).map(|(a, b)| a + b));
            let count = pending.get_mut(parent).expect("registered parent");
            *count -= 1;
            if *count == 0 {
                ready.push_back(parent);
            }
        }
    }
    for (id, count) in pending {
        if count != 0 {
            costs.insert(id, None);
        }
    }
    costs
}
pub(crate) fn publish(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    projection: &Projection,
) -> rusqlite::Result<()> {
    publish_measured(
        tx,
        row,
        projection,
        &mut crate::native_timing::Measurements::default(),
    )
}

pub(crate) fn publish_measured(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    projection: &Projection,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let total_started = measurements.start();
    let started = measurements.start();
    let mut session = projection.session.clone();
    session["content_generation"] = json!(
        row.overview
            .as_ref()
            .map(crate::overview::content_generation)
    );
    measurements.record("trace_session", started, 1);
    let started = measurements.start();
    let digest = format!(
        "{:x}",
        Sha256::digest(
            json!([
                session,
                projection.turns,
                projection.edges,
                projection.bookmarks
            ])
            .to_string()
            .as_bytes()
        )
    );
    measurements.record("trace_digest", started, 1);
    let started = measurements.start();
    let source = crate::retention::source_generation(&row.sources)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    measurements.record("trace_source_generation", started, 1);
    let changed = {
        let started = measurements.start();
        let mut statement=tx.prepare_cached("INSERT INTO session_trace_headers VALUES(?1,?2,?3,?4) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,metadata=excluded.metadata WHERE session_trace_headers.generation!=excluded.generation OR session_trace_headers.source_generation!=excluded.source_generation")?;
        measurements.record("trace_header_prepare", started, 1);
        let started = measurements.start();
        let parameters = params![row.key, digest, source, session.to_string()];
        measurements.record("trace_header_serialization", started, 1);
        let started = measurements.start();
        let changed = statement.execute(parameters)?;
        measurements.record("trace_header_sql", started, 1);
        changed
    };
    if changed == 0 {
        measurements.record("trace_total_inclusive", total_started, 1);
        return Ok(());
    }
    let started = measurements.start();
    tx.prepare_cached("DELETE FROM session_trace_turns WHERE session_key=?1")?
        .execute([&row.key])?;
    tx.prepare_cached("DELETE FROM session_trace_edges WHERE session_key=?1")?
        .execute([&row.key])?;
    measurements.record("trace_turn_edge_removal", started, 2);
    let started = measurements.start();
    let mut turns = tx.prepare_cached("INSERT INTO session_trace_turns VALUES(?1,?2,?3,?4)")?;
    measurements.record("trace_turn_prepare", started, 1);
    for turn in &projection.turns {
        let started = measurements.start();
        let parameters = params![
            row.key,
            turn["id"].as_str(),
            turn["start"].as_str(),
            turn.to_string()
        ];
        measurements.record("trace_turn_serialization", started, 1);
        let started = measurements.start();
        turns.execute(parameters)?;
        measurements.record("trace_turn_sql", started, 1);
    }
    let started = measurements.start();
    let mut edges = tx.prepare_cached("INSERT INTO session_trace_edges VALUES(?1,?2,?3)")?;
    measurements.record("trace_edge_prepare", started, 1);
    for edge in &projection.edges {
        let started = measurements.start();
        let parameters = params![row.key, edge["id"].as_str(), edge.to_string()];
        measurements.record("trace_edge_serialization", started, 1);
        let started = measurements.start();
        edges.execute(parameters)?;
        measurements.record("trace_edge_sql", started, 1);
    }
    let started = measurements.start();
    tx.prepare_cached("DELETE FROM session_trace_bookmarks WHERE session_key=?1")?
        .execute([&row.key])?;
    measurements.record("trace_bookmark_removal", started, 1);
    let started = measurements.start();
    let mut bookmarks =
        tx.prepare_cached("INSERT INTO session_trace_bookmarks VALUES(?1,?2,?3)")?;
    measurements.record("trace_bookmark_prepare", started, 1);
    let started = measurements.start();
    for (target, edge) in &projection.bookmarks {
        bookmarks.execute(params![target, row.key, edge])?;
    }
    measurements.record("trace_bookmark_sql", started, projection.bookmarks.len());
    measurements.record("trace_total_inclusive", total_started, 1);
    Ok(())
}

pub(crate) fn update_current_observation(
    tx: &Transaction<'_>,
    key: &str,
    current: &Projection,
    content_generation: String,
) -> rusqlite::Result<()> {
    let prior: Option<(String, String)> = tx
        .query_row(
            "SELECT metadata,generation FROM session_trace_headers WHERE session_key=?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((encoded, version)) = prior else {
        return Ok(());
    };
    let mut metadata: Value = serde_json::from_str(&encoded)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    for field in ["name", "state"] {
        metadata[field] = current.session[field].clone();
    }
    for field in ["state", "activity"] {
        metadata["details"][field] = current.session["details"][field].clone();
    }
    metadata["content_generation"] = json!(content_generation);
    let text = metadata.to_string();
    if text != encoded {
        let generation = format!(
            "{:x}",
            Sha256::digest(json!([version, metadata]).to_string().as_bytes())
        );
        tx.execute(
            "UPDATE session_trace_headers SET metadata=?2,generation=?3 WHERE session_key=?1",
            params![key, text, generation],
        )?;
    }
    Ok(())
}

/// Exact selected metadata only; list pages never load usage breakdowns. These
/// recorded facts grant neither runtime authority nor native control custody.
pub(crate) fn selected_details(
    tx: &Transaction<'_>,
    key: &str,
    sources: &[crate::model::summary::CatalogSource],
) -> rusqlite::Result<Option<Value>> {
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='session_trace_headers')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Ok(None);
    }
    let expected = crate::retention::source_generation(sources)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let encoded: Option<(Option<String>, bool)> = tx.query_row(
        "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_set(json_extract(metadata,'$.details'),'$.busy',json_extract(metadata,'$.busy'),'$.content_generation',json_extract(metadata,'$.content_generation')) ELSE NULL END,EXISTS(SELECT 1 FROM session_catalog c WHERE c.session_key=?1 AND c.lifecycle='current') FROM session_trace_headers WHERE session_key=?1 AND source_generation=?2",
        params![key, expected], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional()?;
    encoded
        .and_then(|(text, current)| text.map(|text| (text, current)))
        .map(|(text, current)| {
            let mut value: Value = serde_json::from_str(&text)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            value["live_observation"] = json!(current);
            value["family"] = selected_family(tx, key)?;
            Ok(value)
        })
        .transpose()
}

/// Indexed, body-free relatives of the selected owner. This never walks an
/// unrelated workspace. A truncated family retains an explicit unknown total.
fn selected_family(tx: &Transaction<'_>, key: &str) -> rusqlite::Result<Value> {
    let mut bytes = 0;
    let incomplete = std::cell::Cell::new(false);
    let mut compact = tx.prepare("SELECT CASE WHEN octet_length(COALESCE(h.metadata,c.metadata))<=1048576 THEN json_object('key',c.session_key,'name',json_extract(COALESCE(h.metadata,c.metadata),'$.name'),'harness',c.harness,'kind',COALESCE(json_extract(COALESCE(h.metadata,c.metadata),'$.presentation_kind'),json_extract(COALESCE(h.metadata,c.metadata),'$.kind')),'parent',json_extract(COALESCE(h.metadata,c.metadata),'$.parent'),'cost',json_extract(COALESCE(h.metadata,c.metadata),'$.cost')) ELSE NULL END FROM session_catalog c LEFT JOIN session_history_catalog h USING(session_key) WHERE c.session_key=?1")?;
    let mut read = |sid: &str| -> rusqlite::Result<Option<Value>> {
        let text: Option<String> = compact
            .query_row([sid], |row| row.get(0))
            .optional()?
            .flatten();
        let Some(text) = text else {
            incomplete.set(true);
            return Ok(None);
        };
        bytes += text.len();
        if text.len() > 65536 || bytes > 1048576 {
            incomplete.set(true);
            return Ok(None);
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    };
    let mut ancestors = Vec::new();
    let mut seen = BTreeSet::from([key.to_owned()]);
    let mut parent = read(key)?.and_then(|row| row["parent"].as_str().map(str::to_owned));
    while let Some(sid) = parent.take() {
        if ancestors.len() >= 32 || !seen.insert(sid.clone()) {
            incomplete.set(true);
            break;
        }
        let Some(row) = read(&sid)? else {
            break;
        };
        parent = row["parent"].as_str().map(str::to_owned);
        ancestors.push(row);
    }
    ancestors.reverse();
    let mut statement = tx.prepare("WITH RECURSIVE family(session_key) AS (VALUES(?1) UNION SELECT c.session_key FROM session_catalog c JOIN family f ON c.parent_key=f.session_key LIMIT 101) SELECT session_key FROM family WHERE session_key!=?1")?;
    let mut runs = Vec::new();
    for sid in statement.query_map([key], |row| row.get::<_, String>(0))? {
        let sid = sid?;
        if seen.contains(&sid) {
            incomplete.set(true);
            continue;
        }
        if runs.len() >= 99 {
            incomplete.set(true);
            break;
        }
        if let Some(row) = read(&sid)? {
            runs.push(row);
        }
    }
    Ok(
        json!({"state":if incomplete.get() {"incomplete"} else {"complete"},"ancestors":ancestors,"runs":runs}),
    )
}

/// Relationships for the issued native range only. This shares the producer's
/// causal projection; it never constructs a graph or reads native bodies.
pub(crate) fn range_context(
    tx: &Transaction<'_>,
    key: &str,
    slots: &[crate::slot_projection::SavedSlot],
    sources: &[crate::model::summary::CatalogSource],
) -> rusqlite::Result<Value> {
    let mut ids = BTreeSet::new();
    let turn_ids: BTreeSet<_> = slots
        .iter()
        .filter_map(|slot| slot.turn.as_deref())
        .collect();
    for slot in slots {
        if let crate::slot_projection::Recipe::H { id } = &slot.recipe {
            ids.insert(id.clone());
        }
    }
    if !ids.is_empty() || !turn_ids.is_empty() {
        let expected = crate::retention::source_generation(sources)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let published: Option<String> = tx
            .query_row(
                "SELECT source_generation FROM session_trace_headers WHERE session_key=?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        if published.as_deref() != Some(&expected) {
            return Ok(
                json!({"state":"incomplete","turns":[],"handoffs":[],"sessions":{},"previews":{}}),
            );
        }
    }
    let mut missing = tx.query_row("SELECT COALESCE(json_extract(metadata,'$.relationships_incomplete'),0) FROM session_trace_headers WHERE session_key=?1", [key], |row|row.get::<_,bool>(0)).optional()?.unwrap_or(false);
    let mut turns = Vec::new();
    let mut bytes = 0;
    for id in turn_ids {
        let encoded: Option<String> = tx.query_row("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_trace_turns WHERE session_key=?1 AND turn_id=?2",params![key,id],|row|row.get(0)).optional()?;
        let Some(encoded) = encoded else {
            missing = true;
            continue;
        };
        bytes += encoded.len();
        if bytes > 4 * 1024 * 1024 {
            missing = true;
            break;
        }
        let turn: Value = serde_json::from_str(&encoded)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        if turn["sent"].as_array().is_none_or(|sent| sent.len() > 1000) {
            missing = true;
            continue;
        }
        if let Some(start) = turn["start"].as_str() {
            ids.insert(start.to_owned());
        }
        turns.push(turn);
    }
    let mut edges = Vec::new();
    let mut owners = BTreeSet::new();
    let mut participants = BTreeSet::new();
    for id in ids {
        let encoded: Option<String> = tx.query_row(
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
            params![key,id], |row|row.get(0)).optional()?;
        let Some(encoded) = encoded else {
            missing = true;
            continue;
        };
        bytes += encoded.len();
        if bytes > 4 * 1024 * 1024 {
            missing = true;
            break;
        }
        let mut edge: Value = serde_json::from_str(&encoded)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        for field in ["brief_session", "result_session"] {
            if let Some(owner) = edge[field].as_str() {
                owners.insert(owner.to_owned());
            }
        }
        for field in ["from", "to"] {
            if let Some(sid) = edge[field].as_str().filter(|sid| *sid != "you") {
                participants.insert(sid.to_owned());
            }
        }
        if let Some(target) = edge["to"].as_str() {
            let found: Vec<String> = tx.prepare("SELECT turn_id FROM session_trace_turns WHERE session_key=?1 AND start_id=?2 ORDER BY turn_id LIMIT 2")?
                .query_map(params![target,id],|row|row.get(0))?.collect::<Result<_,_>>()?;
            if found.len() == 1 {
                edge["target_turn"] = json!(found[0]);
            }
        }
        edges.push(edge);
    }
    let mut sessions = BTreeMap::new();
    if participants.len() > 201 {
        missing = true;
    }
    for sid in participants.into_iter().take(201) {
        let metadata:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_object('name',json_extract(metadata,'$.name'),'harness',json_extract(metadata,'$.harness'),'model',json_extract(metadata,'$.model'),'kind',json_extract(metadata,'$.kind'),'state',json_extract(metadata,'$.state'),'start',json_extract(metadata,'$.start'),'last',json_extract(metadata,'$.last'),'calls',json_extract(metadata,'$.details.calls'),'usd',json_extract(metadata,'$.family_usd'),'live_observation',EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 AND lifecycle='current')) ELSE NULL END FROM session_trace_headers WHERE session_key=?1",[&sid],|row|row.get(0)).optional()?.flatten();
        if let Some(metadata) = metadata {
            bytes += metadata.len();
            if metadata.len() > 65536 || bytes > 4 * 1024 * 1024 {
                missing = true;
                continue;
            }
            sessions.insert(
                sid,
                serde_json::from_str::<Value>(&metadata)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
            );
        } else {
            missing = true;
        }
    }
    let mut previews = BTreeMap::new();
    if owners.len() > 128 {
        missing = true;
    }
    for sid in owners.into_iter().take(128) {
        let header:Option<(String,Option<String>)>=tx.query_row("SELECT source_generation,json_extract(metadata,'$.content_generation') FROM session_trace_headers WHERE session_key=?1",[&sid],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let Some((generation, Some(content))) = header else {
            missing = true;
            continue;
        };
        let metadata = crate::history_projection::catalog(tx, &sid)?.or(tx
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1",
                [&sid],
                |row| row.get::<_, String>(0),
            )
            .optional()?);
        let Some(metadata) = metadata else {
            missing = true;
            continue;
        };
        let row: crate::model::summary::CatalogRow = serde_json::from_str(&metadata)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        if row.key != sid
            || crate::retention::source_generation(&row.sources)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
                != generation
            || content.len() != 64
        {
            missing = true;
            continue;
        }
        previews.insert(sid, content);
    }
    Ok(
        json!({"state":if missing {"incomplete"} else {"complete"},"turns":turns,"handoffs":edges,"sessions":sessions,"previews":previews}),
    )
}

/// At most 100 connected turns and 1,000 edges. Frontiers support explicit
/// further exploration; absent or ambiguous recorded joins remain absent.
pub fn session_causal_trace(options: &Options, source_key: &str, query: &str) -> ViewerReply {
    let read = || -> Result<Value, u16> {
        let mut args = BTreeMap::new();
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or(400u16)?;
            if !matches!(name, "sid" | "turn" | "machine" | "scope")
                || args
                    .insert(name, crate::viewer::decoded(value).ok_or(400u16)?)
                    .is_some()
            {
                return Err(400);
            }
        }
        let key = args
            .get("sid")
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or(400u16)?;
        let root = args
            .get("turn")
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or(400u16)?;
        if args.get("scope").map(String::as_str) != Some("retained_history") {
            return Err(400);
        }
        let identity = crate::session_catalog_history_identity(options, source_key, key)
            .map_err(|_| 503u16)?
            .ok_or(404u16)?;
        let mut db = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| 503u16)?;
        db.busy_timeout(std::time::Duration::from_secs(1))
            .map_err(|_| 503u16)?;
        let tx = db.transaction().map_err(|_| 503u16)?;
        let generation: String = tx
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| 503u16)?;
        if generation != identity.generation {
            return Err(409);
        }
        let mut queue = VecDeque::from([(key.clone(), root.clone())]);
        let mut seen = BTreeSet::new();
        let mut sessions = BTreeMap::new();
        let mut turns = Vec::new();
        let mut edges = BTreeMap::new();
        let mut frontier = Vec::new();
        let mut versions = Vec::new();
        let mut missing = false;
        let mut bytes_left = 4 * 1024 * 1024;
        let mut decode = |text: String| -> Result<Value, u16> {
            if text.len() > bytes_left {
                return Err(413);
            }
            bytes_left -= text.len();
            serde_json::from_str(&text).map_err(|_| 503u16)
        };
        while let Some((sid, tid)) = queue.pop_front() {
            if !seen.insert((sid.clone(), tid.clone())) {
                continue;
            }
            if turns.len() >= 100 {
                frontier.push(json!({"catalog_key":sid,"turn":tid}));
                continue;
            }
            let encoded:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_trace_turns WHERE session_key=?1 AND turn_id=?2",params![sid,tid],|row|row.get(0)).optional().map_err(|_|503u16)?;
            let Some(encoded) = encoded else {
                missing = true;
                continue;
            };
            let turn = decode(encoded)?;
            if !sessions.contains_key(&sid) {
                let (text,version,source):(String,String,String)=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END,generation,source_generation FROM session_trace_headers WHERE session_key=?1",[&sid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_|503u16)?;
                let metadata = crate::history_projection::catalog(&tx, &sid)
                    .map_err(|_| 503u16)?
                    .or(tx
                        .query_row(
                            "SELECT metadata FROM session_catalog WHERE session_key=?1",
                            [&sid],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(|_| 503u16)?)
                    .ok_or(409u16)?;
                let row: crate::model::summary::CatalogRow =
                    serde_json::from_str(&metadata).map_err(|_| 503u16)?;
                if row.key != sid
                    || crate::retention::source_generation(&row.sources).map_err(|_| 503u16)?
                        != source
                {
                    return Err(409);
                }
                versions.push(json!([sid, version]));
                let current: Option<String> = tx
                    .query_row(
                        "SELECT lifecycle FROM session_catalog WHERE session_key=?1",
                        [&sid],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| 503u16)?;
                let mut header = decode(text)?;
                missing |= header["relationships_incomplete"].as_bool() == Some(true);
                header["live_observation"] = json!(current.as_deref() == Some("current"));
                sessions.insert(sid.clone(), header);
            }
            if let Some(start) = turn["start"].as_str()
                && !edges.contains_key(start)
            {
                let incoming:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",params![sid,start],|row|row.get(0)).optional().map_err(|_|503u16)?;
                if let Some(text) = incoming {
                    edges.insert(start.to_string(), decode(text)?);
                } else {
                    missing = true;
                }
            }
            let ids = turn["sent"].as_array().ok_or(503u16)?;
            if ids.len() > 1000 || edges.len() + ids.len() > 1000 {
                return Err(413);
            }
            for id in ids {
                let id = id.as_str().ok_or(503u16)?;
                let encoded:String=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",params![sid,id],|row|row.get(0)).map_err(|_|503u16)?;
                let edge = decode(encoded)?;
                if matches!(edge["kind"].as_str(), Some("spawn" | "relay"))
                    && let Some(target) = edge["to"].as_str()
                {
                    let mut starts=tx.prepare("SELECT turn_id FROM session_trace_turns WHERE session_key=?1 AND start_id=?2 ORDER BY turn_id LIMIT 2").map_err(|_|503u16)?;
                    let found = starts
                        .query_map(params![target, id], |row| row.get::<_, String>(0))
                        .map_err(|_| 503u16)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| 503u16)?;
                    if found.len() == 1 {
                        queue.push_back((target.into(), found[0].clone()));
                    } else {
                        missing = true;
                    }
                }
                edges.insert(id.to_string(), edge);
            }
            turns.push(turn);
        }
        if turns.is_empty() {
            return Err(404);
        }
        // A recorded peer can have a known name without a recorded turn that
        // joins this branch. Admit only exact edge participants from this source
        // publication; names never fabricate turns, activity or control scope.
        let related: BTreeSet<_> = edges
            .values()
            .flat_map(|edge| {
                ["from", "to"]
                    .into_iter()
                    .filter_map(move |field| edge[field].as_str().map(str::to_owned))
            })
            .filter(|sid| sid != "you" && !sessions.contains_key(sid))
            .collect();
        let mut participants = BTreeMap::new();
        missing |= related.len() > 128;
        for sid in related.into_iter().take(128) {
            let header: Option<(String, String)> = tx
                .query_row(
                    "SELECT generation,source_generation FROM session_trace_headers WHERE session_key=?1",
                    [&sid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| 503u16)?;
            let Some((version, source)) = header else {
                missing = true;
                continue;
            };
            let metadata = crate::history_projection::catalog(&tx, &sid)
                .map_err(|_| 503u16)?
                .or(tx
                    .query_row(
                        "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1",
                        [&sid],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
                    .map_err(|_| 503u16)?
                    .flatten());
            let Some(metadata) = metadata else {
                missing = true;
                continue;
            };
            let row: crate::model::summary::CatalogRow =
                serde_json::from_value(decode(metadata)?).map_err(|_| 503u16)?;
            if row.key != sid
                || crate::retention::source_generation(&row.sources).map_err(|_| 503u16)? != source
            {
                missing = true;
                continue;
            }
            versions.push(json!([sid, version]));
            participants.insert(sid, row.name);
        }
        let mut owners: BTreeSet<_> = edges
            .values()
            .flat_map(|edge| {
                ["brief_session", "result_session"]
                    .into_iter()
                    .filter_map(move |field| edge[field].as_str().map(str::to_owned))
            })
            .collect();
        owners.extend(sessions.keys().cloned());
        let preview_order: Vec<_> = sessions
            .keys()
            .cloned()
            .chain(
                owners
                    .iter()
                    .filter(|sid| !sessions.contains_key(*sid))
                    .cloned(),
            )
            .collect();
        let mut previews = BTreeMap::new();
        let mut previews_incomplete = owners.len() > 128;
        for sid in preview_order.into_iter().take(128) {
            let header:Option<(String,Option<String>)>=tx.query_row("SELECT source_generation,json_extract(metadata,'$.content_generation') FROM session_trace_headers WHERE session_key=?1",[&sid],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|503u16)?;
            let Some((source, Some(content))) = header else {
                previews_incomplete = true;
                continue;
            };
            let metadata=crate::history_projection::catalog(&tx,&sid).map_err(|_|503u16)?.or(tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1",[&sid],|row|row.get::<_,String>(0)).optional().map_err(|_|503u16)?);
            let Some(metadata) = metadata else {
                previews_incomplete = true;
                continue;
            };
            let row: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).map_err(|_| 503u16)?;
            if row.key != sid
                || crate::retention::source_generation(&row.sources).map_err(|_| 503u16)? != source
                || content.len() != 64
            {
                previews_incomplete = true;
                continue;
            }
            previews.insert(sid, content);
        }
        let version = format!(
            "{:x}",
            Sha256::digest(json!([generation, versions]).to_string().as_bytes())
        );
        tx.commit().map_err(|_| 503u16)?;
        Ok(
            json!({"api":1,"source_key":source_key,"catalog_key":key,"root":root,"generation":version,"identity":identity,"sessions":sessions,"participants":participants,"turns":turns,"handoffs":edges.into_values().collect::<Vec<_>>(),"previews":previews,"frontier":frontier,"incomplete":missing,"previews_incomplete":previews_incomplete}),
        )
    };
    match read() {
        Ok(value) => ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
            etag: None,
        },
        Err(status) => crate::catalog::error(
            status,
            "causal_trace",
            match status {
                400 => "Invalid causal trace selection",
                404 => "Recorded causal turn unavailable",
                409 => "Causal source projection changed",
                413 => "Causal branch exceeds its bound; open a narrower recorded turn",
                _ => "Causal projection unavailable",
            },
            matches!(status, 409 | 503),
        ),
    }
}

#[cfg(test)]
mod cost_tests {
    use super::family_costs;

    #[test]
    fn producer_rolls_up_family_cost_once_and_keeps_unknowns_and_cycles_unknown() {
        let costs = family_costs(
            [
                ("parent", None, Some(1.0)),
                ("review", Some("parent"), Some(0.56)),
                ("nested", Some("review"), Some(0.05)),
                ("unpriced", Some("unknown"), None),
                ("unknown", None, Some(2.0)),
                ("cycle-a", Some("cycle-b"), Some(3.0)),
                ("cycle-b", Some("cycle-a"), Some(4.0)),
                ("cycle-leaf", Some("cycle-a"), Some(0.1)),
                ("orphan", Some("missing"), Some(0.2)),
            ]
            .into_iter(),
        );
        assert!((costs["parent"].unwrap() - 1.61).abs() < 1e-12);
        assert!((costs["review"].unwrap() - 0.61).abs() < 1e-12);
        assert_eq!(costs["nested"], Some(0.05));
        assert_eq!(costs["unknown"], None);
        assert_eq!(costs["unpriced"], None);
        assert_eq!(costs["cycle-a"], None);
        assert_eq!(costs["cycle-b"], None);
        assert_eq!(costs["cycle-leaf"], Some(0.1));
        assert_eq!(costs["orphan"], Some(0.2));
    }
}
