//! End-to-end acceptance test for `semon forget --forensic`, driven through
//! the actual compiled CLI binary as separate OS processes.
//!
//! This is the property issue #20 exists to make reachable: #17 proved (at
//! the store layer, via `DROP TABLE`) that `semon log` renders byte-for-byte
//! identically on a store whose forensic region is gone, but nothing could
//! produce that state through a supported path. This test proves `semon
//! forget --forensic` is that path — and, unlike the store-layer test, it
//! must reopen the store through a *fresh process* to prove the deletion
//! survives `TraceStore::open`'s unconditional `CREATE TABLE IF NOT EXISTS`
//! (see `docs/design/forensic-retention-and-exposure.md`'s "Measured
//! consequence" section), not just outlive a single connection.
//!
//! No real session data is used anywhere here — every capture below is a
//! synthetic fixture built directly against the `semon-store` library API.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use semon_store::{
    AuthoredBy, NewOccurrence, NewRawCarrierRecord, RepoSource, SemanticCore, TraceStore,
};

/// A unique scratch path under the OS temp directory, never the default
/// store location — these tests never touch `~/.local/share/semon` or run
/// any installer.
fn unique_store_path(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "semon-forget-cli-test-{label}-{}-{unique}.sqlite3",
        std::process::id()
    ))
}

fn semantic(content: &str) -> SemanticCore {
    SemanticCore::from_value(serde_json::json!({"kind": "intent", "content": content})).unwrap()
}

#[test]
fn shared_claude_line_is_output_once_and_trace_forget_keeps_the_log() {
    let store_path = unique_store_path("shared-claude-line");
    let trace_id = {
        let mut store = TraceStore::open(&store_path).unwrap();
        let cores = [semantic("first"), semantic("second"), semantic("third")];
        let blocks = cores
            .iter()
            .enumerate()
            .map(|(index, core)| {
                (
                    core.clone(),
                    NewOccurrence {
                        session: "claude-session",
                        sequence: index as i64,
                        timestamp: 10,
                        repo: "repo",
                        repo_source: RepoSource::ExplicitOverride,
                        parent_sequence: if index == 0 {
                            None
                        } else {
                            Some(index as i64 - 1)
                        },
                        agent: None,
                        authored_by: AuthoredBy::Agent,
                    },
                )
            })
            .collect::<Vec<_>>();
        store
            .capture_line(
                NewRawCarrierRecord::new("claude", b"shared line\n"),
                "claude-session",
                0,
                10,
                &blocks,
            )
            .unwrap();
        cores[1].trace_id().unwrap().to_string()
    };
    let before_log = run_semon(&store_path, &["log"]);
    assert!(before_log.status.success());
    let forensic = run_semon(&store_path, &["forensic", "--trace", &trace_id]);
    assert!(forensic.status.success());
    assert_eq!(forensic.stdout, b"shared line\n");
    let by_session = run_semon(&store_path, &["forensic", "--session", "claude-session"]);
    assert!(by_session.status.success());
    assert_eq!(by_session.stdout, b"shared line\n");

    let forget = run_semon(
        &store_path,
        &["forget", "--forensic", "--trace", &trace_id, "--yes"],
    );
    assert!(forget.status.success());
    assert!(String::from_utf8_lossy(&forget.stderr).contains("whole raw source line"));
    assert!(
        String::from_utf8_lossy(&forget.stdout).contains("permanently deleted 1 raw record(s)")
    );
    let after_log = run_semon(&store_path, &["log"]);
    assert!(after_log.status.success());
    assert_eq!(after_log.stdout, before_log.stdout);
    let _ = std::fs::remove_file(store_path);
}

/// Runs the compiled `semon` binary as a fresh child process against
/// `store`, with the given arguments (excluding `--store`, which this always
/// appends). `stdin_is_tty` false pipes a closed stdin, guaranteeing
/// `IsTerminal` sees a non-terminal regardless of how the test harness
/// itself was launched.
fn run_semon(store: &Path, args: &[&str]) -> std::process::Output {
    let binary = env!("CARGO_BIN_EXE_semon");
    let mut command = Command::new(binary);
    command
        .args(args)
        .arg("--store")
        .arg(store)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.output().expect("failed to run semon binary")
}

