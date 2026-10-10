//! Real standalone access without Store/capture/Viewer dependencies or homes.
use std::{fs, process::Command};

use rusqlite::{Connection, OpenFlags};

#[test]
fn standalone_export_preserves_sensitive_history_and_refuses_existing_destinations() {
    let root =
        std::env::temp_dir().join(format!("semon-forensic-export-cli-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let source = root.join("original.sqlite3");
    let sentinel = b"private unlinked forensic evidence\0\xff";
    let connection = Connection::open(&source).unwrap();
    connection.execute_batch("PRAGMA user_version=7; CREATE TABLE canonical_traces(id TEXT); CREATE TABLE occurrences(trace_id TEXT,session TEXT); CREATE TABLE raw_carrier_records(raw_bytes BLOB); INSERT INTO canonical_traces VALUES('same'); INSERT INTO occurrences VALUES('same','one'),('same','two');").unwrap();
    connection
        .execute(
            "INSERT INTO raw_carrier_records VALUES(?1)",
            [sentinel.as_slice()],
        )
        .unwrap();
    connection.close().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let original = fs::read(&source).unwrap();
    let original_permissions = fs::metadata(&source).unwrap().permissions();
    let target = root.join("export");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_semon-forensic-export"))
            .arg("--store")
            .arg(&source)
            .arg("--out")
            .arg(&target)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .starts_with("semon-forensic-export: raw output may contain prompts")
    );
    assert!(
        !output
            .stdout
            .windows(sentinel.len())
            .any(|bytes| bytes == sentinel)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["format"], "semon.forensic-store-export");
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["source_schema_version"], 7);
    let copy = Connection::open_with_flags(
        target.join("traces.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        copy.query_row::<Vec<u8>, _, _>("SELECT raw_bytes FROM raw_carrier_records", [], |row| row
            .get(0))
            .unwrap(),
        sentinel
    );
    assert_eq!(
        copy.query_row::<i64, _, _>("SELECT count(*) FROM occurrences", [], |row| row.get(0))
            .unwrap(),
        2
    );
    drop(copy);
    let artifact = fs::read(target.join("traces.sqlite3")).unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read(target.join("traces.sqlite3")).unwrap(), artifact);
    for args in [
        vec!["--store"],
        vec!["--store", "missing", "--out"],
        vec![
            "--store",
            "missing",
            "--store",
            "again",
            "--out",
            "destination",
        ],
        vec!["--store", "missing", "--out", "destination", "--unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_semon-forensic-export"))
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    assert!(!root.join("missing").exists());
    assert!(!root.join("destination").exists());
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(
        fs::metadata(&source).unwrap().permissions(),
        original_permissions
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in ["manifest.json", "traces.sqlite3"] {
            assert_eq!(
                fs::metadata(target.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}
