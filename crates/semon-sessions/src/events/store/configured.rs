//! Configured singleton-native transaction. The global/multi-source and
//! Catalog publisher gates remain closed. Parser/identity logic stays in events.
use super::*;
use crate::storage::sqlite::source_journal::SourceJournal;
use crate::storage::{StorageError, StoreConfig, native_events::EventReceipt};
use rusqlite::functions::FunctionFlags;

// These tables are owned by the sole admitted namespace, not independently
// multiplexed. A second namespace must still be refused before any operation.
const TABLES: &[&str] = &[
    "files",
    "events",
    "signals",
    "usage_identity",
    "usage",
    "codex_usage",
    "meta",
    "catalog_relocation_candidates",
    "catalog_native_dependencies",
    "session_source_mutations",
    "native_usage_sources",
    "native_usage_current",
    "native_usage_facts",
    "native_usage_identities",
    "native_usage_dimensions",
    "native_tree_repairs",
    "native_tree_generations",
    "native_tree_usage_work",
    "native_tree_codex_work",
    "native_tree_headers",
    "native_tree_models",
    "native_tree_labels",
    "native_tree_tools",
    "native_tree_usage_models",
    "native_tree_copied_identities",
];

type CommitFile<'a> = (
    &'a str,
    Option<&'a Ledger>,
    Option<&'a super::super::DirtyRows>,
    &'a Ledger,
    &'a FileIndex,
);

type CompletionRow = (String, String, Option<i64>, Option<i64>, String);

#[cfg(test)]
thread_local! {
    static FAIL_EFFECT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn effect(name: &'static str) -> rusqlite::Result<()> {
    FAIL_EFFECT.with(|failure| {
        if failure.get() == Some(name) {
            failure.set(None);
            Err(rusqlite::Error::InvalidQuery)
        } else {
            Ok(())
        }
    })
}

fn failed(error: rusqlite::Error) -> StorageError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => StorageError::Busy,
        _ => StorageError::Unavailable,
    }
}

pub(super) fn connection(config: &StoreConfig) -> Result<Connection, StorageError> {
    let db = SourceJournal(config.clone()).connection()?;
    db.create_scalar_function(
        "semon_event_writer",
        0,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failed)?;
    Ok(db)
}

pub(super) fn validate(db: &Connection, config: &StoreConfig) -> Result<(), StorageError> {
    SourceJournal(config.clone()).validate(db)?;
    let ready: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_event_operations' AND type='table')", [], |r| r.get(0)).map_err(failed)?;
    if !ready {
        return Err(StorageError::NamespaceUpgradePending);
    }
    // Earlier pre-release singleton stores lack request binding. Never adopt
    // their unbound operation IDs as completions of the current contract.
    let bound: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('storage_event_operations') WHERE name='request_digest' AND type='TEXT' AND \"notnull\"=1)", [], |r| r.get(0)).map_err(failed)?;
    if !bound {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM storage_event_tables WHERE namespace=?1 AND qualification=1",
            [config.namespace.as_str()],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if count != TABLES.len() as i64 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    Ok(())
}

