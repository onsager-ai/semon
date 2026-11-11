#![cfg(feature = "asi-query")]

use std::{
    io::{BufRead, BufReader, Write},
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
    std::fs::write(
        root.join("analytics/manifest.json"),
        serde_json::to_vec(&Manifest::empty()).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("state/analytics-namespace"),
        format!("origin-{}\n", "0".repeat(64)),
    )
    .unwrap();
    dir
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
    let mut server = Server::start(cache);
    let reply = server.request(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{"query":"uniqueftsneedle"}}}));
    let (result, failed) = content(&reply);
    assert!(!failed);
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
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
