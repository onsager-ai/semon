//! Single-source SQLite adapter for the metadata contract.
pub(crate) mod catalog;
pub(crate) mod catalog_namespace;
pub(crate) mod changes;
pub(crate) mod configured_index;
pub(crate) mod history;
pub(crate) mod namespace;
pub(crate) mod navigation;
pub(crate) mod proof_generations;
pub(crate) mod publication;
pub(crate) mod query;
pub(crate) mod search;
pub(crate) mod selected;
pub(crate) mod source_journal;
pub(crate) mod trace;
pub(crate) use configured_index::open as open_configured_index;
#[cfg(test)]
pub(crate) use configured_index::open_private_semantic;
mod reader;
pub(crate) mod semantic;

use super::{
    CatalogChangesRequest, LogicalSourceNamespace, MetadataReadSnapshot, SessionMetadataStore,
    StorageError, StoreBackend, StoreConfig,
};
use crate::{CatalogChangeCompaction, CatalogChangePage, CatalogReadScope};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use std::time::Duration;

pub(super) fn failed(error: rusqlite::Error) -> StorageError {
    #[cfg(test)]
    semantic::diagnose(&error);
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => StorageError::Busy,
        _ => StorageError::Unavailable,
    }
}

pub(super) struct SqliteMetadataStore(StoreConfig, bool);

impl SqliteMetadataStore {
    pub(super) fn new(config: StoreConfig) -> Self {
        Self(config, false)
    }

    pub(super) fn legacy_local(config: StoreConfig) -> Self {
        Self(config, true)
    }

