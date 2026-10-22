use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use semon_sessions::{Options, Query, ViewerCore, collect, render_json, render_text};
use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn native_codex_fork_tree_reports_child_request_usage_without_inherited_counters() {
    let fixture = Fixture::new();
    fixture.write(
        "codex/sessions/parent.jsonl",
        include_str!(
            "../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/fork/initial-rollout.jsonl"
        ),
    );
    fixture.write("codex/sessions/child.jsonl", include_str!("../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/fork/child-turn-rollout.jsonl"));
    let nodes = collect(&fixture.options).unwrap();
    let child = nodes
        .iter()
        .find(|node| node.id == "native-codex-fork-child")
        .unwrap();
    assert_eq!(child.tokens.input, 5);
    assert_eq!(child.tokens.output, 3);
    assert_eq!(child.tokens.total, 8);
    // A pre-migration tree cache has no authority over native evidence. Keep
    // the historical file byte-for-byte, including its older usage semantics.
    let old = br#"{"version":3,"files":{"child":{"summary":{"codex_tokens":{"input":999,"output":999}}}}}"#;
    fs::write(&fixture.options.cache, old).unwrap();
    let reread = collect(&fixture.options).unwrap();
    assert_eq!(
        reread
            .iter()
            .find(|node| node.id == child.id)
            .unwrap()
            .tokens,
        child.tokens
    );
    assert_eq!(fs::read(&fixture.options.cache).unwrap(), old);
}

struct Fixture {
    root: PathBuf,
    options: Options,
}

impl Fixture {
    fn tree_metadata(&self) -> String {
        let connection =
            rusqlite::Connection::open(self.options.cache.with_extension("sqlite3")).unwrap();
        let mut statement = connection
            .prepare("SELECT value FROM meta WHERE key GLOB 'native_tree_summary:*' ORDER BY key")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn new() -> Self {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "semon-sessions-test-{}-{suffix}",
            std::process::id()
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
        fs::create_dir_all(&options.proc_root).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        Self { root, options }
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn jsonl(&self, relative: &str, records: &[Value]) -> PathBuf {
        let text = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        self.write(relative, &text)
    }

    fn process(&self, pid: u32, start: u64) {
        let mut fields = vec!["0"; 20];
        fields[19] = "123";
        let stat = format!("{pid} (agent (worker)) {}\n", fields.join(" "));
        let path = self.options.proc_root.join(pid.to_string()).join("stat");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, stat.replace("123", &start.to_string())).unwrap();
    }

    fn live_claude(&self, pid: u32, id: &str, start: u64, status: &str) {
        self.write(
            &format!("claude/sessions/{pid}.json"),
            &json!({
                "pid": pid, "sessionId": id, "procStart": start, "status": status,
                "name": "work", "cwd": "/tmp/project", "bridgeSessionId": "bridge-1",
                "statusUpdatedAt": 1_800_000_000_000_u64,
            })
            .to_string(),
        );
    }

    fn claude(&self, id: &str, records: &[Value]) -> PathBuf {
        self.jsonl(&format!("claude/projects/project/{id}.jsonl"), records)
    }

    fn codex(&self, id: &str, parent: Option<&str>, records: &[Value]) -> PathBuf {
        let mut lines = vec![
            json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{
                "id":id,"session_id":parent.unwrap_or(id),"parent_thread_id":parent,
                "thread_source":if parent.is_some() {"subagent"} else {"user"},
                "agent_nickname":if parent.is_some() {Some(id)} else {None},
                "agent_path":if parent.is_some() {Some(format!("root/{id}"))} else {None},
                "cwd":"/tmp/project","git":{"branch":"feature"}
            }}),
        ];
        lines.extend_from_slice(records);
        self.jsonl(
            &format!("codex/sessions/2026/09/24/rollout-{id}.jsonl"),
            &lines,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn claude_line(id: &str, blocks: Vec<Value>, output: u64) -> Value {
    json!({"timestamp":"2026-09-24T00:00:00Z","sessionId":id,"cwd":"/tmp/project",
        "gitBranch":"feature","type":"assistant","message":{"id":"message-1","model":"claude-test",
        "usage":{"input_tokens":10,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":output},
        "content":blocks}})
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type":"tool_use","id":id,"name":name,"input":input})
}

fn result(id: &str) -> Value {
    json!({"type":"tool_result","tool_use_id":id,"content":"result"})
}

fn codex_line(kind: &str, payload: Value) -> Value {
    json!({"timestamp":"2026-09-24T00:01:00Z","type":kind,"payload":payload})
}

#[test]
fn claude_liveness_dedup_tools_and_secret_boundary() {
    let fixture = Fixture::new();
    fixture.process(100, 123);
    fixture.live_claude(100, "alive", 123, "busy");
    fixture.live_claude(101, "dead", 123, "busy");
    fixture.process(102, 999);
    fixture.live_claude(102, "reused", 123, "busy");
    fixture.write("claude/sessions/103.json", "\0\0\0");
    let unreadable_pid = fixture.write("claude/sessions/104.json", "{}");
    #[cfg(unix)]
    fs::set_permissions(unreadable_pid, fs::Permissions::from_mode(0o000)).unwrap();
    let key = fixture.write("claude/sessions/secret.key", "must never read");
    #[cfg(unix)]
    fs::set_permissions(key, fs::Permissions::from_mode(0o000)).unwrap();
    fixture.claude(
        "alive",
        &[
            claude_line(
                "alive",
                vec![
                    tool("call-1", "Bash", json!({"command":"SECRET_INPUT_42"})),
                    json!({"type":"text","text":"SECRET_TEXT_42"}),
                ],
                1,
            ),
            claude_line(
                "alive",
                vec![
                    tool("call-2", "Skill", json!({"skill":"test"})),
                    result("call-2"),
                ],
                5,
            ),
        ],
    );
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes.len(), 1);
    let node = &nodes[0];
    assert_eq!(node.state, "busy");
    assert_eq!(node.tokens.input, 15);
    assert_eq!(node.tokens.output, 5);
    assert_eq!(node.open_tools.len(), 1);
    assert_eq!(node.open_tools[0].id, "call-1");
    assert_eq!(
        node.claude_link.as_deref(),
        Some("https://claude.ai/code/bridge-1")
    );
    for output in [
        render_text(&nodes),
        render_json(&nodes),
        fixture.tree_metadata(),
    ] {
        assert!(!output.contains("SECRET_TEXT_42"));
        assert!(!output.contains("SECRET_INPUT_42"));
    }
}

#[test]
fn subagent_state_follows_parent_tool_result() {
    let fixture = Fixture::new();
    fixture.process(100, 123);
    fixture.live_claude(100, "parent", 123, "idle");
    let parent = fixture.claude(
        "parent",
        &[claude_line(
            "parent",
            vec![tool("task-1", "Task", json!({}))],
            1,
        )],
    );
    let subagent = fixture.jsonl(
        "claude/projects/project/parent/subagents/agent-one.jsonl",
        &[claude_line("one", vec![], 1)],
    );
    let mut subagent_file = fs::OpenOptions::new().append(true).open(subagent).unwrap();
    subagent_file.write_all(&[0; 32]).unwrap();
    subagent_file.write_all(b"\n").unwrap();
    fixture.write(
        "claude/projects/project/parent/subagents/agent-one.meta.json",
        &json!({"description":"inspect","toolUseId":"task-1"}).to_string(),
    );
    let first = collect(&fixture.options).unwrap();
    assert_eq!(first[0].children[0].state, "running");
    assert_eq!(first[0].children[0].malformed_lines, 1);
    let mut file = fs::OpenOptions::new().append(true).open(parent).unwrap();
    writeln!(file, "{}", claude_line("parent", vec![result("task-1")], 1)).unwrap();
    let second = collect(&fixture.options).unwrap();
    assert_eq!(second[0].children[0].state, "done");
}

#[test]
fn live_shell_status_is_preserved_and_counts_as_an_alive_parent() {
    let mut fixture = Fixture::new();
    fixture.process(100, 123);
    fixture.live_claude(100, "parent", 123, "shell");
    let mut old_line = claude_line("parent", vec![tool("task-1", "Task", json!({}))], 1);
    old_line["timestamp"] = json!("2000-01-01T00:00:00Z");
    fixture.claude("parent", &[old_line]);
    fixture.jsonl(
        "claude/projects/project/parent/subagents/agent-one.jsonl",
        &[claude_line("one", vec![], 1)],
    );
    fixture.write(
        "claude/projects/project/parent/subagents/agent-one.meta.json",
        &json!({"description":"inspect","toolUseId":"task-1"}).to_string(),
    );
    fixture.options.all = false;
    fixture.options.since = Duration::from_secs(1);
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes[0].state, "shell");
    assert_eq!(nodes[0].children[0].state, "running");
}

#[cfg(unix)]
#[test]
fn unreadable_sources_leave_unknown_nodes_and_do_not_abort() {
    let fixture = Fixture::new();
    fixture.process(100, 123);
    fixture.live_claude(100, "claude-unreadable", 123, "busy");
    let claude = fixture.claude(
        "claude-unreadable",
        &[claude_line("claude-unreadable", vec![], 1)],
    );
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o000)).unwrap();
    let codex = fixture.codex("codex-unreadable", None, &[]);
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o000)).unwrap();
    fixture.claude("good", &[claude_line("good", vec![], 1)]);
    let nodes = collect(&fixture.options).unwrap();
    let claude_node = nodes
        .iter()
        .find(|node| node.id == "claude-unreadable")
        .unwrap();
    assert_eq!(claude_node.state, "unknown");
    assert_eq!(claude_node.label.as_deref(), Some("work"));
    assert_eq!(claude_node.pid, Some(100));
    let codex_node = nodes
        .iter()
        .find(|node| node.id == "codex-unreadable")
        .unwrap();
    assert_eq!(codex_node.state, "unknown");
    assert!(nodes.iter().any(|node| node.id == "good"));
}

