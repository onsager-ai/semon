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
    /// has unknown turn outcome (no durable lifecycle records).
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
            &json!({"pid": 31, "sessionId": "asker", "procStart": 777, "status": "idle", "updatedAt": at(2, 1), "name": "asker-lane"})
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
            ("--copilot-home", "copilot"),
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
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1, "--json is one line: {text}");
        serde_json::from_str(&text).unwrap()
    }

    fn native_contract(&self) -> Value {
        // Captured before compatibility retirement; independent of the Query
        // implementation under test, including turn and handoff ids.
        serde_json::from_str(include_str!("fixtures/agent-native-contract.json")).unwrap()
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
            "tokens": {"input": 1000, "cached": 2000, "output": 500}, "run": null, "turns_truncated": false})
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
        "repo": null, "branch": null, "machine": "testbox", "parent": null, "children": [], "state": "unknown",
        "start": at(5, 0), "last_activity": at(5, 2), "pid": null, "alive": null, "exit": null,
        "open_question": null, "tokens": {"input": 0, "cached": 0, "output": 0}, "run": null,
        "turns_truncated": false});
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
    project_activity(&mut value);
    value
}

/// Keep the historical answers pinned while checking the additive summary.
fn project_activity(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if let Some(activity) = fields.remove("activity_analytics") {
                if fields.get("harness").and_then(Value::as_str) == Some("codex") {
                    assert!(activity.is_null());
                } else {
                    assert_eq!(activity["totals"]["observed"]["steps"], 1);
                    assert_eq!(activity["totals"]["exclusive"]["steps"], 1);
                    assert_eq!(activity["totals"]["shared"]["steps"], 0);
                    assert_eq!(activity["thrash"], false);
                    assert_eq!(activity["family"], Value::Null);
                    assert!(activity.get("steps").is_none());
                }
            }
            for field in fields.values_mut() {
                project_activity(field);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(project_activity),
        _ => {}
    }
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
    let mut codex_session = summary["cx"].clone();
    codex_session["busy"] = json!([[at(5, 0), at(5, 2)]]);
    codex_session["turns"] = json!(
        model["turns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|turn| turn["sid"] == "cx")
            .collect::<Vec<_>>()
    );
    codex_session["handoffs"] = json!(
        model["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|handoff| handoff["to"] == "cx" || handoff["from"] == "cx")
            .collect::<Vec<_>>()
    );
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
            "list_sessions",
            json!({"harness": "codex"}),
            vec!["--harness", "codex"],
            json!({"total": 1, "truncated": false, "sessions": [summary["cx"]]}),
        ),
        (
            "get_session",
            json!({"id": "cx"}),
            vec!["cx"],
            json!({"session": codex_session}),
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
            "read_transcript",
            json!({"id": "cx", "limit": 1}),
            vec!["cx", "--limit", "1"],
            json!({"sid": "cx", "from": 1, "to": 2, "total": 2, "calls": 0, "errors": 0, "entries": [
                {"k": "a", "text": "Fixed the lexer panic", "turn": cx},
            ]}),
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

fn assert_activity_page(page: &Value) {
    assert_eq!(page["session"]["id"], "done");
    assert_eq!(page["kind"], "steps");
    assert!(
        page["generation"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(page["next_cursor"], Value::Null);
    for key in ["position", "command", "output", "body"] {
        assert!(items[0].get(key).is_none(), "{key}");
    }
}

#[test]
fn semon_query_pins_every_tool_and_writes_only_its_cache() {
    let home = Home::new();
    let before = home.inputs();
    let model = home.native_contract();
    for (tool, _, args, answer) in expected(&model) {
        let mut all = vec!["query", tool];
        all.extend(args.iter().copied());
        let got = timeless(home.query(&all));
        assert_eq!(got, matching(&answer, &got), "{all:?}");
    }
    assert_activity_page(&home.query(&["query", "get_session_activity", "done", "--limit", "1"]));
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
    assert_eq!(properties.is_empty(), tool == "stream_cards", "{tool}");
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
                    "default",
                    "properties",
                    "required",
                    "additionalProperties",
                    "items"
                ]
                .contains(&key.as_str()),
                "{tool}.{name}: {key}"
            );
        }
        if property["type"].is_array() {
            assert_eq!(tool, "sql");
            assert_eq!(name, "params");
            continue;
        }
        let kind = property["type"].as_str().unwrap();
        assert!(
            ["string", "integer", "object", "array"].contains(&kind),
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
        if kind == "object" {
            if tool == "asi_add" && name == "values" {
                assert_eq!(
                    property["additionalProperties"],
                    json!({"type":["string","number","null"]})
                );
                continue;
            }
            assert!(property["properties"].is_object());
            assert!(property["required"].is_array());
            assert_eq!(property["additionalProperties"], false);
            continue;
        }
        if kind == "array" {
            assert_eq!(tool, "sql");
            assert_eq!(name, "params");
            assert!(property["items"]["type"].is_array());
            continue;
        }
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
    let model = home.native_contract();
    let mut server = Server::start(&home);

    let init = server.request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "semon");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("GitHub Copilot CLI")
    );
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
    #[cfg(feature = "asi-query")]
    let expected_tools = [
        "list_sessions",
        "get_session",
        "get_session_activity",
        "read_transcript",
        "stalls",
        "sql",
        "context_growth",
        "lane_health",
        "stream_cards",
        "search",
        "view",
        "schema",
        "asi_add",
    ];
    #[cfg(not(feature = "asi-query"))]
    let expected_tools = [
        "list_sessions",
        "get_session",
        "get_session_activity",
        "read_transcript",
        "stalls",
    ];
    assert_eq!(names, expected_tools);
    assert_eq!(
        tools[0]["inputSchema"]["properties"]["harness"]["enum"],
        json!(["claude", "codex", "copilot"])
    );
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            name != "asi_add",
            "{name}"
        );
        check_schema(name, &tool["inputSchema"]);
    }
    // Every active tool answers as `semon query` does.
    for (id, (tool, arguments, _, answer)) in (10..).zip(expected(&model)) {
        let reply = server.call(id, tool, arguments.clone());
        let (value, failed) = content(&reply);
        assert!(!failed, "{tool} {arguments}: {value}");
        let value = timeless(value);
        assert_eq!(value, matching(&answer, &value), "{tool} {arguments}");
    }

    let (page, failed) =
        content(&server.call(29, "get_session_activity", json!({"id":"done","limit":1})));
    assert!(!failed, "{page}");
    assert_activity_page(&page);

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
    assert_eq!(method["error"]["code"], -32602);
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
    let inside = home.query(&[
        "query",
        "list_sessions",
        "--harness",
        "codex",
        "--newer-than",
        "29d",
    ]);
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
    let (inside, failed) = content(&server.call(
        4,
        "list_sessions",
        json!({"harness": "codex", "since": "29d"}),
    ));
    assert!(!failed);
    assert_eq!(inside["window_start"], answer["window_start"]);
    assert!(inside["now"].as_i64().unwrap() >= answer["now"].as_i64().unwrap());
    let (inside, failed) =
        content(&server.call(5, "stalls", json!({"idle_minutes": 5, "since": "29d"})));
    assert!(!failed);
    assert_eq!(inside["window_start"], answer["window_start"]);
    assert!(inside["now"].as_i64().unwrap() >= answer["now"].as_i64().unwrap());
    let (error, failed) =
        content(&server.call(3, "stalls", json!({"idle_minutes": 5, "since": "31d"})));
    assert!(failed);
    assert_eq!(error["error"]["code"], "outside_window");
    server.finish();
}

