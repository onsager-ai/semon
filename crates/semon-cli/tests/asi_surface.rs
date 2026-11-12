#![cfg(feature = "asi-query")]

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;

use semon_asi::{Manifest, Store};
use serde_json::{Value, json};

fn store() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state/analytics-store");
    Store::open(&root).unwrap();
    let mut manifest = Manifest::empty();
    manifest.origin_incarnations.insert(
        format!("origin-{}", "0".repeat(64)),
        format!("producer-{}", "a".repeat(64)),
    );
    std::fs::write(
        root.join("analytics/manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("state/analytics-namespace"),
        serde_json::json!({"format":"semon-producer-1", "namespace":format!("origin-{}", "0".repeat(64)), "origin_incarnation":format!("producer-{}", "a".repeat(64))}).to_string(),
    )
    .unwrap();
    dir
}

#[test]
fn durable_backup_cli_copies_and_refuses_existing_destination() {
    let dir = store();
    let root = dir.path().join("state/analytics-store");
    {
        let mut writer = semon_asi::durable::Writer::open(&root).unwrap();
        writer
            .insert(
                "lane",
                "one",
                std::collections::BTreeMap::from([(
                    "title".into(),
                    semon_asi::durable::Value::Text("private title".into()),
                )]),
            )
            .unwrap();
    }
    let destination = dir.path().join("backup.sqlite3");
    let path = destination.to_str().unwrap();
    let cache = dir.path().join("state/index.json");
    assert!(
        cli(&["asi", "backup", path], cache.clone())
            .status
            .success()
    );
    let bytes = std::fs::read(&destination).unwrap();
    let output = cli(&["asi", "backup", path], cache.clone());
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(bytes, std::fs::read(&destination).unwrap());
    let help = cli(&["asi", "--help"], cache);
    assert_eq!(help.status.code(), Some(1));
    assert!(
        String::from_utf8(help.stderr)
            .unwrap()
            .contains("Usage: semon asi backup PATH")
    );
}

#[test]
fn durable_add_cli_file_and_stdin_round_trip_without_echoing_bodies() {
    let home = fixture::Fixture::new();
    let cache = home.root.join("state/index.json");
    let output = home
        .command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "materialize", "--all", "--proc-root"])
        .arg(home.root.join("empty-proc"))
        .arg("--cache")
        .arg(&cache)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("values.json");
    let body = "private author text with spaces and punctuation!";
    for (table, values) in [
        ("lane", json!({"title":body})),
        (
            "intervention",
            json!({"hypothesis":body,"guardrails":body,"lever":"review"}),
        ),
    ] {
        std::fs::write(&input, values.to_string()).unwrap();
        let output = cli(
            &[
                "asi",
                "add",
                table,
                "--id",
                "one",
                "--values",
                input.to_str().unwrap(),
            ],
            cache.clone(),
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["id"],
            "one"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(body));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(body));
        let output = cli(&["query", "view", &format!("{table}/one")], cache.clone());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(body));
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_semon"))
        .args(["asi", "add", "lane", "--id", "stdin", "--values", "-"])
        .args(cli_args(cache.clone()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(json!({"title":body}).to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(body));
    std::fs::write(&input, json!({"title":body}).to_string()).unwrap();
    let output = cli(
        &[
            "asi",
            "add",
            "lane",
            "--id",
            "one",
            "--values",
            input.to_str().unwrap(),
        ],
        cache.clone(),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("UNIQUE"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(body));
    let _lock = semon_asi::WriterLock::acquire(&home.root.join("state/analytics-store")).unwrap();
    let output = cli(
        &[
            "asi",
            "add",
            "lane",
            "--id",
            "locked",
            "--values",
            input.to_str().unwrap(),
        ],
        cache,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("writer busy"));
}

#[test]
fn durable_add_label_is_insert_only_and_keeps_writer_validation() {
    let dir = store();
    let cache = dir.path().join("state/index.json");
    let input = dir.path().join("label.json");
    let body = "private label mechanism text";
    let mut values = json!({"target_type":"lane","target_uid":"lane/one","field":"quality","value":"good","mechanism_text":body,"evidence_step_ids":"[]","labeler":"human:author","taxonomy_version":1,"created_at":1});
    std::fs::write(&input, values.to_string()).unwrap();
    let args = [
        "asi",
        "add",
        "label",
        "--id",
        "label-one",
        "--values",
        input.to_str().unwrap(),
    ];
    assert!(cli(&args, cache.clone()).status.success());
    let output = cli(&args, cache.clone());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("UNIQUE"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(body));
    values["value"] = json!(body);
    std::fs::write(&input, values.to_string()).unwrap();
    let output = cli(&args, cache);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("identifier shape"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(body));
}

#[test]
fn durable_add_mcp_uses_local_writer_and_refuses_paths_and_duplicates() {
    let dir = store();
    let mut server = Server::start(dir.path().join("state/index.json"));
    let body = "private MCP hypothesis body";
    let args = json!({"table":"intervention","id":"mcp","values":{"hypothesis":body}});
    let request = |id, arguments| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"asi_add","arguments":arguments}});
    let reply = server.request(request(1, args.clone()));
    let (value, failed) = content(&reply);
    assert!(!failed, "{reply}");
    assert_eq!(value, json!({"id":"mcp","revision":1}));
    assert!(!reply.to_string().contains(body));
    let reply = server.request(request(2, args.clone()));
    let (error, failed) = content(&reply);
    assert!(failed);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("UNIQUE")
    );
    assert!(!reply.to_string().contains(body));
    let mut path_args = args.clone();
    path_args["cache"] = json!(dir.path().join("elsewhere"));
    let reply = server.request(request(3, path_args));
    assert!(content(&reply).1);
    assert!(!dir.path().join("elsewhere").exists());
    let _lock = semon_asi::WriterLock::acquire(&dir.path().join("state/analytics-store")).unwrap();
    let reply = server.request(request(4, args));
    let (error, failed) = content(&reply);
    assert!(failed);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("writer busy")
    );
    assert!(!reply.to_string().contains(body));
}

