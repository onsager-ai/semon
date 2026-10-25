//! Atomic discovery/invalidation adapter. The configured family is functional;
//! Event/ledger/proof and Catalog publication remain independently fenced.
use super::{catalog_namespace, failed};
use crate::storage::{NativeReaderBinding, StorageError, StoreBackend, StoreConfig};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::time::Duration;

pub(crate) const SOURCE_JOURNAL_QUALIFICATION_VERSION: u32 = 1;
const MAX_PATHS: usize = 256;
const PENDING_SQL: &str = "SELECT source_path FROM main.session_catalog_invalidations INDEXED BY storage_ns_source_pending WHERE namespace=?1 ORDER BY revision,source_path LIMIT ?2";

// Shared by the existing native transaction and the configured transaction.
// The caller owns the transaction: invalidation never commits independently.
pub(crate) fn invalidate(
    db: &Connection,
    namespace: Option<&str>,
    path: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    crate::native_timing::discovery_count(|counts|counts.statements+=3);
    if namespace.is_none() && super::namespace::is_bound(db)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if let Some(namespace) = namespace {
        if db.execute("UPDATE storage_source_journal SET revision=revision+1 WHERE namespace=?1 AND revision<9223372036854775807", [namespace])? != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        db.execute("INSERT INTO main.session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(?1,?2,(SELECT revision FROM storage_source_journal WHERE namespace=?1),?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET revision=excluded.revision,reason=excluded.reason,observed_at=excluded.observed_at", params![namespace,path,reason,crate::model::now_ms()])?;
    } else {
        db.execute("INSERT INTO meta(key,value) VALUES ('catalog_source_revision','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(meta.value AS INTEGER)+1 AS TEXT)", [])?;
        db.execute("INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at) VALUES (?1,CAST((SELECT value FROM meta WHERE key='catalog_source_revision') AS INTEGER),?2,?3) ON CONFLICT(source_path) DO UPDATE SET revision=excluded.revision,reason=excluded.reason,observed_at=excluded.observed_at", params![path,reason,crate::model::now_ms()])?;
    }
    Ok(())
}

pub(crate) fn discover(
    db: &Connection,
    namespace: Option<&str>,
    paths: &[String],
    complete: bool,
) -> rusqlite::Result<()> {
    crate::native_timing::discovery_count(|counts| { counts.statements+=1; counts.rows+=paths.len(); });
    if namespace.is_none() && super::namespace::is_bound(db)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    for path in paths {
        crate::native_timing::discovery_count(|counts|counts.statements+=1);
        let inserted = if let Some(namespace) = namespace {
            db.execute("INSERT OR IGNORE INTO main.catalog_backfill_sources(namespace,source_path) VALUES(?1,?2)", params![namespace,path])?
        } else {
            db.execute(
                "INSERT OR IGNORE INTO catalog_backfill_sources VALUES(?1)",
                [path],
            )?
        };
        if inserted > 0 {
            invalidate(db, namespace, path, "changed")?;
        }
    }
    let state = if complete { "draining" } else { "discovering" };
    crate::native_timing::discovery_count(|counts|counts.statements+=1);
    if let Some(namespace) = namespace {
        db.execute(
            "UPDATE storage_source_journal SET backfill_state=?2 WHERE namespace=?1",
            params![namespace, state],
        )?;
    } else {
        db.execute(
            "INSERT INTO meta(key,value) VALUES('catalog_backfill_state',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE meta.value<>excluded.value",
            [state],
        )?;
    }
    Ok(())
}

pub(crate) fn claims(
    db: &Connection,
    namespace: Option<&str>,
    paths: &[String],
) -> rusqlite::Result<crate::storage::source_observations::SourceClaims> {
    if namespace.is_none() && super::namespace::is_bound(db)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let sql = if namespace.is_some() {
        "SELECT revision FROM main.session_catalog_invalidations WHERE namespace=?2 AND source_path=?1"
    } else {
        "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1"
    };
    let mut statement = db.prepare(sql)?;
    paths
        .iter()
        .map(|path| {
            let revision = if let Some(namespace) = namespace {
                statement
                    .query_row(params![path, namespace], |r| r.get(0))
                    .optional()?
            } else {
                statement.query_row([path], |r| r.get(0)).optional()?
            };
            Ok((path.clone(), revision))
        })
        .collect()
}

