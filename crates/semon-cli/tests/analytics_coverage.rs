//! Coverage reporting over synthetic published analytics only.

#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Output,
};

use serde_json::Value;

fn coverage(home: &fixture::Fixture, json: bool) -> Output {
    let mut command = home.command(env!("CARGO_BIN_EXE_semon"));
    command
        .args(["analytics", "coverage", "--cache"])
        .arg(home.root.join("state/index.json"));
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

fn materialize(home: &fixture::Fixture) -> Output {
    home.command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "materialize", "--all", "--proc-root"])
        .arg(home.root.join("empty-proc"))
        .arg("--cache")
        .arg(home.root.join("state/index.json"))
        .output()
        .unwrap()
}

fn recursive_listing(root: &Path) -> BTreeSet<PathBuf> {
    fn visit(root: &Path, directory: &Path, entries: &mut BTreeSet<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            entries.insert(path.strip_prefix(root).unwrap().to_owned());
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, entries);
            }
        }
    }

    let mut entries = BTreeSet::new();
    if root.exists() {
        visit(root, root, &mut entries);
    }
    entries
}

#[test]
fn coverage_reads_the_published_live_rows_in_text_and_json() {
    let home = fixture::Fixture::new();
    let state = home.root.join("state");
    let materialized = materialize(&home);
    assert!(
        materialized.status.success(),
        "{}",
        String::from_utf8_lossy(&materialized.stderr)
    );
    let before_coverage = recursive_listing(&state);

    let text = coverage(&home, false);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("PUBLISHED live-row coverage"));
    assert!(text.contains("writer TARGETS:"));
    assert!(text.contains("source-missing UIDs: 0"));
    assert_eq!(recursive_listing(&state), before_coverage);

    let json = coverage(&home, true);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    assert_eq!(recursive_listing(&state), before_coverage);
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["scope"], "PUBLISHED live-row coverage");
    assert_eq!(report["source_missing_uids"], 0);
    let groups = report["groups"].as_array().unwrap();
    assert_eq!(
        groups
            .iter()
            .map(|group| group["session_count"].as_u64().unwrap())
            .sum::<u64>(),
        2
    );
    assert!(groups.iter().all(|group| {
        group["step_count"].as_u64().unwrap() >= group["exclusive_step_count"].as_u64().unwrap()
    }));
}

#[test]
fn coverage_refuses_a_missing_store_without_creating_state() {
    let home = fixture::Fixture::new();
    let state = home.root.join("state");
    let before_coverage = recursive_listing(&state);
    let output = coverage(&home, false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no analytics store"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(recursive_listing(&state), before_coverage);
}
