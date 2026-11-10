//! Durable namespace for an explicitly materialized local analytics origin.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
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
    let write = file
        .write_all(namespace.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
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
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path).map_err(|error| {
        format!(
            "cannot open analytics namespace {}: {error}",
            path.display()
        )
    })?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!(
            "analytics namespace {} must be a regular file",
            path.display()
        ));
    }
    let mut content = String::new();
    // Include one extra byte to detect oversized input without unbounded reads.
    file.take((PREFIX.len() + RANDOM_BYTES * 2 + 2) as u64)
        .read_to_string(&mut content)
        .map_err(|error| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        thread,
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temporary_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-analytics-namespace-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn creates_an_opaque_stable_namespace_once() {
        let root = temporary_root("stable");
        let first = load_or_create_analytics_namespace(&root).unwrap();
        assert!(valid(&first));
        assert_eq!(first, load_or_create_analytics_namespace(&root).unwrap());
        assert_eq!(
            fs::read_to_string(root.join(ANALYTICS_NAMESPACE_FILE)).unwrap(),
            format!("{first}\n")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn created_namespace_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_root("mode");
        load_or_create_analytics_namespace(&root).unwrap();
        let mode = fs::metadata(root.join(ANALYTICS_NAMESPACE_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_creation_uses_one_namespace() {
        let root = temporary_root("concurrent");
        let mut workers = Vec::new();
        for _ in 0..8 {
            let root = root.clone();
            workers.push(thread::spawn(move || {
                load_or_create_analytics_namespace(&root)
            }));
        }
        let first = workers.remove(0).join().unwrap().unwrap();
        for worker in workers {
            assert_eq!(first, worker.join().unwrap().unwrap());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_content_is_refused_without_replacement() {
        let root = temporary_root("malformed");
        let path = root.join(ANALYTICS_NAMESPACE_FILE);
        fs::write(&path, "origin-not-hex\n").unwrap();
        assert!(load_or_create_analytics_namespace(&root).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "origin-not-hex\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn namespace_refuses_empty_whitespace_wrong_charset_and_oversized_files() {
        let root = temporary_root("invalid-content");
        let path = root.join(ANALYTICS_NAMESPACE_FILE);
        for bytes in [
            Vec::new(),
            b" \n".to_vec(),
            format!("origin-{}\n", "g".repeat(64)).into_bytes(),
            format!("origin-{}\n", "A".repeat(64)).into_bytes(),
            vec![0xff; 72],
            format!("origin-{}\nextra", "a".repeat(64)).into_bytes(),
            vec![b'a'; 1024 * 1024],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert!(load_or_create_analytics_namespace(&root).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_namespace_with_existing_store_refuses() {
        let root = temporary_root("existing-store");
        let manifest = root.join("analytics-store/analytics/manifest.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&manifest, "{}").unwrap();
        assert!(load_or_create_analytics_namespace(&root).is_err());
        assert!(!root.join(ANALYTICS_NAMESPACE_FILE).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn namespace_is_lowercase_hex_randomness_with_a_fixed_prefix() {
        let root = temporary_root("opaque");
        let namespace = load_or_create_analytics_namespace(&root).unwrap();
        assert!(namespace.starts_with(PREFIX));
        assert!(
            namespace[PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(all(test, unix))]
mod special_file_tests {
    use super::*;
    use std::os::unix::{ffi::OsStrExt, fs::symlink};

    #[test]
    fn namespace_refuses_symlink_and_fifo_without_following_or_blocking() {
        for fifo in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "semon-namespace-special-{}-{fifo}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let path = root.join(ANALYTICS_NAMESPACE_FILE);
            let target = root.join("target");
            fs::write(&target, format!("origin-{}\n", "a".repeat(64))).unwrap();
            if fifo {
                let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
                    fs::remove_dir_all(root).unwrap();
                    continue;
                }
            } else {
                symlink(&target, &path).unwrap();
            }
            assert!(load_or_create_analytics_namespace(&root).is_err());
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if fifo {
                use std::os::unix::fs::FileTypeExt;
                assert!(kind.is_fifo());
            } else {
                assert!(kind.is_symlink());
            }
            assert_eq!(
                fs::read_to_string(target).unwrap(),
                format!("origin-{}\n", "a".repeat(64))
            );
            fs::remove_dir_all(root).unwrap();
        }
    }
}