#[test]
fn durable_add_refuses_oversized_file_stdin_and_mcp_values() {
    let dir = store();
    let cache = dir.path().join("state/index.json");
    let marker = "private oversized prose";
    let values = json!({"title":marker.repeat(16_384)});
    let bytes = serde_json::to_vec(&values).unwrap();
    let input = dir.path().join("oversized.json");
    std::fs::write(&input, &bytes).unwrap();
    let output = cli(
        &[
            "asi",
            "add",
            "lane",
            "--id",
            "large",
            "--values",
            input.to_str().unwrap(),
        ],
        cache.clone(),
    );
    let check = |output: std::process::Output| {
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("durable values exceed the 256 KiB authoring limit"));
        assert!(!error.contains(marker));
    };
    check(output);
    let mut child = Command::new(env!("CARGO_BIN_EXE_semon"))
        .args(["asi", "add", "lane", "--id", "large", "--values", "-"])
        .args(cli_args(cache.clone()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The bounded reader may close stdin before the entire payload is written.
    let write = child.stdin.take().unwrap().write_all(&bytes);
    if let Err(error) = write {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    check(child.wait_with_output().unwrap());
    let mut server = Server::start(cache);
    let reply = server.request(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"asi_add","arguments":{"table":"lane","id":"large","values":values}}}));
    let (error, failed) = content(&reply);
    assert!(failed);
    assert_eq!(
        error["error"]["message"],
        "durable values exceed the 256 KiB authoring limit"
    );
    assert!(!reply.to_string().contains(marker));
    assert!(
        !dir.path()
            .join("state/analytics-store/asi-durable.sqlite3")
            .exists()
    );
}

fn cli_args(cache: PathBuf) -> Vec<String> {
    ["--cache".to_owned(), cache.to_string_lossy().into_owned()]
        .into_iter()
        .collect()
}

fn cli(args: &[&str], cache: PathBuf) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_semon"))
        .args(args)
        .args(cli_args(cache))
        .output()
        .unwrap()
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<String>,
}

