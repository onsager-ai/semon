//! Tiered compaction (asi-store.md, Generations, publication and versions).
//!
//! Cycle files are level 0. When a table has more than `level0_max_files` of them they
//! are merged into level-1 files. A level-1 file is rewritten only when more than
//! `dead_row_percent` of its rows are dead, together with the adjacent level-1 files
//! that also hold dead rows. Only live rows are copied, and they keep their `_gen`,
//! so the live map is untouched and the live rows are the same before and after.
//! Compaction publishes as another generation; a run with nothing to do publishes
//! nothing.
//!
//! A run is bounded. It takes files in order (level 0 oldest first, then the dirty
//! level-1 files) until their decoded size would pass `max_input_bytes`, always at
//! least one file, and leaves the rest to later runs. Peak memory is about three
//! times the cap (input, merged rows, sorted rows), plus the file that exceeds it
//! when a single file is larger than the cap. Every output file is sorted by
//! namespace and start time.
//!
//! Output files hold at most `level1_max_bytes`, footer included. Oversized row
//! groups are subdivided; a single row larger than the cap is an error.

use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use arrow::array::{BooleanArray, TimestampMillisecondArray, UInt32Array};
use arrow::compute::{concat_batches, filter_record_batch, max, min, take_record_batch};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use crate::error::{Error, Result};
use crate::parquet_io::{OutFile, Written, int_column, read_batches, string_column};
use crate::schema::{GEN_COLUMN, Kind, SESSION_COLUMN, TABLES, Table};
use crate::store::{Clock, FileEntry, Staged, Store, discard_uncommitted, sync_parents};

/// Room kept for the Parquet footer: a base, plus a share per row group.
const FOOTER_BASE_BYTES: u64 = 64 * 1024;
const FOOTER_PER_GROUP_BYTES: u64 = 16 * 1024;

#[derive(Clone, Debug)]
pub struct CompactionPolicy {
    /// Merge level-0 files once a table has more than this many.
    pub level0_max_files: usize,
    /// No output file is larger than this, footer included (see the module docs).
    pub level1_max_bytes: u64,
    /// Rewrite a level-1 file when more than this share of its rows is dead.
    pub dead_row_percent: u64,
    /// Rows per row group of an output file.
    pub chunk_rows: usize,
    /// Decoded bytes one run reads per table (see the module docs).
    pub max_input_bytes: u64,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            level0_max_files: 32,
            level1_max_bytes: 128 * 1024 * 1024,
            dead_row_percent: 30,
            chunk_rows: 4096,
            max_input_bytes: 256 * 1024 * 1024,
        }
    }
}

