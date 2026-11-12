//! Unwired receiver provisioning. Identity is published last; incomplete first-run
//! custody is refused, never repaired by generating replacement secrets.
//! Back up enrollment/ACL independently; exclude cache, secrets and sensitive audit.

use crate::{body_assertion::MemoryKey, body_cache::CacheDirectory, enrollment, receiver_acl};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub const IDENTITY: &str = "receiver.json";
pub const SECRETS: &str = "secrets";
pub const CACHE: &str = "body-cache";
pub const AUDIT: &str = "receiver-audit";
pub const ACL: &str = "acl";
pub const CACHE_KEY: &str = "cache-master.key";
const CAP: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Custody,
    Invalid,
    Incomplete,
    InUse,
    Conflict,
    Random,
    Write,
    CommitUncertain,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Custody => "receiver custody missing or insecure",
            Self::Invalid => "receiver custody invalid or unsupported",
            Self::Incomplete => {
                "incomplete receiver initialization; preserve custody for explicit recovery"
            }
            Self::InUse => "receiver custody locked",
            Self::Conflict => "owner grants conflict",
            Self::Random => "system random source failed",
            Self::Write => "receiver custody write failed",
            Self::CommitUncertain => "receiver custody commit uncertain; reopen before continuing",
        })
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    format: u32,
    receiver_id: String,
    issuer_key_id: String,
}

/// Contains public identity and paths only; seeds are loaded into zeroizing memory.
pub struct ReceiverCustody {
    dir: PathBuf,
    identity: Identity,
    issuer: enrollment::IssuerKey,
    owner: Owner,
}
impl fmt::Debug for ReceiverCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReceiverCustody { private state omitted }")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub principal: String,
    pub audit_id: [u8; 32],
}
impl Owner {
    /// UID is authenticated locally, never supplied by a pairing participant.
    fn authenticated() -> Self {
        // SAFETY: geteuid has no memory preconditions.
        let uid = unsafe { libc::geteuid() };
        let audit_id: [u8; 32] =
            Sha256::digest([b"semon/receiver-owner/v1\0".as_slice(), &uid.to_be_bytes()].concat())
                .into();
        Self {
            principal: format!("owner-{}", hex::encode(audit_id)),
            audit_id,
        }
    }
}

/// Exact pending ACL entries. Private fields ensure installation revalidates scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerGrant {
    principal: String,
    machine: String,
    namespace: String,
    permissions: Vec<receiver_acl::Permission>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDocument {
    format: u32,
    grants: Vec<OwnerGrant>,
}
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct GrantLedger {
    format: u32,
    enrollments: BTreeMap<String, Vec<OwnerGrant>>,
    /// Redo record: a durable full target ACL, replayed before further mutations.
    pending: Option<PolicyDocument>,
}

/// Pure generation; the returned policy is pending until explicitly installed.
pub fn owner_grants(
    owner: &Owner,
    machine: &str,
    namespace: &str,
    body_read: bool,
) -> Result<Vec<OwnerGrant>> {
    if owner.principal != format!("owner-{}", hex::encode(owner.audit_id))
        || machine.contains('*')
        || namespace.contains('*')
    {
        return Err(Error::Invalid);
    }
    let mut permissions = vec![receiver_acl::Permission::Analytics];
    if body_read {
        permissions.push(receiver_acl::Permission::BodyRead);
    }
    let grants = vec![OwnerGrant {
        principal: owner.principal.clone(),
        machine: machine.into(),
        namespace: namespace.into(),
        permissions,
    }];
    validate_grants(&grants)?;
    Ok(grants)
}
/// The exact format-1 receiver_acl policy shape; no enrollment metadata leaks into it.
pub fn grants_document(grants: &[OwnerGrant]) -> Result<Vec<u8>> {
    validate_grants(grants)?;
    encode(&PolicyDocument {
        format: 1,
        grants: grants.to_vec(),
    })
}
fn validate_grants(grants: &[OwnerGrant]) -> Result<()> {
    if grants.is_empty()
        || grants.iter().any(|g| {
            !g.principal
                .strip_prefix("owner-")
                .is_some_and(|id| opaque(id, 64))
                || g.machine.contains('*')
                || g.namespace.contains('*')
                || !g.permissions.contains(&receiver_acl::Permission::Analytics)
        })
    {
        return Err(Error::Invalid);
    }
    let bytes = encode(&PolicyDocument {
        format: 1,
        grants: grants.to_vec(),
    })?;
    receiver_acl::Policy::parse(&bytes).ok_or(Error::Invalid)?;
    Ok(())
}

