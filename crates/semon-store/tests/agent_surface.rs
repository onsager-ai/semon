//! The agent read surface through the `semon` binary: `semon query` pins each
//! tool's JSON on a fixture home, and `semon mcp` is driven over stdio, from
//! the initialize handshake to each tool and malformed requests. Neither
//! writes anything but the metadata cache.

use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Output, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// 2026-09-24T00:00:00Z: days before any run of these tests.
const BASE: i64 = 1_790_208_000_000;

fn at(hour: i64, minute: i64) -> i64 {
    BASE + (hour * 60 + minute) * 60_000
}

fn ts(hour: i64, minute: i64) -> String {
    format!("2026-09-24T{hour:02}:{minute:02}:00.000Z")
}

struct Home {
    root: PathBuf,
    /// The window flags every command gets: `--all` unless a test says.
    window: Vec<&'static str>,
}

impl Home {
    /// `done` ended, `asker` waits on a question, and the Codex run `cx`
    /// ended.
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-agent-surface-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let home = Self {
            root,
            window: vec!["--all"],
        };
        home.write("proc/locks", "");
        home.write("proc/sys/kernel/hostname", "testbox\n");
        home.lines(
            "claude/projects/-work-proj/done.jsonl",
            &[
                human("done", ts(4, 0), "Summarize the design doc"),
                assistant(
                    "done",
                    ts(4, 1),
                    json!([{"type": "text", "text": "The design keeps the frontend and replaces the data."}]),
                ),
            ],
        );
        home.lines(
            "claude/projects/-work-proj/asker.jsonl",
            &[
                human("asker", ts(2, 0), "Pick a name"),
                assistant(
                    "asker",
                    ts(2, 1),
                    json!([{"type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": {"questions": [
                        {"question": "Which name?", "header": "Name", "multiSelect": false,
                            "options": [{"label": "semon"}, {"label": "ostrom"}]}]}}]),
                ),
            ],
        );
        let mut stat = vec!["0"; 20];
        stat[19] = "777";
        home.write("proc/31/stat", &format!("31 (claude) {}\n", stat.join(" ")));
        home.write(
            "claude/sessions/31.json",
            &json!({"pid": 31, "sessionId": "asker", "procStart": 777, "status": "idle", "name": "asker-lane"})
                .to_string(),
        );
        let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
        home.lines(
            "codex/sessions/2026/09/24/rollout-cx.jsonl",
            &[
                codex(
                    ts(5, 0),
                    "session_meta",
                    json!({"id": "cx", "cwd": "/work/proj", "originator": "codex_exec", "thread_source": "user"}),
                ),
                codex(
                    ts(5, 1),
                    "response_item",
                    json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Implement the lexer fix"}]}),
                ),
                codex(
                    ts(5, 2),
                    "response_item",
                    json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fixed the lexer panic"}]}),
                ),
            ],
        );
        home
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn lines(&self, relative: &str, records: &[Value]) {
        let text: String = records.iter().map(|record| format!("{record}\n")).collect();
        self.write(relative, &text);
    }

    /// The home options, as `semon sessions` takes them. The cache goes
    /// under `state/`.
    fn flags(&self) -> Vec<String> {
        [
            ("--claude-home", "claude"),
            ("--codex-home", "codex"),
            ("--proc-root", "proc"),
            ("--cache", "state/index.json"),
        ]
        .into_iter()
        .flat_map(|(flag, path)| {
            [
                flag.to_owned(),
                self.root.join(path).to_string_lossy().into_owned(),
            ]
        })
        .collect()
    }

    fn semon(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_semon"))
            .args(args)
            .args(self.flags())
            .args(&self.window)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// `semon query <args> --json`'s answer.
    fn query(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let output = self.semon(&all);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "--json is one line: {text}");
        serde_json::from_str(&text).unwrap()
    }

