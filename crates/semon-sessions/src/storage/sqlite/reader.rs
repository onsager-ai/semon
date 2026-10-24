//! One admission path for native Catalog/Query reads. Validate inside the same
//! snapshot as payload SQL; never infer namespace or coordinates from Options.
use super::{failed, namespace};
use crate::storage::{LogicalSourceNamespace, NativeReaderBinding, StorageError, StoreBackend};
use rusqlite::{Connection, OpenFlags};
use std::{ops::Deref, time::Duration};

impl NativeReaderBinding {
    pub(super) fn connection(&self) -> Result<Connection, StorageError> {
        let config = self.config();
        let StoreBackend::Sqlite { database } = &config.backend;
        if database.as_os_str().is_empty() || database == std::path::Path::new(":memory:") {
            return Err(StorageError::InvalidArguments);
        }
        if matches!(self, Self::Admitted(_))
            && config.expected_stream_epoch.as_ref().is_none_or(|epoch| {
                epoch.len() != 64 || !epoch.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(StorageError::InvalidArguments);
        }
        let db = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(failed)?;
        db.busy_timeout(Duration::from_millis(100))
            .map_err(failed)?;
        super::catalog_namespace::register(&db, &config.namespace)?;
        Ok(db)
    }

    pub(super) fn validate(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<(), StorageError> {
        let config = self.config();
        if requested != &config.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        match self {
            Self::Admitted(_) => {
                namespace::require_admitted(db, requested)?;
            }
            Self::TransitionalUnboundLocal(_) => {
                if namespace::is_bound(db).map_err(failed)? {
                    return Err(StorageError::NamespaceUpgradePending);
                }
            }
        }
        if !crate::events::store::current_contract(db).map_err(failed)? {
            return Err(StorageError::Unavailable);
        }
        let actual: String = db
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                [],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if config
            .expected_stream_epoch
            .as_ref()
            .is_some_and(|epoch| epoch != &actual)
        {
            return Err(StorageError::ScopeChanged);
        }
        if matches!(self, Self::Admitted(_)) {
            super::catalog_namespace::scope_readers(db)?;
        }
        Ok(())
    }

    pub(super) fn snapshot(
        &self,
        requested: &LogicalSourceNamespace,
    ) -> Result<ReadSnapshot, StorageError> {
        if requested != &self.config().namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let db = self.connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db, requested)?;
        Ok(ReadSnapshot(db))
    }
}

pub(super) struct ReadSnapshot(Connection);
impl Deref for ReadSnapshot {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.0
    }
}
impl Drop for ReadSnapshot {
    fn drop(&mut self) {
        let _ = self.0.execute_batch("ROLLBACK");
    }
}
