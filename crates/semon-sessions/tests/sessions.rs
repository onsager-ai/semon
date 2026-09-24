use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use semon_sessions::{Options, collect, render_json, render_text};
use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    options: Options,
}

impl Fixture {
    fn new() -> Self {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "semon-sessions-test-{}-{suffix}",
            std::process::id()
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
        fs::read_to_string(&fixture.options.cache).unwrap(),
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
    assert!(
        fs::read_to_string(&fixture.options.cache)
            .unwrap()
            .contains("\"malformed_lines\":1")
    );
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
    fixture.codex("parent", None, &[codex_line("turn_context", json!({"model":"gpt-test"})),
        codex_line("event_msg", json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}})),
        codex_line("event_msg", json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":9,"output_tokens":4,"total_tokens":13}}}))]);
    fixture.codex("child-a", Some("parent"), &[]);
    fixture.codex("child-b", Some("parent"), &[]);
    #[cfg(unix)]
    set_lock(&fixture, "parent", true);
    let nodes = collect(&fixture.options).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].children.len(), 2);
    assert_eq!(nodes[0].children[0].id, "child-a");
    assert_eq!(nodes[0].children[1].id, "child-b");
    assert_eq!(nodes[0].tokens.input, 9);
    #[cfg(unix)]
    {
        assert_eq!(nodes[0].state, "running");
        assert_eq!(nodes[0].pid, Some(4242));
        assert_eq!(nodes[0].children[0].state, "ended");
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
    let cache = fs::read_to_string(&fixture.options.cache).unwrap();
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