#[test]
fn claude_skips_and_counts_nul_filled_complete_lines() {
    let fixture = Fixture::new();
    let first = claude_line("damaged", vec![tool("tool-1", "Bash", json!({}))], 1);
    let last = json!({"timestamp":"2026-09-24T00:02:00Z","message":{
        "id":"message-2","model":"claude-test",
        "usage":{"input_tokens":2,"output_tokens":3},
        "content":[result("tool-1")]
    }});
    fixture.claude("clean", &[first.clone(), last.clone()]);
    fixture.write(
        "claude/projects/project/damaged.jsonl",
        &format!("{first}\n{}\n{last}\n", "\0".repeat(128)),
    );
    let nodes = collect(&fixture.options).unwrap();
    let clean = nodes.iter().find(|node| node.id == "clean").unwrap();
    let damaged = nodes.iter().find(|node| node.id == "damaged").unwrap();
    assert_eq!(damaged.tokens, clean.tokens);
    assert_eq!(damaged.models, clean.models);
    assert_eq!(damaged.open_tools, clean.open_tools);
    assert_eq!(damaged.last_activity, clean.last_activity);
    assert_eq!(clean.malformed_lines, 0);
    assert_eq!(damaged.malformed_lines, 1);
    assert!(render_text(&nodes).contains("malformed:1"));
    assert_eq!(
        serde_json::from_str::<Value>(&render_json(&nodes)).unwrap()["roots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "damaged")
            .unwrap()["malformed_lines"],
        1
    );
    assert_eq!(
        collect(&fixture.options)
            .unwrap()
            .iter()
            .find(|node| node.id == "damaged")
            .unwrap()
            .malformed_lines,
        1
    );
    assert!(fixture.tree_metadata().contains("\"malformed_lines\":1"));
}

#[test]
fn valid_json_non_object_line_is_counted_as_malformed() {
    let fixture = Fixture::new();
    fixture.write(
        "claude/projects/project/non-object.jsonl",
        &format!("42\n{}\n", claude_line("non-object", vec![], 1)),
    );
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes[0].malformed_lines, 1);
    assert_eq!(nodes[0].tokens.output, 1);
}

#[cfg(unix)]
fn set_lock(fixture: &Fixture, id: &str, held: bool) {
    let path = fixture.write(&format!("codex/thread-writer-locks/{id}.lock"), "");
    if held {
        let meta = fs::metadata(path).unwrap();
        let dev = meta.dev();
        let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
        let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
        fixture.write(
            "proc/locks",
            &format!(
                "12: FLOCK ADVISORY WRITE 4242 {major:x}:{minor:x}:{} 0 EOF\n",
                meta.ino()
            ),
        );
    }
}