impl ReceiverCustody {
    pub fn open_or_initialize(dir: &Path) -> Result<Self> {
        Self::open_with_fault(dir, || Ok(()))
    }
    fn open_with_fault(dir: &Path, before_identity: impl FnOnce() -> Result<()>) -> Result<Self> {
        if !dir.try_exists().map_err(|_| Error::Custody)? {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|_| Error::Custody)?;
            File::open(dir.parent().ok_or(Error::Custody)?)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::Write)?;
        }
        let root = Directory::open(dir)?;
        let _lock = root.lock("custody.lock")?;
        let has_identity = root.exists(IDENTITY)?;
        if !has_identity {
            // A lock alone is safe to reuse after a crash before the first mutation.
            if root
                .cache
                .names()
                .map_err(|_| Error::Custody)?
                .iter()
                .any(|n| n != Path::new("custody.lock"))
            {
                return Err(Error::Incomplete);
            }
            let secrets = root.create_child(SECRETS)?;
            root.create_child(CACHE)?;
            root.create_child(AUDIT)?;
            let acl = root.create_child(ACL)?;
            // Registry owns its own directory, lock and durable format.
            drop(enrollment::Store::initialize(dir).map_err(|_| Error::Custody)?);
            // MemoryKey takes ownership; the additional seed copy is zeroized too.
            let seed = random_secret()?;
            let key = MemoryKey::from_seed(Zeroizing::new(*seed));
            let public_key = key.public_key().map_err(|_| Error::Invalid)?;
            let key_id = issuer_key_id(&public_key);
            secrets.create(&issuer_filename(&key_id), seed.as_ref())?;
            secrets.create(CACHE_KEY, random_secret()?.as_ref())?;
            acl.create(
                "owners.json",
                &encode(&GrantLedger {
                    format: 1,
                    ..Default::default()
                })?,
            )?;
            acl.create(
                receiver_acl::ACL_FILE,
                &encode(&PolicyDocument {
                    format: 1,
                    grants: vec![],
                })?,
            )?;
            root.sync()?;
            before_identity()?;
            root.publish(
                IDENTITY,
                &encode(&Identity {
                    format: 1,
                    receiver_id: enrollment::random_id().map_err(|_| Error::Random)?,
                    issuer_key_id: key_id,
                })?,
                false,
            )?;
        }
        Self::load(dir, &root)
    }
    fn load(dir: &Path, root: &Directory) -> Result<Self> {
        let identity: Identity = decode(&root.read(IDENTITY)?)?;
        if identity.format != 1
            || !opaque(&identity.receiver_id, 64)
            || !opaque(&identity.issuer_key_id, 32)
        {
            return Err(Error::Invalid);
        }
        let secrets = root.child(SECRETS)?;
        let issuer_name = issuer_filename(&identity.issuer_key_id);
        if secrets
            .cache
            .names()
            .map_err(|_| Error::Custody)?
            .iter()
            .any(|name| name != Path::new(CACHE_KEY) && name != Path::new(&issuer_name))
        {
            return Err(Error::Invalid);
        }
        let seed = read_secret(&secrets, &issuer_filename(&identity.issuer_key_id))?;
        let public_key = MemoryKey::from_seed(seed)
            .public_key()
            .map_err(|_| Error::Invalid)?;
        if issuer_key_id(&public_key) != identity.issuer_key_id {
            return Err(Error::Invalid);
        }
        read_secret(&secrets, CACHE_KEY)?;
        root.child(CACHE)?;
        root.child(AUDIT)?;
        let acl = root.child(ACL)?;
        let _lock = acl.lock("owners.lock")?;
        let mut ledger = load_ledger(&acl)?;
        recover(&acl, &mut ledger)?;
        enrollment::Registry::read_current(dir).map_err(|_| Error::Custody)?;
        Ok(Self {
            dir: dir.into(),
            issuer: enrollment::IssuerKey {
                key_id: identity.issuer_key_id.clone(),
                public_key,
            },
            identity,
            owner: Owner::authenticated(),
        })
    }
    pub fn receiver_id(&self) -> &str {
        &self.identity.receiver_id
    }
    pub fn issuer(&self) -> &enrollment::IssuerKey {
        &self.issuer
    }
    pub fn owner(&self) -> &Owner {
        &self.owner
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join(CACHE)
    }
    pub fn cache_key_path(&self) -> PathBuf {
        self.dir.join(SECRETS).join(CACHE_KEY)
    }
    pub fn audit_dir(&self) -> PathBuf {
        self.dir.join(AUDIT)
    }
    pub fn acl_dir(&self) -> PathBuf {
        self.dir.join(ACL)
    }
    pub fn load_signing_key(&self) -> Result<MemoryKey> {
        let root = Directory::open(&self.dir)?;
        let seed = read_secret(&root.child(SECRETS)?, &issuer_filename(&self.issuer.key_id))?;
        let key = MemoryKey::from_seed(seed);
        if key.public_key().map_err(|_| Error::Invalid)? != self.issuer.public_key {
            return Err(Error::Invalid);
        }
        Ok(key)
    }
    pub fn install_owner_grants(&self, enrollment_id: &str, grants: &[OwnerGrant]) -> Result<()> {
        validate_grants(grants)?;
        if grants
            .iter()
            .any(|grant| grant.principal != self.owner.principal)
        {
            return Err(Error::Invalid);
        }
        self.update_grants(enrollment_id, Some(grants))
    }
    pub fn retire_owner_grants(&self, enrollment_id: &str) -> Result<()> {
        self.update_grants(enrollment_id, None)
    }
    fn update_grants(&self, enrollment_id: &str, grants: Option<&[OwnerGrant]>) -> Result<()> {
        if !opaque(enrollment_id, 64) {
            return Err(Error::Invalid);
        }
        let root = Directory::open(&self.dir)?;
        let acl = root.child(ACL)?;
        let _lock = acl.lock("owners.lock")?;
        let mut ledger = load_ledger(&acl)?;
        recover(&acl, &mut ledger)?;
        let mut policy: PolicyDocument = decode(&acl.read(receiver_acl::ACL_FILE)?)?;
        if let Some(existing) = ledger.enrollments.get(enrollment_id) {
            if let Some(grants) = grants {
                return if existing == grants {
                    Ok(())
                } else {
                    Err(Error::Conflict)
                };
            }
            policy.grants.retain(|g| !existing.contains(g));
            ledger.enrollments.remove(enrollment_id);
        } else if let Some(grants) = grants {
            if grants
                .iter()
                .any(|g| policy.grants.iter().any(|e| scope(e) == scope(g)))
            {
                return Err(Error::Conflict);
            }
            policy.grants.extend_from_slice(grants);
            ledger
                .enrollments
                .insert(enrollment_id.into(), grants.to_vec());
        } else {
            return Ok(());
        }
        ledger.pending = Some(policy);
        acl.publish("owners.json", &encode(&ledger)?, true)?;
        recover(&acl, &mut ledger)
    }
    /// Read-only inspection; missing and invalid are distinct, and nothing is repaired.
    pub fn status(dir: &Path) -> Status {
        let inspect = |name: &str, directory: bool| {
            let path = dir.join(name);
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Piece::Missing,
                Err(_) => Piece::Invalid,
                Ok(_) => {
                    let valid = if directory {
                        Directory::open(dir)
                            .and_then(|root| {
                                let child = root.child(name)?;
                                match name {
                                    SECRETS => {
                                        let identity: Identity = decode(&root.read(IDENTITY)?)?;
                                        if !opaque(&identity.issuer_key_id, 32) {
                                            return Err(Error::Invalid);
                                        }
                                        let key = MemoryKey::from_seed(read_secret(
                                            &child,
                                            &issuer_filename(&identity.issuer_key_id),
                                        )?);
                                        if issuer_key_id(
                                            &key.public_key().map_err(|_| Error::Invalid)?,
                                        ) != identity.issuer_key_id
                                        {
                                            return Err(Error::Invalid);
                                        }
                                        read_secret(&child, CACHE_KEY)?;
                                    }
                                    ACL => {
                                        load_ledger(&child)?;
                                    }
                                    _ => {}
                                }
                                Ok(())
                            })
                            .is_ok()
                    } else {
                        Directory::open(dir)
                            .and_then(|d| d.read(name))
                            .and_then(|b| {
                                let i: Identity = decode(&b)?;
                                if i.format == 1
                                    && opaque(&i.receiver_id, 64)
                                    && opaque(&i.issuer_key_id, 32)
                                {
                                    Ok(())
                                } else {
                                    Err(Error::Invalid)
                                }
                            })
                            .is_ok()
                    };
                    if valid { Piece::Valid } else { Piece::Invalid }
                }
            }
        };
        Status {
            identity: inspect(IDENTITY, false),
            secrets: inspect(SECRETS, true),
            cache: inspect(CACHE, true),
            audit: inspect(AUDIT, true),
            acl: inspect(ACL, true),
        }
    }
}
fn scope(g: &OwnerGrant) -> (&str, &str, &str) {
    (&g.principal, &g.machine, &g.namespace)
}
fn load_ledger(acl: &Directory) -> Result<GrantLedger> {
    let ledger: GrantLedger = decode(&acl.read("owners.json")?)?;
    if ledger.format != 1 {
        return Err(Error::Invalid);
    }
    let policy = acl.read(receiver_acl::ACL_FILE)?;
    receiver_acl::Policy::parse(&policy).ok_or(Error::Invalid)?;
    let current: PolicyDocument = decode(&policy)?;
    let mut scopes = BTreeSet::new();
    for (id, grants) in &ledger.enrollments {
        if !opaque(id, 64) {
            return Err(Error::Invalid);
        }
        validate_grants(grants)?;
        for g in grants {
            if g.principal != Owner::authenticated().principal {
                return Err(Error::Invalid);
            }
            if !scopes.insert(scope(g)) {
                return Err(Error::Conflict);
            }
            let target = ledger.pending.as_ref().unwrap_or(&current);
            if !target.grants.contains(g) {
                return Err(Error::Conflict);
            }
        }
    }
    if let Some(target) = &ledger.pending {
        receiver_acl::Policy::parse(&encode(target)?).ok_or(Error::Invalid)?;
    }
    Ok(ledger)
}
fn recover(acl: &Directory, ledger: &mut GrantLedger) -> Result<()> {
    if let Some(target) = &ledger.pending {
        acl.publish(receiver_acl::ACL_FILE, &encode(target)?, true)?;
        ledger.pending = None;
        acl.publish("owners.json", &encode(ledger)?, true)?;
    }
    Ok(())
}
pub fn issuer_key_id(public_key: &[u8; 32]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"semon/receiver-assertion-issuer/v1\0");
    hash.update(public_key);
    hex::encode(&hash.finalize()[..16])
}
fn issuer_filename(id: &str) -> String {
    format!("issuer-{id}.seed")
}
fn opaque(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn random_secret() -> Result<Zeroizing<[u8; 32]>> {
    let mut seed = Zeroizing::new([0; 32]);
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(seed.as_mut())
        .map_err(|_| Error::Random)?;
    Ok(seed)
}
fn read_secret(dir: &Directory, name: &str) -> Result<Zeroizing<[u8; 32]>> {
    let mut file = dir
        .cache
        .open_file(Path::new(name), libc::O_RDONLY)
        .map_err(|_| Error::Custody)?;
    if file.metadata().map_err(|_| Error::Custody)?.len() != 32 {
        return Err(Error::Invalid);
    }
    let mut seed = Zeroizing::new([0; 32]);
    file.read_exact(seed.as_mut()).map_err(|_| Error::Custody)?;
    Ok(seed)
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid)?;
    if bytes.len() as u64 > CAP {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid)
}

