//! One admission path for native Catalog/Query reads. Validate inside the same
//! snapshot as payload SQL; never infer namespace or coordinates from Options.
use super::{failed, namespace};
use crate::storage::{LogicalSourceNamespace, NativeReaderBinding, StorageError};
use rusqlite::{Connection, OpenFlags};
use std::{ops::Deref, time::Duration};

impl NativeReaderBinding {
    pub(crate) fn runtime_binding_at(
        &self,
        db: &Connection,
    ) -> Result<crate::storage::QueryRuntimeBinding, StorageError> {
        self.validate(db, &self.config().namespace)?;
        let admission = match self {
            Self::Semantic(owner) => owner.admission(),
            Self::Admitted(config) => {
                namespace::require_admitted(db, &config.namespace)?.generation
            }
        };
        Ok(crate::storage::QueryRuntimeBinding {
            namespace: self.config().namespace.clone(),
            admission,
            stream_epoch: self
                .config()
                .expected_stream_epoch
                .clone()
                .ok_or(StorageError::ScopeChanged)?,
        })
    }
    pub(crate) fn connection(&self) -> Result<Connection, StorageError> {
        let config = self.config();
        let database = config.backend.sqlite_database()?;
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
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(failed)?;
        // A Catalog page authenticates rows through the native proof and owner
        // statements together; retain that working set for the whole snapshot.
        db.set_prepared_statement_cache_capacity(256);
        db.busy_timeout(Duration::from_millis(100))
            .map_err(failed)?;
        super::catalog_namespace::register(&db, &config.namespace)?;
        // Cache only a committed layout. Payload snapshots still check the
        // DDL cookie below, including a change between open and BEGIN.
        crate::events::store::configured::NativeOwnership::validate_physical(&db)?;
        Ok(db)
    }

    pub(crate) fn validate(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<bool, StorageError> {
        self.validate_listing(db, requested)
    }

    /// Readiness probes may report an unavailable derived search projection
    /// without opening any payload reader. All other admission fences remain
    /// active, and actual bindings still reject the stale projection.
    pub(crate) fn search_projection_stale(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<bool, StorageError> {
        self.validate_listing_state(db, requested)
            .map(|(stale, _)| stale)
    }

    fn validate_listing(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<bool, StorageError> {
        let (stale, readers) = self.validate_listing_state(db, requested)?;
        if stale {
            return Err(StorageError::NamespaceUpgradePending);
        }
        Ok(readers)
    }

    fn validate_listing_state(
        &self,
        db: &Connection,
        requested: &LogicalSourceNamespace,
    ) -> Result<(bool, bool), StorageError> {
        let config = self.config();
        if requested != &config.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let stale = match self {
            Self::Semantic(owner) => owner.validate_catalog_publication_ready(db)?,
            Self::Admitted(_) => {
                let admission = namespace::require_admitted(db, requested)?;
                crate::events::store::configured::NativeOwnership::validate_physical(db)?;
                crate::events::store::configured::validate_catalog_publication_profile(
                    db,
                    requested,
                    i64::try_from(admission.generation).map_err(|_| StorageError::ScopeChanged)?,
                )?
            }
        };
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
        let readers = super::catalog_namespace::scope_readers(db)?;
        Ok((stale, readers))
    }

    /// Repair and lazy proof readers retain this admitted snapshot until their
    /// last proof handle drops. There is no Options or alternate-store retry.
    pub(crate) fn proof_snapshot(
        &self,
    ) -> Result<std::sync::Arc<crate::events::store::chunk_proofs::Snapshot>, StorageError> {
        let db = self.connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db, &self.config().namespace)?;
        Ok(crate::events::store::chunk_proofs::Snapshot::new(
            db,
            self.clone(),
        ))
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

    /// Query listing uses the same native profile as default local metadata
    /// readers. Selected bodies separately qualify their row and byte proofs;
    /// listing eligibility never establishes complete source membership.
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
