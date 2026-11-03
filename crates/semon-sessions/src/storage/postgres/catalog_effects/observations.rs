//! Original native publication evidence shared by custody issuance and reads.
use super::*;
use crate::storage::{QueryRuntimeBinding, selected::PresentationObservations};

pub(super) type NativeReceipt = (u64, String, String);

pub(super) enum Origin {
    Native(NativeReceipt),
    Imported(crate::storage::proof_lifetime::ProofGenerationId),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct HistoryReference {
    pub(super) version: u32,
    pub(super) lifecycle: crate::model::summary::CatalogLifecycle,
    pub(super) receipt: NativeReceipt,
}

pub(super) fn history_key(owner: &str, metadata: &[u8]) -> String {
    format!(
        "catalog-history-native-observation:{owner}:{:x}",
        Sha256::digest(metadata)
    )
}

/// Owned by one Snapshot; authenticated payloads never cross transactions.
#[derive(Default)]
pub(super) struct Publications {
    verified: std::collections::BTreeMap<(String, NativeReceipt), VerifiedPublication>,
}

impl Publications {
    pub(super) fn observation(
        &mut self,
        tx: &mut PgTxn<'_>,
        current: &QueryRuntimeBinding,
        original: &[u8],
        receipt: &NativeReceipt,
    ) -> Result<Option<PresentationObservations>, StorageError> {
        check_binding(current, receipt)?;
        #[cfg(test)]
        PUBLICATION_READS.with(|counts| {
            let (calls, fetches) = counts.get();
            counts.set((calls + 1, fetches));
        });
        let key = (current.namespace.as_str().to_owned(), receipt.clone());
        let publication = match self.verified.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let publication = load_publication(tx, current, receipt)?;
                #[cfg(test)]
                PUBLICATION_READS.with(|counts| {
                    let (calls, fetches) = counts.get();
                    counts.set((calls, fetches + 1));
                });
                entry.insert(publication)
            }
        };
        publication.observation(original)
    }
}

#[cfg(test)]
thread_local! {
    static PUBLICATION_READS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(in crate::storage::postgres) fn take_publication_reads() -> (usize, usize) {
    PUBLICATION_READS.with(|counts| counts.replace((0, 0)))
}

struct VerifiedPublication {
    rows: Vec<serde_json::Value>,
    observations: Option<PresentationObservations>,
}

impl VerifiedPublication {
    fn observation(
        &self,
        original: &[u8],
    ) -> Result<Option<PresentationObservations>, StorageError> {
        let original: serde_json::Value =
            serde_json::from_slice(original).map_err(|_| StorageError::ScopeChanged)?;
        if !self.rows.contains(&original) {
            return Err(StorageError::ScopeChanged);
        }
        Ok(self.observations.clone())
    }
}

fn check_binding(
    current: &QueryRuntimeBinding,
    receipt: &NativeReceipt,
) -> Result<(), StorageError> {
    if receipt.0 != current.admission || receipt.1 != current.stream_epoch {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(super) fn publication(
    tx: &mut PgTxn<'_>,
    current: &QueryRuntimeBinding,
    original: &[u8],
    receipt: &NativeReceipt,
) -> Result<Option<PresentationObservations>, StorageError> {
    check_binding(current, receipt)?;
    load_publication(tx, current, receipt)?.observation(original)
}

fn load_publication(
    tx: &mut PgTxn<'_>,
    current: &QueryRuntimeBinding,
    receipt: &NativeReceipt,
) -> Result<VerifiedPublication, StorageError> {
    let generation = &receipt.2;
    let ns = current.namespace.as_str().as_bytes();
    let operations = tx.driver.query("SELECT operation_id,CASE WHEN octet_length(payload)<=1048576 THEN payload END FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 2", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?;
    if operations.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let operation_id: Vec<u8> = operations[0].get(0);
    let payload = operations[0]
        .get::<_, Option<Vec<u8>>>(1)
        .ok_or(StorageError::Busy)?;
    if Sha256::digest(&payload).as_slice() != operation_id.as_slice()
        || format!("{:x}", Sha256::digest(&operation_id)) != *generation
    {
        return Err(StorageError::ScopeChanged);
    }
    crate::selected_read::charge(64, payload.len()).map_err(|_| StorageError::Busy)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|_| StorageError::ScopeChanged)?;
    let values = payload.as_array().ok_or(StorageError::ScopeChanged)?;
    if values.len() != 9 || values[0] != "catalog-publication-v1" {
        return Err(StorageError::ScopeChanged);
    }
    let rows = values[1]
        .as_array()
        .ok_or(StorageError::ScopeChanged)?
        .clone();
    let observations =
        serde_json::from_value(values[4].clone()).map_err(|_| StorageError::ScopeChanged)?;
    Ok(VerifiedPublication { rows, observations })
}

pub(super) fn issue_history(
    tx: &mut PgTxn<'_>,
    owner: &str,
    metadata: &[u8],
    lifecycle: crate::model::summary::CatalogLifecycle,
    receipt: NativeReceipt,
) -> Result<(), StorageError> {
    let key = history_key(owner, metadata);
    let value = encoded(&HistoryReference {
        version: 1,
        lifecycle,
        receipt,
    })?;
    let changed = tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_meta.namespace=excluded.namespace AND semon_pg_native_meta.key=excluded.key AND semon_pg_native_meta.value=excluded.value", &[&tx.namespace.as_str().as_bytes(),&key.as_bytes(),&value]).map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::ScopeChanged);
    }
    tx.check()
}