#[test]
fn codex_subagents_use_thread_ids_and_lock_states() {
    let fixture = Fixture::new();
    fixture.codex("parent", None, &[codex_line("event_msg", json!({"type":"task_started","turn_id":"parent-turn"})),
        codex_line("turn_context", json!({"model":"gpt-test"})),
        codex_line("response_item", json!({"type":"function_call","name":"shell","call_id":"open-call","arguments":"{}"})),
        codex_line("event_msg", json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}})),
        codex_line("event_msg", json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":9,"output_tokens":4,"total_tokens":13}}}))]);
    fixture.codex("child-a", Some("parent"), &[]);
    fixture.codex(
        "child-b",
        Some("parent"),
        &[
            codex_line(
                "event_msg",
                json!({"type":"task_started","turn_id":"child-turn"}),
            ),
            codex_line(
                "event_msg",
                json!({"type":"task_complete","turn_id":"child-turn"}),
            ),
        ],
    );
    #[cfg(unix)]
    set_lock(&fixture, "parent", true);
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].children.len(), 2);
    let child_a = nodes[0]
        .children
        .iter()
        .find(|child| child.id == "child-a")
        .unwrap();
    let child_b = nodes[0]
        .children
        .iter()
        .find(|child| child.id == "child-b")
        .unwrap();
    assert_eq!(child_a.id, "child-a");
    assert_eq!(child_b.id, "child-b");
    assert!(child_b.last_activity > child_a.last_activity);
    assert_eq!(
        nodes[0].children[0].id, "child-b",
        "newer activity sorts first"
    );
    // An empty rollout has no terminal evidence; the completed control does.
    assert_eq!(child_a.state, "unknown");
    assert!(child_a.turn_lifecycle.is_none());
    assert_eq!(child_b.state, "ended");
    assert_eq!(
        child_b.turn_lifecycle.as_ref().unwrap().phase,
        semon_sessions::TurnPhase::Completed
    );
    assert_eq!(nodes[0].tokens.input, 9);
    assert!(nodes[0].open_tools.is_empty());
    #[cfg(unix)]
    {
        assert_eq!(nodes[0].state, "running");
        assert_eq!(nodes[0].pid, Some(4242));
        fs::remove_file(fixture.options.proc_root.join("locks")).unwrap();
        assert_eq!(collect(&fixture.options).unwrap()[0].state, "unknown");
    }
}

#[test]
fn codex_skips_and_counts_nul_filled_complete_lines() {
    let fixture = Fixture::new();
    let model = codex_line("turn_context", json!({"model":"gpt-test"}));
    let usage = codex_line(
        "event_msg",
        json!({"type":"token_count","info":{"total_token_usage":{
            "input_tokens":9,"output_tokens":4,"total_tokens":13
        }}}),
    );
    fixture.codex("clean", None, &[model.clone(), usage.clone()]);
    let path = fixture.codex("damaged", None, &[model, usage]);
    let bytes = fs::read(&path).unwrap();
    let second_newline = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(1)
        .unwrap()
        .0
        + 1;
    let mut damaged_bytes = bytes[..second_newline].to_vec();
    damaged_bytes.extend_from_slice(&[0; 128]);
    damaged_bytes.push(b'\n');
    damaged_bytes.extend_from_slice(&bytes[second_newline..]);
    fs::write(path, damaged_bytes).unwrap();
    let nodes = collect(&fixture.options).unwrap();
    let clean = nodes.iter().find(|node| node.id == "clean").unwrap();
    let damaged = nodes.iter().find(|node| node.id == "damaged").unwrap();
    assert_eq!(damaged.tokens, clean.tokens);
    assert_eq!(damaged.models, clean.models);
    assert_eq!(damaged.last_activity, clean.last_activity);
    assert_eq!(damaged.malformed_lines, 1);
    assert_eq!(
        collect(&fixture.options)
            .unwrap()
            .iter()
            .find(|node| node.id == "damaged")
            .unwrap()
            .malformed_lines,
        1
    );
}

#[test]
fn codex_with_damaged_first_line_has_unknown_metadata() {
    let fixture = Fixture::new();
    let id = "12345678-1234-1234-1234-123456789abc";
    fixture.write(
        &format!("codex/sessions/2026/09/24/rollout-2026-09-24T00-00-00-{id}.jsonl"),
        &format!(
            "{}\n{}\n",
            "\0".repeat(64),
            codex_line("turn_context", json!({"model":"gpt-test"}))
        ),
    );
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id, id);
    assert_eq!(nodes[0].label, None);
    assert_eq!(nodes[0].cwd, None);
    assert_eq!(nodes[0].malformed_lines, 1);
    assert_eq!(nodes[0].models, ["gpt-test"]);
}

#[cfg(unix)]
#[test]
fn held_old_codex_lock_still_shows_a_live_root() {
    let mut fixture = Fixture::new();
    let path = fixture.codex("old-live", None, &[]);
    fs::File::open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
        .unwrap();
    set_lock(&fixture, "old-live", true);
    fixture.options.all = false;
    fixture.options.since = Duration::from_secs(1);
    let roots = collect(&fixture.options).unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, "old-live");
    assert_eq!(roots[0].state, "running");
}