/// Constant-size schema preparation, no Raw access or native row copy/reset.
/// ALTER's constant default binds existing metadata keys to the admitted source.
/// Native table ownership is deliberately singleton until full row-key porting.
pub(crate) fn prepare(config: &StoreConfig) -> Result<(), StorageError> {
    SourceJournal(config.clone()).prepare()?;
    let mut db = connection(config)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    SourceJournal(config.clone()).validate(&tx)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_event_operations')",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if exists {
        return validate(&tx, config);
    }
    tx.execute_batch("CREATE TABLE storage_event_tables(namespace TEXT NOT NULL,table_name TEXT NOT NULL,qualification INTEGER NOT NULL CHECK(qualification=1),PRIMARY KEY(namespace,table_name)) STRICT, WITHOUT ROWID;
CREATE TABLE storage_event_operations(namespace TEXT NOT NULL,operation_id TEXT NOT NULL,source_path TEXT NOT NULL,request_digest TEXT NOT NULL CHECK(length(request_digest)=64),base TEXT NOT NULL,observed_revision INTEGER,target TEXT,consumed_bytes INTEGER,state TEXT NOT NULL CHECK(state IN('intent','complete')),PRIMARY KEY(namespace,operation_id)) STRICT, WITHOUT ROWID;
CREATE INDEX storage_event_reconcile ON storage_event_operations(namespace,source_path,base,observed_revision,state);").map_err(failed)?;
    let ns = config.namespace.as_str().replace('\'', "''");
    tx.execute_batch(&format!(
        "ALTER TABLE meta ADD COLUMN namespace TEXT NOT NULL DEFAULT '{ns}';"
    ))
    .map_err(failed)?;
    for table in ["storage_event_tables", "storage_event_operations"] {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let predicate = match operation {
                "UPDATE" => {
                    "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()"
                }
                "DELETE" => "OLD.namespace IS NOT semon_namespace()",
                _ => "NEW.namespace IS NOT semon_namespace()",
            };
            tx.execute_batch(&format!("CREATE TRIGGER storage_event_scope_{table}_{operation} BEFORE {operation} ON {table} WHEN semon_event_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'event operation namespace refused'); END;")).map_err(failed)?;
        }
    }
    for table in TABLES {
        let fences: Vec<String> = tx.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND name GLOB 'storage_namespace_fence_*' ORDER BY name LIMIT 4").map_err(failed)?
            .query_map([table], |r| r.get(0)).map_err(failed)?.collect::<rusqlite::Result<_>>().map_err(failed)?;
        if fences.len() != 3 {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for name in fences {
            tx.execute_batch(&format!("DROP TRIGGER \"{}\";", name.replace('"', "\"\"")))
                .map_err(failed)?;
        }
        tx.execute(
            "INSERT INTO storage_event_tables VALUES(?1,?2,1)",
            params![config.namespace.as_str(), table],
        )
        .map_err(failed)?;
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let record = if operation == "DELETE" { "OLD" } else { "NEW" };
            let metadata = if *table == "meta" {
                // Native helpers may change source/proof metadata only. Catalog
                // epochs, publication markers and custody records stay fenced.
                format!(
                    " OR {record}.namespace IS NOT semon_namespace() OR NOT ({record}.key GLOB 'native_*' OR {record}.key GLOB 'archive_event_index:*')"
                )
            } else {
                String::new()
            };
            tx.execute_batch(&format!("CREATE TRIGGER storage_event_guard_{table}_{operation} BEFORE {operation} ON \"{table}\" WHEN semon_event_writer()<>1 OR (SELECT count(*) FROM storage_namespaces)<>1 OR NOT EXISTS(SELECT 1 FROM storage_event_tables WHERE namespace=semon_namespace() AND table_name='{table}' AND qualification=1){metadata} BEGIN SELECT RAISE(ABORT,'configured event writer refused'); END;")).map_err(failed)?;
        }
    }
    tx.commit().map_err(|_| StorageError::CommitUncertain)
}

#[derive(Clone)]
struct Intent {
    id: String,
    base: String,
    revision: Option<i64>,
}

pub(crate) struct Writer {
    config: StoreConfig,
    intents: BTreeMap<String, Intent>,
}

impl Writer {
    pub(crate) fn new(config: &StoreConfig) -> Self {
        Self {
            config: config.clone(),
            intents: BTreeMap::new(),
        }
    }

