//! One test per rule in the design's table, on synthetic homes only.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{Value, json};

use super::*;
use crate::events::EventCache;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// 2026-09-24T00:00:00Z.
const BASE: i64 = 1_790_208_000_000;
/// A day after BASE: nothing in these homes counts as running by time alone.
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
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-model-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
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
        home.write("proc/sys/kernel/hostname", "testbox\n");
        home
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn lines(&self, relative: &str, records: &[Value]) -> PathBuf {
        let text = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        self.write(relative, &text)
    }

    fn top(&self, id: &str, records: &[Value]) -> PathBuf {
        self.lines(&format!("claude/projects/-work-proj/{id}.jsonl"), records)
    }

    fn agent(&self, parent: &str, agent: &str, tool_use: &str, records: &[Value]) {
        self.lines(
            &format!("claude/projects/-work-proj/{parent}/subagents/agent-{agent}.jsonl"),
            records,
        );
        self.write(
            &format!("claude/projects/-work-proj/{parent}/subagents/agent-{agent}.meta.json"),
            &json!({"agentType":"general-purpose","description":format!("task {agent}"),"toolUseId":tool_use}).to_string(),
        );
    }

    fn codex(&self, id: &str, meta: Value, records: &[Value]) {
        let mut payload =
            json!({"id":id,"cwd":"/work/proj","originator":"codex_exec","thread_source":"user"});
        for (key, value) in meta.as_object().unwrap() {
            payload[key] = value.clone();
        }
        let mut lines = vec![json!({"timestamp":ts(0, 0),"type":"session_meta","payload":payload})];
        lines.extend_from_slice(records);
        self.lines(
            &format!("codex/sessions/2026/09/24/rollout-{id}.jsonl"),
            &lines,
        );
    }

    /// A live Claude process for `session`.
    fn live(&self, pid: u32, session: &str, status: &str, extra: Value) {
        let mut fields = vec!["0"; 20];
        fields[19] = "777";
        self.write(
            &format!("proc/{pid}/stat"),
            &format!("{pid} (claude) {}\n", fields.join(" ")),
        );
        let mut record = json!({"pid":pid,"sessionId":session,"procStart":777,"status":status,"name":format!("lane-{pid}")});
        for (key, value) in extra.as_object().unwrap() {
            record[key] = value.clone();
        }
        self.write(&format!("claude/sessions/{pid}.json"), &record.to_string());
    }

    fn build(&self) -> Built {
        self.build_at(&self.options, NOW)
    }

    /// Builds from this home's persisted event index (which commits as it
    /// reads), and checks the model's invariants.
    fn build_at(&self, options: &Options, now: i64) -> Built {
        let mut cache = EventCache::open(&options.cache);
        let mut dirty = false;
        if options.facts.is_none() {
            cache.refresh_reported_runs(&options.claude_json, now, &mut dirty);
        }
        let built = build(options, &mut cache, &mut dirty, &mut Texts::default(), now).unwrap();
        invariants(&built, options.all || options.scan_window);
        built
    }
}

/// Holds for every model: handoff and turn ids are unique, and every id and
/// session a handoff or turn names exists. Every handoff a stub's transcript
/// names is served; when nothing was trimmed (`whole`), every handoff any
/// transcript names is.
fn invariants(built: &Built, whole: bool) {
    let mut ids = BTreeSet::new();
    for handoff in &built.handoffs {
        assert!(
            ids.insert(handoff.id.clone()),
            "duplicate handoff {}",
            handoff.id
        );
        for end in [Some(&handoff.from), handoff.to.as_ref()]
            .into_iter()
            .flatten()
        {
            assert!(
                end == "you" || built.sessions.contains_key(end),
                "no session {end}"
            );
        }
    }
    let mut turns = BTreeSet::new();
    for turn in &built.turns {
        assert!(turns.insert(turn.id.clone()), "duplicate turn {}", turn.id);
        assert!(built.sessions.contains_key(&turn.sid));
        for id in turn.start.iter().chain(&turn.sent) {
            assert!(
                ids.contains(id),
                "turn {} names missing handoff {id}",
                turn.id
            );
        }
    }
    for (sid, transcript) in &built.tx {
        let stub = sid.starts_with("unsent:") || built.sessions.get(sid).is_some_and(|s| s.stub);
        if !(whole || stub) {
            continue;
        }
        for slot in &transcript.slots {
            if let SlotKind::H(id) = &slot.kind {
                assert!(
                    ids.contains(id),
                    "{sid}'s transcript names unserved handoff {id}"
                );
            }
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn human(sid: &str, time: String, text: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,"origin":{"kind":"human"},"message":{"role":"user","content":text}})
}

fn user(sid: &str, time: String, text: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,"message":{"role":"user","content":text}})
}

fn assistant(sid: &str, time: String, blocks: Vec<Value>) -> Value {
    json!({"type":"assistant","timestamp":time.clone(),"sessionId":sid,"message":{"id":format!("m-{sid}-{time}"),"model":"claude-opus-5-5","role":"assistant","content":blocks,
        "usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":2000,"output_tokens":500}}})
}

fn assistant_usage(sid: &str, time: String, id: &str, model: &str, usage: Value) -> Value {
    json!({"type":"assistant","timestamp":time,"sessionId":sid,"message":{"id":id,"model":model,"role":"assistant","content":[],
        "usage":usage}})
}

fn uuid(mut record: Value, id: &str) -> Value {
    record["uuid"] = json!(id);
    record
}

fn text(value: &str) -> Value {
    json!({"type":"text","text":value})
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type":"tool_use","id":id,"name":name,"input":input})
}

fn result(sid: &str, time: String, id: &str, content: &str, error: bool, outcome: Value) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,"toolUseResult":outcome,
        "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":content,"is_error":error}]}})
}

fn peer(sid: &str, time: String, msg_id: &str, name: &str, pid: u64, body: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,
        "origin":{"kind":"peer","from":format!("uds:/run/user/1000/cc-socks/{pid}.sock"),"name":name,"fromMode":"prompting","msg_id":msg_id,"body":body,"verifiedPeerPid":pid,"verifiedPeerProcStart":"999"},
        "message":{"role":"user","content":format!("<cross-session-message from=\"uds:/run/user/1000/cc-socks/{pid}.sock\" from-name=\"{name}\">{body}</cross-session-message>")}})
}

fn notification(sid: &str, time: String, id: &str, status: &str, body: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,"origin":{"kind":"task-notification"},
        "message":{"role":"user","content":format!("<task-notification><task-id>x</task-id><tool-use-id>{id}</tool-use-id><status>{status}</status><result>{body}</result></task-notification>")}})
}

fn codex_line(time: String, kind: &str, payload: Value) -> Value {
    json!({"timestamp":time,"type":kind,"payload":payload})
}

fn codex_user(time: String, text: &str) -> Value {
    codex_line(
        time,
        "response_item",
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}]}),
    )
}

fn codex_reply(time: String, text: &str) -> Value {
    codex_line(
        time,
        "response_item",
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}),
    )
}

fn only<'a>(built: &'a Built, kind: &str, from: &str, to: &str) -> &'a Handoff {
    let found: Vec<&Handoff> = built
        .handoffs
        .iter()
        .filter(|handoff| {
            handoff.kind == kind && handoff.from == from && handoff.to.as_deref() == Some(to)
        })
        .collect();
    assert_eq!(found.len(), 1, "{kind} {from} -> {to}: {found:?}");
    found[0]
}

fn by_brief<'a>(built: &'a Built, brief: &str) -> &'a Handoff {
    built
        .handoffs
        .iter()
        .find(|handoff| handoff.brief == brief)
        .unwrap_or_else(|| panic!("no handoff with brief {brief:?}: {:?}", built.handoffs))
}

fn turns_of<'a>(built: &'a Built, sid: &str) -> Vec<&'a Turn> {
    built.turns.iter().filter(|turn| turn.sid == sid).collect()
}

#[test]
fn sessions_include_the_latest_effort_and_omit_it_when_missing() {
    let home = Home::new();
    let mut claude_high = assistant("claude-effort", ts(1, 0), Vec::new());
    claude_high["effort"] = json!("high");
    claude_high["perTurnEffort"] = Value::Null;
    let mut claude_override = assistant("claude-effort", ts(1, 1), Vec::new());
    claude_override["effort"] = json!("high");
    claude_override["perTurnEffort"] = json!(" MAX ");
    home.top("claude-effort", &[claude_high, claude_override]);
    home.codex(
        "codex-effort",
        json!({"cwd":"/work/proj"}),
        &[codex_line(
            ts(1, 0),
            "turn_context",
            json!({"cwd":"/work/proj","model":"gpt-6-luna","effort":" XHIGH "}),
        )],
    );
    home.top(
        "claude-no-effort",
        &[assistant("claude-no-effort", ts(1, 2), Vec::new())],
    );

    let built = home.build();
    assert_eq!(
        built.sessions["claude-effort"].effort.as_deref(),
        Some("max")
    );
    assert_eq!(
        built.sessions["codex-effort"].effort.as_deref(),
        Some("xhigh")
    );
    assert_eq!(built.sessions["claude-no-effort"].effort, None);
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert_eq!(model["sessions"]["claude-effort"]["effort"], "max");
    assert_eq!(model["sessions"]["codex-effort"]["effort"], "xhigh");
    assert!(
        model["sessions"]["claude-no-effort"]
            .get("effort")
            .is_none()
    );
}

/// The Session details summary (`tokens`) and its per-model table (`tokens_by_model`, and the
/// cost table's token counts) come from the same billed messages, so they add up to one total:
/// in = input + cache write, cached = cache read, out = output.
#[test]
fn session_tokens_equal_the_sums_of_the_per_model_rows() {
    let home = Home::new();
    let mut unnamed = assistant_usage(
        "claude-sums",
        ts(1, 2),
        "message-3",
        "unused",
        json!({"input_tokens":50_000,"cache_creation_input_tokens":5_000,
            "cache_read_input_tokens":400_000,"output_tokens":7_000}),
    );
    // A usage report that names no model still counts.
    unnamed["message"].as_object_mut().unwrap().remove("model");
    home.top(
        "claude-sums",
        &[
            assistant_usage(
                "claude-sums",
                ts(1, 0),
                "message-1",
                "claude-opus-5-5",
                json!({"input_tokens":90_000,"cache_creation_input_tokens":19_000,
                    "cache_read_input_tokens":29_000,"output_tokens":39_000}),
            ),
            // The same message streamed again: it counts once, as this last report.
            assistant_usage(
                "claude-sums",
                ts(1, 1),
                "message-1",
                "claude-opus-5-5",
                json!({"input_tokens":100_000,"cache_creation_input_tokens":20_000,
                    "cache_read_input_tokens":30_000,"output_tokens":40_000}),
            ),
            unnamed,
        ],
    );

    let built = home.build();
    let session = &built.sessions["claude-sums"];
    let (mut input, mut cache_write, mut cache_read, mut output) = (0, 0, 0, 0);
    for row in session.tokens_by_model.values() {
        input += row.input;
        cache_write += row.cache_write;
        cache_read += row.cache_read;
        output += row.output;
    }
    let million = |value: u64| (value as f64 / 1e6 * 1000.0).round() / 1000.0;
    assert_eq!(
        session.tokens,
        [
            million(input + cache_write),
            million(cache_read),
            million(output)
        ]
    );
    assert_eq!(session.tokens, [0.175, 0.43, 0.047]);
    // The cost table's token counts are the same numbers.
    let (mut billed_input, mut billed_read, mut billed_output) = (0, 0, 0);
    for row in session.cost.by_model.values() {
        billed_input += row.tokens.input + row.tokens.cache_write_5m + row.tokens.cache_write_1h;
        billed_read += row.tokens.cache_read;
        billed_output += row.tokens.output;
    }
    assert_eq!(
        (billed_input, billed_read, billed_output),
        (input + cache_write, cache_read, output)
    );
}

#[test]
fn tokens_by_model_keep_claude_message_models_and_deduplicate_message_ids() {
    let home = Home::new();
    home.top(
        "claude-cost",
        &[
            assistant_usage(
                "claude-cost",
                ts(1, 0),
                "message-1",
                "claude-opus-5-5",
                json!({"input_tokens":90_000,"cache_creation_input_tokens":19_000,
                    "cache_read_input_tokens":29_000,"output_tokens":39_000}),
            ),
            assistant_usage(
                "claude-cost",
                ts(1, 1),
                "message-1",
                "claude-opus-5-5",
                json!({"input_tokens":100_000,"cache_creation_input_tokens":20_000,
                    "cache_read_input_tokens":30_000,"output_tokens":40_000}),
            ),
            assistant_usage(
                "claude-cost",
                ts(1, 2),
                "message-2",
                "claude-sonnet-5",
                json!({"input_tokens":200_000,"cache_creation_input_tokens":0,
                    "cache_read_input_tokens":50_000,"output_tokens":60_000}),
            ),
        ],
    );

    let built = home.build();
    let session = &built.sessions["claude-cost"];
    assert_eq!(
        session.tokens_by_model["claude-opus-5-5"],
        crate::events::ModelTokens {
            input: 100_000,
            output: 40_000,
            cache_write: 20_000,
            cache_read: 30_000,
        }
    );
    assert_eq!(
        session.tokens_by_model["claude-sonnet-5"],
        crate::events::ModelTokens {
            input: 200_000,
            output: 60_000,
            cache_write: 0,
            cache_read: 50_000,
        }
    );
    assert_eq!(session.tokens, [0.32, 0.08, 0.1]);
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert_eq!(
        model["sessions"]["claude-cost"]["tokens_by_model"]["claude-opus-5-5"]["cache_write"],
        20_000
    );
}

#[test]
fn claude_dated_model_ids_strip_only_the_trailing_date_for_price_matching() {
    let home = Home::new();
    home.top(
        "dated-model",
        &[assistant_usage(
            "dated-model",
            ts(1, 0),
            "message-dated",
            "claude-sonnet-4-5-20250929",
            json!({"input_tokens":90,"cache_creation_input_tokens":20,
                "cache_read_input_tokens":30,"output_tokens":40}),
        )],
    );

    let built = home.build();
    let usage = &built.sessions["dated-model"].tokens_by_model;
    assert_eq!(usage.len(), 1);
    assert_eq!(
        usage["claude-sonnet-4-5"],
        crate::events::ModelTokens {
            input: 90,
            output: 40,
            cache_write: 20,
            cache_read: 30,
        }
    );
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert!(
        model["sessions"]["dated-model"]["tokens_by_model"]
            .get("claude-sonnet-4-5-20250929")
            .is_none()
    );
    assert_eq!(
        model["pricing"]["models"]["claude-sonnet-4-5"]["input"],
        3.0
    );
}

