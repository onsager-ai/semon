//! Versioned, rebuildable current-source projection of an atomic facts record.
//! No source body or unrelated manifest row is read by the selected lookup.
use super::{FACTS_VERSION, Facts};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Identity {
    pub(crate) fn of(metadata: &fs::Metadata) -> io::Result<Self> {
        if !metadata.is_file() {
            return Err(unavailable());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                dev: metadata.dev(),
                ino: metadata.ino(),
                size: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            })
        }
        #[cfg(not(unix))]
        {
            Err(unavailable())
        }
    }
}

fn unavailable() -> io::Error {
    io::Error::other("Current source observation unavailable")
}
fn database(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".sources.sqlite");
    PathBuf::from(name)
}
fn current(path: &Path) -> io::Result<Identity> {
    // A projection never turns a symbolic-link substitution into authority.
    Identity::of(&fs::symlink_metadata(path)?)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    version: u32,
    source: Identity,
    facts_version: u32,
    hostname: Option<String>,
    selection_known: bool,
    manifest_digest: String,
}

pub(crate) fn publish(path: &Path, facts: &Facts, identity: &Identity) -> io::Result<()> {
    if current(path)? != *identity {
        return Err(unavailable());
    }
    let destination = database(path);
    let mut create = fs::OpenOptions::new();
    create.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    match create.open(&destination) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&destination)?.is_file() {
                return Err(unavailable());
            }
        }
        Err(error) => return Err(error),
    }
    let mut digest = Sha256::new();
    digest.update([u8::from(facts.codex_rollouts.is_some())]);
    if let Some(paths) = &facts.codex_rollouts {
        for native in paths {
            digest.update((native.len() as u64).to_le_bytes());
            digest.update(native.as_bytes());
        }
    }
    let manifest_digest = format!("{:x}", digest.finalize());
    let head = Head {
        version: VERSION,
        source: identity.clone(),
        facts_version: facts.version,
        hostname: (facts.hostname.len() <= 4096).then(|| facts.hostname.clone()),
        selection_known: facts.codex_rollouts.is_some(),
        manifest_digest: manifest_digest.clone(),
    };
    let result = (|| -> rusqlite::Result<()> {
        let mut connection = Connection::open(&destination)?;
        connection.busy_timeout(Duration::from_millis(100))?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS source_head (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL CHECK(length(value)<=8192)); CREATE TABLE IF NOT EXISTS current_codex (path TEXT PRIMARY KEY) WITHOUT ROWID;")?;
        // JSON writers are independent. The descriptor identity belongs to the
        // supplied facts, and the path must still select that exact descriptor.
        if current(path).ok().as_ref() != Some(identity) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let old: Option<String> = tx
            .query_row("SELECT value FROM source_head WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()?;
        let unchanged = old
            .and_then(|value| serde_json::from_str::<Head>(&value).ok())
            .is_some_and(|previous| {
                previous.version == VERSION && previous.manifest_digest == manifest_digest
            });
        if !unchanged {
            tx.execute("DELETE FROM current_codex", [])?;
            if let Some(paths) = &facts.codex_rollouts {
                let mut insert = tx.prepare("INSERT INTO current_codex(path) VALUES(?1)")?;
                for native in paths {
                    insert.execute([native])?;
                }
            }
        }
        let serialized = serde_json::to_string(&head).map_err(|_| rusqlite::Error::InvalidQuery)?;
        tx.execute("INSERT INTO source_head(id,value) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value", [serialized])?;
        tx.pragma_update(None, "user_version", VERSION)?;
        tx.commit()
    })();
    result.map_err(|_| unavailable())
}

