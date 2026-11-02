//! Private native pipeline injection. Public complete-backend selection is closed.
use super::{
    PgCancellation, PgPool, PgSourceJournal, native_ingest, native_reader, reported_facts,
    sqlstate, txn::TxnKind,
};
use crate::{
    events::{
        DirtyRows, FileIndex, IndexStore, Ledger, Outcome, ReportedFileStamp,
        SESSION_DESCRIPTION_BATCH_LIMIT, SessionDescriptionRequest, Stat, StoreError,
    },
    storage::{LogicalSourceNamespace, StorageError, native_events::EventReceipt},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(crate) struct NativeIndex {
    pool: PgPool,
    namespace: LogicalSourceNamespace,
    control: PgCancellation,
    journal: PgSourceJournal,
    intents: BTreeMap<String, native_ingest::Intent>,
    selection: Option<BTreeSet<String>>,
    semantic_epoch: Option<i64>,
}

pub(crate) fn open(
    pool: PgPool,
    namespace: LogicalSourceNamespace,
    control: PgCancellation,
) -> Result<(Box<dyn IndexStore>, Option<ReportedFileStamp>), StorageError> {
    // Create namespace coordination only, never an engine/cache-path fallback.
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        tx.driver
            .execute(
                "INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT DO NOTHING",
                &[&namespace.as_str().as_bytes()],
            )
            .map_err(sqlstate::error)?;
        super::model_cache::initialize(tx)
    })?;
    let pool = pool.bind_source(&namespace, None, &control)?;
    let journal = PgSourceJournal {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
    };
    let stamp = reported_facts::load(&pool, &namespace, Some(&BTreeSet::new()), &control)?.stamp;
    Ok((
        Box::new(NativeIndex {
            pool,
            namespace,
            control,
            journal,
            intents: BTreeMap::new(),
            selection: None,
            semantic_epoch: None,
        }),
        stamp,
    ))
}

fn err(e: StorageError) -> StoreError {
    StoreError::Configured(e)
}
fn read_error(error: std::io::Error) -> StoreError {
    let kind = error.kind();
    let storage = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<StorageError>())
        .copied()
        .unwrap_or(match kind {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => StorageError::Busy,
            _ => StorageError::Unavailable,
        });
    err(storage)
}

fn source_continuation_paths(tx: &mut super::txn::PgTxn<'_>) -> Result<Vec<String>, StorageError> {
    tx.check()?;
    let rows = tx
        .driver
        .query(
            "SELECT source_path FROM semon_pg_native_source_continuations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 ORDER BY source_path LIMIT 65",
            &[&tx.namespace.as_str().as_bytes()],
        )
        .map_err(sqlstate::error)?;
    rows.into_iter()
        .map(|row| String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged))
        .collect()
}

