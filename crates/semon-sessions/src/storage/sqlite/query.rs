//! SQLite implementation of semantic Query metadata operations.
#[cfg(test)]
use crate::storage::query::measured;
use crate::storage::{
    LogicalSourceNamespace,
    query::{Part, QueryCatalogStore, history_window},
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
            let actual: Option<String> = db
                .query_row(
                    "SELECT CASE WHEN ?2 THEN json_extract(h.metadata,'$.query') ELSE json_extract(c.metadata,'$.query') END FROM (SELECT namespace,session_key FROM main.session_catalog UNION SELECT namespace,session_key FROM main.session_history_catalog) keys LEFT JOIN main.session_catalog c ON c.namespace=keys.namespace AND c.session_key=keys.session_key LEFT JOIN main.session_history_catalog h ON h.namespace=keys.namespace AND h.session_key=keys.session_key WHERE keys.namespace=semon_namespace() AND keys.session_key=?1",
                    rusqlite::params![key, selected_history],
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
    let base_sql = if cutoff.is_some() {
        "SELECT session_key FROM main.session_catalog WHERE namespace=semon_namespace() AND (last_ms>=?1 OR json_extract(metadata,'$.query.last')>=?1 OR (lifecycle='current' AND json_extract(metadata,'$.query.state') IN ('work','wait'))) UNION SELECT h.session_key FROM main.session_history_catalog h WHERE h.namespace=semon_namespace() AND (json_extract(h.metadata,'$.query.last')>=?1 OR json_extract(h.metadata,'$.query.state') IN ('work','wait') OR EXISTS(SELECT 1 FROM json_each(h.metadata,'$.query.selected.edges') q WHERE json_extract(q.value,'$.query.at')>=?1 OR json_extract(q.value,'$.query.done')>=?1 OR json_extract(q.value,'$.query.status') IN ('work','wait','new')))"
    } else {
        "SELECT session_key FROM main.session_catalog WHERE namespace=semon_namespace()"
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
    let mut history_selected = BTreeSet::new();
    let mut history_candidates = db
        .prepare(
            "SELECT h.session_key FROM main.session_history_catalog h LEFT JOIN main.session_catalog c ON c.namespace=h.namespace AND c.session_key=h.session_key WHERE h.namespace=semon_namespace() AND (?1 IS NULL OR (json_extract(h.metadata,'$.query.last')>=?1 OR json_extract(h.metadata,'$.query.state') IN ('work','wait') OR EXISTS(SELECT 1 FROM json_each(h.metadata,'$.query.selected.edges') q WHERE json_extract(q.value,'$.query.at')>=?1 OR json_extract(q.value,'$.query.done')>=?1 OR json_extract(q.value,'$.query.status') IN ('work','wait','new')))) AND (?1 IS NULL OR NOT (c.lifecycle='current' AND (c.last_ms>=?1 OR json_extract(c.metadata,'$.query.last')>=?1 OR json_extract(c.metadata,'$.query.state') IN ('work','wait'))))",
        )
        .map_err(unavailable)?;
    history_selected.extend(
        history_candidates
            .query_map(rusqlite::params![cutoff], |row| row.get::<_, String>(0))
            .map_err(unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(unavailable)?,
    );
    drop(history_candidates);
    let mut edges = BTreeSet::new();
    let mut turns = BTreeSet::new();
    let mut current_graph_owners = BTreeSet::new();
    let mut current_context = BTreeSet::new();
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
        current_context.insert(from.clone());
        keep.insert(from);
        if let Some(to) = to.filter(|to| to != "you") {
            current_context.insert(to.clone());
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
                    current_context.insert(from.clone());
                    keep.insert(from);
                    if let Some(to) = to.filter(|to| to != "you") {
                        current_context.insert(to.clone());
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
    let mut history_context = BTreeSet::new();
    let mut history_required_owners = BTreeSet::new();
    for key in history_selected.intersection(&needed) {
        if current_graph_owners.contains(key) {
            continue;
        }
        let text: Option<String> = db
            .query_row(
                "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_history_catalog WHERE namespace=semon_namespace() AND session_key=?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(unavailable)?
            .flatten();
        let text = text.ok_or_else(|| unavailable("History metadata exceeds its bound"))?;
        spend(bytes_left, text.len())?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&text).map_err(unavailable)?;
        if row.key != *key {
            return Err(unavailable("History metadata identity changed"));
        }
        let window = history_window(&row, cutoff).map_err(unavailable)?;
        if !window.member {
            return Err(unavailable("Selected History row is outside its cutoff"));
        }
        keep.extend(window.rows.keys().cloned());
        keep.extend(window.context.iter().cloned());
        history_context.extend(window.rows.keys().cloned());
        history_context.extend(window.context.iter().cloned());
        history_required_owners.extend(window.context);
        edges.extend(window.edges);
        turns.extend(window.turns);
    }
    if history_context.iter().any(|key| current_context.contains(key))
        || (cutoff.is_none() && current_context.iter().any(|key| history_selected.contains(key)))
        || (cutoff.is_some() && history_required_owners.iter().any(|key| current_context.contains(key)))
    {
        return Err(unavailable("Current and History relationship context select different owners"));
    }
    needed.extend(history_required_owners.iter().cloned());
    needed.extend(current_context.iter().cloned());
    let mut rows = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut content = BTreeMap::new();
    let mut witnesses = Vec::new();
    // In a finite query, select History only if that History row passed its
    // own cutoff; a recent Current row must not be replaced by older history.
    let mut metadata = db
        .prepare(
            "SELECT CASE WHEN ?2 THEN json_extract(hc.metadata,'$.lifecycle') ELSE c.lifecycle END,CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END,CASE WHEN octet_length(hc.metadata)<=1048576 THEN hc.metadata ELSE NULL END,CASE WHEN ?2 THEN NULL ELSE json_extract(h.metadata,'$.content_generation') END,?2 FROM (SELECT namespace,session_key FROM main.session_catalog UNION SELECT namespace,session_key FROM main.session_history_catalog) keys LEFT JOIN main.session_catalog c ON c.namespace=keys.namespace AND c.session_key=keys.session_key LEFT JOIN main.session_history_catalog hc ON hc.namespace=keys.namespace AND hc.session_key=keys.session_key LEFT JOIN main.session_trace_headers h ON h.namespace=keys.namespace AND h.session_key=keys.session_key WHERE keys.namespace=semon_namespace() AND keys.session_key=?1",
        )
        .map_err(unavailable)?;
    for key in needed {
        let selected_history = history_required_owners.contains(&key)
            || (history_selected.contains(&key)
                && !current_graph_owners.contains(&key)
                && (cutoff.is_none() || !current_context.contains(&key)));
        let record = metadata
            .query_row(rusqlite::params![&key, selected_history], |row| {
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
        let Some((lifecycle, current, history, version, selected_history)) = record else {
            continue;
        };
        #[cfg(test)]
        measured(1, 0, 0, 0);
        let text = if selected_history { history } else { current };
        let text = text.ok_or_else(|| unavailable("Catalog metadata exceeds its bound"))?;
        spend(bytes_left, text.len())?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&text).map_err(unavailable)?;
        if row.key != key {
            return Err(unavailable("Catalog metadata identity changed"));
        }
        // A History table row may preserve a still-Current source as well as
        // retained provenance, so its selected metadata supplies its lifecycle.
        if row.lifecycle.as_str() != lifecycle.as_str() {
            return Err(unavailable("Catalog lifecycle changed"));
        }
        let current_lifecycle = !selected_history && row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
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
            let cwds = super::catalog::runtime_source_cwds(db, &row.sources)
                .inspect_err(|_| {
                    if crate::native_timing::enabled() {
                        eprintln!("semon sessions native: observation_stage_refused boundary=query_current_source_cwds");
                    }
                })
                .map_err(unavailable)?;
            let scoped = runtime_observations
                .scope(&row, &cwds)
                .map_err(unavailable)?;
            Some(match row_binding {
                Some(binding) => binding.witness(&scoped).map_err(unavailable)?,
                None => scoped,
            })
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
