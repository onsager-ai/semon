//! Persisted reported-fact migration from the old JSON event cache.
//!
//! Retained because overwritten reported usage cannot be rebuilt from native logs.
//! Only reported runs and their stamp cross this boundary; event rows are rebuilt.
//! This module implements no Model, Viewer transport or historical-log parser.
//! See docs/design/model-oracle-retirement.md for the supported disposition.

use super::*;

/// The `meta` key recording when a retired JSON event cache that couldn't
/// be read was set aside (epoch ms). A readable one needs no marker: it is
/// imported whenever it is found, and removed.
const SET_ASIDE: &str = "legacy_json_set_aside";

/// What the retired JSON event cache (`sessions-index.events.json`) holds
/// that the logs can't give back: the reported runs `~/.claude.json` has
/// since overwritten, and the stamp they were last read at. Only these
/// fields are read; the rest of the file (its version and every file's
/// index) is skipped as it streams past, and those files are read again
/// from their logs.
#[derive(Deserialize)]
struct Legacy {
    #[serde(default)]
    reported_runs: BTreeMap<String, BTreeMap<i64, ReportedRunSnapshot>>,
    #[serde(default)]
    claude_json_stamp: Option<ReportedFileStamp>,
}

#[cfg(test)]
thread_local! {
    pub(in crate::events) static IMPORTS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    pub(in crate::events) static BEFORE_IMPORT: Hook = const { std::cell::RefCell::new(None) };
    pub(in crate::events) static AFTER_CLAIM: Hook = const { std::cell::RefCell::new(None) };
}

/// Which file a handle is: dev, inode, size and modified time. A rename
/// keeps all four; an older semon's save (a new file renamed over the path)
/// changes the inode.
type Identity = (u64, u64, u64, Option<std::time::SystemTime>);

fn identity(file: &fs::File) -> io::Result<Identity> {
    let metadata = file.metadata()?;
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    Ok((dev, ino, metadata.len(), metadata.modified().ok()))
}

/// A JSON cache read from an open handle: which file it was, and what it
/// held. An error when it couldn't be read to its end (an I/O error): it is
/// then left for the next open.
fn read_legacy(file: &fs::File) -> io::Result<(Identity, Result<Legacy, serde_json::Error>)> {
    let read_as = identity(file)?;
    match serde_json::from_reader::<_, Legacy>(io::BufReader::new(file)) {
        Err(error) if error.is_io() => Err(io::Error::other(error)),
        parsed => Ok((read_as, parsed)),
    }
}

/// Imports the retired JSON event cache whenever it is found, then removes
/// it.
///
/// Its reported runs join the store's with `INSERT OR IGNORE`, so a run the
/// store holds (captured since, or imported before) keeps its snapshot, and
/// importing the same file twice, or from two processes at once, changes
/// nothing. Its stamp is taken only if the store has none. It is imported
/// again whenever it is present: an older semon still running after the
/// upgrade rewrites it with runs no process here has seen, and
/// `~/.claude.json` has since overwritten them.
///
/// The file is parsed from its open handle before the write lock is taken,
/// reading only those two fields as it streams past, and its rows commit
/// with `synchronous = FULL`, so they are on disk before the file goes.
///
/// Removal claims the file first, renaming it to `<name>.importing.<pid>.<n>.<16 hex digits>`,
/// so no save an older semon makes to the path is ever removed or set aside
/// unread: the claimed file is removed (or set aside) only if it is the one
/// that was parsed, and read and imported first if it isn't. A claim left by
/// a crash is imported and removed at the next open, as is any other
/// process's claim: every step is idempotent.
///
/// A file that isn't that JSON, whatever its version, is renamed aside
/// (`.corrupt`, then `.corrupt.1`, …; never over an existing file) and
/// recorded in `meta`. A file that can't be opened or read to its end is
/// left for the next open.
pub(super) fn import_legacy(connection: &mut Connection, legacy: &Path) -> rusqlite::Result<()> {
    for claimed in claims(legacy) {
        finish(connection, legacy, &claimed, None)?;
    }
    let Some(file) = open_regular(legacy) else {
        return Ok(());
    };
    let (read_as, parsed) = match read_legacy(&file) {
        Ok(read) => read,
        Err(error) => {
            warn_import(legacy, &error);
            return Ok(());
        }
    };
    // The handle stays open until the claim is checked, so its inode can't
    // be reused by another file meanwhile.
    #[cfg(test)]
    hook(&BEFORE_IMPORT);
    if let Ok(found) = &parsed {
        import_rows(connection, legacy, Some(found))?;
    }
    let claimed = sibling(legacy, &claim_suffix());
    match fs::rename(legacy, &claimed) {
        Ok(()) => {}
        // Claimed or removed by another process meanwhile: its import is
        // the same as this one.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            warn_import(legacy, &error);
            return Ok(());
        }
    }
    #[cfg(test)]
    hook(&AFTER_CLAIM);
    let finished = finish(connection, legacy, &claimed, Some((read_as, parsed)));
    drop(file);
    finished
}

