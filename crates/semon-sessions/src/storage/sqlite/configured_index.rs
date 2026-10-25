//! Explicit configured IndexStore. Unported families still refuse.
use crate::events::{
    DirtyRows, FileIndex, IndexStore, Ledger, Outcome, ReportedFileStamp, Stat, StoreError,
};
use crate::sealed::LogFile;
use crate::storage::source_observations::SourceObservationStore;
use crate::storage::{StorageError, StoreConfig};
use std::{collections::BTreeSet, io, sync::Arc};

pub(super) struct ConfiguredIndex {
    journal: super::source_journal::SourceJournal,
    native: crate::events::store::configured::Writer,
    semantic: Option<super::semantic::Stage>,
}

pub(crate) fn open(config: &StoreConfig) -> Result<Box<dyn IndexStore>, StorageError> {
    crate::events::store::configured::validate_semantic_schema(config)?;
    let journal = super::source_journal::SourceJournal(config.clone());
    journal.prepare()?;
    Ok(Box::new(ConfiguredIndex {
        journal,
        native: crate::events::store::configured::Writer::new(config),
        semantic: None,
    }))
}

/// Crate-private capability only; the public configured factory stays closed.
#[allow(dead_code)] // Internal runtime seam; public qualification remains zero.
pub(crate) fn open_private_semantic(
    config: &StoreConfig,
) -> Result<Box<dyn IndexStore>, StorageError> {
    open_semantic(super::semantic::SemanticStaging::primitive(config)?)
}

#[allow(dead_code)] // Requires an explicit validated capability, never Options.
pub(crate) fn open_semantic(
    capability: super::semantic::SemanticStaging,
) -> Result<Box<dyn IndexStore>, StorageError> {
    let journal = capability.journal();
    journal.prepare()?;
    let native = capability.writer();
    Ok(Box::new(ConfiguredIndex {
        journal,
        native,
        semantic: Some(super::semantic::Stage::open(capability)?),
    }))
}

