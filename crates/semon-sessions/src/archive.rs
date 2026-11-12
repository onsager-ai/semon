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
const CLOCK: &str = "clock.json";

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveClock {
    high_water: u64,
}
const LEVEL: i32 = 3;
const METADATA_MAX: u64 = 64 << 20;
const BUFFER: usize = 1 << 20;
// Overflow starts a native chain instead of evicting and rescanning prefixes.
const MAX_PREFIX_READERS: usize = 16;
const MAX_PREFIX_DIGESTS: usize = 256;

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
    /// Immutable Unix seconds when this chain first entered custody.
    pub admitted_at: u64,
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
    native: Option<NativeSource>,
    /// Exact chain dependencies of durable seal/index custody markers.
    references: BTreeMap<String, String>,
}

/// Admission binds bytes to the configured native identity and original ledger.
/// It is custody metadata, not a new filesystem observation or runtime grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeSource {
    pub(crate) home: PathBuf,
    pub(crate) root: String,
    pub(crate) path: String,
    pub(crate) native_id: String,
    pub(crate) ledger: crate::events::Ledger,
    pub(crate) sidecar: Option<crate::native_sidecar::ClaudeSidecar>,
    pub(crate) chain_id: String,
}

impl NativeSource {
    pub(crate) fn full_path(&self) -> PathBuf {
        self.home.join(&self.path)
    }

    pub(crate) fn allowed(&self, options: &crate::Options) -> bool {
        crate::InputRoot::parse(&self.root).is_some_and(|root| {
            root.home(options) == &self.home
                && crate::is_input_path(&self.root, &self.path)
                && self.path.ends_with(".jsonl")
        })
    }
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
    /// Descriptor observation of the journal whose complete tail we consumed.
    journal: Option<crate::events::Stat>,
    /// Verified chains, invalidated by a change to the file or manifest.
    verified: BTreeMap<String, Verified>,
    active_prefixes: usize,
}

impl Known {
    fn prefix_count(&self) -> usize {
        #[cfg(test)]
        PREFIX_CAPACITY_WORK.with(|work| work.set(work.get() + 1));
        self.active_prefixes
    }

    fn release_prefix(&mut self, chain_id: &str) {
        if let Some(verified) = self.verified.get_mut(chain_id)
            && verified.prefix.take().is_some()
        {
            self.active_prefixes -= 1;
        }
    }

    fn remove_verified(&mut self, chain_id: &str) {
        self.release_prefix(chain_id);
        self.verified.remove(chain_id);
    }
}

struct Verified {
    observation: Verification,
    prefix: Option<ComparedPrefix>,
}

struct ComparedPrefix {
    reader: Box<dyn Read + Send>,
    hash: Sha256,
    offset: u64,
    digests: BTreeMap<u64, [u8; 32]>,
}

#[derive(PartialEq, Eq)]
struct Verification {
    file: crate::events::Stat,
    length: u64,
    stored_bytes: u64,
    sha256: String,
}

impl Verification {
    fn observed(dir: &Path, chain: &Chain) -> io::Result<Self> {
        Ok(Self {
            file: crate::events::Stat::of(&dir.join(format!("{}.zst", chain.chain_id)))?,
            length: chain.length,
            stored_bytes: chain.stored_bytes,
            sha256: chain.sha256.clone(),
        })
    }
}

/// Holds the archive's advisory lock until dropped.
struct Guard(fs::File);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(crate) struct NativeRead {
    pub(crate) file: fs::File,
    guard: Option<Guard>,
    temporary: PathBuf,
    source: PathBuf,
    chain: PathBuf,
    witness: crate::events::Stat,
}

impl NativeRead {
    pub(crate) fn release_lock(&mut self) {
        drop(self.guard.take());
    }