#[test]
fn links_new_and_old_markers_and_leaves_unmarked_unlinked() {
    let fixture = Fixture::new();
    let prompt = "/tmp/semon-prompt-unique.md";
    fixture.claude(
        "claude-parent",
        &[claude_line(
            "claude-parent",
            vec![
                tool(
                    "bash-id",
                    "Bash",
                    json!({"command":format!("codex exec < {prompt}")}),
                ),
                tool("skill-id", "Skill", json!({"args":"run"})),
            ],
            1,
        )],
    );
    fixture.codex("new", None, &[
        codex_line("response_item", json!({"role":"user","content":[{"type":"input_text","text":"<recommended_plugins>injected</recommended_plugins>"}]})),
        codex_line("response_item", json!({"role":"user","content":[{"type":"input_text","text":format!("Semon-Parent: claude:claude-parent\nSemon-Handoff: {prompt}\nbody") }]})),
    ]);
    fixture.codex("old", None, &[
        codex_line("event_msg", json!({"type":"user_message","message":"harness injection"})),
        codex_line("event_msg", json!({"type":"user_message","message":"Semon-Parent: claude:claude-parent:skill-id\nbody"})),
    ]);
    fixture.codex(
        "alone",
        None,
        &[codex_line(
            "event_msg",
            json!({"type":"user_message","message":"body"}),
        )],
    );
    fixture.codex("late", None, &[
        codex_line("response_item", json!({"role":"user","content":[{"type":"input_text","text":"harness injection"}]})),
        codex_line("response_item", json!({"role":"assistant","content":[{"type":"output_text","text":"started"}]})),
        codex_line("event_msg", json!({"type":"user_message","message":"Semon-Parent: claude:claude-parent:skill-id"})),
    ]);
    let nodes = collect(&fixture.options).unwrap();
    let parent = nodes
        .iter()
        .find(|node| node.id == "claude-parent")
        .unwrap();
    assert_eq!(parent.children.len(), 2);
    assert_eq!(
        parent
            .children
            .iter()
            .find(|node| node.id == "new")
            .unwrap()
            .via_tool
            .as_ref()
            .unwrap()
            .id,
        "bash-id"
    );
    assert_eq!(
        parent
            .children
            .iter()
            .find(|node| node.id == "old")
            .unwrap()
            .via_tool
            .as_ref()
            .unwrap()
            .id,
        "skill-id"
    );
    assert!(
        nodes
            .iter()
            .find(|node| node.id == "alone")
            .unwrap()
            .unlinked
    );
    assert!(
        nodes
            .iter()
            .find(|node| node.id == "late")
            .unwrap()
            .unlinked
    );
    let cache = fixture.tree_metadata();
    assert!(!cache.contains(prompt));
    assert_eq!(
        collect(&fixture.options)
            .unwrap()
            .iter()
            .find(|node| node.id == "claude-parent")
            .unwrap()
            .children
            .len(),
        2
    );
}

#[test]
fn variable_built_handoffs_choose_the_launch_call_and_share_calls() {
    let fixture = Fixture::new();
    let command = r#"S=/tmp/x/scratchpad; P=$S/codex-foo-prompt.md; { printf 'Semon-Parent: claude:%s\nSemon-Handoff: %s\n' "$CLAUDE_CODE_SESSION_ID" "$P"; cat "$P"; } | codex exec --json -C /home/u/wt/semon-wt-foo"#;
    let launch = |parent: &str, id: &str, time: &str, command: &str| {
        let mut record = claude_line(
            parent,
            vec![tool(
                id,
                "Bash",
                json!({"command":command,"run_in_background":true}),
            )],
            1,
        );
        record["timestamp"] = json!(time);
        record
    };
    fixture.claude(
        "parent",
        &[
            launch("parent", "first", "2026-09-24T00:00:00Z", command),
            launch("parent", "second", "2026-09-24T00:10:00Z", command),
            launch(
                "parent",
                "multi",
                "2026-09-24T00:20:00Z",
                "codex exec -C /home/u/wt/semon-wt-bar; codex exec -C /home/u/wt/semon-wt-baz",
            ),
            launch(
                "parent",
                "basename",
                "2026-09-24T00:30:00Z",
                "codex exec -C semon-wt-short",
            ),
        ],
    );
    fixture.claude(
        "prefix",
        &[launch(
            "prefix",
            "foobar",
            "2026-09-24T00:00:00Z",
            "codex exec -C /home/u/wt/semon-wt-foobar",
        )],
    );
    let cases = [
        (
            "early",
            "parent",
            "semon-wt-foo",
            "2026-09-24T00:00:08Z",
            Some("first"),
        ),
        (
            "later",
            "parent",
            "semon-wt-foo",
            "2026-09-24T00:10:08Z",
            Some("second"),
        ),
        (
            "bar",
            "parent",
            "semon-wt-bar",
            "2026-09-24T00:20:08Z",
            Some("multi"),
        ),
        (
            "baz",
            "parent",
            "semon-wt-baz",
            "2026-09-24T00:20:09Z",
            Some("multi"),
        ),
        (
            "short",
            "parent",
            "semon-wt-short",
            "2026-09-24T00:30:08Z",
            Some("basename"),
        ),
        (
            "missing",
            "parent",
            "semon-wt-absent",
            "2026-09-24T00:30:08Z",
            None,
        ),
        (
            "prefix-child",
            "prefix",
            "semon-wt-foo",
            "2026-09-24T00:00:08Z",
            None,
        ),
    ];
    for (id, parent, basename, start, _) in cases {
        let prompt = format!("/tmp/x/scratchpad/codex-{id}-prompt.md");
        assert!(!command.contains(&prompt));
        fixture.jsonl(&format!("codex/sessions/2026/09/24/rollout-{id}.jsonl"), &[
            json!({"type":"session_meta","timestamp":start,"payload":{"id":id,"cwd":format!("/home/u/wt/{basename}")}}),
            json!({"type":"event_msg","timestamp":start,"payload":{"type":"user_message","message":format!("Semon-Parent: claude:{parent}\nSemon-Handoff: {prompt}\nImplement {id}")}}),
        ]);
    }
    for _ in 0..2 {
        let nodes = collect(&fixture.options).unwrap();
        for (id, parent, _, _, expected) in cases {
            let parent = nodes.iter().find(|node| node.id == parent).unwrap();
            let child = parent.children.iter().find(|node| node.id == id).unwrap();
            assert_eq!(
                child.via_tool.as_ref().map(|tool| tool.id.as_str()),
                expected,
                "{id}"
            );
        }
    }
}

#[test]
fn window_session_and_json_schema() {
    let mut fixture = Fixture::new();
    fixture.claude("older", &[claude_line("older", vec![], 1)]);
    fixture.options.all = false;
    fixture.options.since = Duration::from_secs(1);
    assert!(collect(&fixture.options).unwrap().is_empty());
    fixture.options.session = Some("older".into());
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes.len(), 1);
    let parsed: Value = serde_json::from_str(&render_json(&nodes)).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["roots"][0]["id"], "older");
    assert!(parsed["roots"][0]["children"].is_array());
}

#[cfg(unix)]
#[test]
fn cache_resumes_growth_and_rescans_inode_change() {
    let fixture = Fixture::new();
    let path = fixture.claude("cache", &[claude_line("cache", vec![], 1)]);
    let first = collect(&fixture.options).unwrap();
    assert_eq!(first[0].tokens.output, 1);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", json!({"timestamp":"2026-09-24T00:00:01Z","message":{"id":"message-2","model":"claude-test","usage":{"input_tokens":1,"output_tokens":3}}})).unwrap();
    let grown = collect(&fixture.options).unwrap();
    assert_eq!(grown[0].tokens.output, 4);
    let fresh_cache = fixture.root.join("fresh.json");
    let mut fresh_options = fixture.options.clone();
    fresh_options.cache = fresh_cache;
    assert_eq!(collect(&fresh_options).unwrap()[0].tokens.output, 4);
    let replacement = fixture.write(
        "replacement",
        &format!("{}\n", claude_line("cache", vec![], 9)),
    );
    fs::rename(replacement, path).unwrap();
    assert_eq!(collect(&fixture.options).unwrap()[0].tokens.output, 9);
}

