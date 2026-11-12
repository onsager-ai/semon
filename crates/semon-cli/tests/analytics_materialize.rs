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
    materialize_with(home, &[])
}

fn materialize_with(home: &fixture::Fixture, arguments: &[&str]) -> Output {
    home.command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "materialize", "--all", "--proc-root"])
        .arg(home.root.join("empty-proc"))
        .arg("--cache")
        .arg(home.root.join("state/index.json"))
        .args(arguments)
        .output()
        .unwrap()
}

// Compare every live fact and dimension, allowing physical generations and row
// order to differ between a one-shot read and several bounded publications.
fn live_rows(root: &Path) -> BTreeMap<String, Vec<Vec<String>>> {
    use arrow::{
        array::{Int64Array, StringArray},
        compute::cast,
        datatypes::DataType,
    };
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let manifest = semon_asi::read_manifest(root).unwrap().unwrap();
    let live = semon_asi::read_live_map(root, &manifest).unwrap();
    let mut result = BTreeMap::new();
    for table in semon_asi::schema::TABLES {
        if table.kind == semon_asi::schema::Kind::Append {
            continue;
        }
        let mut rows = Vec::new();
        for entry in manifest.files_of(table.name) {
            let file = fs::File::open(root.join("analytics").join(&entry.path)).unwrap();
            for batch in ParquetRecordBatchReaderBuilder::try_new(file)
                .unwrap()
                .build()
                .unwrap()
            {
                let batch = batch.unwrap();
                for row in 0..batch.num_rows() {
                    if table.kind == semon_asi::schema::Kind::Session {
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
                        if live.get(uids.value(row)) != Some(&generations.value(row)) {
                            continue;
                        }
                    }
                    rows.push(
                        batch
                            .schema()
                            .fields()
                            .iter()
                            .zip(batch.columns())
                            .filter(|(field, _)| field.name() != "_gen")
                            .map(|(_, array)| {
                                let values = match array.data_type() {
                                    DataType::Timestamp(..) => {
                                        cast(array, &DataType::Int64).unwrap()
                                    }
                                    _ => array.clone(),
                                };
                                arrow::util::display::array_value_to_string(values.as_ref(), row)
                                    .unwrap()
                            })
                            .collect(),
                    );
                }
            }
        }
        rows.sort();
        result.insert(table.name.to_owned(), rows);
    }
    result
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
    let namespace = semon_sessions::read_analytics_namespace(&state)
        .unwrap()
        .unwrap();
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
        semon_sessions::read_analytics_namespace(&state)
            .unwrap()
            .unwrap(),
        namespace
    );
    assert_eq!(live_session_rows(&store), first_rows);
}

#[test]
fn materialize_tiny_budget_drains_several_chunks_and_matches_one_shot_rows() {
    let home = fixture::Fixture::new();
    for index in 3..=5 {
        let id = format!("00000000-0000-4000-8000-{index:012}");
        let user = format!("batch-user-{index}");
        let transcript = [
            serde_json::json!({"type":"user", "uuid":user, "parentUuid":null,
                "sessionId":id, "timestamp":"2026-10-03T08:16:44Z", "cwd":"/fixture/probe/work",
                "message":{"role":"user", "content":"Synthetic bounded CLI fixture"}}),
            serde_json::json!({"type":"assistant", "uuid":format!("batch-assistant-{index}"),
                "parentUuid":user, "sessionId":id, "timestamp":"2026-10-03T08:16:45Z",
                "message":{"id":format!("batch-message-{index}"), "role":"assistant",
                    "model":"claude-sonnet-4-5", "content":[{"type":"text", "text":"Synthetic answer"}],
                    "usage":{"input_tokens":10, "output_tokens":5}}}),
        ].map(|record| record.to_string()).join("\n") + "\n";
        home.write(
            &format!("configured-claude/projects/fixture/{id}.jsonl"),
            transcript.as_bytes(),
        );
    }
    let first = materialize(&home);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let state = home.root.join("state");
    let store = state.join("analytics-store");
    let namespace = semon_sessions::read_analytics_namespace(&state)
        .unwrap()
        .unwrap();
    let expected = live_rows(&store);
    assert_eq!(live_session_rows(&store).len(), 5);
    assert_eq!(
        semon_asi::read_manifest(&store)
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    // Keep the same native index, namespace and source paths for both cold reads.
    fs::rename(&store, state.join("one-shot-store")).unwrap();
    let bounded = materialize_with(&home, &["--batch-sessions", "1", "--batch-steps", "1"]);
    assert!(
        bounded.status.success(),
        "{}",
        String::from_utf8_lossy(&bounded.stderr)
    );
    assert_eq!(
        semon_sessions::read_analytics_namespace(&state)
            .unwrap()
            .unwrap(),
        namespace
    );
    let manifest = semon_asi::read_manifest(&store).unwrap().unwrap();
    assert!(
        manifest.generation >= 5,
        "one generation per bounded whole-session chunk"
    );
    assert_eq!(live_rows(&store), expected);
}

#[test]
fn materialize_refuses_zero_or_invalid_batch_budgets_before_creating_state() {
    let home = fixture::Fixture::new();
    for arguments in [
        ["--batch-sessions", "0"],
        ["--batch-steps", "no"],
        ["--batch-steps", "0"],
    ] {
        let result = materialize_with(&home, &arguments);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("requires a positive integer"));
        assert!(!home.root.join("state").exists());
    }
}

#[test]
fn materialize_refuses_batch_limits_above_sqlite_maximum_before_creating_state() {
    let home = fixture::Fixture::new();
    for flag in ["--batch-sessions", "--batch-steps"] {
        let result = materialize_with(&home, &[flag, "9223372036854775808"]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(&format!(
            "requires a positive integer no greater than {}",
            usize::try_from(i64::MAX).unwrap_or(usize::MAX)
        )));
        assert!(!home.root.join("state").exists());
    }
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
        String::from_utf8_lossy(&output.stderr).contains("unsupported or inconsistent"),
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
        String::from_utf8_lossy(&output.stderr)
            .contains("Another process is writing local analytics"),
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
    let help = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "analytics materialize",
        "--batch-sessions N",
        "--batch-steps N",
        "64 sessions/journal entries and 8192 steps",
        "min(usize::MAX, i64::MAX)",
    ] {
        assert!(help.contains(expected), "missing {expected}: {help}");
    }
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
        assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported or inconsistent"));
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
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Another process is writing local analytics")
    );
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
fn materialize_refuses_non_directory_analytics_before_creating_namespace() {
    let home = fixture::Fixture::new();
    let state = home.root.join("state");
    let store = state.join("analytics-store");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("analytics"), b"preserve").unwrap();
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported or inconsistent"));
    assert_eq!(fs::read(store.join("analytics")).unwrap(), b"preserve");
    assert!(!store.join("analytics.lock").exists());
    assert!(!state.join("analytics-namespace").exists());
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported or inconsistent"));
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
        assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported or inconsistent"));
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported or inconsistent"));
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
