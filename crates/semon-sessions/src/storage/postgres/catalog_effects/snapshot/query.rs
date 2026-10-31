//! Private Query injection over an acknowledged generation. Cohorts merge
//! retained history exactly as the SQLite reader: history rows win per key.
use super::*;
use crate::{
    Options, QueryError,
    storage::{
        catalog::CatalogSnapshot,
        query::{Part, QueryCatalogStore, edge_timed, history_window, turn_kept},
    },
};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::storage::postgres) struct QueryCatalog {
    pub(in crate::storage::postgres) pool: PgPool,
    pub(in crate::storage::postgres) namespace: LogicalSourceNamespace,
    pub(in crate::storage::postgres) control: PgCancellation,
    pub(in crate::storage::postgres) generation: String,
    pub(in crate::storage::postgres) options: Options,
    pub(in crate::storage::postgres) captured: bool,
}

fn cohort_scope_changed(reason: &'static str) -> StorageError {
    #[cfg(test)]
    eprintln!("pg_query_cohort_refused reason={reason}");
    #[cfg(not(test))]
    let _ = reason;
    StorageError::ScopeChanged
}

fn current_runtime_owner<'a>(
    selected: &crate::model::summary::CatalogRow,
    selected_history: bool,
    current: Option<&'a crate::model::summary::CatalogRow>,
) -> Option<&'a crate::model::summary::CatalogRow> {
    current.filter(|current| {
        current.lifecycle == crate::model::summary::CatalogLifecycle::Current
            && (!selected_history || current.harness == selected.harness)
    })
}

struct EdgeSourceNeed {
    source: crate::model::summary::CatalogSource,
    owners: BTreeSet<String>,
}

fn edge_source_needs(
    row: &crate::model::summary::CatalogRow,
) -> Result<(BTreeSet<String>, Vec<EdgeSourceNeed>), StorageError> {
    let selected = row
        .query
        .as_ref()
        .ok_or(StorageError::ScopeChanged)?
        .get("selected");
    let edges = selected
        .and_then(|selected| selected.get("edges"))
        .and_then(serde_json::Value::as_array)
        .ok_or(StorageError::ScopeChanged)?;
    let mut owners = BTreeSet::new();
    let mut needs = Vec::<EdgeSourceNeed>::new();
    for edge in edges {
        let mut edge_owners = BTreeSet::from([row.key.clone()]);
        for field in ["from", "to"] {
            if let Some(owner) = edge["query"][field].as_str() {
                owners.insert(owner.to_owned());
                if owner != "you" {
                    edge_owners.insert(owner.to_owned());
                }
            }
        }
        for recipe in edge["query_content"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, recipe)| recipe)
            .filter(|recipe| !recipe.is_null())
        {
            let path = recipe["path"].as_str().ok_or(StorageError::ScopeChanged)?;
            let source: crate::model::summary::CatalogSource =
                serde_json::from_value(recipe["source"].clone())
                    .map_err(|_| StorageError::ScopeChanged)?;
            if source.path != std::path::Path::new(path) {
                return Err(StorageError::ScopeChanged);
            }
            if let Some(need) = needs.iter_mut().find(|need| need.source == source) {
                need.owners.extend(edge_owners.iter().cloned());
            } else {
                if needs.len() >= 64 {
                    return Err(StorageError::Busy);
                }
                needs.push(EdgeSourceNeed {
                    source,
                    owners: edge_owners.clone(),
                });
            }
        }
    }
    if owners.len() > 64 {
        return Err(StorageError::Busy);
    }
    Ok((owners, needs))
}

