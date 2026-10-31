//! Focused MCP paging from the shared native slot projection and byte proofs.
//! No complete event index, semantic cohort or unrelated source is prepared.
use super::*;
use crate::{
    model::summary::CatalogRow,
    slot_projection::{QueryObservation, SavedSlot, VERSION},
    storage::catalog::CatalogSnapshot,
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
        crate::history_projection::catalog(db, key).map_err(unavailable)?
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
    let mut recipes = Vec::new();
    for owner in context {
        let encoded:Option<String>=db.query_row("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key=?1",params![format!("{}{}",crate::query_projection::prefix("edge",&crate::query_projection::identity(owner)),id)],|r|r.get(0)).optional().map_err(unavailable)?.flatten();
        if let Some(encoded) = encoded {
            recipes.push(serde_json::from_str(&encoded).map_err(unavailable)?);
        }
    }
    render_edge(id, context, records, &recipes, bytes_left)
}

pub(crate) trait QueryRecords {
    fn reader(&self) -> &dyn crate::SessionSourceReader;
    fn source_ref(&self, path: &std::path::Path) -> Option<&crate::SessionSourceRef>;
    fn record_bytes(
        &self,
        path: &std::path::Path,
        offset: u64,
        max: u64,
    ) -> io::Result<Option<Vec<u8>>>;
    fn prompt(&self, file: &crate::model::SlotFile, offset: u64) -> Option<(Value, u64)>;
    fn error(&self, error: io::Error) -> QueryError {
        QueryError::io(&error)
    }
}
impl QueryRecords for crate::query_source::Records<'_> {
    fn reader(&self) -> &dyn crate::SessionSourceReader {
        self
    }
    fn source_ref(&self, path: &std::path::Path) -> Option<&crate::SessionSourceRef> {
        self.source_ref(path)
    }
    fn record_bytes(
        &self,
        path: &std::path::Path,
        offset: u64,
        max: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        self.record_bytes(path, offset, max)
    }
    fn prompt(&self, file: &crate::model::SlotFile, offset: u64) -> Option<(Value, u64)> {
        self.prompt(file, offset)
    }
    fn error(&self, error: io::Error) -> QueryError {
        self.query_error(error)
    }
}
fn render_edge(
    id: &str,
    context: &[CatalogRow],
    records: &dyn QueryRecords,
    recipes: &[Value],
    bytes_left: &std::cell::RefCell<usize>,
) -> Result<Value, QueryError> {
    let mut query = None;
    let mut presence = Value::Null;
    let mut brief_constant = None;
    let mut content = serde_json::Map::new();
    for saved in recipes {
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
        let expected: crate::model::summary::CatalogSource =
            serde_json::from_value(recipe["source"].clone())
                .map_err(|_| unavailable("Handoff source generation unavailable"))?;
        if expected.path != path
            || !context
                .iter()
                .flat_map(|row| &row.sources)
                .any(|source| *source == expected)
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
                records.reader(),
                source,
                &field,
                recipe["kind"].as_str().unwrap_or(""),
                &mut bytes_left.borrow_mut(),
            )
            .map_err(|error| records.error(error))?;
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
            .map_err(|error| records.error(error))?
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
            .ok_or_else(|| {
                #[cfg(test)]
                eprintln!(
                    "native_query_recipe_missing field={field} observations={} brief_constant={brief_constant:?} content_brief={} content_result={} content_answer={}",
                    recipes.len(), content.contains_key("brief"),
                    content.contains_key("result"), content.contains_key("answer")
                );
                unavailable(format!("Native {field} recipe unavailable ({id})"))
            })?;
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
    scope: crate::CatalogReadScope,
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
    let binding = crate::storage::local_navigation_binding(options, "").map_err(unavailable)?;
    let mut db = binding.connection().map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let mut tx = db.transaction().map_err(unavailable)?;
    crate::events::store::require_current(&tx).map_err(unavailable)?;
    let context = match crate::overview_repair::query_context(&tx, key, scope) {
        Ok(context) => context,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unavailable(error)),
    };
    let runtime_snapshot =
        crate::storage::sqlite::catalog::admitted_snapshot(&binding, &tx).map_err(unavailable)?;
    let runtime = runtime_snapshot
        .runtime_generation(options)
        .map_err(unavailable)?;
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
        let observation_owner = if scope == crate::CatalogReadScope::RetainedHistory
            && crate::history_projection::catalog(&tx, &row.key)
                .map_err(unavailable)?
                .is_some()
        {
            crate::storage::catalog::CatalogObservationOwner::History
        } else {
            crate::storage::catalog::CatalogObservationOwner::Catalog
        };
        let row_runtime = runtime_snapshot
            .runtime_observation(options, row, observation_owner, &runtime)
            .map_err(unavailable)?;
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
    drop(runtime_snapshot);
    tx.set_drop_behavior(rusqlite::DropBehavior::Ignore);
    drop(tx);
    let sources = sources.into_values().collect::<Vec<_>>();
    let sources = match &binding {
        crate::storage::NativeReaderBinding::Semantic(owner) => {
            crate::query_source::Sources::capture_bound_snapshot(options, &sources, db, owner)
        }
        crate::storage::NativeReaderBinding::Admitted(_) => {
            crate::query_source::Sources::capture_focused_snapshot(options, &sources, db)
        }
    }
    .map_err(|error| proof_unavailable(&error))?;
    let binding = crate::storage::local_navigation_binding(options, "").map_err(unavailable)?;
    let mut db = binding.connection().map_err(unavailable)?;
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
            crate::overview_repair::query_context(&snapshot, key, scope).map_err(unavailable)?,
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
pub(crate) fn read(
    options: &Options,
    key: &str,
    scope: crate::CatalogReadScope,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Option<Value>, QueryError> {
    read_inner(options, key, scope, args, provider, mode, now, None)
}

pub(crate) fn session(
    options: &Options,
    key: &str,
    scope: crate::CatalogReadScope,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Option<Value>, QueryError> {
    let mut args = args.clone();
    args["_session"] = Value::Bool(true);
    read_inner(options, key, scope, &args, provider, mode, now, None)
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
        crate::CatalogReadScope::RetainedHistory,
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
    scope: crate::CatalogReadScope,
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
    let binding = crate::storage::local_navigation_binding(options, "").map_err(unavailable)?;
    let mut db = binding.connection().map_err(unavailable)?;
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
    let current: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key=?1 AND lifecycle='current')",
            [key],
            |row| row.get(0),
        )
        .map_err(unavailable)?;
    let history = match scope {
        crate::CatalogReadScope::Current => None,
        crate::CatalogReadScope::RetainedHistory => {
            crate::history_projection::catalog(&tx, key).map_err(unavailable)?
        }
    };
    let text = match history.as_ref() {
        Some(text) => Some(text.clone()),
        None => tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key=?1", [key], |row|row.get::<_,Option<String>>(0))
            .optional().map_err(unavailable)?.flatten(),
    };
    let Some(text) = text else {
        tx.commit().map_err(unavailable)?;
        return virtual_read(options, key, scope, args, provider, mode, now);
    };
    if scope == crate::CatalogReadScope::Current && !current {
        return Ok(None);
    }
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
        && !crate::overview_repair::query_window_context_current(&tx, options, key, scope, now)
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
    let (runtime, row_runtime) = {
        let snapshot = crate::storage::sqlite::catalog::admitted_snapshot(&binding, &tx)
            .map_err(unavailable)?;
        if current {
            let runtime = snapshot.runtime_generation(options).map_err(unavailable)?;
            let observed = snapshot
                .runtime_observation(
                    options,
                    &row,
                    if history.is_some() {
                        crate::storage::catalog::CatalogObservationOwner::History
                    } else {
                        crate::storage::catalog::CatalogObservationOwner::Catalog
                    },
                    &runtime,
                )
                .map_err(unavailable)?;
            (Some(runtime), observed)
        } else {
            // Retained custody still requires the original publication binding,
            // even when its consumed file ledger has been retired.
            snapshot.row_runtime_binding(&row).map_err(unavailable)?;
            (None, None)
        }
    };
    if !crate::model::summary::query_observation_matches(
        saved,
        &row.sources,
        current,
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
        crate::overview_repair::query_context(&tx, key, scope).map_err(unavailable)?
    } else {
        vec![row.clone()]
    };
    let mut query_headers = Vec::new();
    if search.is_some() || session {
        for owner in &context {
            if scope == crate::CatalogReadScope::RetainedHistory
                && let Some(encoded) =
                    crate::history_projection::catalog(&tx, &owner.key).map_err(unavailable)?
            {
                // History owns its embedded selected recipes. A later Current
                // publication replaces the mutable Query header for the same
                // native owner without replacing this retained observation.
                // The full context is checked again after rendering below.
                if serde_json::from_str::<Value>(&encoded).map_err(unavailable)?
                    != serde_json::to_value(owner).map_err(unavailable)?
                    || !crate::history_projection::qualified(&tx, owner).map_err(unavailable)?
                {
                    return Ok(None);
                }
                continue;
            }
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
    let sources = match &binding {
        crate::storage::NativeReaderBinding::Semantic(owner) => {
            crate::query_source::Sources::capture_bound_snapshot(
                options,
                &selected_sources,
                db,
                owner,
            )
        }
        crate::storage::NativeReaderBinding::Admitted(_) => {
            crate::query_source::Sources::capture_focused_snapshot(options, &selected_sources, db)
        }
    }
    .map_err(|error| proof_unavailable(&error))?;
    let mut db = crate::events::store::local_reader(options).map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let records = sources.reader_with_mode(provider, mode);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::run(options, "query_transcript_before_read");
    let bytes_left = std::cell::RefCell::new(64 * 1024 * 1024usize);
    if session {
        let mut summary = render_session(
            options,
            key,
            current,
            &context,
            &records,
            now,
            &|turn| {
                let encoded:String=db.query_row(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND json_extract(metadata,'$.turn')=?2 AND json_extract(metadata,'$.first')=1 ORDER BY slot LIMIT 1"),params![key,turn],|row|row.get(0)).map_err(unavailable)?;
                serde_json::from_str(&encoded).map_err(unavailable)
            },
            &|turn| {
                cutoff
                    .map(|cutoff| turn_kept(&db, turn, cutoff))
                    .unwrap_or(Ok(true))
            },
            &|id| {
                cutoff
                    .map(|cutoff| edge_kept(&db, id, cutoff))
                    .unwrap_or(Ok(true))
            },
            &|id| {
                if let Some(cutoff) = cutoff {
                    let Some(saved) = context
                        .iter()
                        .find(|owner| owner.key == id)
                        .and_then(|owner| owner.query.as_ref())
                    else {
                        return Ok(false);
                    };
                    window_member(&db, id, saved, cutoff)
                } else {
                    Ok(true)
                }
            },
        )?;
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
        if let Some(runtime) = &runtime
            && crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?
                != *runtime
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
        let current =
            crate::overview_repair::query_context(&snapshot, key, scope).map_err(unavailable)?;
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
        let current =
            crate::overview_repair::query_context(&snapshot, key, scope).map_err(unavailable)?;
        if serde_json::to_value(&current).map_err(unavailable)?
            != serde_json::to_value(&context).map_err(unavailable)?
        {
            return Err(unavailable("Native query relationship publication changed"));
        }
        snapshot.commit().map_err(unavailable)?;
        if let Some(runtime) = &runtime
            && crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?
                != *runtime
        {
            return Err(unavailable(
                "Native search observation changed during this read",
            ));
        }
        return Ok(Some(Value::Null));
    }
    let answer = render_page(
        options,
        key,
        &row,
        &observation,
        total,
        forward,
        from,
        limit,
        now,
        &SqlitePage {
            db: &db,
            records: &records,
            key,
            history: history.is_some(),
            text: &text,
            headers,
            table,
            slot_generation: &slot_generation,
        },
    )?;
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
    if let Some(runtime) = &runtime
        && crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?
            != *runtime
    {
        return Err(unavailable(
            "Native transcript observation changed during this read",
        ));
    }
    Ok(Some(answer))
}

/// Ordinary selected-session rendering for both physical stores. Recipes
/// contain no prompt/answer text; all content passes the authorized record port.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_session(
    options: &Options,
    key: &str,
    current: bool,
    context: &[CatalogRow],
    records: &dyn QueryRecords,
    now: i64,
    first: &dyn Fn(&str) -> Result<SavedSlot, QueryError>,
    keep_turn: &dyn Fn(&Value) -> Result<bool, QueryError>,
    keep_edge: &dyn Fn(&str) -> Result<bool, QueryError>,
    keep_child: &dyn Fn(&str) -> Result<bool, QueryError>,
) -> Result<Value, QueryError> {
    let row = context
        .iter()
        .find(|row| row.key == key)
        .ok_or_else(|| unavailable("Selected owner unavailable"))?;
    let saved = row
        .query
        .as_ref()
        .ok_or_else(|| unavailable("Selected Query unavailable"))?;
    let selected = &saved["selected"];
    let mut summary = saved["rows"][key]["summary"].clone();
    if !summary.is_object() || !selected["session"]["busy"].is_array() {
        return Err(unavailable("Selected Query recipe unavailable"));
    }
    if !current {
        summary["alive"] = Value::Null;
        summary["run"] = Value::Null;
        if matches!(
            summary["state"].as_str(),
            Some("working" | "waiting-question")
        ) {
            summary["state"] = json!("unknown");
            summary["open_question"] = Value::Null;
        }
    }
    summary["busy"] = selected["session"]["busy"].clone();
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
    let bytes_left = std::cell::RefCell::new(64 * 1024 * 1024usize);
    let mut metadata = 0usize;
    let mut turns = Vec::new();
    for recipe in selected["turns"]
        .as_array()
        .ok_or_else(|| unavailable("Query turns unavailable"))?
    {
        metadata += recipe.to_string().len();
        if metadata > 8 * 1024 * 1024 {
            return Err(unavailable(
                "Session result metadata exceeds its request budget",
            ));
        }
        let mut turn = recipe["query"].clone();
        if !keep_turn(&turn)? {
            continue;
        }
        if recipe["query_text"] == true {
            let slot = first(
                recipe["id"]
                    .as_str()
                    .ok_or_else(|| unavailable("Turn identity unavailable"))?,
            )?
            .restore();
            let file = slot
                .file
                .as_ref()
                .ok_or_else(|| unavailable("Turn prompt source unavailable"))?;
            let (record, bytes) = records
                .prompt(file, slot.offset)
                .ok_or_else(|| unavailable("Turn prompt unavailable"))?;
            let remaining = *bytes_left.borrow();
            *bytes_left.borrow_mut() = remaining
                .checked_sub(bytes as usize)
                .ok_or_else(|| unavailable("Session native bytes exceed the request budget"))?;
            let prompt = crate::model::prompt_text(&record)
                .ok_or_else(|| unavailable("Turn prompt text unavailable"))?;
            turn["text"] = json!(crate::model::cap(
                &crate::model::cap(&prompt, crate::model::TEXT_MAX),
                crate::model::MSG_MAX
            ));
        }
        turns.push(turn);
    }
    summary["turns"] = json!(turns);
    let mut handoffs = Vec::new();
    for recipe in selected["edges"]
        .as_array()
        .ok_or_else(|| unavailable("Query handoffs unavailable"))?
    {
        let id = recipe["id"]
            .as_str()
            .ok_or_else(|| unavailable("Handoff identity unavailable"))?;
        if !keep_edge(id)? {
            continue;
        }
        // This owner captured its complete field recipe at publication. Its
        // retained relationship state must not be mixed with another owner's
        // later Current observation. Context still qualifies every source read.
        let handoff = render_edge(
            id,
            context,
            records,
            std::slice::from_ref(recipe),
            &bytes_left,
        )?;
        metadata += handoff.to_string().len();
        if metadata > 8 * 1024 * 1024 {
            return Err(unavailable(
                "Session result metadata exceeds its request budget",
            ));
        }
        if summary["open_question"]["handoff"] == id {
            summary["open_question"]["text"] = handoff["brief"].clone();
        }
        handoffs.push(handoff);
    }
    handoffs.sort_by(|a, b| {
        a["at"]
            .as_i64()
            .cmp(&b["at"].as_i64())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    summary["handoffs"] = json!(handoffs);
    if let Some(children) = summary["children"].as_array_mut() {
        let mut kept = Vec::new();
        for child in children.iter() {
            if keep_child(
                child
                    .as_str()
                    .ok_or_else(|| unavailable("Child identity unavailable"))?,
            )? {
                kept.push(child.clone());
            }
        }
        *children = kept;
    }
    Ok(summary)
}

/// The ordinary pager consumes immutable recipes and independently authorized
/// records. Physical backends own publication/proof qualification and lifetime.
pub(crate) trait PageSource {
    fn query_error(&self, error: &io::Error) -> QueryError {
        QueryError::io(error)
    }

    fn slots(&self, low: usize, high: usize) -> Result<Vec<SavedSlot>, QueryError>;
    fn read(&self, path: &std::path::Path, offset: u64) -> io::Result<(Option<Value>, u64)>;
    fn prompt(&self, file: &crate::model::SlotFile, offset: u64) -> Option<(Value, u64)>;
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_page(
    options: &Options,
    key: &str,
    row: &CatalogRow,
    observation: &QueryObservation,
    total: usize,
    forward: bool,
    from: usize,
    limit: usize,
    now: i64,
    source: &dyn PageSource,
) -> Result<Value, QueryError> {
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
        let recipes = source.slots(low, high)?;
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
        let mut read = |path: &std::path::Path, offset| source.read(path, offset);
        let mut prompts = |file: &crate::model::SlotFile, offset| source.prompt(file, offset);
        let mut lines = tx::Lines::provider_with_prompts(&mut read, &mut prompts);
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
            &mut lines,
        );
        if lines.failures != 0 {
            return Err(source.query_error(&io::Error::other(
                "Authorized native transcript read failed",
            )));
        }
        match page {
            Ok(page) => break page,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && count < MAX_SLOTS => {
                count = (count * 2).min(MAX_SLOTS)
            }
            Err(error) => return Err(source.query_error(&error)),
        }
    };
    Ok(answer)
}

struct SqlitePage<'a, 'b> {
    db: &'a Connection,
    records: &'a crate::query_source::Records<'b>,
    key: &'a str,
    history: bool,
    text: &'a str,
    headers: &'a str,
    table: &'a str,
    slot_generation: &'a str,
}
impl PageSource for SqlitePage<'_, '_> {
    fn query_error(&self, error: &io::Error) -> QueryError {
        self.records.query_error(error)
    }

    fn slots(&self, low: usize, high: usize) -> Result<Vec<SavedSlot>, QueryError> {
        let snapshot = self.db.unchecked_transaction().map_err(unavailable)?;
        unchanged(
            &snapshot,
            self.key,
            self.history,
            self.text,
            self.headers,
            self.slot_generation,
        )?;
        let mut statement = snapshot.prepare(&format!(
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM main.{} WHERE namespace=semon_namespace() AND session_key=?1 AND slot>=?2 AND slot<?3 ORDER BY slot", self.table
        )).map_err(unavailable)?;
        let texts = statement
            .query_map(params![self.key, low as i64, high as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(unavailable)?;
        let mut bytes = 0;
        let mut recipes = Vec::new();
        for text in texts {
            let text = text.map_err(unavailable)?;
            bytes += text.len();
            if bytes > METADATA_BYTES {
                return Err(unavailable(
                    "Native transcript metadata exceeds its read budget",
                ));
            }
            recipes.push(serde_json::from_str(&text).map_err(unavailable)?);
        }
        drop(statement);
        snapshot.commit().map_err(unavailable)?;
        Ok(recipes)
    }
    fn read(&self, path: &std::path::Path, offset: u64) -> io::Result<(Option<Value>, u64)> {
        self.records.read(path, offset)
    }
    fn prompt(&self, file: &crate::model::SlotFile, offset: u64) -> Option<(Value, u64)> {
        self.records.prompt(file, offset)
    }
}

#[cfg(test)]
mod proof_error_tests {
    use super::*;

    struct NoNativeReads;

    impl crate::SessionSourceReader for NoNativeReads {
        fn read_range(
            &self,
            _: &crate::SessionSourceRef,
            _: Option<&str>,
            _: u64,
            _: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            unreachable!("a source-less recipe must be refused before native I/O")
        }
    }

    impl QueryRecords for NoNativeReads {
        fn reader(&self) -> &dyn crate::SessionSourceReader {
            self
        }

        fn source_ref(&self, _: &std::path::Path) -> Option<&crate::SessionSourceRef> {
            None
        }

        fn record_bytes(&self, _: &std::path::Path, _: u64, _: u64) -> io::Result<Option<Vec<u8>>> {
            unreachable!("a source-less recipe must be refused before native I/O")
        }

        fn prompt(&self, _: &crate::model::SlotFile, _: u64) -> Option<(Value, u64)> {
            None
        }
    }

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

    #[test]
    fn source_less_handoff_recipe_still_refuses_the_selected_read() {
        let saved = serde_json::json!({
            "query": {"id": "edge", "from": "one", "to": "child", "kind": "spawn"},
            "query_presence": {"brief": true},
            "query_content": {
                "brief": {
                    "path": "/fixture/one.jsonl",
                    "offset": 0,
                    "block": 0,
                    "kind": "prompt"
                }
            }
        });
        let error = render_edge(
            "edge",
            &[],
            &NoNativeReads,
            &[saved],
            &std::cell::RefCell::new(1024),
        )
        .unwrap_err();
        assert_eq!(error.code, "io");
        assert_eq!(error.message, "Handoff source generation unavailable");
    }
}

#[cfg(test)]
mod namespace_isolation_tests {
    use super::*;
    use rusqlite::functions::FunctionFlags;

    fn bind(db: &Connection, namespace: &'static str) {
        db.create_scalar_function(
            "semon_namespace",
            0,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            move |_| Ok(namespace.to_owned()),
        )
        .unwrap();
    }

    #[test]
    fn bound_transcript_reads_never_see_another_namespace() {
        // Both namespaces hold the same session key and edge identity; only
        // namespace b has live edges, a live turn and retained history.
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(crate::storage::sqlite::trace::SCHEMA)
            .unwrap();
        db.execute_batch(crate::storage::sqlite::history::SCHEMA)
            .unwrap();
        db.execute_batch(
            "CREATE TABLE meta(namespace TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(namespace,key)) STRICT, WITHOUT ROWID;
             CREATE TABLE session_catalog(namespace TEXT NOT NULL,session_key TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,session_key));
             INSERT INTO meta VALUES('a','catalog_generation','generation-a'),('b','catalog_generation','generation-b');
             INSERT INTO session_catalog VALUES('a','shared','{\"query\":{\"last\":1}}'),('b','shared','{\"query\":{\"state\":\"work\"}}');
             INSERT INTO session_trace_edges VALUES('a','shared','edge','{\"status\":\"done\",\"at\":1}'),('b','shared','edge','{\"status\":\"work\",\"at\":1}'),('b','shared','b-only','{\"status\":\"work\"}');
             INSERT INTO session_trace_turns VALUES('b','shared','turn','edge','{\"start\":\"edge\",\"last\":1}');
             INSERT INTO session_history_catalog VALUES('b','shared','{\"key\":\"shared\"}');",
        )
        .unwrap();
        let stale = serde_json::json!({"last": 1});
        bind(&db, "a");
        assert_eq!(generation(&db).unwrap(), "generation-a");
        assert!(!edge_timed(&db, "edge", 100).unwrap());
        assert!(!edge_timed(&db, "b-only", 100).unwrap());
        assert!(!edge_kept(&db, "edge", 100).unwrap());
        assert!(!window_member(&db, "shared", &stale, 100).unwrap());
        assert_eq!(
            crate::history_projection::catalog(&db, "shared").unwrap(),
            None
        );
        bind(&db, "b");
        assert_eq!(generation(&db).unwrap(), "generation-b");
        assert!(edge_timed(&db, "edge", 100).unwrap());
        assert!(edge_timed(&db, "b-only", 100).unwrap());
        assert!(window_member(&db, "shared", &stale, 100).unwrap());
        assert_eq!(
            crate::history_projection::catalog(&db, "shared")
                .unwrap()
                .as_deref(),
            Some("{\"key\":\"shared\"}")
        );
    }
}
