//! Atomic discovery/invalidation adapter. The configured family is functional;
//! Event/ledger/proof and Catalog publication remain independently fenced.
use super::{catalog_namespace, failed};
use crate::storage::{
    NativeReaderBinding, StorageError, StoreConfig,
    source_observations::{
        SourceIntent, SourceMutationReceipt, SourceObservationStore, SourceOperation,
        operation_replay, validate_intent, validate_paths,
    },
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const SOURCE_JOURNAL_QUALIFICATION_VERSION: u32 = 1;
const MAX_PATHS: usize = 256;
const PENDING_SQL: &str = "SELECT source_path FROM main.session_catalog_invalidations INDEXED BY storage_ns_source_pending WHERE namespace=?1 ORDER BY revision,source_path LIMIT ?2";
const SELECTED_PENDING_SQL: &str = "SELECT i.source_path FROM main.meta AS m CROSS JOIN main.session_catalog_invalidations AS i WHERE m.namespace=?1 AND m.key>='slot_upgrade_selected:' AND m.key<'slot_upgrade_selected;' AND i.namespace=?1 AND i.source_path=m.value AND EXISTS(SELECT 1 FROM main.storage_source_journal WHERE namespace=?1 AND backfill_state='draining') ORDER BY m.key LIMIT ?2";

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

#[allow(clippy::too_many_arguments)]
fn invalidate_prepared<'a>(
    db: &'a Connection,
    namespace: &str,
    path: &str,
    reason: &str,
    now: &mut impl FnMut() -> i64,
    statements: &mut Option<InvalidationStatements<'a>>,
    work: &mut JournalWork,
) -> rusqlite::Result<()> {
    // Preserve the per-invalidation guard, including after a membership write.
    if statements.is_none() {
        let revision = "UPDATE main.storage_source_journal SET revision=revision+1 WHERE namespace=?1 AND revision<9223372036854775807";
        let claim = "INSERT INTO main.session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(?1,?2,(SELECT revision FROM main.storage_source_journal WHERE namespace=?1),?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET revision=excluded.revision,reason=excluded.reason,observed_at=excluded.observed_at";
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
    let rows = statements
        .revision
        .execute([namespace])
        .map_err(|error| diagnose_sql("revision-write", error))?;
    work.revision_rows += rows;
    if rows != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    work.executions += 1;
    work.claim_rows += statements
        .claim
        .execute(params![namespace, path, reason, now()])?;
    Ok(())
}

pub(crate) fn invalidate(
    db: &Connection,
    namespace: &str,
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
    namespace: &str,
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
    namespace: &str,
    paths: &[String],
    complete: bool,
    mut now: impl FnMut() -> i64,
    work: &mut JournalWork,
) -> rusqlite::Result<()> {
    crate::native_timing::discovery_count(|counts| {
        counts.statements += 1;
        counts.rows += paths.len();
    });
    let mut invalidations = None;
    // Empty discovery still publishes its state, as before, but does not
    // prepare any membership or invalidation statements.
    let mut membership = if paths.is_empty() {
        None
    } else {
        let started = crate::native_timing::enabled().then(std::time::Instant::now);
        work.prepares += 1;
        let statement = db.prepare("INSERT OR IGNORE INTO main.catalog_backfill_sources(namespace,source_path) VALUES(?1,?2)")?;
        work.prepare_us += started.map_or(0, |started| started.elapsed().as_micros());
        Some(statement)
    };
    for path in paths {
        crate::native_timing::discovery_count(|counts| counts.statements += 1);
        let membership = membership.as_mut().ok_or(rusqlite::Error::InvalidQuery)?;
        work.executions += 1;
        let inserted = membership
            .execute(params![namespace, path])
            .map_err(|error| diagnose_sql("membership-write", error))?;
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
    let mut statement = db.prepare("UPDATE main.storage_source_journal SET backfill_state=?2 WHERE namespace=?1 AND backfill_state IS NOT ?2")?;
    work.prepare_us += started.map_or(0, |started| started.elapsed().as_micros());
    work.state_rows += statement.execute(params![namespace, state])?;
    Ok(())
}

pub(crate) fn claims(
    db: &Connection,
    namespace: &str,
    paths: &[String],
) -> rusqlite::Result<crate::storage::source_observations::SourceClaims> {
    let sql = "SELECT revision FROM main.session_catalog_invalidations WHERE namespace=?2 AND source_path=?1";
    let mut statement = db.prepare(sql)?;
    paths
        .iter()
        .map(|path| {
            let revision = statement
                .query_row(params![path, namespace], |r| r.get(0))
                .optional()?;
            Ok((path.clone(), revision))
        })
        .collect()
}

pub(crate) struct SourceJournal {
    writer: std::cell::RefCell<Option<Connection>>,
    config: StoreConfig,
    owner: Option<crate::events::store::configured::NativeOwnership>,
}
#[allow(non_snake_case)]
pub(crate) fn SourceJournal(config: StoreConfig) -> SourceJournal {
    SourceJournal {
        writer: std::cell::RefCell::new(None),
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

impl SourceJournal {
    pub(crate) fn owned(owner: crate::events::store::configured::NativeOwnership) -> Self {
        crate::pricing::runtime::register_index(owner.clone());
        Self {
            writer: std::cell::RefCell::new(None),
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
        let database = self.config.backend.sqlite_database()?;
        if database.as_os_str().is_empty() || database == std::path::Path::new(":memory:") {
            return Err(StorageError::InvalidArguments);
        }
        let db = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
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
        register_backfill_writer(&db, false)?;
        Ok(db)
    }

    // Reuse this journal's short operation connection. Long-lived ReaderLease
    // snapshots remain independent. Qualify physical layout before reserving the
    // writer; validate() still checks live admission and epoch in every tx.
    fn writer(&self) -> Result<std::cell::RefMut<'_, Connection>, StorageError> {
        let mut writer = self.writer.borrow_mut();
        if writer.is_none() {
            *writer = Some(self.connection()?);
        }
        let writer = std::cell::RefMut::map(writer, |writer| writer.as_mut().unwrap());
        crate::events::store::configured::NativeOwnership::validate_physical(&writer)?;
        Ok(writer)
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

    /// Validate the enrolled current journal in a read snapshot. Reopen never
    /// creates tables, repairs profiles or adopts metadata counters.
    pub(crate) fn prepare(&self) -> Result<(), StorageError> {
        let mut db = self.writer()?;
        let tx = db.transaction().map_err(failed)?;
        self.validate(&tx)?;
        let actual: Option<String> = tx.query_row("SELECT sql FROM main.sqlite_schema WHERE type='table' AND name='storage_source_journal'", [], |r|r.get(0)).optional().map_err(failed)?;
        let expected = JOURNAL_SCHEMA.replace(" IF NOT EXISTS", "");
        if actual.as_deref() != Some(expected.trim_end_matches(';')) {
            return Err(StorageError::NamespaceUpgradePending);
        }
        let valid: bool = tx.query_row("SELECT qualification=?2 AND revision>=coalesce((SELECT max(revision) FROM main.session_catalog_invalidations WHERE namespace=?1),0) AND (backfill_state IS NULL OR backfill_state IN('discovering','draining','complete')) FROM storage_source_journal WHERE namespace=?1", params![self.config.namespace.as_str(),SOURCE_JOURNAL_QUALIFICATION_VERSION], |r|r.get(0)).optional().map_err(failed)?.unwrap_or(false);
        if !valid {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for table in ["source_observation_receipts", "source_observation_intents"] {
            for (action, predicate) in [
                ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
                (
                    "UPDATE",
                    "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
                ),
                ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
            ] {
                let name = format!("storage_source_guard_{table}_{action}");
                let expected = format!(
                    "CREATE TRIGGER {name} BEFORE {action} ON {table} WHEN semon_event_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'source operation namespace refused'); END"
                );
                let actual: Option<String> = tx.query_row("SELECT sql FROM main.sqlite_schema WHERE type='trigger' AND name=?1 AND tbl_name=?2", params![name,table], |r|r.get(0)).optional().map_err(failed)?;
                if actual.as_deref() != Some(expected.as_str()) {
                    return Err(StorageError::NamespaceUpgradePending);
                }
            }
        }
        catalog_namespace::validate_guard(&tx, "storage_source_journal")?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }
}

impl SourceJournal {
    pub(crate) fn database_path(&self) -> &PathBuf {
        self.config
            .backend
            .sqlite_database()
            .expect("admitted SQLite Source journal")
    }

    pub(crate) fn namespace(&self) -> &str {
        self.config.namespace.as_str()
    }
}

impl SourceObservationStore for SourceJournal {
    fn recovery_is_current(&mut self) -> Result<bool, StorageError> {
        let mut connection = self.writer()?;
        let db = connection.transaction().map_err(failed)?;
        self.validate(&db)?;
        self.owner.as_ref().ok_or(StorageError::Unavailable)?;
        // A retained connection is not a committed discovery witness. Run the
        // bounded durable page until the current producer records completion.
        drop(db);
        Ok(false)
    }

    fn recovery_cursor(&self) -> Result<Option<String>, StorageError> {
        let mut connection = self.writer()?;
        let db = connection.transaction().map_err(failed)?;
        self.validate(&db)?;
        self.owner.as_ref().ok_or(StorageError::Unavailable)?;
        db.query_row(
            "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_recovery_after'",
            [],
            |row| row.get(0),
        ).optional().map_err(failed)
    }

    fn recover(
        &mut self,
        operation: &SourceOperation,
        cursor: Option<&str>,
    ) -> Result<(SourceMutationReceipt, bool), StorageError> {
        self.owner.as_ref().ok_or(StorageError::Unavailable)?;
        if cursor.is_some_and(|value| value.len() > 4096) {
            return Err(StorageError::InvalidArguments);
        }
        // Recovery moves only this Source's cursor; another namespace's exact
        // OpID is refused rather than recorded as a local Conflict.
        if operation.namespace != self.namespace() {
            return Err(StorageError::NamespaceMismatch);
        }
        let namespace = self.namespace();
        let payload = serde_json::to_vec(&cursor).map_err(|_| StorageError::InvalidArguments)?;
        if let Some(receipt) = operation_replay(operation, Some(namespace), "recover", &payload)? {
            return Ok((receipt, false));
        }
        let mut db = self.writer()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some((receipt, done)) = recovery_receipt(&tx, namespace, operation)? {
            let terminal: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.source_observation_receipts WHERE namespace=?1 AND operation_id=?2 AND payload_digest=?3 AND recovery_done IS NOT NULL)",
                params![self.namespace(), operation.id, operation.payload_digest], |row| row.get(0),
            ).map_err(failed)?;
            if terminal {
                acknowledge_intent_sql(&tx, namespace, operation)?;
            }
            tx.commit().map_err(|_| StorageError::CommitUncertain)?;
            return Ok((receipt, done));
        }
        let current: Option<String> = tx.query_row(
            "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_recovery_after'",
            [], |row| row.get(0),
        ).optional().map_err(failed)?;
        let applied = current.as_deref() == cursor;
        let done = applied
            && crate::events::store::recovery_page(&tx).map_err(|error| {
                #[cfg(test)]
                eprintln!(
                    "source-journal phase=recovery-page sqlite_extended_code={:?}",
                    error
                        .get_ref()
                        .and_then(|inner| inner.downcast_ref::<rusqlite::Error>())
                        .and_then(rusqlite::Error::sqlite_error)
                        .map(|code| code.extended_code)
                );
                #[cfg(not(test))]
                let _ = error;
                StorageError::Unavailable
            })?;
        #[cfg(test)]
        crate::events::store::configured::effect("recovery_receipt").map_err(failed)?;
        insert_recovery_receipt(&tx, namespace, operation, applied, done)?;
        acknowledge_intent_sql(&tx, namespace, operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        #[cfg(test)]
        if crate::events::store::configured::effect("recovery_commit_reply").is_err() {
            return Err(StorageError::CommitUncertain);
        }
        Ok((
            if applied {
                SourceMutationReceipt::Applied
            } else {
                SourceMutationReceipt::Conflict
            },
            done,
        ))
    }

    fn persist_intent(&mut self, intent: &SourceIntent) -> Result<(), StorageError> {
        if matches!(intent, SourceIntent::Recover { .. }) && self.owner.is_none() {
            return Err(StorageError::Unavailable);
        }
        let mut db = self.writer()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        persist_intent_sql(&tx, self.namespace(), intent)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }

    fn pending_intents(&self) -> Result<Vec<SourceIntent>, StorageError> {
        let mut db = self.writer()?;
        let tx = db.transaction().map_err(failed)?;
        self.validate(&tx)?;
        load_intents(&tx, self.namespace())
    }

    fn acknowledge_intent(&mut self, operation: &SourceOperation) -> Result<(), StorageError> {
        let mut db = self.writer()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        acknowledge_intent_sql(&tx, self.namespace(), operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }

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
        let mut db = self.writer()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some(receipt) = stored_receipt(&tx, expected_namespace, operation)? {
            return Ok(receipt);
        }
        discover(&tx, expected_namespace, paths, complete)
            .map_err(|error| phase_failed("discover-effect", error))?;
        insert_receipt(&tx, expected_namespace, operation)?;
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
        let mut db = self.writer()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some(receipt) = stored_receipt(&tx, expected_namespace, operation)? {
            return Ok(receipt);
        }
        if claims(&tx, expected_namespace, &paths).map_err(failed)? != expected {
            insert_terminal_receipt(&tx, expected_namespace, operation, "conflict")?;
            tx.commit().map_err(|_| StorageError::CommitUncertain)?;
            return Ok(SourceMutationReceipt::Conflict);
        }
        for path in paths {
            invalidate(&tx, expected_namespace, &path, "changed").map_err(failed)?;
        }
        insert_receipt(&tx, expected_namespace, operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }

    fn claims(
        &self,
        paths: &[String],
    ) -> Result<crate::storage::source_observations::SourceClaims, StorageError> {
        validate_paths(paths)?;
        let mut connection = self.writer()?;
        let db = connection.transaction().map_err(failed)?;
        self.validate(&db)?;
        claims(&db, self.config.namespace.as_str(), paths).map_err(failed)
    }

    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError> {
        if limit == 0 || limit > MAX_PATHS {
            return Err(StorageError::InvalidArguments);
        }
        let mut connection = self.writer()?;
        let db = connection.transaction().map_err(failed)?;
        self.validate(&db)?;
        // Selected demand shares the existing claims. Do not replace their
        // revisions or drain unrelated work ahead of a selected upgrade.
        let selected = db
            .prepare(SELECTED_PENDING_SQL)
            .map_err(failed)?
            .query_map(params![self.config.namespace.as_str(), limit as i64], |r| {
                r.get(0)
            })
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<String>>>()
            .map_err(failed)?;
        if !selected.is_empty() {
            return Ok(selected);
        }
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
        let mut connection = self.writer()?;
        let db = connection.transaction().map_err(failed)?;
        self.validate(&db)?;
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
        let mut db = self.writer()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if let Some(receipt) = stored_receipt(&tx, self.config.namespace.as_str(), operation)? {
            return Ok(receipt);
        }
        let generation: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=?1) OR EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1)",
                [self.config.namespace.as_str()],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if generation.as_deref() != expected_generation || pending {
            insert_terminal_receipt(&tx, self.namespace(), operation, "conflict")?;
            tx.commit().map_err(|_| StorageError::CommitUncertain)?;
            return Ok(SourceMutationReceipt::Conflict);
        }
        register_backfill_writer(&tx, true)?;
        tx.execute_batch("INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_backfill_state'),'catalog_backfill_state','complete'),(semon_meta_scope('catalog_completeness'),'catalog_completeness','complete'),(semon_meta_scope('search_backfill_complete'),'search_backfill_complete','1'); DELETE FROM meta WHERE namespace=semon_meta_scope('catalog_recovery_reason') AND key='catalog_recovery_reason';").map_err(failed)?;
        let updated = tx
            .execute(
                "UPDATE storage_source_journal SET backfill_state='complete' WHERE namespace=?1",
                [self.config.namespace.as_str()],
            )
            .map_err(failed)?;
        if updated != 1 {
            return Err(StorageError::Unavailable);
        }
        insert_receipt(&tx, self.config.namespace.as_str(), operation)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(SourceMutationReceipt::Applied)
    }
}

const JOURNAL_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS storage_source_journal(namespace TEXT PRIMARY KEY,revision INTEGER NOT NULL CHECK(revision>=0),backfill_state TEXT CHECK(backfill_state IN ('discovering','draining','complete')),qualification INTEGER NOT NULL CHECK(qualification=1)) STRICT;";

/// Install the current journal once, inside namespace enrollment's transaction.
pub(crate) fn enroll(db: &Connection, config: &StoreConfig) -> Result<(), StorageError> {
    db.execute_batch(crate::events::store::SOURCE_OBSERVATION_RECEIPTS_SCHEMA)
        .map_err(failed)?;
    qualify_operation_tables(db)?;
    qualify_backfill_metadata(db)?;
    db.execute_batch(JOURNAL_SCHEMA).map_err(failed)?;
    catalog_namespace::guard(db, "storage_source_journal")?;
    db.execute("INSERT INTO storage_source_journal SELECT ?1,coalesce(max(revision),0),NULL,?2 FROM main.session_catalog_invalidations WHERE namespace=?1", params![config.namespace.as_str(),SOURCE_JOURNAL_QUALIFICATION_VERSION]).map_err(failed)?;
    Ok(())
}

fn diagnose_sql(phase: &'static str, error: rusqlite::Error) -> rusqlite::Error {
    #[cfg(test)]
    eprintln!(
        "source-journal phase={phase} sqlite_extended_code={:?}",
        error.sqlite_error().map(|code| code.extended_code)
    );
    #[cfg(not(test))]
    let _ = phase;
    error
}

fn phase_failed(phase: &'static str, error: rusqlite::Error) -> StorageError {
    failed(diagnose_sql(phase, error))
}

pub(crate) fn register_backfill_writer(db: &Connection, enabled: bool) -> Result<(), StorageError> {
    db.create_scalar_function(
        "semon_source_backfill_writer",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        move |_| Ok(i64::from(enabled)),
    )
    .map_err(failed)
}

/// The admitted metadata profile has no Catalog consumer
/// yet. Qualify only the four backfill completion keys for this typed producer;
/// all other metadata writes retain the admission fence. Native ownership's
/// independently installed metadata guards are never removed or rewritten.
fn qualify_backfill_metadata(db: &Connection) -> Result<(), StorageError> {
    let fences: Vec<String> = db.prepare("SELECT name FROM main.sqlite_schema WHERE type='trigger' AND tbl_name='meta' AND name GLOB 'storage_namespace_fence_*' ORDER BY name LIMIT 4").map_err(failed)?
        .query_map([], |row| row.get(0)).map_err(failed)?
        .collect::<rusqlite::Result<_>>().map_err(failed)?;
    if fences.is_empty() {
        return Ok(());
    }
    if fences.len() != 3 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    for name in fences {
        let (action, rows): (&str, &[&str]) = if name.ends_with("_INSERT") {
            ("INSERT", &["NEW"])
        } else if name.ends_with("_UPDATE") {
            ("UPDATE", &["OLD", "NEW"])
        } else if name.ends_with("_DELETE") {
            ("DELETE", &["OLD"])
        } else {
            return Err(StorageError::NamespaceUpgradePending);
        };
        let mut predicate = "semon_source_backfill_writer()<>1 OR (SELECT count(*) FROM storage_namespaces)<>1 OR NOT EXISTS(SELECT 1 FROM storage_namespaces WHERE namespace=semon_namespace())".to_owned();
        for row in rows {
            predicate.push_str(&format!(" OR {row}.key NOT IN ('catalog_backfill_state','catalog_completeness','search_backfill_complete','catalog_recovery_reason')"));
            predicate.push_str(&format!(
                " OR {row}.namespace IS NOT semon_meta_scope({row}.key)"
            ));
        }
        let quoted = name.replace('"', "\"\"");
        db.execute_batch(&format!("DROP TRIGGER \"{quoted}\"; CREATE TRIGGER \"{quoted}\" BEFORE {action} ON meta WHEN {predicate} BEGIN SELECT RAISE(ABORT,'source backfill metadata refused'); END;")).map_err(failed)?;
    }
    Ok(())
}

/// Admission fences every existing table, including the already namespaced
/// operation journal. Replace only these two families' unconditional fences
/// after admission/epoch validation, under the same preparation writer lock.
fn qualify_operation_tables(db: &Connection) -> Result<(), StorageError> {
    for table in ["source_observation_receipts", "source_observation_intents"] {
        // Install the scoped replacement before removing any admission fence.
        for (action, predicate) in [
            ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
            (
                "UPDATE",
                "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
            ),
            ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
        ] {
            db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS storage_source_guard_{table}_{action} BEFORE {action} ON {table} WHEN semon_event_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'source operation namespace refused'); END;")).map_err(failed)?;
        }
        let fences: Vec<String> = db.prepare("SELECT name FROM main.sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND name GLOB 'storage_namespace_fence_*' ORDER BY name LIMIT 4").map_err(failed)?
            .query_map([table], |row| row.get(0)).map_err(failed)?
            .collect::<rusqlite::Result<_>>().map_err(failed)?;
        if !fences.is_empty() && fences.len() != 3 {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for name in fences {
            db.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))
                .map_err(failed)?;
        }
        db.execute(
            "UPDATE storage_writer_inventory SET qualified_version=1 WHERE table_name=?1",
            [table],
        )
        .map_err(failed)?;
    }
    Ok(())
}

pub(crate) fn stored_receipt(
    db: &Connection,
    namespace: &str,
    operation: &SourceOperation,
) -> Result<Option<SourceMutationReceipt>, StorageError> {
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    let captured_digest: Option<String> = db.query_row(
        "SELECT payload_digest FROM main.source_observation_intents WHERE namespace=?1 AND operation_id=?2",
        params![namespace, operation.id], |row| row.get(0),
    ).optional().map_err(|error| phase_failed("intent-binding-read", error))?;
    if captured_digest
        .as_ref()
        .is_some_and(|digest| digest != &operation.payload_digest)
    {
        return Ok(Some(SourceMutationReceipt::Conflict));
    }
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    let found: Option<(String, String)> = db.query_row(
        "SELECT payload_digest,outcome FROM main.source_observation_receipts WHERE namespace=?1 AND operation_id=?2",
        params![namespace, operation.id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(|error| phase_failed("receipt-read", error))?;
    Ok(found.map(|(digest, outcome)| {
        if digest == operation.payload_digest && outcome == "applied" {
            SourceMutationReceipt::AlreadyApplied
        } else {
            SourceMutationReceipt::Conflict
        }
    }))
}

pub(crate) fn recovery_receipt(
    db: &Connection,
    namespace: &str,
    operation: &SourceOperation,
) -> Result<Option<(SourceMutationReceipt, bool)>, StorageError> {
    let Some(receipt) = stored_receipt(db, namespace, operation)? else {
        return Ok(None);
    };
    let done: Option<Option<bool>> = db.query_row(
        "SELECT recovery_done FROM main.source_observation_receipts WHERE namespace=?1 AND operation_id=?2 AND payload_digest=?3",
        params![namespace, operation.id, operation.payload_digest], |row| row.get(0),
    ).optional().map_err(failed)?;
    match done {
        Some(Some(done)) => Ok(Some((receipt, done))),
        // A rebound key is not entitled to either the old result or its ACK.
        None => Ok(Some((SourceMutationReceipt::Conflict, false))),
        Some(None) => Err(StorageError::Conflict),
    }
}

pub(crate) fn insert_recovery_receipt(
    db: &Connection,
    namespace: &str,
    operation: &SourceOperation,
    applied: bool,
    done: bool,
) -> Result<(), StorageError> {
    // Receipt rows are control records, not discovered source rows.
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    db.execute(
        "INSERT INTO main.source_observation_receipts(namespace,operation_id,payload_digest,outcome,recovery_done) VALUES(?1,?2,?3,?4,?5)",
        params![namespace, operation.id, operation.payload_digest, if applied { "applied" } else { "conflict" }, done],
    ).map_err(|error| phase_failed("recovery-receipt-insert", error))?;
    Ok(())
}

pub(crate) fn insert_receipt(
    tx: &Connection,
    namespace: &str,
    operation: &SourceOperation,
) -> Result<(), StorageError> {
    insert_terminal_receipt(tx, namespace, operation, "applied")
}

pub(crate) fn insert_terminal_receipt(
    tx: &Connection,
    namespace: &str,
    operation: &SourceOperation,
    outcome: &str,
) -> Result<(), StorageError> {
    crate::native_timing::discovery_count(|counts| {
        counts.statements += 1;
        counts.rows += 1;
    });
    tx.execute(
        "INSERT INTO main.source_observation_receipts(namespace,operation_id,payload_digest,outcome) VALUES(?1,?2,?3,?4)",
        params![namespace, operation.id, operation.payload_digest, outcome],
    ).map_err(|error| phase_failed("receipt-insert", error))?;
    Ok(())
}

pub(crate) fn persist_intent_sql(
    db: &Connection,
    namespace: &str,
    intent: &SourceIntent,
) -> Result<(), StorageError> {
    validate_intent(Some(namespace), intent)?;
    let operation = intent.operation();
    let payload = serde_json::to_string(intent).map_err(|_| StorageError::InvalidArguments)?;
    if payload.len() > 2 * 1024 * 1024 {
        return Err(StorageError::InvalidArguments);
    }
    let existing: Option<String> = db.query_row(
        "SELECT payload FROM source_observation_intents WHERE namespace=?1 AND operation_id=?2 AND octet_length(payload)<=2097152",
        params![namespace, operation.id], |row| row.get(0),
    ).optional().map_err(failed)?;
    if let Some(existing) = existing {
        return if existing == payload {
            Ok(())
        } else {
            Err(StorageError::Conflict)
        };
    }
    let count: i64 = db.query_row("SELECT count(*) FROM (SELECT 1 FROM source_observation_intents WHERE namespace=?1 LIMIT 256)", [namespace], |row| row.get(0)).map_err(failed)?;
    if count >= 256 {
        return Err(StorageError::Busy);
    }
    if matches!(
        stored_receipt(db, namespace, operation)?,
        Some(SourceMutationReceipt::Conflict)
    ) {
        let same_payload: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM source_observation_receipts WHERE namespace=?1 AND operation_id=?2 AND payload_digest=?3)", params![namespace, operation.id, operation.payload_digest], |row| row.get(0)).map_err(failed)?;
        if !same_payload {
            return Err(StorageError::Conflict);
        }
    }
    db.execute("INSERT INTO source_observation_intents(namespace,operation_id,payload_digest,payload) VALUES(?1,?2,?3,?4)", params![namespace, operation.id, operation.payload_digest, payload]).map_err(failed)?;
    Ok(())
}

pub(crate) fn load_intents(
    db: &Connection,
    namespace: &str,
) -> Result<Vec<SourceIntent>, StorageError> {
    let mut statement = db.prepare("SELECT operation_id,payload_digest,CASE WHEN octet_length(payload)<=2097152 THEN payload END FROM source_observation_intents WHERE namespace=?1 ORDER BY sequence LIMIT 257").map_err(failed)?;
    let rows = statement
        .query_map([namespace], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(failed)?;
    let mut intents = Vec::new();
    for row in rows {
        if intents.len() == 256 {
            return Err(StorageError::Unavailable);
        }
        let (id, digest, payload) = row.map_err(failed)?;
        let intent: SourceIntent = serde_json::from_str(&payload.ok_or(StorageError::Unavailable)?)
            .map_err(|_| StorageError::Unavailable)?;
        validate_intent(Some(namespace), &intent)?;
        if intent.operation().id != id || intent.operation().payload_digest != digest {
            return Err(StorageError::Conflict);
        }
        intents.push(intent);
    }
    Ok(intents)
}

pub(crate) fn acknowledge_intent_sql(
    db: &Connection,
    namespace: &str,
    operation: &SourceOperation,
) -> Result<(), StorageError> {
    if operation.namespace != namespace {
        return Err(StorageError::Conflict);
    }
    let terminal: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM source_observation_receipts WHERE namespace=?1 AND operation_id=?2 AND payload_digest=?3)", params![namespace, operation.id, operation.payload_digest], |row| row.get(0)).map_err(failed)?;
    if !terminal {
        return Err(StorageError::Conflict);
    }
    // A reused key with a different digest is never allowed to clear the old
    // pending intent, even though its public mutation result is Conflict.
    db.execute("DELETE FROM source_observation_intents WHERE namespace=?1 AND operation_id=?2 AND payload_digest=?3 AND EXISTS(SELECT 1 FROM source_observation_receipts r WHERE r.namespace=?1 AND r.operation_id=?2 AND r.payload_digest=?3)", params![namespace, operation.id, operation.payload_digest]).map_err(failed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EventCache, FileIndex, Ledger, Stat, StoreError};
    use crate::storage::StoreBackend;
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

    fn batch_db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE catalog_backfill_sources(namespace TEXT NOT NULL,source_path TEXT NOT NULL,PRIMARY KEY(namespace,source_path)); CREATE TABLE session_catalog_invalidations(namespace TEXT NOT NULL,source_path TEXT NOT NULL,revision INTEGER NOT NULL,reason TEXT NOT NULL,observed_at INTEGER NOT NULL,PRIMARY KEY(namespace,source_path)); CREATE TABLE storage_source_journal(namespace TEXT PRIMARY KEY,revision INTEGER NOT NULL,backfill_state TEXT); INSERT INTO storage_source_journal VALUES('A',0,'draining'),('B',99,'complete'); INSERT INTO session_catalog_invalidations VALUES('B','foreign',99,'retained',11);").unwrap();
        db
    }

    fn batch_rows(db: &Connection) -> Vec<(String, i64, String, i64)> {
        db.prepare("SELECT source_path,revision,reason,observed_at FROM session_catalog_invalidations WHERE namespace='A' ORDER BY source_path").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn discovery_batch_prepares_are_constant_and_claims_match_rowwise() {
        {
            let mut db = batch_db();
            let mut reference = batch_db();
            let namespace = "A";
            let paths: Vec<String> = (0..64).map(|i| format!("source-{i:02}")).collect();
            let mut work = JournalWork::default();
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            discover_prepared(&tx, namespace, &paths, true, || 42, &mut work).unwrap();
            tx.commit().unwrap();
            assert_eq!(work.prepares, 4);
            assert_eq!(work.executions, 193);
            assert_eq!(work.namespace_probes, 0);
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
            assert_eq!(batch_rows(&db), expected);
            assert_eq!(batch_rows(&db), batch_rows(&reference));
            assert_eq!(
                claims(&db, namespace, &paths).unwrap(),
                claims(&reference, namespace, &paths).unwrap()
            );
            // Duplicate discovery neither advances claims nor prepares the two
            // invalidation statements. Membership and state still run.
            let before = batch_rows(&db);
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
            assert_eq!(batch_rows(&db), before);
            {
                let foreign: (i64, String, i64) = db.query_row("SELECT revision,reason,observed_at FROM session_catalog_invalidations WHERE namespace='B'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
                assert_eq!(foreign, (99, "retained".into(), 11));
            }
        }
    }

    #[test]
    fn discovery_batch_empty_refusal_rollback_and_schema_aba() {
        let mut db = batch_db();
        let mut work = JournalWork::default();
        let before = db.total_changes();
        discover_prepared(&db, "A", &[], true, || 42, &mut work).unwrap();
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
                "A",
                &["first".into(), "fault".into()],
                true,
                || 42,
                &mut JournalWork::default()
            )
            .is_err()
        );
        tx.rollback().unwrap();
        assert!(batch_rows(&db).is_empty());
        let membership: i64 = db
            .query_row(
                "SELECT count(*) FROM catalog_backfill_sources WHERE namespace='A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(membership, 0);
        let revision: i64 = db
            .query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace='A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revision, 0);
        // Rolled-back DDL cannot retain prepared statements across a call.
        db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE rolled_back_probe(value TEXT); ROLLBACK;")
            .unwrap();
        discover_prepared(
            &db,
            "A",
            &["after-aba".into()],
            true,
            || 42,
            &mut JournalWork::default(),
        )
        .unwrap();
        assert_eq!(
            batch_rows(&db),
            [("after-aba".into(), 1, "changed".into(), 42)]
        );
    }

    #[test]
    fn discovery_batch_revision_overflow_rolls_back_membership() {
        let mut db = batch_db();
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
                "A",
                &["overflow".into()],
                true,
                || 42,
                &mut JournalWork::default()
            ),
            Err(rusqlite::Error::InvalidQuery)
        ));
        tx.rollback().unwrap();
        assert!(batch_rows(&db).is_empty());
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

    #[test]
    fn current_reopen_needs_no_writer_and_refuses_missing_profile_without_repair() {
        let fixture = Fixture::new();
        let owner =
            crate::events::store::configured::NativeOwnership::restore(&fixture.config).unwrap();
        let journal = SourceJournal::owned(owner);
        let blocker = fixture.db();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        journal.prepare().unwrap();
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::admit(&fixture.config).unwrap();
        drop(crate::storage::sqlite::semantic::Stage::open(capability).unwrap());
        blocker
            .execute_batch("ROLLBACK; DROP TABLE storage_semantic_profiles")
            .unwrap();
        let cookie: i64 = blocker
            .query_row("PRAGMA schema_version", [], |r| r.get(0))
            .unwrap();
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::admit(&fixture.config).unwrap();
        assert!(crate::storage::sqlite::semantic::Stage::open(capability).is_err());
        let after: i64 = blocker
            .query_row("PRAGMA schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cookie, after);
        let absent: bool = blocker.query_row("SELECT NOT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_semantic_profiles')", [], |r|r.get(0)).unwrap();
        assert!(absent);
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
            let config = StoreConfig {
                backend: StoreBackend::Sqlite {
                    database: EventCache::path(&cache_path),
                },
                namespace: LogicalSourceNamespace::new(namespace).unwrap(),
                expected_stream_epoch: Some("a".repeat(64)),
            };
            config.create_native_store().unwrap();
            crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
            let fixture = Self { root, config };
            // Seed through the admitted journal and acknowledge one source through
            // the configured consumer; both counters belong to this namespace.
            let seed_paths = vec!["seed".into(), "acknowledged".into()];
            let mut native = SourceJournal::owned(
                crate::events::store::configured::NativeOwnership::restore(&fixture.config)
                    .unwrap(),
            );
            native
                .discover(
                    &discover_operation(namespace, &seed_paths, true),
                    &seed_paths,
                    true,
                )
                .unwrap();
            drop(native);
            let source =
                crate::storage::HostSource::new(fixture.config.namespace.clone(), "a".repeat(64))
                    .unwrap();
            let mut consumer =
                EventCache::open_scoped(&fixture.root.join("index.json"), &source).unwrap();
            assert!(matches!(
                consumer
                    .acknowledge_source_for_test("acknowledged")
                    .unwrap(),
                crate::events::Outcome::Written
            ));
            drop(consumer);
            fixture
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

    /// Scratch cache initialized by the production producer open.
    struct CacheFixture {
        root: PathBuf,
        cache_path: PathBuf,
    }

    impl CacheFixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-source-observation-cache-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache_path = root.join("index.json");
            drop(EventCache::open_scoped(&cache_path, &crate::Options::default().source).unwrap());
            Self { root, cache_path }
        }
    }

    impl Drop for CacheFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// The fault occurs after the real backend transaction has committed its
    /// effect+receipt, before EventCache observes that terminal result.
    struct LoseDiscoverAck {
        inner: Box<dyn SourceObservationStore>,
        first: bool,
        block_replay: std::sync::Arc<std::sync::atomic::AtomicBool>,
        captured: std::sync::Arc<std::sync::Mutex<Option<SourceOperation>>>,
    }

    impl SourceObservationStore for LoseDiscoverAck {
        fn persist_intent(&mut self, intent: &SourceIntent) -> Result<(), StorageError> {
            self.inner.persist_intent(intent)
        }
        fn pending_intents(&self) -> Result<Vec<SourceIntent>, StorageError> {
            self.inner.pending_intents()
        }
        fn acknowledge_intent(&mut self, operation: &SourceOperation) -> Result<(), StorageError> {
            self.inner.acknowledge_intent(operation)
        }
        fn discover(
            &mut self,
            operation: &SourceOperation,
            paths: &[String],
            complete: bool,
        ) -> Result<SourceMutationReceipt, StorageError> {
            if self.block_replay.load(Ordering::Relaxed) {
                return Err(StorageError::Busy);
            }
            assert!(
                self.inner
                    .pending_intents()?
                    .iter()
                    .any(|intent| intent.operation() == operation)
            );
            let receipt = self.inner.discover(operation, paths, complete)?;
            if self.first && receipt == SourceMutationReceipt::Applied {
                self.first = false;
                *self.captured.lock().unwrap() = Some(operation.clone());
                return Err(StorageError::CommitUncertain);
            }
            Ok(receipt)
        }
        fn invalidate(
            &mut self,
            operation: &SourceOperation,
            expected: &[(String, Option<i64>)],
        ) -> Result<SourceMutationReceipt, StorageError> {
            self.inner.invalidate(operation, expected)
        }
        fn claims(
            &self,
            paths: &[String],
        ) -> Result<crate::storage::source_observations::SourceClaims, StorageError> {
            self.inner.claims(paths)
        }
        fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError> {
            self.inner.pending(limit)
        }
        fn backfill_state(&self) -> Result<Option<String>, StorageError> {
            self.inner.backfill_state()
        }
        fn finish_backfill(
            &mut self,
            operation: &SourceOperation,
            generation: Option<&str>,
        ) -> Result<SourceMutationReceipt, StorageError> {
            self.inner.finish_backfill(operation, generation)
        }
    }

    fn producer_restart_conformance(
        mut open_cache: impl FnMut() -> EventCache,
        mut open_store: impl FnMut() -> Box<dyn SourceObservationStore>,
        database: &std::path::Path,
        root: &std::path::Path,
        namespace: &str,
        restart: bool,
    ) {
        let options = crate::Options {
            claude_home: root.join("claude"),
            cache: root.join("index.json"),
            ..crate::Options::default()
        };
        let first = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/first.jsonl".into(),
        };
        let later = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/later.jsonl".into(),
        };
        let first_path = first.full_path(&options).to_string_lossy().into_owned();
        let later_path = later.full_path(&options).to_string_lossy().into_owned();
        // The ordinary fixture already committed its seed discovery. Track its
        // exact OpIDs separately from the two operations exercised below.
        let baseline = {
            let db = Connection::open(database).unwrap();
            let mut statement = db.prepare("SELECT operation_id FROM source_observation_receipts WHERE namespace=?1 AND outcome='applied' ORDER BY operation_id").unwrap();
            statement
                .query_map([namespace], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut cache = open_cache();
        let block_replay = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        cache.inject_source_observations(Box::new(LoseDiscoverAck {
            inner: open_store(),
            first: true,
            block_replay: block_replay.clone(),
            captured: captured.clone(),
        }));
        assert!(
            cache
                .discover_catalog_sources(&options, &[first], false)
                .is_err()
        );
        let operation = captured.lock().unwrap().clone().unwrap();
        let before = open_store()
            .claims(std::slice::from_ref(&first_path))
            .unwrap();
        assert!(before[0].1.is_some());
        let pending = open_store().pending_intents().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation(), &operation);
        // Reopening the backend proves the terminal receipt, but does not ACK
        // the producer intent. The actual EventCache replay below does that.
        assert_eq!(
            pending[0].apply(open_store().as_mut()).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
        assert_eq!(open_store().pending_intents().unwrap(), pending);
        block_replay.store(true, Ordering::Relaxed);
        assert!(
            cache
                .discover_catalog_sources(&options, std::slice::from_ref(&later), true)
                .is_err()
        );
        assert_eq!(open_store().pending_intents().unwrap(), pending);
        assert_eq!(
            open_store()
                .claims(std::slice::from_ref(&later_path))
                .unwrap(),
            vec![(later_path.clone(), None)]
        );
        block_replay.store(false, Ordering::Relaxed);
        if restart {
            drop(cache);
            cache = open_cache();
            assert!(open_store().pending_intents().unwrap().is_empty());
        }
        cache
            .discover_catalog_sources(&options, &[later], true)
            .unwrap();
        assert!(open_store().pending_intents().unwrap().is_empty());
        assert_eq!(
            open_store()
                .claims(std::slice::from_ref(&first_path))
                .unwrap(),
            before
        );
        assert!(
            open_store()
                .claims(std::slice::from_ref(&later_path))
                .unwrap()[0]
                .1
                .is_some()
        );
        let db = Connection::open(database).unwrap();
        let count: i64 = db.query_row("SELECT count(*) FROM source_observation_receipts WHERE namespace=?1 AND operation_id=?2 AND outcome='applied'", params![namespace, operation.id], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        let mut statement = db.prepare("SELECT operation_id FROM source_observation_receipts WHERE namespace=?1 AND outcome='applied' ORDER BY operation_id").unwrap();
        let applied = statement
            .query_map([namespace], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(baseline.iter().all(|id| applied.contains(id)));
        let added: Vec<_> = applied.iter().filter(|id| !baseline.contains(id)).collect();
        assert!(added.contains(&&operation.id));
        assert_eq!(
            added.len(),
            2,
            "one original operation plus the later operation"
        );
        let later_id = added
            .iter()
            .copied()
            .find(|id| id.as_str() != operation.id.as_str())
            .unwrap();
        let later_digest: String = db.query_row("SELECT payload_digest FROM source_observation_receipts WHERE namespace=?1 AND operation_id=?2", params![namespace, later_id], |row| row.get(0)).unwrap();
        assert_eq!(
            later_digest,
            discover_operation(namespace, std::slice::from_ref(&later_path), true).payload_digest
        );
        assert_eq!(
            stored_receipt(&db, namespace, &operation).unwrap(),
            Some(SourceMutationReceipt::AlreadyApplied)
        );
        // Foreign intent capture and dispatch both refuse before any effects.
        let foreign_paths = vec!["foreign.jsonl".into()];
        let foreign = SourceIntent::Discover {
            operation: discover_operation("foreign", &foreign_paths, true),
            paths: foreign_paths.clone(),
            complete: true,
        };
        let rows_before: i64 = db
            .query_row(
                "SELECT count(*) FROM source_observation_intents",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            open_store().persist_intent(&foreign),
            Err(StorageError::Conflict)
        );
        assert_eq!(
            foreign.apply(open_store().as_mut()).unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(
            open_store().claims(&foreign_paths).unwrap(),
            vec![(foreign_paths[0].clone(), None)]
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM source_observation_intents",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            rows_before
        );
    }

    #[test]
    fn event_cache_reconciles_real_committed_intents_before_later_work_and_after_restart() {
        for restart in [false, true] {
            let configured = Fixture::new_with_namespace("configured\0source");
            let database = configured.config.backend.sqlite_database().unwrap();
            producer_restart_conformance(
                || {
                    EventCache::open_scoped(
                        &configured.root.join("index.json"),
                        &crate::storage::HostSource::new(
                            configured.config.namespace.clone(),
                            "a".repeat(64),
                        )
                        .unwrap(),
                    )
                    .unwrap()
                },
                || {
                    Box::new(SourceJournal::owned(
                        crate::events::store::configured::NativeOwnership::restore(
                            &configured.config,
                        )
                        .unwrap(),
                    )) as Box<dyn SourceObservationStore>
                },
                database,
                &configured.root,
                "configured\0source",
                restart,
            );
        }
    }

    #[test]
    fn finish_backfill_uses_a_real_published_generation() {
        let fixture = CacheFixture::new();
        let mut cache =
            EventCache::open_scoped(&fixture.cache_path, &crate::Options::default().source)
                .unwrap();
        let generation = cache
            .publish_catalog_projection_fenced(
                &crate::slot_projection::Publication {
                    rows: &[],
                    transcripts: None,
                },
                None,
            )
            .unwrap()
            .expect("real publisher generation");
        drop(cache);
        let config = crate::Options::default()
            .source
            .config(&fixture.cache_path)
            .unwrap();
        crate::events::store::configured::NativeOwnership::restore(&config).unwrap();
        let open = || {
            Box::new(SourceJournal::owned(
                crate::events::store::configured::NativeOwnership::restore(&config).unwrap(),
            )) as Box<dyn SourceObservationStore>
        };
        let namespace = config.namespace.as_str();
        let mut store = open();
        let discovery = discover_operation(namespace, &[], true);
        store.discover(&discovery, &[], true).unwrap();
        let wrong = finish_operation(namespace, Some("wrong-generation"));
        assert_eq!(
            store
                .finish_backfill(&wrong, Some("wrong-generation"))
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.backfill_state().unwrap().as_deref(), Some("draining"));
        let operation = finish_operation(namespace, Some(&generation));
        let intent = SourceIntent::FinishBackfill {
            operation: operation.clone(),
            expected_generation: Some(generation),
        };
        store.persist_intent(&intent).unwrap();
        assert_eq!(
            intent.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Applied
        );
        drop(store);
        let mut store = open();
        assert_eq!(
            intent.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
        store.acknowledge_intent(&operation).unwrap();
        assert_eq!(store.backfill_state().unwrap().as_deref(), Some("complete"));
        assert!(store.pending_intents().unwrap().is_empty());
        let journal = SourceJournal(config.clone());
        let db = journal.connection().unwrap();
        journal.validate(&db).unwrap();
        // The backfill-specific guard does not qualify other metadata.
        assert!(
            db.execute(
                "INSERT INTO meta(key,value) VALUES('native_forbidden','x')",
                []
            )
            .is_err()
        );
        assert!(
            db.execute(
                "INSERT INTO meta(key,value) VALUES('catalog_backfill_state','draining')",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn durable_source_queue_enforces_the_256_intent_admission_bound() {
        let fixture = Fixture::new();
        let mut store = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&fixture.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
        for index in 0..256 {
            let paths = vec![format!("source-{index}")];
            let operation = discover_operation("A", &paths, false);
            store
                .persist_intent(&SourceIntent::Discover {
                    operation,
                    paths,
                    complete: false,
                })
                .unwrap();
        }
        let before = store.pending_intents().unwrap();
        assert_eq!(before.len(), 256);
        let paths = vec!["overflow".into()];
        let operation = discover_operation("A", &paths, false);
        assert_eq!(
            store.persist_intent(&SourceIntent::Discover {
                operation,
                paths,
                complete: false
            }),
            Err(StorageError::Busy)
        );
        assert_eq!(store.pending_intents().unwrap(), before);
        assert_eq!(
            store.claims(&["overflow".into()]).unwrap(),
            vec![("overflow".into(), None)]
        );
    }

    #[test]
    fn durable_intent_capture_refuses_rebinding_and_premature_ack() {
        let fixture = Fixture::new();
        let mut store = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&fixture.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
        let paths = vec!["first".into()];
        let operation = discover_operation("A", &paths, true);
        let intent = SourceIntent::Discover {
            operation: operation.clone(),
            paths,
            complete: true,
        };
        store.persist_intent(&intent).unwrap();
        assert_eq!(
            store.acknowledge_intent(&operation),
            Err(StorageError::Conflict)
        );
        let changed_paths = vec!["changed".into()];
        let mut changed_operation = discover_operation("A", &changed_paths, true);
        changed_operation.id = operation.id.clone();
        let changed = SourceIntent::Discover {
            operation: changed_operation.clone(),
            paths: changed_paths,
            complete: true,
        };
        assert_eq!(store.persist_intent(&changed), Err(StorageError::Conflict));
        assert_eq!(
            changed.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(
            store.claims(&["changed".into()]).unwrap(),
            vec![("changed".into(), None)]
        );
        assert_eq!(store.pending_intents().unwrap(), vec![intent.clone()]);
        assert_eq!(
            intent.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Applied
        );
        assert_eq!(
            store.acknowledge_intent(&changed_operation),
            Err(StorageError::Conflict)
        );
        store.acknowledge_intent(&operation).unwrap();
        assert!(store.pending_intents().unwrap().is_empty());
    }

    #[test]
    fn shared_observation_conformance_runs_on_configured_sqlite() {
        let configured = Fixture::new_with_namespace("configured\0source");
        crate::storage::source_observations::conformance::shared_source_observation_conformance(
            || {
                Box::new(SourceJournal::owned(
                    crate::events::store::configured::NativeOwnership::restore(&configured.config)
                        .unwrap(),
                )) as Box<dyn SourceObservationStore>
            },
            "configured\0source",
        );
    }

    #[test]
    fn configured_discovery_claim_cas_restart_and_originals() {
        let f = Fixture::new();
        let mut store = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
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
        let selected_plan: Vec<String> = plan_db
            .prepare(&format!("EXPLAIN QUERY PLAN {SELECTED_PENDING_SQL}"))
            .unwrap()
            .query_map(params!["A", 1], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            selected_plan
                .iter()
                .all(|line| !line.contains("TEMP B-TREE"))
        );
        assert!(selected_plan.iter().all(|line| !line.starts_with("SCAN ")));
        // Repeated discovery is idempotent, including its source claims.
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
        assert_eq!(store.claims(&paths).unwrap(), captured);
        assert_eq!(store.pending(1).unwrap(), vec!["seed".to_owned()]);
        let mut concurrent = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
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
        let restarted = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
        assert_eq!(restarted.claims(&paths).unwrap(), before_conflict);
        let db = f.db();
        assert_eq!(db.query_row("SELECT revision FROM session_catalog_invalidations WHERE namespace='A' AND source_path='seed'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert!(
            db.execute(
                "DELETE FROM session_catalog_invalidations WHERE namespace='A'",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn configured_discovery_rolls_back_mid_batch_and_keeps_foreign_rows_invisible() {
        let f = Fixture::new();
        let owner = crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap();
        let mut store = SourceJournal::owned(owner);
        let mut foreign_config = f.config.clone();
        foreign_config.namespace = LogicalSourceNamespace::new("B").unwrap();
        let foreign_owner =
            crate::events::store::configured::NativeOwnership::admit(&foreign_config).unwrap();
        let mut foreign = SourceJournal::owned(foreign_owner);
        foreign.prepare().unwrap();
        let foreign_paths = vec!["foreign".to_owned()];
        foreign
            .discover(
                &discover_operation("B", &foreign_paths, true),
                &foreign_paths,
                true,
            )
            .unwrap();
        let foreign_before = foreign.claims(&foreign_paths).unwrap();
        let db = f.db();
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
        assert_eq!(foreign.claims(&foreign_paths).unwrap(), foreign_before);
    }

    #[test]
    fn configured_source_writer_epoch_bounds_and_unported_families_refuse() {
        let f = Fixture::new();
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
        let mut store = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
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
        let paths = vec!["after-refusal".into()];
        let operation = SourceOperation::capture(
            f.config.namespace.as_str(),
            "discover",
            &serde_json::to_vec(&(&paths, true)).unwrap(),
        );
        let intent = SourceIntent::Discover {
            operation: operation.clone(),
            paths: paths.clone(),
            complete: true,
        };
        let observations = index
            .source_observations_mut()
            .expect("configured source observation store");
        observations.persist_intent(&intent).unwrap();
        assert_eq!(
            observations.discover(&operation, &paths, true).unwrap(),
            SourceMutationReceipt::Applied
        );
        observations.acknowledge_intent(&operation).unwrap();
        assert_eq!(
            f.config
                .source_observations()
                .unwrap()
                .claims(&["after-refusal".into()])
                .unwrap(),
            vec![("after-refusal".into(), Some(3))]
        );
        assert!(
            EventCache::open_scoped(
                &f.root.join("index.json"),
                &crate::Options::default().source
            )
            .is_err()
        );
        assert_eq!(
            store.claims(&["seed".into()]).unwrap(),
            vec![("seed".into(), Some(1))]
        );
    }

    #[test]
    fn configured_source_writer_revalidates_live_epoch_and_reports_busy() {
        let f = Fixture::new();
        let mut store = Box::new(SourceJournal::owned(
            crate::events::store::configured::NativeOwnership::restore(&f.config).unwrap(),
        )) as Box<dyn SourceObservationStore>;
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