    pub(crate) fn verify(&self) -> io::Result<()> {
        match fs::symlink_metadata(&self.source) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
            Ok(_) => return Err(invalid("native source reappeared during archive read")),
        }
        if crate::events::Stat::of(&self.chain)? != self.witness {
            return Err(invalid("archive chain changed during read"));
        }
        Ok(())
    }
}

impl Drop for NativeRead {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
    }
}

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

    pub(crate) fn directory(&self) -> io::Result<PathBuf> {
        fs::canonicalize(&self.dir)
    }

    /// Verify this exact retained chain and keep writers excluded until the
    /// custody-dependent operation completes. File changes invalidate trust.
    pub(crate) fn with_verified_chain<T>(
        &self,
        source: &str,
        expected: &Chain,
        operation: impl FnOnce(&mut dyn Read) -> io::Result<T>,
    ) -> io::Result<T> {
        let _guard = self.lock(false)?;
        let id = self
            .lookup_locked(source)?
            .ok_or_else(|| invalid("a missing custody source"))?;
        let dir = self.dir.join(id);
        let manifest = read_manifest(&dir)?;
        let chain = manifest
            .chains
            .iter()
            .find(|chain| *chain == expected)
            .ok_or_else(|| invalid("a custody chain that differs from its manifest"))?;
        if manifest.source != source || !self.trusted(&dir, chain) {
            return Err(invalid("an unverified custody chain"));
        }
        operation(&mut open_chain(&dir, chain)?)
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
        let _guard = self.lock(true)?;
        let admitted_at = self.admission_time()?;
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
            admitted_at,
            length,
            sha256,
            stored_bytes,
            redactor_version: redactor_version.to_owned(),
            resume: Some(hasher.resume()),
        };
        if let Some(previous) = manifest.chains.last() {
            self.release_prefix(&previous.chain_id);
        }
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
        self.admit_inner(source, log, offset, prefix, None)
    }

    pub(crate) fn admit_native(
        &self,
        log: &mut crate::sealed::LogFile,
        native: &NativeSource,
    ) -> io::Result<bool> {
        let path = native.full_path();
        let before = crate::events::Stat::from_metadata(&log.metadata()?)?;
        if !same_generation(&native.ledger, before) {
            return Ok(false);
        }
        self.admit_inner(
            &path.to_string_lossy(),
            log,
            native.ledger.offset,
            &native.ledger.prefix,
            Some(native),
        )
    }

    fn admit_inner(
        &self,
        source: &str,
        log: &mut (impl Read + Seek),
        offset: u64,
        prefix: &[u8; 32],
        native: Option<&NativeSource>,
    ) -> io::Result<bool> {
        if offset == 0 {
            return Ok(true);
        }
        let _guard = self.lock(true)?;
        self.admission_time()?;
        let (dir, mut manifest) = self.open_source(source)?;
        // Another Archive may have superseded this source's old chain.
        {
            let mut known = self.known();
            for chain in manifest.chains.iter().rev().skip(1) {
                known.release_prefix(&chain.chain_id);
            }
        }
        // Only a chain that reads back whole is trusted or extended. A
        // missing or damaged one stays as evidence and the log's consumed
        // prefix starts a new chain beside it.
        if let Some(held) = manifest.chains.last().cloned()
            && self.trusted(&dir, &held)
        {
            if held.length == offset && held.sha256 == hex(prefix) {
                self.release_prefix(&held.chain_id);
                let previous = manifest.native.clone();
                let bound = bind_native(&mut manifest, native, &held)?;
                if manifest.native != previous {
                    publish(&dir, MANIFEST, &manifest)?;
                }
                return Ok(bound);
            }
            if held.length > offset && self.matches_prefix(&dir, &held, offset, prefix)? {
                return Ok(true);
            } else if held.length < offset
                && !manifest.references.values().any(|id| id == &held.chain_id)
                && let Some(resume) = &held.resume
            {
                // The chain continues the log when the log's bytes up to its
                // length still hash to the chain's digest, and all of them
                // to what the index consumed.
                let checked = copy_range(log, resume, held.length, offset, &mut io::sink());
                match checked {
                    Ok((hasher, mark)) if hasher.digest() == *prefix && agrees(&held, mark) => {
                        let witness = self.known().verified[&held.chain_id].observation.file;
                        let extended = extend(
                            &dir,
                            &mut manifest,
                            (&held, witness),
                            log,
                            offset,
                            prefix,
                            native,
                        )?;
                        if extended && let Some(chain) = manifest.chains.last().cloned() {
                            self.remember(&dir, &chain)?;
                        }
                        return Ok(extended);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
                    Err(error) => return Err(error),
                }
            }
        }
        if let Some(chain) = manifest.chains.last() {
            self.release_prefix(&chain.chain_id);
        }
        let begun = begin(&dir, &mut manifest, log, offset, prefix, native)?;
        if begun && let Some(chain) = manifest.chains.last().cloned() {
            self.remember(&dir, &chain)?;
        }
        Ok(begun)
    }

    /// Discover only sources admitted under these exact configured native roots.
    pub(crate) fn missing_sources(
        &self,
        options: &crate::Options,
    ) -> io::Result<Vec<NativeSource>> {
        if !self.dir.join(SOURCES).try_exists()? {
            return Ok(Vec::new());
        }
        let _guard = self.lock(false)?;
        let mut known = self.known();
        self.refresh(&mut known)?;
        let mut sources = Vec::new();
        for (source, id) in &known.sources {
            let manifest = read_manifest(&self.dir.join(id))?;
            if let Some(native) = manifest.native {
                if native.full_path().to_string_lossy() != source.as_str() {
                    return Err(invalid("an archive native source binding"));
                }
                if native.allowed(options) {
                    match fs::symlink_metadata(native.full_path()) {
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            sources.push(native)
                        }
                        Err(error) => return Err(error),
                        Ok(_) => {}
                    }
                }
            }
        }
        Ok(sources)
    }

    pub(crate) fn admitted_source(
        &self,
        path: &Path,
        ledger: &crate::events::Ledger,
    ) -> io::Result<Option<NativeSource>> {
        if !self.dir.exists() {
            return Ok(None);
        }
        let _guard = self.lock(false)?;
        let Some(id) = self.lookup_locked(&path.to_string_lossy())? else {
            return Ok(None);
        };
        let manifest = read_manifest(&self.dir.join(id))?;
        Ok(manifest
            .native
            .filter(|native| native.full_path() == path && &native.ledger == ledger))
    }

    pub(crate) fn open_native(&self, native: &NativeSource) -> io::Result<NativeRead> {
        let guard = self.lock(false)?;
        let source = native.full_path();
        let id = self
            .lookup_locked(&source.to_string_lossy())?
            .ok_or_else(|| invalid("a missing archive native source"))?;
        let dir = self.dir.join(id);
        let manifest = read_manifest(&dir)?;
        if manifest.native.as_ref() != Some(native) {
            return Err(invalid("an archive native generation changed"));
        }
        let chain = manifest
            .chains
            .iter()
            .find(|chain| chain.chain_id == native.chain_id)
            .ok_or_else(|| invalid("a missing native custody chain"))?;
        if !self.trusted(&dir, chain) {
            return Err(invalid("an unverified native custody chain"));
        }
        let temporary = self.dir.join(format!("{}.read.tmp", random_id()?));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&temporary)?;
        let mut read = NativeRead {
            file,
            guard: Some(guard),
            temporary,
            source,
            chain: dir.join(format!("{}.zst", chain.chain_id)),
            witness: Verification::observed(&dir, chain)?.file,
        };
        let copied = io::copy(
            &mut open_chain(&dir, chain)?.take(native.ledger.offset),
            &mut read.file,
        )?;
        if copied != native.ledger.offset {
            return Err(invalid("archive prefix is incomplete"));
        }
        let captured = crate::source_chunks::capture(&mut read.file, copied, 0)?;
        if captured.prefix != native.ledger.prefix || captured.tail != native.ledger.tail {
            return Err(invalid("archive prefix differs from admitted ledger"));
        }
        read.file.seek(SeekFrom::Start(0))?;
        read.verify()?;
        Ok(read)
    }

    /// Keep retention excluded through verification, parsing and index commit.
    /// A private temporary provides seek without restoring a native pathname.
    pub(crate) fn with_native<T>(
        &self,
        native: &NativeSource,
        operation: impl FnOnce(&mut fs::File) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut read = self.open_native(native)?;
        let value = operation(&mut read.file)?;
        read.verify()?;
        Ok(value)
    }

    /// Register an exact durable dependency before publishing its external marker.
    /// A crash can over-retain custody, never leave a marker unprotected.
    pub(crate) fn protect(&self, source: &str, chain_id: &str, owner: String) -> io::Result<()> {
        let _guard = self.lock(true)?;
        let (dir, mut manifest) = self.open_source(source)?;
        if !manifest
            .chains
            .iter()
            .any(|chain| chain.chain_id == chain_id)
        {
            return Err(invalid("a missing referenced custody chain"));
        }
        manifest
            .references
            .insert(format!("{owner}:{chain_id}"), chain_id.to_owned());
        publish(&dir, MANIFEST, &manifest)
    }

    /// Bind the ordinary retention marker to the admitted standalone chain.
    pub(crate) fn protect_index(
        &self,
        path: &Path,
        proof: &crate::SessionSourceProof,
    ) -> io::Result<Option<String>> {
        if !self.dir.exists() {
            return Ok(None);
        }
        let source = path.to_string_lossy();
        let _guard = self.lock(true)?;
        let Some(id) = self.lookup_locked(&source)? else {
            return Ok(None);
        };
        let dir = self.dir.join(id);
        let mut manifest = read_manifest(&dir)?;
        let Some(native) = &manifest.native else {
            return Ok(None);
        };
        if native.ledger.stat
            != (crate::events::Stat {
                dev: proof.dev,
                ino: proof.ino,
                size: proof.size,
                modified_ns: proof.modified_ns,
                changed_ns: proof.changed_ns,
            })
            || native.root != proof.source.root
            || native.path != proof.source.path
            || native.native_id != proof.source.native_id
            || native.ledger.offset != proof.source.offset
            || native.ledger.prefix != proof.source.prefix_sha256
            || native.ledger.tail != proof.source.tail_sha256
        {
            return Ok(None);
        }
        let chain_id = native.chain_id.clone();
        manifest
            .references
            .insert(format!("index:{source}:{chain_id}"), chain_id.clone());
        publish(&dir, MANIFEST, &manifest)?;
        Ok(Some(chain_id))
    }

    /// Caller holds the exclusive archive lock. Persist the time before custody
    /// publication so a crash can only postpone retention, never age fresh bytes.
    fn clock(&self) -> io::Result<ArchiveClock> {
        if !self.dir.join(CLOCK).try_exists()? && self.dir.join(SOURCES).try_exists()? {
            return Err(invalid(
                "archive clock state is missing; retention and admission refused",
            ));
        }
        read_json(&self.dir.join(CLOCK))
    }

    fn admission_time(&self) -> io::Result<u64> {
        let mut clock = self.clock()?;
        let observed = archive_now();
        if observed > clock.high_water || !self.dir.join(CLOCK).try_exists()? {
            clock.high_water = observed.max(clock.high_water);
            publish(&self.dir, CLOCK, &clock)?;
        }
        Ok(clock.high_water)
    }

    /// Explicit maintenance only. Unset retention does not even open the archive.
    pub fn prune(&self, retention_days: Option<u64>) -> io::Result<usize> {
        self.prune_at(retention_days, archive_now())
    }

    fn prune_at(&self, retention_days: Option<u64>, now: u64) -> io::Result<usize> {
        let Some(days) = retention_days else {
            return Ok(0);
        };
        let window = days
            .checked_mul(86400)
            .ok_or_else(|| invalid("archive retention days overflow"))?;
        if !self.dir.exists() {
            return Ok(0);
        }
        let _guard = self.lock(true)?;
        let clock = self.clock()?;
        if now < clock.high_water {
            return Err(invalid(
                "archive retention refused: clock moved backward before the recorded high-water time; no chains deleted",
            ));
        }
        let sources = {
            let mut known = self.known();
            self.refresh(&mut known)?;
            known.sources.clone()
        };
        let mut removed = 0;
        for (source, id) in sources {
            let dir = self.dir.join(id);
            let mut manifest = read_manifest(&dir)?;
            if !manifest.source.is_empty() && manifest.source != source {
                return Err(invalid("an archive source manifest mismatch"));
            }
            let expired: Vec<_> = manifest
                .chains
                .iter()
                .filter(|chain| {
                    now.saturating_sub(chain.admitted_at) > window
                        && !manifest.references.values().any(|id| id == &chain.chain_id)
                        && !manifest
                            .native
                            .as_ref()
                            .is_some_and(|native| native.chain_id == chain.chain_id)
                })
                .cloned()
                .collect();
            // Withdraw the manifest first; interruption can leave unreferenced
            // bytes behind but cannot leave a manifest naming deleted bytes.
            manifest
                .chains
                .retain(|chain| !expired.iter().any(|old| old.chain_id == chain.chain_id));
            if !expired.is_empty() {
                publish(&dir, MANIFEST, &manifest)?;
            }
            for chain in expired {
                fs::remove_file(dir.join(format!("{}.zst", chain.chain_id)))?;
                self.known().remove_verified(&chain.chain_id);
                removed += 1;
            }
            sync_dir(&dir)?;
        }
        publish(&self.dir, CLOCK, &ArchiveClock { high_water: now })?;
        Ok(removed)
    }

    /// Trust only the file and manifest previously checked. Missing files
    /// invalidate the cache too; damaged evidence is never extended.
    fn trusted(&self, dir: &Path, chain: &Chain) -> bool {
        let observed = Verification::observed(dir, chain).ok();
        if let Some(observed) = &observed
            && self
                .known()
                .verified
                .get(&chain.chain_id)
                .map(|held| &held.observation)
                == Some(observed)
        {
            return true;
        }
        self.known().remove_verified(&chain.chain_id);
        if let Some(observed) = observed
            && check_chain(dir, chain).is_ok()
            && Verification::observed(dir, chain).ok().as_ref() == Some(&observed)
        {
            self.known().verified.insert(
                chain.chain_id.clone(),
                Verified {
                    observation: observed,
                    prefix: None,
                },
            );
            return true;
        }
        false
    }

    /// Continue comparison inside an already verified chain. Repeated offsets
    /// reuse their digest; increasing bounded scans decompress each byte once.
    fn matches_prefix(
        &self,
        dir: &Path,
        chain: &Chain,
        offset: u64,
        expected: &[u8; 32],
    ) -> io::Result<bool> {
        let mut known = self.known();
        if known
            .verified
            .get(&chain.chain_id)
            .expect("trusted chain")
            .prefix
            .is_none()
        {
            if known.prefix_count() >= MAX_PREFIX_READERS {
                return Ok(false);
            }
            // Failed opens never reserve a slot.
            let reader = Box::new(open_chain(dir, chain)?);
            known
                .verified
                .get_mut(&chain.chain_id)
                .expect("trusted chain")
                .prefix = Some(ComparedPrefix {
                reader,
                hash: Sha256::new(),
                offset: 0,
                digests: BTreeMap::new(),
            });
            known.active_prefixes += 1;
        }
        let compared = known
            .verified
            .get_mut(&chain.chain_id)
            .expect("trusted chain")
            .prefix
            .as_mut()
            .expect("prefix reader");
        if let Some(digest) = compared.digests.get(&offset) {
            return Ok(digest == expected);
        }
        // An unseen earlier offset has no retained comparison proof. Start a
        // new chain from the native prefix rather than reread archived bytes.
        if offset < compared.offset || compared.digests.len() >= MAX_PREFIX_DIGESTS {
            return Ok(false);
        }
        let copied = io::copy(
            &mut compared.reader.by_ref().take(offset - compared.offset),
            &mut HashWriter(&mut compared.hash),
        )?;
        #[cfg(test)]
        VERIFIED_BYTES.with(|total| total.set(total.get() + copied));
        compared.offset += copied;
        let digest: [u8; 32] = compared.hash.clone().finalize().into();
        compared.digests.insert(compared.offset, digest);
        Ok(compared.offset == offset && digest == *expected)
    }

    fn release_prefix(&self, chain_id: &str) {
        self.known().release_prefix(chain_id);
    }

    /// Our own completed writes are already hashed, under the archive lock.
    fn remember(&self, dir: &Path, chain: &Chain) -> io::Result<()> {
        let observed = Verification::observed(dir, chain)?;
        let mut known = self.known();
        known.remove_verified(&chain.chain_id);
        known.verified.insert(
            chain.chain_id.clone(),
            Verified {
                observation: observed,
                prefix: None,
            },
        );
        Ok(())
    }

    fn known(&self) -> MutexGuard<'_, Known> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads what was appended to `sources.log` since it was last read.
    /// A torn final line (a crash mid-append) is ignored.
    fn refresh(&self, known: &mut Known) -> io::Result<()> {
        let mut file = match crate::open_input(&self.dir.join(SOURCES)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound && known.journal.is_none() => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let observed = crate::events::Stat::from_metadata(&file.metadata()?)?;
        if let Some(previous) = known.journal
            && (previous.dev, previous.ino) != (observed.dev, observed.ino)
        {
            return Err(invalid("archive source journal identity changed"));
        }
        if observed.size < known.read {
            *known = Known::default();
        }
        file.seek(SeekFrom::Start(known.read))?;
        let mut bytes = Vec::new();
        #[cfg(test)]
        if let Some(hook) = DURING_JOURNAL_READ.with(|hook| hook.borrow_mut().take()) {
            // Let a test pause after the first byte of the unread record.
            Read::by_ref(&mut file).take(1).read_to_end(&mut bytes)?;
            hook();
        }
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
        known.journal = Some(observed);
        Ok(())
    }

    /// The source's archive id, if it has one.
    fn lookup(&self, source: &str) -> io::Result<Option<String>> {
        let _guard = self.lock(false)?;
        self.lookup_locked(source)
    }

    /// Caller holds the flock. Lock order is always flock, then `known`.
    fn lookup_locked(&self, source: &str) -> io::Result<Option<String>> {
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
        let id = match self.lookup_locked(source)? {
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
        let created = !path.try_exists()?;
        #[cfg(test)]
        if let Some(hook) = BEFORE_REGISTER_OPEN.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        let mut file = crate::read_path::open_state(&path)?;
        let mut known = self.known();
        let observed = crate::events::Stat::from_metadata(&file.metadata()?)?;
        if known.journal.is_some_and(|expected| expected != observed)
            || (known.journal.is_none() && observed.size != 0)
            || crate::events::Stat::from_metadata(&crate::open_input(&path)?.metadata()?)?
                != observed
        {
            return Err(invalid(
                "archive source journal changed before registration",
            ));
        }
        // Cut off a torn final line, then append.
        file.set_len(known.read)?;
        file.seek(SeekFrom::Start(known.read))?;
        file.write_all(&line)?;
        file.sync_data()?;
        if created {
            sync_dir(&self.dir)?;
        }
        known.read += line.len() as u64;
        known.journal = Some(crate::events::Stat::from_metadata(&file.metadata()?)?);
        known.sources.insert(source.to_owned(), id.clone());
        Ok(id)
    }

    /// Takes the flock before any in-memory mutex: shared for journal reads,
    /// exclusive for writes (including torn-tail repair).
    fn lock(&self, exclusive: bool) -> io::Result<Guard> {
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
        let path = self.dir.join(LOCK);
        let file = crate::read_path::open_state(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let operation = if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            };
            if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let guard = Guard(file);
        #[cfg(test)]
        INHERITED_LOCKS.with(|capture| {
            if let Some(files) = capture.borrow_mut().as_mut() {
                files.push(guard.0.try_clone().unwrap());
            }
        });
        let held = crate::events::Stat::from_metadata(&guard.0.metadata()?)?;
        let named = crate::events::Stat::from_metadata(&crate::open_input(&path)?.metadata()?)?;
        if (held.dev, held.ino) != (named.dev, named.ino) {
            return Err(invalid("archive lock identity changed"));
        }
        Ok(guard)
    }
}

fn archive_now() -> u64 {
    #[cfg(test)]
    if let Some(now) = TEST_NOW.with(std::cell::Cell::get) {
        return now;
    }
    u64::try_from(crate::model::now_ms()).unwrap_or(0) / 1000
}

// Bounded index checkpoints synthesize only size. Custody may extend that
// prefix, while discovery still requires the complete physical generation.
fn same_generation(ledger: &crate::events::Ledger, physical: crate::events::Stat) -> bool {
    physical == ledger.stat
        || (ledger.stat.size == ledger.offset
            && physical.size >= ledger.offset
            && crate::events::Stat {
                size: ledger.stat.size,
                ..physical
            } == ledger.stat)
}

fn bind_native(
    manifest: &mut Manifest,
    native: Option<&NativeSource>,
    chain: &Chain,
) -> io::Result<bool> {
    if let Some(native) = native {
        // An incomplete checkpoint or EOF record cannot stand for the whole
        // indexed generation. Keep its bytes without advertising a rebuild.
        if native.ledger.offset != chain.length || hex(&native.ledger.prefix) != chain.sha256 {
            return Ok(false);
        }
        let physical = crate::events::Stat::of(&native.full_path())?;
        if !same_generation(&native.ledger, physical) {
            return Ok(false);
        }
        if native.ledger.offset == physical.size {
            if let Some(previous) = &manifest.native
                && previous.chain_id != chain.chain_id
            {
                manifest.references.insert(
                    format!("index-source:{}", previous.chain_id),
                    previous.chain_id.clone(),
                );
            }
            let mut native = native.clone();
            native.chain_id = chain.chain_id.clone();
            manifest.native = Some(native);
        } else if manifest
            .native
            .as_ref()
            .is_some_and(|held| held.chain_id == chain.chain_id)
        {
            manifest.native = None;
        }
    }
    Ok(true)
}

/// Starts a new chain from the first `offset` bytes of `log`; `false` when
/// they do not hash to `prefix`.
fn begin(
    dir: &Path,
    manifest: &mut Manifest,
    log: &mut (impl Read + Seek),
    offset: u64,
    prefix: &[u8; 32],
    native: Option<&NativeSource>,
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
        admitted_at: read_json::<ArchiveClock>(
            dir.parent()
                .expect("archive source parent")
                .join(CLOCK)
                .as_path(),
        )?
        .high_water,
        length: offset,
        sha256: hex(prefix),
        stored_bytes,
        redactor_version: NATIVE_REDACTOR.to_owned(),
        resume: Some(hasher.resume()),
    });
    let chain = manifest.chains.last().expect("new chain").clone();
    if !bind_native(manifest, native, &chain)? {
        return Ok(false);
    }
    publish(dir, MANIFEST, manifest)?;
    Ok(true)
}

