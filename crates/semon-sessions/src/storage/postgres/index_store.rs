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
        Ok(())
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
fn pending<T>() -> Result<T, StoreError> {
    Err(err(StorageError::NamespaceUpgradePending))
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
        // NativeEventStore is one exact operation/path. Do not manufacture a
        // batch operation from multiple independently retained request IDs.
        if files.len() != 1 {
            return pending();
        }
        let (p, l, i) = &files[0];
        self.commit_file(p, None, None, l, i)
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
        self.pool.transaction(
            &self.namespace,
            TxnKind::ReadSnapshot,
            &self.control,
            native_reader::file_ledgers,
        ).map_err(err)
    }
    fn remove_files(&mut self, _: &[(String, Option<Ledger>)]) -> Result<Outcome, StoreError> {
        pending()
    }
    fn session_description(&self, _: &str, _: u32, _: &str) -> Result<Option<String>, StoreError> {
        pending()
    }
    fn save_session_descriptions(
        &mut self,
        _: &[(String, u32, String, String)],
    ) -> Result<Outcome, StoreError> {
        pending()
    }
    fn session_catalog_generation(&self) -> Result<Option<String>, StoreError> {
        pending()
    }
    fn session_slots(
        &self,
        _: &crate::model::summary::CatalogRow,
        _: Option<&str>,
        _: usize,
    ) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError> {
        pending()
    }
    fn publish_session_catalog_fenced(
        &mut self,
        _: &crate::slot_projection::Publication<'_>,
        _: Option<&str>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        pending()
    }
    fn publish_partial_catalog(
        &mut self,
        _: &crate::slot_projection::Publication<'_>,
        _: Option<&str>,
        _: &[(String, Option<i64>)],
    ) -> Result<(Outcome, Option<String>), StoreError> {
        pending()
    }
    fn publish_closed_catalog_fenced(
        &mut self,
        _: &crate::slot_projection::Publication<'_>,
        _: Option<&str>,
        _: &[(String, Option<i64>)],
    ) -> Result<Option<(Outcome, Option<String>)>, StoreError> {
        pending()
    }
    fn resolve_catalog_input(
        &mut self,
        _: &str,
        _: i64,
        _: Option<(&str, &str)>,
        _: Option<&str>,
    ) -> Result<Outcome, StoreError> {
        pending()
    }
    fn relocate_codex(
        &mut self,
        _: &str,
        _: &Stat,
        _: &str,
        _: &mut crate::sealed::LogFile,
        _: &mut crate::native_timing::Measurements,
    ) -> Result<Option<String>, StoreError> {
        pending()
    }
    fn append_evidence(
        &self,
        _: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError> {
        pending()
    }
    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        pending()
    }
    fn catalog_component_paths(
        &self,
        _: &[String],
        _: &[String],
        _: &[(String, String)],
    ) -> Result<Option<Vec<String>>, StoreError> {
        pending()
    }
    fn query_runtime_binding(
        &self,
    ) -> Result<Option<crate::storage::QueryRuntimeBinding>, StoreError> {
        pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_native126_tree_read_preserves_typed_storage_errors() {
        for expected in [StorageError::Busy, StorageError::Unavailable, StorageError::ScopeChanged] {
            assert!(matches!(read_error(std::io::Error::other(expected)), StoreError::Configured(actual) if actual == expected));
        }
        assert!(matches!(read_error(std::io::ErrorKind::WouldBlock.into()), StoreError::Configured(StorageError::Busy)));
    }
}
