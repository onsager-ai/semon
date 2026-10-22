//! Compact source publications for Home and Analytics. Ordinary reads open only
//! the derived SQLite projection. Visible previews use qualified, bounded native
//! reads; message text never enters the projection or a workspace ViewerCore.
use crate::{
    Options, analytics,
    events::EventCache,
    model::{Handoff, Session, Turn},
    viewer::ViewerReply,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io, sync::Arc};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_overview (
 session_key TEXT PRIMARY KEY, last_ms INTEGER NOT NULL, state TEXT NOT NULL, summary TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS session_overview_order ON session_overview(last_ms DESC,session_key);
CREATE INDEX IF NOT EXISTS session_overview_state ON session_overview(state,last_ms DESC,session_key);
";

pub(crate) mod comparison {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Option<Arc<crate::comparison::Item>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_deref().serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Arc<crate::comparison::Item>>, D::Error> {
        Option::<crate::comparison::Item>::deserialize(d).map(|item| item.map(Arc::new))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    activity: Option<analytics::Activity>,
    home: Value,
    #[serde(default)]
    source_generation: String,
}
impl Summary {
    pub(crate) fn query_recipe(&self, id: &str) -> Value {
        self.home["content"]["events"][id].clone()
    }
    pub(crate) fn capture(
        key: &str,
        session: &Session,
        activity: Option<&analytics::Activity>,
        handoffs: &[&Handoff],
        holding_turns: &BTreeMap<&str, &Turn>,
        content: Value,
        parent: Option<&str>,
    ) -> Self {
        // Persist timing, status and native recipes. Resolve visible text only
        // through the selected source reader; the index contains no bodies.
        let events: Vec<_> = handoffs
            .iter()
            .filter(|h| h.kind == "toyou" && h.from == key && (h.at >= crate::model::now_ms().saturating_sub(analytics::KEEP_MS) || h.done.is_some_and(|done|done >= crate::model::now_ms().saturating_sub(analytics::KEEP_MS)) || matches!(h.status,"wait"|"work"|"new")))
            .map(|h| {
                let holding=holding_turns.get(h.id.as_str());
                json!({"id":h.id,"at":h.at,"done":h.done,"ask":h.ask,"status":h.status,
                "brief":"","answer":if h.declined {Some(Vec::<String>::new())} else {None},"declined":h.declined,"turn":holding.map(|turn|&turn.id)})
            })
            .collect();
        Self {
            activity: activity.cloned(),
            source_generation: String::new(),
            home: json!({"name":session.name,"kind":session.kind,"harness":session.harness,
            "model":session.model,"repo":session.repo,"state":session.state,"last":session.last,
            "start":session.start,"parent":parent,"branch":session.branch,"waiting_for":null,"waiting_since":session.waiting_since,
            "activity":session.activity.as_ref().map(|(name,_,age,at)|(name,"",age,at)),"detail":null,"detail_from":content["detail_from"],"detail_from_name":content["detail_from_name"],"content":content,"cost":session.cost.usd,"events":events}),
        }
    }
    pub(crate) fn with_sources(
        mut self,
        sources: &[crate::model::summary::CatalogSource],
    ) -> io::Result<Self> {
        self.source_generation = crate::retention::source_generation(sources).map_err(failed)?;
        Ok(self)
    }
}

pub(crate) fn publish(
    tx: &Transaction<'_>,
    key: &str,
    last: i64,
    summary: &Summary,
) -> rusqlite::Result<()> {
    publish_measured(
        tx,
        key,
        last,
        summary,
        &mut crate::native_timing::Measurements::default(),
    )
}

fn publish_measured(
    tx: &Transaction<'_>,
    key: &str,
    last: i64,
    summary: &Summary,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let started = measurements.start();
    let text = serde_json::to_string(summary)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    measurements.record("summary_serialization", started, 1);
    let started = measurements.start();
    let changed = tx.prepare_cached("INSERT INTO session_overview VALUES(?1,?2,?3,?4) ON CONFLICT(session_key) DO UPDATE SET last_ms=excluded.last_ms,state=excluded.state,summary=excluded.summary WHERE session_overview.summary IS NOT excluded.summary")?.execute(params![key,last,summary.home["state"].as_str().unwrap_or("off"),text])?;
    measurements.record("summary_upsert", started, 1);
    if changed > 0 {
        let started = measurements.start();
        tx.prepare_cached("INSERT INTO meta(key,value) VALUES('overview_generation','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)")?.execute([])?;
        measurements.record("generation", started, 1);
    }
    Ok(())
}

/// Current native publication cannot replace a complete historical derivation
/// with just the files still on disk. Rebuild missing generations through the
/// qualified producer; unchanged generations retain their exact activity.
pub(crate) fn publish_current(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
) -> rusqlite::Result<()> {
    publish_current_measured(tx, row, &mut crate::native_timing::Measurements::default())
}

fn update_retained_observation(prior: &mut Value, current: &Value) {
    // Retained source recipes cannot describe a different current call.
    let same_activity = prior["activity"][0] == current["activity"][0]
        && prior["activity"][3] == current["activity"][3];
    for field in ["name", "state", "waiting_since", "waiting_for", "activity"] {
        prior[field] = current[field].clone();
    }
    if !same_activity {
        prior["activity"] = Value::Null;
        prior["content"]["activity"] = Value::Null;
    }
}

pub(crate) fn publish_current_measured(
    tx: &Transaction<'_>,
    row: &crate::model::summary::CatalogRow,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let started = measurements.start();
    let history = crate::history_projection::catalog(tx, &row.key)?
        .map(|text| serde_json::from_str::<crate::model::summary::CatalogRow>(&text))
        .transpose()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    measurements.record("history_lookup", started, 1);
    if let Some(history) = history
        && history.sources != row.sources
    {
        let started = measurements.start();
        let expected = crate::retention::source_generation(&history.sources)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let prior=tx.query_row("SELECT CASE WHEN octet_length(summary)<=4194304 THEN summary ELSE NULL END FROM session_overview WHERE session_key=?1",[&row.key],|row|row.get::<_,String>(0)).optional()?.map(|text|serde_json::from_str::<Summary>(&text)).transpose().map_err(|error|rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        measurements.record("history_prior", started, 1);
        if let Some(mut prior) = prior
            && prior.source_generation == expected
        {
            if let Some(current) = &row.overview {
                update_retained_observation(&mut prior.home, &current.home);
                if let Some(activity) = &mut prior.activity {
                    activity.working = current
                        .activity
                        .as_ref()
                        .is_some_and(|activity| activity.working);
                }
                publish_measured(
                    tx,
                    &row.key,
                    history.last.unwrap_or(0),
                    &prior,
                    measurements,
                )?;
            }
            if let Some(current) = &row.trace {
                let started = measurements.start();
                crate::causal_trace::update_current_observation(
                    tx,
                    &row.key,
                    current,
                    content_generation(&prior),
                )?;
                measurements.record("trace_update", started, 1);
            }
            let started = measurements.start();
            tx.prepare_cached("DELETE FROM meta WHERE key=?1")?
                .execute([format!("overview_needed:{}", row.key)])?;
            measurements.record("cleanup", started, 1);
        } else {
            let started = measurements.start();
            tx.prepare_cached("INSERT OR REPLACE INTO meta(key,value) VALUES(?1,'1')")?
                .execute([format!("overview_needed:{}", row.key)])?;
            measurements.record("repair_request", started, 1);
        }
        return Ok(());
    }
    if let Some(trace) = &row.trace {
        let started = measurements.start();
        if started.is_some() {
            crate::causal_trace::publish_measured(tx, row, trace, measurements)?;
        } else {
            crate::causal_trace::publish(tx, row, trace)?;
        }
        measurements.record("trace_publish_inclusive", started, 1);
    }
    if let Some(summary) = &row.overview {
        publish_measured(tx, &row.key, row.last.unwrap_or(0), summary, measurements)?;
    }
    let started = measurements.start();
    tx.prepare_cached("DELETE FROM meta WHERE key=?1")?
        .execute([format!("overview_needed:{}", row.key)])?;
    measurements.record("cleanup", started, 1);
    Ok(())
}

/// A host authorizes each configured source before and after these reads.
/// `up` is an observed machine fact, never inferred from cached history.
#[derive(Clone)]
pub struct OverviewSource {
    pub key: String,
    pub label: String,
    pub options: Options,
    pub up: bool,
}
struct Saved {
    id: String,
    key: String,
    source: String,
    summary: Summary,
    content_generation: String,
}
fn failed(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
fn read(
    source: &OverviewSource,
    since: i64,
    analytics: bool,
) -> io::Result<(Vec<Saved>, bool, String)> {
    let mut connection = Connection::open_with_flags(
        EventCache::path(&source.options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(failed)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(1))
        .map_err(failed)?;
    let db = connection.transaction().map_err(failed)?;
    crate::events::store::require_current(&db)?;
    let complete = db.query_row("SELECT value FROM meta WHERE key='catalog_completeness'",[],|row|row.get::<_,String>(0)).optional().map_err(failed)?.as_deref()==Some("complete")
        && !db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog c LEFT JOIN session_overview o USING(session_key) WHERE c.last_ms>=?1 AND (o.session_key IS NULL OR EXISTS(SELECT 1 FROM meta WHERE key='overview_needed:'||c.session_key)) LIMIT 1)",[since],|row|row.get::<_,bool>(0)).map_err(failed)?;
    let mut generation = db
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(failed)?
        .unwrap_or_default();
    let overview_generation = db
        .query_row(
            "SELECT value FROM meta WHERE key='overview_generation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(failed)?
        .unwrap_or_default();
    generation.push('/');
    generation.push_str(&overview_generation);
    // Home needs its compact timing and event summaries, but never the
    // Analytics activity intervals, usage series or comparison workloads.
    let mut statement = db.prepare("SELECT o.session_key,CASE WHEN octet_length(o.summary)<=4194304 THEN CASE WHEN ?2 THEN json_remove(o.summary,'$.home.content') ELSE json_remove(o.summary,'$.home.content','$.activity') END ELSE NULL END,COALESCE(c.lifecycle,'retained'),json_extract(h.metadata,'$.content_generation'),json_extract(c.metadata,'$.presentation_kind') FROM session_overview o JOIN session_catalog c USING(session_key) LEFT JOIN session_trace_headers h USING(session_key) WHERE o.session_key IN (SELECT session_key FROM session_overview WHERE last_ms>=?1 UNION SELECT session_key FROM session_overview WHERE state IN ('work','wait') UNION SELECT session_key FROM (SELECT session_key FROM session_catalog ORDER BY last_ms DESC,session_key LIMIT 60)) ORDER BY o.last_ms DESC,o.session_key").map_err(failed)?;
    let rows = statement
        .query_map(params![since, analytics], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(failed)?;
    let mut records = rows
        .map(|row| {
            let (key, text, lifecycle, content_generation, kind) = row.map_err(failed)?;
            let mut summary: Summary = serde_json::from_str(&text).map_err(failed)?;
            summary.home["kind"] =
                json!(kind.or_else(|| summary.home["kind"].as_str().map(str::to_owned)));
            let content_generation = content_generation.unwrap_or_default();
            if lifecycle != "current" || !source.up {
                summary.home["state"] = json!("off");
                if let Some(activity) = &mut summary.activity {
                    activity.working = false;
                }
            }
            let id = json!([source.key, key]).to_string();
            if let Some(activity) = &mut summary.activity
                && let Some(item) = &mut activity.comparison
            {
                Arc::make_mut(item).sid = id.clone();
            }
            Ok(Saved {
                id,
                key,
                source: source.key.clone(),
                summary,
                content_generation,
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    drop(statement);
    let mut recent=db.prepare("SELECT c.session_key,CASE WHEN length(CAST(c.metadata AS BLOB))<=1048576 THEN c.metadata ELSE NULL END FROM (SELECT session_key,metadata,last_ms FROM session_catalog ORDER BY last_ms DESC,session_key LIMIT 60) c LEFT JOIN session_overview o USING(session_key) WHERE o.session_key IS NULL ORDER BY c.last_ms DESC,c.session_key").map_err(failed)?;
    for row in recent
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(failed)?
    {
        let (key, encoded) = row.map_err(failed)?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&encoded).map_err(failed)?;
        if row.key != key {
            return Err(failed("Recent metadata identity changed"));
        }
        records.push(Saved {id:json!([source.key,key]).to_string(),key,source:source.key.clone(),content_generation:String::new(),summary:Summary {source_generation:String::new(),activity:None,home:json!({"name":row.name,"kind":row.presentation_kind,"harness":row.harness,"model":row.model,"repo":row.repo,"state":"off","last":row.last.unwrap_or(0),"start":row.start.unwrap_or(0),"parent":row.parent,"branch":row.branch,"waiting_for":null,"waiting_since":null,"cost":row.cost.usd,"events":[]})}});
    }
    drop(recent);
    db.commit().map_err(failed)?;
    Ok((records, complete, generation))
}
pub(crate) fn content_generation(summary: &Summary) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(summary).expect("owned summary"))
    )
}
/// Resolve only named Home previews through the same qualified source reader as
/// selected transcripts. All message text remains request-local.
pub fn session_home_details_with_mode(
    options: &Options,
    source_key: &str,
    query: &str,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
) -> ViewerReply {
    session_home_details_inner(options, source_key, query, provider, mode, true)
}

pub(crate) fn session_home_details_inner(
    options: &Options,
    source_key: &str,
    query: &str,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    include_session: bool,
) -> ViewerReply {
    let read = || -> io::Result<Value> {
        let args: Vec<_> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .collect();
        for name in [
            "sid",
            "generation",
            "events",
            "turns",
            "scope",
            "machine",
            "session_fields",
        ] {
            if args.iter().filter(|(key, _)| *key == name).count() > 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Duplicate Home detail argument",
                ));
            }
        }
        let value = |name: &str| {
            args.iter()
                .find(|(key, _)| *key == name)
                .and_then(|(_, value)| crate::viewer::decoded(value))
        };
        let key = value("sid")
            .filter(|key| !key.is_empty() && key.len() <= 4096)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Supply a session key"))?;
        let expected = value("generation").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Supply an overview generation")
        })?;
        if expected.len() != 64
            || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            || value("scope").as_deref() != Some("retained_history")
            || args.iter().any(|(name, _)| {
                !matches!(
                    *name,
                    "sid"
                        | "generation"
                        | "events"
                        | "turns"
                        | "scope"
                        | "machine"
                        | "session_fields"
                )
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid Home detail scope or generation",
            ));
        }
        let ids: Vec<String> = serde_json::from_str(
            &value("events").unwrap_or_else(|| "[]".into()),
        )
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid event selection"))?;
        let session_fields: Vec<String> = serde_json::from_str(
            &value("session_fields").unwrap_or_else(|| "[\"detail\",\"activity\"]".into()),
        )
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid session field selection",
            )
        })?;
        if session_fields.len() > 2
            || session_fields
                .iter()
                .any(|field| !matches!(field.as_str(), "detail" | "activity"))
            || (session_fields.len() == 2 && session_fields[0] == session_fields[1])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid session field selection",
            ));
        }
        let turns: Vec<String> =
            serde_json::from_str(&value("turns").unwrap_or_else(|| "[]".into())).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid turn selection")
            })?;
        if ids.len() + turns.len() > 100
            || ids
                .iter()
                .chain(&turns)
                .any(|id| id.is_empty() || id.len() > 4096)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Home detail selection exceeds its bound",
            ));
        }
        let identity = crate::session_catalog_history_identity(options, source_key, &key)
            .map_err(failed)?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Recorded session unavailable")
            })?;
        let mut db = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(failed)?;
        db.busy_timeout(std::time::Duration::from_secs(1))
            .map_err(failed)?;
        let tx = db.transaction().map_err(failed)?;
        crate::events::store::require_current(&tx)?;
        let generation: String = tx
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if generation != identity.generation {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Source observation changed",
            ));
        }
        let encoded: String=tx.query_row("SELECT CASE WHEN octet_length(summary)<=4194304 THEN summary ELSE NULL END FROM session_overview WHERE session_key=?1",[&key],|row|row.get(0)).map_err(failed)?;
        let summary: Summary = serde_json::from_str(&encoded).map_err(failed)?;
        if content_generation(&summary) != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Home observation changed",
            ));
        }
        let metadata = crate::history_projection::catalog(&tx, &key)
            .map_err(failed)?
            .or(tx
                .query_row(
                    "SELECT metadata FROM session_catalog WHERE session_key=?1",
                    [&key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?)
            .ok_or_else(|| failed("Source metadata unavailable"))?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&metadata).map_err(failed)?;
        if row.key != key {
            return Err(failed("Source metadata identity changed"));
        }
        if summary.source_generation
            != crate::retention::source_generation(&row.sources).map_err(failed)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Home native generation changed",
            ));
        }
        let history: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_history_projections WHERE session_key=?1 AND version=?2 AND incomplete=0)",params![key,crate::slot_projection::VERSION],|row|row.get(0)).map_err(failed)?;
        let table = if history {
            "session_history_slots"
        } else {
            "session_slots"
        };
        let mut turn_recipes = std::collections::BTreeMap::new();
        let mut statement=tx.prepare(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM {table} WHERE session_key=?1 AND json_extract(metadata,'$.turn')=?2 AND (json_extract(metadata,'$.first')=1 OR json_extract(metadata,'$.recipe.recipe')='U') ORDER BY CASE WHEN json_extract(metadata,'$.recipe.recipe')='U' THEN 0 ELSE 1 END,slot LIMIT 2")).map_err(failed)?;
        for turn in &turns {
            let selected = statement
                .query_map(params![key, turn], |row| row.get::<_, String>(0))
                .map_err(failed)?;
            for encoded in selected {
                let slot: crate::slot_projection::SavedSlot =
                    serde_json::from_str(&encoded.map_err(failed)?).map_err(failed)?;
                if let Some(file) = slot.file {
                    turn_recipes.insert(turn.clone(),json!({"path":file.path,"offset":slot.offset,"block":slot.block,"kind":"trace_prompt","field":slot.field}));
                    break;
                }
            }
        }
        drop(statement);
        tx.commit().map_err(failed)?;
        let content = &summary.home["content"];
        let mut result = json!({"api":1,"identity":identity,"generation":expected,"session":{"detail":null,"detail_from":summary.home["detail_from"],"detail_from_name":summary.home["detail_from_name"],"activity":summary.home["activity"]},"events":{},"turns":{},"incomplete":false});
        crate::transcript_range::with_source_reader(
            options,
            &row.harness,
            &row.sources,
            provider,
            mode,
            |reader, references| {
                let bounded = crate::source_reader::BudgetedReader::new(reader, 256);
                let reader: &dyn crate::SessionSourceReader = &bounded;
                let mut left = 4 * 1024 * 1024;
                let mut records =
                    std::collections::BTreeMap::<(std::path::PathBuf, u64), Value>::new();
                let mut record = |recipe: &Value| -> io::Result<Value> {
                    let path = std::path::PathBuf::from(
                        recipe["path"]
                            .as_str()
                            .ok_or_else(|| failed("Missing content recipe"))?,
                    );
                    let offset = recipe["offset"]
                        .as_u64()
                        .ok_or_else(|| failed("Invalid content offset"))?;
                    let reference = references
                        .get(&path)
                        .ok_or_else(|| failed("Content recipe source is outside this session"))?;
                    if !recipe["field"].is_null() {
                        let field: crate::native_field::NativeField =
                            serde_json::from_value(recipe["field"].clone()).map_err(failed)?;
                        if field.record_offset != offset
                            || Some(u64::from(field.block)) != recipe["block"].as_u64()
                        {
                            return Err(failed("Native preview descriptor changed"));
                        }
                        let text = crate::native_field::preview(
                            reader,
                            reference,
                            &field,
                            recipe["kind"].as_str().unwrap_or(""),
                            &mut left,
                        )?;
                        return crate::native_field::preview_record(
                            &field,
                            &text,
                            recipe["kind"].as_str().unwrap_or(""),
                        )
                        .ok_or_else(|| failed("Native preview pointer unavailable"));
                    }
                    let address = (path.clone(), offset);
                    if let Some(value) = records.get(&address) {
                        return Ok(value.clone());
                    }
                    let value = crate::source_reader::read_record_bounded(
                        reader,
                        reference,
                        offset,
                        1024 * 1024,
                        &mut left,
                    )?;
                    records.insert(address, value.clone());
                    Ok(value)
                };
                for id in &ids {
                    let fields = &content["events"][id];
                    if fields.is_null() {
                        if *id != format!("wait:{}", json!([source_key, key])) {
                            result["incomplete"] = json!(true);
                        }
                        continue;
                    }
                    let question =
                        (fields["brief"]["kind"] == "question").then(|| record(&fields["brief"]));
                    let mut event = json!({"brief":"","answer":null,"origin":null});
                    for field in ["brief", "answer", "origin", "result"] {
                        let recipe = &fields[field];
                        if recipe.is_null() {
                            continue;
                        }
                        let mut recipe = recipe.clone();
                        recipe["question_block"] = fields["brief"]["block"].clone();
                        match record(&recipe) {
                            Ok(value) => {
                                event[field] = crate::model::overview_text(
                                    &recipe,
                                    &value,
                                    question.as_ref().and_then(|value| value.as_ref().ok()),
                                )
                            }
                            Err(_) => result["incomplete"] = json!(true),
                        }
                    }
                    result["events"][id] = event;
                }
                for turn in &turns {
                    match turn_recipes.get(turn).map(&mut record) {
                        Some(Ok(value)) => {
                            result["turns"][turn] =
                                crate::model::overview_text(&turn_recipes[turn], &value, None);
                        }
                        _ => result["incomplete"] = json!(true),
                    }
                }
                for field in ["detail", "activity"].into_iter().filter(|field| {
                    include_session && session_fields.iter().any(|selected| selected == field)
                }) {
                    let recipe = &content[field];
                    if recipe.is_null() {
                        continue;
                    }
                    match record(recipe) {
                        Ok(value) => {
                            let text = crate::model::overview_text(recipe, &value, None);
                            if field == "activity" {
                                result["session"][field][1] = text;
                            } else {
                                result["session"][field] = text;
                            }
                        }
                        Err(_) => result["incomplete"] = json!(true),
                    }
                }
                Ok(())
            },
        )?;
        let after =
            crate::session_catalog_history_identity(options, source_key, &key).map_err(failed)?;
        if after
            .as_ref()
            .is_none_or(|after| after.generation != generation)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Source observation changed during Home read",
            ));
        }
        Ok(result)
    };
    match read() {
        Ok(value) => reply(value),
        Err(error) => crate::catalog::error(
            match error.kind() {
                io::ErrorKind::InvalidInput => 400,
                io::ErrorKind::NotFound => 404,
                io::ErrorKind::InvalidData => 409,
                _ => 503,
            },
            "home_details_unavailable",
            "The recorded preview is unavailable. Retry with the current source observation.",
            true,
        ),
    }
}