#[test]
fn codex_model_switch_attributes_cumulative_token_deltas_to_the_current_model() {
    let home = Home::new();
    home.codex(
        "model-switch",
        json!({}),
        &[
            codex_line(ts(1, 0), "turn_context", json!({"model":"gpt-6-luna"})),
            codex_line(
                ts(1, 1),
                "event_msg",
                json!({"type":"token_count","info":{"total_token_usage":{
                    "input_tokens":100_000,"cached_input_tokens":20_000,"output_tokens":10_000,
                    "reasoning_output_tokens":2_000,"total_tokens":112_000
                }}}),
            ),
            codex_line(ts(1, 2), "turn_context", json!({"model":"gpt-test"})),
            codex_line(
                ts(1, 3),
                "event_msg",
                json!({"type":"token_count","info":{"total_token_usage":{
                    "input_tokens":300_000,"cached_input_tokens":70_000,"output_tokens":40_000,
                    "reasoning_output_tokens":8_000,"total_tokens":348_000
                }}}),
            ),
        ],
    );

    let built = home.build();
    let session = &built.sessions["model-switch"];
    assert_eq!(
        session.tokens_by_model["gpt-6-luna"],
        crate::events::ModelTokens {
            input: 80_000,
            output: 10_000,
            cache_write: 0,
            cache_read: 20_000,
        }
    );
    assert_eq!(
        session.tokens_by_model["gpt-test"],
        crate::events::ModelTokens {
            input: 150_000,
            output: 30_000,
            cache_write: 0,
            cache_read: 50_000,
        }
    );
    assert_eq!(session.tokens, [0.23, 0.07, 0.04]);
}

#[test]
fn codex_rate_limits_keep_the_latest_windows_and_support_both_reset_forms() {
    let home = Home::new();
    let rate_event = |time: String, primary: f64, secondary: f64| {
        codex_line(
            time,
            "event_msg",
            json!({"type":"token_count","info":{
                "total_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"total_tokens":0},
                "rate_limits":{
                    "primary":{"used_percent":primary,"window_minutes":10080,"resets_in_seconds":3600},
                    "secondary":{"used_percent":secondary,"window_minutes":300,
                        "resets_at":at(12,34) / 1000 + 86_400}
                }
            }}),
        )
    };
    home.codex(
        "limited",
        json!({}),
        &[
            codex_line(ts(12, 0), "turn_context", json!({"model":"gpt-6-luna"})),
            rate_event(ts(12, 30), 12.0, 20.0),
            rate_event(ts(12, 34), 62.5, 37.0),
        ],
    );

    let built = home.build();
    let limits = built.sessions["limited"].rate_limits.as_ref().unwrap();
    assert_eq!(limits.recorded_at, at(12, 34));
    assert_eq!(limits.windows.len(), 2);
    assert_eq!(limits.windows[0].minutes, 300);
    assert_eq!(limits.windows[0].used_percent, 37.0);
    assert_eq!(
        limits.windows[0].resets_at,
        (at(12, 34) / 1000 + 86_400) * 1000
    );
    assert_eq!(limits.windows[1].minutes, 10_080);
    assert_eq!(limits.windows[1].used_percent, 62.5);
    assert_eq!(limits.windows[1].resets_at, at(12, 34) + 3_600_000);
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert_eq!(
        model["sessions"]["limited"]["rate_limits"]["recorded_at"],
        at(12, 34)
    );
    assert_eq!(
        model["sessions"]["limited"]["rate_limits"]["windows"][0]["minutes"],
        300
    );
}

#[test]
fn unknown_models_keep_usage_but_have_no_price_row() {
    let home = Home::new();
    home.codex(
        "unknown-model",
        json!({}),
        &[
            codex_line(ts(1, 0), "turn_context", json!({"model":"gpt-test"})),
            codex_line(
                ts(1, 1),
                "event_msg",
                json!({"type":"token_count","info":{"total_token_usage":{
                    "input_tokens":50_000,"cached_input_tokens":10_000,"output_tokens":4_000,
                    "total_tokens":54_000
                }}}),
            ),
        ],
    );

    let built = home.build();
    assert_eq!(
        built.sessions["unknown-model"].tokens_by_model["gpt-test"],
        crate::events::ModelTokens {
            input: 40_000,
            output: 4_000,
            cache_write: 0,
            cache_read: 10_000,
        }
    );
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert!(model["pricing"]["models"].get("gpt-test").is_none());
    assert_eq!(
        model["pricing"]["as_of"],
        serde_json::to_value(crate::pricing::table()).unwrap()["as_of"]
    );
    assert_eq!(
        model["pricing"]["models"]["gpt-6-luna"]["cache_write"],
        0.125
    );
    assert_eq!(
        model["sessions"]["unknown-model"]["tokens_by_model"]["gpt-test"]["input"],
        40_000
    );
}

fn parent_with_agents(home: &Home) {
    home.top(
        "parent",
        &[
            human("parent", ts(1, 0), "Split the work"),
            assistant(
                "parent",
                ts(1, 1),
                vec![
                    tool("t1", "Agent", json!({"description":"one","prompt":"brief one"})),
                    tool("t2", "Agent", json!({"description":"two","prompt":"brief two"})),
                    tool("t3", "Agent", json!({"description":"three","prompt":"brief three"})),
                    tool("t4", "Agent", json!({"description":"four","prompt":"brief four"})),
                    tool("t5", "Agent", json!({"description":"five","prompt":"brief five"})),
                    tool("t6", "Agent", json!({"description":"six","prompt":"brief six"})),
                    tool("t7", "Agent", json!({"description":"seven","prompt":"brief seven"})),
                    tool("t8", "Agent", json!({"description":"eight","prompt":"brief eight"})),
                ],
            ),
            result("parent", ts(1, 2), "t1", "launched", false, json!({"status":"async_launched","agentId":"a1"})),
            result("parent", ts(1, 2), "t2", "launched", false, json!({"status":"async_launched","agentId":"a2"})),
            result("parent", ts(1, 3), "t3", "sync result three", false, json!({"status":"completed"})),
            result("parent", ts(1, 2), "t4", "launched", false, json!({"status":"async_launched"})),
            result("parent", ts(1, 2), "t5", "launched", false, json!({"status":"async_launched"})),
            result("parent", ts(1, 2), "t6", "launched", false, json!({"status":"async_launched"})),
            result("parent", ts(1, 2), "t7", "launched", false, json!({"status":"async_launched"})),
            result("parent", ts(1, 2), "t8", "launched", false, json!({"status":"async_launched"})),
            notification("parent", ts(1, 10), "t1", "completed", "notified one"),
            json!({"type":"user","timestamp":ts(1, 11),"sessionId":"parent",
                "origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/9.sock","handback":true,"senderTaskId":"a2","body":"handed back two"},
                "message":{"role":"user","content":"<agent-message from=\"a2\">handed back two</agent-message>"}}),
            json!({"type":"user","timestamp":ts(1, 15),"sessionId":"parent",
                "origin":{"kind":"peer","from":"lead","handback":true,"senderTaskId":"t8","body":"handed back eight"},
                "message":{"role":"user","content":"x"}}),
            notification("parent", ts(1, 12), "t5", "failed", "five broke"),
            notification("parent", ts(1, 13), "t6", "completed", "six by notification"),
            json!({"type":"user","timestamp":ts(1, 14),"sessionId":"parent",
                "origin":{"kind":"peer","from":"a6","handback":true,"senderTaskId":"a6","body":"six by hand-back"},
                "message":{"role":"user","content":"x"}}),
        ],
    );
    for (agent, tool_use) in [
        ("a1", "t1"),
        ("a2", "t2"),
        ("a3", "t3"),
        ("a5", "t5"),
        ("a6", "t6"),
        ("a7", "t7"),
        ("a8", "t8"),
    ] {
        home.agent(
            "parent",
            agent,
            tool_use,
            &[
                user("parent", ts(1, 1), &format!("brief {agent}")),
                assistant("parent", ts(1, 5), vec![text("working")]),
            ],
        );
    }
    home.agent(
        "parent",
        "a4",
        "t4",
        &[
            user("parent", ts(1, 1), "brief a4"),
            assistant(
                "parent",
                ts(1, 6),
                vec![tool(
                    "hb",
                    "SubagentHandback",
                    json!({"message":"handback tool four"}),
                )],
            ),
        ],
    );
}

#[test]
fn spawn_results_come_from_four_sources_in_order() {
    let home = Home::new();
    parent_with_agents(&home);
    let built = home.build();
    assert_eq!(built.sessions["a1"].parent.as_deref(), Some("parent"));
    let spawn = |agent: &str| only(&built, "spawn", "parent", agent).clone();
    let one = spawn("a1");
    assert_eq!(one.brief, "brief one");
    assert_eq!(one.at, at(1, 1));
    assert_eq!(
        (one.status, one.result.as_deref(), one.done),
        ("done", Some("notified one"), Some(at(1, 10)))
    );
    // A hand-back's `senderTaskId` names the agent, or its spawning call;
    // `from` differs in both.
    assert_eq!(spawn("a2").result.as_deref(), Some("handed back two"));
    assert_eq!(spawn("a8").result.as_deref(), Some("handed back eight"));
    assert_eq!(spawn("a3").result.as_deref(), Some("sync result three"));
    assert_eq!(spawn("a4").result.as_deref(), Some("handback tool four"));
    let five = spawn("a5");
    assert_eq!(
        (five.status, five.result.as_deref()),
        ("err", Some("five broke"))
    );
    assert_eq!(built.sessions["a5"].state, "err");
    // A notification outranks a hand-back.
    assert_eq!(spawn("a6").result.as_deref(), Some("six by notification"));
    // No result and the parent isn't running: done, with nothing returned.
    let seven = spawn("a7");
    assert_eq!((seven.status, seven.result.as_deref()), ("done", None));
    assert_eq!(built.sessions["a7"].kind, Some("Subagent"));
    assert_eq!(built.sessions["a7"].name, "task a7");
    // The spawn sits in the parent's turn and starts the child's.
    let parent_turns = turns_of(&built, "parent");
    assert_eq!(parent_turns.len(), 1);
    assert_eq!(parent_turns[0].sent.len(), 8);
    let child = turns_of(&built, "a1");
    assert_eq!(child[0].start.as_deref(), Some(one.id.as_str()));
    assert_eq!(child[0].end.why, "returned");
}

#[test]
fn a_running_parent_keeps_an_unreturned_subagent_working() {
    let home = Home::new();
    parent_with_agents(&home);
    home.live(10, "parent", "busy", json!({}));
    let built = home.build();
    assert_eq!(only(&built, "spawn", "parent", "a7").status, "work");
    assert_eq!(built.sessions["a7"].state, "work");
    assert_eq!(built.sessions["parent"].state, "work");
}

#[test]
fn codex_runs_link_by_marker_or_parent_thread_and_otherwise_stay_unlinked() {
    let home = Home::new();
    home.top(
        "lead",
        &[
            human("lead", ts(2, 0), "Hand it to Codex"),
            assistant(
                "lead",
                ts(2, 1),
                vec![tool(
                    "bash-1",
                    "Bash",
                    json!({"command":"codex exec < /tmp/p.md"}),
                )],
            ),
            result("lead", ts(2, 30), "bash-1", "ok", false, json!({})),
        ],
    );
    home.codex(
        "run-marked",
        json!({}),
        &[
            codex_user(ts(2, 2), "<environment_context>x</environment_context>"),
            codex_user(
                ts(2, 2),
                "Semon-Parent: claude:lead:bash-1\nImplement the thing",
            ),
            codex_reply(ts(2, 20), "Implemented"),
        ],
    );
    home.codex(
        "run-alone",
        json!({}),
        &[
            codex_user(ts(3, 0), "Do it alone"),
            codex_line(
                ts(3, 5),
                "event_msg",
                json!({"type":"task_complete","error":{"message":"boom"}}),
            ),
        ],
    );
    home.codex(
        "root",
        json!({}),
        &[
            codex_user(ts(4, 0), "Coordinate"),
            codex_line(ts(4, 1), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"c1","arguments":"{\"task_name\":\"worker\",\"message\":\"go\"}"})),
            codex_line(ts(4, 1), "response_item", json!({"type":"function_call_output","call_id":"c1","output":"{\"task_name\":\"worker\"}"})),
            codex_line(ts(4, 2), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"send_message","call_id":"c2","arguments":"{\"target\":\"root/worker\",\"message\":\"also this\"}"})),
            codex_line(ts(4, 2), "response_item", json!({"type":"function_call_output","call_id":"c2","output":"sent"})),
            codex_line(ts(4, 9), "response_item", json!({"type":"agent_message","author":"root/worker","recipient":"root","content":[{"type":"input_text","text":"worker finished"}]})),
        ],
    );
    home.codex(
        "child",
        json!({"parent_thread_id":"root","thread_source":"subagent","agent_nickname":"worker","agent_path":"root/worker"}),
        &[codex_user(ts(4, 1), "go"), codex_reply(ts(4, 8), "done")],
    );
    let built = home.build();
    assert_eq!(built.sessions["run-marked"].parent.as_deref(), Some("lead"));
    let marked = only(&built, "spawn", "lead", "run-marked");
    assert_eq!(marked.brief, "Implement the thing");
    assert_eq!(marked.result.as_deref(), Some("Implemented"));
    let lead = turns_of(&built, "lead");
    assert_eq!(lead[0].sent, std::slice::from_ref(&marked.id));
    assert!(
        built
            .handoffs
            .iter()
            .all(|handoff| handoff.to.as_deref() != Some("run-alone"))
    );
    assert!(built.sessions["run-alone"].lane);
    assert_eq!(built.sessions["run-alone"].state, "err");
    assert_eq!(built.sessions["run-alone"].kind, Some("Codex run"));
    let child = only(&built, "spawn", "root", "child");
    assert_eq!(child.result.as_deref(), Some("worker finished"));
    assert_eq!(built.sessions["child"].name, "worker");
    let relay = only(&built, "relay", "root", "child");
    assert_eq!(
        (relay.brief.as_str(), relay.unmatched),
        ("also this", false)
    );
    let root = turns_of(&built, "root");
    assert_eq!(root[0].sent, [child.id.clone(), relay.id.clone()]);
}