    fn model(&self) -> Value {
        let output = self.semon(&["sessions", "--model-json", "--all"]);
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Every file under the home but the cache, with its bytes.
    fn inputs(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        for dir in ["claude", "codex", "proc"] {
            walk(&self.root.join(dir), &mut out);
        }
        let others: Vec<PathBuf> = fs::read_dir(&self.root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                !["claude", "codex", "proc", "state"]
                    .iter()
                    .any(|dir| path.ends_with(dir))
            })
            .collect();
        assert!(others.is_empty(), "written outside the cache: {others:?}");
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn human(sid: &str, time: String, text: &str) -> Value {
    json!({"type": "user", "timestamp": time, "sessionId": sid, "cwd": "/work/proj", "origin": {"kind": "human"}, "message": {"role": "user", "content": text}})
}

fn assistant(sid: &str, time: String, content: Value) -> Value {
    json!({"type": "assistant", "timestamp": time.clone(), "sessionId": sid, "cwd": "/work/proj",
        "message": {"id": format!("m-{sid}-{time}"), "model": "claude-opus-5-5", "role": "assistant", "content": content,
            "usage": {"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 2000, "output_tokens": 500}}})
}

fn question_id(model: &Value) -> Value {
    model["handoffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|handoff| handoff["kind"] == "toyou")
        .unwrap()["id"]
        .clone()
}

fn turn_id(model: &Value, sid: &str) -> Value {
    model["turns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|turn| turn["sid"] == sid)
        .unwrap()["id"]
        .clone()
}

fn summaries(model: &Value) -> BTreeMap<&'static str, Value> {
    let claude = |id: &str| {
        json!({"id": id, "harness": "claude", "kind": "session", "model": "opus-5.5", "repo": null, "branch": null,
            "machine": "testbox", "parent": null, "children": [], "exit": null,
            "tokens": {"input": 1000, "cached": 2000, "output": 500}, "run": null})
    };
    let mut done = claude("done");
    for (key, value) in [
        ("name", json!("proj")),
        ("state", json!("ended")),
        ("start", json!(at(4, 0))),
        ("last_activity", json!(at(4, 1))),
        ("pid", Value::Null),
        ("alive", Value::Null),
        ("open_question", Value::Null),
    ] {
        done[key] = value;
    }
    let mut asker = claude("asker");
    for (key, value) in [
        ("name", json!("asker-lane")),
        ("state", json!("waiting-question")),
        ("start", json!(at(2, 0))),
        ("last_activity", json!(at(2, 1))),
        ("pid", json!(31)),
        ("alive", json!(true)),
        (
            "open_question",
            json!({"text": "Which name?", "asked_at": at(2, 1), "handoff": question_id(model)}),
        ),
    ] {
        asker[key] = value;
    }
    let cx = json!({"id": "cx", "harness": "codex", "kind": "codex-run", "name": "Codex run", "model": "codex",
        "repo": null, "branch": null, "machine": "testbox", "parent": null, "children": [], "state": "ended",
        "start": at(5, 0), "last_activity": at(5, 2), "pid": null, "alive": null, "exit": null,
        "open_question": null, "tokens": {"input": 0, "cached": 0, "output": 0}, "run": null});
    BTreeMap::from([("done", done), ("asker", asker), ("cx", cx)])
}

/// Drops what depends on the clock: `now`, and each stall's `idle_ms`;
/// and `window_start`, which every answer has, null with `--all`.
fn timeless(mut value: Value) -> Value {
    if let Some(fields) = value.as_object_mut() {
        fields.remove("now");
        assert_eq!(fields.remove("window_start"), Some(Value::Null));
    }
    if let Some(rows) = value.get_mut("stalls").and_then(Value::as_array_mut) {
        for row in rows {
            let idle = row
                .as_object_mut()
                .unwrap()
                .remove("idle_ms")
                .and_then(|idle| idle.as_i64());
            assert!(idle.is_some_and(|idle| idle > 60 * 60_000));
        }
    }
    value
}

/// Each tool, its arguments, and its answer (without the clock) on the
/// fixture home.
fn expected(model: &Value) -> Vec<(&'static str, Value, Vec<&'static str>, Value)> {
    let summary = summaries(model);
    let ask = turn_id(model, "done");
    let cx = turn_id(model, "cx");
    let mut session = summary["done"].clone();
    session["busy"] = json!([[at(4, 0), at(4, 1)]]);
    session["turns"] = Value::Array(
        model["turns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|turn| turn["sid"] == "done")
            .cloned()
            .collect(),
    );
    session["handoffs"] = Value::Array(
        model["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|handoff| handoff["to"] == "done" || handoff["from"] == "done")
            .cloned()
            .collect(),
    );
    let mut stalled = summary["asker"].clone();
    stalled["stall_reason"] = json!("waiting-question");
    vec![
        (
            "list_sessions",
            json!({}),
            vec![],
            json!({"total": 3, "truncated": false, "sessions": [summary["cx"], summary["done"], summary["asker"]]}),
        ),
        (
            "list_sessions",
            json!({"state": "working"}),
            vec!["--state", "working"],
            json!({"total": 0, "truncated": false, "sessions": []}),
        ),
        (
            "get_session",
            json!({"id": "done"}),
            vec!["done"],
            json!({"session": session}),
        ),
        (
            "read_transcript",
            json!({"id": "done"}),
            vec!["done"],
            json!({"sid": "done", "from": 0, "to": 2, "total": 2, "calls": 0, "errors": 0, "entries": [
                {"k": "h", "id": ask, "turn": ask},
                {"k": "a", "text": "The design keeps the frontend and replaces the data."},
            ]}),
        ),
        (
            "read_transcript",
            json!({"id": "done", "limit": 1}),
            vec!["done", "--limit", "1"],
            json!({"sid": "done", "from": 1, "to": 2, "total": 2, "calls": 0, "errors": 0, "entries": [
                {"k": "a", "text": "The design keeps the frontend and replaces the data.", "turn": ask},
            ]}),
        ),
        (
            "find",
            json!({"text": "lexer", "limit": 1}),
            vec!["lexer", "--limit", "1"],
            json!({"text": "lexer", "matches": [
                {"session": "cx", "position": 1, "turn": cx, "kind": "a", "part": "text", "at": at(5, 2),
                    "snippet": "Fixed the lexer panic"},
            ], "sessions": [
                {"id": "cx", "name": "Codex run", "repo": null, "last_activity": at(5, 2), "fields": [], "entries": 1},
            ], "scanned_bytes": "any", "truncated": true, "truncated_by": "limit"}),
        ),
        (
            "find",
            json!({"text": "nothing says this"}),
            vec!["nothing says this"],
            json!({"text": "nothing says this", "matches": [], "sessions": [], "scanned_bytes": "any",
                "truncated": false, "truncated_by": null}),
        ),
        (
            "stalls",
            json!({"idle_minutes": 60}),
            vec!["--idle-minutes", "60"],
            json!({"idle_minutes": 60, "rule": "any", "stalls": [stalled]}),
        ),
    ]
}

/// `expected` with the fields it marks `"any"` (a byte count, the stated
/// rule) taken from `value`, after checking `value` has them.
fn matching(expected: &Value, value: &Value) -> Value {
    let mut expected = expected.clone();
    if let (Some(fields), Some(actual)) = (expected.as_object_mut(), value.as_object()) {
        for (key, field) in fields.iter_mut() {
            if *field == "any" {
                assert!(
                    actual.get(key).is_some_and(|value| !value.is_null()),
                    "{key}"
                );
                *field = actual[key].clone();
            }
        }
    }
    expected
}

#[test]
fn semon_query_pins_every_tool_and_writes_only_its_cache() {
    let home = Home::new();
    let before = home.inputs();
    let model = home.model();
    for (tool, _, args, answer) in expected(&model) {
        let mut all = vec!["query", tool];
        all.extend(args.iter().copied());
        let got = timeless(home.query(&all));
        assert_eq!(got, matching(&answer, &got), "{all:?}");
    }
    // An unknown id is a clear error: nonzero, a JSON error with --json.
    let output = home.semon(&["query", "get_session", "nobody", "--json"]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "unknown_session");
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown_session"));
    for refused in [
        &["query", "nope"][..],
        &["query", "stalls"][..],
        &["query", "list_sessions", "--bogus", "1"][..],
        &["query", "stalls", "--idle-minutes", "soon"][..],
        &["query", "list_sessions", "extra"][..],
    ] {
        assert!(!home.semon(refused).status.success(), "{refused:?}");
    }
    // Pretty without --json, and the same answer.
    let output = home.semon(&["query", "list_sessions"]);
    assert!(output.status.success());
    let pretty = String::from_utf8(output.stdout).unwrap();
    assert!(pretty.lines().count() > 1);
    let pretty: Value = serde_json::from_str(&pretty).unwrap();
    assert_eq!(timeless(pretty)["total"], 3);
    assert_eq!(
        home.inputs(),
        before,
        "the agent homes and /proc are unchanged"
    );
}

/// A running `semon mcp`, spoken to one line at a time.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<String>,
}

impl Server {
    fn start(home: &Home) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_semon"))
            .arg("mcp")
            .args(home.flags())
            .args(&home.window)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
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

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn reply(&self) -> Value {
        let line = self
            .replies
            .recv_timeout(Duration::from_secs(120))
            .expect("a reply within two minutes");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line}"))
    }

    fn request(&mut self, message: Value) -> Value {
        self.send(&message.to_string());
        let reply = self.reply();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], message["id"], "{reply}");
        reply
    }

    fn call(&mut self, id: i64, tool: &str, arguments: Value) -> Value {
        self.request(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}}))
    }

    /// Closes stdin and waits for a clean exit.
    fn finish(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                break;
            }
            assert!(Instant::now() < deadline, "semon mcp didn't exit at EOF");
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            self.replies.try_recv().is_err(),
            "nothing after the last reply"
        );
    }
}

