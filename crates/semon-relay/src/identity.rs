use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use age::{secrecy::ExposeSecret, x25519};
use chacha20poly1305::aead::{OsRng, rand_core::RngCore};
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const AGE_IDENTITY_FILE: &str = "identity.age";
pub const SIGNING_KEY_FILE: &str = "signing.key";
pub const RECIPIENTS_FILE: &str = "recipients.txt";
pub const MACHINES_FILE: &str = "machines.txt";

#[derive(Clone)]
pub struct MachineIdentity {
    pub age: x25519::Identity,
    pub signing: SigningKey,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InitOutcome {
    pub age_created: bool,
    pub signing_created: bool,
}

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("cannot access identity file {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid age identity in {0}")]
    AgeIdentity(PathBuf),
    #[error("invalid age recipient on line {line} of {path}: {value}")]
    AgeRecipient {
        path: PathBuf,
        line: usize,
        value: String,
    },
    #[error("no recipients are enrolled in {0}")]
    NoRecipients(PathBuf),
    #[error("invalid signing key in {0}; expected 32 bytes of hex")]
    SigningKey(PathBuf),
    #[error("invalid signing public key: expected 32 bytes of hex")]
    SigningPublicKey,
    #[error("enrollment name must not be empty or contain a newline")]
    EnrollmentName,
    #[error("invalid machine key on line {line} of {path}")]
    MachineKey { path: PathBuf, line: usize },
    #[error("identity path {path} has unsafe permissions {mode:o}")]
    UnsafePermissions { path: PathBuf, mode: u32 },
    #[error("identity path {0} is not a regular file")]
    NotFile(PathBuf),
    #[error("identity path {0} is not a directory")]
    NotDirectory(PathBuf),
}

impl MachineIdentity {
    pub fn load(config_dir: &Path) -> Result<Self, IdentityError> {
        require_private_dir(config_dir)?;
        Ok(Self {
            age: load_age_identity(&config_dir.join(AGE_IDENTITY_FILE))?,
            signing: load_signing_key(&config_dir.join(SIGNING_KEY_FILE))?,
        })
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.signing.verifying_key())
    }
}

pub fn init(config_dir: &Path) -> Result<InitOutcome, IdentityError> {
    create_private_dir(config_dir)?;
    let age_path = config_dir.join(AGE_IDENTITY_FILE);
    let signing_path = config_dir.join(SIGNING_KEY_FILE);
    let age_created = if age_path.exists() {
        require_private_file(&age_path)?;
        false
    } else {
        let identity = x25519::Identity::generate();
        create_secret(&age_path, identity.to_string().expose_secret().as_bytes())?;
        true
    };
    let signing_created = if signing_path.exists() {
        require_private_file(&signing_path)?;
        false
    } else {
        let mut secret = [0_u8; 32];
        OsRng.fill_bytes(&mut secret);
        let encoded = hex::encode(secret);
        create_secret(&signing_path, encoded.as_bytes())?;
        true
    };
    Ok(InitOutcome {
        age_created,
        signing_created,
    })
}

pub fn load_age_identity(path: &Path) -> Result<x25519::Identity, IdentityError> {
    require_private_file(path)?;
    let value = read_string(path)?;
    x25519::Identity::from_str(value.trim())
        .map_err(|_| IdentityError::AgeIdentity(path.to_path_buf()))
}

pub fn load_recipients(path: &Path) -> Result<Vec<x25519::Recipient>, IdentityError> {
    require_private_file(path)?;
    let value = read_string(path)?;
    let mut recipients = Vec::new();
    for (index, raw) in value.lines().enumerate() {
        let key = raw.split('#').next().unwrap_or_default().trim();
        if key.is_empty() {
            continue;
        }
        recipients.push(x25519::Recipient::from_str(key).map_err(|_| {
            IdentityError::AgeRecipient {
                path: path.to_path_buf(),
                line: index + 1,
                value: key.to_owned(),
            }
        })?);
    }
    if recipients.is_empty() {
        Err(IdentityError::NoRecipients(path.to_path_buf()))
    } else {
        Ok(recipients)
    }
}