/// Appends the bytes of `log` from `held.length` to `offset` to `held`, the
/// manifest's last chain, whose prefix the caller has just checked; `false`
/// when the log changed since and the bytes do not hash to `prefix`.
fn extend(
    dir: &Path,
    manifest: &mut Manifest,
    trusted: (&Chain, crate::events::Stat),
    log: &mut (impl Read + Seek),
    offset: u64,
    prefix: &[u8; 32],
    native: Option<&NativeSource>,
) -> io::Result<bool> {
    let (held, witness) = trusted;
    let path = dir.join(format!("{}.zst", held.chain_id));
    #[cfg(test)]
    if let Some(hook) = BEFORE_EXTEND_OPEN.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let mut file = crate::read_path::open_update(&path)?;
    if crate::events::Stat::from_metadata(&file.metadata()?)? != witness
        || crate::events::Stat::from_metadata(&crate::open_input(&path)?.metadata()?)? != witness
    {
        return Err(invalid("archive chain changed before extension"));
    }
    if file.metadata()?.len() < held.stored_bytes {
        return Err(invalid("an archive chain shorter than its manifest"));
    }
    // A torn tail from a crash after the last manifest.
    file.set_len(held.stored_bytes)?;
    file.seek(SeekFrom::End(0))?;
    let mut encoder = zstd::stream::write::Encoder::new(file, LEVEL)?;
    let copied = copy_range(
        log,
        held.resume.as_ref().expect("extendible chain"),
        held.length,
        offset,
        &mut encoder,
    );
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
    let chain = last.clone();
    if native.is_none()
        && manifest
            .native
            .as_ref()
            .is_some_and(|source| source.chain_id == chain.chain_id)
    {
        return Err(invalid(
            "native chain extension requires its source binding",
        ));
    }
    if !bind_native(manifest, native, &chain)? {
        return Ok(false);
    }
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
    if manifest
        .references
        .values()
        .any(|id| !seen.contains(id.as_str()))
    {
        return Err(invalid("a missing referenced archive chain"));
    }
    if let Some(native) = &manifest.native {
        let chain = manifest
            .chains
            .iter()
            .find(|chain| chain.chain_id == native.chain_id);
        if !crate::is_input_path(&native.root, &native.path)
            || !native.path.ends_with(".jsonl")
            || native.native_id.is_empty()
            || native.native_id.len() > 256
            || native.ledger.offset != native.ledger.stat.size
            || native.full_path().to_string_lossy() != manifest.source
            || native
                .sidecar
                .as_ref()
                .is_some_and(|sidecar| !sidecar.valid())
            || chain.is_none_or(|chain| {
                chain.length != native.ledger.offset || chain.sha256 != hex(&native.ledger.prefix)
            })
        {
            return Err(invalid("an invalid native archive binding"));
        }
    }
    Ok(manifest)
}

/// Reads `chain` back whole and checks its length and SHA-256.
fn check_chain(dir: &Path, chain: &Chain) -> io::Result<()> {
    let mut hash = Sha256::new();
    let bytes = io::copy(&mut open_chain(dir, chain)?, &mut HashWriter(&mut hash))?;
    #[cfg(test)]
    VERIFIED_BYTES.with(|total| total.set(total.get() + bytes));
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
    let temporary = dir.join(format!("{name}.{}.tmp", random_id()?));
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
    static TEST_NOW: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    static BEFORE_REGISTER_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_EXTEND_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static VERIFIED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static PREFIX_CAPACITY_WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static DURING_JOURNAL_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    /// Fails the next [`Archive::register`] before it writes.
    static FAIL_REGISTER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Fails the next [`publish`] after its temporary file is synced and
    /// before the rename, as a crash there would.
    static FAIL_BEFORE_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static INHERITED_LOCKS: std::cell::RefCell<Option<Vec<fs::File>>> = const { std::cell::RefCell::new(None) };
}

/// A new exclusive file, 0600; never follows a planted symlink.
fn create_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
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
