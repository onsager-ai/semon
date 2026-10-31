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
pub(crate) mod transfer;
pub(crate) use configured_index::open as open_configured_index;
#[cfg(test)]
pub(crate) use configured_index::open_private_semantic;
mod reader;
pub(crate) mod semantic;

use super::{
    CatalogChangesRequest, LogicalSourceNamespace, MetadataReadSnapshot, SessionMetadataStore,
    StorageError, StoreConfig,
};
use crate::{CatalogChangeCompaction, CatalogChangePage, CatalogReadScope};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use std::time::Duration;

fn error_class(error: &rusqlite::Error) -> (&'static str, Option<i32>) {
    let kind = match error {
        rusqlite::Error::SqliteFailure(..) => "sqlite_failure",
        rusqlite::Error::QueryReturnedNoRows => "no_rows",
        rusqlite::Error::UserFunctionError(_) => "user_function",
        rusqlite::Error::InvalidQuery => "invalid_query",
        rusqlite::Error::InvalidColumnType(..) => "column_type",
        rusqlite::Error::FromSqlConversionFailure(..) => "from_sql",
        _ => "other",
    };
    (kind, error.sqlite_error().map(|error| error.extended_code))
}

pub(super) fn failed(error: rusqlite::Error) -> StorageError {
    #[cfg(test)]
    {
        semantic::diagnose(&error);
        // Static kind and code only: SQLite messages can contain SQL or native data.
        let (kind, extended_code) = error_class(&error);
        eprintln!(
            "storage_sqlite_failure kind={kind} sqlite_extended_code={:?}",
            extended_code
        );
    }
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => StorageError::Busy,
        _ => StorageError::Unavailable,
    }
}

pub(super) struct SqliteMetadataStore(StoreConfig);

impl SqliteMetadataStore {
    pub(super) fn new(config: StoreConfig) -> Self {
        Self(config)
    }

    fn admit(
        &self,
        namespace: &LogicalSourceNamespace,
        write: bool,
    ) -> Result<Connection, StorageError> {
        if namespace != &self.0.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let database = self.0.backend.sqlite_database()?;
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
        namespace::require_admitted(db, &self.0.namespace)?;
        if !crate::events::store::current_contract(db).map_err(failed)? {
            return Err(StorageError::Unavailable);
        }
        // This read establishes the snapshot even when discovery has no expected epoch.
        let epoch: String = db
            .query_row(
                "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
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
        catalog_namespace::register(&db, namespace)?;
        catalog_namespace::scope_readers(&db)?;
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
        if namespace != &self.0.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        // Compaction is an owned Catalog effect. A plain metadata connection
        // cannot mutate the protected hints or their durable cursor floors.
        let owner = crate::events::store::configured::NativeOwnership::restore(&self.0)?;
        let mut db = semantic::pricing_connection(&owner)?;
        namespace::require_admitted(&db, namespace)?;
        catalog_namespace::register(&db, namespace)?;
        changes::compact(&mut db, scope, stream_epoch, acknowledged_through, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoreBackend;
    use crate::storage::conformance::{MetadataFixture, metadata_conformance};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
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
            // The current factory creates the store; the namespace is
            // admitted and written only through the current qualified writer.
            let cache = root.join("index.json");
            let namespace = LogicalSourceNamespace::new("authorized-source").unwrap();
            let source =
                crate::storage::HostSource::new(namespace.clone(), "a".repeat(64)).unwrap();
            drop(crate::events::EventCache::open_scoped(&cache, &source).unwrap());
            let database = crate::events::EventCache::path(&cache);
            let epoch = Connection::open(&database)
                .unwrap()
                .query_row(
                    "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let config = StoreConfig {
                backend: StoreBackend::Sqlite { database },
                namespace,
                expected_stream_epoch: Some(epoch),
            };
            let db = semantic::Stage::open(semantic::SemanticStaging::admit(&config).unwrap())
                .unwrap()
                .into_test_connection();
            for (key, value) in [
                (
                    "catalog_version",
                    crate::model::summary::CATALOG_VERSION.to_string(),
                ),
                ("catalog_generation", "a".repeat(64)),
            ] {
                db.execute("INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", (key, value)).unwrap();
            }
            Self { root, config, db }
        }
        fn config(&self) -> StoreConfig {
            self.config.clone()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    impl MetadataFixture for Fixture {
        fn store(&self) -> Box<dyn SessionMetadataStore> {
            self.config().open().unwrap()
        }
        fn namespace(&self) -> LogicalSourceNamespace {
            LogicalSourceNamespace::new("authorized-source").unwrap()
        }
        fn publish(&mut self, key: &str) {
            let row: crate::model::summary::CatalogRow =
                serde_json::from_value(serde_json::json!({
                    "key":key,"name":key,"harness":"claude","kind":"session",
                    "native_ids":[key],"parent":null,"parent_source":null,"repo":"project",
                    "branch":null,"model":"","effort":null,"start":null,"last":0,
                    "tokens":[0,0,0],"cost":crate::pricing::Cost::default(),"sources":[]
                }))
                .unwrap();
            self.db
                .execute("INSERT INTO main.session_catalog(namespace,session_key,last_ms,harness,repo,metadata) VALUES(semon_namespace(),?1,0,'claude','project',?2)", (key, serde_json::to_string(&row).unwrap()))
                .unwrap();
        }
        fn restart(&mut self) {
            self.db =
                semantic::Stage::open(semantic::SemanticStaging::admit(&self.config).unwrap())
                    .unwrap()
                    .into_test_connection();
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
            config.open().unwrap().read_snapshot(&fixture.namespace()),
            Err(StorageError::ScopeChanged)
        ));
        let mut config = fixture.config();
        let absent = fixture.root.join("absent.sqlite3");
        config.backend = StoreBackend::Sqlite {
            database: absent.clone(),
        };
        let store = config.open().unwrap();
        assert!(matches!(
            store.read_snapshot(&fixture.namespace()),
            Err(StorageError::Unavailable)
        ));
        assert!(!absent.exists());
        assert_eq!(
            store.compact_catalog_changes(
                &fixture.namespace(),
                CatalogReadScope::Current,
                fixture.config.expected_stream_epoch.as_deref().unwrap(),
                0,
                1
            ),
            Err(StorageError::Unavailable)
        );
        assert!(!absent.exists());
    }
}
