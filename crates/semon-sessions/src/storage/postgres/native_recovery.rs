//! Current native Source repair. No parser, Raw access or Catalog publication.
use super::{OperationIdentity, native_keys, source_journal, sqlstate, txn::PgTxn};
use crate::storage::{
    StorageError,
    source_observations::{
        SourceIntent, SourceMutationReceipt,
        recovery::{BYTES, Page},
    },
};
use sha2::{Digest, Sha256};

const CURSOR: &[u8] = b"catalog_recovery_after";

pub(super) fn cursor(tx: &mut PgTxn<'_>) -> Result<Option<String>, StorageError> {
    native_keys::read_meta(tx, CURSOR, 4096)?
        .map(String::from_utf8)
        .transpose()
        .map_err(|_| StorageError::ScopeChanged)
}

pub(super) fn head(tx: &mut PgTxn<'_>) -> Result<String, StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    // The locked Source head selects the immutable current generation. An
    // unbound head cannot stand in for a successfully recovered empty Catalog.
    let generation: Option<String> = tx.driver.query_one(
        "SELECT (SELECT catalog_generation FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)",
        &[&ns],
    ).map_err(sqlstate::error)?.try_get(0).map_err(|_| StorageError::ScopeChanged)?;
    let generation = generation.ok_or(StorageError::NamespaceUpgradePending)?;
    if generation.len() != 64 || !generation.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(generation)
}

