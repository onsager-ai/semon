//! Bounded retained normalized evidence and complete-cohort Query derivation.
use super::{Namespace, PgProjectionTransaction, StorageError, backend};
use crate::{events::{FileIndex, Ledger}, model::summary::CatalogRow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    index: FileIndex,
    events: Vec<String>,
    extras: Vec<String>,
    signals: Vec<crate::events::Signal>,
    observations: Option<Vec<crate::events::store::native_usage::Observation>>,
    hashes: Vec<[u8;32]>,
}
fn identity(ledger: &Ledger) -> Result<String, StorageError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(ledger).map_err(backend)?)))
}
impl PgProjectionTransaction<'_> {
    pub(super) async fn retain_query_index(&self, namespace: &Namespace, path: &str) -> Result<(), StorageError> {
        let referenced: bool = self.tx.query_one("SELECT EXISTS(SELECT 1 FROM semon_projection.units WHERE namespace=$1 AND path=$2)", &[&namespace.binding(), &path]).await.map_err(backend)?.get(0);
        if !referenced { return Ok(()); }
        let Some((ledger, index)) = self.load_index(namespace, path).await? else { return Ok(()) };
        let key = identity(&ledger)?;
        let saved: bool = self.tx.query_one("SELECT EXISTS(SELECT 1 FROM semon_projection.history_indexes WHERE namespace=$1 AND source_generation=$2)", &[&namespace.binding(), &key]).await.map_err(backend)?.get(0);
        if saved { return Ok(()); }
        let snapshot = Snapshot {
            events: index.events.iter().map(super::index::encode_event).collect::<Result<_,_>>()?,
            extras: index.extras.iter().map(super::index::encode_event).collect::<Result<_,_>>()?,
            signals: index.signals.clone(), observations: index.native_usage_observations.clone(),
            hashes: index.source_chunks.as_ref().ok_or_else(|| backend("retained Query proofs unavailable"))?.hashes.clone(), index,
        };
        let metadata = serde_json::to_string(&snapshot).map_err(backend)?;
        if metadata.len() > 1048576 { return Err(backend("retained Query index budget exceeded")); }
        self.tx.execute("INSERT INTO semon_projection.history_indexes(namespace,source_generation,metadata) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", &[&namespace.binding(), &key, &metadata]).await.map_err(backend)?;
        Ok(())
    }

    async fn history_index(&self, namespace: &Namespace, source: &crate::model::summary::CatalogSource) -> Result<(String, Ledger, FileIndex), StorageError> {
        let ledger = crate::source_carrier::ledger(source);
        let path = source.path.to_str().ok_or_else(|| backend("invalid retained Query path"))?;
        if self.ledger(namespace, path).await?.is_some() && let Some((current, index)) = self.load_index(namespace, path).await? && current == ledger { return Ok((path.into(), ledger, index)); }
        let saved = self.tx.query_opt("SELECT metadata FROM semon_projection.history_indexes WHERE namespace=$1 AND source_generation=$2", &[&namespace.binding(), &identity(&ledger)?]).await.map_err(backend)?.ok_or_else(|| backend("retained Query index unavailable"))?;
        let snapshot: Snapshot = serde_json::from_str(saved.get(0)).map_err(backend)?;
        let mut index = snapshot.index;
        index.events = snapshot.events.iter().map(|text| super::index::decode_event(text)).collect::<Result<_,_>>()?;
        index.extras = snapshot.extras.iter().map(|text| super::index::decode_event(text)).collect::<Result<_,_>>()?;
        index.signals = snapshot.signals;
        index.native_usage_observations = snapshot.observations;
        index.source_chunks = Some(std::sync::Arc::new(crate::source_chunks::Chunks { offset: ledger.offset, hashes: snapshot.hashes, resume: None }));
        Ok((path.into(), ledger, index))
    }

    pub(super) async fn derive_history_query(&self, namespace: &Namespace, options: &crate::Options, key: &str) -> Result<(), StorageError> {
        let Some(record) = self.tx.query_opt("SELECT metadata FROM semon_projection.history_catalog WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)? else { return Ok(()) };
        let mut row: CatalogRow = serde_json::from_str(record.get(0)).map_err(backend)?;
        let generation = crate::retention::source_generation(&row.sources).map_err(backend)?;
        if row.query.is_some() {
            self.tx.execute("INSERT INTO semon_projection.history_query_context(namespace,session_key,source_generation,metadata) SELECT namespace,session_key,source_generation,metadata FROM semon_projection.query_context WHERE namespace=$1 AND session_key=$2 AND source_generation=$3 ON CONFLICT(namespace,session_key) DO UPDATE SET source_generation=excluded.source_generation,metadata=excluded.metadata", &[&namespace.binding(), &key, &generation]).await.map_err(backend)?;
            return Ok(());
        }
        // Distinct generations at one path cannot share a path-keyed native
        // derivation. Keep their exact custody without inventing an aggregate.
        let mut paths = std::collections::BTreeSet::new();
        if row.sources.iter().any(|source| !paths.insert(&source.path)) { return Ok(()); }
        let mut files = Vec::new();
        for source in &row.sources { files.push(self.history_index(namespace, source).await?); }
        let options = crate::Options { all: true, scan_window: false, ..options.clone() };
        let prepared = crate::overview_repair::projected_history(&options, std::slice::from_ref(&row), files, crate::model::now_ms()).map_err(backend)?;
        let derived = prepared.rows.iter().find(|derived| derived.key == key).ok_or_else(|| backend("retained Query owner changed"))?;
        let trace = derived.trace.as_ref().ok_or_else(|| backend("retained Query context unavailable"))?;
        if !trace.closed_over(&std::collections::BTreeSet::from([key])) { return Err(backend("retained Query relationship closure unavailable")); }
        row.query = derived.query.clone();
        if let Some(query) = &mut row.query { query["source_generation"] = serde_json::json!(generation); }
        self.tx.execute("UPDATE semon_projection.history_catalog SET metadata=$3 WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key, &serde_json::to_string(&row).map_err(backend)?]).await.map_err(backend)?;
        self.tx.execute("INSERT INTO semon_projection.history_query_context(namespace,session_key,source_generation,metadata) VALUES($1,$2,$3,$4) ON CONFLICT(namespace,session_key) DO UPDATE SET source_generation=excluded.source_generation,metadata=excluded.metadata", &[&namespace.binding(), &key, &generation, &trace.storage_context().to_string()]).await.map_err(backend)?;
        Ok(())
    }
}
