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
fn materialize_propagates_catalog_producer_failure_without_publishing() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let manifest = state.join("analytics-store/analytics/manifest.json");
    let before = fs::read(&manifest).unwrap();
    // The read-only analytics format check passes, but the native producer must
    // refuse a current-format index bound to a different source namespace.
    let connection = rusqlite::Connection::open(state.join("index.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE meta SET value='synthetic-other-origin' WHERE key='storage_namespace'",
            [],
        )
        .unwrap();
    drop(connection);
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("catalog refresh:"));
    assert_eq!(fs::read(manifest).unwrap(), before);
}
