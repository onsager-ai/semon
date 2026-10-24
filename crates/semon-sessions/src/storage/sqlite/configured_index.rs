//! Explicit configured IndexStore. Only source observation operations qualify.
use crate::events::{
    DirtyRows, FileIndex, IndexStore, Ledger, Outcome, ReportedFileStamp, Stat, StoreError,
};
use crate::sealed::LogFile;
use crate::storage::source_observations::SourceObservationStore;
use crate::storage::{StorageError, StoreConfig};
use std::{collections::BTreeSet, io, sync::Arc};

pub(super) struct ConfiguredIndex(pub(super) super::source_journal::SourceJournal);

pub(crate) fn open(config: &StoreConfig) -> Result<Box<dyn IndexStore>, StorageError> {
    let journal = super::source_journal::SourceJournal(config.clone());
    journal.prepare()?;
    Ok(Box::new(ConfiguredIndex(journal)))
}

impl IndexStore for ConfiguredIndex {
    fn invalidate_catalog_sources(
        &mut self,
        expected: &[(String, Option<i64>)],
    ) -> Result<bool, StoreError> {
        self.0.invalidate(expected).map_err(StoreError::Configured)
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
        self.0.backfill_state().map_err(StoreError::Configured)
    }
    fn discover_catalog_sources(
        &mut self,
        paths: &[String],
        complete: bool,
    ) -> Result<(), StoreError> {
        self.0
            .discover(paths, complete)
            .map_err(StoreError::Configured)
    }
    fn finish_catalog_backfill(&mut self, _expected: Option<&str>) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
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
        self.0.pending(limit).map_err(StoreError::Configured)
    }
    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn catalog_claims(&self, paths: &[String]) -> Result<Vec<(String, Option<i64>)>, StoreError> {
        self.0.claims(paths).map_err(StoreError::Configured)
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
        _paths: &[String],
        _runtime: &str,
        _journaled_runtime: bool,
    ) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn semantic_batch(
        &mut self,
        _limit: usize,
    ) -> Result<Option<crate::model::relation_stage::Batch>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn record_semantic_batch(
        &mut self,
        _batch: &crate::model::relation_stage::Batch,
        _prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn semantic_rejection(&self) -> Option<&'static str> {
        Some("namespace_upgrade_pending")
    }
    fn publish_semantic_catalog(
        &mut self,
        _batch: &crate::model::relation_stage::Batch,
        _prepared: &crate::model::PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn finish_semantic_stage(&mut self, _epoch: i64) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn semantic_select(
        &mut self,
        _epoch: i64,
        _selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn semantic_replay_page(
        &mut self,
        _epoch: i64,
        _owners: &[String],
        _after: &str,
    ) -> Result<crate::model::relation_stage::Context, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn ledger(&self, _path: &str) -> Result<Option<Ledger>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn load_file(&self, _path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn native_usage_seen(
        &self,
        _path: &str,
        _ledger: &Ledger,
        _identity: &str,
    ) -> Result<crate::events::store::native_usage::Seen, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
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
        _path: &str,
        _expected: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn commit_initial_files(
        &mut self,
        _files: &[(String, Ledger, Arc<FileIndex>)],
    ) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
    }
    fn commit_file(
        &mut self,
        _path: &str,
        _expected: Option<&Ledger>,
        _changes: Option<&DirtyRows>,
        _ledger: &Ledger,
        _index: &FileIndex,
    ) -> Result<Outcome, StoreError> {
        Err(StoreError::Configured(
            StorageError::NamespaceUpgradePending,
        ))
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
