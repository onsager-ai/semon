//! Private ordinary model producer metadata. This is not Catalog admission.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{SESSION_DESCRIPTION_BATCH_LIMIT, SessionDescriptionRequest},
    storage::{QueryRuntimeBinding, StorageError},
};

const MAX: usize = 1024 * 1024;

pub(super) fn initialize(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.check()?;
    tx.driver.execute(
        "INSERT INTO semon_pg_native_semantic_runtime(namespace,singleton,admission,stream_epoch) VALUES($1,true,1,convert_to(encode(sha256(convert_to(gen_random_uuid()::text,'UTF8')),'hex'),'UTF8')) ON CONFLICT DO NOTHING",
        &[&tx.namespace.as_str().as_bytes()],
    ).map_err(sqlstate::error)?;
    binding(tx).map(|_| ())
}

pub(super) fn binding(tx: &mut PgTxn<'_>) -> Result<QueryRuntimeBinding, StorageError> {
    if super::native_keys::read_meta(tx, b"source-unenrolled", 1)?
        .is_some_and(|value| !value.is_empty())
    {
        return Err(StorageError::NamespaceUnadmitted);
    }
    raw_binding(tx)
}

pub(super) fn raw_binding(tx: &mut PgTxn<'_>) -> Result<QueryRuntimeBinding, StorageError> {
    tx.check()?;
    let rows = tx.driver.query(
        "SELECT admission,CASE WHEN octet_length(stream_epoch)=64 THEN stream_epoch END FROM semon_pg_native_semantic_runtime WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton",
        &[&tx.namespace.as_str().as_bytes()],
    ).map_err(sqlstate::error)?;
    if rows.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let admission =
        u64::try_from(rows[0].get::<_, i64>(0)).map_err(|_| StorageError::ScopeChanged)?;
    let epoch = rows[0]
        .get::<_, Option<Vec<u8>>>(1)
        .ok_or(StorageError::ScopeChanged)?;
    let stream_epoch = String::from_utf8(epoch).map_err(|_| StorageError::ScopeChanged)?;
    if admission == 0 || !stream_epoch.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(QueryRuntimeBinding {
        namespace: tx.namespace.clone(),
        admission,
        stream_epoch,
    })
}

pub(super) fn generation(tx: &mut PgTxn<'_>) -> Result<Option<String>, StorageError> {
    tx.check()?;
    let row = tx.driver.query_one(
        "SELECT CASE WHEN octet_length(catalog_generation)=64 THEN catalog_generation END,catalog_generation IS NULL FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        &[&tx.namespace.as_str().as_bytes()],
    ).map_err(sqlstate::error)?;
    let value: Option<String> = row.get(0);
    if value
        .as_ref()
        .is_some_and(|s| !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || (value.is_none() && !row.get::<_, bool>(1))
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(value)
}

pub(super) fn descriptions(
    tx: &mut PgTxn<'_>,
    requests: &[SessionDescriptionRequest],
) -> Result<Vec<Option<String>>, StorageError> {
    if requests.len() > SESSION_DESCRIPTION_BATCH_LIMIT {
        return Err(StorageError::InvalidArguments);
    }
    let mut descriptions = Vec::with_capacity(requests.len());
    for request in requests {
        if request.key.len().saturating_add(request.inputs.len()) > MAX {
            return Err(StorageError::Busy);
        }
        tx.check()?;
        let rows = tx
            .driver
            .query(
                "SELECT session_key=$2,version=$3 AND inputs=$4,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_descriptions WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea)",
                &[
                    &tx.namespace.as_str().as_bytes(),
                    &request.key.as_bytes(),
                    &i64::from(request.version),
                    &request.inputs.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            descriptions.push(None);
            continue;
        };
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        if !row.get::<_, bool>(1) {
            descriptions.push(None);
            continue;
        }
        let bytes = row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?;
        crate::selected_read::charge(0, bytes.len()).map_err(|_| StorageError::Busy)?;
        descriptions.push(Some(
            String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)?,
        ));
    }
    Ok(descriptions)
}

pub(super) fn save_descriptions(
    tx: &mut PgTxn<'_>,
    descriptions: &[(String, u32, String, String)],
) -> Result<(), StorageError> {
    let mut bytes = 0usize;
    if descriptions.len() > 64 {
        return Err(StorageError::Busy);
    }
    for (key, version, inputs, metadata) in descriptions {
        bytes = bytes
            .checked_add(key.len())
            .and_then(|v| v.checked_add(inputs.len()))
            .and_then(|v| v.checked_add(metadata.len()))
            .ok_or(StorageError::Busy)?;
        if bytes > MAX {
            return Err(StorageError::Busy);
        }
        tx.check()?;
        let changed = tx.driver.execute(
            "INSERT INTO semon_pg_native_descriptions(namespace,key_digest,session_key,version,inputs,metadata) VALUES($1,sha256($2::bytea),$2,$3,$4,$5) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET version=excluded.version,inputs=excluded.inputs,metadata=excluded.metadata WHERE semon_pg_native_descriptions.namespace=excluded.namespace AND semon_pg_native_descriptions.session_key=excluded.session_key",
            &[&tx.namespace.as_str().as_bytes(), &key.as_bytes(), &i64::from(*version), &inputs.as_bytes(), &metadata.as_bytes()],
        ).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

pub(super) fn append_evidence(
    tx: &mut PgTxn<'_>,
    path: &str,
) -> Result<Option<crate::source_change::AppendEvidence>, StorageError> {
    tx.check()?;
    let rows = tx.driver.query(
        "SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence END FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2",
        &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()],
    ).map_err(sqlstate::error)?;
    rows.first()
        .map(|row| {
            let bytes = row
                .get::<_, Option<Vec<u8>>>(0)
                .ok_or(StorageError::ScopeChanged)?;
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)
        })
        .transpose()
}
