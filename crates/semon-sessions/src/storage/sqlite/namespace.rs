//! Current-source admission and a closed public multi-source writer gate.
//! Native creation owns namespace custody; public readers never adopt rows.
use super::failed;
use crate::storage::{LogicalSourceNamespace, NamespaceAdmission, StorageError, StoreConfig};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::time::Duration;

/// Zero deliberately means no native writer qualification. Never infer readiness
/// from reader ports or from a mutable database flag. Raising this version needs
/// the real same-store native writer conformance and complete table qualification.
pub(crate) const WRITER_QUALIFICATION_VERSION: u32 = 0;
const FORMAT_VERSION: u32 = 1;
pub(crate) const REGISTRY_SCHEMA: &str = "CREATE TABLE storage_namespaces(namespace TEXT PRIMARY KEY,admission_generation INTEGER NOT NULL CHECK(admission_generation>0),admitted_at INTEGER NOT NULL,format_version INTEGER NOT NULL,writer_qualification INTEGER NOT NULL) STRICT; CREATE TABLE storage_writer_inventory(table_name TEXT PRIMARY KEY,qualified_version INTEGER NOT NULL) STRICT;";

pub(crate) fn is_bound(db: &Connection) -> rusqlite::Result<bool> {
    db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='storage_namespaces')")?.query_row([], |row| row.get(0))
}

/// Private native ownership validates one explicit admission without changing
/// the public sole-Source rule. This is never selected by Options/config parsing.
pub(crate) fn require_semantic_admitted(
    db: &Connection,
    namespace: &LogicalSourceNamespace,
) -> Result<NamespaceAdmission, StorageError> {
    let row: Option<(i64,i64,u32,u32)> = db.query_row("SELECT admission_generation,admitted_at,format_version,writer_qualification FROM storage_namespaces WHERE namespace=?1",[namespace.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failed)?;
    let Some((generation, admitted_at, version, qualification)) = row else {
        return Err(StorageError::ScopeChanged);
    };
    if generation <= 0 || version != FORMAT_VERSION || qualification != WRITER_QUALIFICATION_VERSION
    {
        return Err(StorageError::NamespaceUpgradePending);
    }
    Ok(NamespaceAdmission {
        namespace: namespace.clone(),
        generation: generation as u64,
        admitted_at,
    })
}

pub(crate) fn require_admitted(
    db: &Connection,
    namespace: &LogicalSourceNamespace,
) -> Result<NamespaceAdmission, StorageError> {
    if !is_bound(db).map_err(failed)? {
        return Err(StorageError::NamespaceUnadmitted);
    }
    let rows: Vec<(String, i64, i64, u32)> = db.prepare("SELECT namespace,admission_generation,admitted_at,format_version FROM storage_namespaces ORDER BY namespace LIMIT 2").map_err(failed)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).map_err(failed)?
        .collect::<rusqlite::Result<_>>().map_err(failed)?;
    let [(name, generation, admitted_at, version)] = rows.as_slice() else {
        return Err(StorageError::MultiSourceUnqualified);
    };
    if *version != FORMAT_VERSION || *generation <= 0 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    if name != namespace.as_str() {
        return Err(StorageError::NamespaceMismatch);
    }
    Ok(NamespaceAdmission {
        namespace: namespace.clone(),
        generation: *generation as u64,
        admitted_at: *admitted_at,
    })
}

