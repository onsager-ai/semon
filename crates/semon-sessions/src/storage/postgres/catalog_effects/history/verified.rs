//! Verified local History composition. Original custody keeps its own path.
use super::*;
use crate::storage::{
    QueryRuntimeBinding,
    catalog::{
        HistoryCompositionSnapshot, VerifiedHistoryComposition, VerifiedHistoryCompositionStore,
    },
    selected::PresentationObservations,
};

#[derive(serde::Serialize, serde::Deserialize)]
pub(in crate::storage::postgres::catalog_effects) struct ComposedGrant {
    admission: u64,
    stream: String,
    capture: PresentationObservations,
    pub(in crate::storage::postgres::catalog_effects) receipts: Vec<observations::NativeReceipt>,
}

fn grant_key(owner: &str, metadata: &[u8]) -> String {
    format!(
        "catalog-history-composed-observation:{owner}:{:x}",
        Sha256::digest(metadata)
    )
}

/// The physical owner and exact metadata select this grant. Every predecessor
/// receipt is reauthenticated; a compact capture never grants slot custody.
pub(in crate::storage::postgres::catalog_effects) fn read_grant(
    tx: &mut PgTxn<'_>,
    binding: &QueryRuntimeBinding,
    owner: &str,
    metadata: &[u8],
    row: &CatalogRow,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<Option<ComposedGrant>, StorageError> {
    let key = grant_key(owner, metadata);
    let Some(bytes) = super::super::super::native_keys::read_meta(tx, key.as_bytes(), MAX)? else {
        return Ok(None);
    };
    budget.charge(bytes.len()).map_err(|_| StorageError::Busy)?;
    let grant: ComposedGrant =
        serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    let acknowledged: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2)", &[&tx.namespace.as_str().as_bytes(), &owner.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if !acknowledged
        || grant.admission != binding.admission
        || grant.stream != binding.stream_epoch
        || grant.receipts.is_empty()
        || grant.receipts.len() > 64
        || !grant.capture.is_runtime_capture_for(row)
    {
        return Err(StorageError::ScopeChanged);
    }
    let source_runtime = grant
        .capture
        .source_runtime
        .as_deref()
        .ok_or(StorageError::ScopeChanged)?;
    if grant.capture.runtime.as_deref()
        != Some(
            binding
                .witness(source_runtime)
                .map_err(|_| StorageError::ScopeChanged)?
                .as_str(),
        )
    {
        return Err(StorageError::ScopeChanged);
    }
    for receipt in &grant.receipts {
        if observations::composition_receipt(tx, binding, row, receipt, budget)?.is_empty() {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(Some(grant))
}

impl ComposedGrant {
    pub(in crate::storage::postgres::catalog_effects) fn into_capture(
        self,
    ) -> PresentationObservations {
        self.capture
    }
}

impl VerifiedHistoryCompositionStore for History<'_, '_> {
    fn composition_snapshot(
        &self,
        current: &CatalogRow,
        budget: &mut crate::overview::SummaryPublicationBudget,
        authenticate_receipts: bool,
    ) -> Result<HistoryCompositionSnapshot, StorageError> {
        let owner = self.owner(&current.key)?;
        let history = owner.is_some();
        let previous = CompositeHistoryStore::previous(self, &current.key)?;
        let (_, recipe_header) = self.previous_recipe(&current.key)?;
        let mut snapshot = HistoryCompositionSnapshot {
            key: current.key.clone(),
            original: self.original(&current.key)?,
            previous,
            history_exists: history,
            recipe_history: history,
            recipe_header,
            recipe_slots: Vec::new(),
            relocated: Default::default(),
            receipts: Vec::new(),
            current_cwds: None,
            unavailable: false,
        };
        if snapshot.original {
            return Ok(snapshot);
        }
        if budget
            .charge(snapshot.previous.as_ref().map_or(0, String::len))
            .is_err()
        {
            snapshot.unavailable = true;
            return Ok(snapshot);
        }
        let Some(metadata) = snapshot.previous.as_ref() else {
            return Ok(snapshot);
        };
        let row: CatalogRow =
            serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        if !authenticate_receipts
            && !catalog::replaces_original(&row, current)
            && row
                .sources
                .iter()
                .all(|source| current.sources.iter().any(|now| now.path == source.path))
        {
            return Ok(snapshot);
        }
        if snapshot.recipe_header.is_some() {
            let Some(slots) = self.bounded_recipe_slots(&current.key, history)? else {
                snapshot.unavailable = true;
                return Ok(snapshot);
            };
            snapshot.recipe_slots = slots;
        }
        if budget
            .charge(snapshot.recipe_slots.iter().map(String::len).sum::<usize>())
            .is_err()
        {
            snapshot.unavailable = true;
            return Ok(snapshot);
        }
        let mut tx = self.effects.tx.borrow_mut();
        snapshot.current_cwds = if current.sources.is_empty() {
            Some(Default::default())
        } else {
            super::super::super::native_reader::runtime_source_cwds_if_current(
                &mut tx,
                &current.sources,
            )?
        };
        let binding = super::super::super::model_cache::binding(&mut tx)?;
        if row.sources.len() > 64 {
            return Err(StorageError::Busy);
        }
        let receipts = if let Some((owner, _)) = owner {
            if let Some(grant) =
                read_grant(&mut tx, &binding, &owner, metadata.as_bytes(), &row, budget)?
            {
                grant.receipts
            } else {
                Vec::new()
            }
        } else {
            let origin = snapshot::in_transaction(&mut tx, self.previous, |view| {
                view.row_observation(&row, Some(catalog::CatalogObservationOwner::Catalog))
                    .map_err(|_| StorageError::ScopeChanged)?
                    .map(|(_, _, origin)| origin)
                    .ok_or(StorageError::ScopeChanged)
            })?;
            match origin {
                observations::Origin::Native(receipt) => vec![receipt],
                observations::Origin::Imported(_) | observations::Origin::Composed => Vec::new(),
            }
        };
        for receipt in &receipts {
            snapshot.receipts.extend(observations::composition_receipt(
                &mut tx, &binding, &row, receipt, budget,
            )?);
        }
        drop(tx);
        for source in &row.sources {
            if !current.sources.iter().any(|now| now.path == source.path)
                && self.relocated(source, &current.sources)?
            {
                snapshot
                    .relocated
                    .insert(source.path.to_string_lossy().into_owned());
            }
        }
        Ok(snapshot)
    }

    fn composed_verified(
        &self,
        composition: VerifiedHistoryComposition<'_>,
    ) -> Result<(), StorageError> {
        let mut budget = crate::overview::SummaryPublicationBudget::after_catalog_metadata(0);
        let row: CatalogRow =
            serde_json::from_str(composition.metadata).map_err(|_| StorageError::ScopeChanged)?;
        let snapshot = self.composition_snapshot(&row, &mut budget, true)?;
        let mut tx = self.effects.tx.borrow_mut();
        let binding = super::super::super::model_cache::binding(&mut tx)?;
        let mut receipts = Vec::new();
        for id in composition.receipt_ids {
            if !snapshot.receipts.iter().any(|receipt| &receipt.id == id) {
                return Err(StorageError::ScopeChanged);
            }
            receipts.push((binding.admission, binding.stream_epoch.clone(), id.clone()));
        }
        let capture: PresentationObservations =
            serde_json::from_str(composition.capture).map_err(|_| StorageError::ScopeChanged)?;
        if !capture.is_runtime_capture_for(&row) || receipts.is_empty() || receipts.len() > 64 {
            return Err(StorageError::ScopeChanged);
        }
        let grant = ComposedGrant {
            admission: binding.admission,
            stream: binding.stream_epoch,
            capture,
            receipts,
        };
        let key = grant_key(self.effects.generation, composition.metadata.as_bytes());
        let value = encoded(&grant)?;
        if value.len() > MAX {
            return Err(StorageError::Busy);
        }
        let changed = tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_meta.namespace=excluded.namespace AND semon_pg_native_meta.key=excluded.key AND semon_pg_native_meta.value=excluded.value", &[&tx.namespace.as_str().as_bytes(), &key.as_bytes(), &value]).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
        drop(tx);
        self.composed(
            composition.key,
            composition.generation,
            composition.sources,
            composition.metadata,
            composition.texts,
            composition.incomplete,
        )
    }
}