/// A tool's result: its text content, parsed, and whether it failed.
fn content(reply: &Value) -> (Value, bool) {
    let result = &reply["result"];
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    (
        serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap(),
        result["isError"].as_bool().unwrap(),
    )
}

/// Checks a tool's input schema against the part of JSON Schema it may use:
/// an object of typed properties, with valid bounds, enums, defaults and
/// required names.
fn check_schema(tool: &str, schema: &Value) {
    let top = schema.as_object().unwrap();
    for key in top.keys() {
        assert!(
            ["type", "properties", "required", "additionalProperties"].contains(&key.as_str()),
            "{tool}: {key}"
        );
    }
    assert_eq!(schema["type"], "object", "{tool}");
    assert_eq!(schema["additionalProperties"], false, "{tool}");
    let properties = schema["properties"].as_object().unwrap();
    assert!(!properties.is_empty(), "{tool}");
    for (name, property) in properties {
        let property = property.as_object().unwrap();
        for key in property.keys() {
            assert!(
                [
                    "type",
                    "description",
                    "enum",
                    "minimum",
                    "maximum",
                    "minLength",
                    "maxLength",
                    "default"
                ]
                .contains(&key.as_str()),
                "{tool}.{name}: {key}"
            );
        }
        let kind = property["type"].as_str().unwrap();
        assert!(
            ["string", "integer"].contains(&kind),
            "{tool}.{name}: {kind}"
        );
        assert!(
            property["description"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "{tool}.{name} has a description"
        );
        let bound = |key: &str| {
            property.get(key).map(|value| {
                value
                    .as_i64()
                    .unwrap_or_else(|| panic!("{tool}.{name}.{key}"))
            })
        };
        if kind == "string" {
            assert!(bound("minimum").is_none() && bound("maximum").is_none());
            if let (Some(min), Some(max)) = (bound("minLength"), bound("maxLength")) {
                assert!(0 <= min && min <= max, "{tool}.{name}");
            }
            if let Some(options) = property.get("enum") {
                let options: Vec<&str> = options
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|option| option.as_str().unwrap())
                    .collect();
                assert!(!options.is_empty(), "{tool}.{name}");
                let mut unique = options.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(unique.len(), options.len(), "{tool}.{name}");
            }
        } else {
            assert!(bound("minLength").is_none() && bound("maxLength").is_none());
            assert!(property.get("enum").is_none());
            let (min, max) = (bound("minimum"), bound("maximum"));
            if let (Some(min), Some(max)) = (min, max) {
                assert!(min <= max, "{tool}.{name}");
            }
            if let Some(default) = property.get("default") {
                let default = default.as_i64().unwrap();
                assert!(min.is_none_or(|min| min <= default));
                assert!(max.is_none_or(|max| default <= max));
            }
        }
    }
    let required = schema
        .get("required")
        .map(|required| required.as_array().unwrap().clone())
        .unwrap_or_default();
    for name in &required {
        let name = name.as_str().unwrap();
        assert!(properties.contains_key(name), "{tool}: required {name}");
        assert!(
            properties[name].get("default").is_none(),
            "{tool}: {name} is required and has a default"
        );
    }
}

