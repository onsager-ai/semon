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

pub(super) fn publication(
    tx: &mut PgTxn<'_>,
    current: &QueryRuntimeBinding,
    original: &[u8],
    receipt: &NativeReceipt,
) -> Result<Option<PresentationObservations>, StorageError> {
    let (admission, stream, generation) = receipt;
    if *admission != current.admission || stream != &current.stream_epoch {
        return Err(StorageError::ScopeChanged);
    }
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
    let original: serde_json::Value =
        serde_json::from_slice(original).map_err(|_| StorageError::ScopeChanged)?;
    let values = payload.as_array().ok_or(StorageError::ScopeChanged)?;
    if values.len() != 9
        || payload[0] != "catalog-publication-v1"
        || !payload[1]
            .as_array()
            .is_some_and(|rows| rows.contains(&original))
    {
        return Err(StorageError::ScopeChanged);
    }
    serde_json::from_value(values[4].clone()).map_err(|_| StorageError::ScopeChanged)
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