impl Server {
    fn start(cache: PathBuf) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_semon"))
            .arg("mcp")
            .args(cli_args(cache))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            replies,
        }
    }

    fn request(&mut self, value: Value) -> Value {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
        let reply = self.replies.recv_timeout(Duration::from_secs(30)).unwrap();
        serde_json::from_str(&reply).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

struct Viewer {
    child: Child,
    address: String,
    token: String,
}

impl Viewer {
    fn start(cache: PathBuf, remote: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_semon"));
        command
            .args(["sessions", "--serve", "--listen", "127.0.0.1:0"])
            .args(cli_args(cache));
        if remote {
            command.args(["--machine", "/missing/receiver"]);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            let _ = sender.send(line);
        });
        let mut viewer = Self {
            child,
            address: String::new(),
            token: String::new(),
        };
        let url = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
        let (address, token) = url
            .trim()
            .strip_prefix("http://")
            .unwrap()
            .split_once("/?t=")
            .unwrap();
        viewer.address = address.into();
        viewer.token = token.into();
        viewer
    }

    fn request(&self, method: &str, path: &str, body: &str, headers: &str) -> (u16, String, Value) {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: {}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", self.address, body.len()).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, head.into(), serde_json::from_str(body).unwrap())
    }

    fn headers(&self) -> String {
        format!(
            "Cookie: semon_session={}\r\nOrigin: http://{}\r\nContent-Type: application/json\r\n",
            self.token, self.address
        )
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn viewer_asi_routes_enforce_admission_and_worker_boundaries() {
    let dir = store();
    let viewer = Viewer::start(dir.path().join("state/index.json"), false);
    for (method, tool) in [
        ("POST", "sql"),
        ("GET", "schema"),
        ("POST", "search"),
        ("POST", "view"),
    ] {
        let path = format!("/api/asi/{tool}");
        assert_eq!(viewer.request(method, &path, "{}", "").0, 403);
        let headers = viewer.headers();
        for bad in [
            headers.replace(
                &format!("Origin: http://{}", viewer.address),
                "Origin: http://evil.test",
            ),
            headers.replace(
                &format!("Cookie: semon_session={}", viewer.token),
                "Cookie: semon_session=wrong",
            ),
            format!("{headers}Host: evil.test\r\n"),
            format!("{headers}Origin: http://{}\r\n", viewer.address),
            format!("{headers}Cookie: semon_session={}\r\n", viewer.token),
        ] {
            assert_eq!(viewer.request(method, &path, "{}", &bad).0, 403);
        }
    }
    let headers = viewer.headers();
    let (status, head, value) = viewer.request(
        "POST",
        "/api/asi/sql",
        r#"{"query":"SELECT 42 AS answer"}"#,
        &headers,
    );
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["rows"], json!([[42]]));
    assert!(head.contains("Cache-Control: no-store"));
    assert!(head.contains("Referrer-Policy: no-referrer"));
    // Same-origin browser GET omits Origin. This HTTP regression covers that
    // admission contract directly; a separate browser regression is unnecessary.
    let cookie_only = format!("Cookie: semon_session={}\r\n", viewer.token);
    assert_eq!(
        viewer.request("GET", "/api/asi/schema", "", &cookie_only).0,
        200
    );
    assert_eq!(viewer.request("GET", "/api/asi/schema", "", "").0, 403);
    let foreign = format!("{cookie_only}Origin: http://evil.test\r\n");
    assert_eq!(
        viewer.request("GET", "/api/asi/schema", "", &foreign).0,
        403
    );
    assert_eq!(
        viewer
            .request(
                "POST",
                "/api/asi/sql",
                "{}",
                &format!("{cookie_only}Content-Type: application/json\r\n")
            )
            .0,
        403
    );
    let (status, _, value) = viewer.request(
        "POST",
        "/api/asi/sql",
        r#"{"query":"SELECT * FROM read_csv('/outside/private.csv')"}"#,
        &headers,
    );
    assert_eq!(status, 400);
    assert_eq!(value["error"]["code"], "not_allowed");
    for body in [
        "{".to_owned(),
        "x".repeat(512 * 1024 + 1),
        r#"{"query":"SELECT 1","root":"/another/store"}"#.into(),
    ] {
        assert_eq!(
            viewer.request("POST", "/api/asi/sql", &body, &headers).0,
            400
        );
    }
    assert_eq!(
        viewer
            .request("POST", "/api/asi/search?query=secret", "{}", &headers)
            .0,
        400
    );
    assert_eq!(
        viewer.request("GET", "/api/asi/search", "", &headers).0,
        403
    );
    let receiver = Viewer::start(dir.path().join("absent/index.json"), true);
    for tool in ["search", "view", "sql", "schema"] {
        let (status, _, value) = receiver.request(
            if tool == "schema" { "GET" } else { "POST" },
            &format!("/api/asi/{tool}"),
            "{}",
            &receiver.headers(),
        );
        assert_eq!(status, 503);
        assert_eq!(value["error"]["code"], "capability_unavailable");
    }
    assert!(!dir.path().join("absent").exists());
}

fn content(reply: &Value) -> (Value, bool) {
    let result = &reply["result"];
    (
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        result["isError"].as_bool().unwrap(),
    )
}

#[test]
fn cli_and_mcp_search_and_view_native_fixture_text() {
    let home = fixture::Fixture::new();
    home.write("configured-claude/projects/fixture/00000000-0000-4000-8000-000000000001.jsonl", b"{\"type\":\"user\",\"uuid\":\"prompt\",\"sessionId\":\"00000000-0000-4000-8000-000000000001\",\"timestamp\":\"2026-11-10T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"uniqueftsneedle\"}}\n");
    let output = home
        .command(env!("CARGO_BIN_EXE_semon"))
        .args(["analytics", "materialize", "--all", "--proc-root"])
        .arg(home.root.join("empty-proc"))
        .arg("--cache")
        .arg(home.root.join("state/index.json"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cache = home.root.join("state/index.json");
    let output = cli(&["query", "search", "uniqueftsneedle"], cache.clone());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    let uid = result["hits"][0]["body_uid"].as_str().unwrap().to_owned();
    let output = cli(&["query", "view", &uid], cache.clone());
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["bodies"][0]["text"]["untrusted_text"],
        "uniqueftsneedle"
    );
    let viewer = Viewer::start(cache.clone(), false);
    let (status, headers, result) = viewer.request(
        "POST",
        "/api/asi/search",
        r#"{"query":"uniqueftsneedle"}"#,
        &viewer.headers(),
    );
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    assert!(headers.contains("Cache-Control: no-store"));
    assert!(headers.contains("Referrer-Policy: no-referrer"));
    let (status, headers, result) = viewer.request(
        "POST",
        "/api/asi/view",
        &json!({"uid": uid}).to_string(),
        &viewer.headers(),
    );
    assert_eq!(status, 200, "{result}");
    assert_eq!(
        result["bodies"][0]["text"]["untrusted_text"],
        "uniqueftsneedle"
    );
    assert!(headers.contains("Cache-Control: no-store"));
    let mut server = Server::start(cache);
    let reply = server.request(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{"query":"uniqueftsneedle"}}}));
    let (result, failed) = content(&reply);
    assert!(!failed);
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
}

