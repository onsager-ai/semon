//! Durable first-source admission and a closed multi-source writer gate.
//! Enrollment is opt-in, content-free and atomic. Native rows are never reset.
use super::failed;
use crate::storage::{
    LogicalSourceNamespace, NamespaceAdmission, StorageError, StoreBackend, StoreConfig,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Zero deliberately means no native writer qualification. Never infer readiness
/// from reader ports or from a mutable database flag. Raising this version needs
/// the real same-store native writer conformance and complete table qualification.
pub(crate) const WRITER_QUALIFICATION_VERSION: u32 = 0;
const FORMAT_VERSION: u32 = 1;
const TABLE_LIMIT: usize = 128;

pub(crate) fn is_bound(db: &Connection) -> rusqlite::Result<bool> {
    db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='storage_namespaces')")?.query_row([], |row| row.get(0))
}

pub(super) fn require_admitted(
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
    let StoreBackend::Sqlite { database } = &config.backend;
    let mut db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(failed)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(failed)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let epoch: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if epoch != expected {
        return Err(StorageError::ScopeChanged);
    }
    if is_bound(&tx).map_err(failed)? {
        return match require_admitted(&tx, &config.namespace) {
            Err(StorageError::NamespaceMismatch) => Err(StorageError::MultiSourceUnqualified),
            other => other,
        };
    }
    // Check the supported current contract before any DDL, binding or data write.
    crate::events::store::require_current(&tx).map_err(|_| StorageError::Unavailable)?;
    let tables: Vec<String> = tx.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'storage_namespace_sequence' ORDER BY name LIMIT 129").map_err(failed)?
        .query_map([], |row| row.get(0)).map_err(failed)?.collect::<rusqlite::Result<_>>().map_err(failed)?;
    if tables.len() > TABLE_LIMIT
        || tables
            .iter()
            .any(|name| name.len() > 256 || name.starts_with("storage_"))
    {
        return Err(StorageError::NamespaceUpgradePending);
    }
    tx.execute_batch("CREATE TABLE storage_namespaces(namespace TEXT PRIMARY KEY,admission_generation INTEGER NOT NULL CHECK(admission_generation>0),admitted_at INTEGER NOT NULL,format_version INTEGER NOT NULL,writer_qualification INTEGER NOT NULL) STRICT; CREATE TABLE storage_writer_inventory(table_name TEXT PRIMARY KEY,qualified_version INTEGER NOT NULL) STRICT;").map_err(failed)?;
    let sequence_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='storage_namespace_sequence')", [], |r| r.get(0)).map_err(failed)?;
    let generation = if sequence_exists {
        let prior: i64 = tx
            .query_row(
                "SELECT last_generation FROM storage_namespace_sequence WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if prior <= 0 {
            return Err(StorageError::NamespaceUpgradePending);
        }
        prior
            .checked_add(1)
            .ok_or(StorageError::NamespaceUpgradePending)?
    } else {
        1
    };
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StorageError::Unavailable)?
            .as_secs(),
    )
    .map_err(|_| StorageError::Unavailable)?;
    tx.execute(
        "INSERT INTO storage_namespaces VALUES(?1,?2,?3,?4,?5)",
        (
            config.namespace.as_str(),
            generation,
            now,
            FORMAT_VERSION,
            WRITER_QUALIFICATION_VERSION,
        ),
    )
    .map_err(failed)?;
    for (index, table) in tables.iter().enumerate() {
        tx.execute("INSERT INTO storage_writer_inventory VALUES(?1,0)", [table])
            .map_err(failed)?;
        let quoted = table.replace('"', "\"\"");
        // No native row/key rewrite is qualified yet. Durable fences also stop
        // already-open or older writers that do not know the admission registry.
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            tx.execute_batch(&format!("CREATE TRIGGER storage_namespace_fence_{index}_{operation} BEFORE {operation} ON \"{quoted}\" BEGIN SELECT RAISE(ABORT,'native namespace writer upgrade pending'); END;")).map_err(failed)?;
        }
    }
    let admission = require_admitted(&tx, &config.namespace)?;
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    Ok(admission)
}

