//! `semon push` against a tiny in-process receiver that implements the
//! mirror protocol (docs/mirror-protocol.md) as any receiver would.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use semon_push::{
    Client, Credential, PushOptions, StateLock, Stop, Token, read_token,
    wire::{Append, HEAD_BYTES, Length, base64_decode, head_sha256},
};
use semon_sessions::{Facts, Options, is_input_path};
use serde_json::json;
use tiny_http::{Header, Response, Server};

static NEXT: AtomicU64 = AtomicU64::new(0);
const TOKEN: &str = "test-token-0123456789";

#[derive(Default)]
struct Received {
    files: BTreeMap<String, Vec<u8>>,
    facts: Option<Facts>,
    appends: u64,
    replaces: u64,
    conflicts: u64,
    /// Answer 500 after this many successful appends.
    fail_after: Option<u64>,
    /// Never answer an append after this many successful ones: the push is
    /// left waiting mid-upload.
    hang_after: Option<u64>,
    /// Appends left unanswered so far.
    held: u64,
    /// The unanswered appends themselves: dropping them ends their
    /// connections.
    held_requests: Vec<tiny_http::Request>,
    /// Stop this push once this many appends have succeeded, before
    /// answering the last of them.
    stop_after: Option<(u64, Stop)>,
    /// Refuse the token (401) for any append after this many successful
    /// ones.
    refuse_after: Option<u64>,
    /// Commit one append and deliberately lose its acknowledgement.
    drop_ack_once: bool,
}

struct Receiver {
    url: String,
    server: Arc<Server>,
    state: Arc<Mutex<Received>>,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

fn json_response(status: u16, body: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(body.to_string().into_bytes())
        .with_status_code(tiny_http::StatusCode(status))
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

fn receiver() -> Receiver {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let state = Arc::new(Mutex::new(Received::default()));
    let (worker, shared) = (Arc::clone(&server), Arc::clone(&state));
    thread::spawn(move || {
        for mut request in worker.incoming_requests() {
            let authorized = request.headers().iter().any(|header| {
                header.field.equiv("Authorization")
                    && header.value.as_str() == format!("Bearer {TOKEN}")
            });
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let url = request.url().to_owned();
            if authorized && url == "/v1/mirror/append" {
                let mut received = shared.lock().unwrap();
                if received
                    .hang_after
                    .is_some_and(|limit| received.appends >= limit)
                {
                    received.held += 1;
                    received.held_requests.push(request);
                    continue;
                }
                if received
                    .refuse_after
                    .is_some_and(|limit| received.appends >= limit)
                {
                    drop(received);
                    let _ = request.respond(json_response(401, json!({"error":"revoked"})));
                    continue;
                }
            }
            let response = if !authorized {
                json_response(401, json!({"error":"unauthorized"}))
            } else if url == "/v1/mirror/facts" {
                shared.lock().unwrap().facts = serde_json::from_str(&body).ok();
                json_response(200, json!({}))
            } else if url == "/v1/mirror/append" {
                handle_append(&mut shared.lock().unwrap(), &body)
            } else {
                json_response(404, json!({}))
            };
            if authorized
                && url == "/v1/mirror/append"
                && std::mem::take(&mut shared.lock().unwrap().drop_ack_once)
            {
                drop(request);
                continue;
            }
            let _ = request.respond(response);
        }
    });
    Receiver {
        url: format!("http://127.0.0.1:{port}"),
        server,
        state,
    }
}

fn handle_append(received: &mut Received, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let Ok(append) = serde_json::from_str::<Append>(body) else {
        return json_response(400, json!({"error":"body"}));
    };
    if !is_input_path(&append.root, &append.path) {
        return json_response(400, json!({"error":"path"}));
    }
    let Some(bytes) = base64_decode(&append.bytes) else {
        return json_response(400, json!({"error":"bytes"}));
    };
    if received
        .fail_after
        .is_some_and(|limit| received.appends >= limit)
    {
        return json_response(500, json!({"error":"down"}));
    }
    let key = format!("{}/{}", append.root, append.path);
    let current = received.files.get(&key).cloned().unwrap_or_default();
    let conflict = |received: &mut Received, current: &[u8]| {
        received.conflicts += 1;
        json_response(
            409,
            serde_json::to_value(Length {
                length: current.len() as u64,
                head_sha256: Some(head_sha256(current)),
            })
            .unwrap(),
        )
    };
    let base: &[u8] = if append.replace {
        if append.offset != 0 {
            return json_response(400, json!({"error":"replace at 0"}));
        }
        &[]
    } else {
        if append.offset != current.len() as u64 {
            return conflict(received, &current);
        }
        &current
    };
    let mut next = base.to_vec();
    next.extend_from_slice(&bytes);
    if head_sha256(&next[..next.len().min(HEAD_BYTES)]) != append.head_sha256 {
        return conflict(received, &current);
    }
    received.appends += 1;
    received.replaces += u64::from(append.replace);
    let length = next.len() as u64;
    received.files.insert(key, next);
    if let Some((after, stop)) = &received.stop_after
        && received.appends >= *after
    {
        stop.stop();
    }
    json_response(200, json!({"length": length}))
}

struct Home {
    root: PathBuf,
    options: Options,
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-push-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let options = Options {
            claude_home: root.join("claude"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            ..Options::default()
        };
        let home = Self { root, options };
        home.write("proc/locks", "");
        home.write("proc/sys/kernel/hostname", "laptop\n");
        home.write("token", TOKEN);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home.root.join("token"), fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        home
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn append(&self, relative: &str, content: &str) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self.root.join(relative))
            .unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    fn push_options(&self, url: &str) -> PushOptions {
        PushOptions {
            url: url.to_owned(),
            credential: Credential::File(self.root.join("token")),
            sessions: self.options.clone(),
            state: self.root.join("state/push.json"),
        }
    }

    fn client(&self, receiver: &Receiver) -> Client {
        Client::new(&self.push_options(&receiver.url)).unwrap()
    }
}

const LOG: &str = "claude/projects/-work/lane.jsonl";

fn line(text: &str) -> String {
    json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"lane","message":{"role":"user","content":text}}).to_string() + "\n"
}

