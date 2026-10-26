//! Focused MCP paging from the shared native slot projection and byte proofs.
//! No complete event index, semantic cohort or unrelated source is prepared.
use super::*;
use crate::{
    events::EventCache,
    model::summary::CatalogRow,
    slot_projection::{QueryObservation, SavedSlot, VERSION},
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::Digest;

const MAX_SLOTS: usize = 512;
const METADATA_BYTES: usize = 4 * 1024 * 1024;

#[cfg(test)]
thread_local! { pub(super) static SLOTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("io", error.to_string())
}

fn proof_unavailable(error: &io::Error) -> QueryError {
    if crate::source_carrier::FailureStage::of(error).is_some()
        || error
            .get_ref()
            .is_some_and(|error| error.is::<crate::source_carrier::RevisionChanged>())
    {
        QueryError::io(error)
    } else {
        QueryError::new("io", "Original native transcript proofs are unavailable")
    }
}

fn unchanged(
    db: &Connection,
    key: &str,
    history: bool,
    text: &str,
    headers: &str,
    slot_generation: &str,
) -> Result<(), QueryError> {
    let current = if history {
        crate::history_projection::admitted_catalog(db, key).map_err(unavailable)?
    } else {
        db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key=?1", [key], |row|row.get::<_,Option<String>>(0)).optional().map_err(unavailable)?.flatten()
    };
    let current_slot: Option<String> = db
        .query_row(
            &format!("SELECT generation FROM main.{headers} WHERE namespace=semon_namespace() AND session_key=?1"),
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if current.as_deref() != Some(text) || current_slot.as_deref() != Some(slot_generation) {
        return Err(unavailable(
            "Selected native transcript publication changed",
        ));
    }
    Ok(())
}

fn edge(
    db: &Connection,
    id: &str,
    context: &[CatalogRow],
    records: &crate::query_source::Records<'_>,
    bytes_left: &std::cell::RefCell<usize>,
) -> Result<Value, QueryError> {
    let mut query = None;
    let mut presence = Value::Null;
    let mut brief_constant = None;
    let mut content = serde_json::Map::new();
    for owner in context {
        let encoded:Option<String>=db.query_row("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key=?1",params![format!("{}{}",crate::query_projection::prefix("edge",&crate::query_projection::identity(owner)),id)],|r|r.get(0)).optional().map_err(unavailable)?.flatten();
        let Some(encoded) = encoded else { continue };
        let saved: Value = serde_json::from_str(&encoded).map_err(unavailable)?;
        if !saved["query"].is_object() {
            return Err(unavailable("Native handoff query recipe unavailable"));
        }
        if query.as_ref().is_some_and(|query| *query != saved["query"]) {
            return Err(unavailable("Native handoff observations disagree"));
        }
        query = Some(saved["query"].clone());
        presence = saved["query_presence"].clone();
        brief_constant = saved["query_brief_constant"].as_u64();
        for (field, recipe) in saved["query_content"].as_object().into_iter().flatten() {
            if recipe.is_null() {
                continue;
            }
            if let Some(prior) = content.insert(field.clone(), recipe.clone())
                && prior != *recipe
            {
                return Err(unavailable("Native handoff source recipes disagree"));
            }
        }
    }
    let mut query = query.ok_or_else(|| unavailable("Native handoff identity unavailable"))?;
    let read = |recipe: &Value| -> Result<Value, QueryError> {
        let path = std::path::Path::new(
            recipe["path"]
                .as_str()
                .ok_or_else(|| unavailable("Handoff source unavailable"))?,
        );
        if !context
            .iter()
            .flat_map(|row| &row.sources)
            .any(|source| source.path == path)
        {
            return Err(unavailable("Handoff source is outside qualified context"));
        }
        let offset = recipe["offset"]
            .as_u64()
            .ok_or_else(|| unavailable("Handoff source offset unavailable"))?;
        if !recipe["field"].is_null() {
            let field: crate::native_field::NativeField =
                serde_json::from_value(recipe["field"].clone()).map_err(unavailable)?;
            if field.record_offset != offset
                || Some(u64::from(field.block)) != recipe["block"].as_u64()
            {
                return Err(unavailable("Native handoff field descriptor changed"));
            }
            let source = records
                .source_ref(path)
                .ok_or_else(|| unavailable("Native handoff source reference unavailable"))?;
            let text = crate::native_field::preview(
                records,
                source,
                &field,
                recipe["kind"].as_str().unwrap_or(""),
                &mut bytes_left.borrow_mut(),
            )
            .map_err(|error| records.query_error(error))?;
            return crate::native_field::preview_record(
                &field,
                &text,
                recipe["kind"].as_str().unwrap_or(""),
            )
            .ok_or_else(|| unavailable("Native handoff field unavailable"));
        }
        let remaining = *bytes_left.borrow();
        let bytes = records
            .record_bytes(path, offset, remaining as u64)
            .map_err(|error| records.query_error(error))?
            .ok_or_else(|| unavailable("Native handoff record unavailable"))?;
        *bytes_left.borrow_mut() = remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| unavailable("Native handoff bytes exceed the request budget"))?;
        crate::tx::parse_native_record(&bytes)
            .ok_or_else(|| unavailable("Native handoff record unavailable"))
    };
    let question = if content
        .get("brief")
        .is_some_and(|recipe| recipe["kind"] == "question")
    {
        Some(read(&content["brief"])?)
    } else {
        None
    };
    for field in ["brief", "result", "answer", "answers"] {
        if presence[field] != true {
            continue;
        }
        if query["declined"] == true && matches!(field, "answer" | "answers") {
            query[field] = json!([]);
            continue;
        }
        if field == "brief"
            && !content.contains_key("brief")
            && let Some(constant) = brief_constant
        {
            query[field] = json!(match constant {
                0 => "",
                1 => "(brief not in these logs)",
                2 => "(observed child run)",
                3 => "(question)",
                _ => return Err(unavailable("Unsupported native brief constant")),
            });
            continue;
        }
        let name = if field == "answers" { "answer" } else { field };
        let mut recipe = content
            .get(name)
            .cloned()
            .ok_or_else(|| unavailable(format!("Native {field} recipe unavailable ({id})")))?;
        if field == "answers" {
            recipe["kind"] = json!("answers");
        }
        recipe["question_block"] = content
            .get("brief")
            .map(|recipe| recipe["block"].clone())
            .unwrap_or(Value::Null);
        let value = read(&recipe)?;
        query[field] = crate::model::overview_text(&recipe, &value, question.as_ref());
    }
    Ok(query)
}

// The native producer's output-window closure: timed or live edges keep their
// endpoints; retained turns keep their incoming/outgoing links even when old.
fn edge_timed(db: &Connection, id: &str, cutoff: i64) -> Result<bool, QueryError> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_trace_edges WHERE namespace=semon_namespace() AND edge_id=?1 AND (json_extract(metadata,'$.at')>=?2 OR json_extract(metadata,'$.done')>=?2 OR json_extract(metadata,'$.status') IN ('work','wait','new')))", params![id,cutoff], |r|r.get(0)).map_err(unavailable)
}
fn turn_kept(db: &Connection, turn: &Value, cutoff: i64) -> Result<bool, QueryError> {
    Ok(turn["last"] == true
        || turn["at"].as_i64().is_some_and(|at| at >= cutoff)
        || match turn["start"].as_str() {
            Some(id) => edge_timed(db, id, cutoff)?,
            None => false,
        })
}
fn edge_kept(db: &Connection, id: &str, cutoff: i64) -> Result<bool, QueryError> {
    if edge_timed(db, id, cutoff)? {
        return Ok(true);
    }
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_trace_turns t JOIN main.session_catalog c ON c.namespace=t.namespace AND c.session_key=t.session_key WHERE t.namespace=semon_namespace() AND t.session_key IN (SELECT session_key FROM main.session_trace_edges WHERE namespace=semon_namespace() AND edge_id=?1) AND (json_extract(c.metadata,'$.query.last')>=?2 OR json_extract(c.metadata,'$.query.state') IN ('work','wait') OR EXISTS(SELECT 1 FROM main.session_trace_edges live WHERE live.namespace=t.namespace AND live.session_key=t.session_key AND (json_extract(live.metadata,'$.at')>=?2 OR json_extract(live.metadata,'$.done')>=?2 OR json_extract(live.metadata,'$.status') IN ('work','wait','new')))) AND (json_extract(t.metadata,'$.start')=?1 OR EXISTS(SELECT 1 FROM json_each(t.metadata,'$.sent') WHERE value=?1)) AND (json_extract(t.metadata,'$.last')=1 OR json_extract(t.metadata,'$.at')>=?2 OR EXISTS(SELECT 1 FROM main.session_trace_edges e WHERE e.namespace=t.namespace AND e.edge_id=json_extract(t.metadata,'$.start') AND (json_extract(e.metadata,'$.at')>=?2 OR json_extract(e.metadata,'$.done')>=?2 OR json_extract(e.metadata,'$.status') IN ('work','wait','new')))))", params![id,cutoff], |r|r.get(0)).map_err(unavailable)
}
fn window_member(
    db: &Connection,
    key: &str,
    saved: &Value,
    cutoff: i64,
) -> Result<bool, QueryError> {
    if saved["last"].as_i64().is_some_and(|last| last >= cutoff)
        || matches!(saved["state"].as_str(), Some("work" | "wait"))
    {
        return Ok(true);
    }
    let mut statement = db
        .prepare("SELECT edge_id FROM main.session_trace_edges WHERE namespace=semon_namespace() AND session_key=?1 ORDER BY edge_id")
        .map_err(unavailable)?;
    for id in statement
        .query_map([key], |r| r.get::<_, String>(0))
        .map_err(unavailable)?
    {
        if edge_kept(db, &id.map_err(unavailable)?, cutoff)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn generation(db: &Connection) -> Result<String, QueryError> {
    db.query_row(
        "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
        [],
        |row| row.get(0),
    )
    .map_err(unavailable)
}

fn qualify_virtual_aliases(summary: &mut Value, context: &[CatalogRow], alias: Option<&str>) {
    let Some(alias) = alias else {
        return;
    };
    let stubs: std::collections::BTreeSet<_> = context
        .iter()
        .filter_map(|row| row.query.as_ref())
        .flat_map(|query| query["rows"].as_object().into_iter().flatten())
        .filter(|(_, value)| value["summary"]["kind"] == "stub")
        .map(|(key, _)| key.as_str())
        .collect();
    let qualify = |value: &mut Value| {
        if let Some(key) = value.as_str().filter(|key| stubs.contains(key)) {
            *value = json!(format!("{key}@{alias}"));
        }
    };
    qualify(&mut summary["id"]);
    qualify(&mut summary["parent"]);
    for child in summary["children"].as_array_mut().into_iter().flatten() {
        qualify(child);
    }
    for handoff in summary["handoffs"].as_array_mut().into_iter().flatten() {
        qualify(&mut handoff["from"]);
        qualify(&mut handoff["to"]);
    }
}

/// Virtual transcripts use recipes published by their existing physical owner.
/// Their slot addresses include the NoActivity entry and never imply custody
/// or runtime authority of the far end.
fn virtual_read(
    options: &Options,
    key: &str,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Option<Value>, QueryError> {
    let requested = args["id"].as_str().expect("validated focused id");
    let served = args["_machine_alias"]
        .as_str()
        .map_or_else(|| key.to_owned(), |alias| format!("{key}@{alias}"));
    if requested != served {
        return Err(QueryError::unknown_session(requested));
    }
    let mut db = crate::events::store::local_reader(options).map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let mut tx = db.transaction().map_err(unavailable)?;
    crate::events::store::require_current(&tx).map_err(unavailable)?;
    let context = match crate::overview_repair::query_context(&tx, key) {
        Ok(context) => context,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unavailable(error)),
    };
    let runtime = crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?;
    let catalog_generation = generation(&tx)?;
    let cutoff = (!options.all && !options.scan_window)
        .then(|| now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX)));
    let mut owner = None;
    let mut headers = Vec::new();
    let mut summary = None;
    for row in &context {
        let Some(query) = &row.query else {
            return Ok(None);
        };
        let row_runtime = crate::native_evidence::RuntimeObservations::new(options).expected(row);
        if !crate::model::summary::query_observation_matches(
            query,
            &row.sources,
            true,
            row_runtime.as_deref(),
        )
        .map_err(unavailable)?
        {
            return Ok(None);
        }
        if row.sources.iter().any(|source| {
            !crate::model::catalog_source_in_window(source, crate::model::scan_cutoff(options, now))
        }) {
            return Ok(None);
        }
        let identity = crate::query_projection::identity(row);
        let Some(header) = crate::query_projection::header(&tx, &identity).map_err(unavailable)?
        else {
            return Ok(None);
        };
        let saved: Value = serde_json::from_str(&header).map_err(unavailable)?;
        if saved["version"] != 1
            || saved["complete"] != true
            || saved["query_generation"]
                != format!(
                    "{:x}",
                    sha2::Sha256::digest(serde_json::to_vec(&row.query).map_err(unavailable)?)
                )
            || saved["source_generation"]
                != crate::retention::source_generation(&row.sources).map_err(unavailable)?
        {
            return Ok(None);
        }
        headers.push((identity.clone(), header));
        let encoded: Option<String> = tx.query_row("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key=?1",[format!("{}{}",crate::query_projection::prefix("virtual",&identity),key)],|r|r.get(0)).optional().map_err(unavailable)?.flatten();
        if let Some(encoded) = encoded {
            // A shared virtual transcript is an aggregate, not the first
            // physical owner's recipe vector. Retain the native aggregate
            // resolver until its generation-bound ordered projection exists.
            if owner.is_some() {
                return Ok(None);
            }
            let header: Value = serde_json::from_str(&encoded).map_err(unavailable)?;
            if owner.is_none() {
                owner = Some((
                    identity,
                    header["total"]
                        .as_u64()
                        .ok_or_else(|| unavailable("Virtual transcript total unavailable"))?
                        as usize,
                ));
            }
        }
        if let Some(value) = query["rows"].get(key) {
            summary = Some(value["summary"].clone());
        }
    }
    let Some((identity, total)) = owner else {
        return Ok(None);
    };
    if cutoff.is_some() {
        // A clipped virtual transcript needs the producer's filtered slot ranks.
        // Preserve its existing resolver until that indexed publication exists.
        return Ok(None);
    }
    let session = args["_session"] == true;
    let mut sources = std::collections::BTreeMap::new();
    for source in context.iter().flat_map(|row| &row.sources) {
        if sources
            .insert(source.path.clone(), source.clone())
            .is_some_and(|prior| prior != *source)
        {
            return Err(unavailable("Virtual source generations disagree"));
        }
    }
    // Transfer the selection's WAL snapshot to the lazy byte-proof readers.
    // Later metadata reads retain their existing publication rechecks.
    tx.set_drop_behavior(rusqlite::DropBehavior::Ignore);
    drop(tx);
    let sources = crate::query_source::Sources::capture_focused_snapshot(
        options,
        &sources.into_values().collect::<Vec<_>>(),
        db,
    )
    .map_err(|error| proof_unavailable(&error))?;
    let mut db = crate::events::store::local_reader(options).map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let records = sources.reader_with_mode(provider, mode);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::run(options, "query_transcript_before_read");
    let prefix = format!(
        "{}{}:{key}:",
        crate::query_projection::prefix("virtual-slot", &identity),
        key.len()
    );
    let load = |db: &Connection,
                low: usize,
                high: usize|
     -> Result<Vec<crate::model::Slot>, QueryError> {
        let mut slots = Vec::new();
        if low == 0 && high > 0 {
            slots.push(crate::model::Slot::new(
                crate::model::SlotKind::NoActivity,
                None,
                0,
                0,
                None,
            ));
        }
        let mut statement = db
                .prepare("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2 ORDER BY key")
                .map_err(unavailable)?;
        let mut metadata_bytes = 0usize;
        for encoded in statement
            .query_map(
                params![
                    format!("{prefix}{:020}", low.max(1)),
                    format!("{prefix}{high:020}")
                ],
                |r| r.get::<_, String>(0),
            )
            .map_err(unavailable)?
        {
            let encoded = encoded.map_err(unavailable)?;
            metadata_bytes += encoded.len();
            if metadata_bytes > METADATA_BYTES {
                return Err(unavailable(
                    "Virtual transcript metadata exceeds its read budget",
                ));
            }
            let value: Value = serde_json::from_str(&encoded).map_err(unavailable)?;
            slots.push(crate::model::Slot::new(
                crate::model::SlotKind::H(
                    value["id"]
                        .as_str()
                        .ok_or_else(|| unavailable("Virtual handoff unavailable"))?
                        .into(),
                ),
                None,
                0,
                0,
                value["at"].as_i64(),
            ));
        }
        if slots.len() != high - low {
            return Err(unavailable("Virtual slots are incomplete"));
        }
        #[cfg(test)]
        SLOTS.with(|n| n.set(n.get() + slots.len()));
        Ok(slots)
    };
    let answer = if session {
        let Some(mut summary) = summary else {
            return Err(QueryError::unknown_session(key));
        };
        summary["id"] = args["id"].clone();
        summary["busy"] = json!([]);
        summary["turns"] = json!([]);
        let mut handoffs = Vec::new();
        let bytes_left = std::cell::RefCell::new(64 * 1024 * 1024);
        let mut metadata_bytes = summary.to_string().len();
        for low in (1..total).step_by(64) {
            for slot in load(&db, low, (low + 64).min(total))? {
                let crate::model::SlotKind::H(id) = slot.kind else {
                    unreachable!()
                };
                let mut value = edge(&db, &id, &context, &records, &bytes_left)?;
                for endpoint in ["from", "to"] {
                    if value[endpoint] == key {
                        value[endpoint] = args["id"].clone();
                    }
                }
                metadata_bytes += value.to_string().len();
                if metadata_bytes > 8 * 1024 * 1024 {
                    return Err(unavailable(
                        "Session result metadata exceeds its request budget",
                    ));
                }
                handoffs.push(value);
            }
        }
        summary["handoffs"] = json!(handoffs);
        qualify_virtual_aliases(&mut summary, &context, args["_machine_alias"].as_str());
        json!({"now":now,"session":summary})
    } else {
        let position = |name: &str| integer(args, name).map(|value| value.max(0) as usize);
        let anchor = tx::Anchor::of(position("before"), position("after"), string(args, "turn"))
            .ok_or_else(|| QueryError::invalid("before, after and turn are exclusive"))?;
        let (forward, from) = match anchor {
            tx::Anchor::Last => (false, total),
            tx::Anchor::Before(before) => (false, before.min(total)),
            tx::Anchor::After(after) => (true, after.min(total)),
            tx::Anchor::Turn(turn) => {
                return Err(QueryError::new(
                    "unknown_turn",
                    format!("session {requested:?} has no turn {turn:?}"),
                ));
            }
        };
        let limit = integer(args, "limit").map_or(tx::PAGE_ENTRIES, |value| value as usize);
        let count = limit.saturating_add(1).clamp(2, MAX_SLOTS);
        let (low, high) = if forward {
            (
                from.saturating_sub(1),
                from.saturating_add(count).min(total),
            )
        } else {
            (from.saturating_sub(count).saturating_sub(1), from)
        };
        let slots = load(&db, low, high)?;
        let mut read = |path: &std::path::Path, offset| records.read(path, offset);
        tx::page_from_window(
            &tx::RenderContext {
                home: None,
                harness: None,
                bounded: false,
            },
            tx::PageWindow {
                sid: key,
                slots: &slots,
                start: low,
                total,
                from,
                forward,
                calls: 0,
                errors: 0,
                bg_running: &[],
            },
            now,
            limit,
            &mut tx::Lines::provider(&mut read),
        )
        .map_err(|error| records.query_error(error))?
    };
    records
        .finish()
        .map_err(|error| records.query_error(error))?;
    let snapshot = db.transaction().map_err(unavailable)?;
    if generation(&snapshot)? != catalog_generation
        || serde_json::to_value(
            crate::overview_repair::query_context(&snapshot, key).map_err(unavailable)?,
        )
        .map_err(unavailable)?
            != serde_json::to_value(&context).map_err(unavailable)?
    {
        return Err(unavailable("Virtual publication changed"));
    }
    for (identity, header) in headers {
        if crate::query_projection::header(&snapshot, &identity)
            .map_err(unavailable)?
            .as_deref()
            != Some(&header)
        {
            return Err(unavailable("Virtual query recipes changed"));
        }
    }
    if crate::native_evidence::query_runtime_generation(options).map_err(unavailable)? != runtime {
        return Err(unavailable("Virtual observation changed"));
    }
    Ok(Some(answer))
}

