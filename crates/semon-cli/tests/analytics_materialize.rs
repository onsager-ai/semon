//! Explicit local analytics publication over synthetic native homes only.

#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;

use std::{
    fs,
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
    let first_manifest = fs::read(&manifest).unwrap();
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
    assert_eq!(fs::read(&manifest).unwrap(), first_manifest);
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
}

#[test]
fn materialize_reports_a_busy_writer_without_publishing() {
    let home = fixture::Fixture::new();
    let lock_path = home.root.join("state/analytics-store/analytics.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock = fs::File::create(lock_path).unwrap();
    lock.lock().unwrap();
    let output = materialize(&home);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("analytics writer busy"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
            .args(arguments)
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
