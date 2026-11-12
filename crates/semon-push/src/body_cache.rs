//! Expendable receiver custody for already-redacted typed pages, never native logs.
//!
//! Unix only: custody requires descriptor-checked private files and `O_NOFOLLOW`.
//! The operator supplies a separate 32-byte key file and excludes this directory
//! from backups. The future ACL/channel layer must authorize and audit each read
//! before constructing its single-use admission. This module does not verify ACLs.

use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, OsString},
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStringExt,
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const META_MAX: usize = 16 * 1024;
const HEADER: usize = 60; // wrapping nonce + encrypted 32-byte data key + tag
const OVERHEAD: usize = HEADER + 12 + 4 + 16;
const STATE_BYTES: u64 = 64;
const CLOCK_TOLERANCE: u64 = 2;

/// A consumed source in the exact admitted cohort; IDs are opaque, never paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceWitness {
    pub source_id: String,
    pub chain: String,
    pub generation: String,
    pub consumed_boundary: u64,
}

/// Complete origin-verified redacted envelope identity. All fields become AAD.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheKey {
    pub namespace: String,
    pub origin_incarnation: String,
    pub cohort: Vec<SourceWitness>,
    pub activity_generation: String,
    pub transcript_generation: String,
    pub export_view_generation: String,
    pub selector: Selector,
    pub payload_digest: [u8; 32],
    pub policy_versions: Vec<String>,
    pub authorization_scope: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Selector {
    Page(String),
    Field(String),
}

/// A trusted integration-layer assertion, not a bearer credential or ACL check.
/// No Clone: one admission is consumed by one lookup (including a miss).
/// Construct only after current origin authorization/audit and receiver
/// authorization/read-attempt audit have succeeded for this exact key.
pub struct ReadAdmission {
    key: CacheKey,
    origin_limits: Option<Limits>,
}

impl ReadAdmission {
    pub fn after_online_authorization_and_audit(
        key: CacheKey,
        origin_limits: Option<Limits>,
    ) -> Self {
        Self { key, origin_limits }
    }
}

/// Receiver ceilings; optional origin limits can only narrow these values.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub global_bytes: u64,
    pub source_bytes: u64,
    pub ttl_seconds: u64,
    pub response_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            global_bytes: 256 * 1024 * 1024,
            source_bytes: 64 * 1024 * 1024,
            ttl_seconds: 15 * 60,
            response_bytes: 256 * 1024,
        }
    }
}

impl Limits {
    fn narrowed(self, other: Self) -> Self {
        Self {
            global_bytes: self.global_bytes.min(other.global_bytes),
            source_bytes: self.source_bytes.min(other.source_bytes),
            ttl_seconds: self.ttl_seconds.min(other.ttl_seconds),
            response_bytes: self.response_bytes.min(other.response_bytes),
        }
    }
}

/// Seconds since the Unix epoch; inject a fake clock in tests. Never sleep.
pub trait Clock {
    fn now(&self) -> u64;
}

#[derive(Debug, PartialEq, Eq)]
pub enum CacheError {
    /// No plaintext may be released; facts and native storage are unaffected.
    Disabled(&'static str),
    ResourceLimit,
    InvalidEnvelope,
    Integrity,
    Io(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled(reason) => write!(formatter, "receiver body cache disabled: {reason}"),
            Self::ResourceLimit => formatter
                .write_str("receiver body cache resource limit; request a narrower page or field"),
            Self::InvalidEnvelope => formatter.write_str("invalid receiver body envelope"),
            Self::Integrity => formatter.write_str("receiver body cache authentication failed"),
            Self::Io(error) => write!(formatter, "receiver body cache I/O: {error}"),
        }
    }
}

impl std::error::Error for CacheError {}

type Result<T> = std::result::Result<T, CacheError>;

impl From<std::io::Error> for CacheError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    key: CacheKey,
    fetched: u64,
    expires: u64,
}

struct Entry {
    metadata: Metadata,
    bytes: u64,
    used: u64,
}

/// Exclusive owner of a dedicated, private cache directory. No transport wiring.
/// Methods require mutable access, so staging and eviction are serialized.
pub struct BodyCache<C: Clock> {
    dir: PathBuf,
    directory: CacheDirectory,
    key_path: PathBuf,
    master: Zeroizing<[u8; 32]>,
    clock: C,
    high_water: u64,
    sequence: u64,
    limits: Limits,
    entries: BTreeMap<String, Entry>,
    disabled: bool,
    _lock: File,
}

