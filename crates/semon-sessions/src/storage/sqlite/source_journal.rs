//! Atomic discovery/invalidation adapter. The configured family is functional;
//! Event/ledger/proof and Catalog publication remain independently fenced.
use super::{catalog_namespace, failed};
use crate::storage::{
    NativeReaderBinding, StorageError, StoreBackend, StoreConfig,
    source_observations::{SourceMutationReceipt, SourceObservationStore, SourceOperation},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const SOURCE_JOURNAL_QUALIFICATION_VERSION: u32 = 1;
const MAX_PATHS: usize = 256;
const PENDING_SQL: &str = "SELECT source_path FROM main.session_catalog_invalidations INDEXED BY storage_ns_source_pending WHERE namespace=?1 ORDER BY revision,source_path LIMIT ?2";

// Shared by the existing native transaction and the configured transaction.
// The caller owns the transaction: invalidation never commits independently.
#[derive(Debug, Default, PartialEq, Eq)]
struct JournalWork {
    prepares: usize,
    prepare_us: u128,
    executions: usize,
    namespace_probes: usize,
    membership_rows: usize,
    revision_rows: usize,
    claim_rows: usize,
    state_rows: usize,
}

// This context cannot outlive the caller's connection/transaction. It selects
// authored SQL once per call; no shape or namespace result survives the call.
struct InvalidationStatements<'a> {
    revision: rusqlite::Statement<'a>,
    claim: rusqlite::Statement<'a>,
}

