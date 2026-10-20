//! Exact summaries from the native Catalog publication. Source text is resolved
//! only for returned open questions, using the existing native preview reader.
use super::*;
use crate::events::EventCache;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

pub(super) fn list_response(
    mut rows: Vec<Value>,
    args: &Value,
    since: Option<i64>,
    now: i64,
) -> Result<Value, QueryError> {
    let parent = string(args, "parent");
    if let Some(parent) = &parent
        && !rows.iter().any(|row| row["id"] == *parent)
    {
        return Err(QueryError::unknown_session(parent));
    }
    rows.retain(|row| {
        ["state", "repo", "parent", "harness"]
            .into_iter()
            .all(|key| string(args, key).is_none_or(|value| row[key] == value))
            && since.is_none_or(|since| {
                row["last_activity"]
                    .as_i64()
                    .is_some_and(|last| last >= since)
            })
    });
    rows.sort_by(newest_first);
    let total = rows.len();
    rows.truncate(integer(args, "limit").unwrap_or(LIST_LIMIT) as usize);
    Ok(json!({"now":now,"total":total,"truncated":total>rows.len(),"sessions":rows}))
}

pub(super) fn stall_response(
    rows: Vec<(Value, Option<String>)>,
    args: &Value,
    since: Option<i64>,
    now: i64,
) -> Value {
    let idle_minutes = integer(args, "idle_minutes").unwrap_or(1);
    let threshold = idle_minutes.saturating_mul(60_000);
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter_map(|(mut row, recorded_status)| {
            let last = row["last_activity"].as_i64()?;
            if since.is_some_and(|since| last < since) || now.saturating_sub(last) < threshold {
                return None;
            }
            let reason = if row["alive"] == false && recorded_status.as_deref() == Some("busy") {
                StallReason::ProcessGone
            } else {
                match row["state"].as_str() {
                    Some("waiting-question") => StallReason::WaitingQuestion,
                    Some("working") => StallReason::NoOutput,
                    _ => return None,
                }
            };
            row["stall_reason"] = json!(reason.name());
            row["idle_ms"] = json!(now.saturating_sub(last));
            Some(row)
        })
        .collect();
    rows.sort_by(|a, b| {
        a["last_activity"]
            .as_i64()
            .cmp(&b["last_activity"].as_i64())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    json!({"now":now,"idle_minutes":idle_minutes,"rule":STALL_RULE,"stalls":rows})
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Work {
    pub metadata: usize,
    pub edges: usize,
    pub turns: usize,
    pub vm_steps: i32,
}
#[cfg(test)]
thread_local! { pub(super) static WORK: std::cell::Cell<Work> = const {std::cell::Cell::new(Work {metadata:0,edges:0,turns:0,vm_steps:0})}; }
#[cfg(test)]
fn measured(metadata: usize, edges: usize, turns: usize, vm_steps: i32) {
    WORK.with(|work| {
        let prior = work.get();
        work.set(Work {
            metadata: prior.metadata + metadata,
            edges: prior.edges + edges,
            turns: prior.turns + turns,
            vm_steps: prior.vm_steps + vm_steps,
        });
    });
}

struct Part {
    generation: String,
    runtime: String,
    rows: BTreeMap<String, Value>,
    owners: BTreeMap<String, String>,
    content: BTreeMap<String, String>,
    witnesses: Vec<(String, String)>,
    edges: BTreeSet<String>,
    turns: BTreeSet<String>,
}
fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("unavailable", error.to_string())
}
fn generation(db: &Connection) -> Result<String, QueryError> {
    db.query_row(
        "SELECT value FROM meta WHERE key='catalog_generation'",
        [],
        |row| row.get(0),
    )
    .map_err(unavailable)
}
fn open(options: &Options) -> Result<Connection, QueryError> {
    let db = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    Ok(db)
}
fn read(options: &Options, now: i64, bytes_left: &mut usize) -> Result<Part, QueryError> {
    let db = open(options)?;
    db.execute_batch("BEGIN").map_err(unavailable)?;
    let generation = generation(&db)?;
    let ready: bool = db.query_row("SELECT (SELECT value FROM meta WHERE key='catalog_version')=?1 AND COALESCE((SELECT value FROM meta WHERE key='catalog_completeness'),'complete')='complete' AND (SELECT value FROM meta WHERE key='slot_projection_catalog_generation')=?2 AND NOT EXISTS(SELECT 1 FROM session_catalog_invalidations)",rusqlite::params![crate::model::summary::CATALOG_VERSION.to_string(),generation],|row|row.get(0)).map_err(unavailable)?;
    if !ready {
        return Err(unavailable("Native Catalog publication is incomplete"));
    }
    let runtime = crate::native_evidence::query_runtime_generation(options)
        .map_err(|error| QueryError::io(&error))?;
    let runtime_observations = crate::native_evidence::RuntimeObservations::new(options);
    let cutoff = (!options.all)
        .then(|| now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX)));
    let base_sql = if cutoff.is_some() {
        "SELECT session_key FROM session_catalog WHERE last_ms>=?1 UNION SELECT session_key FROM session_catalog WHERE json_extract(metadata,'$.query.last')>=?1 UNION SELECT session_key FROM session_catalog WHERE lifecycle='current' AND json_extract(metadata,'$.query.state') IN ('work','wait') UNION SELECT session_key FROM session_history_catalog WHERE json_extract(metadata,'$.query.last')>=?1"
    } else {
        "SELECT session_key FROM session_catalog"
    };
    let mut base = db.prepare(base_sql).map_err(unavailable)?;
    let mut keep: BTreeSet<String> = base
        .query_map(rusqlite::params_from_iter(cutoff), |row| row.get(0))
        .map_err(unavailable)?
        .collect::<rusqlite::Result<_>>()
        .map_err(unavailable)?;
    #[cfg(test)]
    measured(0, 0, 0, base.get_status(rusqlite::StatementStatus::VmStep));
    drop(base);
    let mut needed = keep.clone();
    let mut edges = BTreeSet::new();
    let mut turns = BTreeSet::new();
    let edge_select = "SELECT session_key,edge_id,json_extract(metadata,'$.from'),json_extract(metadata,'$.to') FROM session_trace_edges";
    let edge_sql = if cutoff.is_some() {
        format!(
            "{edge_select} WHERE json_extract(metadata,'$.at')>=?1 UNION {edge_select} WHERE json_extract(metadata,'$.done')>=?1 UNION {edge_select} WHERE json_extract(metadata,'$.status') IN ('work','wait','new')"
        )
    } else {
        edge_select.to_owned()
    };
    let mut edge = db.prepare(&edge_sql).map_err(unavailable)?;
    for item in edge
        .query_map(rusqlite::params_from_iter(cutoff), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(unavailable)?
    {
        let (owner, id, from, to) = item.map_err(unavailable)?;
        #[cfg(test)]
        measured(0, 1, 0, 0);
        spend(
            bytes_left,
            owner.len() + id.len() + from.len() + to.as_ref().map_or(0, String::len),
        )?;
        needed.insert(owner);
        edges.insert(id);
        keep.insert(from);
        keep.extend(to.filter(|to| to != "you"));
    }
    #[cfg(test)]
    measured(0, 0, 0, edge.get_status(rusqlite::StatementStatus::VmStep));
    drop(edge);
    let initially_kept = keep.clone();
    let turn_select = "SELECT turn_id,CASE WHEN octet_length(metadata)<=65536 THEN json_object('start',json_extract(metadata,'$.start'),'sent',json_extract(metadata,'$.sent')) ELSE NULL END FROM session_trace_turns WHERE session_key=?1";
    let turn_sql = if cutoff.is_some() {
        format!(
            "{turn_select} AND json_extract(metadata,'$.last')=1 UNION {turn_select} AND json_extract(metadata,'$.at')>=?2"
        )
    } else {
        turn_select.to_owned()
    };
    let mut recent = db.prepare(&turn_sql).map_err(unavailable)?;
    let mut endpoint = db.prepare("SELECT json_extract(metadata,'$.from'),json_extract(metadata,'$.to') FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2").map_err(unavailable)?;
    for owner in &initially_kept {
        let params: Vec<rusqlite::types::Value> = std::iter::once(owner.clone().into())
            .chain(cutoff.map(Into::into))
            .collect();
        for item in recent
            .query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(unavailable)?
        {
            let (id, links) = item.map_err(unavailable)?;
            #[cfg(test)]
            measured(0, 0, 1, 0);
            let links = links.ok_or_else(|| unavailable("Turn context exceeds its bound"))?;
            spend(bytes_left, id.len() + links.len())?;
            turns.insert(id);
            let links: Value = serde_json::from_str(&links).map_err(unavailable)?;
            for id in links["start"].as_str().into_iter().chain(
                links["sent"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            ) {
                if edges.insert(id.to_owned())
                    && let Some((from, to)) = endpoint
                        .query_row(rusqlite::params![owner, id], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                        })
                        .optional()
                        .map_err(unavailable)?
                {
                    keep.insert(from);
                    keep.extend(to.filter(|to| to != "you"));
                }
            }
        }
    }
    #[cfg(test)]
    measured(
        0,
        0,
        0,
        recent.get_status(rusqlite::StatementStatus::VmStep)
            + endpoint.get_status(rusqlite::StatementStatus::VmStep),
    );
    drop(recent);
    drop(endpoint);
    needed.extend(keep.iter().cloned());
    let mut rows = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut content = BTreeMap::new();
    let mut witnesses = Vec::new();
    let mut metadata = db.prepare("SELECT c.lifecycle,CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END,CASE WHEN octet_length(hc.metadata)<=1048576 THEN hc.metadata ELSE NULL END,json_extract(h.metadata,'$.content_generation'),hc.session_key IS NOT NULL FROM session_catalog c LEFT JOIN session_history_catalog hc USING(session_key) LEFT JOIN session_trace_headers h USING(session_key) WHERE c.session_key=?1").map_err(unavailable)?;
    for key in needed {
        let record = metadata
            .query_row([&key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })
            .optional()
            .map_err(unavailable)?;
        let Some((lifecycle, current, history, version, has_history)) = record else {
            continue;
        };
        #[cfg(test)]
        measured(1, 0, 0, 0);
        let text = if has_history { history } else { current };
        let text = text.ok_or_else(|| unavailable("Catalog metadata exceeds its bound"))?;
        spend(bytes_left, text.len())?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&text).map_err(unavailable)?;
        if row.key != key {
            return Err(unavailable("Catalog metadata identity changed"));
        }
        if row.sources.is_empty()
            || row.sources.len() > 64
            || row.sources.iter().any(|source| {
                crate::catalog::source_relative(options, &row.harness, source).is_none()
            })
        {
            return Err(unavailable(
                "Catalog sources are outside configured native authority",
            ));
        }
        let row_runtime = runtime_observations.expected(&row);
        let saved = row
            .query
            .ok_or_else(|| unavailable("Exact Catalog query facts are unavailable"))?;
        if !crate::model::summary::query_observation_matches(
            &saved,
            &row.sources,
            lifecycle == "current",
            row_runtime.as_deref(),
        )
        .map_err(unavailable)?
        {
            return Err(unavailable(
                "Native Query observations changed; retry after publication",
            ));
        }
        witnesses.push((key.clone(), saved.to_string()));
        let summaries = saved["rows"]
            .as_object()
            .ok_or_else(|| unavailable("Invalid Catalog summaries"))?;
        if !summaries.contains_key(&key) {
            return Err(unavailable("Catalog summary ownership changed"));
        }
        for (id, saved) in summaries.iter().filter(|(id, _)| keep.contains(*id)) {
            let mut saved = saved.clone();
            if saved["summary"]["id"] != *id {
                return Err(unavailable("Catalog summary identity changed"));
            }
            if lifecycle != "current" {
                saved["summary"]["alive"] = Value::Null;
                saved["summary"]["run"] = Value::Null;
                if matches!(
                    saved["summary"]["state"].as_str(),
                    Some("working" | "waiting-question")
                ) {
                    saved["summary"]["state"] = json!("unknown");
                    saved["summary"]["open_question"] = Value::Null;
                }
            }
            if let Some(prior) = rows.get(id)
                && prior != &saved
            {
                return Err(unavailable("Catalog relationship observations disagree"));
            }
            rows.insert(id.clone(), saved);
            owners.entry(id.clone()).or_insert_with(|| key.clone());
        }
        if let Some(version) = version {
            content.insert(key, version);
        }
    }
    #[cfg(test)]
    measured(
        0,
        0,
        0,
        metadata.get_status(rusqlite::StatementStatus::VmStep),
    );
    drop(metadata);
    if keep.iter().any(|id| id != "you" && !rows.contains_key(id)) {
        return Err(unavailable("Catalog relationship context is unavailable"));
    }
    db.execute_batch("COMMIT").map_err(unavailable)?;
    Ok(Part {
        generation,
        runtime,
        rows,
        owners,
        content,
        witnesses,
        edges,
        turns,
    })
}
fn spend(left: &mut usize, bytes: usize) -> Result<(), QueryError> {
    *left = left
        .checked_sub(bytes)
        .ok_or_else(|| unavailable("Catalog query metadata exceeds the request budget"))?;
    Ok(())
}