impl<C: Clock> BodyCache<C> {
    /// The directory must be dedicated to this cache, not facts or archives.
    /// An absent key is an error; this API never creates or repairs operator keys.
    pub fn open(dir: &Path, key_path: &Path, clock: C, limits: Limits) -> Result<Self> {
        if !dir.exists() {
            fs::DirBuilder::new().mode(0o700).create(dir)?;
        }
        let directory = CacheDirectory::open(dir)?;
        let dir = fs::canonicalize(dir)?;
        let key_path = fs::canonicalize(key_path.parent().unwrap_or(Path::new(".")))
            .map_err(|_| CacheError::Disabled("operator master key is missing or unreadable"))?
            .join(
                key_path
                    .file_name()
                    .ok_or(CacheError::Disabled("invalid key path"))?,
            );
        if key_path.starts_with(&dir) {
            return Err(CacheError::Disabled(
                "master key must be outside cache and its backups",
            ));
        }
        let master = read_master(&key_path)?;
        let lock = directory.open_file(Path::new("lock"), libc::O_RDWR | libc::O_CREAT)?;
        lock.try_lock()
            .map_err(|_| CacheError::Disabled("cache already in use"))?;
        let now = clock.now();
        let mut cache = Self {
            dir,
            directory,
            key_path,
            master,
            clock,
            high_water: now,
            sequence: 0,
            limits: Limits::default().narrowed(limits),
            entries: BTreeMap::new(),
            disabled: false,
            _lock: lock,
        };
        #[cfg(test)]
        tests::before_startup_sweep();
        // Interrupted staging is never admitted. Its bytes are removed before
        // any serving or new allocation, including when the clock state is lost.
        for path in cache.directory.names()? {
            if path.extension().is_some_and(|ext| ext == "stage") {
                cache.directory.destroy(&path)?;
            }
        }
        let state_path = Path::new("clock");
        let trusted = if cache
            .directory
            .names()?
            .iter()
            .any(|name| name == state_path)
        {
            let state = cache
                .directory
                .read_bounded(state_path, STATE_BYTES as usize)?;
            let decoded = decrypt(&cache.master, &state, b"semon-body-cache-clock")
                .map_err(|_| CacheError::Disabled("wrong master key or corrupt clock state"))?;
            let bytes: [u8; 8] = decoded
                .as_slice()
                .try_into()
                .map_err(|_| CacheError::Disabled("inconsistent clock state"))?;
            let previous = u64::from_le_bytes(bytes);
            cache.high_water = now.max(previous);
            now.saturating_add(CLOCK_TOLERANCE) >= previous
        } else {
            false
        };
        for path in cache.directory.names()? {
            if path.extension().is_none_or(|ext| ext != "entry") {
                if path
                    .file_name()
                    .is_some_and(|name| name == "clock" || name == "lock")
                {
                    continue;
                }
                return Err(CacheError::Disabled("unexpected cache directory member"));
            }
            let loaded = if trusted {
                cache.load_entry(&path, now)
            } else {
                Err(CacheError::Integrity)
            };
            match loaded {
                Ok(entry) => {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_owned();
                    cache.sequence = cache.sequence.max(entry.used);
                    cache.entries.insert(name, entry);
                }
                Err(_) => cache.directory.destroy(&path)?,
            }
        }
        if !trusted {
            cache.high_water = now;
        }
        cache.enforce(0, &[], cache.limits)?;
        cache.save_clock()?;
        Ok(cache)
    }