pub(super) fn source_continuation_snapshot(
    tx: &mut super::txn::PgTxn<'_>,
    paths: &[String],
    complete: bool,
) -> Result<Vec<crate::storage::catalog::SourceContinuationCapture>, StorageError> {
    if paths.len() > crate::storage::catalog::SOURCE_CONTINUATION_LIMIT
        || paths.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(StorageError::InvalidArguments);
    }
    tx.check()?;
    if complete && source_continuation_paths(tx)? != paths {
        return Err(StorageError::Conflict);
    }
    let namespace = tx.namespace.as_str().as_bytes().to_vec();
    let generation = super::model_cache::generation(tx)?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let requested = pg_source_path_values(2, paths.len());
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> =
        vec![Box::new(namespace.clone())];
    params.extend(paths.iter().map(|path| {
        Box::new(path.as_bytes().to_vec()) as Box<dyn tokio_postgres::types::ToSql + Sync>
    }));
    let params = params
        .iter()
        .map(|value| &**value as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect::<Vec<_>>();
    let rows = tx.driver.query(
        &format!(
            "WITH requested(source_path) AS (VALUES {requested}) \
             SELECT r.source_path,c.source_path IS NOT NULL,CASE WHEN octet_length(c.evidence)<=8192 THEN c.evidence END, \
                    i.revision,f.file_id,f.dev::text,f.ino::text,f.size::text,f.mtime_ns,f.resume_at::text, \
                    f.head_sha256,f.tail_sha256,f.ctime_ns, \
                    EXISTS(SELECT 1 FROM semon_pg_native_source_mutations m WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.source_path=r.source_path), \
                    EXISTS(SELECT 1 FROM semon_pg_native_meta m WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.key_digest=sha256(convert_to('raw-write-inflight:','UTF8') || r.source_path) AND semon_pg_native_key_matches(m.key,m.key_digest,convert_to('raw-write-inflight:','UTF8') || r.source_path)) \
             FROM requested r \
             LEFT JOIN semon_pg_native_source_continuations c ON c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.source_path=r.source_path \
             LEFT JOIN semon_pg_source_invalidations i ON i.namespace_digest=sha256($1::bytea) AND i.namespace=$1 AND i.search_path_digest=sha256(r.source_path) AND semon_pg_native_key_matches(i.source_path,i.search_path_digest,r.source_path) \
             LEFT JOIN semon_pg_native_files f ON f.namespace_digest=sha256($1::bytea) AND f.namespace=$1 AND f.path_digest=sha256(r.source_path) AND semon_pg_native_key_matches(f.path,f.path_digest,r.source_path) \
             ORDER BY r.source_path"
        ),
        &params,
    )
    .map_err(sqlstate::error)?;
    if rows.len() != paths.len() {
        return Err(StorageError::ScopeChanged);
    }
    let mut captures = Vec::with_capacity(rows.len());
    for (row, expected_path) in rows.into_iter().zip(paths) {
        let path =
            String::from_utf8(row.get::<_, Vec<u8>>(0)).map_err(|_| StorageError::ScopeChanged)?;
        if path != *expected_path {
            return Err(StorageError::ScopeChanged);
        }
        let has_cell: bool = row.get(1);
        let cell: Option<Vec<u8>> = row.get(2);
        if has_cell != cell.is_some() || (complete && !has_cell) {
            return Err(StorageError::ScopeChanged);
        }
        if let Some(bytes) = &cell {
            let decoded =
                serde_json::from_slice::<crate::storage::catalog::SourceContinuationCell>(bytes)
                    .map_err(|_| StorageError::ScopeChanged)?;
            if !crate::storage::catalog::valid_source_continuation_cell(&decoded) {
                return Err(StorageError::ScopeChanged);
            }
        }
        let ledger = pg_source_continuation_ledger(&row)?;
        if has_cell && ledger.is_none() {
            return Err(StorageError::ScopeChanged);
        }
        captures.push(crate::storage::catalog::SourceContinuationCapture {
            path,
            cell,
            claim: row.get(3),
            ledger,
            pending_mutation: row.get(13),
            raw_inflight: row.get(14),
            catalog_session_key: None,
            catalog_source: None,
            physical: None,
        });
    }

    let cell_paths = captures
        .iter()
        .filter(|capture| capture.cell.is_some())
        .map(|capture| capture.path.clone())
        .collect::<Vec<_>>();
    if !cell_paths.is_empty()
        && let Some(generation) = generation
    {
        let requested = pg_source_path_values(3, cell_paths.len());
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = vec![
            Box::new(namespace.clone()),
            Box::new(generation.as_bytes().to_vec()),
        ];
        params.extend(cell_paths.iter().map(|path| {
            Box::new(path.as_bytes().to_vec()) as Box<dyn tokio_postgres::types::ToSql + Sync>
        }));
        let params = params
            .iter()
            .map(|value| &**value as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect::<Vec<_>>();
        let owner_rows = tx.driver.query(
            &format!(
                "WITH requested(source_path) AS (VALUES {requested}), owners AS ( \
                 SELECT c.source_path,c.session_key,c.metadata,COUNT(*) OVER(PARTITION BY c.source_path) AS owner_count, \
                        ROW_NUMBER() OVER(PARTITION BY c.source_path ORDER BY c.session_key) AS owner_number \
                 FROM semon_pg_native_catalog_sources c JOIN requested r ON r.source_path=c.source_path \
                 WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.generation=$2 AND c.path_digest=sha256(r.source_path) \
                   AND semon_pg_native_key_matches(c.source_path,c.path_digest,r.source_path)) \
                 SELECT source_path,session_key,owner_count,octet_length(metadata)::bigint \
                 FROM owners WHERE owner_number<=2 ORDER BY source_path,session_key"
            ),
            &params,
        ).map_err(sqlstate::error)?;
        let mut owners = BTreeMap::new();
        let mut metadata_bytes = 0usize;
        for row in owner_rows {
            let path = String::from_utf8(row.get::<_, Vec<u8>>(0))
                .map_err(|_| StorageError::ScopeChanged)?;
            let count: i64 = row.get(2);
            if count != 1 || owners.contains_key(&path) {
                return Err(StorageError::ScopeChanged);
            }
            let size =
                usize::try_from(row.get::<_, i64>(3)).map_err(|_| StorageError::ScopeChanged)?;
            metadata_bytes = metadata_bytes.checked_add(size).ok_or(StorageError::Busy)?;
            if size > 1_048_576 || metadata_bytes > 1_048_576 {
                return Err(StorageError::Busy);
            }
            let session_key = String::from_utf8(row.get::<_, Vec<u8>>(1))
                .map_err(|_| StorageError::ScopeChanged)?;
            owners.insert(path, session_key);
        }
        crate::selected_read::charge(owners.len() * 128, metadata_bytes)
            .map_err(|_| StorageError::Busy)?;
        if !owners.is_empty() {
            let owner_paths = owners.keys().cloned().collect::<Vec<_>>();
            let requested = pg_source_path_values(3, owner_paths.len());
            let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = vec![
                Box::new(namespace),
                Box::new(generation.as_bytes().to_vec()),
            ];
            params.extend(owner_paths.iter().map(|path| {
                Box::new(path.as_bytes().to_vec()) as Box<dyn tokio_postgres::types::ToSql + Sync>
            }));
            let params = params
                .iter()
                .map(|value| &**value as &(dyn tokio_postgres::types::ToSql + Sync))
                .collect::<Vec<_>>();
            let metadata_rows = tx.driver.query(
                &format!(
                    "WITH requested(source_path) AS (VALUES {requested}) \
                     SELECT c.source_path,c.session_key,c.metadata FROM semon_pg_native_catalog_sources c \
                     JOIN requested r ON r.source_path=c.source_path \
                     WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.generation=$2 \
                       AND c.path_digest=sha256(r.source_path) AND semon_pg_native_key_matches(c.source_path,c.path_digest,r.source_path) \
                     ORDER BY c.source_path"
                ),
                &params,
            ).map_err(sqlstate::error)?;
            if metadata_rows.len() != owners.len() {
                return Err(StorageError::ScopeChanged);
            }
            for row in metadata_rows {
                let path = String::from_utf8(row.get::<_, Vec<u8>>(0))
                    .map_err(|_| StorageError::ScopeChanged)?;
                let session_key = String::from_utf8(row.get::<_, Vec<u8>>(1))
                    .map_err(|_| StorageError::ScopeChanged)?;
                if owners.get(&path) != Some(&session_key) {
                    return Err(StorageError::ScopeChanged);
                }
                let bytes: Vec<u8> = row.get(2);
                let source = serde_json::from_slice::<crate::model::summary::CatalogSource>(&bytes)
                    .map_err(|_| StorageError::ScopeChanged)?;
                if source.path != std::path::Path::new(&path) {
                    return Err(StorageError::ScopeChanged);
                }
                let index = captures
                    .binary_search_by(|capture| capture.path.cmp(&path))
                    .map_err(|_| StorageError::ScopeChanged)?;
                captures[index].catalog_session_key = Some(session_key);
                captures[index].catalog_source = Some(source);
            }
        }
    }
    tx.check()?;
    Ok(captures)
}

fn pg_source_path_values(first: usize, count: usize) -> String {
    (0..count)
        .map(|offset| format!("(${}::bytea)", first + offset))
        .collect::<Vec<_>>()
        .join(",")
}

fn pg_source_continuation_ledger(
    row: &tokio_postgres::Row,
) -> Result<Option<Ledger>, StorageError> {
    if row.get::<_, Option<i64>>(4).is_none() {
        return Ok(None);
    }
    let numeric = |column| -> Result<u64, StorageError> {
        row.get::<_, Option<String>>(column)
            .ok_or(StorageError::ScopeChanged)?
            .parse()
            .map_err(|_| StorageError::ScopeChanged)
    };
    let stat_bytes = |column| -> Result<[u8; 16], StorageError> {
        row.get::<_, Option<Vec<u8>>>(column)
            .ok_or(StorageError::ScopeChanged)?
            .try_into()
            .map_err(|_| StorageError::ScopeChanged)
    };
    let digest = |column| -> Result<[u8; 32], StorageError> {
        row.get::<_, Option<Vec<u8>>>(column)
            .ok_or(StorageError::ScopeChanged)?
            .try_into()
            .map_err(|_| StorageError::ScopeChanged)
    };
    let changed = row
        .try_get::<_, Option<Vec<u8>>>(12)
        .map_err(|_| StorageError::ScopeChanged)?
        .map(|bytes| {
            bytes
                .try_into()
                .map(i128::from_be_bytes)
                .map_err(|_| StorageError::ScopeChanged)
        })
        .transpose()?;
    let ledger = Ledger {
        stat: Stat {
            dev: numeric(5)?,
            ino: numeric(6)?,
            size: numeric(7)?,
            modified_ns: u128::from_be_bytes(stat_bytes(8)?),
            changed_ns: changed,
        },
        offset: numeric(9)?,
        prefix: digest(10)?,
        tail: digest(11)?,
    };
    if ledger.offset > ledger.stat.size {
        return Err(StorageError::ScopeChanged);
    }
    Ok(Some(ledger))
}

impl NativeIndex {
    fn summary(
        &self,
        path: &str,
        ledger: &Ledger,
    ) -> Result<crate::native_tree::Summary, StoreError> {
        super::tree_checkpoint::Snapshot::restore(
            self.pool.clone(),
            self.namespace.clone(),
            path,
            ledger,
            self.control.clone(),
        )
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("PG_INDEX_REFUSED summary error_kind={:?}", _error.kind());
        })
        .map_err(read_error)
    }
}

