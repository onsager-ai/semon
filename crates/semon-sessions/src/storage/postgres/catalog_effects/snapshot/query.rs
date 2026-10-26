//! Private Query injection over an acknowledged generation. It deliberately
//! refuses cohorts requiring retained-history merge until that route is ported.
use super::*;
use crate::{
    Options, QueryError,
    storage::{catalog::CatalogSnapshot, query::{Part, QueryCatalogStore}},
};
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
    fn selected_body(
        &self,
        namespace: &LogicalSourceNamespace,
        key: &str,
        args: &serde_json::Value,
        now: i64,
        host: &dyn crate::SessionSourceReader,
    ) -> Result<serde_json::Value, QueryError> {
        if namespace != &self.namespace || !self.options.all {
            return Err(unavailable(StorageError::NamespaceUpgradePending));
        }
        with_admitted_snapshot(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.generation,
            |snapshot| {
                let (row, header, _) = snapshot
                    .selected(key, false, 0, 1)?
                    .ok_or(StorageError::ScopeChanged)?;
                let total = usize::try_from(header.3).map_err(|_| StorageError::ScopeChanged)?;
                let observation: crate::slot_projection::QueryObservation = serde_json::from_value(
                    row.query.as_ref().ok_or(StorageError::ScopeChanged)?["transcript_observation"]
                        .clone(),
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                if !observation.valid(total) {
                    return Err(StorageError::ScopeChanged);
                }
                let mut context = vec![row.clone()];
                if args["_session"] == true {
                    let selected =
                        &row.query.as_ref().ok_or(StorageError::ScopeChanged)?["selected"];
                    let mut keys = BTreeSet::new();
                    for edge in selected["edges"]
                        .as_array()
                        .ok_or(StorageError::ScopeChanged)?
                    {
                        for field in ["from", "to"] {
                            if let Some(key) = edge["query"][field].as_str() {
                                keys.insert(key.to_owned());
                            }
                        }
                    }
                    if keys.len() > 64 {
                        return Err(StorageError::Busy);
                    }
                    for owner in keys {
                        if owner != key {
                            if let Some(row) = snapshot.catalog(&owner, false)? {
                                context.push(row);
                            }
                        }
                    }
                }
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
                    let summary = crate::query::transcript::render_session(
                        &self.options,
                        key,
                        &context,
                        &records,
                        now,
                        &|turn| {
                            snapshot
                                .query_first_slot(key, turn)
                                .map(|(_, slot)| slot)
                                .map_err(unavailable)
                        },
                        &|_| Ok(true),
                        &|_| Ok(true),
                        &|_| Ok(true),
                    )
                    .map_err(|_| StorageError::Unavailable)?;
                    records.finish().map_err(|_| StorageError::ScopeChanged)?;
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
                        (true, snapshot.query_first_slot(key, &turn)?.0)
                    }
                };
                let page = PgPage {
                    snapshot,
                    key,
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
                .map_err(|_| StorageError::Unavailable)?;
                records.finish().map_err(|_| StorageError::ScopeChanged)?;
                Ok(answer)
            },
        )
        .map_err(unavailable)
    }
    fn read_cohort(
        &self,
        namespace: &LogicalSourceNamespace,
        _: i64,
        bytes_left: &mut usize,
    ) -> Result<Part, QueryError> {
        if namespace != &self.namespace || !self.options.all {
            return Err(unavailable(StorageError::NamespaceUpgradePending));
        }
        let runtime = crate::native_evidence::query_runtime_generation(&self.options)
            .map_err(|error| QueryError::io(&error))?;
        with_admitted_snapshot(&self.pool, namespace, &self.control, &self.generation, |snapshot| {
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
                use crate::storage::selected::ContextLookup;
                let version = snapshot.preview_header(&row.key).map_err(|_| StorageError::Unavailable)?;
                if let Some((_, Some(version))) = version { content.insert(row.key, version); }
            }
            if rows.len() > 64 { return Err(StorageError::Busy); }
            // Each saved owner contains its complete relationship summaries.
            // Exact edge/turn sets are loaded below from the immutable effects.
            let (edges, turns) = {
                let tx = snapshot.tx.lock().map_err(|_|StorageError::Unavailable)?;
                let mut sets = Vec::new();
                for (table, column) in [("semon_pg_native_catalog_trace_edges", "edge_id"), ("semon_pg_native_catalog_trace_turns", "turn_id")] {
                    let values = tx.driver.query(&format!("SELECT CASE WHEN octet_length({column})<=65536 THEN {column} END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 65"), &[&namespace.as_str().as_bytes(), &self.generation.as_bytes()]).map_err(sqlstate::error)?;
                    if values.len() > 64 { return Err(StorageError::Busy); }
                    let values = values.iter().map(|row| text(row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?)).collect::<Result<BTreeSet<_>, _>>()?;
                    sets.push(values);
                }
                (sets.remove(0), sets.remove(0))
            };
            for id in edges.iter().chain(turns.iter()) {
                *bytes_left = bytes_left.checked_sub(id.len()).ok_or(StorageError::Busy)?;
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
                let Some((header, slots)) = snapshot.recipe(key, false, 0, 64)? else {
                    return Ok(false);
                };
                if header.3 > 64 {
                    return Err(StorageError::Busy);
                }
                for slot in slots {
                    let slot: serde_json::Value =
                        serde_json::from_str(&slot).map_err(|_| StorageError::ScopeChanged)?;
                    if since
                        .is_none_or(|since| slot["t"].as_i64().is_some_and(|time| time >= since))
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            },
        )
        .map_err(unavailable)
    }
    fn validate(&self, namespace: &LogicalSourceNamespace, part: &Part) -> Result<(), QueryError> {
        if namespace != &self.namespace
            || part.generation != self.generation
            || crate::native_evidence::query_runtime_generation(&self.options)
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

struct PgPage<'a, 'b, 'tx, 'r> {
    snapshot: &'a Snapshot<'b, 'tx>,
    key: &'a str,
    records: &'a super::super::selected_reader::SelectedRecords<'r, 'b, 'tx>,
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
                .recipe(self.key, false, start as i64, (high - start).min(64) as i64)
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
    fn query_first_slot(
        &self,
        key: &str,
        turn: &str,
    ) -> Result<(usize, crate::slot_projection::SavedSlot), StorageError> {
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let rows=tx.driver.query("SELECT slot,CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM semon_pg_native_catalog_recipe_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND (CASE WHEN octet_length(metadata)<=65536 THEN convert_from(metadata,'UTF8')::jsonb END)->>'turn'=$4 AND (CASE WHEN octet_length(metadata)<=65536 THEN convert_from(metadata,'UTF8')::jsonb END)->>'first'='true' ORDER BY slot LIMIT 1", &[&tx.namespace.as_str().as_bytes(),&self.generation.as_bytes(),&key.as_bytes(),&turn]).map_err(sqlstate::error)?;
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
