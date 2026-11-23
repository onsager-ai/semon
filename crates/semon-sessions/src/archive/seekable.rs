//! Origin-local, rebuildable copies of archive chains for bounded Viewer reads.
use super::*;
use crate::{SessionSourceRef, events::Stat, source_chunks::SIZE};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::sync::{LazyLock, mpsc};

const CACHE: &str = "archive-ranges-v1";

fn chunk_ordinal(offset: u64) -> i64 {
    // A chunk ordinal fits a SQLite integer even when its byte offset does not.
    (offset / SIZE as u64) as i64
}

#[derive(Clone)]
struct Candidate {
    archive: PathBuf,
    dir: PathBuf,
    chain: Chain,
    observed: Stat,
    manifest: Stat,
}

impl Candidate {
    fn cache(&self) -> PathBuf {
        self.archive.with_file_name(CACHE)
    }

    fn copy(&self) -> PathBuf {
        self.cache().join(format!("{}.sqlite", self.chain.chain_id))
    }

    fn original(&self) -> PathBuf {
        self.dir.join(format!("{}.zst", self.chain.chain_id))
    }

    fn verify(&self) -> io::Result<()> {
        if Stat::of(&self.original())? != self.observed
            || (Stat::of(&self.dir.join(MANIFEST))? != self.manifest
                && !read_manifest(&self.dir)?.chains.contains(&self.chain))
        {
            return Err(invalid("archive chain changed"));
        }
        Ok(())
    }

