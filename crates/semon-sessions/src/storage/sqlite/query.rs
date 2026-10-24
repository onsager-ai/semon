//! SQLite implementation of semantic Query metadata operations.
#[cfg(test)]
use crate::storage::query::measured;
use crate::storage::{
    LogicalSourceNamespace,
    query::{Part, QueryCatalogStore},
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
        let db = self.binding.snapshot(namespace).map_err(storage_error)?;
        read(&db, &self.options, now, bytes_left)
    }
    fn has_search_slots(
        &self,
        namespace: &LogicalSourceNamespace,
        key: &str,
        since: Option<i64>,
    ) -> Result<bool, QueryError> {
        let db = self.binding.snapshot(namespace).map_err(storage_error)?;
        has_search_slots(&db, key, since)
    }
    fn validate(&self, namespace: &LogicalSourceNamespace, part: &Part) -> Result<(), QueryError> {
        let db = self.binding.snapshot(namespace).map_err(storage_error)?;
        let options = &self.options;
        if generation(&db)? != part.generation
            || crate::native_evidence::query_runtime_generation(options)
                .map_err(|error| QueryError::io(&error))?
                != part.runtime
        {
            return Err(unavailable("Native Catalog changed during the query"));
        }
        for (key, expected) in &part.witnesses {
            let actual:Option<String>=db.query_row("SELECT CASE WHEN h.session_key IS NOT NULL THEN json_extract(h.metadata,'$.query') ELSE json_extract(c.metadata,'$.query') END FROM session_catalog c LEFT JOIN session_history_catalog h USING(session_key) WHERE c.session_key=?1",[key],|row|row.get(0)).optional().map_err(unavailable)?.flatten();
            let actual = actual.and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if actual.as_ref().map(Value::to_string).as_deref() != Some(expected.as_str()) {
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
        "SELECT value FROM meta WHERE key='catalog_generation'",
        [],
        |row| row.get(0),
    )
    .map_err(unavailable)
}
fn read(db: &Connection, options: &Options, now: i64, bytes_left: &mut usize) -> Result<Part, QueryError> {
    let generation = generation(db)?;
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
