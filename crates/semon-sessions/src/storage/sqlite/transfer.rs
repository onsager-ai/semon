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

pub(crate) fn page(config: &StoreConfig, manifest: &SnapshotManifest, sequence: u64) -> Result<TransferPage, StorageError> {
    transaction(config, |tx| transfer::page_tx(tx, manifest, sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{LogicalSourceNamespace, StoreBackend};

    #[test]
    fn current_transfer225_sqlite_current_inventory_and_atomic_checkpoint_failure() {
        let root = std::env::temp_dir().join(format!("semon-transfer225-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let config = StoreConfig { backend: StoreBackend::Sqlite { database: root.join("current.sqlite3") }, namespace: LogicalSourceNamespace::new("transfer225").unwrap(), expected_stream_epoch: Some("a".repeat(64)) };
        config.create_native_store().unwrap();
        crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
        let db = Connection::open(config.backend.sqlite_database().unwrap()).unwrap();
        let tables: Vec<String> = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 257").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert!(!tables.is_empty() && tables.len() < 257);
        for table in &tables { transfer::inventory::require(transfer::inventory::sqlite(table), table); }
        let manifest = transfer::tests::manifest();
        let inbox = config.transfer_inbox();
        let initial = inbox.begin(&manifest).unwrap();
        db.execute_batch("CREATE TRIGGER transfer225_checkpoint_fault BEFORE UPDATE ON meta WHEN NEW.key GLOB 'native_transfer_v1:*:checkpoint' BEGIN SELECT RAISE(ABORT,'injected transfer checkpoint failure'); END;").unwrap();
        let page = transfer::tests::page(&manifest, 1, [0; 32]);
        assert!(inbox.stage_page(&manifest, &page).is_err());
        assert_eq!(inbox.checkpoint(&manifest).unwrap(), initial);
        assert!(inbox.staged_page(&manifest, 1).is_err());
        db.execute_batch("DROP TRIGGER transfer225_checkpoint_fault").unwrap();
        assert_eq!(inbox.stage_page(&manifest, &page).unwrap().sequence, 1);
        assert_eq!(db.query_row("SELECT count(*) FROM files", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
