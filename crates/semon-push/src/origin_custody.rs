//! Private origin enrollment custody. The final directory rename is the only
//! publication point; incomplete staging directories are never read as enrollment.
use crate::{body_cache::CacheDirectory, pairing::ActivationInput, provision_wire::Activated};
use ring::signature::{EcdsaKeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, String>;
const FILE_CAP: usize = 4 * 1024 * 1024;
const ERROR: &str = "invalid or unreadable origin enrollment custody";
#[cfg(test)]
#[path = "origin_custody_tests.rs"]
mod tests;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub version: u32,
    pub endpoint: String,
    pub receiver_id: String,
    pub enrollment_id: String,
    pub machine: String,
    pub server_roots: String,
    pub scope: ActivationInput,
    pub credential_file: String,
    pub private_key_file: String,
    pub consent_file: String,
    pub consent_sha256: String,
    pub credential_sha256: String,
    pub client_certificate_sha256: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Signed {
    configuration: Configuration,
    public_key: String,
    signature: String,
}

pub fn default_directory() -> Result<PathBuf> {
    configuration_directory(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}
fn configuration_directory(
    xdg: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf> {
    let base = xdg
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".config"))
        })
        .ok_or("origin custody requires absolute XDG_CONFIG_HOME or HOME")?;
    Ok(base.join("semon/origin"))
}
struct InstallLock(fs::File);
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
#[cfg(test)]
thread_local! { static LOCK_DUPLICATE: std::cell::RefCell<Option<fs::File>> = const { std::cell::RefCell::new(None) }; }
struct Directory {
    fd: fs::File,
    checked: CacheDirectory,
}
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        let fd = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| ERROR)?;
        Self::from_fd(fd)
    }
    fn from_fd(fd: fs::File) -> Result<Self> {
        let checked =
            CacheDirectory::from_file(fd.try_clone().map_err(|_| ERROR)?).map_err(|_| ERROR)?;
        Ok(Self { fd, checked })
    }
    fn child(&self, name: &str) -> Result<Self> {
        let name = std::ffi::CString::new(name).map_err(|_| ERROR)?;
        // SAFETY: live directory fd and a validated single-component name.
        let fd = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(ERROR.into());
        }
        // SAFETY: openat returned a fresh owned descriptor.
        Self::from_fd(unsafe { fs::File::from_raw_fd(fd) })
    }
    fn create_child(&self, name: &str) -> Result<Self> {
        let name_c = std::ffi::CString::new(name).map_err(|_| ERROR)?;
        // SAFETY: live parent fd and single-component name.
        if unsafe { libc::mkdirat(self.fd.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
            return Err(ERROR.into());
        }
        self.child(name)
    }
    fn install_lock(&self) -> Result<InstallLock> {
        let lock = self
            .checked
            .open_file(Path::new("install.lock"), libc::O_RDWR | libc::O_CREAT)
            .map_err(|_| ERROR)?;
        lock.try_lock().map_err(|_| "origin custody is in use")?;
        Ok(InstallLock(lock))
    }
    fn disabled(&self) -> Result<bool> {
        let Some(file) = self
            .checked
            .open_optional_file(Path::new("disabled"), libc::O_RDONLY)
            .map_err(|_| ERROR)?
        else {
            return Ok(false);
        };
        let mut bytes = Vec::new();
        file.take(10).read_to_end(&mut bytes).map_err(|_| ERROR)?;
        if bytes != b"disabled\n" {
            return Err(ERROR.into());
        }
        Ok(true)
    }
    fn read(&self, name: &str) -> Result<Zeroizing<Vec<u8>>> {
        let file = self
            .checked
            .open_file(Path::new(name), libc::O_RDONLY)
            .map_err(|_| ERROR)?;
        let mut bytes = Zeroizing::new(Vec::new());
        file.take((FILE_CAP + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ERROR)?;
        if bytes.len() > FILE_CAP {
            return Err(ERROR.into());
        }
        Ok(bytes)
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        validate_size(name, bytes)?;
        let mut file = self
            .checked
            .open_file(
                Path::new(name),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            )
            .map_err(|_| ERROR)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| ERROR.to_owned())
    }
}