/// The receiver's copy of `relative`.
fn copy(receiver: &Receiver, relative: &str) -> Vec<u8> {
    receiver
        .state
        .lock()
        .unwrap()
        .files
        .get(relative)
        .cloned()
        .unwrap_or_default()
}

fn redacted(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    semon_push::redact::redact(&mut bytes);
    bytes
}

#[test]
fn only_inputs_go_redacted_and_an_incomplete_line_waits() {
    let home = Home::new();
    let receiver = receiver();
    let secret = "sk-ant-api03-SECRETSECRETSECRETSECRET";
    let first = line(&format!("use {secret} please"));
    home.write(LOG, &first);
    home.append(LOG, "{\"type\":\"user\",\"half");
    home.write(
        "claude/sessions/12.json",
        &json!({"pid":12,"sessionId":"lane","procStart":1}).to_string(),
    );
    home.write("claude/sessions/12.key", "NEVER-SENT");
    home.write("claude/settings.json", "{\"NEVER\":1}");
    let mut client = home.client(&receiver);
    let report = client.pass(&home.options).unwrap();
    assert_eq!(report.files, 2);
    let sent = copy(&receiver, LOG);
    assert_eq!(sent, redacted(&first), "complete lines only, redacted");
    assert_eq!(sent.len(), first.len());
    assert!(!String::from_utf8_lossy(&sent).contains(secret));
    let files: Vec<String> = receiver
        .state
        .lock()
        .unwrap()
        .files
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        files,
        [LOG.to_owned(), "claude/sessions/12.json".to_owned()]
    );

    // The line completes: only its bytes go.
    home.append(LOG, "\":1}\n");
    let report = client.pass(&home.options).unwrap();
    assert_eq!(
        report.bytes,
        "{\"type\":\"user\",\"half\":1}\n".len() as u64
    );
    let mut expected = redacted(&first);
    expected.extend_from_slice(b"{\"type\":\"user\",\"half\":1}\n");
    assert_eq!(copy(&receiver, LOG), expected);
    // Nothing new: nothing sent.
    assert_eq!(client.pass(&home.options).unwrap().files, 0);
}

#[test]
fn an_interrupted_push_resumes_where_the_receiver_stopped() {
    let home = Home::new();
    let receiver = receiver();
    let text: String = (0..40)
        .map(|number| line(&format!("line {number}")))
        .collect();
    home.write(LOG, &text);
    receiver.state.lock().unwrap().fail_after = Some(3);
    let mut client = home.client(&receiver).with_chunk(300);
    assert!(
        client.pass(&home.options).is_err(),
        "the receiver went down"
    );
    let partial = copy(&receiver, LOG).len();
    assert!(partial > 0 && partial < text.len());
    // A new process: the state file says where it stopped.
    receiver.state.lock().unwrap().fail_after = None;
    let mut client = home.client(&receiver).with_chunk(300);
    let report = client.pass(&home.options).unwrap();
    assert_eq!(report.bytes, (text.len() - partial) as u64, "only the rest");
    assert_eq!(copy(&receiver, LOG), text.as_bytes());
    assert_eq!(receiver.state.lock().unwrap().replaces, 0);
}

