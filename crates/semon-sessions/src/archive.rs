//! Raw custody: zstd chains of log bytes kept apart from the logs, so that
//! rewriting, replacing or pruning a log never loses what it held.
//!
//! An [`Archive`] is a directory:
//! - `sources.log` maps each source (a caller-chosen key) to its random
//!   `archive_source_id`, one JSON record per line, appended and synced
//!   under the archive lock before anything is written for the new id;
//! - `<archive_source_id>/manifest.json` names its source and lists the
//!   source's chains in order: each chain's length, the SHA-256 of its plain
//!   bytes, its `redactor_version` and the stored size;
//! - `<archive_source_id>/<chain_id>.zst` is one chain, the plain bytes
//!   compressed as zstd frames (plain `zstd -d` reads it back).
//!
//! A manifest is published by writing a temporary file, syncing it and
//! renaming it over the old one, then syncing the directory, so a failure
//! before the rename leaves the last manifest as it was. A chain
//! file is renamed into place and synced before the manifest that names it,
//! so a manifest never names a chain that is not durable.
//!
//! Two writers use it:
//! - a receiver calls [`Archive::preserve`] with the logical bytes of a
//!   mirrored file before it replaces them: an immutable chain per replace;
//! - a standalone machine calls [`Archive::admit`] with each native log and
//!   the offset and digest its index has consumed: one chain per native
//!   generation, extended by appending frames, and a new chain when the log
//!   was rewritten. The old chain stays.
//!
//! The archive only copies: it never touches, shortens or removes a log.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{sealed::hex, source_chunks::PrefixState};

/// The archive's directory name, under a receiver's machine directory and
/// under the Semon state directory.
pub const DIR: &str = "archive";
/// Recorded for bytes whose redaction is not known.
pub const UNKNOWN_REDACTOR: &str = "unknown";
/// Recorded for a file whose bytes were appended under different versions.
pub const MIXED_REDACTOR: &str = "mixed";
/// Recorded for native bytes, which nothing redacted.
pub const NATIVE_REDACTOR: &str = "none";

const SOURCES: &str = "sources.log";
const MANIFEST: &str = "manifest.json";
const LOCK: &str = ".lock";
const LEVEL: i32 = 3;
const METADATA_MAX: u64 = 64 << 20;
const BUFFER: usize = 1 << 20;

/// The standalone archive beside the index cache at `cache`, in the Semon
/// state directory.
pub fn standalone_dir(cache: &Path) -> PathBuf {
    cache.parent().unwrap_or_else(|| Path::new("")).join(DIR)
}

/// One chain of a source: bytes `[0, length)` of one generation of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    /// Random; names the chain's file, `<chain_id>.zst`.
    pub chain_id: String,
    /// Plain bytes the chain holds.
    pub length: u64,
    /// SHA-256 (lowercase hex) of those bytes.
    pub sha256: String,
    /// Bytes of the chain file the manifest vouches for. A chain being
    /// extended may hold a torn tail past this after a crash; it is ignored
    /// and cut off at the next write.
    pub stored_bytes: u64,
    /// The version of the rules that redacted the bytes, or
    /// [`UNKNOWN_REDACTOR`], [`MIXED_REDACTOR`] or [`NATIVE_REDACTOR`].
    pub redactor_version: String,
    /// Continues the SHA-256 at the last whole 64-byte block of the chain,
    /// so the next extension verifies its prefix without rereading it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<PrefixState>,
}

#[derive(Default, Serialize, Deserialize)]
struct Manifest {
    /// The source key this directory belongs to, so a directory can always
    /// be attributed, whatever happens to `sources.log`.
    source: String,
    chains: Vec<Chain>,
}

/// One line of `sources.log`.
#[derive(Serialize, Deserialize)]
struct Record {
    source: String,
    id: String,
}

/// An archive directory. Cheap to create; nothing is made on disk until a
/// chain is written.
pub struct Archive {
    dir: PathBuf,
    known: Mutex<Known>,
}

