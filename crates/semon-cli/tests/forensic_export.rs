//! Complete historical data export through the actual CLI, with no native homes.
use std::{fs, process::Command};

use rusqlite::{Connection, OpenFlags};
use semon_store::{
    AuthoredBy, NewOccurrence, NewRawCarrierRecord, RepoSource, SemanticCore, TraceStore,
};

#[test]
fn complete_export_preserves_real_store_links_unprojected_and_legacy_raw_records() {
    let root = std::env::temp_dir().join(format!("semon-export-cli-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let source = root.join("original.sqlite3");
    let sentinel = b"private forensic sentinel\x00\xff";
    let mut store = TraceStore::open(&source).unwrap();
    let core =
        SemanticCore::from_value(serde_json::json!({"kind":"intent","content":"repeated content"}))
            .unwrap();
    for (session, sequence) in [("session-one", 0), ("session-two", 0)] {
        store
            .capture_line(
                NewRawCarrierRecord::new("claude", sentinel),
                session,
                sequence,
                10,
                &[(
                    core.clone(),
                    NewOccurrence {
                        session,
                        sequence,
                        timestamp: 10,
                        repo: "repo",
                        repo_source: RepoSource::ExplicitOverride,
                        parent_sequence: None,
                        agent: Some("native-agent"),
                        authored_by: AuthoredBy::Agent,
                    },
                )],
            )
            .unwrap();
    }
    // Same line key with different bytes is historical evidence, not a duplicate.
    store
        .capture_line(
            NewRawCarrierRecord::new("claude", b"rewritten unprojected line"),
            "session-one",
            0,
            20,
            &[],
        )
        .unwrap();
    drop(store);
    let connection = Connection::open(&source).unwrap();
    connection
        .execute(
            "INSERT INTO raw_carrier_records(carrier,raw_bytes) VALUES('legacy',?1)",
            [b"unlinked legacy bytes".as_slice()],
        )
        .unwrap();
    connection.close().unwrap();
    let before = fs::read(&source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_semon"))
        .args(["forensic", "--store"])
        .arg(&source)
        .arg("--export-store")
        .arg(root.join("export"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("may contain prompts"));
    assert!(
        !output
            .stdout
            .windows(sentinel.len())
            .any(|bytes| bytes == sentinel)
    );
    assert_eq!(fs::read(&source).unwrap(), before);

    let copy = Connection::open_with_flags(
        root.join("export/traces.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    for (table, expected) in [
        ("canonical_traces", 1),
        ("occurrences", 2),
        ("raw_record_traces", 2),
        ("raw_carrier_records", 4),
    ] {
        assert_eq!(
            copy.query_row::<i64, _, _>(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        copy.query_row::<Vec<u8>, _, _>(
            "SELECT raw_bytes FROM raw_carrier_records WHERE carrier='legacy'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        b"unlinked legacy bytes"
    );
    assert_eq!(
        copy.query_row::<i64, _, _>(
            "SELECT count(*) FROM occurrences WHERE agent='native-agent'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        2
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("export/manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["source_schema_version"], 7);
    assert_eq!(manifest["includes_forensic"], true);
    for name in [
        "capture_source_occurrences",
        "capture_evidence_generations",
        "capture_evidence_records",
    ] {
        assert!(
            manifest["tables"]
                .as_array()
                .unwrap()
                .iter()
                .any(|table| table["name"] == name),
            "{manifest}"
        );
    }
    drop(copy);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn export_refuses_mixed_modes_and_never_creates_a_missing_original() {
    let root =
        std::env::temp_dir().join(format!("semon-export-cli-refusal-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let source = root.join("missing.sqlite3");
    for extra in [
        vec!["--session", "session"],
        vec!["--out", "excerpt"],
        vec!["--export-store", "again"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_semon"))
            .args(["forensic", "--store"])
            .arg(&source)
            .arg("--export-store")
            .arg(root.join("export"))
            .args(extra)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!source.exists());
        assert!(!root.join("export").exists());
    }
    fs::remove_dir_all(root).unwrap();
}
