//! Private native pipeline injection. Public complete-backend selection is closed.
use super::{
    PgCancellation, PgPool, PgSourceJournal, native_ingest, native_reader, reported_facts,
    sqlstate, txn::TxnKind,
};
use crate::{
    events::{
        DirtyRows, FileIndex, IndexStore, Ledger, Outcome, ReportedFileStamp, Stat, StoreError,
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
    let journal = PgSourceJournal {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
    };
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
        .map_err(read_error)
    }
}

impl IndexStore for NativeIndex {
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
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::semantic_stage::next(tx, limit),
            )
            .map_err(err)
    }
    fn record_semantic_batch(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
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
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::semantic_stage::publish(tx, batch, prepared)
            })
            .map_err(err)
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
    fn describe(&self) -> String {
        "private PostgreSQL native index".to_owned()
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
    fn load_file(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError> {
        let loaded = self
            .pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| native_reader::file(tx, path),
            )
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
        index: &FileIndex,
    ) -> Result<Outcome, StoreError> {
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
            index,
        )
        .map_err(err)?;
        Ok(Outcome::Written)
    }
    fn commit_initial_files(
        &mut self,
        files: &[(String, Ledger, Arc<FileIndex>)],
    ) -> Result<Outcome, StoreError> {
        native_ingest::commit_initial(
            &self.pool,
            &self.namespace,
            &self.control,
            &self.intents,
            files,
        )
        .map_err(err)?;
        Ok(Outcome::Written)
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
    fn remove_files(&mut self, files: &[(String, Option<Ledger>)]) -> Result<Outcome, StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::catalog_effects::withdrawal::remove(tx, files)
            })
            .map_err(err)
    }
    fn session_description(
        &self,
        key: &str,
        version: u32,
        inputs: &str,
    ) -> Result<Option<String>, StoreError> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| super::model_cache::description(tx, key, version, inputs),
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
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::catalog_effects::publication::publish(tx, publication, expected, &[], false)
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
    ) -> Result<Option<String>, StoreError> {
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
        let result = self
            .pool
            .transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
                super::source_lifecycle::relocate(tx, &old, path, id, stat, &prior, summary)
            })
            .map_err(err)?;
        measurements.record("relocation_candidate", started, 1);
        Ok(result.then_some(old))
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