impl<C: Clock> Store<C> {
    /// Compacts every table that needs it into one new generation, and returns its
    /// number, or `None` when there was nothing to do. A run handles a bounded
    /// amount of input; call again while it returns a generation to finish.
    pub fn compact(&mut self, policy: &CompactionPolicy) -> Result<Option<u64>> {
        discard_uncommitted(&self.analytics, self.manifest.generation)?;
        if self.count_unknown_dead_rows(policy)? {
            return Ok(Some(self.manifest.generation));
        }
        let generation = self.manifest.generation + 1;
        let mut dropped: HashSet<String> = HashSet::new();
        let mut added: Vec<FileEntry> = Vec::new();
        for table in TABLES {
            if table.kind == Kind::Dimension {
                continue;
            }
            let inputs = self.compaction_inputs(table, policy);
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
        let dropped = self.dropped_paths(&files, manifest.live_map.as_deref());
        manifest.files = files;
        self.install(Staged {
            manifest,
            live: None,
            dropped,
        })?;
        Ok(Some(generation))
    }

    /// Counts dead rows of the level-1 files whose count a publication invalidated,
    /// and records them. Files with a known count are not read.
    fn count_unknown_dead_rows(&mut self, policy: &CompactionPolicy) -> Result<bool> {
        let mut manifest = self.manifest.clone();
        let mut changed = false;
        let mut memory = 0u64;
        for file in &mut manifest.files {
            let session_table = TABLES
                .iter()
                .any(|table| table.name == file.table && table.kind == Kind::Session);
            if file.level == 1 && session_table && file.dead.is_none() {
                if changed && memory.saturating_add(file.memory) > policy.max_input_bytes {
                    break;
                }
                file.dead = Some(self.dead_rows(file)?);
                memory = memory.saturating_add(file.memory);
                changed = true;
            }
        }
        if changed {
            self.install(Staged {
                manifest,
                live: None,
                dropped: Vec::new(),
            })?;
        }
        Ok(changed)
    }

    fn compaction_inputs(&self, table: &Table, policy: &CompactionPolicy) -> Vec<FileEntry> {
        let mut candidates: Vec<&FileEntry> = Vec::new();
        let mut level0: Vec<&FileEntry> = self
            .manifest
            .files_of(table.name)
            .filter(|file| file.level == 0)
            .collect();
        if level0.len() > policy.level0_max_files {
            level0.sort_by_key(|file| file.generation);
            candidates.extend(level0);
        }
        if table.kind == Kind::Session {
            let level1: Vec<&FileEntry> = self
                .manifest
                .files_of(table.name)
                .filter(|file| file.level == 1)
                .collect();
            let dirty: Vec<bool> = level1
                .iter()
                .map(|file| dead_of(file) * 100 > file.rows * policy.dead_row_percent)
                .collect();
            for (index, file) in level1.iter().enumerate() {
                let next_to_dirty = (index > 0 && dirty[index - 1])
                    || dirty.get(index + 1).copied().unwrap_or(false);
                if dirty[index] || (dead_of(file) > 0 && next_to_dirty) {
                    candidates.push(file);
                }
            }
        }
        let mut inputs: Vec<FileEntry> = Vec::new();
        let mut memory = 0;
        for file in candidates {
            if !inputs.is_empty() && memory + file.memory > policy.max_input_bytes {
                break;
            }
            memory += file.memory;
            inputs.push(file.clone());
        }
        inputs
    }

    fn dead_rows(&self, file: &FileEntry) -> Result<u64> {
        let path = self.analytics.join(&file.path);
        let mut dead = 0;
        for batch in read_batches(&path, Some(&[SESSION_COLUMN, GEN_COLUMN]))? {
            dead += self
                .live_flags(&batch)?
                .iter()
                .filter(|live| !**live)
                .count() as u64;
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
        drop(batches);
        if merged.num_rows() == 0 {
            return Ok(Vec::new());
        }
        let order = UInt32Array::from(sort_order(table, &merged)?);
        let sorted = take_record_batch(&merged, &order)?;
        drop(merged);

        fs::create_dir_all(self.analytics.join(table.name))?;
        let total = sorted.num_rows();
        let step = policy.chunk_rows.max(1);
        let mut groups: Vec<(usize, usize)> = (0..total)
            .step_by(step)
            .map(|offset| (offset, step.min(total - offset)))
            .collect();
        let mut outputs = Vec::new();
        let mut next = 0;
        while next < groups.len() {
            let path = format!("{}/g{generation}-{}.parquet", table.name, outputs.len());
            let run = WriteRun {
                path: self.analytics.join(&path),
                schema: &schema,
                sorted: &sorted,
                groups: &groups[next..],
                cap: policy.level1_max_bytes,
            };
            let Some((written, taken, low, high)) = run.write()? else {
                let (offset, len) = groups[next];
                if len == 1 {
                    return Err(Error::invalid(
                        "a single row exceeds the compaction output cap",
                    ));
                }
                let first = len / 2;
                groups[next] = (offset, first);
                groups.insert(next + 1, (offset + first, len - first));
                continue;
            };
            next += taken;
            outputs.push(FileEntry {
                table: table.name.to_owned(),
                path,
                level: 1,
                generation,
                min_row_gen: low as u64,
                max_row_gen: high as u64,
                rows: written.rows,
                bytes: written.bytes,
                memory: written.memory,
                dead: Some(0),
            });
        }
        Ok(outputs)
    }
}

fn dead_of(file: &FileEntry) -> u64 {
    file.dead.unwrap_or(0)
}

/// One output file: as many of the next row groups as fit under the byte cap.
struct WriteRun<'a> {
    path: std::path::PathBuf,
    schema: &'a SchemaRef,
    sorted: &'a RecordBatch,
    groups: &'a [(usize, usize)],
    cap: u64,
}

impl WriteRun<'_> {
    /// Writes the file and returns what it holds, how many row groups it took and
    /// the range of `_gen` it covers.
    ///
    /// Before each row group the bytes so far and a prediction from the average row
    /// size are checked against the cap. The prediction can be wrong for variable
    /// data, so the finished file is measured, footer included; if it is over the
    /// cap, the file is written again with one row group fewer. An oversized first
    /// group is returned to the caller for subdivision.
    fn write(&self) -> Result<Option<(Written, usize, i64, i64)>> {
        let mut limit = self.groups.len();
        loop {
            let mut out = OutFile::create(&self.path, self.schema.clone())?;
            let (mut low, mut high) = (i64::MAX, i64::MIN);
            let mut taken = 0;
            for &(offset, len) in &self.groups[..limit] {
                if taken > 0 {
                    let predicted = out.size() * len as u64 / out.rows().max(1);
                    let footer = FOOTER_BASE_BYTES + FOOTER_PER_GROUP_BYTES * (taken as u64 + 1);
                    if out.size() + predicted + footer > self.cap {
                        break;
                    }
                }
                let slice = self.sorted.slice(offset, len);
                let generations = int_column(&slice, GEN_COLUMN)?;
                low = low.min(min(generations).unwrap_or(i64::MAX));
                high = high.max(max(generations).unwrap_or(i64::MIN));
                // Materialize the slice so its buffers reflect the rows in this group.
                let indices = UInt32Array::from_iter_values(0..len as u32);
                let materialized = take_record_batch(&slice, &indices)?;
                out.write_group(&materialized, materialized.get_array_memory_size() as u64)?;
                taken += 1;
            }
            let written = out.finish()?;
            if written.bytes > self.cap {
                if taken == 1 {
                    return Ok(None);
                }
                limit = taken - 1;
                continue;
            }
            return Ok(Some((written, taken, low, high)));
        }
    }
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
    use crate::parquet_io::READS;
    use crate::schema::FACT_STEP;
    use crate::store::{RETIREMENT_DELAY_MS, SourceMissing, TableBatch, Update, read_manifest};
    use crate::testutil::{
        FACT_TABLES, TestClock, all_rows, live_steps, noise, replace_column, rows_of, sample,
        update, update_all,
    };

