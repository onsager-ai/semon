//! Explicit local analytics publication over synthetic native homes only.

#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Output, Stdio},
};

fn materialize(home: &fixture::Fixture) -> Output {
    home.command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "materialize", "--all", "--proc-root"])
        .arg(home.root.join("empty-proc"))
        .arg("--cache")
        .arg(home.root.join("state/index.json"))
        .output()
        .unwrap()
}

// Read the published live view, excluding its physical generation column.
fn live_session_rows(root: &Path) -> BTreeMap<String, Vec<String>> {
    use arrow::{
        array::{Int64Array, StringArray},
        compute::cast,
        datatypes::DataType,
    };
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let manifest = semon_asi::read_manifest(root).unwrap().unwrap();
    let live = semon_asi::read_live_map(root, &manifest).unwrap();
    let mut rows = BTreeMap::new();
    for entry in manifest.files_of("fact_session") {
        let file = fs::File::open(root.join("analytics").join(&entry.path)).unwrap();
        for batch in ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap()
        {
            let batch = batch.unwrap();
            let uids = batch
                .column_by_name("session_uid")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let generations = batch
                .column_by_name("_gen")
                .unwrap()
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            for row in 0..batch.num_rows() {
                if live.get(uids.value(row)) != Some(&generations.value(row)) {
                    continue;
                }
                let values = batch
                    .schema()
                    .fields()
                    .iter()
                    .zip(batch.columns())
                    .filter(|(field, _)| field.name() != "_gen")
                    .map(|(_, array)| {
                        // Named timestamp zones need chrono-tz to render. Compare
                        // the recorded integer values instead.
                        let values = match array.data_type() {
                            DataType::Timestamp(..) => cast(array, &DataType::Int64).unwrap(),
                            _ => array.clone(),
                        };
                        arrow::util::display::array_value_to_string(values.as_ref(), row).unwrap()
                    })
                    .collect();
                assert!(rows.insert(uids.value(row).to_owned(), values).is_none());
            }
        }
    }
    rows
}

#[test]
fn materialize_publishes_once_and_reuses_its_namespace() {
    let home = fixture::Fixture::new();
    let first = materialize(&home);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let state = home.root.join("state");
    let namespace = fs::read_to_string(state.join("analytics-namespace")).unwrap();
    let manifest = state.join("analytics-store/analytics/manifest.json");
    assert!(manifest.exists());
    let store = state.join("analytics-store");
    let first_rows = live_session_rows(&store);
    let expected = [
        format!(
            "{}/claude/00000000-0000-4000-8000-000000000001",
            namespace.trim()
        ),
        format!(
            "{}/codex/00000000-0000-4000-8000-000000000002",
            namespace.trim()
        ),
    ];
    assert_eq!(first_rows.keys().cloned().collect::<Vec<_>>(), expected);
    assert!(
        String::from_utf8_lossy(&first.stdout).contains(namespace.trim()),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );

    let second = materialize(&home);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(state.join("analytics-namespace")).unwrap(),
        namespace
    );
    assert_eq!(live_session_rows(&store), first_rows);
}

#[test]
fn materialize_refuses_an_unsupported_index_without_changing_its_bytes() {
    let home = fixture::Fixture::new();
    let index = home.root.join("state/index.sqlite3");
    fs::create_dir_all(index.parent().unwrap()).unwrap();
    let before = b"not a sqlite database";
    fs::write(&index, before).unwrap();
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("analytics writer refused"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(index).unwrap(), before);
    assert!(!home.root.join("state/analytics-namespace").exists());
    assert!(!home.root.join("state/analytics-store").exists());
}

#[test]
fn materialize_reports_a_busy_writer_without_publishing() {
    let home = fixture::Fixture::new();
    let lock_path = home.root.join("state/analytics-store/analytics.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock = fs::File::create(&lock_path).unwrap();
    let before_lock = fs::read(&lock_path).unwrap();
    lock.lock().unwrap();
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("analytics writer busy"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&lock_path).unwrap(), before_lock);
    assert_eq!(fs::read_dir(home.root.join("state")).unwrap().count(), 1);
    assert_eq!(
        fs::read_dir(lock_path.parent().unwrap()).unwrap().count(),
        1
    );
    assert!(!home.root.join("state/index.sqlite3").exists());
    assert!(!home.root.join("state/analytics-namespace").exists());
    lock.unlock().unwrap();
}

#[test]
fn materialize_is_the_only_default_cli_path_that_creates_analytics_state() {
    let home = fixture::Fixture::new();
    let cache = home.root.join("state/index.json");
    for arguments in [
        vec!["sessions", "--all", "--json"],
        vec!["query", "list_sessions", "--all"],
        vec!["mcp", "--all"],
    ] {
        let output = home
            .command(env!("CARGO_BIN_EXE_semon"))
            .args(&arguments)
            .arg("--proc-root")
            .arg(home.root.join("empty-proc"))
            .arg("--cache")
            .arg(&cache)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!home.root.join("state/analytics-namespace").exists());
    assert!(!home.root.join("state/analytics-store").exists());
}

