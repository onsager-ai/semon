//! Durable namespace for an explicitly materialized local analytics origin.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// The durable file beside the Event Index that owns the local origin ID.
pub const ANALYTICS_NAMESPACE_FILE: &str = "analytics-namespace";

const PREFIX: &str = "origin-";
const RANDOM_BYTES: usize = 32;

/// Loads the local origin namespace or creates it once, without replacing an
/// existing value. A store without this file is ambiguous, so it is refused.
pub fn load_or_create_analytics_namespace(state_dir: &Path) -> Result<String, String> {
    let path = state_dir.join(ANALYTICS_NAMESPACE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => return load(&path),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!(
                "cannot inspect analytics namespace {}: {error}",
                path.display()
            ));
        }
        Err(_) => {}
    }

    let manifest = state_dir.join("analytics-store/analytics/manifest.json");
    if manifest.exists() {
        return Err(format!(
            "analytics namespace {} is missing while store manifest {} exists; refusing to generate a namespace that would fork session identities",
            path.display(),
            manifest.display()
        ));
    }
    fs::create_dir_all(state_dir).map_err(|error| {
        format!(
            "cannot create analytics namespace directory {}: {error}",
            state_dir.display()
        )
    })?;

    let namespace = random_namespace()?;
    let temporary = temporary_path(state_dir, &namespace);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).map_err(|error| {
        format!(
            "cannot create temporary analytics namespace {}: {error}",
            temporary.display()
        )
    })?;
    let write = (|| {
        file.write_all(namespace.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
    })();
    drop(file);
    if let Err(error) = write {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot write analytics namespace {}: {error}",
            temporary.display()
        ));
    }

    match fs::hard_link(&temporary, &path) {
        Ok(()) => {
            fs::remove_file(&temporary).map_err(|error| {
                format!(
                    "cannot finalize analytics namespace {}: {error}",
                    path.display()
                )
            })?;
            sync_directory(state_dir)?;
            Ok(namespace)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temporary);
            load(&path)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(format!(
                "cannot install analytics namespace {}: {error}",
                path.display()
            ))
        }
    }
}

fn load(path: &Path) -> Result<String, String> {
    let content = fs::read_to_string(path).map_err(|error| {
        format!(
            "cannot read analytics namespace {}: {error}",
            path.display()
        )
    })?;
    let namespace = content.strip_suffix('\n').ok_or_else(|| {
        format!(
            "malformed analytics namespace {}; expected one newline-terminated opaque ID",
            path.display()
        )
    })?;
    if !valid(namespace) {
        return Err(format!(
            "malformed analytics namespace {}; expected {PREFIX} followed by lowercase hexadecimal randomness",
            path.display()
        ));
    }
    Ok(namespace.to_owned())
}

fn random_namespace() -> Result<String, String> {
    let mut bytes = [0_u8; RANDOM_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| format!("cannot obtain OS randomness: {error}"))?;
    let mut namespace = String::with_capacity(PREFIX.len() + RANDOM_BYTES * 2);
    namespace.push_str(PREFIX);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut namespace, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(namespace)
}

fn temporary_path(state_dir: &Path, namespace: &str) -> PathBuf {
    state_dir.join(format!(
        ".{ANALYTICS_NAMESPACE_FILE}.{}-{namespace}.tmp",
        std::process::id()
    ))
}

fn valid(namespace: &str) -> bool {
    namespace.len() == PREFIX.len() + RANDOM_BYTES * 2
        && namespace.starts_with(PREFIX)
        && namespace[PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "cannot sync analytics namespace directory {}: {error}",
                path.display()
            )
        })?;
    Ok(())
}
