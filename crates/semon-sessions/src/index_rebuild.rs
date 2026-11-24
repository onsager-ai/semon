//! Rebuild a refused Index from host custody. Only a complete database is
//! linked into the active placement. Unsupported databases are never opened
//! by the replay or renamed, and a crash cannot publish a work in progress.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    Input, InputRoot, Options,
    archive::{Archive, NativeSource, RecoveryRecord},
    events::{self, CACHE_VERSION, EventCache, Ledger, Stat, store},
    sealed::LogFile,
    source_change::SourceLock,
    storage::{Namespace, sqlite},
};

/// Receiver-wide custody admission, shared with every received machine's rebuild.
#[derive(Debug, Default)]
pub struct ReceivedCustody {
    state: Mutex<CustodyState>,
    drained: Condvar,
}

#[derive(Debug, Default)]
struct CustodyState {
    writes: usize,
    rebuilds: usize,
}

impl ReceivedCustody {
    /// Admit an append or facts write before it takes any of its write locks.
    /// The caller keeps the guard until all of those writes have finished.
    pub fn admit_write(&self) -> Option<ReceivedWrite<'_>> {
        let mut state = crate::viewer::lock(&self.state);
        if state.rebuilds != 0 {
            return None;
        }
        state.writes += 1;
        Some(ReceivedWrite(self))
    }

    fn pause(&self) -> ReceivedRebuild<'_> {
        let mut state = crate::viewer::lock(&self.state);
        state.rebuilds += 1;
        drop(
            self.drained
                .wait_while(state, |state| state.writes != 0)
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        ReceivedRebuild(self)
    }
}

/// Holds admission through a custody write and all of its locked updates.
pub struct ReceivedWrite<'a>(&'a ReceivedCustody);

impl Drop for ReceivedWrite<'_> {
    fn drop(&mut self) {
        let mut state = crate::viewer::lock(&self.0.state);
        state.writes -= 1;
        if state.writes == 0 {
            self.0.drained.notify_all();
        }
    }
}

struct ReceivedRebuild<'a>(&'a ReceivedCustody);

impl Drop for ReceivedRebuild<'_> {
    fn drop(&mut self) {
        crate::viewer::lock(&self.0.state).rebuilds -= 1;
    }
}

fn rebuilt_path(cache: &Path) -> PathBuf {
    cache.with_extension(format!(
        "rebuilt-{}-{CACHE_VERSION}.sqlite3",
        store::SCHEMA_VERSION
    ))
}

pub(crate) fn active_path(cache: &Path) -> PathBuf {
    let rebuilt = rebuilt_path(cache);
    if fs::symlink_metadata(&rebuilt).is_ok_and(|meta| meta.is_file()) {
        rebuilt
    } else {
        cache.with_extension("sqlite3")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Rebuilding { done: usize, total: usize },
    Failed(String),
}

fn statuses() -> &'static Mutex<BTreeMap<PathBuf, Status>> {
    static STATES: OnceLock<Mutex<BTreeMap<PathBuf, Status>>> = OnceLock::new();
    STATES.get_or_init(Mutex::default)
}

pub(crate) fn status(cache: &Path) -> Option<Status> {
    let state = crate::viewer::lock(statuses()).get(cache).cloned();
    if matches!(state, Some(Status::Failed(_))) && active_path(cache) == rebuilt_path(cache) {
        // Another host sharing this placement may have finished recovery.
        return None;
    }
    state
}

pub(crate) fn history_reply(options: &Options) -> Option<crate::ViewerReply> {
    use serde_json::json;
    let state = status(&options.cache)?;
    let (code, message, progress) = match state {
        Status::Rebuilding { done, total } => (
            "history_rebuilding",
            format!("Rebuilding history ({done} of {total} sources)"),
            Some(json!({"done": done, "total": total})),
        ),
        Status::Failed(message) => ("history_recovery_failed", message, None),
    };
    let retryable = progress.is_some();
    Some(crate::ViewerReply {
        status: 503,
        content_type: "application/json; charset=utf-8",
        body: json!({"error": {"code": code, "message": message, "retryable": retryable, "progress": progress}}).to_string().into_bytes(),
        etag: None,
        extra_headers: if retryable { vec![("Retry-After", "1".into())] } else { Vec::new() },
    })
}

pub(crate) fn needed(options: &Options) -> io::Result<bool> {
    if !options.index_recovery.allowed() {
        return Ok(false);
    }
    let path = active_path(&options.cache);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
        Ok(meta) if !meta.is_file() => Err(io::Error::other("Index path is not a regular file")),
        Ok(_) => store::rebuild_probe(&path),
    }
}

/// Mark an owned placement before queuing recovery on the existing producer.
pub(crate) fn queue(options: &Options) -> bool {
    if !needed(options).unwrap_or(false) {
        return false;
    }
    crate::viewer::lock(statuses())
        .entry(options.cache.clone())
        .or_insert(Status::Rebuilding { done: 0, total: 0 });
    true
}

pub(crate) fn ensure(options: &Options, namespace: &Namespace) -> io::Result<()> {
    if !options.index_recovery.allowed() {
        return Ok(());
    }
    if !needed(options)? {
        // A different process can publish after this host queued recovery.
        crate::viewer::lock(statuses()).remove(&options.cache);
        return Ok(());
    }
    recover(options, namespace, false)
}