pub(crate) struct CurrentSources {
    connection: Connection,
    path: PathBuf,
    head: Head,
}
impl CurrentSources {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let identity = current(path)?;
        let destination = database(path);
        if !fs::symlink_metadata(&destination)?.is_file() {
            return Err(unavailable());
        }
        let connection = Connection::open_with_flags(
            destination,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| unavailable())?;
        connection
            .busy_timeout(Duration::from_millis(50))
            .map_err(|_| unavailable())?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| unavailable())?;
        let value: String = connection
            .query_row(
                "SELECT value FROM source_head WHERE id=1 AND length(CAST(value AS BLOB))<=8192",
                [],
                |row| row.get(0),
            )
            .map_err(|_| unavailable())?;
        let head: Head = serde_json::from_str(&value).map_err(|_| unavailable())?;
        if version != VERSION
            || head.version != VERSION
            || head.source != identity
            || head.hostname.as_ref().is_some_and(|name| name.len() > 4096)
            || head.manifest_digest.len() != 64
            || !head
                .manifest_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || current(path)? != identity
        {
            return Err(unavailable());
        }
        Ok(Self {
            connection,
            path: path.to_owned(),
            head,
        })
    }
    pub(crate) fn validate(&self) -> io::Result<()> {
        if current(&self.path)? != self.head.source {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(crate) fn facts_known(&self) -> bool {
        self.head.facts_version == FACTS_VERSION
    }
    pub(crate) fn selection_known(&self) -> bool {
        self.facts_known() && self.head.selection_known
    }
    pub(crate) fn hostname(&self) -> Option<String> {
        self.facts_known()
            .then(|| self.head.hostname.clone())
            .flatten()
    }
    pub(crate) fn codex_current(&self, relative: &str) -> io::Result<Option<bool>> {
        if current(&self.path)? != self.head.source {
            return Err(unavailable());
        }
        if !self.selection_known() {
            return Ok(None);
        }
        let found: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM current_codex WHERE path=?1)",
                params![relative],
                |row| row.get(0),
            )
            .map_err(|_| unavailable())?;
        if current(&self.path)? != self.head.source {
            return Err(unavailable());
        }
        Ok(Some(found))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::facts::{read_facts, write_facts};

    fn fixture(paths: Option<&[&str]>) -> Facts {
        Facts {
            version: FACTS_VERSION,
            hostname: "observed-machine".into(),
            codex_rollouts: paths.map(|paths| paths.iter().map(|path| (*path).into()).collect()),
            ..Facts::default()
        }
    }
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-source-authority-{}",
            format_args!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn distinguishes_unknown_empty_current_and_unknown_version() {
        let root = root();
        let path = root.join("facts.json");
        for (facts, expected) in [
            (fixture(None), None),
            (fixture(Some(&[])), Some(false)),
            (fixture(Some(&["sessions/selected.jsonl"])), Some(true)),
            (
                Facts {
                    version: 999,
                    ..fixture(Some(&["sessions/selected.jsonl"]))
                },
                None,
            ),
        ] {
            write_facts(&path, &facts).unwrap();
            let source = CurrentSources::open(&path).unwrap();
            assert_eq!(
                source.codex_current("sessions/selected.jsonl").unwrap(),
                expected
            );
            assert_eq!(source.facts_known(), facts.version == FACTS_VERSION);
            assert_eq!(source.hostname().is_some(), facts.version == FACTS_VERSION);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn descriptor_prevents_late_writer_from_binding_another_record() {
        let root = root();
        let path = root.join("facts.json");
        let a = fixture(Some(&["a.jsonl"]));
        let file_a = crate::save_json_file(&path, &a).unwrap();
        let identity_a = Identity::of(&file_a.metadata().unwrap()).unwrap();
        write_facts(&path, &fixture(Some(&["b.jsonl"]))).unwrap();
        assert!(publish(&path, &a, &identity_a).is_err());
        let b = CurrentSources::open(&path).unwrap();
        assert_eq!(b.codex_current("a.jsonl").unwrap(), Some(false));
        assert_eq!(b.codex_current("b.jsonl").unwrap(), Some(true));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_in_place_change_and_disappearance_invalidate_open_reader() {
        let root = root();
        let path = root.join("facts.json");
        write_facts(&path, &fixture(Some(&["a.jsonl"]))).unwrap();
        let old = CurrentSources::open(&path).unwrap();
        write_facts(&path, &fixture(Some(&["b.jsonl"]))).unwrap();
        assert!(old.codex_current("a.jsonl").is_err());
        let old = CurrentSources::open(&path).unwrap();
        fs::write(&path, b"{}").unwrap();
        assert!(old.validate().is_err());
        assert!(CurrentSources::open(&path).is_err());
        fs::remove_file(&path).unwrap();
        assert!(old.validate().is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn optional_cache_failure_preserves_original_and_full_observation_repairs() {
        let root = root();
        let path = root.join("facts.json");
        fs::create_dir(database(&path)).unwrap();
        let facts = fixture(Some(&["selected.jsonl"]));
        write_facts(&path, &facts).unwrap();
        assert_eq!(read_facts(&path).unwrap(), facts);
        assert!(CurrentSources::open(&path).is_err());
        fs::remove_dir(database(&path)).unwrap();
        assert_eq!(read_facts(&path).unwrap(), facts);
        assert_eq!(
            CurrentSources::open(&path)
                .unwrap()
                .codex_current("selected.jsonl")
                .unwrap(),
            Some(true)
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(database(&path)).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlink_substitution_never_supplies_current_authority() {
        let root = root();
        let path = root.join("facts.json");
        write_facts(&path, &fixture(Some(&["a.jsonl"]))).unwrap();
        fs::rename(&path, root.join("original.json")).unwrap();
        std::os::unix::fs::symlink(root.join("original.json"), &path).unwrap();
        assert!(CurrentSources::open(&path).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_lookup_vm_work_stays_constant_as_unrelated_manifest_grows() {
        let root = root();
        let path = root.join("facts.json");
        let mut measured = Vec::new();
        for count in [100, 20_000] {
            let facts = Facts {
                codex_rollouts: Some(
                    (0..count)
                        .map(|n| format!("sessions/{n:08}.jsonl"))
                        .collect(),
                ),
                ..fixture(None)
            };
            write_facts(&path, &facts).unwrap();
            let source = CurrentSources::open(&path).unwrap();
            let mut query = source
                .connection
                .prepare("SELECT EXISTS(SELECT 1 FROM current_codex WHERE path=?1)")
                .unwrap();
            assert!(
                query
                    .query_row(["sessions/00000042.jsonl"], |row| row.get::<_, bool>(0))
                    .unwrap()
            );
            measured.push(query.get_status(rusqlite::StatementStatus::VmStep));
            drop(query);
            drop(source);
            let mut legacy = Vec::new();
            let mut focused = Vec::new();
            for _ in 0..31 {
                let start = std::time::Instant::now();
                let facts: Facts = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                assert!(
                    facts
                        .codex_rollouts
                        .unwrap()
                        .contains("sessions/00000042.jsonl")
                );
                legacy.push(start.elapsed().as_micros());
                let start = std::time::Instant::now();
                assert_eq!(
                    CurrentSources::open(&path)
                        .unwrap()
                        .codex_current("sessions/00000042.jsonl")
                        .unwrap(),
                    Some(true)
                );
                focused.push(start.elapsed().as_micros());
            }
            legacy.sort_unstable();
            focused.sort_unstable();
            eprintln!(
                "manifest rows={count} facts_bytes={} full_read_median_us={} indexed_open_lookup_median_us={} samples=31",
                fs::metadata(&path).unwrap().len(),
                legacy[15],
                focused[15]
            );
        }
        assert_eq!(measured[0], measured[1]);
        eprintln!("fixed selected source VM steps: {measured:?}");
        fs::remove_dir_all(root).unwrap();
    }
}