fn has_search_slots(options: &Options, key: &str, since: Option<i64>) -> Result<bool, QueryError> {
    let db = open(options)?;
    let history: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_history_projections WHERE session_key=?1)",
            [key],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    let table = if history {
        "session_history_slots"
    } else {
        "session_slots"
    };
    db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE session_key=?1 AND (?2 IS NULL OR json_extract(metadata,'$.t')>=?2))"),rusqlite::params![key,since],|r|r.get(0)).map_err(unavailable)
}

impl Query {
    #[allow(clippy::too_many_arguments)]
    fn search_catalog(
        &self,
        args: &Value,
        since: Option<i64>,
        now: i64,
        rows: &BTreeMap<String, Value>,
        owner: &BTreeMap<String, (usize, String)>,
        parts: &[Part],
        provider: Option<&CatalogReaderFactory>,
        mode: crate::SessionSourceReadMode,
    ) -> Result<Value, QueryError> {
        let summaries = rows
            .iter()
            .map(|(id, saved)| (id.clone(), saved["summary"].clone()))
            .collect();
        let mut search = super::search::Search::new(args, since, summaries);
        for served in search.order() {
            let (index, own) = &owner[&served];
            let (machine, source) = &self.sources.machines[*index];
            let machine = if machine.is_empty() { "local" } else { machine };
            let key = &parts[*index].owners[own];
            // The old stop rule runs before the next eligible slot, even when
            // that slot would not match. Qualify that condition using recipes
            // before creating another archive reader or decoding its evidence.
            if search.at_capacity() {
                if has_search_slots(&source.options, key, since)? {
                    search.stop();
                    break;
                }
                continue;
            }
            let reader = provider
                .map(|factory| factory(machine, key))
                .transpose()
                .map_err(|error| QueryError::io(&error))?;
            let bounded = reader
                .as_deref()
                .map(|reader| crate::source_reader::BudgetedReader::new(reader, 1024));
            let reader = bounded
                .as_ref()
                .map(|reader| reader as &dyn crate::SessionSourceReader);
            if super::transcript::search(
                &source.options,
                key,
                &served,
                reader,
                mode,
                now,
                &mut search,
            )? {
                if search.stopped() {
                    break;
                }
                continue;
            }
            crate::overview_repair::with_query(&source.options, key, reader, mode, now, |data| {
                let data = Arc::new(data);
                let native_parts = self
                    .sources
                    .machines
                    .iter()
                    .enumerate()
                    .map(|(part, (key, source))| {
                        (
                            key.clone(),
                            if part == *index {
                                data.clone()
                            } else {
                                Arc::new(crate::model::QueryData::identity(&source.options, now))
                            },
                        )
                    })
                    .collect();
                let native = QueryRead::new(native_parts, usize::MAX);
                let view = View::native(
                    &native,
                    now,
                    ReadShape::Search,
                    reader.map(|reader| (reader, mode)),
                )?;
                let transcript = data
                    .tx
                    .get(own)
                    .ok_or_else(|| unavailable("Selected search transcript unavailable"))?;
                let records = data.sources.reader_with_mode(reader, mode);
                search.scan(&served, transcript, &data, &view.handoffs, &records)?;
                records.finish().map_err(|error| QueryError::io(&error))
            })
            .map_err(|error| QueryError::io(&error))??;
            if search.stopped() {
                break;
            }
        }
        Ok(search.finish(now))
    }