#[test]
fn timestamp_parser_handles_offsets_through_age() {
    let fixture = Fixture::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fixture.claude(
        "timestamp",
        &[json!({"timestamp":"2026-09-24T08:00:00+08:00","message":{}})],
    );
    let nodes = collect(&fixture.options).unwrap();
    let expected = now.saturating_sub(1_790_208_000);
    assert!(
        nodes[0]
            .last_activity_age_seconds
            .unwrap()
            .abs_diff(expected)
            <= 1
    );
}

// ---- The session model (#40 M1) --------------------------------------------------------

fn model_home() -> Fixture {
    let fixture = Fixture::new();
    fixture.write("proc/sys/kernel/hostname", "testbox\n");
    fixture
}

fn line(kind: &str, sid: &str, minute: u32, cwd: &Path, extra: Value) -> Value {
    let mut record = json!({"type":kind,"timestamp":format!("2026-09-24T10:{minute:02}:00.000Z"),
        "sessionId":sid,"cwd":cwd,"gitBranch":"main"});
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    record
}

fn said(blocks: Value) -> Value {
    json!({"message":{"id":"msg","model":"claude-opus-5-5","role":"assistant","content":blocks,
        "usage":{"input_tokens":1200,"cache_creation_input_tokens":300,"cache_read_input_tokens":45000,"output_tokens":700}}})
}

/// A home with one of each: your message, a subagent that returns, a relay
/// between two lanes, an answered question, a Codex run, and a reply that is
/// a result to you: the subagent's agent-end notification before it is a
/// status, not an incoming message, so it doesn't break the attribution.
fn write_model_home(fixture: &Fixture, secrets: bool) {
    // A worktree path names its repo without touching the disk, and a fixed
    // path keeps the golden file's byte offsets independent of the temp dir.
    let cwd = Path::new("/work/harbor/.claude/worktrees/m1");
    let secret = |label: &str| {
        if secrets {
            format!(" SECRET_{label}_42")
        } else {
            String::new()
        }
    };
    fixture.jsonl(
        "claude/projects/-work-harbor/harbor.jsonl",
        &[
            line("user", "harbor", 0, cwd, json!({"origin":{"kind":"human"},"message":{"role":"user","content":format!("Fix the sync tests{}", secret("ASK"))}})),
            line("assistant", "harbor", 1, cwd, said(json!([
                {"type":"text","text":format!("Running them{}", secret("TEXT"))},
                {"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":format!("cargo test{}", secret("INPUT"))}},
            ]))),
            line("user", "harbor", 2, cwd, json!({"toolUseResult":{"stdout":"ok"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash-1","content":format!("test result: ok{}", secret("OUTPUT"))}]}})),
            line("assistant", "harbor", 3, cwd, said(json!([
                {"type":"tool_use","id":"agent-1","name":"Agent","input":{"description":"Review","prompt":format!("Review the diff{}", secret("BRIEF"))}},
            ]))),
            line("user", "harbor", 3, cwd, json!({"toolUseResult":{"status":"async_launched"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"agent-1","content":"launched"}]}})),
            line("user", "harbor", 9, cwd, json!({"origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification><tool-use-id>agent-1</tool-use-id><status>completed</status><result>Looks right</result></task-notification>"}})),
            line("assistant", "harbor", 10, cwd, said(json!([
                {"type":"tool_use","id":"send-1","name":"SendMessage","input":{"to":"quill-7a","message":format!("Sync is fixed{}", secret("RELAY")),"type":"message"}},
            ]))),
            line("user", "harbor", 10, cwd, json!({"toolUseResult":{"success":true,"message":"sent","msg_id":"msg-1"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"send-1","content":"sent"}]}})),
            line("assistant", "harbor", 11, cwd, said(json!([
                {"type":"tool_use","id":"ask-1","name":"AskUserQuestion","input":{"questions":[{"question":"Merge now?","header":"Merge","multiSelect":false,"options":[{"label":"Yes","description":"d"},{"label":"No","description":"d"}]}]}},
            ]))),
            line("user", "harbor", 12, cwd, json!({"toolUseResult":{"questions":[],"answers":{"Merge now?":format!("Yes{}", secret("ANSWER"))}},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"ask-1","content":"Your questions have been answered"}]}})),
            line("assistant", "harbor", 13, cwd, said(json!([
                {"type":"tool_use","id":"codex-1","name":"Bash","input":{"command":"codex exec"}},
            ]))),
            line("user", "harbor", 20, cwd, json!({"toolUseResult":{},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"codex-1","content":"done"}]}})),
            line("assistant", "harbor", 21, cwd, said(json!([{"type":"text","text":"All merged."}]))),
        ],
    );
    fixture.jsonl(
        "claude/projects/-work-harbor/harbor/subagents/agent-a1.jsonl",
        &[
            line(
                "user",
                "harbor",
                4,
                cwd,
                json!({"message":{"role":"user","content":"Review the diff"}}),
            ),
            line(
                "assistant",
                "harbor",
                8,
                cwd,
                said(json!([{"type":"text","text":"Looks right"}])),
            ),
        ],
    );
    fixture.write(
        "claude/projects/-work-harbor/harbor/subagents/agent-a1.meta.json",
        &json!({"agentType":"general-purpose","description":"Review the diff","toolUseId":"agent-1"}).to_string(),
    );
    fixture.jsonl(
        "claude/projects/-work-harbor/quill.jsonl",
        &[
            line("custom-title", "quill", 0, cwd, json!({"customTitle":"quill"})),
            line("user", "quill", 10, cwd, json!({"origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/100.sock","name":"harbor-3c","msg_id":"msg-1","body":"Sync is fixed","verifiedPeerPid":100,"verifiedPeerProcStart":"123"},
                "message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/100.sock\" from-name=\"harbor-3c\">Sync is fixed</cross-session-message>"}})),
            line("assistant", "quill", 11, cwd, said(json!([{"type":"text","text":"Thanks, rebasing."}]))),
        ],
    );
    fixture.jsonl(
        "codex/sessions/2026/09/24/rollout-codex-run.jsonl",
        &[
            json!({"timestamp":"2026-09-24T10:14:00.000Z","type":"session_meta","payload":{"id":"codex-run","cwd":cwd,"originator":"codex_exec","git":{"branch":"feat/flush"}}}),
            json!({"timestamp":"2026-09-24T10:14:00.000Z","type":"turn_context","payload":{"model":"gpt-6-luna"}}),
            json!({"timestamp":"2026-09-24T10:14:00.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Semon-Parent: claude:harbor:codex-1\nImplement the flush"}]}}),
            json!({"timestamp":"2026-09-24T10:18:00.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Flush implemented."}]}}),
        ],
    );
    fixture.process(100, 123);
    fixture.write(
        "claude/sessions/100.json",
        &json!({"pid":100,"sessionId":"harbor","procStart":123,"status":"idle","name":"harbor"})
            .to_string(),
    );
}

