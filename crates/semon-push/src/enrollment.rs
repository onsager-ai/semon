//! Atomic receiver registry for pairing, read tokens and machine metadata.
//! Missing custody never means an empty registry.
use serde::{Deserialize, Serialize};
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub const DIRECTORY: &str = "enrollment";
pub const REGISTRY_FILE: &str = "registry.json";
pub const LOCK_FILE: &str = "registry.lock";
pub const FORMAT: u32 = semon_sessions::received_registry::RECEIVER_FORMAT;
pub const CAP: u64 = 4 * 1024 * 1024;
pub const TERMINAL_RESERVE: u64 = 256 * 1024;
pub const ADMISSION_CAP: u64 = CAP - TERMINAL_RESERVE;

pub const UNSUPPORTED_FORMAT: &str = "Receiver state uses an unsupported format. Keep DIR unchanged; use a new empty receiver directory and pair each origin again. No state is migrated or deleted.";
pub const STATE_FULL: &str = "Receiver state is full. Keep DIR unchanged; use a new empty receiver directory and pair each origin again. No state is migrated or deleted.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Custody,
    InUse,
    Invalid,
    UnsupportedVersion,
    Terminal,
    StaleSnapshot,
    Random,
    Write,
    CommitUncertain,
    LegacyTokens,
    Full,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Custody => "enrollment custody is missing, unreadable or insecure",
            Self::InUse => "enrollment registry is locked",
            Self::Invalid => "invalid enrollment registry",
            Self::UnsupportedVersion => UNSUPPORTED_FORMAT,
            Self::Full => STATE_FULL,
            Self::Terminal => "terminal pairing or credential history cannot be cleared",
            Self::StaleSnapshot => "stale enrollment snapshot; reload before updating",
            Self::Random => "system random source failed",
            Self::Write => "enrollment update failed before commit",
            Self::CommitUncertain => "enrollment update was renamed; reopen custody to resolve commit outcome",
            Self::LegacyTokens => "legacy tokens found; keep this directory unchanged and pair origins into a fresh receiver directory",
        })
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