#[test]
fn semon_mcp_speaks_the_protocol_and_serves_every_tool() {
    let home = Home::new();
    let before = home.inputs();
    let model = home.model();
    let mut server = Server::start(&home);

    let init = server.request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "semon");
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    // The notification gets no answer: the next line is the ping's.
    server.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    let ping = server.request(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
    assert_eq!(ping["result"], json!({}));

    let listed = server.request(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "list_sessions",
            "get_session",
            "read_transcript",
            "find",
            "stalls"
        ]
    );
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
        check_schema(name, &tool["inputSchema"]);
    }

    // Every tool answers as `semon query` does.
    for (id, (tool, arguments, _, answer)) in (10..).zip(expected(&model)) {
        let reply = server.call(id, tool, arguments.clone());
        let (value, failed) = content(&reply);
        assert!(!failed, "{tool} {arguments}: {value}");
        let value = timeless(value);
        assert_eq!(value, matching(&answer, &value), "{tool} {arguments}");
    }

    // A tool's failure is a result with isError; the protocol's are errors.
    let (error, failed) = content(&server.call(30, "get_session", json!({"id": "nobody"})));
    assert!(failed);
    assert_eq!(error["error"]["code"], "unknown_session");
    let (error, failed) = content(&server.call(31, "list_sessions", json!({"bogus": 1})));
    assert!(failed);
    assert_eq!(error["error"]["code"], "invalid_arguments");
    let unknown = server.call(32, "nope", json!({}));
    assert_eq!(unknown["error"]["code"], -32602);
    let method = server.request(json!({"jsonrpc": "2.0", "id": 33, "method": "resources/read"}));
    assert_eq!(method["error"]["code"], -32601);
    for (line, id, code) in [
        ("{not json", Value::Null, -32700),
        ("[1, 2]", Value::Null, -32600),
        (
            r#"{"jsonrpc":"1.0","id":34,"method":"ping"}"#,
            json!(34),
            -32600,
        ),
        (r#"{"jsonrpc":"2.0","id":35}"#, json!(35), -32600),
        (
            r#"{"jsonrpc":"2.0","id":36,"method":"tools/call","params":"list_sessions"}"#,
            json!(36),
            -32602,
        ),
    ] {
        server.send(line);
        let reply = server.reply();
        assert_eq!(
            (&reply["id"], reply["error"]["code"].as_i64()),
            (&id, Some(code)),
            "{line}"
        );
    }
    // Still serving after all that.
    let ping = server.request(json!({"jsonrpc": "2.0", "id": 37, "method": "ping"}));
    assert_eq!(ping["result"], json!({}));
    server.finish();
    assert_eq!(
        home.inputs(),
        before,
        "the agent homes and /proc are unchanged"
    );
}