#[test]
fn native_query_and_catalog_preserve_the_independent_session_reference() {
    let fixture = model_home();
    write_model_home(&fixture, false);
    // Preserve the native semantics from the former golden; the retired
    // model envelope, pricing transport and transcript growth marks are gone.
    let expected: Value =
        serde_json::from_str(include_str!("golden/native-session-evidence.json")).unwrap();
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut query = Query::new(fixture.options.clone());
    let listed = query
        .call_at("list_sessions", &json!({"limit":1000}), now)
        .unwrap();
    assert_eq!(listed["truncated"], false);
    let ids: BTreeSet<_> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        expected["sessions"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    );
    let core = ViewerCore::new(fixture.options.clone());
    let read_catalog = || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let reply = core.respond("GET", "/api/sessions", "machine=local&limit=100", None);
            let value = serde_json::from_slice::<Value>(&reply.body).unwrap();
            if reply.status == 200 && value["completeness"]["state"] == "complete" {
                break value;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native fixture publication: {value}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let catalog = read_catalog();
    assert_eq!(catalog["completeness"]["state"], "complete");
    assert!(catalog["next_cursor"].is_null());
    assert_eq!(catalog["items"].as_array().unwrap().len(), ids.len());
    let mut intervals = Vec::new();
    for id in ids {
        let selected = query
            .call_at("get_session", &json!({"id":id}), now)
            .unwrap();
        let session = &selected["session"];
        let prior = &expected["sessions"][id];
        let row = catalog["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == id)
            .unwrap();
        for field in [
            "name", "harness", "machine", "model", "repo", "branch", "parent", "start", "busy",
            "signals",
        ] {
            assert_eq!(session[field], prior[field], "{id}: {field}");
        }
        assert_eq!(
            session["last_activity"], prior["last"],
            "{id}: last activity"
        );
        let state = match prior["state"].as_str().unwrap() {
            // The frozen codex-run source has no lifecycle: absent writers
            // no longer turn its historical `done` inference into completion.
            "done"
                if id == "codex-run"
                    && session["harness"] == "codex"
                    && session.get("turn_lifecycle").is_none() =>
            {
                "unknown"
            }
            "done" => "ended",
            "idle" => "idle",
            state => panic!("unexpected pinned state: {state}"),
        };
        assert_eq!(session["state"], state, "{id}: state");
        let kind = match prior["kind"].as_str() {
            Some("Subagent") => "subagent",
            Some("Codex run") => "codex-run",
            None => "session",
            kind => panic!("unexpected pinned kind: {kind:?}"),
        };
        assert_eq!(session["kind"], kind, "{id}: native kind");
        assert_eq!(row["kind"], kind);
        assert_eq!(row["presentation_kind"], prior["kind"]);
        assert_eq!(row["parent_source"], prior["parent_source"]);
        assert_eq!(row["tokens"], prior["tokens"], "{id}: displayed usage");
        let models: serde_json::Map<String, Value> = row["cost"]["by_model"].as_object().unwrap().iter()
            .map(|(model, cost)| {
                let tokens=&cost["tokens"];
                (model.clone(), json!({"input":tokens["input"],"output":tokens["output"],
                    "cache_write":tokens["cache_write_5m"].as_u64().unwrap()+tokens["cache_write_1h"].as_u64().unwrap(),
                    "cache_read":tokens["cache_read"]}))
            }).collect();
        assert_eq!(
            Value::Object(models),
            prior["tokens_by_model"],
            "{id}: exact per-model native usage"
        );
        if id == "codex-run"
            && session["harness"] == "codex"
            && session.get("turn_lifecycle").is_none()
        {
            // Inspect the full synthetic-fixture difference in CI before any
            // further contract adaptation. The strict assertions below stay.
            let related = |field: &str| {
                Value::Array(
                    expected[field]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|row| row["sid"] == id || row["from"] == id || row["to"] == id)
                        .cloned()
                        .collect(),
                )
            };
            let parent = query
                .call_at("get_session", &json!({"id":"harbor"}), now)
                .unwrap();
            let parent_edges: Vec<_> = parent["session"]["handoffs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|edge| edge["from"] == id || edge["to"] == id)
                .cloned()
                .collect();
            let diagnostic = json!({"id":id,
                "actual_turns":session["turns"],"expected_turns":related("turns"),
                "actual_handoffs":session["handoffs"],"expected_handoffs":related("handoffs"),
                "actual_parent_handoff_copies":parent_edges})
            .to_string();
            assert!(
                diagnostic.len() <= 16384,
                "synthetic lifecycle diagnostic exceeds bound"
            );
            eprintln!("missing-lifecycle-contract {diagnostic}");
        }
        for field in ["turns", "handoffs"] {
            let rows: Vec<_> = expected[field]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| {
                    if field == "turns" {
                        row["sid"] == id
                    } else {
                        row["from"] == id || row["to"] == id
                    }
                })
                .cloned()
                .collect();
            assert_eq!(session[field], Value::Array(rows), "{id}: {field}");
        }
        intervals.extend(
            session["busy"]
                .as_array()
                .unwrap()
                .iter()
                .map(|range| (range[0].as_i64().unwrap(), range[1].as_i64().unwrap())),
        );
    }
    intervals.sort_unstable();
    let mut busy: Vec<(i64, i64)> = Vec::new();
    // Native machine activity uses the Event Index's existing five-minute
    // clustering rule. Keep its independently captured aggregate interval.
    for (start, end) in intervals {
        if let Some(last) = busy
            .last_mut()
            .filter(|last| start - last.1 <= 5 * 60 * 1000)
        {
            last.1 = last.1.max(end);
        } else {
            busy.push((start, end));
        }
    }
    assert_eq!(json!(busy), expected["busy"]["testbox"]);
    // Warm native publication retains its exact generation and evidence.
    assert_eq!(read_catalog(), catalog);
    core.close();
}

/// Every other content path the model reads: task-notification results,
/// hand-back bodies, `SubagentHandback` input, a Codex brief, result and
/// arguments, question text, option labels, previews and free text, a queued
/// prompt, a received body with no send, and thinking.
fn write_more_secrets(fixture: &Fixture) {
    let cwd = Path::new("/work/harbor/.claude/worktrees/m1");
    let say = |minute, blocks| line("assistant", "tasks", minute, cwd, said(blocks));
    let user = |minute, extra| line("user", "tasks", minute, cwd, extra);
    let asked = |id: &str, question: &str, labels: [&str; 2]| {
        json!([{"type":"tool_use","id":id,"name":"AskUserQuestion","input":{"questions":[{"question":question,"header":"h","multiSelect":false,
            "options":[{"label":labels[0],"description":"d"},{"label":labels[1],"description":"d"}]}]}}])
    };
    fixture.jsonl(
        "claude/projects/-work-harbor/tasks.jsonl",
        &[
            line("attachment", "tasks", 30, cwd, json!({"attachment":{"type":"queued_command","commandMode":"prompt",
                "prompt":"Queue SECRET_QUEUED_42","origin":{"kind":"human"}}})),
            say(31, json!([{"type":"thinking","thinking":"SECRET_THINKING_42"},
                {"type":"tool_use","id":"t-tn","name":"Agent","input":{"prompt":"one"}},
                {"type":"tool_use","id":"t-hb","name":"Agent","input":{"prompt":"two"}},
                {"type":"tool_use","id":"t-sh","name":"Agent","input":{"prompt":"three"}}])),
            user(31, json!({"toolUseResult":{"status":"async_launched"},"message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t-tn","content":"launched"},
                {"type":"tool_result","tool_use_id":"t-hb","content":"launched"},
                {"type":"tool_result","tool_use_id":"t-sh","content":"launched"}]}})),
            user(32, json!({"origin":{"kind":"task-notification"},"message":{"role":"user","content":
                "<task-notification><tool-use-id>t-tn</tool-use-id><status>completed</status><result>SECRET_TN_RESULT_42</result></task-notification>"}})),
            user(33, json!({"origin":{"kind":"peer","handback":true,"from":"lead","senderTaskId":"a-hb","body":"SECRET_HANDBACK_42"},
                "message":{"role":"user","content":"x"}})),
            user(34, json!({"origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/7.sock","name":"gone","msg_id":"m-orphan",
                "body":"SECRET_BODY_42","verifiedPeerPid":7,"verifiedPeerProcStart":"1"},"message":{"role":"user","content":"SECRET_BODY_42"}})),
            say(35, asked("q-label", "Pick SECRET_QUESTION_42?", ["SECRET_LABEL_42", "SECRET_UNCHOSEN_42"])),
            user(35, json!({"toolUseResult":{"answers":{"Pick SECRET_QUESTION_42?":"SECRET_LABEL_42"}},
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"q-label","content":"answered"}]}})),
            say(36, asked("q-free", "Layout?", ["Grid", "List"])),
            user(36, json!({"toolUseResult":{"answers":{"Layout?":"SECRET_FREE_42"},"annotations":{"Layout?":{"preview":"SECRET_PREVIEW_42"}}},
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"q-free","content":"answered"}]}})),
        ],
    );
    for (agent, tool, records) in [
        (
            "a-tn",
            "t-tn",
            vec![line(
                "user",
                "tasks",
                31,
                cwd,
                json!({"message":{"role":"user","content":"one"}}),
            )],
        ),
        (
            "a-hb",
            "t-hb",
            vec![line(
                "user",
                "tasks",
                31,
                cwd,
                json!({"message":{"role":"user","content":"two"}}),
            )],
        ),
        (
            "a-sh",
            "t-sh",
            vec![
                line(
                    "user",
                    "tasks",
                    31,
                    cwd,
                    json!({"message":{"role":"user","content":"three"}}),
                ),
                line(
                    "assistant",
                    "tasks",
                    32,
                    cwd,
                    said(
                        json!([{"type":"tool_use","id":"sh","name":"SubagentHandback",
                "input":{"message":"SECRET_SUBAGENT_HANDBACK_42"}}]),
                    ),
                ),
            ],
        ),
    ] {
        fixture.jsonl(
            &format!("claude/projects/-work-harbor/tasks/subagents/agent-{agent}.jsonl"),
            &records,
        );
        fixture.write(
            &format!("claude/projects/-work-harbor/tasks/subagents/agent-{agent}.meta.json"),
            &json!({"description":agent,"toolUseId":tool}).to_string(),
        );
    }
    fixture.jsonl(
        "codex/sessions/2026/09/24/rollout-codex-secret.jsonl",
        &[
            json!({"timestamp":"2026-09-24T10:40:00.000Z","type":"session_meta","payload":{"id":"codex-secret","cwd":cwd,"originator":"codex_exec"}}),
            json!({"timestamp":"2026-09-24T10:40:00.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Semon-Parent: claude:tasks\nDo SECRET_CODEX_BRIEF_42"}]}}),
            json!({"timestamp":"2026-09-24T10:41:00.000Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"command\":\"echo SECRET_CODEX_ARGS_42\"}"}}),
            json!({"timestamp":"2026-09-24T10:41:00.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"SECRET_CODEX_OUTPUT_42"}}),
            json!({"timestamp":"2026-09-24T10:42:00.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"SECRET_CODEX_RESULT_42"}]}}),
        ],
    );
}

/// Every string in `value` with its path, e.g. `handoffs.3.brief`.
fn strings(value: &Value, path: String, output: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => output.push((path, text.clone())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                strings(item, format!("{path}.{index}"), output);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                strings(item, format!("{path}.{key}"), output);
            }
        }
        _ => {}
    }
}