#[test]
fn variable_handoffs_are_placed_on_their_calls_and_parent_scans_resume() {
    let home = Home::new();
    let command = r#"S=/tmp/x/scratchpad; P=$S/codex-foo-prompt.md; { printf 'Semon-Parent: claude:%s\nSemon-Handoff: %s\n' "$CLAUDE_CODE_SESSION_ID" "$P"; cat "$P"; } | codex exec --json -C /home/u/wt/semon-wt-foo"#;
    let launch = |id: &str, time: String, command: &str| {
        assistant(
            "lead",
            time,
            vec![tool(
                id,
                "Bash",
                json!({"command":command,"run_in_background":true}),
            )],
        )
    };
    let path = home.top(
        "lead",
        &[
            human("lead", ts(0, 0), "Launch the runs"),
            launch("first", ts(0, 1), command),
            launch("second", ts(0, 11), command),
            launch(
                "multi",
                ts(0, 21),
                "codex exec -C /home/u/wt/semon-wt-bar; codex exec -C /home/u/wt/semon-wt-baz",
            ),
            launch("basename", ts(0, 31), "codex exec -C semon-wt-short"),
            launch(
                "prefix",
                ts(0, 41),
                "codex exec -C /home/u/wt/semon-wt-foobar",
            ),
        ],
    );
    let cases = [
        (
            "early",
            "semon-wt-foo",
            "2026-09-24T00:01:08Z",
            Some("first"),
        ),
        (
            "later",
            "semon-wt-foo",
            "2026-09-24T00:11:08Z",
            Some("second"),
        ),
        ("bar", "semon-wt-bar", "2026-09-24T00:21:08Z", Some("multi")),
        ("baz", "semon-wt-baz", "2026-09-24T00:21:09Z", Some("multi")),
        (
            "short",
            "semon-wt-short",
            "2026-09-24T00:31:08Z",
            Some("basename"),
        ),
        ("missing", "semon-wt-absent", "2026-09-24T00:31:08Z", None),
    ];
    for (id, basename, start, _) in cases {
        let prompt = format!("/tmp/x/scratchpad/codex-{id}-prompt.md");
        assert!(!command.contains(&prompt));
        home.lines(&format!("codex/sessions/2026/09/24/rollout-{id}.jsonl"), &[
            json!({"timestamp":start,"type":"session_meta","payload":{"id":id,"cwd":format!("/home/u/wt/{basename}")}}),
            codex_user(start.into(), &format!("Semon-Parent: claude:lead\nSemon-Handoff: {prompt}\nImplement {id}")),
            codex_reply(ts(1, 0), "done"),
        ]);
    }
    let parses = crate::handoff::PARSES.with(|count| count.get());
    let bytes = crate::handoff::BYTES.with(|count| count.get());
    let built = home.build();
    assert_eq!(crate::handoff::PARSES.with(|count| count.get()) - parses, 5);
    assert_eq!(
        crate::handoff::BYTES.with(|count| count.get()) - bytes,
        fs::metadata(&path).unwrap().len()
    );
    let records: Vec<Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (id, _, _, call) in cases {
        let spawn = only(&built, "spawn", "lead", id);
        let slots: Vec<_> = built.tx["lead"]
            .slots
            .iter()
            .filter(|slot| matches!(&slot.kind, SlotKind::H(found) if found == &spawn.id))
            .collect();
        if let Some(call) = call {
            assert_eq!(slots.len(), 1, "{id}");
            let record = records
                .iter()
                .find(|record| record["message"]["content"][0]["id"] == call)
                .unwrap();
            assert_eq!(slots[0].t, events::record_time(record), "{id}");
            assert_eq!(
                read_line(&path, slots[0].offset).unwrap()["message"]["content"]
                    [slots[0].block as usize]["id"],
                call
            );
            assert!(
                turns_of(&built, "lead")
                    .iter()
                    .any(|turn| turn.sent.contains(&spawn.id)),
                "{id}"
            );
        } else {
            assert!(slots.is_empty());
        }
    }
    let scanned = crate::handoff::BYTES.with(|count| count.get());
    home.build();
    assert_eq!(crate::handoff::PARSES.with(|count| count.get()) - parses, 5);
    assert_eq!(crate::handoff::BYTES.with(|count| count.get()), scanned);
    let appended = format!("{}\n", launch("third", ts(0, 51), command));
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(appended.as_bytes())
        .unwrap();
    let after = home.build();
    assert_eq!(crate::handoff::PARSES.with(|count| count.get()) - parses, 6);
    assert_eq!(
        crate::handoff::BYTES.with(|count| count.get()) - scanned,
        appended.len() as u64
    );
    assert_eq!(only(&after, "spawn", "lead", "early").at, at(0, 1));
    assert_eq!(only(&after, "spawn", "lead", "later").at, at(0, 11));
}

#[test]
fn worktree_prefix_does_not_place_a_spawn_on_the_wrong_call() {
    let home = Home::new();
    home.top(
        "lead",
        &[
            human("lead", ts(0, 0), "Launch"),
            assistant(
                "lead",
                ts(0, 1),
                vec![tool(
                    "foobar",
                    "Bash",
                    json!({"command":"codex exec -C /home/u/wt/semon-wt-foobar"}),
                )],
            ),
        ],
    );
    let start = "2026-09-24T00:01:08Z";
    home.lines("codex/sessions/2026/09/24/rollout-child.jsonl", &[
        json!({"type":"session_meta","timestamp":start,"payload":{"id":"child","cwd":"/home/u/wt/semon-wt-foo"}}),
        codex_user(start.into(), "Semon-Parent: claude:lead\nSemon-Handoff: /tmp/absent.md\nImplement"),
    ]);
    let built = home.build();
    let spawn = only(&built, "spawn", "lead", "child");
    assert_eq!(spawn.at, events::parse_ms(start).unwrap());
    assert!(
        built.tx["lead"]
            .slots
            .iter()
            .all(|slot| !matches!(&slot.kind, SlotKind::H(id) if id == &spawn.id))
    );
    assert!(
        turns_of(&built, "lead")
            .iter()
            .all(|turn| !turn.sent.contains(&spawn.id))
    );
}

fn relay_home() -> Home {
    let home = Home::new();
    home.top(
        "alpha",
        &[
            json!({"type":"custom-title","customTitle":"Alpha","sessionId":"alpha"}),
            human("alpha", ts(5, 0), "Tell the others"),
            assistant(
                "alpha",
                ts(5, 1),
                vec![
                    tool(
                        "s1",
                        "SendMessage",
                        json!({"to":"beta-01","message":"hello beta","type":"message"}),
                    ),
                    tool(
                        "s2",
                        "SendMessage",
                        json!({"to":"nobody-9f","message":"lost one","type":"message"}),
                    ),
                    tool(
                        "s3",
                        "SendMessage",
                        json!({"to":"beta-01","message":"denied one","type":"message"}),
                    ),
                    tool(
                        "s4",
                        "SendMessage",
                        json!({"to":"Bee","message":"by unique name","type":"message"}),
                    ),
                    tool(
                        "s5",
                        "SendMessage",
                        json!({"to":"Twin [a1b2]","message":"to a twin","type":"message"}),
                    ),
                    tool(
                        "s6",
                        "SendMessage",
                        json!({"to":"a1","message":"to my subagent","type":"message"}),
                    ),
                ],
            ),
            result(
                "alpha",
                ts(5, 1),
                "s1",
                "sent",
                false,
                json!({"success":true,"message":"sent","msg_id":"m1"}),
            ),
            result(
                "alpha",
                ts(5, 1),
                "s2",
                "no such peer",
                false,
                json!({"success":false,"message":"no such peer"}),
            ),
            result(
                "alpha",
                ts(5, 1),
                "s3",
                "Permission denied",
                true,
                json!("Error: permission denied"),
            ),
            result(
                "alpha",
                ts(5, 1),
                "s4",
                "sent",
                false,
                json!({"success":true,"message":"sent"}),
            ),
            result(
                "alpha",
                ts(5, 1),
                "s5",
                "sent",
                false,
                json!({"success":true,"message":"sent"}),
            ),
            result(
                "alpha",
                ts(5, 1),
                "s6",
                "sent",
                false,
                json!({"success":true,"message":"sent","pin":{"id":"a1","name":"x","ref":"y"}}),
            ),
        ],
    );
    home.top(
        "beta",
        &[
            json!({"type":"custom-title","customTitle":"Bee","sessionId":"beta"}),
            peer("beta", ts(5, 2), "m1", "alpha-3c [lead]", 111, "hello beta"),
            assistant("beta", ts(5, 3), vec![text("got it")]),
            peer("beta", ts(6, 0), "m9", "ghost", 4242, "from a gone session"),
        ],
    );
    for id in ["twin-1", "twin-2"] {
        home.top(
            id,
            &[
                json!({"type":"custom-title","customTitle":"Twin","sessionId":id}),
                human(id, ts(0, 30), "twin work"),
            ],
        );
    }
    home
}

#[test]
fn relays_join_on_msg_id() {
    let home = relay_home();
    let built = home.build();
    let relay = by_brief(&built, "hello beta");
    assert_eq!(
        (relay.from.as_str(), relay.to.as_deref()),
        ("alpha", Some("beta"))
    );
    assert_eq!(
        (relay.status, relay.unmatched, relay.at),
        ("done", false, at(5, 1))
    );
    let beta = turns_of(&built, "beta");
    assert_eq!(beta[0].start.as_deref(), Some(relay.id.as_str()));
    assert_eq!(beta[0].end.why, "replied");
    // One relay per message: the receiving side doesn't add another.
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.brief == "hello beta")
            .count(),
        1
    );
}

#[test]
fn failed_and_denied_sends_are_failed_relays_with_no_receiver() {
    let home = relay_home();
    let built = home.build();
    for (brief, target) in [("lost one", "nobody-9f"), ("denied one", "beta-01")] {
        let relay = by_brief(&built, brief);
        assert_eq!(relay.status, "err", "{brief}");
        assert_eq!(relay.to, None, "{brief}");
        assert_eq!(relay.target.as_deref(), Some(target));
    }
    // No stub stands in for a receiver that never got the message.
    assert!(
        built
            .sessions
            .values()
            .all(|session| session.name != "nobody-9f" && session.name != "beta-01")
    );
    // A send to a subagent isn't a relay.
    assert!(
        built
            .handoffs
            .iter()
            .all(|handoff| handoff.brief != "to my subagent")
    );
}

#[test]
fn received_message_from_a_gone_sender_keeps_a_stub() {
    let home = relay_home();
    let built = home.build();
    let relay = by_brief(&built, "from a gone session");
    assert_eq!(relay.to.as_deref(), Some("beta"));
    assert!(relay.unmatched);
    let stub = &built.sessions[&relay.from];
    assert!(stub.stub && stub.lane);
    assert_eq!((stub.name.as_str(), stub.state), ("ghost", "done"));
    assert_eq!((stub.start, stub.last), (at(6, 0), at(6, 0)));
}

#[test]
fn a_send_without_msg_id_resolves_a_unique_name_only() {
    let home = relay_home();
    let built = home.build();
    let unique = by_brief(&built, "by unique name");
    assert_eq!(
        (unique.to.as_deref(), unique.unmatched),
        (Some("beta"), false)
    );
    let twin = by_brief(&built, "to a twin");
    assert!(twin.unmatched);
    let stub = &built.sessions[twin.to.as_ref().unwrap()];
    assert!(stub.stub);
    assert_eq!(stub.name, "Twin");
}

fn question(text: &str, labels: &[&str], multi: bool) -> Value {
    json!({"question":text,"header":"h","multiSelect":multi,
        "options":labels.iter().map(|label| json!({"label":label,"description":"d"})).collect::<Vec<_>>()})
}

#[test]
fn questions_take_answers_from_the_structured_result() {
    let home = Home::new();
    let calls = [
        (
            "q1",
            vec![question("Ship it?", &["Yes", "No"], false)],
            json!({"answers":{"Ship it?":"Yes"}}),
            "",
            false,
        ),
        (
            "q3",
            vec![
                question("First?", &["A", "B"], false),
                question("Second?", &["C", "D"], false),
                question("Third?", &["E", "F"], false),
            ],
            json!({"answers":{"Third?":"F","First?":"A","Second?":"D"}}),
            "",
            false,
        ),
        (
            "multi-list",
            vec![question("Which?", &["Red", "Green", "Blue, light"], true)],
            json!({"answers":{"Which?":["Red","Blue, light"]}}),
            "",
            false,
        ),
        (
            "multi-joined",
            vec![question("Pick?", &["Red", "Blue, light", "Green"], true)],
            json!({"answers":{"Pick?":"Blue, light,Green"}}),
            "",
            false,
        ),
        (
            "free",
            vec![question("Name?", &["Semon", "Other"], false)],
            json!({"answers":{"Name?":"Call it Tern"}}),
            "",
            false,
        ),
        (
            "preview",
            vec![question("Layout?", &["Grid", "List"], false)],
            json!({"answers":{"Layout?":"Grid"},"annotations":{"Layout?":{"preview":"[grid]"}}}),
            "",
            false,
        ),
        (
            "declined",
            vec![question("Delete it?", &["Yes", "No"], false)],
            json!("User declined to answer questions"),
            "User declined",
            true,
        ),
        (
            "text-only",
            vec![
                question("Is \"quoted\" fine?", &["Yes", "No"], false),
                question("Next?", &["Go"], false),
            ],
            Value::Null,
            "Your questions have been answered: \"Is \"quoted\" fine?\"=\"Yes, \"really\"\", \"Next?\"=\"Go\". You can now continue with these answers in mind.",
            false,
        ),
    ];
    let mut records = vec![human("asker", ts(7, 0), "Ask me things")];
    for (minute, (id, questions, outcome, content, error)) in calls.iter().enumerate() {
        let minute = i64::try_from(minute).unwrap() * 2;
        records.push(assistant(
            "asker",
            ts(7, minute + 1),
            vec![tool(id, "AskUserQuestion", json!({"questions":questions}))],
        ));
        let mut line = result(
            "asker",
            ts(7, minute + 2),
            id,
            content,
            *error,
            outcome.clone(),
        );
        if outcome.is_null() {
            line.as_object_mut().unwrap().remove("toolUseResult");
        }
        records.push(line);
    }
    home.top("asker", &records);
    let built = home.build();
    golden("answers", &built);
    let get = |first: &str| by_brief(&built, first).clone();
    let one = get("Ship it?");
    assert_eq!(
        (one.kind, one.ask, one.status),
        ("toyou", Some("question"), "done")
    );
    assert_eq!(one.answer.as_deref(), Some(&["Yes".to_owned()][..]));
    assert_eq!(one.done, Some(at(7, 2)));
    let three = get("First?\nSecond?\nThird?");
    assert_eq!(three.answer.unwrap(), ["A", "D", "F"]);
    let list = get("Which?").answers.unwrap();
    assert_eq!(list[0].values, ["Red", "Blue, light"]);
    assert!(list[0].multi && !list[0].free);
    let joined = get("Pick?");
    assert_eq!(
        joined.answers.as_ref().unwrap()[0].values,
        ["Blue, light", "Green"]
    );
    assert_eq!(joined.answer.unwrap(), ["Blue, light, Green"]);
    let free = get("Name?").answers.unwrap();
    assert!(free[0].free);
    assert_eq!(free[0].values, ["Call it Tern"]);
    assert_eq!(
        get("Layout?").answers.unwrap()[0].preview.as_deref(),
        Some("[grid]")
    );
    let declined = get("Delete it?");
    assert!(declined.declined);
    assert_eq!(declined.answer.unwrap(), Vec::<String>::new());
    let text_only = get("Is \"quoted\" fine?\nNext?");
    assert_eq!(text_only.answer.unwrap(), ["Yes, \"really\"", "Go"]);
}

