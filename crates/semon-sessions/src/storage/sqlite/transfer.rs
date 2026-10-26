//! Namespace-owned durable transfer transport, separate from native publication.
use super::failed;
use crate::storage::{StorageError, StoreConfig, transfer::{self, InboxTransaction, SnapshotManifest, TransferCheckpoint, TransferPage}};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::time::{Duration, Instant};

struct Inbox<'a>(&'a Connection, &'a str);

impl InboxTransaction for Inbox<'_> {
    fn get(&mut self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, StorageError> {
        let row: Option<Option<String>> = self.0.query_row(
            "SELECT CASE WHEN octet_length(value)<=?3 THEN value END FROM main.meta WHERE namespace=?1 AND key=?2",
            params![self.1, key, limit as i64], |row| row.get(0),
        ).optional().map_err(failed)?;
        row.map(|value| value.map(String::into_bytes).ok_or(StorageError::ScopeChanged)).transpose()
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let value = std::str::from_utf8(value).map_err(|_| StorageError::InvalidArguments)?;
        self.0.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", params![self.1, key, value]).map_err(failed)?;
        Ok(())
    }
}

fn transaction<T>(config: &StoreConfig, operation: impl FnOnce(&mut Inbox<'_>) -> Result<T, StorageError>) -> Result<T, StorageError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let owner = crate::events::store::configured::NativeOwnership::restore(config)?;
    let mut db = super::source_journal::SourceJournal::owned(owner.clone()).connection()?;
    db.progress_handler(1000, Some(move || Instant::now() >= deadline));
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
    owner.validate_ready(&tx)?;
    if Instant::now() >= deadline { return Err(StorageError::Busy); }
    let result = operation(&mut Inbox(&tx, config.namespace.as_str()))?;
    if Instant::now() >= deadline { return Err(StorageError::Busy); }
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    Ok(result)
}

pub(crate) fn begin(config: &StoreConfig, manifest: &SnapshotManifest) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, |tx| transfer::begin_tx(tx, manifest))
}

pub(crate) fn stage(config: &StoreConfig, manifest: &SnapshotManifest, page: &TransferPage, bytes: &[u8]) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, |tx| transfer::stage_tx(tx, manifest, page, bytes))
}