/// Revert only the still-read-only first-source preparation stage. All checks
/// precede trigger removal under one writer transaction. No native table/row is
/// dropped, copied or reset; qualified/migrated state must use future rollback.
pub(crate) fn unenroll(
    config: &StoreConfig,
    expected: &NamespaceAdmission,
) -> Result<(), StorageError> {
    if expected.namespace != config.namespace {
        return Err(StorageError::NamespaceMismatch);
    }
    let epoch = config
        .expected_stream_epoch
        .as_deref()
        .ok_or(StorageError::InvalidArguments)?;
    let StoreBackend::Sqlite { database } = &config.backend;
    let mut db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(failed)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(failed)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let actual = require_admitted(&tx, &config.namespace)?;
    if super::catalog_namespace::started(&tx).map_err(failed)? {
        return Err(StorageError::NamespaceUpgradePending);
    }
    if actual != *expected {
        return Err(StorageError::ScopeChanged);
    }
    let actual_epoch: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if epoch != actual_epoch {
        return Err(StorageError::ScopeChanged);
    }
    let qualification: i64 = tx
        .query_row(
            "SELECT writer_qualification FROM storage_namespaces WHERE namespace=?1",
            [config.namespace.as_str()],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if qualification != 0 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let tables: Vec<(String, i64)> = tx.prepare("SELECT table_name,qualified_version FROM storage_writer_inventory ORDER BY table_name LIMIT 129").map_err(failed)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(failed)?.collect::<rusqlite::Result<_>>().map_err(failed)?;
    if tables.is_empty()
        || tables.len() > TABLE_LIMIT
        || tables
            .iter()
            .any(|(name, version)| name.len() > 256 || *version != 0)
    {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let native_tables: i64 = tx.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('storage_namespaces','storage_writer_inventory','storage_namespace_sequence')", [], |r| r.get(0)).map_err(failed)?;
    let fence_count: i64 = tx.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND name GLOB 'storage_namespace_fence_*'", [], |r| r.get(0)).map_err(failed)?;
    if native_tables != tables.len() as i64 || fence_count != (tables.len() * 3) as i64 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let mut triggers = Vec::new();
    for (index, (table, _)) in tables.iter().enumerate() {
        let quoted = table.replace('"', "\"\"");
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let name = format!("storage_namespace_fence_{index}_{operation}");
            let expected_sql = format!(
                "CREATE TRIGGER {name} BEFORE {operation} ON \"{quoted}\" BEGIN SELECT RAISE(ABORT,'native namespace writer upgrade pending'); END"
            );
            let stored: Option<(String, String)> = tx
                .query_row(
                    "SELECT tbl_name,sql FROM sqlite_schema WHERE type='trigger' AND name=?1",
                    [&name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            if stored.as_ref().is_none_or(|(owner, sql)| {
                owner != table || sql.trim_end_matches(';') != expected_sql
            }) {
                return Err(StorageError::NamespaceUpgradePending);
            }
            triggers.push(name);
        }
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS storage_namespace_sequence(singleton INTEGER PRIMARY KEY CHECK(singleton=1),last_generation INTEGER NOT NULL CHECK(last_generation>0)) STRICT;").map_err(failed)?;
    let generation = i64::try_from(actual.generation).map_err(|_| StorageError::ScopeChanged)?;
    tx.execute("INSERT INTO storage_namespace_sequence VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET last_generation=max(last_generation,excluded.last_generation)", [generation]).map_err(failed)?;
    for trigger in triggers {
        tx.execute_batch(&format!("DROP TRIGGER \"{trigger}\";"))
            .map_err(failed)?;
    }
    tx.execute_batch("DROP TABLE storage_writer_inventory; DROP TABLE storage_namespaces;")
        .map_err(failed)?;
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            drop(EventCache::open_scoped(&cache).unwrap());
            let database = EventCache::path(&cache);
            let db = Connection::open(&database).unwrap();
            db.execute("INSERT INTO meta(key,value) VALUES('catalog_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [crate::model::summary::CATALOG_VERSION.to_string()]).unwrap();
            db.execute("INSERT INTO meta(key,value) VALUES('catalog_generation',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", ["a".repeat(64)]).unwrap();
            let epoch = db
                .query_row(
                    "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let config = StoreConfig {
                backend: StoreBackend::Sqlite { database },
                namespace: LogicalSourceNamespace::new("source-A").unwrap(),
                expected_stream_epoch: Some(epoch),
            };
            Self { root, config }
        }
        fn db(&self) -> Connection {
            let StoreBackend::Sqlite { database } = &self.config.backend;
            Connection::open(database).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn durable_admission_refuses_unbound_reads_mismatches_and_second_namespace() {
        let f = Fixture::new();
        assert!(matches!(
            f.config
                .clone()
                .open()
                .unwrap()
                .read_snapshot(&f.config.namespace),
            Err(StorageError::NamespaceUnadmitted)
        ));
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
    fn enrollment_fences_warm_native_writers_and_cannot_be_reset_by_producer() {
        let f = Fixture::new();
        let warm = f.db();
        warm.prepare_cached("UPDATE meta SET value='bad' WHERE key='catalog_generation'")
            .unwrap();
        f.config.admit_namespace().unwrap();
        assert!(
            warm.execute(
                "UPDATE meta SET value='bad' WHERE key='catalog_generation'",
                []
            )
            .is_err()
        );
        let options = Options {
            cache: f.root.join("index.json"),
            ..Options::default()
        };
        assert!(EventCache::open_scoped(&options.cache).is_err());
        assert!(is_bound(&f.db()).unwrap());
        assert_eq!(
            f.db()
                .query_row(
                    "SELECT value FROM meta WHERE key='catalog_generation'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "a".repeat(64)
        );
        assert_eq!(
            f.config.clone().open().unwrap().compact_catalog_changes(
                &f.config.namespace,
                crate::CatalogReadScope::Current,
                f.config.expected_stream_epoch.as_ref().unwrap(),
                0,
                1
            ),
            Err(StorageError::NamespaceUpgradePending)
        );
    }

    #[test]
    fn missing_or_stale_binding_refuses_before_schema_mutation() {
        let f = Fixture::new();
        let mut config = f.config.clone();
        config.expected_stream_epoch = None;
        assert_eq!(
            config.admit_namespace(),
            Err(StorageError::InvalidArguments)
        );
        config.expected_stream_epoch = Some("0".repeat(64));
        assert_eq!(config.admit_namespace(), Err(StorageError::ScopeChanged));
        assert!(!is_bound(&f.db()).unwrap());
    }

    #[test]
    fn qualification_inventory_covers_every_table_and_remains_closed() {
        let f = Fixture::new();
        f.config.admit_namespace().unwrap();
        let db = f.db();
        let missing: i64 = db.query_row("SELECT count(*) FROM sqlite_schema s WHERE s.type='table' AND s.name NOT LIKE 'sqlite_%' AND s.name NOT IN ('storage_namespaces','storage_writer_inventory') AND NOT EXISTS(SELECT 1 FROM storage_writer_inventory i WHERE i.table_name=s.name AND i.qualified_version=0)", [], |r| r.get(0)).unwrap();
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
        // Mutable qualification metadata can never enable multi-source mode.
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
        let bound = Fixture::new();
        bound.config.admit_namespace().unwrap();
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
        let local_options = Options {
            cache: bound.root.join("index.json"),
            ..Options::default()
        };
        let request = crate::catalog::Request::parse("limit=1").unwrap();
        let refused = crate::storage::read_local_catalog_page(&local_options, "source-A", &request)
            .err()
            .unwrap();
        assert_eq!(
            *refused.downcast::<StorageError>().unwrap(),
            StorageError::NamespaceUpgradePending
        );
        let local =
            crate::storage::local_query_catalog(&local_options, bound.config.namespace.clone());
        assert_eq!(
            local
                .read_cohort(&bound.config.namespace, 1, &mut 1024)
                .err()
                .unwrap()
                .code,
            "namespace_upgrade_pending"
        );
        // The configured reader resolves the real database even though Options
        // points at an absent decoy, and returns the namespace-labelled page.
        let page = bound.config.catalog_page(&options, "limit=1").unwrap();
        assert_eq!(page.status, 200);
    }

    #[test]
    fn unenroll_round_trip_preserves_native_rows_restores_warm_writer_and_fences_old_token() {
        let f = Fixture::new();
        let warm = f.db();
        warm.execute(
            "INSERT INTO meta VALUES('reported_fact_test','preserved')",
            [],
        )
        .unwrap();
        let before: i64 = warm
            .query_row("SELECT count(*) FROM meta", [], |r| r.get(0))
            .unwrap();
        let first = f.config.admit_namespace().unwrap();
        assert!(
            warm.execute(
                "UPDATE meta SET value='changed' WHERE key='reported_fact_test'",
                []
            )
            .is_err()
        );
        let mut wrong = f.config.clone();
        wrong.expected_stream_epoch = Some("0".repeat(64));
        assert_eq!(
            wrong.unenroll_namespace(&first),
            Err(StorageError::ScopeChanged)
        );
        assert!(is_bound(&warm).unwrap());
        f.config.unenroll_namespace(&first).unwrap();
        assert!(!is_bound(&warm).unwrap());
        assert_eq!(
            warm.query_row("SELECT count(*) FROM meta", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        assert_eq!(
            warm.query_row(
                "SELECT value FROM meta WHERE key='reported_fact_test'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "preserved"
        );
        warm.execute(
            "UPDATE meta SET value='changed' WHERE key='reported_fact_test'",
            [],
        )
        .unwrap();
        assert!(EventCache::open_scoped(&f.root.join("index.json")).is_ok());
        let second = f.config.admit_namespace().unwrap();
        assert!(second.generation > first.generation);
        assert_eq!(
            f.config.unenroll_namespace(&first),
            Err(StorageError::ScopeChanged)
        );
        f.config.unenroll_namespace(&second).unwrap();
        assert_eq!(
            warm.query_row(
                "SELECT value FROM meta WHERE key='reported_fact_test'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "changed"
        );
    }

    #[test]
    fn unenroll_refuses_tampered_fences_without_partial_release() {
        let f = Fixture::new();
        let admitted = f.config.admit_namespace().unwrap();
        let db = f.db();
        db.execute_batch("DROP TRIGGER storage_namespace_fence_0_DELETE")
            .unwrap();
        assert_eq!(
            f.config.unenroll_namespace(&admitted),
            Err(StorageError::NamespaceUpgradePending)
        );
        assert!(is_bound(&db).unwrap());
        assert!(
            db.execute(
                "UPDATE meta SET value='bad' WHERE key='catalog_generation'",
                []
            )
            .is_err()
        );
    }
}