/// What this instance knows of the archive. An id never changes once
/// assigned, so a hit is always right; a miss reads the new tail of
/// `sources.log`.
#[derive(Default)]
struct Known {
    sources: BTreeMap<String, String>,
    /// Bytes of `sources.log` read: up to the end of its last whole line.
    read: u64,
    /// Chains read back whole in this process, by id, with the stored size
    /// they had then; one that changed size is read back again.
    verified: BTreeMap<String, u64>,
}

/// Holds the archive's advisory lock until dropped.
struct Guard(#[allow(dead_code)] fs::File);

impl Archive {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            known: Mutex::default(),
        }
    }

    /// The source's chains, oldest first; none for a source never written.
    pub fn chains(&self, source: &str) -> io::Result<Vec<Chain>> {
        let Some(id) = self.lookup(source)? else {
            return Ok(Vec::new());
        };
        Ok(read_manifest(&self.dir.join(id))?.chains)
    }

    /// Opens the plain bytes of one of `source`'s chains.
    pub fn open_chain(&self, source: &str, chain: &Chain) -> io::Result<impl Read + use<>> {
        let id = self
            .lookup(source)?
            .ok_or_else(|| invalid("an archive source that does not exist"))?;
        open_chain(&self.dir.join(id), chain)
    }

    /// Checks every chain of `source` against its manifest: the length and
    /// the SHA-256 of the bytes it decompresses to. An error when a chain
    /// is missing, unreadable or differs.
    pub fn verify(&self, source: &str) -> io::Result<()> {
        let Some(id) = self.lookup(source)? else {
            return Ok(());
        };
        let dir = self.dir.join(id);
        for chain in read_manifest(&dir)?.chains {
            check_chain(&dir, &chain)?;
        }
        Ok(())
    }

    /// Writes everything `reader` yields as a new, immutable chain of
    /// `source`, durably, and returns it. Bytes that an earlier chain holds
    /// intact (same length, SHA-256 and version) are not written again:
    /// that chain is returned. `None` for an empty reader.
    pub fn preserve(
        &self,
        source: &str,
        reader: &mut impl Read,
        redactor_version: &str,
    ) -> io::Result<Option<Chain>> {
        let _guard = self.lock()?;
        let (dir, mut manifest) = self.open_source(source)?;
        let chain_id = random_id()?;
        let temporary = dir.join(format!("{chain_id}.zst.tmp"));
        let written = (|| -> io::Result<(u64, Hasher, u64)> {
            let mut encoder = zstd::stream::write::Encoder::new(create_file(&temporary)?, LEVEL)?;
            let mut hasher = Hasher::new(PrefixState::default());
            let mut buffer = vec![0; BUFFER];
            let mut length = 0;
            loop {
                let count = read_some(reader, &mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
                encoder.write_all(&buffer[..count])?;
                length += count as u64;
            }
            let file = encoder.finish()?;
            file.sync_all()?;
            Ok((length, hasher, file.metadata()?.len()))
        })();
        let (length, hasher, stored_bytes) = match written {
            Ok(written) => written,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        let sha256 = hex(&hasher.digest());
        if length == 0 {
            fs::remove_file(&temporary)?;
            return Ok(None);
        }
        // Reuse a chain only if it is the same bytes under the same version
        // and still reads back whole. A damaged one stays, as evidence, and
        // the fresh copy is published beside it.
        let reusable = manifest.chains.iter().find(|held| {
            held.length == length
                && held.sha256 == sha256
                && held.redactor_version == redactor_version
                && check_chain(&dir, held).is_ok()
        });
        if let Some(held) = reusable {
            let held = held.clone();
            fs::remove_file(&temporary)?;
            return Ok(Some(held));
        }
        fs::rename(&temporary, dir.join(format!("{chain_id}.zst")))?;
        sync_dir(&dir)?;
        let chain = Chain {
            chain_id,
            length,
            sha256,
            stored_bytes,
            redactor_version: redactor_version.to_owned(),
            resume: Some(hasher.resume()),
        };
        manifest.chains.push(chain.clone());
        publish(&dir, MANIFEST, &manifest)?;
        Ok(Some(chain))
    }

    /// Makes the archive hold the first `offset` bytes of the native `log`,
    /// whose SHA-256 the index recorded as `prefix` when it consumed them.
    ///
    /// Only bytes that hash to `prefix` are admitted. When the source's last
    /// chain is a prefix of them, it is extended with the new bytes; when
    /// the log was rewritten (the chain is not a prefix of it) a new chain
    /// starts and the old one stays. Returns `false`, writing nothing, when
    /// the log no longer holds the consumed bytes (it changed after the
    /// index read it); the next index read admits it.
    pub fn admit(
        &self,
        source: &str,
        log: &mut (impl Read + Seek),
        offset: u64,
        prefix: &[u8; 32],
    ) -> io::Result<bool> {
        if offset == 0 {
            return Ok(true);
        }
        let _guard = self.lock()?;
        let (dir, mut manifest) = self.open_source(source)?;
        // Only a chain that reads back whole is trusted or extended. A
        // missing or damaged one stays as evidence and the log's consumed
        // prefix starts a new chain beside it.
        if let Some(held) = manifest.chains.last().cloned()
            && self.trusted(&dir, &held)
        {
            if held.length == offset && held.sha256 == hex(prefix) {
                return Ok(true);
            }
            if held.length > offset {
                // The archive may already hold more than the index has read.
                let mut hash = Sha256::new();
                let mut bytes = open_chain(&dir, &held)?.take(offset);
                let copied = io::copy(&mut bytes, &mut HashWriter(&mut hash))?;
                if copied == offset && hash.finalize().as_slice() == prefix {
                    return Ok(true);
                }
            } else if held.length < offset
                && let Some(resume) = &held.resume
            {
                // The chain continues the log when the log's bytes up to its
                // length still hash to the chain's digest, and all of them
                // to what the index consumed.
                let checked = copy_range(log, resume, held.length, offset, &mut io::sink());
                match checked {
                    Ok((hasher, mark)) if hasher.digest() == *prefix && agrees(&held, mark) => {
                        let extended =
                            extend(&dir, &mut manifest, &held, resume, log, offset, prefix)?;
                        if extended {
                            let stored = manifest.chains.last().map(|c| c.stored_bytes);
                            self.verified(&held.chain_id, stored);
                        }
                        return Ok(extended);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let begun = begin(&dir, &mut manifest, log, offset, prefix)?;
        if begun && let Some(chain) = manifest.chains.last() {
            self.verified(&chain.chain_id, Some(chain.stored_bytes));
        }
        Ok(begun)
    }

    /// Whether `chain` reads back whole: checked once per process for each
    /// stored size it has.
    fn trusted(&self, dir: &Path, chain: &Chain) -> bool {
        if self.known().verified.get(&chain.chain_id) == Some(&chain.stored_bytes) {
            return true;
        }
        let sound = check_chain(dir, chain).is_ok();
        self.verified(&chain.chain_id, sound.then_some(chain.stored_bytes));
        sound
    }

    fn verified(&self, chain_id: &str, stored: Option<u64>) {
        let mut known = self.known();
        match stored {
            Some(stored) => known.verified.insert(chain_id.to_owned(), stored),
            None => known.verified.remove(chain_id),
        };
    }

    fn known(&self) -> MutexGuard<'_, Known> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads what was appended to `sources.log` since it was last read.
    /// A torn final line (a crash mid-append) is ignored.
    fn refresh(&self, known: &mut Known) -> io::Result<()> {
        let mut file = match crate::open_input(&self.dir.join(SOURCES)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() < known.read {
            *known = Known::default();
        }
        file.seek(SeekFrom::Start(known.read))?;
        let mut bytes = Vec::new();
        file.take(METADATA_MAX + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > METADATA_MAX {
            return Err(invalid("an oversized archive source log"));
        }
        let whole = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        for line in bytes[..whole]
            .split(|byte| *byte == b'\n')
            .filter(|l| !l.is_empty())
        {
            let record: Record = serde_json::from_slice(line)
                .map_err(|_| invalid("an unreadable archive source log"))?;
            if !is_id(&record.id) {
                return Err(invalid("an archive source id that is not an id"));
            }
            known.sources.entry(record.source).or_insert(record.id);
        }
        known.read += whole as u64;
        Ok(())
    }

    /// The source's archive id, if it has one.
    fn lookup(&self, source: &str) -> io::Result<Option<String>> {
        let mut known = self.known();
        if !known.sources.contains_key(source) {
            self.refresh(&mut known)?;
        }
        Ok(known.sources.get(source).cloned())
    }

    /// The source's directory and manifest. A new source is recorded in
    /// `sources.log`, durably, before its directory is made, so nothing is
    /// ever written under an id that is not recorded. Holds the archive lock,
    /// so another instance's allocation is found and reused.
    fn open_source(&self, source: &str) -> io::Result<(PathBuf, Manifest)> {
        let id = match self.lookup(source)? {
            Some(id) => id,
            None => self.register(source)?,
        };
        let dir = self.dir.join(id);
        let mut manifest = read_manifest(&dir)?;
        if manifest.source.is_empty() {
            // A new directory, or one a crash left before its manifest.
            make_dir(&dir)?;
            sync_dir(&self.dir)?;
            manifest.source = source.to_owned();
            publish(&dir, MANIFEST, &manifest)?;
        } else if manifest.source != source {
            return Err(invalid(
                "an archive directory that belongs to another source",
            ));
        }
        Ok((dir, manifest))
    }

    /// Appends a new id for `source` to `sources.log` and syncs it.
    fn register(&self, source: &str) -> io::Result<String> {
        #[cfg(test)]
        if FAIL_REGISTER.with(std::cell::Cell::take) {
            return Err(io::Error::other("an injected registration failure"));
        }
        let id = random_id()?;
        let mut line = serde_json::to_vec(&Record {
            source: source.to_owned(),
            id: id.clone(),
        })
        .map_err(io::Error::other)?;
        line.push(b'\n');
        let path = self.dir.join(SOURCES);
        let created = !path.exists();
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        let mut known = self.known();
        // Cut off a torn final line, then append.
        file.set_len(known.read)?;
        file.seek(SeekFrom::Start(known.read))?;
        file.write_all(&line)?;
        file.sync_data()?;
        if created {
            sync_dir(&self.dir)?;
        }
        known.read += line.len() as u64;
        known.sources.insert(source.to_owned(), id.clone());
        Ok(id)
    }

    /// Takes the archive's advisory lock, making the directory if needed.
    fn lock(&self) -> io::Result<Guard> {
        match fs::symlink_metadata(&self.dir) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Err(invalid("the archive is not a directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                make_dir(&self.dir)?;
                if let Some(parent) = self.dir.parent().filter(|p| !p.as_os_str().is_empty()) {
                    sync_dir(parent)?;
                }
            }
            Err(error) => return Err(error),
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(LOCK))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Guard(file))
    }
}

/// Starts a new chain from the first `offset` bytes of `log`; `false` when
/// they do not hash to `prefix`.
fn begin(
    dir: &Path,
    manifest: &mut Manifest,
    log: &mut (impl Read + Seek),
    offset: u64,
    prefix: &[u8; 32],
) -> io::Result<bool> {
    let chain_id = random_id()?;
    let temporary = dir.join(format!("{chain_id}.zst.tmp"));
    let written = (|| -> io::Result<Option<(Hasher, u64)>> {
        let mut encoder = zstd::stream::write::Encoder::new(create_file(&temporary)?, LEVEL)?;
        match copy_range(log, &PrefixState::default(), 0, offset, &mut encoder) {
            Ok((hasher, _)) if hasher.digest() == *prefix => {
                let file = encoder.finish()?;
                file.sync_all()?;
                Ok(Some((hasher, file.metadata()?.len())))
            }
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(error) => Err(error),
        }
    })();
    let (hasher, stored_bytes) = match written {
        Ok(Some(written)) => written,
        other => {
            let _ = fs::remove_file(&temporary);
            return other.map(|_| false);
        }
    };
    fs::rename(&temporary, dir.join(format!("{chain_id}.zst")))?;
    sync_dir(dir)?;
    manifest.chains.push(Chain {
        chain_id,
        length: offset,
        sha256: hex(prefix),
        stored_bytes,
        redactor_version: NATIVE_REDACTOR.to_owned(),
        resume: Some(hasher.resume()),
    });
    publish(dir, MANIFEST, manifest)?;
    Ok(true)
}

/// Appends the bytes of `log` from `held.length` to `offset` to `held`, the
/// manifest's last chain, whose prefix the caller has just checked; `false`
/// when the log changed since and the bytes do not hash to `prefix`.
fn extend(
    dir: &Path,
    manifest: &mut Manifest,
    held: &Chain,
    resume: &PrefixState,
    log: &mut (impl Read + Seek),
    offset: u64,
    prefix: &[u8; 32],
) -> io::Result<bool> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(dir.join(format!("{}.zst", held.chain_id)))?;
    if file.metadata()?.len() < held.stored_bytes {
        return Err(invalid("an archive chain shorter than its manifest"));
    }
    // A torn tail from a crash after the last manifest.
    file.set_len(held.stored_bytes)?;
    file.seek(SeekFrom::End(0))?;
    let mut encoder = zstd::stream::write::Encoder::new(file, LEVEL)?;
    let copied = copy_range(log, resume, held.length, offset, &mut encoder);
    let hasher = match copied {
        Ok((hasher, mark)) if hasher.digest() == *prefix && agrees(held, mark) => hasher,
        other => {
            let _ = encoder.get_ref().set_len(held.stored_bytes);
            return match other {
                Err(error) if error.kind() != io::ErrorKind::UnexpectedEof => Err(error),
                _ => Ok(false),
            };
        }
    };
    let file = encoder.finish()?;
    file.sync_all()?;
    let last = manifest.chains.last_mut().expect("a held chain");
    last.length = offset;
    last.sha256 = hex(prefix);
    last.stored_bytes = file.metadata()?.len();
    last.resume = Some(hasher.resume());
    publish(dir, MANIFEST, manifest)?;
    Ok(true)
}

/// Whether the digest of the log's first `held.length` bytes is the chain's.
fn agrees(held: &Chain, mark: Option<[u8; 32]>) -> bool {
    mark.is_some_and(|mark| hex(&mark) == held.sha256)
}

/// SHA-256 that can be continued: the compression state at the last whole
/// block, and the bytes since.
struct Hasher {
    state: PrefixState,
    pending: Vec<u8>,
}

impl Hasher {
    fn new(state: PrefixState) -> Self {
        Self {
            state,
            pending: Vec::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let whole = self.pending.len() / 64 * 64;
        if whole > 0 {
            self.state.advance_blocks(&self.pending[..whole]);
            self.pending.drain(..whole);
        }
    }

    fn digest(&self) -> [u8; 32] {
        self.state.finish(&self.pending)
    }

    /// The state at the last whole block.
    fn resume(&self) -> PrefixState {
        self.state.clone()
    }
}

struct HashWriter<'a>(&'a mut Sha256);

impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads `log` from `start.at` to `to`, hashing it from `start` and writing
/// the bytes at or past `write_from` to `out`. Also returns the digest of
/// the log's first `write_from` bytes, when `write_from` is reached. An
/// `UnexpectedEof` error when the log ends before `to`.
fn copy_range(
    log: &mut (impl Read + Seek),
    start: &PrefixState,
    write_from: u64,
    to: u64,
    out: &mut impl Write,
) -> io::Result<(Hasher, Option<[u8; 32]>)> {
    log.seek(SeekFrom::Start(start.at))?;
    let mut hasher = Hasher::new(start.clone());
    let mut mark = (start.at == write_from).then(|| hasher.digest());
    let mut buffer = vec![0; BUFFER];
    let mut at = start.at;
    while at < to {
        // Never read across `write_from`: the digest there needs a stop.
        let limit = if at < write_from {
            write_from.min(to)
        } else {
            to
        };
        let want = (limit - at).min(BUFFER as u64) as usize;
        let count = read_some(log, &mut buffer[..want])?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        hasher.update(&buffer[..count]);
        if at + count as u64 > write_from {
            let skip = write_from.saturating_sub(at) as usize;
            out.write_all(&buffer[skip..count])?;
        }
        at += count as u64;
        if at == write_from {
            mark = Some(hasher.digest());
        }
    }
    Ok((hasher, mark))
}

fn read_some(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

/// The source's manifest in `dir`, checked before anything is built from
/// it: ids, digests and the continuation state must be what this module
/// writes, so a damaged or hostile manifest cannot name another path.
fn read_manifest(dir: &Path) -> io::Result<Manifest> {
    let manifest: Manifest = read_json(&dir.join(MANIFEST))?;
    if manifest.source.is_empty() && !manifest.chains.is_empty() {
        return Err(invalid("an archive manifest that is not what was written"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for chain in &manifest.chains {
        let sound = is_id(&chain.chain_id)
            && seen.insert(chain.chain_id.as_str())
            && chain.length > 0
            && chain.stored_bytes > 0
            && chain.sha256.len() == 64
            && chain
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && !chain.redactor_version.is_empty()
            && chain.redactor_version.len() <= 64
            && chain
                .resume
                .as_ref()
                .is_none_or(|resume| resume.at <= chain.length && resume.at % 64 == 0);
        if !sound {
            return Err(invalid("an archive manifest that is not what was written"));
        }
    }
    Ok(manifest)
}

/// Reads `chain` back whole and checks its length and SHA-256.
fn check_chain(dir: &Path, chain: &Chain) -> io::Result<()> {
    let mut hash = Sha256::new();
    let bytes = io::copy(&mut open_chain(dir, chain)?, &mut HashWriter(&mut hash))?;
    if bytes != chain.length || hex(&hash.finalize()) != chain.sha256 {
        return Err(invalid(&format!(
            "archive chain {} differs from its manifest",
            chain.chain_id
        )));
    }
    Ok(())
}

/// The plain bytes of `chain`, reading no more of its file than its
/// manifest vouches for.
fn open_chain(dir: &Path, chain: &Chain) -> io::Result<impl Read + use<>> {
    if !is_id(&chain.chain_id) {
        return Err(invalid("an archive chain id that is not an id"));
    }
    let file = crate::open_input(&dir.join(format!("{}.zst", chain.chain_id)))?;
    zstd::stream::read::Decoder::new(file.take(chain.stored_bytes))
}

fn is_id(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// 128 random bits as lowercase hex.
fn random_id() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex(&bytes))
}

/// `T::default()` when the file does not exist.
fn read_json<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    let file = match crate::open_input(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(METADATA_MAX + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > METADATA_MAX {
        return Err(invalid("oversized archive metadata"));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("unreadable archive metadata"))
}

/// Writes `value` as `dir/name`: a temporary file, synced, renamed over the
/// old one, and the directory synced.
fn publish(dir: &Path, name: &str, value: &impl Serialize) -> io::Result<()> {
    let temporary = dir.join(format!("{name}.tmp"));
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let mut file = create_file(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(test)]
    if FAIL_BEFORE_RENAME.with(std::cell::Cell::take) {
        return Err(io::Error::other("an injected failure before the rename"));
    }
    fs::rename(&temporary, dir.join(name))?;
    sync_dir(dir)
}

#[cfg(test)]
thread_local! {
    /// Fails the next [`Archive::register`] before it writes.
    static FAIL_REGISTER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Fails the next [`publish`] after its temporary file is synced and
    /// before the rename, as a crash there would.
    static FAIL_BEFORE_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A new or truncated file, 0600.
fn create_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn make_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
