//! Receiver enrollment registry, deliberately unwired from receive/CLI defaults.
//! A single transaction contains bindings and hashes; missing custody never means
//! an empty registry. Unix custody follows the receiver ACL/audit conventions.
use crate::credentials::{Bearer, Generations, Kind};
use serde::{Deserialize, Serialize};
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub const DIRECTORY: &str = "enrollment";
pub const REGISTRY_FILE: &str = "registry.json";
pub const LOCK_FILE: &str = "registry.lock";
pub const FORMAT: u32 = 2;
pub const CAP: u64 = 4 * 1024 * 1024;
pub const TERMINAL_RESERVE: u64 = 256 * 1024;
pub const ADMISSION_CAP: u64 = CAP - TERMINAL_RESERVE;

pub const UNSUPPORTED_FORMAT: &str = "Receiver state uses an unsupported format. Keep DIR unchanged; use a new empty receiver directory and pair each origin again. See docs/design/asi-enrollment.md#offline-machines-and-missed-renewal.";
pub const STATE_FULL: &str = "Receiver state is full. Keep DIR unchanged; use a new empty receiver directory and pair each origin again. See docs/design/asi-enrollment.md#offline-machines-and-missed-renewal.";
pub const AUDIT_UNAVAILABLE: &str = "Receiver audit is unavailable. Keep DIR unchanged; restore access to its audit storage before enrolling or renewing.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Custody,
    GateUnavailable,
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
    AuditUnavailable,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Custody => "enrollment custody is missing, unreadable or insecure",
            Self::GateUnavailable => "pairing gate is unavailable; session creation refused",
            Self::InUse => "enrollment registry is locked",
            Self::Invalid => "invalid enrollment registry",
            Self::UnsupportedVersion => UNSUPPORTED_FORMAT,
            Self::Full => STATE_FULL,
            Self::AuditUnavailable => AUDIT_UNAVAILABLE,
            Self::Terminal => "terminal enrollment or credential history cannot be cleared; recover with a new enrollment ID",
            Self::StaleSnapshot => "stale enrollment snapshot; reload before updating",
            Self::Random => "system random source failed",
            Self::Write => "enrollment update failed before commit",
            Self::CommitUncertain => "enrollment update was renamed; reopen custody to resolve commit outcome",
            Self::LegacyTokens => "legacy tokens found; re-issue kind credentials through explicit enrollment; leave tokens and history untouched",
        })
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