fn qualify_context_candidate(
    snapshot: &Snapshot<'_, '_>,
    options: &Options,
    row: &crate::model::summary::CatalogRow,
) -> Result<(), StorageError> {
    if row.sources.len() > 64 {
        return Err(StorageError::ScopeChanged);
    }
    let source_generation = crate::retention::source_generation(&row.sources)
        .map_err(|_| StorageError::ScopeChanged)?;
    if let Some(query) = &row.query
        && (query["source_generation"] != source_generation
            || !crate::model::summary::query_observation_matches(query, &row.sources, false, None)
                .map_err(|_| StorageError::ScopeChanged)?)
    {
        return Err(StorageError::ScopeChanged);
    }
    if snapshot
        .row_runtime_binding(row)
        .map_err(|_| StorageError::Unavailable)?
        .is_none()
    {
        return Err(StorageError::ScopeChanged);
    }
    for source in &row.sources {
        if crate::catalog::source_relative(options, &row.harness, source).is_none() {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

fn selected_edge_context(
    snapshot: &Snapshot<'_, '_>,
    options: &Options,
    row: &crate::model::summary::CatalogRow,
    history: bool,
) -> Result<Vec<crate::model::summary::CatalogRow>, StorageError> {
    let (_, needs) = edge_source_needs(row)?;
    let mut context = vec![row.clone()];
    let mut context_sources = BTreeMap::new();
    for source in &row.sources {
        if crate::catalog::source_relative(options, &row.harness, source).is_none() {
            return Err(StorageError::ScopeChanged);
        }
        if let Some(prior) = context_sources.insert(source.path.clone(), source.clone())
            && prior != *source
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    if context_sources.len() > 64 {
        return Err(StorageError::Busy);
    }

    let mut candidates =
        BTreeMap::<(String, bool), Option<crate::model::summary::CatalogRow>>::new();
    let mut qualified = BTreeSet::<(String, bool)>::new();
    for need in needs {
        match context_sources.get(&need.source.path) {
            Some(source) if source == &need.source => continue,
            Some(_) => return Err(StorageError::ScopeChanged),
            None => {}
        }

        let mut matched = false;
        for owner in &need.owners {
            for candidate_history in [history, !history] {
                let candidate_key = (owner.clone(), candidate_history);
                if !candidates.contains_key(&candidate_key) {
                    candidates.insert(
                        candidate_key.clone(),
                        snapshot.catalog(owner, candidate_history)?,
                    );
                }
                let Some(candidate) = candidates.get(&candidate_key).cloned().flatten() else {
                    continue;
                };
                if candidate.key != *owner
                    || !candidate.sources.contains(&need.source)
                    || crate::catalog::source_relative(options, &candidate.harness, &need.source)
                        .is_none()
                {
                    continue;
                }
                if qualified.insert(candidate_key.clone()) {
                    qualify_context_candidate(snapshot, options, &candidate)?;
                }

                let mut additions = Vec::new();
                let mut compatible = true;
                for source in &candidate.sources {
                    if let Some(prior) = context_sources.get(&source.path) {
                        if prior != source {
                            compatible = false;
                            break;
                        }
                    } else {
                        additions.push(source.clone());
                    }
                }
                if !compatible || context_sources.len() + additions.len() > 64 {
                    continue;
                }
                context.push(candidate.clone());
                for source in additions {
                    context_sources.insert(source.path.clone(), source);
                }
                matched = true;
                break;
            }
            if matched {
                break;
            }
        }
        if !matched {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(context)
}

/// Query includes retained-only owners without changing the Current Catalog
/// pager. Preserve each Current header for window/runtime membership before
/// choosing its proof-owned history, as the SQLite Query reader does.
/// The boolean records physical Catalog membership separately from lifecycle.
fn cohort_page(
    snapshot: &Snapshot<'_, '_>,
    after: Option<&(i64, Vec<u8>)>,
    cutoff: Option<i64>,
    bytes_left: &mut usize,
) -> Result<Vec<(crate::model::summary::CatalogRow, bool)>, StorageError> {
    snapshot
        .require_current()
        .map_err(|_| cohort_scope_changed("snapshot_not_current"))?;
    let tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
    tx.check()?;
    let namespace = tx.namespace.as_str().as_bytes();
    let after_last = after.map(|cursor| cursor.0);
    let after_key = after.map(|cursor| cursor.1.as_slice());
    let page = tx
        .driver
        .query(
            "WITH visible AS (
            SELECT session_key,metadata,last_ms,true AS catalog_member
            FROM semon_pg_native_catalog_rows r
            WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2
            UNION ALL
            SELECT o.session_key,h.metadata,o.last_ms,false
            FROM semon_pg_native_catalog_history_owners o
            LEFT JOIN semon_pg_native_catalog_history_headers h
              ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace
             AND h.generation=o.owner_generation AND h.key_digest=o.key_digest
             AND h.session_key=o.session_key
            WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2
              AND $5::bigint IS NULL
              AND NOT EXISTS (
                SELECT 1 FROM semon_pg_native_catalog_rows r
                WHERE r.namespace_digest=o.namespace_digest AND r.namespace=o.namespace
                  AND r.generation=o.generation AND r.key_digest=o.key_digest
                  AND r.session_key=o.session_key)
        ), page AS MATERIALIZED (
            SELECT session_key,metadata,last_ms,catalog_member FROM visible
            WHERE $3::bigint IS NULL OR last_ms<$3 OR (last_ms=$3 AND session_key>$4)
            ORDER BY last_ms DESC,session_key LIMIT 65
        ) SELECT session_key,
            CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END,
            last_ms,catalog_member FROM page ORDER BY last_ms DESC,session_key",
            &[
                &namespace,
                &snapshot.generation.as_bytes(),
                &after_last,
                &after_key,
                &cutoff,
            ],
        )
        .map_err(sqlstate::error)?;
    let mut output = Vec::new();
    for row in page {
        let bytes = row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?;
        let key = row.get::<_, Vec<u8>>(0);
        *bytes_left = bytes_left
            .checked_sub(64 + key.len() + bytes.len())
            .ok_or(StorageError::Busy)?;
        crate::selected_read::charge(64, bytes.len()).map_err(|_| StorageError::Busy)?;
        let metadata: crate::model::summary::CatalogRow = serde_json::from_slice(&bytes)
            .map_err(|_| cohort_scope_changed("page_metadata_invalid"))?;
        if metadata.key.as_bytes() != key
            || metadata.last.unwrap_or(0) != row.get::<_, i64>(2)
            || metadata.sources.len() > 64
        {
            return Err(cohort_scope_changed("page_metadata_witness_mismatch"));
        }
        output.push((metadata, row.get::<_, bool>(3)));
    }
    tx.check()?;
    Ok(output)
}

/// Page each finite History predicate on its own owner index before joining
/// immutable headers. Expired History never enters this request's scan.
fn finite_history_page(
    snapshot: &Snapshot<'_, '_>,
    cutoff: i64,
    active: bool,
    after: Option<&(i64, Vec<u8>)>,
    bytes_left: &mut usize,
) -> Result<Vec<(crate::model::summary::CatalogRow, i64, Vec<u8>)>, StorageError> {
    let page = finite_history_page_rows(snapshot, cutoff, active, after, *bytes_left)?;
    let mut output = Vec::new();
    for record in page {
        let key = record
            .get::<_, Option<Vec<u8>>>(0)
            .ok_or(StorageError::Busy)?;
        let metadata = record
            .get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::Busy)?;
        *bytes_left = bytes_left
            .checked_sub(64 + key.len() + metadata.len())
            .ok_or(StorageError::Busy)?;
        let row: crate::model::summary::CatalogRow = serde_json::from_slice(&metadata)
            .map_err(|_| cohort_scope_changed("finite_history_metadata_invalid"))?;
        let key_digest = record.get::<_, Vec<u8>>(3);
        if row.key.as_bytes() != key || digest(row.key.as_bytes()) != key_digest {
            return Err(cohort_scope_changed("finite_history_identity_mismatch"));
        }
        output.push((
            row,
            record.get::<_, Option<i64>>(2).unwrap_or(0),
            key_digest,
        ));
    }
    snapshot
        .tx
        .lock()
        .map_err(|_| StorageError::Unavailable)?
        .check()?;
    Ok(output)
}

fn finite_history_page_rows(
    snapshot: &Snapshot<'_, '_>,
    cutoff: i64,
    active: bool,
    after: Option<&(i64, Vec<u8>)>,
    bytes_left: usize,
) -> Result<Vec<tokio_postgres::Row>, StorageError> {
    snapshot
        .require_current()
        .map_err(|_| cohort_scope_changed("snapshot_not_current"))?;
    let tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
    tx.check()?;
    let request_budget = i64::try_from(bytes_left).map_err(|_| StorageError::Busy)?;
    let predicate = match (active, after.is_some()) {
        (true, false) => {
            "query_active=true AND $5::bytea IS NULL AND $3::bigint IS NOT NULL AND $4::bigint IS NULL"
        }
        (true, true) => {
            "query_active=true AND key_digest>$5 AND $3::bigint IS NOT NULL AND $4::bigint IS NULL"
        }
        (false, false) => "query_activity>=$3 AND $4::bigint IS NULL AND $5::bytea IS NULL",
        (false, true) => "query_activity>=$3 AND (query_activity,key_digest)>($4,$5)",
    };
    let order = if active {
        "key_digest"
    } else {
        "query_activity,key_digest"
    };
    let after_activity = if active {
        None
    } else {
        after.map(|cursor| cursor.0)
    };
    let output_order = if active {
        "p.key_digest"
    } else {
        "p.query_activity,p.key_digest"
    };
    let after_key = after.map(|cursor| cursor.1.as_slice());
    let page = tx.driver.query(&format!(
        "WITH page AS MATERIALIZED (
            SELECT session_key,key_digest,owner_generation,query_activity
            FROM semon_pg_native_catalog_history_owners
            WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {predicate}
            ORDER BY {order} LIMIT 65
        ), bounded AS MATERIALIZED (
            SELECT p.session_key,h.metadata,p.query_activity,p.key_digest,
                   count(*) OVER ()*64+sum(octet_length(p.session_key)+coalesce(octet_length(h.metadata),0)) OVER ()<=$6::bigint AS within_request_budget,
                   sum(octet_length(h.metadata)) OVER ()<=1048576 AS within_metadata_budget
            FROM page p LEFT JOIN semon_pg_native_catalog_history_headers h
              ON h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.generation=p.owner_generation
             AND h.key_digest=p.key_digest AND h.session_key=p.session_key
        ) SELECT CASE WHEN octet_length(session_key)<=1048576 AND within_request_budget THEN session_key END,
                 CASE WHEN within_request_budget AND within_metadata_budget THEN metadata END,
                 query_activity,key_digest
        FROM bounded p
        ORDER BY {output_order}"
    ), &[&tx.namespace.as_str().as_bytes(), &snapshot.generation.as_bytes(), &cutoff, &after_activity, &after_key, &request_budget]).map_err(sqlstate::error)?;
    tx.check()?;
    Ok(page)
}

fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("unavailable", error.to_string())
}

fn charge_catalog_row(
    row: &crate::model::summary::CatalogRow,
    bytes_left: &mut usize,
) -> Result<(), StorageError> {
    let metadata = serde_json::to_vec(row).map_err(|_| StorageError::ScopeChanged)?;
    *bytes_left = bytes_left
        .checked_sub(64 + row.key.len() + metadata.len())
        .ok_or(StorageError::Busy)?;
    Ok(())
}

fn retained_window(
    row: &crate::model::summary::CatalogRow,
    generation: &str,
    cutoff: i64,
) -> Result<(Part, bool), StorageError> {
    let selected = history_window(row, Some(cutoff)).map_err(cohort_scope_changed)?;
    Ok((
        Part {
            generation: generation.to_owned(),
            runtime: row
                .query
                .as_ref()
                .and_then(|saved| saved["runtime"].as_str())
                .unwrap_or_default()
                .to_owned(),
            rows: selected.rows,
            owners: BTreeMap::new(),
            content: BTreeMap::new(),
            witnesses: Vec::new(),
            edges: selected.edges,
            turns: selected.turns,
        },
        selected.member,
    ))
}

impl QueryCatalogStore for QueryCatalog {
    fn selected_body(
        &self,
        namespace: &LogicalSourceNamespace,
        key: &str,
        args: &serde_json::Value,
        now: i64,
        host: &dyn crate::SessionSourceReader,
    ) -> Result<serde_json::Value, QueryError> {
        if namespace != &self.namespace {
            return Err(unavailable(StorageError::NamespaceUpgradePending));
        }
        if self.captured && !self.options.all {
            return Err(unavailable(StorageError::InvalidArguments));
        }
        let needs_window = !self.options.all && !self.options.scan_window;
        let (selected_current, retained_window_data) = with_admitted_snapshot(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.generation,
            |snapshot| {
                let history = snapshot.owner(key)?.is_some();
                let selected = snapshot.catalog(key, history)?;
                let current_row = snapshot.catalog(key, false)?;
                let current = selected
                    .as_ref()
                    .and_then(|selected| {
                        current_runtime_owner(selected, history, current_row.as_ref())
                    })
                    .is_some();
                let retained_window_data = if needs_window && !current {
                    snapshot
                        .selected(key, history, 0, 1)?
                        .map(|(row, _, _)| {
                            snapshot
                                .row_runtime_binding(&row)
                                .map_err(|_| StorageError::ScopeChanged)?
                                .ok_or(StorageError::ScopeChanged)?;
                            let saved = row.query.as_ref().ok_or(StorageError::ScopeChanged)?;
                            if !crate::model::summary::query_observation_matches(
                                saved,
                                &row.sources,
                                false,
                                None,
                            )
                            .map_err(|_| StorageError::ScopeChanged)?
                            {
                                return Err(StorageError::ScopeChanged);
                            }
                            retained_window(
                                &row,
                                &self.generation,
                                now.saturating_sub(
                                    i64::try_from(self.options.since.as_millis())
                                        .unwrap_or(i64::MAX),
                                ),
                            )
                        })
                        .transpose()?
                } else {
                    None
                };
                Ok((current, retained_window_data))
            },
        )
        .map_err(unavailable)?;
        let runtime_observation = if !self.captured && selected_current {
            Some(
                super::super::super::current_sources::Runtime::open(
                    &self.pool,
                    &self.namespace,
                    &self.control,
                    &self.options,
                )
                .map_err(unavailable)?,
            )
        } else {
            None
        };
        let runtime = runtime_observation
            .as_ref()
            .map(|observation| observation.generation(&self.options))
            .transpose()
            .map_err(|error| QueryError::io(&error))?;
        // Current owners use the qualified cohort window. A retained-only
        // owner derives its cutoff from the exact history row admitted above.
        let window = if needs_window && selected_current {
            let part = self.read_cohort(namespace, now, &mut (8 * 1024 * 1024))?;
            if part.generation != self.generation
                || Some(part.runtime.as_str()) != runtime.as_deref()
            {
                return Err(unavailable(StorageError::ScopeChanged));
            }
            if !part.rows.contains_key(key) {
                return Err(QueryError::unknown_session(key));
            }
            Some(part)
        } else if needs_window {
            let Some((part, member)) = retained_window_data else {
                return Err(QueryError::unknown_session(key));
            };
            if !member {
                return Err(QueryError::unknown_session(key));
            }
            Some(part)
        } else {
            None
        };
        with_admitted_snapshot(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.generation,
            |snapshot| {
                // The retained composite wins for the same owner, as in the
                // SQLite Query reader; live qualification is independent.
                // Bodies still come only from authorized Raw Source bytes.
                let history = snapshot.owner(key)?.is_some();
                let (row, header, _) = snapshot
                    .selected(key, history, 0, 1)
                    .inspect_err(|_error| {
                        #[cfg(test)]
                        eprintln!("pg_selected_snapshot_lookup outcome=refused");
                    })?
                    .ok_or(StorageError::ScopeChanged)?;
                let current_row = snapshot.catalog(key, false)?;
                let current_row = current_runtime_owner(&row, history, current_row.as_ref());
                let current = current_row.is_some();
                // The admitted generation is immutable. Rechecking the exact
                // selected owner keeps pre-window classification tied to this
                // snapshot and does not borrow another harness's runtime.
                if current != selected_current {
                    return Err(StorageError::ScopeChanged);
                }
                if current
                    && let (Some(window), Some(runtime)) = (&window, runtime.as_deref())
                    && window.runtime != runtime
                {
                    return Err(StorageError::ScopeChanged);
                }
                // History owns the selected source vector and publication
                // receipt. A still-Current owner supplies the separate live
                // runtime observation for that same harness identity.
                snapshot
                    .row_runtime_binding(&row)
                    .map_err(|_| StorageError::ScopeChanged)?
                    .ok_or(StorageError::ScopeChanged)?;
                let selected_saved = row.query.as_ref().ok_or(StorageError::ScopeChanged)?;
                if !crate::model::summary::query_observation_matches(
                    selected_saved,
                    &row.sources,
                    false,
                    None,
                )
                .map_err(|_| StorageError::ScopeChanged)?
                {
                    return Err(StorageError::ScopeChanged);
                }
                if !self.captured && current {
                    let current_row = current_row.ok_or(StorageError::ScopeChanged)?;
                    let runtime = runtime.as_deref().ok_or(StorageError::ScopeChanged)?;
                    let saved = current_row
                        .query
                        .as_ref()
                        .ok_or(StorageError::ScopeChanged)?;
                    let expected = snapshot
                        .runtime_observation(&self.options, current_row, runtime)
                        .map_err(|_| StorageError::ScopeChanged)?
                        .ok_or(StorageError::ScopeChanged)?;
                    if !crate::model::summary::query_observation_matches(
                        saved,
                        &current_row.sources,
                        true,
                        Some(expected.as_str()),
                    )
                    .map_err(|_| StorageError::ScopeChanged)?
                    {
                        return Err(StorageError::ScopeChanged);
                    }
                }
                let total = usize::try_from(header.3).map_err(|_| StorageError::ScopeChanged)?;
                let observation: crate::slot_projection::QueryObservation = serde_json::from_value(
                    row.query.as_ref().ok_or(StorageError::ScopeChanged)?["transcript_observation"]
                        .clone(),
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                if !observation.valid(total) {
                    return Err(StorageError::ScopeChanged);
                }
                let context = if args["_session"] == true {
                    selected_edge_context(snapshot, &self.options, &row, history)?
                } else {
                    vec![row.clone()]
                };
                let mut sources = Vec::new();
                let mut paths = BTreeMap::new();
                for owner in &context {
                    for source in &owner.sources {
                        let path =
                            crate::catalog::source_relative(&self.options, &owner.harness, source)
                                .ok_or(StorageError::ScopeChanged)?;
                        if let Some(prior) = paths.insert(source.path.clone(), source) {
                            if prior != source {
                                return Err(StorageError::ScopeChanged);
                            }
                            continue;
                        }
                        sources.push((
                            source,
                            crate::SessionSourceRef {
                                root: owner.harness.clone(),
                                path,
                                native_id: source.native_id.clone(),
                                offset: source.offset,
                                prefix_sha256: source.prefix_sha256,
                                tail_sha256: source.tail_sha256,
                            },
                        ));
                    }
                }
                if sources.is_empty() || sources.len() > 64 {
                    return Err(StorageError::Busy);
                }
                let records = super::super::selected_reader::SelectedRecords {
                    tx: &snapshot.tx,
                    generation: &self.generation,
                    host,
                    sources,
                    pins: std::sync::Mutex::new(BTreeMap::new()),
                    bytes_left: std::sync::Mutex::new(64 * 1024 * 1024),
                };
                if args["_session"] == true {
                    let mut summary = crate::query::transcript::render_session(
                        &self.options,
                        key,
                        current,
                        &context,
                        &records,
                        now,
                        &|turn| {
                            snapshot
                                .query_first_slot(key, turn, history)
                                .map(|(_, slot)| slot)
                                .map_err(unavailable)
                        },
                        &|turn| {
                            Ok(window.as_ref().is_none_or(|part| {
                                turn["id"]
                                    .as_str()
                                    .is_some_and(|id| part.turns.contains(id))
                            }))
                        },
                        &|id| Ok(window.as_ref().is_none_or(|part| part.edges.contains(id))),
                        &|id| {
                            Ok(window
                                .as_ref()
                                .is_none_or(|part| part.rows.contains_key(id)))
                        },
                    )
                    .map_err(|error| {
                        #[cfg(test)]
                        eprintln!("pg_selected_render_session outcome=refused");
                        let _ = error;
                        StorageError::Unavailable
                    })?;
                    if let Some(machine) = args["_machine_alias"].as_str() {
                        summary["machine"] = serde_json::json!(machine);
                    }
                    records.finish().map_err(|_| StorageError::ScopeChanged)?;
                    if runtime_observation
                        .as_ref()
                        .map(|observation| observation.generation(&self.options))
                        .transpose()
                        .map_err(|_| StorageError::Unavailable)?
                        != runtime
                    {
                        return Err(StorageError::ScopeChanged);
                    }
                    return Ok(serde_json::json!({"now":now,"session":summary}));
                }
                let anchor = crate::tx::Anchor::of(
                    args["before"].as_u64().map(|v| v as usize),
                    args["after"].as_u64().map(|v| v as usize),
                    args["turn"].as_str().map(str::to_owned),
                )
                .ok_or(StorageError::InvalidArguments)?;
                let (forward, from) = match anchor {
                    crate::tx::Anchor::Last => (false, total),
                    crate::tx::Anchor::Before(at) => (false, at.min(total)),
                    crate::tx::Anchor::After(at) => (true, at.min(total)),
                    crate::tx::Anchor::Turn(turn) => {
                        (true, snapshot.query_first_slot(key, &turn, history)?.0)
                    }
                };
                let page = PgPage {
                    snapshot,
                    key,
                    history,
                    records: &records,
                };
                let answer = crate::query::transcript::render_page(
                    &self.options,
                    key,
                    &row,
                    &observation,
                    total,
                    forward,
                    from,
                    args["limit"]
                        .as_u64()
                        .map_or(crate::tx::PAGE_ENTRIES, |v| v as usize),
                    now,
                    &page,
                )
                .map_err(|error| {
                    #[cfg(test)]
                    eprintln!("pg_selected_render_page outcome=refused");
                    let _ = error;
                    StorageError::Unavailable
                })?;
                records.finish().map_err(|error| {
                    #[cfg(test)]
                    eprintln!("pg_selected_records_finish outcome=refused");
                    let _ = error;
                    StorageError::ScopeChanged
                })?;
                if runtime_observation
                    .as_ref()
                    .map(|observation| observation.generation(&self.options))
                    .transpose()
                    .map_err(|_| StorageError::Unavailable)?
                    != runtime
                {
                    return Err(StorageError::ScopeChanged);
                }
                Ok(answer)
            },
        )
        .map_err(unavailable)
    }
    fn read_cohort(
        &self,
        namespace: &LogicalSourceNamespace,
        now: i64,
        bytes_left: &mut usize,
    ) -> Result<Part, QueryError> {
        if namespace != &self.namespace {
            return Err(unavailable(StorageError::NamespaceUpgradePending));
        }
        // The recent window uses the SQLite reader's cutoff rules.
        let cutoff = (!self.options.all).then(|| {
            now.saturating_sub(i64::try_from(self.options.since.as_millis()).unwrap_or(i64::MAX))
        });
        let runtime_observation = super::super::super::current_sources::Runtime::open(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.options,
        )
        .map_err(unavailable)?;
        let runtime = runtime_observation
            .generation(&self.options)
            .map_err(|error| QueryError::io(&error))?;
        with_admitted_snapshot(&self.pool, namespace, &self.control, &self.generation, |snapshot| {
            let mut rows = BTreeMap::new();
            let mut owners = BTreeMap::new();
            let mut saved_rows = BTreeMap::new();
            let mut current_rows = BTreeMap::new();
            let mut witnesses = Vec::new();
            let mut content = BTreeMap::new();
            let mut emit = |row: crate::model::summary::CatalogRow, current: bool, selected_history: bool, keep: Option<&BTreeSet<String>>, bytes_left: &mut usize| -> Result<(), StorageError> {
                if row.sources.is_empty() || row.sources.len() > 64 || row.sources.iter().any(|source| crate::catalog::source_relative(&self.options, &row.harness, source).is_none()) { return Err(cohort_scope_changed("query_source_outside_authority")); }
                let expected = if current {
                    snapshot.runtime_observation(&self.options, &row, &runtime)
                        .map_err(|_| cohort_scope_changed("query_runtime_observation_refused"))?
                } else {
                    snapshot.row_runtime_binding(&row)
                        .map_err(|_| cohort_scope_changed("query_runtime_binding_refused"))?
                        .ok_or_else(|| cohort_scope_changed("query_runtime_binding_missing"))?;
                    None
                };
                let saved = row.query.as_ref().ok_or_else(|| cohort_scope_changed("query_observation_missing"))?;
                let matches = crate::model::summary::query_observation_matches(saved, &row.sources, current, expected.as_deref()).map_err(|_| cohort_scope_changed("query_observation_invalid"))?;
                if !matches { return Err(cohort_scope_changed("query_observation_mismatch")); }
                let witness = saved.to_string();
                *bytes_left = bytes_left.checked_sub(witness.len()).ok_or(StorageError::Busy)?;
                let summaries = saved["rows"].as_object().ok_or_else(|| cohort_scope_changed("query_summaries_missing"))?;
                if !summaries.contains_key(&row.key) { return Err(cohort_scope_changed("query_owner_summary_missing")); }
                for (id, value) in summaries.iter().filter(|(id, _)| keep.is_none_or(|keep| keep.contains(*id))) {
                    if value["summary"]["id"] != *id { return Err(cohort_scope_changed("query_relationship_summary_mismatch")); }
                    let observed = value.clone();
                    if saved_rows.get(id).is_some_and(|prior| prior != &observed) { return Err(cohort_scope_changed("query_relationship_summary_mismatch")); }
                    let mut value = value.clone();
                    if !current {
                        value["summary"]["alive"] = serde_json::Value::Null;
                        value["summary"]["run"] = serde_json::Value::Null;
                        value["summary"]["open_question"] = serde_json::Value::Null;
                        if matches!(value["summary"]["state"].as_str(), Some("working" | "waiting-question")) {
                            value["summary"]["state"] = serde_json::json!("unknown");
                        }
                    }
                    if saved_rows.contains_key(id) {
                        if current && !current_rows.get(id).copied().unwrap_or(false) {
                            current_rows.insert(id.clone(), true);
                            rows.insert(id.clone(), value);
                            owners.insert(id.clone(), row.key.clone());
                        }
                    } else {
                        saved_rows.insert(id.clone(), observed);
                        current_rows.insert(id.clone(), current);
                        rows.insert(id.clone(), value);
                        owners.insert(id.clone(), row.key.clone());
                    }
                }
                witnesses.push((row.key.clone(), witness, selected_history));
                // The preview header is a Current trace witness. History has
                // no retained overview summary from which to derive this hint.
                if !selected_history {
                    use crate::storage::selected::ContextLookup;
                    let version = snapshot.preview_header(&row.key).map_err(|_| StorageError::Unavailable)?;
                    if let Some((_, Some(version))) = version { content.insert(row.key, version); }
                }
                Ok(())
            };
            // As the SQLite reader, retained history wins for the same owner.
            let chosen = |row: crate::model::summary::CatalogRow, bytes_left: &mut usize| -> Result<(crate::model::summary::CatalogRow, bool), StorageError> {
                Ok(match snapshot.history_catalog(&row.key).map_err(|_| StorageError::Unavailable)? {
                    Some(text) => {
                        *bytes_left = bytes_left.checked_sub(64 + row.key.len() + text.len()).ok_or(StorageError::Busy)?;
                        let history: crate::model::summary::CatalogRow = serde_json::from_str(&text).map_err(|_| cohort_scope_changed("history_metadata_invalid"))?;
                        if history.key != row.key { return Err(cohort_scope_changed("history_identity_mismatch")); }
                        (history, true)
                    }
                    None => (row, false),
                })
            };
            let query_last = |row: &crate::model::summary::CatalogRow| row.query.as_ref().and_then(|query| query["last"].as_i64());
            let mut loaded = BTreeMap::new();
            let mut after = None;
            loop {
                let mut catalog = cohort_page(snapshot, after.as_ref(), cutoff, bytes_left)?;
                let more = catalog.len() > 64;
                catalog.truncate(64);
                let next = catalog.last().map(|(row,_)| (row.last.unwrap_or(0),row.key.as_bytes().to_vec()));
                for (row,catalog_member) in catalog {
                    let current_member = catalog_member && row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
                    let Some(c) = cutoff else {
                            let (selected, selected_history) = if catalog_member { chosen(row, bytes_left)? } else { (row,true) };
                            let current = !selected_history && selected.lifecycle == crate::model::summary::CatalogLifecycle::Current;
                            loaded.insert(selected.key.clone(), (selected,current,selected_history));
                        continue;
                    };
                    // The chosen history may have older timestamps than its
                    // Current header. Only physical Catalog members qualify
                    // by header time or live state; retained-only owners use
                    // their saved Query time below.
                    let state = current_member && row.query.as_ref().and_then(|query| query["state"].as_str()).is_some_and(|state| matches!(state, "work" | "wait"));
                    let current_recent = current_member && (row.last.is_some_and(|last| last >= c) || query_last(&row).is_some_and(|last| last >= c) || state);
                    if current_recent {
                        loaded.insert(row.key.clone(), (row,true,false));
                    }
                }
                if !more { break; }
                after = Some(next.ok_or_else(|| cohort_scope_changed("catalog_page_cursor_missing"))?);
            }
            if let Some(c) = cutoff {
                for active in [false, true] {
                    let mut after = None;
                    loop {
                        let mut page = finite_history_page(snapshot, c, active, after.as_ref(), bytes_left)?;
                        let more = page.len() > 64;
                        page.truncate(64);
                        let next = page.last().map(|(_, activity, key_digest)| (*activity, key_digest.clone()));
                        for (row, _, _) in page {
                            if loaded.get(&row.key).is_some_and(|(_, current, _)| *current) {
                                continue;
                            }
                            if !history_window(&row, Some(c)).map_err(cohort_scope_changed)?.member {
                                return Err(cohort_scope_changed("history_activity_projection_mismatch"));
                            }
                            loaded.insert(row.key.clone(), (row, false, true));
                        }
                        if !more { break; }
                        after = Some(next.ok_or_else(|| cohort_scope_changed("history_page_cursor_missing"))?);
                    }
                }
            }
            let mut history_edges = BTreeSet::new();
            let mut history_turns = BTreeSet::new();
            let mut history_context = BTreeSet::new();
            let mut history_required_owners = BTreeSet::new();
            for (row, _, selected_history) in loaded.values() {
                if !selected_history { continue; }
                let window = history_window(row, cutoff).map_err(cohort_scope_changed)?;
                if !window.member { return Err(cohort_scope_changed("selected_history_outside_window")); }
                history_edges.extend(window.edges);
                history_turns.extend(window.turns);
                history_context.extend(window.rows.keys().cloned());
                history_context.extend(window.context.iter().cloned());
                history_required_owners.extend(window.context);
            }
            for key in &history_required_owners {
                if let Some(text) = snapshot.history_catalog(key).map_err(|_| StorageError::Unavailable)? {
                    *bytes_left = bytes_left.checked_sub(64 + key.len() + text.len()).ok_or(StorageError::Busy)?;
                    let row: crate::model::summary::CatalogRow = serde_json::from_str(&text)
                        .map_err(|_| cohort_scope_changed("history_context_metadata_invalid"))?;
                    if row.key != *key { return Err(cohort_scope_changed("history_context_identity_mismatch")); }
                    if !loaded.get(key).is_some_and(|(_, current, _)| *current) {
                        loaded.insert(key.clone(), (row, false, true));
                    }
                } else if !loaded.contains_key(key)
                    && let Some(row) = snapshot.catalog(key, false).map_err(|_| StorageError::Unavailable)? {
                    charge_catalog_row(&row, bytes_left)?;
                    let current = row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
                    loaded.insert(key.clone(), (row, current, false));
                }
            }
            let Some(c) = cutoff else {
                // An immutable owner's relationship summaries may still name
                // a withdrawn peer. Membership comes from this cohort, as in
                // the SQLite reader, rather than from embedded old summaries.
                let mut keep = loaded.keys().cloned().collect::<BTreeSet<_>>();
                keep.extend(history_context.iter().cloned());
                // Each saved owner contains its complete relationship summaries.
                // Exact edge/turn sets are loaded below from the immutable effects.
                let mut current_context = BTreeSet::new();
                let (mut current_edges, mut current_turns) = {
                    let tx = snapshot.tx.lock().map_err(|_|StorageError::Unavailable)?;
                    let mut sets = Vec::new();
                    for (table, column) in [("semon_pg_native_catalog_trace_edges", "edge_id"), ("semon_pg_native_catalog_trace_turns", "turn_id")] {
                        let digest_column = if column == "edge_id" { "edge_digest" } else { "turn_digest" };
                        let mut after: Option<(Vec<u8>, Vec<u8>)> = None;
                        let mut values = BTreeSet::new();
                        loop {
                            tx.check()?;
                            let after_key = after.as_ref().map(|v| v.0.as_slice());
                            let after_id = after.as_ref().map(|v| v.1.as_slice());
                            let page = tx.driver.query(&format!("SELECT key_digest,{digest_column},CASE WHEN octet_length(session_key)<=65536 THEN session_key END,CASE WHEN octet_length({column})<=65536 THEN {column} END,CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND ($3::bytea IS NULL OR (key_digest,{digest_column})>($3,$4)) ORDER BY key_digest,{digest_column} LIMIT 64"), &[&namespace.as_str().as_bytes(), &self.generation.as_bytes(), &after_key, &after_id]).map_err(sqlstate::error)?;
                            for row in &page {
                                let owner = text(row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?)?;
                                if !loaded.get(&owner).is_some_and(|(_, _, history)| !*history) { continue; }
                                let value = text(row.get::<_, Option<Vec<u8>>>(3).ok_or(StorageError::Busy)?)?;
                                *bytes_left = bytes_left.checked_sub(value.len()).ok_or(StorageError::Busy)?;
                                if column == "edge_id" {
                                    let bytes = row.get::<_, Option<Vec<u8>>>(4).ok_or(StorageError::Busy)?;
                                    *bytes_left = bytes_left.checked_sub(bytes.len()).ok_or(StorageError::Busy)?;
                                    let metadata: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| cohort_scope_changed("all_time_edge_metadata_invalid"))?;
                                    let from = metadata["from"].as_str().ok_or_else(|| cohort_scope_changed("all_time_edge_source_missing"))?;
                                    keep.insert(from.to_owned());
                                    current_context.insert(from.to_owned());
                                    if let Some(to) = metadata["to"].as_str().filter(|to| *to != "you") {
                                        keep.insert(to.to_owned());
                                        current_context.insert(to.to_owned());
                                    }
                                }
                                values.insert(value);
                            }
                            if page.len() < 64 { break; }
                            after = page.last().map(|row| (row.get(0), row.get(1)));
                        }
                        sets.push(values);
                    }
                    (sets.remove(0), sets.remove(0))
                };
                current_edges.extend(history_edges);
                current_turns.extend(history_turns);
                for (row,current,selected_history) in loaded.into_values() { emit(row, current, selected_history, Some(&keep), bytes_left)?; }
                if keep.iter().any(|id| id != "you" && !rows.contains_key(id)) {
                    return Err(cohort_scope_changed("all_time_relationship_owner_missing"));
                }
                return Ok(Part { generation: self.generation.clone(), runtime: runtime.clone(), rows, owners, content, witnesses, edges: current_edges, turns: current_turns });
            };
            // Recent window: edges active or touched since the cutoff, then the
            // last/recent turns of each kept owner and their linked endpoints.
            let mut keep: BTreeSet<String> = loaded.keys().cloned().collect();
            keep.extend(history_context.iter().cloned());
            let mut needed = keep.clone();
            needed.retain(|key| loaded.contains_key(key));
            needed.extend(history_context.iter().cloned());
            let mut edges = BTreeSet::new();
            let mut turns = BTreeSet::new();
            let mut current_graph_owners = BTreeSet::new();
            let mut current_context = BTreeSet::new();
            let json = |bytes: Option<Vec<u8>>| -> Result<serde_json::Value, StorageError> {
                serde_json::from_slice(&bytes.ok_or(StorageError::Busy)?)
                    .map_err(|_| cohort_scope_changed("recent_relationship_metadata_invalid"))
            };
            let endpoint = |metadata: &serde_json::Value| -> Result<(String, Option<String>), StorageError> {
                let from = metadata["from"].as_str().ok_or_else(|| cohort_scope_changed("recent_relationship_source_missing"))?.to_owned();
                Ok((from, metadata["to"].as_str().filter(|to| *to != "you").map(str::to_owned)))
            };
            {
                let tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
                let mut after: Option<(Vec<u8>, Vec<u8>)> = None;
                loop {
                    tx.check()?;
                    let after_key = after.as_ref().map(|v| v.0.as_slice());
                    let after_id = after.as_ref().map(|v| v.1.as_slice());
                    let page = tx.driver.query("SELECT key_digest,edge_digest,CASE WHEN octet_length(session_key)<=65536 AND octet_length(edge_id)<=65536 AND octet_length(metadata)<=65536 THEN session_key END,edge_id,metadata FROM semon_pg_native_catalog_trace_edges WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND ($3::bytea IS NULL OR (key_digest,edge_digest)>($3,$4)) ORDER BY key_digest,edge_digest LIMIT 64", &[&namespace.as_str().as_bytes(), &self.generation.as_bytes(), &after_key, &after_id]).map_err(sqlstate::error)?;
                    for row in &page {
                        let owner = text(row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?)?;
                        if loaded.get(&owner).is_some_and(|(_, _, history)| *history) { continue; }
                        let id = text(row.get(3))?;
                        let metadata = json(row.get(4))?;
                        if !edge_timed(&metadata, c) { continue; }
                        let (from, to) = endpoint(&metadata)?;
                        *bytes_left = bytes_left.checked_sub(owner.len() + id.len() + from.len() + to.as_ref().map_or(0, String::len)).ok_or(StorageError::Busy)?;
                        current_graph_owners.insert(owner.clone());
                        needed.insert(owner);
                        edges.insert(id);
                        keep.insert(from.clone());
                        current_context.extend(to.iter().cloned());
                        current_context.insert(from.clone());
                        keep.extend(to);
                    }
                    if page.len() < 64 { break; }
                    after = page.last().map(|row| (row.get(0), row.get(1)));
                }
            }
            let timed_edges = edges.clone();
            for owner in keep.clone() {
                if loaded.get(&owner).is_some_and(|(_, _, history)| *history) { continue; }
                let mut links = Vec::new();
                {
                    let tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
                    let mut after: Option<Vec<u8>> = None;
                    loop {
                        tx.check()?;
                        let page = tx.driver.query("SELECT turn_digest,CASE WHEN octet_length(turn_id)<=65536 AND octet_length(metadata)<=65536 THEN turn_id END,metadata FROM semon_pg_native_catalog_trace_turns WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND ($4::bytea IS NULL OR turn_digest>$4) ORDER BY turn_digest LIMIT 64", &[&namespace.as_str().as_bytes(), &self.generation.as_bytes(), &owner.as_bytes(), &after]).map_err(sqlstate::error)?;
                        for row in &page {
                            let id = text(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?)?;
                            let metadata = json(row.get(2))?;
                            if !turn_kept(&metadata, c, &timed_edges) { continue; }
                            let start = metadata["start"].as_str().map(str::to_owned);
                            let sent: Vec<String> = metadata["sent"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
                            *bytes_left = bytes_left.checked_sub(id.len() + start.as_ref().map_or(0, String::len) + sent.iter().map(String::len).sum::<usize>()).ok_or(StorageError::Busy)?;
                            turns.insert(id);
                            links.extend(start.into_iter().chain(sent));
                        }
                        if page.len() < 64 { break; }
                        after = page.last().map(|row| row.get(0));
                    }
                }
                use crate::storage::selected::ContextLookup;
                for id in links {
                    if edges.insert(id.clone())
                        && let Some(metadata) = snapshot.edge(&owner, &id).map_err(|_| StorageError::Unavailable)?
                    {
                        let metadata = serde_json::from_str(&metadata)
                            .map_err(|_| cohort_scope_changed("recent_turn_edge_metadata_invalid"))?;
                        let (from, to) = endpoint(&metadata)?;
                        current_context.insert(from.clone());
                        keep.insert(from);
                        current_context.extend(to.iter().cloned());
                        keep.extend(to);
                    }
                }
            }
            if cutoff.is_some() {
                for key in &current_context {
                    if loaded.get(key).is_some_and(|(_, _, history)| *history)
                        && let Some(row) = snapshot.catalog(key, false).map_err(|error| match error {
                            StorageError::ScopeChanged => cohort_scope_changed("recent_current_context_refused"),
                            other => other,
                        })? {
                        charge_catalog_row(&row, bytes_left)?;
                        let current = row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
                        let eligible = current
                            && (row.last.is_some_and(|last| last >= c)
                                || query_last(&row).is_some_and(|last| last >= c)
                                || row.query.as_ref().and_then(|query| query["state"].as_str()).is_some_and(|state| matches!(state, "work" | "wait")));
                        if eligible {
                            loaded.insert(key.clone(), (row, true, false));
                        }
                    }
                }
            }
            needed.extend(current_context.iter().cloned());
            edges.extend(history_edges);
            turns.extend(history_turns);
            for key in &needed {
                let (row,current,selected_history) = match loaded.remove(key) {
                    Some(row) => row,
                    None if history_required_owners.contains(key) => continue,
                    None => match snapshot.catalog(key, false).map_err(|error| match error {
                        StorageError::ScopeChanged => cohort_scope_changed("recent_current_catalog_refused"),
                        other => other,
                    })? {
                        Some(row) => {
                            charge_catalog_row(&row, bytes_left)?;
                            let current = row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
                            (row,current,false)
                        },
                        None if current_graph_owners.contains(key) => continue,
                        None => continue,
                    },
                };
                emit(row, current, selected_history, Some(&keep), bytes_left)?;
            }
            if keep.iter().any(|id| id != "you" && !rows.contains_key(id)) {
                return Err(cohort_scope_changed("recent_relationship_owner_missing"));
            }
            Ok(Part { generation: self.generation.clone(), runtime: runtime.clone(), rows, owners, content, witnesses, edges, turns })
        }).map_err(unavailable)
    }
    fn has_search_slots(
        &self,
        namespace: &LogicalSourceNamespace,
        key: &str,
        since: Option<i64>,
    ) -> Result<bool, QueryError> {
        if namespace != &self.namespace {
            return Err(unavailable(StorageError::ScopeChanged));
        }
        with_generation(
            &self.pool,
            namespace,
            &self.control,
            &self.generation,
            |snapshot| {
                let mut first = 0;
                loop {
                    let Some((header, slots)) = snapshot.recipe(key, false, first, 64)? else {
                        return Ok(false);
                    };
                    for slot in &slots {
                        let slot: serde_json::Value =
                            serde_json::from_str(slot).map_err(|_| StorageError::ScopeChanged)?;
                        if since.is_none_or(|since| {
                            slot["t"].as_i64().is_some_and(|time| time >= since)
                        }) {
                            return Ok(true);
                        }
                    }
                    first += slots.len() as i64;
                    if first >= header.3 {
                        return Ok(false);
                    }
                }
            },
        )
        .map_err(unavailable)
    }
    fn validate(&self, namespace: &LogicalSourceNamespace, part: &Part) -> Result<(), QueryError> {
        let runtime_observation = super::super::super::current_sources::Runtime::open(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.options,
        )
        .map_err(unavailable)?;
        if namespace != &self.namespace
            || part.generation != self.generation
            || runtime_observation
                .generation(&self.options)
                .map_err(|error| QueryError::io(&error))?
                != part.runtime
        {
            return Err(unavailable(StorageError::ScopeChanged));
        }
        with_admitted_generation(
            &self.pool,
            namespace,
            &self.control,
            &self.generation,
            |_| Ok(()),
        )
        .map_err(unavailable)
    }
}

pub(in crate::storage::postgres) struct PgPage<'a, 'b, 'tx, 'r> {
    pub(in crate::storage::postgres) snapshot: &'a Snapshot<'b, 'tx>,
    pub(in crate::storage::postgres) key: &'a str,
    pub(in crate::storage::postgres) history: bool,
    pub(in crate::storage::postgres) records:
        &'a super::super::selected_reader::SelectedRecords<'r, 'b, 'tx>,
}
impl crate::query::transcript::PageSource for PgPage<'_, '_, '_, '_> {
    fn slots(
        &self,
        low: usize,
        high: usize,
    ) -> Result<Vec<crate::slot_projection::SavedSlot>, QueryError> {
        let mut result = Vec::new();
        let mut bytes = 0;
        for start in (low..high).step_by(64) {
            let (_, slots) = self
                .snapshot
                .recipe(
                    self.key,
                    self.history,
                    start as i64,
                    (high - start).min(64) as i64,
                )
                .map_err(unavailable)?
                .ok_or_else(|| unavailable(StorageError::ScopeChanged))?;
            for slot in slots {
                bytes += slot.len();
                if bytes > 4 * 1024 * 1024 {
                    return Err(unavailable(StorageError::Busy));
                }
                result.push(serde_json::from_str(&slot).map_err(unavailable)?);
            }
        }
        Ok(result)
    }
    fn read(
        &self,
        path: &std::path::Path,
        offset: u64,
    ) -> std::io::Result<(Option<serde_json::Value>, u64)> {
        self.records.read(path, offset)
    }
    fn prompt(
        &self,
        file: &crate::model::SlotFile,
        offset: u64,
    ) -> Option<(serde_json::Value, u64)> {
        self.records.prompt(file, offset)
    }
}

impl Snapshot<'_, '_> {
    pub(in crate::storage::postgres) fn query_first_slot(
        &self,
        key: &str,
        turn: &str,
        history: bool,
    ) -> Result<(usize, crate::slot_projection::SavedSlot), StorageError> {
        let (owner, slots) = if history {
            (
                self.owner(key)?.ok_or(StorageError::ScopeChanged)?,
                "semon_pg_native_catalog_history_slots",
            )
        } else {
            (
                self.generation.clone(),
                "semon_pg_native_catalog_recipe_rows",
            )
        };
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let rows=tx.driver.query(&format!("SELECT slot,CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM {slots} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND (CASE WHEN octet_length(metadata)<=65536 THEN convert_from(metadata,'UTF8')::jsonb END)->>'turn'=$4 AND (CASE WHEN octet_length(metadata)<=65536 THEN convert_from(metadata,'UTF8')::jsonb END)->>'first'='true' ORDER BY slot LIMIT 1"), &[&tx.namespace.as_str().as_bytes(),&owner.as_bytes(),&key.as_bytes(),&turn]).map_err(sqlstate::error)?;
        tx.check()?;
        let row = rows.first().ok_or(StorageError::ScopeChanged)?;
        let slot =
            serde_json::from_slice(&row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?)
                .map_err(|_| StorageError::ScopeChanged)?;
        Ok((
            usize::try_from(row.get::<_, i64>(0)).map_err(|_| StorageError::ScopeChanged)?,
            slot,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::{
        PgCancellation,
        tests::{fixture, ns},
        txn::TxnKind,
    };

    #[test]
    fn finite_history_page_bounds_aggregate_keys_and_metadata_before_materializing_cells() {
        let (pool, _) = fixture(|_| {});
        let namespace = ns("finite-history-page-budget");
        let control = PgCancellation::default();
        let generation = "a".repeat(64);
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            let operation_id = b"finite-history-query".to_vec();
            let payload = b"fixture".to_vec();
            tx.driver.execute(
                "INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,1)",
                &[&ns, &operation_id, &payload, &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            tx.driver.execute(
                "WITH source AS (
                    SELECT convert_to(repeat('x',8200)||lpad(i::text,2,'0'),'UTF8') AS session_key
                    FROM generate_series(1,65) AS n(i)
                ) INSERT INTO semon_pg_native_catalog_history_owners(
                    namespace,generation,key_digest,session_key,owner_generation,original,
                    last_ms,harness,repo,model,query_activity,query_active
                ) SELECT $1,$2,sha256(session_key),session_key,$2,false,0,'claude','','',100,false
                  FROM source",
                &[&ns, &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            tx.driver.execute(
                "WITH source AS (
                    SELECT convert_to(repeat('x',8200)||lpad(i::text,2,'0'),'UTF8') AS session_key
                    FROM generate_series(1,65) AS n(i)
                ), metadata AS (
                    SELECT session_key,convert_to(jsonb_build_object(
                        'key',convert_from(session_key,'UTF8'),'name','fixture','harness','claude',
                        'kind','session','native_ids','[]'::jsonb,'parent',NULL,'parent_source',NULL,
                        'repo',NULL,'branch',NULL,'model','','effort',NULL,'start',NULL,'last',0,
                        'tokens','[0,0,0]'::jsonb,
                        'cost',jsonb_build_object('usd',NULL,'unpriced_models','[]'::jsonb,
                            'split_unknown_messages',0,'by_model','{}'::jsonb,'by_day','{}'::jsonb),
                        'sources','[]'::jsonb
                    )::text,'UTF8') AS metadata
                    FROM source
                ) INSERT INTO semon_pg_native_catalog_history_headers(
                    namespace,generation,key_digest,session_key,recipe_generation,source_generation,
                    version,total,incomplete,metadata,original_header,last_ms,harness,repo,model,
                    query_activity,query_active
                ) SELECT $1,$2,sha256(session_key),session_key,$2,NULL,1,0,false,metadata,NULL,
                         0,'claude','','',100,false
                  FROM metadata",
                &[&ns, &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            let sizes = tx.driver.query_one(
                "SELECT sum(octet_length(o.session_key)),sum(octet_length(h.metadata))
                 FROM semon_pg_native_catalog_history_owners o
                 JOIN semon_pg_native_catalog_history_headers h
                   ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace
                  AND h.generation=o.owner_generation AND h.key_digest=o.key_digest
                  AND h.session_key=o.session_key
                 WHERE o.namespace=$1 AND o.generation=$2",
                &[&ns, &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            let key_bytes: i64 = sizes.get(0);
            let metadata_bytes: i64 = sizes.get(1);
            assert!(key_bytes < 1024 * 1024);
            assert!(metadata_bytes < 1024 * 1024);
            assert!(key_bytes + metadata_bytes + 65 * 64 > 1024 * 1024);
            Ok(())
        }).unwrap();

        let mut bytes_left = 1024 * 1024;
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            let rows = finite_history_page_rows(snapshot, 0, false, None, bytes_left)?;
            assert_eq!(rows.len(), 65);
            assert!(
                rows.iter()
                    .all(|row| row.get::<_, Option<Vec<u8>>>(0).is_none())
            );
            assert!(
                rows.iter()
                    .all(|row| row.get::<_, Option<Vec<u8>>>(1).is_none())
            );
            assert_eq!(
                finite_history_page(snapshot, 0, false, None, &mut bytes_left).err(),
                Some(StorageError::Busy)
            );
            Ok(())
        })
        .unwrap();

        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            tx.driver.execute(
                "DELETE FROM semon_pg_native_catalog_history_headers WHERE namespace=$1 AND generation=$2",
                &[&ns, &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            tx.driver.execute(
                "DELETE FROM semon_pg_native_catalog_history_owners WHERE namespace=$1 AND generation=$2 AND key_digest<>(SELECT key_digest FROM semon_pg_native_catalog_history_owners WHERE namespace=$1 AND generation=$2 ORDER BY key_digest LIMIT 1)",
                &[&ns, &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            Ok(())
        }).unwrap();

        bytes_left = 1024 * 1024;
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            let rows = finite_history_page_rows(snapshot, 0, false, None, bytes_left)?;
            assert_eq!(rows.len(), 1);
            assert!(rows[0].get::<_, Option<Vec<u8>>>(0).is_some());
            assert!(rows[0].get::<_, Option<Vec<u8>>>(1).is_none());
            assert_eq!(
                finite_history_page(snapshot, 0, false, None, &mut bytes_left).err(),
                Some(StorageError::Busy)
            );
            Ok(())
        })
        .unwrap();
    }
}
