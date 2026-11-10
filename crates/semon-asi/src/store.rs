//! The generation store: a single writer, immutable Parquet files and an atomically
//! renamed manifest (asi-store.md, Generations, publication and versions).
//!
//! ```text
//! {root}/analytics.lock                    single-writer file lock
//! {root}/analytics/manifest.json           the current generation
//! {root}/analytics/{table}/g{gen}.parquet  rows with a `_gen` column
//! {root}/analytics/live/g{gen}.parquet     session_uid -> _gen
//! ```
//!
//! A row of a session table is live when the live map maps its `session_uid` to its
//! `_gen`. A session is recomputed whole, so a newer generation makes the older rows
//! dead without touching their files.
//!
//! Once the manifest rename has happened the generation is published, whatever
//! follows: the writer adopts it in memory before the directory fsync, so a failure
//! there is reported but never lets the next publish reuse a visible generation.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow::array::{Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::parquet_io::{int_column, read_batches, string_column, with_generation, write_file};
use crate::schema::{
    ASI_SCHEMA_VERSION, GEN_COLUMN, Kind, SESSION_COLUMN, TABLES, Table, live_map_schema, table,
};

const ANALYTICS_DIR: &str = "analytics";
const LOCK_FILE: &str = "analytics.lock";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_TMP: &str = "manifest.json.tmp";
const LIVE_DIR: &str = "live";

/// A file leaves the store this long after the first generation that no longer names it.
pub const RETIREMENT_DELAY_MS: i64 = 10 * 60 * 1000;

/// Source of the time used to stamp unpublication and source loss; injectable for tests.
pub trait Clock {
    fn now_ms(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64)
    }
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now_ms(&self) -> i64 {
        (**self).now_ms()
    }
}

/// Writer targets for future publications; retained rows keep their recorded
/// per-session versions and may therefore differ from these targets.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Versions {
    pub transform: u32,
    pub rules: u32,
    pub price: String,
}

/// A session whose native source is gone. Its rows and versions are kept.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceMissing {
    /// When the writer first found the source missing, in UTC milliseconds.
    pub detected_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub table: String,
    /// Relative to the `analytics/` directory.
    pub path: String,
    /// 0 for a cycle file, 1 for a compacted file.
    pub level: u8,
    /// The generation that wrote the file.
    pub generation: u64,
    /// Smallest and largest `_gen` among the rows of the file.
    pub min_row_gen: u64,
    pub max_row_gen: u64,
    pub rows: u64,
    pub bytes: u64,
    /// Decoded size of the rows; compaction bounds its input by it.
    pub memory: u64,
    /// Rows no longer live. `None` means unknown: a publication may have killed rows
    /// of the file, and compaction counts them again before it decides.
    pub dead: Option<u64>,
}

/// What readers see: the files, the live map and the checkpoints of one generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub generation: u64,
    pub schema_version: u32,
    pub versions: Versions,
    pub files: Vec<FileEntry>,
    /// Relative path of the live map; absent before the first publish.
    pub live_map: Option<String>,
    /// Opaque per-namespace checkpoint (index incarnation and `seq`).
    pub checkpoints: BTreeMap<String, String>,
    /// Last durable revision exported to `fact_label`.
    pub durable_revision: u64,
    /// Live sessions whose native source is gone, by `session_uid`.
    pub source_missing: BTreeMap<String, SourceMissing>,
    /// Files no generation names any more, with the time of the first such generation.
    pub unpublished: BTreeMap<String, i64>,
}

impl Manifest {
    pub fn empty() -> Self {
        Self {
            generation: 0,
            schema_version: ASI_SCHEMA_VERSION,
            versions: Versions::default(),
            files: Vec::new(),
            live_map: None,
            checkpoints: BTreeMap::new(),
            durable_revision: 0,
            source_missing: BTreeMap::new(),
            unpublished: BTreeMap::new(),
        }
    }

    pub fn files_of<'a>(&'a self, table: &'a str) -> impl Iterator<Item = &'a FileEntry> {
        self.files.iter().filter(move |file| file.table == table)
    }
}

/// Rows of one table for one publish, in the table's declared columns (no `_gen`).
#[derive(Clone, Debug)]
pub struct TableBatch {
    pub table: &'static Table,
    pub batch: RecordBatch,
}

/// One cycle's output.
#[derive(Clone, Debug, Default)]
pub struct Update {
    /// The full current rows of the touched sessions (facts), the new rows (labels) or
    /// the whole table (dimensions). A table without rows may be left out.
    pub tables: Vec<TableBatch>,
    /// Sessions recomputed whole in this generation, including those with no rows.
    pub touched_sessions: Vec<String>,
    /// Sessions that no longer exist; their rows become dead.
    pub removed_sessions: Vec<String>,
    /// Live sessions to mark as source-missing (kept as they are, added to the manifest).
    pub source_missing: BTreeMap<String, SourceMissing>,
    /// Sessions whose source is back.
    pub source_restored: Vec<String>,
    /// Checkpoints to set, keyed by namespace; others are kept.
    pub checkpoints: BTreeMap<String, String>,
    pub durable_revision: u64,
    pub versions: Versions,
}

