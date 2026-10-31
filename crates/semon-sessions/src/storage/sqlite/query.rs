//! SQLite implementation of semantic Query metadata operations.
#[cfg(test)]
use crate::storage::query::measured;
use crate::storage::{
    LogicalSourceNamespace,
    query::{Part, QueryCatalogStore, history_window, select_history},
};
use crate::{Options, QueryError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct SqliteQueryCatalog {
    pub(crate) options: Options,
    pub(crate) binding: crate::storage::NativeReaderBinding,
}
impl QueryCatalogStore for SqliteQueryCatalog {
    fn read_cohort(
        &self,
        namespace: &LogicalSourceNamespace,
        now: i64,
        bytes_left: &mut usize,
    ) -> Result<Part, QueryError> {
        let db = self
            .binding
            .listing_snapshot(namespace)
            .map_err(storage_error)?;
        let runtime_binding = self.binding.runtime_binding().map_err(storage_error)?;
        let admitted_binding = self
            .binding
            .runtime_binding_at(&db)
            .map_err(storage_error)?;
        read(
            &db,
            &self.options,
            now,
            bytes_left,
            matches!(
                self.binding,
                crate::storage::NativeReaderBinding::Semantic(_)
            ),
            runtime_binding.as_ref(),
            &admitted_binding,
        )
    }
    fn has_search_slots(
        &self,
        namespace: &LogicalSourceNamespace,
        key: &str,
        since: Option<i64>,
    ) -> Result<bool, QueryError> {
        if matches!(
            self.binding,
            crate::storage::NativeReaderBinding::Semantic(_)
        ) {
            return Err(QueryError::new(
                "namespace_upgrade_pending",
                "Admitted search reader is not qualified",
            ));
        }
        let db = self.binding.snapshot(namespace).map_err(storage_error)?;
        has_search_slots(&db, key, since)
    }
    fn validate(&self, namespace: &LogicalSourceNamespace, part: &Part) -> Result<(), QueryError> {
        let db = self
            .binding
            .listing_snapshot(namespace)
            .map_err(storage_error)?;
        let options = &self.options;
        if generation(&db)? != part.generation
            || crate::native_evidence::query_runtime_generation(options)
                .map_err(|error| QueryError::io(&error))?
                != part.runtime
        {
            return Err(unavailable("Native Catalog changed during the query"));
        }
        for (key, expected, selected_history) in &part.witnesses {
            let table = if *selected_history {
                "session_history_catalog"
            } else {
                "session_catalog"
            };
            let actual: Option<String> = db
                .query_row(
                    &format!("SELECT json_extract(metadata,'$.query') FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1"),
                    [key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(unavailable)?
                .flatten();
            let actual = actual.and_then(|text| serde_json::from_str::<Value>(&text).ok());
            let expected: Value = serde_json::from_str(expected).map_err(unavailable)?;
            if actual.as_ref() != Some(&expected) {
                return Err(unavailable("Native Query facts changed during the query"));
            }
        }
        Ok(())
    }
}
fn storage_error(error: crate::storage::StorageError) -> QueryError {
    use crate::storage::StorageError;
    let code = match error {
        StorageError::NamespaceUnadmitted => "namespace_unadmitted",
        StorageError::NamespaceMismatch => "namespace_mismatch",
        StorageError::ScopeChanged => "storage_epoch_changed",
        StorageError::NamespaceUpgradePending => "namespace_upgrade_pending",
        StorageError::InvalidArguments => "invalid_arguments",
        _ => "unavailable",
    };
    QueryError::new(code, error.to_string())
}

fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("unavailable", error.to_string())
}
fn generation(db: &Connection) -> Result<String, QueryError> {
    db.query_row(
        "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
        [],
        |row| row.get(0),
    )
    .map_err(unavailable)
}

fn load_catalog_row(
    db: &Connection,
    key: &str,
    history: bool,
    bytes_left: &mut usize,
) -> Result<Option<crate::model::summary::CatalogRow>, QueryError> {
    let table = if history {
        "session_history_catalog"
    } else {
        "session_catalog"
    };
    let record: Option<Option<String>> = db
        .query_row(
            &format!(
                "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1"
            ),
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    let Some(text) = record else {
        return Ok(None);
    };
    let text = text.ok_or_else(|| unavailable("Catalog metadata exceeds its bound"))?;
    spend(bytes_left, text.len())?;
    let row: crate::model::summary::CatalogRow =
        serde_json::from_str(&text).map_err(unavailable)?;
    if row.key != key {
        return Err(unavailable("Catalog metadata identity changed"));
    }
    Ok(Some(row))
}

fn read(
    db: &Connection,
    options: &Options,
    now: i64,
    bytes_left: &mut usize,
    private_listing: bool,
    runtime_binding: Option<&crate::storage::QueryRuntimeBinding>,
    admitted_binding: &crate::storage::QueryRuntimeBinding,
) -> Result<Part, QueryError> {
    let generation = generation(db)?;
    let imported_origin =
        super::proof_generations::imported_catalog(db, admitted_binding).map_err(unavailable)?;
    let admission =
        i64::try_from(admitted_binding.admission).map_err(|_| unavailable("Invalid admission"))?;
    let owner_grant = format!(
        "{}:{}",
        admitted_binding.admission, admitted_binding.stream_epoch
    );
    let publication_grant = format!(
        "{generation}:{}:{}",
        admitted_binding.admission, admitted_binding.stream_epoch
    );
    let imported: bool = imported_origin
        && db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.meta a WHERE a.namespace=?1 AND ((a.key='native_transfer_v1:catalog-owner:'||?2 AND a.value=?3) OR (a.key='catalog_native_publication' AND a.value=?4) OR (a.key='catalog_native_publication:'||?2 AND a.value=?3 AND EXISTS(SELECT 1 FROM storage_semantic_publications p WHERE p.namespace=a.namespace AND p.generation=?2 AND p.admission=?5))))",
                rusqlite::params![
                    admitted_binding.namespace.as_str(),
                    generation,
                    owner_grant,
                    publication_grant,
                    admission
                ],
                |row| row.get(0),
            )
            .map_err(unavailable)?;
    let ready_sql = if private_listing || imported {
        "SELECT (SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_version')=?1 AND ((SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_completeness')='complete' OR (SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='semantic_listing_generation')=?2) AND (SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='slot_projection_catalog_generation')=?2 AND NOT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=semon_namespace())"
    } else {
        "SELECT (SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_version')=?1 AND (SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_completeness')='complete' AND (SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='slot_projection_catalog_generation')=?2 AND NOT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=semon_namespace())"
    };
    let ready: bool = db
        .query_row(
            ready_sql,
            rusqlite::params![
                crate::model::summary::CATALOG_VERSION.to_string(),
                generation
            ],
            |row| row.get::<_, Option<bool>>(0),
        )
        .map_err(unavailable)?
        .unwrap_or(false);
    if !ready {
        return Err(unavailable("Native Catalog publication is incomplete"));
    }
    let runtime = crate::native_evidence::query_runtime_generation(options)
        .map_err(|error| QueryError::io(&error))?;
    let runtime_observations = crate::native_evidence::RuntimeObservations::new(options);
    let cutoff = (!options.all)
        .then(|| now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX)));
    // Each branch is an indexed stream. Charge every candidate before set
    // insertion, including duplicates, rather than letting UNION materialize
    // an unbounded temporary key set before request accounting begins.
    let branches: &[&str] = if cutoff.is_some() {
        &[
            "SELECT session_key FROM main.session_catalog INDEXED BY session_catalog_current_order WHERE namespace=semon_namespace() AND lifecycle='current' AND last_ms>=?1",
            "SELECT session_key FROM main.session_catalog INDEXED BY session_catalog_query_last WHERE namespace=semon_namespace() AND lifecycle='current' AND json_extract(metadata,'$.query.last')>=?1",
            "SELECT session_key FROM main.session_catalog INDEXED BY session_catalog_query_current_state WHERE namespace=semon_namespace() AND lifecycle='current' AND json_extract(metadata,'$.query.state') IN ('work','wait') AND ?1 IS NOT NULL",
            "SELECT session_key FROM main.session_history_catalog INDEXED BY session_history_query_activity WHERE namespace=semon_namespace() AND query_activity>=?1",
            "SELECT session_key FROM main.session_history_catalog INDEXED BY session_history_query_active WHERE namespace=semon_namespace() AND query_active=1 AND ?1 IS NOT NULL",
        ]
    } else {
        &["SELECT session_key FROM main.session_catalog WHERE namespace=semon_namespace()"]
    };
    let mut candidates = BTreeSet::new();
    for sql in branches {
        let mut statement = db.prepare(sql).map_err(unavailable)?;
        let mut records = statement
            .query(rusqlite::params_from_iter(cutoff))
            .map_err(unavailable)?;
        while let Some(record) = records.next().map_err(unavailable)? {
            let key = record
                .get_ref(0)
                .map_err(unavailable)?
                .as_str()
                .map_err(unavailable)?;
            spend(bytes_left, 64 + key.len())?;
            candidates.insert(key.to_owned());
        }
        drop(records);
        #[cfg(test)]
        measured(
            0,
            0,
            0,
            statement.get_status(rusqlite::StatementStatus::VmStep),
        );
    }
    let mut keep = BTreeSet::new();
    let mut selected_rows = BTreeMap::new();
    let mut history_selected = BTreeSet::new();
    for key in candidates {
        let current = load_catalog_row(db, &key, false, bytes_left)?;
        let current_eligible = cutoff.is_some_and(|cutoff| {
            current.as_ref().is_some_and(|row| {
                row.lifecycle == crate::model::summary::CatalogLifecycle::Current
                    && (row.last.is_some_and(|last| last >= cutoff)
                        || row.query.as_ref().is_some_and(|query| {
                            query["last"].as_i64().is_some_and(|last| last >= cutoff)
                                || matches!(query["state"].as_str(), Some("work" | "wait"))
                        }))
            })
        });
        // A missing Current row leaves a finite History candidate eligible;
        // an eligible Current row takes priority.
        if (cutoff.is_none() || !current_eligible)
            && let Some(row) = load_catalog_row(db, &key, true, bytes_left)?
        {
            let member = history_window(&row, cutoff).map_err(unavailable)?.member;
            if select_history(cutoff, current_eligible, true, member) {
                history_selected.insert(key.clone());
                selected_rows.insert(key.clone(), (row, true));
                keep.insert(key.clone());
                continue;
            }
        }
        if (current_eligible || cutoff.is_none())
            && let Some(row) = current
        {
            selected_rows.insert(key.clone(), (row, false));
            keep.insert(key);
        }
    }
    let mut needed = keep.clone();
    let mut edges = BTreeSet::new();
    let mut turns = BTreeSet::new();
    let mut current_graph_owners = BTreeSet::new();
    let edge_select = "SELECT session_key,edge_id,json_extract(metadata,'$.from'),json_extract(metadata,'$.to') FROM main.session_trace_edges WHERE namespace=semon_namespace()";
    let edge_sql = if cutoff.is_some() {
        format!(
            "{edge_select} AND (json_extract(metadata,'$.at')>=?1 OR json_extract(metadata,'$.done')>=?1 OR json_extract(metadata,'$.status') IN ('work','wait','new'))"
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
        if history_selected.contains(&owner) {
            continue;
        }
        spend(
            bytes_left,
            owner.len() + id.len() + from.len() + to.as_ref().map_or(0, String::len),
        )?;
        current_graph_owners.insert(owner.clone());
        needed.insert(owner);
        edges.insert(id);
        keep.insert(from);
        if let Some(to) = to.filter(|to| to != "you") {
            keep.insert(to);
        }
    }
    #[cfg(test)]
    measured(0, 0, 0, edge.get_status(rusqlite::StatementStatus::VmStep));
    drop(edge);
    let initially_kept = keep.clone();
    let turn_select = "SELECT turn_id,CASE WHEN octet_length(metadata)<=65536 THEN json_object('start',json_extract(metadata,'$.start'),'sent',json_extract(metadata,'$.sent')) ELSE NULL END FROM main.session_trace_turns WHERE namespace=semon_namespace() AND session_key=?1";
    let turn_sql = if cutoff.is_some() {
        format!(
            "{turn_select} AND json_extract(metadata,'$.last')=1 UNION {turn_select} AND json_extract(metadata,'$.at')>=?2"
        )
    } else {
        turn_select.to_owned()
    };
    let mut recent = db.prepare(&turn_sql).map_err(unavailable)?;
    let mut endpoint = db.prepare("SELECT json_extract(metadata,'$.from'),json_extract(metadata,'$.to') FROM main.session_trace_edges WHERE namespace=semon_namespace() AND session_key=?1 AND edge_id=?2").map_err(unavailable)?;
    for owner in &initially_kept {
        if history_selected.contains(owner) {
            continue;
        }
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
                    if let Some(to) = to.filter(|to| to != "you") {
                        keep.insert(to);
                    }
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
    let mut history_required_owners = BTreeSet::new();
    for key in history_selected.intersection(&needed) {
        let (row, selected_history) = selected_rows
            .get(key)
            .ok_or_else(|| unavailable("Selected History owner is unavailable"))?;
        if !selected_history {
            return Err(unavailable("Selected History owner changed cohorts"));
        }
        let window = history_window(row, cutoff).map_err(unavailable)?;
        if !window.member {
            return Err(unavailable("Selected History row is outside its cutoff"));
        }
        keep.extend(window.rows.keys().cloned());
        keep.extend(window.context.iter().cloned());
        history_required_owners.extend(window.context);
        edges.extend(window.edges);
        turns.extend(window.turns);
    }
    needed.extend(history_required_owners.iter().cloned());
    let mut rows = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut saved_rows = BTreeMap::new();
    let mut current_rows = BTreeMap::new();
    let mut content = BTreeMap::new();
    let mut witnesses = Vec::new();
    let mut header = db
        .prepare(
            "SELECT json_extract(metadata,'$.content_generation') FROM main.session_trace_headers WHERE namespace=semon_namespace() AND session_key=?1",
        )
        .map_err(unavailable)?;
    for key in needed {
        let (row, selected_history) = if let Some(row) = selected_rows.remove(&key) {
            row
        } else if history_required_owners.contains(&key) {
            (
                load_catalog_row(db, &key, true, bytes_left)?
                    .ok_or_else(|| unavailable("History relationship owner is unavailable"))?,
                true,
            )
        } else if current_graph_owners.contains(&key) {
            let Some(row) = load_catalog_row(db, &key, false, bytes_left)? else {
                continue;
            };
            (row, false)
        } else {
            continue;
        };
        #[cfg(test)]
        measured(1, 0, 0, 0);
        if row.key != key {
            return Err(unavailable("Catalog metadata identity changed"));
        }
        let current_lifecycle =
            !selected_history && row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
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
        let row_binding = if imported_origin
            || row
                .query
                .as_ref()
                .and_then(|query| query["runtime"].as_str())
                .is_some_and(|saved| saved.starts_with("source-runtime-v1:"))
        {
            Some(
                super::proof_generations::observation_binding(db, admitted_binding, &row)
                    .map_err(unavailable)?,
            )
        } else {
            runtime_binding.cloned()
        };
        // Retained Query rows keep their provenance checks above, but their
        // recorded summaries do not require a live runtime or source ledger.
        let row_runtime = if current_lifecycle {
            let cwds = super::catalog::runtime_source_cwds_if_current(db, &row.sources)
                .inspect_err(|_| {
                    if crate::native_timing::enabled() {
                        eprintln!("semon sessions native: observation_stage_refused boundary=query_current_source_cwds");
                    }
                })
                .map_err(unavailable)?;
            match cwds {
                Some(cwds) => {
                    let scoped = runtime_observations
                        .scope(&row, &cwds)
                        .map_err(unavailable)?;
                    Some(match row_binding {
                        Some(binding) => binding.witness(&scoped).map_err(unavailable)?,
                        None => scoped,
                    })
                }
                None if imported_origin => {
                    if crate::native_timing::enabled() {
                        eprintln!("semon sessions native: observation_stage_refused boundary=query_current_source_cwds");
                    }
                    return Err(unavailable(crate::storage::StorageError::ScopeChanged));
                }
                None => {
                    // Index can move past a published row. Reuse its captured
                    // observation only under the exact proof and live global fence.
                    let (binding, observations) =
                        super::proof_generations::observation_binding_with_observations(
                            db,
                            admitted_binding,
                            &row,
                        )
                        .map_err(unavailable)?;
                    let current_global = binding.witness(&runtime).map_err(unavailable)?;
                    if observations.runtime.as_deref() != Some(current_global.as_str()) {
                        return Err(unavailable(
                            "Native Query runtime observations changed during the query",
                        ));
                    }
                    let captured = observations
                        .runtime_for(&row)
                        .ok_or_else(|| unavailable("Exact Catalog query runtime is unavailable"))?;
                    Some(captured)
                }
            }
        } else {
            None
        };
        let saved = row
            .query
            .ok_or_else(|| unavailable("Exact Catalog query facts are unavailable"))?;
        if !crate::model::summary::query_observation_matches(
            &saved,
            &row.sources,
            current_lifecycle,
            row_runtime.as_deref(),
        )
        .map_err(unavailable)?
        {
            return Err(unavailable(
                "Native Query observations changed; retry after publication",
            ));
        }
        witnesses.push((key.clone(), saved.to_string(), selected_history));
        let summaries = saved["rows"]
            .as_object()
            .ok_or_else(|| unavailable("Invalid Catalog summaries"))?;
        if !summaries.contains_key(&key) {
            return Err(unavailable("Catalog summary ownership changed"));
        }
        for (id, saved) in summaries.iter().filter(|(id, _)| keep.contains(*id)) {
            let observed = saved.clone();
            let mut saved = saved.clone();
            if saved["summary"]["id"] != *id {
                return Err(unavailable("Catalog summary identity changed"));
            }
            if !current_lifecycle {
                saved["summary"]["alive"] = Value::Null;
                saved["summary"]["run"] = Value::Null;
                saved["summary"]["open_question"] = Value::Null;
                if matches!(
                    saved["summary"]["state"].as_str(),
                    Some("working" | "waiting-question")
                ) {
                    saved["summary"]["state"] = json!("unknown");
                }
            }
            if let Some(prior) = saved_rows.get(id) {
                if prior != &observed {
                    return Err(unavailable("Catalog relationship observations disagree"));
                }
                if current_lifecycle && !current_rows.get(id).copied().unwrap_or(false) {
                    current_rows.insert(id.clone(), true);
                    rows.insert(id.clone(), saved);
                    owners.insert(id.clone(), key.clone());
                }
            } else {
                saved_rows.insert(id.clone(), observed);
                current_rows.insert(id.clone(), current_lifecycle);
                rows.insert(id.clone(), saved);
                owners.insert(id.clone(), key.clone());
            }
        }
        let version = if selected_history {
            None
        } else {
            header
                .query_row([&key], |row| row.get::<_, Option<String>>(0))
                .optional()
                .map_err(unavailable)?
                .flatten()
        };
        if let Some(version) = version {
            content.insert(key, version);
        }
    }
    #[cfg(test)]
    measured(
        0,
        0,
        0,
        header.get_status(rusqlite::StatementStatus::VmStep),
    );
    drop(header);
    if keep.iter().any(|id| id != "you" && !rows.contains_key(id)) {
        return Err(unavailable("Catalog relationship context is unavailable"));
    }
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

fn has_search_slots(db: &Connection, key: &str, since: Option<i64>) -> Result<bool, QueryError> {
    let history: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.session_history_projections WHERE namespace=semon_namespace() AND session_key=?1)",
            [key],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    let table = if history {
        "session_history_slots"
    } else {
        "session_slots"
    };
    db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND (?2 IS NULL OR json_extract(metadata,'$.t')>=?2))"),rusqlite::params![key,since],|r|r.get(0)).map_err(unavailable)
}