fn refuse_bound(
    db: &Connection,
    namespace: Option<&str>,
    work: &mut JournalWork,
) -> rusqlite::Result<()> {
    if namespace.is_none() {
        work.namespace_probes += 1;
        if super::namespace::is_bound(db)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn invalidate_prepared<'a>(
    db: &'a Connection,
    namespace: Option<&str>,
    path: &str,
    reason: &str,
    now: &mut impl FnMut() -> i64,
    statements: &mut Option<InvalidationStatements<'a>>,
    work: &mut JournalWork,
) -> rusqlite::Result<()> {
    // Preserve the per-invalidation guard, including after a membership write.
    refuse_bound(db, namespace, work)?;
    if statements.is_none() {
        let (revision, claim) = if namespace.is_some() {
            (
                "UPDATE storage_source_journal SET revision=revision+1 WHERE namespace=?1 AND revision<9223372036854775807",
                "INSERT INTO main.session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(?1,?2,(SELECT revision FROM storage_source_journal WHERE namespace=?1),?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET revision=excluded.revision,reason=excluded.reason,observed_at=excluded.observed_at",
            )
        } else {
            (
                "INSERT INTO meta(key,value) VALUES ('catalog_source_revision','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(meta.value AS INTEGER)+1 AS TEXT)",
                "INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at) VALUES (?1,CAST((SELECT value FROM meta WHERE key='catalog_source_revision') AS INTEGER),?2,?3) ON CONFLICT(source_path) DO UPDATE SET revision=excluded.revision,reason=excluded.reason,observed_at=excluded.observed_at",
            )
        };
        let started = crate::native_timing::enabled().then(std::time::Instant::now);
        work.prepares += 1;
        let revision = db.prepare(revision)?;
        work.prepares += 1;
        let claim = db.prepare(claim)?;
        work.prepare_us += started.map_or(0, |started| started.elapsed().as_micros());
        *statements = Some(InvalidationStatements { revision, claim });
    }
    let statements = statements.as_mut().ok_or(rusqlite::Error::InvalidQuery)?;
    work.executions += 1;
    if let Some(namespace) = namespace {
        let rows = statements.revision.execute([namespace])?;
        work.revision_rows += rows;
        if rows != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        work.executions += 1;
        work.claim_rows += statements
            .claim
            .execute(params![namespace, path, reason, now()])?;
    } else {
        work.revision_rows += statements.revision.execute([])?;
        work.executions += 1;
        work.claim_rows += statements.claim.execute(params![path, reason, now()])?;
    }
    Ok(())
}

pub(crate) fn invalidate(
    db: &Connection,
    namespace: Option<&str>,
    path: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    crate::native_timing::discovery_count(|counts| counts.statements += 3);
    invalidate_prepared(
        db,
        namespace,
        path,
        reason,
        &mut crate::model::now_ms,
        &mut None,
        &mut JournalWork::default(),
    )
}

pub(crate) fn discover(
    db: &Connection,
    namespace: Option<&str>,
    paths: &[String],
    complete: bool,
) -> rusqlite::Result<()> {
    let started = crate::native_timing::enabled().then(std::time::Instant::now);
    let mut work = JournalWork::default();
    let result = discover_prepared(
        db,
        namespace,
        paths,
        complete,
        crate::model::now_ms,
        &mut work,
    );
    if let Some(started) = started {
        eprintln!(
            "semon sessions native: source_journal_batch elapsed_us={} authored_write_prepares={} prepare_us={} write_executions={} namespace_probes={} membership_rows={} revision_rows={} claim_rows={} state_rows={} success={}",
            started.elapsed().as_micros(),
            work.prepares,
            work.prepare_us,
            work.executions,
            work.namespace_probes,
            work.membership_rows,
            work.revision_rows,
            work.claim_rows,
            work.state_rows,
            result.is_ok()
        );
    }
    result
}

fn discover_prepared(
    db: &Connection,
    namespace: Option<&str>,
    paths: &[String],
    complete: bool,
    mut now: impl FnMut() -> i64,
    work: &mut JournalWork,
) -> rusqlite::Result<()> {
    crate::native_timing::discovery_count(|counts| {
        counts.statements += 1;
        counts.rows += paths.len();
    });
    refuse_bound(db, namespace, work)?;
    let mut invalidations = None;
    // Empty discovery still publishes its state, as before, but does not
    // prepare any membership or invalidation statements.
    let mut membership = if paths.is_empty() {
        None
    } else {
        let started = crate::native_timing::enabled().then(std::time::Instant::now);
        work.prepares += 1;
        let statement = db.prepare(if namespace.is_some() {
            "INSERT OR IGNORE INTO main.catalog_backfill_sources(namespace,source_path) VALUES(?1,?2)"
        } else {
            "INSERT OR IGNORE INTO catalog_backfill_sources VALUES(?1)"
        })?;
        work.prepare_us += started.map_or(0, |started| started.elapsed().as_micros());
        Some(statement)
    };
    for path in paths {
        crate::native_timing::discovery_count(|counts| counts.statements += 1);
        let membership = membership.as_mut().ok_or(rusqlite::Error::InvalidQuery)?;
        work.executions += 1;
        let inserted = if let Some(namespace) = namespace {
            membership.execute(params![namespace, path])?
        } else {
            membership.execute([path])?
        };
        work.membership_rows += inserted;
        if inserted > 0 {
            crate::native_timing::discovery_count(|counts| counts.statements += 3);
            invalidate_prepared(
                db,
                namespace,
                path,
                "changed",
                &mut now,
                &mut invalidations,
                work,
            )?;
        }
    }
    let state = if complete { "draining" } else { "discovering" };
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    work.prepares += 1;
    work.executions += 1;
    let started = crate::native_timing::enabled().then(std::time::Instant::now);
    if let Some(namespace) = namespace {
        let mut statement =
            db.prepare("UPDATE storage_source_journal SET backfill_state=?2 WHERE namespace=?1")?;
        work.prepare_us += started.map_or(0, |started| started.elapsed().as_micros());
        work.state_rows += statement.execute(params![namespace, state])?;
    } else {
        let mut statement = db
            .prepare("INSERT INTO meta(key,value) VALUES('catalog_backfill_state',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE meta.value<>excluded.value")?;
        work.prepare_us += started.map_or(0, |started| started.elapsed().as_micros());
        work.state_rows += statement.execute([state])?;
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

pub(crate) fn open(config: &StoreConfig) -> Result<Box<dyn SourceObservationStore>, StorageError> {
    crate::events::store::configured::validate_semantic_schema(config)?;
    let journal = SourceJournal(config.clone());
    journal.prepare()?;
    Ok(configured(config))
}

pub(crate) fn configured(config: &StoreConfig) -> Box<dyn SourceObservationStore> {
    Box::new(SourceJournal(config.clone()))
}

pub(crate) fn legacy(database: impl Into<PathBuf>) -> Box<dyn SourceObservationStore> {
    Box::new(LegacySourceJournal::new(database))
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
        db.create_scalar_function(
            "semon_semantic_publisher",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
            |_| Ok(0_i64),
        )
        .map_err(failed)?;
        // This typed, admitted source-observation producer is itself the
        // writer authority for its fenced source tables.  Configured callers
        // do not carry NativeOwnership, but their writes are still subject to
        // the existing namespace and epoch checks in `validate` below.
        db.create_scalar_function(
            "semon_event_writer",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
            |_| Ok(1_i64),
        )
        .map_err(failed)?;
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
        tx.execute_batch(crate::events::store::SOURCE_OBSERVATION_RECEIPTS_SCHEMA)
            .map_err(failed)?;
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

impl SourceJournal {
    pub(crate) fn database_path(&self) -> &PathBuf {
        match &self.config.backend {
            StoreBackend::Sqlite { database } => database,
        }
    }

    pub(crate) fn namespace(&self) -> &str {
        self.config.namespace.as_str()
    }

    pub(crate) fn operation(&self, kind: &str, payload: &[u8]) -> SourceOperation {
        SourceOperation::capture(self.namespace(), kind, payload)
    }

    pub(crate) fn invalidate_current(
        &mut self,
        expected: &[(String, Option<i64>)],
    ) -> Result<bool, StorageError> {
        let payload = serde_json::to_vec(expected).map_err(|_| StorageError::InvalidArguments)?;
        let operation = self.operation("invalidate", &payload);
        Ok(matches!(
            SourceObservationStore::invalidate(self, &operation, expected)?,
            SourceMutationReceipt::Applied | SourceMutationReceipt::AlreadyApplied
        ))
    }
}

impl SourceObservationStore for SourceJournal {
    fn discover(
        &mut self,
        operation: &SourceOperation,
        paths: &[String],
        complete: bool,
    ) -> Result<SourceMutationReceipt, StorageError> {
        validate_paths(paths)?;
        let payload =
            serde_json::to_vec(&(paths, complete)).map_err(|_| StorageError::InvalidArguments)?;
        let expected_namespace = self.config.namespace.as_str();
        if let Some(receipt) =
            operation_replay(operation, Some(expected_namespace), "discover", &payload)?
        {
            return Ok(receipt);
        }
        crate::native_timing::discovery_count(|counts| counts.statements += 2);
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some(receipt) = stored_receipt(&tx, Some(expected_namespace), operation)? {
            return Ok(receipt);
        }
        discover(&tx, Some(expected_namespace), paths, complete).map_err(failed)?;
        insert_receipt(&tx, Some(expected_namespace), operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }

    fn invalidate(
        &mut self,
        operation: &SourceOperation,
        expected: &[(String, Option<i64>)],
    ) -> Result<SourceMutationReceipt, StorageError> {
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
        let expected_namespace = self.config.namespace.as_str();
        let payload = serde_json::to_vec(expected).map_err(|_| StorageError::InvalidArguments)?;
        if let Some(receipt) =
            operation_replay(operation, Some(expected_namespace), "invalidate", &payload)?
        {
            return Ok(receipt);
        }
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some(receipt) = stored_receipt(&tx, Some(expected_namespace), operation)? {
            return Ok(receipt);
        }
        if claims(&tx, Some(expected_namespace), &paths).map_err(failed)? != expected {
            return Ok(SourceMutationReceipt::Conflict);
        }
        for path in paths {
            invalidate(&tx, Some(expected_namespace), &path, "changed").map_err(failed)?;
        }
        insert_receipt(&tx, Some(expected_namespace), operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
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

    fn finish_backfill(
        &mut self,
        operation: &SourceOperation,
        expected_generation: Option<&str>,
    ) -> Result<SourceMutationReceipt, StorageError> {
        let payload =
            serde_json::to_vec(&expected_generation).map_err(|_| StorageError::InvalidArguments)?;
        if let Some(receipt) = operation_replay(
            operation,
            Some(self.config.namespace.as_str()),
            "finish_backfill",
            &payload,
        )? {
            return Ok(receipt);
        }
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some(receipt) = stored_receipt(&tx, Some(self.config.namespace.as_str()), operation)?
        {
            return Ok(receipt);
        }
        let generation: Option<String> = tx
            .query_row(
                crate::events::store::configured::metadata_sql(
                    &tx,
                    "SELECT value FROM meta WHERE key='catalog_generation'",
                    "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation'",
                ).map_err(failed)?,
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=?1) OR EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [self.config.namespace.as_str()],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if generation.as_deref() != expected_generation || pending {
            return Ok(SourceMutationReceipt::Conflict);
        }
        tx.execute_batch(crate::events::store::configured::metadata_sql(
            &tx,
            "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state','complete'),('catalog_completeness','complete'),('search_backfill_complete','1'); DELETE FROM meta WHERE key='catalog_recovery_reason';",
            "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_backfill_state'),'catalog_backfill_state','complete'),(semon_meta_scope('catalog_completeness'),'catalog_completeness','complete'),(semon_meta_scope('search_backfill_complete'),'search_backfill_complete','1'); DELETE FROM meta WHERE namespace=semon_meta_scope('catalog_recovery_reason') AND key='catalog_recovery_reason';",
        ).map_err(failed)?).map_err(failed)?;
        let updated = tx
            .execute(
                "UPDATE storage_source_journal SET backfill_state='complete' WHERE namespace=?1",
                [self.config.namespace.as_str()],
            )
            .map_err(failed)?;
        if updated != 1 {
            return Err(StorageError::Unavailable);
        }
        insert_receipt(&tx, Some(self.config.namespace.as_str()), operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }
}

/// Legacy SQLite binding for the local native producer. It shares the exact
/// discovery, invalidation and claims SQL with the configured SourceJournal.
pub(crate) struct LegacySourceJournal {
    database: PathBuf,
}

impl LegacySourceJournal {
    pub(crate) fn new(database: impl Into<PathBuf>) -> Self {
        Self {
            database: database.into(),
        }
    }

    pub(crate) fn operation(&self, kind: &str, payload: &[u8]) -> SourceOperation {
        SourceOperation::capture("", kind, payload)
    }

    fn connection(&self) -> Result<Connection, StorageError> {
        let db = Connection::open_with_flags(
            &self.database,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(failed)?;
        db.busy_timeout(Duration::from_millis(100))
            .map_err(failed)?;
        Ok(db)
    }
}

impl SourceObservationStore for LegacySourceJournal {
    fn discover(
        &mut self,
        operation: &SourceOperation,
        paths: &[String],
        complete: bool,
    ) -> Result<SourceMutationReceipt, StorageError> {
        validate_paths(paths)?;
        let payload =
            serde_json::to_vec(&(paths, complete)).map_err(|_| StorageError::InvalidArguments)?;
        if let Some(receipt) = operation_replay(operation, None, "discover", &payload)? {
            return Ok(receipt);
        }
        crate::native_timing::discovery_count(|counts| counts.statements += 2);
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        refuse_bound(&tx, None, &mut JournalWork::default()).map_err(failed)?;
        crate::events::store::require_current_sql(&tx).map_err(failed)?;
        if let Some(receipt) = stored_receipt(&tx, None, operation)? {
            return Ok(receipt);
        }
        discover(&tx, None, paths, complete).map_err(failed)?;
        crate::events::store::require_current_sql(&tx).map_err(failed)?;
        insert_receipt(&tx, None, operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }

    fn invalidate(
        &mut self,
        operation: &SourceOperation,
        expected: &[(String, Option<i64>)],
    ) -> Result<SourceMutationReceipt, StorageError> {
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
        let payload = serde_json::to_vec(expected).map_err(|_| StorageError::InvalidArguments)?;
        if let Some(receipt) = operation_replay(operation, None, "invalidate", &payload)? {
            return Ok(receipt);
        }
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        refuse_bound(&tx, None, &mut JournalWork::default()).map_err(failed)?;
        crate::events::store::require_current_sql(&tx).map_err(failed)?;
        if let Some(receipt) = stored_receipt(&tx, None, operation)? {
            return Ok(receipt);
        }
        if claims(&tx, None, &paths).map_err(failed)? != expected {
            return Ok(SourceMutationReceipt::Conflict);
        }
        for path in paths {
            invalidate(&tx, None, &path, "changed").map_err(failed)?;
        }
        insert_receipt(&tx, None, operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }

    fn claims(
        &self,
        paths: &[String],
    ) -> Result<crate::storage::source_observations::SourceClaims, StorageError> {
        validate_paths(paths)?;
        let db = self.connection()?;
        refuse_bound(&db, None, &mut JournalWork::default()).map_err(failed)?;
        claims(&db, None, paths).map_err(failed)
    }

    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError> {
        if limit == 0 || limit > MAX_PATHS {
            return Err(StorageError::InvalidArguments);
        }
        let mut db = self.connection()?;
        let tx = db.unchecked_transaction().map_err(failed)?;
        refuse_bound(&tx, None, &mut JournalWork::default()).map_err(failed)?;
        crate::events::store::require_current_sql(&tx).map_err(failed)?;
        let selected: Vec<String> = tx.prepare("SELECT i.source_path FROM meta m JOIN session_catalog_invalidations i ON i.source_path=m.value WHERE m.key>='slot_upgrade_selected:' AND m.key<'slot_upgrade_selected;' ORDER BY i.revision,i.source_path LIMIT ?1").map_err(failed)?
            .query_map([limit as i64], |row| row.get(0)).map_err(failed)?
            .collect::<rusqlite::Result<_>>().map_err(failed)?;
        if !selected.is_empty() {
            return Ok(selected);
        }
        let mut statement = tx.prepare("SELECT source_path FROM session_catalog_invalidations ORDER BY revision,source_path LIMIT ?1").map_err(failed)?;
        let paths = statement
            .query_map([limit as i64], |row| row.get(0))
            .map_err(failed)?
            .collect::<rusqlite::Result<_>>()
            .map_err(failed)?;
        Ok(paths)
    }

    fn backfill_state(&self) -> Result<Option<String>, StorageError> {
        let db = self.connection()?;
        refuse_bound(&db, None, &mut JournalWork::default()).map_err(failed)?;
        crate::events::store::require_current_sql(&db).map_err(failed)?;
        db.query_row(
            "SELECT value FROM meta WHERE key='catalog_backfill_state'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)
    }

    fn finish_backfill(
        &mut self,
        operation: &SourceOperation,
        expected_generation: Option<&str>,
    ) -> Result<SourceMutationReceipt, StorageError> {
        let payload =
            serde_json::to_vec(&expected_generation).map_err(|_| StorageError::InvalidArguments)?;
        if let Some(receipt) = operation_replay(operation, None, "finish_backfill", &payload)? {
            return Ok(receipt);
        }
        let mut db = self.connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        refuse_bound(&tx, None, &mut JournalWork::default()).map_err(failed)?;
        crate::events::store::require_current_sql(&tx).map_err(failed)?;
        if let Some(receipt) = stored_receipt(&tx, None, operation)? {
            return Ok(receipt);
        }
        if !finish_catalog_backfill_sql(&tx, None, expected_generation)? {
            return Ok(SourceMutationReceipt::Conflict);
        }
        insert_receipt(&tx, None, operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }
}

pub(crate) fn operation_replay(
    operation: &SourceOperation,
    namespace: Option<&str>,
    kind: &str,
    payload: &[u8],
) -> Result<Option<SourceMutationReceipt>, StorageError> {
    if operation.namespace != namespace.unwrap_or("") {
        return Ok(Some(SourceMutationReceipt::Conflict));
    }
    if operation.id.is_empty()
        || operation.id.len() > 256
        || !operation.id.is_ascii()
        || operation.payload_digest.len() != 64
        || !operation
            .payload_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StorageError::InvalidArguments);
    }
    let prefix = format!(
        "source-observation-v1:{}:",
        SourceOperation::namespace_tag(namespace.unwrap_or(""))
    );
    let Some(suffix) = operation.id.strip_prefix(&prefix) else {
        return Ok(Some(SourceMutationReceipt::Conflict));
    };
    let parts = suffix.split(':').collect::<Vec<_>>();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(StorageError::InvalidArguments);
    }
    let expected = SourceOperation::capture(namespace.unwrap_or(""), kind, payload);
    if operation.payload_digest != expected.payload_digest {
        return Ok(Some(SourceMutationReceipt::Conflict));
    }
    Ok(None)
}

pub(crate) fn stored_receipt(
    db: &Connection,
    namespace: Option<&str>,
    operation: &SourceOperation,
) -> Result<Option<SourceMutationReceipt>, StorageError> {
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    let digest: Option<String> = if let Some(namespace) = namespace {
        db.query_row(
            "SELECT payload_digest FROM main.source_observation_receipts WHERE namespace=?1 AND operation_id=?2",
            params![namespace, operation.id],
            |row| row.get(0),
        ).optional().map_err(failed)?
    } else {
        db.query_row(
            "SELECT payload_digest FROM main.source_observation_receipts WHERE namespace='' AND operation_id=?1",
            [operation.id.as_str()],
            |row| row.get(0),
        ).optional().map_err(failed)?
    };
    Ok(digest.map(|digest| {
        if digest == operation.payload_digest {
            SourceMutationReceipt::AlreadyApplied
        } else {
            SourceMutationReceipt::Conflict
        }
    }))
}

pub(crate) fn insert_receipt(
    tx: &Connection,
    namespace: Option<&str>,
    operation: &SourceOperation,
) -> Result<(), StorageError> {
    crate::native_timing::discovery_count(|counts| {
        counts.statements += 1;
        counts.rows += 1;
    });
    if let Some(namespace) = namespace {
        tx.execute(
            "INSERT INTO main.source_observation_receipts(namespace,operation_id,payload_digest) VALUES(?1,?2,?3)",
            params![namespace, operation.id, operation.payload_digest],
        ).map_err(failed)?;
    } else {
        tx.execute(
            "INSERT INTO main.source_observation_receipts(namespace,operation_id,payload_digest) VALUES('',?1,?2)",
            params![operation.id, operation.payload_digest],
        ).map_err(failed)?;
    }
    Ok(())
}

fn finish_catalog_backfill_sql(
    tx: &Connection,
    namespace: Option<&str>,
    expected_generation: Option<&str>,
) -> Result<bool, StorageError> {
    let generation_sql = if namespace.is_some() {
        "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation'"
    } else {
        "SELECT value FROM meta WHERE key='catalog_generation'"
    };
    let generation: Option<String> = tx
        .query_row(generation_sql, [], |row| row.get(0))
        .optional()
        .map_err(failed)?;
    let pending: bool = if let Some(namespace) = namespace {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=?1) OR EXISTS(SELECT 1 FROM catalog_semantic_stage)",
            [namespace],
            |row| row.get(0),
        ).map_err(failed)?
    } else {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations) OR EXISTS(SELECT 1 FROM catalog_semantic_stage)",
            [],
            |row| row.get(0),
        ).map_err(failed)?
    };
    if generation.as_deref() != expected_generation || pending {
        return Ok(false);
    }
    if let Some(namespace) = namespace {
        let sql = crate::events::store::configured::metadata_sql(
            tx,
            "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state','complete'),('catalog_completeness','complete'),('search_backfill_complete','1'); DELETE FROM meta WHERE key='catalog_recovery_reason';",
            "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_backfill_state'),'catalog_backfill_state','complete'),(semon_meta_scope('catalog_completeness'),'catalog_completeness','complete'),(semon_meta_scope('search_backfill_complete'),'search_backfill_complete','1'); DELETE FROM meta WHERE namespace=semon_meta_scope('catalog_recovery_reason') AND key='catalog_recovery_reason';",
        ).map_err(failed)?;
        tx.execute_batch(&sql).map_err(failed)?;
        let updated = tx
            .execute(
                "UPDATE storage_source_journal SET backfill_state='complete' WHERE namespace=?1",
                [namespace],
            )
            .map_err(failed)?;
        if updated != 1 {
            return Err(StorageError::Unavailable);
        }
    } else {
        tx.execute_batch("INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state','complete'),('catalog_completeness','complete'),('search_backfill_complete','1'); DELETE FROM meta WHERE key='catalog_recovery_reason';").map_err(failed)?;
    }
    Ok(true)
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

    fn discover_operation(namespace: &str, paths: &[String], complete: bool) -> SourceOperation {
        let payload = serde_json::to_vec(&(paths, complete)).unwrap();
        SourceOperation::capture(namespace, "discover", &payload)
    }

    fn invalidate_operation(
        namespace: &str,
        expected: &[(String, Option<i64>)],
    ) -> SourceOperation {
        let payload = serde_json::to_vec(expected).unwrap();
        SourceOperation::capture(namespace, "invalidate", &payload)
    }

    fn finish_operation(namespace: &str, generation: Option<&str>) -> SourceOperation {
        let payload = serde_json::to_vec(&generation).unwrap();
        SourceOperation::capture(namespace, "finish_backfill", &payload)
    }

    fn batch_db(scoped: bool) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO meta VALUES('catalog_backfill_state','draining');").unwrap();
        if scoped {
            db.execute_batch("CREATE TABLE catalog_backfill_sources(namespace TEXT NOT NULL,source_path TEXT NOT NULL,PRIMARY KEY(namespace,source_path)); CREATE TABLE session_catalog_invalidations(namespace TEXT NOT NULL,source_path TEXT NOT NULL,revision INTEGER NOT NULL,reason TEXT NOT NULL,observed_at INTEGER NOT NULL,PRIMARY KEY(namespace,source_path)); CREATE TABLE storage_source_journal(namespace TEXT PRIMARY KEY,revision INTEGER NOT NULL,backfill_state TEXT); INSERT INTO storage_source_journal VALUES('A',0,'draining'),('B',99,'complete'); INSERT INTO session_catalog_invalidations VALUES('B','foreign',99,'retained',11);").unwrap();
        } else {
            db.execute_batch("CREATE TABLE catalog_backfill_sources(source_path TEXT PRIMARY KEY); CREATE TABLE session_catalog_invalidations(source_path TEXT PRIMARY KEY,revision INTEGER NOT NULL,reason TEXT NOT NULL,observed_at INTEGER NOT NULL);").unwrap();
        }
        db
    }

    fn batch_rows(db: &Connection, scoped: bool) -> Vec<(String, i64, String, i64)> {
        db.prepare(if scoped {
            "SELECT source_path,revision,reason,observed_at FROM session_catalog_invalidations WHERE namespace='A' ORDER BY source_path"
        } else {
            "SELECT source_path,revision,reason,observed_at FROM session_catalog_invalidations ORDER BY source_path"
        }).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn discovery_batch_prepares_are_constant_and_claims_match_rowwise() {
        for scoped in [false, true] {
            let mut db = batch_db(scoped);
            let mut reference = batch_db(scoped);
            let namespace = scoped.then_some("A");
            let paths: Vec<String> = (0..64).map(|i| format!("source-{i:02}")).collect();
            let mut work = JournalWork::default();
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            discover_prepared(&tx, namespace, &paths, true, || 42, &mut work).unwrap();
            tx.commit().unwrap();
            assert_eq!(work.prepares, 4);
            assert_eq!(work.executions, 193);
            assert_eq!(work.namespace_probes, if scoped { 0 } else { 65 });
            assert_eq!(
                (work.membership_rows, work.revision_rows, work.claim_rows),
                (64, 64, 64)
            );
            let tx = reference
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let mut rowwise = JournalWork::default();
            for path in &paths {
                discover_prepared(
                    &tx,
                    namespace,
                    std::slice::from_ref(path),
                    true,
                    || 42,
                    &mut rowwise,
                )
                .unwrap();
            }
            tx.commit().unwrap();
            assert_eq!(rowwise.prepares, 256);
            let expected: Vec<_> = paths
                .iter()
                .enumerate()
                .map(|(i, path)| (path.clone(), i as i64 + 1, "changed".to_owned(), 42))
                .collect();
            assert_eq!(batch_rows(&db, scoped), expected);
            assert_eq!(batch_rows(&db, scoped), batch_rows(&reference, scoped));
            assert_eq!(
                claims(&db, namespace, &paths).unwrap(),
                claims(&reference, namespace, &paths).unwrap()
            );
            // Duplicate discovery neither advances claims nor prepares the two
            // invalidation statements. Membership and state still run.
            let before = batch_rows(&db, scoped);
            let mut repeated = JournalWork::default();
            discover_prepared(&db, namespace, &paths, true, || 77, &mut repeated).unwrap();
            assert_eq!(repeated.prepares, 2);
            assert_eq!(
                (
                    repeated.membership_rows,
                    repeated.revision_rows,
                    repeated.claim_rows
                ),
                (0, 0, 0)
            );
            assert_eq!(batch_rows(&db, scoped), before);
            if scoped {
                let foreign: (i64, String, i64) = db.query_row("SELECT revision,reason,observed_at FROM session_catalog_invalidations WHERE namespace='B'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
                assert_eq!(foreign, (99, "retained".into(), 11));
            }
        }
    }

    #[test]
    fn discovery_batch_empty_refusal_rollback_and_schema_aba() {
        let mut db = batch_db(false);
        let mut work = JournalWork::default();
        let before = db.total_changes();
        discover_prepared(&db, None, &[], true, || 42, &mut work).unwrap();
        assert_eq!(db.total_changes(), before);
        assert_eq!((work.prepares, work.executions), (1, 1));
        assert_eq!(
            (
                work.membership_rows,
                work.revision_rows,
                work.claim_rows,
                work.state_rows
            ),
            (0, 0, 0, 0)
        );
        // Caller-owned rollback retires all prepared statements with the call.
        db.execute_batch("CREATE TRIGGER batch_fault BEFORE INSERT ON catalog_backfill_sources WHEN NEW.source_path='fault' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(
            discover_prepared(
                &tx,
                None,
                &["first".into(), "fault".into()],
                true,
                || 42,
                &mut JournalWork::default()
            )
            .is_err()
        );
        tx.rollback().unwrap();
        assert!(batch_rows(&db, false).is_empty());
        let membership: i64 = db
            .query_row("SELECT count(*) FROM catalog_backfill_sources", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(membership, 0);
        let revision: Option<String> = db
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_source_revision'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(revision, None);
        // A rolled-back binding DDL must not poison a subsequent unbound call.
        db.execute_batch(
            "BEGIN IMMEDIATE; CREATE TABLE storage_namespaces(namespace TEXT); ROLLBACK;",
        )
        .unwrap();
        discover_prepared(
            &db,
            None,
            &["after-aba".into()],
            true,
            || 42,
            &mut JournalWork::default(),
        )
        .unwrap();
        db.execute_batch("CREATE TABLE storage_namespaces(namespace TEXT)")
            .unwrap();
        let before = db.total_changes();
        let mut refused = JournalWork::default();
        assert!(matches!(
            discover_prepared(&db, None, &["refused".into()], true, || 42, &mut refused),
            Err(rusqlite::Error::InvalidQuery)
        ));
        assert_eq!(refused.prepares, 0);
        assert_eq!(db.total_changes(), before);
        assert!(matches!(
            invalidate(&db, None, "refused", "changed"),
            Err(rusqlite::Error::InvalidQuery)
        ));
        assert_eq!(db.total_changes(), before);
    }

    #[test]
    fn discovery_batch_revision_overflow_rolls_back_membership() {
        let mut db = batch_db(true);
        db.execute(
            "UPDATE storage_source_journal SET revision=9223372036854775807 WHERE namespace='A'",
            [],
        )
        .unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(matches!(
            discover_prepared(
                &tx,
                Some("A"),
                &["overflow".into()],
                true,
                || 42,
                &mut JournalWork::default()
            ),
            Err(rusqlite::Error::InvalidQuery)
        ));
        tx.rollback().unwrap();
        assert!(batch_rows(&db, true).is_empty());
        let membership: i64 = db
            .query_row("SELECT count(*) FROM catalog_backfill_sources", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(membership, 0);
        let revision: i64 = db
            .query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace='A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revision, i64::MAX);
    }

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
    }
    impl Fixture {
        fn new() -> Self {
            Self::new_with_namespace("A")
        }

        fn new_with_namespace(namespace: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-source-journal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache_path = root.join("index.json");
            drop(EventCache::open_scoped(&cache_path).unwrap());
            // Exercise the actual unbound producer before admission. Its old
            // counter must survive even when a newer observation was acked.
            let seed_paths = vec!["seed".into(), "acknowledged".into()];
            let mut native = legacy(EventCache::path(&cache_path));
            native
                .discover(
                    &discover_operation("", &seed_paths, true),
                    &seed_paths,
                    true,
                )
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
                namespace: LogicalSourceNamespace::new(namespace).unwrap(),
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

    struct LegacyFixture {
        root: PathBuf,
        cache_path: PathBuf,
    }

    impl LegacyFixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-source-observation-legacy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache_path = root.join("index.json");
            drop(EventCache::open_scoped(&cache_path).unwrap());
            Self { root, cache_path }
        }

        fn open(&self) -> Box<dyn SourceObservationStore> {
            legacy(EventCache::path(&self.cache_path))
        }
    }

    impl Drop for LegacyFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn shared_source_observation_conformance(
        mut open: impl FnMut() -> Box<dyn SourceObservationStore>,
        mut drain_before_finish: impl FnMut(),
        namespace: &str,
    ) {
        let paths = vec!["conformance-a".to_owned(), "conformance-b".to_owned()];
        let discovery = discover_operation(namespace, &paths, true);
        let mut store = open();
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::Applied
        );
        let initial_claims = store.claims(&paths).unwrap();
        drop(store);

        // The exact same id and payload reconcile a committed operation after
        // reopening, including the unknown-ACK retry case.
        let mut store = open();
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
        assert_eq!(store.claims(&paths).unwrap(), initial_claims);

        let partial = vec!["conformance-partial".to_owned()];
        let partial_operation = discover_operation(namespace, &partial, false);
        assert_eq!(
            store.discover(&partial_operation, &partial, false).unwrap(),
            SourceMutationReceipt::Applied
        );
        let before_cas = store.claims(&partial).unwrap();
        let accepted_cas = invalidate_operation(namespace, &before_cas);
        assert_eq!(
            store.invalidate(&accepted_cas, &before_cas).unwrap(),
            SourceMutationReceipt::Applied
        );
        let after_cas = store.claims(&partial).unwrap();
        let failed_cas = invalidate_operation(namespace, &before_cas);
        assert_eq!(
            store.invalidate(&failed_cas, &before_cas).unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.claims(&partial).unwrap(), after_cas);

        let changed_paths = vec!["conformance-changed-payload".to_owned()];
        let mut changed_operation = discover_operation(namespace, &changed_paths, false);
        changed_operation.id = discovery.id.clone();
        let before_conflict = store.claims(&changed_paths).unwrap();
        assert_eq!(
            store
                .discover(&changed_operation, &changed_paths, false)
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.claims(&changed_paths).unwrap(), before_conflict);

        let foreign_paths = vec!["conformance-foreign".to_owned()];
        let foreign_operation = discover_operation("foreign-namespace", &foreign_paths, true);
        assert_eq!(
            store
                .discover(&foreign_operation, &foreign_paths, true)
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(
            store.claims(&foreign_paths).unwrap(),
            vec![(foreign_paths[0].clone(), None)]
        );

        // Namespace bytes are hashed as length-delimited UTF-8; a NUL never
        // enters the SQL identity or becomes a second operation boundary.
        let encoded = SourceOperation::capture(namespace, "discover", b"nul-safe");
        assert!(encoded.id.is_ascii());
        assert!(!encoded.id.as_bytes().contains(&0));
        assert_eq!(encoded.namespace.as_bytes(), namespace.as_bytes());

        // Finishing is a strict generation/CAS transition: source claims above
        // deliberately left invalidations pending, so model the real consumer
        // acknowledgement before attempting the finish.
        drain_before_finish();
        let finish = finish_operation(namespace, None);
        assert_eq!(
            store.finish_backfill(&finish, None).unwrap(),
            SourceMutationReceipt::Applied
        );
        let completed = store.backfill_state().unwrap();
        let wrong_generation = finish_operation(namespace, Some("wrong-generation"));
        assert_eq!(
            store
                .finish_backfill(&wrong_generation, Some("wrong-generation"))
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.backfill_state().unwrap(), completed);
        drop(store);
        let mut reopened = open();
        assert_eq!(
            reopened.finish_backfill(&finish, None).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
    }

    #[test]
    fn shared_observation_conformance_runs_on_legacy_and_configured_sqlite() {
        let legacy = LegacyFixture::new();
        shared_source_observation_conformance(
            || legacy.open(),
            || {
                let db = Connection::open(EventCache::path(&legacy.cache_path)).unwrap();
                db.execute("DELETE FROM session_catalog_invalidations", []).unwrap();
            },
            "",
        );

        let configured = Fixture::new_with_namespace("configured\0source");
        configured.assign();
        shared_source_observation_conformance(
            || configured.config.source_observations().unwrap(),
            || {
                let db = SourceJournal(configured.config.clone()).connection().unwrap();
                db.execute(
                    "DELETE FROM session_catalog_invalidations WHERE namespace=?1",
                    [configured.config.namespace.as_str()],
                )
                .unwrap();
            },
            "configured\0source",
        );
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
        let discovery = discover_operation("A", &paths, true);
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::Applied
        );
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
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
        assert_eq!(store.claims(&paths).unwrap(), captured);
        assert_eq!(store.pending(1).unwrap(), vec!["seed".to_owned()]);
        let mut concurrent = f.config.source_observations().unwrap();
        let concurrent_cas = invalidate_operation("A", &captured[1..]);
        assert_eq!(
            concurrent
                .invalidate(&concurrent_cas, &captured[1..])
                .unwrap(),
            SourceMutationReceipt::Applied
        );
        let before_conflict = store.claims(&paths).unwrap();
        let failed_cas = invalidate_operation("A", &captured);
        assert_eq!(
            store.invalidate(&failed_cas, &captured).unwrap(),
            SourceMutationReceipt::Conflict
        );
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
        let operation = discover_operation("A", &paths, false);
        assert_eq!(
            store.discover(&operation, &paths, false),
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
        let overlong = vec!["x".repeat(4097)];
        assert_eq!(
            store.discover(&discover_operation("A", &overlong, true), &overlong, true),
            Err(StorageError::InvalidArguments)
        );
        assert_eq!(
            store.claims(&vec!["x".into(); 257]),
            Err(StorageError::InvalidArguments)
        );
        assert_eq!(store.pending(0), Err(StorageError::InvalidArguments));
        assert_eq!(store.pending(257), Err(StorageError::InvalidArguments));
        let duplicate_claim = vec![("seed".into(), Some(1)), ("seed".into(), Some(1))];
        assert_eq!(
            store.invalidate(
                &invalidate_operation("A", &duplicate_claim),
                &duplicate_claim,
            ),
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
        let locked = vec!["locked".into()];
        assert_eq!(
            store.discover(&discover_operation("A", &locked, true), &locked, true),
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
        let next_epoch = vec!["new-epoch".into()];
        assert_eq!(
            store.discover(
                &discover_operation("A", &next_epoch, true),
                &next_epoch,
                true
            ),
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