    /// Select summaries or search native sources using a published Catalog.
    /// Publication remains the caller's existing source coordinator's job.
    /// Providers are request-owned and scoped to their admitted machine key.
    pub fn call_catalog(
        &mut self,
        tool: &str,
        args: &Value,
        provider: Option<&CatalogReaderFactory>,
        mode: crate::SessionSourceReadMode,
    ) -> Result<Value, QueryError> {
        self.call_catalog_at(tool, args, provider, mode, now_ms())
    }
    #[doc(hidden)]
    pub fn call_catalog_at(
        &mut self,
        tool: &str,
        args: &Value,
        provider: Option<&CatalogReaderFactory>,
        mode: crate::SessionSourceReadMode,
        now: i64,
    ) -> Result<Value, QueryError> {
        validate_query_call(tool, args)?;
        self.execute_catalog_at(tool, args, provider, mode, now)
    }

    // Retained optional search qualification never opens a product override.
    #[cfg(test)]
    pub(super) fn call_optional_catalog_at(
        &mut self,
        tool: &str,
        args: &Value,
        provider: Option<&CatalogReaderFactory>,
        mode: crate::SessionSourceReadMode,
        now: i64,
    ) -> Result<Value, QueryError> {
        validate_tool_call(&retained_query_tools(), tool, args)?;
        self.execute_catalog_at(tool, args, provider, mode, now)
    }

