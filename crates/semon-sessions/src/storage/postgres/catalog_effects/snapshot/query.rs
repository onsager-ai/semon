//! Private Query injection over an acknowledged generation. Cohorts merge
//! retained history exactly as the SQLite reader: history rows win per key.
use super::*;
use crate::{
    Options, QueryError,
    storage::{
        catalog::CatalogSnapshot,
        query::{Part, QueryCatalogStore},
    },
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
        if namespace != &self.namespace {
            return Err(unavailable(StorageError::NamespaceUpgradePending));
        }
        with_admitted_snapshot(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.generation,
            |snapshot| {
                // A withdrawn key reads its proof-owned retained recipe; the
                // body itself still comes only from Raw Source bytes.
                let history = snapshot.catalog(key, false)?.is_none();
                let (row, header, _) = snapshot
                    .selected(key, history, 0, 1)?
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
                        if owner != key
                            && let Some(row) = snapshot.catalog(&owner, false)?
                        {
                            context.push(row);
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
                                .query_first_slot(key, turn, history)
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
        let runtime = crate::native_evidence::query_runtime_generation(&self.options)
            .map_err(|error| QueryError::io(&error))?;
        with_admitted_snapshot(&self.pool, namespace, &self.control, &self.generation, |snapshot| {
            let tx_snapshot = snapshot.runtime_binding().ok_or(StorageError::ScopeChanged)?;
            let observations = crate::native_evidence::RuntimeObservations::bound(&tx_snapshot, &runtime).map_err(|_| StorageError::ScopeChanged)?;
            let mut request = crate::catalog::Request::parse("limit=64").map_err(|_| StorageError::InvalidArguments)?;
            let mut rows = BTreeMap::new();
            let mut owners = BTreeMap::new();
            let mut witnesses = Vec::new();
            let mut content = BTreeMap::new();
            let mut emit = |row: crate::model::summary::CatalogRow, keep: Option<&BTreeSet<String>>, bytes_left: &mut usize| -> Result<(), StorageError> {
                if row.sources.is_empty() || row.sources.len() > 64 || row.sources.iter().any(|source| crate::catalog::source_relative(&self.options, &row.harness, source).is_none()) { return Err(StorageError::ScopeChanged); }
                let expected = observations.expected(&row);
                let saved = row.query.as_ref().ok_or(StorageError::ScopeChanged)?;
                if !crate::model::summary::query_observation_matches(saved, &row.sources, true, expected.as_deref()).map_err(|_| StorageError::ScopeChanged)? { return Err(StorageError::ScopeChanged); }
                let witness = saved.to_string();
                *bytes_left = bytes_left.checked_sub(witness.len()).ok_or(StorageError::Busy)?;
                let summaries = saved["rows"].as_object().ok_or(StorageError::ScopeChanged)?;
                if !summaries.contains_key(&row.key) { return Err(StorageError::ScopeChanged); }
                for (id, value) in summaries.iter().filter(|(id, _)| keep.is_none_or(|keep| keep.contains(*id))) {
                    if value["summary"]["id"] != *id || rows.get(id).is_some_and(|prior| prior != value) { return Err(StorageError::ScopeChanged); }
                    rows.insert(id.clone(), value.clone());
                    owners.entry(id.clone()).or_insert_with(|| row.key.clone());
                }
                witnesses.push((row.key.clone(), witness));
                // Content generation belongs to the same immutable selected trace.
                use crate::storage::selected::ContextLookup;
                let version = snapshot.preview_header(&row.key).map_err(|_| StorageError::Unavailable)?;
                if let Some((_, Some(version))) = version { content.insert(row.key, version); }
                Ok(())
            };
            // As the SQLite reader: a Current key with retained history
            // contributes its retained history row to the cohort.
            let chosen = |row: crate::model::summary::CatalogRow| -> Result<(crate::model::summary::CatalogRow, bool), StorageError> {
                Ok(match snapshot.history_catalog(&row.key).map_err(|_| StorageError::Unavailable)? {
                    Some(text) => {
                        let history: crate::model::summary::CatalogRow = serde_json::from_str(&text).map_err(|_| StorageError::ScopeChanged)?;
                        if history.key != row.key { return Err(StorageError::ScopeChanged); }
                        (history, true)
                    }
                    None => (row, false),
                })
            };
            let query_last = |row: &crate::model::summary::CatalogRow| row.query.as_ref().and_then(|query| query["last"].as_i64());
            let mut loaded = BTreeMap::new();
            loop {
                let mut catalog = snapshot.rows(&request).map_err(|_| StorageError::Unavailable)?;
                let more = catalog.len() > request.limit;
                catalog.truncate(request.limit);
                let next = catalog.last().map(|row| crate::catalog::Cursor {
                    inclusive: false,
                    scope: crate::CatalogReadScope::Current,
                    version: crate::model::summary::CATALOG_VERSION,
                    generation: self.generation.clone(),
                    machine: String::new(), harness: None, repo: None, model: None, q: None,
                    last: row.last.unwrap_or(0), key: row.key.clone(),
                });
                for row in catalog {
                    let Some(c) = cutoff else {
                        emit(chosen(row)?.0, None, bytes_left)?;
                        continue;
                    };
                    let state = row.query.as_ref().and_then(|query| query["state"].as_str()).is_some_and(|state| matches!(state, "work" | "wait"));
                    let current_recent = row.last.is_some_and(|last| last >= c) || query_last(&row).is_some_and(|last| last >= c) || state;
                    let (row, history) = chosen(row)?;
                    if current_recent || (history && query_last(&row).is_some_and(|last| last >= c)) {
                        loaded.insert(row.key.clone(), row);
                    }
                }
                if !more { break; }
                request.cursor = Some(next.ok_or(StorageError::ScopeChanged)?);
            }
            let Some(c) = cutoff else {
                // Each saved owner contains its complete relationship summaries.
                // Exact edge/turn sets are loaded below from the immutable effects.
                let (edges, turns) = {
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
                            let page = tx.driver.query(&format!("SELECT key_digest,{digest_column},CASE WHEN octet_length({column})<=65536 THEN {column} END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND ($3::bytea IS NULL OR (key_digest,{digest_column})>($3,$4)) ORDER BY key_digest,{digest_column} LIMIT 64"), &[&namespace.as_str().as_bytes(), &self.generation.as_bytes(), &after_key, &after_id]).map_err(sqlstate::error)?;
                            for row in &page {
                                let value = text(row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?)?;
                                *bytes_left = bytes_left.checked_sub(value.len()).ok_or(StorageError::Busy)?;
                                values.insert(value);
                            }
                            if page.len() < 64 { break; }
                            after = page.last().map(|row| (row.get(0), row.get(1)));
                        }
                        sets.push(values);
                    }
                    (sets.remove(0), sets.remove(0))
                };
                return Ok(Part { generation: self.generation.clone(), runtime: runtime.clone(), rows, owners, content, witnesses, edges, turns });
            };
            // Recent window: edges active or touched since the cutoff, then the
            // last/recent turns of each kept owner and their linked endpoints.
            let mut keep: BTreeSet<String> = loaded.keys().cloned().collect();
            let mut needed = keep.clone();
            let mut edges = BTreeSet::new();
            let mut turns = BTreeSet::new();
            let json = |bytes: Option<Vec<u8>>| -> Result<serde_json::Value, StorageError> {
                serde_json::from_slice(&bytes.ok_or(StorageError::Busy)?).map_err(|_| StorageError::ScopeChanged)
            };
            let endpoint = |metadata: &serde_json::Value| -> Result<(String, Option<String>), StorageError> {
                let from = metadata["from"].as_str().ok_or(StorageError::ScopeChanged)?.to_owned();
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
                        let id = text(row.get(3))?;
                        let metadata = json(row.get(4))?;
                        let status = metadata["status"].as_str().is_some_and(|status| matches!(status, "work" | "wait" | "new"));
                        if !(metadata["at"].as_i64().is_some_and(|at| at >= c) || metadata["done"].as_i64().is_some_and(|done| done >= c) || status) { continue; }
                        let (from, to) = endpoint(&metadata)?;
                        *bytes_left = bytes_left.checked_sub(owner.len() + id.len() + from.len() + to.as_ref().map_or(0, String::len)).ok_or(StorageError::Busy)?;
                        needed.insert(owner);
                        edges.insert(id);
                        keep.insert(from);
                        keep.extend(to);
                    }
                    if page.len() < 64 { break; }
                    after = page.last().map(|row| (row.get(0), row.get(1)));
                }
            }
            for owner in keep.clone() {
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
                            let last = metadata["last"] == serde_json::json!(1) || metadata["last"] == serde_json::json!(true);
                            if !(last || metadata["at"].as_i64().is_some_and(|at| at >= c)) { continue; }
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
                        let (from, to) = endpoint(&serde_json::from_str(&metadata).map_err(|_| StorageError::ScopeChanged)?)?;
                        keep.insert(from);
                        keep.extend(to);
                    }
                }
            }
            needed.extend(keep.iter().cloned());
            for key in &needed {
                let row = match loaded.remove(key) {
                    Some(row) => row,
                    None => match snapshot.catalog(key, false)? {
                        Some(row) => chosen(row)?.0,
                        None => continue,
                    },
                };
                emit(row, Some(&keep), bytes_left)?;
            }
            if keep.iter().any(|id| id != "you" && !rows.contains_key(id)) {
                return Err(StorageError::ScopeChanged);
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
    history: bool,
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
                .recipe(self.key, self.history, start as i64, (high - start).min(64) as i64)
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
        history: bool,
    ) -> Result<(usize, crate::slot_projection::SavedSlot), StorageError> {
        let (owner, slots) = if history {
            (
                self.owner(key)?.ok_or(StorageError::ScopeChanged)?,
                "semon_pg_native_catalog_history_slots",
            )
        } else {
            (self.generation.clone(), "semon_pg_native_catalog_recipe_rows")
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