#[test]
fn query_help_distinguishes_native_and_search_since_flags() {
    let dir = tempfile::tempdir().unwrap();
    let output = cli(&["query"], dir.path().join("index.json"));
    let help = String::from_utf8(output.stderr).unwrap();
    let search = help
        .lines()
        .find(|line| line.trim_start().starts_with("search "))
        .unwrap();
    assert!(search.contains("--since VALUE"));
    assert!(!search.contains("--newer-than"));
    let native = help
        .lines()
        .find(|line| line.trim_start().starts_with("list_sessions "))
        .unwrap();
    assert!(native.contains("--newer-than VALUE"));
}

#[test]
fn cli_body_queries_emit_json_and_remote_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("state/index.json");
    for args in [
        vec!["query", "search", "needle"],
        vec!["query", "view", "label/one"],
    ] {
        let output = cli(&args, cache.clone());
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "not_materialized");
    }
    let output = cli(
        &[
            "query",
            "search",
            "needle",
            "--machine",
            "/missing/receiver",
        ],
        cache,
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "capability_unavailable");
}

#[test]
fn durable_text_is_not_an_analytics_sql_column() {
    let dir = store();
    let output = cli(
        &["query", "sql", "SELECT body FROM fact_label"],
        dir.path().join("state/index.json"),
    );
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["error"]["code"].is_string());
    let schema: Value = serde_json::from_str(&semon_asi::reference::json()).unwrap();
    for table in schema["tables"].as_array().unwrap() {
        for column in table["columns"].as_array().unwrap() {
            assert_ne!(column["class"], "body");
        }
    }
}

