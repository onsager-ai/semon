//! RLS-bound transfer inbox. Uses the ordinary admission/publication lock order.
use super::{
    PgCancellation, PgPool, model_cache, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{
    StorageError, StoreConfig,
    transfer::{self, InboxTransaction, SnapshotManifest, TransferCheckpoint, TransferPage},
};

struct Inbox<'a, 'b>(&'a mut PgTxn<'b>);

impl InboxTransaction for Inbox<'_, '_> {
    fn get(&mut self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, StorageError> {
        self.0.check()?;
        let rows = self.0.driver.query("SELECT key=$2,CASE WHEN octet_length(value)<=$3::bigint THEN value END FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea)", &[&self.0.namespace.as_str().as_bytes(), &key.as_bytes(), &(limit as i64)]).map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        row.get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::ScopeChanged)
            .map(Some)
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.0.check()?;
        let changed = self.0.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_meta.namespace=excluded.namespace AND semon_pg_native_meta.key=excluded.key", &[&self.0.namespace.as_str().as_bytes(), &key.as_bytes(), &value]).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

fn transaction<T>(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    operation: impl FnOnce(&mut Inbox<'_, '_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let bounded = control.bounded(std::time::Instant::now() + std::time::Duration::from_secs(2));
    pool.transaction(&config.namespace, TxnKind::Transfer, &bounded, |tx| {
        let binding = model_cache::binding(tx)?;
        if config.expected_stream_epoch.as_deref() != Some(binding.stream_epoch.as_str()) {
            return Err(StorageError::ScopeChanged);
        }
        operation(&mut Inbox(tx))
    })
}

pub(crate) fn begin(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| transfer::begin_tx(tx, manifest))
}

pub(crate) fn stage(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
    page: &TransferPage,
    bytes: &[u8],
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| {
        transfer::stage_tx(tx, manifest, page, bytes)
    })
}

pub(crate) fn page(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<TransferPage, StorageError> {
    transaction(config, pool, control, |tx| {
        transfer::page_tx(tx, manifest, sequence)
    })
}

pub(crate) fn checkpoint(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| {
        transfer::checkpoint_tx(tx, manifest)
    })
}

pub(crate) fn capture(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    operation: &[u8; 32],
) -> Result<transfer::CaptureProgress, StorageError> {
    transaction(config, pool, control, |tx| {
        let ns = config.namespace.as_str().as_bytes();
        let Some(mut state) = transfer::reported::capture_state(tx, config, operation)? else {
            let rows = tx.0.driver.query("SELECT CASE WHEN octet_length(stamp)<=16384 THEN stamp END FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton", &[&ns]).map_err(sqlstate::error)?;
            let stamp = rows
                .first()
                .map(|row| {
                    crate::events::reported_rows::decode_stamp(
                        &row.get::<_, Option<Vec<u8>>>(0)
                            .ok_or(StorageError::ScopeChanged)?,
                    )
                    .map_err(|_| StorageError::ScopeChanged)
                })
                .transpose()?;
            let revision:i64 = tx.0.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
            let binding = model_cache::binding(tx.0)?.admission;
            return transfer::reported::start_capture(
                tx,
                config,
                operation,
                binding,
                u64::try_from(revision).map_err(|_| StorageError::ScopeChanged)?,
                stamp,
            )
            .map(|state| state.progress());
        };
        let binding = model_cache::binding(tx.0)?.admission;
        let revision:i64=tx.0.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
        if binding != state.manifest.binding_epoch
            || u64::try_from(revision).ok() != Some(state.manifest.revision)
        {
            return Err(StorageError::ScopeChanged);
        }
        if state.done {
            return Ok(state.progress());
        }
        let cursor = state.cursor.as_ref().map(|(key, _)| key.as_slice());
        let start = state.cursor.as_ref().map_or(0, |(_, start)| *start);
        let sizes = tx.0.driver.query("SELECT octet_length(record)::bigint+octet_length(session_id)::bigint FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND ($2::bytea IS NULL OR (session_digest,start)>($2,$3)) ORDER BY session_digest,start LIMIT 64", &[&ns,&cursor,&start]).map_err(sqlstate::error)?;
        if sizes.is_empty() {
            transfer::reported::finish_capture(tx, &mut state)?;
            return Ok(state.progress());
        }
        let mut count = 0_i64;
        let mut bytes = 0_i64;
        for row in sizes {
            let size: i64 = row.get(0);
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
            return Err(StorageError::Busy);
        }
        let rows = tx.0.driver.query("SELECT session_digest,session_id,start,record FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND ($2::bytea IS NULL OR (session_digest,start)>($2,$3)) ORDER BY session_digest,start LIMIT $4", &[&ns,&cursor,&start,&count]).map_err(sqlstate::error)?;
        let mut runs = Vec::new();
        for row in rows {
            let record =
                crate::events::reported_rows::ReportedRow::decode(&row.get::<_, Vec<u8>>(3))
                    .map_err(|_| StorageError::ScopeChanged)?;
            if record.run.last_session_id.as_bytes() != row.get::<_, Vec<u8>>(1)
                || record.run.last_start_time != row.get::<_, i64>(2)
            {
                return Err(StorageError::ScopeChanged);
            }
            runs.push((row.get::<_, Vec<u8>>(0), record.run));
        }
        let (records, cursor) = transfer::reported::fit_facts(runs)?;
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
    pool: &PgPool,
    control: &PgCancellation,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<TransferCheckpoint, StorageError> {
    transaction(config, pool, control, |tx| {
        let page = transfer::reported::prepare_apply(tx, manifest, sequence)?;
        if page.replayed {
            return Ok(page.checkpoint);
        }
        let ns = config.namespace.as_str().as_bytes();
        if page.first {
            let occupied:bool=tx.0.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) OR EXISTS(SELECT 1 FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) OR EXISTS(SELECT 1 FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)", &[&ns]).map_err(sqlstate::error)?.get(0);
            if occupied {
                return Err(StorageError::Conflict);
            }
        }
        for record in &page.records {
            match record {
                transfer::TransferRecord::ReportedFact(run) => {
                    let record = crate::events::reported_rows::ReportedRow::new(run)
                        .and_then(|row| row.encode())
                        .map_err(|_| StorageError::ScopeChanged)?;
                    tx.0.driver.execute("INSERT INTO semon_pg_native_reported_runs(namespace,session_digest,session_id,start,record) VALUES($1,sha256($2::bytea),$2,$3,$4)", &[&ns,&run.last_session_id.as_bytes(),&run.last_start_time,&record]).map_err(sqlstate::error)?;
                }
                transfer::TransferRecord::Metadata { key, value } if key == "claude_json_stamp" => {
                    tx.0.driver.execute("INSERT INTO semon_pg_native_reported_stamp(namespace,singleton,stamp) VALUES($1,true,$2)", &[&ns,&value.as_bytes()]).map_err(sqlstate::error)?;
                }
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
    fn pg_current_transfer225_reported_authoritative_roundtrip_lost_ack_and_postwrite_reverse() {
        use transfer::reported::tests::{apply, observe, read, root, sqlite, stage};
        let root = root();
        let source = sqlite(&root, "source");
        let rollback = sqlite(&root, "rollback");
        let input = root.join(".claude.json");
        for start in 1..=200 {
            observe(&source, &input, start, start + 1000);
        }
        let expected = read(&source);
        assert_eq!(expected.len(), 200);
        let (pool, _) = super::super::tests::fixture(|_| {});
        let config = StoreConfig {
            backend: StoreBackend::Postgres {
                pool: pool.clone(),
                control: PgCancellation::default(),
            },
            namespace: source.namespace.clone(),
            expected_stream_epoch: source.expected_stream_epoch.clone(),
        };
        config.create_native_store().unwrap();
        let manifest = stage(&source, &config, &[3; 32]);
        let count = config
            .transfer_inbox()
            .checkpoint(&manifest)
            .unwrap()
            .sequence;
        assert!(count >= 5);
        super::super::tests::LOSE_ACK.with(|flag| flag.set(true));
        assert!(matches!(
            config.transfer_inbox().apply_page(&manifest, 1),
            Err(StorageError::CommitUncertain)
        ));
        apply(&config, &manifest);
        assert_eq!(read(&config), expected);
        observe(&config, &input, 300, 1300);
        let postwrite = read(&config);
        assert_eq!(postwrite.len(), 201);
        super::super::tests::LOSE_ACK.with(|flag| flag.set(true));
        assert!(matches!(
            config.transfer_inbox().capture(&[4; 32]),
            Err(StorageError::CommitUncertain)
        ));
        let reverse = stage(&config, &rollback, &[4; 32]);
        apply(&rollback, &reverse);
        assert_eq!(read(&rollback), postwrite);
        assert!(input.exists());
        let foreign = LogicalSourceNamespace::new("foreign231").unwrap();
        pool.transaction(
            &foreign,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| {
                let count: i64 = tx
                    .driver
                    .query_one("SELECT count(*) FROM semon_pg_native_reported_runs", &[])
                    .map_err(sqlstate::error)?
                    .get(0);
                assert_eq!(count, 0);
                Ok(())
            },
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pg_current_transfer225_inbox_restart_exact_replay_rls_and_unknown_commit() {
        let (pool, factory) = super::super::tests::fixture(|_| {});
        let control = PgCancellation::default();
        let config = StoreConfig {
            backend: StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            },
            namespace: LogicalSourceNamespace::new("transfer225").unwrap(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        let tables = factory.client().query("SELECT relname FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relkind='r' AND relname LIKE 'semon_pg_%' ORDER BY relname LIMIT 257", &[]).unwrap();
        assert!(!tables.is_empty() && tables.len() < 257);
        for table in tables {
            let name: String = table.get(0);
            crate::storage::transfer::inventory::require(
                crate::storage::transfer::inventory::postgres(&name),
                &name,
            );
        }
        crate::storage::transfer::tests::exercise(&config);
        let mut manifest = crate::storage::transfer::tests::manifest();
        manifest.revision += 1;
        manifest.seal().unwrap();
        let inbox = config.transfer_inbox();
        inbox.begin(&manifest).unwrap();
        let page = crate::storage::transfer::tests::page(&manifest, 1, [0; 32]);
        super::super::tests::LOSE_ACK.with(|flag| flag.set(true));
        assert!(matches!(
            inbox.stage_page(&manifest, &page),
            Err(StorageError::CommitUncertain)
        ));
        assert_eq!(inbox.stage_page(&manifest, &page).unwrap().sequence, 1);
        assert_eq!(inbox.checkpoint(&manifest).unwrap().sequence, 1);
        let foreign = LogicalSourceNamespace::new("foreign225").unwrap();
        pool.transaction(&foreign, TxnKind::ReadSnapshot, &control, |tx| {
            let count: i64 = tx
                .driver
                .query_one("SELECT count(*) FROM semon_pg_native_meta", &[])
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(count, 0, "forced RLS must conceal the entire staged inbox");
            Ok(())
        })
        .unwrap();
    }
}