#[test]
fn planted_secrets_never_reach_the_cache_and_reach_native_query_only_as_their_session() {
    let fixture = model_home();
    write_model_home(&fixture, true);
    write_more_secrets(&fixture);
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut query = Query::new(fixture.options.clone());
    let listed = query
        .call_at("list_sessions", &json!({"limit": 1000}), now)
        .unwrap();
    assert_eq!(listed["truncated"], false, "inspect every fixture session");
    let mut sessions = serde_json::Map::new();
    let mut handoffs = std::collections::BTreeMap::new();
    for row in listed["sessions"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let selected = query
            .call_at("get_session", &json!({"id": id}), now)
            .unwrap();
        for handoff in selected["session"]["handoffs"].as_array().unwrap() {
            handoffs.insert(handoff["id"].as_str().unwrap().to_owned(), handoff.clone());
        }
        sessions.insert(id.to_owned(), selected["session"].clone());
    }
    let evidence =
        json!({"sessions": sessions, "handoffs": handoffs.into_values().collect::<Vec<_>>()});
    let nodes = collect(&fixture.options).unwrap();
    let cache = fixture.tree_metadata();
    // The event index's bytes, and its WAL's if it has one: a secret stored
    // in any row would show here as plain UTF-8.
    let index = fixture.options.cache.with_extension("sqlite3");
    let mut events = String::from_utf8_lossy(&fs::read(&index).unwrap()).into_owned();
    if let Ok(wal) = fs::read(index.with_extension("sqlite3-wal")) {
        events.push_str(&String::from_utf8_lossy(&wal));
    }
    assert!(
        events.contains("harbor"),
        "the event index holds the fixture's files"
    );
    // label -> a session the handoff carrying it must touch; None: never shown.
    let owners: [(&str, Option<&str>); 22] = [
        ("ASK", Some("harbor")),
        ("BRIEF", Some("harbor")),
        ("RELAY", Some("harbor")),
        ("ANSWER", Some("harbor")),
        ("TEXT", None),
        ("INPUT", None),
        ("OUTPUT", None),
        ("QUEUED", Some("tasks")),
        ("THINKING", None),
        ("TN_RESULT", Some("a-tn")),
        ("HANDBACK", Some("a-hb")),
        ("SUBAGENT_HANDBACK", Some("a-sh")),
        ("BODY", Some("tasks")),
        ("QUESTION", Some("tasks")),
        ("LABEL", Some("tasks")),
        ("UNCHOSEN", None),
        ("FREE", Some("tasks")),
        ("PREVIEW", Some("tasks")),
        ("CODEX_BRIEF", Some("codex-secret")),
        ("CODEX_RESULT", Some("codex-secret")),
        ("CODEX_ARGS", None),
        ("CODEX_OUTPUT", None),
    ];
    for (label, _) in owners {
        let secret = format!("SECRET_{label}_42");
        for (name, output) in [
            ("native tree metadata", &cache),
            ("event cache", &events),
            ("tree text", &render_text(&nodes)),
            ("tree json", &render_json(&nodes)),
        ] {
            assert!(!output.contains(&secret), "{secret} in the {name}");
        }
    }
    // Native Query exposes selected handoff content and its owning session's
    // open question. Neither metadata caches nor unrelated identity/fact fields
    // may retain that content.
    let content = |path: &str| {
        let parts: Vec<&str> = path.split('.').collect();
        matches!(
            parts.as_slice(),
            ["", "handoffs", _, "brief" | "result" | "target"]
                | ["", "handoffs", _, "answer", _]
                | ["", "handoffs", _, "answers", _, "question" | "preview"]
                | ["", "handoffs", _, "answers", _, "values", _]
                | ["", "sessions", _, "open_question", "text"]
                | [
                    "",
                    "sessions",
                    _,
                    "handoffs",
                    _,
                    "brief" | "result" | "target"
                ]
                | ["", "sessions", _, "handoffs", _, "answer", _]
                | [
                    "",
                    "sessions",
                    _,
                    "handoffs",
                    _,
                    "answers",
                    _,
                    "question" | "preview"
                ]
                | ["", "sessions", _, "handoffs", _, "answers", _, "values", _]
        )
    };
    let mut found = Vec::new();
    strings(&evidence, String::new(), &mut found);
    let mut seen = BTreeSet::new();
    for (path, text) in found {
        for (label, owner) in owners {
            if !text.contains(&format!("SECRET_{label}_42")) {
                continue;
            }
            assert!(content(&path), "{label} at {path}");
            let owner =
                owner.unwrap_or_else(|| panic!("{label} must not reach native Query: {path}"));
            let parts: Vec<_> = path.split('.').collect();
            if parts[1] == "sessions" && parts[3] == "open_question" {
                assert_eq!(parts[2], owner, "{label} left {owner}: {path}");
            } else {
                let handoff = if parts[1] == "sessions" {
                    &evidence["sessions"][parts[2]]["handoffs"][parts[4].parse::<usize>().unwrap()]
                } else {
                    &evidence["handoffs"][parts[2].parse::<usize>().unwrap()]
                };
                assert!(
                    handoff["from"] == owner || handoff["to"] == owner,
                    "{label} left {owner}: {handoff}"
                );
            }
            seen.insert(label);
        }
    }
    let shown: BTreeSet<&str> = owners
        .iter()
        .filter(|(_, owner)| owner.is_some())
        .map(|(label, _)| *label)
        .collect();
    assert_eq!(seen, shown);
}

#[test]
fn native_claude_copied_usage_tree_preserves_observed_and_unknown_owner() {
    let fixture = Fixture::new();
    fixture.write("claude/projects/fixture/native-claude-parent.jsonl", include_str!("../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/resumed-transcript.jsonl"));
    fixture.write(
        "claude/projects/fixture/native-claude-child.jsonl",
        include_str!(
            "../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/forked-transcript.jsonl"
        ),
    );
    let mut options = fixture.options.clone();
    options.all = true;
    for _ in 0..2 {
        let nodes = collect(&options).unwrap();
        let parent = nodes
            .iter()
            .find(|n| n.id == "native-claude-parent")
            .unwrap();
        let child = nodes
            .iter()
            .find(|n| n.id == "native-claude-child")
            .unwrap();
        assert_eq!(parent.tokens.input, 0);
        assert_eq!(child.tokens.input, 5);
        let p = parent.claude_usage.as_ref().unwrap();
        let c = child.claude_usage.as_ref().unwrap();
        assert_eq!(p.observed.input + c.observed.input, 25);
        assert_eq!(p.shared, c.shared);
        assert!(p.fresh.is_none() && c.fresh.is_none());
        assert!(p.shared_owner.is_none() && c.shared_owner.is_none());
    }
}