#[cfg(feature = "asi-query")]
#[test]
fn stream_cards_return_only_latest_well_formed_root_cards() {
    let home = Home::new();
    let card = |stream: &str, number: &str| {
        format!(
            "Stream:  {stream} \nLane goal: Ship the query\nNumber: {number}\nNow: parked until review\nNext owner decision: none"
        )
    };
    let latest = card("Index", "2 sources");
    let claude_reply =
        |id, minute, text: &str| assistant(id, ts(9, minute), json!([{"type":"text","text":text}]));
    home.lines(
        "claude/projects/-work-proj/stream-claude.jsonl",
        &[
            human("stream-claude", ts(9, 0), "PRIVATE-CLAUDE-PROMPT"),
            json!({"type":"custom-title","sessionId":"stream-claude","customTitle":"Claude Manager"}),
            claude_reply("stream-claude", 1, &card("Old stream", "1 source")),
            claude_reply("stream-claude", 2, &latest),
            claude_reply("stream-claude", 3, &latest.replace("Number:", "Metric:")),
            claude_reply("stream-claude", 4, &format!("{latest}\nPRIVATE-CLAUDE-FOOTER")),
            human("stream-claude", ts(9, 5), &card("PRIVATE-USER-CARD", "0")),
        ],
    );
    home.lines(
        "claude/projects/-work-proj/stream-claude/subagents/agent-stream-child.jsonl",
        &[claude_reply(
            "stream-child",
            6,
            &card("PRIVATE-CLAUDE-CHILD", "0"),
        )],
    );
    home.write(
        "claude/sessions/31.json",
        &json!({"pid":31,"sessionId":"stream-claude","procStart":777,"status":"idle","updatedAt":at(9,5)}).to_string(),
    );

    let codex =
        |minute, kind, payload| json!({"type":kind,"timestamp":ts(10,minute),"payload":payload});
    let codex_message = |minute, role, part, text: &str| {
        codex(
            minute,
            "response_item",
            json!({"type":"message","role":role,"content":[{"type":part,"text":text}]}),
        )
    };
    let long = "数".repeat(340);
    home.lines(
        "codex/sessions/2026/09/24/rollout-stream-codex.jsonl",
        &[
            codex(0, "session_meta", json!({"id":"stream-codex","thread_source":"user","agent_nickname":"Codex Manager","cwd":"/work/proj"})),
            codex(1, "event_msg", json!({"type":"task_started","turn_id":"turn"})),
            codex_message(2, "user", "input_text", "PRIVATE-CODEX-PROMPT"),
            codex_message(3, "assistant", "output_text", &card("Warehouse", &long)),
            codex_message(4, "assistant", "output_text", &format!("PRIVATE-CODEX-QUOTE\n{latest}")),
            codex(5, "event_msg", json!({"type":"task_complete","turn_id":"turn","last_agent_message":"PRIVATE-CODEX-TERMINAL"})),
        ],
    );
    home.lines(
        "codex/sessions/2026/09/24/rollout-stream-worker.jsonl",
        &[
            codex(0, "session_meta", json!({"id":"stream-worker","thread_source":"subagent","parent_thread_id":"missing-parent"})),
            codex_message(6, "assistant", "output_text", &card("PRIVATE-CODEX-WORKER", "0")),
        ],
    );

    for role in ["subagent", "guardian_review"] {
        home.lines(
            &format!("codex/sessions/2026/09/24/rollout-stream-{role}.jsonl"),
            &[
                codex(
                    0,
                    "session_meta",
                    json!({"id":format!("stream-{role}"),"thread_source":role}),
                ),
                codex_message(
                    6,
                    "assistant",
                    "output_text",
                    &card("PRIVATE-CODEX-NONROOT", "0"),
                ),
            ],
        );
    }

    let copilot = |minute, kind, data| json!({"type":kind,"id":format!("copilot-{minute}"),"timestamp":ts(11,minute),"data":data});
    home.lines(
        "copilot/session-state/stream-copilot/events.jsonl",
        &[
            copilot(
                0,
                "session.start",
                json!({"version":1,"copilotVersion":"1.0.91","sessionId":"stream-copilot"}),
            ),
            copilot(
                1,
                "assistant.message",
                json!({"content":card("Collection", "3 sources")}),
            ),
            copilot(
                2,
                "user.message",
                json!({"content":card("PRIVATE-COPILOT-USER", "0")}),
            ),
        ],
    );

    let published = home.semon(&["analytics", "materialize"]);
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    let namespace = semon_sessions::read_analytics_namespace(&home.root.join("state"))
        .unwrap()
        .unwrap();
    let expected = json!({"cards":[
        {"session_uid":format!("{namespace}/claude/stream-claude"),"title":"Claude Manager","state":"idle","host":"testbox",
         "last_message_at":at(9,5),"card_posted_at":at(9,2),"stream":"Index","lane_goal":"Ship the query",
         "number":"2 sources","now":"parked until review","next_owner_decision":"none"},
        {"session_uid":format!("{namespace}/codex/stream-codex"),"title":"Codex Manager","state":"ended","host":"testbox",
         "last_message_at":at(10,4),"card_posted_at":at(10,3),"stream":"Warehouse","lane_goal":"Ship the query",
         "number":"数".repeat(300),"now":"parked until review","next_owner_decision":"none"},
        {"session_uid":format!("{namespace}/copilot/stream-copilot"),"title":"Copilot CLI","state":null,"host":"testbox",
         "last_message_at":at(11,2),"card_posted_at":at(11,1),"stream":"Collection","lane_goal":"Ship the query",
         "number":"3 sources","now":"parked until review","next_owner_decision":"none"}
    ], "data_notice":semon_sessions::body_query::DATA_NOTICE});
    assert_eq!(home.query(&["query", "stream_cards"]), expected);
    let mut server = Server::start(&home);
    let (mcp, failed) = content(&server.call(1, "stream_cards", json!({})));
    assert!(!failed, "{mcp}");
    assert_eq!(mcp, expected);
    let (error, failed) = content(&server.call(2, "stream_cards", json!({"unknown": true})));
    assert!(failed);
    assert_eq!(error["error"]["code"], "invalid_argument");
    server.finish();
}

