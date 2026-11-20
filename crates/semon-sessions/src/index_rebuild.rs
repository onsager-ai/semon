//! Rebuild a refused Index from host custody. Only a complete database is
//! linked into the active placement. Unsupported databases are never opened
//! by the replay or renamed, and a crash cannot publish a work in progress.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    Input, InputRoot, Options,
    archive::{Archive, NativeSource, RecoveryRecord},
    events::{self, CACHE_VERSION, EventCache, Stat, store},
    sealed::LogFile,
    source_change::SourceLock,
    storage::{Namespace, sqlite},
};

fn rebuilt_path(cache: &Path) -> PathBuf {
    cache.with_extension(format!(
        "rebuilt-{}-{CACHE_VERSION}.sqlite3",
        store::SCHEMA_VERSION
    ))
}

pub(crate) fn active_path(cache: &Path) -> PathBuf {
    let rebuilt = rebuilt_path(cache);
    if rebuilt.is_file() {
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
    crate::viewer::lock(statuses()).get(cache).cloned()
}

pub(crate) fn ensure(options: &Options, namespace: &Namespace) -> io::Result<()> {
    let final_path = rebuilt_path(&options.cache);
    if !store::rebuild_needed(&active_path(&options.cache))? {
        return Ok(());
    }
    let _lock = SourceLock::acquire(options, "\0index-rebuild", true)?;
    if !store::rebuild_needed(&active_path(&options.cache))? {
        return Ok(());
    }
    let progress = |status| {
        crate::viewer::lock(statuses()).insert(options.cache.clone(), status);
    };
    progress(Status::Rebuilding { done: 0, total: 0 });
    let result = rebuild(options, namespace, &final_path, |done, total| {
        progress(Status::Rebuilding { done, total });
    });
    match result {
        Ok(()) => {
            crate::viewer::lock(statuses()).remove(&options.cache);
            Ok(())
        }
        Err(error) => {
            let message = format!("history recovery failed: {error}");
            progress(Status::Failed(message.clone()));
            Err(io::Error::other(message))
        }
    }
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

fn supported(root: InputRoot) -> bool {
    matches!(root, InputRoot::Claude | InputRoot::Codex)
}

fn local_inventory(options: &Options) -> io::Result<BTreeMap<Input, Stat>> {
    crate::inputs(options)?
        .into_iter()
        .filter(|input| supported(input.root) && input.path.ends_with(".jsonl"))
        .map(|input| {
            let path = input.full_path(options);
            let stat = Stat::from_metadata(&crate::open_input(&path)?.metadata()?)?;
            Ok((input, stat))
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
    [InputRoot::Claude, InputRoot::Codex]
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
    (supported(root) && path.ends_with(".jsonl") && crate::is_input_path(harness, path)).then(
        || Input {
            root,
            path: path.to_owned(),
        },
    )
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

fn verify_receiver(
    options: &Options,
    root: &Path,
    originals: &BTreeMap<Input, Stat>,
) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let mut candidates = BTreeMap::new();
    // Receiver markers cover metadata inputs as well as native logs. Validate
    // their bindings without treating metadata files as replayable logs.
    for input in crate::inputs(options)? {
        if crate::is_input_path(input.root.as_str(), &input.path) {
            let key = format!("{}/{}", input.root.as_str(), input.path);
            candidates.insert(format!("{:x}.json", Sha256::digest(key)), input);
        }
    }
    let mut records = BTreeMap::new();
    let marker_root = root.join("sync-generations");
    let entries = match fs::symlink_metadata(&marker_root) {
        Ok(meta) if meta.is_dir() => Some(fs::read_dir(&marker_root)?),
        Ok(_) => {
            return Err(io::Error::other(
                "receiver generation directory is not regular",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let input = candidates.get(&name).ok_or_else(|| {
            io::Error::other(format!(
                "receiver generation {name} has no authorized original"
            ))
        })?;
        let mut bytes = Vec::new();
        crate::open_input(&entry.path())?
            .take(4097)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(io::Error::other("receiver generation record is oversized"));
        }
        let marker: ReceiverGeneration =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let path = input.full_path(options);
        let log = LogFile::open(&path)?;
        let metadata = log.metadata()?;
        let stat = Stat::from_metadata(&metadata)?;
        let mut head = Vec::new();
        log.take(4096).read_to_end(&mut head)?;
        let signed_mtime = match metadata.modified()?.duration_since(std::time::UNIX_EPOCH) {
            Ok(time) => time.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        };
        if marker.generation.is_empty()
            || marker.length != stat.size
            || marker.identity.len != stat.size
            || marker.identity.dev != stat.dev
            || marker.identity.ino != stat.ino
            || marker.identity.mtime_ns != Some(signed_mtime)
            || marker.head_sha256 != format!("{:x}", Sha256::digest(head))
            || originals
                .get(input)
                .is_some_and(|original| original != &stat)
        {
            return Err(io::Error::other(format!(
                "receiver generation changed for {}",
                path.display()
            )));
        }
        records.insert(name, bytes);
    }
    let facts_path = root.join("facts.json");
    match fs::symlink_metadata(&facts_path) {
        Ok(_) => {
            let facts = crate::read_facts(&facts_path)?;
            if let Some(rollouts) = &facts.codex_rollouts {
                for path in rollouts {
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
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(records)
}

fn replay_original(
    cache: &mut EventCache,
    input: &Input,
    options: &Options,
    expected: Stat,
) -> io::Result<()> {
    let path = input.full_path(options);
    let mut dirty = false;
    events::scan_file(&path, input.root.as_str(), cache, &mut dirty)?;
    let ledger = cache.committed_rebuild_ledger(&path)?;
    // The parser permits an incomplete last line. A verified complete suffix
    // after the committed boundary would mean replay failed to account for it.
    let mut log = LogFile::open(&path)?;
    use std::io::{Seek, SeekFrom};
    log.seek(SeekFrom::Start(ledger.offset))?;
    let mut remainder = io::BufReader::new(log);
    use std::io::BufRead;
    let tail = remainder.skip_until(b'\n')? as u64;
    if ledger.stat != expected
        || ledger.offset.checked_add(tail) != Some(expected.size)
        || (tail != 0 && {
            let mut log = remainder.into_inner();
            log.seek(SeekFrom::Start(expected.size - 1))?;
            let mut last = [0];
            log.read_exact(&mut last)?;
            last[0] == b'\n'
        })
    {
        return Err(io::Error::other(format!(
            "original boundary changed for {}",
            path.display()
        )));
    }
    Ok(())
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
    let originals = local_inventory(options)?;
    let archive = Archive::new(crate::archive::standalone_dir(&options.cache));
    let native_archive = read_native_archive(options, &archive)?;
    let received_archive = options
        .receiver_custody
        .as_ref()
        .map(|root| Archive::new(root.join(crate::archive::DIR)));
    let received = match &received_archive {
        Some(archive) => archive.recovery_inventory()?,
        None => Vec::new(),
    };
    let receiver = options
        .receiver_custody
        .as_ref()
        .map(|root| verify_receiver(options, root, &originals))
        .transpose()?;
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
    for (done, input) in sources.into_iter().enumerate() {
        let path = input.full_path(options);
        let live = originals.get(input).copied();
        let mut native: Option<&NativeSource> = None;
        if let Some(records) = archives.get(input) {
            for (archive, record) in records {
                if let Some(admitted) = record.native.as_ref() {
                    if !admitted.allowed(options) || native.replace(admitted).is_some() {
                        return Err(io::Error::other("ambiguous native archive binding"));
                    }
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
            replay_original(&mut cache, input, options, stat)?;
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
    if local_inventory(options)? != originals
        || read_native_archive(options, &archive)? != native_archive
        || received_archive
            .as_ref()
            .map(Archive::recovery_inventory)
            .transpose()?
            .unwrap_or_default()
            != received
        || options
            .receiver_custody
            .as_ref()
            .map(|root| verify_receiver(options, root, &originals))
            .transpose()?
            != receiver
    {
        return Err(io::Error::other("source inventory changed during rebuild"));
    }
    // The completed database is durable before the single atomic publication.
    // A hard link refuses occupied paths without replacing a previous Index.
    fs::File::open(&index)?.sync_all()?;
    fs::hard_link(&index, final_path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
