//! SQLite physical native-commit stages. The semantic sequence is shared.
use super::*;
use crate::events::native_commit::{
    self as shared, Effects, Input, RelationshipGroup, Relationships,
};

pub(super) struct Writer<'a, 'tx, 'conn> {
    pub(super) transaction: &'tx Transaction<'conn>,
    pub(super) layout: &'a WriterLayout<'tx, 'conn>,
    pub(super) measurements: &'a mut crate::native_timing::Measurements,
    pub(super) namespace: Option<&'a str>,
    pub(super) usage_deletes: &'a mut UsageDeletes<'tx>,
    pub(super) relationship_started: Option<Instant>,
    pub(super) relationship_rows_before: Option<u64>,
    pub(super) dirty_started: Option<Instant>,
}

impl Relationships for Writer<'_, '_, '_> {
    type Error = rusqlite::Error;
    fn metadata_error(_: serde_json::Error) -> Self::Error {
        rusqlite::Error::InvalidQuery
    }
    fn clear(&mut self, path: &str, group: RelationshipGroup) -> rusqlite::Result<()> {
        self.relationship_started = self.measurements.start();
        self.relationship_rows_before = self
            .relationship_started
            .map(|_| self.transaction.total_changes());
        let (singleton, composite) = match group {
            RelationshipGroup::Lineage => (
                "DELETE FROM catalog_native_dependencies WHERE source_path=?1 AND (harness LIKE 'lineage:%' OR harness LIKE 'bridge:%')",
                "DELETE FROM catalog_native_dependencies WHERE namespace=semon_namespace() AND source_path=?1 AND (harness LIKE 'lineage:%' OR harness LIKE 'bridge:%')",
            ),
            RelationshipGroup::Semantic => (
                "DELETE FROM catalog_native_dependencies WHERE source_path=?1 AND harness IN ('semantic-native:codex','semantic-parent:codex')",
                "DELETE FROM catalog_native_dependencies WHERE namespace=semon_namespace() AND source_path=?1 AND harness IN ('semantic-native:codex','semantic-parent:codex')",
            ),
        };
        self.transaction
            .execute(self.layout.support(singleton, composite), [path])?;
        Ok(())
    }
    fn put(&mut self, path: &str, harness: &str, id: &str) -> rusqlite::Result<()> {
        self.transaction.execute(self.layout.support(
            "INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,?2,?3)",
            "INSERT OR IGNORE INTO catalog_native_dependencies(namespace,source_path,harness,native_id) VALUES(semon_namespace(),?1,?2,?3)"), params![path,harness,id])?;
        Ok(())
    }
    fn finished(&mut self, group: RelationshipGroup) -> rusqlite::Result<()> {
        let (label, rows) = match group {
            RelationshipGroup::Lineage => ("dependencies", "dependencies_written_rows"),
            RelationshipGroup::Semantic => (
                "semantic_dependencies",
                "semantic_dependencies_written_rows",
            ),
        };
        self.measurements
            .record(label, self.relationship_started, 1);
        record_writer_rows(
            self.transaction,
            self.measurements,
            rows,
            self.relationship_rows_before,
        );
        #[cfg(test)]
        configured::effect(label)?;
        Ok(())
    }
}

