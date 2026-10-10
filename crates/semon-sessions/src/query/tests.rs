//! The read surface's tools on a synthetic home, their JSON pinned: one
//! session per state, and a stalled one per reason code.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{Value, json};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// 2026-09-24T00:00:00Z.
const BASE: i64 = 1_790_208_000_000;
/// A day later: every session in the home has been quiet for hours.
const NOW: i64 = BASE + 86_400_000;

fn at(hour: i64, minute: i64) -> i64 {
    BASE + (hour * 60 + minute) * 60_000
}

fn ts(hour: i64, minute: i64) -> String {
    format!("2026-09-24T{hour:02}:{minute:02}:00.000Z")
}

struct Home {
    root: PathBuf,
    options: Options,
}

impl Home {
    fn new(host: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-query-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        let home = Self { root, options };
        home.write("proc/locks", "");
        home.write("proc/sys/kernel/hostname", &format!("{host}\n"));
        home
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn lines(&self, relative: &str, records: &[Value]) -> PathBuf {
        let text: String = records.iter().map(|record| format!("{record}\n")).collect();
        self.write(relative, &text)
    }

    fn top(&self, id: &str, records: &[Value]) {
        self.lines(&format!("claude/projects/-work-proj/{id}.jsonl"), records);
    }

    fn agent(&self, parent: &str, agent: &str, tool_use: &str, records: &[Value]) {
        self.lines(
            &format!("claude/projects/-work-proj/{parent}/subagents/agent-{agent}.jsonl"),
            records,
        );
        self.write(
            &format!("claude/projects/-work-proj/{parent}/subagents/agent-{agent}.meta.json"),
            &json!({"agentType": "general-purpose", "description": format!("task {agent}"), "toolUseId": tool_use})
                .to_string(),
        );
    }

    /// A process whose start time (`/proc/<pid>/stat` field 22) is `start`.
    fn process(&self, pid: u32, start: u64) {
        let mut fields = vec!["0".to_owned(); 20];
        fields[19] = start.to_string();
        self.write(
            &format!("proc/{pid}/stat"),
            &format!("{pid} (claude) {}\n", fields.join(" ")),
        );
    }

    /// Claude's `sessions/<pid>.json` for `session`, recording `start`.
    fn pid_file(&self, pid: u32, session: &str, status: &str, start: u64) {
        self.write(
            &format!("claude/sessions/{pid}.json"),
            &json!({"pid": pid, "sessionId": session, "procStart": start, "status": status, "name": format!("{session}-lane")})
                .to_string(),
        );
    }

    fn query(&self) -> Query {
        Query::new(self.options.clone())
    }

    /// `--model-json` of this home at NOW: the reference the tools must
    /// agree with.
    fn model(&self) -> Value {
        serde_json::from_str(&crate::model_json_at(&self.options, NOW).unwrap()).unwrap()
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

fn user(sid: &str, time: String, text: &str) -> Value {
    json!({"type": "user", "timestamp": time, "sessionId": sid, "cwd": "/work/proj", "message": {"role": "user", "content": text}})
}

fn assistant(sid: &str, time: String, blocks: Vec<Value>) -> Value {
    json!({"type": "assistant", "timestamp": time.clone(), "sessionId": sid, "cwd": "/work/proj",
        "message": {"id": format!("m-{sid}-{time}"), "model": "claude-opus-5-5", "role": "assistant", "content": blocks,
            "usage": {"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 2000, "output_tokens": 500}}})
}

fn text(value: &str) -> Value {
    json!({"type": "text", "text": value})
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn codex_line(time: String, kind: &str, payload: Value) -> Value {
    json!({"timestamp": time, "type": kind, "payload": payload})
}

/// One session per state: `lead` works (and its subagent `scan` with it),
/// `asker` waits on a question, `crashed`'s process died while its pid file
/// said busy, `done` ended, and the Codex run `cx` ended.
fn fixture() -> Home {
    let home = Home::new("testbox");
    home.top(
        "lead",
        &[
            human("lead", ts(1, 0), "Port the parser"),
            assistant("lead", ts(1, 1), vec![text("On it.")]),
            assistant(
                "lead",
                ts(1, 2),
                vec![tool(
                    "t1",
                    "Agent",
                    json!({"description": "scan", "prompt": "Scan the parser for panics"}),
                )],
            ),
            assistant(
                "lead",
                ts(1, 3),
                vec![tool(
                    "b1",
                    "Bash",
                    json!({"command": "cargo test -p parser"}),
                )],
            ),
        ],
    );
    home.process(30, 777);
    home.pid_file(30, "lead", "busy", 777);
    home.agent(
        "lead",
        "scan",
        "t1",
        &[
            user("scan", ts(1, 2), "Scan the parser for panics"),
            assistant("scan", ts(1, 4), vec![text("Found one in lexer.rs")]),
        ],
    );
    home.top(
        "asker",
        &[
            human("asker", ts(2, 0), "Pick a name"),
            assistant(
                "asker",
                ts(2, 1),
                vec![tool(
                    "q1",
                    "AskUserQuestion",
                    json!({"questions": [{"question": "Which name?", "header": "Name", "multiSelect": false,
                        "options": [{"label": "semon"}, {"label": "ostrom"}]}]}),
                )],
            ),
        ],
    );
    home.process(31, 777);
    home.pid_file(31, "asker", "idle", 777);
    home.top(
        "crashed",
        &[
            human("crashed", ts(3, 0), "Refactor the store"),
            assistant(
                "crashed",
                ts(3, 1),
                vec![tool("b2", "Bash", json!({"command": "cargo build"}))],
            ),
        ],
    );
    // Its pid file says busy, but no process 32 runs.
    home.pid_file(32, "crashed", "busy", 555);
    home.top(
        "done",
        &[
            human("done", ts(4, 0), "Summarize the design doc"),
            assistant(
                "done",
                ts(4, 1),
                vec![text("The design keeps the frontend and replaces the data.")],
            ),
        ],
    );
    home.lines(
        "codex/sessions/2026/09/24/rollout-cx.jsonl",
        &[
            codex_line(
                ts(5, 0),
                "session_meta",
                json!({"id": "cx", "cwd": "/work/proj", "originator": "codex_exec", "thread_source": "user"}),
            ),
            codex_line(
                ts(5, 1),
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Implement the lexer fix"}]}),
            ),
            codex_line(
                ts(5, 2),
                "response_item",
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fixed the lexer panic"}]}),
            ),
        ],
    );
    home
}

/// The open question's handoff id and each session's first turn id, from
/// the model: they are hashes of source ids.
fn question_id(model: &Value) -> String {
    model["handoffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|handoff| handoff["kind"] == "toyou" && handoff["from"] == "asker")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn turn_id(model: &Value, sid: &str) -> String {
    model["turns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|turn| turn["sid"] == sid)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Every session's summary, pinned.
fn summary(id: &str, question: &str) -> Value {
    let tokens = json!({"input": 1000, "cached": 2000, "output": 500});
    let base = json!({
        "id": id, "harness": "claude", "kind": "session", "model": "opus-5.5", "repo": null, "branch": null,
        "machine": "testbox", "parent": null, "children": [], "pid": null, "alive": null, "exit": null,
        "open_question": null, "tokens": tokens, "run": null, "turns_truncated": false,
    });
    let fields = match id {
        "lead" => {
            json!({"name": "lead-lane", "children": ["scan"], "state": "working", "start": at(1, 0),
            "last_activity": at(1, 3), "pid": 30, "alive": true,
            "tokens": {"input": 3000, "cached": 6000, "output": 1500}})
        }
        "scan" => {
            json!({"kind": "subagent", "name": "task scan", "parent": "lead", "state": "working",
            "start": at(1, 2), "last_activity": at(1, 4)})
        }
        "asker" => json!({"name": "asker-lane", "state": "waiting-question", "start": at(2, 0),
            "last_activity": at(2, 1), "pid": 31, "alive": true,
            "open_question": {"text": "Which name?", "asked_at": at(2, 1), "handoff": question}}),
        "crashed" => {
            json!({"name": "proj", "state": "ended", "start": at(3, 0), "last_activity": at(3, 1),
            "pid": 32, "alive": false})
        }
        "done" => {
            json!({"name": "proj", "state": "ended", "start": at(4, 0), "last_activity": at(4, 1)})
        }
        "cx" => {
            json!({"harness": "codex", "kind": "codex-run", "name": "Codex run", "model": "codex",
            "state": "ended", "start": at(5, 0), "last_activity": at(5, 2),
            "tokens": {"input": 0, "cached": 0, "output": 0}})
        }
        _ => panic!("no session {id}"),
    };
    let mut out = base;
    for (key, value) in fields.as_object().unwrap() {
        out[key] = value.clone();
    }
    out
}

fn call(query: &mut Query, tool: &str, args: Value) -> Value {
    query
        .call_at(tool, &args, NOW)
        .unwrap_or_else(|error| panic!("{tool} {args}: {error}"))
}

fn failure(query: &mut Query, tool: &str, args: Value) -> &'static str {
    match query.call_at(tool, &args, NOW) {
        Ok(value) => panic!("{tool} {args} succeeded: {value}"),
        Err(error) => error.code,
    }
}

#[test]
fn list_sessions_summarizes_every_session_newest_first() {
    let home = fixture();
    let question = question_id(&home.model());
    let mut query = home.query();
    let all = call(&mut query, "list_sessions", json!({}));
    let order = ["cx", "done", "crashed", "asker", "scan", "lead"];
    assert_eq!(
        all,
        json!({
            "now": NOW,
            "window_start": null,
            "total": 6,
            "truncated": false,
            "sessions": order.iter().map(|id| summary(id, &question)).collect::<Vec<_>>(),
        })
    );
    let mut ids = |args: Value| -> Vec<String> {
        call(&mut query, "list_sessions", args)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(json!({"state": "working"})), ["scan", "lead"]);
    assert_eq!(ids(json!({"state": "waiting-question"})), ["asker"]);
    assert_eq!(ids(json!({"state": "ended"})), ["cx", "done", "crashed"]);
    assert!(ids(json!({"state": "idle"})).is_empty());
    assert_eq!(ids(json!({"harness": "codex"})), ["cx"]);
    assert_eq!(ids(json!({"parent": "lead"})), ["scan"]);
    assert!(ids(json!({"parent": "scan"})).is_empty());
    assert!(ids(json!({"repo": "semon"})).is_empty());
    assert_eq!(
        ids(json!({"since": "2026-09-24T03:00:00Z"})),
        ["cx", "done", "crashed"]
    );
    // 20 hours before NOW is 04:00.
    assert_eq!(ids(json!({"since": "20h"})), ["cx", "done"]);
    let limited = call(&mut query, "list_sessions", json!({"limit": 2}));
    assert_eq!(
        (&limited["total"], &limited["truncated"]),
        (&json!(6), &json!(true))
    );
    assert_eq!(limited["sessions"].as_array().unwrap().len(), 2);
    let empty = call(&mut query, "list_sessions", json!({"state": "error"}));
    assert_eq!(
        empty,
        json!({"now": NOW, "window_start": null, "total": 0, "truncated": false, "sessions": []})
    );
    assert_eq!(
        failure(&mut query, "list_sessions", json!({"parent": "nobody"})),
        "unknown_session"
    );
}

#[test]
fn get_session_adds_busy_turns_and_handoffs_from_the_model() {
    let home = fixture();
    let model = home.model();
    let question = question_id(&model);
    let mut query = home.query();
    let got = call(&mut query, "get_session", json!({"id": "asker"}));
    let of = |list: &str, keep: &dyn Fn(&Value) -> bool| -> Value {
        Value::Array(
            model[list]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| keep(item))
                .cloned()
                .collect(),
        )
    };
    let turns = of("turns", &|turn| turn["sid"] == "asker");
    let handoffs = of("handoffs", &|handoff| {
        handoff["from"] == "asker" || handoff["to"] == "asker"
    });
    let mut expected = summary("asker", &question);
    expected["busy"] = json!([[at(2, 0), at(2, 1)]]);
    expected["turns"] = turns.clone();
    expected["handoffs"] = handoffs.clone();
    assert_eq!(
        got,
        json!({"now": NOW, "window_start": null, "session": expected})
    );
    // The model's own turn and handoffs: your message, then the question.
    assert_eq!(turns.as_array().unwrap().len(), 1);
    assert_eq!(turns[0]["end"]["why"], "toyou");
    let kinds: Vec<&str> = handoffs
        .as_array()
        .unwrap()
        .iter()
        .map(|handoff| handoff["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["ask", "toyou"]);
    assert_eq!(handoffs[1]["status"], "wait");

    let lead = call(&mut query, "get_session", json!({"id": "lead"}));
    assert_eq!(lead["session"]["children"], json!(["scan"]));
    let spawn = lead["session"]["handoffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|handoff| handoff["kind"] == "spawn")
        .unwrap();
    assert_eq!(
        (&spawn["from"], &spawn["to"]),
        (&json!("lead"), &json!("scan"))
    );
    assert_eq!(
        failure(&mut query, "get_session", json!({"id": "nobody"})),
        "unknown_session"
    );
}

#[test]
fn read_transcript_pages_as_the_viewer_does() {
    let home = fixture();
    let model = home.model();
    let ask = turn_id(&model, "done");
    let mut query = home.query();
    let page = call(&mut query, "read_transcript", json!({"id": "done"}));
    assert_eq!(
        page,
        json!({"sid": "done", "from": 0, "to": 2, "total": 2, "calls": 0, "errors": 0, "entries": [
            {"k": "h", "id": ask, "turn": ask},
            {"k": "a", "text": "The design keeps the frontend and replaces the data."},
        ], "window_start": null})
    );
    // The same page the viewer serves at /api/tx.
    let core = ViewerCore::new(home.options.clone());
    for (args, url) in [
        (json!({"id": "done"}), "sid=done".to_owned()),
        (json!({"id": "scan"}), "sid=scan".to_owned()),
        (
            json!({"id": "done", "after": 1}),
            "sid=done&after=1".to_owned(),
        ),
        (
            json!({"id": "done", "turn": ask}),
            format!("sid=done&turn={ask}"),
        ),
    ] {
        let served = core.respond("GET", "/api/tx", &url, None);
        assert_eq!(served.status, 200, "{url}");
        let served: Value = serde_json::from_slice(&served.body).unwrap();
        let mut ours = call(&mut query, "read_transcript", args);
        let window = ours.as_object_mut().unwrap().remove("window_start");
        assert_eq!(window, Some(Value::Null));
        assert_eq!(ours, served, "{url}");
    }
    let last = call(
        &mut query,
        "read_transcript",
        json!({"id": "done", "limit": 1}),
    );
    assert_eq!(
        (&last["from"], &last["to"], &last["entries"]),
        (
            &json!(1),
            &json!(2),
            &json!([{"k": "a", "text": "The design keeps the frontend and replaces the data.", "turn": ask}])
        )
    );
    let before = call(
        &mut query,
        "read_transcript",
        json!({"id": "done", "before": 1}),
    );
    assert_eq!(
        before["entries"],
        json!([{"k": "h", "id": ask, "turn": ask}])
    );
    assert_eq!(
        failure(&mut query, "read_transcript", json!({"id": "nobody"})),
        "unknown_session"
    );
    assert_eq!(
        failure(
            &mut query,
            "read_transcript",
            json!({"id": "done", "turn": "nope"})
        ),
        "unknown_turn"
    );
    assert_eq!(
        failure(
            &mut query,
            "read_transcript",
            json!({"id": "done", "before": 1, "after": 0})
        ),
        "invalid_arguments"
    );
}

#[test]
fn find_searches_entries_newest_first_within_its_bounds() {
    let home = fixture();
    let model = home.model();
    let (cx, scan) = (turn_id(&model, "cx"), turn_id(&model, "scan"));
    let mut query = home.query();
    let mut found = call(&mut query, "find", json!({"text": "LEXER"}));
    let scanned = found["scanned_bytes"].as_u64().unwrap();
    let written: u64 = ["claude/projects", "codex/sessions"]
        .iter()
        .flat_map(|dir| walk(&home.root.join(dir)))
        .map(|path| fs::metadata(path).unwrap().len())
        .sum();
    assert!(
        scanned > 0 && scanned <= written,
        "{scanned} of {written} bytes"
    );
    found["scanned_bytes"] = json!("counted");
    assert_eq!(
        found,
        json!({
            "now": NOW,
            "window_start": null,
            "text": "LEXER",
            "matches": [
                {"session": "cx", "position": 1, "turn": cx, "kind": "a", "part": "text", "at": at(5, 2),
                    "snippet": "Fixed the lexer panic"},
                {"session": "cx", "position": 0, "turn": cx, "kind": "u", "part": "text", "at": at(5, 1),
                    "snippet": "Implement the lexer fix"},
                {"session": "scan", "position": 1, "turn": scan, "kind": "a", "part": "text", "at": at(1, 4),
                    "snippet": "Found one in lexer.rs"},
            ],
            "sessions": [
                {"id": "cx", "name": "Codex run", "repo": null, "last_activity": at(5, 2), "fields": [], "entries": 2},
                {"id": "scan", "name": "task scan", "repo": null, "last_activity": at(1, 4), "fields": [], "entries": 1},
            ],
            "scanned_bytes": "counted",
            "truncated": false,
            "truncated_by": null,
        })
    );
    // Handoffs are searched by their text in the model; tool calls by
    // their input and output.
    let brief = call(&mut query, "find", json!({"text": "panics"}));
    let hits: Vec<(&str, &str, &str)> = brief["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| {
            (
                hit["session"].as_str().unwrap(),
                hit["kind"].as_str().unwrap(),
                hit["part"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        hits,
        [("scan", "h", "brief"), ("lead", "h", "brief")],
        "{brief}"
    );
    let command = call(&mut query, "find", json!({"text": "cargo build"}));
    assert_eq!(command["matches"][0]["kind"], "tool");
    assert_eq!(command["matches"][0]["name"], "Bash");
    assert_eq!(command["matches"][0]["part"], "in");
    // A session's name is matched too.
    let named = call(&mut query, "find", json!({"text": "task sc"}));
    assert_eq!(named["matches"], json!([]));
    assert_eq!(
        named["sessions"],
        json!([{"id": "scan", "name": "task scan", "repo": null, "last_activity": at(1, 4), "fields": ["name"], "entries": 0}])
    );
    // Bounds: a result limit, a byte budget, and since.
    let limited = call(&mut query, "find", json!({"text": "lexer", "limit": 1}));
    assert_eq!(limited["matches"].as_array().unwrap().len(), 1);
    assert_eq!(
        (&limited["truncated"], &limited["truncated_by"]),
        (&json!(true), &json!("limit"))
    );
    let budget = call(&mut query, "find", json!({"text": "lexer", "max_bytes": 1}));
    assert_eq!(budget["matches"].as_array().unwrap().len(), 1);
    assert_eq!(budget["truncated_by"], "bytes");
    let recent = call(
        &mut query,
        "find",
        json!({"text": "lexer", "since": "2026-09-24T05:01:30Z"}),
    );
    assert_eq!(recent["matches"].as_array().unwrap().len(), 1);
    assert_eq!(recent["matches"][0]["position"], 1);
    let none = call(&mut query, "find", json!({"text": "no such words"}));
    assert_eq!(
        (&none["matches"], &none["sessions"], &none["truncated"]),
        (&json!([]), &json!([]), &json!(false))
    );
    assert_eq!(
        failure(&mut query, "find", json!({"text": ""})),
        "invalid_arguments"
    );
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    out
}

#[test]
fn stalls_give_one_closed_reason_per_session() {
    let home = fixture();
    let question = question_id(&home.model());
    let mut query = home.query();
    let stalls = call(&mut query, "stalls", json!({"idle_minutes": 60}));
    let row = |id: &str, reason: &str, last: i64| {
        let mut row = summary(id, &question);
        row["stall_reason"] = json!(reason);
        row["idle_ms"] = json!(NOW - last);
        row
    };
    assert_eq!(
        stalls,
        json!({
            "now": NOW,
            "window_start": null,
            "idle_minutes": 60,
            "rule": STALL_RULE,
            "stalls": [
                row("lead", "no-output", at(1, 3)),
                row("scan", "no-output", at(1, 4)),
                row("asker", "waiting-question", at(2, 1)),
                row("crashed", "process-gone", at(3, 1)),
            ],
        })
    );
    let recent = call(
        &mut query,
        "stalls",
        json!({"idle_minutes": 60, "since": "22h"}),
    );
    let ids: Vec<&str> = recent["stalls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["asker", "crashed"]);
    // Nothing has been quiet for a year.
    let none = call(&mut query, "stalls", json!({"idle_minutes": 525_600}));
    assert_eq!(none["stalls"], json!([]));
    assert_eq!(
        failure(&mut query, "stalls", json!({})),
        "invalid_arguments"
    );
}

#[test]
fn arguments_are_checked_against_the_schema() {
    let home = fixture();
    let mut query = home.query();
    assert_eq!(failure(&mut query, "nope", json!({})), "unknown_tool");
    for (tool, args) in [
        ("list_sessions", json!([])),
        ("list_sessions", json!({"state": "sleeping"})),
        ("list_sessions", json!({"limit": 0})),
        ("list_sessions", json!({"limit": "ten"})),
        ("list_sessions", json!({"since": "yesterday"})),
        ("list_sessions", json!({"bogus": 1})),
        ("get_session", json!({})),
        ("get_session", json!({"id": 5})),
        ("read_transcript", json!({"id": "done", "limit": 201})),
        ("stalls", json!({"idle_minutes": 1.5})),
    ] {
        assert_eq!(
            failure(&mut query, tool, args.clone()),
            "invalid_arguments",
            "{tool} {args}"
        );
    }
    // A null is an argument not given, and a whole number is an integer.
    let listed = call(
        &mut query,
        "list_sessions",
        json!({"state": null, "limit": 2.0}),
    );
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(call(&mut query, "list_sessions", Value::Null)["total"], 6);
}

#[test]
fn several_machines_answer_as_one_model() {
    let first = fixture();
    let second = Home::new("desktop");
    second.top(
        "other",
        &[
            human("other", ts(6, 0), "Review the lexer change"),
            assistant("other", ts(6, 1), vec![text("Looks right.")]),
        ],
    );
    let mut query = Query::with_machines(vec![
        ("a".into(), first.options.clone()),
        ("b".into(), second.options.clone()),
    ]);
    let listed = call(
        &mut query,
        "list_sessions",
        json!({"since": "2026-09-24T05:00:00Z"}),
    );
    let rows: Vec<(&str, &str)> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["machine"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("other", "desktop"), ("cx", "testbox")]);
    let page = call(&mut query, "read_transcript", json!({"id": "other"}));
    assert_eq!(page["entries"][1]["text"], "Looks right.");
    let found = call(&mut query, "find", json!({"text": "lexer change"}));
    assert_eq!(found["matches"][0]["session"], "other");
    assert_eq!(found["matches"][0]["part"], "brief");
    assert_eq!(
        failure(&mut query, "get_session", json!({"id": "nobody"})),
        "unknown_session"
    );
    assert_eq!(
        failure(&mut query, "read_transcript", json!({"id": "nobody"})),
        "unknown_session"
    );
}

/// `ms` (epoch ms, UTC) as RFC 3339, as the logs write it.
fn iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let of_day = ms.rem_euclid(86_400_000);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1000 % 60,
        of_day % 1000
    )
}

#[test]
fn iso_writes_what_the_logs_parse() {
    for ms in [BASE, at(13, 7) + 250, 951_782_400_000, 1_709_164_800_999] {
        assert_eq!(crate::events::parse_ms(&iso(ms)), Some(ms), "{}", iso(ms));
    }
}

const DAY_MS: i64 = 86_400_000;

/// A home on the real clock: `recent` wrote an hour ago; `old` wrote 60
/// days ago, and so did its file.
fn aged() -> Home {
    let home = Home::new("testbox");
    let now = now_ms();
    let session = |id: &str, at: i64| {
        [
            human(id, iso(at), &format!("Work on {id}")),
            assistant(id, iso(at + 60_000), vec![text(&format!("{id} is done"))]),
        ]
    };
    home.top("recent", &session("recent", now - 3_600_000));
    home.top("old", &session("old", now - 60 * DAY_MS));
    let old = home.root.join("claude/projects/-work-proj/old.jsonl");
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(60 * 86_400))
        .unwrap();
    home
}

fn windowed(home: &Home, all: bool) -> Query {
    Query::new(Options {
        all,
        since: DEFAULT_WINDOW,
        ..home.options.clone()
    })
}

fn ids_of(answer: &Value) -> Vec<&str> {
    answer["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect()
}

/// Whether the event index holds a file's index: whether a build read it.
fn indexed(home: &Home, file: &str) -> bool {
    crate::events::EventCache::open(&home.options.cache)
        .paths()
        .any(|path| path.ends_with(file))
}

#[test]
fn the_window_bounds_what_is_read_and_says_where_it_starts() {
    let home = aged();
    let mut query = windowed(&home, false);
    let listed = query.call("list_sessions", &json!({})).unwrap();
    assert_eq!(ids_of(&listed), ["recent"]);
    assert_eq!(
        listed["now"].as_i64().unwrap() - listed["window_start"].as_i64().unwrap(),
        30 * DAY_MS
    );
    assert!(indexed(&home, "recent.jsonl"));
    assert!(
        !indexed(&home, "old.jsonl"),
        "a file outside the window is not read"
    );
    // Every tool names the window.
    for (tool, args) in [
        ("get_session", json!({"id": "recent"})),
        ("read_transcript", json!({"id": "recent"})),
        ("find", json!({"text": "recent", "since": "10d"})),
        ("stalls", json!({"idle_minutes": 1, "since": "30d"})),
    ] {
        let answer = query.call(tool, &args).unwrap();
        assert!(answer["window_start"].is_i64(), "{tool}: {answer}");
    }
    assert_eq!(
        query
            .call("get_session", &json!({"id": "old"}))
            .unwrap_err()
            .code,
        "unknown_session"
    );
    // --all reads everything and has no start; a windowed build after it
    // keeps the older file's index for the next full build.
    let mut all = windowed(&home, true);
    let everything = all.call("list_sessions", &json!({})).unwrap();
    assert_eq!(ids_of(&everything), ["recent", "old"]);
    assert_eq!(everything["window_start"], Value::Null);
    assert!(indexed(&home, "old.jsonl"));
    let mut again = windowed(&home, false);
    assert_eq!(
        ids_of(&again.call("list_sessions", &json!({})).unwrap()),
        ["recent"]
    );
    assert!(indexed(&home, "old.jsonl"));
}

#[test]
fn a_since_before_the_window_is_refused_not_cut_short() {
    let home = aged();
    let mut query = windowed(&home, false);
    let before = iso(now_ms() - 40 * DAY_MS);
    for (tool, args) in [
        ("list_sessions", json!({"since": "40d"})),
        ("list_sessions", json!({"since": before})),
        ("find", json!({"text": "old", "since": "31d"})),
        ("stalls", json!({"idle_minutes": 5, "since": "6w"})),
    ] {
        let error = query.call(tool, &args).unwrap_err();
        assert_eq!(error.code, "outside_window", "{tool} {args}");
        let start = error.window_start.unwrap();
        assert!((now_ms() - 30 * DAY_MS - start).abs() < 60_000);
        assert_eq!(error.to_json()["error"]["window_start"], start);
    }
    let inside = query
        .call("list_sessions", &json!({"since": "29d"}))
        .unwrap();
    assert_eq!(ids_of(&inside), ["recent"]);
    // With --all there is no window to reach outside of.
    let mut all = windowed(&home, true);
    let old = all.call("list_sessions", &json!({"since": "90d"})).unwrap();
    assert_eq!(ids_of(&old), ["recent", "old"]);
}

/// `path`'s modification time, `ago_ms` before now.
fn age(path: &std::path::Path, ago_ms: i64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(
            std::time::SystemTime::now() - Duration::from_millis(u64::try_from(ago_ms).unwrap()),
        )
        .unwrap();
}

#[test]
fn the_window_start_itself_is_inside_the_window() {
    let home = aged();
    let mut query = windowed(&home, false);
    let now = now_ms();
    let listed = query.call_at("list_sessions", &json!({}), now).unwrap();
    let start = listed["window_start"].as_i64().unwrap();
    // Built on this call's clock: the start is exactly 30 days back.
    assert_eq!(start, now - 30 * DAY_MS);
    for since in [json!("30d"), json!(iso(start))] {
        let answer = query
            .call_at("list_sessions", &json!({"since": since}), now)
            .unwrap();
        assert_eq!(ids_of(&answer), ["recent"], "{since}");
    }
    let error = query
        .call_at("list_sessions", &json!({"since": iso(start - 1)}), now)
        .unwrap_err();
    assert_eq!(
        (error.code, error.window_start),
        ("outside_window", Some(start))
    );
}

#[test]
fn a_session_that_crosses_the_window_start_keeps_every_turn() {
    let home = Home::new("testbox");
    let now = now_ms();
    let early = now - 40 * DAY_MS;
    let late = now - 3_600_000;
    // One file, written an hour ago, with a turn from 40 days back.
    home.top(
        "long",
        &[
            human("long", iso(early), "Start the migration"),
            assistant("long", iso(early + 60_000), vec![text("Started.")]),
            human("long", iso(late), "Finish the migration"),
            assistant("long", iso(late + 60_000), vec![text("Finished.")]),
        ],
    );
    // A session split over two files: the first written 60 days ago, the
    // second naming it by `session_id`.
    let first = now - 60 * DAY_MS;
    home.top(
        "first",
        &[
            human("first", iso(first), "Plan the release"),
            assistant("first", iso(first + 60_000), vec![text("Planned.")]),
        ],
    );
    age(
        &home.root.join("claude/projects/-work-proj/first.jsonl"),
        60 * DAY_MS,
    );
    let resumed_at = now - 7_200_000;
    let mut resumed = human("second", iso(resumed_at), "Ship the release");
    resumed["session_id"] = json!("first");
    let mut shipped = assistant("second", iso(resumed_at + 60_000), vec![text("Shipped.")]);
    shipped["session_id"] = json!("first");
    home.top("second", &[resumed, shipped]);

    let mut query = windowed(&home, false);
    let long = query
        .call_at("get_session", &json!({"id": "long"}), now)
        .unwrap();
    let session = &long["session"];
    let turns: Vec<i64> = session["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|turn| turn["at"].as_i64().unwrap())
        .collect();
    assert_eq!(
        turns,
        [early, late],
        "no turn is trimmed at the window's start"
    );
    assert_eq!(session["handoffs"].as_array().unwrap().len(), 2);
    assert_eq!(
        session["busy"],
        json!([[early, early + 60_000], [late, late + 60_000]])
    );
    assert_eq!(session["start"], early);
    assert_eq!(session["turns_truncated"], false);
    // The resumed session's earlier file wasn't read: it says so.
    let listed = query.call_at("list_sessions", &json!({}), now).unwrap();
    let rows: Vec<(&str, bool)> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["turns_truncated"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("long", false), ("second", true)]);
    // Everything read: one session under its first file's id, whole.
    let mut all = windowed(&home, true);
    let listed = all.call_at("list_sessions", &json!({}), now).unwrap();
    let rows: Vec<(&str, bool)> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["turns_truncated"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("long", false), ("first", false)]);
    let whole = all
        .call_at("get_session", &json!({"id": "first"}), now)
        .unwrap();
    assert_eq!(whole["session"]["turns"].as_array().unwrap().len(), 2);
}

#[test]
fn each_machine_keeps_its_own_window_and_the_latest_start_answers() {
    let now = now_ms();
    let wide = Home::new("wide");
    let narrow = Home::new("narrow");
    for (home, id) in [(&wide, "wide-ten"), (&narrow, "narrow-ten")] {
        let at = now - 10 * DAY_MS;
        home.top(
            id,
            &[
                human(id, iso(at), "Ten days ago"),
                assistant(id, iso(at + 60_000), vec![text("Done.")]),
            ],
        );
        age(
            &home
                .root
                .join(format!("claude/projects/-work-proj/{id}.jsonl")),
            10 * DAY_MS,
        );
    }
    let options = |home: &Home, days: u64| Options {
        all: false,
        since: Duration::from_secs(days * 86_400),
        ..home.options.clone()
    };
    let mut query = Query::with_machines(vec![
        ("wide".into(), options(&wide, 30)),
        ("narrow".into(), options(&narrow, 7)),
    ]);
    let listed = query.call_at("list_sessions", &json!({}), now).unwrap();
    // The narrow machine read only its last 7 days; the wide one, 30.
    assert_eq!(ids_of(&listed), ["wide-ten"]);
    assert_eq!(listed["window_start"], now - 7 * DAY_MS);
    let error = query
        .call_at("list_sessions", &json!({"since": "10d"}), now)
        .unwrap_err();
    assert_eq!(error.code, "outside_window");
}

/// Names and values an environment holds that must never reach an answer,
/// a facts file or a log.
const SECRETS: [&str; 6] = [
    "AWS_SECRET_ACCESS_KEY",
    "aws-secret-value",
    "GITHUB_TOKEN",
    "ghp_notarealtoken",
    "wrong-run",
    "run-huge",
];

/// `/proc/<pid>/environ` of the fixture's proc root.
fn environ(home: &Home, pid: u32, bytes: &[u8]) {
    let path = home.root.join(format!("proc/{pid}/environ"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
fn hold_lock(home: &Home, id: &str, pid: u32) {
    use std::os::unix::fs::MetadataExt;
    let path = home.write(&format!("codex/thread-writer-locks/{id}.lock"), "");
    let meta = fs::metadata(path).unwrap();
    let dev = meta.dev();
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    home.write(
        "proc/locks",
        &format!(
            "12: FLOCK ADVISORY WRITE {pid} {major:x}:{minor:x}:{} 0 EOF\n",
            meta.ino()
        ),
    );
}

/// Every live process's environment, each a case: the run ids among other
/// secrets, none, an oversized one, a gone process, an unreadable one, a
/// value that isn't UTF-8, and a Codex writer lock's holder.
#[cfg(unix)]
fn with_runs() -> Home {
    let home = Home::new("testbox");
    for (pid, id) in [
        (40, "ostrom"),
        (41, "plain"),
        (42, "huge"),
        (43, "gone"),
        (44, "unreadable"),
        (45, "binary"),
    ] {
        home.top(
            id,
            &[
                human(id, ts(9, 0), "Go"),
                assistant(id, ts(9, 1), vec![text("Going.")]),
            ],
        );
        home.pid_file(pid, id, "busy", 777);
        if id != "gone" {
            home.process(pid, 777);
        }
    }
    home.agent(
        "ostrom",
        "helper",
        "t9",
        &[
            user("helper", ts(9, 1), "Help"),
            assistant("helper", ts(9, 2), vec![text("Helped.")]),
        ],
    );
    environ(
        &home,
        40,
        b"PATH=/usr/bin\0AWS_SECRET_ACCESS_KEY=aws-secret-value\0OSTROM_RUN_ID=run-1\0\
GITHUB_TOKEN=ghp_notarealtoken\0OSTROM_RUN_IDX=wrong-run\0XOSTROM_RUN_ID=wrong-run\0\
OSTROM_WORK_ORDER_ID=order-7\0OSTROM_RUN_ID=wrong-run\0NOEQUALS\0",
    );
    environ(&home, 41, b"PATH=/usr/bin\0HOME=/home/fake-user\0");
    let mut huge = b"OSTROM_RUN_ID=run-huge\0PAD=".to_vec();
    huge.extend(std::iter::repeat_n(b'x', 300_000));
    huge.push(0);
    environ(&home, 42, &huge);
    fs::create_dir_all(home.root.join("proc/44/environ")).unwrap();
    environ(&home, 45, b"OSTROM_RUN_ID=\xff\xfe\0");
    home.lines(
        "codex/sessions/2026/09/24/rollout-cx.jsonl",
        &[
            codex_line(
                ts(9, 0),
                "session_meta",
                json!({"id": "cx", "cwd": "/work/proj", "originator": "codex_exec", "thread_source": "user"}),
            ),
            codex_line(
                ts(9, 1),
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Go"}]}),
            ),
        ],
    );
    hold_lock(&home, "cx", 4242);
    environ(
        &home,
        4242,
        b"OSTROM_RUN_ID=run-cx\0GITHUB_TOKEN=ghp_notarealtoken\0OSTROM_WORK_ORDER_ID=order-cx\0",
    );
    home
}

fn runs_of(answer: &Value) -> BTreeMap<String, Value> {
    answer["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["id"].as_str().unwrap().to_owned(), row["run"].clone()))
        .collect()
}

fn expected_runs() -> BTreeMap<String, Value> {
    [
        (
            "ostrom",
            json!({"OSTROM_RUN_ID": "run-1", "OSTROM_WORK_ORDER_ID": "order-7"}),
        ),
        ("helper", Value::Null),
        ("plain", json!({})),
        ("huge", Value::Null),
        ("gone", Value::Null),
        ("unreadable", Value::Null),
        ("binary", Value::Null),
        (
            "cx",
            json!({"OSTROM_RUN_ID": "run-cx", "OSTROM_WORK_ORDER_ID": "order-cx"}),
        ),
    ]
    .into_iter()
    .map(|(id, run)| (id.to_owned(), run))
    .collect()
}

fn assert_no_secrets(what: &str, text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{what} holds {secret}");
    }
}

#[cfg(unix)]
#[test]
fn run_ids_come_from_the_allowlist_and_nothing_else() {
    let home = with_runs();
    let mut query = home.query();
    let listed = query.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(runs_of(&listed), expected_runs());
    let rows = listed["sessions"].as_array().unwrap();
    let row = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap();
    assert_eq!(
        (&row("gone")["alive"], &row("cx")["pid"]),
        (&json!(false), &json!(4242))
    );
    // No exit status is in these logs: never inferred.
    assert!(rows.iter().all(|row| row["exit"].is_null()));
    // Nothing else of any environment reaches any answer.
    for (tool, args) in [
        ("list_sessions", json!({})),
        ("get_session", json!({"id": "ostrom"})),
        ("get_session", json!({"id": "cx"})),
        ("read_transcript", json!({"id": "ostrom"})),
        ("find", json!({"text": "o"})),
        ("stalls", json!({"idle_minutes": 1})),
    ] {
        let answer = query.call_at(tool, &args, NOW).unwrap();
        assert_no_secrets(tool, &answer.to_string());
    }
    // Over MCP, the same run ids and nothing else.
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_session","arguments":{"id":"ostrom"}}}"#,
        "\n",
    );
    let mut output = Vec::new();
    crate::serve_mcp(&mut query, input.as_bytes(), &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_no_secrets("MCP output", &output);
    let replies: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let text = |reply: &Value| -> Value {
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    assert_eq!(runs_of(&text(&replies[1])), expected_runs());
    assert_eq!(
        text(&replies[2])["session"]["run"],
        json!({"OSTROM_RUN_ID": "run-1", "OSTROM_WORK_ORDER_ID": "order-7"})
    );
}

#[cfg(unix)]
#[test]
fn run_ids_travel_in_the_facts_and_a_mirror_reads_no_proc() {
    let home = with_runs();
    let facts = crate::local_facts(&home.options).unwrap();
    let run = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    };
    assert_eq!(
        facts.runs,
        BTreeMap::from([
            (
                40,
                run(&[
                    ("OSTROM_RUN_ID", "run-1"),
                    ("OSTROM_WORK_ORDER_ID", "order-7")
                ])
            ),
            (41, run(&[])),
            (
                4242,
                run(&[
                    ("OSTROM_RUN_ID", "run-cx"),
                    ("OSTROM_WORK_ORDER_ID", "order-cx")
                ])
            ),
        ])
    );
    let path = home.root.join("facts.json");
    crate::write_facts(&path, &facts).unwrap();
    assert_no_secrets("the facts file", &fs::read_to_string(&path).unwrap());
    assert_eq!(crate::read_facts(&path).unwrap(), facts);
    // A mirror: the same logs, the recorded facts, and no /proc at all.
    let mirror = Options {
        proc_root: home.root.join("no-proc"),
        cache: home.root.join("mirror/index.json"),
        facts: Some(path),
        ..home.options.clone()
    };
    let mut query = Query::new(mirror);
    let listed = query.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(runs_of(&listed), expected_runs());
    // The offline machine has no live process, so no run.
    let offline = facts.offline(NOW);
    assert!(offline.runs.is_empty());
}

#[test]
fn an_empty_union_answers_without_creating_a_home() {
    let mut query = Query::with_machines(Vec::new());
    let listed = query.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(listed["sessions"], json!([]));
    assert_eq!(listed["window_start"], Value::Null);
    assert_eq!(
        query
            .call_at("find", &json!({"text":"hello"}), NOW)
            .unwrap()["matches"],
        json!([])
    );
    assert_eq!(
        query
            .call_at("get_session", &json!({"id":"missing"}), NOW)
            .unwrap_err()
            .code,
        "unknown_session"
    );
}

#[test]
fn shared_queries_read_the_viewer_cache_and_expose_its_output_window() {
    use crate::Refresh;
    let home = Home::new("shared");
    let mut options = home.options.clone();
    options.all = false;
    options.since = Duration::from_secs(86400);
    let mut viewer = ViewerCore::new(options);
    viewer.set_refresh(Refresh::OnInvalidate);
    let core = Arc::new(viewer);
    let mut first = Query::from_core(core.clone());
    let answer = first.call_at("list_sessions", &json!({}), NOW).unwrap();
    let start = answer["window_start"]
        .as_i64()
        .expect("output-trimmed models have a window too");
    let second = Query::from_core(core.clone())
        .call_at("list_sessions", &json!({}), NOW)
        .unwrap();
    assert_eq!(answer, second);
    assert_eq!(
        first
            .call_at(
                "list_sessions",
                &json!({"since":"1970-01-01T00:00:00Z"}),
                NOW
            )
            .unwrap_err()
            .window_start,
        Some(start)
    );
    // Closing keeps the last snapshot served: the Query does not instantiate
    // or synchronously rebuild another core behind the embedding server.
    core.close();
    assert_eq!(
        first.call_at("list_sessions", &json!({}), NOW).unwrap(),
        answer
    );
}

#[test]
fn embedded_native_queries_preserve_output_windows_and_legacy_tool_answers() {
    let home = fixture();
    let peer = Home::new("testbox");
    peer.top(
        "other",
        &[human("other", ts(6, 0), "Review the lexer change")],
    );
    for seconds in [86_400, 21_600] {
        for multiple in [false, true] {
            let options = |home: &Home| Options {
                all: false,
                since: Duration::from_secs(seconds),
                scan_window: false,
                ..home.options.clone()
            };
            let mut machines = vec![("a".into(), options(&home))];
            if multiple {
                machines.push(("b".into(), options(&peer)));
            }
            let mut viewer = ViewerCore::with_machines(machines);
            viewer.set_refresh(crate::Refresh::OnInvalidate);
            let core = Arc::new(viewer);
            let calls = [
                ("list_sessions", json!({})),
                ("list_sessions", json!({"parent":"lead", "limit":1})),
                ("list_sessions", json!({"since":"1970-01-01T00:00:00Z"})),
                ("get_session", json!({"id":"lead"})),
                ("get_session", json!({"id":"scan"})),
                ("get_session", json!({"id":"asker"})),
                ("get_session", json!({"id":"cx"})),
                ("get_session", json!({"id":"absent"})),
                ("read_transcript", json!({"id":"done", "limit":1})),
                (
                    "read_transcript",
                    json!({"id":"scan", "after":0, "limit":2}),
                ),
                ("read_transcript", json!({"id":"done", "turn":"absent"})),
                ("find", json!({"text":"lexer", "limit":2})),
                ("find", json!({"text":"cargo", "max_bytes":1})),
                ("stalls", json!({"idle_minutes":1})),
            ];
            let mut native = Query::from_native_core(core.clone());
            let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
            let actual: Vec<_> = calls
                .iter()
                .map(|(tool, args)| native.call_at(tool, args, NOW))
                .collect();
            assert_eq!(
                crate::model::COMPAT_BUILDS.with(|count| count.get()),
                before
            );
            let snapshot = native.native.as_ref().unwrap().parts[0].data.clone();
            assert_eq!(snapshot.window_start, Some(NOW - seconds as i64 * 1000));
            let mut second = Query::from_native_core(core.clone());
            second.call_at("list_sessions", &json!({}), NOW).unwrap();
            assert!(Arc::ptr_eq(
                &snapshot,
                &second.native.as_ref().unwrap().parts[0].data
            ));

            // The existing production constructor remains the independent
            // compatibility reference until Hub switches its consumer.
            core.model_at(NOW, Reading::At(NOW)).unwrap().unwrap();
            let mut legacy = Query::from_core(core.clone());
            for ((tool, args), answer) in calls.iter().zip(actual) {
                assert_eq!(
                    answer,
                    legacy.call_at(tool, args, NOW),
                    "{tool} {args}, seconds={seconds}, multiple={multiple}"
                );
            }
            core.close();
        }
    }
}

#[test]
fn embedded_native_query_retirement_serves_only_retained_snapshots() {
    let home = fixture();
    let core = Arc::new(ViewerCore::new(home.options.clone()));
    let mut native = Query::from_native_core(core.clone());
    let before = native.call_at("list_sessions", &json!({}), NOW).unwrap();
    core.close();
    home.top(
        "after-close",
        &[human("after-close", ts(6, 0), "Closed source")],
    );
    assert_eq!(
        native.call_at("list_sessions", &json!({}), NOW).unwrap(),
        before
    );
    let unopened = Arc::new(ViewerCore::new(home.options.clone()));
    unopened.close();
    assert_eq!(
        Query::from_native_core(unopened)
            .call_at("list_sessions", &json!({}), NOW)
            .unwrap_err()
            .code,
        "io"
    );
}

#[test]
fn local_tools_match_legacy_evidence_without_compatibility_preparation() {
    let home = fixture();
    // Same hostname exercises presentation disambiguation independently of the
    // authoritative source keys. The second source has its own native evidence.
    let peer = Home::new("testbox");
    peer.top(
        "other",
        &[
            human("other", ts(6, 0), "Review the lexer change"),
            assistant("other", ts(6, 1), vec![text("Looks right.")]),
        ],
    );
    for multiple in [false, true] {
        let mut machines = vec![("a".to_owned(), home.options.clone())];
        if multiple {
            machines.push(("b".to_owned(), peer.options.clone()));
        }
        let mut native = Query::with_machines(machines.clone());
        for revision in 0..3 {
            if revision == 1 {
                home.top(
                    "done",
                    &[
                        human("done", ts(4, 0), "Summarize the design doc"),
                        assistant("done", ts(4, 1), vec![text("Updated lexer evidence.")]),
                        assistant("done", ts(7, 0), vec![text("A later response.")]),
                    ],
                );
            }
            if revision == 2 {
                let path = home.root.join("proc/30/stat");
                if path.exists() {
                    fs::remove_file(path).unwrap();
                }
            }
            let calls = [
                ("list_sessions", json!({})),
                ("list_sessions", json!({"parent":"lead", "limit":1})),
                ("get_session", json!({"id":"lead"})),
                ("get_session", json!({"id":"scan"})),
                ("get_session", json!({"id":"asker"})),
                ("get_session", json!({"id":"cx"})),
                ("read_transcript", json!({"id":"done", "limit":1})),
                (
                    "read_transcript",
                    json!({"id":"scan", "after":0, "limit":2}),
                ),
                ("read_transcript", json!({"id":"done", "turn":"absent"})),
                ("get_session", json!({"id":"absent"})),
                ("find", json!({"text":"lexer", "limit":2})),
                ("find", json!({"text":"cargo", "max_bytes":1})),
                ("stalls", json!({"idle_minutes":1})),
            ];
            let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
            let actual: Vec<_> = calls
                .iter()
                .map(|(tool, args)| native.call_at(tool, args, NOW))
                .collect();
            assert_eq!(
                crate::model::COMPAT_BUILDS.with(|count| count.get()),
                before
            );
            let snapshot = native.native.as_ref().unwrap().parts[0].data.clone();
            native.call_at("list_sessions", &json!({}), NOW).unwrap();
            assert!(Arc::ptr_eq(
                &snapshot,
                &native.native.as_ref().unwrap().parts[0].data
            ));

            // Use the old production consumer as the independent transport
            // reference, with the same file-selection window and clock.
            let machines = machines
                .iter()
                .map(|(key, options)| {
                    (
                        key.clone(),
                        Options {
                            scan_window: true,
                            session: None,
                            ..options.clone()
                        },
                    )
                })
                .collect();
            let core = Arc::new(ViewerCore::with_machines(machines));
            core.model_at(NOW, Reading::At(NOW)).unwrap().unwrap();
            core.close();
            let mut legacy = Query::from_core(core);
            for ((tool, args), actual) in calls.iter().zip(actual) {
                assert_eq!(
                    actual,
                    legacy.call_at(tool, args, NOW),
                    "{tool} {args}, multiple={multiple}, revision={revision}"
                );
            }
        }
    }
}

#[test]
fn local_query_conflicts_and_source_removal_use_native_snapshots() {
    let first = Home::new("same");
    let second = Home::new("same");
    for home in [&first, &second] {
        home.top(
            "duplicate",
            &[
                human("duplicate", ts(1, 0), "Same native session ID"),
                assistant("duplicate", ts(1, 1), vec![text("Evidence")]),
            ],
        );
    }
    let mut native = Query::with_machines(vec![
        ("a".into(), first.options.clone()),
        ("b".into(), second.options.clone()),
    ]);
    let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
    for tool in ["list_sessions", "get_session", "read_transcript"] {
        let args = if tool == "list_sessions" {
            json!({})
        } else {
            json!({"id":"duplicate"})
        };
        assert_eq!(
            native.call_at(tool, &args, NOW).unwrap_err().code,
            "id_conflict"
        );
    }
    fs::remove_file(
        second
            .root
            .join("claude/projects/-work-proj/duplicate.jsonl"),
    )
    .unwrap();
    assert_eq!(
        native.call_at("list_sessions", &json!({}), NOW).unwrap()["total"],
        1
    );
    assert!(
        native
            .call_at("read_transcript", &json!({"id":"duplicate"}), NOW)
            .is_ok()
    );
    assert_eq!(
        crate::model::COMPAT_BUILDS.with(|count| count.get()),
        before
    );
}
