//! `semon receive`'s listener over HTTP: each answer a client can get, from
//! raw requests on loopback, over plain HTTP and TLS, and the deadlines and
//! limits that keep a slow or greedy client from holding it. Tests that
//! wait on a deadline use short limits rather than the real ones.

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use semon_push::{
    mirror::MAX_BODY_BYTES,
    serve::{Limits, ServeOptions, Server, TlsFiles},
    tokens::{add_token, revoke_token},
    wire::{Append, base64_encode, head_sha256},
};
use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Receiving {
    dir: PathBuf,
    address: SocketAddr,
}

impl Drop for Receiving {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A receiver on a free loopback port, with a token for each machine.
fn receiving(machines: &[&str]) -> (Receiving, Vec<String>) {
    receiving_with(machines, Limits::default(), false)
}

/// The test-only certificate and key (tests/fixtures/README.md).
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Limits short enough to wait out in a test.
fn short() -> Limits {
    Limits {
        head: Duration::from_secs(1),
        linger: Duration::from_millis(200),
        ..Limits::default()
    }
}

fn receiving_with(machines: &[&str], limits: Limits, tls: bool) -> (Receiving, Vec<String>) {
    let dir = std::env::temp_dir().join(format!(
        "semon-receive-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    let tokens = machines
        .iter()
        .map(|name| add_token(&dir, name).unwrap())
        .collect();
    let mut options = ServeOptions::new(&dir);
    options.listen = "127.0.0.1:0".parse().unwrap();
    if tls {
        options.tls = Some(TlsFiles {
            certificate: fixture("test-cert.pem"),
            private_key: fixture("test-key.pem"),
        });
    }
    let server = Server::bind(&options).unwrap().with_limits(limits);
    let address = server.local_addr().unwrap();
    thread::spawn(move || server.run());
    (Receiving { dir, address }, tokens)
}

struct Answer {
    status: u16,
    head: String,
    body: Value,
}

fn parse(raw: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").expect("a response head");
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("a status");
    Answer {
        status,
        head: head.to_owned(),
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    }
}

/// Sends `request` as is and reads the answer until the server closes.
fn exchange(address: SocketAddr, request: &[u8]) -> Answer {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    parse(&raw)
}

fn post(address: SocketAddr, route: &str, token: Option<&str>, body: &[u8]) -> Answer {
    exchange(address, &request(address, route, token, body))
}

/// A POST that closes the connection after its answer.
fn request(address: SocketAddr, route: &str, token: Option<&str>, body: &[u8]) -> Vec<u8> {
    let mut request = format!(
        "POST {route} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    let mut bytes = request.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// The answers in `raw`, one after another, each by its Content-Length.
fn answers(raw: &[u8]) -> Vec<Answer> {
    let mut found = Vec::new();
    let mut rest = raw;
    while let Some(end) = rest.windows(4).position(|window| window == b"\r\n\r\n") {
        let head = String::from_utf8_lossy(&rest[..end]).to_string();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .map_or(0, |value| value.trim().parse().unwrap());
        let total = (end + 4 + length).min(rest.len());
        found.push(parse(&rest[..total]));
        rest = &rest[total..];
    }
    found
}

/// A rustls client that trusts the test certificate alone.
fn tls_client(address: SocketAddr) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
    let mut roots = rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_file_iter(fixture("test-cert.pem")).unwrap() {
        roots.add(certificate.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = ServerName::try_from("127.0.0.1").unwrap().to_owned();
    let connection = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    rustls::StreamOwned::new(connection, socket)
}

/// Writes `bytes` one at a time, every 50 ms, until `stop` or an error.
fn trickle(mut socket: TcpStream, bytes: Vec<u8>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        for byte in bytes {
            if stop.load(Ordering::Relaxed) || socket.write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
}

/// How long until the server ends `socket` (EOF or reset), reading and
/// dropping what it sends.
fn time_to_close(mut socket: TcpStream, started: Instant) -> Duration {
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut sink = [0u8; 4096];
    loop {
        match socket.read(&mut sink) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                panic!("still open after {:?}", started.elapsed());
            }
            Err(_) => break,
        }
    }
    started.elapsed()
}

fn append_body(path: &str, before: &[u8], offset: u64, bytes: &[u8]) -> Vec<u8> {
    let mut head = before.to_vec();
    head.extend_from_slice(bytes);
    serde_json::to_vec(&Append {
        root: "claude".into(),
        path: path.into(),
        offset,
        head_sha256: head_sha256(&head),
        bytes: base64_encode(bytes),
        replace: false,
    })
    .unwrap()
}

const LOG: &str = "projects/-work/lane.jsonl";
const APPEND: &str = "/v1/mirror/append";

#[test]
fn an_append_is_answered_over_http() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let answer = post(
        receiving.address,
        APPEND,
        Some(&tokens[0]),
        &append_body(LOG, b"", 0, b"one\n"),
    );
    assert_eq!(answer.status, 200, "{}", answer.head);
    assert_eq!(answer.body, json!({"length": 4}));
    assert!(answer.head.contains("Content-Type: application/json"));
    let answer = post(
        receiving.address,
        APPEND,
        Some(&tokens[0]),
        &append_body(LOG, b"", 0, b"one\n"),
    );
    assert_eq!(answer.status, 409);
    assert_eq!(
        answer.body,
        json!({"length": 4, "head_sha256": head_sha256(b"one\n")})
    );
    let copy = receiving.dir.join("machines/laptop/claude").join(LOG);
    assert_eq!(fs::read(copy).unwrap(), b"one\n");
}

#[test]
fn unknown_and_revoked_tokens_are_401() {
    let (receiving, tokens) = receiving(&["laptop", "desk"]);
    let body = append_body(LOG, b"", 0, b"one\n");
    for token in [None, Some("nope"), Some("")] {
        let answer = post(receiving.address, APPEND, token, &body);
        assert_eq!(answer.status, 401, "{token:?}");
        assert!(answer.head.contains("WWW-Authenticate: Bearer"));
    }
    // A token in another scheme is no token.
    let request = format!(
        "POST {APPEND} HTTP/1.1\r\nAuthorization: Basic {}\r\nContent-Length: 0\r\n\r\n",
        tokens[0]
    );
    assert_eq!(exchange(receiving.address, request.as_bytes()).status, 401);
    assert_eq!(
        post(receiving.address, APPEND, Some(&tokens[0]), &body).status,
        200
    );

    // Revoked while the receiver runs: refused from the next request on.
    revoke_token(&receiving.dir, "laptop").unwrap();
    let more = append_body(LOG, b"one\n", 4, b"two\n");
    assert_eq!(
        post(receiving.address, APPEND, Some(&tokens[0]), &more).status,
        401
    );
    let facts = json!({"version": 2, "hostname": "laptop", "home": null, "proc_starts": {}, "codex_locks": {}, "repos": {}});
    let facts = facts.to_string().into_bytes();
    assert_eq!(
        post(
            receiving.address,
            "/v1/mirror/facts",
            Some(&tokens[0]),
            &facts
        )
        .status,
        401
    );
    // The other machine's token still works.
    assert_eq!(
        post(
            receiving.address,
            "/v1/mirror/facts",
            Some(&tokens[1]),
            &facts
        )
        .status,
        200
    );
}

#[test]
fn an_oversize_body_is_413_before_it_is_read() {
    let (receiving, tokens) = receiving(&["laptop"]);
    // Only the head is sent: the answer can't wait for the body.
    let request = format!(
        "POST {APPEND} HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n",
        tokens[0],
        MAX_BODY_BYTES + 1
    );
    let answer = exchange(receiving.address, request.as_bytes());
    assert_eq!(answer.status, 413);
    assert!(answer.head.contains("Connection: close"));
    // Without a token it is 401 first: nothing is read for an unknown token.
    let request = format!(
        "POST {APPEND} HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY_BYTES + 1
    );
    assert_eq!(exchange(receiving.address, request.as_bytes()).status, 401);
    // A chunked body, whose size isn't declared, is refused.
    let request = format!(
        "POST {APPEND} HTTP/1.1\r\nAuthorization: Bearer {}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        tokens[0]
    );
    assert_eq!(exchange(receiving.address, request.as_bytes()).status, 411);
}

#[test]
fn only_post_to_the_two_endpoints() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let answer = exchange(
        receiving.address,
        format!(
            "GET {APPEND} HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n",
            tokens[0]
        )
        .as_bytes(),
    );
    assert_eq!(answer.status, 405);
    assert!(answer.head.contains("Allow: POST"));
    for route in [
        "/",
        "/v1/mirror",
        "/v1/mirror/append/",
        "/v1/mirror/append?x=1",
        "/v1/frames",
    ] {
        assert_eq!(
            post(receiving.address, route, Some(&tokens[0]), b"{}").status,
            404,
            "{route}"
        );
        let get = format!("GET {route} HTTP/1.1\r\n\r\n");
        assert_eq!(
            exchange(receiving.address, get.as_bytes()).status,
            404,
            "{route}"
        );
    }
    assert_eq!(exchange(receiving.address, b"GARBAGE\r\n\r\n").status, 400);
    assert_eq!(
        exchange(receiving.address, b"POST / HTTP/2.0\r\n\r\n").status,
        505
    );
}

#[test]
fn malformed_bodies_are_400() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let bodies: [&[u8]; 5] = [b"", b"{", b"[1,2]", b"\"text\"", b"\xff"];
    for body in bodies {
        for route in [APPEND, "/v1/mirror/facts"] {
            let answer = post(receiving.address, route, Some(&tokens[0]), body);
            assert_eq!(answer.status, 400, "{route} {body:?}");
            assert!(answer.body["error"].is_string());
        }
    }
    let outside = append_body("../../x.jsonl", b"", 0, b"x\n");
    assert_eq!(
        post(receiving.address, APPEND, Some(&tokens[0]), &outside).status,
        400
    );
    // The receiver is still serving.
    let good = append_body(LOG, b"", 0, b"x\n");
    assert_eq!(
        post(receiving.address, APPEND, Some(&tokens[0]), &good).status,
        200
    );
}

#[test]
fn a_token_writes_only_its_own_machine() {
    let (receiving, tokens) = receiving(&["laptop", "desk"]);
    let desk = append_body(LOG, b"", 0, b"desk\n");
    assert_eq!(
        post(receiving.address, APPEND, Some(&tokens[1]), &desk).status,
        200
    );
    // The laptop's token, the same path: the laptop's own copy, which is
    // empty, so an append that assumes the desk's bytes is a 409.
    let onto_desk = append_body(LOG, b"desk\n", 5, b"more\n");
    let answer = post(receiving.address, APPEND, Some(&tokens[0]), &onto_desk);
    assert_eq!(answer.status, 409);
    assert_eq!(answer.body["length"], 0);
    let laptop = append_body(LOG, b"", 0, b"laptop\n");
    assert_eq!(
        post(receiving.address, APPEND, Some(&tokens[0]), &laptop).status,
        200
    );
    let machines = receiving.dir.join("machines");
    assert_eq!(
        fs::read(machines.join("desk/claude").join(LOG)).unwrap(),
        b"desk\n"
    );
    assert_eq!(
        fs::read(machines.join("laptop/claude").join(LOG)).unwrap(),
        b"laptop\n"
    );
}

#[test]
fn a_kept_alive_connection_serves_several_requests() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let mut stream = TcpStream::connect(receiving.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut before = Vec::new();
    let lines: [&[u8]; 3] = [b"one\n", b"two\n", b"three\n"];
    for line in lines {
        let body = append_body(LOG, &before, before.len() as u64, line);
        let request = format!(
            "POST {APPEND} HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n",
            tokens[0],
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        before.extend_from_slice(line);
        // Read one response: its head, then Content-Length bytes.
        let mut raw = Vec::new();
        let mut byte = [0u8];
        while !raw.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            raw.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&raw).to_string();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).unwrap();
        raw.extend_from_slice(&body);
        let answer = parse(&raw);
        assert_eq!(answer.status, 200, "{head}");
        assert_eq!(answer.body["length"], before.len());
    }
}

#[test]
fn an_append_is_answered_over_tls() {
    let (receiving, tokens) = receiving_with(&["laptop"], Limits::default(), true);
    let mut stream = tls_client(receiving.address);
    let body = append_body(LOG, b"", 0, b"one\n");
    stream
        .write_all(&request(receiving.address, APPEND, Some(&tokens[0]), &body))
        .unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let answer = parse(&raw);
    assert_eq!(answer.status, 200, "{}", answer.head);
    assert_eq!(answer.body, json!({"length": 4}));
    let copy = receiving.dir.join("machines/laptop/claude").join(LOG);
    assert_eq!(fs::read(copy).unwrap(), b"one\n");
}

#[test]
fn a_trickled_tls_handshake_ends_at_the_head_deadline() {
    let (receiving, _tokens) = receiving_with(&["laptop"], short(), true);
    let socket = TcpStream::connect(receiving.address).unwrap();
    let started = Instant::now();
    // A handshake record of 512 bytes, sent a byte every 50 ms: each read
    // returns at once, so only the deadline can end it.
    let mut hello = vec![
        0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00, 0x01, 0xfc, 0x03, 0x03,
    ];
    hello.resize(5 + 512, 0);
    let stop = Arc::new(AtomicBool::new(false));
    trickle(socket.try_clone().unwrap(), hello, Arc::clone(&stop));
    let elapsed = time_to_close(socket, started);
    stop.store(true, Ordering::Relaxed);
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(4),
        "closed after {elapsed:?}; the head deadline is 1 s"
    );
}