    /// Insert only origin-rendered, redacted, verified typed bytes. `fetched` is
    /// the fetch time, not insertion time; delaying insertion cannot renew TTL.
    pub fn insert(
        &mut self,
        key: CacheKey,
        body: &[u8],
        fetched: u64,
        origin: Option<Limits>,
    ) -> Result<()> {
        let now = self.prepare()?;
        let limits = self.limits.narrowed(origin.unwrap_or_default());
        validate_key(&key)?;
        if body.len() > limits.response_bytes || limits.ttl_seconds == 0 {
            return Err(CacheError::ResourceLimit);
        }
        if Sha256::digest(body).as_slice() != key.payload_digest {
            return Err(CacheError::Integrity);
        }
        let expires = fetched
            .checked_add(limits.ttl_seconds)
            .ok_or(CacheError::InvalidEnvelope)?;
        if fetched > now || expires <= now {
            return Err(CacheError::InvalidEnvelope);
        }
        let metadata = Metadata {
            key,
            fetched,
            expires,
        };
        let aad = serde_json::to_vec(&metadata).map_err(|_| CacheError::InvalidEnvelope)?;
        if aad.len() > META_MAX || aad.len() + body.len() > limits.response_bytes {
            return Err(CacheError::ResourceLimit);
        }
        let size = (aad.len() + body.len() + OVERHEAD) as u64;
        let sources = source_scopes(&metadata.key);
        // A newly verified cohort/policy is also a detected invalidation signal.
        let stale: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                let old = &entry.metadata.key;
                old.namespace == metadata.key.namespace
                    && old.origin_incarnation == metadata.key.origin_incarnation
                    && old.cohort.iter().any(|source| {
                        metadata.key.cohort.iter().any(|new| {
                            source.source_id == new.source_id
                                && (source.generation != new.generation
                                    || source.chain != new.chain
                                    || old.policy_versions != metadata.key.policy_versions)
                        })
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.remove(&id)?;
        }
        self.enforce(size, &sources, limits)?;
        let id = key_id(&metadata.key)?;
        // Existing entries count while staging a replacement. Removing first
        // also makes a crash during replacement a miss rather than stale data.
        self.remove(&id)?;
        let mut data_key = Zeroizing::new([0u8; 32]);
        random(&mut *data_key)?;
        let wrapped = encrypt(&self.master, data_key.as_ref(), &aad)?;
        let encrypted = encrypt(&data_key, body, &aad)?;
        let mut record = Vec::with_capacity(size as usize);
        record.extend_from_slice(&wrapped);
        record.extend_from_slice(&(aad.len() as u32).to_le_bytes());
        record.extend_from_slice(&aad);
        record.extend_from_slice(&encrypted);
        let stage = self.dir.join(format!("{id}.stage"));
        let mut renamed = false;
        let result = (|| -> Result<()> {
            let mut file = self.directory.create(&stage)?;
            file.write_all(&record)?;
            file.sync_all()?;
            self.directory.rename(&stage, &self.path(&id))?;
            renamed = true;
            #[cfg(test)]
            tests::after_rename()?;
            self.directory.0.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.disabled = true;
            if renamed {
                self.directory.unlink(&self.path(&id))?;
            }
            return result;
        }
        self.sequence += 1;
        self.entries.insert(
            id,
            Entry {
                metadata,
                bytes: size,
                used: self.sequence,
            },
        );
        Ok(())
    }

    /// Consumes proof of a fresh online authorization and both pre-read audits.
    /// The caller must recheck limits/policy and acquire a new admission on misses.
    pub fn lookup(&mut self, admission: ReadAdmission) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let now = self.prepare()?;
        validate_key(&admission.key)?;
        let limits = self
            .limits
            .narrowed(admission.origin_limits.unwrap_or_default());
        self.enforce(0, &[], limits)?;
        let id = key_id(&admission.key)?;
        let Some(entry) = self.entries.get(&id) else {
            return Ok(None);
        };
        if entry.metadata.key != admission.key {
            return Err(CacheError::Integrity);
        }
        if now.saturating_sub(entry.metadata.fetched) >= limits.ttl_seconds {
            self.remove(&id)?;
            return Ok(None);
        }
        let path = self.path(&id);
        let result = self.decode(&path);
        let (_, body) = match result {
            Ok(value) => value,
            Err(error) => {
                self.remove(&id)?;
                return Err(error);
            }
        };
        let envelope_bytes = serde_json::to_vec(&self.entries[&id].metadata)
            .map_err(|_| CacheError::Integrity)?
            .len()
            + body.len();
        if envelope_bytes > limits.response_bytes {
            return Err(CacheError::ResourceLimit);
        }
        self.sequence += 1;
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.used = self.sequence;
        }
        Ok(Some(body))
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Remove a request's temporary entry if its final release decision fails.
    pub(crate) fn discard(&mut self, key: &CacheKey) -> Result<()> {
        self.remove(&key_id(key)?)
    }

    /// Policy/redactor changes, rewrites and access withdrawal all revoke an
    /// entire affected source, across authorization scopes and cohort members.
    pub fn invalidate_source(
        &mut self,
        namespace: &str,
        incarnation: &str,
        source: &str,
    ) -> Result<()> {
        self.prepare()?;
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| {
                e.metadata.key.namespace == namespace
                    && e.metadata.key.origin_incarnation == incarnation
                    && e.metadata.key.cohort.iter().any(|s| s.source_id == source)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.remove(&id)?;
        }
        Ok(())
    }

    /// Explicit rotation: invalidate all entries before loading a replacement
    /// operator key. No old policy or entry is promoted to the new key.
    pub fn rotate_key(&mut self, key_path: &Path) -> Result<()> {
        self.disabled = true;
        self.clear()?;
        self.master.zeroize();
        let path = fs::canonicalize(key_path.parent().unwrap_or(Path::new(".")))?.join(
            key_path
                .file_name()
                .ok_or(CacheError::Disabled("invalid key path"))?,
        );
        if path.starts_with(&self.dir) {
            return Err(CacheError::Disabled("master key must be outside cache"));
        }
        self.master = read_master(&path)?;
        self.key_path = path;
        self.high_water = self.clock.now();
        self.save_clock()?;
        self.disabled = false;
        Ok(())
    }

    fn prepare(&mut self) -> Result<u64> {
        if self.disabled {
            return Err(CacheError::Disabled(
                "cache disabled; reopen after fixing custody",
            ));
        }
        let current = read_master(&self.key_path);
        if !matches!(current, Ok(ref key) if **key == *self.master) {
            self.disabled = true;
            self.master.zeroize();
            return Err(CacheError::Disabled(
                "operator key missing, unreadable or changed; rotate explicitly",
            ));
        }
        let now = self.clock.now();
        if now.saturating_add(CLOCK_TOLERANCE) < self.high_water {
            self.clear()?;
            self.disabled = true;
            return Err(CacheError::Disabled(
                "clock regressed; cached bodies invalidated",
            ));
        }
        self.high_water = self.high_water.max(now);
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| e.metadata.expires <= self.high_water)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.remove(&id)?;
        }
        if let Err(error) = self.save_clock() {
            self.disabled = true;
            return Err(error);
        }
        Ok(self.high_water)
    }

    fn save_clock(&self) -> Result<()> {
        let bytes = encrypt(
            &self.master,
            &self.high_water.to_le_bytes(),
            b"semon-body-cache-clock",
        )?;
        let path = self.dir.join("clock.stage");
        let mut file = self.directory.create(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.directory.rename(&path, Path::new("clock"))?;
        self.directory.0.sync_all()?;
        Ok(())
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.entry"))
    }

    fn remove(&mut self, id: &str) -> Result<()> {
        if self.entries.contains_key(id) {
            if let Err(error) = self.directory.destroy(&self.path(id)) {
                self.disabled = true;
                return Err(error);
            }
            self.entries.remove(id);
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        for id in ids {
            self.remove(&id)?;
        }
        Ok(())
    }

    fn enforce(&mut self, staged: u64, sources: &[String], limits: Limits) -> Result<()> {
        // Two clock records coexist during its atomic replacement. Lock is empty.
        let fixed = 2 * STATE_BYTES;
        if staged.saturating_add(fixed) > limits.global_bytes || staged > limits.source_bytes {
            return Err(CacheError::ResourceLimit);
        }
        loop {
            let total: u64 = self.entries.values().map(|e| e.bytes).sum();
            let mut usage = BTreeMap::<String, u64>::new();
            for entry in self.entries.values() {
                for source in source_scopes(&entry.metadata.key) {
                    *usage.entry(source).or_default() += entry.bytes;
                }
            }
            for source in sources {
                *usage.entry(source.clone()).or_default() += staged;
            }
            let over_sources: Vec<_> = usage
                .into_iter()
                .filter(|(_, bytes)| *bytes > limits.source_bytes)
                .map(|(source, _)| source)
                .collect();
            let global_over =
                total.saturating_add(staged).saturating_add(fixed) > limits.global_bytes;
            if !global_over && over_sources.is_empty() {
                return Ok(());
            }
            let victim = self
                .entries
                .iter()
                .filter(|(_, e)| {
                    global_over
                        || source_scopes(&e.metadata.key)
                            .iter()
                            .any(|s| over_sources.contains(s))
                })
                .min_by_key(|(_, e)| e.used)
                .map(|(id, _)| id.clone())
                .ok_or(CacheError::ResourceLimit)?;
            self.remove(&victim)?;
        }
    }

    fn decode(&self, path: &Path) -> Result<(Metadata, Zeroizing<Vec<u8>>)> {
        let record = self
            .directory
            .read_bounded(path, META_MAX + Limits::default().response_bytes + OVERHEAD)?;
        if record.len() < OVERHEAD {
            return Err(CacheError::Integrity);
        }
        let len = u32::from_le_bytes(
            record[HEADER..HEADER + 4]
                .try_into()
                .map_err(|_| CacheError::Integrity)?,
        ) as usize;
        if len > META_MAX || HEADER + 4 + len + 28 > record.len() {
            return Err(CacheError::Integrity);
        }
        let aad = &record[HEADER + 4..HEADER + 4 + len];
        let key = decrypt(&self.master, &record[..HEADER], aad)?;
        let data_key: [u8; 32] = key
            .as_slice()
            .try_into()
            .map_err(|_| CacheError::Integrity)?;
        let data_key = Zeroizing::new(data_key);
        let body = decrypt(&data_key, &record[HEADER + 4 + len..], aad)?;
        let metadata: Metadata = serde_json::from_slice(aad).map_err(|_| CacheError::Integrity)?;
        validate_key(&metadata.key)?;
        let expected_name = format!("{}.entry", key_id(&metadata.key)?);
        if path.file_name().and_then(|s| s.to_str()) != Some(expected_name.as_str())
            || Sha256::digest(body.as_slice()).as_slice() != metadata.key.payload_digest
        {
            return Err(CacheError::Integrity);
        }
        Ok((metadata, body))
    }

    fn load_entry(&self, path: &Path, now: u64) -> Result<Entry> {
        let (metadata, body) = self.decode(path)?;
        if metadata.fetched > now.saturating_add(CLOCK_TOLERANCE)
            || metadata.expires <= self.high_water
            || metadata.expires <= metadata.fetched
            || metadata.expires - metadata.fetched > self.limits.ttl_seconds
            || serde_json::to_vec(&metadata)
                .map_err(|_| CacheError::Integrity)?
                .len()
                + body.len()
                > self.limits.response_bytes
        {
            return Err(CacheError::Integrity);
        }
        let bytes = self
            .directory
            .open_file(path, libc::O_RDONLY)?
            .metadata()?
            .len();
        let used = metadata.fetched;
        Ok(Entry {
            metadata,
            bytes,
            used,
        })
    }
}

fn validate_key(key: &CacheKey) -> Result<()> {
    if key.cohort.is_empty()
        || key.cohort.len() > 64
        || key.policy_versions.is_empty()
        || key.policy_versions.len() > 64
    {
        return Err(CacheError::InvalidEnvelope);
    }
    let selector = match &key.selector {
        Selector::Page(s) | Selector::Field(s) => s,
    };
    let mut fields = vec![
        &key.namespace,
        &key.origin_incarnation,
        &key.activity_generation,
        &key.transcript_generation,
        &key.export_view_generation,
        selector,
        &key.authorization_scope,
    ];
    for source in &key.cohort {
        fields.extend([&source.source_id, &source.chain, &source.generation]);
    }
    fields.extend(&key.policy_versions);
    if fields.iter().any(|s| s.is_empty())
        || fields.iter().map(|s| s.len()).sum::<usize>() > META_MAX / 2
    {
        return Err(CacheError::InvalidEnvelope);
    }
    Ok(())
}

fn key_id(key: &CacheKey) -> Result<String> {
    let bytes = serde_json::to_vec(key).map_err(|_| CacheError::InvalidEnvelope)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn source_scopes(key: &CacheKey) -> Vec<String> {
    key.cohort
        .iter()
        .map(|s| {
            // Length-prefixed tuple serialization avoids delimiter collisions.
            serde_json::to_string(&(&key.namespace, &key.origin_incarnation, &s.source_id))
                .expect("string tuple serialization")
        })
        .collect()
}

fn private_open(path: &Path, create: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.permissions().mode() & 0o777 != 0o600 {
        return Err(CacheError::Disabled(
            "custody file must be a regular 0600 file",
        ));
    }
    Ok(file)
}

#[cfg(test)]
fn private_create(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

fn read_master(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let mut file = private_open(path, false).map_err(|_| {
        CacheError::Disabled("operator key must be a readable regular 0600 file, not a symlink")
    })?;
    if file.metadata()?.len() != 32 {
        return Err(CacheError::Disabled(
            "operator key must contain exactly 32 bytes",
        ));
    }
    let mut key = Zeroizing::new([0u8; 32]);
    file.read_exact(&mut *key)
        .map_err(|_| CacheError::Disabled("operator key unreadable"))?;
    Ok(key)
}

/// Pins custody to an inode, even if a parent writer replaces its pathname.
pub(crate) struct CacheDirectory(File);

impl CacheDirectory {
    pub(crate) fn sync(&self) -> Result<()> {
        self.0.sync_all()?;
        Ok(())
    }

    pub(crate) fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let meta = file.metadata()?;
        // SAFETY: geteuid has no arguments or memory preconditions.
        let owner = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.uid() != owner || meta.permissions().mode() & 0o777 != 0o700 {
            return Err(CacheError::Disabled(
                "cache directory must be owned by this user and mode 0700",
            ));
        }
        Ok(Self(file))
    }

    fn name(path: &Path) -> Result<CString> {
        let name = path.file_name().ok_or(CacheError::Integrity)?;
        use std::os::unix::ffi::OsStrExt;
        CString::new(name.as_bytes()).map_err(|_| CacheError::Integrity)
    }

    pub(crate) fn open_file(&self, path: &Path, flags: libc::c_int) -> Result<File> {
        let name = Self::name(path)?;
        // SAFETY: the directory fd is live, the name is NUL terminated and the
        // mode argument is provided even when O_CREAT is absent.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600 as libc::mode_t,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a fresh descriptor, now exclusively owned here.
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata()?;
        // SAFETY: geteuid has no memory preconditions.
        let owner = unsafe { libc::geteuid() };
        if !meta.is_file()
            || meta.uid() != owner
            || meta.nlink() != 1
            || meta.permissions().mode() & 0o777 != 0o600
        {
            return Err(CacheError::Disabled(
                "cache member must be an owned regular 0600 file with one link",
            ));
        }
        Ok(file)
    }

    fn create(&self, path: &Path) -> Result<File> {
        self.open_file(path, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let from = Self::name(from)?;
        let to = Self::name(to)?;
        // SAFETY: both names are valid C strings and the directory fd stays open.
        let result = unsafe {
            libc::renameat(
                self.0.as_raw_fd(),
                from.as_ptr(),
                self.0.as_raw_fd(),
                to.as_ptr(),
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(crate) fn names(&self) -> Result<Vec<PathBuf>> {
        // A separate open file description starts each scan at offset zero.
        // SAFETY: the literal is NUL terminated and the directory fd is live.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fd is a fresh directory descriptor; fdopendir takes ownership on success.
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            // SAFETY: fdopendir failed, so ownership of fd remains ours.
            unsafe {
                libc::close(fd);
            }
            return Err(error.into());
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                // SAFETY: this is the unique owner of a successful fdopendir.
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(stream);
        let mut names = Vec::new();
        loop {
            // readdir_r returns errors directly without platform-specific errno access.
            // SAFETY: zero is a valid initial dirent; readdir_r initializes the entry.
            let mut entry: libc::dirent = unsafe { std::mem::zeroed() };
            let mut result = std::ptr::null_mut();
            // SAFETY: the stream is live, output storage is valid, and access is serialized.
            #[allow(deprecated)]
            let error = unsafe { libc::readdir_r(stream.0, &mut entry, &mut result) };
            if error != 0 {
                return Err(std::io::Error::from_raw_os_error(error).into());
            }
            if result.is_null() {
                break;
            }
            // SAFETY: a successful readdir_r produces a NUL-terminated d_name.
            let name = unsafe { CStr::from_ptr(entry.d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                names.push(PathBuf::from(OsString::from_vec(name.to_vec())));
            }
        }
        Ok(names)
    }

    fn read_bounded(&self, path: &Path, cap: usize) -> Result<Vec<u8>> {
        let file = self.open_file(path, libc::O_RDONLY)?;
        if file.metadata()?.len() > cap as u64 {
            return Err(CacheError::Integrity);
        }
        let mut bytes = Vec::new();
        file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > cap {
            return Err(CacheError::Integrity);
        }
        Ok(bytes)
    }

    /// Cleanup must unlink even when the failing operation was directory fsync.
    fn unlink(&self, path: &Path) -> Result<()> {
        let name = Self::name(path)?;
        // SAFETY: the filename is validated and the pinned directory fd is live.
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.0.sync_all()?;
        Ok(())
    }
    /// Destroy the wrapped key before unlink, without following any path component.
    fn destroy(&self, path: &Path) -> Result<()> {
        let mut file = self.open_file(path, libc::O_WRONLY)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&[0; HEADER][..HEADER.min(len as usize)])?;
        file.sync_all()?;
        let name = Self::name(path)?;
        // SAFETY: name is NUL terminated, directory fd is live; no directory deletion.
        let result = unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.0.sync_all()?;
        Ok(())
    }
}

fn random(bytes: &mut [u8]) -> Result<()> {
    getrandom::fill(bytes).map_err(|_| CacheError::Disabled("system randomness unavailable"))
}

fn encrypt(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| CacheError::Integrity)?;
    let mut nonce = [0u8; 12];
    random(&mut nonce)?;
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| CacheError::Integrity)?;
    let mut result = nonce.to_vec();
    result.extend_from_slice(&encrypted);
    Ok(result)
}

fn decrypt(key: &[u8; 32], bytes: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() < 28 {
        return Err(CacheError::Integrity);
    }
    let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| CacheError::Integrity)?;
    cipher
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CacheError::Integrity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    thread_local! {
        static BEFORE_STARTUP_SWEEP: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn before_startup_sweep() {
        let hook = BEFORE_STARTUP_SWEEP.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }

    #[derive(Clone)]
    struct FakeClock(Rc<Cell<u64>>);
    impl Clock for FakeClock {
        fn now(&self) -> u64 {
            self.0.get()
        }
    }

    struct Fixture {
        root: PathBuf,
        key: PathBuf,
        dir: PathBuf,
        clock: FakeClock,
    }
    impl Fixture {
        fn new() -> Self {
            let mut nonce = [0; 16];
            random(&mut nonce).unwrap();
            let root =
                std::env::temp_dir().join(format!("semon-body-cache-{}", hex::encode(nonce)));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let key = root.join("operator-key");
            private_create(&key).unwrap().write_all(&[7; 32]).unwrap();
            Self {
                dir: root.join("cache"),
                root,
                key,
                clock: FakeClock(Rc::new(Cell::new(1000))),
            }
        }
        fn open(&self, limits: Limits) -> BodyCache<FakeClock> {
            BodyCache::open(&self.dir, &self.key, self.clock.clone(), limits).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    fn key(selector: &str, source: &str, body: &[u8]) -> CacheKey {
        CacheKey {
            namespace: "namespace".into(),
            origin_incarnation: "enrollment-1".into(),
            cohort: vec![SourceWitness {
                source_id: source.into(),
                chain: "chain".into(),
                generation: "generation-1".into(),
                consumed_boundary: 100,
            }],
            activity_generation: "activity-1".into(),
            transcript_generation: "transcript-1".into(),
            export_view_generation: "export-1".into(),
            selector: Selector::Page(selector.into()),
            payload_digest: Sha256::digest(body).into(),
            policy_versions: vec!["policy-1".into(), "redactor-1".into()],
            authorization_scope: "principal-1".into(),
        }
    }
    fn admission(key: &CacheKey) -> ReadAdmission {
        ReadAdmission::after_online_authorization_and_audit(key.clone(), None)
    }

    thread_local! { static FAIL_RENAMED_SYNC: Cell<bool> = const { Cell::new(false) }; }
    pub(super) fn after_rename() -> Result<()> {
        FAIL_RENAMED_SYNC.with(|fail| {
            if fail.get() {
                Err(CacheError::Integrity)
            } else {
                Ok(())
            }
        })
    }
    #[test]
    fn renamed_entry_is_unlinked_on_sync_failure_and_absent_after_reopen() {
        let fixture = Fixture::new();
        let body = b"private";
        let key = key("page", "source", body);
        let mut cache = fixture.open(Limits::default());
        FAIL_RENAMED_SYNC.with(|fail| fail.set(true));
        assert!(cache.insert(key.clone(), body, 1000, None).is_err());
        FAIL_RENAMED_SYNC.with(|fail| fail.set(false));
        assert!(!cache.path(&key_id(&key).unwrap()).exists());
        assert_eq!(cache.entry_count(), 0);
        drop(cache);
        let mut reopened = fixture.open(Limits::default());
        assert!(reopened.lookup(admission(&key)).unwrap().is_none());
    }
    #[test]
    fn roundtrip_restart_and_no_plaintext_files() {
        let f = Fixture::new();
        let sentinel = b"PRIVATE-BODY-SENTINEL-DO-NOT-PERSIST";
        let key = key("first", "source", sentinel);
        let mut cache = f.open(Limits::default());
        cache.insert(key.clone(), sentinel, 1000, None).unwrap();
        assert_eq!(
            cache.lookup(admission(&key)).unwrap().unwrap().as_slice(),
            sentinel
        );
        for item in fs::read_dir(&f.dir).unwrap() {
            let bytes = fs::read(item.unwrap().path()).unwrap();
            assert!(!bytes.windows(sentinel.len()).any(|part| part == sentinel));
        }
        drop(cache);
        let mut cache = f.open(Limits::default());
        assert_eq!(
            cache.lookup(admission(&key)).unwrap().unwrap().as_slice(),
            sentinel
        );
    }

    #[test]
    fn ttl_is_absolute_and_origin_can_narrow_it() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let key = key("page", "source", b"body");
        cache.insert(key.clone(), b"body", 1000, None).unwrap();
        f.clock.0.set(1899);
        assert!(cache.lookup(admission(&key)).unwrap().is_some());
        f.clock.0.set(1900);
        assert!(cache.lookup(admission(&key)).unwrap().is_none());
        cache.insert(key.clone(), b"body", 1900, None).unwrap();
        f.clock.0.set(1905);
        let limits = Limits {
            ttl_seconds: 5,
            ..Limits::default()
        };
        assert!(
            cache
                .lookup(ReadAdmission::after_online_authorization_and_audit(
                    key,
                    Some(limits)
                ))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn tamper_and_metadata_swap_are_refused() {
        for metadata_swap in [false, true] {
            let f = Fixture::new();
            let mut cache = f.open(Limits::default());
            let first = key("first", "source", b"body");
            let second = key("other", "source", b"body");
            cache.insert(first.clone(), b"body", 1000, None).unwrap();
            cache.insert(second.clone(), b"body", 1000, None).unwrap();
            let path = cache.path(&key_id(&first).unwrap());
            let mut bytes = fs::read(&path).unwrap();
            if metadata_swap {
                let other = fs::read(cache.path(&key_id(&second).unwrap())).unwrap();
                let end = HEADER
                    + 4
                    + u32::from_le_bytes(bytes[HEADER..HEADER + 4].try_into().unwrap()) as usize;
                bytes[HEADER + 4..end].copy_from_slice(&other[HEADER + 4..end]);
            } else {
                *bytes.last_mut().unwrap() ^= 1;
            }
            fs::write(path, bytes).unwrap();
            assert_eq!(
                cache.lookup(admission(&first)).unwrap_err(),
                CacheError::Integrity
            );
        }
    }

    #[test]
    fn missing_wrong_loose_and_symlink_keys_fail_closed() {
        let f = Fixture::new();
        let cache = f.open(Limits::default());
        drop(cache);
        fs::write(&f.key, [9; 32]).unwrap();
        assert!(matches!(
            BodyCache::open(&f.dir, &f.key, f.clock.clone(), Limits::default()),
            Err(CacheError::Disabled(_))
        ));
        fs::set_permissions(&f.key, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_master(&f.key).is_err());
        fs::set_permissions(&f.key, fs::Permissions::from_mode(0o600)).unwrap();
        let link = f.root.join("key-link");
        std::os::unix::fs::symlink(&f.key, &link).unwrap();
        assert!(BodyCache::open(&f.dir, &link, f.clock.clone(), Limits::default()).is_err());
        fs::remove_file(&f.key).unwrap();
        assert!(BodyCache::open(&f.dir, &f.key, f.clock.clone(), Limits::default()).is_err());
    }

    #[test]
    fn live_key_loss_disables_without_releasing_body() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let key = key("page", "source", b"body");
        cache.insert(key.clone(), b"body", 1000, None).unwrap();
        fs::remove_file(&f.key).unwrap();
        assert!(matches!(
            cache.lookup(admission(&key)),
            Err(CacheError::Disabled(_))
        ));
    }

    #[test]
    fn caps_include_metadata_staging_and_lru() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let a = key("a", "source", b"body");
        let b = key("b", "source", b"body");
        let c = key("c", "source", b"body");
        cache.insert(a.clone(), b"body", 1000, None).unwrap();
        let size = cache.entries.values().next().unwrap().bytes;
        cache.limits.global_bytes = size * 2 + 2 * STATE_BYTES;
        cache.insert(b.clone(), b"body", 1000, None).unwrap();
        cache.lookup(admission(&a)).unwrap();
        cache.insert(c.clone(), b"body", 1000, None).unwrap();
        assert!(cache.lookup(admission(&b)).unwrap().is_none());
        assert!(cache.lookup(admission(&a)).unwrap().is_some());
        assert!(cache.lookup(admission(&c)).unwrap().is_some());
        cache.limits.source_bytes = size;
        cache.insert(b.clone(), b"body", 1000, None).unwrap();
        assert_eq!(cache.entries.len(), 1);
        // A staged replacement must fit in addition to the old entry.
        cache.insert(b, b"body", 1000, None).unwrap();
        assert_eq!(cache.entries.len(), 1);
        let limited = Limits {
            global_bytes: size + 2 * STATE_BYTES - 1,
            ..Limits::default()
        };
        assert_eq!(
            cache.insert(a, b"body", 1000, Some(limited)),
            Err(CacheError::ResourceLimit)
        );
        let too_large = vec![0; 256 * 1024 + 1];
        assert_eq!(
            cache.insert(key("big", "source", &too_large), &too_large, 1000, None),
            Err(CacheError::ResourceLimit)
        );
    }

    #[test]
    fn source_cap_is_independent_and_counts_each_cohort_member() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let a = key("a", "one", b"body");
        let b = key("b", "two", b"body");
        cache.insert(a.clone(), b"body", 1000, None).unwrap();
        let size = cache.entries.values().next().unwrap().bytes;
        cache.limits.source_bytes = size;
        cache.insert(b.clone(), b"body", 1000, None).unwrap();
        assert_eq!(cache.entries.len(), 2);
        let c = key("c", "one", b"body");
        cache.insert(c, b"body", 1000, None).unwrap();
        assert!(cache.lookup(admission(&a)).unwrap().is_none());
        assert!(cache.lookup(admission(&b)).unwrap().is_some());
    }

    #[test]
    fn invalidation_generation_policy_withdrawal_and_rotation() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let a = key("page", "source", b"body");
        cache.insert(a.clone(), b"body", 1000, None).unwrap();
        let mut b = a.clone();
        b.cohort[0].generation = "rewrite".into();
        cache.insert(b.clone(), b"body", 1000, None).unwrap();
        assert!(cache.lookup(admission(&a)).unwrap().is_none());
        let mut c = b.clone();
        c.policy_versions = vec!["new-policy".into()];
        cache.insert(c.clone(), b"body", 1000, None).unwrap();
        assert!(cache.lookup(admission(&b)).unwrap().is_none());
        cache
            .invalidate_source("namespace", "enrollment-1", "source")
            .unwrap();
        assert!(cache.lookup(admission(&c)).unwrap().is_none());
        cache.insert(c.clone(), b"body", 1000, None).unwrap();
        fs::write(&f.key, [8; 32]).unwrap();
        cache.rotate_key(&f.key).unwrap();
        assert!(cache.lookup(admission(&c)).unwrap().is_none());
    }

    #[test]
    fn startup_sweeps_expiry_invalid_ttl_and_crash_leftovers() {
        for missing_clock in [false, true] {
            let f = Fixture::new();
            let mut cache = f.open(Limits::default());
            let key = key("page", "source", b"body");
            cache.insert(key.clone(), b"body", 1000, None).unwrap();
            drop(cache);
            private_create(&f.dir.join("crash.stage"))
                .unwrap()
                .write_all(b"encrypted partial write")
                .unwrap();
            if missing_clock {
                fs::remove_file(f.dir.join("clock")).unwrap();
            } else {
                f.clock.0.set(1900);
            }
            let mut cache = f.open(Limits::default());
            assert!(cache.lookup(admission(&key)).unwrap().is_none());
            assert!(!f.dir.join("crash.stage").exists());
        }
    }

    #[test]
    fn cohort_charges_each_source_and_smaller_response_limits_apply_on_hits() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let mut cohort = key("page", "one", b"body");
        let mut member = cohort.cohort[0].clone();
        member.source_id = "two".into();
        cohort.cohort.push(member);
        cache.insert(cohort.clone(), b"body", 1000, None).unwrap();
        let size = cache.entries.values().next().unwrap().bytes;
        cache.limits.source_bytes = size;
        let next = key("next", "two", b"body");
        cache.insert(next.clone(), b"body", 1000, None).unwrap();
        assert!(cache.lookup(admission(&cohort)).unwrap().is_none());
        let smaller = Limits {
            response_bytes: 4,
            ..Limits::default()
        };
        assert_eq!(
            cache
                .lookup(ReadAdmission::after_online_authorization_and_audit(
                    next,
                    Some(smaller)
                ))
                .unwrap_err(),
            CacheError::ResourceLimit
        );
    }

    #[test]
    fn origin_header_source_limit_refuses_insertion() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let limits = crate::origin_read::CacheLimits {
            cache_per_source_bytes: std::num::NonZeroU64::new(1),
            ..crate::origin_read::CacheLimits::default()
        }
        .narrowed(Limits::default());
        assert_eq!(
            cache
                .insert(key("page", "source", b"body"), b"body", 1000, Some(limits))
                .unwrap_err(),
            CacheError::ResourceLimit
        );
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn authenticated_but_inconsistent_ttl_is_swept() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let key = key("page", "source", b"body");
        cache.insert(key.clone(), b"body", 1000, None).unwrap();
        let id = key_id(&key).unwrap();
        let path = cache.path(&id);
        let (mut metadata, body) = cache.decode(&path).unwrap();
        metadata.expires = metadata.fetched;
        let aad = serde_json::to_vec(&metadata).unwrap();
        let data_key = Zeroizing::new([3; 32]);
        let mut record = encrypt(&cache.master, data_key.as_ref(), &aad).unwrap();
        record.extend_from_slice(&(aad.len() as u32).to_le_bytes());
        record.extend_from_slice(&aad);
        record.extend_from_slice(&encrypt(&data_key, &body, &aad).unwrap());
        fs::write(path, record).unwrap();
        drop(cache);
        let mut cache = f.open(Limits::default());
        assert!(cache.lookup(admission(&key)).unwrap().is_none());
    }

    #[test]
    fn directory_swap_cannot_redirect_writes_reads_or_key_destruction() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let first = key("first", "source", b"body");
        cache.insert(first.clone(), b"body", 1000, None).unwrap();
        let original = f.root.join("original-cache");
        fs::rename(&f.dir, &original).unwrap();
        let outside = f.root.join("outside");
        fs::DirBuilder::new().mode(0o700).create(&outside).unwrap();
        let victim = outside.join(format!("{}.entry", key_id(&first).unwrap()));
        let sentinel = b"outside-file-must-not-be-overwritten-or-removed";
        fs::write(&victim, sentinel).unwrap();
        std::os::unix::fs::symlink(&outside, &f.dir).unwrap();
        assert_eq!(
            cache.lookup(admission(&first)).unwrap().unwrap().as_slice(),
            b"body"
        );
        let second = key("second", "source", b"next");
        cache.insert(second.clone(), b"next", 1000, None).unwrap();
        assert!(
            original
                .join(format!("{}.entry", key_id(&second).unwrap()))
                .exists()
        );
        cache
            .invalidate_source("namespace", "enrollment-1", "source")
            .unwrap();
        assert_eq!(fs::read(&victim).unwrap(), sentinel);
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        assert!(
            !original
                .join(format!("{}.entry", key_id(&first).unwrap()))
                .exists()
        );
        assert!(
            !original
                .join(format!("{}.entry", key_id(&second).unwrap()))
                .exists()
        );
        drop(cache);
        fs::remove_file(&f.dir).unwrap();
        fs::rename(&original, &f.dir).unwrap();
        let cache = f.open(Limits::default());
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn startup_sweep_stays_on_pinned_directory_after_path_swap() {
        let f = Fixture::new();
        let mut cache = f.open(Limits::default());
        let key = key("expired", "source", b"body");
        cache.insert(key.clone(), b"body", 1000, None).unwrap();
        let entry_name = format!("{}.entry", key_id(&key).unwrap());
        drop(cache);
        private_create(&f.dir.join("crash.stage"))
            .unwrap()
            .write_all(b"interrupted staging")
            .unwrap();
        f.clock.0.set(1900);

        let original = f.root.join("pinned-cache");
        let outside = f.root.join("outside-startup");
        fs::DirBuilder::new().mode(0o700).create(&outside).unwrap();
        let sentinel = b"outside-startup-sentinel";
        for name in ["sentinel", "outside.stage", entry_name.as_str()] {
            private_create(&outside.join(name))
                .unwrap()
                .write_all(sentinel)
                .unwrap();
        }
        let cache_path = f.dir.clone();
        let moved_path = original.clone();
        let outside_path = outside.clone();
        let called = Rc::new(Cell::new(false));
        let hook_called = Rc::clone(&called);
        BEFORE_STARTUP_SWEEP.with(|slot| {
            assert!(
                slot.borrow_mut()
                    .replace(Box::new(move || {
                        fs::rename(&cache_path, &moved_path).unwrap();
                        std::os::unix::fs::symlink(&outside_path, &cache_path).unwrap();
                        hook_called.set(true);
                    }))
                    .is_none()
            );
        });

        let cache = f.open(Limits::default());
        assert!(called.get());
        assert!(cache.entries.is_empty());
        assert!(!original.join("crash.stage").exists());
        assert!(!original.join(&entry_name).exists());
        for name in ["sentinel", "outside.stage", entry_name.as_str()] {
            assert_eq!(fs::read(outside.join(name)).unwrap(), sentinel);
        }
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 3);
    }

    #[test]
    fn backwards_clock_invalidates_in_process_and_after_restart() {
        for restart in [false, true] {
            let f = Fixture::new();
            let mut cache = f.open(Limits::default());
            let key = key("page", "source", b"body");
            cache.insert(key.clone(), b"body", 1000, None).unwrap();
            f.clock.0.set(900);
            if restart {
                drop(cache);
                let mut cache = f.open(Limits::default());
                assert!(cache.lookup(admission(&key)).unwrap().is_none());
            } else {
                assert!(matches!(
                    cache.lookup(admission(&key)),
                    Err(CacheError::Disabled(_))
                ));
                assert!(cache.entries.is_empty());
            }
        }
    }
}