fn reply(value: Value) -> ViewerReply {
    ViewerReply {
        status: 200,
        content_type: "application/json; charset=utf-8",
        body: value.to_string().into_bytes(),
        etag: None,
    }
}

/// Optional exact source boundary for focused machine activity. Hosts use the
/// same validated key before admitting options or scheduling source observation.
pub fn overview_source_key(query: &str) -> io::Result<Option<String>> {
    let mut selected = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        if pair == "source" || pair.starts_with("source=") {
            let value = pair
                .strip_prefix("source=")
                .and_then(crate::viewer::decoded)
                .filter(|value| !value.is_empty() && value.len() <= 4096);
            if selected.is_some() || value.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid overview source",
                ));
            }
            selected = value;
        }
    }
    Ok(selected)
}

/// Aggregate compact sixty-day activity, preserving source-qualified selection
/// and existing cost, waits, concurrency, allowance and comparison semantics.
pub fn session_overview_reply(
    sources: &[OverviewSource],
    page: &str,
    query: &str,
    now: i64,
) -> io::Result<ViewerReply> {
    if !query.is_empty() && query.split('&').any(str::is_empty) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid overview arguments",
        ));
    }
    let selected_source = overview_source_key(query)?;
    let sources: Vec<_> = sources
        .iter()
        .filter(|source| {
            selected_source
                .as_ref()
                .is_none_or(|key| source.key == *key)
        })
        .collect();
    if selected_source.is_some() && sources.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Overview source unavailable",
        ));
    }
    // Page the existing metadata migration queue by its authorized source and
    // session keys. A failed item cannot indefinitely hide later publications.
    let mut after: Option<[String; 2]> = None;
    let mut retained_query = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        if pair.starts_with("source=") {
            continue;
        }
        if let Some(value) = pair.strip_prefix("observation_after=") {
            if after.is_some() || value.len() > 32768 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid observation cursor",
                ));
            }
            let decoded = crate::viewer::decoded(value).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid observation cursor")
            })?;
            let selected: [String; 2] = serde_json::from_str(&decoded).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid observation cursor")
            })?;
            if selected
                .iter()
                .any(|key| key.is_empty() || key.len() > 4096)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid observation cursor",
                ));
            }
            after = Some(selected);
        } else {
            retained_query.push(pair);
        }
    }
    let query = retained_query.join("&");
    let query = query.as_str();
    let mut seen = std::collections::BTreeSet::new();
    let mut has_seen = false;
    let mut offsets = std::collections::BTreeMap::new();
    if page == "home" && !query.is_empty() {
        for pair in query.split('&') {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid Home query"))?;
            if key == "seen" {
                if has_seen || value.len() > 1536 * 1024 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Invalid seen results",
                    ));
                }
                let decoded = crate::viewer::decoded(value).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid seen results")
                })?;
                let ids: Vec<String> = serde_json::from_str(&decoded).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid seen results")
                })?;
                if ids.len() > 2000
                    || decoded.len() > 512 * 1024
                    || ids.iter().any(|id| id.is_empty() || id.len() > 4096)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Seen results exceed their bound",
                    ));
                }
                has_seen = true;
                seen.extend(ids);
                continue;
            }
            if !matches!(key, "inbox_offset" | "working_offset" | "answered_offset")
                || offsets.contains_key(key)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid Home pagination",
                ));
            }
            let offset = value
                .parse::<usize>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid Home cursor"))?;
            if offset > 1_000_000 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Home cursor exceeds its bound",
                ));
            }
            offsets.insert(key.to_owned(), offset);
        }
    }
    let mut records = Vec::new();
    let mut observations = Vec::new();
    let mut versions = Vec::new();
    let mut pending = Vec::new();
    let mut ordered: Vec<_> = sources.iter().collect();
    ordered.sort_by_key(|source| &source.key);
    for source in &ordered {
        if pending.len() == 8 {
            break;
        }
        let cursor = match &after {
            Some([machine, _]) if source.key < *machine => continue,
            Some([machine, key]) if source.key == *machine => key.as_str(),
            _ => "",
        };
        for key in
            crate::overview_repair::candidates_after(&source.options, 8 - pending.len(), cursor)
                .unwrap_or_default()
        {
            pending.push(json!({"source_key":source.key,"catalog_key":key}));
        }
    }
    if pending.is_empty() && after.is_some() {
        for source in &ordered {
            if pending.len() == 8 {
                break;
            }
            for key in crate::overview_repair_candidates(&source.options, 8 - pending.len())
                .unwrap_or_default()
            {
                pending.push(json!({"source_key":source.key,"catalog_key":key}));
            }
        }
    }
    for source in &sources {
        match read(
            source,
            now.saturating_sub(analytics::KEEP_MS),
            page == "analytics",
        ) {
            Ok((rows, complete, generation)) => {
                versions.push(json!([source.key, generation, source.up]));
                records.extend(rows);
                if !complete {
                    observations.push(json!({"source_key":source.key,"state":"partial"}));
                }
            }
            Err(_) => observations.push(json!({"source_key":source.key,"state":"unavailable"})),
        }
    }
    use sha2::{Digest, Sha256};
    let version = format!(
        "{:x}",
        Sha256::digest(json!(versions).to_string().as_bytes())
    );
    let mut recent: Vec<_> = records.iter().collect();
    recent.sort_by_key(|row| std::cmp::Reverse(row.summary.home["last"].as_i64().unwrap_or(0)));
    let recent: Vec<_> = recent
        .into_iter()
        .take(60)
        .map(|row| {
            let mut session = row.summary.home.clone();
            if !row.content_generation.is_empty() {
                session["content_generation"] = json!(row.content_generation);
            }
            session.as_object_mut().unwrap().remove("events");
            session.as_object_mut().unwrap().remove("content");
            json!({"id":row.id,"source_key":row.source,"catalog_key":row.key,"session":session})
        })
        .collect();
    if page == "analytics" {
        let request = analytics::Request::parse(query).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid analytics range or filters",
            )
        })?;
        let rows: Vec<_> = records
            .iter()
            .filter_map(|row| {
                Some(analytics::Row {
                    id: &row.id,
                    machine: &row.source,
                    activity: row.summary.activity.as_ref()?,
                })
            })
            .collect();
        let mut data = analytics::answer(&rows, &request, now, &version);
        fn selected_ids(value: &Value, ids: &mut std::collections::BTreeSet<String>) {
            match value {
                Value::Object(row) => {
                    if let Some(id) = row.get("sid").and_then(Value::as_str) {
                        ids.insert(id.into());
                    }
                    for child in row.values() {
                        selected_ids(child, ids);
                    }
                }
                Value::Array(rows) => {
                    for row in rows {
                        selected_ids(row, ids);
                    }
                }
                _ => {}
            }
        }
        let mut named = std::collections::BTreeSet::new();
        selected_ids(&data, &mut named);
        if let Some(sessions) = data["sessions"].as_object() {
            named.extend(sessions.keys().cloned());
        }
        data["recent"] = json!(recent);
        data["now"] = json!(now);
        data["observations"] = json!(observations);
        data["pending_observations"] = json!(pending);
        data["machine_labels"] = json!(
            sources
                .iter()
                .map(|s| (s.key.clone(), json!(s.label)))
                .collect::<serde_json::Map<_, _>>()
        );
        data["selections"] = json!(
            records
                .iter()
                .filter(|row| named.contains(&row.id))
                .map(|row| (
                    row.id.clone(),
                    json!({"source_key":row.source,"catalog_key":row.key})
                ))
                .collect::<serde_json::Map<_, _>>()
        );
        return Ok(reply(data));
    }
    if page != "home" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unknown overview page",
        ));
    }
    let mut waiting = Vec::new();
    let mut working = Vec::new();
    let mut answered = Vec::new();
    for row in &records {
        let home = &row.summary.home;
        let mut preview = home.clone();
        preview.as_object_mut().unwrap().remove("events");
        preview.as_object_mut().unwrap().remove("content");
        preview["content_generation"] =
            json!((!row.content_generation.is_empty()).then_some(&row.content_generation));
        let selected =
            json!({"id":row.id,"source_key":row.source,"catalog_key":row.key,"session":preview});
        if home["state"] == "work" {
            working.push(selected.clone());
        }
        let mut has_wait = false;
        for event in home["events"].as_array().into_iter().flatten() {
            let window = sources
                .iter()
                .find(|source| source.key == row.source)
                .map(|source| i64::try_from(source.options.since.as_millis()).unwrap_or(i64::MAX))
                .unwrap_or(analytics::DAY_MS);
            if event["at"].as_i64().unwrap_or(0) < now.saturating_sub(window)
                && event["done"].as_i64().unwrap_or(0) < now.saturating_sub(window)
                && !matches!(event["status"].as_str(), Some("wait" | "work" | "new"))
            {
                continue;
            }
            let mut value = selected.clone();
            value["event"] = event.clone();
            let seen_result = event["ask"] == "result"
                && seen.contains(&json!([row.source, event["id"]]).to_string());
            if !seen_result && (event["status"] == "wait" || event["ask"] == "result") {
                has_wait |= event["status"] == "wait";
                waiting.push(value.clone());
            }
            if event["status"] == "done" {
                answered.push(value);
            }
        }
        if home["state"] == "wait" && !has_wait {
            let mut value = selected;
            value["event"] = json!({"id":format!("wait:{}",row.id),"at":home["waiting_since"].as_i64().unwrap_or(home["last"].as_i64().unwrap_or(now)),
                "done":null,"ask":"decision","status":"wait","brief":home["waiting_for"].as_str().unwrap_or("Waiting for your input or permission"),"answer":null,"declined":false});
            waiting.push(value);
        }
    }
    waiting.sort_by_key(|row| std::cmp::Reverse(row["event"]["at"].as_i64().unwrap_or(0)));
    working.sort_by_key(|row| std::cmp::Reverse(row["session"]["last"].as_i64().unwrap_or(0)));
    answered.sort_by_key(|row| std::cmp::Reverse(row["event"]["at"].as_i64().unwrap_or(0)));
    let totals = json!({"waiting":waiting.len(),"working":working.len(),"answered":answered.len()});
    let mut next = serde_json::Map::new();
    for (key, rows) in [
        ("inbox", &mut waiting),
        ("working", &mut working),
        ("answered", &mut answered),
    ] {
        let offset = offsets.get(&format!("{key}_offset")).copied().unwrap_or(0);
        next.insert(
            key.into(),
            if rows.len() > offset.saturating_add(100) {
                json!(offset + 100)
            } else {
                Value::Null
            },
        );
        *rows = rows.iter().skip(offset).take(100).cloned().collect();
    }
    Ok(reply(
        json!({"api":1,"version":version,"now":now,"observations":observations,"pending_observations":pending,"next":next,"totals":totals,"inbox":waiting,"working":working,"answered":answered,
        "recent":recent,"machines":sources.iter().map(|s| {
            let observed:Vec<_>=records.iter().filter(|row|row.source==s.key).collect();
            json!({"key":s.key,"label":s.label,"up":s.up,
                "sessions":observed.len(),
                "working":observed.iter().filter(|row|row.summary.home["state"]=="work").count(),
                "last":(!s.up).then(|| s.options.facts.as_ref().and_then(|path| crate::read_facts(path).ok()).and_then(|facts| facts.offline_since)).flatten()
                    .or_else(|| observed.iter().filter_map(|row|row.summary.home["last"].as_i64()).max())})
        }).collect::<Vec<_>>()}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_activity_never_joins_old_arguments_to_a_new_observation() {
        let prior = json!({"name":"before","state":"work","activity":["Read","","1s",10],"content":{"activity":{"offset":42},"events":{"edge":{}}}});
        for activity in [
            json!(["Read", "", "2s", 11]),
            json!(["Edit", "", "2s", 10]),
            Value::Null,
        ] {
            let mut retained = prior.clone();
            update_retained_observation(
                &mut retained,
                &json!({"name":"after","state":"work","activity":activity}),
            );
            assert!(retained["activity"].is_null());
            assert!(retained["content"]["activity"].is_null());
            assert_eq!(retained["name"], "after");
            assert_eq!(retained["content"]["events"], prior["content"]["events"]);
        }
        let mut retained = prior.clone();
        update_retained_observation(&mut retained, &json!({"activity":["Read","","2s",10]}));
        assert_eq!(
            retained["content"]["activity"],
            prior["content"]["activity"]
        );
        assert_eq!(retained["activity"][2], "2s");
    }
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Fixture {
        source: OverviewSource,
        root: std::path::PathBuf,
    }
    impl Fixture {
        fn new(key: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-overview-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let options = Options {
                cache: root.join("index.json"),
                ..Options::default()
            };
            let db = Connection::open(EventCache::path(&options.cache)).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute_batch(crate::causal_trace::SCHEMA).unwrap();
            db.execute_batch("CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY,lifecycle TEXT NOT NULL,last_ms INTEGER NOT NULL,metadata TEXT NOT NULL);CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);INSERT INTO meta VALUES('catalog_completeness','complete');").unwrap();
            Self {
                source: OverviewSource {
                    key: key.into(),
                    label: key.into(),
                    options,
                    up: true,
                },
                root,
            }
        }
        fn put(&self, key: &str, state: &str, lifecycle: &str) {
            let mut db = Connection::open(EventCache::path(&self.source.options.cache)).unwrap();
            let tx = db.transaction().unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO session_catalog VALUES(?1,?2,1000,'{}')",
                params![key, lifecycle],
            )
            .unwrap();
            publish(&tx,key,1000,&Summary {source_generation:String::new(),activity:None,home:json!({"name":key,"harness":"codex","model":"model","repo":null,"state":state,"last":1000,"start":0,"waiting_for":null,"waiting_since":null,"cost":null,"events":[{"id":"answer","at":500,"done":1000,"ask":"question","status":"done","brief":"Answer this","answer":["Yes"],"declined":false}]})}).unwrap();
            tx.commit().unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn home(sources: &[OverviewSource], query: &str) -> Value {
        serde_json::from_slice(
            &session_overview_reply(sources, "home", query, 2000)
                .unwrap()
                .body,
        )
        .unwrap()
    }
    #[test]
    fn focused_machine_activity_never_opens_an_unrelated_source_index() {
        let selected = Fixture::new("selected/machine");
        let unrelated = Fixture::new("unrelated");
        selected.put("selected-session", "work", "current");
        std::fs::write(
            EventCache::path(&unrelated.source.options.cache),
            "unavailable unrelated index",
        )
        .unwrap();
        let sources = [
            selected.source.clone_for_test(),
            unrelated.source.clone_for_test(),
        ];
        let focused = home(&sources, "source=selected%2Fmachine");
        assert_eq!(focused["observations"], json!([]));
        assert_eq!(focused["machines"].as_array().unwrap().len(), 1);
        assert_eq!(focused["machines"][0]["key"], "selected/machine");
        assert_eq!(focused["machines"][0]["working"], 1);
        assert_eq!(focused["recent"][0]["catalog_key"], "selected-session");
        let broad = home(&sources, "");
        assert_eq!(
            broad["observations"],
            json!([{ "source_key":"unrelated", "state":"unavailable" }])
        );
        assert_eq!(
            session_overview_reply(&sources, "home", "source=missing", 2000)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        for query in [
            "source",
            "source=",
            "source=%ZZ",
            "source=selected%2Fmachine&source=unrelated",
        ] {
            assert_eq!(
                overview_source_key(query).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{query}"
            );
        }
    }

    #[test]
    fn cross_source_collisions_remain_qualified_and_retired_sessions_are_not_live() {
        let first = Fixture::new("first");
        let second = Fixture::new("second");
        first.put("same", "work", "current");
        second.put("same", "work", "retained");
        let rows = read(&first.source, 0, false).unwrap().0;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, r#"["first","same"]"#);
        assert_eq!(
            read(&second.source, 0, false).unwrap().0[0].summary.home["state"],
            "off"
        );
        let output = home(
            &[
                first.source.clone_for_test(),
                second.source.clone_for_test(),
            ],
            "",
        );
        assert_eq!(output["totals"]["working"], 1);
        assert_eq!(output["totals"]["answered"], 2);
        assert_ne!(
            output["answered"][0]["source_key"],
            output["answered"][1]["source_key"]
        );
    }
    #[test]
    fn home_pages_are_bounded_and_can_read_every_answer() {
        let fixture = Fixture::new("source");
        for n in 0..205 {
            fixture.put(&format!("s{n:03}"), "idle", "current");
        }
        let first = home(&[fixture.source.clone_for_test()], "");
        assert_eq!(first["answered"].as_array().unwrap().len(), 100);
        assert_eq!(first["totals"]["answered"], 205);
        assert_eq!(first["next"]["answered"], 100);
        let last = home(&[fixture.source.clone_for_test()], "answered_offset=200");
        assert_eq!(last["answered"].as_array().unwrap().len(), 5);
        assert!(last["next"]["answered"].is_null());
        assert!(
            session_overview_reply(
                &[fixture.source.clone_for_test()],
                "home",
                "answered_offset=1&answered_offset=2",
                2000
            )
            .is_err()
        );
    }
    #[test]
    fn pending_observation_cursor_advances_past_failed_items_and_wraps_without_loading_native_sources()
     {
        let fixture = Fixture::new("source");
        let db = Connection::open(EventCache::path(&fixture.source.options.cache)).unwrap();
        for index in 0..19 {
            db.execute(
                "INSERT INTO meta(key,value) VALUES(?1,'1')",
                [format!("overview_needed:queued-{index:02}")],
            )
            .unwrap();
        }
        let first = home(&[fixture.source.clone_for_test()], "");
        assert_eq!(first["pending_observations"].as_array().unwrap().len(), 8);
        assert_eq!(first["pending_observations"][7]["catalog_key"], "queued-07");
        // The first batch remains queued, representing unavailable native records.
        let second = home(
            &[fixture.source.clone_for_test()],
            "observation_after=[\"source\",\"queued-07\"]",
        );
        assert_eq!(
            second["pending_observations"][0]["catalog_key"],
            "queued-08"
        );
        assert_eq!(
            second["pending_observations"][7]["catalog_key"],
            "queued-15"
        );
        let third = home(
            &[fixture.source.clone_for_test()],
            "observation_after=[\"source\",\"queued-15\"]",
        );
        assert_eq!(third["pending_observations"].as_array().unwrap().len(), 3);
        let wrapped = home(
            &[fixture.source.clone_for_test()],
            "observation_after=[\"source\",\"queued-18\"]",
        );
        assert_eq!(
            wrapped["pending_observations"],
            first["pending_observations"]
        );
        assert!(
            session_overview_reply(
                &[fixture.source.clone_for_test()],
                "home",
                "observation_after=[]",
                2000
            )
            .is_err()
        );
        assert!(session_overview_reply(&[fixture.source.clone_for_test()], "home", "observation_after=[\"source\",\"queued-00\"]&observation_after=[\"source\",\"queued-01\"]", 2000).is_err());
    }
    #[test]
    fn cold_reads_are_explicit_and_do_not_create_or_restore_source_storage() {
        let fixture = Fixture::new("source");
        std::fs::remove_file(EventCache::path(&fixture.source.options.cache)).unwrap();
        let output = home(&[fixture.source.clone_for_test()], "");
        assert_eq!(output["observations"][0]["source_key"], "source");
        assert!(!EventCache::path(&fixture.source.options.cache).exists());
    }
    #[test]
    fn seen_results_are_filtered_before_counts_and_pagination_without_cross_source_aliases() {
        let first = Fixture::new("first");
        let second = Fixture::new("second");
        for fixture in [&first, &second] {
            fixture.put("same", "idle", "current");
            let db = Connection::open(EventCache::path(&fixture.source.options.cache)).unwrap();
            db.execute("UPDATE session_overview SET summary=json_set(summary,'$.home.events[0].ask','result')", []).unwrap();
        }
        let encoded = crate::viewer::percent_encode(r#"["[\"first\",\"answer\"]"]"#);
        let output = home(
            &[
                first.source.clone_for_test(),
                second.source.clone_for_test(),
            ],
            &format!("seen={encoded}"),
        );
        assert_eq!(output["totals"]["waiting"], 1);
        assert_eq!(output["inbox"][0]["source_key"], "second");
        assert_eq!(output["totals"]["answered"], 2);
        let unscoped = crate::viewer::percent_encode(r#"["answer"]"#);
        assert_eq!(
            home(
                &[
                    first.source.clone_for_test(),
                    second.source.clone_for_test()
                ],
                &format!("seen={unscoped}")
            )["totals"]["waiting"],
            2
        );
        assert_eq!(
            home(
                &[first.source.clone_for_test()],
                &format!("seen={unscoped}")
            )["totals"]["waiting"],
            1
        );
        assert!(
            session_overview_reply(
                &[first.source.clone_for_test()],
                "home",
                "seen=%5B%5D&seen=%5B%5D",
                2000
            )
            .is_err()
        );
    }
    #[test]
    fn publication_rolls_back_all_overview_values_and_deleted_members_are_excluded() {
        let fixture = Fixture::new("source");
        fixture.put("one", "work", "current");
        let before = home(&[fixture.source.clone_for_test()], "");
        let mut db = Connection::open(EventCache::path(&fixture.source.options.cache)).unwrap();
        {
            let tx = db.transaction().unwrap();
            tx.execute("UPDATE session_overview SET state='off',summary=json_set(summary,'$.home.state','off')", []).unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO meta VALUES('catalog_generation','aborted')",
                [],
            )
            .unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(home(&[fixture.source.clone_for_test()], ""), before);
        db.execute("DELETE FROM session_catalog", []).unwrap();
        assert_eq!(
            home(&[fixture.source.clone_for_test()], "")["totals"]["working"],
            0
        );
    }
    impl OverviewSource {
        fn clone_for_test(&self) -> Self {
            Self {
                key: self.key.clone(),
                label: self.label.clone(),
                options: self.options.clone(),
                up: self.up,
            }
        }
    }
}
