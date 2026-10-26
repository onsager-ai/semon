//! RLS-bound transfer inbox. Uses the ordinary admission/publication lock order.
use super::{
    PgCancellation, PgPool, model_cache, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{
    StorageError, StoreConfig,
    transfer::{self, InboxTransaction, SnapshotManifest, TransferCheckpoint, TransferPage},
};

struct Inbox<'a, 'b>(&'a mut PgTxn<'b>);

impl InboxTransaction for Inbox<'_, '_> {
    fn get(&mut self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, StorageError> {
        self.0.check()?;
        let rows = self.0.driver.query("SELECT key=$2,CASE WHEN octet_length(value)<=$3::bigint THEN value END FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea)", &[&self.0.namespace.as_str().as_bytes(), &key.as_bytes(), &(limit as i64)]).map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        row.get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::ScopeChanged)
            .map(Some)
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.0.check()?;
        let changed = self.0.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_meta.namespace=excluded.namespace AND semon_pg_native_meta.key=excluded.key", &[&self.0.namespace.as_str().as_bytes(), &key.as_bytes(), &value]).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

fn transaction<T>(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    operation: impl FnOnce(&mut Inbox<'_, '_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let bounded = control.bounded(std::time::Instant::now() + std::time::Duration::from_secs(2));
    pool.transaction(&config.namespace, TxnKind::Write, &bounded, |tx| {
        let binding = model_cache::binding(tx)?;
        if config.expected_stream_epoch.as_deref() != Some(binding.stream_epoch.as_str()) {
            return Err(StorageError::ScopeChanged);
        }
        operation(&mut Inbox(tx))
    })
}

pub(crate) fn begin(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| transfer::begin_tx(tx, manifest))
}

pub(crate) fn stage(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
    page: &TransferPage,
    bytes: &[u8],
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| {
        transfer::stage_tx(tx, manifest, page, bytes)
    })
}

pub(crate) fn page(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<TransferPage, StorageError> {
    transaction(config, pool, control, |tx| {
        transfer::page_tx(tx, manifest, sequence)
    })
}

pub(crate) fn checkpoint(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| {
        transfer::checkpoint_tx(tx, manifest)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{LogicalSourceNamespace, StoreBackend};

    #[test]
    fn pg_current_transfer225_inbox_restart_exact_replay_rls_and_unknown_commit() {
        let (pool, factory) = super::super::tests::fixture(|_| {});
        let control = PgCancellation::default();
        let config = StoreConfig {
            backend: StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            },
            namespace: LogicalSourceNamespace::new("transfer225").unwrap(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        let tables = factory.client().query("SELECT relname FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relkind='r' AND relname LIKE 'semon_pg_%' ORDER BY relname LIMIT 257", &[]).unwrap();
        assert!(!tables.is_empty() && tables.len() < 257);
        for table in tables {
            let name: String = table.get(0);
            crate::storage::transfer::inventory::require(
                crate::storage::transfer::inventory::postgres(&name),
                &name,
            );
        }
        crate::storage::transfer::tests::exercise(&config);
        let mut manifest = crate::storage::transfer::tests::manifest();
        manifest.revision += 1;
        manifest.seal().unwrap();
        let inbox = config.transfer_inbox();
        inbox.begin(&manifest).unwrap();
        let page = crate::storage::transfer::tests::page(&manifest, 1, [0; 32]);
        super::super::tests::LOSE_ACK.with(|flag| flag.set(true));
        assert!(matches!(
            inbox.stage_page(&manifest, &page),
            Err(StorageError::CommitUncertain)
        ));
        assert_eq!(inbox.stage_page(&manifest, &page).unwrap().sequence, 1);
        assert_eq!(inbox.checkpoint(&manifest).unwrap().sequence, 1);
        let foreign = LogicalSourceNamespace::new("foreign225").unwrap();
        pool.transaction(&foreign, TxnKind::ReadSnapshot, &control, |tx| {
            let count: i64 = tx
                .driver
                .query_one("SELECT count(*) FROM semon_pg_native_meta", &[])
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(count, 0, "forced RLS must conceal the entire staged inbox");
            Ok(())
        })
        .unwrap();
    }
}