    fn admit(
        &self,
        namespace: &LogicalSourceNamespace,
        write: bool,
    ) -> Result<Connection, StorageError> {
        if namespace != &self.0.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let StoreBackend::Sqlite { database } = &self.0.backend;
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        // Never CREATE, URI, in-memory, repair, or alternate-engine fallback.
        let db = Connection::open_with_flags(database, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(failed)?;
        db.busy_timeout(Duration::from_millis(100))
            .map_err(failed)?;
        Ok(db)
    }

    fn check_epoch(&self, db: &Connection) -> Result<(), StorageError> {
        if self.1 {
            if namespace::is_bound(db).map_err(failed)? {
                return Err(StorageError::NamespaceUpgradePending);
            }
        } else {
            namespace::require_admitted(db, &self.0.namespace)?;
        }
        if !crate::events::store::current_contract(db).map_err(failed)? {
            return Err(StorageError::Unavailable);
        }
        // This read establishes the snapshot even when discovery has no expected epoch.
        let epoch: String = db
            .query_row(
                crate::events::store::configured::metadata_sql(
                    db,
                    "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                    "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                )
                .map_err(failed)?,
                [],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if epoch.len() != 64 || !epoch.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StorageError::Unavailable);
        }
        if self
            .0
            .expected_stream_epoch
            .as_ref()
            .is_some_and(|expected| expected != &epoch)
        {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

struct Snapshot {
    db: Connection,
    namespace: LogicalSourceNamespace,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // Connection close also rolls back if rollback itself fails.
        let _ = self.db.execute_batch("ROLLBACK");
    }
}

impl MetadataReadSnapshot for Snapshot {
    fn catalog_changes(
        &mut self,
        namespace: &LogicalSourceNamespace,
        request: CatalogChangesRequest<'_>,
    ) -> Result<CatalogChangePage, StorageError> {
        if namespace != &self.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        request.validate()?;
        changes::read_snapshot(
            &self.db,
            namespace.as_str(),
            request.scope,
            request.stream_epoch,
            request.after,
            request.limit,
        )
    }
}

impl SessionMetadataStore for SqliteMetadataStore {
    fn read_snapshot(
        &self,
        namespace: &LogicalSourceNamespace,
    ) -> Result<Box<dyn MetadataReadSnapshot>, StorageError> {
        let db = self.admit(namespace, false)?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.check_epoch(&db)?;
        if !self.1 {
            catalog_namespace::register(&db, namespace)?;
            catalog_namespace::scope_readers(&db)?;
        }
        Ok(Box::new(Snapshot {
            db,
            namespace: namespace.clone(),
        }))
    }

    fn compact_catalog_changes(
        &self,
        namespace: &LogicalSourceNamespace,
        scope: CatalogReadScope,
        stream_epoch: &str,
        acknowledged_through: u64,
        limit: usize,
    ) -> Result<CatalogChangeCompaction, StorageError> {
        if limit == 0 || limit > 256 || acknowledged_through > i64::MAX as u64 {
            return Err(StorageError::InvalidArguments);
        }
        if self
            .0
            .expected_stream_epoch
            .as_deref()
            .is_some_and(|expected| expected != stream_epoch)
        {
            return Err(StorageError::ScopeChanged);
        }
        let mut db = self.admit(namespace, true)?;
        if namespace::is_bound(&db).map_err(failed)? {
            namespace::require_admitted(&db, namespace)?;
            return Err(StorageError::NamespaceUpgradePending);
        }
        if !self.1 {
            return Err(StorageError::NamespaceUnadmitted);
        }
        changes::compact(&mut db, scope, stream_epoch, acknowledged_through, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::conformance::{MetadataFixture, metadata_conformance};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture {
        root: PathBuf,
        db: Connection,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-storage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            let db = Connection::open(root.join("store.sqlite3")).unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            db.execute_batch(crate::events::store::metadata_schema(false))
                .unwrap();
            db.execute_batch("CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY,metadata TEXT); CREATE TABLE session_history_catalog(session_key TEXT PRIMARY KEY,metadata TEXT);").unwrap();
            db.execute_batch(changes::SCHEMA).unwrap();
            db.pragma_update(None, "user_version", crate::events::store::SCHEMA_VERSION)
                .unwrap();
            for (key, value) in [
                ("cache_version", crate::events::CACHE_VERSION.to_string()),
                (
                    "catalog_version",
                    crate::model::summary::CATALOG_VERSION.to_string(),
                ),
                ("catalog_generation", "a".repeat(64)),
            ] {
                db.execute("INSERT INTO meta VALUES(?1,?2)", (key, value))
                    .unwrap();
            }
            Self { root, db }
        }
        fn config(&self) -> StoreConfig {
            StoreConfig {
                backend: StoreBackend::Sqlite {
                    database: self.root.join("store.sqlite3"),
                },
                namespace: self.namespace(),
                expected_stream_epoch: None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    impl MetadataFixture for Fixture {
        fn store(&self) -> Box<dyn SessionMetadataStore> {
            self.config().open_local().unwrap()
        }
        fn namespace(&self) -> LogicalSourceNamespace {
            LogicalSourceNamespace::new("authorized-source").unwrap()
        }
        fn publish(&mut self, key: &str) {
            self.db
                .execute("INSERT INTO session_catalog VALUES(?1,'metadata')", [key])
                .unwrap();
        }
        fn restart(&mut self) {
            self.db = Connection::open(self.root.join("store.sqlite3")).unwrap();
        }
    }
    #[test]
    fn sqlite_metadata_contract_conformance() {
        metadata_conformance(&mut Fixture::new());
    }

    #[test]
    fn configured_store_refuses_replaced_epoch_and_missing_storage_without_creating_it() {
        let fixture = Fixture::new();
        let mut config = fixture.config();
        config.expected_stream_epoch = Some("0".repeat(64));
        assert!(matches!(
            config
                .open_local()
                .unwrap()
                .read_snapshot(&fixture.namespace()),
            Err(StorageError::ScopeChanged)
        ));
        let mut config = fixture.config();
        let absent = fixture.root.join("absent.sqlite3");
        config.backend = StoreBackend::Sqlite {
            database: absent.clone(),
        };
        let store = config.open_local().unwrap();
        assert!(matches!(
            store.read_snapshot(&fixture.namespace()),
            Err(StorageError::Unavailable)
        ));
        assert!(!absent.exists());
        assert_eq!(
            store.compact_catalog_changes(
                &fixture.namespace(),
                CatalogReadScope::Current,
                &"a".repeat(64),
                0,
                1
            ),
            Err(StorageError::Unavailable)
        );
        assert!(!absent.exists());
    }
}
