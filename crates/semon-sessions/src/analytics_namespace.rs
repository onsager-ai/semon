//! Durable identity for an explicitly initialized local analytics producer.

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use crate::analytics::{Binding, Error, ErrorKind, Stage};

pub const ANALYTICS_NAMESPACE_FILE: &str = "analytics-namespace";
const FORMAT: &str = "semon-producer-1";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub format: String,
    pub namespace: String,
    pub origin_incarnation: String,
}

/// Reads identity without creating state or recovering unpublished files.
pub fn read_analytics_namespace(state_dir: &Path) -> Result<Option<String>, String> {
    read_identity(state_dir)
        .map(|identity| identity.map(|identity| identity.namespace))
        .map_err(|error| error.to_string())
}

pub(crate) fn read_identity(state_dir: &Path) -> Result<Option<Identity>, Error> {
    let path = state_dir.join(ANALYTICS_NAMESPACE_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(state_dir.join("analytics-store/analytics/manifest.json"))
                .is_ok()
            {
                return Err(Error::new(ErrorKind::InconsistentState, Stage::Identity));
            }
            return Ok(None);
        }
        Err(_) => return Err(Error::new(ErrorKind::Io, Stage::Identity)),
    };
    if !metadata.is_file() || metadata.len() > 512 {
        return Err(Error::new(ErrorKind::InconsistentState, Stage::Identity));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options
        .open(&path)
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Identity))?;
    let mut bytes = Vec::new();
    file.take(513)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Identity))?;
    // The superseded one-line namespace format has no producer incarnation.
    if bytes.starts_with(b"origin-") {
        return Err(Error::new(ErrorKind::UnsupportedState, Stage::Identity));
    }
    let identity: Identity = serde_json::from_slice(&bytes)
        .map_err(|_| Error::new(ErrorKind::InconsistentState, Stage::Identity))?;
    if identity.format != FORMAT {
        return Err(Error::new(ErrorKind::UnsupportedState, Stage::Identity));
    }
    if !valid(&identity.namespace, "origin-") || !valid(&identity.origin_incarnation, "producer-") {
        return Err(Error::new(ErrorKind::InconsistentState, Stage::Identity));
    }
    if let Some(manifest) = semon_asi::read_manifest(&state_dir.join("analytics-store"))
        .map_err(|error| Error::store(error, Stage::Identity))?
        && manifest.origin_incarnations.get(&identity.namespace)
            != Some(&identity.origin_incarnation)
    {
        return Err(Error::new(ErrorKind::BindingMismatch, Stage::Identity));
    }
    Ok(Some(identity))
}

pub(crate) fn create(state_dir: &Path) -> Result<Identity, Error> {
    let identity = Identity {
        format: FORMAT.into(),
        namespace: random_id("origin-")?,
        origin_incarnation: random_id("producer-")?,
    };
    let path = state_dir.join(ANALYTICS_NAMESPACE_FILE);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Identity))?;
    let bytes = serde_json::to_vec(&identity).expect("identity serialization");
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Identity))?;
    // Exclusive creation under the writer lock never replaces existing bytes.
    // An interrupted write remains an explicit partial-state refusal on retry.
    #[cfg(unix)]
    File::open(state_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Identity))?;
    Ok(identity)
}

impl Identity {
    pub(crate) fn binding(&self, index_incarnation: String) -> Binding {
        Binding {
            namespace: self.namespace.clone(),
            origin_incarnation: self.origin_incarnation.clone(),
            index_incarnation,
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static REFUSE_RANDOMNESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn random_id(prefix: &str) -> Result<String, Error> {
    #[cfg(test)]
    if REFUSE_RANDOMNESS.with(|refuse| refuse.get()) {
        return Err(Error::new(
            ErrorKind::RandomnessUnavailable,
            Stage::Identity,
        ));
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| Error::new(ErrorKind::RandomnessUnavailable, Stage::Identity))?;
    let mut id = prefix.to_owned();
    for byte in bytes {
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").expect("writing to String");
    }
    Ok(id)
}

fn valid(id: &str, prefix: &str) -> bool {
    id.len() == prefix.len() + 64
        && id.starts_with(prefix)
        && id[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