pub(crate) fn admit(config: &StoreConfig) -> Result<NamespaceAdmission, StorageError> {
    let expected = config
        .expected_stream_epoch
        .as_deref()
        .filter(|epoch| epoch.len() == 64 && epoch.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(StorageError::InvalidArguments)?;
    let database = config.backend.sqlite_database()?;
    let mut db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(failed)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(failed)?;
    // Bind current namespace statements; no physical state is created here.
    super::catalog_namespace::register(&db, &config.namespace)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let epoch: String = tx
        .query_row(
            "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if epoch != expected {
        return Err(StorageError::ScopeChanged);
    }
    if is_bound(&tx).map_err(failed)? {
        let admission = match require_admitted(&tx, &config.namespace) {
            Err(StorageError::NamespaceMismatch) => Err(StorageError::MultiSourceUnqualified),
            other => other,
        }?;
        if !crate::events::store::current_contract(&tx).map_err(failed)? {
            return Err(StorageError::NamespaceUpgradePending);
        }
        crate::events::store::configured::NativeOwnership::validate_physical(&tx)?;
        crate::events::store::configured::validate_read_profile(
            &tx,
            &config.namespace,
            i64::try_from(admission.generation).map_err(|_| StorageError::ScopeChanged)?,
        )?;
        super::changes::initialize_source_floors(&tx, &config.namespace)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        return Ok(admission);
    }
    // Current native stores are created with their namespace custody. Public
    // admission cannot adopt an unowned store or install a second runtime.
    Err(StorageError::NamespaceUnadmitted)
}

pub(crate) fn unenroll(
    config: &StoreConfig,
    expected: &NamespaceAdmission,
) -> Result<(), StorageError> {
    if expected.namespace != config.namespace {
        return Err(StorageError::NamespaceMismatch);
    }
    let actual = admit(config)?;
    if actual != *expected {
        return Err(StorageError::ScopeChanged);
    }
    // Removing the registry would turn current rows into an unowned runtime.
    // SQLite revocation has no qualified current-format implementation yet.
    Err(StorageError::NamespaceUpgradePending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoreBackend;
    use crate::{Options, events::EventCache};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-admission-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache = root.join("index.json");
            let source = crate::storage::HostSource::new(
                LogicalSourceNamespace::new("source-A").unwrap(),
                "a".repeat(64),
            )
            .unwrap();
            drop(EventCache::open_scoped(&cache, &source).unwrap());
            let database = EventCache::path(&cache);
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
                namespace: LogicalSourceNamespace::new("source-A").unwrap(),
                expected_stream_epoch: Some(epoch),
            };
            // Source metadata is written through the current qualified writer.
            let db = super::super::semantic::Stage::open(
                super::super::semantic::SemanticStaging::admit(&config).unwrap(),
            )
            .unwrap()
            .into_test_connection();
            for (key, value) in [
                (
                    "catalog_version",
                    crate::model::summary::CATALOG_VERSION.to_string(),
                ),
                ("catalog_generation", "a".repeat(64)),
            ] {
                db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", (key, value)).unwrap();
            }
            Self { root, config }
        }
        fn db(&self) -> Connection {
            let database = self.config.backend.sqlite_database().unwrap();
            Connection::open(database).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn durable_admission_preserves_current_custody_and_rejects_second_namespace() {
        let f = Fixture::new();
        let before: i64 = f
            .db()
            .query_row("SELECT count(*) FROM meta", [], |r| r.get(0))
            .unwrap();
        let admission = f.config.admit_namespace().unwrap();
        assert_eq!(admission.generation, 1);
        assert_eq!(f.config.admit_namespace().unwrap(), admission);
        assert_eq!(
            f.db()
                .query_row("SELECT count(*) FROM meta", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        assert!(
            f.config
                .clone()
                .open()
                .unwrap()
                .read_snapshot(&f.config.namespace)
                .is_ok()
        );
        let mut other = f.config.clone();
        other.namespace = LogicalSourceNamespace::new("source-B").unwrap();
        assert_eq!(
            other.admit_namespace(),
            Err(StorageError::MultiSourceUnqualified)
        );
        assert!(matches!(
            other
                .open()
                .unwrap()
                .read_snapshot(&LogicalSourceNamespace::new("source-B").unwrap()),
            Err(StorageError::NamespaceMismatch)
        ));
        assert_eq!(
            f.db()
                .query_row("SELECT count(*) FROM storage_namespaces", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn current_admission_keeps_unowned_writers_fenced() {
        let f = Fixture::new();
        let warm = f.db();
        warm.prepare_cached(
            "UPDATE meta SET value='bad' WHERE namespace='source-A' AND key='catalog_generation'",
        )
        .unwrap();
        f.config.admit_namespace().unwrap();
        assert!(warm.execute("UPDATE meta SET value='bad' WHERE namespace='source-A' AND key='catalog_generation'", []).is_err());
        assert!(is_bound(&warm).unwrap());
        assert_eq!(
            warm.query_row(
                "SELECT value FROM meta WHERE namespace='source-A' AND key='catalog_generation'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "a".repeat(64)
        );
    }

    #[test]
    fn missing_or_stale_binding_refuses_without_mutation() {
        let f = Fixture::new();
        let before = std::fs::read(f.config.backend.sqlite_database().unwrap()).unwrap();
        let mut config = f.config.clone();
        config.expected_stream_epoch = None;
        assert_eq!(
            config.admit_namespace(),
            Err(StorageError::InvalidArguments)
        );
        config.expected_stream_epoch = Some("0".repeat(64));
        assert_eq!(config.admit_namespace(), Err(StorageError::ScopeChanged));
        assert_eq!(
            std::fs::read(f.config.backend.sqlite_database().unwrap()).unwrap(),
            before
        );
        assert!(is_bound(&f.db()).unwrap());
    }

    #[test]
    fn qualification_inventory_covers_current_tables_and_remains_closed() {
        let f = Fixture::new();
        f.config.admit_namespace().unwrap();
        let db = f.db();
        let missing: i64 = db.query_row("SELECT count(*) FROM sqlite_schema s WHERE s.type='table' AND s.name NOT LIKE 'sqlite_%' AND s.name NOT LIKE 'storage_%' AND NOT EXISTS(SELECT 1 FROM storage_writer_inventory i WHERE i.table_name=s.name AND i.qualified_version=0)", [], |r| r.get(0)).unwrap();
        assert_eq!(missing, 0);
        let inventory: serde_json::Value =
            serde_json::from_str(include_str!("writer-inventory.json")).unwrap();
        assert_eq!(
            inventory["writer_qualification"],
            WRITER_QUALIFICATION_VERSION
        );
        assert!(
            !inventory["unqualified_paths"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        db.execute("UPDATE storage_namespaces SET writer_qualification=999", [])
            .unwrap();
        let mut other = f.config.clone();
        other.namespace = LogicalSourceNamespace::new("source-B").unwrap();
        assert_eq!(
            other.admit_namespace(),
            Err(StorageError::MultiSourceUnqualified)
        );
    }

    #[test]
    fn configured_native_reads_refuse_before_payload_and_options_cannot_bypass_admission() {
        let unbound = Fixture::new();
        // Corrupt current custody is a refusal fixture, never a supported
        // unbound runtime. No reader may recreate the removed registry.
        unbound
            .db()
            .execute_batch("DROP TABLE storage_namespaces")
            .unwrap();
        let bound = Fixture::new();
        let options = Options {
            cache: bound.root.join("decoy-does-not-exist.json"),
            ..Options::default()
        };
        let mut mismatch = bound.config.clone();
        mismatch.namespace = LogicalSourceNamespace::new("foreign-source").unwrap();
        let mut stale = bound.config.clone();
        stale.expected_stream_epoch = Some("0".repeat(64));
        crate::storage::conformance::configured_native_read_refusals(
            &options,
            unbound.config.clone(),
            mismatch,
            stale,
        );
        assert!(!options.cache.with_extension("sqlite3").exists());
        assert_eq!(
            bound
                .config
                .catalog_page(&options, "limit=1")
                .unwrap()
                .status,
            200
        );
        assert!(!is_bound(&unbound.db()).unwrap());
    }

    #[test]
    fn unenroll_refuses_current_custody_without_releasing_writers_or_rows() {
        let f = Fixture::new();
        let admitted = f.config.admit_namespace().unwrap();
        let mut stale = f.config.clone();
        stale.expected_stream_epoch = Some("0".repeat(64));
        assert_eq!(
            stale.unenroll_namespace(&admitted),
            Err(StorageError::ScopeChanged)
        );
        assert_eq!(
            f.config.unenroll_namespace(&admitted),
            Err(StorageError::NamespaceUpgradePending)
        );
        assert!(is_bound(&f.db()).unwrap());
        assert_eq!(f.config.admit_namespace().unwrap(), admitted);
        assert!(f.db().execute("UPDATE meta SET value='bad' WHERE namespace='source-A' AND key='catalog_generation'", []).is_err());
        assert_eq!(f.db().query_row("SELECT value FROM meta WHERE namespace='source-A' AND key='catalog_generation'", [], |r| r.get::<_, String>(0)).unwrap(), "a".repeat(64));
    }
}
