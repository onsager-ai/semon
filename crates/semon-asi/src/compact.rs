//! Tiered compaction (asi-store.md, Generations, publication and versions).
//!
//! Cycle files are level 0. When a table has more than `level0_max_files` of them they
//! are merged into level-1 files. A level-1 file is rewritten only when more than
//! `dead_row_percent` of its rows are dead. Only live rows are copied, and they keep
//! their `_gen`, so the live map is untouched and the live rows are the same before
//! and after. Compaction publishes as another generation; a run with nothing to do
//! publishes nothing.

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use arrow::array::{BooleanArray, TimestampMillisecondArray, UInt32Array};
use arrow::compute::{concat_batches, filter_record_batch, take_record_batch};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use crate::error::{Error, Result};
use crate::parquet_io::{OutFile, int_column, read_batches, string_column};
use crate::schema::{GEN_COLUMN, Kind, SESSION_COLUMN, TABLES, Table};
use crate::store::{Clock, FileEntry, Staged, Store, sync_parents};

#[derive(Clone, Debug)]
pub struct CompactionPolicy {
    /// Merge level-0 files once a table has more than this many.
    pub level0_max_files: usize,
    /// A level-1 file is closed once it holds about this many bytes.
    pub level1_max_bytes: u64,
    /// Rewrite a level-1 file when more than this share of its rows is dead.
    pub dead_row_percent: u64,
    /// Rows written per Parquet write call.
    pub chunk_rows: usize,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            level0_max_files: 32,
            level1_max_bytes: 128 * 1024 * 1024,
            dead_row_percent: 30,
            chunk_rows: 8192,
        }
    }
}

impl<C: Clock> Store<C> {
    /// Compacts every table that needs it into one new generation, and returns its
    /// number, or `None` when there was nothing to do.
    pub fn compact(&mut self, policy: &CompactionPolicy) -> Result<Option<u64>> {
        let generation = self.manifest.generation + 1;
        let mut dropped: HashSet<String> = HashSet::new();
        let mut added: Vec<FileEntry> = Vec::new();
        for table in TABLES {
            if table.kind == Kind::Dimension {
                continue;
            }
            let inputs = self.compaction_inputs(table, policy)?;
            if inputs.is_empty() {
                continue;
            }
            added.extend(self.merge(table, &inputs, generation, policy)?);
            dropped.extend(inputs.into_iter().map(|file| file.path));
        }
        if dropped.is_empty() {
            return Ok(None);
        }

        sync_parents(&self.analytics, added.iter().map(|file| file.path.as_str()))?;
        let mut files: Vec<FileEntry> = self
            .manifest
            .files
            .iter()
            .filter(|file| !dropped.contains(&file.path))
            .cloned()
            .collect();
        files.extend(added);
        let mut manifest = self.manifest.clone();
        manifest.generation = generation;
        manifest.unpublished = self.unpublish_dropped(&files, manifest.live_map.as_deref());
        manifest.files = files;
        self.commit(Staged {
            manifest,
            live: None,
        })?;
        Ok(Some(generation))
    }

    fn compaction_inputs(
        &self,
        table: &Table,
        policy: &CompactionPolicy,
    ) -> Result<Vec<FileEntry>> {
        let files: Vec<&FileEntry> = self.manifest.files_of(table.name).collect();
        let level0: Vec<&FileEntry> = files.iter().copied().filter(|f| f.level == 0).collect();
        let mut inputs: Vec<FileEntry> = Vec::new();
        if level0.len() > policy.level0_max_files {
            inputs.extend(level0.into_iter().cloned());
        }
        if table.kind == Kind::Session {
            for file in files.iter().copied().filter(|f| f.level == 1) {
                if self.dead_rows(file)? * 100 > file.rows * policy.dead_row_percent {
                    inputs.push(file.clone());
                }
            }
        }
        Ok(inputs)
    }

    fn dead_rows(&self, file: &FileEntry) -> Result<u64> {
        let path = self.analytics.join(&file.path);
        let mut dead = 0;
        for batch in read_batches(&path, Some(&[SESSION_COLUMN, GEN_COLUMN]))? {
            dead += self.live_flags(&batch)?.iter().filter(|live| !**live).count() as u64;
        }
        Ok(dead)
    }

    /// Whether each row's `(session_uid, _gen)` is in the live map.
    fn live_flags(&self, batch: &RecordBatch) -> Result<Vec<bool>> {
        let sessions = string_column(batch, SESSION_COLUMN)?;
        let generations = int_column(batch, GEN_COLUMN)?;
        Ok((0..batch.num_rows())
            .map(|row| self.live.get(sessions.value(row)) == Some(&generations.value(row)))
            .collect())
    }