    fn open(dir: &std::path::Path) -> Store<Arc<TestClock>> {
        Store::open_with_clock(dir, TestClock::new(0)).unwrap()
    }

    fn policy() -> CompactionPolicy {
        CompactionPolicy {
            level0_max_files: 2,
            level1_max_bytes: u64::MAX,
            dead_row_percent: 30,
            chunk_rows: 2,
            max_input_bytes: u64::MAX,
        }
    }

    /// One row group per file: every row group is its own level-1 file.
    fn one_group_per_file(chunk_rows: usize) -> CompactionPolicy {
        CompactionPolicy {
            level1_max_bytes: 64 * 1024,
            chunk_rows,
            ..policy()
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

    fn session(index: usize) -> String {
        format!("m1/claude/s{index:03}")
    }

    /// Publishes sessions `range` (one row each) as one generation.
    fn publish_range(store: &mut Store<Arc<TestClock>>, range: std::ops::Range<usize>, tag: &str) {
        let names: Vec<String> = range.map(session).collect();
        let sessions: Vec<(&str, usize)> = names.iter().map(|name| (name.as_str(), 1)).collect();
        store.publish(&update_all(&sessions, tag, 0)).unwrap();
    }

    #[test]
    fn level_zero_merges_only_above_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .publish(&update(&[("m1/claude/a", 1)], "a", 0))
            .unwrap();
        store
            .publish(&update(&[("m1/claude/b", 1)], "b", 0))
            .unwrap();
        assert_eq!(store.compact(&policy()).unwrap(), None);
        assert_eq!(store.manifest().generation, 2);

        store
            .publish(&update(&[("m1/claude/c", 1)], "c", 0))
            .unwrap();
        assert_eq!(store.compact(&CompactionPolicy::default()).unwrap(), None);
        assert_eq!(store.compact(&policy()).unwrap(), Some(4));
        assert!(levels(&store, 0).is_empty());
        assert_eq!(levels(&store, 1).len(), 1);
    }

    #[test]
    fn complete_row_multisets_are_identical_before_and_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let names = ["m1/claude/a", "m1/claude/b", "m1/claude/c"];
        store
            .publish(&update_all(&[(names[0], 2), (names[1], 2)], "old", 0))
            .unwrap();
        store
            .publish(&update_all(&[(names[0], 3)], "new", 10))
            .unwrap();
        let mut last = update_all(&[(names[2], 1)], "c", 20);
        last.removed_sessions.push(names[1].to_owned());
        store.publish(&last).unwrap();
        store
            .publish(&update_all(&[("m1/claude/d", 2)], "d", 30))
            .unwrap();

        let before = all_rows(&store, &FACT_TABLES);
        assert_eq!(before[0].values().sum::<usize>(), 3 + 1 + 2);
        // Labels are append-only: all four publishes' labels stay.
        assert_eq!(before[5].values().sum::<usize>(), 4);

        store.compact(&policy()).unwrap().expect("a generation");
        assert_eq!(all_rows(&store, &FACT_TABLES), before);
        for table in FACT_TABLES {
            let files: Vec<_> = store.manifest().files_of(table.name).collect();
            assert!(!files.is_empty());
            assert!(files.iter().all(|file| file.level == 1), "{}", table.name);
        }
        let reopened = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(&reopened, store.manifest());
    }

