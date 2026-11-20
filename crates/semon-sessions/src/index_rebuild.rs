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

#[derive(PartialEq, Eq)]
struct ReceiverInventory {
    markers: BTreeMap<String, Vec<u8>>,
    rollouts: [Option<BTreeSet<String>>; 2],
    seals: BTreeMap<Input, Option<crate::archive::Chain>>,
}

fn verify_receiver(
    options: &Options,
    root: &Path,
    originals: &BTreeMap<Input, Stat>,
) -> io::Result<ReceiverInventory> {
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
        let file = crate::open_input(&path)?;
        let metadata = file.metadata()?;
        let stat = Stat::from_metadata(&metadata)?;
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
    let rollouts = match fs::symlink_metadata(&facts_path) {
        Ok(metadata) if metadata.is_file() => {
            let facts = crate::received::read_facts(&facts_path)?;
            [facts.codex_rollouts, facts.codex_provisional_rollouts]
        }
        Ok(_) => return Err(io::Error::other("receiver Facts is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => [None, None],
        Err(error) => return Err(error),
    };
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
        .filter(|input| supported(input.root))
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
    Ok(ReceiverInventory {
        markers: records,
        rollouts,
        seals,
    })
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
        || Stat::from_metadata(&remainder.get_ref().metadata()?)? != expected
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
        .index_recovery
        .receiver_custody()
        .map(|root| Archive::new(root.join(crate::archive::DIR)));
    let received = match &received_archive {
        Some(archive) => archive.recovery_inventory()?,
        None => Vec::new(),
    };
    let receiver = options
        .index_recovery
        .receiver_custody()
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
            .index_recovery
            .receiver_custody()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Ledger;

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
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let native = NativeSource {
            home: options.claude_home.clone(),
            root: "claude".into(),
            path: format!("projects/project/{id}.jsonl"),
            native_id: id.into(),
            ledger: Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: bytes.len() as u64,
                prefix: digest,
                tail: digest,
            },
            sidecar: None,
            chain_id: String::new(),
        };
        let archive = Archive::new(crate::archive::standalone_dir(&options.cache));
        archive
            .admit_native(&mut LogFile::open(&path).unwrap(), &native)
            .unwrap()
            .unwrap();
        fs::remove_file(&path).unwrap();
        path
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
        for version in [0, 49, store::SCHEMA_VERSION + 1] {
            let (_stage, options) = fixture();
            let (live, bytes) = source(&options, "live");
            let archived = archive(&options, "archive-only");
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
            assert!(!archived.exists());
            assert_eq!(status(&options.cache), None);
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

    #[test]
    fn unmatched_receiver_generation_stops_recovery() {
        let (stage, mut options) = fixture();
        options.index_recovery = crate::IndexRecovery::Received(stage.0.clone());
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
        options.index_recovery = crate::IndexRecovery::Received(receiver.clone());
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
        options.index_recovery = crate::IndexRecovery::Received(stage.0.clone());
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