pub(crate) struct SourceJournal {
    config: StoreConfig,
    owner: Option<crate::events::store::configured::NativeOwnership>,
}
#[allow(non_snake_case)]
pub(crate) fn SourceJournal(config: StoreConfig) -> SourceJournal {
    SourceJournal {
        config,
        owner: None,
    }
}

impl SourceJournal {
    pub(crate) fn owned(owner: crate::events::store::configured::NativeOwnership) -> Self {
        Self {
            config: owner.config().clone(),
            owner: Some(owner),
        }
    }
    pub(crate) fn connection(&self) -> Result<Connection, StorageError> {
        if self
            .config
            .expected_stream_epoch
            .as_ref()
            .is_none_or(|epoch| epoch.len() != 64 || !epoch.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(StorageError::InvalidArguments);
        }
        let StoreBackend::Sqlite { database } = &self.config.backend;
        if database.as_os_str().is_empty() || database == std::path::Path::new(":memory:") {
            return Err(StorageError::InvalidArguments);
        }
        let db = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(failed)?;
        db.busy_timeout(Duration::from_millis(100))
            .map_err(failed)?;
        catalog_namespace::register(&db, &self.config.namespace)?;
        if self.owner.is_some() {
            db.create_scalar_function(
                "semon_event_writer",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                |_| Ok(1_i64),
            )
            .map_err(failed)?;
        }
        Ok(db)
    }