impl IndexStore for ConfiguredIndex {
    fn query_runtime_binding(
        &self,
    ) -> Result<Option<crate::storage::QueryRuntimeBinding>, StoreError> {
        self.semantic
            .as_ref()
            .ok_or(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ))?
            .runtime_binding()
            .map(Some)
    }
    fn prepare_file(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        operation_id: &str,
        harness: &str,
    ) -> Result<(), StoreError> {
        self.native
            .begin(path, expected, operation_id, harness)
            .map_err(StoreError::Configured)
    }
    fn reconcile_event(
        &self,
        operation_id: &str,
        path: &str,
        harness: &str,
    ) -> Result<Option<crate::storage::native_events::EventReceipt>, StoreError> {
        self.native
            .reconcile(operation_id, path, harness)
            .map_err(StoreError::Configured)
    }
    fn event_receipt(
        &self,
        path: &str,
    ) -> Result<crate::storage::native_events::EventReceipt, StoreError> {
        self.native.receipt(path).map_err(StoreError::Configured)
    }

    fn invalidate_catalog_sources(
        &mut self,
        expected: &[(String, Option<i64>)],
    ) -> Result<bool, StoreError> {
        let payload = serde_json::to_vec(expected)
            .map_err(|_| StoreError::Configured(StorageError::InvalidArguments))?;
        let operation = self.journal.operation("invalidate", &payload);
        self.journal
            .invalidate(&operation, expected)
            .map(|receipt| {
                receipt != crate::storage::source_observations::SourceMutationReceipt::Conflict
            })
            .map_err(StoreError::Configured)
    }
    fn describe(&self) -> String {
        "configured source observations".to_owned()
    }
    fn session_description(
        &self,
        _key: &str,
        _version: u32,
        _inputs: &str,
    ) -> Result<Option<String>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn save_session_descriptions(
        &mut self,
        _descriptions: &[(String, u32, String, String)],
    ) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn session_catalog_generation(&self) -> Result<Option<String>, StoreError> {
        if let Some(stage) = &self.semantic {
            return stage.generation();
        }
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn session_slots(
        &self,
        _row: &crate::model::summary::CatalogRow,
        _expected_generation: Option<&str>,
        _max: usize,
    ) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn publish_session_catalog_fenced(
        &mut self,
        _publication: &crate::slot_projection::Publication<'_>,
        _expected_generation: Option<&str>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn publish_partial_catalog(
        &mut self,
        _publication: &crate::slot_projection::Publication<'_>,
        _expected_generation: Option<&str>,
        _claims: &[(String, Option<i64>)],
    ) -> Result<(Outcome, Option<String>), StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn publish_closed_catalog_fenced(
        &mut self,
        _publication: &crate::slot_projection::Publication<'_>,
        _expected_generation: Option<&str>,
        _claims: &[(String, Option<i64>)],
    ) -> Result<Option<(Outcome, Option<String>)>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn backfill_state(&self) -> Result<Option<String>, StoreError> {
        self.journal
            .backfill_state()
            .map_err(StoreError::Configured)
    }
    fn discover_catalog_sources(
        &mut self,
        paths: &[String],
        complete: bool,
    ) -> Result<(), StoreError> {
        let payload = serde_json::to_vec(&(paths, complete))
            .map_err(|_| StoreError::Configured(StorageError::InvalidArguments))?;
        let operation = self.journal.operation("discover", &payload);
        match SourceObservationStore::discover(&mut self.journal, &operation, paths, complete)
            .map_err(StoreError::Configured)?
        {
            crate::storage::source_observations::SourceMutationReceipt::Applied
            | crate::storage::source_observations::SourceMutationReceipt::AlreadyApplied => Ok(()),
            crate::storage::source_observations::SourceMutationReceipt::Conflict => {
                Err(StoreError::Configured(StorageError::Conflict))
            }
        }
    }
    fn finish_catalog_backfill(&mut self, expected: Option<&str>) -> Result<Outcome, StoreError> {
        let payload = serde_json::to_vec(&expected)
            .map_err(|_| StoreError::Configured(StorageError::InvalidArguments))?;
        let operation = self.journal.operation("finish_backfill", &payload);
        match SourceObservationStore::finish_backfill(&mut self.journal, &operation, expected)
            .map_err(StoreError::Configured)?
        {
            crate::storage::source_observations::SourceMutationReceipt::Applied
            | crate::storage::source_observations::SourceMutationReceipt::AlreadyApplied => {
                Ok(Outcome::Written)
            }
            crate::storage::source_observations::SourceMutationReceipt::Conflict => {
                Ok(Outcome::Conflict)
            }
        }
    }
    fn resolve_catalog_input(
        &mut self,
        _path: &str,
        _revision: i64,
        _native: Option<(&str, &str)>,
        _log: Option<&str>,
    ) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn relocate_codex(
        &mut self,
        _path: &str,
        _stat: &Stat,
        _id: &str,
        _file: &mut LogFile,
        _measurements: &mut crate::native_timing::Measurements,
    ) -> Result<Option<String>, StoreError> {
        // Cross-path relocation admission is unported; ordinary parsing still
        // commits its relocation advance/dependency effects atomically.
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn append_evidence(
        &self,
        _path: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn pending_catalog_sources(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        self.journal.pending(limit).map_err(StoreError::Configured)
    }
    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn catalog_claims(&self, paths: &[String]) -> Result<Vec<(String, Option<i64>)>, StoreError> {
        self.journal.claims(paths).map_err(StoreError::Configured)
    }
    fn catalog_component_paths(
        &self,
        _paths: &[String],
        _keys: &[String],
        _native_parents: &[(String, String)],
    ) -> Result<Option<Vec<String>>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn begin_semantic_stage(
        &mut self,
        paths: &[String],
        runtime: &str,
        _journaled_runtime: bool,
    ) -> Result<Outcome, StoreError> {
        if let Some(stage) = &mut self.semantic {
            return stage.begin(paths, runtime);
        }
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn semantic_batch(
        &mut self,
        limit: usize,
    ) -> Result<Option<crate::model::relation_stage::Batch>, StoreError> {
        if let Some(stage) = &mut self.semantic {
            return stage.batch(limit);
        }
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn record_semantic_batch(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
        if let Some(stage) = &mut self.semantic {
            return stage.record(batch, prepared);
        }
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn semantic_rejection(&self) -> Option<&'static str> {
        if self.semantic.is_some() {
            None
        } else {
            Some("namespace_upgrade_pending")
        }
    }
    fn publish_semantic_catalog(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.semantic
            .as_mut()
            .ok_or(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ))?
            .publish(batch, prepared)
    }
    fn finish_semantic_stage(&mut self, epoch: i64) -> Result<Outcome, StoreError> {
        self.semantic
            .as_mut()
            .ok_or(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ))?
            .finish(epoch)
    }
    fn semantic_select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        self.semantic
            .as_mut()
            .ok_or(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ))?
            .select(epoch, selector)
    }
    fn semantic_replay_page(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<crate::model::relation_stage::Context, StoreError> {
        self.semantic
            .as_mut()
            .ok_or(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ))?
            .replay(epoch, owners, after)
    }
    fn ledger(&self, path: &str) -> Result<Option<Ledger>, StoreError> {
        self.native.ledger(path).map_err(StoreError::Configured)
    }
    fn load_file(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError> {
        self.native.load(path).map_err(StoreError::Configured)
    }
    fn native_usage_seen(
        &self,
        path: &str,
        ledger: &Ledger,
        identity: &str,
    ) -> Result<crate::events::store::native_usage::Seen, StoreError> {
        self.native
            .seen(path, ledger, identity)
            .map_err(StoreError::Configured)
    }
    fn advance_tree_repair(
        &mut self,
        _options: &crate::Options,
    ) -> io::Result<(bool, Option<String>)> {
        Err(io::Error::other(
            StorageError::NamespaceUpgradePending.to_string(),
        ))
    }
    fn tree_repair_pending(&self) -> Result<bool, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn queue_tree_repair(
        &mut self,
        _path: &str,
        _ledger: &Ledger,
        _harness: &str,
    ) -> io::Result<()> {
        Err(io::Error::other(
            StorageError::NamespaceUpgradePending.to_string(),
        ))
    }
    fn load_tree(
        &self,
        path: &str,
        expected: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StoreError> {
        self.native
            .tree(path, expected)
            .map_err(StoreError::Configured)
    }
    fn commit_initial_files(
        &mut self,
        files: &[(String, Ledger, Arc<FileIndex>)],
    ) -> Result<Outcome, StoreError> {
        if files
            .iter()
            .any(|(_, ledger, _)| ledger.offset != ledger.stat.size)
        {
            return Err(StoreError::Configured(StorageError::InvalidArguments));
        }
        let files = files
            .iter()
            .map(|(path, ledger, index)| (path.as_str(), None, None, ledger, index.as_ref()))
            .collect::<Vec<_>>();
        self.native.commit(&files).map_err(StoreError::Configured)
    }
    fn commit_file(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        changes: Option<&DirtyRows>,
        ledger: &Ledger,
        index: &FileIndex,
    ) -> Result<Outcome, StoreError> {
        self.native
            .commit(&[(path, expected, changes, ledger, index)])
            .map_err(StoreError::Configured)
    }
    fn select_reported_runs(
        &mut self,
        _ids: &BTreeSet<String>,
    ) -> Result<Vec<crate::facts::ReportedRunSnapshot>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn file_ledgers(&self) -> Result<Vec<(String, Option<Ledger>)>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn remove_files(&mut self, _files: &[(String, Option<Ledger>)]) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn save_runs(
        &mut self,
        _runs: &[crate::facts::ReportedRunSnapshot],
        _stamp: &ReportedFileStamp,
    ) -> Result<Option<Vec<crate::facts::ReportedRunSnapshot>>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
}