/// None identifies evidence not yet owned by this projection (an omitted-source
/// window, composite history or an observation awaiting its native producer). Once admitted,
/// corrupted or changed evidence is an explicit error, never a body fallback.
pub(super) fn read(
    options: &Options,
    key: &str,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Option<Value>, QueryError> {
    read_inner(options, key, args, provider, mode, now, None)
}

pub(super) fn session(
    options: &Options,
    key: &str,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Option<Value>, QueryError> {
    let mut args = args.clone();
    args["_session"] = Value::Bool(true);
    read_inner(options, key, &args, provider, mode, now, None)
}

pub(super) fn search(
    options: &Options,
    key: &str,
    served: &str,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
    search: &mut super::search::Search,
) -> Result<bool, QueryError> {
    Ok(read_inner(
        options,
        key,
        &json!({"id":key}),
        provider,
        mode,
        now,
        Some((served, search)),
    )?
    .is_some())
}

#[allow(clippy::too_many_arguments)]
fn read_inner(
    options: &Options,
    key: &str,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
    search: Option<(&str, &mut super::search::Search)>,
) -> Result<Option<Value>, QueryError> {
    #[cfg(test)]
    SLOTS.with(|slots| slots.set(0));
    // Raw aliases can change under a physical file window. Only exact native
    // Catalog identities enter this pager; their context is qualified below.
    let requested = args["id"].as_str().expect("validated focused id");
    let suffixed = requested != key
        && requested
            .rsplit_once('@')
            .is_some_and(|(owner, _)| owner == key);
    if requested != key && !suffixed {
        return Ok(None);
    }
    let mut db = crate::events::store::local_reader(options).map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let mut tx = db.transaction().map_err(unavailable)?;
    crate::events::store::require_current(&tx).map_err(unavailable)?;
    let catalog_generation = generation(&tx)?;
    let projection_generation: Option<String> = tx
        .query_row(
            "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='slot_projection_catalog_generation'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if projection_generation.as_deref() != Some(&catalog_generation) {
        return Ok(None);
    }
    let history = crate::history_projection::admitted_catalog(&tx, key).map_err(unavailable)?;
    let text = match history.as_ref() {
        Some(text) => Some(text.clone()),
        None => tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key=?1", [key], |row|row.get::<_,Option<String>>(0))
            .optional().map_err(unavailable)?.flatten(),
    };
    let Some(text) = text else {
        tx.commit().map_err(unavailable)?;
        return virtual_read(options, key, args, provider, mode, now);
    };
    let row: CatalogRow = serde_json::from_str(&text).map_err(unavailable)?;
    if row.sources.iter().any(|source| {
        !crate::model::catalog_source_in_window(source, crate::model::scan_cutoff(options, now))
    }) {
        return Ok(None);
    }
    let Some(saved) = &row.query else {
        return Ok(None);
    };
    // Recorded native IDs are global. Machine suffixes belong only to virtual
    // owners, which have no Catalog row and retain their indexed owner resolver.
    // Reject this known mismatch before loading a complete selected cohort.
    if suffixed {
        return Err(QueryError::unknown_session(requested));
    }
    // A finite file window can omit an identity collision, parent or handoff
    // source. Reuse the existing bounded native closure, in this publication
    // snapshot, before admitting its complete Catalog recipes. Omitted, stale
    // or provisional context keeps the native window resolver.
    if options.scan_window
        && !options.all
        && !crate::overview_repair::query_window_context_current(&tx, options, key, now)
            .map_err(unavailable)?
    {
        return Ok(None);
    }
    let cutoff = (!options.all && !options.scan_window)
        .then(|| now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX)));
    if let Some(cutoff) = cutoff
        && !window_member(&tx, key, saved, cutoff)?
    {
        return Err(QueryError::unknown_session(key));
    }
    if saved["transcript_observation"].is_null() {
        return Ok(None);
    }
    let runtime = crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?;
    let row_runtime = crate::native_evidence::RuntimeObservations::new(options).expected(&row);
    // Retention preserves byte custody, not a live process observation. Its
    // pending tools and backgrounds need the same runtime qualification.
    if !crate::model::summary::query_observation_matches(
        saved,
        &row.sources,
        true,
        row_runtime.as_deref(),
    )
    .map_err(unavailable)?
    {
        return Ok(None);
    }
    let observation: QueryObservation =
        serde_json::from_value(saved["transcript_observation"].clone()).map_err(unavailable)?;
    let (headers, table) = if history.is_some() {
        ("session_history_projections", "session_history_slots")
    } else {
        ("session_slot_projections", "session_slots")
    };
    let header: Option<(String,u32,i64,Option<String>)> = tx.query_row(&format!(
        "SELECT generation,version,total,source_generation FROM main.{headers} WHERE namespace=semon_namespace() AND session_key=?1"
    ), [key], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(unavailable)?;
    let Some((slot_generation, version, total, source_generation)) = header else {
        return Ok(None);
    };
    let total = usize::try_from(total).map_err(unavailable)?;
    if !observation.valid(total) {
        return Err(unavailable("Native transcript observation is invalid"));
    }
    if version != VERSION {
        let mut error = unavailable("Native transcript projection upgrade required");
        error.stage = Some("header-version");
        return Err(error);
    }
    if source_generation.as_deref()
        != Some(&crate::retention::source_generation(&row.sources).map_err(unavailable)?)
    {
        return Err(unavailable("Native transcript source generation changed"));
    }
    let position = |name: &str| integer(args, name).map(|value| value.max(0) as usize);
    let anchor = tx::Anchor::of(position("before"), position("after"), string(args, "turn"))
        .ok_or_else(|| QueryError::invalid("before, after and turn are exclusive"))?;
    let (forward, from) = match &anchor {
        tx::Anchor::Last => (false, total),
        tx::Anchor::Before(before) => (false, (*before).min(total)),
        tx::Anchor::After(after) => (true, (*after).min(total)),
        tx::Anchor::Turn(turn) => {
            let turn_column = if history.is_some() {
                "json_extract(metadata,'$.turn')"
            } else {
                "CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.turn') END"
            };
            let first: Option<i64> = tx.query_row(&format!(
                "SELECT slot FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND {turn_column}=?2 AND json_extract(metadata,'$.first')=1 ORDER BY slot LIMIT 1"
            ), params![key,turn], |row|row.get(0)).optional().map_err(unavailable)?;
            (
                true,
                usize::try_from(first.ok_or_else(|| {
                    QueryError::new(
                        "unknown_turn",
                        format!("session {key:?} has no turn {turn:?}"),
                    )
                })?)
                .map_err(unavailable)?,
            )
        }
    };
    let limit = integer(args, "limit").map_or(tx::PAGE_ENTRIES, |value| value as usize);
    // Read proof pages and root witnesses only for touched byte ranges. Each
    // request-owned source retains at most one decoded hash page.
    let session = args["_session"] == true;
    let context = if search.is_some() || session {
        crate::overview_repair::query_context(&tx, key).map_err(unavailable)?
    } else {
        vec![row.clone()]
    };
    let mut query_headers = Vec::new();
    if search.is_some() || session {
        for owner in &context {
            let Some(encoded) =
                crate::query_projection::header(&tx, &crate::query_projection::identity(owner))
                    .map_err(unavailable)?
            else {
                return Ok(None);
            };
            let header: Value = serde_json::from_str(&encoded).map_err(unavailable)?;
            if header["version"] != 1
                || header["complete"] != true
                || header["source_generation"]
                    != crate::retention::source_generation(&owner.sources).map_err(unavailable)?
                || header["query_generation"]
                    != format!(
                        "{:x}",
                        sha2::Sha256::digest(
                            serde_json::to_vec(&owner.query).map_err(unavailable)?
                        )
                    )
            {
                return Ok(None);
            }
            query_headers.push((crate::query_projection::identity(owner), encoded));
        }
    }
    let mut selected_sources = std::collections::BTreeMap::new();
    for source in context.iter().flat_map(|row| &row.sources) {
        if let Some(prior) = selected_sources.insert(source.path.clone(), source.clone())
            && prior != *source
        {
            return Err(unavailable("Selected source generations disagree"));
        }
    }
    let selected_sources: Vec<_> = selected_sources.into_values().collect();
    let incomplete: bool = if history.is_some() {
        tx.query_row(
            "SELECT incomplete FROM main.session_history_projections WHERE namespace=semon_namespace() AND session_key=?1",
            [key],
            |row| row.get(0),
        )
        .map_err(unavailable)?
    } else {
        false
    };
    if incomplete {
        return Ok(None);
    }
    tx.set_drop_behavior(rusqlite::DropBehavior::Ignore);
    drop(tx);
    let sources =
        crate::query_source::Sources::capture_focused_snapshot(options, &selected_sources, db)
            .map_err(|error| proof_unavailable(&error))?;
    let mut db = crate::events::store::local_reader(options).map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let records = sources.reader_with_mode(provider, mode);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::run(options, "query_transcript_before_read");
    let bytes_left = std::cell::RefCell::new(64 * 1024 * 1024usize);
    if session {
        let session_key =
            crate::query_projection::prefix("session", &crate::query_projection::identity(&row));
        let encoded:Option<String>=db.query_row("SELECT CASE WHEN octet_length(value)<=1048576 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key=?1",[session_key],|r|r.get(0)).optional().map_err(unavailable)?.flatten();
        let Some(encoded) = encoded else {
            return Ok(None);
        };
        let session_metadata: Value = serde_json::from_str(&encoded).map_err(unavailable)?;
        let mut summary = saved["rows"][key]["summary"].clone();
        if !summary.is_object() || !session_metadata["busy"].is_array() {
            return Ok(None);
        }
        summary["busy"] = session_metadata["busy"].clone();
        if !options.all && !options.scan_window {
            let cutoff =
                now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX));
            if let Some(intervals) = summary["busy"].as_array_mut() {
                intervals.retain(|interval| interval[1].as_i64().is_some_and(|end| end >= cutoff));
                if let Some(first) = intervals.first_mut() {
                    first[0] = json!(first[0].as_i64().unwrap_or(cutoff).max(cutoff));
                }
            }
        }
        let own_identity = crate::query_projection::identity(&row);
        let own_header: Value = serde_json::from_str(
            &query_headers
                .iter()
                .find(|(identity, _)| identity == &own_identity)
                .ok_or_else(|| unavailable("Selected query header unavailable"))?
                .1,
        )
        .map_err(unavailable)?;
        let mut turns = Vec::new();
        let mut after = -1i64;
        let mut metadata_bytes = encoded.len() + summary.to_string().len();
        loop {
            let snapshot = db.transaction().map_err(unavailable)?;
            unchanged(
                &snapshot,
                key,
                history.is_some(),
                &text,
                headers,
                &slot_generation,
            )?;
            let mut statement=snapshot.prepare("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key>?1 AND key<?2 ORDER BY key LIMIT 64").map_err(unavailable)?;
            let texts = statement
                .query_map(
                    params![
                        format!(
                            "{}{:020}",
                            crate::query_projection::prefix(
                                "turn",
                                &crate::query_projection::identity(&row)
                            ),
                            after
                        ),
                        format!(
                            "{}~",
                            crate::query_projection::prefix(
                                "turn",
                                &crate::query_projection::identity(&row)
                            )
                        )
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(unavailable)?;
            drop(statement);
            let mut recipes = Vec::new();
            for encoded in &texts {
                metadata_bytes += encoded.len();
                if metadata_bytes > 8 * 1024 * 1024 {
                    return Err(unavailable(
                        "Session result metadata exceeds its request budget",
                    ));
                }
                let turn: Value = serde_json::from_str(encoded).map_err(unavailable)?;
                after = turn["query_order"]
                    .as_i64()
                    .ok_or_else(|| unavailable("Query turn order unavailable"))?;
                if let Some(cutoff) = cutoff
                    && !turn_kept(&snapshot, &turn["query"], cutoff)?
                {
                    continue;
                }
                let recipe = if turn["query_text"] == true {
                    let encoded:String=snapshot.query_row(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND json_extract(metadata,'$.turn')=?2 AND json_extract(metadata,'$.first')=1 ORDER BY slot LIMIT 1"),params![key,turn["id"].as_str()],|row|row.get(0)).map_err(unavailable)?;
                    Some(serde_json::from_str::<SavedSlot>(&encoded).map_err(unavailable)?)
                } else {
                    None
                };
                recipes.push((turn["query"].clone(), recipe));
            }
            snapshot.commit().map_err(unavailable)?;
            for (mut turn, recipe) in recipes {
                if let Some(recipe) = recipe {
                    #[cfg(test)]
                    SLOTS.with(|slots| slots.set(slots.get() + 1));
                    let slot = recipe.restore();
                    let file = slot
                        .file
                        .as_ref()
                        .ok_or_else(|| unavailable("Turn prompt source unavailable"))?;
                    let (record, read_bytes) = records
                        .prompt(file, slot.offset)
                        .ok_or_else(|| records.query_error("Turn prompt unavailable"))?;
                    let remaining = *bytes_left.borrow();
                    *bytes_left.borrow_mut() =
                        remaining.checked_sub(read_bytes as usize).ok_or_else(|| {
                            unavailable("Session native bytes exceed the request budget")
                        })?;
                    let prompt = crate::model::prompt_text(&record)
                        .ok_or_else(|| unavailable("Turn prompt text unavailable"))?;
                    turn["text"] = json!(crate::model::cap(
                        &crate::model::cap(&prompt, crate::model::TEXT_MAX),
                        crate::model::MSG_MAX
                    ));
                }
                turns.push(turn);
            }
            if texts.len() < 64 {
                break;
            }
        }
        if cutoff.is_none() && own_header["turns"].as_u64() != Some(turns.len() as u64) {
            return Err(unavailable("Selected query turns are incomplete"));
        }
        summary["turns"] = json!(turns);
        let mut handoffs = Vec::new();
        let mut after = String::new();
        loop {
            let edge_prefix = crate::query_projection::prefix("edge", &own_identity);
            let mut statement = db
                .prepare(
                    "SELECT substr(key,?3) FROM main.meta WHERE namespace=semon_namespace() AND key>?1 AND key<?2 ORDER BY key LIMIT 64",
                )
                .map_err(unavailable)?;
            let ids = statement
                .query_map(
                    params![
                        format!("{edge_prefix}{after}"),
                        format!("{edge_prefix}~"),
                        edge_prefix.chars().count() as i64 + 1
                    ],
                    |r| r.get::<_, String>(0),
                )
                .map_err(unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(unavailable)?;
            drop(statement);
            for id in &ids {
                after = id.clone();
                if let Some(cutoff) = cutoff
                    && !edge_kept(&db, id, cutoff)?
                {
                    continue;
                }
                let handoff = edge(&db, id, &context, &records, &bytes_left)?;
                metadata_bytes += handoff.to_string().len();
                if metadata_bytes > 8 * 1024 * 1024 {
                    return Err(unavailable(
                        "Session result metadata exceeds its request budget",
                    ));
                }
                if summary["open_question"]["handoff"] == *id {
                    summary["open_question"]["text"] = handoff["brief"].clone();
                }
                handoffs.push(handoff);
                after = id.clone();
            }
            if ids.len() < 64 {
                break;
            }
        }
        if cutoff.is_none() && own_header["edges"].as_u64() != Some(handoffs.len() as u64) {
            return Err(unavailable("Selected query handoffs are incomplete"));
        }
        handoffs.sort_by(|a, b| {
            a["at"]
                .as_i64()
                .cmp(&b["at"].as_i64())
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        summary["handoffs"] = json!(handoffs);
        if let Some(cutoff) = cutoff
            && let Some(children) = summary["children"].as_array_mut()
        {
            let mut kept = Vec::new();
            for child in children.iter() {
                let id = child
                    .as_str()
                    .ok_or_else(|| unavailable("Child identity unavailable"))?;
                if let Some(owner) = context.iter().find(|row| row.key == id)
                    && let Some(saved) = &owner.query
                    && window_member(&db, id, saved, cutoff)?
                {
                    kept.push(child.clone());
                }
            }
            *children = kept;
        }
        records
            .finish()
            .map_err(|error| records.query_error(error))?;
        let snapshot = db.transaction().map_err(unavailable)?;
        unchanged(
            &snapshot,
            key,
            history.is_some(),
            &text,
            headers,
            &slot_generation,
        )?;
        if crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?
            != runtime
        {
            return Err(unavailable(
                "Native session observation changed during this read",
            ));
        }
        for (owner, expected) in &query_headers {
            if crate::query_projection::header(&snapshot, owner)
                .map_err(unavailable)?
                .as_deref()
                != Some(expected)
            {
                return Err(unavailable("Native query recipes changed"));
            }
        }
        let current = crate::overview_repair::query_context(&snapshot, key).map_err(unavailable)?;
        if serde_json::to_value(&current).map_err(unavailable)?
            != serde_json::to_value(&context).map_err(unavailable)?
        {
            return Err(unavailable("Native query relationship publication changed"));
        }
        qualify_virtual_aliases(&mut summary, &context, args["_machine_alias"].as_str());
        return Ok(Some(json!({"now":now,"session":summary})));
    }
    if let Some((served, search)) = search {
        let data = crate::model::QueryData::identity(options, now);
        let mut high = total;
        while high > 0 && !search.stopped() {
            if search.at_capacity() {
                let eligible:bool=db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND slot<?2 AND (?3 IS NULL OR json_extract(metadata,'$.t')>=?3))"),params![key,high as i64,search.since()],|r|r.get(0)).map_err(unavailable)?;
                if eligible {
                    search.stop();
                }
                break;
            }
            let low = high.saturating_sub(64);
            let snapshot = db.transaction().map_err(unavailable)?;
            unchanged(
                &snapshot,
                key,
                history.is_some(),
                &text,
                headers,
                &slot_generation,
            )?;
            let mut statement = snapshot.prepare(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND slot>=?2 AND slot<?3 ORDER BY slot")).map_err(unavailable)?;
            let recipes = statement
                .query_map(params![key, low as i64, high as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(unavailable)?
                .map(|text| {
                    serde_json::from_str::<SavedSlot>(&text.map_err(unavailable)?)
                        .map_err(unavailable)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if recipes.len() != high - low {
                return Err(unavailable("Native search slots are incomplete"));
            }
            drop(statement);
            snapshot.commit().map_err(unavailable)?;
            #[cfg(test)]
            SLOTS.with(|slots| slots.set(slots.get() + recipes.len()));
            let transcript = crate::model::Transcript {
                slots: recipes
                    .iter()
                    .enumerate()
                    .map(|(index, saved)| {
                        let mut slot = saved.restore();
                        observation.apply(saved, &mut slot, low + index);
                        slot
                    })
                    .collect(),
                ..Default::default()
            };
            for (index, slot) in transcript.slots.into_iter().enumerate().rev() {
                if !search.eligible(slot.t) {
                    continue;
                }
                if search.at_capacity() {
                    search.stop();
                    break;
                }
                let handoffs = if let crate::model::SlotKind::H(id) = &slot.kind {
                    if let Some(cutoff) = cutoff
                        && !edge_kept(&db, id, cutoff)?
                    {
                        vec![]
                    } else {
                        vec![edge(&db, id, &context, &records, &bytes_left)?]
                    }
                } else {
                    vec![]
                };
                let transcript = crate::model::Transcript {
                    slots: vec![slot],
                    ..Default::default()
                };
                search.scan_range(served, &transcript, low + index, &data, &handoffs, &records)?;
            }
            high = low;
        }
        records
            .finish()
            .map_err(|error| records.query_error(error))?;
        let snapshot = db.transaction().map_err(unavailable)?;
        unchanged(
            &snapshot,
            key,
            history.is_some(),
            &text,
            headers,
            &slot_generation,
        )?;
        for (owner, expected) in &query_headers {
            if crate::query_projection::header(&snapshot, owner)
                .map_err(unavailable)?
                .as_deref()
                != Some(expected)
            {
                return Err(unavailable("Native query recipes changed"));
            }
        }
        let current = crate::overview_repair::query_context(&snapshot, key).map_err(unavailable)?;
        if serde_json::to_value(&current).map_err(unavailable)?
            != serde_json::to_value(&context).map_err(unavailable)?
        {
            return Err(unavailable("Native query relationship publication changed"));
        }
        snapshot.commit().map_err(unavailable)?;
        if crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?
            != runtime
        {
            return Err(unavailable(
                "Native search observation changed during this read",
            ));
        }
        return Ok(Some(Value::Null));
    }
    // Every requested block verifies its original indexed proof. Finish fences
    // only touched sources; unrelated fragments of this session need no body I/O.
    let mut count = limit.saturating_add(1).clamp(2, MAX_SLOTS);
    let home = crate::facts::MachineFacts::of(options).home();
    let bg_running: Vec<_> = observation
        .backgrounds
        .iter()
        .map(|(_, id)| id.clone())
        .collect();
    let answer = loop {
        let (low, high) = if forward {
            (
                from.saturating_sub(1),
                from.saturating_add(count).min(total),
            )
        } else {
            (from.saturating_sub(count).saturating_sub(1), from)
        };
        let snapshot = db.transaction().map_err(unavailable)?;
        unchanged(
            &snapshot,
            key,
            history.is_some(),
            &text,
            headers,
            &slot_generation,
        )?;
        let mut statement = snapshot.prepare(&format!(
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND slot>=?2 AND slot<?3 ORDER BY slot"
        )).map_err(unavailable)?;
        let mut bytes = 0;
        let mut recipes = Vec::new();
        for text in statement
            .query_map(
                params![
                    key,
                    i64::try_from(low).map_err(unavailable)?,
                    i64::try_from(high).map_err(unavailable)?
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(unavailable)?
        {
            let text = text.map_err(unavailable)?;
            bytes += text.len();
            if bytes > METADATA_BYTES {
                return Err(unavailable(
                    "Native transcript metadata exceeds its read budget",
                ));
            }
            recipes.push(serde_json::from_str::<SavedSlot>(&text).map_err(unavailable)?);
        }
        drop(statement);
        snapshot.commit().map_err(unavailable)?;
        if recipes.len() != high - low {
            return Err(unavailable("Native transcript slots are incomplete"));
        }
        #[cfg(test)]
        SLOTS.with(|slots| slots.set(slots.get() + recipes.len()));
        let slots: Vec<_> = recipes
            .iter()
            .enumerate()
            .map(|(index, saved)| {
                let mut slot = saved.restore();
                observation.apply(saved, &mut slot, low + index);
                slot
            })
            .collect();
        let mut read = |path: &std::path::Path, offset| records.read(path, offset);
        let mut prompts = |file: &crate::model::SlotFile, offset| records.prompt(file, offset);
        let page = tx::page_from_window(
            &tx::RenderContext {
                home: home.as_deref(),
                harness: Some(&row.harness),
                bounded: false,
            },
            tx::PageWindow {
                sid: key,
                slots: &slots,
                start: low,
                total,
                from,
                forward,
                calls: observation.calls,
                errors: observation.errors,
                bg_running: &bg_running,
            },
            now,
            limit,
            &mut tx::Lines::provider_with_prompts(&mut read, &mut prompts),
        );
        match page {
            Ok(page) => break page,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && count < MAX_SLOTS => {
                count = (count * 2).min(MAX_SLOTS)
            }
            Err(error) => return Err(records.query_error(error)),
        }
    };
    records
        .finish()
        .map_err(|error| records.query_error(error))?;
    let snapshot = db.transaction().map_err(unavailable)?;
    unchanged(
        &snapshot,
        key,
        history.is_some(),
        &text,
        headers,
        &slot_generation,
    )?;
    snapshot.commit().map_err(unavailable)?;
    if crate::native_evidence::query_runtime_generation(options).map_err(unavailable)? != runtime {
        return Err(unavailable(
            "Native transcript observation changed during this read",
        ));
    }
    Ok(Some(answer))
}

#[cfg(test)]
mod proof_error_tests {
    use super::*;

    #[test]
    fn unstaged_proof_errors_are_generic_and_staged_errors_keep_the_stage() {
        let untyped = proof_unavailable(&io::Error::other("private sqlite detail"));
        assert_eq!(untyped.code, "io");
        assert_eq!(
            untyped.message,
            "Original native transcript proofs are unavailable"
        );
        assert!(!untyped.message.contains("private sqlite detail"));

        let staged_error = crate::source_carrier::FailureStage::HeaderLedger
            .wrap(io::Error::other("private ledger detail"));
        assert_eq!(staged_error.kind(), io::ErrorKind::Other);
        let staged = proof_unavailable(&staged_error);
        assert_eq!(staged.code, "io");
        assert_eq!(staged.stage, Some("header_ledger"));
        assert_eq!(
            staged.message,
            "Original native transcript proofs are unavailable"
        );
    }

    #[test]
    fn unstaged_revision_error_preserves_the_typed_source_fence() {
        let error = crate::source_carrier::RevisionReason::SizeGrew.error();
        let mapped = proof_unavailable(&error);
        assert_eq!(mapped.code, "stale_projection");
        assert_eq!(mapped.local_revision_reason, Some("size_grew"));
        assert_eq!(mapped.stage, None);
    }
}
