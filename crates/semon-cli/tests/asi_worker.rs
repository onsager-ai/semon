#![cfg(all(feature = "asi-query", unix))]

use std::io::Write;
use std::process::{Command, Stdio};

use semon_asi::query::{DATA_NOTICE, Request, run};
use semon_asi::{Manifest, Store};
use serde_json::json;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    std::fs::write(
        dir.path().join("analytics/manifest.json"),
        serde_json::to_vec(&Manifest::empty()).unwrap(),
    )
    .unwrap();
    (dir, store)
}

#[test]
fn real_worker_success_errors_and_deadline() {
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_semon"));
    let (dir, _store) = store();
    let mut request = Request::new("SELECT ? AS value");
    request.params = vec![json!(42)];
    request.timeout_ms = 10_000;
    let result = run(binary, dir.path(), &request).unwrap();
    assert_eq!(result.rows, vec![vec![json!(42)]]);
    assert_eq!(result.snapshot, "0");
    assert_eq!(result.data_notice, DATA_NOTICE);
    request.query = "SELECT getenv('HOME')".into();
    assert_eq!(
        run(binary, dir.path(), &request).unwrap_err().error.code,
        "not_allowed"
    );
    request.query =
        "SELECT sum(a.i*b.i) FROM range(1000000000) a(i) CROSS JOIN range(1000000000) b(i)".into();
    // Whether expiry occurs during startup or execution, this query cannot
    // complete within the budget; the parent must return no partial result.
    request.timeout_ms = 1;
    assert_eq!(
        run(binary, dir.path(), &request).unwrap_err().error.code,
        "query_timeout"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess runner invoked by worker_dies_with_runner"]
fn parent_death_runner() {
    let root = std::env::var_os("SEMON_QUERY_PARENT_DEATH_ROOT").unwrap();
    let mut request = Request::new(
        "SELECT sum(a.i*b.i) FROM range(1000000000) a(i) CROSS JOIN range(1000000000) b(i)",
    );
    request.timeout_ms = 10_000;
    let _ = run(
        std::path::Path::new(env!("CARGO_BIN_EXE_semon")),
        std::path::Path::new(&root),
        &request,
    );
}

#[cfg(target_os = "linux")]
#[test]
fn worker_dies_with_runner() {
    use std::time::{Duration, Instant};

    // Discover the real worker through /proc after the runner spawns it.
    let (dir, _store) = store();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "parent_death_runner",
            "--ignored",
            "--test-threads=1",
        ])
        .env("SEMON_QUERY_PARENT_DEATH_ROOT", dir.path())
        .stdout(Stdio::null());
    let runner = command.spawn().unwrap();
    struct Cleanup(std::process::Child, Option<libc::pid_t>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(pid) = self.1 {
                // SAFETY: test-owned worker PID, kept in this guard until exit.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut cleanup = Cleanup(runner, None);
    let began = Instant::now();
    let worker = loop {
        // Rust's test harness runs the probe on a test thread; Linux records
        // children on the thread that forked, not necessarily the leader.
        let worker = std::fs::read_dir(format!("/proc/{}/task", cleanup.0.id()))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .find_map(|children| {
                children
                    .split_whitespace()
                    .find_map(|p| p.parse::<libc::pid_t>().ok())
            });
        if let Some(pid) = worker {
            break pid;
        }
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "runner did not spawn worker"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    };
    cleanup.1 = Some(worker);
    // Wait for exec: PDEATHSIG and inherited admission FD must already be set.
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_semon")).unwrap();
    while std::fs::read_link(format!("/proc/{worker}/exe"))
        .ok()
        .as_ref()
        != Some(&binary)
    {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "worker did not exec"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    // SAFETY: this is our subprocess; stopping it keeps its own watchdog/CPU
    // limit from satisfying the test in place of the kernel parent-death signal.
    assert_eq!(unsafe { libc::kill(worker, libc::SIGSTOP) }, 0);
    let state = |pid| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(") ")
                    .and_then(|(_, tail)| tail.chars().next())
            })
    };
    while state(worker) != Some('T') {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "worker did not stop"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    // The worker holds the same flock after exec, independently of the parent.
    let lock = std::fs::File::open(dir.path().join("analytics/query-0.lock")).unwrap();
    use std::os::fd::AsRawFd;
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EWOULDBLOCK)
    );
    cleanup.0.kill().unwrap();
    cleanup.0.wait().unwrap();
    let killed = Instant::now();
    while !matches!(state(worker), None | Some('Z' | 'X')) {
        assert!(
            killed.elapsed() < Duration::from_secs(30),
            "orphan worker survived runner SIGKILL"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    // Zombies have exited and hold no FDs; PID 1 may reap them asynchronously.
    cleanup.1 = None;
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
}

#[test]
fn direct_worker_protocol_limits_are_not_caller_fields() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_semon"))
        .args(["asi", "query-worker"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"root":"/unused","request":{"query":"SELECT 1","memory":"unlimited"}}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "invalid_argument");
    assert_eq!(error.as_object().unwrap().len(), 1);
}
