//! Coverage reporting over synthetic published analytics only.

#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;

use std::{
    collections::{BTreeMap, BTreeSet},
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

fn export(home: &fixture::Fixture, destination: &Path) -> Output {
    home.command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "export"])
        .arg(destination)
        .arg("--cache")
        .arg(home.root.join("state/index.json"))
        .output()
        .unwrap()
}

fn reset(home: &fixture::Fixture, arguments: &[&str]) -> Output {
    let mut command = home.command(env!("CARGO_BIN_EXE_semon"));
    command.args(["analytics", "reset"]);
    command.args(arguments);
    command
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

fn file_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
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
fn export_copies_a_verified_snapshot_without_changing_the_store() {
    let home = fixture::Fixture::new();
    let materialized = materialize(&home);
    assert!(
        materialized.status.success(),
        "{}",
        String::from_utf8_lossy(&materialized.stderr)
    );
    let state = home.root.join("state");
    let before = recursive_listing(&state);
    let destination = home.root.join("analytics-export");
    let output = export(&home, &destination);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(destination.join("export.json").is_file());
    assert!(destination.join("analytics/manifest.json").is_file());
    assert!(destination.join("analytics-namespace").is_file());
    assert_eq!(recursive_listing(&state), before);
}

#[test]
fn export_refuses_nonempty_and_inside_store_destinations() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let destination = home.root.join("occupied");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep"), b"keep").unwrap();
    let output = export(&home, &destination);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not empty"));
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"keep");

    let inside = home.root.join("state/analytics-store/export");
    let output = export(&home, &inside);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("inside the source store"));
}

#[test]
fn export_and_reset_refuse_a_busy_writer_without_mutation() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let namespace = fs::read_to_string(state.join("analytics-namespace"))
        .unwrap()
        .trim()
        .to_owned();
    let bundle = home.root.join("analytics-export");
    assert!(export(&home, &bundle).status.success());
    let before = recursive_listing(&state);
    let lock = fs::OpenOptions::new()
        .write(true)
        .open(state.join("analytics-store/analytics.lock"))
        .unwrap();
    lock.lock().unwrap();

    let destination = home.root.join("blocked-export");
    let export = export(&home, &destination);
    assert!(!export.status.success());
    assert!(String::from_utf8_lossy(&export.stderr).contains("writer busy"));
    assert!(!destination.exists());

    let reset = reset(&home, &["--confirm", &namespace, "--without-export"]);
    assert!(!reset.status.success());
    assert!(String::from_utf8_lossy(&reset.stderr).contains("writer busy"));
    assert_eq!(recursive_listing(&state), before);
    lock.unlock().unwrap();
}

#[test]
fn reset_refuses_a_tampered_export_without_mutation() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let namespace = fs::read_to_string(state.join("analytics-namespace"))
        .unwrap()
        .trim()
        .to_owned();
    let bundle = home.root.join("analytics-export");
    assert!(export(&home, &bundle).status.success());
    fs::write(bundle.join("analytics-namespace"), "tampered\n").unwrap();
    let before = recursive_listing(&state);

    let output = reset(
        &home,
        &[
            "--confirm",
            &namespace,
            "--verified-export",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("verified export"));
    assert_eq!(recursive_listing(&state), before);
}