    fn request_digest(&self, path: &str, harness: &str) -> Result<String, StorageError> {
        if path.is_empty()
            || path.len() > 4096
            || !matches!(harness, "claude" | "codex" | "copilot")
        {
            return Err(StorageError::InvalidArguments);
        }
        let mut digest = Sha256::new();
        digest.update(b"semon-native-event-request-v1");
        digest.update(CACHE_VERSION.to_be_bytes());
        for input in [self.config.namespace.as_str(), path, harness] {
            digest.update((input.len() as u64).to_be_bytes());
            digest.update(input.as_bytes());
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    fn snapshot(&self) -> Result<Connection, StorageError> {
        let db = connection(&self.config)?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        validate(&db, &self.config)?;
        Ok(db)
    }

    pub(crate) fn ledger(&self, path: &str) -> Result<Option<Ledger>, StorageError> {
        let db = self.snapshot()?;
        ledger_row(&db, path)
            .map(|row| row.map(|(_, ledger)| ledger))
            .map_err(failed)
    }

    pub(crate) fn load(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StorageError> {
        // read_one starts a transaction, so use the snapshot's already pinned
        // connection directly with the same native row decoder.
        let db = self.snapshot()?;
        let mut readers = Readers::new(&db).map_err(failed)?;
        let mut statement = db
            .prepare(&format!("SELECT {FILE_COLUMNS} FROM files WHERE path=?1"))
            .map_err(failed)?;
        let mut rows = statement.query([path]).map_err(failed)?;
        let mut found = rows
            .next()
            .map_err(failed)?
            .map(|row| read_file(&mut readers, row).map(|(_, ledger, index)| (ledger, index)))
            .transpose()
            .map_err(failed)?;
        if let Some((_, index)) = &mut found {
            self.bind_tree(index.tree_summary.as_mut());
        }
        Ok(found)
    }

    pub(crate) fn tree(
        &self,
        path: &str,
        stat: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StorageError> {
        let db = self.snapshot()?;
        let Some((_, ledger)) = ledger_row(&db, path).map_err(failed)? else {
            return Ok(None);
        };
        if ledger.stat != *stat {
            return Ok(None);
        }
        let mut statement = db.prepare(NATIVE_METADATA_VALUE).map_err(failed)?;
        let Some(mut summary) =
            read_tree_summary(&db, &mut statement, path, &ledger).map_err(failed)?
        else {
            return Ok(None);
        };
        self.bind_tree(Some(&mut summary));
        if !chunk_proofs::header_ready(&db, path, &ledger).map_err(failed)? {
            return Ok(None);
        }
        let Some(marker) = read_parent_marker(&mut statement, path, &ledger).map_err(failed)?
        else {
            return Ok(None);
        };
        let Some(calls) = launch_calls::read(&db, path, &ledger).map_err(failed)? else {
            return Ok(None);
        };
        Ok(Some((
            ledger,
            crate::native_tree::Metadata {
                summary,
                chunks: None,
                marker,
                calls: std::sync::Arc::new(calls),
            },
        )))
    }

    fn bind_tree(&self, summary: Option<&mut crate::native_tree::Summary>) {
        if let Some(snapshot) = summary.and_then(|summary| summary.stored.as_mut()) {
            snapshot.bind_configured(self.config.clone());
        }
    }

    pub(crate) fn seen(
        &self,
        path: &str,
        ledger: &Ledger,
        identity: &str,
    ) -> Result<native_usage::Seen, StorageError> {
        native_usage::seen(&self.snapshot()?, path, ledger, identity).map_err(failed)
    }

    pub(crate) fn begin(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        operation_id: &str,
        harness: &str,
    ) -> Result<(), StorageError> {
        let request_digest = self.request_digest(path, harness)?;
        if path.is_empty()
            || path.len() > 4096
            || operation_id.len() != 64
            || !operation_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(StorageError::InvalidArguments);
        }
        let mut db = connection(&self.config)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        validate(&tx, &self.config)?;
        if ledger_row(&tx, path)
            .map_err(failed)?
            .as_ref()
            .map(|(_, l)| l)
            != expected
        {
            return Err(StorageError::Conflict);
        }
        let revision = crate::storage::sqlite::source_journal::claims(
            &tx,
            Some(self.config.namespace.as_str()),
            &[path.to_owned()],
        )
        .map_err(failed)?[0]
            .1;
        let base = expected.map(header_ledger).unwrap_or_default();
        let prior: Option<(String, String, Option<i64>, String)> = tx.query_row("SELECT source_path,base,observed_revision,request_digest FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2", params![self.config.namespace.as_str(),operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failed)?;
        if let Some((old_path, old_base, old_revision, old_request)) = prior {
            if old_path != path
                || old_base != base
                || old_revision != revision
                || old_request != request_digest
            {
                return Err(StorageError::Conflict);
            }
        } else {
            tx.execute("INSERT INTO storage_event_operations(namespace,operation_id,source_path,request_digest,base,observed_revision,state) VALUES(?1,?2,?3,?4,?5,?6,'intent')", params![self.config.namespace.as_str(),operation_id,path,request_digest,base,revision]).map_err(failed)?;
        }
        let id = operation_id.to_owned();
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        self.intents
            .insert(path.to_owned(), Intent { id, base, revision });
        Ok(())
    }

    pub(crate) fn commit(&mut self, files: &[CommitFile<'_>]) -> Result<Outcome, StorageError> {
        if files.is_empty()
            || files.len() > 64
            || files
                .iter()
                .map(|f| f.0)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != files.len()
        {
            return Err(StorageError::InvalidArguments);
        }
        // Establish explicit admission before any Raw access, then release the
        // read snapshot. The writer transaction validates again below.
        drop(self.snapshot()?);
        // Raw checks precede the DB reservation. The parser qualifies complete
        // records and proofs on its open handle; no Raw I/O runs in the DB tx.
        for (path, _, _, ledger, _) in files {
            if Stat::of(Path::new(path)).ok().as_ref() != Some(&ledger.stat) {
                return Err(StorageError::Conflict);
            }
        }
        let mut db = connection(&self.config)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        validate(&tx, &self.config)?;
        let mut work = Vec::new();
        for (path, expected, _, ledger, _) in files {
            let intent = self
                .intents
                .get(*path)
                .ok_or(StorageError::InvalidArguments)?;
            if intent.base != expected.map(header_ledger).unwrap_or_default() {
                return Err(StorageError::Conflict);
            }
            let (state, target): (String, Option<String>) = tx.query_row("SELECT state,target FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2 AND source_path=?3 AND base=?4 AND observed_revision IS ?5", params![self.config.namespace.as_str(),intent.id,path,intent.base,intent.revision], |r| Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
            if state == "complete" {
                let target_ledger = header_ledger(ledger);
                if target.as_deref() != Some(target_ledger.as_str()) {
                    return Err(StorageError::Conflict);
                }
                work.push(false);
                continue;
            }
            if ledger_row(&tx, path)
                .map_err(failed)?
                .as_ref()
                .map(|(_, l)| l)
                != *expected
            {
                return Err(StorageError::Conflict);
            }
            let revision = crate::storage::sqlite::source_journal::claims(
                &tx,
                Some(self.config.namespace.as_str()),
                &[(*path).to_owned()],
            )
            .map_err(failed)?[0]
                .1;
            if revision != intent.revision {
                return Err(StorageError::Conflict);
            }
            work.push(true);
        }
        for ((path, expected, changes, ledger, index), write) in files.iter().zip(work) {
            if !write {
                continue;
            }
            if changes.is_none()
                && let Some((id, _)) = ledger_row(&tx, path).map_err(failed)?
            {
                clear(&tx, id).map_err(failed)?;
            }
            write_index_changes_measured(
                &tx,
                path,
                ledger,
                index,
                *changes,
                *expected,
                &mut crate::native_timing::Measurements::default(),
                Some(self.config.namespace.as_str()),
            )
            .map_err(failed)?;
            let intent = &self.intents[*path];
            tx.execute("UPDATE storage_event_operations SET target=?3,consumed_bytes=?4,state='complete' WHERE namespace=?1 AND operation_id=?2 AND state='intent'", params![self.config.namespace.as_str(),intent.id,header_ledger(ledger),int(ledger.offset)]).map_err(failed)?;
        }
        #[cfg(test)]
        effect("completion").map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        #[cfg(test)]
        if effect("commit_reply").is_err() {
            return Err(StorageError::CommitUncertain);
        }
        Ok(Outcome::Written)
    }

    pub(crate) fn reconcile(
        &self,
        operation_id: &str,
        path: &str,
        harness: &str,
    ) -> Result<Option<EventReceipt>, StorageError> {
        let request_digest = self.request_digest(path, harness)?;
        if operation_id.len() != 64 || !operation_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidArguments);
        }
        let db = self.snapshot()?;
        let row: Option<CompletionRow> = db.query_row("SELECT source_path,state,consumed_bytes,observed_revision,request_digest FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2", params![self.config.namespace.as_str(),operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(failed)?;
        let Some((source, state, consumed, revision, stored_request)) = row else {
            return Ok(None);
        };
        if source != path || stored_request != request_digest {
            return Err(StorageError::Conflict);
        }
        if state != "complete" {
            return Ok(None);
        }
        Ok(Some(EventReceipt {
            operation_id: operation_id.to_owned(),
            consumed_bytes: uint(consumed.ok_or(StorageError::Unavailable)?),
            observed_revision: revision,
            replayed: true,
        }))
    }

    pub(crate) fn receipt(&self, path: &str) -> Result<EventReceipt, StorageError> {
        let intent = self.intents.get(path).ok_or(StorageError::Unavailable)?;
        let db = self.snapshot()?;
        let consumed: i64 = db.query_row("SELECT consumed_bytes FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2 AND state='complete'", params![self.config.namespace.as_str(),intent.id], |r| r.get(0)).map_err(failed)?;
        Ok(EventReceipt {
            operation_id: intent.id.clone(),
            consumed_bytes: uint(consumed),
            observed_revision: intent.revision,
            replayed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventCache;
    use crate::storage::{LogicalSourceNamespace, StoreBackend};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
        raw: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-configured-event-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache = root.join("index.json");
            drop(EventCache::open_scoped(&cache).unwrap());
            let database = EventCache::path(&cache);
            let db = Connection::open(&database).unwrap();
            db.execute(
                "INSERT INTO meta(key,value) VALUES('original-custody','preserve-verbatim')",
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
                namespace: LogicalSourceNamespace::new("configured-events").unwrap(),
                expected_stream_epoch: Some(epoch),
            };
            config.admit_namespace().unwrap();
            for _ in 0..128 {
                if config.assign_catalog_namespace(64).unwrap().complete {
                    break;
                }
            }
            let raw = root.join("native.jsonl");
            fs::write(&raw, concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"native-session\",\"cwd\":\"/sanitized\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"sanitized fixture\"}]}}\n"
            )).unwrap();
            Self { root, config, raw }
        }
        fn db(&self) -> Connection {
            let StoreBackend::Sqlite { database } = &self.config.backend;
            Connection::open(database).unwrap()
        }
        fn state(&self) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
            let db = self.db();
            TABLES
                .iter()
                .copied()
                .chain(["storage_source_journal", "session_catalog_invalidations"])
                .map(|table| {
                    let mut statement = db
                        .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                        .unwrap();
                    let columns = statement.column_count();
                    let rows = statement
                        .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
                        .unwrap()
                        .collect::<rusqlite::Result<_>>()
                        .unwrap();
                    (table.to_owned(), rows)
                })
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn configured_ingest_restart_uncertain_reply_and_operation_identity() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        FAIL_EFFECT.with(|failure| failure.set(Some("commit_reply")));
        assert_eq!(
            writer.ingest(&"a".repeat(64), &f.raw, "codex"),
            Err(StorageError::CommitUncertain)
        );
        let state = f.state();
        assert_eq!(state["files"].len(), 1);
        assert!(!state["events"].is_empty());
        assert!(!state["meta"].is_empty());
        assert!(!state["catalog_native_dependencies"].is_empty());
        drop(writer);
        // Reconciliation does no Raw I/O and applies no effects a second time.
        fs::remove_file(&f.raw).unwrap();
        let mut restarted = f.config.native_events().unwrap();
        let receipt = restarted.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(receipt.consumed_bytes > 0);
        assert!(receipt.replayed);
        assert_eq!(
            restarted.ingest(&"a".repeat(64), &f.raw, "claude"),
            Err(StorageError::Conflict)
        );
        assert_eq!(state, f.state());
        assert_eq!(
            restarted.ingest(&"a".repeat(64), &f.root.join("other"), "codex"),
            Err(StorageError::Conflict)
        );
        assert!(
            !current(&f.db()).unwrap(),
            "Catalog/unbound writer fence opened"
        );
        let qualified: i64 = f
            .db()
            .query_row(
                "SELECT writer_qualification FROM storage_namespaces",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(qualified, 0);
        let mut foreign = f.config.clone();
        foreign.namespace = LogicalSourceNamespace::new("foreign").unwrap();
        assert!(matches!(
            foreign.native_events(),
            Err(StorageError::NamespaceMismatch)
        ));
        let mut stale = f.config.clone();
        stale.expected_stream_epoch = Some("0".repeat(64));
        assert!(matches!(
            stale.native_events(),
            Err(StorageError::ScopeChanged)
        ));
        assert!(
            f.db()
                .execute("UPDATE files SET title='unbound'", [])
                .is_err()
        );
        assert!(
            connection(&f.config)
                .unwrap()
                .execute(
                    "UPDATE meta SET value='forged' WHERE key='catalog_changes_epoch'",
                    []
                )
                .is_err()
        );
        let custody: String = f
            .db()
            .query_row(
                "SELECT value FROM meta WHERE key='original-custody'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(custody, "preserve-verbatim");
        // Already-open configured writers also reject a replaced epoch.
        f.db().execute_batch("DROP TRIGGER storage_event_guard_meta_UPDATE; UPDATE meta SET value='0000000000000000000000000000000000000000000000000000000000000000' WHERE key='catalog_changes_epoch';").unwrap();
        assert_eq!(
            restarted.ingest(&"a".repeat(64), &f.raw, "codex"),
            Err(StorageError::ScopeChanged)
        );
    }

    #[test]
    fn configured_replay_after_append_returns_original_receipt_and_fresh_id_advances() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        let first = writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(!first.replayed);
        let before = f.state();
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(&f.raw).unwrap().write_all(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"new work\"}]}}\n").unwrap();
        drop(writer);
        let mut writer = f.config.native_events().unwrap();
        let replay = writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.consumed_bytes, first.consumed_bytes);
        assert_eq!(replay.observed_revision, first.observed_revision);
        assert_eq!(before, f.state());
        assert_eq!(
            writer.ingest(&"a".repeat(64), &f.raw, "claude"),
            Err(StorageError::Conflict)
        );
        let fresh = writer.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert!(!fresh.replayed);
        assert!(fresh.consumed_bytes > replay.consumed_bytes);
        assert!(f.state()["events"].len() > before["events"].len());
        assert_eq!(
            writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap(),
            replay
        );
    }

    #[test]
    fn configured_same_handle_recovers_busy_and_other_operation_refusals() {
        use crate::storage::native_events::NativeEventStore;
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let mut cache = EventCache::open_configured(&f.config).unwrap();
        let lock = f.db();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            cache.ingest(&"a".repeat(64), &f.raw, "codex"),
            Err(StorageError::Busy)
        );
        lock.execute_batch("ROLLBACK").unwrap();
        let receipt = cache.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(!receipt.replayed);
        let error = cache
            .retain(&std::collections::BTreeSet::new(), &mut false)
            .unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<StorageError>()),
            Some(&StorageError::NamespaceUpgradePending)
        );
        let replay = cache.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.consumed_bytes, receipt.consumed_bytes);
        let stamp = ReportedFileStamp {
            dev: 0,
            ino: 0,
            size: 0,
            modified_ns: 0,
        };
        let error = cache.save_runs(&[], &stamp).unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<StorageError>()),
            Some(&StorageError::NamespaceUpgradePending)
        );
        assert!(
            cache
                .ingest(&"a".repeat(64), &f.raw, "codex")
                .unwrap()
                .replayed
        );
    }