/// The window flags: 30 days by default, `--since`, or `--all`; a tool's
/// `since` (`--newer-than` on the command line) that reaches before the
/// window is the error `outside_window`, over the CLI and MCP alike.
#[test]
fn the_window_is_30_days_unless_widened_and_since_stays_inside_it() {
    const DAY_MS: i64 = 86_400_000;
    let mut home = Home::new();
    let window_of =
        |answer: &Value| answer["now"].as_i64().unwrap() - answer["window_start"].as_i64().unwrap();
    home.window = vec![];
    let answer = home.query(&["query", "list_sessions"]);
    assert_eq!(window_of(&answer), 30 * DAY_MS);
    let output = home.semon(&["query", "list_sessions", "--newer-than", "40d", "--json"]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "outside_window");
    assert!(error["error"]["window_start"].is_i64());
    let inside = home.query(&["query", "find", "lexer", "--newer-than", "29d"]);
    assert_eq!(window_of(&inside), 30 * DAY_MS);

    home.window = vec!["--since", "60d"];
    let wider = home.query(&["query", "list_sessions", "--newer-than", "40d"]);
    assert_eq!(window_of(&wider), 60 * DAY_MS);

    home.window = vec!["--all"];
    let all = home.query(&[
        "query",
        "stalls",
        "--idle-minutes",
        "5",
        "--newer-than",
        "400d",
    ]);
    assert_eq!(all["window_start"], Value::Null);

    home.window = vec!["--all", "--since", "7d"];
    assert!(!home.semon(&["query", "list_sessions"]).status.success());
    assert!(!home.semon(&["mcp"]).status.success());
    home.window = vec!["--since", "soon"];
    assert!(!home.semon(&["query", "list_sessions"]).status.success());

    home.window = vec![];
    let mut server = Server::start(&home);
    server.request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}));
    let (answer, failed) = content(&server.call(2, "list_sessions", json!({})));
    assert!(!failed);
    assert_eq!(window_of(&answer), 30 * DAY_MS);
    let (error, failed) =
        content(&server.call(3, "stalls", json!({"idle_minutes": 5, "since": "31d"})));
    assert!(failed);
    assert_eq!(error["error"]["code"], "outside_window");
    server.finish();
}