/// Uses the same secure random provider as tokens.rs; identities are never names.
pub fn random_id() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    Ok(hex::encode(bytes))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    format: u32,
    revision: u64,
    pub(crate) read_tokens: crate::read_token::State,
    pub(crate) simple_pair: crate::simple_pair::State,
    pub(crate) machines: Vec<semon_sessions::received_registry::ReceivedMachine>,
    pub(crate) machine_audit: Vec<semon_sessions::received_registry::MachineAudit>,
    pub(crate) latest: u64,
}
impl Registry {
    pub fn empty() -> Self {
        Self {
            format: FORMAT,
            revision: 0,
            read_tokens: crate::read_token::State::default(),
            simple_pair: crate::simple_pair::State::default(),
            machines: vec![],
            machine_audit: vec![],
            latest: 0,
        }
    }
    pub fn machines(&self) -> &[semon_sessions::received_registry::ReceivedMachine] {
        &self.machines
    }
    pub fn machine_audit(&self) -> &[semon_sessions::received_registry::MachineAudit] {
        &self.machine_audit
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// A bounded, custody-checked snapshot of the atomically published registry.
    /// Readers need no transaction lock and never hold a lock while handling requests.
    pub fn read_current(receiver_dir: &Path) -> Result<Self> {
        Self::read_current_at(&Directory::open(receiver_dir)?.0)
    }
    /// Snapshot relative to an already pinned receiver root; never reopens its path.
    pub(crate) fn read_current_at(receiver_dir: &File) -> Result<Self> {
        let parent = Directory::from_file(receiver_dir.try_clone().map_err(|_| Error::Custody)?)?;
        let dir = parent.child()?;
        dir.file(LOCK_FILE, libc::O_RDONLY)?;
        dir.load()
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > CAP {
            return Err(Error::Invalid);
        }
        // Inspect only the version before the strict current-format decoder. Old
        // fields never reach a legacy reader or become an empty current registry.
        let version: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if version.get("format").and_then(serde_json::Value::as_u64) != Some(u64::from(FORMAT)) {
            return Err(Error::UnsupportedVersion);
        }
        let registry: Self = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        registry.validate()?;
        Ok(registry)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::UnsupportedVersion);
        }
        if !semon_sessions::received_registry::validate_machines(&self.machines) {
            return Err(Error::Invalid);
        }
        if self.machine_audit.iter().any(|entry| {
            entry.ids != [entry.before.id.clone()]
                || entry.before.id != entry.after.id
                || entry.before.directory != entry.after.directory
                || !self.machines.iter().any(|m| m.id == entry.after.id)
                || entry.time > self.latest
                || !semon_sessions::received_registry::validate_machines(std::slice::from_ref(
                    &entry.before,
                ))
                || !semon_sessions::received_registry::validate_machines(std::slice::from_ref(
                    &entry.after,
                ))
                || match entry.action.as_str() {
                    "name_set" => entry.after.owner_name.is_none(),
                    "name_clear" => entry.after.owner_name.is_some(),
                    "paired" | "paired_replacement" => entry.before != entry.after,
                    _ => true,
                }
        }) {
            return Err(Error::Invalid);
        }
        self.validate_read_tokens()?;
        self.validate_simple_pair()
    }
    fn follows(&self, old: &Self) -> Result<()> {
        self.read_tokens_follow(old)?;
        self.simple_pair_follows(old)?;
        if self.latest < old.latest || !self.machine_audit.starts_with(&old.machine_audit) {
            return Err(Error::Terminal);
        }
        for machine in &old.machines {
            let next = self
                .machines
                .iter()
                .find(|m| m.id == machine.id && m.directory == machine.directory)
                .ok_or(Error::Terminal)?;
            let mut expected = machine.clone();
            for entry in &self.machine_audit[old.machine_audit.len()..] {
                if entry.ids == [machine.id.clone()] {
                    if entry.before != expected {
                        return Err(Error::Invalid);
                    }
                    expected = entry.after.clone();
                }
            }
            if &expected != next {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub(crate) fn ensure_machine(&mut self, id: &str, name: Option<&str>) -> Result<()> {
        if !semon_sessions::machine_id::valid(id) {
            return Err(Error::Invalid);
        }
        if !self.machines.iter().any(|m| m.id == id) {
            self.machines
                .push(semon_sessions::received_registry::ReceivedMachine {
                    id: id.into(),
                    directory: id.into(),
                    owner_name: name.map(str::to_owned),
                });
        }
        Ok(())
    }
    pub(crate) fn check_admission(&self) -> Result<()> {
        let size = serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() as u64;
        if size > ADMISSION_CAP
            || size.saturating_add(self.simple_pair.tokens.len() as u64 * 20) > CAP
        {
            return Err(Error::Full);
        }
        Ok(())
    }
}
/// Pure decision over a caller-observed directory entry; no import, read or write.
pub fn legacy_state(present: bool) -> Result<()> {
    if present {
        Err(Error::LegacyTokens)
    } else {
        Ok(())
    }
}
/// Inspect only metadata, including dangling symlinks. Never open legacy tokens.
pub fn refuse_legacy_tokens(receiver_dir: &Path) -> Result<()> {
    match fs::symlink_metadata(receiver_dir.join(crate::tokens::TOKENS_FILE)) {
        Ok(_) => legacy_state(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => legacy_state(false),
        Err(_) => Err(Error::Custody),
    }
}

/// Descriptor-relative custody; no pathname is reopened after directory validation.
struct Directory(File);
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| Error::Custody)?;
        Self::from_file(file)
    }
    fn from_file(file: File) -> Result<Self> {
        let meta = file.metadata().map_err(|_| Error::Custody)?;
        // SAFETY: geteuid has no memory preconditions.
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.permissions().mode() & 0o777 != 0o700
        {
            return Err(Error::Custody);
        }
        Ok(Self(file))
    }
    fn child(&self) -> Result<Self> {
        // SAFETY: live directory descriptor and constant NUL-terminated name.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                c"enrollment".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(Error::Custody);
        }
        // SAFETY: openat returned a fresh owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata().map_err(|_| Error::Custody)?;
        // SAFETY: geteuid has no memory preconditions.
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.permissions().mode() & 0o777 != 0o700
        {
            return Err(Error::Custody);
        }
        Ok(Self(file))
    }
    fn file(&self, name: &str, flags: i32) -> Result<File> {
        let name = std::ffi::CString::new(name).map_err(|_| Error::Custody)?;
        // SAFETY: live directory descriptor, NUL-terminated single-component name.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600 as libc::mode_t,
            )
        };
        if fd < 0 {
            return Err(Error::Custody);
        }
        // SAFETY: openat returned a newly owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let m = file.metadata().map_err(|_| Error::Custody)?;
        // SAFETY: geteuid has no memory preconditions.
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.nlink() != 1
            || m.permissions().mode() & 0o777 != 0o600
        {
            return Err(Error::Custody);
        }
        Ok(file)
    }
    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let from = std::ffi::CString::new(from).map_err(|_| Error::Write)?;
        let to = std::ffi::CString::new(to).map_err(|_| Error::Write)?;
        // SAFETY: valid C strings and a live directory descriptor.
        if unsafe {
            libc::renameat(
                self.0.as_raw_fd(),
                from.as_ptr(),
                self.0.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(Error::Write);
        }
        Ok(())
    }
    fn remove(&self, name: &str) {
        if let Ok(name) = std::ffi::CString::new(name) {
            // SAFETY: valid C string and live descriptor. Only our fresh stage is removed.
            unsafe {
                libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0);
            }
        }
    }
    fn load(&self) -> Result<Registry> {
        let file = self.file(REGISTRY_FILE, libc::O_RDONLY)?;
        let mut bytes = vec![];
        file.take(CAP + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Custody)?;
        Registry::parse(&bytes)
    }
}
struct LockGuard(File);
impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Hold the exclusive lock for a transaction and authorize from a fresh snapshot.
/// Drop explicitly unlocks, even if another process inherited/duplicated the fd.
pub struct Store {
    dir: Directory,
    _lock: LockGuard,
    failed: bool,
}
impl Store {
    /// Explicit first initialization only. Existing/missing registry is never
    /// silently repaired. The receiver parent must already exist and be private.
    pub fn initialize(receiver_dir: &Path) -> Result<Self> {
        Self::initialize_at(&Directory::open(receiver_dir)?.0)
    }
    /// Initialize under an already pinned receiver root, including gate and lock.
    pub(crate) fn initialize_at(receiver_dir: &File) -> Result<Self> {
        let parent = Directory::from_file(receiver_dir.try_clone().map_err(|_| Error::Custody)?)?;
        let name = c"enrollment";
        // SAFETY: live parent descriptor and constant child name.
        if unsafe { libc::mkdirat(parent.0.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            return Err(Error::Custody);
        }
        let mut store = Self::acquire_at(&parent)?;
        store.write(&Registry::empty(), |_| Ok(()))?;
        parent.0.sync_all().map_err(|_| Error::CommitUncertain)?;
        Ok(store)
    }
    fn acquire(receiver_dir: &Path) -> Result<Self> {
        // Validate parent as well; opening a symlink as the dedicated child fails.
        Self::acquire_at(&Directory::open(receiver_dir)?)
    }
    fn acquire_at(parent: &Directory) -> Result<Self> {
        let dir = parent.child()?;
        let file = dir.file(LOCK_FILE, libc::O_RDWR | libc::O_CREAT)?;
        file.try_lock().map_err(|_| Error::InUse)?;
        Ok(Self {
            dir,
            _lock: LockGuard(file),
            failed: false,
        })
    }
    pub fn open(receiver_dir: &Path) -> Result<Self> {
        let store = Self::acquire(receiver_dir)?;
        store.dir.load()?;
        Ok(store)
    }
    pub fn snapshot(&self) -> Result<Registry> {
        if self.failed {
            Err(Error::CommitUncertain)
        } else {
            self.dir.load()
        }
    }
    /// Re-read once under the registry lock when a pairing caller observed a
    /// stale revision. The closure edits only that fresh snapshot; no retry loop.
    pub(crate) fn pairing_transaction<T>(
        &mut self,
        expected: u64,
        edit: impl FnOnce(&mut Registry) -> Result<T>,
    ) -> Result<T> {
        if self.failed {
            return Err(Error::CommitUncertain);
        }
        let current = self.dir.load()?;
        // Expected revision is advisory here: a stale observation is resolved by
        // the fresh read under this lock, never by committing cached authority.
        let _stale = expected != current.revision();
        let mut next = current.clone();
        let output = edit(&mut next)?;
        next.follows(&current)?;
        if next.simple_pair.tokens.len() > current.simple_pair.tokens.len() {
            next.check_admission()?;
        }
        next.revision = current.revision.checked_add(1).ok_or(Error::Invalid)?;
        self.write(&next, |_| Ok(()))?;
        Ok(output)
    }
    /// Compare-and-swap a snapshot revision under the exclusive lock. Callers
    /// must supply the revision they read, including for newly built records.
    pub fn commit(&mut self, expected_revision: u64, next: &Registry) -> Result<()> {
        self.commit_with_fault(expected_revision, next, |_| Ok(()))
    }
    pub(crate) fn commit_with_fault(
        &mut self,
        expected_revision: u64,
        next: &Registry,
        fault: impl FnMut(Stage) -> Result<()>,
    ) -> Result<()> {
        if self.failed {
            return Err(Error::CommitUncertain);
        }
        let current = self.dir.load()?;
        if expected_revision != current.revision {
            return Err(Error::StaleSnapshot);
        }
        next.follows(&current)?;
        if next.simple_pair.tokens.len() > current.simple_pair.tokens.len() {
            next.check_admission()?;
        }
        let mut next = next.clone();
        next.revision = current.revision.checked_add(1).ok_or(Error::Invalid)?;
        self.write(&next, fault)
    }
    fn write(&mut self, next: &Registry, mut fault: impl FnMut(Stage) -> Result<()>) -> Result<()> {
        next.validate()?;
        let bytes = serde_json::to_vec(next).map_err(|_| Error::Invalid)?;
        if bytes.len() as u64 > CAP {
            return Err(Error::Invalid);
        }
        let stage = format!("stage-{}", random_id()?);
        let mut file = self
            .dir
            .file(&stage, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        let before = (|| {
            file.write_all(&bytes).map_err(|_| Error::Write)?;
            fault(Stage::Written)?;
            file.sync_all().map_err(|_| Error::Write)?;
            fault(Stage::Synced)?;
            self.dir.rename(&stage, REGISTRY_FILE)
        })();
        if let Err(e) = before {
            self.dir.remove(&stage);
            return Err(e);
        }
        // Never continue with a cached pre-commit view after rename uncertainty.
        if fault(Stage::Renamed).is_err()
            || { self.dir.0.sync_all().is_err() }
            || fault(Stage::DirectorySynced).is_err()
        {
            self.failed = true;
            return Err(Error::CommitUncertain);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Written,
    Synced,
    Renamed,
    DirectorySynced,
}

impl Store {
    pub fn name_machine(&mut self, id: &str, name: Option<&str>, now: u64) -> Result<()> {
        if name.is_some_and(|n| !semon_sessions::received_registry::valid_owner_name(n)) {
            return Err(Error::Invalid);
        }
        let mut next = self.snapshot()?;
        let machine = next
            .machines
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(Error::Invalid)?;
        let before = machine.clone();
        machine.owner_name = name.map(str::to_owned);
        let after = machine.clone();
        let time = now.max(next.latest);
        next.machine_audit
            .push(semon_sessions::received_registry::MachineAudit {
                time,
                action: if name.is_some() {
                    "name_set"
                } else {
                    "name_clear"
                }
                .into(),
                ids: vec![id.into()],
                before,
                after,
            });
        next.latest = time;
        next.check_admission()?;
        self.commit(next.revision(), &next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_registry_roundtrips_and_retired_state_is_refused() {
        let registry = Registry::empty();
        let mut value = serde_json::to_value(&registry).unwrap();
        assert_eq!(
            Registry::parse(&serde_json::to_vec(&value).unwrap()).unwrap(),
            registry
        );
        value["format"] = serde_json::json!(FORMAT - 1);
        assert_eq!(
            Registry::parse(&serde_json::to_vec(&value).unwrap()),
            Err(Error::UnsupportedVersion)
        );
        value["format"] = serde_json::json!(FORMAT);
        value["enrollments"] = serde_json::json!([]);
        assert_eq!(
            Registry::parse(&serde_json::to_vec(&value).unwrap()),
            Err(Error::Invalid)
        );
    }
}
