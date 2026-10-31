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
    query_recipe: Option<&[u8]>,
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
    publication_observations(&payload, original, query_recipe)
}

pub(super) fn publication_observations(
    payload: &serde_json::Value,
    original: &[u8],
    query_recipe: Option<&[u8]>,
) -> Result<Option<PresentationObservations>, StorageError> {
    let values = payload.as_array().ok_or(StorageError::ScopeChanged)?;
    if values.len() != 9 || values[0] != "catalog-publication-v2" {
        return Err(StorageError::ScopeChanged);
    }
    let cells: Vec<(String, String, Option<String>)> =
        serde_json::from_str(values[1].as_str().ok_or(StorageError::ScopeChanged)?)
            .map_err(|_| StorageError::ScopeChanged)?;
    let original_row: CatalogRow =
        serde_json::from_slice(original).map_err(|_| StorageError::ScopeChanged)?;
    let mut keys = std::collections::BTreeSet::new();
    let mut exact = false;
    for (key, metadata, recipe) in cells {
        let row: CatalogRow =
            serde_json::from_str(&metadata).map_err(|_| StorageError::ScopeChanged)?;
        if row.key != key || !keys.insert(key.clone()) {
            return Err(StorageError::ScopeChanged);
        }
        crate::storage::proof_codec::validate_query_recipe_cell(&metadata, recipe.as_deref())?;
        exact |= key == original_row.key
            && metadata.as_bytes() == original
            && recipe.as_deref().map(str::as_bytes) == query_recipe;
    }
    if !exact {
        return Err(StorageError::ScopeChanged);
    }
    serde_json::from_value(values[4].clone()).map_err(|_| StorageError::ScopeChanged)
}

/// Read the exact selected owner's pair within the admitted transaction. The
/// publication receipt must bind these bytes, including insignificant whitespace.
pub(super) fn query_recipe_at(
    tx: &mut PgTxn<'_>,
    generation: &str,
    history: bool,
    key: &str,
    metadata: &[u8],
) -> Result<Option<Vec<u8>>, StorageError> {
    let table = if history {
        "semon_pg_native_catalog_history_headers"
    } else {
        "semon_pg_native_catalog_rows"
    };
    tx.check()?;
    let cells = tx.driver.query(&format!("SELECT octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576,CASE WHEN octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576 THEN query_recipe END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND metadata=$4"), &[&tx.namespace.as_str().as_bytes(), &generation.as_bytes(), &key.as_bytes(), &metadata]).map_err(sqlstate::error)?;
    if cells.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    if !cells[0].get::<_, bool>(0) {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let recipe = cells[0].get::<_, Option<Vec<u8>>>(1);
    crate::selected_read::charge(64, metadata.len() + recipe.as_ref().map_or(0, Vec::len))
        .map_err(|_| StorageError::Busy)?;
    tx.check()?;
    Ok(recipe)
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
