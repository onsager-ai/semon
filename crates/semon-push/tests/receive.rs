//! `semon receive`'s listener over HTTP: each answer a client can get, from
//! raw requests on loopback.

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use semon_push::{
    mirror::MAX_BODY_BYTES,
    serve::{ServeOptions, Server},
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
    let server = Server::bind(&ServeOptions {
        dir: dir.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        tls: None,
    })
    .unwrap();
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
    exchange(address, &bytes)
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
