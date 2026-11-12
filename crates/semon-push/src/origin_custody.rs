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
const ERROR: &str = "invalid or unreadable origin enrollment custody";

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
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Signed {
    configuration: Configuration,
    public_key: String,
    signature: String,
}

pub fn default_directory() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("semon/origin")
}
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
    fn read(&self, name: &str) -> Result<Zeroizing<Vec<u8>>> {
        let file = self
            .checked
            .open_file(Path::new(name), libc::O_RDONLY)
            .map_err(|_| ERROR)?;
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(65_537)
            .read_to_end(&mut bytes)
            .map_err(|_| ERROR)?;
        if bytes.len() > 65_536 {
            return Err(ERROR.into());
        }
        Ok(bytes)
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
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

pub(crate) fn install(
    base: &Path,
    endpoint: &str,
    machine: &str,
    scope: &ActivationInput,
    delivery: &Activated,
    private: &[u8],
    sources: &[PathBuf],
) -> Result<String> {
    if delivery.credentials.len() != 1
        || delivery.credentials[0].0 != crate::credentials::Kind::MirrorPush
    {
        return Err("invalid mirror credential delivery".into());
    }
    crate::Token::new(&delivery.credentials[0].1)?;
    // Parents contain no credentials. Dedicated custody itself must be owned 0700.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)
        .map_err(|_| ERROR)?;
    let root = Directory::open(base)?;
    let lock = root
        .checked
        .open_file(Path::new("install.lock"), libc::O_RDWR | libc::O_CREAT)
        .map_err(|_| ERROR)?;
    lock.try_lock().map_err(|_| "origin custody is in use")?;
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
    };
    let key = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        private,
        &ring::rand::SystemRandom::new(),
    )
    .map_err(|_| ERROR)?;
    let bytes = serde_json::to_vec(&configuration).map_err(|_| ERROR)?;
    let signature = key
        .sign(&ring::rand::SystemRandom::new(), &bytes)
        .map_err(|_| ERROR)?;
    let signed = Signed {
        configuration,
        public_key: crate::provision_wire::encode(key.public_key().as_ref()),
        signature: crate::provision_wire::encode(signature.as_ref()),
    };
    let consent = Consent {
        receiver_id: configuration_receiver(&signed),
        enrollment_id: delivery.enrollment_id.clone(),
        machine: machine.into(),
        sources: sources.to_vec(),
    };
    dir.write(
        "source-consent.json",
        &serde_json::to_vec(&consent).map_err(|_| ERROR)?,
    )?;
    dir.write("origin-key.pk8", private)?;
    dir.write("mirror-push.secret", delivery.credentials[0].1.as_bytes())?;
    dir.write(
        "enrollment.json",
        &serde_json::to_vec(&signed).map_err(|_| ERROR)?,
    )?;
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

/// Select by the stable name printed by connect; an omitted name requires exactly
/// one enrollment. No partially staged enrollment or absent/corrupt secret is used.
pub fn load(base: &Path, receiver: Option<&str>) -> Result<(Configuration, crate::Credential)> {
    let root =
        Directory::open(base).map_err(|_| "no readable enrollment; run semon connect --to URL")?;
    let name = match receiver {
        Some(name)
            if name.len() == 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
        {
            name.to_owned()
        }
        Some(_) => return Err("invalid receiver name; use the name printed by connect".into()),
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
                return Err("push requires exactly one enrollment or --receiver NAME; run semon connect --to URL".into());
            }
            names[0].clone()
        }
    };
    let dir = root.child(&name)?;
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
        || config.scope.kinds != [(crate::credentials::Kind::MirrorPush, false)]
        || !matches!(
            config.scope.binding,
            crate::enrollment::Binding::Inapplicable
        )
        || !config.scope.client_certificate_pins.is_empty()
        || crate::pairing::normalize_endpoint(&config.endpoint).map_err(|_| ERROR)?
            != config.endpoint
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
    let secret = dir.read("mirror-push.secret")?;
    let token = crate::Token::new(std::str::from_utf8(&secret).map_err(|_| ERROR)?)?;
    Ok((config, crate::Credential::Memory(token)))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub receiver_id: String,
    pub enrollment_id: String,
    pub machine: String,
    pub sources: Vec<PathBuf>,
}
fn configuration_receiver(signed: &Signed) -> String {
    signed.configuration.receiver_id.clone()
}
pub fn consent(base: &Path, receiver: &str, config: &Configuration) -> Result<Vec<PathBuf>> {
    let dir = Directory::open(base)?.child(receiver)?;
    let consent: Consent =
        serde_json::from_slice(&dir.read("source-consent.json")?).map_err(|_| ERROR)?;
    if consent.receiver_id != config.receiver_id
        || consent.enrollment_id != config.enrollment_id
        || consent.machine != config.machine
    {
        return Err(ERROR.into());
    }
    Ok(consent.sources)
}