#[test]
fn reset_refuses_an_old_generation_after_reset_and_republication() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let namespace = fs::read_to_string(state.join("analytics-namespace"))
        .unwrap()
        .trim()
        .to_owned();
    let old_bundle = home.root.join("old-export");
    assert!(export(&home, &old_bundle).status.success());
    assert!(
        reset(&home, &["--confirm", &namespace, "--without-export"])
            .status
            .success()
    );
    let transcript = home
        .root
        .join("configured-claude/projects/fixture/00000000-0000-4000-8000-000000000001.jsonl");
    let mut transcript_file = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    use std::io::Write as _;
    writeln!(transcript_file, "{}", serde_json::json!({"type":"assistant", "uuid":"late-assistant", "sessionId":"00000000-0000-4000-8000-000000000001", "timestamp":"2026-10-03T08:02:01Z", "message":{"id":"late-message", "role":"assistant", "model":"claude-sonnet-4-5", "content":[{"type":"text", "text":"Synthetic late response"}], "usage":{"input_tokens":30, "output_tokens":7}}})).unwrap();
    drop(transcript_file);
    assert!(materialize(&home).status.success());
    let old_receipt: Value =
        serde_json::from_slice(&fs::read(old_bundle.join("export.json")).unwrap()).unwrap();
    let current_manifest: Value = serde_json::from_slice(
        &fs::read(state.join("analytics-store/analytics/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        old_receipt["manifest_generation"],
        current_manifest["generation"]
    );
    let before = file_bytes(&state);

    let output = reset(
        &home,
        &[
            "--confirm",
            &namespace,
            "--verified-export",
            old_bundle.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("current published snapshot"));
    assert_eq!(file_bytes(&state), before);
}

#[test]
fn reset_refuses_an_export_inside_the_store_without_deleting_it() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let namespace = fs::read_to_string(state.join("analytics-namespace"))
        .unwrap()
        .trim()
        .to_owned();
    let bundle = home.root.join("analytics-export");
    assert!(export(&home, &bundle).status.success());
    let inside = state.join("analytics-store/backup");
    fs::rename(&bundle, &inside).unwrap();
    let before = file_bytes(&state);

    let output = reset(
        &home,
        &[
            "--confirm",
            &namespace,
            "--verified-export",
            inside.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside the analytics store"));
    assert_eq!(file_bytes(&state), before);
}

#[test]
fn export_and_reset_refuse_a_foreign_live_namespace_without_mutation() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let state = home.root.join("state");
    let namespace_path = state.join("analytics-namespace");
    let namespace = fs::read_to_string(&namespace_path)
        .unwrap()
        .trim()
        .to_owned();
    let foreign = format!("origin-{}\n", "f".repeat(64));
    fs::write(&namespace_path, &foreign).unwrap();
    let before = file_bytes(&state);

    let destination = home.root.join("foreign-export");
    let output = export(&home, &destination);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("do not belong"));
    assert!(!destination.exists());

    let output = reset(&home, &["--confirm", foreign.trim(), "--without-export"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("do not belong"));
    assert_eq!(file_bytes(&state), before);
    assert_ne!(foreign.trim(), namespace);
}

#[test]
fn reset_requires_matching_confirmation_and_current_verified_export() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let namespace = fs::read_to_string(home.root.join("state/analytics-namespace"))
        .unwrap()
        .trim()
        .to_owned();
    let handles_path = home
        .root
        .join("state")
        .join(semon_asi::egress::HANDLES_FILE);
    drop(semon_asi::egress::Handles::open(&home.root.join("state"), &namespace).unwrap());
    let mapping_bytes = fs::read(&handles_path).unwrap();
    assert!(coverage(&home, false).status.success());
    let bundle = home.root.join("analytics-export");
    assert!(export(&home, &bundle).status.success());
    assert!(!bundle.join(semon_asi::egress::HANDLES_FILE).exists());

    let bundle_arg = bundle.to_str().unwrap();
    let wrong = reset(
        &home,
        &["--confirm", "origin-wrong", "--verified-export", bundle_arg],
    );
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("exactly match"));

    let reset = reset(
        &home,
        &["--confirm", &namespace, "--verified-export", bundle_arg],
    );
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    assert!(home.root.join("state/analytics-namespace").is_file());
    assert_eq!(fs::read(&handles_path).unwrap(), mapping_bytes);
    assert!(
        home.root
            .join("state/analytics-store/analytics.lock")
            .is_file()
    );
    assert!(!home.root.join("state/analytics-store/analytics").exists());
}

#[test]
fn reset_requires_one_export_choice_and_can_remove_the_namespace_explicitly() {
    let home = fixture::Fixture::new();
    assert!(materialize(&home).status.success());
    let namespace = fs::read_to_string(home.root.join("state/analytics-namespace"))
        .unwrap()
        .trim()
        .to_owned();
    let handles_path = home
        .root
        .join("state")
        .join(semon_asi::egress::HANDLES_FILE);
    drop(semon_asi::egress::Handles::open(&home.root.join("state"), &namespace).unwrap());
    let missing = reset(&home, &["--confirm", &namespace]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("exactly one"));

    let reset = reset(
        &home,
        &[
            "--confirm",
            &namespace,
            "--without-export",
            "--new-namespace",
        ],
    );
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    assert!(!home.root.join("state/analytics-namespace").exists());
    assert!(!handles_path.exists());
    assert!(
        home.root
            .join("state/analytics-store/analytics.lock")
            .is_file()
    );
    assert!(!home.root.join("state/analytics-store/analytics").exists());
    assert!(materialize(&home).status.success());
    let next_namespace = fs::read_to_string(home.root.join("state/analytics-namespace")).unwrap();
    assert_ne!(next_namespace.trim(), namespace);
    drop(
        semon_asi::egress::Handles::open(&home.root.join("state"), next_namespace.trim()).unwrap(),
    );
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

#[test]
fn coverage_refuses_symlinked_or_namespace_less_stores_without_mutation() {
    let home = fixture::Fixture::new();
    let state = home.root.join("state");
    let materialized = materialize(&home);
    assert!(
        materialized.status.success(),
        "{}",
        String::from_utf8_lossy(&materialized.stderr)
    );

    let namespace = state.join("analytics-namespace");
    let kept = home.root.join("kept-namespace");
    fs::rename(&namespace, &kept).unwrap();
    let before = recursive_listing(&state);
    let output = coverage(&home, false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("namespace"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(recursive_listing(&state), before);
    fs::rename(&kept, &namespace).unwrap();

    let store = state.join("analytics-store");
    let moved = home.root.join("moved-store");
    fs::rename(&store, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &store).unwrap();
    let before = recursive_listing(&state);
    let output = coverage(&home, false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a real directory"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(recursive_listing(&state), before);
}
