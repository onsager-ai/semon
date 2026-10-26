//! Host-owned namespace enrollment. Revocation preserves every native row.
use super::{
    PgCancellation, PgPool, model_cache, native_keys, sqlstate, store_metadata, txn::TxnKind,
};
use crate::storage::{NamespaceAdmission, StorageError, StoreConfig};

fn epoch(config: &StoreConfig) -> Result<&str, StorageError> {
    config
        .expected_stream_epoch
        .as_deref()
        .filter(|epoch| epoch.len() == 64 && epoch.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or(StorageError::InvalidArguments)
}

pub(crate) fn admit(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
) -> Result<NamespaceAdmission, StorageError> {
    let expected = epoch(config)?;
    pool.transaction(&config.namespace, TxnKind::StoreWrite, control, |tx| {
        store_metadata::authorize(tx)?;
        let binding = model_cache::raw_binding(tx)?;
        let revoked = native_keys::read_meta(tx, b"source-unenrolled", 1)?.is_some_and(|value| !value.is_empty());
        if (!revoked && binding.stream_epoch != expected) || (revoked && binding.stream_epoch == expected) {
            return Err(StorageError::ScopeChanged);
        }
        let prior = native_keys::read_meta(tx, b"source-admitted-at", 32)?;
        let admitted_at = if revoked || prior.is_none() {
            crate::model::now_ms()
        } else {
            std::str::from_utf8(prior.as_ref().unwrap()).ok().and_then(|value| value.parse::<i64>().ok()).ok_or(StorageError::ScopeChanged)?
        };
        let generation = if revoked {
            let next = binding.admission.checked_add(1).and_then(|value| i64::try_from(value).ok()).ok_or(StorageError::ScopeChanged)?;
            let changed = tx.driver.execute("UPDATE semon_pg_native_semantic_runtime SET admission=$2,stream_epoch=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND admission=$4", &[&config.namespace.as_str().as_bytes(), &next, &expected.as_bytes(), &(binding.admission as i64)]).map_err(sqlstate::error)?;
            if changed != 1 { return Err(StorageError::Conflict); }
            next as u64
        } else { binding.admission };
        native_keys::put_meta(tx, b"source-admitted-at", admitted_at.to_string().as_bytes())?;
        native_keys::put_meta(tx, b"source-unenrolled", b"")?;
        Ok(NamespaceAdmission { namespace: config.namespace.clone(), generation, admitted_at })
    })
}

pub(crate) fn unenroll(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    expected: &NamespaceAdmission,
) -> Result<(), StorageError> {
    if expected.namespace != config.namespace {
        return Err(StorageError::NamespaceMismatch);
    }
    let expected_epoch = epoch(config)?;
    // The existing exclusive admission lock waits for all source transactions
    // before revocation. Future source transactions validate their pinned epoch.
    pool.transaction(&config.namespace, TxnKind::StoreWrite, control, |tx| {
        store_metadata::authorize(tx)?;
        let binding = model_cache::binding(tx)?;
        let admitted_at = native_keys::read_meta(tx, b"source-admitted-at", 32)?;
        if binding.admission != expected.generation
            || binding.stream_epoch != expected_epoch
            || admitted_at.as_deref() != Some(expected.admitted_at.to_string().as_bytes())
        {
            return Err(StorageError::ScopeChanged);
        }
        for key in [
            b"native_transfer_v1:source-freeze".as_slice(),
            b"native_transfer_v1:incoming".as_slice(),
        ] {
            if native_keys::read_meta(tx, key, 1024 * 1024)?.is_some_and(|value| !value.is_empty())
            {
                return Err(StorageError::Conflict);
            }
        }
        native_keys::put_meta(tx, b"source-unenrolled", b"1")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pg_vertical140_namespace233_revokes_open_and_future_capabilities() {
        let (pool, _) = super::super::tests::fixture(|_| {});
        let namespace = super::super::tests::ns("namespace233\0🦀");
        let control = PgCancellation::default();
        let config = StoreConfig {
            backend: crate::storage::StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            },
            namespace: namespace.clone(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        let admission = config.admit_namespace().unwrap();
        assert_eq!(config.admit_namespace().unwrap(), admission);
        let pinned = pool
            .bind_source(&namespace, Some(&"a".repeat(64)), &control)
            .unwrap();
        let journal = config.source_observations().unwrap();
        let mut writer = config.native_events().unwrap();
        assert!(journal.recovery_cursor().is_ok());
        pinned
            .transaction(&namespace, TxnKind::ReadSnapshot, &control, |_| Ok(()))
            .unwrap();
        config.unenroll_namespace(&admission).unwrap();
        for kind in [TxnKind::ReadSnapshot, TxnKind::Write] {
            assert_eq!(
                pinned
                    .transaction(&namespace, kind, &control, |_| Ok(()))
                    .unwrap_err(),
                StorageError::NamespaceUnadmitted
            );
        }
        assert_eq!(
            journal.recovery_cursor().unwrap_err(),
            StorageError::NamespaceUnadmitted
        );
        // The production writer must reject its old capability before touching
        // Raw, even when the requested file does not exist.
        let absent = std::path::Path::new("/namespace233-absent.jsonl");
        assert_eq!(
            writer.ingest(&"c".repeat(64), absent, "claude").unwrap_err(),
            StorageError::NamespaceUnadmitted
        );
        assert!(config.native_events().is_err());
        assert!(config.source_observations().is_err());
        assert_eq!(
            config.admit_namespace().unwrap_err(),
            StorageError::ScopeChanged
        );
        let mut successor = config.clone();
        successor.expected_stream_epoch = Some("b".repeat(64));
        let next = successor.admit_namespace().unwrap();
        assert_eq!(next.generation, admission.generation + 1);
        assert_eq!(
            config.unenroll_namespace(&admission).unwrap_err(),
            StorageError::ScopeChanged
        );
        assert!(
            config.native_events().is_err(),
            "future old hosts cannot adopt the successor epoch"
        );
        assert_eq!(
            writer.ingest(&"c".repeat(64), absent, "claude").unwrap_err(),
            StorageError::ScopeChanged
        );
        for kind in [TxnKind::ReadSnapshot, TxnKind::Write] {
            assert_eq!(
                pinned
                    .transaction(&namespace, kind, &control, |_| Ok(()))
                    .unwrap_err(),
                StorageError::ScopeChanged
            );
        }
        let current = pool
            .bind_source(&namespace, Some(&"b".repeat(64)), &control)
            .unwrap();
        current
            .transaction(&namespace, TxnKind::Write, &control, |_| Ok(()))
            .unwrap();
        assert!(successor.native_events().is_ok());
        let foreign = super::super::tests::ns("namespace233-other");
        assert_eq!(
            current
                .transaction(&foreign, TxnKind::ReadSnapshot, &control, |_| Ok(()))
                .unwrap_err(),
            StorageError::NamespaceMismatch
        );
    }
}