/// A claim's suffix, unique to this call: the pid, a counter, and 64
/// random bits, so no two claims share a name (a rename would replace the
/// other), whichever processes make them.
fn claim_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    static CLAIMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let claim = CLAIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut random = std::collections::hash_map::RandomState::new().build_hasher();
    random.write_u64(claim);
    random.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    format!(
        ".importing.{}.{claim}.{:016x}",
        std::process::id(),
        random.finish()
    )
}

/// Whether `suffix` (after `<name>`) is a claim's: `.importing.<pid>.<n>.
/// <16 hex digits>`, exactly as [`claim_suffix`] makes it.
fn is_claim(suffix: &str) -> bool {
    let Some(rest) = suffix.strip_prefix(".importing.") else {
        return false;
    };
    fn digits(part: &str) -> bool {
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
    }
    let parts: Vec<&str> = rest.split('.').collect();
    matches!(
        parts.as_slice(),
        [pid, n, random]
            if digits(pid)
                && digits(n)
                && random.len() == 16
                && random.bytes().all(|byte| byte.is_ascii_hexdigit())
    )
}

/// The claims (`<name>.importing.…`) a crash or another process left.
fn claims(legacy: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (legacy.parent(), legacy.file_name()) else {
        return Vec::new();
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let name = name.to_string_lossy();
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .strip_prefix(&*name)
                        .is_some_and(is_claim)
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Ends the import of a claimed file: removes it when it is the file whose
/// rows were imported (or sets it aside, when that file was unreadable), and
/// otherwise reads it and does the same with its own contents.
fn finish(
    connection: &mut Connection,
    legacy: &Path,
    claimed: &Path,
    read: Option<(Identity, Result<Legacy, serde_json::Error>)>,
) -> rusqlite::Result<()> {
    // Gone (finished by another process), or not a regular file.
    let Some(file) = open_regular(claimed) else {
        return Ok(());
    };
    let parsed = match (identity(&file), read) {
        (Ok(now), Some((read_as, parsed))) if now == read_as => parsed,
        // Not the file that was parsed (or no file was): read this one.
        _ => match read_legacy(&file) {
            Ok((_, parsed)) => {
                if let Ok(found) = &parsed {
                    import_rows(connection, legacy, Some(found))?;
                }
                parsed
            }
            Err(error) => {
                warn_import(legacy, &error);
                return Ok(());
            }
        },
    };
    drop(file);
    match parsed {
        Ok(Legacy { reported_runs, .. }) => {
            remove_legacy(claimed);
            let runs: usize = reported_runs.values().map(BTreeMap::len).sum();
            eprintln!(
                "semon: imported {runs} reported runs from {} into the session index, and removed it",
                legacy.display()
            );
        }
        // Recorded only once it is set aside, so a claim that can't be is
        // tried again rather than recorded as done.
        Err(error) => match set_aside_legacy(legacy, claimed) {
            Ok(Some(aside)) => {
                import_rows(connection, legacy, None)?;
                eprintln!(
                    "semon: {} couldn't be read ({error}); it was set aside as {} and not imported",
                    legacy.display(),
                    aside.display()
                );
            }
            // Set aside by another process meanwhile.
            Ok(None) => {}
            Err(rename) => warn_import(legacy, &rename),
        },
    }
    Ok(())
}

/// Moves an unreadable claimed file to the first free name of
/// `<legacy>.corrupt`, `<legacy>.corrupt.1`, …, never over an existing
/// file: hard-linked there (a link fails rather than replaces), then
/// unlinked from its claim. Where the filesystem has no hard links, it is
/// left intact and an error is returned; a check then rename could overwrite
/// another process's archive. `None` when the claim is
/// already gone: another process set it aside.
pub(super) fn set_aside_legacy(legacy: &Path, claimed: &Path) -> io::Result<Option<PathBuf>> {
    for n in 0..1000 {
        let aside = if n == 0 {
            sibling(legacy, ".corrupt")
        } else {
            sibling(legacy, &format!(".corrupt.{n}"))
        };
        {
            match fs::hard_link(claimed, &aside) {
                Ok(()) => {
                    match fs::remove_file(claimed) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => {
                            return Err(error);
                        }
                        _ => {}
                    }
                    return Ok(Some(aside));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if let (Ok(source), Ok(target)) =
                            (fs::symlink_metadata(claimed), fs::symlink_metadata(&aside))
                            && source.dev() == target.dev()
                            && source.ino() == target.ino()
                        {
                            match fs::remove_file(claimed) {
                                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                                    return Err(error);
                                }
                                _ => return Ok(Some(aside)),
                            }
                        }
                    }
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                // No hard links here (EPERM, unsupported): preserve the claim.
                Err(_) => {}
            }
        }
        // Refuse to rename on filesystems without hard links: rename can
        // replace another process's archive after a check for existence.
        // Keep the original claim intact rather than risk losing either file.
        return Err(io::Error::other(
            "cannot archive the claim safely: this filesystem has no hard links",
        ));
    }
    Err(io::Error::other("no free .corrupt name"))
}

