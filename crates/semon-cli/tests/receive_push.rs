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

    fn install_origin(&self, url: &str) {
        use semon_push::{
            credentials::Kind, enrollment::Binding, pairing::ActivationInput,
            provision_wire::Activated,
        };
        let base = self.path("config/semon/origin");
        if base.exists() {
            return;
        }
        let encoded = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgL_E8qFEyPcpHM_jv-bQj0alxU3tkrY_1OSwOWVOwhs-hRANCAATA08frMi95YZiy3BekFDqwQHAS2SIpv7yA1ckqs3drjLLJqCyFKRx7VY_MJawLw_RbjwOyGKFasQHvnfrO1MUQ";
        let private = semon_push::provision_wire::decode_fixed::<138>(encoded).unwrap();
        let scope = ActivationInput {
            receiver_id: "receiver-test".into(),
            machine: "laptop".into(),
            binding: Binding::Inapplicable,
            kinds: vec![(Kind::MirrorPush, false)],
            client_certificate_pins: vec![],
            valid_until: u64::MAX,
        };
        let delivery = Activated {
            version: 1,
            enrollment_id: "enrollment-test".into(),
            client_certificate: None,
            credentials: vec![(
                Kind::MirrorPush,
                fs::read_to_string(self.path("token")).unwrap(),
            )],
        };
        let home = self.path("home");
        let options = semon_sessions::Options {
            claude_home: home.join("claude"),
            codex_home: home.join("codex"),
            copilot_home: home.join("copilot"),
            proc_root: home.join("proc"),
            ..Default::default()
        };
        let sources = semon_sessions::inputs(&options)
            .unwrap()
            .iter()
            .map(|input| input.full_path(&options))
            .collect::<Vec<_>>();
        semon_push::origin_custody::install(
            &base, url, "laptop", &scope, &delivery, &private, &sources,
        )
        .unwrap();
    }

    /// `semon push` from the synthetic home.
    fn push(&self, url: &str) -> Output {
        self.install_origin(url);
        let home = self.path("home");
        semon()
            .args(["push"])
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
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_STATE_HOME", self.path("state"))
            .output()
            .unwrap()
    }

    /// `semon sessions --json --all --machines DIR --no-local`, with
    /// the reader's own homes and cache in the fixture.
    fn tree(&self) -> serde_json::Value {
        let viewer = self.path("viewer");
        let output = semon()
            .args(["sessions", "--json", "--all", "--no-local", "--machines"])
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
        assert_ok(&output, "the native tree over DIR");
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

fn enroll_fixture(dir: &Path) -> String {
    use semon_push::{
        credentials::{Bearer, Kind, Secret},
        enrollment::{Binding, Enrollment, KindScope, Registry, Store},
    };
    semon_push::tokens::prepare_private_dir(dir, true).unwrap();
    let token = "enrolled-test-mirror-bearer".to_owned();
    let id = "a".repeat(64);
    let registry = Registry::from_records(
        vec![Enrollment {
            enrollment_id: id.clone(),
            receiver_id: "receiver-1".into(),
            machine: "laptop".into(),
            sequence: 1,
            origin_key: vec![4; 65],
            endpoint: "https://receiver.example:443".into(),
            binding: Binding::Inapplicable,
            client_certificate_pins: vec![],
            credential_kinds: vec![KindScope::new(Kind::MirrorPush, 1)],
            revoked_at: None,
        }],
        vec![Bearer {
            credential_id: "b".repeat(64),
            enrollment_id: id,
            kind: Kind::MirrorPush,
            sha256: Secret::new(token.clone()).hash(),
            generation: 1,
            valid_from: 0,
            valid_until: u64::MAX,
            revoked_at: None,
        }],
    )
    .unwrap();
    Store::initialize(dir)
        .unwrap()
        .commit(0, &registry)
        .unwrap();
    token
}

#[test]
fn push_mirrors_a_home_into_semon_receive() {
    let mut fixture = Fixture::new();
    let dir = fixture.dir();

    let token = enroll_fixture(&dir);
    fixture.write("token", token.as_bytes());
    private(&fixture.path("token"));

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

    // The native Viewer tree names both pushed sessions and their machine.
    let tree = fixture.tree();
    assert_eq!(tree["schema_version"], 1);
    let roots = tree["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 2);
    for session in ["lane", "cx"] {
        let found = roots.iter().find(|root| root["id"] == session).unwrap();
        assert_eq!(found["machine"], "laptop");
    }
    // The pushed facts still qualify an up machine independently of history.
    let options = semon_sessions::Options {
        cache: fixture.path("viewer/state/index.json"),
        claude_home: fixture.path("viewer/claude"),
        codex_home: fixture.path("viewer/codex"),
        copilot_home: fixture.path("viewer/copilot"),
        proc_root: fixture.path("home/proc"),
        all: true,
        ..Default::default()
    };
    let core = semon_sessions::ViewerCore::with_received(
        Vec::new(),
        semon_sessions::ReceivedMachines::new(&dir, &options),
    );
    let reply = core.respond("GET", "/api/viewer/home", "", None);
    assert_eq!(reply.status, 200);
    let overview: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(overview["machines"].as_array().unwrap().len(), 1);
    assert_eq!(overview["machines"][0]["label"], "laptop");
    assert_eq!(overview["machines"][0]["up"], true);
    core.close();

    // New files after enrollment are not silently included in source consent.
    fixture.write(
        "home/claude/projects/-work-app/unapproved.jsonl",
        claude("future source").as_bytes(),
    );

    // Rewritten shorter: sent again whole, with replace.
    fixture.write(&log, claude("rewritten").as_bytes());
    let pushed = fixture.push(&url);
    assert_ok(&pushed, "the push after a rewrite");
    assert!(
        !machine
            .join("claude/projects/-work-app/unapproved.jsonl")
            .exists()
    );
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
    let mut store = semon_push::enrollment::Store::open(&dir).unwrap();
    let registry = store.snapshot().unwrap();
    let mut enrollments = registry.enrollments().to_vec();
    enrollments[0].revoked_at = Some(1);
    let next = semon_push::enrollment::Registry::from_records(
        enrollments,
        registry.credentials().to_vec(),
    )
    .unwrap();
    store.commit(registry.revision(), &next).unwrap();
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

#[cfg(unix)]
#[test]
fn push_watch_stops_on_ctrl_c_and_holds_its_state_alone_until_then() {
    push_watch_stops_on("INT", 2);
}

#[cfg(unix)]
#[test]
fn push_watch_stops_on_sigterm_and_holds_its_state_alone_until_then() {
    push_watch_stops_on("TERM", 15);
}

/// `semon push --watch` ends on SIG`name` (Ctrl-C, or a service manager's
/// stop) through its stop, not by the signal's default action alone: it
/// says so, then ends by that signal as it always did. While it runs, a
/// second push with its state fails at once; after, the state lock is
/// free.
#[cfg(unix)]
fn push_watch_stops_on(name: &str, number: i32) {
    use std::os::unix::process::ExitStatusExt;

    let mut fixture = Fixture::new();
    let dir = fixture.dir();
    let token = enroll_fixture(&dir);
    fixture.write("token", token.as_bytes());
    private(&fixture.path("token"));
    let url = fixture.start_receiver();
    let log = format!("home/{LOG}");
    fixture.write(&log, claude("watched").as_bytes());

    let home = fixture.path("home");
    fixture.install_origin(&url);
    let mut watch = semon()
        .args(["push", "--watch"])
        .env("XDG_CONFIG_HOME", fixture.path("config"))
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

    // Synchronize with the completed initial pass through its progress event.
    loop {
        let line = said
            .recv_timeout(Duration::from_secs(60))
            .expect("watch initial pass");
        if line.starts_with("semon push: ") && line.contains(" files, ") {
            break;
        }
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
        .args([format!("-{name}"), watch.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupted.success());
    let status = watch.wait().unwrap();
    assert_eq!(status.signal(), Some(number), "{status:?}");
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

#[test]
fn enroll_refuses_legacy_tokens_before_terminal_or_custody_initialization() {
    let fixture = Fixture::new();
    let dir = fixture.dir();
    std::fs::create_dir_all(&dir).unwrap();
    let tokens = b"preserve legacy receiver bytes";
    std::fs::write(dir.join("tokens"), tokens).unwrap();
    let output = fixture.receive(&[
        "enroll",
        "--machine",
        "laptop",
        "--advertise",
        "https://receiver.example",
        "--dir",
        dir.to_str().unwrap(),
        "--tls-cert",
        "unused.pem",
        "--tls-key",
        "unused-key.pem",
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("legacy tokens found"));
    assert!(stderr(&output).contains("fresh receiver directory"));
    assert_eq!(std::fs::read(dir.join("tokens")).unwrap(), tokens);
    assert!(!dir.join("receiver.json").exists());
}