pub fn enroll_recipient(
    config_dir: &Path,
    public_key: &str,
    name: &str,
) -> Result<(), IdentityError> {
    x25519::Recipient::from_str(public_key).map_err(|_| IdentityError::AgeRecipient {
        path: config_dir.join(RECIPIENTS_FILE),
        line: 0,
        value: public_key.to_owned(),
    })?;
    append_enrollment(&config_dir.join(RECIPIENTS_FILE), public_key, name)
}

pub fn enroll_machine(
    receiver_dir: &Path,
    public_key: &str,
    name: &str,
) -> Result<String, IdentityError> {
    let key = parse_verifying_key(public_key)?;
    append_enrollment(&receiver_dir.join(MACHINES_FILE), public_key, name)?;
    Ok(fingerprint(&key))
}

pub fn load_machines(path: &Path) -> Result<Vec<(String, VerifyingKey)>, IdentityError> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(IdentityError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let value = read_string(path)?;
    value
        .lines()
        .enumerate()
        .filter_map(|(index, raw)| {
            let key = raw.split('#').next().unwrap_or_default().trim();
            (!key.is_empty()).then_some((index, key))
        })
        .map(|(index, value)| {
            let key = parse_verifying_key(value).map_err(|_| IdentityError::MachineKey {
                path: path.to_path_buf(),
                line: index + 1,
            })?;
            Ok((fingerprint(&key), key))
        })
        .collect()
}

pub fn load_signing_key(path: &Path) -> Result<SigningKey, IdentityError> {
    require_private_file(path)?;
    let value = read_string(path)?;
    let bytes = hex::decode(value.trim()).map_err(|_| IdentityError::SigningKey(path.into()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| IdentityError::SigningKey(path.into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub fn parse_verifying_key(value: &str) -> Result<VerifyingKey, IdentityError> {
    let bytes = hex::decode(value.trim()).map_err(|_| IdentityError::SigningPublicKey)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| IdentityError::SigningPublicKey)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| IdentityError::SigningPublicKey)
}

pub fn fingerprint(key: &VerifyingKey) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

fn append_enrollment(path: &Path, public_key: &str, name: &str) -> Result<(), IdentityError> {
    if name.trim().is_empty() || name.contains(['\n', '\r']) {
        return Err(IdentityError::EnrollmentName);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_private_dir(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => require_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(IdentityError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    writeln!(file, "{} # {}", public_key.trim(), name.trim()).map_err(|source| {
        IdentityError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    file.sync_all().map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn create_secret(path: &Path, bytes: &[u8]) -> Result<(), IdentityError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|source| IdentityError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn read_string(path: &Path) -> Result<String, IdentityError> {
    fs::read_to_string(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn create_private_dir(path: &Path) -> Result<(), IdentityError> {
    fs::create_dir_all(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(IdentityError::NotDirectory(path.to_path_buf()));
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        IdentityError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(())
}

#[cfg(unix)]
fn require_private_file(path: &Path) -> Result<(), IdentityError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(IdentityError::NotFile(path.to_path_buf()));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        Ok(())
    } else {
        Err(IdentityError::UnsafePermissions {
            path: path.to_path_buf(),
            mode,
        })
    }
}

#[cfg(not(unix))]
fn require_private_file(_path: &Path) -> Result<(), IdentityError> {
    Ok(())
}

#[cfg(unix)]
fn require_private_dir(path: &Path) -> Result<(), IdentityError> {
    let metadata = fs::metadata(path).map_err(|source| IdentityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mode = metadata.permissions().mode() & 0o777;
    if metadata.is_dir() && mode & 0o077 == 0 {
        Ok(())
    } else {
        Err(IdentityError::UnsafePermissions {
            path: path.to_path_buf(),
            mode,
        })
    }
}

#[cfg(not(unix))]
fn require_private_dir(_path: &Path) -> Result<(), IdentityError> {
    Ok(())
}