impl Effects for Writer<'_, '_, '_> {
    type Error = rusqlite::Error;
    fn planning_started(&mut self) {
        self.dirty_started = self.measurements.start();
    }
    fn planning_finished(&mut self) {
        self.measurements
            .record("dirty_rows", self.dirty_started, 1);
    }
    fn begin(&mut self, input: &Input<'_>) -> rusqlite::Result<()> {
        let transaction = self.transaction;
        let measurements = &mut *self.measurements;
        let Input {
            path,
            ledger,
            changes,
            expected,
            ..
        } = *input;
        let started = measurements.start();
        let rows_before = started.map(|_| transaction.total_changes());
        relocation::advance(transaction, path, expected, ledger, changes.is_some())?;
        native_usage::begin_source(transaction, path, expected, ledger, changes.is_some())?;
        measurements.record("relocation", started, 1);
        record_writer_rows(
            transaction,
            measurements,
            "relocation_written_rows",
            rows_before,
        );
        #[cfg(test)]
        configured::effect("relocation")?;
        Ok(())
    }
    fn file(
        &mut self,
        input: &Input<'_>,
        changes: &super::super::DirtyRows,
    ) -> rusqlite::Result<()> {
        let measurements = &mut *self.measurements;
        let Input {
            path,
            ledger,
            index,
            ..
        } = *input;
        let usage_deletes = &mut *self.usage_deletes;
        write_file_with_usage_deletes(
            path,
            ledger,
            changes,
            index,
            measurements,
            self.layout,
            usage_deletes,
        )?;
        Ok(())
    }
    fn usage(&mut self, input: &Input<'_>) -> rusqlite::Result<()> {
        let Input {
            path,
            ledger,
            index,
            ..
        } = *input;
        let transaction = self.transaction;
        native_usage::write(transaction, path, ledger, index)?;
        #[cfg(test)]
        configured::effect("native_usage")?;
        Ok(())
    }
    fn tree(&mut self, input: &Input<'_>) -> rusqlite::Result<()> {
        write_native_tree(self.transaction, input.path, input.ledger, input.index, self.measurements)
    }
    fn auxiliary(&mut self, input: &Input<'_>, stage: shared::Auxiliary) -> rusqlite::Result<()> {
        write_native_aux(self.transaction, input.path, input.ledger, input.index, self.measurements, stage)
    }
    fn relationships(&mut self, input: &Input<'_>) -> rusqlite::Result<()> {
        shared::relationships(self, input.path, input.index)
    }
    fn archive(&mut self, path: &str) -> rusqlite::Result<()> {
        let transaction = self.transaction;
        let measurements = &mut *self.measurements;
        let started = measurements.start();
        transaction.execute(
            self.layout.meta(
                "DELETE FROM meta WHERE key=?1",
                "DELETE FROM meta WHERE namespace=semon_meta_scope(?1) AND key=?1",
            ),
            [archive_index_key(path)],
        )?;
        measurements.record("archive_metadata", started, 1);
        #[cfg(test)]
        configured::effect("archive_metadata")?;
        Ok(())
    }
    fn invalidate(&mut self, path: &str, semantic_change: bool) -> rusqlite::Result<()> {
        let transaction = self.transaction;
        let measurements = &mut *self.measurements;
        let namespace = self.namespace;
        let started = measurements.start();
        if semantic_change {
            crate::storage::sqlite::source_journal::invalidate(
                transaction,
                namespace,
                path,
                "changed",
            )?;
            measurements.record("invalidation", started, 1);
            #[cfg(test)]
            configured::effect("invalidation")?;
        } else {
            measurements.record("representation_repair", started, 1);
        }
        Ok(())
    }
    fn retire_mutation(&mut self, path: &str) -> rusqlite::Result<()> {
        let transaction = self.transaction;
        let measurements = &mut *self.measurements;
        let started = measurements.start();
        let rows_before = started.map(|_| transaction.total_changes());
        transaction.execute(
        self.layout.support(
            "DELETE FROM session_source_mutations WHERE source_path=?1",
            "DELETE FROM session_source_mutations WHERE namespace=semon_namespace() AND source_path=?1"),
        [path],
    )?;
        measurements.record("mutation_retirement", started, 1);
        record_writer_rows(
            transaction,
            measurements,
            "mutation_retirement_written_rows",
            rows_before,
        );
        #[cfg(test)]
        configured::effect("mutation_retirement")?;
        Ok(())
    }
}

/// Independent oracle executes the production parser, EventCache and IndexStore
/// before the PG phase. No SQLite connection is returned to that phase.
#[cfg(all(test, feature = "postgres"))]
pub(crate) fn parser_oracle(
    cache_path: &Path,
    source: &Path,
    harness: &str,
) -> (FileIndex, Vec<(String, String)>) {
    let mut cache = super::super::EventCache::open(cache_path).unwrap();
    let index = super::super::scan_file(source, harness, &mut cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    drop(cache);
    let connection = Connection::open(super::super::EventCache::path(cache_path)).unwrap();
    let mut statement = connection.prepare("SELECT harness,native_id FROM catalog_native_dependencies WHERE source_path=?1 ORDER BY harness COLLATE BINARY,native_id COLLATE BINARY").unwrap();
    let rows = statement
        .query_map([source.to_str().unwrap()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    ((*index).clone(), rows)
}
