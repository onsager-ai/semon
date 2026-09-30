//! The answer journal: a plain append-only JSON Lines record of every answer,
//! every refused answer, every refused registration, and every tool run found
//! after a viewer deny.
//!
//! It is not tamper-evident: the user, and code running as the user, can
//! edit it. Step 3 adds a hash chain with an outside anchor.

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

use serde_json::Value;

/// An open journal file.
#[derive(Debug)]
pub struct Journal {
    file: File,
}

impl Journal {
    /// Opens `path` for appending, creating it with mode 0600 and its parent
    /// directory with mode 0700. Refuses a parent directory that is a
    /// symlink, is not owned by this process's user or is open to anyone
    /// else, and a journal path that exists but isn't a regular file.
    pub fn open(path: &Path) -> io::Result<Self> {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| refuse("the journal path has no parent directory"))?;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let directory_metadata = fs::symlink_metadata(directory)?;
        if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
            return Err(refuse(
                "the journal's directory is a symlink or not a directory",
            ));
        }
        if directory_metadata.uid() != fs::metadata("/proc/self")?.uid() {
            return Err(refuse("the journal's directory belongs to another user"));
        }
        if directory_metadata.mode() & 0o777 != 0o700 {
            return Err(refuse("the journal's directory must have mode 0700"));
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(refuse("the journal path exists and is not a regular file"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(path)?;
        if file.metadata()?.mode() & 0o077 != 0 {
            return Err(refuse("the journal file must not be open to other users"));
        }
        Ok(Self { file })
    }

    /// Appends `line` as one line of JSON.
    pub fn append(&mut self, line: &Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(line)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)
    }
}

fn refuse(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use serde_json::{Value, json};

    use super::Journal;

    /// A fresh directory path under the system temp dir, not yet created.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "semon-control-{}-{}-{}",
            std::process::id(),
            name,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    pub(crate) fn lines(path: &std::path::Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("each journal line is JSON"))
            .collect()
    }

    #[test]
    fn creates_a_0600_file_in_a_0700_directory_and_appends_lines() {
        let directory = scratch("modes").join("semon").join("control");
        let path = directory.join("journal.jsonl");
        let mut journal = Journal::open(&path).unwrap();
        journal.append(&json!({"event": "one"})).unwrap();
        journal.append(&json!({"event": "two"})).unwrap();
        drop(journal);
        let mut journal = Journal::open(&path).unwrap();
        journal.append(&json!({"event": "three"})).unwrap();

        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let events: Vec<Value> = lines(&path)
            .into_iter()
            .map(|line| line["event"].clone())
            .collect();
        assert_eq!(events, [json!("one"), json!("two"), json!("three")]);
    }

    #[test]
    fn refuses_a_symlinked_directory() {
        let base = scratch("symlink");
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        let link = base.join("link");
        symlink(&real, &link).unwrap();
        assert!(Journal::open(&link.join("journal.jsonl")).is_err());
        assert!(!real.join("journal.jsonl").exists());
    }

    #[test]
    fn refuses_a_directory_open_to_others() {
        let directory = scratch("loose");
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Journal::open(&directory.join("journal.jsonl")).is_err());
    }

    #[test]
    fn refuses_a_journal_path_that_is_a_symlink() {
        let directory = scratch("file-link");
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let target = directory.join("elsewhere");
        fs::write(&target, b"").unwrap();
        let path = directory.join("journal.jsonl");
        symlink(&target, &path).unwrap();
        assert!(Journal::open(&path).is_err());
    }
}
