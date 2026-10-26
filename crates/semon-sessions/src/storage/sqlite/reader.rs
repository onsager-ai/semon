//! One admission path for native Catalog/Query reads. Validate inside the same
//! snapshot as payload SQL; never infer namespace or coordinates from Options.
use super::{failed, namespace};
use crate::storage::{LogicalSourceNamespace, NativeReaderBinding, StorageError, StoreBackend};
use rusqlite::{Connection, OpenFlags};
use std::{ops::Deref, time::Duration};

impl NativeReaderBinding {
    pub(crate) fn connection(&self) -> Result<Connection, StorageError> {
        let config = self.config();
        let StoreBackend::Sqlite { database } = &config.backend;
        if database.as_os_str().is_empty() || database == std::path::Path::new(":memory:") {
            return Err(StorageError::InvalidArguments);
        }
        if config.expected_stream_epoch.as_ref().is_none_or(|epoch| {
            epoch.len() != 64 || !epoch.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
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

    pub(crate) fn validate(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<bool, StorageError> {
        self.validate_listing(db, requested)
    }

    fn validate_listing(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<bool, StorageError> {
        let config = self.config();
        if requested != &config.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        match self {
            Self::Semantic(owner) => owner.validate_ready(db)?,
            Self::Admitted(_) => {
                namespace::require_admitted(db, requested)?;
            }
        }
        if !crate::events::store::current_contract(db).map_err(failed)? {
            return Err(StorageError::Unavailable);
        }
        let actual: String = db
            .query_row(
                "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
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
        super::catalog_namespace::scope_readers(db)
    }

    /// Repair and lazy proof readers retain this admitted snapshot until their
    /// last proof handle drops. There is no Options or alternate-store retry.
    pub(crate) fn proof_snapshot(
        &self,
    ) -> Result<std::sync::Arc<crate::events::store::chunk_proofs::Snapshot>, StorageError> {
        let db = self.connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db, &self.config().namespace)?;
        Ok(crate::events::store::chunk_proofs::Snapshot::new(db, self.clone()))
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

    /// Only the private Query listing adapter can consume producer custody.
    /// General Catalog, navigation, selected and proof factories still refuse.
    pub(super) fn listing_snapshot(
        &self,
        requested: &LogicalSourceNamespace,
    ) -> Result<ReadSnapshot, StorageError> {
        if !matches!(self, Self::Semantic(_)) {
            return self.snapshot(requested);
        }
        if requested != &self.config().namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let db = self.connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate_listing(&db, requested)?;
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