#[test]
fn only_human_origin_prompts_are_your_messages() {
    let home = Home::new();
    home.top(
        "lane",
        &[
            human("lane", ts(8, 0), "a real message"),
            user("lane", ts(8, 1), "/compact"),
            human("lane", ts(8, 1), "/compact keep the test output"),
            human("lane", ts(8, 2), "/compactor notes are fine"),
            user("lane", ts(8, 2), "<command-name>/compact</command-name>"),
            human("lane", ts(8, 3), "<command-name>/prompt-for-decisions</command-name>\n<command-args></command-args>"),
            json!({"type":"attachment","timestamp":ts(8, 4),"sessionId":"lane","attachment":{"type":"queued_command","commandMode":"prompt","prompt":"queued from you","origin":{"kind":"human"}}}),
            human("lane", ts(8, 5), "<system-reminder>only a reminder</system-reminder>"),
            json!({"type":"user","timestamp":ts(8, 6),"sessionId":"lane","isMeta":true,"origin":{"kind":"human"},"message":{"role":"user","content":"meta"}}),
        ],
    );
    home.top(
        "sdk",
        &[
            json!({"type":"user","timestamp":ts(9, 0),"sessionId":"sdk","entrypoint":"sdk-cli","promptSource":"sdk","message":{"role":"user","content":"a program's brief"}}),
            assistant("sdk", ts(9, 1), vec![text("done")]),
        ],
    );
    let built = home.build();
    let asks: Vec<&str> = built
        .handoffs
        .iter()
        .filter(|handoff| handoff.kind == "ask")
        .map(|handoff| handoff.brief.as_str())
        .collect();
    assert_eq!(
        asks,
        [
            "a real message",
            "/compactor notes are fine",
            "<command-name>/prompt-for-decisions</command-name>\n<command-args></command-args>",
            "queued from you"
        ]
    );
    let sdk = turns_of(&built, "sdk");
    assert_eq!(sdk.len(), 1);
    assert!(sdk[0].u && sdk[0].start.is_none());
    assert_eq!(sdk[0].end.why, "replied");
}

#[test]
fn lineages_join_by_session_id_continued_in_and_bridge_only() {
    let home = Home::new();
    home.top("root", &[human("root", ts(10, 0), "first")]);
    home.top(
        "cleared",
        &[json!({"type":"user","timestamp":ts(10, 30),"sessionId":"cleared","session_id":"root","origin":{"kind":"human"},"message":{"role":"user","content":"after clear"}})],
    );
    // A copy-resume: the new file starts with the old file's lines, uuids
    // and all.
    let history = [
        uuid(human("old", ts(11, 0), "before copy"), "u1"),
        uuid(
            assistant(
                "old",
                ts(11, 1),
                vec![tool(
                    "q-copied",
                    "AskUserQuestion",
                    json!({"questions":[question("Copied?", &["Yes"], false)]}),
                )],
            ),
            "u2",
        ),
        uuid(
            result(
                "old",
                ts(11, 2),
                "q-copied",
                "answered",
                false,
                json!({"answers":{"Copied?":"Yes"}}),
            ),
            "u3",
        ),
    ];
    let mut old = history.to_vec();
    old.push(json!({"type":"continued-in","continuedInSessionId":"new","sessionId":"old","timestamp":ts(11, 5)}));
    home.top("old", &old);
    let mut new = history.to_vec();
    new.push(uuid(human("new", ts(11, 10), "after copy"), "u4"));
    home.top("new", &new);
    home.top(
        "bridged-1",
        &[
            json!({"type":"bridge-session","bridgeSessionId":"rc-1","sessionId":"bridged-1"}),
            human("bridged-1", ts(12, 0), "remote one"),
        ],
    );
    home.top(
        "bridged-2",
        &[
            json!({"type":"bridge-session","bridgeSessionId":"rc-1","sessionId":"bridged-2"}),
            human("bridged-2", ts(13, 0), "remote two"),
        ],
    );
    home.top(
        "restarted",
        &[
            json!({"type":"bridge-session","bridgeSessionId":"rc-1","sessionId":"restarted"}),
            human("restarted", ts(12, 30), "while the first was open"),
            human("restarted", ts(14, 0), "after restart"),
        ],
    );
    home.live(20, "restarted", "idle", json!({}));
    // Only a live pid file names this bridge: that link would vanish with the
    // process, so it isn't one.
    home.top("pid-only", &[human("pid-only", ts(14, 30), "not joined")]);
    home.live(21, "pid-only", "idle", json!({"bridgeSessionId":"rc-1"}));
    home.top(
        "neighbour",
        &[human("neighbour", ts(10, 31), "close in time, no link")],
    );
    let built = home.build();
    golden("lineage", &built);
    let keys: Vec<&str> = built.sessions.keys().map(String::as_str).collect();
    assert_eq!(keys, ["bridged-1", "neighbour", "old", "pid-only", "root"]);
    let asks = |to: &str| -> Vec<&str> {
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.kind == "ask" && handoff.to.as_deref() == Some(to))
            .map(|handoff| handoff.brief.as_str())
            .collect()
    };
    assert_eq!(asks("root"), ["first", "after clear"]);
    assert_eq!(asks("old"), ["before copy", "after copy"]);
    let copied: Vec<&Handoff> = built
        .handoffs
        .iter()
        .filter(|handoff| handoff.brief == "Copied?")
        .collect();
    assert_eq!(copied.len(), 1);
    assert_eq!(turns_of(&built, "old").len(), 2);
    // The copied assistant message counts once: usage merges by message id.
    assert_eq!(built.sessions["old"].tokens, [0.001, 0.002, 0.001]);
    assert_eq!(
        asks("bridged-1"),
        [
            "remote one",
            "while the first was open",
            "remote two",
            "after restart"
        ]
    );
    // Files of one Remote Control session that were written at the same time
    // interleave by time.
    let starts: Vec<Option<i64>> = turns_of(&built, "bridged-1")
        .iter()
        .map(|turn| turn.at)
        .collect();
    assert_eq!(
        starts,
        [
            Some(at(12, 0)),
            Some(at(12, 30)),
            Some(at(13, 0)),
            Some(at(14, 0))
        ]
    );
    assert_eq!(built.sessions["bridged-1"].state, "idle");
    assert_eq!(built.sessions["bridged-1"].name, "lane-20");
}

#[test]
fn busy_clusters_every_line_of_a_lineage_within_five_minutes() {
    let home = Home::new();
    home.top(
        "one",
        &[
            human("one", ts(15, 0), "go"),
            json!({"type":"system","timestamp":ts(15, 4),"sessionId":"one"}),
        ],
    );
    home.top(
        "two",
        &[
            json!({"type":"system","timestamp":ts(15, 8),"sessionId":"two","session_id":"one"}),
            json!({"type":"system","timestamp":ts(15, 20),"sessionId":"two","session_id":"one"}),
        ],
    );
    let built = home.build();
    assert_eq!(
        built.sessions["one"].busy,
        [(at(15, 0), at(15, 8)), (at(15, 20), at(15, 20))]
    );
}

#[test]
fn states_follow_the_process_the_question_and_the_last_word() {
    let home = Home::new();
    home.top("working", &[human("working", ts(16, 0), "busy work")]);
    home.live(30, "working", "busy", json!({}));
    home.top(
        "asking",
        &[
            human("asking", ts(16, 0), "ask me"),
            assistant(
                "asking",
                ts(16, 1),
                vec![tool(
                    "open-q",
                    "AskUserQuestion",
                    json!({"questions":[question("Now?", &["Yes"], false)]}),
                )],
            ),
        ],
    );
    // The capture showed `waiting` while a question is on screen; a record
    // that still reads `busy` is defensive, and the open question outranks it.
    home.live(31, "asking", "busy", json!({}));
    // Claude Code 2.1.285 records `waiting` with the reason while it is
    // stopped on a dialog: a question, or a permission prompt.
    home.top(
        "input-needed",
        &[human("input-needed", ts(16, 0), "dialog")],
    );
    home.live(
        38,
        "input-needed",
        "waiting",
        json!({"waitingFor":"input needed"}),
    );
    home.top("permission", &[human("permission", ts(16, 0), "touch it")]);
    home.live(
        39,
        "permission",
        "waiting",
        json!({"waitingFor":"permission prompt"}),
    );
    // A finished turn with background shells still running is not counted as
    // working: it reads idle, as before.
    home.top("shell", &[human("shell", ts(16, 0), "start a server")]);
    home.live(40, "shell", "shell", json!({}));
    // A `waitingFor` outranks a `busy` status (defensive: a stale reason left
    // in the record while the turn runs again reads as needs you).
    home.top("stale-wait", &[human("stale-wait", ts(16, 0), "go on")]);
    home.live(
        41,
        "stale-wait",
        "busy",
        json!({"waitingFor":"permission prompt"}),
    );
    home.top(
        "answered",
        &[
            human("answered", ts(16, 0), "do it"),
            assistant(
                "answered",
                ts(16, 2),
                vec![text("All done: 3 files changed.")],
            ),
        ],
    );
    home.live(32, "answered", "idle", json!({}));
    home.top(
        "relayed-reply",
        &[
            peer(
                "relayed-reply",
                ts(16, 4),
                "relay-result",
                "Advisor",
                34,
                "finish the check",
            ),
            assistant(
                "relayed-reply",
                ts(16, 5),
                vec![text("The check is finished.")],
            ),
        ],
    );
    home.live(34, "relayed-reply", "idle", json!({}));
    home.top(
        "scheduled-reply",
        &[
            user("scheduled-reply", ts(16, 6), "scheduled prompt"),
            assistant(
                "scheduled-reply",
                ts(16, 7),
                vec![text("The scheduled check is finished.")],
            ),
        ],
    );
    home.live(35, "scheduled-reply", "idle", json!({}));
    home.top(
        "notified-reply",
        &[
            human("notified-reply", ts(16, 8), "check the task"),
            notification(
                "notified-reply",
                ts(16, 9),
                "task-check",
                "completed",
                "task finished",
            ),
            assistant(
                "notified-reply",
                ts(16, 10),
                vec![text("The task is finished.")],
            ),
        ],
    );
    home.live(36, "notified-reply", "idle", json!({}));
    home.top(
        "handback-reply",
        &[
            human("handback-reply", ts(16, 11), "check the agent"),
            json!({"type":"user","timestamp":ts(16, 12),"sessionId":"handback-reply",
                "origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/37.sock","handback":true,"senderTaskId":"agent-37","body":"agent handed back"},
                "message":{"role":"user","content":"agent handed back"}}),
            assistant(
                "handback-reply",
                ts(16, 13),
                vec![text("The agent is finished.")],
            ),
        ],
    );
    home.live(37, "handback-reply", "idle", json!({}));
    home.top(
        "idle",
        &[
            assistant("idle", ts(16, 0), vec![text("earlier")]),
            human("idle", ts(16, 3), "one more thing"),
        ],
    );
    home.live(33, "idle", "idle", json!({}));
    home.top("ended", &[human("ended", ts(16, 0), "long ago")]);
    let built = home.build();
    let state = |key: &str| built.sessions[key].state;
    assert_eq!(
        [
            state("working"),
            state("asking"),
            state("input-needed"),
            state("permission"),
            state("shell"),
            state("stale-wait"),
            state("answered"),
            state("relayed-reply"),
            state("scheduled-reply"),
            state("notified-reply"),
            state("handback-reply"),
            state("idle"),
            state("ended")
        ],
        [
            "work", "wait", "wait", "wait", "idle", "wait", "idle", "idle", "idle", "idle", "idle",
            "idle", "done"
        ]
    );
    assert_eq!(by_brief(&built, "busy work").status, "work");
    assert_eq!(by_brief(&built, "Now?").status, "wait");
    let result = by_brief(&built, "All done: 3 files changed.");
    assert_eq!(
        (result.kind, result.ask, result.status),
        ("toyou", Some("result"), "new")
    );
    assert_eq!(built.sessions["answered"].state, "idle");
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.from == "answered" && handoff.ask == Some("result"))
            .count(),
        1
    );
    assert!(
        !built
            .handoffs
            .iter()
            .any(|handoff| { handoff.from == "relayed-reply" && handoff.ask == Some("result") })
    );
    assert!(
        !built
            .handoffs
            .iter()
            .any(|handoff| { handoff.from == "scheduled-reply" && handoff.ask == Some("result") })
    );
    assert!(
        !built
            .handoffs
            .iter()
            .any(|handoff| { handoff.from == "notified-reply" && handoff.ask == Some("result") })
    );
    assert!(
        !built
            .handoffs
            .iter()
            .any(|handoff| { handoff.from == "handback-reply" && handoff.ask == Some("result") })
    );
    let answered = turns_of(&built, "answered");
    assert_eq!(answered[0].end.why, "toyou");
    assert_eq!(answered[0].end.st, "done");
    assert_eq!(turns_of(&built, "working")[0].end.why, "working");
}

