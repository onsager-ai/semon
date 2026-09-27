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
            codex_home: root.join("codex"),
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
        "open_question": null, "tokens": tokens, "run": null,
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
    let mut core = ViewerCore::new(home.options.clone());
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

/// Whether the event cache holds a file's index: whether a build read it.
fn indexed(home: &Home, file: &str) -> bool {
    let cache = crate::events::EventCache::path(&home.options.cache);
    fs::read_to_string(cache).is_ok_and(|cache| cache.contains(file))
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