#[cfg(feature = "asi-query")]
#[test]
fn lane_health_reports_published_agent_lineage_in_cli_and_mcp() {
    let home = Home::new();
    let private = "PRIVATE-LANE-FIXTURE-BODY";
    let reply = |id, minute, content| {
        let mut row = assistant(id, ts(6, minute), content);
        row["message"]["model"] = json!("claude-sonnet-4-5");
        row["uuid"] = json!(format!("{id}-reply-{minute}"));
        row
    };
    let spawn = |id| json!([{"type":"tool_use","id":id,"name":"Agent","input":{"prompt":private}}]);
    let text = || json!([{"type":"text","text":private}]);
    let compact = |minute, trigger, pre_tokens| {
        json!({
            "type":"system", "subtype":"compact_boundary", "uuid":format!("compact-{minute}"),
            "sessionId":"manager", "timestamp":ts(6, minute), "compactMetadata":{"trigger":trigger,"preTokens":pre_tokens}
        })
    };
    let copied = reply("manager", 1, text());
    home.lines(
        "claude/projects/-work-proj/manager.jsonl",
        &[
            human("manager", ts(6, 0), private),
            reply("manager", 0, spawn("child-call")),
            copied.clone(),
            // A native prompt separates independent compactions. Consecutive
            // boundaries at one event position normalize to a single signal.
            compact(2, "auto", 1000),
            human("manager", ts(6, 2), private),
            compact(3, "manual", 1000),
            human("manager", ts(6, 3), private),
            compact(4, "auto", 2000),
            reply("manager", 5, text()),
        ],
    );
    let mut child_copy = copied;
    child_copy["sessionId"] = json!("lane-child");
    let mut child_reply = reply("lane-child", 4, spawn("grandchild-call"));
    child_reply["message"]["usage"]["input_tokens"] = json!(20000);
    child_reply["message"]["model"] = json!("unknown-fixture-model");
    let agents = "claude/projects/-work-proj/manager/subagents";
    home.lines(
        &format!("{agents}/agent-lane-child.jsonl"),
        &[
            human("lane-child", ts(6, 1), private),
            child_copy,
            child_reply,
        ],
    );
    home.write(
        &format!("{agents}/agent-lane-child.meta.json"),
        r#"{"toolUseId":"child-call"}"#,
    );
    home.lines(
        &format!("{agents}/agent-lane-grandchild.jsonl"),
        &[
            human("lane-grandchild", ts(6, 5), private),
            reply("lane-grandchild", 6, text()),
        ],
    );
    home.write(
        &format!("{agents}/agent-lane-grandchild.meta.json"),
        r#"{"toolUseId":"grandchild-call"}"#,
    );

    let published = home.semon(&["analytics", "materialize"]);
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    let namespace = semon_sessions::read_analytics_namespace(&home.root.join("state"))
        .unwrap()
        .unwrap();
    let uid = |id| format!("{namespace}/claude/{id}");
    let root = uid("manager");
    let child = uid("lane-child");
    let grandchild = uid("lane-grandchild");
    let mut report = home.query(&["query", "lane_health", &root]);
    assert!(!report.to_string().contains(private));
    assert_eq!(report["asi_schema_version"], 5);
    let manifest = semon_asi::read_manifest(&home.root.join("state/analytics-store"))
        .unwrap()
        .unwrap();
    assert_eq!(report["snapshot"], manifest.generation.to_string());
    assert_eq!(report["root_session_uid"], root);
    assert_eq!(report["observed_children"], json!([child]));
    let members = report["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    let member = |uid: &str| {
        members
            .iter()
            .find(|member| member["session_uid"] == uid)
            .unwrap()
    };
    assert_eq!(member(&grandchild)["parent_session_uid"], child);
    assert_eq!(member(&child)["parent_session_uid"], root);
    assert_eq!(member(&child)["compactions"], json!([]));
    assert_eq!(member(&root)["compaction_count"], 3);
    let compactions = member(&root)["compactions"].as_array().unwrap();
    assert_eq!(compactions[0]["trigger"], "auto");
    assert_eq!(compactions[0]["pre_tokens"], 1000);
    assert_eq!(compactions[0]["post_refill_tokens"], 3000);
    assert_eq!(compactions[0]["early"], true);
    assert_eq!(compactions[1]["early"], false);
    assert_eq!(compactions[2]["early"], false);
    assert!(
        compactions
            .iter()
            .all(|compaction| compaction["storm"] == true)
    );
    let tools = report["manager_tool_mix"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["calls"], 1);
    let usage = report["usage"].as_array().unwrap();
    let bucket = |owner: &str| {
        usage
            .iter()
            .find(|row| row["usage_owner"] == owner)
            .unwrap()
    };
    assert_eq!(bucket("exclusive")["steps"], 4);
    assert_eq!(bucket("exclusive")["priced_steps"], 3);
    assert_eq!(bucket("exclusive")["unpriced_steps"], 1);
    assert_eq!(bucket("copied")["steps"], 2);
    assert!(report["runtime"]["observed_at"].is_number(), "{report}");
    assert!(
        members
            .iter()
            .all(|member| member["live"]["last_activity"].is_number()),
        "{report}"
    );
    assert!(member(&child)["live"]["alive"].is_null());

    let mut server = Server::start(&home);
    let response = server.call(1, "lane_health", json!({"root_session_uid": root}));
    assert_eq!(response["result"]["isError"], false, "{response}");
    let mut mcp: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    for key in ["now", "observed_at"] {
        report["runtime"][key] = Value::Null;
        mcp["runtime"][key] = Value::Null;
    }
    assert_eq!(mcp, report);
    server.finish();
}