#[test]
fn a_trickled_head_gets_408_at_the_head_deadline() {
    let (receiving, _tokens) = receiving_with(&["laptop"], short(), false);
    let mut socket = TcpStream::connect(receiving.address).unwrap();
    let started = Instant::now();
    socket
        .write_all(format!("POST {APPEND} HTTP/1.1\r\n").as_bytes())
        .unwrap();
    let mut slow = b"X-Slow: ".to_vec();
    slow.resize(400, b'a');
    let stop = Arc::new(AtomicBool::new(false));
    trickle(socket.try_clone().unwrap(), slow, Arc::clone(&stop));
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut raw = Vec::new();
    let _ = socket.read_to_end(&mut raw);
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    assert_eq!(parse(&raw).status, 408);
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(4),
        "answered after {elapsed:?}; the head deadline is 1 s"
    );
}

#[test]
fn one_address_gets_at_most_its_share_of_connections() {
    let limits = Limits {
        connections_per_ip: 2,
        ..short()
    };
    let (receiving, tokens) = receiving_with(&["laptop"], limits, false);
    let first = TcpStream::connect(receiving.address).unwrap();
    let _second = TcpStream::connect(receiving.address).unwrap();
    let started = Instant::now();
    let third = TcpStream::connect(receiving.address).unwrap();
    let elapsed = time_to_close(third, started);
    assert!(
        elapsed < Duration::from_millis(900),
        "a third connection from one address is closed at once, not after {elapsed:?}"
    );
    // Once one ends, a new one is served.
    drop(first);
    let body = append_body(LOG, b"", 0, b"one\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut socket = TcpStream::connect(receiving.address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = socket.write_all(&request(receiving.address, APPEND, Some(&tokens[0]), &body));
        let mut raw = Vec::new();
        let _ = socket.read_to_end(&mut raw);
        if !raw.is_empty() {
            assert_eq!(parse(&raw).status, 200);
            break;
        }
        assert!(Instant::now() < deadline, "the freed slot was never reused");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn expect_100_continue_is_answered_only_once_the_request_may_go_on() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let body = append_body(LOG, b"", 0, b"one\n");
    let head = |token: &str| {
        format!(
            "POST {APPEND} HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        )
    };
    let mut socket = TcpStream::connect(receiving.address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    socket.write_all(head(&tokens[0]).as_bytes()).unwrap();
    let mut interim = Vec::new();
    let mut byte = [0u8];
    while !interim.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        interim.push(byte[0]);
    }
    assert_eq!(interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    socket.write_all(&body).unwrap();
    let mut raw = Vec::new();
    let _ = socket.read_to_end(&mut raw);
    assert_eq!(parse(&raw).status, 200);
    // A request that will be refused gets the refusal, not 100.
    let answer = exchange(receiving.address, head("nope").as_bytes());
    assert_eq!(answer.status, 401);
}

#[test]
fn pipelined_requests_are_answered_in_order() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let first = append_body(LOG, b"", 0, b"one\n");
    let second = append_body(LOG, b"one\n", 4, b"two\n");
    let keep = format!(
        "POST {APPEND} HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n",
        tokens[0],
        first.len()
    );
    let mut both = keep.into_bytes();
    both.extend_from_slice(&first);
    both.extend_from_slice(&request(
        receiving.address,
        APPEND,
        Some(&tokens[0]),
        &second,
    ));
    let mut socket = TcpStream::connect(receiving.address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    socket.write_all(&both).unwrap();
    let mut raw = Vec::new();
    let _ = socket.read_to_end(&mut raw);
    let answers = answers(&raw);
    assert_eq!(answers.len(), 2, "{}", String::from_utf8_lossy(&raw));
    assert_eq!(answers[0].body, json!({"length": 4}));
    assert_eq!(answers[1].body, json!({"length": 8}));
}

#[test]
fn a_post_without_a_length_is_411() {
    let (receiving, tokens) = receiving(&["laptop"]);
    let request = format!(
        "POST {APPEND} HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n",
        tokens[0]
    );
    assert_eq!(exchange(receiving.address, request.as_bytes()).status, 411);
}