#[test]
fn a_409_resumes_from_the_receivers_length_or_replaces_a_different_copy() {
    let home = Home::new();
    let receiver = receiver();
    let text: String = (0..10)
        .map(|number| line(&format!("line {number}")))
        .collect();
    home.write(LOG, &text);
    home.client(&receiver).pass(&home.options).unwrap();
    // The state is lost; the receiver already has everything so far.
    fs::remove_file(home.root.join("state/push.json")).unwrap();
    home.append(LOG, &line("more"));
    let report = home.client(&receiver).pass(&home.options).unwrap();
    assert_eq!(report.bytes, line("more").len() as u64);
    assert_eq!(report.replaced, 0);
    assert_eq!(receiver.state.lock().unwrap().conflicts, 1);
    assert_eq!(copy(&receiver, LOG), fs::read(home.root.join(LOG)).unwrap());

    // The receiver's copy differs from ours: it is replaced.
    receiver
        .state
        .lock()
        .unwrap()
        .files
        .insert(LOG.into(), line("someone else's").into_bytes());
    fs::remove_file(home.root.join("state/push.json")).unwrap();
    let report = home.client(&receiver).pass(&home.options).unwrap();
    assert_eq!(report.replaced, 1);
    assert_eq!(copy(&receiver, LOG), fs::read(home.root.join(LOG)).unwrap());
}

#[test]
fn a_rewritten_or_shrunk_file_is_sent_again_with_replace() {
    let home = Home::new();
    let receiver = receiver();
    home.write(LOG, &(line("one") + &line("two") + &line("three")));
    let mut client = home.client(&receiver);
    client.pass(&home.options).unwrap();
    // Shrunk.
    home.write(LOG, &line("uno"));
    let report = client.pass(&home.options).unwrap();
    assert_eq!(report.replaced, 1);
    assert_eq!(copy(&receiver, LOG), line("uno").into_bytes());
    // Same length, new head.
    home.write(LOG, &line("dos"));
    let report = client.pass(&home.options).unwrap();
    assert_eq!(report.replaced, 1);
    assert_eq!(copy(&receiver, LOG), line("dos").into_bytes());
    // A whole-file input that changes is sent again whole.
    home.write("claude/sessions/5.json", "{\"pid\":5,\"status\":\"busy\"}");
    client.pass(&home.options).unwrap();
    home.write("claude/sessions/5.json", "{\"pid\":5,\"status\":\"idle\"}");
    client.pass(&home.options).unwrap();
    assert_eq!(
        copy(&receiver, "claude/sessions/5.json"),
        b"{\"pid\":5,\"status\":\"idle\"}"
    );
}

#[test]
fn a_long_line_is_redacted_whole_before_it_is_cut_into_chunks() {
    let home = Home::new();
    let receiver = receiver();
    // The key straddles the 64-byte chunk boundaries.
    let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz";
    let long = line(&format!("{} {secret} end", "x".repeat(40)));
    home.write(LOG, &(line("short") + &long));
    let mut client = home.client(&receiver).with_chunk(64);
    client.pass(&home.options).unwrap();
    let sent = copy(&receiver, LOG);
    assert_eq!(sent, redacted(&(line("short") + &long)));
    assert!(!String::from_utf8_lossy(&sent).contains("ABCDEFGHIJ"));
    assert!(receiver.state.lock().unwrap().appends > 3);
}

