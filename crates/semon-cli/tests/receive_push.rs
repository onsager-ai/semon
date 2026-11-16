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

    /// Starts `semon receive` on a free loopback port and returns its URL.
    fn start_receiver(&mut self) -> String {
        self.start_receiver_with(&[])
    }

    fn start_receiver_with(&mut self, extra: &[&str]) -> String {
        let mut child = semon()
            .args(["receive", "--listen", "127.0.0.1:0", "--dir"])
            .arg(self.dir())
            .args(extra)
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
        semon_push::simple_pair::Config {
            url: url.into(),
            token: fs::read_to_string(self.path("token")).unwrap(),
            machine_id: "machine-6754a9c1e83f27b4".into(),
        }
        .save_at(&self.path("config/semon/pair.json"))
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
        fs::read(
            self.dir()
                .join("machines/machine-6754a9c1e83f27b4")
                .join(relative),
        )
        .unwrap_or_default()
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

fn pair_fixture(dir: &Path) -> String {
    semon_push::simple_pair::initialize(dir).unwrap();
    let registry_path = dir.join("enrollment/registry.json");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
    let token = "paired-test-push-bearer".to_owned();
    use semon_push::credentials::Secret;
    registry["simple_pair"]["pending"] = serde_json::Value::Null;
    registry["machines"] = json!([{"id":"machine-6754a9c1e83f27b4","directory":"machine-6754a9c1e83f27b4","owner_name":null}]);
    registry["simple_pair"]["tokens"] = json!([{"credential_id":"b".repeat(64),"enrollment_id":"machine-6754a9c1e83f27b4","kind":"push","sha256":Secret::new(token.clone()).hash(),"generation":1,"valid_from":0,"valid_until":u64::MAX,"revoked_at":null}]);
    fs::write(registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
    token
}

#[test]
fn push_mirrors_a_home_into_semon_receive() {
    let mut fixture = Fixture::new();
    let dir = fixture.dir();

    let token = pair_fixture(&dir);
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
    let machine = dir.join("machines/machine-6754a9c1e83f27b4");
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

    // Paired push discovers new supported files on each pass.
    fixture.write(
        "home/claude/projects/-work-app/unapproved.jsonl",
        claude("future source").as_bytes(),
    );

    // Rewritten shorter: sent again whole, with replace.
    fixture.write(&log, claude("rewritten").as_bytes());
    let pushed = fixture.push(&url);
    assert_ok(&pushed, "the push after a rewrite");
    assert!(
        machine
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
    let registry = semon_push::enrollment::Registry::read_current(&dir).unwrap();
    semon_push::simple_pair::unpair(&dir, &registry.machines()[0].id, u64::MAX - 1).unwrap();
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
    let token = pair_fixture(&dir);
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
fn pair_and_unpair_commands_store_replace_and_revoke_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    let url = f.start_receiver();
    let config = f.path("config");
    let pair_path = config.join("semon/pair.json");
    // Pair/unpair must never contact the host's service manager, including in CI.
    let fake_bin = f.path("fake-service-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    for program in ["systemctl", "loginctl", "launchctl"] {
        let path = fake_bin.join(program);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let issue = || {
        let output = semon()
            .args(["pair", "--dir"])
            .arg(f.dir())
            .output()
            .unwrap();
        assert_ok(&output, "receiver pair");
        String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    };
    let run_pair = |code: &str| {
        semon()
            .args(["pair", &url, code])
            .env("PATH", &fake_bin)
            .env("XDG_CONFIG_HOME", &config)
            .env("HOME", f.path("home"))
            .output()
            .unwrap()
    };
    let first = run_pair(&issue());
    assert_ok(&first, "machine pair");
    assert!(
        String::from_utf8(first.stdout)
            .unwrap()
            .contains("Background sync started; logs:")
    );
    let first: serde_json::Value = serde_json::from_slice(&fs::read(&pair_path).unwrap()).unwrap();
    assert_eq!(
        fs::metadata(&pair_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_ok(&run_pair(&issue()), "re-pair");
    let second: serde_json::Value = serde_json::from_slice(&fs::read(&pair_path).unwrap()).unwrap();
    assert_eq!(first["machine_id"], second["machine_id"]);
    assert_ne!(first["token"], second["token"]);
    let home = f.path("home");
    let pushed = semon()
        .arg("push")
        .arg("--state")
        .arg(f.path("paired-state.json"))
        .arg("--claude-home")
        .arg(home.join("claude"))
        .arg("--codex-home")
        .arg(home.join("codex"))
        .arg("--copilot-home")
        .arg(home.join("copilot"))
        .arg("--proc-root")
        .arg(home.join("proc"))
        .arg("--cache")
        .arg(f.path("paired-cache"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .output()
        .unwrap();
    assert_ok(&pushed, "push with pair.json defaults");
    assert!(
        f.dir()
            .join(format!(
                "machines/{}/facts.json",
                second["machine_id"].as_str().unwrap()
            ))
            .is_file()
    );
    let output = semon()
        .arg("unpair")
        .env("PATH", &fake_bin)
        .env("HOME", f.path("home"))
        .env("XDG_CONFIG_HOME", &config)
        .output()
        .unwrap();
    assert_ok(&output, "machine unpair");
    assert!(!pair_path.exists());
    // A container without a user bus/logind still pairs and can be unpaired.
    for program in ["systemctl", "loginctl", "launchctl"] {
        fs::write(fake_bin.join(program), "#!/bin/sh\nexit 1\n").unwrap();
    }
    let without_init = run_pair(&issue());
    assert_ok(&without_init, "pair without user service manager");
    let message = String::from_utf8(without_init.stdout).unwrap();
    assert!(message.contains("Background sync is not running"));
    assert!(message.contains("push --watch"));
    assert!(pair_path.exists());
    let output = semon()
        .args(["unpair", second["machine_id"].as_str().unwrap(), "--dir"])
        .arg(f.dir())
        .output()
        .unwrap();
    assert_ok(&output, "receiver unpair");
    // Losing reachability still removes the local configuration with a warning.
    let mut child = f.receiver.take().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let output = semon()
        .arg("unpair")
        .env("PATH", &fake_bin)
        .env("HOME", f.path("home"))
        .env("XDG_CONFIG_HOME", &config)
        .output()
        .unwrap();
    assert_ok(&output, "offline unpair");
    assert!(stderr(&output).contains("warning: receiver unreachable"));
    assert!(!pair_path.exists());
}

#[test]
fn receiver_viewer_read_tokens_login_revocation_and_route_separation() {
    use std::io::{Read, Write};
    fn http(url: &str, path: &str, headers: &str, body: Option<&str>) -> String {
        let address = url.strip_prefix("http://").unwrap();
        let mut socket = std::net::TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let method = if body.is_some() { "POST" } else { "GET" };
        let body = body.unwrap_or("");
        write!(socket, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
        let mut reply = String::new();
        socket.read_to_string(&mut reply).unwrap();
        reply
    }
    fn header<'a>(response: &'a str, name: &str) -> &'a str {
        response
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name).then_some(value.trim())
            })
            .unwrap()
    }
    let mut f = Fixture::new();
    let mut url = f.start_receiver();
    let token_command = |arguments: &[&str]| {
        semon()
            .arg("token")
            .args(arguments)
            .arg("--dir")
            .arg(f.dir())
            .output()
            .unwrap()
    };
    let created = token_command(&[]);
    assert_ok(&created, "create read token");
    let secret = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    let listing = token_command(&["--list"]);
    assert_ok(&listing, "list read tokens");
    let listing = String::from_utf8(listing.stdout).unwrap();
    assert!(!listing.contains(&secret));
    let id = listing
        .lines()
        .nth(1)
        .unwrap()
        .split('\t')
        .next()
        .unwrap()
        .to_owned();
    let stored = fs::read_to_string(f.dir().join("enrollment/registry.json")).unwrap();
    assert!(!stored.contains(&secret));
    assert!(stored.contains("created"));
    assert!(http(&url, "/", "", None).starts_with("HTTP/1.1 403"));
    let landing = http(&url, &format!("/?t={secret}&keep=1"), "", None);
    assert!(landing.starts_with("HTTP/1.1 303"), "{landing}");
    assert_eq!(header(&landing, "Location"), "/?keep=1");
    let set_cookie = header(&landing, "Set-Cookie");
    assert!(set_cookie.contains("HttpOnly; SameSite=Strict; Path=/"));
    assert!(!set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    // Authentication succeeds, but the existing Viewer has no root page until
    // a machine is paired. Its source inventory remains available when empty.
    let empty_page = http(&url, "/", &format!("Cookie: {cookie}\r\n"), None);
    assert!(empty_page.starts_with("HTTP/1.1 404"), "{empty_page}");
    let inventory = http(
        &url,
        "/api/session-sources",
        &format!("Cookie: {cookie}\r\n"),
        None,
    );
    assert!(inventory.starts_with("HTTP/1.1 200"), "{inventory}");
    assert!(
        http(
            &url,
            "/api/session-sources",
            &format!("Authorization: Bearer {secret}\r\n"),
            None
        )
        .starts_with("HTTP/1.1 200")
    );
    // Neither a malformed duplicate login nor a network-path redirect is admitted.
    assert!(http(&url, &format!("/?t={secret}&t={secret}"), "", None).starts_with("HTTP/1.1 403"));
    assert!(
        http(&url, &format!("//other.example/?t={secret}"), "", None).starts_with("HTTP/1.1 403")
    );
    // Pairing and push credentials cannot authorize read routes, or each other's routes.
    let (code, _) =
        semon_push::simple_pair::issue(&f.dir(), semon_push::simple_pair::now().unwrap()).unwrap();
    let pair_body = serde_json::json!({"version":1,"code":code.expose(),"machine_id":"machine-6754a9c1e83f27b4","hostname":"laptop"}).to_string();
    assert!(
        http(
            &url,
            "/v1/pair",
            &format!("Authorization: Bearer {secret}\r\n"),
            Some(&pair_body)
        )
        .starts_with("HTTP/1.1 403")
    );
    let paired = http(&url, "/v1/pair", "", Some(&pair_body));
    assert!(paired.starts_with("HTTP/1.1 200"), "{paired}");
    let paired: serde_json::Value =
        serde_json::from_str(paired.split_once("\r\n\r\n").unwrap().1).unwrap();
    let push = paired["token"].as_str().unwrap();
    for wrong in [push, code.expose()] {
        assert!(
            http(
                &url,
                "/",
                &format!("Authorization: Bearer {wrong}\r\n"),
                None
            )
            .starts_with("HTTP/1.1 403")
        );
        assert!(http(&url, &format!("/?t={wrong}"), "", None).starts_with("HTTP/1.1 403"));
    }
    for wrong in [secret.as_str(), code.expose()] {
        assert!(
            http(
                &url,
                "/v1/push/status",
                &format!("Authorization: Bearer {wrong}\r\n"),
                Some("{}")
            )
            .starts_with("HTTP/1.1 401")
        );
    }
    let facts = serde_json::json!({"version":2,"hostname":"laptop","home":null,"proc_starts":{},"codex_locks":{},"repos":{},"repo_ids":{}}).to_string();
    let pushed = http(
        &url,
        "/v1/push/facts",
        &format!("Authorization: Bearer {push}\r\n"),
        Some(&facts),
    );
    assert!(pushed.starts_with("HTTP/1.1 200"), "{pushed}");
    assert!(
        f.dir()
            .join("machines/machine-6754a9c1e83f27b4/facts.json")
            .is_file()
    );
    assert!(
        http(
            &url,
            "/v1/pair",
            &format!("Authorization: Bearer {push}\r\n"),
            Some(&pair_body)
        )
        .starts_with("HTTP/1.1 403")
    );
    // Four slow Viewer bodies from the tunnel peer cannot consume its four
    // unauthenticated push slots. 100 Continue witnesses admission without sleeps.
    let mut held = Vec::new();
    for _ in 0..4 {
        let mut socket =
            std::net::TcpStream::connect(url.strip_prefix("http://").unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write!(socket, "POST /api/viewer/home HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {secret}\r\nContent-Length: 4096\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n", url.strip_prefix("http://").unwrap()).unwrap();
        let mut interim = Vec::new();
        while !interim.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            interim.push(byte[0]);
        }
        assert!(
            String::from_utf8(interim)
                .unwrap()
                .starts_with("HTTP/1.1 100")
        );
        held.push(socket);
    }
    assert!(
        http(
            &url,
            "/v1/push/facts",
            &format!("Authorization: Bearer {push}\r\n"),
            Some(&facts)
        )
        .starts_with("HTTP/1.1 200")
    );
    drop(held);
    // Tokens and ID-bound cookies survive restarting the server, including a port change.
    let mut child = f.receiver.take().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    url = f.start_receiver_with(&["--public-url", "https://receiver.example"]);
    let page = http(&url, "/", &format!("Cookie: {cookie}\r\n"), None);
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    let address = url.strip_prefix("http://").unwrap();
    let public_request = |host: &str, origin: &str, path: &str| {
        let mut socket = std::net::TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write!(socket, "GET {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nAuthorization: Bearer {secret}\r\nConnection: close\r\n\r\n").unwrap();
        let mut reply = String::new();
        socket.read_to_string(&mut reply).unwrap();
        reply
    };
    assert!(
        public_request("receiver.example", "https://receiver.example", "/")
            .starts_with("HTTP/1.1 200")
    );
    let secure_login = public_request(
        "receiver.example",
        "https://receiver.example",
        &format!("/?t={secret}"),
    );
    assert!(header(&secure_login, "Set-Cookie").contains("; Secure"));
    assert!(
        public_request("attacker.example", "https://attacker.example", "/")
            .starts_with("HTTP/1.1 403")
    );
    assert!(
        public_request("receiver.example", "https://attacker.example", "/")
            .starts_with("HTTP/1.1 403")
    );
    let created_public = semon()
        .args(["token", "--dir"])
        .arg(f.dir())
        .output()
        .unwrap();
    assert_ok(&created_public, "public read token");
    assert!(
        String::from_utf8(created_public.stdout)
            .unwrap()
            .contains("https://receiver.example/?t=")
    );
    let revoked = semon()
        .args(["token", "--revoke", &id, "--dir"])
        .arg(f.dir())
        .output()
        .unwrap();
    assert_ok(&revoked, "revoke read token");
    assert!(http(&url, "/", &format!("Cookie: {cookie}\r\n"), None).starts_with("HTTP/1.1 403"));
    assert!(
        http(
            &url,
            "/",
            &format!("Authorization: Bearer {secret}\r\n"),
            None
        )
        .starts_with("HTTP/1.1 403")
    );
    assert!(http(&url, &format!("/?t={secret}"), "", None).starts_with("HTTP/1.1 403"));
    let stored = fs::read_to_string(f.dir().join("enrollment/registry.json")).unwrap();
    assert!(stored.contains("revoked"));
}