/// Also called after a normal writer refuses a contract whose latest state was
/// in the WAL. All publication paths serialize under the same reservation.
pub(crate) fn recover(options: &Options, namespace: &Namespace, refused: bool) -> io::Result<()> {
    let final_path = rebuilt_path(&options.cache);
    let _lock = SourceLock::acquire(options, "\0index-rebuild", true)?;
    let path = active_path(&options.cache);
    if (path == final_path || !refused) && !needed(options)? {
        crate::viewer::lock(statuses()).remove(&options.cache);
        return Ok(());
    }
    let progress = |status| {
        crate::viewer::lock(statuses()).insert(options.cache.clone(), status);
    };
    progress(Status::Rebuilding { done: 0, total: 0 });
    let _pause = match &options.index_recovery {
        crate::IndexRecovery::Received { custody, .. } => Some(custody.pause()),
        _ => None,
    };
    let result = (|| {
        if !verify_refusal(&path)? {
            return Ok(());
        }
        rebuild(options, namespace, &final_path, |done, total| {
            progress(Status::Rebuilding { done, total });
        })
    })();
    match result {
        Ok(()) => {
            crate::viewer::lock(statuses()).remove(&options.cache);
            Ok(())
        }
        Err(error) => {
            let message = format!("history recovery failed: {error}");
            eprintln!("semon: {}: {message}", options.cache.display());
            progress(Status::Failed(message.clone()));
            Err(io::Error::other(message))
        }
    }
}

fn verify_refusal(path: &Path) -> io::Result<bool> {
    let wal = path.with_extension("sqlite3-wal");
    let observed = match fs::symlink_metadata(&wal) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => Stat::from_metadata(&metadata)?,
        Ok(metadata) if !metadata.is_file() => {
            return Err(io::Error::other("Index WAL is not a regular file"));
        }
        Ok(_) => return store::rebuild_probe(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return store::rebuild_probe(path),
        Err(error) => return Err(error),
    };
    let db_stat = Stat::of(path)?;
    #[cfg(unix)]
    let result = store::rebuild_probe_wal(path)?;
    #[cfg(not(unix))]
    let result = false;
    if Stat::of(path)? != db_stat || Stat::of(&wal)? != observed {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "refused Index changed during qualification",
        ));
    }
    #[cfg(not(unix))]
    return Err(io::Error::other(
        "cannot qualify a refused Index with a pending WAL on this platform",
    ));
    #[cfg(unix)]
    Ok(result)
}

struct Stage(PathBuf);