#[cfg(feature = "asi-query")]
#[test]
fn lane_health_matches_active_continuation_and_discovers_first_publication() {
    let mut home = Home::new();
    let original = "claude/projects/-work-proj/resume-root.jsonl";
    home.lines(
        original,
        &[
            human("resume-root", ts(7, 0), "Original prompt"),
            assistant("resume-root", ts(7, 1), json!([{"type":"text","text":"Original reply"}])),
            json!({"type":"continued-in", "sessionId":"resume-root", "continuedInSessionId":"resume-active"}),
        ],
    );
    fs::File::options()
        .write(true)
        .open(home.root.join(original))
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    home.lines(
        "claude/projects/-work-proj/resume-active.jsonl",
        &[human("resume-active", ts(8, 0), "Resumed prompt")],
    );
    home.write(
        "claude/sessions/31.json",
        &json!({"pid":31, "sessionId":"resume-active", "procStart":777, "status":"busy", "updatedAt":at(8, 0)}).to_string(),
    );

    home.window = vec!["--since", "1d"];
    let mut server = Server::start(&home);
    let (error, failed) = content(&server.call(
        1,
        "lane_health",
        json!({"root_session_uid":"origin/claude/resume-root"}),
    ));
    assert!(failed);
    assert_eq!(error["error"]["code"], "store_unavailable");

    home.window = vec!["--all"];
    let published = home.semon(&["analytics", "materialize"]);
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    home.window = vec!["--since", "1d"];
    let namespace = semon_sessions::read_analytics_namespace(&home.root.join("state"))
        .unwrap()
        .unwrap();
    let uid = format!("{namespace}/claude/resume-root");
    let native = home.query(&["query", "list_sessions"]);
    let sessions = native["sessions"].as_array().unwrap();
    assert!(
        !sessions
            .iter()
            .any(|session| session["id"] == "resume-root")
    );
    let active = sessions
        .iter()
        .find(|session| session["id"] == "resume-active")
        .unwrap();
    assert_eq!(active["alive"], true);
    assert_eq!(active["state"], "working");
    let expected =
        json!({"state":active["state"],"alive":true,"last_activity":active["last_activity"]});
    let (mcp, failed) = content(&server.call(2, "lane_health", json!({"root_session_uid":uid})));
    assert!(!failed, "{mcp}");
    let cli = home.query(&["query", "lane_health", &uid]);
    for report in [&cli, &mcp] {
        assert_eq!(report["members"].as_array().unwrap().len(), 1);
        assert_eq!(report["members"][0]["session_uid"], uid);
        assert_eq!(report["members"][0]["live"], expected);
    }
    server.finish();
}