/// Writes a parsed JSON cache's runs and stamp in one `BEGIN IMMEDIATE`
/// transaction with `synchronous = FULL`, or, for one that couldn't be read
/// (`None`), the record that it is set aside.
fn import_rows(
    connection: &mut Connection,
    legacy: &Path,
    parsed: Option<&Legacy>,
) -> rusqlite::Result<()> {
    connection.pragma_update(None, "synchronous", "FULL")?;
    let written = write_import(connection, parsed);
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    written?;
    #[cfg(test)]
    IMPORTS.with(|imports| imports.borrow_mut().push(legacy.to_owned()));
    #[cfg(not(test))]
    let _ = legacy;
    Ok(())
}

fn write_import(connection: &mut Connection, parsed: Option<&Legacy>) -> rusqlite::Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match parsed {
        Some(Legacy {
            reported_runs,
            claude_json_stamp,
        }) => {
            let mut put = transaction.prepare(&PUT_RUN.replacen("OR REPLACE", "OR IGNORE", 1))?;
            for run in reported_runs.values().flat_map(BTreeMap::values) {
                put_run(&mut put, run)?;
            }
            drop(put);
            if let Some(stamp) = claude_json_stamp {
                transaction.execute(
                    "INSERT OR IGNORE INTO meta (key, value) VALUES ('claude_json_stamp', ?1)",
                    [json(stamp)?],
                )?;
            }
        }
        None => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis())
                .to_string();
            transaction.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                [SET_ASIDE, now.as_str()],
            )?;
        }
    }
    transaction.commit()
}

/// Removes the imported JSON cache; failing that, it is imported again at
/// the next open.
fn remove_legacy(legacy: &Path) {
    match fs::remove_file(legacy) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => warn_import(legacy, &error),
        _ => {}
    }
}