/// Seeds a store with three synthetic occurrences across two sessions, one
/// of whose raw records carries a unique sentinel string. Returns the day
/// (`YYYY-MM-DD`, UTC) every occurrence's timestamp falls strictly before,
/// so `--before` that date selects everything.
fn seed_synthetic_store(store_path: &Path, sentinel: &[u8]) -> String {
    let mut store = TraceStore::open(store_path).unwrap();

    store
        .capture(
            &semantic("synthetic goal: compile the report"),
            NewRawCarrierRecord::new("codex", sentinel),
            NewOccurrence {
                session: "synthetic-session-a",
                sequence: 0,
                timestamp: 1_000,
                repo: "semon",
                repo_source: RepoSource::GitRemote,
                parent_sequence: None,
                agent: None,
                authored_by: AuthoredBy::Human,
            },
        )
        .unwrap();
    store
        .capture(
            &semantic("synthetic outcome: report compiled"),
            NewRawCarrierRecord::new("codex", b"synthetic raw bytes, no sentinel here"),
            NewOccurrence {
                session: "synthetic-session-a",
                sequence: 1,
                timestamp: 2_000,
                repo: "semon",
                repo_source: RepoSource::GitRemote,
                parent_sequence: Some(0),
                agent: None,
                authored_by: AuthoredBy::Human,
            },
        )
        .unwrap();
    store
        .capture(
            &semantic("synthetic goal: unrelated session"),
            NewRawCarrierRecord::new("codex", b"synthetic raw bytes, session b"),
            NewOccurrence {
                session: "synthetic-session-b",
                sequence: 0,
                timestamp: 3_000,
                repo: "semon",
                repo_source: RepoSource::GitRemote,
                parent_sequence: None,
                agent: None,
                authored_by: AuthoredBy::Human,
            },
        )
        .unwrap();

    drop(store);

    // Every timestamp above is nanoseconds since the Unix epoch, tiny and
    // far in the past, so any date comfortably after 1970 is "after
    // everything".
    "2099-01-01".to_owned()
}

fn file_contains(path: &Path, needle: &[u8]) -> bool {
    let bytes = std::fs::read(path).unwrap();
    bytes.windows(needle.len()).any(|window| window == needle)
}

/// Seeds a store with one trace captured under *two* different sessions,
/// with distinct raw bytes per capture — the fixture shape the over-delete
/// bug needs to be caught at all. A fixture where every session captures
/// distinct content (as [`seed_synthetic_store`] does) cannot detect
/// trace-scoped selection being wrong, because trace-scoped and
/// capture-scoped selection agree whenever no trace recurs across sessions.
fn seed_shared_trace_store(store_path: &Path) {
    let mut store = TraceStore::open(store_path).unwrap();
    let shared = semantic("synthetic goal: identical content in two sessions");

    store
        .capture(
            &shared,
            NewRawCarrierRecord::new("codex", b"session-a's own capture"),
            NewOccurrence {
                session: "synthetic-session-a",
                sequence: 0,
                timestamp: 1_000,
                repo: "semon",
                repo_source: RepoSource::GitRemote,
                parent_sequence: None,
                agent: None,
                authored_by: AuthoredBy::Human,
            },
        )
        .unwrap();
    store
        .capture(
            &shared,
            NewRawCarrierRecord::new("codex", b"session-b's own capture"),
            NewOccurrence {
                session: "synthetic-session-b",
                sequence: 0,
                timestamp: 2_000,
                repo: "semon",
                repo_source: RepoSource::GitRemote,
                parent_sequence: None,
                agent: None,
                authored_by: AuthoredBy::Human,
            },
        )
        .unwrap();

    drop(store);
}

fn seed_raw_only_store(store_path: &Path) -> String {
    let mut store = TraceStore::open(store_path).unwrap();
    let (start, _) = semon_store::day_bounds_ns(2026, 9, 20);
    store
        .capture_raw_only(
            "codex",
            b"synthetic unprojected raw line\n",
            "raw-only-session",
            12,
            start + 1,
        )
        .unwrap();
    drop(store);

    semantic("projected content absent from this store")
        .trace_id()
        .unwrap()
        .to_string()
}