fn page(tx: &mut PgTxn<'_>, after: &str) -> Result<bool, StorageError> {
    let generation = head(tx)?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let key = if after.is_empty() {
        ""
    } else {
        let (head, key) = after.split_once(':').ok_or(StorageError::ScopeChanged)?;
        if head != generation || key.len() != 128 || !key.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err(StorageError::ScopeChanged);
        }
        key
    };
    // Seek the actual immutable primary key. Retained rows still advance the
    // cursor; lifecycle filtering never hides a corrupt row or scans history.
    let filter = "FROM semon_pg_native_catalog_sources s JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=s.namespace_digest AND r.namespace=s.namespace AND r.generation=s.generation AND r.key_digest=s.key_digest AND r.session_key=s.session_key WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND (s.key_digest,s.path_digest)>(decode(substr($3::text,1,64),'hex'),decode(substr($3::text,65,64),'hex')) ORDER BY s.key_digest,s.path_digest LIMIT 64";
    let args: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&ns, &generation.as_bytes(), &key];
    let probe = tx
        .driver
        .query(
            &format!(
                "SELECT (octet_length(s.source_path)+octet_length(r.metadata)+128)::bigint {filter}"
            ),
            args,
        )
        .map_err(sqlstate::error)?;
    let mut bytes = 0usize;
    for row in &probe {
        bytes = bytes
            .checked_add(
                usize::try_from(
                    row.try_get::<_, i64>(0)
                        .map_err(|_| StorageError::ScopeChanged)?,
                )
                .map_err(|_| StorageError::ScopeChanged)?,
            )
            .ok_or(StorageError::Busy)?;
    }
    if bytes > BYTES {
        return Err(StorageError::Busy);
    }
    crate::selected_read::charge(probe.len() * 64, bytes).map_err(|_| StorageError::Busy)?;
    tx.check()?;
    let rows = tx.driver.query(&format!("SELECT encode(s.key_digest,'hex')||encode(s.path_digest,'hex'),s.source_path,r.metadata,s.session_key,s.path_digest {filter}"), args).map_err(sqlstate::error)?;
    if rows.len() != probe.len() {
        return Err(StorageError::ScopeChanged);
    }
    let entries = rows
        .iter()
        .map(|row| {
            let key: String = row.try_get(0).map_err(|_| StorageError::ScopeChanged)?;
            let path: Vec<u8> = row.try_get(1).map_err(|_| StorageError::ScopeChanged)?;
            let metadata: Vec<u8> = row.try_get(2).map_err(|_| StorageError::ScopeChanged)?;
            let owner: Vec<u8> = row.try_get(3).map_err(|_| StorageError::ScopeChanged)?;
            let path_digest: Vec<u8> = row.try_get(4).map_err(|_| StorageError::ScopeChanged)?;
            let catalog: crate::model::summary::CatalogRow =
                serde_json::from_slice(&metadata).map_err(|_| StorageError::ScopeChanged)?;
            if owner != catalog.key.as_bytes()
                || path_digest.as_slice() != &Sha256::digest(&path)[..]
                || key[..64] != format!("{:x}", Sha256::digest(&owner))
            {
                return Err(StorageError::ScopeChanged);
            }
            let path = String::from_utf8(path).map_err(|_| StorageError::ScopeChanged)?;
            if !catalog
                .sources
                .iter()
                .any(|source| source.path.to_str() == Some(path.as_str()))
            {
                return Err(StorageError::ScopeChanged);
            }
            Ok((
                key,
                if catalog.lifecycle == crate::model::summary::CatalogLifecycle::Current {
                    Some(path)
                } else {
                    None
                },
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let plan = Page::keyed(key, entries)?;
    // Qualify ALL selected custody markers before the first repair effect.
    for path in &plan.paths {
        tx.check()?;
        let key = format!("archive_event_index:{path}");
        let sizes = tx.driver.query("SELECT octet_length(value)::bigint FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND semon_pg_native_key_matches(key,key_digest,$2)", &[&ns, &key.as_bytes()]).map_err(sqlstate::error)?;
        if let Some(row) = sizes.first() {
            let size = usize::try_from(
                row.try_get::<_, i64>(0)
                    .map_err(|_| StorageError::ScopeChanged)?,
            )
            .map_err(|_| StorageError::ScopeChanged)?;
            bytes = bytes.checked_add(size).ok_or(StorageError::Busy)?;
            if bytes > BYTES {
                return Err(StorageError::Busy);
            }
        }
        if let Some(marker) = native_keys::read_meta(tx, key.as_bytes(), 16384)? {
            let marker = String::from_utf8(marker).map_err(|_| StorageError::ScopeChanged)?;
            crate::events::store::validate_archive_marker(&marker)?;
        }
    }
    for path in &plan.paths {
        let inserted = tx.driver.execute("INSERT INTO semon_pg_source_members(namespace,source_path) VALUES($1,$2) ON CONFLICT(namespace_digest,source_path) DO NOTHING", &[&ns, &path.as_bytes()]).map_err(sqlstate::error)?;
        boundary(tx)?;
        if inserted == 1 {
            source_journal::invalidate(tx, path)?;
            boundary(tx)?;
        }
    }
    if let Some(next) = &plan.next {
        native_keys::put_meta(tx, CURSOR, format!("{generation}:{next}").as_bytes())?;
    } else {
        native_keys::delete_meta(tx, CURSOR)?;
    }
    boundary(tx)?;
    Ok(plan.done)
}

pub(super) fn recover(
    tx: &mut PgTxn<'_>,
    intent: &SourceIntent,
) -> Result<(SourceMutationReceipt, bool), StorageError> {
    let SourceIntent::Recover {
        operation,
        cursor: expected,
    } = intent
    else {
        return Err(StorageError::InvalidArguments);
    };
    let prior = source_journal::receipt(tx, operation)?;
    if prior == Some(SourceMutationReceipt::Conflict) {
        return Ok((SourceMutationReceipt::Conflict, false));
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    if prior.is_none() {
        let payload = serde_json::to_string(intent).map_err(|_| StorageError::InvalidArguments)?;
        let row = tx.driver.query("SELECT payload=$3 FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2", &[&ns, &operation.id, &payload]).map_err(sqlstate::error)?;
        if row.len() != 1
            || !row[0]
                .try_get::<_, bool>(0)
                .map_err(|_| StorageError::ScopeChanged)?
        {
            return Err(StorageError::Conflict);
        }
    }
    let id = format!("{:x}", Sha256::digest(operation.id.as_bytes()));
    let request: [u8; 32] = Sha256::digest(operation.payload_digest.as_bytes()).into();
    let value = tx.apply_operation(&OperationIdentity { operation_id: &id, kind: "native-source-recovery", request_digest: &request }, |tx| {
        let current = cursor(tx)?;
        let applied = current == *expected;
        let done = if applied {
            if let Some(after) = current.as_deref() { page(tx, after)? } else { head(tx)?; true }
        } else { false };
        let outcome = if applied { "applied" } else { "conflict" };
        tx.driver.execute("INSERT INTO semon_pg_source_receipts(namespace,operation_id,payload_digest,outcome) VALUES($1,$2,$3,$4)", &[&ns, &operation.id, &operation.payload_digest, &outcome]).map_err(sqlstate::error)?;
        boundary(tx)?;
        tx.driver.execute("DELETE FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2 AND payload_digest=$3", &[&ns, &operation.id, &operation.payload_digest]).map_err(sqlstate::error)?;
        boundary(tx)?;
        Ok(serde_json::json!({"applied":applied,"done":done}))
    })?;
    let applied = value
        .get("applied")
        .and_then(serde_json::Value::as_bool)
        .ok_or(StorageError::ScopeChanged)?;
    let done = value
        .get("done")
        .and_then(serde_json::Value::as_bool)
        .ok_or(StorageError::ScopeChanged)?;
    // apply_operation does not re-run effects for a re-captured exact intent.
    if prior.is_some() {
        tx.driver.execute("DELETE FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2 AND payload_digest=$3", &[&ns, &operation.id, &operation.payload_digest]).map_err(sqlstate::error)?;
        boundary(tx)?;
    }
    Ok((
        if !applied {
            SourceMutationReceipt::Conflict
        } else if prior.is_some() {
            SourceMutationReceipt::AlreadyApplied
        } else {
            SourceMutationReceipt::Applied
        },
        done,
    ))
}

#[cfg(test)]
thread_local! {
    pub(super) static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(super) static BOUNDARIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn boundary(tx: &PgTxn<'_>) -> Result<(), StorageError> {
    #[cfg(test)]
    {
        BOUNDARIES.with(|v| v.set(v.get() + 1));
        if FAIL_AFTER.with(|v| match v.get() {
            Some(1) => {
                v.set(None);
                true
            }
            Some(n) => {
                v.set(Some(n - 1));
                false
            }
            None => false,
        }) {
            return Err(StorageError::Unavailable);
        }
    }
    tx.check()
}