#[test]
fn a_failed_codex_operation_ends_its_turn_as_a_failed_step() {
    let home = Home::new();
    home.codex(
        "failed-operation",
        json!({}),
        &[
            codex_user(ts(19, 0), "Run the command"),
            codex_line(
                ts(19, 1),
                "response_item",
                json!({"type":"custom_tool_call","call_id":"exec","name":"exec","input":"tools.exec_command({cmd:'false'})"}),
            ),
            codex_line(
                ts(19, 2),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","false"],"exit_code":1,"aggregated_output":"failed"}}),
            ),
            codex_line(
                ts(19, 3),
                "response_item",
                json!({"type":"custom_tool_call_output","call_id":"exec","output":"done"}),
            ),
        ],
    );
    let built = home.build();
    let turns = turns_of(&built, "failed-operation");
    let turn = turns.last().unwrap();
    assert_eq!(turn.end.why, "failed_step");
    assert_eq!(turn.end.st, "err");
}

#[test]
fn a_script_error_after_its_operations_ends_the_turn_as_failed() {
    let home = Home::new();
    home.codex(
        "late-script-error",
        json!({}),
        &[
            codex_user(ts(19, 0), "Run the script"),
            codex_line(
                ts(19, 1),
                "response_item",
                json!({"type":"custom_tool_call","call_id":"exec","name":"exec","input":"await tools.exec_command({cmd:'ls'}); throw new Error('boom')"}),
            ),
            codex_line(
                ts(19, 2),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","ls"],"exit_code":0,"aggregated_output":"a.rs"}}),
            ),
            codex_line(
                ts(19, 3),
                "response_item",
                json!({"type":"custom_tool_call_output","call_id":"exec","output":[{"type":"input_text","text":"Script error: boom"}]}),
            ),
        ],
    );
    let built = home.build();
    let turns = turns_of(&built, "late-script-error");
    let turn = turns.last().unwrap();
    assert_eq!((turn.end.st, turn.end.why), ("err", "failed_step"));
    let tx = &built.tx["late-script-error"];
    assert_eq!((tx.calls, tx.errors), (2, 1));
}

#[test]
fn a_result_follows_the_last_block_of_its_reply() {
    let home = Home::new();
    home.top(
        "blocks",
        &[
            human("blocks", ts(20, 0), "Summarise it"),
            assistant(
                "blocks",
                ts(20, 1),
                vec![text("First part."), text("Final part.")],
            ),
        ],
    );
    home.live(38, "blocks", "idle", json!({}));
    let built = home.build();
    let result = built
        .handoffs
        .iter()
        .find(|handoff| handoff.from == "blocks" && handoff.ask == Some("result"))
        .expect("a result for a reply to your message");
    let slots = &built.tx["blocks"].slots;
    let reply = slots
        .iter()
        .position(|slot| matches!(slot.kind, SlotKind::A) && slot.block == 1)
        .expect("the reply's last block");
    let marker = slots
        .iter()
        .position(|slot| matches!(&slot.kind, SlotKind::H(id) if *id == result.id))
        .expect("the result in the transcript");
    assert!(marker > reply, "result at {marker}, reply at {reply}");
    let turns = turns_of(&built, "blocks");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].end.why, "toyou");
}

#[test]
fn turns_split_at_each_incoming_entry_and_at_a_gap() {
    let home = Home::new();
    let path = home.top(
        "turny",
        &[
            human("turny", ts(17, 0), "first ask"),
            assistant("turny", ts(17, 1), vec![text("reply one")]),
            peer("turny", ts(17, 2), "m-in", "other", 5, "a relay in"),
            assistant(
                "turny",
                ts(17, 3),
                vec![tool("bad", "Bash", json!({"command":"false"}))],
            ),
            result("turny", ts(17, 3), "bad", "exit 1", true, json!({})),
        ],
    );
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str("{not json\n");
    for record in [
        assistant(
            "turny",
            ts(17, 5),
            vec![tool("never", "Read", json!({"file_path":"/x"}))],
        ),
        user("turny", ts(17, 6), "a prompt that isn't yours"),
        assistant("turny", ts(17, 7), vec![text("reply two")]),
    ] {
        content.push_str(&record.to_string());
        content.push('\n');
    }
    fs::write(&path, content).unwrap();
    let built = home.build();
    golden("turns", &built);
    let turns = turns_of(&built, "turny");
    let summary: Vec<(Option<&str>, bool, &str)> = turns
        .iter()
        .map(|turn| (turn.start.as_deref(), turn.u, turn.end.why))
        .collect();
    let ask = by_brief(&built, "first ask").id.clone();
    let relay = by_brief(&built, "a relay in").id.clone();
    assert_eq!(
        summary,
        [
            (Some(ask.as_str()), false, "replied"),
            (Some(relay.as_str()), false, "failed_step"),
            (None, false, "unfinished_step"),
            (None, true, "replied"),
        ]
    );
    assert_eq!(turns[0].id, ask);
    assert_eq!(turns[0].offset, 0);
    assert!(turns[2].id.starts_with("turny:turny:"));
    assert!(turns[3].last && !turns[2].last);
    // Turn offsets point at the first entry's line.
    let content = fs::read_to_string(&path).unwrap();
    let line = &content[usize::try_from(turns[3].offset).unwrap()..];
    assert!(line.starts_with("{\"type\":\"user\""));
}

#[test]
fn handoff_ids_are_stable_across_rebuilds_and_growth() {
    let home = relay_home();
    let first = home.build();
    let path = home.root.join("claude/projects/-work-proj/beta.jsonl");
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str(&format!("{}\n", human("beta", ts(6, 30), "later ask")));
    fs::write(&path, content).unwrap();
    let second = home.build();
    for handoff in &first.handoffs {
        assert!(
            second
                .handoffs
                .iter()
                .any(|other| other.id == handoff.id && other.brief == handoff.brief),
            "{} moved",
            handoff.id
        );
    }
    assert_eq!(second.handoffs.len(), first.handoffs.len() + 1);
}

#[test]
fn the_window_trims_output_but_not_links() {
    let mut home = relay_home();
    home.options.all = false;
    home.options.since = Duration::from_secs(3600);
    let built = home.build_at(&home.options, at(6, 30));
    let briefs: Vec<&str> = built
        .handoffs
        .iter()
        .map(|handoff| handoff.brief.as_str())
        .collect();
    assert_eq!(briefs, ["from a gone session"]);
    assert_eq!(built.sessions.len(), 2);
    assert!(
        built.sessions["beta"]
            .busy
            .iter()
            .all(|interval| interval.0 >= at(5, 30))
    );
}

#[test]
fn an_unread_result_survives_the_since_window() {
    let mut home = Home::new();
    home.top(
        "old-result",
        &[
            human("old-result", ts(0, 0), "Do this"),
            assistant("old-result", ts(0, 1), vec![text("Finished long ago.")]),
        ],
    );
    home.live(90, "old-result", "idle", json!({}));
    home.options.all = false;
    home.options.since = Duration::from_secs(3600);
    let built = home.build_at(&home.options, NOW);
    let handoff = by_brief(&built, "Finished long ago.");
    assert_eq!((handoff.ask, handoff.status), (Some("result"), "new"));
    assert!(built.sessions.contains_key("old-result"));
}

#[test]
fn a_subagent_message_to_a_sibling_names_its_sender_agent() {
    let home = Home::new();
    home.top(
        "lead",
        &[
            human("lead", ts(18, 0), "Run a team"),
            assistant(
                "lead",
                ts(18, 1),
                vec![
                    tool("ta", "Agent", json!({"prompt":"brief a"})),
                    tool("tb", "Agent", json!({"prompt":"brief b"})),
                ],
            ),
        ],
    );
    home.agent(
        "lead",
        "aa",
        "ta",
        &[
            user("lead", ts(18, 1), "brief a"),
            assistant(
                "lead",
                ts(18, 2),
                vec![tool(
                    "to-b",
                    "SendMessage",
                    json!({"to":"ab","message":"sibling note"}),
                )],
            ),
            result(
                "lead",
                ts(18, 2),
                "to-b",
                "sent",
                false,
                json!({"success":true,"message":"sent","pin":{"id":"ab"}}),
            ),
        ],
    );
    home.agent(
        "lead",
        "ab",
        "tb",
        &[
            user("lead", ts(18, 1), "brief b"),
            json!({"type":"user","timestamp":ts(18, 3),"sessionId":"lead",
                "origin":{"kind":"peer","from":"aa","name":"worker-a","body":"sibling note"},
                "message":{"role":"user","content":"<cross-session-message from=\"aa\" from-name=\"worker-a\">sibling note</cross-session-message>"}}),
        ],
    );
    let built = home.build();
    let relay = only(&built, "relay", "aa", "ab");
    assert_eq!(
        (relay.brief.as_str(), relay.unmatched),
        ("sibling note", false)
    );
    assert!(built.sessions.values().all(|session| !session.stub));
    let receiver = turns_of(&built, "ab");
    assert_eq!(receiver[1].start.as_deref(), Some(relay.id.as_str()));
}

#[test]
fn a_session_carries_its_transcripts_tool_call_and_error_counts() {
    let home = Home::new();
    home.top(
        "counted",
        &[
            human("counted", ts(17, 0), "run two"),
            assistant(
                "counted",
                ts(17, 1),
                vec![
                    tool("good", "Bash", json!({"command":"true"})),
                    tool("bad", "Bash", json!({"command":"false"})),
                ],
            ),
            result("counted", ts(17, 2), "good", "ok", false, json!({})),
            result("counted", ts(17, 3), "bad", "exit 1", true, json!({})),
            assistant("counted", ts(17, 4), vec![text("done")]),
        ],
    );
    let built = home.build();
    let session = &built.sessions["counted"];
    // The same totals `/api/tx` reports, from the same transcript index.
    let transcript = &built.tx["counted"];
    assert_eq!((transcript.calls, transcript.errors), (2, 1));
    assert_eq!((session.calls, session.errors), (Some(2), Some(1)));
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert_eq!(model["sessions"]["counted"]["calls"], 2);
    assert_eq!(model["sessions"]["counted"]["errors"], 1);
}

#[test]
fn a_session_counts_its_tool_calls_by_tool() {
    let home = Home::new();
    home.top(
        "named",
        &[
            human("named", ts(18, 0), "read, read, run"),
            assistant(
                "named",
                ts(18, 1),
                vec![
                    tool("r1", "Read", json!({"file_path":"/a"})),
                    tool("r2", "Read", json!({"file_path":"/b"})),
                    tool("b1", "Bash", json!({"command":"true"})),
                    tool("b2", "Bash", json!({"command":"false"})),
                    tool("e1", "Edit", json!({"file_path":"/a"})),
                ],
            ),
            result("named", ts(18, 2), "r1", "ok", false, json!({})),
            result("named", ts(18, 2), "r2", "ok", false, json!({})),
            result("named", ts(18, 2), "b1", "ok", false, json!({})),
            result("named", ts(18, 3), "b2", "exit 1", true, json!({})),
            result("named", ts(18, 3), "e1", "ok", false, json!({})),
            assistant("named", ts(18, 4), vec![text("done")]),
        ],
    );
    home.top(
        "quiet",
        &[
            human("quiet", ts(18, 0), "nothing to run"),
            assistant("quiet", ts(18, 1), vec![text("ok")]),
        ],
    );
    let built = home.build();
    let session = &built.sessions["named"];
    let expected = BTreeMap::from([
        ("Bash".to_owned(), 2),
        ("Edit".to_owned(), 1),
        ("Read".to_owned(), 2),
    ]);
    // The same transcript index as the totals, so the counts add up to `calls`.
    assert_eq!(session.tool_calls, expected);
    assert_eq!(
        session.tool_calls.values().sum::<usize>(),
        session.calls.unwrap()
    );
    assert_eq!(built.tx["named"].tools, expected);
    let model: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    assert_eq!(
        model["sessions"]["named"]["tool_calls"],
        json!({"Bash": 2, "Edit": 1, "Read": 2})
    );
    // A session with no calls has no map at all.
    assert!(model["sessions"]["quiet"].get("tool_calls").is_none());
}

#[test]
fn a_tool_name_is_at_most_the_name_cap_and_long_names_stay_apart() {
    // Invariant: no kept tool name is longer than TOOL_NAME_MAX characters, a short name is kept as it is, and two long
    // names that share a prefix stay different.
    assert_eq!(tool_label("Bash"), "Bash");
    let edge = "x".repeat(TOOL_NAME_MAX);
    assert_eq!(tool_label(&edge), edge);
    let long_a = format!("mcp__server__{}", "a".repeat(200));
    let long_b = format!("mcp__server__{}b", "a".repeat(200));
    let (a, b) = (tool_label(&long_a), tool_label(&long_b));
    assert_eq!(
        (a.chars().count(), b.chars().count()),
        (TOOL_NAME_MAX, TOOL_NAME_MAX)
    );
    assert!(a.starts_with("mcp__server__aaa") && a != b);
    // Characters, not bytes: a name of multi-byte characters is cut on a character boundary.
    let wide = "é".repeat(TOOL_NAME_MAX * 2);
    assert_eq!(tool_label(&wide).chars().count(), TOOL_NAME_MAX);
}

#[test]
fn tool_counts_keep_the_top_tools_and_one_other_bucket_that_sums_to_the_calls() {
    // Invariant: at most TOOL_KINDS_MAX names plus one "other" bucket, the largest kept, and the counts add up to the calls.
    let few: BTreeMap<String, usize> = (0..TOOL_KINDS_MAX)
        .map(|i| (format!("t{i:02}"), i + 1))
        .collect();
    assert_eq!(
        capped_tool_counts(few.clone()),
        few,
        "at the cap nothing is folded"
    );
    let many: BTreeMap<String, usize> = (0..TOOL_KINDS_MAX + 8)
        .map(|i| (format!("t{i:02}"), i + 1))
        .collect();
    let total: usize = many.values().sum();
    let capped = capped_tool_counts(many.clone());
    assert_eq!(capped.len(), TOOL_KINDS_MAX + 1);
    assert_eq!(capped.values().sum::<usize>(), total);
    // The eight smallest (1 through 8 calls) are the bucket; the twelve largest are kept as they were.
    assert_eq!(capped[TOOL_OTHER], (1..=8).sum::<usize>());
    assert!((8..TOOL_KINDS_MAX + 8).all(|i| capped[&format!("t{i:02}")] == i + 1));
    // A tool that is really named "other" adds to the bucket rather than replacing it.
    let mut named = many;
    named.insert(TOOL_OTHER.to_owned(), 1);
    let capped = capped_tool_counts(named);
    assert_eq!(capped.values().sum::<usize>(), total + 1);
    assert!(capped.len() <= TOOL_KINDS_MAX + 1);
}