#[test]
fn archive_and_unarchive_pushes_select_current_sources_without_deleting_old_copies() {
    let mut home = Home::new();
    home.options.all = true;
    let receiver = receiver();
    let mirrored = Home::new();
    let mut mirrored_options = mirrored.options.clone();
    mirrored_options.all = true;
    mirrored_options.facts = Some(mirrored.root.join("facts.json"));
    let active = "codex/sessions/2026/09/24/rollout-archive-root.jsonl";
    let archive = "codex/archived_sessions/rollout-archive-root.jsonl";
    let secret = "sk-ant-api03-SECRETSECRETSECRETSECRET";
    let record = |number| {
        json!({"timestamp":format!("2026-09-24T00:00:0{number}Z"),"type":"event_msg","payload":{"type":"user_message","message":format!("fixture {number} {secret}")}}).to_string()+"\n"
    };
    let initial = json!({"timestamp":"2026-09-24T00:00:00Z","type":"session_meta","payload":{"id":"archive-root","cwd":"/fixture/repo"}}).to_string()+"\n"+&record(1);
    home.write(active, &initial);
    for pass in 0..3 {
        if pass == 1 {
            home.append(active, &record(2));
            fs::create_dir_all(home.root.join("codex/archived_sessions")).unwrap();
            fs::rename(home.root.join(active), home.root.join(archive)).unwrap();
        } else if pass == 2 {
            fs::rename(home.root.join(archive), home.root.join(active)).unwrap();
            home.append(active, &record(3));
        }
        semon_push::push(&home.push_options(&receiver.url), false).unwrap();
        let (files, facts) = {
            let state = receiver.state.lock().unwrap();
            (state.files.clone(), state.facts.clone().unwrap())
        };
        let current = if pass == 1 { archive } else { active };
        assert_eq!(facts.codex_rollouts.as_ref().unwrap().len(), 1);
        assert!(
            facts
                .codex_rollouts
                .as_ref()
                .unwrap()
                .contains(current.strip_prefix("codex/").unwrap())
        );
        for (path, bytes) in &files {
            let destination = mirrored.root.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(destination, bytes).unwrap();
            assert!(!String::from_utf8_lossy(bytes).contains(secret));
        }
        semon_sessions::write_facts(mirrored_options.facts.as_ref().unwrap(), &facts).unwrap();
        // Compare against a deliberate redacted current-source copy, not the
        // unredacted local transcript. The stale receiver paths remain on disk.
        let expected = Home::new();
        let mut expected_options = expected.options.clone();
        expected_options.all = true;
        expected_options.facts = Some(expected.root.join("facts.json"));
        semon_sessions::write_facts(expected_options.facts.as_ref().unwrap(), &facts).unwrap();
        let destination = expected.root.join(current);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(
            destination,
            redacted(&fs::read_to_string(home.root.join(current)).unwrap()),
        )
        .unwrap();
        let now = 1_790_208_000_000;
        assert_eq!(
            semon_sessions::model_json_at(&mirrored_options, now).unwrap(),
            semon_sessions::model_json_at(&expected_options, now).unwrap(),
            "pass {pass}"
        );
        let core = semon_sessions::ViewerCore::new(mirrored_options.clone());
        let expected_core = semon_sessions::ViewerCore::new(expected_options);
        for (path, query) in [
            ("/api/tx", "sid=archive-root"),
            ("/api/transcript", "harness=codex&id=archive-root"),
        ] {
            let actual = core.respond("GET", path, query, None);
            let wanted = expected_core.respond("GET", path, query, None);
            assert_eq!(actual.status, 200, "{path} pass {pass}");
            assert_eq!(actual.body, wanted.body, "{path} pass {pass}");
        }
        assert_eq!(
            fs::read(home.root.join(current)).unwrap(),
            if pass == 0 {
                initial.clone()
            } else if pass == 1 {
                initial.clone() + &record(2)
            } else {
                initial.clone() + &record(2) + &record(3)
            }
            .as_bytes()
        );
        if pass > 0 {
            assert!(files.contains_key(active));
            assert!(files.contains_key(archive));
            assert!(mirrored.root.join(active).exists());
            assert!(mirrored.root.join(archive).exists());
        }
    }
}