/// A generation whose files are on disk but whose manifest is not yet current.
pub(crate) struct Staged {
    pub manifest: Manifest,
    pub live: Option<HashMap<String, i64>>,
    /// Files the current manifest names that this one drops; stamped at installation.
    pub dropped: Vec<String>,
}

/// Failure points for tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Fault {
    /// After the new manifest is durable under its temporary name, before the rename.
    BeforeRename,
    /// After the rename, where the directory fsync would be.
    AfterRename,
}

/// The analytics writer for one store directory. Holding it holds `analytics.lock`.
pub struct Store<C: Clock = SystemClock> {
    pub(crate) analytics: PathBuf,
    pub(crate) clock: C,
    pub(crate) manifest: Manifest,
    pub(crate) live: HashMap<String, i64>,
    #[cfg(test)]
    pub(crate) fault: Option<Fault>,
    _lock: File,
}

impl Store<SystemClock> {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_clock(root, SystemClock)
    }
}

impl<C: Clock> Store<C> {
    /// Takes the writer lock and discards files of a generation that never committed.
    pub fn open_with_clock(root: &Path, clock: C) -> Result<Self> {
        let analytics = root.join(ANALYTICS_DIR);
        create_durable_dir(&analytics)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(LOCK_FILE))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(Error::WriterBusy),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let manifest = read_manifest_in(&analytics)?.unwrap_or_else(Manifest::empty);
        let live = match &manifest.live_map {
            Some(path) => read_live_map_file(&analytics.join(path))?,
            None => HashMap::new(),
        };
        discard_uncommitted(&analytics, manifest.generation)?;
        Ok(Self {
            analytics,
            clock,
            manifest,
            live,
            #[cfg(test)]
            fault: None,
            _lock: lock,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn live_map(&self) -> &HashMap<String, i64> {
        &self.live
    }

    /// Reloads the published manifest and its matching live map while retaining
    /// this process's writer lock.
    ///
    /// A directory fsync failure can be reported after the manifest rename has
    /// made a generation visible. Writers use this before retrying such an
    /// operation so they do not publish from a stale checkpoint.
    pub fn reload_manifest(&mut self) -> Result<()> {
        let manifest = read_manifest_in(&self.analytics)?.unwrap_or_else(Manifest::empty);
        let live = match &manifest.live_map {
            Some(path) => read_live_map_file(&self.analytics.join(path))?,
            None => HashMap::new(),
        };
        self.manifest = manifest;
        self.live = live;
        Ok(())
    }

    pub fn analytics_dir(&self) -> &Path {
        &self.analytics
    }

    pub(crate) fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Writes one generation and makes it current. A crash before the manifest rename
    /// leaves the previous generation. An error after the rename still leaves this
    /// store holding the new generation: it is visible to readers.
    pub fn publish(&mut self, update: &Update) -> Result<u64> {
        let staged = self.stage(update)?;
        let generation = staged.manifest.generation;
        self.install(staged)?;
        Ok(generation)
    }

    /// Writes the files of the next generation and fsyncs them; the manifest still
    /// names the previous generation.
    pub(crate) fn stage(&self, update: &Update) -> Result<Staged> {
        let touched: HashSet<&str> = update.touched_sessions.iter().map(String::as_str).collect();
        if update
            .removed_sessions
            .iter()
            .any(|session| touched.contains(session.as_str()))
        {
            return Err(Error::invalid("a session is both touched and removed"));
        }
        if update.durable_revision < self.manifest.durable_revision {
            return Err(Error::invalid(
                "the durable revision checkpoint went backwards",
            ));
        }
        let mut seen = HashSet::new();
        if !update
            .tables
            .iter()
            .all(|item| seen.insert(item.table.name))
        {
            return Err(Error::invalid("a table appears twice in one update"));
        }

        discard_uncommitted(&self.analytics, self.manifest.generation)?;
        // Validate all batches before writing any file in this generation.
        for item in &update.tables {
            with_generation(item.table, &item.batch, 0)?;
        }
        let generation = self.manifest.generation + 1;
        let stamp = generation as i64;
        let mut files = self.manifest.files.clone();
        let mut written = Vec::new();
        for item in &update.tables {
            let table = item.table;
            if table.kind == Kind::Dimension {
                files.retain(|file| file.table != table.name);
            } else if item.batch.num_rows() == 0 {
                continue;
            }
            if table.kind == Kind::Session {
                let sessions = string_column(&item.batch, SESSION_COLUMN)?;
                if let Some(stray) = sessions.iter().flatten().find(|s| !touched.contains(s)) {
                    return Err(Error::invalid(format!(
                        "{} holds rows of {stray}, which is not a touched session",
                        table.name
                    )));
                }
            }
            let path = format!("{}/g{generation}.parquet", table.name);
            let batch = with_generation(table, &item.batch, stamp)?;
            create_durable_dir(&self.analytics.join(table.name))?;
            let out = write_file(&self.analytics.join(&path), &batch)?;
            let entry = FileEntry {
                table: table.name.to_owned(),
                path,
                level: 0,
                generation,
                min_row_gen: generation,
                max_row_gen: generation,
                rows: out.rows,
                bytes: out.bytes,
                memory: out.memory,
                dead: None,
            };
            written.push(entry.clone());
            files.push(entry);
        }

        // The rows these sessions had until now die. A compacted file whose generation
        // range holds one of those generations has an unknown dead count from here on.
        let dying: BTreeSet<i64> = update
            .touched_sessions
            .iter()
            .chain(&update.removed_sessions)
            .filter_map(|session| self.live.get(session))
            .copied()
            .collect();
        for file in &mut files {
            let session_table = table(&file.table).is_some_and(|t| t.kind == Kind::Session);
            let (low, high) = (file.min_row_gen as i64, file.max_row_gen as i64);
            if file.level == 1 && session_table && dying.range(low..=high).next().is_some() {
                file.dead = None;
            }
        }

        let mut live = self.live.clone();
        for session in &update.touched_sessions {
            live.insert(session.clone(), stamp);
        }
        for session in &update.removed_sessions {
            live.remove(session);
        }
        let mut source_missing = self.manifest.source_missing.clone();
        for session in update
            .removed_sessions
            .iter()
            .chain(&update.source_restored)
        {
            source_missing.remove(session);
        }
        source_missing.extend(update.source_missing.clone());
        if let Some(stray) = source_missing.keys().find(|s| !live.contains_key(*s)) {
            return Err(Error::invalid(format!(
                "{stray} is marked source-missing but is not a live session"
            )));
        }

        let live_map = format!("{LIVE_DIR}/g{generation}.parquet");
        create_durable_dir(&self.analytics.join(LIVE_DIR))?;
        write_live_map(&self.analytics.join(&live_map), &live)?;
        sync_parents(
            &self.analytics,
            written.iter().map(|file| file.path.as_str()),
        )?;
        sync_dir(&self.analytics.join(LIVE_DIR))?;

        let mut checkpoints = self.manifest.checkpoints.clone();
        checkpoints.extend(update.checkpoints.clone());
        let live_map = Some(live_map);
        let dropped = self.dropped_paths(&files, live_map.as_deref());
        let manifest = Manifest {
            generation,
            schema_version: ASI_SCHEMA_VERSION,
            versions: update.versions.clone(),
            unpublished: self.manifest.unpublished.clone(),
            files,
            live_map,
            checkpoints,
            durable_revision: update.durable_revision,
            source_missing,
        };
        Ok(Staged {
            manifest,
            live: Some(live),
            dropped,
        })
    }

    /// Makes a staged generation current: the manifest is written under a temporary
    /// name and fsynced, renamed into place, and the directory is fsynced. The
    /// rename is the publication: from there on this store holds the new manifest
    /// and live map even if the directory fsync fails, so no later generation can
    /// reuse a number a reader may already see. Unpublication is stamped with the
    /// time of this publication, not of staging.
    pub(crate) fn install(&mut self, staged: Staged) -> Result<()> {
        let Staged {
            mut manifest,
            live,
            dropped,
        } = staged;
        let now = self.clock.now_ms();
        for path in dropped {
            manifest.unpublished.entry(path).or_insert(now);
        }
        let temporary = self.analytics.join(MANIFEST_TMP);
        let mut file = File::create(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        file.sync_all()?;
        #[cfg(test)]
        self.inject(Fault::BeforeRename)?;
        fs::rename(&temporary, self.analytics.join(MANIFEST_FILE))?;
        self.manifest = manifest;
        if let Some(live) = live {
            self.live = live;
        }
        #[cfg(test)]
        self.inject(Fault::AfterRename)?;
        sync_dir(&self.analytics)
    }

    #[cfg(test)]
    fn inject(&mut self, point: Fault) -> Result<()> {
        if self.fault == Some(point) {
            self.fault = None;
            return Err(Error::Io(std::io::Error::other("injected fault")));
        }
        Ok(())
    }

    /// The files the current manifest names that a manifest naming `files` and
    /// `live_map` drops.
    pub(crate) fn dropped_paths(&self, files: &[FileEntry], live_map: Option<&str>) -> Vec<String> {
        let named: HashSet<&str> = files
            .iter()
            .map(|file| file.path.as_str())
            .chain(live_map)
            .collect();
        self.manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .chain(self.manifest.live_map.as_deref())
            .filter(|path| !named.contains(path))
            .map(str::to_owned)
            .collect()
    }

    /// Deletes every file unpublished at least [`RETIREMENT_DELAY_MS`] ago and returns
    /// their paths. The files go first: a crash leaves entries that the next call
    /// clears, not a manifest that names deleted files.
    pub fn retire(&mut self) -> Result<Vec<String>> {
        let now = self.clock.now_ms();
        let due: Vec<String> = self
            .manifest
            .unpublished
            .iter()
            .filter(|(_, at)| **at + RETIREMENT_DELAY_MS <= now)
            .map(|(path, _)| path.clone())
            .collect();
        if due.is_empty() {
            return Ok(due);
        }
        for path in &due {
            match fs::remove_file(self.analytics.join(path)) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut manifest = self.manifest.clone();
        for path in &due {
            manifest.unpublished.remove(path);
        }
        self.install(Staged {
            manifest,
            live: None,
            dropped: Vec::new(),
        })?;
        Ok(due)
    }
}

/// The current manifest of a store directory, or `None` before the first publish.
/// Readers need no lock: files are immutable and the manifest is replaced by rename.
pub fn read_manifest(root: &Path) -> Result<Option<Manifest>> {
    read_manifest_in(&root.join(ANALYTICS_DIR))
}

/// The live map a manifest names: `session_uid -> _gen`.
pub fn read_live_map(root: &Path, manifest: &Manifest) -> Result<HashMap<String, i64>> {
    match &manifest.live_map {
        Some(path) => read_live_map_file(&root.join(ANALYTICS_DIR).join(path)),
        None => Ok(HashMap::new()),
    }
}

fn read_manifest_in(analytics: &Path) -> Result<Option<Manifest>> {
    let bytes = match fs::read(analytics.join(MANIFEST_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema_version != ASI_SCHEMA_VERSION {
        return Err(Error::invalid(format!(
            "analytics schema version {} is not {ASI_SCHEMA_VERSION}; export and reset",
            manifest.schema_version
        )));
    }
    Ok(Some(manifest))
}

fn write_live_map(path: &Path, live: &HashMap<String, i64>) -> Result<()> {
    let mut entries: Vec<(&String, &i64)> = live.iter().collect();
    entries.sort();
    let sessions =
        StringArray::from_iter_values(entries.iter().map(|(session, _)| session.as_str()));
    let generations = Int64Array::from_iter_values(entries.iter().map(|(_, stamp)| **stamp));
    let batch = RecordBatch::try_new(
        Arc::new(live_map_schema()),
        vec![Arc::new(sessions), Arc::new(generations)],
    )?;
    write_file(path, &batch)?;
    Ok(())
}

fn read_live_map_file(path: &Path) -> Result<HashMap<String, i64>> {
    let mut live = HashMap::new();
    for batch in read_batches(path, None)? {
        let sessions = string_column(&batch, SESSION_COLUMN)?;
        let generations = int_column(&batch, GEN_COLUMN)?;
        for row in 0..batch.num_rows() {
            live.insert(sessions.value(row).to_owned(), generations.value(row));
        }
    }
    Ok(live)
}

fn create_durable_dir(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return create_durable_dir(&std::env::current_dir()?.join(path));
    }
    if let Some(parent) = path.parent() {
        // Existing ancestors may come from an attempt whose parent sync failed.
        // Repeat the sync chain on retries before publishing into this directory.
        create_durable_dir(parent)?;
        fs::create_dir_all(path)?;
        sync_dir(parent)?;
    }
    Ok(())
}

pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

/// Fsyncs the directory of each relative path.
pub(crate) fn sync_parents<'a>(
    analytics: &Path,
    paths: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let dirs: BTreeSet<PathBuf> = paths
        .filter_map(|path| analytics.join(path).parent().map(Path::to_path_buf))
        .collect();
    for dir in dirs {
        sync_dir(&dir)?;
    }
    Ok(())
}

/// The generation number in a file name such as `g12.parquet` or `g12-0.parquet`.
fn generation_of(name: &str) -> Option<u64> {
    let digits: String = name
        .strip_prefix('g')?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Generation numbers only grow, and the writer lock is held, so a file of a later
/// generation than the manifest's is the leftover of a publish that never committed.
pub(crate) fn discard_uncommitted(analytics: &Path, committed: u64) -> Result<()> {
    match fs::remove_file(analytics.join(MANIFEST_TMP)) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let dirs = TABLES.iter().map(|table| table.name).chain([LIVE_DIR]);
    for dir in dirs {
        let entries = match fs::read_dir(analytics.join(dir)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(generation_of)
                .is_some_and(|generation| generation > committed)
            {
                fs::remove_file(entry.path())?;
            }
        }
        // A previous attempt may have removed files but failed before syncing.
        // Sync even an unchanged directory before allowing generation reuse.
        sync_dir(&analytics.join(dir))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DIM_PRICE, DIM_REPO, FACT_LABEL, FACT_SESSION, FACT_STEP};
    use crate::testutil::{
        FACT_TABLES, TestClock, all_rows, live_steps, replace_column, rows_of, sample, update,
        update_all,
    };

    const S1: &str = "m1/claude/s1";
    const S2: &str = "m1/claude/s2";

    fn open(dir: &Path, clock: &Arc<TestClock>) -> Store<Arc<TestClock>> {
        Store::open_with_clock(dir, clock.clone()).unwrap()
    }

    fn g2_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        for table in ["fact_step", "fact_session", "live"] {
            let path = dir.join(format!("analytics/{table}/g2.parquet"));
            files.push((table.to_owned(), fs::read(path).unwrap()));
        }
        files
    }

    #[test]
    fn rejects_invalid_enum_before_writing_generation() {
        for (table, column, invalid) in [
            (&FACT_STEP, "usage_owner", "Exclusive"),
            (&DIM_PRICE, "unit", "token"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path(), &TestClock::new(0));
            let batch = sample(table, &[(S1, 1)], "a", 0);
            let batch = replace_column(&batch, column, Arc::new(StringArray::from(vec![invalid])));
            let update = Update {
                tables: vec![TableBatch { table, batch }],
                touched_sessions: vec![S1.to_owned()],
                ..Update::default()
            };
            assert!(store.publish(&update).is_err());
            assert_eq!(store.manifest().generation, 0);
            assert!(!store.analytics_dir().join(table.name).exists());
        }
    }

    #[test]
    fn publish_writes_files_live_map_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        assert_eq!(
            store.publish(&update(&[(S1, 2), (S2, 1)], "a", 0)).unwrap(),
            1
        );

        let manifest = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(&manifest, store.manifest());
        assert_eq!(manifest.generation, 1);
        assert_eq!(manifest.live_map.as_deref(), Some("live/g1.parquet"));
        let step = manifest.files_of("fact_step").next().unwrap();
        assert_eq!(step.path, "fact_step/g1.parquet");
        assert_eq!((step.level, step.rows), (0, 3));
        assert_eq!((step.min_row_gen, step.max_row_gen), (1, 1));
        let on_disk = fs::metadata(dir.path().join("analytics").join(&step.path)).unwrap();
        assert_eq!(step.bytes, on_disk.len());
        assert!(
            dir.path()
                .join("analytics/fact_session/g1.parquet")
                .is_file()
        );

        let live = read_live_map(dir.path(), &manifest).unwrap();
        assert_eq!(
            live,
            HashMap::from([(S1.to_owned(), 1), (S2.to_owned(), 1)])
        );
    }

    #[test]
    fn a_later_generation_supersedes_and_removes_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        store
            .publish(&update(&[(S1, 2), (S2, 2)], "old", 0))
            .unwrap();
        let mut next = update(&[(S1, 3)], "new", 0);
        next.removed_sessions.push(S2.to_owned());
        next.checkpoints
            .insert("m1".to_owned(), "inc1:7".to_owned());
        store.publish(&next).unwrap();

        let live = live_steps(&store);
        assert_eq!(live.len(), 3);
        assert!(live.iter().all(|(session, generation, step)| {
            session == S1 && *generation == 2 && step.ends_with(":new")
        }));
        assert_eq!(store.manifest().checkpoints["m1"], "inc1:7");
        assert_eq!(store.live_map().len(), 1);
    }

    #[test]
    fn unpublished_files_are_stamped_once() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(1_000);
        let mut store = open(dir.path(), &clock);
        store.publish(&update(&[(S1, 1)], "a", 0)).unwrap();
        assert!(store.manifest().unpublished.is_empty());
        clock.advance(5);
        store.publish(&update(&[(S1, 1)], "b", 0)).unwrap();
        // Generation 1's live map is dropped; its fact files stay named (level 0).
        assert_eq!(
            store.manifest().unpublished,
            BTreeMap::from([("live/g1.parquet".to_owned(), 1_005)])
        );
    }

    #[test]
    fn unpublication_is_stamped_when_the_generation_is_published_not_staged() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(1_000);
        let mut store = open(dir.path(), &clock);
        store.publish(&update(&[(S1, 1)], "a", 0)).unwrap();

        let staged = store.stage(&update(&[(S1, 1)], "b", 0)).unwrap();
        clock.advance(7_000);
        store.install(staged).unwrap();
        assert_eq!(
            store.manifest().unpublished,
            BTreeMap::from([("live/g1.parquet".to_owned(), 8_000)])
        );
    }

