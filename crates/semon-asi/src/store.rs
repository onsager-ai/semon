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

    pub fn analytics_dir(&self) -> &Path {
        &self.analytics
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
        // A previous attempt may have removed files but failed before syncing.
        // Sync even an unchanged directory before allowing generation reuse.
        sync_dir(&analytics.join(dir))?;
    }
    Ok(())
}