    pub(crate) fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        if let Some(owner) = &self.owner {
            return owner.validate(db);
        }
        if !NativeReaderBinding::Admitted(self.config.clone())
            .validate(db, &self.config.namespace)?
        {
            return Err(StorageError::NamespaceUpgradePending);
        }
        Ok(())
    }

    /// Explicit bounded preparation after row assignment. Counter seeding keeps
    /// the old high watermark even when an earlier claim was acknowledged.
    pub(crate) fn prepare(&self) -> Result<(), StorageError> {
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        let old: Option<String> = tx
            .query_row(
                crate::events::store::configured::metadata_sql(&tx,
                    "SELECT value FROM meta WHERE key='catalog_source_revision'",
                    "SELECT value FROM meta WHERE namespace=semon_namespace() AND key='catalog_source_revision'").map_err(failed)?,
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;
        let old = old
            .map(|s| s.parse::<i64>())
            .transpose()
            .map_err(|_| StorageError::Unavailable)?
            .unwrap_or(0);
        if old < 0 {
            return Err(StorageError::Unavailable);
        }
        let head: i64 = tx.query_row("SELECT coalesce(max(revision),0) FROM main.session_catalog_invalidations WHERE namespace=?1", [self.config.namespace.as_str()], |r| r.get(0)).map_err(failed)?;
        if head < 0 {
            return Err(StorageError::Unavailable);
        }
        let state: Option<String> = tx
            .query_row(
                crate::events::store::configured::metadata_sql(&tx,
                    "SELECT value FROM meta WHERE key='catalog_backfill_state'",
                    "SELECT value FROM meta WHERE namespace=semon_namespace() AND key='catalog_backfill_state'").map_err(failed)?,
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;
        if state
            .as_deref()
            .is_some_and(|s| !matches!(s, "discovering" | "draining" | "complete"))
        {
            return Err(StorageError::Unavailable);
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS storage_source_journal(namespace TEXT PRIMARY KEY,revision INTEGER NOT NULL CHECK(revision>=0),backfill_state TEXT CHECK(backfill_state IN ('discovering','draining','complete')),qualification INTEGER NOT NULL CHECK(qualification=1)) STRICT;").map_err(failed)?;
        tx.execute(
            "INSERT OR IGNORE INTO storage_source_journal VALUES(?1,?2,?3,?4)",
            params![
                self.config.namespace.as_str(),
                old.max(head),
                state,
                SOURCE_JOURNAL_QUALIFICATION_VERSION
            ],
        )
        .map_err(failed)?;
        catalog_namespace::guard(&tx, "storage_source_journal")?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }

    fn snapshot(&self) -> Result<Connection, StorageError> {
        let db = self.connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db)?;
        Ok(db)
    }
}

impl crate::storage::source_observations::SourceObservationStore for SourceJournal {
    fn invalidate(&mut self, expected: &[(String, Option<i64>)]) -> Result<bool, StorageError> {
        let paths = expected
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        validate_paths(&paths)?;
        if paths
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != paths.len()
        {
            return Err(StorageError::InvalidArguments);
        }
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if claims(&tx, Some(self.config.namespace.as_str()), &paths).map_err(failed)? != expected {
            return Ok(false);
        }
        for path in paths {
            invalidate(&tx, Some(self.config.namespace.as_str()), &path, "changed")
                .map_err(failed)?;
        }
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(true)
    }
    fn discover(&mut self, paths: &[String], complete: bool) -> Result<(), StorageError> {
        validate_paths(paths)?;
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        discover(&tx, Some(self.config.namespace.as_str()), paths, complete).map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }
    fn claims(
        &self,
        paths: &[String],
    ) -> Result<crate::storage::source_observations::SourceClaims, StorageError> {
        validate_paths(paths)?;
        let db = self.snapshot()?;
        claims(&db, Some(self.config.namespace.as_str()), paths).map_err(failed)
    }
    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError> {
        if limit == 0 || limit > MAX_PATHS {
            return Err(StorageError::InvalidArguments);
        }
        let db = self.snapshot()?;
        let mut statement = db.prepare(PENDING_SQL).map_err(failed)?;
        let paths = statement
            .query_map(params![self.config.namespace.as_str(), limit as i64], |r| {
                r.get(0)
            })
            .map_err(failed)?
            .collect::<rusqlite::Result<_>>()
            .map_err(failed)?;
        Ok(paths)
    }
    fn backfill_state(&self) -> Result<Option<String>, StorageError> {
        let db = self.snapshot()?;
        db.query_row(
            "SELECT backfill_state FROM storage_source_journal WHERE namespace=?1",
            [self.config.namespace.as_str()],
            |r| r.get(0),
        )
        .map_err(failed)
    }
}

fn validate_paths(paths: &[String]) -> Result<(), StorageError> {
    if paths.len() > MAX_PATHS || paths.iter().any(|p| p.is_empty() || p.len() > 4096) {
        return Err(StorageError::InvalidArguments);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EventCache, FileIndex, Ledger, Stat, StoreError};
    use crate::storage::{LogicalSourceNamespace, source_observations::SourceObservationStore};
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
                "semon-source-journal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache_path = root.join("index.json");
            let mut native = EventCache::open_scoped(&cache_path).unwrap();
            // Exercise the actual unbound producer before admission. Its old
            // counter must survive even when a newer observation was acked.
            native
                .discover(&["seed".into(), "acknowledged".into()], true)
                .unwrap();
            drop(native);
            let database = EventCache::path(&cache_path);
            let db = Connection::open(&database).unwrap();
            db.execute(
                "DELETE FROM session_catalog_invalidations WHERE source_path='acknowledged'",
                [],
            )
            .unwrap();
            let epoch = db
                .query_row(
                    "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let config = StoreConfig {
                backend: StoreBackend::Sqlite { database },
                namespace: LogicalSourceNamespace::new("A").unwrap(),
                expected_stream_epoch: Some(epoch),
            };
            config.admit_namespace().unwrap();
            Self { root, config }
        }
        fn db(&self) -> Connection {
            let StoreBackend::Sqlite { database } = &self.config.backend;
            Connection::open(database).unwrap()
        }
        fn assign(&self) {
            for _ in 0..128 {
                if self.config.assign_catalog_namespace(64).unwrap().complete {
                    return;
                }
            }
            panic!("source journal assignment did not finish");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn configured_discovery_claim_cas_restart_and_originals() {
        let f = Fixture::new();
        assert!(matches!(
            f.config.source_observations(),
            Err(StorageError::NamespaceUpgradePending)
        ));
        // First one-row migration call and reopening retain bounded progress.
        f.config.assign_catalog_namespace(1).unwrap();
        f.assign();
        let mut store = f.config.source_observations().unwrap();
        let paths = vec!["seed".into(), "new".into()];
        assert_eq!(
            store.claims(&paths).unwrap(),
            vec![("seed".into(), Some(1)), ("new".into(), None)]
        );
        store.discover(&paths, true).unwrap();
        let captured = store.claims(&paths).unwrap();
        assert_eq!(
            captured,
            vec![("seed".into(), Some(1)), ("new".into(), Some(3))]
        );
        assert_eq!(store.backfill_state().unwrap().as_deref(), Some("draining"));
        let plan_db = f.db();
        let plan: Vec<String> = plan_db
            .prepare(&format!("EXPLAIN QUERY PLAN {PENDING_SQL}"))
            .unwrap()
            .query_map(params!["A", 1], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("storage_ns_source_pending"))
        );
        assert!(plan.iter().all(|line| !line.contains("TEMP B-TREE")));
        // Repeated discovery is idempotent, including its source claims.
        store.discover(&paths, true).unwrap();
        assert_eq!(store.claims(&paths).unwrap(), captured);
        assert_eq!(store.pending(1).unwrap(), vec!["seed".to_owned()]);
        let mut concurrent = f.config.source_observations().unwrap();
        assert!(concurrent.invalidate(&captured[1..]).unwrap());
        let before_conflict = store.claims(&paths).unwrap();
        assert!(!store.invalidate(&captured).unwrap());
        assert_eq!(store.claims(&paths).unwrap(), before_conflict);
        drop(store);
        let restarted = f.config.source_observations().unwrap();
        assert_eq!(restarted.claims(&paths).unwrap(), before_conflict);
        let db = f.db();
        let originals:Vec<(String,i64)> = db.prepare("SELECT source_path,revision FROM storage_original_session_catalog_invalidations ORDER BY source_path").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(originals, vec![("seed".into(), 1)]);
        assert!(
            db.execute(
                "DELETE FROM storage_original_session_catalog_invalidations",
                []
            )
            .is_err()
        );
        assert_eq!(super::super::namespace::WRITER_QUALIFICATION_VERSION, 0);
    }

    #[test]
    fn configured_discovery_rolls_back_mid_batch_and_keeps_foreign_rows_invisible() {
        let f = Fixture::new();
        f.assign();
        let mut store = f.config.source_observations().unwrap();
        let db = f.db();
        // Isolated adapter fixture only: the registry still admits exactly A.
        catalog_namespace::register(&db, &LogicalSourceNamespace::new("B").unwrap()).unwrap();
        db.execute(
            "INSERT INTO session_catalog_invalidations VALUES('B','foreign',100,'changed',0)",
            [],
        )
        .unwrap();
        db.execute_batch("CREATE TRIGGER journal_test_fault BEFORE INSERT ON catalog_backfill_sources WHEN NEW.source_path='fault' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let paths = vec!["first".into(), "fault".into()];
        assert_eq!(
            store.discover(&paths, false),
            Err(StorageError::Unavailable)
        );
        assert_eq!(
            store.claims(&paths).unwrap(),
            vec![("first".into(), None), ("fault".into(), None)]
        );
        assert_eq!(store.pending(256).unwrap(), vec!["seed".to_owned()]);
        let revision: i64 = db
            .query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace='A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revision, 2);
        let first:i64=db.query_row("SELECT count(*) FROM catalog_backfill_sources WHERE namespace='A' AND source_path='first'",[],|r|r.get(0)).unwrap();
        assert_eq!(first, 0);
        assert_eq!(store.backfill_state().unwrap().as_deref(), Some("draining"));
    }

    #[test]
    fn configured_source_writer_epoch_bounds_and_unported_families_refuse() {
        let f = Fixture::new();
        f.assign();
        let mut wrong = f.config.clone();
        wrong.expected_stream_epoch = None;
        assert!(matches!(
            wrong.source_observations(),
            Err(StorageError::InvalidArguments)
        ));
        wrong.expected_stream_epoch = Some("0".repeat(64));
        assert!(matches!(
            wrong.source_observations(),
            Err(StorageError::ScopeChanged)
        ));
        wrong = f.config.clone();
        wrong.namespace = LogicalSourceNamespace::new("B").unwrap();
        assert!(matches!(
            wrong.source_observations(),
            Err(StorageError::NamespaceMismatch)
        ));
        wrong = f.config.clone();
        let missing = f.root.join("absent.sqlite3");
        wrong.backend = StoreBackend::Sqlite {
            database: missing.clone(),
        };
        assert!(matches!(
            wrong.source_observations(),
            Err(StorageError::Unavailable)
        ));
        assert!(!missing.exists());
        let unbound_cache = f.root.join("unbound.json");
        drop(EventCache::open_scoped(&unbound_cache).unwrap());
        let unbound_database = EventCache::path(&unbound_cache);
        let unbound_db = Connection::open(&unbound_database).unwrap();
        let unbound_epoch = unbound_db
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        wrong.backend = StoreBackend::Sqlite {
            database: unbound_database,
        };
        wrong.expected_stream_epoch = Some(unbound_epoch);
        assert!(matches!(
            wrong.source_observations(),
            Err(StorageError::NamespaceUnadmitted)
        ));
        assert_eq!(
            unbound_db
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name='storage_source_journal'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let mut store = f.config.source_observations().unwrap();
        assert_eq!(
            store.discover(&["x".repeat(4097)], true),
            Err(StorageError::InvalidArguments)
        );
        assert_eq!(
            store.claims(&vec!["x".into(); 257]),
            Err(StorageError::InvalidArguments)
        );
        assert_eq!(store.pending(0), Err(StorageError::InvalidArguments));
        assert_eq!(store.pending(257), Err(StorageError::InvalidArguments));
        assert_eq!(
            store.invalidate(&[("seed".into(), Some(1)), ("seed".into(), Some(1))]),
            Err(StorageError::InvalidArguments)
        );
        let mut index = crate::storage::sqlite::open_configured_index(&f.config).unwrap();
        let raw = f.root.join("native.jsonl");
        fs::write(&raw, b"{}").unwrap();
        let ledger = Ledger {
            stat: Stat::of(&raw).unwrap(),
            offset: 2,
            prefix: [0; 32],
            tail: [0; 32],
        };
        assert!(matches!(
            index.commit_file(
                raw.to_str().unwrap(),
                None,
                None,
                &ledger,
                &FileIndex::default()
            ),
            Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending
            ))
        ));
        assert!(matches!(
            index.commit_initial_files(&[]),
            Err(StoreError::Configured(StorageError::InvalidArguments))
        ));
        // A real initial batch still requires explicit Event-family preparation.
        assert!(matches!(
            index.commit_initial_files(&[(
                raw.to_string_lossy().into_owned(),
                ledger.clone(),
                std::sync::Arc::new(FileIndex::default()),
            )]),
            Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending
            ))
        ));
        let publication = crate::slot_projection::Publication {
            rows: &[],
            transcripts: None,
        };
        assert!(matches!(
            index.publish_session_catalog_fenced(&publication, None),
            Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending
            ))
        ));
        assert!(matches!(
            index.begin_semantic_stage(&[], "", false),
            Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending
            ))
        ));
        // A refused family neither detaches nor substitutes this store.
        index
            .discover_catalog_sources(&["after-refusal".into()], true)
            .unwrap();
        assert_eq!(
            f.config
                .source_observations()
                .unwrap()
                .claims(&["after-refusal".into()])
                .unwrap(),
            vec![("after-refusal".into(), Some(3))]
        );
        assert!(EventCache::open_scoped(&f.root.join("index.json")).is_err());
        assert_eq!(
            store.claims(&["seed".into()]).unwrap(),
            vec![("seed".into(), Some(1))]
        );
    }

    #[test]
    fn configured_source_writer_revalidates_live_epoch_and_reports_busy() {
        let f = Fixture::new();
        f.assign();
        let mut store = f.config.source_observations().unwrap();
        let db = f.db();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            store.discover(&["locked".into()], true),
            Err(StorageError::Busy)
        );
        db.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            store.claims(&["locked".into()]).unwrap(),
            vec![("locked".into(), None)]
        );
        // Test-owned coordinate replacement: bypass only this fixture's epoch
        // fence. The already-open semantic writer must validate again.
        let names:Vec<String>=db.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND tbl_name='meta' AND sql LIKE '%BEFORE UPDATE%'").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(names.len(), 1);
        db.execute_batch(&format!("DROP TRIGGER \"{}\"", names[0]))
            .unwrap();
        db.execute(
            "UPDATE meta SET value=?1 WHERE key='catalog_changes_epoch'",
            ["f".repeat(64)],
        )
        .unwrap();
        assert_eq!(
            store.discover(&["new-epoch".into()], true),
            Err(StorageError::ScopeChanged)
        );
        assert_eq!(
            store.claims(&["seed".into()]),
            Err(StorageError::ScopeChanged)
        );
        let count:i64=db.query_row("SELECT count(*) FROM catalog_backfill_sources WHERE namespace='A' AND source_path='new-epoch'",[],|r|r.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}