#[cfg(feature = "asi-query")]
#[test]
fn lane_health_reports_worker_dispositions_and_keeps_guardian_in_family() {
    let home = Home::new();
    let private = "PRIVATE-WORKER-OUTCOME-FIXTURE";
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let pr = "https://github.com/example/repo/pull/1";
    let blocked = format!("Outcome: blocked · PR {pr} · head {sha} · checks red");
    let cases = [
        ("completed", "Outcome: completed", "completed", false),
        ("blocked", blocked.as_str(), "blocked", false),
        ("cancelled", "Outcome: cancelled", "cancelled", false),
        (
            "malformed",
            "Outcome: blocked · PR https://github.com/example/repo/pull/1",
            "unknown",
            false,
        ),
        ("missing", "", "unknown", false),
        ("failed", "", "unknown", true),
        ("unlinked", "Outcome: completed", "unknown", false),
        ("reused", "Outcome: completed", "unknown", false),
    ];
    let project = "claude/projects/-work-proj";
    let mut claude_parent = vec![human("lane-manager", ts(6, 0), private)];
    let mut codex_parent = Vec::new();
    let codex = |kind, payload| json!({"type":kind,"timestamp":ts(6, 1),"payload":payload});
    let codex_prefix = |id: &str, header| {
        vec![
            codex("session_meta", header),
            codex("turn_context", json!({"model":"gpt-6-sol"})),
            codex(
                "token_usage_record",
                json!({"thread_id":id,"session_id":"session","turn_id":"turn","response_id":"response", "usage":{
                    "input_tokens":7,"output_tokens":0,"total_tokens":7,"cached_input_tokens":0,
                    "cache_write_input_tokens":0,"reasoning_output_tokens":0
                }}),
            ),
        ]
    };
    codex_parent.extend(codex_prefix(
        "lane-manager",
        json!({"id":"lane-manager","thread_source":"user"}),
    ));
    for (i, (worker, line, _, failed)) in cases.iter().enumerate() {
        let call = format!("spawn-{worker}");
        let text = format!("{private}\n{line}");
        if *worker != "unlinked" {
            claude_parent.push(assistant(
                "lane-manager",
                ts(6, i as i64 + 1),
                json!([
                    {"type":"tool_use","id":call,"name":"Agent","input":{"prompt":private}}
                ]),
            ));
            home.write(
                &format!("{project}/lane-manager/subagents/agent-{worker}.meta.json"),
                &json!({"toolUseId":call}).to_string(),
            );
            let task = if *worker == "completed" {
                "audit_parser"
            } else {
                worker
            };
            codex_parent.push(codex("response_item", json!({"type":"function_call","name":"spawn_agent",
                "call_id":call, "arguments":json!({"task_name":task,"prompt":private}).to_string()})));
        }
        let mut final_reply = assistant(
            worker,
            ts(6, 15),
            json!([
                {"type":"text","text":"Earlier block ends with a decoy\nOutcome: completed"},
                {"type":"text","text":text}
            ]),
        );
        final_reply["message"]["stop_reason"] = json!("end_turn");
        let mut claude_child = vec![human(worker, ts(6, 10), "Outcome: completed"), final_reply];
        if *worker == "reused" {
            claude_child.push(human(worker, ts(6, 16), private));
        }
        home.lines(
            &format!("{project}/lane-manager/subagents/agent-{worker}.jsonl"),
            &claude_child,
        );
        if *failed {
            claude_parent.push(json!({"type":"user","timestamp":ts(6, 16),"sessionId":"lane-manager",
                "origin":{"kind":"task-notification"},"message":{"role":"user","content":format!(
                    "<task-notification><tool-use-id>{call}</tool-use-id><status>failed</status><result>{private}</result></task-notification>"
                )}}));
        }
        let mut header = json!({"id":worker,"thread_source":"subagent",
            "parent_thread_id":"lane-manager","agent_nickname":worker});
        if *worker == "completed" {
            header["agent_path"] = json!("/root/audit_parser");
            header["agent_nickname"] = json!("Socrates");
            codex_parent.push(codex(
                "response_item",
                json!({"type":"function_call","name":"spawn_agent",
                "call_id":"nickname-decoy", "arguments":"{\"task_name\":\"Socrates\"}"}),
            ));
        }
        let mut records = codex_prefix(worker, header);
        records.push(codex(
            "event_msg",
            json!({"type":"task_started","turn_id":"turn"}),
        ));
        records.push(codex("event_msg", json!({"type":"task_complete","turn_id":"unowned","last_agent_message":"Outcome: completed"})));
        let terminal = if *worker == "cancelled" {
            "turn_complete"
        } else {
            "task_complete"
        };
        let mut completion = json!({"type":terminal,"turn_id":"turn","last_agent_message":text});
        if *failed {
            completion["error"] = json!({"message":private});
            completion["last_agent_message"] = json!("Outcome: completed");
        }
        records.push(codex("event_msg", completion));
        if *worker == "reused" {
            records.push(codex(
                "event_msg",
                json!({"type":"turn_started","turn_id":"second"}),
            ));
        }
        home.lines(
            &format!("codex/sessions/2026/09/24/rollout-{worker}.jsonl"),
            &records,
        );
    }
    home.lines(&format!("{project}/lane-manager.jsonl"), &claude_parent);
    home.lines(
        "codex/sessions/2026/09/24/rollout-lane-manager.jsonl",
        &codex_parent,
    );
    home.lines("codex/sessions/2026/09/24/rollout-guardian.jsonl", &codex_prefix("guardian", json!({
        "id":"guardian", "thread_source":"guardian_review", "parent_thread_id":"lane-manager",
        // A matching name would otherwise compete for the Worker's dispatch.
        "agent_nickname":"audit_parser"
    })));

    let published = home.semon(&["analytics", "materialize"]);
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    let state = home.root.join("state");
    let namespace = semon_sessions::read_analytics_namespace(&state)
        .unwrap()
        .unwrap();
    for harness in ["claude", "codex"] {
        let root = format!("{namespace}/{harness}/lane-manager");
        let report = home.query(&["query", "lane_health", &root]);
        assert!(!report.to_string().contains(private));
        assert_eq!(
            report["worker_stops"],
            json!({"completed":1,"blocked":1,"cancelled":1,
            "unknown":5,"stops_without_blocker":1,"harness_failed":1})
        );
        let tasks = report["worker_tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), cases.len());
        for (worker, _, disposition, failed) in cases {
            let task = tasks
                .iter()
                .find(|task| task["session_uid"] == format!("{namespace}/{harness}/{worker}"))
                .unwrap();
            assert_eq!(task["disposition"], disposition, "{task}");
            assert_eq!(task["harness_failed"], failed);
            if worker == "unlinked" {
                assert!(task["dispatch_call_id"].is_null());
            } else {
                assert_eq!(task["dispatch_call_id"], format!("spawn-{worker}"));
            }
            if worker == "blocked" {
                assert_eq!(task["pr_url"], pr);
                assert_eq!(task["head_sha"], sha);
                assert_eq!(task["checks_state"], "red");
            } else {
                assert!(task["pr_url"].is_null());
            }
        }
        if harness == "codex" {
            let guardian = report["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|member| member["session_uid"] == format!("{namespace}/codex/guardian"))
                .unwrap();
            assert_eq!(guardian["parent_session_uid"], root);
            assert_eq!(guardian["role"], "reviewer");
        }
    }
    let index = semon_sessions::analytics_index_path(&state.join("index.json"));
    let db =
        rusqlite::Connection::open_with_flags(index, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = db.prepare("SELECT s.harness,o.block,o.disposition,o.pr_url,o.head_sha,o.checks_state
        FROM fact_worker_outcome o JOIN warehouse_source s USING(source) ORDER BY s.harness,o.disposition").unwrap();
    let mut rows = statement.query([]).unwrap();
    for (harness, position) in [("claude", 1), ("codex", 0)] {
        for disposition in [
            "blocked",
            "cancelled",
            "completed",
            "completed",
            "completed",
        ] {
            let row = rows.next().unwrap().unwrap();
            assert_eq!(row.get::<_, String>(0).unwrap(), harness);
            assert_eq!(row.get::<_, i64>(1).unwrap(), position);
            assert_eq!(row.get::<_, String>(2).unwrap(), disposition);
            let expected = if disposition == "blocked" {
                [Some(pr), Some(sha), Some("red")]
            } else {
                [None; 3]
            };
            for (i, value) in (3..6).zip(expected) {
                assert_eq!(row.get::<_, Option<String>>(i).unwrap().as_deref(), value);
            }
        }
    }
    assert!(rows.next().unwrap().is_none());
}
