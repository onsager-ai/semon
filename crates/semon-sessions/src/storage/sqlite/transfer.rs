//! Namespace-owned durable transfer transport, separate from native publication.
mod catalog;
mod native;
mod pages;
mod rows;
use super::failed;
use crate::storage::{
    StorageError, StoreConfig,
    transfer::{self, InboxTransaction, SnapshotManifest, TransferCheckpoint, TransferPage},
};
use native::{apply_native, capture_native};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::time::{Duration, Instant};

struct Inbox<'a>(&'a Connection, &'a str);

impl InboxTransaction for Inbox<'_> {
    fn get(&mut self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, StorageError> {
        let row: Option<Option<String>> = self.0.query_row(
            "SELECT CASE WHEN octet_length(value)<=?3 THEN value END FROM main.meta WHERE namespace=?1 AND key=?2",
            params![self.1, key, limit as i64], |row| row.get(0),
        ).optional().map_err(failed)?;
        row.map(|value| {
            value
                .map(String::into_bytes)
                .ok_or(StorageError::ScopeChanged)
        })
        .transpose()
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let value = std::str::from_utf8(value).map_err(|_| StorageError::InvalidArguments)?;
        self.0.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", params![self.1, key, value]).map_err(failed)?;
        Ok(())
    }
}

fn transaction<T>(
    config: &StoreConfig,
    operation: impl FnOnce(&mut Inbox<'_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let owner = crate::events::store::configured::NativeOwnership::restore_transfer(config)?;
    let mut db = super::source_journal::SourceJournal::owned(owner.clone()).connection()?;
    transfer_writer(&db)?;
    db.progress_handler(1000, Some(move || Instant::now() >= deadline))
        .map_err(failed)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    owner.validate_ready(&tx)?;
    if Instant::now() >= deadline {
        return Err(StorageError::Busy);
    }
    let result = operation(&mut Inbox(&tx, config.namespace.as_str()))?;
    if Instant::now() >= deadline {
        return Err(StorageError::Busy);
    }
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    Ok(result)
}

pub(crate) fn begin(
    config: &StoreConfig,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, |tx| transfer::begin_tx(tx, manifest))
}

pub(crate) fn stage(
    config: &StoreConfig,
    manifest: &SnapshotManifest,
    page: &TransferPage,
    bytes: &[u8],
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, |tx| transfer::stage_tx(tx, manifest, page, bytes))
}

pub(crate) fn page(
    config: &StoreConfig,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<TransferPage, StorageError> {
    transaction(config, |tx| transfer::page_tx(tx, manifest, sequence))
}

pub(crate) fn checkpoint(
    config: &StoreConfig,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, |tx| transfer::checkpoint_tx(tx, manifest))
}

fn transfer_writer(db: &Connection) -> Result<(), StorageError> {
    db.create_scalar_function(
        "semon_transfer_writer",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failed)
}

fn install_fence(db: &Connection) -> Result<(), StorageError> {
    // The fence belongs to the existing namespace, not a new admission epoch.
    // Persistent triggers cover already-open connections and future host opens.
    let tables = db.prepare("SELECT name FROM main.sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 257").map_err(failed)?.query_map([], |r| r.get::<_,String>(0)).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
    if tables.len() > 256 {
        return Err(StorageError::ScopeChanged);
    }
    for (index, table) in tables.iter().enumerate() {
        let scoped: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace')",
                [table],
                |r| r.get(0),
            )
            .map_err(failed)?;
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let row = if operation == "DELETE" { "OLD" } else { "NEW" };
            let scope = if scoped {
                format!("AND m.namespace={row}.namespace")
            } else {
                String::new()
            };
            let old_scope = if scoped && operation == "UPDATE" {
                " OR m.namespace=OLD.namespace"
            } else {
                ""
            };
            // UPDATE must not move a frozen row to a different namespace.
            let scope = if old_scope.is_empty() {
                scope
            } else {
                format!("AND (m.namespace=NEW.namespace{old_scope})")
            };
            let sql = format!(
                "CREATE TRIGGER IF NOT EXISTS storage_transfer_fence_{index}_{operation} BEFORE {operation} ON \"{}\" WHEN semon_transfer_writer()<>1 AND EXISTS(SELECT 1 FROM main.meta m WHERE m.key IN('native_transfer_v1:source-freeze','native_transfer_v1:incoming') AND m.value<>'' {scope}) BEGIN SELECT RAISE(ABORT,'source transfer fenced'); END;",
                table.replace('"', "\"\"")
            );
            db.execute_batch(&sql).map_err(failed)?;
        }
    }
    Ok(())
}