    #[test]
    fn configured_each_effect_rolls_back_rows_proofs_revision_and_completion() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(&f.raw).unwrap().write_all(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"sanitized reply\"}]}}\n").unwrap();
        let before = f.state();
        for (i, phase) in [
            "relocation",
            "proof_header",
            "proof_sidecar",
            "proof_chunks",
            "proof_parent",
            "proof_launch",
            "file_upsert",
            "proof_tree",
            "events",
            "signals",
            "usage",
            "codex_usage",
            "native_usage",
            "dependencies",
            "semantic_dependencies",
            "archive_metadata",
            "invalidation",
            "mutation_retirement",
            "completion",
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("{:064x}", i + 1);
            FAIL_EFFECT.with(|failure| failure.set(Some(phase)));
            assert_eq!(
                writer.ingest(&id, &f.raw, "codex"),
                Err(StorageError::Unavailable),
                "{phase}"
            );
            assert_eq!(
                FAIL_EFFECT.with(|failure| failure.get()),
                None,
                "effect was not reached: {phase}"
            );
            assert_eq!(before, f.state(), "{phase} escaped rollback");
            let state: String = f
                .db()
                .query_row(
                    "SELECT state FROM storage_event_operations WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(state, "intent", "completion escaped rollback");
        }
        writer.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert_ne!(before, f.state());
    }