    fn execute_catalog_at(
        &mut self,
        tool: &str,
        args: &Value,
        provider: Option<&CatalogReaderFactory>,
        mode: crate::SessionSourceReadMode,
        now: i64,
    ) -> Result<Value, QueryError> {
        #[cfg(test)]
        WORK.with(|work| work.set(Work::default()));
        if !matches!(tool, "list_sessions" | "stalls" | "find") {
            return Err(QueryError::invalid(
                "Catalog calls require a listing, stall or search tool",
            ));
        }
        if self.sources.is_retired() {
            return Err(unavailable("Native sources are closed"));
        }
        self.start = self
            .sources
            .machines
            .iter()
            .filter(|(_, source)| !source.options.all)
            .map(|(_, source)| {
                now.saturating_sub(
                    i64::try_from(source.options.since.as_millis()).unwrap_or(i64::MAX),
                )
            })
            .max();
        let since = self.since(args, now)?;
        let identity_observations: Vec<_> = self
            .sources
            .machines
            .iter()
            .map(|(_, source)| {
                crate::native_evidence::query_runtime_generation(&source.options)
                    .map_err(|error| QueryError::io(&error))
            })
            .collect::<Result<_, _>>()?;
        let labels = self.source_labels();
        let mut parts = Vec::new();
        let mut bytes_left = 8 * 1024 * 1024;
        let mut rows = BTreeMap::new();
        let mut owner = BTreeMap::new();
        let mut edge_ids = BTreeSet::new();
        let mut turn_ids = BTreeSet::new();
        for (index, (_, source)) in self.sources.machines.iter().enumerate() {
            if source.options.scan_window {
                return Err(QueryError::invalid(
                    "Catalog summaries require a complete native output window",
                ));
            }
            let part = read(&source.options, now, &mut bytes_left)?;
            if part.runtime != identity_observations[index] {
                return Err(unavailable(
                    "Native machine identity changed during the query",
                ));
            }
            let served = |id: &str| {
                if self.sources.machines.len() > 1
                    && (id.starts_with("unsent:")
                        || part
                            .rows
                            .get(id)
                            .is_some_and(|row| row["summary"]["kind"] == "stub"))
                {
                    format!("{id}@{}", labels[index].1)
                } else {
                    id.to_owned()
                }
            };
            for (ids, seen) in [(&part.edges, &mut edge_ids), (&part.turns, &mut turn_ids)] {
                for id in ids {
                    if !seen.insert(id.clone()) {
                        return Err(QueryError::new(
                            "id_conflict",
                            format!("two machines both have {id}"),
                        ));
                    }
                }
            }
            for (id, saved) in &part.rows {
                let mut saved = saved.clone();
                let served_id = served(id);
                saved["summary"]["id"] = json!(served_id);
                if let Some(parent) = saved["summary"]["parent"].as_str() {
                    saved["summary"]["parent"] = json!(served(parent));
                }
                if self.sources.machines.len() > 1 {
                    saved["summary"]["machine"] = json!(labels[index].1);
                }
                if rows.insert(served_id.clone(), saved).is_some() {
                    return Err(QueryError::new(
                        "id_conflict",
                        format!("two machines both have {served_id}"),
                    ));
                }
                owner.insert(served_id, (index, id.clone()));
            }
            parts.push(part);
        }
        // Children belong to the selected output cohort, not every historical
        // row that the producer observed before trimming its window.
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, saved) in &rows {
            if let Some(parent) = saved["summary"]["parent"].as_str() {
                children
                    .entry(parent.to_owned())
                    .or_default()
                    .push(id.clone());
            }
        }
        for (id, saved) in &mut rows {
            saved["summary"]["children"] = json!(children.get(id).cloned().unwrap_or_default());
        }
        let mut result = if tool == "find" {
            self.search_catalog(args, since, now, &rows, &owner, &parts, provider, mode)?
        } else if tool == "list_sessions" {
            list_response(
                rows.values()
                    .map(|saved| saved["summary"].clone())
                    .collect(),
                args,
                since,
                now,
            )?
        } else {
            stall_response(
                rows.values()
                    .map(|saved| {
                        (
                            saved["summary"].clone(),
                            saved["recorded_status"].as_str().map(str::to_owned),
                        )
                    })
                    .collect(),
                args,
                since,
                now,
            )
        };
        let returned = if tool != "stalls" {
            "sessions"
        } else {
            "stalls"
        };
        for row in result[returned].as_array_mut().expect("native answer") {
            if tool == "find" {
                break;
            }
            let Some(question) = row["open_question"]["handoff"].as_str().map(str::to_owned) else {
                continue;
            };
            let (index, id) = &owner[row["id"].as_str().expect("summary id")];
            let (key, source) = &self.sources.machines[*index];
            let key = if key.is_empty() { "local" } else { key };
            let catalog_key = &parts[*index].owners[id];
            let version = parts[*index]
                .content
                .get(catalog_key)
                .ok_or_else(|| unavailable("Native question recipe unavailable"))?;
            let query = format!(
                "sid={}&generation={}&events={}&scope=retained_history",
                crate::viewer::percent_encode(catalog_key),
                version,
                crate::viewer::percent_encode(&json!([question]).to_string())
            );
            let reader = provider
                .map(|provider| provider(key, catalog_key))
                .transpose()
                .map_err(|error| QueryError::io(&error))?;
            let answer = crate::overview::session_home_details_inner(
                &source.options,
                key,
                &query,
                reader.as_deref(),
                mode,
                false,
            );
            let body: Value = serde_json::from_slice(&answer.body).map_err(unavailable)?;
            if answer.status != 200 || body["incomplete"] == true {
                return Err(unavailable("Native question source is unavailable"));
            }
            row["open_question"]["text"] = body["events"][&question]["brief"].clone();
        }
        // Remote preview I/O does not hold a collection transaction. Recheck the
        // published generation and process observations before returning it.
        for ((_, source), part) in self.sources.machines.iter().zip(parts) {
            let db = open(&source.options)?;
            if generation(&db)? != part.generation
                || crate::native_evidence::query_runtime_generation(&source.options)
                    .map_err(|error| QueryError::io(&error))?
                    != part.runtime
            {
                return Err(unavailable("Native Catalog changed during the query"));
            }
            for (key, expected) in part.witnesses {
                let actual:Option<String>=db.query_row("SELECT CASE WHEN h.session_key IS NOT NULL THEN json_extract(h.metadata,'$.query') ELSE json_extract(c.metadata,'$.query') END FROM session_catalog c LEFT JOIN session_history_catalog h USING(session_key) WHERE c.session_key=?1",[&key],|row|row.get(0)).optional().map_err(unavailable)?.flatten();
                let actual = actual.and_then(|text| serde_json::from_str::<Value>(&text).ok());
                if actual.as_ref().map(Value::to_string).as_deref() != Some(expected.as_str()) {
                    return Err(unavailable("Native Query facts changed during the query"));
                }
            }
        }
        if self.sources.is_retired() {
            return Err(unavailable("Native sources are closed"));
        }
        result["window_start"] = json!(self.start);
        Ok(result)
    }
}
