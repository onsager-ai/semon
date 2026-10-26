//! Exact summaries from the native Catalog publication. Source text is resolved
//! only for returned open questions, using the existing native preview reader.
use super::*;
#[cfg(test)]
pub(super) use crate::storage::query::{WORK, Work};
use crate::storage::{LogicalSourceNamespace, query::Part};

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

fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("unavailable", error.to_string())
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
                let namespace = source.options.source.namespace().map_err(unavailable)?;
                if source
                    .query_catalog()
                    .map_err(unavailable)?
                    .has_search_slots(&namespace, key, since)?
                {
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
        if matches!(tool, "get_session" | "read_transcript") {
            if self.sources.is_retired() {
                return Err(unavailable("Native sources are closed"));
            }
            let id = args["id"].as_str().expect("validated focused id");
            let mut selected = None;
            for (source_key, source) in &self.sources.machines {
                let key = if source_key.is_empty() {
                    "local"
                } else {
                    source_key
                };
                if let Some(target) = source
                    .local_query_target(id, now)
                    .map_err(|error| QueryError::io(&error))?
                {
                    if selected.is_some() {
                        return Err(QueryError::new(
                            "id_conflict",
                            format!("two machines both have {id}"),
                        ));
                    }
                    selected = Some((key.to_owned(), target));
                }
            }
            let (source_key, target) = selected.ok_or_else(|| QueryError::unknown_session(id))?;
            let reader = provider
                .map(|factory| factory(&source_key, &target))
                .transpose()
                .map_err(|error| QueryError::io(&error))?;
            return self.execute_selected_at(
                &source_key,
                &target,
                tool,
                args,
                reader.as_deref(),
                mode,
                now,
            );
        }
        self.execute_catalog_store_at(tool, args, provider, mode, now, None)
    }

    /// Private single-Source binding for the admitted producer qualification.
    /// Rendering, filtering, windows and validation use the ordinary Query path.
    #[allow(dead_code)]
    pub(crate) fn call_bound_catalog_at(
        &mut self,
        tool: &str,
        args: &Value,
        now: i64,
        store: &dyn crate::storage::query::QueryCatalogStore,
    ) -> Result<Value, QueryError> {
        validate_query_call(tool, args)?;
        if self.sources.machines.len() != 1
            || !matches!(tool, "list_sessions" | "read_transcript" | "get_session")
        {
            return Err(QueryError::new(
                "namespace_upgrade_pending",
                "Admitted Query selector is not qualified",
            ));
        }
        if matches!(tool, "read_transcript" | "get_session") {
            return Err(unavailable(
                "Selected Query requires an authorized Raw reader",
            ));
        }
        self.execute_catalog_store_at(
            tool,
            args,
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            now,
            Some(store),
        )
    }

    /// Host authorization is explicit and separate from Catalog admission.
    #[allow(dead_code)]
    pub(crate) fn call_bound_selected_at(
        &mut self,
        tool: &str,
        args: &Value,
        now: i64,
        store: &dyn crate::storage::query::QueryCatalogStore,
        host: &dyn crate::SessionSourceReader,
    ) -> Result<Value, QueryError> {
        validate_query_call(tool, args)?;
        if self.sources.is_retired()
            || self.sources.machines.len() != 1
            || !matches!(tool, "read_transcript" | "get_session")
        {
            return Err(unavailable("Admitted selected Query tool unavailable"));
        }
        let mut selected_args = args.clone();
        selected_args["_session"] = serde_json::Value::Bool(tool == "get_session");
        let namespace = self.sources.machines[0]
            .1
            .options
            .source
            .namespace()
            .map_err(unavailable)?;
        let mut answer = store.selected_body(
            &namespace,
            args["id"].as_str().expect("validated selected id"),
            &selected_args,
            now,
            host,
        )?;
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
        answer["window_start"] = json!(self.start);
        Ok(answer)
    }

    fn execute_catalog_store_at(
        &mut self,
        tool: &str,
        args: &Value,
        provider: Option<&CatalogReaderFactory>,
        mode: crate::SessionSourceReadMode,
        now: i64,
        bound: Option<&dyn crate::storage::query::QueryCatalogStore>,
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
                source
                    .query_runtime()
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
            let namespace =
                LogicalSourceNamespace::new(if self.sources.machines[index].0.is_empty() {
                    "local"
                } else {
                    &self.sources.machines[index].0
                })
                .map_err(unavailable)?;
            let namespace = if bound.is_none() {
                source.options.source.namespace().map_err(unavailable)?
            } else {
                namespace
            };
            let local;
            let store = if let Some(store) = bound {
                store
            } else {
                local = source.query_catalog().map_err(unavailable)?;
                &*local
            };
            let part = store.read_cohort(&namespace, now, &mut bytes_left)?;
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
            if bound.is_some() {
                return Err(QueryError::new(
                    "namespace_upgrade_pending",
                    "Admitted selected question reader is not qualified",
                ));
            }
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
        for ((source_key, source), part) in self.sources.machines.iter().zip(parts) {
            let namespace = LogicalSourceNamespace::new(if source_key.is_empty() {
                "local"
            } else {
                source_key
            })
            .map_err(unavailable)?;
            let namespace = if bound.is_none() {
                source.options.source.namespace().map_err(unavailable)?
            } else {
                namespace
            };
            if let Some(store) = bound {
                store.validate(&namespace, &part)?;
            } else {
                source
                    .query_catalog()
                    .map_err(unavailable)?
                    .validate(&namespace, &part)?;
            }
        }
        if self.sources.is_retired() {
            return Err(unavailable("Native sources are closed"));
        }
        result["window_start"] = json!(self.start);
        Ok(result)
    }
}
