//! Private Query injection over an acknowledged generation. It deliberately
//! refuses cohorts requiring retained-history merge until that route is ported.
use super::*;
use crate::{Options, QueryError, storage::query::{Part, QueryCatalogStore}};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::storage::postgres) struct QueryCatalog {
    pub(in crate::storage::postgres) pool: PgPool,
    pub(in crate::storage::postgres) namespace: LogicalSourceNamespace,
    pub(in crate::storage::postgres) control: PgCancellation,
    pub(in crate::storage::postgres) generation: String,
    pub(in crate::storage::postgres) options: Options,
}
fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("unavailable", error.to_string())
}
impl QueryCatalogStore for QueryCatalog {
    fn read_cohort(&self, namespace: &LogicalSourceNamespace, _: i64, bytes_left: &mut usize) -> Result<Part, QueryError> {
        if namespace != &self.namespace || !self.options.all { return Err(unavailable(StorageError::NamespaceUpgradePending)); }
        let runtime = crate::native_evidence::query_runtime_generation(&self.options).map_err(|error| QueryError::io(&error))?;
        with_admitted_generation(&self.pool, namespace, &self.control, &self.generation, |snapshot| {
            use crate::storage::catalog::CatalogSnapshot;
            let tx_snapshot = snapshot.runtime_binding().ok_or(StorageError::ScopeChanged)?;
            let observations = crate::native_evidence::RuntimeObservations::bound(&tx_snapshot, &runtime).map_err(|_| StorageError::ScopeChanged)?;
            let request = crate::catalog::Request::parse("limit=64").map_err(|_| StorageError::InvalidArguments)?;
            let catalog = snapshot.rows(&request).map_err(|_| StorageError::Unavailable)?;
            let mut rows = BTreeMap::new();
            let mut owners = BTreeMap::new();
            let mut witnesses = Vec::new();
            let mut content = BTreeMap::new();
            for row in catalog {
                if snapshot.has_history(&row.key).map_err(|_| StorageError::Unavailable)? { return Err(StorageError::NamespaceUpgradePending); }
                if row.sources.is_empty() || row.sources.len() > 64 || row.sources.iter().any(|source| crate::catalog::source_relative(&self.options, &row.harness, source).is_none()) { return Err(StorageError::ScopeChanged); }
                let expected = observations.expected(&row);
                let saved = row.query.as_ref().ok_or(StorageError::ScopeChanged)?;
                if !crate::model::summary::query_observation_matches(saved, &row.sources, true, expected.as_deref()).map_err(|_| StorageError::ScopeChanged)? { return Err(StorageError::ScopeChanged); }
                let witness = saved.to_string();
                *bytes_left = bytes_left.checked_sub(witness.len()).ok_or(StorageError::Busy)?;
                let summaries = saved["rows"].as_object().ok_or(StorageError::ScopeChanged)?;
                if !summaries.contains_key(&row.key) { return Err(StorageError::ScopeChanged); }
                for (id, value) in summaries {
                    if value["summary"]["id"] != *id || rows.get(id).is_some_and(|prior| prior != value) { return Err(StorageError::ScopeChanged); }
                    rows.insert(id.clone(), value.clone());
                    owners.entry(id.clone()).or_insert_with(|| row.key.clone());
                }
                witnesses.push((row.key.clone(), witness));
                // Content generation belongs to the same immutable selected trace.
                let version = with_generation(&self.pool, namespace, &self.control, &self.generation, |physical| {
                    use crate::storage::selected::ContextLookup;
                    physical.preview_header(&row.key).map_err(|_| StorageError::Unavailable)
                })?;
                if let Some((_, Some(version))) = version { content.insert(row.key, version); }
            }
            if rows.len() > 64 { return Err(StorageError::Busy); }
            // Each saved owner contains its complete relationship summaries.
            // Exact edge/turn sets are loaded below from the immutable effects.
            let (edges, turns) = self.pool.transaction(namespace, TxnKind::ReadSnapshot, &self.control, |tx| {
                let mut sets = Vec::new();
                for (table, column) in [("semon_pg_native_catalog_trace_edges", "edge_id"), ("semon_pg_native_catalog_trace_turns", "turn_id")] {
                    let values = tx.driver.query(&format!("SELECT {column} FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 65"), &[&namespace.as_str().as_bytes(), &self.generation.as_bytes()]).map_err(sqlstate::error)?;
                    if values.len() > 64 { return Err(StorageError::Busy); }
                    let values = values.iter().map(|row| text(row.get(0))).collect::<Result<BTreeSet<_>, _>>()?;
                    sets.push(values);
                }
                Ok((sets.remove(0), sets.remove(0)))
            })?;
            Ok(Part { generation: self.generation.clone(), runtime: runtime.clone(), rows, owners, content, witnesses, edges, turns })
        }).map_err(unavailable)
    }
    fn has_search_slots(&self, namespace: &LogicalSourceNamespace, key: &str, since: Option<i64>) -> Result<bool, QueryError> {
        if namespace != &self.namespace { return Err(unavailable(StorageError::ScopeChanged)); }
        with_generation(&self.pool, namespace, &self.control, &self.generation, |snapshot| {
            let Some((header, slots)) = snapshot.recipe(key, false, 0, 64)? else { return Ok(false); };
            if header.3 > 64 { return Err(StorageError::Busy); }
            for slot in slots {
                let slot: serde_json::Value = serde_json::from_str(&slot).map_err(|_| StorageError::ScopeChanged)?;
                if since.is_none_or(|since| slot["t"].as_i64().is_some_and(|time| time >= since)) { return Ok(true); }
            }
            Ok(false)
        }).map_err(unavailable)
    }
    fn validate(&self, namespace: &LogicalSourceNamespace, part: &Part) -> Result<(), QueryError> {
        if namespace != &self.namespace || part.generation != self.generation || crate::native_evidence::query_runtime_generation(&self.options).map_err(|error| QueryError::io(&error))? != part.runtime { return Err(unavailable(StorageError::ScopeChanged)); }
        with_admitted_generation(&self.pool, namespace, &self.control, &self.generation, |_| Ok(())).map_err(unavailable)
    }
}
