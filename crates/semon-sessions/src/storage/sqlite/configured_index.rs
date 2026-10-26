//! Explicit configured IndexStore. Unported families still refuse.
use crate::events::store::{
    begin_source_write, complete_source_write, enqueue_facts, enqueue_native, enqueue_pid,
    resume_runtime_input,
};
use crate::events::{
    DirtyRows, FileIndex, IndexStore, Ledger, Outcome, ReportedFileStamp, Stat, StoreError,
};
use crate::sealed::LogFile;
use crate::storage::{StorageError, StoreConfig};
use std::path::Path;
use std::{collections::BTreeSet, io, sync::Arc};

pub(super) struct ConfiguredIndex {
    reported_selection: Option<BTreeSet<String>>,
    native: crate::events::store::configured::Writer,
    semantic: Option<super::semantic::Stage>,
    journal: super::source_journal::SourceJournal,
}

pub(crate) fn open(config: &StoreConfig) -> Result<Box<dyn IndexStore>, StorageError> {
    crate::events::store::configured::validate_semantic_schema(config)?;
    let journal = super::source_journal::SourceJournal(config.clone());
    journal.prepare()?;
    Ok(Box::new(ConfiguredIndex {
        reported_selection: None,
        native: crate::events::store::configured::Writer::new(config)?,
        semantic: None,
        journal,
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
    let native = capability.writer()?;
    Ok(Box::new(ConfiguredIndex {
        reported_selection: None,
        native,
        semantic: Some(super::semantic::Stage::open(capability)?),
        journal,
    }))
}

pub(crate) fn open_semantic_with_stamp(
    capability: super::semantic::SemanticStaging,
) -> Result<(Box<dyn IndexStore>, Option<ReportedFileStamp>), StorageError> {
    let stamp = capability.reported_stamp()?;
    Ok((open_semantic(capability)?, stamp))
}

impl IndexStore for ConfiguredIndex {
    fn facts_authority(&self, path: &Path) -> io::Result<crate::facts::source_authority::Sources> {
        crate::facts::source_authority::Sources::sqlite(path)
    }
    fn facts_write_begin(
        &mut self,
        options: &crate::Options,
        intent: &crate::source_change::FactsIntent,
    ) -> Result<(), StoreError> {
        enqueue_facts(options, &intent.before, &intent.wanted, intent.push_driven)
            .map_err(|e| StoreError::Data(e.to_string()))
    }
    fn facts_write_complete(
        &mut self,
        options: &crate::Options,
        intent: &crate::source_change::FactsIntent,
    ) -> Result<(), StoreError> {
        enqueue_facts(options, &intent.before, &intent.wanted, intent.push_driven)
            .map_err(|e| StoreError::Data(e.to_string()))
    }
    fn source_write_begin(
        &mut self,
        options: &crate::Options,
        input: &crate::Input,
        _: &crate::source_change::WriteIntent,
    ) -> Result<(i64, Option<crate::source_change::AppendEvidence>), StoreError> {
        let path = input.full_path(options).to_string_lossy().into_owned();
        resume_runtime_input(options, &path).map_err(|e| StoreError::Data(e.to_string()))?;
        let state =
            begin_source_write(options, input).map_err(|e| StoreError::Data(e.to_string()))?;
        if input.root == crate::InputRoot::Claude && input.path.starts_with("sessions/") {
            let id = crate::read_regular_at_most(&input.full_path(options), 65536)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v["sessionId"].as_str().map(str::to_owned));
            if let Some(id) = id {
                enqueue_native(options, "claude", &id)
                    .map_err(|e| StoreError::Data(e.to_string()))?;
            }
            if let Some(pid) = Path::new(&input.path)
                .file_stem()
                .and_then(|p| p.to_str())
                .and_then(|p| p.parse::<u32>().ok())
            {
                enqueue_pid(options, pid).map_err(|e| StoreError::Data(e.to_string()))?;
            }
        }
        Ok(state)
    }
    fn source_write_complete(
        &mut self,
        options: &crate::Options,
        input: &crate::Input,
        _: &crate::source_change::WriteIntent,
        revision: i64,
        completion: &crate::source_change::WriteCompletion,
    ) -> Result<(), StoreError> {
        complete_source_write(options, input, revision, completion.evidence.as_ref())
            .map_err(|e| StoreError::Data(e.to_string()))
    }

    fn resume_runtime_input(
        &mut self,
        options: &crate::Options,
        path: &str,
    ) -> Result<(), StoreError> {
        resume_runtime_input(options, path).map_err(|e| StoreError::Data(e.to_string()))
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

    fn session_description(
        &self,
        key: &str,
        version: u32,
        inputs: &str,
    ) -> Result<Option<String>, StoreError> {
        self.native
            .session_description(key, version, inputs)
            .map_err(StoreError::Configured)
    }
    fn save_session_descriptions(
        &mut self,
        descriptions: &[(String, u32, String, String)],
    ) -> Result<Outcome, StoreError> {
        self.native
            .save_session_descriptions(descriptions)
            .map_err(StoreError::Configured)
    }
    fn session_catalog_generation(&self) -> Result<Option<String>, StoreError> {
        self.native.catalog_generation()
    }
    fn note_catalog_audit(&mut self, expected: Option<&str>, now: i64) -> Result<bool, StoreError> {
        self.native.note_catalog_audit(expected, now)
    }
    fn session_slots(
        &self,
        row: &crate::model::summary::CatalogRow,
        expected_generation: Option<&str>,
        max: usize,
    ) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError> {
        self.native.slots(row, expected_generation, max)
    }
    fn publish_session_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.native
            .publish_catalog(publication, expected_generation, None, false)
            .and_then(|result| result.ok_or(StoreError::Configured(StorageError::Unavailable)))
    }
    fn publish_partial_catalog(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.native
            .publish_catalog(publication, expected_generation, Some(claims), false)
            .and_then(|result| result.ok_or(StoreError::Configured(StorageError::Unavailable)))
    }
    fn publish_closed_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<Option<(Outcome, Option<String>)>, StoreError> {
        self.native
            .publish_catalog(publication, expected_generation, Some(claims), true)
    }
    fn resolve_catalog_input(
        &mut self,
        path: &str,
        revision: i64,
        native: Option<(&str, &str)>,
        log: Option<&str>,
    ) -> Result<Outcome, StoreError> {
        self.native
            .resolve_catalog_input(path, revision, native, log)
    }
    fn relocate_codex(
        &mut self,
        path: &str,
        stat: &Stat,
        id: &str,
        file: &mut LogFile,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Result<Option<String>, StoreError> {
        self.native
            .relocate_codex(path, stat, id, file, measurements)
    }
    fn append_evidence(
        &self,
        path: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError> {
        self.native
            .append_evidence(path)
            .map_err(StoreError::Configured)
    }
    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        self.native.qualified_catalog()
    }
    fn catalog_component_paths(
        &self,
        paths: &[String],
        keys: &[String],
        native_parents: &[(String, String)],
    ) -> Result<Option<Vec<String>>, StoreError> {
        self.native
            .catalog_component_paths(paths, keys, native_parents)
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
        options: &crate::Options,
    ) -> io::Result<(bool, Option<String>)> {
        self.native.advance_tree_repair(options)
    }
    fn tree_repair_pending(&self) -> Result<bool, StoreError> {
        self.native.tree_repair_pending()
    }
    fn queue_tree_repair(&mut self, path: &str, ledger: &Ledger, _harness: &str) -> io::Result<()> {
        self.native.queue_tree_repair(path, ledger)
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
        if !crate::events::native_commit::initial_batch(files)
            .map_err(|_| StoreError::Configured(StorageError::InvalidArguments))?
        {
            return Ok(Outcome::Conflict);
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
        ids: &BTreeSet<String>,
    ) -> Result<Vec<crate::facts::ReportedRunSnapshot>, StoreError> {
        let runs = self
            .native
            .reported_runs(ids)
            .map_err(StoreError::Configured)?;
        self.reported_selection = Some(ids.clone());
        Ok(runs)
    }
    fn file_ledgers(&self) -> Result<Vec<(String, Option<Ledger>)>, StoreError> {
        self.native.file_ledgers()
    }
    fn remove_files(&mut self, files: &[(String, Option<Ledger>)]) -> Result<Outcome, StoreError> {
        self.native.remove_files(files)
    }
    fn save_runs(
        &mut self,
        runs: &[crate::facts::ReportedRunSnapshot],
        stamp: &ReportedFileStamp,
    ) -> Result<Option<Vec<crate::facts::ReportedRunSnapshot>>, StoreError> {
        self.native
            .save_reported_runs(runs, stamp, self.reported_selection.as_ref())
            .map(Some)
            .map_err(StoreError::Configured)
    }
}