    fn open(&self) -> io::Result<Option<Connection>> {
        match fs::symlink_metadata(self.copy()) {
            Ok(stat) if stat.is_file() => (),
            Ok(_) => return Err(invalid("invalid archive copy path")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let db = Connection::open_with_flags(
            self.copy(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(io::Error::other)?;
        let witness: String = db
            .query_row("SELECT witness FROM chain", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        Ok((witness == self.witness()?).then_some(db))
    }

    fn witness(&self) -> io::Result<String> {
        serde_json::to_string(&(&self.chain, self.observed)).map_err(io::Error::other)
    }

    fn build(&self) -> io::Result<()> {
        self.verify()?;
        if self.open()?.is_some() {
            return Ok(());
        }
        let cache = self.cache();
        make_dir(&cache)?;
        let temporary = cache.join(format!("{}.tmp", random_id()?));
        drop(create_file(&temporary)?);
        let result = self.write_copy(&temporary);
        let _ = fs::remove_file(&temporary);
        result
    }

    fn write_copy(&self, temporary: &Path) -> io::Result<()> {
        let mut db = Connection::open_with_flags(
            temporary,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(io::Error::other)?;
        // The private staging database is disposable until it has synced and
        // been atomically published. It needs no journal alongside it.
        db.execute_batch(
            "PRAGMA journal_mode=OFF;
            CREATE TABLE chain(witness TEXT NOT NULL);
            CREATE TABLE chunk(ordinal INTEGER PRIMARY KEY, body BLOB NOT NULL,
                sha256 BLOB NOT NULL, prefix TEXT NOT NULL);",
        )
        .map_err(io::Error::other)?;
        let tx = db.transaction().map_err(io::Error::other)?;
        tx.execute("INSERT INTO chain VALUES(?1)", [self.witness()?])
            .map_err(io::Error::other)?;
        let mut reader = open_chain(&self.dir, &self.chain)?;
        let mut buffer = vec![0; SIZE];
        let mut offset = 0;
        let mut prefix = PrefixState::default();
        let mut digest = Sha256::new();
        {
            let mut insert = tx
                .prepare("INSERT INTO chunk VALUES(?1,?2,?3,?4)")
                .map_err(io::Error::other)?;
            while offset < self.chain.length {
                let count = (self.chain.length - offset).min(SIZE as u64) as usize;
                let bytes = &mut buffer[..count];
                reader.read_exact(bytes)?;
                digest.update(&*bytes);
                insert
                    .execute(params![
                        chunk_ordinal(offset),
                        &*bytes,
                        &Sha256::digest(&*bytes)[..],
                        serde_json::to_string(&prefix).map_err(io::Error::other)?
                    ])
                    .map_err(io::Error::other)?;
                if count == SIZE {
                    prefix.advance(bytes);
                }
                offset += count as u64;
            }
        }
        if reader.read(&mut buffer[..1])? != 0
            || format!("{:x}", digest.finalize()) != self.chain.sha256
        {
            return Err(invalid("archive chain differs from its manifest"));
        }
        tx.commit().map_err(io::Error::other)?;
        drop(db);
        crate::open_input(temporary)?.sync_all()?;
        // Pruning and append writers only contend with final publication.
        let archive = Archive::new(self.archive.clone());
        let _guard = archive.existing_read_lock(false)?;
        self.verify()?;
        if !read_manifest(&self.dir)?.chains.contains(&self.chain) {
            return Err(invalid("archive chain custody was withdrawn"));
        }
        fs::rename(temporary, self.copy())?;
        sync_dir(&self.cache())
    }
}

#[derive(Default)]
struct Jobs {
    pending: std::collections::BTreeSet<PathBuf>,
    failed: BTreeMap<PathBuf, Stat>,
}

struct Worker {
    sender: mpsc::SyncSender<Candidate>,
    jobs: std::sync::Arc<Mutex<Jobs>>,
}

static WORKER: LazyLock<io::Result<Worker>> = LazyLock::new(|| {
    let (sender, receiver) = mpsc::sync_channel::<Candidate>(16);
    let jobs = std::sync::Arc::new(Mutex::new(Jobs::default()));
    let state = jobs.clone();
    std::thread::Builder::new()
        .name("semon-archive-ranges".into())
        .spawn(move || {
            while let Ok(candidate) = receiver.recv() {
                let path = candidate.copy();
                let result = candidate.build();
                let Ok(mut jobs) = state.lock() else { return };
                jobs.pending.remove(&path);
                if let Err(error) = result {
                    eprintln!("semon archive range preparation failed: {}", error.kind());
                    // Retain only a bounded set of failures for unchanged custody.
                    if jobs.failed.len() == 16 {
                        jobs.failed.pop_first();
                    }
                    jobs.failed.insert(path, candidate.observed);
                } else {
                    jobs.failed.remove(&path);
                }
            }
        })?;
    Ok(Worker { sender, jobs })
});

fn queue(candidate: Candidate) -> io::Result<bool> {
    let worker = WORKER
        .as_ref()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut jobs = worker
        .jobs
        .lock()
        .map_err(|_| invalid("archive worker state unavailable"))?;
    let path = candidate.copy();
    if jobs.failed.get(&path) == Some(&candidate.observed) {
        return Ok(false);
    }
    if jobs.pending.contains(&path) {
        return Ok(true);
    }
    match worker.sender.try_send(candidate) {
        Ok(()) => {
            jobs.pending.insert(path);
        }
        Err(mpsc::TrySendError::Full(_)) => (),
        Err(mpsc::TrySendError::Disconnected(_)) => {
            return Err(invalid("archive worker unavailable"));
        }
    }
    Ok(true)
}

#[derive(Debug)]
pub(crate) struct Preparing;
impl std::fmt::Display for Preparing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Preparing recorded history")
    }
}
impl std::error::Error for Preparing {}

pub(super) fn remove(archive: &Path, chain: &Chain) -> io::Result<()> {
    let cache = archive.with_file_name(CACHE);
    match fs::remove_file(cache.join(format!("{}.sqlite", chain.chain_id))) {
        Ok(()) => sync_dir(&cache),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn candidates(
    archive: &Archive,
    path: &Path,
    source: &SessionSourceRef,
    expected: Option<&str>,
) -> io::Result<Vec<Candidate>> {
    let name = path.to_str().ok_or_else(missing)?;
    if !archive.dir.is_dir() {
        return Err(missing());
    }
    let _guard = archive.existing_read_lock(true).map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            io::Error::other(Preparing)
        } else {
            error
        }
    })?;
    let id = archive.lookup_locked(name)?.ok_or_else(missing)?;
    let dir = archive.dir.join(id);
    let manifest_stat = Stat::of(&dir.join(MANIFEST))?;
    let manifest = read_manifest(&dir)?;
    if manifest.source != name {
        return Err(missing());
    }
    let digest = hex(&source.prefix_sha256);
    manifest
        .chains
        .into_iter()
        .rev()
        .filter(|chain| {
            chain.redactor_version == NATIVE_REDACTOR
                && chain.length >= source.offset
                && (chain.length > source.offset || chain.sha256 == digest)
                && expected.is_none_or(|value| value == chain.chain_id)
        })
        .map(|chain| {
            let observed = Verification::observed(&dir, &chain)?.file;
            Ok(Candidate {
                archive: archive.dir.clone(),
                dir: dir.clone(),
                chain,
                observed,
                manifest: manifest_stat,
            })
        })
        .collect()
}

fn missing() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "Frozen native prefix is unavailable",
    )
}

pub(crate) struct CopyRead {
    candidate: Candidate,
    db: Connection,
    blocks: BTreeMap<u64, Vec<u8>>,
}

impl CopyRead {
    pub(crate) fn open(
        archive: &Archive,
        path: &Path,
        source: &SessionSourceRef,
        expected: Option<&str>,
        budget: &mut usize,
    ) -> io::Result<Self> {
        let mut preparing = false;
        for candidate in candidates(archive, path, source, expected)? {
            let Some(db) = candidate.open()? else {
                preparing |= queue(candidate)?;
                continue;
            };
            let mut reader = Self {
                candidate,
                db,
                blocks: BTreeMap::new(),
            };
            if reader.qualifies(source, budget)? {
                reader.candidate.verify()?;
                return Ok(reader);
            }
        }
        if preparing {
            Err(io::Error::other(Preparing))
        } else {
            Err(missing())
        }
    }