    #[test]
    fn configured_ledger_and_observation_cas_are_distinct_and_busy_is_typed() {
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let mut writer = Writer::new(&f.config);
        let path = f.raw.to_string_lossy().into_owned();
        writer.begin(&path, None, &"a".repeat(64), "codex").unwrap();
        let mut observations = f.config.source_observations().unwrap();
        observations
            .discover(std::slice::from_ref(&path), true)
            .unwrap();
        let ledger = Ledger {
            stat: Stat::of(&f.raw).unwrap(),
            offset: 0,
            prefix: Sha256::digest(b"").into(),
            tail: Sha256::digest(b"").into(),
        };
        assert_eq!(
            writer
                .commit(&[(&path, None, None, &ledger, &FileIndex::default())])
                .unwrap_err(),
            StorageError::Conflict
        );
        assert!(writer.ledger(&path).unwrap().is_none());
        let lock = f.db();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            writer.begin(&path, None, &"b".repeat(64), "codex"),
            Err(StorageError::Busy)
        );
        lock.execute_batch("ROLLBACK").unwrap();
        let mut stale = ledger.clone();
        stale.offset = 1;
        assert_eq!(
            writer.begin(&path, Some(&stale), &"c".repeat(64), "codex"),
            Err(StorageError::Conflict)
        );
    }

    #[test]
    fn configured_initial_batch_is_one_transaction_and_replay_is_idempotent() {
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let mut index = crate::storage::sqlite::open_configured_index(&f.config).unwrap();
        let files = ["first", "second"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let raw = f.root.join(name);
                fs::write(&raw, b"").unwrap();
                let path = raw.to_string_lossy().into_owned();
                let ledger = Ledger {
                    stat: Stat::of(&raw).unwrap(),
                    offset: 0,
                    prefix: Sha256::digest(b"").into(),
                    tail: Sha256::digest(b"").into(),
                };
                index
                    .prepare_file(&path, None, &format!("{:064x}", i + 1), "codex")
                    .unwrap();
                (path, ledger, std::sync::Arc::new(FileIndex::default()))
            })
            .collect::<Vec<_>>();
        let before = f.state();
        f.db().execute_batch("CREATE TRIGGER fail_second_event BEFORE INSERT ON files WHEN NEW.path LIKE '%/second' BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
        assert!(matches!(
            index.commit_initial_files(&files),
            Err(StoreError::Configured(StorageError::Unavailable))
        ));
        assert_eq!(before, f.state());
        f.db()
            .execute_batch("DROP TRIGGER fail_second_event")
            .unwrap();
        assert_eq!(
            index.commit_initial_files(&files).unwrap(),
            Outcome::Written
        );
        let committed = f.state();
        assert_eq!(
            index.commit_initial_files(&files).unwrap(),
            Outcome::Written
        );
        assert_eq!(committed, f.state());
    }
}