#[test]
fn forget_forensic_makes_the_raw_dropped_state_reachable_through_the_cli() {
    const SENTINEL: &[u8] = b"SEMON-FORGET-SENTINEL-8f3a";

    let store_path = unique_store_path("acceptance");
    let after_everything = seed_synthetic_store(&store_path, SENTINEL);

    // The sentinel must actually be on disk before forget runs, or this test
    // could not detect its absence meaning anything.
    assert!(
        file_contains(&store_path, SENTINEL),
        "test setup failure: sentinel must be present in the raw file bytes before forget"
    );

    // Output A: render the log through the CLI, in its own process, before
    // any forgetting happens.
    let before = run_semon(&store_path, &["log"]);
    assert!(
        before.status.success(),
        "semon log (before) failed: {}",
        String::from_utf8_lossy(&before.stderr)
    );
    let output_a = before.stdout;
    assert!(!output_a.is_empty(), "log output must not be empty");

    // Delete every raw record whose occurrences are all before a date well
    // after everything captured above.
    let forget = run_semon(
        &store_path,
        &[
            "forget",
            "--forensic",
            "--before",
            &after_everything,
            "--yes",
        ],
    );
    assert!(
        forget.status.success(),
        "semon forget --forensic failed: {}",
        String::from_utf8_lossy(&forget.stderr)
    );
    let forget_stdout = String::from_utf8_lossy(&forget.stdout);
    assert!(
        forget_stdout.contains("permanently deleted 3 raw record(s)"),
        "expected a count of 3 deleted records, got: {forget_stdout}"
    );

    // Reopen through the CLI — a fresh process, so TraceStore::open's own
    // `CREATE TABLE IF NOT EXISTS raw_carrier_records` runs again — and
    // render the log a second time.
    let after = run_semon(&store_path, &["log"]);
    assert!(
        after.status.success(),
        "semon log (after) failed: {}",
        String::from_utf8_lossy(&after.stderr)
    );
    let output_b = after.stdout;

    assert_eq!(
        output_a, output_b,
        "semon log must render byte-for-byte identically after forget --forensic, \
         reopened through a fresh process"
    );

    // The deletion must have survived the reopen: raw records are still
    // gone, not silently healed back by CREATE TABLE IF NOT EXISTS.
    let store = TraceStore::open(&store_path).unwrap();
    let remaining_raw: i64 = {
        // Route through the public API rather than reaching into the
        // connection: forensic reads over every occurrence's trace must
        // come back empty.
        let by_session_a = store
            .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
                "synthetic-session-a",
            ))
            .unwrap();
        let by_session_b = store
            .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
                "synthetic-session-b",
            ))
            .unwrap();
        (by_session_a.len() + by_session_b.len()) as i64
    };
    assert_eq!(
        remaining_raw, 0,
        "all raw records must still be gone after reopening through the CLI"
    );
    drop(store);

    // And the sentinel bytes are actually gone from the file on disk, not
    // merely unindexed on a freelist page.
    assert!(
        !file_contains(&store_path, SENTINEL),
        "sentinel must be absent from the raw file bytes after forget + reopen"
    );

    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn forget_forensic_with_no_selector_errors_via_the_cli() {
    let store_path = unique_store_path("no-selector");
    seed_synthetic_store(&store_path, b"unused sentinel");

    let output = run_semon(&store_path, &["forget", "--forensic"]);

    assert!(
        !output.status.success(),
        "a bare `semon forget --forensic` with no selector must fail, not silently empty the region"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("exactly one of --trace, --session, or --before"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn forget_forensic_without_yes_on_non_tty_stdin_errors_via_the_cli() {
    let store_path = unique_store_path("non-tty");
    seed_synthetic_store(&store_path, b"unused sentinel");

    // run_semon always pipes a closed (non-terminal) stdin and no --yes is
    // passed here.
    let output = run_semon(
        &store_path,
        &["forget", "--forensic", "--session", "synthetic-session-a"],
    );

    assert!(
        !output.status.success(),
        "non-interactive stdin without --yes must fail rather than proceed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a terminal"),
        "unexpected stderr: {stderr}"
    );

    // Confirm nothing was deleted.
    let log_after = run_semon(&store_path, &["log"]);
    assert!(log_after.status.success());
    let store = TraceStore::open(&store_path).unwrap();
    let remaining = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "synthetic-session-a",
        ))
        .unwrap();
    assert_eq!(
        remaining.len(),
        2,
        "no raw records should have been deleted"
    );
    drop(store);

    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn forget_forensic_by_session_leaves_the_other_session_intact_via_the_cli() {
    let store_path = unique_store_path("session-selective");
    seed_synthetic_store(&store_path, b"unused sentinel");

    let output = run_semon(
        &store_path,
        &[
            "forget",
            "--forensic",
            "--session",
            "synthetic-session-a",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "forget by session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = TraceStore::open(&store_path).unwrap();
    let session_a = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "synthetic-session-a",
        ))
        .unwrap();
    assert!(session_a.is_empty(), "session-a's raw records must be gone");
    let session_b = store
        .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
            "synthetic-session-b",
        ))
        .unwrap();
    assert_eq!(
        session_b.len(),
        1,
        "session-b's raw record must be left intact"
    );
    // The log is unaffected regardless of which session's raw records were
    // forgotten.
    assert_eq!(
        store.log(&semon_store::LogFilter::default()).unwrap().len(),
        3
    );
    drop(store);

    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn forget_forensic_by_session_leaves_a_shared_traces_other_session_intact_via_the_cli() {
    // Shaped around the measured failure: over a real week of capture,
    // forgetting one session by trace_id deleted 2,386 raw rows belonging
    // to 41 *other* sessions, because 152 traces recurred across sessions.
    // This fixture reproduces that shape at minimum scale: one trace, two
    // sessions.
    let store_path = unique_store_path("shared-trace-session-selective");
    seed_shared_trace_store(&store_path);

    let output = run_semon(
        &store_path,
        &[
            "forget",
            "--forensic",
            "--session",
            "synthetic-session-a",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "forget by session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("permanently deleted 1 raw record(s)"),
        "must delete exactly session-a's own one raw record, got: {stdout}"
    );

    let store = TraceStore::open(&store_path).unwrap();
    let shared_id = semantic("synthetic goal: identical content in two sessions")
        .trace_id()
        .unwrap();
    let remaining = store.fetch_raw_carrier_records(&shared_id).unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "session-b's copy of the shared trace must survive"
    );
    assert_eq!(
        remaining[0].bytes(),
        b"session-b's own capture",
        "the surviving raw record must be session-b's own bytes, field for field"
    );
    drop(store);

    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn forensic_by_session_on_a_shared_trace_returns_only_that_sessions_capture_via_the_cli() {
    // The read-side counterpart: `semon forensic --session` must not
    // over-read a shared trace's other-session capture either.
    let store_path = unique_store_path("shared-trace-forensic-selective");
    seed_shared_trace_store(&store_path);
    let out_path = std::env::temp_dir().join(format!(
        "semon-forget-cli-test-shared-trace-forensic-{}.out",
        std::process::id()
    ));

    let output = run_semon(
        &store_path,
        &[
            "forensic",
            "--session",
            "synthetic-session-a",
            "--out",
            out_path.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "forensic by session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let contents = std::fs::read_to_string(&out_path).unwrap();
    assert!(contents.contains("session-a's own capture"));
    assert!(!contents.contains("session-b's own capture"));

    let _ = std::fs::remove_file(&store_path);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn forensic_day_and_forget_before_reach_an_unprojected_raw_row_via_the_cli() {
    let store_path = unique_store_path("raw-only-time-selectors");
    seed_raw_only_store(&store_path);
    let out_path = std::env::temp_dir().join(format!(
        "semon-forget-cli-test-raw-only-{}.out",
        std::process::id()
    ));

    let forensic = run_semon(
        &store_path,
        &[
            "forensic",
            "--day",
            "2026-09-20",
            "--out",
            out_path.to_str().unwrap(),
        ],
    );
    assert!(
        forensic.status.success(),
        "forensic by raw timestamp failed: {}",
        String::from_utf8_lossy(&forensic.stderr)
    );
    assert_eq!(
        std::fs::read(&out_path).unwrap(),
        b"synthetic unprojected raw line\n"
    );

    let forget = run_semon(
        &store_path,
        &["forget", "--forensic", "--before", "2026-09-21", "--yes"],
    );
    assert!(
        forget.status.success(),
        "forget by raw timestamp failed: {}",
        String::from_utf8_lossy(&forget.stderr)
    );
    assert!(
        String::from_utf8_lossy(&forget.stdout).contains("permanently deleted 1 raw record(s)")
    );

    let store = TraceStore::open(&store_path).unwrap();
    assert!(
        store
            .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
                "raw-only-session"
            ))
            .unwrap()
            .is_empty()
    );

    let _ = std::fs::remove_file(&store_path);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn trace_selectors_explain_that_unprojected_rows_are_out_of_scope() {
    let store_path = unique_store_path("raw-only-trace-selector");
    let absent_trace = seed_raw_only_store(&store_path);
    let out_path = std::env::temp_dir().join(format!(
        "semon-forget-cli-test-raw-only-trace-{}.out",
        std::process::id()
    ));

    let forensic = run_semon(
        &store_path,
        &[
            "forensic",
            "--trace",
            &absent_trace,
            "--out",
            out_path.to_str().unwrap(),
        ],
    );
    assert!(forensic.status.success());
    assert!(std::fs::read(&out_path).unwrap().is_empty());
    assert!(
        String::from_utf8_lossy(&forensic.stderr)
            .contains("unprojected raw records have no trace links")
    );

    let forget = run_semon(
        &store_path,
        &["forget", "--forensic", "--trace", &absent_trace, "--yes"],
    );
    assert!(forget.status.success());
    assert!(
        String::from_utf8_lossy(&forget.stderr)
            .contains("unprojected raw records have no trace links")
    );
    assert!(String::from_utf8_lossy(&forget.stdout).contains("no matching raw records"));

    let store = TraceStore::open(&store_path).unwrap();
    assert_eq!(
        store
            .fetch_raw_carrier_records_for_occurrences(semon_store::OccurrenceSelector::Session(
                "raw-only-session"
            ))
            .unwrap()
            .len(),
        1
    );

    let _ = std::fs::remove_file(&store_path);
    let _ = std::fs::remove_file(&out_path);
}