    #[test]
    fn a_repeated_compaction_is_a_no_op_that_reads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for (index, session) in ["a", "b", "c", "d"].into_iter().enumerate() {
            let uid = format!("m1/claude/{session}");
            store
                .publish(&update_all(&[(uid.as_str(), 2)], "x", index as i64))
                .unwrap();
        }
        let first = store.compact(&policy()).unwrap().expect("a generation");
        let manifest = store.manifest().clone();

        READS.with(|reads| reads.set(0));
        assert_eq!(store.compact(&policy()).unwrap(), None);
        assert_eq!(READS.with(|reads| reads.get()), 0);
        assert_eq!(store.manifest(), &manifest);
        assert_eq!(store.manifest().generation, first);
        assert_eq!(read_manifest(dir.path()).unwrap().as_ref(), Some(&manifest));
    }

    #[test]
    fn level_one_files_are_sorted_by_namespace_then_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .publish(&update(&[("z/claude/a", 2)], "x", 50))
            .unwrap();
        store
            .publish(&update(&[("a/claude/b", 2)], "x", 40))
            .unwrap();
        store
            .publish(&update(&[("a/claude/c", 2)], "x", 10))
            .unwrap();
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
    fn a_level_one_file_is_rewritten_only_above_thirty_percent_dead() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let wide = CompactionPolicy {
            chunk_rows: 1000,
            ..policy()
        };
        publish_range(&mut store, 0..40, "x");
        publish_range(&mut store, 40..80, "x");
        publish_range(&mut store, 80..100, "x");
        store.compact(&wide).unwrap().expect("a generation");
        let merged = levels(&store, 1);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].rows, 100);

        // Exactly 30 of 100 rows dead: not rewritten.
        publish_range(&mut store, 0..30, "y");
        let generation = store.manifest().generation;
        assert_eq!(store.compact(&wide).unwrap(), Some(generation));
        assert_eq!(store.compact(&wide).unwrap(), None);
        assert_eq!(store.manifest().generation, generation);
        assert_eq!(levels(&store, 1)[0].path, merged[0].path);
        assert_eq!(levels(&store, 1)[0].dead, Some(30));

        // 31 of 100: rewritten, with only live rows.
        publish_range(&mut store, 30..31, "y");
        let before = all_rows(&store, &FACT_TABLES);
        store.compact(&wide).unwrap().expect("a recount");
        store.compact(&wide).unwrap().expect("a generation");
        let rewritten = levels(&store, 1);
        assert_eq!(rewritten.len(), 1);
        assert_ne!(rewritten[0].path, merged[0].path);
        assert_eq!((rewritten[0].rows, rewritten[0].dead), (69, Some(0)));
        assert_eq!(all_rows(&store, &FACT_TABLES), before);
        assert_eq!(store.compact(&wide).unwrap(), None);
    }

    #[test]
    fn dirty_files_are_rewritten_with_their_dead_row_neighbours_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for start in [0, 10, 20] {
            publish_range(&mut store, start..start + 10, "x");
        }
        let split = one_group_per_file(10);
        store.compact(&split).unwrap().expect("a generation");
        let files = levels(&store, 1);
        assert_eq!(files.len(), 3);

        // File A loses 4 of 10 rows, B loses 1, C none.
        let names: Vec<String> = (0..4).chain(10..11).map(session).collect();
        let rows: Vec<(&str, usize)> = names.iter().map(|name| (name.as_str(), 1)).collect();
        store.publish(&update_all(&rows, "y", 0)).unwrap();

        let before = all_rows(&store, &FACT_TABLES);
        store.compact(&split).unwrap().expect("a recount");
        store.compact(&split).unwrap().expect("a generation");
        let paths: Vec<String> = levels(&store, 1).into_iter().map(|f| f.path).collect();
        assert!(paths.contains(&files[2].path), "the clean file is kept");
        assert!(!paths.contains(&files[0].path));
        assert!(!paths.contains(&files[1].path));
        assert_eq!(all_rows(&store, &FACT_TABLES), before);
    }

    #[test]
    fn recount_of_invalidated_files_is_bounded_and_makes_progress() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for start in [0, 10, 20] {
            publish_range(&mut store, start..start + 10, "x");
        }
        store.compact(&one_group_per_file(10)).unwrap();
        publish_range(&mut store, 0..30, "new");
        let before = store
            .manifest
            .files
            .iter()
            .filter(|f| f.level == 1 && f.dead.is_none())
            .count();
        assert!(before > 1);
        let bounded = CompactionPolicy {
            max_input_bytes: 1,
            ..policy()
        };
        store.compact(&bounded).unwrap();
        let after = store
            .manifest
            .files
            .iter()
            .filter(|f| f.level == 1 && f.dead.is_none())
            .count();
        assert_eq!(after, before - 1);
    }

    #[test]
    fn a_large_selection_is_processed_in_bounded_passes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for start in 0..6 {
            publish_range(&mut store, start..start + 1, "x");
        }
        let before = all_rows(&store, &FACT_TABLES);
        let bounded = CompactionPolicy {
            max_input_bytes: 1,
            ..policy()
        };

        // Each pass takes the oldest file of each table: 6 cycle files become 5, 4, 3
        // and then 2, which is no longer above the threshold.
        let mut passes = 0;
        let mut taken = Vec::new();
        while store.compact(&bounded).unwrap().is_some() {
            passes += 1;
            taken.push(levels(&store, 0).len());
            assert!(passes < 10, "compaction does not converge");
        }
        assert_eq!(passes, 4);
        assert_eq!(taken, [5, 4, 3, 2]);
        assert_eq!(levels(&store, 1).len(), 4);
        assert_eq!(all_rows(&store, &FACT_TABLES), before);

        // With room for everything, one pass does it.
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for start in 0..6 {
            publish_range(&mut store, start..start + 1, "x");
        }
        store.compact(&policy()).unwrap().expect("a generation");
        assert_eq!(store.compact(&policy()).unwrap(), None);
    }

    #[test]
    fn output_files_stay_under_the_byte_cap_with_variable_length_data() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let cap = 300_000;
        for part in 0..3u64 {
            let name = session(part as usize);
            let rows = 2_000;
            let mut batch = sample(&FACT_STEP, &[(name.as_str(), rows)], "x", 0);
            batch = replace_column(
                &batch,
                "step_uid",
                noise(rows, 100 + part as usize * 60, 7 + part),
            );
            batch = replace_column(&batch, "turn_uid", noise(rows, 80, 99 + part));
            store
                .publish(&Update {
                    tables: vec![TableBatch {
                        table: &FACT_STEP,
                        batch,
                    }],
                    touched_sessions: vec![name],
                    ..Update::default()
                })
                .unwrap();
        }
        let before = rows_of(
            store.analytics_dir(),
            store.manifest(),
            store.live_map(),
            &FACT_STEP,
        )
        .unwrap();
        let capped = CompactionPolicy {
            level1_max_bytes: cap,
            chunk_rows: 400,
            ..policy()
        };
        store.compact(&capped).unwrap().expect("a generation");

        let files = levels(&store, 1);
        assert!(files.len() >= 2, "{} files", files.len());
        assert_eq!(files.iter().map(|file| file.rows).sum::<u64>(), 6_000);
        for file in &files {
            let actual = fs::metadata(store.analytics_dir().join(&file.path))
                .unwrap()
                .len();
            assert_eq!(file.bytes, actual);
            assert!(actual <= cap, "{} is {actual} bytes", file.path);
        }
        let after = rows_of(
            store.analytics_dir(),
            store.manifest(),
            store.live_map(),
            &FACT_STEP,
        )
        .unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn single_row_larger_than_the_cap_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for start in 0..3 {
            publish_range(&mut store, start * 4..start * 4 + 4, "x");
        }
        let tiny = CompactionPolicy {
            level1_max_bytes: 1,
            chunk_rows: 4,
            ..policy()
        };
        assert!(store.compact(&tiny).is_err());
        assert_eq!(store.manifest().generation, 3);
    }

    #[test]
    fn compaction_unpublishes_its_inputs_for_later_retirement() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        for session in ["a", "b", "c"] {
            let uid = format!("m1/claude/{session}");
            store
                .publish(&update(&[(uid.as_str(), 1)], "x", 0))
                .unwrap();
        }
        let inputs = levels(&store, 0);
        store.compact(&policy()).unwrap().expect("a generation");
        for file in &inputs {
            assert!(store.manifest().unpublished.contains_key(&file.path));
            assert!(store.analytics_dir().join(&file.path).is_file());
        }
    }

    #[test]
    fn a_reader_on_an_older_manifest_is_undisturbed_by_compaction_and_retirement() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = Store::open_with_clock(dir.path(), clock.clone()).unwrap();
        for start in 0..3 {
            publish_range(&mut store, start..start + 1, "x");
        }
        let snapshot = read_manifest(dir.path()).unwrap().unwrap();
        let analytics = dir.path().join("analytics");
        let live = crate::store::read_live_map(dir.path(), &snapshot).unwrap();
        let read = || {
            FACT_TABLES
                .iter()
                .map(|table| rows_of(&analytics, &snapshot, &live, table).unwrap())
                .collect::<Vec<_>>()
        };
        let before = read();

        store.compact(&policy()).unwrap().expect("a generation");
        assert_eq!(read(), before);
        clock.advance(RETIREMENT_DELAY_MS - 1);
        store.retire().unwrap();
        assert_eq!(read(), before);
        assert_eq!(all_rows(&store, &FACT_TABLES), before);

        // The snapshot's cycle files are gone ten minutes after compaction dropped
        // them; the current manifest reads fine.
        clock.advance(1);
        assert!(!store.retire().unwrap().is_empty());
        assert!(rows_of(&analytics, &snapshot, &live, &FACT_STEP).is_err());
        assert_eq!(all_rows(&store, &FACT_TABLES), before);
    }

    #[test]
    fn source_missing_sessions_survive_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut first = update(&[("m1/claude/a", 1)], "a", 0);
        first
            .source_missing
            .insert("m1/claude/a".to_owned(), SourceMissing { detected_at: 5 });
        store.publish(&first).unwrap();
        store
            .publish(&update(&[("m1/claude/b", 1)], "b", 0))
            .unwrap();
        store
            .publish(&update(&[("m1/claude/c", 1)], "c", 0))
            .unwrap();
        let expected = store.manifest().source_missing.clone();
        assert_eq!(expected.len(), 1);

        store.compact(&policy()).unwrap().expect("a generation");
        assert_eq!(store.manifest().source_missing, expected);
        let reopened = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(reopened.source_missing, expected);
    }

    #[test]
    fn live_steps_still_agree_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        publish_range(&mut store, 0..2, "old");
        publish_range(&mut store, 0..1, "new");
        publish_range(&mut store, 2..3, "x");
        let before = live_steps(&store);
        store.compact(&policy()).unwrap().expect("a generation");
        assert_eq!(live_steps(&store), before);
    }
}
