//! Retirement export, independent of capture and schema-migrating Store APIs.
//! All tables, including unlinked raw records and custody, belong in this explicit
//! forensic artifact. Ordinary session, log and replication reads never call it.
use std::{
    error::Error,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use rusqlite::{Connection, OpenFlags, backup::Backup, backup::StepResult};
use serde_json::json;
use sha2::{Digest, Sha256};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn export(source: &Path, directory: &Path) -> Result<()> {
    if !fs::metadata(source)?.is_file() {
        return Err("the trace store must be an existing regular SQLite file".into());
    }
    let mut directories = DirBuilder::new();
    #[cfg(unix)]
    directories.mode(0o700);
    directories.create(directory)?; // Atomic create; never reuse an existing target.
    let snapshot = directory.join(".input");
    directories.create(&snapshot)?;
    let source_files = snapshot_sources(source, &snapshot, || {})?;
    // SQLite can coordinate/recover its WAL/SHM only in this private copy.
    // Originals have only been opened as read-only file handles, never by SQLite.
    let original = Connection::open_with_flags(
        snapshot.join("traces.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    original.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
    let version: u32 = original.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    // These are the retired Store versions this export contract qualifies.
    // Do not infer a future version's meaning or migrate the historical original.
    if version > 7 {
        return Err(
            format!("unsupported trace schema version {version}; export supports 0..=7").into(),
        );
    }
    let trace_store: bool = original.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='canonical_traces')",
        [],
        |row| row.get(0),
    )?;
    if !trace_store {
        return Err("not a Canonical Trace Store; Session Event Index files are separate".into());
    }

    let database = directory.join("traces.sqlite3");
    private_file(&database)?.sync_all()?;
    let mut copy = Connection::open_with_flags(
        &database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    {
        let backup = Backup::new(&original, &mut copy)?;
        loop {
            match backup.step(256)? {
                StepResult::Done => break,
                StepResult::More => {}
                _ => {
                    return Err(
                        "snapshot became busy; export is incomplete, retry to a new directory"
                            .into(),
                    );
                }
            }
        }
    }
    // The artifact is self-contained: committed WAL evidence has been copied,
    // and only this private destination is converted to rollback-journal mode.
    copy.execute_batch("PRAGMA journal_mode=DELETE;")?;
    let integrity: String = copy.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err("exported SQLite integrity check failed; export is incomplete".into());
    }
    let mut names =
        copy.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?;
    let mut tables = Vec::new();
    for name in names.query_map([], |row| row.get::<_, String>(0))? {
        let name = name?;
        let quoted = name.replace('"', "\"\"");
        let rows: i64 =
            copy.query_row(&format!("SELECT count(*) FROM \"{quoted}\""), [], |row| {
                row.get(0)
            })?;
        tables.push(json!({"name": name, "rows": rows}));
    }
    drop(names);
    copy.close().map_err(|(_, error)| error)?;
    original.close().map_err(|(_, error)| error)?;
    // Only temporary copies created inside this new private export are removed.
    // The original database, sidecars, custody and cursors remain untouched.
    fs::remove_dir_all(snapshot)?;
    let mut input = File::open(&database)?;
    input.sync_all()?;
    let hash = stream_hash(&mut input, None)?;
    let manifest = json!({
        "format": "semon.forensic-store-export",
        "version": 1,
        "source_schema_version": version,
        "includes_forensic": true,
        "database": {"file": "traces.sqlite3", "bytes": input.metadata()?.len(), "sha256": hash},
        "tables": tables,
        "source_files": source_files,
    });
    // Manifest publication is the completion marker. Failures leave private
    // incomplete files, never overwrite user data or advertise a valid export.
    let staging = directory.join("manifest.json.partial");
    let mut output = private_file(&staging)?;
    serde_json::to_writer_pretty(&mut output, &manifest)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    fs::rename(staging, directory.join("manifest.json"))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
fn stamp(path: &Path) -> io::Result<Option<Stamp>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Err(io::Error::other("source sidecar must be a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(Stamp {
        length: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        identity: (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ),
    }))
}
fn members(source: &Path) -> [PathBuf; 4] {
    ["", "-wal", "-shm", "-journal"].map(|suffix| {
        let mut path = source.as_os_str().to_os_string();
        path.push(suffix);
        path.into()
    })
}
fn snapshot_sources(
    source: &Path,
    snapshot: &Path,
    before_verify: impl FnOnce(),
) -> Result<Vec<serde_json::Value>> {
    let resolved = fs::canonicalize(source)?;
    let paths = members(&resolved);
    let before: Vec<_> = paths
        .iter()
        .map(|path| stamp(path))
        .collect::<io::Result<_>>()?;
    let mut hashes = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if before[index].is_none() {
            hashes.push(None);
            continue;
        }
        let name = [
            "traces.sqlite3",
            "traces.sqlite3-wal",
            "traces.sqlite3-shm",
            "traces.sqlite3-journal",
        ][index];
        let mut original = File::open(path)?;
        let mut copy = private_file(&snapshot.join(name))?;
        let hash = stream_hash(&mut original, Some(&mut copy))?;
        copy.sync_all()?;
        hashes.push(Some((name, hash)));
    }
    before_verify();
    // Require all original files to keep their identity/change stamps and exact
    // bytes across the complete copy. Creation/removal of any sidecar also fences
    // publication. Operators pause writers; this check rejects an observed race.
    for (index, path) in paths.iter().enumerate() {
        if stamp(path)? != before[index] {
            return Err(
                "source generation changed; pause writers and retry to a new directory".into(),
            );
        }
        if let Some((_, copied)) = &hashes[index]
            && stream_hash(&mut File::open(path)?, None)? != *copied
        {
            return Err("source bytes changed; pause writers and retry to a new directory".into());
        }
    }
    for (index, path) in paths.iter().enumerate() {
        if stamp(path)? != before[index] {
            return Err(
                "source generation changed during verification; retry to a new directory".into(),
            );
        }
    }
    Ok(hashes
        .into_iter()
        .flatten()
        .map(|(name, hash)| json!({"file":name, "sha256":hash}))
        .collect())
}
fn stream_hash(input: &mut File, mut output: Option<&mut File>) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        if let Some(file) = &mut output {
            file.write_all(&buffer[..n])?;
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn directory(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-export-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn legacy_versions_export_all_tables_without_migration_or_permission_changes() {
        for version in 0..=7 {
            let root = directory("legacy");
            let source = root.join("original.sqlite3");
            let connection = Connection::open(&source).unwrap();
            connection
                .execute_batch(&format!(
                    "CREATE TABLE canonical_traces(id TEXT PRIMARY KEY, content BLOB); \
                 CREATE TABLE occurrences(id INTEGER PRIMARY KEY, trace_id TEXT, parent INTEGER); \
                 CREATE TABLE raw_carrier_records(id INTEGER PRIMARY KEY, raw BLOB); \
                 CREATE TABLE capture_evidence_generations(id INTEGER PRIMARY KEY, custody BLOB); \
                 INSERT INTO canonical_traces VALUES ('repeated', x'00ff'); \
                 INSERT INTO occurrences VALUES(1,'repeated',NULL),(2,'repeated',1); \
                 INSERT INTO raw_carrier_records VALUES(1,x'ff00'),(2,x'010203'); \
                 INSERT INTO capture_evidence_generations VALUES(1,x'004455'); \
                 PRAGMA user_version={version};"
                ))
                .unwrap();
            connection.close().unwrap();
            let before = fs::read(&source).unwrap();
            let permissions = fs::metadata(&source).unwrap().permissions();
            let exported = root.join("export");
            export(&source, &exported).unwrap();
            assert_eq!(fs::read(&source).unwrap(), before);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&source).unwrap().permissions().mode(),
                    permissions.mode()
                );
                assert_eq!(
                    fs::metadata(&exported).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                for name in ["traces.sqlite3", "manifest.json"] {
                    assert_eq!(
                        fs::metadata(exported.join(name))
                            .unwrap()
                            .permissions()
                            .mode()
                            & 0o777,
                        0o600
                    );
                }
            }
            let copy = Connection::open_with_flags(
                exported.join("traces.sqlite3"),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert_eq!(
                copy.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                    .unwrap(),
                version
            );
            assert_eq!(
                copy.query_row::<Vec<u8>, _, _>(
                    "SELECT raw FROM raw_carrier_records WHERE id=2",
                    [],
                    |r| r.get(0)
                )
                .unwrap(),
                [1, 2, 3]
            );
            assert_eq!(
                copy.query_row::<u32, _, _>("SELECT parent FROM occurrences WHERE id=2", [], |r| r
                    .get(0))
                    .unwrap(),
                1
            );
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(exported.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(manifest["source_schema_version"], version);
            assert_eq!(manifest["tables"].as_array().unwrap().len(), 4);
            let bytes = fs::read(exported.join("traces.sqlite3")).unwrap();
            assert_eq!(
                manifest["database"]["sha256"],
                format!("{:x}", Sha256::digest(&bytes))
            );
            assert!(export(&source, &exported).is_err());
            assert_eq!(fs::read(&source).unwrap(), before);
            drop(copy);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn quiescent_wal_commits_survive_and_original_wal_shm_are_unchanged() {
        let root = directory("wal");
        let live = root.join("live.sqlite3");
        let writer = Connection::open(&live).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE canonical_traces(id TEXT); INSERT INTO canonical_traces VALUES('committed-in-wal');").unwrap();
        // Copy synthetic evidence while the fixture writer is idle. The copied
        // source has no connection, matching an operator-paused historical store.
        let source = root.join("quiescent.sqlite3");
        for suffix in ["", "-wal", "-shm"] {
            fs::copy(
                format!("{}{suffix}", live.display()),
                format!("{}{suffix}", source.display()),
            )
            .unwrap();
        }
        let originals: Vec<_> = ["", "-wal", "-shm"]
            .map(|suffix| {
                let path = format!("{}{suffix}", source.display());
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .into();
        let exported = root.join("export");
        export(&source, &exported).unwrap();
        for (path, bytes) in originals {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        let copy = Connection::open(exported.join("traces.sqlite3")).unwrap();
        assert_eq!(
            copy.query_row::<String, _, _>("SELECT id FROM canonical_traces", [], |r| r.get(0))
                .unwrap(),
            "committed-in-wal"
        );
        assert!(!exported.join("traces.sqlite3-wal").exists());
        // A later WAL mutation cannot qualify an already-copied generation.
        let raced = root.join("raced");
        fs::create_dir(&raced).unwrap();
        let error = snapshot_sources(&live, &raced, || {
            writer
                .execute_batch("INSERT INTO canonical_traces VALUES('new-generation')")
                .unwrap();
        })
        .unwrap_err();
        assert!(error.to_string().contains("changed"));
        drop(copy);
        drop(writer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_version_non_trace_and_missing_sources_fail_before_output_creation() {
        let root = directory("refusal");
        let source = root.join("source.sqlite3");
        assert!(export(&source, &root.join("missing")).is_err());
        assert!(!source.exists());
        let connection = Connection::open(&source).unwrap();
        connection
            .execute_batch("CREATE TABLE canonical_traces(id TEXT); PRAGMA user_version=8;")
            .unwrap();
        connection.close().unwrap();
        let before = fs::read(&source).unwrap();
        assert!(
            export(&source, &root.join("future"))
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(!root.join("future/manifest.json").exists());
        assert_eq!(fs::read(&source).unwrap(), before);
        let connection = Connection::open(&source).unwrap();
        connection.execute_batch("DROP TABLE canonical_traces; CREATE TABLE session_catalog(id TEXT); PRAGMA user_version=7;").unwrap();
        connection.close().unwrap();
        assert!(
            export(&source, &root.join("index"))
                .unwrap_err()
                .to_string()
                .contains("Session Event Index")
        );
        assert!(!root.join("index/manifest.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removed_forensic_region_is_not_recreated_by_export() {
        let root = directory("removed-raw");
        let source = root.join("source.sqlite3");
        let original = Connection::open(&source).unwrap();
        original
            .execute_batch("CREATE TABLE canonical_traces(id TEXT); PRAGMA user_version=5;")
            .unwrap();
        original.close().unwrap();
        let before = fs::read(&source).unwrap();
        export(&source, &root.join("export")).unwrap();
        assert_eq!(fs::read(&source).unwrap(), before);
        let copy = Connection::open(root.join("export/traces.sqlite3")).unwrap();
        assert_eq!(
            copy.query_row::<i64, _, _>(
                "SELECT count(*) FROM sqlite_schema WHERE name='raw_carrier_records'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        drop(copy);
        fs::remove_dir_all(root).unwrap();
    }
}
