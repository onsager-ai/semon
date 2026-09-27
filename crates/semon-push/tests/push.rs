//! `semon push` against a tiny in-process receiver that implements the
//! mirror protocol (docs/mirror-protocol.md) as any receiver would.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use semon_push::{
    Client, PushOptions, read_token,
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
        .with_status_code(status)
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
            token_file: self.root.join("token"),
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
        "http://localhost:1",
    ] {
        assert!(semon_push::check_url(good).is_ok(), "{good}");
    }
    for bad in [
        "http://mirror.example/api/push",
        "ftp://127.0.0.1",
        "https://user:pw@mirror.example",
        "https://mirror.example/?x=1",
        "mirror.example",
    ] {
        assert!(semon_push::check_url(bad).is_err(), "{bad}");
    }
}