#[test]
fn facts_are_sent_and_a_bad_token_is_refused() {
    let home = Home::new();
    let receiver = receiver();
    home.write(LOG, &line("hello"));
    semon_push::push(&home.push_options(&receiver.url), false).unwrap();
    let facts = receiver.state.lock().unwrap().facts.clone().unwrap();
    assert_eq!(facts.hostname, "laptop");
    assert_eq!(facts, semon_sessions::local_facts(&home.options).unwrap());

    home.write("token", "wrong-token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(home.root.join("token"), fs::Permissions::from_mode(0o600)).unwrap();
    }
    home.append(LOG, &line("more"));
    let error = home.client(&receiver).pass(&home.options).unwrap_err();
    assert!(error.contains("refused the token"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_token_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let path = home.root.join("token");
    for mode in [0o644, 0o640, 0o604, 0o660] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let error = read_token(&path).unwrap_err();
        assert!(error.contains("0600"), "{mode:o}: {error}");
        assert!(Client::new(&home.push_options("http://127.0.0.1:9")).is_err());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_token(&path).unwrap(), TOKEN);
    // The state file the client writes is private too.
    let receiver = receiver();
    home.write(LOG, &line("hello"));
    home.client(&receiver).pass(&home.options).unwrap();
    let state = home.root.join("state/push.json");
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(state.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn only_https_or_loopback_http() {
    for good in [
        "https://mirror.example/api/push",
        "http://127.0.0.1:8080/x",
        "http://127.0.0.2:8781/api/push",
        "http://localhost:1",
    ] {
        assert!(semon_push::check_url(good).is_ok(), "{good}");
    }
    for bad in [
        "http://mirror.example/api/push",
        "http://10.0.0.1/api/push",
        "ftp://127.0.0.1",
        "https://user:pw@mirror.example",
        "https://mirror.example/?x=1",
        "mirror.example",
    ] {
        assert!(semon_push::check_url(bad).is_err(), "{bad}");
    }
}

/// A stop is waited on for at most this long; a push that waited out its
/// 2 s sleep or its 120 s request timeout would miss it.
const STOP_BOUND: Duration = Duration::from_secs(1);

fn watch_in_background(options: &PushOptions, stop: &Stop) -> JoinHandle<Result<(), String>> {
    let (options, stop) = (options.clone(), stop.clone());
    thread::spawn(move || semon_push::push_until(&options, true, &stop))
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Stops the push and returns what it returned, failing unless it returned
/// within [`STOP_BOUND`].
fn stop_and_join(stop: &Stop, push: JoinHandle<Result<(), String>>) -> Result<(), String> {
    let stopped = Instant::now();
    stop.stop();
    while !push.is_finished() {
        assert!(
            stopped.elapsed() < STOP_BOUND,
            "the push was still running {:?} after the stop",
            stopped.elapsed()
        );
        thread::sleep(Duration::from_millis(5));
    }
    push.join().unwrap()
}

/// After a stop: the state file, if any, is whole; no temporary file is
/// left beside it; and its lock is free again once the push's threads have
/// let go. Returns the state.
fn assert_stopped_cleanly(options: &PushOptions) -> serde_json::Value {
    let value = state_after_stop(options);
    assert_lock_freed(options);
    value
}

/// The lock can be taken again within a moment: nothing of the stopped
/// push is left running.
fn assert_lock_freed(options: &PushOptions) {
    let deadline = Instant::now() + STOP_BOUND * 2;
    loop {
        match StateLock::acquire(&options.state) {
            Ok(_lock) => return,
            Err(error) => assert!(Instant::now() < deadline, "still locked: {error}"),
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The state file, if any, is whole, and no temporary file is left beside
/// it. Returns the state.
fn state_after_stop(options: &PushOptions) -> serde_json::Value {
    let state = &options.state;
    let value = if state.exists() {
        serde_json::from_slice(&fs::read(state).unwrap()).expect("a whole state file")
    } else {
        json!({"files": {}})
    };
    let leftovers: Vec<String> = fs::read_dir(state.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    value
}

/// A later push brings the receiver's copies level with the originals,
/// without sending any file again whole.
fn assert_resumes(home: &Home, receiver: &Receiver, logs: &[(&str, String)]) {
    {
        let mut received = receiver.state.lock().unwrap();
        received.hang_after = None;
        received.stop_after = None;
        received.refuse_after = None;
    }
    semon_push::push(&home.push_options(&receiver.url), false).unwrap();
    for (relative, text) in logs {
        assert_eq!(copy(receiver, relative), redacted(text), "{relative}");
    }
    assert_eq!(receiver.state.lock().unwrap().replaces, 0);
}

fn two_logs(home: &Home) -> Vec<(&'static str, String)> {
    let logs = vec![
        ("claude/projects/-work/first.jsonl", line("first")),
        ("claude/projects/-work/second.jsonl", line("second")),
    ];
    for (relative, text) in &logs {
        home.write(relative, text);
    }
    logs
}

#[test]
fn a_stop_ends_the_sleep_between_passes() {
    let home = Home::new();
    let receiver = receiver();
    let logs = two_logs(&home);
    let options = home.push_options(&receiver.url);
    let stop = Stop::new();
    let push = watch_in_background(&options, &stop);
    // Facts go after the first pass; then the watch sleeps for 2 s.
    wait_for("the first facts", || {
        receiver.state.lock().unwrap().facts.is_some()
    });
    thread::sleep(Duration::from_millis(100));
    stop_and_join(&stop, push).unwrap();
    let state = assert_stopped_cleanly(&options);
    assert_eq!(state["files"].as_object().unwrap().len(), logs.len());
    assert_resumes(&home, &receiver, &logs);
}

#[test]
fn a_stop_midway_through_a_pass_leaves_the_rest_for_the_next_push() {
    let home = Home::new();
    let receiver = receiver();
    let logs = two_logs(&home);
    let options = home.push_options(&receiver.url);
    let stop = Stop::new();
    receiver.state.lock().unwrap().stop_after = Some((1, stop.clone()));
    let push = watch_in_background(&options, &stop);
    wait_for("the stop", || stop.is_stopped());
    stop_and_join(&stop, push).unwrap();
    {
        let received = receiver.state.lock().unwrap();
        assert_eq!(received.appends, 1, "nothing is sent after the stop");
        assert!(received.facts.is_none(), "nor facts");
    }
    // The first file is recorded when its answer came before the stop was
    // seen, and not otherwise; either way the next push resumes.
    let state = assert_stopped_cleanly(&options);
    assert!(state["files"].as_object().unwrap().len() <= 1);
    assert_resumes(&home, &receiver, &logs);
}

#[test]
fn a_stop_mid_upload_returns_at_once_and_keeps_the_lock_until_the_request_ends() {
    let home = Home::new();
    let receiver = receiver();
    let logs = two_logs(&home);
    let options = home.push_options(&receiver.url);
    let stop = Stop::new();
    receiver.state.lock().unwrap().hang_after = Some(1);
    let push = watch_in_background(&options, &stop);
    wait_for("an unanswered append", || {
        receiver.state.lock().unwrap().held == 1
    });
    stop_and_join(&stop, push).unwrap();

    // The request is still out: no other push may start and overlap it,
    // and the refusal says why.
    let started = Instant::now();
    let error = semon_push::push(&options, false).unwrap_err();
    assert!(started.elapsed() < STOP_BOUND, "{:?}", started.elapsed());
    assert!(error.contains("still finishing"), "{error}");
    let error = StateLock::acquire(&options.state).unwrap_err();
    assert!(error.contains("still finishing"), "{error}");
    thread::sleep(Duration::from_millis(200));
    assert!(
        StateLock::acquire(&options.state).is_err(),
        "held while out"
    );

    // The receiver lets the request go unanswered: it ends, and so does
    // the lock.
    receiver.state.lock().unwrap().held_requests.clear();
    assert_lock_freed(&options);

    // The answered file is recorded; the unanswered one isn't.
    let state = state_after_stop(&options);
    let files = state["files"].as_object().unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    let (key, file) = files.iter().next().unwrap();
    let text = &logs
        .iter()
        .find(|(relative, _)| *relative == key.as_str())
        .unwrap()
        .1;
    assert_eq!(file["sent"], text.len());
    assert_resumes(&home, &receiver, &logs);
}

#[test]
fn a_second_push_on_the_same_state_fails_at_once_until_the_first_stops() {
    let home = Home::new();
    let receiver = receiver();
    let logs = two_logs(&home);
    let options = home.push_options(&receiver.url);
    let stop = Stop::new();
    let push = watch_in_background(&options, &stop);
    wait_for("the first facts", || {
        receiver.state.lock().unwrap().facts.is_some()
    });

    let started = Instant::now();
    let error = semon_push::push(&options, false).unwrap_err();
    assert!(started.elapsed() < STOP_BOUND, "{:?}", started.elapsed());
    assert!(error.contains("already running"), "{error}");
    let lock = StateLock::path_for(&options.state);
    assert!(error.contains(&lock.display().to_string()), "{error}");
    let error = semon_push::push_until(&options, true, &Stop::new()).unwrap_err();
    assert!(error.contains("already running"), "{error}");
    assert!(StateLock::acquire(&options.state).is_err());

    stop_and_join(&stop, push).unwrap();
    assert_stopped_cleanly(&options);
    assert_resumes(&home, &receiver, &logs);
}

/// The stop comes once the push has read the 401: a refused pass saves
/// the state file (replacing it, so its inode changes) before it returns,
/// and only after the answer is in. Which of the refusal and the stop the
/// watch sees first is then up to it, and either way the refusal is the
/// result (the ordering itself is `after_pass`'s unit test).
#[cfg(unix)]
#[test]
fn a_refused_token_is_reported_even_when_a_stop_comes_with_it() {
    use std::os::unix::fs::MetadataExt;

    let home = Home::new();
    let receiver = receiver();
    home.write(LOG, &line("one"));
    let options = home.push_options(&receiver.url);
    let stop = Stop::new();
    receiver.state.lock().unwrap().refuse_after = Some(1);
    let push = watch_in_background(&options, &stop);
    wait_for("the first facts", || {
        receiver.state.lock().unwrap().facts.is_some()
    });
    let inode = || {
        fs::metadata(&options.state)
            .map(|metadata| metadata.ino())
            .ok()
    };
    let before = inode();
    assert!(before.is_some(), "the first pass saved its state");
    // The next pass is refused; the stop follows the save that refusal makes.
    home.append(LOG, &line("two"));
    wait_for("the refused pass's save", || inode() != before);
    let error = stop_and_join(&stop, push).unwrap_err();
    assert!(error.contains("refused the token"), "{error}");
    assert_stopped_cleanly(&options);
}

#[test]
fn spellings_of_one_receiver_share_one_state_file_and_lock() {
    assert_eq!(
        semon_push::check_url("HTTPS://Mirror.Example/api/Push/").unwrap(),
        "https://mirror.example/api/Push"
    );
    assert_eq!(
        semon_push::check_url("http://LOCALHOST:8735").unwrap(),
        "http://localhost:8735"
    );
    let state = semon_push::default_state_path("https://mirror.example");
    for spelling in [
        "https://mirror.example/",
        "HTTPS://MIRROR.EXAMPLE",
        "https://Mirror.Example//",
    ] {
        assert_eq!(
            semon_push::default_state_path(spelling),
            state,
            "{spelling}"
        );
    }
    assert_ne!(
        semon_push::default_state_path("https://mirror.example/other"),
        state
    );
}

#[test]
fn an_in_memory_token_is_used_and_never_written_or_shown() {
    let home = Home::new();
    fs::remove_file(home.root.join("token")).unwrap();
    let receiver = receiver();
    home.write(LOG, &line("hello"));
    let token = Token::new(&format!(" {TOKEN}\n")).unwrap();
    let options = PushOptions {
        credential: Credential::Memory(token.clone()),
        ..home.push_options(&receiver.url)
    };
    semon_push::push(&options, false).unwrap();
    assert!(receiver.state.lock().unwrap().facts.is_some());
    assert_eq!(copy(&receiver, LOG), redacted(&line("hello")));

    for path in files_under(&home.root) {
        let bytes = fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(TOKEN.len())
                .any(|window| window == TOKEN.as_bytes()),
            "{} holds the token",
            path.display()
        );
    }
    for shown in [
        format!("{token:?}"),
        format!("{:?}", options.credential),
        format!("{options:?}"),
        format!("{options:#?}"),
    ] {
        assert!(!shown.contains(TOKEN), "{shown}");
        assert!(shown.contains("Token(<redacted>)"), "{shown}");
    }

    let wrong = PushOptions {
        credential: Credential::Memory(Token::new("wrong-token").unwrap()),
        ..home.push_options(&receiver.url)
    };
    home.append(LOG, &line("more"));
    let error = semon_push::push(&wrong, false).unwrap_err();
    assert!(error.contains("refused the token"), "{error}");
    assert!(!error.contains("wrong-token"), "{error}");
    for bad in ["", "  ", "two words", "a\tb", "line\nbreak"] {
        assert!(Token::new(bad).is_err(), "{bad:?}");
    }
}

fn files_under(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn copied_claude_usage_has_redacted_http_mirror_and_restart_parity() {
    let mut home = Home::new();
    home.options.all = true;
    let receiver = receiver();
    let mirrored = Home::new();
    let expected = Home::new();
    let secret = "sk-ant-api03-SECRETSECRETSECRETSECRET";
    let files = [
        (
            "claude/projects/fixture/native-claude-parent.jsonl",
            include_str!(
                "../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/resumed-transcript.jsonl"
            ),
        ),
        (
            "claude/projects/fixture/native-claude-child.jsonl",
            include_str!(
                "../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/forked-transcript.jsonl"
            ),
        ),
    ];
    for (path, text) in files {
        home.write(path, &text.replace("SYNTHETIC_CLAUDE_ACK", secret));
    }
    for _ in 0..2 {
        semon_push::push(&home.push_options(&receiver.url), false).unwrap();
        let (received, facts) = {
            let state = receiver.state.lock().unwrap();
            (state.files.clone(), state.facts.clone().unwrap())
        };
        for (path, bytes) in received {
            assert!(!String::from_utf8_lossy(&bytes).contains(secret));
            let destination = mirrored.root.join(&path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(destination, bytes).unwrap();
        }
        for (path, _) in files {
            let source = fs::read_to_string(home.root.join(path)).unwrap();
            assert!(source.contains(secret));
            let destination = expected.root.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(destination, redacted(&source)).unwrap();
        }
        let configure = |home: &Home| {
            let options = semon_sessions::Options {
                all: true,
                facts: Some(home.root.join("facts.json")),
                ..home.options.clone()
            };
            semon_sessions::write_facts(options.facts.as_ref().unwrap(), &facts).unwrap();
            options
        };
        let local = configure(&expected);
        let remote = configure(&mirrored);
        let now = 1_791_072_000_000;
        assert_eq!(
            semon_sessions::model_json_at(&local, now).unwrap(),
            semon_sessions::model_json_at(&remote, now).unwrap()
        );
        let json: serde_json::Value =
            serde_json::from_str(&semon_sessions::model_json_at(&remote, now).unwrap()).unwrap();
        assert_eq!(
            json["sessions"]["native-claude-child"]["claude_usage"]["exclusive"]["input"],
            5
        );
        assert!(json["sessions"]["native-claude-child"]["claude_usage"]["shared_owner"].is_null());
        let core = semon_sessions::ViewerCore::new(remote);
        let expected_core = semon_sessions::ViewerCore::new(local);
        for id in ["native-claude-parent", "native-claude-child"] {
            for (path, query) in [
                ("/api/tx", format!("sid={id}")),
                ("/api/transcript", format!("harness=claude&id={id}")),
            ] {
                let actual = core.respond("GET", path, &query, None);
                let wanted = expected_core.respond("GET", path, &query, None);
                assert_eq!(actual.status, 200);
                assert_eq!(actual.body, wanted.body);
            }
        }
    }
}

#[test]
fn copilot_redacted_mirror_survives_lost_ack_restart_partial_and_replacement() {
    for version in ["1.0.90", "1.0.91"] {
        let mut home = Home::new();
        home.options.all = true;
        let receiver = receiver();
        let mirrored = Home::new();
        let expected = Home::new();
        let read = |name: &str| {
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/fixtures/compatibility")
                    .join(format!("copilot-{version}/lifecycle/{name}.events.jsonl")),
            )
            .unwrap()
            .replace("SEMON_SYNTHETIC_initial", "Bearer SECRETSECRETSECRETSECRET")
        };
        let initial = read("initial");
        let resumed = read("resumed");
        let header: serde_json::Value =
            serde_json::from_str(initial.lines().next().unwrap()).unwrap();
        let id = header["data"]["sessionId"].as_str().unwrap();
        let path = format!("copilot/session-state/{id}/events.jsonl");
        home.write(&path, &initial);
        home.write(
            "copilot/session-store.db",
            "Bearer SECRETSECRETSECRETSECRET",
        );
        home.write(
            &format!("copilot/session-state/{id}/workspace.yaml"),
            "Bearer SECRETSECRETSECRETSECRET",
        );
        receiver.state.lock().unwrap().drop_ack_once = true;
        assert!(
            semon_push::push(&home.push_options(&receiver.url), false).is_err(),
            "lost ACK is not acknowledged state"
        );
        assert_eq!(copy(&receiver, &path), redacted(&initial));
        semon_push::push(&home.push_options(&receiver.url), false).unwrap();
        assert!(
            receiver.state.lock().unwrap().conflicts > 0,
            "restart reconciles the committed unacknowledged prefix"
        );
        let partial = resumed.len() - resumed.lines().last().unwrap().len() / 2;
        home.write(&path, &resumed[..partial]);
        semon_push::push(&home.push_options(&receiver.url), false).unwrap();
        assert!(copy(&receiver, &path).ends_with(b"\n"));
        for source in [
            &resumed,
            &resumed.replace("SEMON_SYNTHETIC_resumed", "SEMON_SYNTHETIC_changed"),
        ] {
            home.write(&path, source);
            semon_push::push(&home.push_options(&receiver.url), false).unwrap();
            // A fresh Client/cursor-state load tests restart and duplicate replay.
            semon_push::push(&home.push_options(&receiver.url), false).unwrap();
            let (received, facts) = {
                let state = receiver.state.lock().unwrap();
                (state.files.clone(), state.facts.clone().unwrap())
            };
            assert_eq!(
                received.len(),
                1,
                "only exact authoritative Copilot events may cross the wire"
            );
            assert_eq!(received[&path], redacted(source));
            assert!(!String::from_utf8_lossy(&received[&path]).contains("SECRETSECRET"));
            assert_eq!(fs::read_to_string(home.root.join(&path)).unwrap(), *source);
            for (destination, bytes) in [
                (&mirrored.root, received[&path].clone()),
                (&expected.root, redacted(source)),
            ] {
                let file = destination.join(&path);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, bytes).unwrap();
            }
            let configure = |home: &Home| {
                let options = Options {
                    all: true,
                    facts: Some(home.root.join("facts.json")),
                    ..home.options.clone()
                };
                semon_sessions::write_facts(options.facts.as_ref().unwrap(), &facts).unwrap();
                options
            };
            let local = configure(&expected);
            let remote = configure(&mirrored);
            let now = 1_791_072_000_000;
            assert_eq!(
                semon_sessions::model_json_at(&local, now).unwrap(),
                semon_sessions::model_json_at(&remote, now).unwrap()
            );
            let local = semon_sessions::ViewerCore::new(local);
            let remote = semon_sessions::ViewerCore::new(remote);
            for (route, query) in [
                ("/api/tx", format!("sid={id}")),
                ("/api/transcript", format!("harness=copilot&id={id}")),
            ] {
                let actual = remote.respond("GET", route, &query, None);
                let wanted = local.respond("GET", route, &query, None);
                assert_eq!(actual.status, 200);
                assert_eq!(actual.body, wanted.body);
            }
        }
    }
}