impl IndexStore for NativeIndex {
    fn prepare_facts_authority(&self, options: &crate::Options) -> std::io::Result<()> {
        if options.facts.is_some() {
            super::current_sources::prepare(&self.pool, &self.namespace, &self.control, options)
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
    fn facts_authority(
        &self,
        path: &std::path::Path,
    ) -> std::io::Result<crate::facts::source_authority::Sources> {
        super::current_sources::CurrentSources::open(
            self.pool.clone(),
            self.namespace.clone(),
            self.control.clone(),
            path,
        )
        .map(crate::facts::source_authority::Sources::Postgres)
        .map_err(std::io::Error::other)
    }

    fn begin_semantic_stage(
        &mut self,
        paths: &[String],
        runtime: &str,
        _: bool,
    ) -> Result<Outcome, StoreError> {
        let (outcome, epoch) = self
            .pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::begin(tx, paths, runtime, self.semantic_epoch)
            })
            .map_err(err)?;
        self.semantic_epoch = Some(epoch);
        Ok(outcome)
    }
    fn semantic_batch(
        &mut self,
        limit: usize,
    ) -> Result<Option<crate::model::relation_stage::Batch>, StoreError> {
        let batch = self
            .pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::next(tx, limit)
            })
            .map_err(err)?;
        if let Some(batch) = &batch {
            self.semantic_epoch = Some(batch.epoch);
        }
        Ok(batch)
    }
    fn record_semantic_batch(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
        #[cfg(test)]
        eprintln!("PG_SEMANTIC_RECORD phase={:?}", batch.phase);
        let _withdrawal_guard = prepared
            .withdrawal_guard()
            .map_err(|_| err(StorageError::Conflict))?;
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::record(tx, batch, prepared)
            })
            .map_err(err)
    }
    fn publish_semantic_catalog(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        let withdrawals = prepared
            .withdrawal_guard()
            .map_err(|_| StorageError::Conflict);
        let physical = withdrawals.as_ref().map(|_| ()).map_err(|error| *error);
        let lifecycle = (|| {
            if let Some(authority) = &prepared.relationships.lifecycle_authority {
                self.pool.transaction(
                    &self.namespace,
                    TxnKind::ReadSnapshot,
                    &self.control,
                    |tx| {
                        let revision: i64 = tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&tx.namespace.as_str().as_bytes()]).map_err(super::sqlstate::error)?.get(0);
                        if revision != batch.source_revision
                            || !super::current_sources::lifecycle_recorded(tx, authority)?
                        {
                            return Err(StorageError::Conflict);
                        }
                        Ok(())
                    },
                )?;
            }
            super::catalog_effects::publication::preflight(
                prepared.relationships.lifecycle_authority.as_ref(),
            )
        })();
        let result = self
            .pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::publish(tx, batch, prepared, lifecycle, physical)
            })
            .map_err(err);
        // The callback returns before COMMIT. Keep SourceChange exclusion
        // until transaction() has acknowledged commit or completed rollback.
        drop(withdrawals);
        result
    }
    fn finish_semantic_stage(&mut self, epoch: i64) -> Result<Outcome, StoreError> {
        let outcome = self
            .pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::finish(tx, epoch)
            })
            .map_err(err)?;
        self.semantic_epoch = None;
        Ok(outcome)
    }
    fn semantic_select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::select(tx, epoch, selector)
            })
            .and_then(|selected| selected.ok_or(StorageError::Conflict))
            .map_err(err)
    }
    fn semantic_replay_page(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<crate::model::relation_stage::Context, StoreError> {
        use crate::model::relation_stage::Context;
        if owners.len() > 64 {
            return Err(err(StorageError::Busy));
        }
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| {
                    let context = super::semantic_stage::context(tx, epoch)?;
                    let mut output = Context {
                        epoch: Some(epoch),
                        ..Default::default()
                    };
                    for (key, relation) in context.relations.range((
                        std::ops::Bound::Excluded(after.to_owned()),
                        std::ops::Bound::Unbounded,
                    )) {
                        if !owners.contains(&relation.from)
                            && !relation.to.as_ref().is_some_and(|to| owners.contains(to))
                        {
                            continue;
                        }
                        for key in std::iter::once(&relation.from).chain(relation.to.iter()) {
                            if let Some(endpoint) = context.endpoints.get(key) {
                                output.endpoints.insert(key.clone(), endpoint.clone());
                            }
                        }
                        output.relations.insert(key.clone(), relation.clone());
                        if output.relations.len() == 64 {
                            break;
                        }
                    }
                    Ok(output)
                },
            )
            .map_err(err)
    }
    fn source_observations(
        &self,
    ) -> Option<&dyn crate::storage::source_observations::SourceObservationStore> {
        Some(&self.journal)
    }
    fn source_observations_mut(
        &mut self,
    ) -> Option<&mut dyn crate::storage::source_observations::SourceObservationStore> {
        Some(&mut self.journal)
    }
    fn prepare_file(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        operation_id: &str,
        harness: &str,
    ) -> Result<(), StoreError> {
        let intent = native_ingest::begin(
            &self.pool,
            &self.namespace,
            &self.control,
            path,
            harness,
            operation_id,
            expected,
        )
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("PG_INDEX_REFUSED prepare_file error={_error:?}");
        })
        .map_err(err)?;
        self.intents.insert(path.to_owned(), intent);
        Ok(())
    }
    fn reconcile_event(
        &self,
        id: &str,
        path: &str,
        harness: &str,
    ) -> Result<Option<EventReceipt>, StoreError> {
        native_ingest::reconcile(
            &self.pool,
            &self.namespace,
            &self.control,
            id,
            path,
            harness,
        )
        .map_err(err)
    }
    fn event_receipt(&self, path: &str) -> Result<EventReceipt, StoreError> {
        let i = self
            .intents
            .get(path)
            .ok_or_else(|| err(StorageError::Unavailable))?;
        let mut receipt = native_ingest::reconcile(
            &self.pool,
            &self.namespace,
            &self.control,
            &i.id,
            path,
            i.harness(),
        )
        .map_err(err)?
        .ok_or_else(|| err(StorageError::Unavailable))?;
        receipt.replayed = false;
        Ok(receipt)
    }
    fn ledger(&self, path: &str) -> Result<Option<Ledger>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| native_reader::ledger(tx, path).map(|v| v.map(|(_, l)| l)),
            )
            .map_err(err)
    }
    fn ledger_batch(&self, paths: &[String]) -> Result<Vec<Option<Ledger>>, StoreError> {
        if paths.len() > 64 {
            return Err(err(StorageError::InvalidArguments));
        }
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| {
                    paths
                        .iter()
                        .map(|path| native_reader::ledger(tx, path).map(|v| v.map(|(_, l)| l)))
                        .collect()
                },
            )
            .map_err(err)
    }
    fn load_file(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError> {
        let loaded = self
            .pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| native_reader::file(tx, path),
            )
            .inspect_err(|_error| {
                #[cfg(test)]
                eprintln!("PG_INDEX_REFUSED load_file error={_error:?}");
            })
            .map_err(err)?;
        let Some((ledger, mut index)) = loaded else {
            return Ok(None);
        };
        if index.copilot.is_none() {
            index.tree_summary = Some(self.summary(path, &ledger)?);
        }
        Ok(Some((ledger, index)))
    }
    fn load_tree(
        &self,
        path: &str,
        expected: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StoreError> {
        let parts = self
            .pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| native_reader::tree_parts(tx, path, expected),
            )
            .map_err(err)?;
        let Some((ledger, marker, calls)) = parts else {
            return Ok(None);
        };
        let summary = self.summary(path, &ledger)?;
        Ok(Some((
            ledger,
            crate::native_tree::Metadata {
                summary,
                chunks: None,
                marker,
                calls: Arc::new(calls),
            },
        )))
    }
    fn native_usage_seen(
        &self,
        path: &str,
        ledger: &Ledger,
        identity: &str,
    ) -> Result<crate::events::store::native_usage::Seen, StoreError> {
        self.pool.transaction(&self.namespace,TxnKind::ReadSnapshot,&self.control,|tx|{
            let ns=self.namespace.as_str().as_bytes();
            let source=tx.driver.query("SELECT s.source_id,s.receipt,s.usage_ready FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace,source_id) WHERE c.namespace_digest=sha256($1) AND c.namespace=$1 AND c.source_path=$2 AND s.parser_version=$3",&[&ns,&path.as_bytes(),&i64::from(crate::events::CACHE_VERSION)]).map_err(sqlstate::error)?;
            let Some(row)=source.first() else {return Err(StorageError::ScopeChanged);};
            if row.get::<_,Vec<u8>>(1)!=crate::events::store::header_ledger(ledger).as_bytes(){return Err(StorageError::ScopeChanged);}
            if !row.get::<_,bool>(2){return Ok(crate::events::store::native_usage::Seen::default());}
            let id:i64=row.get(0);
            let row=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_usage_identities WHERE namespace=$1 AND source_id=$2 AND scope=$4 AND identity=$3),EXISTS(SELECT 1 FROM semon_pg_native_usage_identities WHERE namespace=$1 AND source_id=$2 AND scope=$5 AND identity=$3)",&[&ns,&id,&identity.as_bytes(),&b"tree".as_slice(),&b"model".as_slice()]).map_err(sqlstate::error)?;
            Ok(crate::events::store::native_usage::Seen {tree:row.get(0),model:row.get(1)})
        }).map_err(err)
    }
    fn commit_file(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        changes: Option<&DirtyRows>,
        ledger: &Ledger,
        physical_stat: &Stat,
        index: &FileIndex,
    ) -> Result<crate::events::IndexWriteResult, StoreError> {
        let intent = self
            .intents
            .get(path)
            .ok_or_else(|| err(StorageError::InvalidArguments))?;
        native_ingest::commit(
            &self.pool,
            &self.namespace,
            &self.control,
            intent,
            path,
            expected,
            changes,
            ledger,
            physical_stat,
            index,
        )
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("PG_INDEX_REFUSED commit_file error={_error:?}");
        })
        .map_err(err)
    }
    fn commit_initial_files(
        &mut self,
        files: &[(String, Ledger, Arc<FileIndex>)],
    ) -> Result<crate::events::IndexWriteResult, StoreError> {
        let source_revision_delta = native_ingest::commit_initial(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.intents,
            files,
        )
        .map_err(err)?;
        Ok(crate::events::IndexWriteResult::new(
            Outcome::Written,
            source_revision_delta,
        ))
    }
    fn select_reported_runs(
        &mut self,
        ids: &BTreeSet<String>,
    ) -> Result<Vec<crate::facts::ReportedRunSnapshot>, StoreError> {
        let loaded = reported_facts::load(&self.pool, &self.namespace, Some(ids), &self.control)
            .map_err(err)?;
        self.selection = Some(ids.clone());
        Ok(loaded.runs)
    }
    fn load_all_reported_runs(
        &mut self,
    ) -> Result<Vec<crate::facts::ReportedRunSnapshot>, StoreError> {
        let loaded =
            reported_facts::load(&self.pool, &self.namespace, None, &self.control).map_err(err)?;
        self.selection = None;
        Ok(loaded.runs)
    }
    fn save_runs(
        &mut self,
        runs: &[crate::facts::ReportedRunSnapshot],
        stamp: &ReportedFileStamp,
    ) -> Result<Option<Vec<crate::facts::ReportedRunSnapshot>>, StoreError> {
        let loaded = reported_facts::load(
            &self.pool,
            &self.namespace,
            Some(&BTreeSet::new()),
            &self.control,
        )
        .map_err(err)?;
        let bytes = serde_json::to_vec(&(self.namespace.as_str(), loaded.revision, runs, stamp))
            .map_err(|_| err(StorageError::InvalidArguments))?;
        use sha2::{Digest, Sha256};
        let id = format!("{:x}", Sha256::digest(bytes));
        reported_facts::save_selected(
            &self.pool,
            &self.namespace,
            loaded.revision,
            &id,
            runs,
            stamp,
            self.selection.as_ref(),
            &self.control,
        )
        .map(|l| Some(l.runs))
        .map_err(err)
    }
    fn file_ledgers(&self) -> Result<Vec<(String, Option<Ledger>)>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                native_reader::file_ledgers,
            )
            .map_err(err)
    }
    fn remove_files(
        &mut self,
        files: &[(String, Option<Ledger>)],
    ) -> Result<crate::events::IndexWriteResult, StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::catalog_effects::withdrawal::remove(tx, files).inspect_err(|error| {
                    eprintln!(
                        "477-withdrawal dispatch body files={} error={error:?}",
                        files.len()
                    );
                })
            })
            .inspect_err(|error| {
                eprintln!(
                    "477-withdrawal dispatch transaction files={} error={error:?}",
                    files.len()
                );
            })
            .map_err(err)
    }
    fn session_descriptions(
        &self,
        requests: &[SessionDescriptionRequest],
    ) -> Result<Vec<Option<String>>, StoreError> {
        if requests.len() > SESSION_DESCRIPTION_BATCH_LIMIT {
            return Err(StoreError::Configured(StorageError::InvalidArguments));
        }
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::model_cache::descriptions(tx, requests),
            )
            .map_err(err)
    }
    fn save_session_descriptions(
        &mut self,
        descriptions: &[(String, u32, String, String)],
    ) -> Result<Outcome, StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::model_cache::save_descriptions(tx, descriptions)
            })
            .map(|_| Outcome::Written)
            .map_err(err)
    }
    fn session_catalog_generation(&self) -> Result<Option<String>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                super::model_cache::generation,
            )
            .map_err(err)
    }
    fn note_catalog_audit(&mut self, expected: Option<&str>, now: i64) -> Result<bool, StoreError> {
        self.pool.transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
            let ns = self.namespace.as_str().as_bytes();
            tx.driver.query_one("SELECT true FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
            let generation = super::model_cache::generation(tx)?;
            if generation.as_deref() != expected || generation.is_none() { return Ok(false); }
            let ready: bool = tx.driver.query_one("SELECT NOT EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND active)", &[&ns]).map_err(sqlstate::error)?.get(0);
            if ready { super::native_keys::put_meta(tx, b"catalog_observed_at", now.to_string().as_bytes())?; }
            Ok(ready)
        }).map_err(err)
    }
    fn session_slots(
        &self,
        row: &crate::model::summary::CatalogRow,
        expected: Option<&str>,
        max: usize,
    ) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError> {
        let Some(generation) = expected else {
            return Ok(None);
        };
        if max == 0 || max > 64 {
            return Err(err(StorageError::Busy));
        }
        super::catalog_effects::snapshot::with_generation(
            &self.pool,
            &self.namespace,
            &self.control,
            generation,
            |snapshot| {
                let Some((stored, header, slots)) =
                    snapshot.selected(&row.key, false, 0, max as i64)?
                else {
                    return Ok(None);
                };
                if serde_json::to_vec(&stored.sources).ok() != serde_json::to_vec(&row.sources).ok()
                    || header.3 > max as i64
                {
                    return Ok(None);
                }
                slots
                    .iter()
                    .map(|text| serde_json::from_str(text).map_err(|_| StorageError::ScopeChanged))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some)
            },
        )
        .map_err(err)
    }
    fn publish_session_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected: Option<&str>,
        revision_fence: Option<&crate::storage::source_observations::FullCatalogRevisionFence>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::catalog_effects::publication::publish(
                    tx,
                    publication,
                    expected,
                    &[],
                    false,
                    None,
                    revision_fence,
                )
            })
            .map_err(err)
    }
    fn publish_partial_catalog(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::catalog_effects::publication::publish(
                    tx,
                    publication,
                    expected,
                    claims,
                    false,
                    None,
                    None,
                )
            })
            .map_err(err)
    }
    fn publish_closed_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<Option<(Outcome, Option<String>)>, StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::catalog_effects::publication::publish(
                    tx,
                    publication,
                    expected,
                    claims,
                    true,
                    None,
                    None,
                )
                .map(Some)
            })
            .map_err(err)
    }
    fn facts_write_begin(
        &mut self,
        options: &crate::Options,
        intent: &crate::source_change::FactsIntent,
    ) -> Result<(), StoreError> {
        let path = options
            .facts
            .as_ref()
            .ok_or_else(|| err(StorageError::InvalidArguments))?;
        if path.to_string_lossy() != intent.path {
            return Err(err(StorageError::InvalidArguments));
        }
        let actual = match crate::read_facts(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => crate::Facts::default(),
            Err(e) => return Err(StoreError::Data(e.to_string())),
        };
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::facts_begin(tx, intent, &actual)
            })
            .map_err(err)
    }
    fn facts_write_complete(
        &mut self,
        options: &crate::Options,
        intent: &crate::source_change::FactsIntent,
    ) -> Result<(), StoreError> {
        let path = options
            .facts
            .as_ref()
            .ok_or_else(|| err(StorageError::InvalidArguments))?;
        if path.to_string_lossy() != intent.path {
            return Err(err(StorageError::InvalidArguments));
        }
        let actual = crate::read_facts(path).map_err(|e| StoreError::Data(e.to_string()))?;
        let pids = crate::model::pid_files(
            options,
            &crate::facts::MachineFacts::Recorded(Box::new(actual.clone())),
        )
        .into_iter()
        .map(|p| (p.pid, p.session))
        .collect::<Vec<_>>();
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::facts_complete(tx, options, intent, &actual, &pids)
            })
            .map_err(err)
    }
    fn source_write_prepare(
        &self,
        options: &crate::Options,
        input: &crate::Input,
        intent: &mut crate::source_change::WriteIntent,
    ) -> Result<(), StoreError> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let check = || {
            if self.control.is_cancelled() || std::time::Instant::now() >= until {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "source proof cancelled",
                ))
            } else {
                Ok(())
            }
        };
        if let Some(before) = intent.before {
            let continuation = if intent.generation.is_some()
                && intent.from == before.size
                && intent
                    .ledger
                    .as_ref()
                    .is_some_and(|l| l.stat == before && l.offset == before.size)
            {
                let ledger = intent.ledger.as_ref().unwrap();
                self.pool
                    .transaction(
                        &self.namespace,
                        TxnKind::ReadSnapshot,
                        &self.control,
                        |tx| {
                            let pinned = super::native_pages::current(tx, &intent.path, ledger)?;
                            let header: crate::events::native_pages::chunks::Header =
                                serde_json::from_str(&pinned.chunks_header)
                                    .map_err(|_| StorageError::ScopeChanged)?;
                            let state = header.resume.ok_or(StorageError::ScopeChanged)?;
                            Ok(Some(crate::source_change::AppendContinuation {
                                proof: pinned.id.0,
                                state,
                                offset: ledger.offset,
                                prefix: ledger.prefix,
                            }))
                        },
                    )
                    .map_err(err)?
            } else {
                None
            };
            intent.before_hash = Some(if let Some(c) = &continuation {
                let mut file = crate::sealed::LogFile::open(&input.full_path(options))
                    .map_err(|e| StoreError::Data(e.to_string()))?;
                crate::source_chunks::continue_prefix(
                    &mut file, &c.state, c.offset, c.prefix, c.offset, check,
                )
                .map_err(|e| StoreError::Data(e.to_string()))?
            } else {
                crate::source_change::bytes_hash(&input.full_path(options), before.size, check)
                    .map_err(|e| StoreError::Data(e.to_string()))?
            });
            intent.continuation = continuation;
        }
        Ok(())
    }
    fn source_write_proofs(&self) -> bool {
        true
    }
    fn source_write_begin(
        &mut self,
        options: &crate::Options,
        input: &crate::Input,
        intent: &crate::source_change::WriteIntent,
    ) -> Result<(i64, Option<crate::source_change::AppendEvidence>), StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::write_begin(tx, options, input, intent)
            })
            .map_err(err)
    }
    fn source_write_complete(
        &mut self,
        options: &crate::Options,
        input: &crate::Input,
        intent: &crate::source_change::WriteIntent,
        revision: i64,
        completion: &crate::source_change::WriteCompletion,
    ) -> Result<(), StoreError> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let check = || {
            if self.control.is_cancelled() || std::time::Instant::now() >= until {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "source proof cancelled",
                ))
            } else {
                Ok(())
            }
        };
        let mut completion = completion.clone();
        completion.hash = if let Some(c) = &intent.continuation {
            let mut file = crate::sealed::LogFile::open(&input.full_path(options))
                .map_err(|e| StoreError::Data(e.to_string()))?;
            crate::source_chunks::continue_prefix(
                &mut file,
                &c.state,
                c.offset,
                c.prefix,
                completion.to,
                check,
            )
            .map_err(|e| StoreError::Data(e.to_string()))?
        } else {
            crate::source_change::bytes_hash(&input.full_path(options), completion.to, check)
                .map_err(|e| StoreError::Data(e.to_string()))?
        };
        if Stat::of(&input.full_path(options)).ok() != Some(completion.after) {
            return Err(err(StorageError::Conflict));
        }
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::write_complete(
                    tx,
                    options,
                    input,
                    intent,
                    revision,
                    &completion,
                )
            })
            .map_err(err)
    }
    fn resume_runtime_input(
        &mut self,
        options: &crate::Options,
        path: &str,
    ) -> Result<(), StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::resume_runtime(tx, options, path)
            })
            .map_err(err)
    }
    fn resolve_catalog_input(
        &mut self,
        path: &str,
        revision: i64,
        native: Option<(&str, &str)>,
        log: Option<&str>,
    ) -> Result<Outcome, StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::resolve(tx, path, revision, native, log)
            })
            .map_err(err)
    }
    fn relocate_codex(
        &mut self,
        path: &str,
        stat: &Stat,
        id: &str,
        file: &mut crate::sealed::LogFile,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Result<Option<crate::events::RelocationResult>, StoreError> {
        let started = measurements.start();
        let candidate = self
            .pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::source_lifecycle::relocation_candidate(tx, path, id),
            )
            .map_err(err)?;
        let Some(old) = candidate else {
            return Ok(None);
        };
        let prior = self
            .pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| native_reader::ledger(tx, &old),
            )
            .map_err(err)?
            .map(|(_, l)| l);
        let Some(prior) = prior else {
            return Ok(None);
        };
        if stat.size < prior.offset {
            return Ok(None);
        }
        let hashes = crate::events::consumed_hashes(file, prior.offset)
            .map_err(|e| StoreError::Data(e.to_string()))?;
        if hashes != (prior.prefix, prior.tail)
            || Stat::of(std::path::Path::new(path)).ok().as_ref() != Some(stat)
        {
            return Ok(None);
        }
        let mut summary = self.summary(&old, &prior)?;
        summary
            .load_shared_usage()
            .map_err(|e| StoreError::Data(e.to_string()))?;
        summary.stored = None;
        let source_revision_delta = self
            .pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::relocate(tx, &old, path, id, stat, &prior, summary)
            })
            .map_err(err)?;
        measurements.record("relocation_candidate", started, 1);
        Ok(Some(crate::events::RelocationResult {
            old_path: old,
            source_revision_delta,
        }))
    }
    fn append_evidence(
        &self,
        path: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::model_cache::append_evidence(tx, path),
            )
            .map_err(err)
    }
    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        self.pool.transaction(&self.namespace, TxnKind::ReadSnapshot, &self.control, |tx| {
            let Some(generation) = super::model_cache::generation(tx)? else { return Ok(false); };
            let row = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2)", &[&self.namespace.as_str().as_bytes(), &generation.as_bytes()]).map_err(sqlstate::error)?;
            Ok(row.get(0))
        }).map_err(err)
    }
    fn catalog_component_paths(
        &self,
        paths: &[String],
        keys: &[String],
        native_parents: &[(String, String)],
    ) -> Result<Option<Vec<String>>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::source_lifecycle::component_paths(tx, paths, keys, native_parents),
            )
            .map_err(err)
    }
    fn source_continuation_paths(&self) -> Result<Vec<String>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                source_continuation_paths,
            )
            .map_err(err)
    }
    fn source_continuation_snapshot(
        &self,
        paths: &[String],
        complete: bool,
    ) -> Result<Vec<crate::storage::catalog::SourceContinuationCapture>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| source_continuation_snapshot(tx, paths, complete),
            )
            .map_err(err)
    }
    fn runtime_source_cwds(
        &self,
        sources: &[crate::model::summary::CatalogSource],
    ) -> Result<std::collections::BTreeSet<String>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::native_reader::runtime_source_cwds(tx, sources),
            )
            .map_err(err)
    }
    fn query_runtime_binding(
        &self,
    ) -> Result<Option<crate::storage::QueryRuntimeBinding>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                super::model_cache::binding,
            )
            .map(Some)
            .map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_native126_tree_read_preserves_typed_storage_errors() {
        for expected in [
            StorageError::Busy,
            StorageError::Unavailable,
            StorageError::ScopeChanged,
        ] {
            assert!(
                matches!(read_error(std::io::Error::other(expected)), StoreError::Configured(actual) if actual == expected)
            );
        }
        assert!(matches!(
            read_error(std::io::ErrorKind::WouldBlock.into()),
            StoreError::Configured(StorageError::Busy)
        ));
    }
}