// Reuse CacheDirectory's fd-checked O_NOFOLLOW regular-file opens. Directory
// creation/publication additionally needs mkdirat/renameat, private in that module.
struct Directory {
    fd: File,
    cache: CacheDirectory,
    path: PathBuf,
}
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        let fd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| Error::Custody)?;
        let meta = fd.metadata().map_err(|_| Error::Custody)?;
        // SAFETY: geteuid has no memory preconditions.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o7777 != 0o700 {
            return Err(Error::Custody);
        }
        let cache = CacheDirectory::open(path).map_err(|_| Error::Custody)?;
        let current = fs::symlink_metadata(path).map_err(|_| Error::Custody)?;
        if current.dev() != meta.dev() || current.ino() != meta.ino() {
            return Err(Error::Custody);
        }
        Ok(Self {
            fd,
            cache,
            path: path.into(),
        })
    }
    fn child(&self, name: &str) -> Result<Self> {
        let name_c = CString::new(name).map_err(|_| Error::Invalid)?;
        // SAFETY: live descriptor, valid single-component name.
        let fd = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(Error::Custody);
        }
        // SAFETY: fresh owned descriptor from openat.
        let fd = unsafe { File::from_raw_fd(fd) };
        let path = self.path.join(name);
        let result = Self::open(&path)?;
        let a = fd.metadata().map_err(|_| Error::Custody)?;
        let b = result.fd.metadata().map_err(|_| Error::Custody)?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(Error::Custody);
        }
        Ok(result)
    }
    fn create_child(&self, name: &str) -> Result<Self> {
        let c = CString::new(name).map_err(|_| Error::Invalid)?;
        // SAFETY: live parent fd and valid name.
        if unsafe { libc::mkdirat(self.fd.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
            return Err(Error::Custody);
        }
        self.sync()?;
        self.child(name)
    }
    fn exists(&self, name: &str) -> Result<bool> {
        let c = CString::new(name).map_err(|_| Error::Invalid)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: valid fd/name and writable stat buffer; no symlink traversal.
        if unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                c.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            return Ok(true);
        }
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(Error::Custody)
        }
    }
    fn read(&self, name: &str) -> Result<Vec<u8>> {
        let file = self
            .cache
            .open_file(Path::new(name), libc::O_RDONLY)
            .map_err(|_| Error::Custody)?;
        let mut bytes = vec![];
        file.take(CAP + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Custody)?;
        if bytes.len() as u64 > CAP {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }
    fn create(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let mut file = self
            .cache
            .open_file(
                Path::new(name),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            )
            .map_err(|_| Error::Custody)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::Write)?;
        self.sync()
    }
    fn sync(&self) -> Result<()> {
        self.cache.sync().map_err(|_| Error::CommitUncertain)
    }
    fn lock(&self, name: &str) -> Result<LockGuard> {
        let file = self
            .cache
            .open_file(Path::new(name), libc::O_RDWR | libc::O_CREAT)
            .map_err(|_| Error::Custody)?;
        file.try_lock().map_err(|_| Error::InUse)?;
        self.sync()?;
        Ok(LockGuard(file))
    }
    fn publish(&self, name: &str, bytes: &[u8], replace: bool) -> Result<()> {
        if !replace && self.exists(name)? {
            return Err(Error::Conflict);
        }
        if replace {
            self.cache
                .open_file(Path::new(name), libc::O_RDONLY)
                .map_err(|_| Error::Custody)?;
        }
        let stage = format!(
            "stage-{}",
            enrollment::random_id().map_err(|_| Error::Random)?
        );
        self.create(&stage, bytes)?;
        let from = CString::new(stage).map_err(|_| Error::Invalid)?;
        let to = CString::new(name).map_err(|_| Error::Invalid)?;
        // SAFETY: live fd, single-component valid C strings. Writers hold a lock.
        if unsafe {
            libc::renameat(
                self.fd.as_raw_fd(),
                from.as_ptr(),
                self.fd.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(Error::Write);
        }
        self.sync()
    }
}
struct LockGuard(File);
impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Missing,
    Valid,
    Invalid,
}
#[derive(Debug)]
pub struct Status {
    pub identity: Piece,
    pub secrets: Piece,
    pub cache: Piece,
    pub audit: Piece,
    pub acl: Piece,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Satisfied,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostCustody {
    pub core_dumps: Observation,
    pub swap: Observation,
    pub backup_exclusions: Observation,
}
/// Pure injectable observation. No swap encryption or backup attestation is inferred.
pub fn host_custody(dumpable: Option<i32>) -> HostCustody {
    HostCustody {
        core_dumps: match dumpable {
            Some(0) => Observation::Satisfied,
            Some(_) => Observation::Failed,
            None => Observation::Unknown,
        },
        swap: Observation::Unknown,
        backup_exclusions: Observation::Unknown,
    }
}
/// Only the future body-serving caller opts into process-wide hardening.
pub fn harden_process() -> Result<HostCustody> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: scalar prctl operations have no pointer arguments.
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err(Error::Custody);
        }
        // SAFETY: PR_GET_DUMPABLE takes no pointer arguments.
        let dumpable = unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) };
        Ok(host_custody((dumpable >= 0).then_some(dumpable)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(host_custody(None))
    }
}

#[cfg(test)]
#[path = "receiver_custody_tests.rs"]
mod tests;