#[test]
fn a_session_with_many_tools_serves_a_bounded_tool_calls_map_that_sums_to_its_calls() {
    let home = Home::new();
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for i in 0..TOOL_KINDS_MAX + 5 {
        let id = format!("c{i}");
        let name = format!("mcp__server__{}{}", "n".repeat(90), i);
        calls.push(tool(&id, &name, json!({})));
        results.push(result("many", ts(19, 2), &id, "ok", false, json!({})));
    }
    let mut lines = vec![
        human("many", ts(19, 0), "call many tools"),
        assistant("many", ts(19, 1), calls),
    ];
    lines.extend(results);
    home.top("many", &lines);
    let built = home.build();
    let session = &built.sessions["many"];
    assert_eq!(session.calls, Some(TOOL_KINDS_MAX + 5));
    assert_eq!(session.tool_calls.len(), TOOL_KINDS_MAX + 1);
    assert_eq!(
        session.tool_calls.values().sum::<usize>(),
        TOOL_KINDS_MAX + 5
    );
    assert!(
        session
            .tool_calls
            .keys()
            .all(|name| name.chars().count() <= TOOL_NAME_MAX)
    );
    assert_eq!(session.tool_calls[TOOL_OTHER], 5);
}

fn golden(name: &str, built: &Built) {
    let mut expected_shape: Value = serde_json::from_str(&built.json(NOW)).unwrap();
    // These snapshots cover the legacy model surface. The cost additions and
    // the transcript totals (`calls`, `errors`) have focused synthetic
    // assertions below and stay out of old fixtures.
    if let Some(sessions) = expected_shape["sessions"].as_object_mut() {
        for session in sessions.values_mut() {
            if let Some(session) = session.as_object_mut() {
                session.shift_remove("cost");
                session.shift_remove("reported_runs");
                session.shift_remove("cost_check");
                session.shift_remove("calls");
                session.shift_remove("errors");
                session.shift_remove("tool_calls");
            }
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/model/golden")
        .join(format!("{name}.json"));
    let expected = fs::read_to_string(&path).unwrap();
    let previous: Value = serde_json::from_str(&expected).unwrap();
    // The model version now also covers the cost keys, which were not part
    // of these compatibility snapshots.
    let update = std::env::var_os("SEMON_UPDATE_GOLDEN").is_some();
    if !update {
        expected_shape["version"] = previous["version"].clone();
    }
    let actual = serde_json::to_string_pretty(&expected_shape).unwrap() + "\n";
    if update {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &actual).unwrap();
    }
    assert_eq!(actual, expected, "{name} golden");
}

#[test]
fn rule_goldens() {
    golden("relays", &relay_home().build());
    let home = Home::new();
    parent_with_agents(&home);
    golden("spawns", &home.build());
}

#[test]
fn a_sender_that_cleared_without_a_link_stays_a_stub() {
    let home = Home::new();
    // A sent to B, then ran `/clear` with no session_id or bridge link, and
    // A's old file is gone. The same process now writes A's new file.
    home.top("a-new", &[human("a-new", ts(19, 0), "after the clear")]);
    home.live(42, "a-new", "idle", json!({}));
    let mut line = peer("b", ts(18, 30), "m-a", "a-3c", 42, "sent before the clear");
    line["origin"]["verifiedPeerProcStart"] = json!("777");
    home.top("b", &[line]);
    let built = home.build();
    let relay = by_brief(&built, "sent before the clear");
    assert_ne!(relay.from, "a-new");
    assert!(relay.unmatched);
    assert!(built.sessions[&relay.from].stub);
    assert_eq!(built.sessions[&relay.from].name, "a-3c");
}

#[test]
fn an_unchanged_second_build_parses_no_lines() {
    let home = relay_home();
    let first = home.build();
    let before = events::PARSED.with(std::cell::Cell::get);
    let second = home.build();
    assert_eq!(events::PARSED.with(std::cell::Cell::get), before);
    assert_eq!(first.version, second.version);
    let path = home.root.join("claude/projects/-work-proj/beta.jsonl");
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str(&human("beta", ts(6, 30), "one more").to_string());
    content.push('\n');
    fs::write(&path, content).unwrap();
    home.build();
    assert_eq!(events::PARSED.with(std::cell::Cell::get), before + 1);
}

#[test]
fn a_resumed_cache_builds_the_same_model_as_a_cold_one() {
    let home = relay_home();
    home.build();
    let path = home.root.join("claude/projects/-work-proj/alpha.jsonl");
    let append = |records: &[Value], malformed: bool| {
        let mut content = fs::read_to_string(&path).unwrap();
        if malformed {
            content.push_str("{not json\n");
        }
        for record in records {
            content.push_str(&record.to_string());
            content.push('\n');
        }
        fs::write(&path, content).unwrap();
    };
    append(
        &[assistant(
            "alpha",
            ts(7, 0),
            vec![
                text("checking"),
                tool("late", "Bash", json!({"command":"ls"})),
            ],
        )],
        true,
    );
    home.build();
    append(
        &[
            result("alpha", ts(7, 1), "late", "ok", false, json!({})),
            human("alpha", ts(7, 2), "thanks"),
        ],
        false,
    );
    let warm = home.build().json(NOW);
    let mut cold = home.options.clone();
    cold.cache = home.root.join("cold/index.json");
    assert_eq!(warm, home.build_at(&cold, NOW).json(NOW));
}

#[test]
fn non_ascii_tag_attributes_parse_without_panicking() {
    let home = Home::new();
    home.top(
        "lead",
        &[
            human("lead", ts(20, 0), "go"),
            assistant("lead", ts(20, 1), vec![tool("tz", "Agent", json!({"prompt":"brief"}))]),
            result("lead", ts(20, 1), "tz", "launched", false, json!({"status":"async_launched"})),
            user(
                "lead",
                ts(20, 5),
                "<agent-message 名前=\"x\" from=\"az\">über fertig</agent-message>",
            ),
            user(
                "lead",
                ts(20, 6),
                "<cross-session-message from=\"uds:ü\" from-name=\"名前-ü\">héllo</cross-session-message>",
            ),
        ],
    );
    home.agent("lead", "az", "tz", &[user("lead", ts(20, 1), "brief")]);
    let built = home.build();
    assert_eq!(
        only(&built, "spawn", "lead", "az").result.as_deref(),
        Some("über fertig")
    );
    let relay = by_brief(&built, "héllo");
    assert_eq!(built.sessions[&relay.from].name, "名前-ü");
}

#[test]
fn a_running_tool_ages_when_served_and_expires_after_thirty_minutes() {
    let home = Home::new();
    home.top(
        "busy",
        &[
            human("busy", ts(21, 0), "run it"),
            assistant(
                "busy",
                ts(21, 1),
                vec![tool(
                    "run",
                    "Bash",
                    json!({"command":"cargo test -p semon"}),
                )],
            ),
        ],
    );
    home.live(50, "busy", "busy", json!({}));
    let built = home.build_at(&home.options, at(21, 2));
    let activity = |now: i64| {
        serde_json::from_str::<Value>(&built.json(now)).unwrap()["sessions"]["busy"]["activity"]
            .clone()
    };
    // `[name, argument, age in seconds when served, start in epoch ms]`.
    assert_eq!(
        activity(at(21, 2)),
        json!(["Bash", "cargo test -p semon", 60, at(21, 1)])
    );
    assert_eq!(
        activity(at(21, 11)),
        json!(["Bash", "cargo test -p semon", 600, at(21, 1)])
    );
    assert_eq!(activity(at(21, 31)), Value::Null);
    // The running step keeps its turn working, whatever the time.
    assert_eq!(turns_of(&built, "busy")[0].end.why, "working");
}

#[cfg(unix)]
fn hold_lock(home: &Home, id: &str) {
    use std::os::unix::fs::MetadataExt;
    let path = home.write(&format!("codex/thread-writer-locks/{id}.lock"), "");
    let meta = fs::metadata(path).unwrap();
    let dev = meta.dev();
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    home.write(
        "proc/locks",
        &format!(
            "12: FLOCK ADVISORY WRITE 4242 {major:x}:{minor:x}:{} 0 EOF\n",
            meta.ino()
        ),
    );
}

/// A yielded command started in turn 1 and polled in turn 3 is running
/// while its session works: it is live, no error, and turn 1 doesn't end on
/// an unfinished step.
#[cfg(unix)]
#[test]
fn a_command_polled_in_a_later_turn_runs_in_the_turn_that_started_it() {
    let home = Home::new();
    let script = |id: &str, script: &str| {
        codex_line(
            ts(1, 0),
            "response_item",
            json!({"type":"custom_tool_call","call_id":id,"name":"exec","input":script}),
        )
    };
    let yielded = |time: String, id: &str| {
        let result = json!({"wall_time_seconds":1.0,"session_id":4242,"output":"listening\n"});
        codex_line(
            time,
            "response_item",
            json!({"type":"custom_tool_call_output","call_id":id,"output":[
                {"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type":"input_text","text":result.to_string()},
            ]}),
        )
    };
    let mut start = script(
        "start",
        "const r = await tools.exec_command({cmd:\"serve\",yield_time_ms:1000});\ntext(JSON.stringify(r));",
    );
    start["timestamp"] = json!(ts(1, 1));
    let mut poll = script(
        "poll",
        "const r = await tools.write_stdin({session_id:4242,chars:\"\",yield_time_ms:30000});\ntext(JSON.stringify(r));",
    );
    poll["timestamp"] = json!(ts(3, 1));
    home.codex(
        "serving",
        json!({}),
        &[
            codex_user(ts(1, 0), "Start the server"),
            start,
            yielded(ts(1, 2), "start"),
            codex_user(ts(2, 0), "Anything else?"),
            codex_reply(ts(2, 1), "No."),
            codex_user(ts(3, 0), "Check on it"),
            poll,
        ],
    );
    hold_lock(&home, "serving");
    let built = home.build();
    assert_eq!(built.sessions["serving"].state, "work");
    let tx = &built.tx["serving"];
    let shown: Vec<Shown> = tx
        .slots
        .iter()
        .filter_map(|slot| match &slot.kind {
            SlotKind::Yielded { shown, .. } => Some(*shown),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [Shown::Live]);
    assert_eq!(tx.errors, 0);
    let turns = turns_of(&built, "serving");
    assert_eq!(turns.len(), 3);
    assert_eq!((turns[0].end.st, turns[0].end.why), ("idle", "no_reply"));
    assert_eq!(turns[2].end.why, "working");

    // Its process gone, the same step is unfinished, and turn 1 ends on it.
    home.write("proc/locks", "");
    let built = home.build();
    assert_ne!(built.sessions["serving"].state, "work");
    let tx = &built.tx["serving"];
    assert!(tx.slots.iter().any(|slot| matches!(
        slot.kind,
        SlotKind::Yielded {
            shown: Shown::Unfinished,
            ..
        }
    )));
    assert_eq!(tx.errors, 1);
    assert_eq!(turns_of(&built, "serving")[0].end.why, "unfinished_step");
}

#[cfg(unix)]
#[test]
fn same_name_codex_spawns_stay_unplaced_and_questions_wait_only_while_unanswered() {
    let home = Home::new();
    let call = |minute: i64, id: &str| {
        codex_line(
            ts(22, minute),
            "response_item",
            json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":id,"arguments":"{\"task_name\":\"worker\",\"message\":\"go\"}"}),
        )
    };
    let ask = |minute: i64, id: &str, title: &str| {
        codex_line(
            ts(22, minute),
            "response_item",
            json!({"type":"function_call","name":"request_user_input_async","call_id":id,
                "arguments":format!("{{\"questions\":[{{\"title\":\"{title}\"}}]}}")}),
        )
    };
    let reply = |minute: i64, id: &str, output: &str| {
        codex_line(
            ts(22, minute),
            "response_item",
            json!({"type":"function_call_output","call_id":id,"output":output}),
        )
    };
    home.codex(
        "root",
        json!({}),
        &[
            codex_user(ts(22, 0), "Coordinate"),
            call(1, "c1"),
            call(5, "c2"),
            ask(6, "followed", "Followed by a message?"),
            reply(6, "followed", "{\"accepted\":true}"),
            codex_user(ts(22, 7), "Use the first"),
            ask(8, "answered", "Answered in the output?"),
            reply(8, "answered", "{\"accepted\":true,\"answer\":\"yes\"}"),
            ask(9, "open", "Still open?"),
            reply(9, "open", "{\"accepted\":true}"),
        ],
    );
    for id in ["first", "second"] {
        home.codex(
            id,
            json!({"parent_thread_id":"root","thread_source":"subagent","agent_nickname":"worker","agent_path":"root/worker"}),
            &[codex_user(ts(22, 3), "go")],
        );
    }
    hold_lock(&home, "root");
    let built = home.build();
    for child in ["first", "second"] {
        let spawn = only(&built, "spawn", "root", child);
        // Two calls and two runs share the task name: no pairing is exact.
        assert!(spawn.ambiguous, "{child}");
        assert_eq!(spawn.at, built.sessions[child].start);
    }
    let sent: Vec<&String> = built
        .turns
        .iter()
        .filter(|turn| turn.sid == "root")
        .flat_map(|turn| &turn.sent)
        .collect();
    assert!(sent.iter().all(|id| !id.starts_with('s')));
    assert_eq!(by_brief(&built, "Followed by a message?").status, "done");
    assert_eq!(by_brief(&built, "Answered in the output?").status, "done");
    assert_eq!(by_brief(&built, "Still open?").status, "wait");
}

#[test]
fn a_unique_codex_spawn_call_is_placed() {
    let home = Home::new();
    home.codex(
        "root",
        json!({}),
        &[
            codex_user(ts(23, 0), "Coordinate"),
            codex_line(ts(23, 1), "response_item", json!({"type":"function_call","name":"spawn_agent","call_id":"only","arguments":"{\"task_name\":\"solo\"}"})),
        ],
    );
    home.codex(
        "solo",
        json!({"parent_thread_id":"root","thread_source":"subagent","agent_nickname":"solo","agent_path":"root/solo"}),
        &[codex_user(ts(23, 2), "go")],
    );
    let built = home.build();
    let spawn = only(&built, "spawn", "root", "solo");
    assert!(!spawn.ambiguous);
    assert_eq!(spawn.at, at(23, 1));
    assert_eq!(
        turns_of(&built, "root")[0].sent,
        std::slice::from_ref(&spawn.id)
    );
}

#[test]
fn only_the_last_turn_can_hold_a_running_tool() {
    let home = Home::new();
    home.top(
        "stuck",
        &[
            human("stuck", ts(23, 0), "first"),
            assistant(
                "stuck",
                ts(23, 1),
                vec![tool("never", "Bash", json!({"command":"hang"}))],
            ),
            human("stuck", ts(23, 2), "second"),
            assistant("stuck", ts(23, 3), vec![text("on it")]),
        ],
    );
    home.live(60, "stuck", "busy", json!({}));
    home.top(
        "running",
        &[
            human("running", ts(23, 0), "first"),
            assistant(
                "running",
                ts(23, 1),
                vec![tool("old", "Read", json!({"file_path":"/old"}))],
            ),
            human("running", ts(23, 2), "second"),
            assistant(
                "running",
                ts(23, 3),
                vec![tool("now", "Bash", json!({"command":"make"}))],
            ),
        ],
    );
    home.live(61, "running", "busy", json!({}));
    let built = home.build_at(&home.options, at(23, 4));
    let ends = |sid: &str| -> Vec<&str> {
        turns_of(&built, sid)
            .iter()
            .map(|turn| turn.end.why)
            .collect()
    };
    assert_eq!(ends("stuck"), ["unfinished_step", "working"]);
    assert_eq!(ends("running"), ["unfinished_step", "working"]);
    let model: Value = serde_json::from_str(&built.json(at(23, 4))).unwrap();
    assert_eq!(model["sessions"]["stuck"]["activity"], Value::Null);
    assert_eq!(
        model["sessions"]["running"]["activity"],
        json!(["Bash", "make", 60, at(23, 3)])
    );
}

#[test]
fn every_assistant_text_survives_into_the_turn() {
    let home = Home::new();
    let path = home.top(
        "chatty",
        &[
            human("chatty", ts(2, 0), "talk to me"),
            assistant("chatty", ts(2, 1), vec![text("first"), text("second")]),
            assistant("chatty", ts(2, 2), vec![text("third")]),
            assistant("chatty", ts(2, 3), vec![text("the last word")]),
        ],
    );
    home.live(70, "chatty", "idle", json!({}));
    home.codex(
        "run",
        json!({}),
        &[
            codex_user(ts(3, 0), "Semon-Parent: claude:chatty\nDo it"),
            codex_reply(ts(3, 1), "halfway"),
            codex_reply(ts(3, 2), "all done"),
        ],
    );
    let built = home.build();
    let content = fs::read_to_string(&path).unwrap();
    let offset = |needle: &str| -> u64 {
        let at = content.find(needle).unwrap();
        u64::try_from(content[..at].rfind('\n').map_or(0, |line| line + 1)).unwrap()
    };
    // All four texts are in the one turn, in order.
    assert_eq!(
        built.texts["chatty"],
        [
            (0, offset("\"first\""), 0),
            (0, offset("\"first\""), 1),
            (0, offset("\"third\""), 0),
            (0, offset("the last word"), 0),
        ]
    );
    assert_eq!(turns_of(&built, "chatty")[0].end.why, "toyou");
    // The waiting result and a run's returned result are the last reply.
    let waiting = built
        .handoffs
        .iter()
        .find(|handoff| handoff.ask == Some("result"))
        .unwrap();
    assert_eq!(waiting.brief, "the last word");
    assert_eq!(
        only(&built, "spawn", "chatty", "run").result.as_deref(),
        Some("all done")
    );
    assert_eq!(built.texts["run"].len(), 2);
}

/// A copy of exactly `inputs()` of `home`, with `local_facts()` of the
/// original written beside it: the options that build the copy. The copy's
/// own `/proc` names another machine and nothing running, so only the facts
/// can make its model match.
fn copy_with_facts(home: &Home) -> (Home, Options) {
    let copy = Home::new();
    copy.write("proc/sys/kernel/hostname", "elsewhere\n");
    let mut options = copy.options.clone();
    for input in crate::inputs(&home.options).unwrap() {
        assert!(crate::is_input_path(input.root.as_str(), &input.path));
        let to = input.full_path(&options);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(input.full_path(&home.options), to).unwrap();
    }
    let facts = crate::local_facts(&home.options).unwrap();
    let path = copy.root.join("facts.json");
    crate::write_facts(&path, &facts).unwrap();
    options.facts = Some(path);
    (copy, options)
}

/// Every session's last transcript page, as served.
fn pages(built: &Built) -> Vec<String> {
    built
        .tx
        .keys()
        .map(|sid| crate::tx::page(built, sid, &crate::tx::Anchor::Last, NOW).unwrap())
        .collect()
}

fn assert_mirrors(home: &Home) {
    let original = home.build();
    let (_copy, options) = copy_with_facts(home);
    let mirrored = home.build_at(&options, NOW);
    assert_eq!(mirrored.json(NOW), original.json(NOW), "the model");
    assert_eq!(pages(&mirrored), pages(&original), "the transcripts");
}

#[cfg(unix)]
#[test]
fn a_copy_of_the_inputs_with_the_facts_builds_the_same_model() {
    let home = Home::new();
    parent_with_agents(&home);
    home.live(10, "parent", "busy", json!({}));
    home.top("working", &[human("working", ts(16, 0), "busy work")]);
    home.live(30, "working", "busy", json!({}));
    home.top("ended", &[human("ended", ts(16, 0), "long ago")]);
    // A pid file whose process is gone is not live.
    home.write(
        "claude/sessions/99.json",
        &json!({"pid":99,"sessionId":"ended","procStart":5,"status":"busy"}).to_string(),
    );
    // A repository on this disk, named by a session's and a run's cwd.
    let repo = home.root.join("work/harbor");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    let cwd = repo.join("src").to_string_lossy().into_owned();
    home.top(
        "in-repo",
        &[
            json!({"type":"user","timestamp":ts(17, 0),"sessionId":"in-repo","cwd":&cwd,
            "origin":{"kind":"human"},"message":{"role":"user","content":"fix the build"}}),
        ],
    );
    // A running Codex run, holding its writer lock.
    home.codex(
        "root",
        json!({"cwd": &cwd}),
        &[codex_user(ts(18, 0), "run the suite")],
    );
    hold_lock(&home, "root");
    // Never an input.
    home.write("claude/sessions/secret.key", "NEVER");

    let original = home.build();
    assert_eq!(original.sessions["working"].state, "work");
    assert_eq!(original.sessions["root"].state, "work");
    assert_eq!(original.sessions["in-repo"].repo.as_deref(), Some("harbor"));
    let inputs = crate::inputs(&home.options).unwrap();
    assert!(inputs.iter().all(|input| !input.path.ends_with(".key")));
    assert!(inputs.iter().any(|input| input.path == "sessions/30.json"));
    assert!(
        inputs
            .iter()
            .any(|input| input.path.ends_with("subagents/agent-a1.meta.json"))
    );
    assert_mirrors(&home);
}

#[test]
fn relays_and_lineages_mirror_too() {
    assert_mirrors(&relay_home());
}

#[test]
fn recorded_facts_decide_liveness_hostname_home_and_repos() {
    let home = Home::new();
    home.top(
        "reader",
        &[
            human("reader", ts(16, 0), "read it"),
            assistant(
                "reader",
                ts(16, 1),
                vec![tool(
                    "r1",
                    "Read",
                    json!({"file_path":"/home/fake-user/notes.md"}),
                )],
            ),
        ],
    );
    home.write(
        "claude/sessions/40.json",
        &json!({"pid":40,"sessionId":"reader","procStart":777,"status":"busy"}).to_string(),
    );
    let path = home.root.join("facts.json");
    let facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "laptop".into(),
        home: Some("/home/fake-user".into()),
        proc_starts: BTreeMap::from([(40, 777)]),
        codex_locks: BTreeMap::new(),
        repos: BTreeMap::new(),
        offline_since: None,
        runs: BTreeMap::new(),
        reported_runs: Vec::new(),
    };
    crate::write_facts(&path, &facts).unwrap();
    let mut options = home.options.clone();
    options.facts = Some(path.clone());
    let live = home.build_at(&options, NOW);
    assert_eq!(live.machine_id, "laptop");
    assert_eq!(live.sessions["reader"].state, "work");
    assert!(pages(&live).concat().contains("~/notes.md"));
    // Offline: the same machine, nothing running.
    crate::write_facts(&path, &facts.offline(1_790_000_000_000)).unwrap();
    let offline = home.build_at(&options, NOW);
    assert_eq!(offline.machine_id, "laptop");
    assert_ne!(offline.sessions["reader"].state, "work");
    // A missing facts file: nothing is live, whatever `/proc` says.
    options.facts = Some(home.root.join("missing.json"));
    let unknown = home.build_at(&options, NOW);
    assert_eq!(unknown.machine_id, "localhost");
    assert_ne!(unknown.sessions["reader"].state, "work");
}