    /// Copies the live rows of `inputs` into new level-1 files, sorted by namespace
    /// and start time.
    fn merge(
        &self,
        table: &Table,
        inputs: &[FileEntry],
        generation: u64,
        policy: &CompactionPolicy,
    ) -> Result<Vec<FileEntry>> {
        let schema: SchemaRef = Arc::new(table.file_schema());
        let mut batches = Vec::new();
        for file in inputs {
            for batch in read_batches(&self.analytics.join(&file.path), None)? {
                let batch = if table.kind == Kind::Session {
                    let live = BooleanArray::from(self.live_flags(&batch)?);
                    filter_record_batch(&batch, &live)?
                } else {
                    batch
                };
                batches.push(batch);
            }
        }
        let merged = concat_batches(&schema, &batches)?;
        if merged.num_rows() == 0 {
            return Ok(Vec::new());
        }
        let order = UInt32Array::from(sort_order(table, &merged)?);
        let sorted = take_record_batch(&merged, &order)?;

        fs::create_dir_all(self.analytics.join(table.name))?;
        let mut outputs = Vec::new();
        let mut open: Option<(OutFile, String)> = None;
        let mut offset = 0;
        while offset < sorted.num_rows() {
            let rows = policy.chunk_rows.max(1).min(sorted.num_rows() - offset);
            let (mut file, path) = match open.take() {
                Some(open) => open,
                None => {
                    let path = format!("{}/g{generation}-{}.parquet", table.name, outputs.len());
                    (OutFile::create(&self.analytics.join(&path), schema.clone())?, path)
                }
            };
            file.write(&sorted.slice(offset, rows))?;
            offset += rows;
            if file.size_hint() >= policy.level1_max_bytes {
                outputs.push(finish(table, path, generation, file)?);
            } else {
                open = Some((file, path));
            }
        }
        if let Some((file, path)) = open {
            outputs.push(finish(table, path, generation, file)?);
        }
        Ok(outputs)
    }
}

fn finish(table: &Table, path: String, generation: u64, file: OutFile) -> Result<FileEntry> {
    let written = file.finish()?;
    Ok(FileEntry {
        table: table.name.to_owned(),
        path,
        level: 1,
        generation,
        rows: written.rows,
        bytes: written.bytes,
    })
}