/// Install delivery from an approved, mutually confirmed provisioning exchange.
/// Embedders own that exchange and must verify receiver confirmation and send
/// approval before calling this custody primitive. Normal callers use connect.
pub fn install(
    base: &Path,
    endpoint: &str,
    machine: &str,
    scope: &ActivationInput,
    delivery: &Activated,
    private: &[u8],
    sources: &[PathBuf],
) -> Result<String> {
    install_at(
        base,
        endpoint,
        machine,
        scope,
        delivery,
        private,
        sources,
        now()?,
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_at(
    base: &Path,
    endpoint: &str,
    machine: &str,
    scope: &ActivationInput,
    delivery: &Activated,
    private: &[u8],
    sources: &[PathBuf],
    now: u64,
) -> Result<String> {
    if delivery.credentials.len() != 1
        || delivery.credentials[0].0 != crate::credentials::Kind::MirrorPush
    {
        return Err("invalid mirror credential delivery".into());
    }
    crate::Token::new(&delivery.credentials[0].1)?;
    let key = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        private,
        &ring::rand::SystemRandom::new(),
    )
    .map_err(|_| ERROR)?;
    let certificate = delivery
        .client_certificate
        .as_deref()
        .map(decode_certificate)
        .transpose()?;
    validate_certificate(
        scope,
        certificate.as_deref(),
        key.public_key().as_ref(),
        now,
    )?;
    // Parents contain no credentials. Dedicated custody itself must be owned 0700.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)
        .map_err(|_| ERROR)?;
    let root = Directory::open(base)?;
    let _lock = root.install_lock()?;
    #[cfg(test)]
    LOCK_DUPLICATE.with(|slot| *slot.borrow_mut() = Some(_lock.0.try_clone().unwrap()));
    let name =
        hex::encode(ring::digest::digest(&ring::digest::SHA256, endpoint.as_bytes()).as_ref());
    if root
        .checked
        .names()
        .map_err(|_| ERROR)?
        .iter()
        .any(|p| p == Path::new(&name))
    {
        return Err("receiver already enrolled; explicit revocation/recovery required".into());
    }
    let stage = format!(
        "stage-{}",
        crate::enrollment::random_id().map_err(|_| ERROR)?
    );
    let dir = root.create_child(&stage)?;
    let scope: ActivationInput =
        serde_json::from_slice(&serde_json::to_vec(scope).map_err(|_| ERROR)?)
            .map_err(|_| ERROR)?;
    let consent = Consent {
        receiver_id: scope.receiver_id.clone(),
        enrollment_id: delivery.enrollment_id.clone(),
        machine: machine.into(),
        sources: sources.to_vec(),
    };
    let consent_bytes = serde_json::to_vec(&consent).map_err(|_| ERROR)?;
    let configuration = Configuration {
        version: 1,
        endpoint: endpoint.into(),
        receiver_id: scope.receiver_id.clone(),
        enrollment_id: delivery.enrollment_id.clone(),
        machine: machine.into(),
        server_roots: "public_webpki".into(),
        scope,
        credential_file: "mirror-push.secret".into(),
        private_key_file: "origin-key.pk8".into(),
        consent_file: "source-consent.json".into(),
        consent_sha256: crate::wire::sha256_hex(&consent_bytes),
        credential_sha256: crate::wire::sha256_hex(delivery.credentials[0].1.as_bytes()),
        client_certificate_sha256: certificate.as_deref().map(crate::wire::sha256_hex),
    };
    let bytes = serde_json::to_vec(&configuration).map_err(|_| ERROR)?;
    let signature = key
        .sign(&ring::rand::SystemRandom::new(), &bytes)
        .map_err(|_| ERROR)?;
    let signed = Signed {
        configuration,
        public_key: crate::provision_wire::encode(key.public_key().as_ref()),
        signature: crate::provision_wire::encode(signature.as_ref()),
    };
    let signed_bytes = serde_json::to_vec(&signed).map_err(|_| ERROR)?;
    for (name, bytes) in [
        ("source-consent.json", consent_bytes.as_slice()),
        ("origin-key.pk8", private),
        ("mirror-push.secret", delivery.credentials[0].1.as_bytes()),
        ("enrollment.json", signed_bytes.as_slice()),
    ] {
        validate_size(name, bytes)?;
    }
    dir.write("source-consent.json", &consent_bytes)?;
    dir.write("origin-key.pk8", private)?;
    dir.write("mirror-push.secret", delivery.credentials[0].1.as_bytes())?;
    if let Some(certificate) = &certificate {
        dir.write("client-certificate.der", certificate)?;
    }
    dir.write("enrollment.json", &signed_bytes)?;
    dir.checked.sync().map_err(|_| ERROR)?;
    let from = std::ffi::CString::new(stage).map_err(|_| ERROR)?;
    let to = std::ffi::CString::new(name.clone()).map_err(|_| ERROR)?;
    // SAFETY: both names are single components under the locked private fd.
    if unsafe {
        libc::renameat(
            root.fd.as_raw_fd(),
            from.as_ptr(),
            root.fd.as_raw_fd(),
            to.as_ptr(),
        )
    } != 0
    {
        return Err(ERROR.into());
    }
    root.checked
        .sync()
        .map_err(|_| "origin enrollment commit durability uncertain")?;
    Ok(name)
}

/// Resolve an operator URL through the pairing endpoint contract. Custody keys
/// remain internal, including when exactly one enrolled receiver is selected.
pub fn receiver_endpoint(receiver: &str) -> Result<String> {
    crate::pairing::normalize_endpoint(receiver)
        .map_err(|_| "receiver requires an HTTPS URL without path, credentials or query".into())
}

pub fn receiver_host(endpoint: &str) -> Result<String> {
    Ok(receiver_endpoint(endpoint)?["https://".len()..].to_owned())
}

fn custody_name(endpoint: &str) -> String {
    crate::wire::sha256_hex(endpoint.as_bytes())
}

/// An omitted URL requires exactly one enrollment. Staging is never selected.
pub fn load(base: &Path, receiver: Option<&str>) -> Result<(Configuration, crate::Credential)> {
    load_at(base, receiver, now()?)
}
pub(crate) fn load_at(
    base: &Path,
    receiver: Option<&str>,
    now: u64,
) -> Result<(Configuration, crate::Credential)> {
    let selected = receiver.map(receiver_endpoint).transpose()?;
    let root =
        Directory::open(base).map_err(|_| "no readable enrollment; run semon connect --to URL")?;
    let name = match selected {
        Some(endpoint) => custody_name(&endpoint),
        None => {
            let names: Vec<_> = root
                .checked
                .names()
                .map_err(|_| ERROR)?
                .into_iter()
                .filter_map(|p| p.to_str().map(str::to_owned))
                .filter(|n| {
                    n.len() == 64
                        && n.bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
                .collect();
            if names.len() != 1 {
                return Err("push requires exactly one enrollment or --receiver URL; run semon connect --to URL".into());
            }
            names[0].clone()
        }
    };
    let dir = root.child(&name)?;
    if dir.disabled()? {
        return Err("receiver is disconnected on this machine; run semon connect --to URL".into());
    }
    let (config, key) = read_configuration(&dir, &name)?;
    let private = dir.read("origin-key.pk8")?;
    let secret = dir.read("mirror-push.secret")?;
    if crate::wire::sha256_hex(&secret) != config.credential_sha256 {
        return Err(ERROR.into());
    }
    let token = crate::Token::new(std::str::from_utf8(&secret).map_err(|_| ERROR)?)?;
    let certificate = config
        .client_certificate_sha256
        .as_ref()
        .map(|hash| {
            let bytes = dir.read("client-certificate.der")?;
            if crate::wire::sha256_hex(&bytes) != *hash {
                return Err(ERROR.to_owned());
            }
            Ok(bytes.to_vec())
        })
        .transpose()?;
    validate_certificate(
        &config.scope,
        certificate.as_deref(),
        key.public_key().as_ref(),
        now,
    )?;
    let credential = crate::Credential::Enrolled {
        token,
        identity: certificate.map(|certificate| crate::ClientIdentity {
            certificate,
            private: Zeroizing::new(private.to_vec()),
        }),
        sources: consent(base, &name, &config)?,
        authority: Authority::new(base, &config)?,
    };
    Ok((config, credential))
}

fn read_configuration(dir: &Directory, name: &str) -> Result<(Configuration, EcdsaKeyPair)> {
    let signed: Signed =
        serde_json::from_slice(&dir.read("enrollment.json")?).map_err(|_| ERROR)?;
    let config = signed.configuration;
    if config.version != 1
        || config.server_roots != "public_webpki"
        || config.credential_file != "mirror-push.secret"
        || config.private_key_file != "origin-key.pk8"
        || config.consent_file != "source-consent.json"
        || config.receiver_id != config.scope.receiver_id
        || config.machine != config.scope.machine
        || config.scope.kinds.len() != 1
        || config.scope.kinds[0].0 != crate::credentials::Kind::MirrorPush
        || !matches!(
            config.scope.binding,
            crate::enrollment::Binding::Inapplicable
        )
        || crate::check_url(&config.endpoint).map_err(|_| ERROR)? != config.endpoint
        || hex::encode(
            ring::digest::digest(&ring::digest::SHA256, config.endpoint.as_bytes()).as_ref(),
        ) != name
    {
        return Err(ERROR.into());
    }
    let private = dir.read("origin-key.pk8")?;
    let key = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        &private,
        &ring::rand::SystemRandom::new(),
    )
    .map_err(|_| ERROR)?;
    if crate::provision_wire::encode(key.public_key().as_ref()) != signed.public_key {
        return Err(ERROR.into());
    }
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_FIXED,
        key.public_key().as_ref(),
    )
    .verify(
        &serde_json::to_vec(&config).map_err(|_| ERROR)?,
        &crate::provision_wire::decode_fixed::<64>(&signed.signature).map_err(|_| ERROR)?,
    )
    .map_err(|_| ERROR)?;
    Ok((config, key))
}

/// Nonsecret local authority for a selected channel. Missing or insecure custody
/// stops existing delivery through the same Stop hook used by embedded callers.
#[derive(Clone, Debug)]
pub struct Authority {
    base: PathBuf,
    name: String,
}
impl Authority {
    pub fn new(base: &Path, config: &Configuration) -> Result<Self> {
        let endpoint = &config.endpoint;
        let authority = Self {
            base: base.into(),
            name: custody_name(endpoint),
        };
        if !authority.is_active() {
            return Err("receiver is disconnected or its custody is unavailable".into());
        }
        Ok(authority)
    }
    /// Keep a shared delivery lock until the HTTP operation has ended. Disconnect
    /// publishes disabled first, then drains these locks before reporting success.
    pub(crate) fn delivery(&self) -> Result<Delivery> {
        let dir = Directory::open(&self.base)?.child(&self.name)?;
        let guard = delivery_lock(&dir, libc::LOCK_SH)?;
        if !self.is_active() {
            return Err("receiver is disconnected or its custody is unavailable".into());
        }
        Ok(guard)
    }
    pub(crate) fn is_active(&self) -> bool {
        Directory::open(&self.base)
            .and_then(|root| root.child(&self.name))
            .and_then(|dir| dir.disabled())
            .is_ok_and(|disabled| !disabled)
    }
}

pub(crate) struct Delivery(fs::File);
impl Drop for Delivery {
    fn drop(&mut self) {
        // Explicit unlock also covers descriptors inherited by unrelated forks.
        let _ = self.0.unlock();
    }
}
fn delivery_lock(dir: &Directory, operation: libc::c_int) -> Result<Delivery> {
    delivery_lock_until(
        dir,
        operation,
        std::time::Instant::now() + std::time::Duration::from_secs(5),
    )
}
fn delivery_lock_until(
    dir: &Directory,
    operation: libc::c_int,
    deadline: std::time::Instant,
) -> Result<Delivery> {
    let file = dir
        .checked
        .open_file(Path::new("delivery.lock"), libc::O_RDWR | libc::O_CREAT)
        .map_err(|_| ERROR)?;
    loop {
        // SAFETY: this owned descriptor remains live until the guard drops.
        if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock || std::time::Instant::now() >= deadline {
            return Err(ERROR.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(Delivery(file))
}

pub const DISCONNECT_OWNER: &str =
    "disconnect requires the local owner's terminal; run this command there.";
pub const DISCONNECT_UNCERTAIN: &str = "disconnect could not be confirmed; rerun the same command to check its earlier outcome before retrying.";

/// Holds install.lock from the fresh state read through confirmation and commit.
/// No network operation or receiver revocation is involved.
pub struct Disconnect {
    _lock: InstallLock,
    dir: Directory,
    disabled: bool,
}
impl Disconnect {
    pub fn open(base: &Path, receiver: &str) -> Result<Self> {
        let endpoint = receiver_endpoint(receiver)?;
        let host = receiver_host(&endpoint)?;
        let unknown = || {
            format!(
                "receiver {host} was not found; use the HTTPS URL given to connect as --receiver URL, then retry."
            )
        };
        let metadata = match fs::symlink_metadata(base) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(unknown()),
            Err(_) => return Err(DISCONNECT_UNCERTAIN.into()),
        };
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(DISCONNECT_OWNER.into());
        }
        let root = Directory::open(base).map_err(|_| DISCONNECT_UNCERTAIN)?;
        let lock = root.install_lock().map_err(|_| DISCONNECT_UNCERTAIN)?;
        let name = custody_name(&endpoint);
        if !root
            .checked
            .names()
            .map_err(|_| DISCONNECT_UNCERTAIN)?
            .iter()
            .any(|p| p == Path::new(&name))
        {
            return Err(unknown());
        }
        let dir = root.child(&name).map_err(|_| DISCONNECT_UNCERTAIN)?;
        read_configuration(&dir, &name).map_err(|_| DISCONNECT_UNCERTAIN)?;
        let disabled = dir.disabled().map_err(|_| DISCONNECT_UNCERTAIN)?;
        Ok(Self {
            _lock: lock,
            dir,
            disabled,
        })
    }
    pub fn is_disconnected(&self) -> bool {
        self.disabled
    }
    pub fn commit(self) -> Result<()> {
        self.commit_until(std::time::Instant::now() + std::time::Duration::from_secs(5))
    }
    fn commit_until(mut self, deadline: std::time::Instant) -> Result<()> {
        if self.disabled {
            // Reconcile a previous uncertain directory fsync before success.
            self.dir.checked.sync().map_err(|_| DISCONNECT_UNCERTAIN)?;
            let _drained = delivery_lock_until(&self.dir, libc::LOCK_EX, deadline)
                .map_err(|_| DISCONNECT_UNCERTAIN)?;
            return Ok(());
        }
        let stage = format!(
            "disabled-stage-{}",
            crate::enrollment::random_id().map_err(|_| DISCONNECT_UNCERTAIN)?
        );
        self.dir
            .write(&stage, b"disabled\n")
            .map_err(|_| DISCONNECT_UNCERTAIN)?;
        let from = std::ffi::CString::new(stage).map_err(|_| DISCONNECT_UNCERTAIN)?;
        let to = c"disabled";
        // SAFETY: both single-component names are relative to this owned directory.
        if unsafe {
            libc::renameat(
                self.dir.fd.as_raw_fd(),
                from.as_ptr(),
                self.dir.fd.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(DISCONNECT_UNCERTAIN.into());
        }
        self.disabled = true;
        self.dir.checked.sync().map_err(|_| DISCONNECT_UNCERTAIN)?;
        let _drained = delivery_lock_until(&self.dir, libc::LOCK_EX, deadline)
            .map_err(|_| DISCONNECT_UNCERTAIN)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub receiver_id: String,
    pub enrollment_id: String,
    pub machine: String,
    pub sources: Vec<PathBuf>,
}
pub fn consent(base: &Path, receiver: &str, config: &Configuration) -> Result<Vec<PathBuf>> {
    if receiver.len() != 64 || !receiver.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ERROR.into());
    }
    let dir = Directory::open(base)?.child(receiver)?;
    let bytes = dir.read("source-consent.json")?;
    if crate::wire::sha256_hex(&bytes) != config.consent_sha256 {
        return Err(ERROR.into());
    }
    let consent: Consent = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    if consent.receiver_id != config.receiver_id
        || consent.enrollment_id != config.enrollment_id
        || consent.machine != config.machine
    {
        return Err(ERROR.into());
    }
    Ok(consent.sources)
}

fn decode_certificate(encoded: &str) -> Result<Vec<u8>> {
    // Reuse the canonical base64url decoder while enforcing a narrow leaf cap.
    if encoded.len() > 2048 || encoded.len() % 4 == 1 {
        return Err(ERROR.into());
    }
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = Vec::new();
    let mut bits = 0u32;
    let mut count = 0;
    for b in encoded.bytes() {
        let value = alphabet.iter().position(|v| *v == b).ok_or(ERROR)?;
        bits = (bits << 6) | value as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            output.push((bits >> count) as u8);
        }
    }
    if crate::provision_wire::encode(&output) != encoded {
        return Err(ERROR.into());
    }
    Ok(output)
}
fn validate_certificate(
    scope: &ActivationInput,
    certificate: Option<&[u8]>,
    public: &[u8],
    now: u64,
) -> Result<()> {
    let required = scope.kinds.iter().any(|(_, required)| *required);
    if required != certificate.is_some() {
        return Err(ERROR.into());
    }
    let expected = if required {
        vec![crate::client_ca::pin(public).map_err(|_| ERROR)?]
    } else {
        vec![]
    };
    if scope.client_certificate_pins != expected {
        return Err(ERROR.into());
    }
    if let Some(certificate) = certificate {
        crate::client_ca::validate_at(certificate, public, scope.valid_until, now)
            .map_err(str::to_owned)?;
    }
    Ok(())
}
fn now() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "origin clock unavailable".into())
}
fn validate_size(name: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > FILE_CAP {
        return Err(format!(
            "origin {name} exceeds the 4 MiB custody limit; narrow source selection"
        ));
    }
    Ok(())
}