pub(crate) fn capture(
    config: &StoreConfig,
    operation: &[u8; 32],
) -> Result<transfer::CaptureProgress, StorageError> {
    transaction(config, |tx| {
        let mut state = if let Some(state) =
            transfer::reported::capture_state(tx, config, operation)?
        {
            state
        } else {
            install_fence(tx.0)?;
            let stamp: Option<Option<String>> = tx.0.query_row("SELECT CASE WHEN octet_length(value)<=16384 THEN value END FROM main.meta WHERE namespace=?1 AND key='claude_json_stamp'", [tx.1], |row|row.get(0)).optional().map_err(failed)?;
            let stamp = stamp
                .map(|value| {
                    crate::events::reported_rows::decode_stamp(
                        value.ok_or(StorageError::ScopeChanged)?.as_bytes(),
                    )
                    .map_err(|_| StorageError::ScopeChanged)
                })
                .transpose()?;
            let (binding, revision): (i64,i64) = tx.0.query_row("SELECT n.admission_generation,j.revision FROM storage_namespaces n JOIN storage_source_journal j USING(namespace) WHERE n.namespace=?1", [tx.1], |r|Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
            let state = transfer::reported::start_capture(
                tx,
                config,
                operation,
                u64::try_from(binding).map_err(|_| StorageError::ScopeChanged)?,
                u64::try_from(revision).map_err(|_| StorageError::ScopeChanged)?,
                stamp,
            )?;
            return Ok(state.progress());
        };
        let (binding, revision):(i64,i64)=tx.0.query_row("SELECT n.admission_generation,j.revision FROM storage_namespaces n JOIN storage_source_journal j USING(namespace) WHERE n.namespace=?1", [tx.1], |r|Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
        if u64::try_from(binding).ok() != Some(state.manifest.binding_epoch)
            || u64::try_from(revision).ok() != Some(state.manifest.revision)
        {
            return Err(StorageError::ScopeChanged);
        }
        if state.native && !state.done {
            return capture_native(tx, &mut state);
        }
        if state.done {
            return Ok(state.progress());
        }
        let cursor = state
            .cursor
            .as_ref()
            .map(|(key, _)| {
                std::str::from_utf8(key)
                    .map(str::to_owned)
                    .map_err(|_| StorageError::ScopeChanged)
            })
            .transpose()?;
        let start = state.cursor.as_ref().map_or(0, |(_, start)| *start);
        let sizes = tx.0.prepare("SELECT octet_length(session_id)+octet_length(model_usage)+512 FROM reported_runs WHERE namespace=?1 AND (?2 IS NULL OR (session_id,start)>(?2,?3)) ORDER BY session_id,start LIMIT 64").map_err(failed)?.query_map(params![tx.1,cursor,start], |r|r.get::<_,i64>(0)).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
        let mut count = 0usize;
        let mut bytes = 0_i64;
        for size in sizes {
            if size < 0
                || bytes
                    .checked_add(size)
                    .is_none_or(|sum| sum > transfer::MAX_PAGE_BYTES as i64)
            {
                break;
            }
            bytes += size;
            count += 1;
        }
        if count == 0 {
            let more: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM reported_runs WHERE namespace=?1 AND (?2 IS NULL OR (session_id,start)>(?2,?3)))", params![tx.1,cursor,start], |r|r.get(0)).map_err(failed)?;
            if more {
                return Err(StorageError::Busy);
            }
            state.native = true;
            state.cursor = None;
            transfer::reported::save_capture(tx, &state)?;
            return Ok(state.progress());
        }
        let sql = format!(
            "SELECT {} FROM reported_runs WHERE namespace=?1 AND (?2 IS NULL OR (session_id,start)>(?2,?3)) ORDER BY session_id,start LIMIT ?4",
            crate::events::store::RUN_COLUMNS
        );
        let runs =
            tx.0.prepare(&sql)
                .map_err(failed)?
                .query_map(
                    params![tx.1, cursor, start, count as i64],
                    crate::events::store::read_run,
                )
                .map_err(failed)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(failed)?;
        let rows = runs
            .into_iter()
            .map(|run| (run.last_session_id.as_bytes().to_vec(), run))
            .collect();
        let (records, cursor) = transfer::reported::fit_facts(rows)?;
        if state.checkpoint.rows[transfer::Category::ALL
            .iter()
            .position(|c| *c == transfer::Category::Metadata)
            .unwrap()]
            == 0
        {
            return Err(StorageError::ScopeChanged);
        }
        transfer::reported::append_capture(
            tx,
            &mut state,
            transfer::Category::ReportedFacts,
            records,
        )?;
        state.cursor = cursor;
        transfer::reported::save_capture(tx, &state)?;
        Ok(state.progress())
    })
}

pub(crate) fn apply(
    config: &StoreConfig,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, |tx| {
        let page = transfer::reported::prepare_apply(tx, manifest, sequence)?;
        if page.replayed {
            return Ok(page.checkpoint);
        }
        if page.first {
            let occupied: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM reported_runs WHERE namespace=?1) OR EXISTS(SELECT 1 FROM meta WHERE namespace=?1 AND key='claude_json_stamp') OR EXISTS(SELECT 1 FROM files WHERE namespace=?1) OR EXISTS(SELECT 1 FROM session_catalog WHERE namespace=?1) OR EXISTS(SELECT 1 FROM session_history_catalog WHERE namespace=?1)", [tx.1], |r|r.get(0)).map_err(failed)?;
            let published: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key IN ('catalog_generation','native_transfer_v1:catalog-content-generation') AND value<>'') OR EXISTS(SELECT 1 FROM main.storage_semantic_publications WHERE namespace=?1)", [tx.1], |r|r.get(0)).map_err(failed)?;
            if occupied || published {
                return Err(StorageError::Conflict);
            }
            install_fence(tx.0)?;
        }
        for record in &page.records {
            match record {
                transfer::TransferRecord::ReportedFact(run) => {
                    let exists: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM reported_runs WHERE namespace=?1 AND session_id=?2 AND start=?3)", params![tx.1,run.last_session_id,run.last_start_time], |r|r.get(0)).map_err(failed)?;
                    if exists {
                        return Err(StorageError::Conflict);
                    }
                    let mut statement =
                        tx.0.prepare(crate::events::store::PUT_RUN_SCOPED)
                            .map_err(failed)?;
                    crate::events::store::put_run(&mut statement, run).map_err(failed)?;
                }
                transfer::TransferRecord::Metadata { key, value } if key == "claude_json_stamp" => {
                    tx.put(key, value.as_bytes())?
                }
                transfer::TransferRecord::NativeRow { .. } => rows::apply(tx, record)?,
                transfer::TransferRecord::CatalogRow { .. } => catalog::apply(tx, record)?,
                transfer::TransferRecord::NativeSource(_)
                | transfer::TransferRecord::NativePage { .. }
                | transfer::TransferRecord::NativeMetadata { .. } => pages::apply(tx, record)?,
                transfer::TransferRecord::NativeFile { .. }
                | transfer::TransferRecord::NativeEvent { .. } => apply_native(tx, record)?,
                _ => return Err(StorageError::Conflict),
            }
        }
        transfer::reported::acknowledge_apply(tx, &page)?;
        Ok(page.checkpoint)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{LogicalSourceNamespace, StoreBackend};

    #[test]
    fn current_transfer225_sqlite_current_inventory_and_atomic_checkpoint_failure() {
        let root =
            std::env::temp_dir().join(format!("semon-transfer225-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let config = StoreConfig {
            backend: StoreBackend::Sqlite {
                database: root.join("current.sqlite3"),
            },
            namespace: LogicalSourceNamespace::new("transfer225").unwrap(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
        let db = Connection::open(config.backend.sqlite_database().unwrap()).unwrap();
        let tables: Vec<String> = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 257").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert!(!tables.is_empty() && tables.len() < 257);
        for table in &tables {
            transfer::inventory::require(transfer::inventory::sqlite(table), table);
        }
        let manifest = transfer::tests::manifest();
        let inbox = config.transfer_inbox();
        let initial = inbox.begin(&manifest).unwrap();
        db.execute_batch("CREATE TRIGGER transfer225_checkpoint_fault BEFORE UPDATE ON meta WHEN NEW.key GLOB 'native_transfer_v1:*:checkpoint' BEGIN SELECT RAISE(ABORT,'injected transfer checkpoint failure'); END;").unwrap();
        let page = transfer::tests::page(&manifest, 1, [0; 32]);
        assert!(inbox.stage_page(&manifest, &page).is_err());
        assert_eq!(inbox.checkpoint(&manifest).unwrap(), initial);
        assert!(inbox.staged_page(&manifest, 1).is_err());
        db.execute_batch("DROP TRIGGER transfer225_checkpoint_fault")
            .unwrap();
        assert_eq!(inbox.stage_page(&manifest, &page).unwrap().sequence, 1);
        assert_eq!(
            db.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