/// Row order by namespace (the first UID component), then the table's start time.
/// Equal keys keep their input order, so a repeated merge is deterministic.
fn sort_order(table: &Table, batch: &RecordBatch) -> Result<Vec<u32>> {
    let sessions = match table.column(SESSION_COLUMN) {
        Some(_) => Some(string_column(batch, SESSION_COLUMN)?),
        None => None,
    };
    let times = match table.sort_time {
        Some(name) => Some(
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<TimestampMillisecondArray>())
                .ok_or_else(|| Error::invalid(format!("column {name} is not a timestamp")))?,
        ),
        None => None,
    };
    let namespace = |row: usize| {
        sessions.map_or("", |sessions| {
            let uid = sessions.value(row);
            uid.split('/').next().unwrap_or(uid)
        })
    };
    let time = |row: usize| times.map_or(0, |times| times.value(row));
    let mut order: Vec<u32> = (0..batch.num_rows() as u32).collect();
    order.sort_by(|a, b| {
        let (a, b) = (*a as usize, *b as usize);
        namespace(a)
            .cmp(namespace(b))
            .then_with(|| time(a).cmp(&time(b)))
    });
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FACT_STEP;
    use crate::store::read_manifest;
    use crate::testutil::{TestClock, live_steps, update};

    fn open(dir: &std::path::Path) -> Store<Arc<TestClock>> {
        Store::open_with_clock(dir, TestClock::new(0)).unwrap()
    }

    fn policy() -> CompactionPolicy {
        CompactionPolicy {
            level0_max_files: 2,
            level1_max_bytes: u64::MAX,
            dead_row_percent: 30,
            chunk_rows: 2,
        }
    }

    fn levels(store: &Store<Arc<TestClock>>, level: u8) -> Vec<FileEntry> {
        store
            .manifest()
            .files_of(FACT_STEP.name)
            .filter(|file| file.level == level)
            .cloned()
            .collect()
    }

    #[test]
    fn level_zero_merges_only_above_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.publish(&update(&[("m1/claude/a", 1)], "a", 0)).unwrap();
        store.publish(&update(&[("m1/claude/b", 1)], "b", 0)).unwrap();
        assert_eq!(store.compact(&policy()).unwrap(), None);
        assert_eq!(store.manifest().generation, 2);

        store.publish(&update(&[("m1/claude/c", 1)], "c", 0)).unwrap();
        assert_eq!(store.compact(&CompactionPolicy::default()).unwrap(), None);
        assert_eq!(store.compact(&policy()).unwrap(), Some(4));
        assert!(levels(&store, 0).is_empty());
        assert_eq!(levels(&store, 1).len(), 1);
    }

    #[test]
    fn live_rows_are_identical_before_and_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.publish(&update(&[("m1/claude/a", 2), ("m1/claude/b", 2)], "old", 0)).unwrap();
        store.publish(&update(&[("m1/claude/a", 3)], "new", 10)).unwrap();
        let mut last = update(&[("m1/claude/c", 1)], "c", 20);
        last.removed_sessions.push("m1/claude/b".to_owned());
        store.publish(&last).unwrap();
        let before = live_steps(&store);
        assert_eq!(before.len(), 4);

        store.compact(&policy()).unwrap().expect("a generation");
        assert_eq!(live_steps(&store), before);
        // Dead rows were not copied: the merged file holds exactly the live rows.
        let merged = levels(&store, 1);
        assert_eq!(merged.iter().map(|file| file.rows).sum::<u64>(), 4);

        let reopened = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(&reopened, store.manifest());
    }

    #[test]
    fn a_repeated_compaction_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for (index, session) in ["a", "b", "c", "d"].into_iter().enumerate() {
            let uid = format!("m1/claude/{session}");
            store.publish(&update(&[(uid.as_str(), 2)], "x", index as i64)).unwrap();
        }
        let first = store.compact(&policy()).unwrap().expect("a generation");
        let manifest = store.manifest().clone();

        assert_eq!(store.compact(&policy()).unwrap(), None);
        assert_eq!(store.manifest(), &manifest);
        assert_eq!(store.manifest().generation, first);
        assert_eq!(read_manifest(dir.path()).unwrap().as_ref(), Some(&manifest));
    }

    #[test]
    fn level_one_files_are_sorted_by_namespace_then_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.publish(&update(&[("z/claude/a", 2)], "x", 50)).unwrap();
        store.publish(&update(&[("a/claude/b", 2)], "x", 40)).unwrap();
        store.publish(&update(&[("a/claude/c", 2)], "x", 10)).unwrap();
        store.compact(&policy()).unwrap().expect("a generation");

        let files = levels(&store, 1);
        let batches = read_batches(&store.analytics_dir().join(&files[0].path), None).unwrap();
        let mut order = Vec::new();
        for batch in &batches {
            let sessions = string_column(batch, SESSION_COLUMN).unwrap();
            order.extend((0..batch.num_rows()).map(|row| sessions.value(row).to_owned()));
        }
        let expected = ["a/claude/c", "a/claude/c", "a/claude/b", "a/claude/b"];
        assert_eq!(&order[..4], &expected);
        assert_eq!(order[4], "z/claude/a");
    }

    #[test]
    fn only_level_one_files_over_the_dead_threshold_are_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for session in ["a", "b", "c", "d"] {
            let uid = format!("m1/claude/{session}");
            store.publish(&update(&[(uid.as_str(), 1)], "x", 0)).unwrap();
        }
        // One row per file, so each session lands in its own level-1 file.
        let split = CompactionPolicy {
            level1_max_bytes: 1,
            chunk_rows: 1,
            ..policy()
        };
        store.compact(&split).unwrap().expect("a generation");
        let files = levels(&store, 1);
        assert_eq!(files.len(), 4);
        let before = live_steps(&store);

        // Session a is recomputed: its level-1 row is now entirely dead.
        store.publish(&update(&[("m1/claude/a", 1)], "y", 0)).unwrap();
        let generation = store.compact(&split).unwrap().expect("a generation");
        let after = levels(&store, 1);
        assert_eq!(after.len(), 3);
        assert!(after.iter().all(|file| file.generation < generation));
        let kept: HashSet<_> = files.iter().map(|file| &file.path).collect();
        assert!(after.iter().all(|file| kept.contains(&file.path)));

        let live = live_steps(&store);
        assert_eq!(live.len(), before.len());
        assert_eq!(store.compact(&split).unwrap(), None);
    }

    #[test]
    fn compaction_unpublishes_its_inputs_for_later_retirement() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for session in ["a", "b", "c"] {
            let uid = format!("m1/claude/{session}");
            store.publish(&update(&[(uid.as_str(), 1)], "x", 0)).unwrap();
        }
        let inputs = levels(&store, 0);
        store.compact(&policy()).unwrap().expect("a generation");
        for file in &inputs {
            assert!(store.manifest().unpublished.contains_key(&file.path));
            assert!(store.analytics_dir().join(&file.path).is_file());
        }
    }
}