#[test]
fn usage_lists_materialize() {
    let output = fixture::Fixture::new()
        .command(env!("CARGO_BIN_EXE_semon"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("analytics materialize"));
}

#[test]
fn materialize_refuses_old_and_new_sqlite_schemas_unchanged() {
    for schema in [1, 2_147_483_647] {
        let home = fixture::Fixture::new();
        let index = home.root.join("state/index.sqlite3");
        fs::create_dir_all(index.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&index).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('retained')",
            )
            .unwrap();
        connection
            .pragma_update(None, "user_version", schema)
            .unwrap();
        drop(connection);
        let before = fs::read(&index).unwrap();
        let output = materialize(&home);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("analytics writer refused"));
        assert_eq!(fs::read(index).unwrap(), before);
        assert!(!home.root.join("state/analytics-namespace").exists());
        assert!(!home.root.join("state/analytics-store").exists());
    }
}

#[test]
fn materialize_busy_writer_preserves_an_existing_index_and_namespace() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let index = fs::read(state.join("index.sqlite3")).unwrap();
    let namespace = fs::read(state.join("analytics-namespace")).unwrap();
    let manifest = fs::read(state.join("analytics-store/analytics/manifest.json")).unwrap();
    let lock = fs::OpenOptions::new()
        .write(true)
        .open(state.join("analytics-store/analytics.lock"))
        .unwrap();
    lock.lock().unwrap();
    // A source change must not reach the producer while another writer owns it.
    home.write("configured-claude/.claude.json", b"{\"changed\":true}");
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("analytics writer busy"));
    assert_eq!(fs::read(state.join("index.sqlite3")).unwrap(), index);
    assert_eq!(
        fs::read(state.join("analytics-namespace")).unwrap(),
        namespace
    );
    assert_eq!(
        fs::read(state.join("analytics-store/analytics/manifest.json")).unwrap(),
        manifest
    );
    lock.unlock().unwrap();
}

#[test]
fn materialize_refuses_machine_before_creating_state() {
    let home = fixture::Fixture::new();
    let output = home
        .command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "materialize", "--machine"])
        .arg(&home.root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--machine is not supported"));
    assert!(!home.root.join("semon-state").exists());
}

#[test]
fn materialize_requires_the_existing_stores_namespace() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let manifest = state.join("analytics-store/analytics/manifest.json");
    let before = fs::read(&manifest).unwrap();
    fs::remove_file(state.join("analytics-namespace")).unwrap();
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to generate a namespace"));
    assert_eq!(fs::read(manifest).unwrap(), before);
    assert!(!state.join("analytics-namespace").exists());
}

#[test]
fn materialize_namespace_refusal_preserves_crash_leftovers() {
    for malformed in [false, true] {
        let home = fixture::Fixture::new();
        assert!(materialize(&home).status.success());
        let state = home.root.join("state");
        let analytics = state.join("analytics-store/analytics");
        let manifest = analytics.join("manifest.json");
        let before = fs::read(&manifest).unwrap();
        let leftovers = [
            analytics.join("manifest.json.tmp"),
            analytics.join("fact_session/g999999.parquet"),
            analytics.join("live/g999999.parquet"),
        ];
        for path in &leftovers {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"synthetic uncommitted bytes").unwrap();
        }
        let namespace = state.join("analytics-namespace");
        if malformed {
            fs::write(&namespace, b"malformed\n").unwrap();
        } else {
            fs::remove_file(&namespace).unwrap();
        }
        let output = materialize(&home);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("analytics namespace"));
        assert_eq!(fs::read(manifest).unwrap(), before);
        for path in &leftovers {
            assert_eq!(fs::read(path).unwrap(), b"synthetic uncommitted bytes");
        }
        if malformed {
            assert_eq!(fs::read(namespace).unwrap(), b"malformed\n");
        } else {
            assert!(!namespace.exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn materialize_refuses_a_symlinked_index_without_publishing() {
    let target_home = fixture::Fixture::new();
    assert!(materialize(&target_home).status.success());
    let target = target_home.root.join("state/index.sqlite3");
    let before = fs::read(&target).unwrap();
    let home = fixture::Fixture::new();
    let state = home.root.join("state");
    fs::create_dir_all(&state).unwrap();
    std::os::unix::fs::symlink(&target, state.join("index.sqlite3")).unwrap();
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("analytics writer refused"));
    assert_eq!(fs::read(target).unwrap(), before);
    assert!(
        fs::symlink_metadata(state.join("index.sqlite3"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!state.join("analytics-namespace").exists());
    assert!(!state.join("analytics-store").exists());
}

#[test]
fn analytics_input_validation_preserves_live_wal_entries_and_bytes() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let index = state.join("index.sqlite3");
    let connection = rusqlite::Connection::open(&index).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; \
        CREATE TABLE synthetic_validation_probe(value INTEGER); \
        INSERT INTO synthetic_validation_probe VALUES(1); BEGIN; \
        SELECT value FROM synthetic_validation_probe;",
        )
        .unwrap();
    // Keep the WAL and its latest read mark live while validating another
    // read-only connection. No checkpoint or writer shutdown occurs here.
    assert!(fs::metadata(state.join("index.sqlite3-wal")).unwrap().len() > 32);
    assert!(state.join("index.sqlite3-shm").exists());
    let snapshot = || -> BTreeMap<std::ffi::OsString, Option<Vec<u8>>> {
        fs::read_dir(&state)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let bytes = if entry.file_type().unwrap().is_file() {
                    Some(fs::read(entry.path()).unwrap())
                } else {
                    None
                };
                (entry.file_name(), bytes)
            })
            .collect()
    };
    let before = snapshot();
    let input = semon_sessions::SqliteAnalyticsInput::open(&index, "synthetic-validation").unwrap();
    drop(input);
    assert_eq!(snapshot(), before);
    connection.execute_batch("ROLLBACK").unwrap();
}