    pub(crate) fn generation(&self) -> &str {
        &self.candidate.chain.chain_id
    }

    fn qualifies(&mut self, source: &SessionSourceRef, budget: &mut usize) -> io::Result<bool> {
        let at = source.offset.saturating_sub(1) / SIZE as u64;
        let boundary = at * SIZE as u64;
        let state: String = self
            .db
            .query_row(
                "SELECT prefix FROM chunk WHERE ordinal=?1",
                [chunk_ordinal(source.offset.saturating_sub(1))],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        let state: PrefixState = serde_json::from_str(&state).map_err(io::Error::other)?;
        let end = (source.offset - boundary) as usize;
        if state.at != boundary
            || state.finish(self.block(at, budget)?.get(..end).ok_or_else(missing)?)
                != source.prefix_sha256
        {
            return Ok(false);
        }
        let tail_len = source.offset.min(4096) as usize;
        let tail = self.range(source.offset - tail_len as u64, tail_len, budget)?;
        Ok(<[u8; 32]>::from(Sha256::digest(tail)) == source.tail_sha256)
    }

    fn block(&mut self, ordinal: u64, budget: &mut usize) -> io::Result<&[u8]> {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.blocks.entry(ordinal) {
            let start = ordinal * SIZE as u64;
            let len = self
                .candidate
                .chain
                .length
                .checked_sub(start)
                .ok_or_else(missing)?
                .min(SIZE as u64) as usize;
            *budget = budget.checked_sub(len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::WouldBlock, "Archive proof budget exhausted")
            })?;
            let record = self
                .db
                .query_row(
                    "SELECT body,sha256 FROM chunk
                WHERE ordinal=?1 AND length(body)=?2 AND length(sha256)=32",
                    params![chunk_ordinal(start), len as i64],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, [u8; 32]>(1)?)),
                )
                .optional()
                .map_err(io::Error::other)?
                .ok_or_else(missing)?;
            if <[u8; 32]>::from(Sha256::digest(&record.0)) != record.1 {
                return Err(invalid("archive copy chunk failed verification"));
            }
            entry.insert(record.0);
        }
        Ok(&self.blocks[&ordinal])
    }

    pub(crate) fn range(
        &mut self,
        offset: u64,
        count: usize,
        budget: &mut usize,
    ) -> io::Result<Vec<u8>> {
        let mut result = Vec::with_capacity(count);
        while result.len() < count {
            let at = offset + result.len() as u64;
            let within = (at % SIZE as u64) as usize;
            let block = self.block(at / SIZE as u64, budget)?;
            let size = (count - result.len()).min(block.len().saturating_sub(within));
            if size == 0 {
                return Err(missing());
            }
            result.extend_from_slice(&block[within..within + size]);
        }
        self.candidate.verify()?;
        Ok(result)
    }
}

#[cfg(test)]
pub(crate) fn prepare(archive: &Archive, path: &Path, source: &SessionSourceRef) -> io::Result<()> {
    for candidate in candidates(archive, path, source, None)? {
        candidate.build()?;
    }
    Ok(())
}