#[test]
fn cli_sql_json_tsv_and_typed_errors() {
    let dir = store();
    let cache = dir.path().join("state/index.json");
    let output = cli(
        &["query", "sql", "SELECT ? AS value", "--params", "[42]"],
        cache.clone(),
    );
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["rows"], json!([[42]]));

    let output = cli(
        &[
            "query",
            "sql",
            "SELECT 'a\tb\nc' AS \"d\te\nf\"",
            "--format",
            "tsv",
        ],
        cache.clone(),
    );
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "d e f\na b c\n");

    let output = cli(&["query", "schema"], cache.clone());
    assert!(output.status.success());
    let schema: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(schema["tables"].as_array().is_some());
    assert_eq!(schema["data_notice"], semon_asi::query::DATA_NOTICE);

    let output = cli(&["query", "sql", "SELECT getenv('HOME')"], cache.clone());
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "not_allowed");

    for arguments in [
        vec!["query", "sql"],
        vec!["query", "sql", "SELECT 1", "--limit", "nope"],
        vec!["query", "sql", "SELECT 1", "--params", "["],
        vec!["query", "sql", "SELECT 1", "--params", "[{}]"],
        vec!["query", "sql", "SELECT 1", "--machine", "/received/machine"],
        vec!["query", "schema", "--machine", "/received/machine"],
        vec!["query", "sql", "SELECT 1", "--claude-home", "/other/home"],
    ] {
        let output = cli(&arguments, cache.clone());
        assert!(!output.status.success(), "{arguments:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["error"]["code"], "invalid_argument", "{arguments:?}");
    }
}

#[test]
fn mcp_advertises_asi_tools_and_serves_schema_resource() {
    let dir = store();
    let mut server = Server::start(dir.path().join("state/index.json"));
    let initialized = server.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}));
    assert_eq!(
        initialized["result"]["capabilities"]["resources"]["subscribe"],
        false
    );
    let listed = server.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"sql"));
    assert!(names.contains(&"schema"));
    assert!(names.contains(&"search"));
    assert!(names.contains(&"view"));
    assert!(names.contains(&"asi_add"));
    let add = tools.iter().find(|tool| tool["name"] == "asi_add").unwrap();
    assert_eq!(add["inputSchema"]["additionalProperties"], false);
    assert_eq!(add["annotations"]["readOnlyHint"], false);
    for name in ["search", "view"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    assert!(!names.contains(&"find"));
    let sql = tools.iter().find(|tool| tool["name"] == "sql").unwrap();
    assert_eq!(sql["inputSchema"]["required"], json!(["query"]));
    assert_eq!(sql["inputSchema"]["additionalProperties"], false);
    assert_eq!(sql["inputSchema"]["properties"]["params"]["type"], "array");

    let resources = server.request(json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}));
    assert_eq!(
        resources["result"]["resources"][0]["uri"],
        "semon://asi/schema"
    );
    let resource = server.request(json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"semon://asi/schema"}}));
    let schema: Value =
        serde_json::from_str(resource["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert!(schema["tables"].as_array().is_some());
    assert_eq!(schema["data_notice"], semon_asi::query::DATA_NOTICE);

    let schema = server
        .request(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"schema"}}));
    let (schema, failed) = content(&schema);
    assert!(!failed);
    assert!(schema["tables"].as_array().is_some());
    assert_eq!(schema["data_notice"], semon_asi::query::DATA_NOTICE);
    let schema = server.request(json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"schema","arguments":{"table":"dim_session"}}}));
    let (schema, failed) = content(&schema);
    assert!(!failed);
    assert_eq!(schema["table"]["name"], "dim_session");
    assert_eq!(schema["data_notice"], semon_asi::query::DATA_NOTICE);

    let reply = server
        .request(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"sql"}}));
    let (error, failed) = content(&reply);
    assert!(failed);
    assert_eq!(error["error"]["code"], "invalid_argument");

    let reply = server.request(json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"sql","arguments":{"query":"SELECT getenv('HOME')"}}}));
    let (error, failed) = content(&reply);
    assert!(failed);
    assert_eq!(error["error"]["code"], "not_allowed");
}