impl Stage {
    fn new(parent: &Path) -> io::Result<Self> {
        let mut id = [0_u8; 16];
        getrandom::fill(&mut id).map_err(|error| io::Error::other(error.to_string()))?;
        let path = parent.join(format!(".index-rebuild-{}", crate::sealed::hex(&id)));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        // The stage belongs to this invocation, and holds only derived data.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Serialize the receiver's marker update with its short recovery observation.
/// Only the receiver process grants this custody; native replay never holds
/// this lock while hashing or parsing a log.
pub fn with_received_file_lock<T>(path: &Path, operation: impl FnOnce() -> T) -> T {
    type FileLocks = Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>;
    static LOCKS: OnceLock<FileLocks> = OnceLock::new();
    let locks = LOCKS.get_or_init(Mutex::default);
    let entry = Arc::clone(
        crate::viewer::lock(locks)
            .entry(path.to_owned())
            .or_default(),
    );
    let result = {
        let _guard = crate::viewer::lock(&entry);
        operation()
    };
    let mut locks = crate::viewer::lock(locks);
    if Arc::strong_count(&entry) == 2 {
        locks.remove(path);
    }
    result
}

struct Original {
    stat: Stat,
    generation: Option<String>,
}

fn is_original(input: &Input) -> bool {
    input.path.ends_with(".jsonl")
}

fn local_inventory(options: &Options) -> io::Result<BTreeMap<Input, Original>> {
    crate::inputs(options)?
        .into_iter()
        .filter(is_original)
        .map(|input| {
            let path = input.full_path(options);
            let stat = Stat::from_metadata(&crate::open_input(&path)?.metadata()?)?;
            Ok((
                input,
                Original {
                    stat,
                    generation: None,
                },
            ))
        })
        .collect()
}

fn read_native_archive(options: &Options, archive: &Archive) -> io::Result<Vec<RecoveryRecord>> {
    Ok(archive
        .recovery_inventory()?
        .into_iter()
        .filter(|record| standalone_input(options, &record.source).is_some())
        .collect())
}

fn standalone_input(options: &Options, path: &str) -> Option<Input> {
    [InputRoot::Claude, InputRoot::Codex, InputRoot::Copilot]
        .into_iter()
        .find_map(|root| {
            let path = Path::new(path)
                .strip_prefix(root.home(options))
                .ok()?
                .to_str()?;
            (path.ends_with(".jsonl") && crate::is_input_path(root.as_str(), path)).then(|| Input {
                root,
                path: path.to_owned(),
            })
        })
}

fn received_input(path: &str) -> Option<Input> {
    let (harness, path) = path.split_once('/')?;
    let root = InputRoot::parse(harness)?;
    (path.ends_with(".jsonl") && crate::is_input_path(harness, path)).then(|| Input {
        root,
        path: path.to_owned(),
    })
}

#[derive(Deserialize)]
struct ReceiverGeneration {
    identity: ReceiverIdentity,
    generation: String,
    length: u64,
    head_sha256: String,
}

#[derive(Deserialize)]
struct ReceiverIdentity {
    len: u64,
    mtime_ns: Option<i128>,
    dev: u64,
    ino: u64,
}

type Originals = BTreeMap<Input, Original>;
type ReceiverSeals = BTreeMap<Input, Option<crate::archive::Chain>>;

fn receiver_inventory(options: &Options, root: &Path) -> io::Result<(Originals, ReceiverSeals)> {
    // Enumerate markers first so a newly received input cannot leave a marker
    // in this inventory without its carrier. Appends replace a marker under
    // the same file lock that protects the binding observation below.
    let marker_root = root.join("sync-generations");
    let mut markers = BTreeSet::new();
    match fs::symlink_metadata(&marker_root) {
        Ok(meta) if meta.is_dir() => {
            for entry in fs::read_dir(&marker_root)? {
                let name = entry?.file_name().to_string_lossy().into_owned();
                if name.ends_with(".json") && !name.starts_with('.') {
                    markers.insert(name);
                }
            }
        }
        Ok(_) => {
            return Err(io::Error::other(
                "receiver generation directory is not regular",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let facts_path = root.join("facts.json");
    let rollouts = match fs::symlink_metadata(&facts_path) {
        Ok(metadata) if metadata.is_file() => {
            let facts = crate::received::read_facts(&facts_path)?;
            [facts.codex_rollouts, facts.codex_provisional_rollouts]
        }
        Ok(_) => return Err(io::Error::other("receiver Facts is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => [None, None],
        Err(error) => return Err(error),
    };
    let mut originals = BTreeMap::new();
    for input in crate::inputs(options)? {
        if !crate::is_input_path(input.root.as_str(), &input.path) {
            continue;
        }
        let key = format!("{}/{}", input.root.as_str(), input.path);
        let name = format!("{:x}.json", Sha256::digest(key));
        let path = input.full_path(options);
        let original = with_received_file_lock(&path, || {
            observe_received(&path, &marker_root.join(&name), markers.remove(&name))
        })?;
        if is_original(&input) {
            originals.insert(input, original);
        }
    }
    if let Some(name) = markers.first() {
        return Err(io::Error::other(format!(
            "receiver generation {name} has no authorized original"
        )));
    }
    for path in rollouts.iter().flatten().flatten() {
        let input = Input {
            root: InputRoot::Codex,
            path: path.clone(),
        };
        if !originals.contains_key(&input) {
            return Err(io::Error::other(format!(
                "receiver Facts references missing Codex source {path}"
            )));
        }
    }
    let archive = Archive::new(root.join(crate::archive::DIR));
    let seals = crate::inputs::receiver_seal_inputs(options)?
        .into_iter()
        .map(|input| {
            let path = input.full_path(options);
            let source = format!("{}/{}", input.root.as_str(), input.path);
            let custody = crate::sealed::verify_recovery_custody(
                &path,
                &source,
                &archive,
                originals.contains_key(&input),
            )?;
            Ok((input, custody))
        })
        .collect::<io::Result<_>>()?;
    Ok((originals, seals))
}

fn observe_received(path: &Path, marker_path: &Path, had_marker: bool) -> io::Result<Original> {
    let file = crate::open_input(path)?;
    let metadata = file.metadata()?;
    let stat = Stat::from_metadata(&metadata)?;
    let marker = match crate::open_input(marker_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !had_marker => {
            return Ok(Original {
                stat,
                generation: None,
            });
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    marker.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("receiver generation record is oversized"));
    }
    let marker: ReceiverGeneration = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let mut head = Vec::new();
    file.take(4096).read_to_end(&mut head)?;
    let signed_mtime = match metadata.modified()?.duration_since(std::time::UNIX_EPOCH) {
        Ok(time) => time.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    if marker.generation.len() != 32
        || !marker
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || marker.length != stat.size
        || marker.identity.len != stat.size
        || marker.identity.dev != stat.dev
        || marker.identity.ino != stat.ino
        || marker.identity.mtime_ns != Some(signed_mtime)
        || marker.head_sha256 != format!("{:x}", Sha256::digest(head))
    {
        return Err(io::Error::other(format!(
            "receiver generation changed for {}",
            path.display()
        )));
    }
    Ok(Original {
        stat,
        generation: Some(marker.generation),
    })
}

fn inventory(options: &Options) -> io::Result<(Originals, ReceiverSeals)> {
    match options.index_recovery.receiver_custody() {
        Some(root) => receiver_inventory(options, root),
        None => Ok((local_inventory(options)?, BTreeMap::new())),
    }
}

fn boundary_changed(path: &Path) -> io::Error {
    io::Error::other(format!("original boundary changed for {}", path.display()))
}

fn hash_bytes(log: &mut LogFile, length: u64, hash: &mut Sha256, path: &Path) -> io::Result<()> {
    if io::copy(&mut log.take(length), hash)? != length {
        return Err(boundary_changed(path));
    }
    Ok(())
}

fn original_proof(
    path: &Path,
    expected: Stat,
    observed: Stat,
    ledger: Option<&Ledger>,
) -> io::Result<[u8; 32]> {
    let mut log = LogFile::open(path)?;
    let stat = Stat::from_metadata(&log.metadata()?)?;
    if !observed.is_prefix_of(&stat) {
        return Err(boundary_changed(path));
    }
    let mut hash = Sha256::new();
    if let Some(ledger) = ledger {
        if ledger.stat != expected || ledger.offset > expected.size {
            return Err(boundary_changed(path));
        }
        hash_bytes(&mut log, ledger.offset, &mut hash, path)?;
        if <[u8; 32]>::from(hash.clone().finalize()) != ledger.prefix {
            return Err(boundary_changed(path));
        }
        // The replay must cover every complete record in the captured range.
        // The bytes after its receipt may only be an unfinished last line.
        let mut remaining = expected.size - ledger.offset;
        let mut buffer = [0; 8192];
        while remaining != 0 {
            let len = remaining.min(buffer.len() as u64) as usize;
            let chunk = &mut buffer[..len];
            log.read_exact(chunk)?;
            if chunk.contains(&b'\n') {
                return Err(boundary_changed(path));
            }
            hash.update(chunk);
            remaining -= len as u64;
        }
    } else {
        hash_bytes(&mut log, expected.size, &mut hash, path)?;
    }
    let current = Stat::from_metadata(&crate::open_input(path)?.metadata()?)?;
    if !stat.is_prefix_of(&current) || (current.size == stat.size && current != stat) {
        return Err(boundary_changed(path));
    }
    Ok(hash.finalize().into())
}

fn rebuild(
    options: &Options,
    namespace: &Namespace,
    final_path: &Path,
    progress: impl Fn(usize, usize),
) -> io::Result<()> {
    if final_path.try_exists()? {
        return Err(io::Error::other(
            "the rebuilt Index placement is already occupied",
        ));
    }
    let (originals, seals) = inventory(options)?;
    let mut proofs = BTreeMap::new();
    for (input, original) in &originals {
        let path = input.full_path(options);
        proofs.insert(
            input,
            original_proof(&path, original.stat, original.stat, None)?,
        );
    }
    let archive = Archive::new(crate::archive::standalone_dir(&options.cache));
    let native_archive = read_native_archive(options, &archive)?;
    let received_archive = options
        .index_recovery
        .receiver_custody()
        .map(|root| Archive::new(root.join(crate::archive::DIR)));
    let received = match &received_archive {
        Some(archive) => archive.recovery_inventory()?,
        None => Vec::new(),
    };
    let mut archives = BTreeMap::<Input, Vec<(&Archive, &RecoveryRecord)>>::new();
    for record in &native_archive {
        let input = standalone_input(options, &record.source).expect("authorized native archive");
        archives.entry(input).or_default().push((&archive, record));
    }
    for record in &received {
        if let Some(input) = received_input(&record.source) {
            archives
                .entry(input)
                .or_default()
                .push((received_archive.as_ref().expect("receiver archive"), record));
        }
    }
    let sources: BTreeSet<_> = originals.keys().chain(archives.keys()).collect();
    let total = sources.len();
    progress(0, total);
    let parent = final_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stage = Stage::new(parent)?;
    let index = stage.0.join("index.sqlite3");
    let mut cache = EventCache::open_scoped_at(&index, &options.cache).map_err(io::Error::other)?;
    sqlite::bind(&mut cache, namespace).map_err(io::Error::other)?;
    let mut ledgers = BTreeMap::new();
    for (done, input) in sources.into_iter().enumerate() {
        let path = input.full_path(options);
        let live = originals.get(input).map(|original| original.stat);
        let mut native: Option<&NativeSource> = None;
        if let Some(records) = archives.get(input) {
            for (archive, record) in records {
                if let Some(admitted) = record.native.as_ref()
                    && (!admitted.allowed(options) || native.replace(admitted).is_some())
                {
                    return Err(io::Error::other("ambiguous native archive binding"));
                }
                for chain in &record.chains {
                    archive.with_verified_chain(&record.source, chain, |bytes| {
                        if native.is_some_and(|admitted| admitted.chain_id == chain.chain_id)
                            && live.is_none()
                        {
                            return Ok(()); // Replay below with the native receipt and sidecar.
                        }
                        events::replay_archive_history(
                            &mut cache,
                            &stage.0,
                            &path,
                            input.root.as_str(),
                            chain,
                            bytes,
                            live,
                        )
                    })?;
                }
            }
        }
        if let Some(stat) = live {
            events::scan_rebuild_file(&path, input.root.as_str(), &mut cache, stat)?;
            ledgers.insert(input, cache.committed_rebuild_ledger(&path)?);
        } else if let Some(native) = native {
            events::scan_archive(options, native, &mut cache, &mut false)?;
            if cache.committed_rebuild_ledger(&path)? != native.ledger {
                return Err(io::Error::other("archive boundary was not committed"));
            }
        }
        progress(done + 1, total);
    }
    drop(cache);
    store::checkpoint_rebuilt_index(&index)?;
    let (current, current_seals) = inventory(options)?;
    if current_seals != seals
        || read_native_archive(options, &archive)? != native_archive
        || received_archive
            .as_ref()
            .map(Archive::recovery_inventory)
            .transpose()?
            .unwrap_or_default()
            != received
    {
        return Err(io::Error::other("source custody changed during rebuild"));
    }
    for (input, original) in &originals {
        let path = input.full_path(options);
        let current = current.get(input).ok_or_else(|| boundary_changed(&path))?;
        if !original.stat.is_prefix_of(&current.stat)
            || original
                .generation
                .as_ref()
                .is_some_and(|generation| current.generation.as_ref() != Some(generation))
            || original_proof(&path, original.stat, current.stat, Some(&ledgers[input]))?
                != proofs[input]
        {
            return Err(boundary_changed(&path));
        }
    }
    // The completed database is durable before the single atomic publication.
    // A hard link refuses occupied paths without replacing a previous Index.
    fs::File::open(&index)?.sync_all()?;
    fs::hard_link(&index, final_path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Ledger;

    #[test]
    fn receiver_rebuild_drains_writes_and_keeps_admission_closed_across_rebuilds() {
        use std::{
            sync::mpsc,
            thread,
            time::{Duration, Instant},
        };

        let custody = ReceivedCustody::default();
        let write = custody.admit_write().unwrap();
        let (snapshot, started) = mpsc::channel();
        let (publish, finish) = mpsc::channel();
        thread::scope(|scope| {
            let custody = &custody;
            let rebuild = scope.spawn(move || {
                let _pause = custody.pause();
                snapshot.send(()).unwrap();
                let _ = finish.recv_timeout(Duration::from_secs(10));
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while custody.admit_write().is_some() && Instant::now() < deadline {
                thread::yield_now();
            }
            let refused = custody.admit_write().is_none();
            let before_drain = started.try_recv();
            drop(write);
            assert!(refused);
            assert!(matches!(before_drain, Err(mpsc::TryRecvError::Empty)));
            started.recv_timeout(Duration::from_secs(10)).unwrap();
            let another_rebuild = custody.pause();
            publish.send(()).unwrap();
            rebuild.join().unwrap();
            assert!(custody.admit_write().is_none());
            drop(another_rebuild);
        });
        assert!(custody.admit_write().is_some());
    }

    fn fixture() -> (Stage, Options) {
        let stage = Stage::new(&std::env::temp_dir()).unwrap();
        let options = Options {
            cache: stage.0.join("sessions-index.json"),
            claude_home: stage.0.join("claude"),
            codex_home: stage.0.join("codex"),
            copilot_home: stage.0.join("copilot"),
            index_recovery: crate::IndexRecovery::Local,
            ..Options::default()
        };
        (stage, options)
    }

    fn source(options: &Options, id: &str) -> (PathBuf, Vec<u8>) {
        let path = options
            .claude_home
            .join(format!("projects/project/{id}.jsonl"));
        let bytes = format!("{{\"type\":\"user\",\"sessionId\":\"{id}\",\"uuid\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"fixture\"}}}}\n").into_bytes();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    }

    fn archive(options: &Options, id: &str) -> PathBuf {
        let (path, bytes) = source(options, id);
        archive_source(options, InputRoot::Claude, &path, id, &bytes);
        path
    }

    fn archive_source(options: &Options, root: InputRoot, path: &Path, id: &str, bytes: &[u8]) {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let home = root.home(options);
        let native = NativeSource {
            home: home.clone(),
            root: root.as_str().into(),
            path: path.strip_prefix(home).unwrap().to_str().unwrap().into(),
            native_id: id.into(),
            ledger: Ledger {
                stat: Stat::of(path).unwrap(),
                offset: bytes.len() as u64,
                prefix: digest,
                tail: digest,
            },
            sidecar: None,
            chain_id: String::new(),
        };
        let archive = Archive::new(crate::archive::standalone_dir(&options.cache));
        archive
            .admit_native(&mut LogFile::open(path).unwrap(), &native)
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn catalog_and_transcript_report_rebuild_progress() {
        let (_stage, options) = fixture();
        refused(&options, 49);
        let core = crate::ViewerCore::new(options.clone());
        let reader = || {
            for (endpoint, query) in [
                ("/api/sessions", "machine=local"),
                ("/api/session-transcript", "machine=local&sid=fixture"),
            ] {
                let reply = core.respond("GET", endpoint, query, None);
                assert_eq!(reply.status, 503);
                assert!(reply.extra_headers.contains(&("Retry-After", "1".into())));
                let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
                assert_eq!(body["error"]["code"], "history_rebuilding");
                assert_eq!(
                    body["error"]["progress"],
                    serde_json::json!({ "done": 1, "total": 3 })
                );
            }
        };
        // Hold the rebuild reservation while exercising the HTTP core. The
        // queued producer cannot change this deterministic progress fixture.
        let lock = SourceLock::acquire(&options, "\0index-rebuild", true).unwrap();
        crate::viewer::lock(statuses()).insert(
            options.cache.clone(),
            Status::Rebuilding { done: 1, total: 3 },
        );
        reader();
        crate::viewer::lock(statuses()).remove(&options.cache);
        drop(lock);
        core.close();
    }

    fn refused(options: &Options, version: i64) -> PathBuf {
        let old = active_path(&options.cache);
        let db = rusqlite::Connection::open(&old).unwrap();
        db.execute_batch(
            "CREATE TABLE unrelated (content TEXT); INSERT INTO unrelated VALUES('old only');",
        )
        .unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        drop(db);
        old
    }

    #[test]
    fn host_rebuilds_unsupported_stores_from_native_and_archive_custody() {
        for version in [0, store::SCHEMA_VERSION - 1, store::SCHEMA_VERSION + 1] {
            let (_stage, options) = fixture();
            let (live, bytes) = source(&options, "live");
            let archived = archive(&options, "archive-only");
            let copilot = options
                .copilot_home
                .join("session-state/copilot-fixture/events.jsonl");
            let copilot_text = include_str!("../tests/fixtures/star-catalog/copilot.jsonl");
            fs::create_dir_all(copilot.parent().unwrap()).unwrap();
            fs::write(&copilot, copilot_text).unwrap();
            let archived_copilot = options
                .copilot_home
                .join("session-state/copilot-archive/events.jsonl");
            let archived_text = copilot_text.replace("copilot-fixture", "copilot-archive");
            fs::create_dir_all(archived_copilot.parent().unwrap()).unwrap();
            fs::write(&archived_copilot, &archived_text).unwrap();
            archive_source(
                &options,
                InputRoot::Copilot,
                &archived_copilot,
                "copilot-archive",
                archived_text.as_bytes(),
            );
            let old = refused(&options, version);
            let original = fs::read(&old).unwrap();
            assert!(EventCache::open_scoped(&options.cache).is_err());
            assert_eq!(fs::read(&old).unwrap(), original);
            let namespace = Namespace::new("local").unwrap();
            let cache = sqlite::open_index(&options, &namespace).unwrap();
            assert_eq!(active_path(&options.cache), rebuilt_path(&options.cache));
            assert_eq!(fs::read(&old).unwrap(), original);
            assert_eq!(fs::read(&live).unwrap(), bytes);
            assert_eq!(
                cache.committed_rebuild_ledger(&live).unwrap().offset,
                bytes.len() as u64
            );
            assert!(cache.committed_rebuild_ledger(&archived).is_ok());
            assert_eq!(
                cache.committed_rebuild_ledger(&copilot).unwrap().offset,
                copilot_text.len() as u64
            );
            assert_eq!(
                cache
                    .committed_rebuild_ledger(&archived_copilot)
                    .unwrap()
                    .offset,
                archived_text.len() as u64
            );
            let db = rusqlite::Connection::open(active_path(&options.cache)).unwrap();
            let usage = db.prepare(
                "SELECT d.native_id,u.input FROM dim_session d JOIN current_copilot_usage u USING(source) ORDER BY d.native_id",
            ).unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
                .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            assert_eq!(
                usage,
                [
                    ("copilot-archive".to_owned(), 12000),
                    ("copilot-fixture".to_owned(), 12000)
                ]
            );
            assert!(!archived_copilot.exists());
            assert!(!archived.exists());
            assert_eq!(status(&options.cache), None);
        }
    }

    #[test]
    fn receiver_republishes_catalog_on_a_current_rebuilt_index() {
        use crate::native_evidence::{NativeEvidence, Work};

        for with_source in [true, false] {
            let (stage, mut options) = fixture();
            let facts = stage.0.join("facts.json");
            crate::write_facts(
                &facts,
                &crate::Facts {
                    version: crate::facts::FACTS_VERSION,
                    ..Default::default()
                },
            )
            .unwrap();
            options.facts = Some(facts);
            options.index_recovery = crate::IndexRecovery::Received {
                root: stage.0.clone(),
                custody: Default::default(),
            };
            options.annotations = stage.0.join("annotations");
            let native = with_source.then(|| source(&options, "live"));
            if let Some((path, _)) = &native {
                write_generation(&stage.0, path);
            }
            let original = refused(&options, store::SCHEMA_VERSION - 1);
            let original_bytes = fs::read(&original).unwrap();
            let namespace = Namespace::new("machine").unwrap();
            let refresh = || {
                let evidence = NativeEvidence::for_source(namespace.clone(), options.clone());
                let mut work = Work::default();
                assert!((0..24).any(|_| {
                    evidence
                        .refresh_catalog(&mut work, crate::model::now_ms())
                        .unwrap()
                }));
            };
            refresh();
            let rebuilt = rebuilt_path(&options.cache);
            assert_eq!(active_path(&options.cache), rebuilt);
            let read_state = |db: &rusqlite::Connection| {
                db.query_row(
                    "SELECT (SELECT value FROM meta WHERE key='incarnation'),(SELECT value FROM meta WHERE key='catalog_generation')",
                    [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                ).unwrap()
            };
            let db = rusqlite::Connection::open(&rebuilt).unwrap();
            let (index_before, catalog_before) = read_state(&db);
            let previous = crate::model::summary::CATALOG_VERSION - 1;
            sqlite::set_creation_projector_for_test(&db, previous).unwrap();
            db.execute(
                "UPDATE meta SET value=?1 WHERE key='catalog_version'",
                [previous.to_string()],
            )
            .unwrap();
            drop(db);

            assert!(!crate::catalog::sqlite_page_ready(&options).unwrap());
            assert!(store::catalog_work_pending(&options).unwrap());
            assert!(!needed(&options).unwrap());
            // A recovery request that lost a publication race must accept
            // this occupied placement as the valid active Index.
            recover(&options, &namespace, true).unwrap();
            assert_eq!(status(&options.cache), None);
            refresh();
            assert!(crate::catalog::sqlite_page_ready(&options).unwrap());

            let db = rusqlite::Connection::open(&rebuilt).unwrap();
            let (index_after, catalog_after) = read_state(&db);
            assert_eq!(index_after, index_before);
            assert_ne!(catalog_after, catalog_before);
            assert_eq!(active_path(&options.cache), rebuilt);
            assert_eq!(fs::read(&original).unwrap(), original_bytes);
            if let Some((path, bytes)) = native {
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
            let core = crate::ViewerCore::with_machines(vec![("machine".into(), options)]);
            let reply = core.respond("GET", "/api/sessions", "machine=machine", None);
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
            let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(
                body["items"].as_array().unwrap().len(),
                usize::from(with_source)
            );
            core.close();
        }
    }

    #[test]
    fn missing_custody_chain_or_archive_journal_leaves_no_active_rebuild() {
        for missing_journal in [false, true] {
            let (_stage, options) = fixture();
            let archived = archive(&options, "archive-only");
            let old = refused(&options, 49);
            let original = fs::read(&old).unwrap();
            let archive_root = crate::archive::standalone_dir(&options.cache);
            let journal = archive_root.join("sources.log");
            if missing_journal {
                fs::remove_file(journal).unwrap();
            } else {
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(journal).unwrap()).unwrap();
                let archive = Archive::new(&archive_root);
                let chains = archive.chains(&archived.to_string_lossy()).unwrap();
                fs::remove_file(
                    archive_root
                        .join(record["id"].as_str().unwrap())
                        .join(format!("{}.zst", chains[0].chain_id)),
                )
                .unwrap();
            }
            let namespace = Namespace::new("local").unwrap();
            let result = ensure(&options, &namespace);
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("history recovery failed")
            );
            assert_eq!(active_path(&options.cache), old);
            assert_eq!(fs::read(&old).unwrap(), original);
            assert!(!rebuilt_path(&options.cache).exists());
        }
    }

    fn write_generation(root: &Path, path: &Path) {
        let directory = root.join("sync-generations");
        fs::create_dir_all(&directory).unwrap();
        let key = path.strip_prefix(root).unwrap().to_str().unwrap();
        let key = Sha256::digest(key.replace(std::path::MAIN_SEPARATOR, "/"));
        let metadata = fs::metadata(path).unwrap();
        let stat = Stat::from_metadata(&metadata).unwrap();
        let mut head = Vec::new();
        fs::File::open(path)
            .unwrap()
            .take(4096)
            .read_to_end(&mut head)
            .unwrap();
        let marker = serde_json::json!({
            "generation": "a".repeat(32),
            "length": stat.size,
            "identity": {"len": stat.size, "mtime_ns": stat.modified_ns, "dev": stat.dev, "ino": stat.ino},
            "head_sha256": format!("{:x}", Sha256::digest(head)),
        });
        fs::write(directory.join(format!("{key:x}.json")), marker.to_string()).unwrap();
    }

    #[test]
    fn local_and_receiver_rebuilds_cut_a_boundary_while_a_writer_appends() {
        use std::{io::Write, sync::mpsc, thread, time::Duration};

        for receiver in [false, true] {
            let (stage, mut options) = fixture();
            if receiver {
                options.index_recovery = crate::IndexRecovery::Received {
                    root: stage.0.clone(),
                    custody: Default::default(),
                };
            }
            let (path, record) = source(&options, "live");
            if receiver {
                write_generation(&stage.0, &path);
            }
            let old = refused(&options, 49);
            let before = fs::read(&old).unwrap();
            let namespace = Namespace::new(if receiver { "machine" } else { "local" }).unwrap();
            let appends = thread::scope(|scope| {
                let (append, requested) = mpsc::channel();
                let (written, observe) = mpsc::channel();
                let path = &path;
                let record = &record;
                let root = &stage.0;
                scope.spawn(move || {
                    for () in requested {
                        with_received_file_lock(path, || {
                            fs::OpenOptions::new()
                                .append(true)
                                .open(path)
                                .unwrap()
                                .write_all(record)
                                .unwrap();
                            if receiver {
                                write_generation(root, path);
                            }
                        });
                        written.send(fs::metadata(path).unwrap().len()).unwrap();
                    }
                });
                rebuild(
                    &options,
                    &namespace,
                    &rebuilt_path(&options.cache),
                    |done, total| {
                        assert_eq!(total, 1);
                        assert!(done <= total);
                        append.send(()).unwrap();
                        let size = observe.recv_timeout(Duration::from_secs(10)).unwrap();
                        assert_eq!(size, (done as u64 + 2) * record.len() as u64);
                    },
                )
                .unwrap();
                assert_eq!(active_path(&options.cache), rebuilt_path(&options.cache));
                // A further record lands after publication.
                append.send(()).unwrap();
                observe.recv_timeout(Duration::from_secs(10)).unwrap()
            });
            assert_eq!(appends, 4 * record.len() as u64);
            assert_eq!(fs::read(old).unwrap(), before);
            let mut cache = sqlite::open_index(&options, &namespace).unwrap();
            let ledger = cache.committed_rebuild_ledger(&path).unwrap();
            assert_eq!(ledger.offset, record.len() as u64);
            assert_eq!(ledger.prefix, <[u8; 32]>::from(Sha256::digest(&record)));
            events::scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            assert_eq!(
                cache.committed_rebuild_ledger(&path).unwrap().offset,
                appends
            );
        }
    }

    #[test]
    fn rewritten_or_truncated_original_during_rebuild_refuses_publication() {
        for truncate in [false, true] {
            let (_stage, options) = fixture();
            let (path, record) = source(&options, "live");
            let old = refused(&options, 49);
            let before = fs::read(&old).unwrap();
            let rewritten = if truncate {
                Vec::new()
            } else {
                // Retain the inode and grow, but alter bytes inside the prefix.
                let changed = String::from_utf8(record.clone())
                    .unwrap()
                    .replace("fixture", "changed");
                [changed.as_bytes(), &record].concat()
            };
            let namespace = Namespace::new("local").unwrap();
            let error = rebuild(
                &options,
                &namespace,
                &rebuilt_path(&options.cache),
                |done, total| {
                    if done == total {
                        fs::write(&path, &rewritten).unwrap();
                    }
                },
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("original boundary changed"),
                "{error}"
            );
            assert_eq!(active_path(&options.cache), old);
            assert_eq!(fs::read(old).unwrap(), before);
            assert_eq!(fs::read(path).unwrap(), rewritten);
        }
    }

    #[test]
    fn unmatched_receiver_generation_stops_recovery() {
        let (stage, mut options) = fixture();
        options.index_recovery = crate::IndexRecovery::Received {
            root: stage.0.clone(),
            custody: Default::default(),
        };
        let (path, _) = source(&options, "removed");
        fs::remove_file(path).unwrap();
        let markers = stage.0.join("sync-generations");
        fs::create_dir(&markers).unwrap();
        let key = Sha256::digest(b"claude/projects/project/removed.jsonl");
        fs::write(markers.join(format!("{key:x}.json")), b"{}").unwrap();
        let old = refused(&options, 49);
        let namespace = Namespace::new("machine").unwrap();
        assert!(
            ensure(&options, &namespace)
                .unwrap_err()
                .to_string()
                .contains("has no authorized original")
        );
        assert_eq!(active_path(&options.cache), old);
    }

    #[test]
    fn receiver_replays_verified_archived_generations_without_live_authority() {
        let (stage, mut options) = fixture();
        let receiver = stage.0.join("machine");
        options.index_recovery = crate::IndexRecovery::Received {
            root: receiver.clone(),
            custody: Default::default(),
        };
        options.claude_home = receiver.join("claude");
        let archive = Archive::new(receiver.join(crate::archive::DIR));
        let (path, bytes) = source(&options, "raw-only");
        let key = "claude/projects/project/raw-only.jsonl";
        archive
            .preserve(key, &mut bytes.as_slice(), crate::archive::NATIVE_REDACTOR)
            .unwrap()
            .unwrap();
        fs::remove_file(&path).unwrap();
        refused(&options, 49);
        let cache = sqlite::open_index(&options, &Namespace::new("machine").unwrap()).unwrap();
        assert!(cache.committed_rebuild_ledger(&path).is_err());
        let db = store::focused_connection(&active_path(&options.cache)).unwrap();
        let rows: (i64, i64) = db.query_row(
            "SELECT (SELECT count(*) FROM fact_record), (SELECT count(*) FROM warehouse_current)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(rows, (1, 0));
        archive.verify(key).unwrap();
    }

    #[test]
    fn receiver_seal_custody_must_have_its_referenced_manifest() {
        let (stage, mut options) = fixture();
        options.index_recovery = crate::IndexRecovery::Received {
            root: stage.0.clone(),
            custody: Default::default(),
        };
        let (path, _) = source(&options, "lost-seal");
        let dir = crate::sealed::seal_dir(&path);
        fs::create_dir(&dir).unwrap();
        let record = dir.join("custody.json");
        fs::write(&record, b"{}").unwrap();
        let old = refused(&options, 49);
        let result = ensure(&options, &Namespace::new("machine").unwrap());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("history recovery failed")
        );
        assert_eq!(active_path(&options.cache), old);
        assert_eq!(fs::read(record).unwrap(), b"{}");
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_preserves_old_wal_and_shm_with_uncheckpointed_version() {
        let (_stage, options) = fixture();
        let old = refused(&options, 0);
        let db = rusqlite::Connection::open(&old).unwrap();
        db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .unwrap();
        db.pragma_update(None, "user_version", 49).unwrap();
        let wal = old.with_extension("sqlite3-wal");
        let shm = old.with_extension("sqlite3-shm");
        let snapshot = [&old, &wal, &shm].map(|path| fs::read(path).unwrap());
        let namespace = Namespace::new("local").unwrap();
        ensure(&options, &namespace).unwrap();
        assert_eq!(active_path(&options.cache), rebuilt_path(&options.cache));
        assert_eq!(
            [&old, &wal, &shm].map(|path| fs::read(path).unwrap()),
            snapshot
        );
    }
}
