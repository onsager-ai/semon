//! `semon push` against `semon receive`, both the real binary, on loopback:
//! a synthetic home with one Claude session and one Codex run is mirrored,
//! redacted, into the receiver's directory, and `semon sessions --machines
//! DIR` then shows that machine and both its sessions; a rewrite is sent again with
//! `replace`; a copy lost on the receiver is recovered through a 409; and a
//! revoked token is refused with 401.

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde_json::json;

const SECRET: &str = "sk-ant-api03-SECRETSECRETSECRETSECRETSECRET";
const LOG: &str = "claude/projects/-work-app/lane.jsonl";
const ROLLOUT: &str = "codex/sessions/2026/09/29/rollout-2026-09-29T10-00-00-cx.jsonl";

fn semon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_semon"))
}

struct Fixture {
    root: PathBuf,
    receiver: Option<Child>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.receiver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "semon-receive-push-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fixture = Self {
            root,
            receiver: None,
        };
        fixture.write("home/proc/locks", b"");
        fixture.write("home/proc/sys/kernel/hostname", b"laptop\n");
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn append(&self, relative: &str, bytes: &[u8]) {
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(self.path(relative))
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    fn dir(&self) -> PathBuf {
        self.path("receiver")
    }

    /// `semon receive …` with no environment beyond the synthetic home.
    fn receive(&self, arguments: &[&str]) -> Output {
        semon()
            .arg("receive")
            .args(arguments)
            .env("HOME", self.path("home"))
            .output()
            .unwrap()
    }

    /// Starts `semon receive` on a free loopback port and returns its URL.
    fn start_receiver(&mut self) -> String {
        let mut child = semon()
            .args(["receive", "--listen", "127.0.0.1:0", "--dir"])
            .arg(self.dir())
            .env("HOME", self.path("home"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        self.receiver = Some(child);
        let (lines, received) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                eprintln!("[receiver] {line}");
                // Kept draining after the test stops listening, so the
                // receiver never blocks on stderr.
                let _ = lines.send(line);
            }
        });
        loop {
            let line = received
                .recv_timeout(Duration::from_secs(60))
                .expect("the receiver says where it listens");
            if let Some(rest) = line.strip_prefix("semon receive: listening on ") {
                return rest.split(',').next().unwrap().to_owned();
            }
        }
    }

    /// `semon push --to URL` from the synthetic home.
    fn push(&self, url: &str) -> Output {
        let home = self.path("home");
        semon()
            .args(["push", "--to", url, "--token-file"])
            .arg(self.path("token"))
            .arg("--state")
            .arg(self.path("state/push.json"))
            .arg("--claude-home")
            .arg(home.join("claude"))
            .arg("--codex-home")
            .arg(home.join("codex"))
            .arg("--proc-root")
            .arg(home.join("proc"))
            .arg("--cache")
            .arg(self.path("state/index.json"))
            .env("HOME", &home)
            .env("XDG_STATE_HOME", self.path("state"))
            .output()
            .unwrap()
    }

    /// `semon sessions --model-json --all --machines DIR --no-local`, with
    /// the reader's own homes and cache in the fixture.
    fn model(&self) -> serde_json::Value {
        let viewer = self.path("viewer");
        let output = semon()
            .args([
                "sessions",
                "--model-json",
                "--all",
                "--no-local",
                "--machines",
            ])
            .arg(self.dir())
            .arg("--claude-home")
            .arg(viewer.join("claude"))
            .arg("--codex-home")
            .arg(viewer.join("codex"))
            .arg("--proc-root")
            .arg(self.path("home/proc"))
            .arg("--cache")
            .arg(viewer.join("state/index.json"))
            .env("HOME", &viewer)
            .env("XDG_STATE_HOME", viewer.join("state"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_ok(&output, "the model over DIR");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// The receiver's copy of a file under the home.
    fn copy(&self, relative: &str) -> Vec<u8> {
        fs::read(self.dir().join("machines/laptop").join(relative)).unwrap_or_default()
    }

    /// The file under the home as a push sends it.
    fn redacted(&self, relative: &str) -> Vec<u8> {
        let mut bytes = fs::read(self.path("home").join(relative)).unwrap();
        semon_push::redact::redact(&mut bytes);
        bytes
    }
}

fn line(value: serde_json::Value) -> String {
    value.to_string() + "\n"
}

fn claude(text: &str) -> String {
    line(json!({
        "type": "user", "timestamp": "2026-09-29T10:00:00.000Z", "sessionId": "lane",
        "cwd": "/work/app", "message": {"role": "user", "content": text}
    }))
}

fn assistant(text: &str) -> String {
    line(json!({
        "type": "assistant", "timestamp": "2026-09-29T10:00:01.000Z", "sessionId": "lane",
        "cwd": "/work/app",
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}
    }))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_ok(output: &Output, what: &str) {
    assert!(output.status.success(), "{what} failed: {}", stderr(output));
}

#[cfg(unix)]
fn private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn private(_path: &Path) {}

#[test]
fn push_mirrors_a_home_into_semon_receive() {
    let mut fixture = Fixture::new();
    let dir = fixture.dir();

    // A token for the machine, printed once.
    let added = fixture.receive(&["token", "add", "laptop", "--dir", dir.to_str().unwrap()]);
    assert_ok(&added, "token add");
    let token = String::from_utf8(added.stdout).unwrap().trim().to_owned();
    assert_eq!(token.len(), 43, "{token}");
    fixture.write("token", token.as_bytes());
    private(&fixture.path("token"));
    let listed = fixture.receive(&["token", "list", "--dir", dir.to_str().unwrap()]);
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "laptop\n");
    assert!(
        !fs::read_to_string(dir.join("tokens"))
            .unwrap()
            .contains(&token)
    );

    let url = fixture.start_receiver();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");

    // One Claude session, with a secret in it, and one Codex run.
    let log = format!("home/{LOG}");
    fixture.write(
        &log,
        (claude(&format!("deploy with {SECRET} please")) + &assistant("deployed")).as_bytes(),
    );
    let rollout = format!("home/{ROLLOUT}");
    fixture.write(
        &rollout,
        [
            line(json!({"timestamp": "2026-09-29T10:00:00.000Z", "type": "session_meta",
                "payload": {"id": "cx", "cwd": "/work/app", "originator": "codex_exec", "thread_source": "user"}})),
            line(json!({"timestamp": "2026-09-29T10:00:01.000Z", "type": "response_item",
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Fix the lexer"}]}})),
            line(json!({"timestamp": "2026-09-29T10:00:02.000Z", "type": "response_item",
                "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fixed"}]}})),
        ]
        .concat()
        .as_bytes(),
    );
    fixture.write("home/claude/settings.json", b"{\"NEVER\":\"SENT\"}");

    let pushed = fixture.push(&url);
    assert_ok(&pushed, "the first push");
    assert_eq!(fixture.copy(LOG), fixture.redacted(LOG));
    assert!(!String::from_utf8_lossy(&fixture.copy(LOG)).contains(SECRET));
    assert_eq!(fixture.copy(ROLLOUT), fixture.redacted(ROLLOUT));
    assert!(fixture.copy(ROLLOUT).len() > 100);
    let machine = dir.join("machines/laptop");
    assert!(!machine.join("claude/settings.json").exists());
    let facts = semon_sessions::read_facts(&machine.join("facts.json")).unwrap();
    assert_eq!(facts.hostname, "laptop");

    // The viewer over DIR shows the pushed machine, up, with both its
    // sessions. With one machine the model is that machine's own: `machine`,
    // and no `machines` list to pick it from.
    let model = fixture.model();
    assert_eq!(model["machine"]["id"], "laptop", "{}", model["machine"]);
    assert_eq!(model["machine"]["up"], true);
    assert!(
        model["machines"]
            .as_array()
            .is_none_or(|machines| machines.len() == 1)
    );
    for session in ["lane", "cx"] {
        let found = &model["sessions"][session];
        assert!(found.is_object(), "{session}: {}", model["sessions"]);
        if let Some(machine) = found.get("machine") {
            assert_eq!(machine, "laptop");
        }
    }

    // Rewritten shorter: sent again whole, with replace.
    fixture.write(&log, claude("rewritten").as_bytes());
    let pushed = fixture.push(&url);
    assert_ok(&pushed, "the push after a rewrite");
    assert!(
        stderr(&pushed).contains("1 sent again whole"),
        "{}",
        stderr(&pushed)
    );
    assert_eq!(fixture.copy(LOG), claude("rewritten").into_bytes());

    // The receiver loses its copy: the next append gets a 409 with length
    // 0, and the client sends the file again from the start.
    fs::remove_file(machine.join(LOG)).unwrap();
    fixture.append(&log, claude(&format!("more {SECRET}")).as_bytes());
    let pushed = fixture.push(&url);
    assert_ok(&pushed, "the push after the copy was lost");
    assert_eq!(fixture.copy(LOG), fixture.redacted(LOG));
    assert_eq!(
        fixture.copy(LOG).len(),
        fs::read(fixture.path(&log)).unwrap().len()
    );

    // Revoked while the receiver runs: the next push is refused.
    let revoked = fixture.receive(&["token", "revoke", "laptop", "--dir", dir.to_str().unwrap()]);
    assert_ok(&revoked, "token revoke");
    fixture.append(&log, claude("after the revoke").as_bytes());
    let pushed = fixture.push(&url);
    assert!(!pushed.status.success());
    assert!(
        stderr(&pushed).contains("refused the token (401)"),
        "{}",
        stderr(&pushed)
    );
    assert_ne!(fixture.copy(LOG), fixture.redacted(LOG));
}

/// `semon push --watch` ends on Ctrl-C through its stop, not by the
/// signal's default action alone: it says so, then ends by SIGINT as it
/// always did. While it runs, a second push with its state fails at once;
/// after, the state lock is free.
#[cfg(unix)]
#[test]
fn push_watch_stops_on_ctrl_c_and_holds_its_state_alone_until_then() {
    use std::{os::unix::process::ExitStatusExt, time::Instant};

    let mut fixture = Fixture::new();
    let dir = fixture.dir();
    let added = fixture.receive(&["token", "add", "laptop", "--dir", dir.to_str().unwrap()]);
    assert_ok(&added, "token add");
    let token = String::from_utf8(added.stdout).unwrap().trim().to_owned();
    fixture.write("token", token.as_bytes());
    private(&fixture.path("token"));
    let url = fixture.start_receiver();
    let log = format!("home/{LOG}");
    fixture.write(&log, claude("watched").as_bytes());

    let home = fixture.path("home");
    let mut watch = semon()
        .args(["push", "--watch", "--to", &url, "--token-file"])
        .arg(fixture.path("token"))
        .arg("--state")
        .arg(fixture.path("state/push.json"))
        .arg("--claude-home")
        .arg(home.join("claude"))
        .arg("--codex-home")
        .arg(home.join("codex"))
        .arg("--proc-root")
        .arg(home.join("proc"))
        .arg("--cache")
        .arg(fixture.path("state/index.json"))
        .env("HOME", &home)
        .env("XDG_STATE_HOME", fixture.path("state"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let said_on = watch.stderr.take().unwrap();
    // Ended if the test fails before the watch does.
    struct Ended(Option<Child>);
    impl Drop for Ended {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let mut watch = Ended(Some(watch));
    let watch = watch.0.as_mut().unwrap();
    let (lines, said) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(said_on).lines() {
            let Ok(line) = line else { break };
            eprintln!("[push --watch] {line}");
            let _ = lines.send(line);
        }
    });

    // The first pass and facts are in: the watch sleeps between passes.
    let facts = dir.join("machines/laptop/facts.json");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !facts.exists() {
        assert!(Instant::now() < deadline, "no facts from the watch");
        assert!(watch.try_wait().unwrap().is_none(), "the watch ended");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.copy(LOG), fixture.redacted(LOG));

    let second = fixture.push(&url);
    assert!(!second.status.success());
    assert!(
        stderr(&second).contains("already running"),
        "{}",
        stderr(&second)
    );

    let interrupted = Command::new("kill")
        .args(["-INT", &watch.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupted.success());
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = watch.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the watch ignored Ctrl-C");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(2), "{status:?}");
    let mut stopped = false;
    while let Ok(line) = said.recv_timeout(Duration::from_secs(5)) {
        stopped |= line == "semon push: stopped";
    }
    assert!(stopped, "the watch didn't stop through its stop");

    fixture.append(&log, claude("after the stop").as_bytes());
    let pushed = fixture.push(&url);
    assert_ok(&pushed, "a push after the watch stopped");
    assert_eq!(fixture.copy(LOG), fixture.redacted(LOG));
}

#[test]
fn receive_refuses_plain_http_off_loopback() {
    let fixture = Fixture::new();
    let dir = fixture.dir();
    let refused = fixture.receive(&["--listen", "0.0.0.0:0", "--dir", dir.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("--tls-cert"),
        "{}",
        stderr(&refused)
    );
    let refused = fixture.receive(&[
        "--listen",
        "0.0.0.0:0",
        "--tls-cert",
        "cert.pem",
        "--dir",
        dir.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("both --tls-cert and --tls-key"),
        "{}",
        stderr(&refused)
    );
}