#[test]
fn reported_cost_checks_keep_overwritten_runs_and_drop_account_values() {
    let home = Home::new();
    let real_usage = json!({
        "input_tokens":1034,
        "output_tokens":2080,
        "output_tokens_details":{"thinking_tokens":1900},
        "cache_read_input_tokens":518410,
        "cache_creation_input_tokens":151872,
        "cache_creation":{"ephemeral_1h_input_tokens":151872,"ephemeral_5m_input_tokens":0},
        "server_tool_use":{"web_search_requests":0},
        "speed":"standard","service_tier":"standard"
    });
    home.top(
        "run-one",
        &[assistant_usage(
            "run-one",
            ts(0, 10),
            "m-run-one",
            "claude-opus-5",
            real_usage.clone(),
        )],
    );
    let first_file = json!({
        "oauthAccount":{"emailAddress":"fixture-model@example.invalid","accountUuid":"fixture-model-account"},
        "projects":{"/private/fixture/project":{
            "lastSessionId":"run-one","lastStartTime":at(0,0),"lastCost":1.835095,
            "lastDuration":9000,"lastAPIDuration":8000,"lastToolDuration":1000,
            "lastLinesAdded":12,"lastLinesRemoved":3,"privateProjectValue":"private-project-value",
            "lastModelUsage":{"claude-opus-5[1m]":{
                "inputTokens":1034,"outputTokens":2080,"thinkingTokens":1900,
                "cacheReadInputTokens":518410,"cacheCreationInputTokens":151872,
                "webSearchRequests":0,"costUSD":1.835095,"privateModelValue":"private-model-value"
            }}
        }},
        "privateRootValue":"private-root-value"
    });
    home.write(".claude.json", &first_file.to_string());
    let first = home.build();
    assert!((first.sessions["run-one"].cost.usd.unwrap() - 1.835095).abs() < 1e-12);
    assert_eq!(first.sessions["run-one"].cost_check[0].ok, Some(true));
    assert_eq!(
        first.sessions["run-one"].reported_runs[0].by_model["claude-opus-5[1m]"].thinking_tokens,
        1900
    );
    assert_eq!(first.sessions["run-one"].cost.split_unknown_messages, 0);

    home.top(
        "run-two",
        &[assistant_usage(
            "run-two",
            ts(0, 20),
            "m-run-two",
            "claude-opus-5",
            real_usage,
        )],
    );
    let mut overwritten = first_file;
    overwritten["projects"]["/private/fixture/project"]["lastSessionId"] = json!("run-two");
    overwritten["projects"]["/private/fixture/project"]["lastStartTime"] = json!(at(0, 15));
    overwritten["projects"]["/private/fixture/project"]["lastCost"] = json!(2.0);
    home.write(".claude.json", &overwritten.to_string());
    let second = home.build();
    assert_eq!(second.sessions["run-one"].reported_runs.len(), 1);
    assert_eq!(second.sessions["run-one"].cost_check[0].ok, Some(true));
    assert_eq!(second.sessions["run-two"].reported_runs.len(), 1);
    assert_eq!(second.sessions["run-two"].cost_check[0].ok, Some(false));

    let served = second.json(NOW);
    let served_json: Value = serde_json::from_str(&served).unwrap();
    assert!(
        (served_json["sessions"]["run-one"]["cost"]["usd"]
            .as_f64()
            .unwrap()
            - 1.835095)
            .abs()
            < 1e-12
    );
    assert_eq!(
        served_json["sessions"]["run-one"]["cost"]["by_model"]["claude-opus-5"]["tokens"]["cache_write_1h"],
        151872
    );
    assert_eq!(
        served_json["sessions"]["run-one"]["reported_runs"][0]["duration_ms"],
        9000
    );
    assert_eq!(
        served_json["sessions"]["run-one"]["cost_check"][0]["ok"],
        true
    );
    assert!(
        served_json["sessions"]["run-one"]["tokens_by_model"]["claude-opus-5"]
            .get("cache_write")
            .is_some()
    );
    for secret in [
        "fixture-model@example.invalid",
        "fixture-model-account",
        "/private/fixture/project",
        "private-project-value",
        "private-model-value",
        "private-root-value",
    ] {
        assert!(!served.contains(secret));
    }
    let facts = crate::local_facts(&home.options).unwrap();
    let facts_json = serde_json::to_string(&facts).unwrap();
    assert_eq!(facts.reported_runs.len(), 2);
    assert!(!facts_json.contains("fixture-model@example.invalid"));
    assert!(!facts_json.contains("fixture-model-account"));
    assert!(!facts_json.contains("/private/fixture/project"));
}

#[test]
fn input_paths_are_the_builders_and_nothing_else() {
    for (root, path) in [
        ("claude", "projects/-work-proj/abc.jsonl"),
        ("claude", "projects/-work-proj/abc/subagents/agent-a1.jsonl"),
        (
            "claude",
            "projects/-work-proj/abc/subagents/agent-a1.meta.json",
        ),
        ("claude", "sessions/1234.json"),
        ("codex", "sessions/2026/09/24/rollout-x.jsonl"),
    ] {
        assert!(crate::is_input_path(root, path), "{root}/{path}");
    }
    for (root, path) in [
        ("claude", "sessions/1234.key"),
        ("claude", "sessions/abc.json"),
        ("claude", "sessions/1234.json/x"),
        ("claude", "projects/../sessions/1.key"),
        ("claude", "projects/p/../../x.jsonl"),
        ("claude", "/etc/passwd"),
        ("claude", "projects//x.jsonl"),
        ("claude", "projects/./x.jsonl"),
        ("claude", "projects/p\\x.jsonl"),
        ("claude", "projects/p/x.jsonl\n"),
        ("claude", "projects/p/x.json"),
        ("claude", "projects/p/agent-a.meta.json"),
        ("claude", "projects/p/subagents/agent-.meta.json"),
        ("claude", "settings.json"),
        ("claude", "history.jsonl"),
        ("claude", ""),
        ("codex", "auth.json"),
        ("codex", "thread-writer-locks/x.lock"),
        ("codex", "sessions/.jsonl"),
        ("elsewhere", "sessions/1.json"),
    ] {
        assert!(!crate::is_input_path(root, path), "{root}/{path:?}");
    }
    let home = Home::new();
    home.write(
        ".claude.json",
        r#"{"oauthAccount":{"emailAddress":"fixture@example.invalid"}}"#,
    );
    home.top("input-check", &[human("input-check", ts(0, 0), "fixture")]);
    let inputs = crate::inputs(&home.options).unwrap();
    assert!(
        inputs
            .iter()
            .all(|input| { input.full_path(&home.options) != home.options.claude_json })
    );
}