/// Uses the same secure random provider as tokens.rs; identities are never names.
pub fn random_id() -> Result<String> {
    let mut bytes = [0u8; 32];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| Error::Random)?;
    Ok(hex::encode(bytes))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "sha256",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Pin {
    Spki([u8; 32]),
    Leaf([u8; 32]),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificatePin {
    pub pin: Pin,
    pub generation: u64,
    /// Terminal pin history is retained rather than deleting replaced authority.
    pub revoked_at: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "epoch",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ClaimEpoch {
    Unassigned,
    Assigned(u64),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssuerKind {
    Receiver,
    External,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Issuer {
    pub kind: IssuerKind,
    pub origin_trust_acknowledged: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuerKey {
    pub key_id: String,
    pub public_key: [u8; 32],
}

/// Inapplicable is explicit for mirror-only scope; no placeholder namespace,
/// incarnation, epoch or issuer is fabricated. Admitted bindings are supplied
/// by the owning local producer/provisioning lifecycle, never by request bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Binding {
    Inapplicable,
    Admitted {
        namespace: String,
        origin_incarnation: String,
        index_incarnation: String,
        publisher_claim_epoch: ClaimEpoch,
        issuer: Issuer,
        assertion_issuer_keys: Vec<IssuerKey>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindScope {
    pub kind: Kind,
    pub client_cert_required: bool,
    pub generations: Generations,
    pub available: bool,
    pub enable_authorization: Option<EnableAuthorization>,
}
impl KindScope {
    pub fn new(kind: Kind, generation: u64) -> Self {
        Self {
            kind,
            client_cert_required: kind == Kind::BodyChannel,
            generations: Generations {
                current: generation,
                retiring: None,
            },
            available: true,
            enable_authorization: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableAuthorization {
    pub enrollment_id: String,
    pub kind: Kind,
    pub owner: String,
    pub authorization_id: String,
    pub created_at: u64,
    pub consumed_renewal_id: Option<String>,
}
/// Compact terminal lookup and no-resurrection evidence; never grants authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tombstone {
    pub enrollment_id: String,
    pub machine: String,
    pub sequence: u64,
    pub pin: Option<Pin>,
    /// Analytics, body, mirror, respectively. Zero denotes a never-enrolled kind.
    pub high_water: [u64; 3],
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub enrollment_id: String,
    pub receiver_id: String,
    pub machine: String,
    pub sequence: u64,
    pub origin_key: Vec<u8>,
    pub endpoint: String,
    pub enrolled_pin: Option<Pin>,
    pub binding: Binding,
    pub client_certificate_pins: Vec<CertificatePin>,
    pub credential_kinds: Vec<KindScope>,
    pub revoked_at: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    format: u32,
    revision: u64,
    pub(crate) endpoint: Option<String>,
    pub(crate) enrollments: Vec<Enrollment>,
    pub(crate) credentials: Vec<Bearer>,
    pub(crate) pairing_receipts: Vec<crate::pairing::Receipt>,
    pub(crate) tombstones: Vec<Tombstone>,
    pub(crate) latest: u64,
    pub(crate) next_sequence: u64,
}
impl Registry {
    pub fn empty() -> Self {
        Self {
            format: FORMAT,
            revision: 0,
            endpoint: None,
            enrollments: vec![],
            credentials: vec![],
            pairing_receipts: vec![],
            tombstones: vec![],
            latest: 0,
            next_sequence: 1,
        }
    }
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
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
    pub fn enrollments(&self) -> &[Enrollment] {
        &self.enrollments
    }
    pub fn credentials(&self) -> &[Bearer] {
        &self.credentials
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
    /// Build an activation snapshot. Persistence compares it with current custody
    /// under lock; a caller cannot remove or rewrite terminal history.
    pub fn from_records(enrollments: Vec<Enrollment>, credentials: Vec<Bearer>) -> Result<Self> {
        let next_sequence = enrollments
            .iter()
            .map(|e| e.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Invalid)?;
        let endpoint = enrollments.first().map(|e| e.endpoint.clone());
        let registry = Self {
            format: FORMAT,
            revision: 0,
            endpoint,
            enrollments,
            credentials,
            pairing_receipts: vec![],
            tombstones: vec![],
            latest: 0,
            next_sequence,
        };
        registry.validate()?;
        Ok(registry)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::UnsupportedVersion);
        }
        if self
            .endpoint
            .as_ref()
            .is_some_and(|e| crate::pairing::normalize_endpoint(e).as_ref() != Ok(e))
            || (self.endpoint.is_none() && !self.enrollments.is_empty())
        {
            return Err(Error::Invalid);
        }
        let mut identities = BTreeSet::new();
        let mut machines = BTreeSet::new();
        let mut namespaces = BTreeSet::new();
        let mut pins = BTreeSet::new();
        let mut receiver = None;
        for t in &self.tombstones {
            if !opaque(&t.enrollment_id)
                || !semon_sessions::is_machine_name(&t.machine)
                || !identities.insert(&t.enrollment_id)
                || serde_json::to_vec(t).map_err(|_| Error::Invalid)?.len() >= 512
            {
                return Err(Error::Invalid);
            }
            if let Some(pin) = &t.pin {
                if pins.iter().any(|(p, _)| *p == pin) {
                    return Err(Error::Invalid);
                }
                pins.insert((pin, &t.enrollment_id));
            }
        }
        for e in &self.enrollments {
            if !opaque(&e.enrollment_id)
                || !identifier(&e.receiver_id)
                || !semon_sessions::is_machine_name(&e.machine)
                || !identities.insert(&e.enrollment_id)
                || e.credential_kinds.is_empty()
                || e.credential_kinds.len() > 3
                || e.origin_key.len() != 65
                || e.endpoint != crate::pairing::normalize_endpoint(&e.endpoint)?
                || serde_json::to_vec(e).map_err(|_| Error::Invalid)?.len() >= 8192
                || receiver.is_some_and(|id| id != &e.receiver_id)
            {
                return Err(Error::Invalid);
            }
            receiver = Some(&e.receiver_id);
            if e.revoked_at.is_none()
                && self.endpoint.as_deref() == Some(e.endpoint.as_str())
                && !machines.insert(&e.machine)
            {
                return Err(Error::Invalid);
            }
            let full = e
                .credential_kinds
                .iter()
                .any(|k| k.kind != Kind::MirrorPush);
            match &e.binding {
                Binding::Inapplicable if !full => {}
                Binding::Admitted {
                    namespace,
                    origin_incarnation,
                    index_incarnation,
                    publisher_claim_epoch,
                    issuer,
                    assertion_issuer_keys,
                } if full => {
                    if !identifier(namespace)
                        || !identifier(origin_incarnation)
                        || !identifier(index_incarnation)
                        || matches!(publisher_claim_epoch, ClaimEpoch::Assigned(0))
                        || !issuer.origin_trust_acknowledged
                        || assertion_issuer_keys.is_empty()
                        || assertion_issuer_keys.len() > 2
                        || assertion_issuer_keys.iter().any(|k| !identifier(&k.key_id))
                        || assertion_issuer_keys
                            .iter()
                            .map(|k| &k.key_id)
                            .collect::<BTreeSet<_>>()
                            .len()
                            != assertion_issuer_keys.len()
                        || assertion_issuer_keys
                            .iter()
                            .map(|k| k.public_key)
                            .collect::<BTreeSet<_>>()
                            .len()
                            != assertion_issuer_keys.len()
                        || (e.revoked_at.is_none() && !namespaces.insert(namespace))
                    {
                        return Err(Error::Invalid);
                    }
                }
                _ => return Err(Error::Invalid),
            }
            let mut kinds = BTreeSet::new();
            for k in &e.credential_kinds {
                if !kinds.insert(k.kind)
                    || k.generations.current == 0
                    || k.generations
                        .retiring
                        .as_ref()
                        .is_some_and(|r| r.generation == 0 || r.generation >= k.generations.current)
                    || (k.kind == Kind::BodyChannel && !k.client_cert_required)
                {
                    return Err(Error::Invalid);
                }
                if k.enable_authorization.as_ref().is_some_and(|a| {
                    a.enrollment_id != e.enrollment_id
                        || a.kind != k.kind
                        || !opaque(&a.authorization_id)
                        || !identifier(&a.owner)
                        || a.consumed_renewal_id.as_ref().is_some_and(|id| !opaque(id))
                        || (k.available && a.consumed_renewal_id.is_none())
                }) {
                    return Err(Error::Invalid);
                }
                if k.available
                    && e.revoked_at.is_none()
                    && self.credentials.iter().any(|b| {
                        b.enrollment_id == e.enrollment_id
                            && b.kind == k.kind
                            && b.generation == k.generations.current
                            && b.valid_until > self.latest
                    })
                    && k.client_cert_required
                    && !e
                        .client_certificate_pins
                        .iter()
                        .any(|p| p.generation == k.generations.current && p.revoked_at.is_none())
                {
                    return Err(Error::Invalid);
                }
            }
            if let Some(pin) = &e.enrolled_pin {
                if pins
                    .iter()
                    .any(|(p, id)| *p == pin && *id != &e.enrollment_id)
                {
                    return Err(Error::Invalid);
                }
                pins.insert((pin, &e.enrollment_id));
            }
            let mut local_pins = BTreeSet::new();
            for p in &e.client_certificate_pins {
                if p.generation == 0 || !local_pins.insert((&p.pin, p.generation)) {
                    return Err(Error::Invalid);
                }
                // A certificate cannot identify two enrollments, even after revocation.
                if !pins.insert((&p.pin, &e.enrollment_id)) {
                    continue;
                }
                if pins
                    .iter()
                    .any(|(pin, id)| *pin == &p.pin && *id != &e.enrollment_id)
                {
                    return Err(Error::Invalid);
                }
            }
        }
        let mut ids = BTreeSet::new();
        let mut hashes = BTreeSet::new();
        let mut generations = BTreeSet::new();
        for b in &self.credentials {
            let e = self
                .enrollments
                .iter()
                .find(|e| e.enrollment_id == b.enrollment_id)
                .ok_or(Error::Invalid)?;
            if !opaque(&b.credential_id)
                || b.generation == 0
                || b.valid_from >= b.valid_until
                || serde_json::to_vec(b).map_err(|_| Error::Invalid)?.len() >= 1024
                || !ids.insert(&b.credential_id)
                || !hashes.insert(b.sha256)
                || !generations.insert((&b.enrollment_id, b.kind, b.generation))
                || !e
                    .credential_kinds
                    .iter()
                    .any(|k| k.kind == b.kind && b.generation <= k.generations.current)
            {
                return Err(Error::Invalid);
            }
        }
        crate::pairing::validate_receipts(self)?;
        Ok(())
    }
    fn follows(&self, old: &Self) -> Result<()> {
        if self.latest < old.latest || self.next_sequence < old.next_sequence {
            return Err(Error::Terminal);
        }
        for tombstone in &old.tombstones {
            if !self.tombstones.contains(tombstone) {
                return Err(Error::Terminal);
            }
        }
        for receipt in &old.pairing_receipts {
            let next = self
                .pairing_receipts
                .iter()
                .find(|r| r.session_id == receipt.session_id)
                .ok_or(Error::Terminal)?;
            let mut expected = receipt.clone();
            expected.deliveries = next.deliveries;
            if &expected != next || next.deliveries < receipt.deliveries {
                return Err(Error::Terminal);
            }
        }
        self.validate()?;
        for e in &old.enrollments {
            let next = self
                .enrollments
                .iter()
                .find(|n| n.enrollment_id == e.enrollment_id)
                .ok_or(Error::Terminal)?;
            if e.receiver_id != next.receiver_id
                || e.machine != next.machine
                || e.binding != next.binding
                || e.origin_key != next.origin_key
                || e.endpoint != next.endpoint
                || e.enrolled_pin != next.enrolled_pin
                || e.sequence != next.sequence
                || (e.revoked_at.is_some() && e != next)
            {
                return Err(Error::Terminal);
            }
            for pin in &e.client_certificate_pins {
                let n = next
                    .client_certificate_pins
                    .iter()
                    .find(|n| n.pin == pin.pin && n.generation == pin.generation)
                    .ok_or(Error::Terminal)?;
                if pin.revoked_at.is_some() && pin.revoked_at != n.revoked_at {
                    return Err(Error::Terminal);
                }
            }
            for scope in &e.credential_kinds {
                let n = next
                    .credential_kinds
                    .iter()
                    .find(|n| n.kind == scope.kind)
                    .ok_or(Error::Terminal)?;
                if (!scope.available
                    && n.available
                    && !scope.enable_authorization.as_ref().is_some_and(|a| {
                        a.consumed_renewal_id.is_none()
                            && n.enable_authorization.as_ref().is_some_and(|b| {
                                b.authorization_id == a.authorization_id
                                    && b.consumed_renewal_id.is_some()
                            })
                    }))
                    || n.generations.current < scope.generations.current
                    || (scope.client_cert_required && !n.client_cert_required)
                    || (n.generations.current == scope.generations.current
                        && n.generations.retiring.is_some()
                        && n.generations.retiring != scope.generations.retiring)
                    || n.generations.retiring.as_ref().is_some_and(|r| {
                        r.generation != scope.generations.current
                            && scope.generations.retiring.as_ref() != Some(r)
                    })
                {
                    return Err(Error::Terminal);
                }
            }
        }
        for b in &old.credentials {
            let n = self
                .credentials
                .iter()
                .find(|n| n.credential_id == b.credential_id)
                .ok_or(Error::Terminal)?;
            let mut revoked = b.clone();
            revoked.revoked_at = n.revoked_at;
            if &revoked != n || (b.revoked_at.is_some() && b.revoked_at != n.revoked_at) {
                return Err(Error::Terminal);
            }
        }
        Ok(())
    }
}
fn opaque(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
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
    pairing_owner: bool,
    #[cfg(test)]
    pub(crate) fail_pairing_stage_creation: u8,
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
        // Gate and registry are initialized once under the same exclusive lock.
        // Publish the gate first: an interrupted initialization never leaves a
        // usable registry whose gate may be silently recreated.
        store.initialize_pairing_file(
            br#"{"format":1,"latest":0,"creations":[],"burns":[],"pause_until":0}"#,
        )?;
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
            pairing_owner: false,
            #[cfg(test)]
            fail_pairing_stage_creation: 0,
        })
    }
    pub fn open(receiver_dir: &Path) -> Result<Self> {
        let store = Self::acquire(receiver_dir)?;
        store.dir.load()?;
        Ok(store)
    }
    pub(crate) fn into_pairing_owner(mut self) -> Result<Self> {
        let file = self
            .dir
            .file("pairing.lock", libc::O_RDWR | libc::O_CREAT)?;
        file.try_lock().map_err(|_| Error::InUse)?;
        self._lock = LockGuard(file); // explicitly releases registry lock
        self.pairing_owner = true;
        Ok(self)
    }
    fn registry_transaction(&self) -> Result<Option<LockGuard>> {
        if !self.pairing_owner {
            return Ok(None);
        }
        let file = self.dir.file(LOCK_FILE, libc::O_RDWR)?;
        file.try_lock().map_err(|_| Error::InUse)?;
        Ok(Some(LockGuard(file)))
    }
    /// Pairing gate custody shares the pairing owner's exclusive lock and directory.
    pub(crate) fn read_pairing_file(&self) -> Result<Vec<u8>> {
        if self.failed {
            return Err(Error::CommitUncertain);
        }
        let mut bytes = vec![];
        self.dir
            .file(crate::pairing::GATE_FILE, libc::O_RDONLY)?
            .take(crate::pairing::GATE_CAP + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Custody)?;
        if bytes.len() as u64 > crate::pairing::GATE_CAP {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }
    fn initialize_pairing_file(&mut self, bytes: &[u8]) -> Result<()> {
        // Explicit, exclusive first initialization; never reinterpret a missing file as empty.
        let mut file = self.dir.file(
            crate::pairing::GATE_FILE,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        file.write_all(bytes).map_err(|_| Error::Write)?;
        file.sync_all().map_err(|_| Error::Write)?;
        self.dir.0.sync_all().map_err(|_| Error::CommitUncertain)
    }
    pub(crate) fn write_pairing_file(&mut self, bytes: &[u8]) -> Result<()> {
        self.read_pairing_file()?;
        #[cfg(test)]
        if self.fail_pairing_stage_creation > 0 {
            self.fail_pairing_stage_creation -= 1;
            if self.fail_pairing_stage_creation == 0 {
                return Err(Error::Write);
            }
        }
        let stage = format!("stage-{}", random_id()?);
        let mut file = self
            .dir
            .file(&stage, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        let result = (|| {
            file.write_all(bytes).map_err(|_| Error::Write)?;
            file.sync_all().map_err(|_| Error::Write)?;
            self.dir.rename(&stage, crate::pairing::GATE_FILE)
        })();
        if result.is_err() {
            self.dir.remove(&stage);
            self.failed = true;
            return result;
        }
        if self.dir.0.sync_all().is_err() {
            self.failed = true;
            return Err(Error::CommitUncertain);
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Registry> {
        let _transaction = self.registry_transaction()?;
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
        let _transaction = self.registry_transaction()?;
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
        if next.enrollments.len() > current.enrollments.len()
            || next.credentials.len() > current.credentials.len()
        {
            next.check_admission()?;
        }
        next.revision = current.revision.checked_add(1).ok_or(Error::Invalid)?;
        self.write(&next, |_| Ok(()))?;
        Ok(output)
    }
    /// Compare-and-swap a snapshot revision under the exclusive lock. Callers
    /// must supply the revision they read, including for newly built records.
    pub fn commit(&mut self, expected_revision: u64, next: &Registry) -> Result<()> {
        let _transaction = self.registry_transaction()?;
        if self.failed {
            return Err(Error::CommitUncertain);
        }
        let current = self.dir.load()?;
        if expected_revision != current.revision {
            return Err(Error::StaleSnapshot);
        }
        next.follows(&current)?;
        if next.enrollments.len() > current.enrollments.len()
            || next.credentials.len() > current.credentials.len()
        {
            next.check_admission()?;
        }
        let mut next = next.clone();
        next.revision = current.revision.checked_add(1).ok_or(Error::Invalid)?;
        self.write(&next, |_| Ok(()))
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
            || self.dir.0.sync_all().is_err()
            || fault(Stage::DirectorySynced).is_err()
        {
            self.failed = true;
            return Err(Error::CommitUncertain);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Written,
    Synced,
    Renamed,
    DirectorySynced,
}

#[path = "enrollment_state.rs"]
mod state;
pub use state::{OwnerAction, OwnerState};

#[cfg(test)]
#[path = "enrollment_state_tests.rs"]
mod state_tests;
#[cfg(test)]
#[path = "enrollment_tests.rs"]
mod tests;
