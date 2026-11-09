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
    ASI_SCHEMA_VERSION, GEN_COLUMN, Kind, SESSION_COLUMN, TABLES, Table, live_map_schema,
};

const ANALYTICS_DIR: &str = "analytics";
const LOCK_FILE: &str = "analytics.lock";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_TMP: &str = "manifest.json.tmp";
const LIVE_DIR: &str = "live";

/// A file leaves the store this long after the first generation that no longer names it.
pub const RETIREMENT_DELAY_MS: i64 = 10 * 60 * 1000;

/// Source of the time used to stamp unpublication; injectable for tests.
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

/// Versions of the code that produced the rows; a stale row is requeued.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Versions {
    pub transform: u32,
    pub rules: u32,
    pub price: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub table: String,
    /// Relative to the `analytics/` directory.
    pub path: String,
    /// 0 for a cycle file, 1 for a compacted file.
    pub level: u8,
    pub generation: u64,
    pub rows: u64,
    pub bytes: u64,
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
    /// Checkpoints to set, keyed by namespace; others are kept.
    pub checkpoints: BTreeMap<String, String>,
    pub durable_revision: u64,
    pub versions: Versions,
}

pub(crate) struct Staged {
    pub manifest: Manifest,
    pub live: Option<HashMap<String, i64>>,
}

/// The analytics writer for one store directory. Holding it holds `analytics.lock`.
pub struct Store<C: Clock = SystemClock> {
    pub(crate) analytics: PathBuf,
    pub(crate) clock: C,
    pub(crate) manifest: Manifest,
    pub(crate) live: HashMap<String, i64>,
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
        fs::create_dir_all(&analytics)?;
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
            _lock: lock,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn live_map(&self) -> &HashMap<String, i64> {
        &self.live
    }

    pub fn analytics_dir(&self) -> &Path {
        &self.analytics
    }

    /// Writes one generation and makes it current. A crash before the manifest rename
    /// leaves the previous generation.
    pub fn publish(&mut self, update: &Update) -> Result<u64> {
        let staged = self.stage(update)?;
        let generation = staged.manifest.generation;
        self.commit(staged)?;
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
            return Err(Error::invalid("the durable revision checkpoint went backwards"));
        }
        let mut seen = HashSet::new();
        if !update.tables.iter().all(|item| seen.insert(item.table.name)) {
            return Err(Error::invalid("a table appears twice in one update"));
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
            fs::create_dir_all(self.analytics.join(table.name))?;
            let out = write_file(&self.analytics.join(&path), &batch)?;
            let entry = FileEntry {
                table: table.name.to_owned(),
                path,
                level: 0,
                generation,
                rows: out.rows,
                bytes: out.bytes,
            };
            written.push(entry.clone());
            files.push(entry);
        }

        let mut live = self.live.clone();
        for session in &update.touched_sessions {
            live.insert(session.clone(), stamp);
        }
        for session in &update.removed_sessions {
            live.remove(session);
        }
        let live_map = format!("{LIVE_DIR}/g{generation}.parquet");
        fs::create_dir_all(self.analytics.join(LIVE_DIR))?;
        write_live_map(&self.analytics.join(&live_map), &live)?;
        sync_parents(&self.analytics, written.iter().map(|file| file.path.as_str()))?;
        sync_dir(&self.analytics.join(LIVE_DIR))?;

        let mut checkpoints = self.manifest.checkpoints.clone();
        checkpoints.extend(update.checkpoints.clone());
        let live_map = Some(live_map);
        let manifest = Manifest {
            generation,
            schema_version: ASI_SCHEMA_VERSION,
            versions: update.versions.clone(),
            unpublished: self.unpublish_dropped(&files, live_map.as_deref()),
            files,
            live_map,
            checkpoints,
            durable_revision: update.durable_revision,
        };
        Ok(Staged {
            manifest,
            live: Some(live),
        })
    }

    /// Makes a staged generation current: manifest, fsync, rename, directory fsync.
    pub(crate) fn commit(&mut self, staged: Staged) -> Result<()> {
        write_manifest(&self.analytics, &staged.manifest)?;
        self.manifest = staged.manifest;
        if let Some(live) = staged.live {
            self.live = live;
        }
        Ok(())
    }

    /// The unpublication times for a manifest that will name `files` and `live_map`:
    /// the existing ones, plus now for every file the current manifest names that the
    /// next one drops.
    pub(crate) fn unpublish_dropped(
        &self,
        files: &[FileEntry],
        live_map: Option<&str>,
    ) -> BTreeMap<String, i64> {
        let now = self.clock.now_ms();
        let named: HashSet<&str> = files
            .iter()
            .map(|file| file.path.as_str())
            .chain(live_map)
            .collect();
        let mut unpublished = self.manifest.unpublished.clone();
        let previous = self
            .manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .chain(self.manifest.live_map.as_deref());
        for path in previous {
            if !named.contains(path) {
                unpublished.entry(path.to_owned()).or_insert(now);
            }
        }
        unpublished
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
        write_manifest(&self.analytics, &manifest)?;
        self.manifest = manifest;
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

fn write_manifest(analytics: &Path, manifest: &Manifest) -> Result<()> {
    let temporary = analytics.join(MANIFEST_TMP);
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    file.sync_all()?;
    fs::rename(&temporary, analytics.join(MANIFEST_FILE))?;
    sync_dir(analytics)
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
fn discard_uncommitted(analytics: &Path, committed: u64) -> Result<()> {
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FACT_SESSION;
    use crate::testutil::{TestClock, live_steps, update};

    const S1: &str = "m1/claude/s1";
    const S2: &str = "m1/claude/s2";

    fn open(dir: &Path, clock: &Arc<TestClock>) -> Store<Arc<TestClock>> {
        Store::open_with_clock(dir, clock.clone()).unwrap()
    }

    #[test]
    fn publish_writes_files_live_map_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        assert_eq!(store.publish(&update(&[(S1, 2), (S2, 1)], "a", 0)).unwrap(), 1);

        let manifest = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(&manifest, store.manifest());
        assert_eq!(manifest.generation, 1);
        assert_eq!(manifest.live_map.as_deref(), Some("live/g1.parquet"));
        let step = manifest.files_of("fact_step").next().unwrap();
        assert_eq!((step.path.as_str(), step.level, step.rows), ("fact_step/g1.parquet", 0, 3));
        assert!(dir.path().join("analytics/fact_session/g1.parquet").is_file());

        let live = read_live_map(dir.path(), &manifest).unwrap();
        assert_eq!(live, HashMap::from([(S1.to_owned(), 1), (S2.to_owned(), 1)]));
    }

    #[test]
    fn a_later_generation_supersedes_and_removes_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path(), &TestClock::new(0));
        store.publish(&update(&[(S1, 2), (S2, 2)], "old", 0)).unwrap();
        let mut next = update(&[(S1, 3)], "new", 0);
        next.removed_sessions.push(S2.to_owned());
        next.checkpoints.insert("m1".to_owned(), "inc1:7".to_owned());
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
        assert_eq!(read_manifest(dir.path()).unwrap().as_ref(), Some(store.manifest()));
        assert!(store.retire().unwrap().is_empty());
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
        fs::write(&path, text.replace("\"schema_version\": 1", "\"schema_version\": 99")).unwrap();
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
}