#[test]
fn a_worktrees_sessions_belong_to_its_repository_and_keep_their_branch() {
    let home = Home::new();
    let main = home.root.join("work/semon");
    fs::create_dir_all(main.join(".git/worktrees/x")).unwrap();
    let worktree = home.root.join("work/semon-wt-x");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/x").display()),
    )
    .unwrap();
    let cwd = worktree.join("src").to_string_lossy().into_owned();
    home.top(
        "claude-wt",
        &[
            json!({"type":"user","timestamp":ts(17, 0),"sessionId":"claude-wt","cwd":&cwd,
            "gitBranch":"fix/x","origin":{"kind":"human"},
            "message":{"role":"user","content":"fix it"}}),
        ],
    );
    home.codex(
        "codex-wt",
        json!({"cwd": &cwd, "git": {"branch": "fix/y"}}),
        &[codex_user(ts(18, 0), "run the suite")],
    );
    let built = home.build();
    for (id, branch) in [("claude-wt", "fix/x"), ("codex-wt", "fix/y")] {
        assert_eq!(built.sessions[id].repo.as_deref(), Some("semon"), "{id}");
        assert_eq!(built.sessions[id].branch.as_deref(), Some(branch), "{id}");
    }
}

#[test]
fn the_repo_cache_is_bounded() {
    let mut texts = Texts::default();
    for n in 0..REPO_CACHE_MAX * 2 + 3 {
        assert_eq!(texts.repo(&format!("/nowhere/repo-{n}/plain")), None);
        assert!(texts.repos.len() <= REPO_CACHE_MAX);
    }
    // A cwd met again after the cache was dropped is looked up afresh.
    assert_eq!(
        texts
            .repo("/nowhere/harbor/.claude/worktrees/agent-1")
            .as_deref(),
        Some("harbor")
    );
    assert!(texts.repos.len() <= REPO_CACHE_MAX);
}

/// What `/api/analytics?range=7d` answers over this model, as the served
/// build computes it: from the activity kept before the model is trimmed.
fn week(built: &Built) -> Value {
    let rows = crate::analytics::rows([("testbox", &built.activity)]);
    let request = crate::analytics::Request::parse("range=7d").unwrap();
    crate::analytics::answer(&rows, &request, NOW, "v1")
}

const MINUTE: i64 = 60_000;

/// A question answered four minutes after it was asked is four minutes
/// waited on you, whether the answer picked an option or was declined, and
/// a session that never asked contributes nothing.
#[test]
fn an_answered_question_is_time_waited_on_you() {
    let home = Home::new();
    let ask = |id: &str, minute: i64| {
        assistant(
            "asker",
            ts(10, minute),
            vec![tool(
                id,
                "AskUserQuestion",
                json!({"questions":[question(&format!("Ship {id}?"), &["Yes", "No"], false)]}),
            )],
        )
    };
    home.top(
        "asker",
        &[
            human("asker", ts(10, 0), "Ask me things"),
            ask("first", 1),
            result(
                "asker",
                ts(10, 5),
                "first",
                "",
                false,
                json!({"answers":{"Ship first?":"Yes"}}),
            ),
            ask("second", 10),
            result(
                "asker",
                ts(10, 11),
                "second",
                "User declined",
                true,
                json!("User declined to answer questions"),
            ),
        ],
    );
    home.top(
        "plain",
        &[
            human("plain", ts(10, 0), "Just work"),
            assistant("plain", ts(10, 3), vec![text("Done.")]),
        ],
    );
    let built = home.build();
    let week = week(&built);
    // Four minutes, then one: a wait ends at its answer, or its decline.
    assert_eq!(week["current"]["wait_ms"], 5 * MINUTE);
    assert_eq!(week["current"]["median_wait_ms"], (4 * MINUTE + MINUTE) / 2);
    assert_eq!(week["current"]["longest_wait_ms"], 4 * MINUTE);
    assert_eq!(
        week["top"]["waited"],
        json!([{ "sid": "asker", "ms": 5 * MINUTE }])
    );
    // Nothing waits now, and the earlier week had no waits.
    assert!(week["longest_current_wait"].is_null());
    assert_eq!(week["previous"]["wait_ms"], 0);
    // A session with no question waited on no one.
    let waited: Vec<&str> = week["top"]["waited"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["sid"].as_str().unwrap())
        .collect();
    assert!(!waited.contains(&"plain"));
}

/// An open question has waited since it was asked, and is the longest
/// current wait; another session's answered one is not current.
#[test]
fn an_unanswered_question_is_the_longest_current_wait() {
    let home = Home::new();
    home.top(
        "asking",
        &[
            human("asking", ts(11, 0), "ask me"),
            assistant(
                "asking",
                ts(11, 1),
                vec![tool(
                    "open-q",
                    "AskUserQuestion",
                    json!({"questions":[question("Now?", &["Yes"], false)]}),
                )],
            ),
        ],
    );
    home.live(41, "asking", "idle", json!({}));
    home.top(
        "answered",
        &[
            human("answered", ts(11, 0), "ask me"),
            assistant(
                "answered",
                ts(11, 2),
                vec![tool(
                    "done-q",
                    "AskUserQuestion",
                    json!({"questions":[question("Then?", &["Yes"], false)]}),
                )],
            ),
            result(
                "answered",
                ts(11, 4),
                "done-q",
                "",
                false,
                json!({"answers":{"Then?":"Yes"}}),
            ),
        ],
    );
    let built = home.build();
    assert_eq!(built.sessions["asking"].state, "wait");
    let open = NOW - at(11, 1);
    let week = week(&built);
    assert_eq!(
        week["longest_current_wait"],
        json!({ "sid": "asking", "ms": open })
    );
    assert_eq!(week["current"]["wait_ms"], open + 2 * MINUTE);
    assert_eq!(week["current"]["longest_wait_ms"], open);
    assert_eq!(
        week["top"]["waited"],
        json!([
            { "sid": "asking", "ms": open },
            { "sid": "answered", "ms": 2 * MINUTE },
        ])
    );
}

/// A Codex question waits from the call until the answer in its output, or,
/// when the output only acknowledges it, until your next message; one still
/// unanswered in a live run is a current wait.
#[cfg(unix)]
#[test]
fn a_codex_question_is_time_waited_on_you() {
    let home = Home::new();
    let ask = |hour: i64, minute: i64, id: &str, title: &str| {
        codex_line(
            ts(hour, minute),
            "response_item",
            json!({"type":"function_call","name":"request_user_input_async","call_id":id,
                "arguments":format!("{{\"questions\":[{{\"title\":\"{title}\"}}]}}")}),
        )
    };
    let reply = |hour: i64, minute: i64, id: &str, output: &str| {
        codex_line(
            ts(hour, minute),
            "response_item",
            json!({"type":"function_call_output","call_id":id,"output":output}),
        )
    };
    home.codex(
        "cx-answered",
        json!({}),
        &[
            codex_user(ts(12, 0), "Go"),
            ask(12, 1, "acked", "Which one?"),
            reply(12, 1, "acked", "{\"accepted\":true}"),
            codex_user(ts(12, 7), "The first"),
            ask(12, 10, "answered", "Really?"),
            reply(12, 12, "answered", "{\"accepted\":true,\"answer\":\"yes\"}"),
        ],
    );
    home.codex(
        "cx-open",
        json!({}),
        &[
            codex_user(ts(13, 0), "Go"),
            ask(13, 1, "open", "Still there?"),
            reply(13, 1, "open", "{\"accepted\":true}"),
        ],
    );
    hold_lock(&home, "cx-open");
    let built = home.build();
    assert_eq!(by_brief(&built, "Which one?").status, "done");
    assert_eq!(by_brief(&built, "Still there?").status, "wait");
    let open = NOW - at(13, 1);
    let week = week(&built);
    assert_eq!(
        week["top"]["waited"],
        json!([
            { "sid": "cx-open", "ms": open },
            { "sid": "cx-answered", "ms": 8 * MINUTE },
        ])
    );
    assert_eq!(week["current"]["wait_ms"], open + 8 * MINUTE);
    assert_eq!(
        week["longest_current_wait"],
        json!({ "sid": "cx-open", "ms": open })
    );
}

/// "Top sessions · waited on" lists the sessions that waited longest first.
#[test]
fn top_sessions_waited_on_list_the_longest_first() {
    let home = Home::new();
    for (sid, minutes) in [("brief", 1), ("long", 4)] {
        let id = format!("{sid}-q");
        home.top(
            sid,
            &[
                human(sid, ts(14, 0), "ask me"),
                assistant(
                    sid,
                    ts(14, 1),
                    vec![tool(
                        &id,
                        "AskUserQuestion",
                        json!({"questions":[question("Which?", &["A", "B"], false)]}),
                    )],
                ),
                result(
                    sid,
                    ts(14, 1 + minutes),
                    &id,
                    "",
                    false,
                    json!({"answers":{"Which?":"A"}}),
                ),
            ],
        );
    }
    let week = week(&home.build());
    assert_eq!(
        week["top"]["waited"],
        json!([
            { "sid": "long", "ms": 4 * MINUTE },
            { "sid": "brief", "ms": MINUTE },
        ])
    );
}

/// A subagent's `SendMessage` to `main` is Claude Code's address for its own
/// parent conversation: a relay to the session that spawned it, not a peer
/// named "main". A top-level session's send to `main` still resolves by name,
/// else stands in a stub.
#[test]
fn a_subagent_send_to_main_is_a_relay_to_its_parent() {
    let home = Home::new();
    home.top(
        "lead",
        &[
            human("lead", ts(19, 0), "Start a worker"),
            assistant(
                "lead",
                ts(19, 1),
                vec![tool("tw", "Agent", json!({"prompt":"worker brief"}))],
            ),
            result(
                "lead",
                ts(19, 1),
                "tw",
                "launched",
                false,
                json!({"status":"async_launched"}),
            ),
            assistant(
                "lead",
                ts(19, 5),
                vec![tool(
                    "lead-main",
                    "SendMessage",
                    json!({"to":"main","message":"lead to a peer called main"}),
                )],
            ),
            result(
                "lead",
                ts(19, 5),
                "lead-main",
                "sent",
                false,
                json!({"success":true,"message":"sent"}),
            ),
        ],
    );
    home.agent(
        "lead",
        "aw",
        "tw",
        &[
            user("lead", ts(19, 1), "worker brief"),
            assistant(
                "lead",
                ts(19, 2),
                vec![tool(
                    "to-main",
                    "SendMessage",
                    json!({"to":"main","summary":"progress","message":"halfway there","type":"message","recipient":"main"}),
                )],
            ),
            result(
                "lead",
                ts(19, 2),
                "to-main",
                "queued",
                false,
                json!({"success":true,"message":"Message queued for the main conversation's next turn."}),
            ),
            assistant("lead", ts(19, 3), vec![text("worker done")]),
        ],
    );
    let built = home.build();
    let relay = only(&built, "relay", "aw", "lead");
    assert_eq!(
        (
            relay.brief.as_str(),
            relay.status,
            relay.unmatched,
            relay.at
        ),
        ("halfway there", "done", false, at(19, 2))
    );
    assert_eq!(relay.target, None);
    // The subagent's turn sent it; the subagent is still the lead's child.
    assert!(
        turns_of(&built, "aw")
            .iter()
            .any(|turn| turn.sent.contains(&relay.id))
    );
    assert_eq!(built.sessions["aw"].parent.as_deref(), Some("lead"));
    // The lead's own send to "main" names a peer: no session has that name,
    // so a stub stands in, as before.
    let peer = by_brief(&built, "lead to a peer called main");
    assert!(peer.unmatched);
    let stub = &built.sessions[peer.to.as_ref().unwrap()];
    assert_eq!((stub.stub, stub.name.as_str()), (true, "main"));
    assert_eq!(
        built
            .sessions
            .values()
            .filter(|session| session.stub)
            .count(),
        1
    );
    assert!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.from == "aw")
            .all(|handoff| handoff.to.as_deref() != peer.to.as_deref())
    );
}

/// A stub's transcript lists only the handoffs the windowed model serves:
/// an older send to the same stub doesn't leave the page an entry it has
/// no handoff for.
#[test]
fn a_stubs_transcript_names_only_served_handoffs() {
    let mut home = Home::new();
    let send = |id: &str, hour: i64, message: &str| {
        [
            assistant(
                "sender",
                ts(hour, 0),
                vec![tool(
                    id,
                    "SendMessage",
                    json!({"to":"nobody-7c","message":message}),
                )],
            ),
            result(
                "sender",
                ts(hour, 0),
                id,
                "sent",
                false,
                json!({"success":true,"message":"sent"}),
            ),
        ]
    };
    let mut records = vec![human("sender", ts(1, 0), "Tell nobody")];
    records.extend(send("old", 1, "an old note"));
    // A turn of its own: the window keeps the last turn's links whole.
    records.push(human("sender", ts(4, 59), "Tell nobody again"));
    records.extend(send("new", 5, "a new note"));
    home.top("sender", &records);
    home.options.all = false;
    home.options.since = Duration::from_secs(3600);
    let built = home.build_at(&home.options, at(5, 30));
    let new = by_brief(&built, "a new note");
    assert!(
        built
            .handoffs
            .iter()
            .all(|handoff| handoff.brief != "an old note")
    );
    let stub = new.to.clone().unwrap();
    assert!(built.sessions[&stub].stub);
    let named: Vec<&str> = built.tx[&stub]
        .slots
        .iter()
        .filter_map(|slot| match &slot.kind {
            SlotKind::H(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(named, [new.id.as_str()]);
    // The page the viewer gets holds that one handoff entry.
    let page: Value = serde_json::from_str(
        &crate::tx::page(&built, &stub, &crate::tx::Anchor::Last, at(5, 30)).unwrap(),
    )
    .unwrap();
    let entries: Vec<&Value> = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["k"] == "h")
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], new.id.as_str());
}