    #[test]
    fn a_crash_before_the_manifest_rename_leaves_the_old_generation() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = open(dir.path(), &clock);
        store.publish(&update(&[(S1, 2)], "old", 0)).unwrap();
        let before = live_steps(&store);

        // Everything of generation 2 is on disk, but the manifest is not renamed.
        let staged = store.stage(&update(&[(S1, 5)], "new", 0)).unwrap();
        assert_eq!(staged.manifest.generation, 2);
        assert!(dir.path().join("analytics/fact_step/g2.parquet").is_file());
        assert!(dir.path().join("analytics/live/g2.parquet").is_file());
        assert_eq!(read_manifest(dir.path()).unwrap().unwrap().generation, 1);
        drop(staged);
        drop(store);

        let mut store = open(dir.path(), &clock);
        assert_eq!(store.manifest().generation, 1);
        assert_eq!(live_steps(&store), before);
        assert!(!dir.path().join("analytics/fact_step/g2.parquet").exists());
        assert!(!dir.path().join("analytics/live/g2.parquet").exists());
        assert_eq!(store.publish(&update(&[(S1, 1)], "again", 0)).unwrap(), 2);
    }

    #[test]
    fn a_failure_before_the_rename_keeps_the_old_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        store.publish(&update(&[(S1, 2)], "old", 0)).unwrap();
        let before = live_steps(&store);

        store.fault = Some(Fault::BeforeRename);
        assert!(matches!(
            store.publish(&update(&[(S1, 5)], "new", 0)),
            Err(Error::Io(_))
        ));
        assert_eq!(store.manifest().generation, 1);
        assert_eq!(read_manifest(dir.path()).unwrap().unwrap().generation, 1);
        assert_eq!(live_steps(&store), before);
        assert_eq!(store.publish(&update(&[(S1, 1)], "again", 0)).unwrap(), 2);
    }

    #[test]
    fn a_failure_after_the_rename_still_publishes_and_the_number_is_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = open(dir.path(), &clock);
        store.publish(&update(&[(S1, 2)], "one", 0)).unwrap();

        // The directory fsync after the rename fails: the error is reported, but
        // generation 2 is visible to readers and to this writer.
        store.fault = Some(Fault::AfterRename);
        assert!(matches!(
            store.publish(&update(&[(S1, 3)], "two", 0)),
            Err(Error::Io(_))
        ));
        assert_eq!(store.manifest().generation, 2);
        let visible = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(&visible, store.manifest());
        let reader_rows = rows_of(
            &dir.path().join("analytics"),
            &visible,
            &read_live_map(dir.path(), &visible).unwrap(),
            &FACT_STEP,
        )
        .unwrap();
        let reader_files = g2_bytes(dir.path());

        // Continuing: the next publish takes generation 3 and leaves generation 2 alone.
        assert_eq!(store.publish(&update(&[(S1, 4)], "three", 0)).unwrap(), 3);
        assert_eq!(g2_bytes(dir.path()), reader_files);
        assert_eq!(
            rows_of(
                &dir.path().join("analytics"),
                &visible,
                &read_live_map(dir.path(), &visible).unwrap(),
                &FACT_STEP,
            )
            .unwrap(),
            reader_rows
        );
        drop(store);

        // Reopening after such a failure behaves the same.
        let mut store = open(dir.path(), &clock);
        assert_eq!(store.manifest().generation, 3);
        assert_eq!(store.publish(&update(&[(S1, 1)], "four", 0)).unwrap(), 4);
        assert_eq!(g2_bytes(dir.path()), reader_files);
    }

    #[test]
    fn a_failed_publication_followed_by_a_reopen_keeps_the_visible_generation() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = open(dir.path(), &clock);
        store.publish(&update(&[(S1, 2)], "one", 0)).unwrap();
        store.fault = Some(Fault::AfterRename);
        assert!(store.publish(&update(&[(S1, 3)], "two", 0)).is_err());
        let reader_files = g2_bytes(dir.path());
        drop(store);

        let mut store = open(dir.path(), &clock);
        assert_eq!(store.manifest().generation, 2);
        assert_eq!(g2_bytes(dir.path()), reader_files);
        assert_eq!(store.publish(&update(&[(S1, 1)], "three", 0)).unwrap(), 3);
    }

    #[test]
    fn reload_manifest_adopts_the_published_checkpoint_before_retry() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = open(dir.path(), &clock);
        let mut update = update(&[(S1, 1)], "published", 0);
        update
            .checkpoints
            .insert("machine".into(), "incarnation:7".into());
        store.publish(&update).unwrap();
        let expected = read_manifest(dir.path()).unwrap().unwrap();

        // Model the stale in-memory view a caller must repair after an error
        // reported after manifest rename; the durable manifest remains source
        // of truth for the next publication's checkpoint.
        store.manifest = Manifest::empty();
        store.live.clear();
        store.reload_manifest().unwrap();

        assert_eq!(store.manifest(), &expected);
        assert_eq!(
            store.live_map(),
            &read_live_map(dir.path(), &expected).unwrap()
        );
    }

    #[test]
    fn retirement_waits_ten_minutes_after_unpublication() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(1_000_000);
        let mut store = open(dir.path(), &clock);
        store.publish(&update(&[(S1, 1)], "a", 0)).unwrap();
        store.publish(&update(&[(S1, 1)], "b", 0)).unwrap();
        let old_live = dir.path().join("analytics/live/g1.parquet");
        let current_live = dir.path().join("analytics/live/g2.parquet");

        clock.advance(RETIREMENT_DELAY_MS - 1);
        assert!(store.retire().unwrap().is_empty());
        assert!(old_live.is_file());

        clock.advance(1);
        assert_eq!(store.retire().unwrap(), vec!["live/g1.parquet".to_owned()]);
        assert!(!old_live.exists());
        assert!(current_live.is_file());
        assert!(store.manifest().unpublished.is_empty());
        assert_eq!(
            read_manifest(dir.path()).unwrap().as_ref(),
            Some(store.manifest())
        );
        assert!(store.retire().unwrap().is_empty());
    }

    #[test]
    fn an_older_snapshot_stays_readable_until_its_files_are_retired() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = open(dir.path(), &clock);
        store.publish(&update_all(&[(S1, 2)], "old", 0)).unwrap();
        let snapshot = read_manifest(dir.path()).unwrap().unwrap();
        let analytics = dir.path().join("analytics");
        let live = read_live_map(dir.path(), &snapshot).unwrap();
        let before = rows_of(&analytics, &snapshot, &live, &FACT_STEP).unwrap();

        store.publish(&update_all(&[(S1, 3)], "new", 0)).unwrap();
        assert_eq!(
            rows_of(&analytics, &snapshot, &live, &FACT_STEP).unwrap(),
            before
        );

        clock.advance(RETIREMENT_DELAY_MS - 1);
        store.retire().unwrap();
        assert_eq!(
            rows_of(&analytics, &snapshot, &live, &FACT_STEP).unwrap(),
            before
        );

        // The snapshot's live map was unpublished by the second generation.
        clock.advance(1);
        store.retire().unwrap();
        assert!(read_live_map(dir.path(), &snapshot).is_err());
        let current = read_manifest(dir.path()).unwrap().unwrap();
        assert!(read_live_map(dir.path(), &current).is_ok());
    }

    #[test]
    fn a_second_writer_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let store = open(dir.path(), &clock);
        assert!(matches!(
            Store::open_with_clock(dir.path(), clock.clone()),
            Err(Error::WriterBusy)
        ));
        drop(store);
        assert!(Store::open_with_clock(dir.path(), clock).is_ok());
    }

    #[test]
    fn a_session_row_outside_the_touched_set_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        let mut stray = update(&[(S1, 1)], "a", 0);
        stray.touched_sessions.clear();
        assert!(matches!(store.publish(&stray), Err(Error::Invalid(_))));
        assert_eq!(store.manifest().generation, 0);
    }

    #[test]
    fn an_unsupported_schema_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        store.publish(&update(&[(S1, 1)], "a", 0)).unwrap();
        drop(store);
        let path = dir.path().join("analytics/manifest.json");
        let text = fs::read_to_string(&path).unwrap();
        let current = format!("\"schema_version\": {ASI_SCHEMA_VERSION}");
        assert!(
            text.contains(&current),
            "the manifest records the schema version"
        );
        fs::write(&path, text.replace(&current, "\"schema_version\": 99")).unwrap();
        assert!(matches!(read_manifest(dir.path()), Err(Error::Invalid(_))));
    }

    #[test]
    fn a_batch_must_match_its_declared_schema() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        let mut wrong = update(&[(S1, 1)], "a", 0);
        wrong.tables.truncate(1);
        wrong.tables[0].table = &FACT_SESSION;
        assert!(matches!(store.publish(&wrong), Err(Error::Invalid(_))));
    }

    #[test]
    fn replacing_a_dimension_replaces_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        let dimension = |rows: usize, tag: &str| Update {
            tables: vec![TableBatch {
                table: &DIM_REPO,
                batch: sample(&DIM_REPO, &[("repo", rows)], tag, 0),
            }],
            ..Update::default()
        };
        store.publish(&dimension(2, "v1")).unwrap();
        store.publish(&dimension(3, "v2")).unwrap();

        let files: Vec<_> = store.manifest().files_of("dim_repo").collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "dim_repo/g2.parquet");
        assert!(
            store
                .manifest()
                .unpublished
                .contains_key("dim_repo/g1.parquet")
        );
        let rows = rows_of(
            store.analytics_dir(),
            store.manifest(),
            store.live_map(),
            &DIM_REPO,
        )
        .unwrap();
        assert_eq!(rows.values().sum::<usize>(), 3);
        assert!(
            rows.keys()
                .all(|row| row.contains(":v2|") && !row.contains(":v1|"))
        );
    }

    #[test]
    fn source_missing_sessions_persist_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = open(dir.path(), &clock);
        let mut first = update(&[(S1, 1), (S2, 1)], "a", 0);
        first
            .source_missing
            .insert(S1.to_owned(), SourceMissing { detected_at: 42 });
        store.publish(&first).unwrap();
        drop(store);

        let mut store = open(dir.path(), &clock);
        assert_eq!(
            store.manifest().source_missing,
            BTreeMap::from([(S1.to_owned(), SourceMissing { detected_at: 42 })])
        );
        let on_disk = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(on_disk.source_missing, store.manifest().source_missing);

        // A later publish keeps the mark; it must name a live session.
        store.publish(&update(&[(S2, 1)], "b", 0)).unwrap();
        assert!(store.manifest().source_missing.contains_key(S1));
        let mut stray = update(&[(S2, 1)], "c", 0);
        stray.source_missing.insert(
            "m1/claude/nobody".to_owned(),
            SourceMissing { detected_at: 1 },
        );
        assert!(matches!(store.publish(&stray), Err(Error::Invalid(_))));

        let mut restored = update(&[(S2, 1)], "d", 0);
        restored.source_restored.push(S1.to_owned());
        store.publish(&restored).unwrap();
        assert!(store.manifest().source_missing.is_empty());

        let mut again = update(&[(S1, 1)], "e", 0);
        again
            .source_missing
            .insert(S1.to_owned(), SourceMissing { detected_at: 9 });
        store.publish(&again).unwrap();
        let mut removed = update(&[(S2, 1)], "f", 0);
        removed.removed_sessions.push(S1.to_owned());
        store.publish(&removed).unwrap();
        assert!(store.manifest().source_missing.is_empty());
    }

    #[test]
    fn equal_token_totals_with_different_billing_inputs_stay_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        let batch = sample(&FACT_STEP, &[(S1, 3)], "a", 0);
        let ints = |values: [i64; 3]| -> arrow::array::ArrayRef {
            Arc::new(Int64Array::from(values.to_vec()))
        };
        let texts = |values: [Option<&str>; 3]| -> arrow::array::ArrayRef {
            Arc::new(StringArray::from(values.to_vec()))
        };
        // Each row bills 100 cache-write tokens, split differently.
        let batch = replace_column(&batch, "cache_write_5m_tokens", ints([100, 0, 100]));
        let batch = replace_column(&batch, "cache_write_1h_tokens", ints([0, 100, 0]));
        let batch = replace_column(&batch, "web_search_requests", ints([0, 0, 2]));
        let batch = replace_column(
            &batch,
            "speed",
            texts([Some("standard"), Some("standard"), Some("fast")]),
        );
        let mut update = update(&[(S1, 3)], "a", 0);
        update.tables[0].batch = batch;
        store.publish(&update).unwrap();

        let rows = rows_of(
            store.analytics_dir(),
            store.manifest(),
            store.live_map(),
            &FACT_STEP,
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        let file = store.manifest().files_of("fact_step").next().unwrap();
        let read =
            crate::parquet_io::read_batches(&store.analytics_dir().join(&file.path), None).unwrap();
        let five = crate::parquet_io::int_column(&read[0], "cache_write_5m_tokens").unwrap();
        let hour = crate::parquet_io::int_column(&read[0], "cache_write_1h_tokens").unwrap();
        let searches = crate::parquet_io::int_column(&read[0], "web_search_requests").unwrap();
        let speed = crate::parquet_io::string_column(&read[0], "speed").unwrap();
        let seen: Vec<(i64, i64, i64, &str)> = (0..3)
            .map(|row| {
                (
                    five.value(row),
                    hour.value(row),
                    searches.value(row),
                    speed.value(row),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (100, 0, 0, "standard"),
                (0, 100, 0, "standard"),
                (100, 0, 2, "fast")
            ]
        );
    }

    #[test]
    fn every_fact_table_publishes_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        store
            .publish(&update_all(&[(S1, 2), (S2, 1)], "a", 0))
            .unwrap();
        let rows = all_rows(&store, &FACT_TABLES);
        assert_eq!(rows.len(), 6);
        assert!(rows.iter().all(|table| !table.is_empty()));
        let labels = rows_of(
            store.analytics_dir(),
            store.manifest(),
            store.live_map(),
            &FACT_LABEL,
        )
        .unwrap();
        assert_eq!(labels.values().sum::<usize>(), 1);
    }
}
