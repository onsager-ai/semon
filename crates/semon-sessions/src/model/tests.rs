//! One test per rule in the design's table, on synthetic homes only.

use std::{
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use super::*;
use crate::events::EventCache;

// Serialize only supported native semantic fields for field/snapshot assertions.
// Ask bodies use the same generation-bound recipes as get_session, on demand.
fn semantic_values(data: &QueryData) -> Value {
    let mut handoffs = serde_json::to_value(&data.handoffs).unwrap();
    let records = data.sources.reader();
    for handoff in handoffs.as_array_mut().unwrap() {
        if let Some(brief) = data.ask_briefs.get(handoff["id"].as_str().unwrap()) {
            handoff["brief"] = json!(brief.read(&records).unwrap());
        }
    }
    records.finish().unwrap();
    json!({"sessions":data.sessions,"handoffs":handoffs,"turns":data.turns})
}

fn native_activity(data: &QueryData, now: i64) -> BTreeMap<String, crate::analytics::Activity> {
    crate::analytics::activity(&data.sessions, &data.tx, &data.turns, &data.handoffs, now)
}

fn assistant_offsets(data: &QueryData, sid: &str) -> Vec<(usize, u64, u32)> {
    let turns = turns_of(data, sid);
    data.tx[sid]
        .slots
        .iter()
        .filter(|slot| matches!(slot.kind, SlotKind::A))
        .map(|slot| {
            let turn = turns
                .iter()
                .position(|turn| Some(turn.id.as_str()) == slot.turn.as_deref())
                .unwrap();
            (turn, slot.offset, slot.block)
        })
        .collect()
}

fn scoped_runtime_reader_oracle(options: &Options, rows: &[summary::CatalogRow]) -> Vec<String> {
    let initial = crate::native_evidence::query_runtime_generation(options).unwrap();
    let publication = crate::native_evidence::RuntimeObservations::for_publication_measured(
        options,
        &initial,
        &mut crate::native_timing::Measurements::default(),
    );
    let reader = crate::native_evidence::RuntimeObservations::new(options);
    let scopes = rows
        .iter()
        .map(|row| {
            let expected = reader.scope(row).unwrap();
            assert_eq!(publication.scope(row).unwrap(), expected);
            expected
        })
        .collect();
    assert_eq!(
        crate::native_evidence::query_runtime_generation(options).unwrap(),
        initial
    );
    scopes
}

#[test]
fn publication_runtime_scopes_match_fresh_reader_observations() {
    let home = Home::new();
    let native = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    home.top("root", &[human("root", ts(0, 0), "native root")]);
    home.live(42, "root", "working", json!({}));
    home.codex(native, json!({}), &[codex_user(ts(0, 1), "native Codex")]);
    home.lines(
        "codex/session_index.jsonl",
        &[json!({"id":native,"thread_name":"native name","updated_at":"2026-09-24T00:02:00Z"})],
    );
    home.build_query();
    let (_, rows) = read_catalog(&home);
    assert_eq!(rows.len(), 2);
    let local = scoped_runtime_reader_oracle(&home.options, &rows);
    assert!(local.iter().all(|scope| scope.starts_with("scoped-v1:")));
    for (row, scope) in rows.iter().zip(&local) {
        assert_eq!(
            row.query.as_ref().unwrap()["runtime"].as_str(),
            Some(scope.as_str())
        );
    }

    let mut facts = crate::local_runtime_facts(&home.options).unwrap();
    facts.native_names = crate::facts::MachineFacts::Local.native_names(&home.options);
    assert_eq!(facts.native_names.len(), 1);
    facts
        .repos
        .insert("/work/proj".into(), Some("recorded-repo".into()));
    let facts_path = home.root.join("scope-facts.json");
    crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
    let options = Options {
        facts: Some(facts_path.clone()),
        ..home.options.clone()
    };
    let recorded = scoped_runtime_reader_oracle(&options, &rows);
    for change in ["process-start", "name", "repository", "lock"] {
        let mut changed = facts.clone();
        match change {
            "process-start" => {
                changed.proc_starts.insert(42, 778);
            }
            "name" => changed.native_names[0].name = "updated native name".into(),
            "repository" => {
                changed
                    .repos
                    .insert("/work/proj".into(), Some("other-repo".into()));
            }
            "lock" => {
                changed.codex_locks.insert(native.into(), 42);
            }
            _ => unreachable!(),
        }
        crate::write_facts(&facts_path, &changed, &crate::Options::default().source).unwrap();
        assert_ne!(
            scoped_runtime_reader_oracle(&options, &rows),
            recorded,
            "{change}"
        );
    }
    crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
    home.live(42, "root", "waiting", json!({"waitingFor":"approval"}));
    assert_ne!(scoped_runtime_reader_oracle(&options, &rows), recorded);

    let initial = crate::native_evidence::query_runtime_generation(&options).unwrap();
    let reader = crate::native_evidence::RuntimeObservations::new(&options);
    let mut row = rows[0].clone();
    row.query = Some(json!({"runtime":initial}));
    assert_eq!(reader.expected(&row).as_deref(), Some(initial.as_str()));
    row.query = Some(json!({"runtime":"unsupported-runtime"}));
    assert_eq!(reader.expected(&row).as_deref(), Some(initial.as_str()));
    assert_ne!(
        reader.expected(&row).as_deref(),
        Some("unsupported-runtime")
    );
}

#[test]
fn publication_runtime_unavailable_inputs_keep_reader_scope_fallbacks() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "fallback root")]);
    home.build_query();
    let (_, rows) = read_catalog(&home);
    let absent_index = Options {
        cache: home.root.join("absent/index.json"),
        ..home.options.clone()
    };
    let initial = crate::native_evidence::query_runtime_generation(&absent_index).unwrap();
    assert_eq!(
        scoped_runtime_reader_oracle(&absent_index, &rows),
        vec![initial]
    );

    let unreadable = home.root.join("unreadable-facts");
    fs::create_dir(&unreadable).unwrap();
    let oversized = home.root.join("oversized-facts.json");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    for facts in [unreadable, oversized] {
        for cache in [&home.options.cache, &absent_index.cache] {
            let options = Options {
                facts: Some(facts.clone()),
                cache: cache.clone(),
                ..home.options.clone()
            };
            let scope = scoped_runtime_reader_oracle(&options, &rows);
            if cache == &home.options.cache {
                assert!(scope[0].starts_with("scoped-v1:"));
            } else {
                assert_eq!(
                    scope[0],
                    crate::native_evidence::query_runtime_generation(&options).unwrap()
                );
            }
        }
    }
}

#[test]
fn scoped_publication_rejects_runtime_change_after_initial_observation() {
    for selected in [false, true] {
        let home = Home::new();
        home.top("root", &[human("root", ts(0, 0), "runtime fenced root")]);
        let facts_path = home.root.join("changing-facts.json");
        let facts = crate::local_runtime_facts(&home.options).unwrap();
        crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
        let options = Options {
            facts: Some(facts_path.clone()),
            ..home.options.clone()
        };
        home.build_at(&options, NOW);
        let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let generation = cache.session_catalog_generation();
        let initial = crate::native_evidence::query_runtime_generation(&options).unwrap();
        AFTER_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let mut changed = facts;
                changed.hostname = "new runtime observation".into();
                crate::write_facts(&facts_path, &changed, &crate::Options::default().source)
                    .unwrap();
            }));
        });
        let error = if selected {
            prepare_sources(
                &options,
                &mut cache,
                &[crate::inputs::Input {
                    root: crate::inputs::InputRoot::Claude,
                    path: "projects/-work-proj/root.jsonl".into(),
                }],
                NOW,
            )
            .err()
            .expect("Sources must reject the changed runtime")
        } else {
            observe_catalog(&options, &mut cache, &mut false, &mut Texts::default(), NOW)
                .expect_err("Catalog must reject the changed runtime")
        };
        assert_eq!(
            error.to_string(),
            "Native Query runtime observations changed"
        );
        assert_ne!(
            crate::native_evidence::query_runtime_generation(&options).unwrap(),
            initial
        );
        assert_eq!(cache.session_catalog_generation(), generation);
        assert_eq!(read_catalog(&home).0, generation.unwrap());
    }
}

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
            source: crate::Options::default().source,
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

    fn build_query(&self) -> QueryData {
        self.build_at(&self.options, NOW)
    }

    /// Runs the production native Query producer over this synthetic home's
    /// persisted Event Index and checks shared identity/relationship invariants.
    fn build_at(&self, options: &Options, now: i64) -> QueryData {
        let mut cache = EventCache::open(&options.cache, &options.source).unwrap();
        let mut dirty = false;
        if options.facts.is_none() {
            cache
                .refresh_reported_runs(&options.claude_json, now, &mut dirty)
                .unwrap();
        }
        let built =
            build_query(options, &mut cache, &mut dirty, &mut Texts::default(), now).unwrap();
        invariants(&built, options.all || options.scan_window);
        built
    }
}

/// Holds for every native Query: handoff and turn ids are unique, and every id and
/// session a handoff or turn names exists. Every handoff a stub's transcript
/// names is served; when nothing was trimmed (`whole`), every handoff any
/// transcript names is.
fn invariants(built: &QueryData, whole: bool) {
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

fn only<'a>(built: &'a QueryData, kind: &str, from: &str, to: &str) -> &'a Handoff {
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

fn by_brief(built: &QueryData, brief: &str) -> Handoff {
    if let Some(found) = built.handoffs.iter().find(|handoff| handoff.brief == brief) {
        return found.clone();
    }
    let records = built.sources.reader();
    let found = built
        .handoffs
        .iter()
        .find(|handoff| {
            built
                .ask_briefs
                .get(&handoff.id)
                .is_some_and(|recipe| recipe.read(&records).unwrap() == brief)
        })
        .unwrap_or_else(|| panic!("no handoff with brief {brief:?}: {:?}", built.handoffs));
    records.finish().unwrap();
    found.clone()
}

fn turns_of<'a>(built: &'a QueryData, sid: &str) -> Vec<&'a Turn> {
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

    let built = home.build_query();
    assert_eq!(
        built.sessions["claude-effort"].effort.as_deref(),
        Some("max")
    );
    assert_eq!(
        built.sessions["codex-effort"].effort.as_deref(),
        Some("xhigh")
    );
    assert_eq!(built.sessions["claude-no-effort"].effort, None);
    let model: Value = semantic_values(&built);
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

    let built = home.build_query();
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

    let built = home.build_query();
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
    let model: Value = semantic_values(&built);
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

    let built = home.build_query();
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
    let model: Value = semantic_values(&built);
    assert!(
        model["sessions"]["dated-model"]["tokens_by_model"]
            .get("claude-sonnet-4-5-20250929")
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(crate::pricing::table()).unwrap()["models"]["claude-sonnet-4-5"]["input"],
        3.0
    );
}

#[test]
fn source_shaped_cross_harness_native_id_and_call_id_collisions_stay_separate() {
    let home = Home::new();
    home.write(
        "claude/projects/project/00000000-0000-4000-8000-000000000001.jsonl",
        include_str!("../../../../tests/fixtures/compatibility/v1/namespace-claude.jsonl"),
    );
    home.write(
        "codex/sessions/rollout.jsonl",
        include_str!("../../../../tests/fixtures/compatibility/v1/namespace-codex.jsonl"),
    );
    let built = home.build_query();
    assert_eq!(built.sessions.len(), 2, "native IDs are scoped by harness");
    let mut harnesses = BTreeSet::new();
    for (id, session) in &built.sessions {
        harnesses.insert(session.harness);
        assert_eq!(session.calls, Some(1));
        assert_eq!(session.errors, Some(0));
        let tools: Vec<_> = built.tx[id]
            .slots
            .iter()
            .filter_map(|slot| {
                if let SlotKind::Tool {
                    reply: Some(reply), ..
                } = &slot.kind
                {
                    Some((slot, reply))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(tools.len(), 1);
        let (slot, reply) = tools[0];
        let bytes = fs::read(&slot.file.as_ref().unwrap().path).unwrap();
        let result = std::str::from_utf8(&bytes[reply.o as usize..]).unwrap();
        let expected = if session.harness == "claude" {
            "SEMON_SYNTHETIC_CLAUDE_OUTPUT"
        } else {
            "SEMON_SYNTHETIC_CODEX_OUTPUT"
        };
        assert!(result.lines().next().unwrap().contains(expected));
    }
    assert_eq!(harnesses, BTreeSet::from(["claude", "codex"]));
}

#[test]
fn qualified_viewer_keys_do_not_shadow_native_identifiers() {
    let home = Home::new();
    home.top(
        "same-id",
        &[human("same-id", ts(0, 0), "Synthetic Claude query")],
    );
    home.codex("same-id", json!({}), &[]);
    home.top(
        "claude:same-id",
        &[human(
            "claude:same-id",
            ts(0, 0),
            "Synthetic reserved query",
        )],
    );
    let built = home.build_query();
    assert_eq!(built.sessions.len(), 3);
    assert_eq!(built.sessions["claude:same-id"].harness, "claude");
    assert_eq!(built.sessions[":claude:same-id"].harness, "claude");
    assert_eq!(built.sessions["codex:same-id"].harness, "codex");
}

#[test]
fn repeated_tool_ids_use_the_explicit_claude_parent() {
    let home = Home::new();
    for (parent, child) in [
        ("first-parent", "first-child"),
        ("second-parent", "second-child"),
    ] {
        home.top(
            parent,
            &[
                human(parent, ts(0, 0), "Synthetic namespace query"),
                assistant(
                    parent,
                    ts(0, 1),
                    vec![tool(
                        "shared-call",
                        "Agent",
                        json!({"prompt":"Synthetic child query"}),
                    )],
                ),
                result(
                    parent,
                    ts(0, 2),
                    "shared-call",
                    "Synthetic result",
                    false,
                    json!({}),
                ),
            ],
        );
        home.agent(
            parent,
            child,
            "shared-call",
            &[
                user(parent, ts(0, 1), "Synthetic child query"),
                assistant(parent, ts(0, 2), vec![text("Synthetic child response")]),
            ],
        );
    }
    home.agent(
        "missing-parent",
        "ambiguous-child",
        "shared-call",
        &[user("missing-parent", ts(0, 1), "Synthetic child query")],
    );
    let built = home.build_query();
    for (parent, child) in [
        ("first-parent", "first-child"),
        ("second-parent", "second-child"),
    ] {
        assert_eq!(built.sessions[child].parent.as_deref(), Some(parent));
    }
    assert!(built.sessions["ambiguous-child"].parent.is_none());
}

#[test]
fn nested_claude_calls_resolve_under_a_shared_storage_ancestor() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(0, 0), "Synthetic root query"),
            assistant(
                "root",
                ts(0, 1),
                vec![tool(
                    "outer-call",
                    "Agent",
                    json!({"prompt":"Synthetic middle query"}),
                )],
            ),
        ],
    );
    home.agent(
        "root",
        "middle",
        "outer-call",
        &[
            user("root", ts(0, 1), "Synthetic middle query"),
            assistant(
                "root",
                ts(0, 2),
                vec![tool(
                    "inner-call",
                    "Agent",
                    json!({"prompt":"Synthetic nested query"}),
                )],
            ),
        ],
    );
    home.agent(
        "root",
        "nested",
        "inner-call",
        &[user("root", ts(0, 2), "Synthetic nested query")],
    );
    let built = home.build_query();
    assert_eq!(built.sessions["middle"].parent.as_deref(), Some("root"));
    assert_eq!(built.sessions["nested"].parent.as_deref(), Some("middle"));
}

#[test]
fn native_codex_parents_resolve_only_within_codex() {
    let home = Home::new();
    home.top(
        "shared-parent",
        &[human("shared-parent", ts(0, 0), "Synthetic query")],
    );
    home.codex("shared-parent", json!({}), &[]);
    home.codex("child", json!({"parent_thread_id":"shared-parent"}), &[]);
    home.top(
        "claude-only",
        &[human("claude-only", ts(0, 0), "Synthetic query")],
    );
    home.codex(
        "unmatched-child",
        json!({"parent_thread_id":"claude-only"}),
        &[],
    );
    let built = home.build_query();
    assert_eq!(
        built.sessions["child"].parent.as_deref(),
        Some("codex:shared-parent")
    );
    assert!(built.sessions["unmatched-child"].parent.is_none());
}

#[test]
fn native_codex_fork_reports_only_child_owned_usage() {
    let home = Home::new();
    home.write(
        "codex/sessions/parent.jsonl",
        include_str!("../../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/fork/initial-rollout.jsonl"),
    );
    home.write(
        "codex/sessions/child.jsonl",
        include_str!("../../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/fork/forked-rollout.jsonl"),
    );
    let empty_child = home.build_query();
    assert!(
        empty_child.sessions["native-codex-fork-child"]
            .tokens_by_model
            .is_empty()
    );
    home.write(
        "codex/sessions/child.jsonl",
        include_str!("../../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/fork/child-turn-rollout.jsonl"),
    );
    let built = home.build_query();
    let child = &built.sessions["native-codex-fork-child"];
    assert_eq!(child.tokens_by_model["mock-model"].input, 5);
    assert_eq!(child.tokens_by_model["mock-model"].output, 3);
    assert_eq!(
        built.sessions["native-codex-fork-parent"].tokens_by_model["mock-model"].input,
        5
    );
    assert!(child.parent.is_none(), "a fork is not a spawned subagent");
    let metadata = serde_json::to_value(child.codex_history.as_ref().unwrap()).unwrap();
    assert_eq!(metadata["forked_from_id"], "native-codex-fork-parent");
    assert_eq!(metadata["forked_from_ordinal_exclusive"], 13);
    assert_eq!(
        metadata["history_base"]["thread_id"],
        "native-codex-fork-parent"
    );
    let restarted = home.build_query();
    assert_eq!(
        restarted.sessions["native-codex-fork-child"].tokens_by_model,
        child.tokens_by_model
    );
    // Repeated exact source identities do not turn replay into fresh requests.
    let path = home.options.codex_home.join("sessions/child.jsonl");
    let records: Vec<Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let repeated = records
        .iter()
        .find(|row| row["type"] == "token_usage_record")
        .unwrap();
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{repeated}"
    )
    .unwrap();
    assert_eq!(
        home.build_query().sessions["native-codex-fork-child"].tokens_by_model,
        child.tokens_by_model
    );
}

#[test]
fn codex_fork_cumulative_or_incomplete_reports_are_not_fresh_usage() {
    let home = Home::new();
    home.codex(
        "child",
        json!({"forked_from_id":"absent-parent"}),
        &[
            codex_line(ts(1, 0), "turn_context", json!({"model":"gpt-test"})),
            codex_line(
                ts(1, 1),
                "event_msg",
                json!({"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":10,"output_tokens":6,"total_tokens":16}}}),
            ),
            codex_line(
                ts(1, 2),
                "token_usage_record",
                json!({
                    "thread_id":"child","session_id":"child","turn_id":"t1","response_id":"r1",
                    "usage":{"input_tokens":5,"output_tokens":3,"total_tokens":8}
                }),
            ),
        ],
    );
    let built = home.build_query();
    assert!(built.sessions["child"].tokens_by_model.is_empty());
    assert!(built.sessions["child"].parent.is_none());
    assert!(
        home.build_query().sessions["child"]
            .tokens_by_model
            .is_empty()
    );
}

#[test]
fn codex_logical_fork_and_physical_history_are_independent_native_fields() {
    let home = Home::new();
    home.codex("logical", json!({}), &[]);
    home.codex("physical", json!({}), &[]);
    home.codex(
        "child",
        json!({
            "session_id":"child", "forked_from_id":"logical",
            "forked_from_ordinal_exclusive":0,
            "history_base":{"thread_id":"physical","end_ordinal_exclusive":7,"end_byte_offset":99}
        }),
        &[],
    );
    home.codex(
        "physical-only",
        json!({"history_base":{"thread_id":"physical"}}),
        &[],
    );
    let built = home.build_query();
    let history = serde_json::to_value(&built.sessions["child"].codex_history).unwrap();
    assert_eq!(history["forked_from_id"], "logical");
    assert_eq!(history["forked_from_ordinal_exclusive"], 0);
    assert_eq!(history["history_base"]["thread_id"], "physical");
    assert_eq!(history["history_base"]["end_ordinal_exclusive"], 7);
    let partial = serde_json::to_value(&built.sessions["physical-only"].codex_history).unwrap();
    assert!(partial.get("forked_from_id").is_none());
    assert!(
        partial["history_base"]
            .get("end_ordinal_exclusive")
            .is_none()
    );
    for id in ["child", "physical-only"] {
        assert!(built.sessions[id].parent.is_none());
    }
    assert!(built.sessions["logical"].codex_history.is_none());
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

    let built = home.build_query();
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

    let built = home.build_query();
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
    let model: Value = semantic_values(&built);
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

    let built = home.build_query();
    assert_eq!(
        built.sessions["unknown-model"].tokens_by_model["gpt-test"],
        crate::events::ModelTokens {
            input: 40_000,
            output: 4_000,
            cache_write: 0,
            cache_read: 10_000,
        }
    );
    let model: Value = semantic_values(&built);
    assert!(
        serde_json::to_value(crate::pricing::table()).unwrap()["models"]
            .get("gpt-test")
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(crate::pricing::table()).unwrap()["as_of"],
        serde_json::to_value(crate::pricing::table()).unwrap()["as_of"]
    );
    assert_eq!(
        serde_json::to_value(crate::pricing::table()).unwrap()["models"]["gpt-6-luna"]["cache_write"],
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
    let built = home.build_query();
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
    // A run's hand-back is its result; the notification that ends the same
    // run gives only the status, and the run ended at whichever came first.
    let six = spawn("a6");
    assert_eq!(
        (six.status, six.result.as_deref(), six.done),
        ("done", Some("six by hand-back"), Some(at(1, 13)))
    );
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
    assert_bounded_causal_previews(&home, &built, parent_turns[0]);
}

fn assert_bounded_causal_previews(home: &Home, built: &QueryData, root: &Turn) {
    let query = format!(
        "sid={}&turn={}&scope=retained_history",
        crate::viewer::percent_encode(&root.sid),
        crate::viewer::percent_encode(&root.id)
    );
    let reply = crate::session_causal_trace(&home.options, "fixture", &query);
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let trace: Value = serde_json::from_slice(&reply.body).unwrap();
    for edge in trace["handoffs"].as_array().unwrap() {
        let native = built
            .handoffs
            .iter()
            .find(|handoff| handoff.id == edge["id"])
            .unwrap();
        for (field, selector) in [("brief", "brief_session"), ("result", "result_session")] {
            let Some(owner) = edge[selector].as_str() else {
                continue;
            };
            let generation = trace["previews"][owner].as_str().unwrap_or_else(|| {
                panic!(
                    "missing preview owner {owner}: {edge}; registry {}",
                    trace["previews"]
                )
            });
            let query = format!(
                "sid={}&generation={generation}&scope=retained_history&events={}",
                crate::viewer::percent_encode(owner),
                crate::viewer::percent_encode(&json!([native.id]).to_string())
            );
            let reply = crate::session_home_details_with_mode(
                &home.options,
                "fixture",
                &query,
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
            );
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
            let details: Value = serde_json::from_slice(&reply.body).unwrap();
            let expected = if field == "brief" {
                if let Some(brief) = built.ask_briefs.get(&native.id) {
                    let records = built.sources.reader();
                    let text = brief.read(&records).unwrap();
                    records.finish().unwrap();
                    json!(text)
                } else {
                    json!(native.brief)
                }
            } else {
                json!(native.result)
            };
            assert_eq!(
                details["events"][&native.id][field], expected,
                "{} {owner} {field}",
                native.id
            );
        }
    }
}

#[test]
fn bounded_causal_previews_preserve_large_native_unicode_and_cleaned_prompt_caps() {
    for prompt in [
        "normal native text ".repeat(16000),
        format!(
            "<system-reminder>{}</system-reminder>\n{}",
            "skip internal reminder ".repeat(1000),
            "中文🚀 quoted \\\" body ".repeat(16000)
        ),
    ] {
        let home = Home::new();
        home.top(
            "large",
            &[uuid(
                human("large", ts(1, 0), &prompt),
                "large-native-event",
            )],
        );
        let built = home.build_query();
        assert!(prompt.len() > 128 * 1024);
        assert_bounded_causal_previews(&home, &built, turns_of(&built, "large")[0]);
    }
}

#[test]
fn background_bash_notifications_keep_their_position_and_spawn_notifications_stay_handoffs() {
    let home = Home::new();
    home.top(
        "parent",
        &[
            human("parent", ts(1, 0), "Fetch and delegate"),
            assistant(
                "parent",
                ts(1, 1),
                vec![
                    tool(
                        "bash",
                        "Bash",
                        json!({"command":"git fetch","run_in_background":true}),
                    ),
                    tool(
                        "agent",
                        "Agent",
                        json!({"prompt":"Review","run_in_background":true}),
                    ),
                ],
            ),
            result(
                "parent",
                ts(1, 2),
                "bash",
                "Command running in background",
                false,
                json!({}),
            ),
            result(
                "parent",
                ts(1, 2),
                "agent",
                "launched",
                false,
                json!({"status":"async_launched","agentId":"child"}),
            ),
            assistant("parent", ts(1, 3), vec![text("Waiting for results")]),
            notification("parent", ts(1, 4), "bash", "completed", "Fetch finished"),
            assistant("parent", ts(1, 5), vec![text("Fetch is done")]),
            notification("parent", ts(1, 6), "agent", "completed", "Review finished"),
        ],
    );
    home.agent(
        "parent",
        "child",
        "agent",
        &[
            user("parent", ts(1, 1), "Review"),
            assistant("parent", ts(1, 3), vec![text("Reviewing")]),
        ],
    );
    home.top(
        "other",
        &[
            human("other", ts(1, 0), "Other session"),
            notification("other", ts(1, 7), "bash", "failed", "Must not attach"),
        ],
    );
    // The second build reads the persisted index, including its background flag.
    for _ in 0..2 {
        let built = home.build_query();
        let slots = &built.tx["parent"].slots;
        let ends: Vec<_> = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| matches!(slot.kind, SlotKind::BgEnd { .. }))
            .collect();
        assert_eq!(ends.len(), 1);
        let (position, end) = ends[0];
        assert_eq!(end.t, Some(at(1, 4)));
        assert!(
            matches!(&end.kind, SlotKind::BgEnd { call, status, .. } if call == "bash" && status == "completed")
        );
        assert!(matches!(slots[position - 1].kind, SlotKind::A));
        assert_eq!(slots[position - 1].t, Some(at(1, 3)));
        assert!(matches!(slots[position + 1].kind, SlotKind::A));
        assert_eq!(slots[position + 1].t, Some(at(1, 5)));
        let call = slots
            .iter()
            .find_map(|slot| match &slot.kind {
                SlotKind::Tool { bg: Some(bg), .. } => Some(bg),
                _ => None,
            })
            .unwrap();
        assert_eq!(call.tid, "bash");
        assert_eq!(call.end.as_ref().unwrap().t, Some(at(1, 4)));
        assert!(
            built.tx["other"]
                .slots
                .iter()
                .all(|slot| !matches!(slot.kind, SlotKind::BgEnd { .. }))
        );
        let spawn = only(&built, "spawn", "parent", "child");
        assert_eq!(spawn.result.as_deref(), Some("Review finished"));
        assert_eq!(spawn.done, Some(at(1, 6)));
        assert!(
            slots
                .iter()
                .any(|slot| matches!(&slot.kind, SlotKind::H(id) if id == &spawn.id))
        );
    }
}

#[test]
fn a_running_parent_keeps_an_unreturned_subagent_working() {
    let home = Home::new();
    parent_with_agents(&home);
    home.live(10, "parent", "busy", json!({}));
    let built = home.build_query();
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
            codex_line(
                ts(3, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"failed-run"}),
            ),
            codex_user(ts(3, 0), "Do it alone"),
            codex_line(
                ts(3, 5),
                "event_msg",
                json!({"type":"task_complete","turn_id":"failed-run","error":{"message":"boom"}}),
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
    let built = home.build_query();
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
fn guardian_reviews_get_a_stable_kind_and_name_without_losing_codex_data() {
    let home = Home::new();
    let records = |minute: i64, prompt: &str, answer: &str| {
        vec![
            codex_user(ts(5, minute), prompt),
            codex_reply(ts(5, minute + 1), answer),
            codex_line(
                ts(5, minute + 2),
                "turn_context",
                json!({"model":"gpt-6-luna"}),
            ),
            codex_line(
                ts(5, minute + 3),
                "event_msg",
                json!({"type":"token_count","info":{"total_token_usage":{
                    "input_tokens":1_000,"cached_input_tokens":200,"output_tokens":50,
                    "reasoning_output_tokens":10,"total_tokens":1_060
                }}}),
            ),
        ]
    };
    home.codex(
        "codex-parent",
        json!({}),
        &records(0, "coordinate", "parent transcript"),
    );
    home.codex(
        "codex-worker",
        json!({"parent_thread_id":"codex-parent","thread_source":"subagent","agent_nickname":"worker","agent_path":"root/worker"}),
        &records(10, "implement", "worker transcript"),
    );
    home.codex(
        "guardian-canonical",
        json!({"parent_thread_id":"codex-parent","thread_source":"guardian_review","agent_nickname":"reviewer","agent_path":"root/reviewer"}),
        &records(20, "review one", "canonical review transcript"),
    );
    home.codex(
        "guardian-nested-source",
        json!({"parent_thread_id":"codex-parent","source":{"subagent":{"other":"guardian"}}}),
        &records(30, "review two", "nested-source review transcript"),
    );
    home.codex(
        "approval-review-name-only",
        json!({"parent_thread_id":"codex-parent","thread_source":"subagent","agent_nickname":"Approval review helper","agent_path":"root/helper"}),
        &records(40, "implement helper", "ordinary helper transcript"),
    );

    let built = home.build_query();
    assert_eq!(built.sessions["codex-parent"].kind, Some("Codex run"));
    for id in [
        "codex-worker",
        "guardian-canonical",
        "guardian-nested-source",
        "approval-review-name-only",
    ] {
        assert_eq!(built.sessions[id].parent.as_deref(), Some("codex-parent"));
        assert!(built.tx.contains_key(id), "{id} keeps its transcript");
        assert!(
            built.sessions[id]
                .tokens_by_model
                .contains_key("gpt-6-luna")
        );
    }
    assert_eq!(built.sessions["codex-worker"].kind, Some("Codex run"));
    assert_eq!(built.sessions["codex-worker"].name, "worker");
    for id in ["guardian-canonical", "guardian-nested-source"] {
        assert_eq!(built.sessions[id].kind, Some("Approval review"));
        assert_eq!(built.sessions[id].name, "Approval review");
    }
    assert_eq!(
        built.sessions["approval-review-name-only"].kind,
        Some("Codex run")
    );
    assert_eq!(built.sessions["approval-review-name-only"].name, "helper");
    let (_, catalog) = read_catalog(&home);
    for id in ["guardian-canonical", "guardian-nested-source"] {
        let row = catalog.iter().find(|row| row.key == id).unwrap();
        assert_eq!(row.presentation_kind.as_deref(), Some("Approval review"));
        assert_eq!(row.kind, "codex-run", "native identity kind remains stable");
        assert_eq!(
            serde_json::to_value(&row.cost).unwrap(),
            serde_json::to_value(&built.sessions[id].cost).unwrap()
        );
        assert_eq!(row.parent.as_deref(), Some("codex-parent"));
    }
    assert_ne!(
        catalog
            .iter()
            .find(|row| row.key == "approval-review-name-only")
            .unwrap()
            .presentation_kind
            .as_deref(),
        Some("Approval review")
    );
    assert_eq!(
        built.sessions["guardian-canonical"].tokens_by_model["gpt-6-luna"],
        crate::events::ModelTokens {
            input: 800,
            output: 50,
            cache_write: 0,
            cache_read: 200,
        }
    );
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
    let built = home.build_query();
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
    let native = native_marker_query(&home);
    let mut query = crate::Query::from_sources(Arc::new(crate::SessionSources::with_machines(
        vec![(String::new(), home.options.clone())],
    )));
    assert_eq!(
        query.call("get_session", &json!({"id":"lead"})).unwrap()["session"]["handoffs"],
        semantic_values(&built)["handoffs"]
    );
    assert_eq!(
        serde_json::to_value(&native.sessions).unwrap(),
        serde_json::to_value(&built.sessions).unwrap()
    );
    home.build_query();
    let appended = format!("{}\n", launch("third", ts(0, 51), command));
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(appended.as_bytes())
        .unwrap();
    let after = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
fn causal_peer_names_do_not_invent_branch_turns_or_reuse_stale_evidence() {
    let home = relay_home();
    let built = home.build_query();
    let root = turns_of(&built, "beta")[0];
    let query = format!(
        "sid=beta&turn={}&scope=retained_history",
        crate::viewer::percent_encode(&root.id)
    );
    let read = || {
        let reply = crate::session_causal_trace(&home.options, "fixture", &query);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        serde_json::from_slice::<Value>(&reply.body).unwrap()
    };
    let before = read();
    assert_eq!(before["participants"]["alpha"], "Alpha");
    assert_eq!(before["participants"].as_object().unwrap().len(), 1);
    assert!(before["sessions"].get("alpha").is_none());
    assert!(
        before["turns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|turn| turn["sid"] == "beta")
    );
    assert!(before["participants"].get("twin-1").is_none());
    // A title update is a native source publication, not an independent name
    // cache. The branch generation includes its qualified participant version.
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/alpha.jsonl");
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{}",
        json!({"type":"custom-title","customTitle":"Updated Alpha","sessionId":"alpha"})
    )
    .unwrap();
    home.build_query();
    let renamed = read();
    assert_eq!(renamed["participants"]["alpha"], "Updated Alpha");
    assert_ne!(renamed["generation"], before["generation"]);
    assert_eq!(renamed["turns"], before["turns"]);
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    db.execute(
        "UPDATE session_trace_headers SET source_generation='stale' WHERE session_key='alpha'",
        [],
    )
    .unwrap();
    let stale = read();
    assert!(stale["participants"].get("alpha").is_none());
    assert_eq!(stale["incomplete"], true);
    assert_eq!(stale["turns"], before["turns"]);
    assert_eq!(stale["sessions"], before["sessions"]);
}

#[test]
fn failed_and_denied_sends_are_failed_relays_with_no_receiver() {
    let home = relay_home();
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
    let records = built.sources.reader();
    let asks: Vec<String> = built
        .handoffs
        .iter()
        .filter(|h| h.kind == "ask")
        .map(|h| built.ask_briefs[&h.id].read(&records).unwrap())
        .collect();
    records.finish().unwrap();
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
    let built = home.build_query();
    golden("lineage", &built);
    let keys: Vec<&str> = built.sessions.keys().map(String::as_str).collect();
    assert_eq!(keys, ["bridged-1", "neighbour", "old", "pid-only", "root"]);
    let records = built.sources.reader();
    let asks = |to: &str| -> Vec<String> {
        built
            .handoffs
            .iter()
            .filter(|h| h.kind == "ask" && h.to.as_deref() == Some(to))
            .map(|h| built.ask_briefs[&h.id].read(&records).unwrap())
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
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
    let first = home.build_query();
    let path = home.root.join("claude/projects/-work-proj/beta.jsonl");
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str(&format!("{}\n", human("beta", ts(6, 30), "later ask")));
    fs::write(&path, content).unwrap();
    let second = home.build_query();
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
    let built = home.build_query();
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
    let built = home.build_query();
    let session = &built.sessions["counted"];
    // The same totals `/api/tx` reports, from the same transcript index.
    let transcript = &built.tx["counted"];
    assert_eq!((transcript.calls, transcript.errors), (2, 1));
    assert_eq!((session.calls, session.errors), (Some(2), Some(1)));
    let model: Value = semantic_values(&built);
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
    let built = home.build_query();
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
    let model: Value = semantic_values(&built);
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
    let built = home.build_query();
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

fn golden(name: &str, data: &QueryData) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/model/golden")
        .join(format!("{name}.json"));
    let previous: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let mut actual = semantic_values(data);
    // These independently preserved snapshots predate the focused usage/count
    // additions. Their explicit assertions above still cover those additions.
    for session in actual["sessions"].as_object_mut().unwrap().values_mut() {
        for field in [
            "cost",
            "reported_runs",
            "cost_check",
            "calls",
            "errors",
            "tool_calls",
        ] {
            session.as_object_mut().unwrap().remove(field);
        }
    }
    for field in ["sessions", "handoffs", "turns"] {
        assert_eq!(actual[field], previous[field], "{name} native {field}");
    }
    assert_eq!(
        serde_json::to_value(crate::pricing::table()).unwrap(),
        previous["pricing"]
    );
    assert_eq!(data.machine_id, previous["machine"]["id"]);
    let busy = events::busy_merge(
        data.sessions
            .values()
            .flat_map(|s| s.busy.iter().copied())
            .collect(),
    );
    assert_eq!(json!(busy), previous["busy"][&data.machine_id]);
}

#[test]
fn rule_goldens() {
    golden("relays", &relay_home().build_query());
    let home = Home::new();
    parent_with_agents(&home);
    golden("spawns", &home.build_query());
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
    let built = home.build_query();
    let relay = by_brief(&built, "sent before the clear");
    assert_ne!(relay.from, "a-new");
    assert!(relay.unmatched);
    assert!(built.sessions[&relay.from].stub);
    assert_eq!(built.sessions[&relay.from].name, "a-3c");
}

#[test]
fn an_unchanged_second_build_parses_no_lines() {
    let home = relay_home();
    let first = home.build_query();
    let before = events::PARSED.with(std::cell::Cell::get);
    let second = home.build_query();
    assert_eq!(events::PARSED.with(std::cell::Cell::get), before);
    assert_equivalent(&first, &second, NOW);
    let path = home.root.join("claude/projects/-work-proj/beta.jsonl");
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str(&human("beta", ts(6, 30), "one more").to_string());
    content.push('\n');
    fs::write(&path, content).unwrap();
    home.build_query();
    assert_eq!(events::PARSED.with(std::cell::Cell::get), before + 1);
}

#[test]
fn resumed_and_cold_caches_preserve_native_query_semantics() {
    let home = relay_home();
    home.build_query();
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
    home.build_query();
    append(
        &[
            result("alpha", ts(7, 1), "late", "ok", false, json!({})),
            human("alpha", ts(7, 2), "thanks"),
        ],
        false,
    );
    let warm = home.build_query();
    let mut cold = home.options.clone();
    cold.cache = home.root.join("cold/index.json");
    assert_equivalent(&warm, &home.build_at(&cold, NOW), NOW);
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
    let built = home.build_query();
    assert_eq!(
        only(&built, "spawn", "lead", "az").result.as_deref(),
        Some("über fertig")
    );
    let relay = by_brief(&built, "héllo");
    assert_eq!(built.sessions[&relay.from].name, "名前-ü");
}

#[test]
fn native_running_activity_keeps_the_recorded_start_and_qualified_liveness() {
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
    for now in [at(21, 2), at(21, 11), at(21, 31)] {
        let data = home.build_at(&home.options, now);
        assert_eq!(
            data.sessions["busy"].activity,
            Some(("Bash".into(), "cargo test -p semon".into(), 0, at(21, 1)))
        );
        assert_eq!(turns_of(&data, "busy")[0].end.why, "working");
    }
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
            codex_line(
                ts(1, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"serve-1"}),
            ),
            codex_user(ts(1, 0), "Start the server"),
            start,
            yielded(ts(1, 2), "start"),
            codex_line(
                ts(2, 0),
                "event_msg",
                json!({"type":"task_complete","turn_id":"serve-1"}),
            ),
            codex_line(
                ts(2, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"serve-2"}),
            ),
            codex_user(ts(2, 0), "Anything else?"),
            codex_reply(ts(2, 1), "No."),
            codex_line(
                ts(2, 1),
                "event_msg",
                json!({"type":"task_complete","turn_id":"serve-2"}),
            ),
            codex_line(
                ts(3, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"serve-3"}),
            ),
            codex_user(ts(3, 0), "Check on it"),
            poll,
        ],
    );
    hold_lock(&home, "serving");
    let built = home.build_query();
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
    let built = home.build_query();
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
            json!({"type":"function_call","name":"request_user_input","call_id":id,
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
            codex_line(
                ts(22, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"question-root"}),
            ),
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
    let built = home.build_query();
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
    let built = home.build_query();
    let spawn = only(&built, "spawn", "root", "solo");
    assert!(!spawn.ambiguous);
    assert_eq!(spawn.at, at(23, 1));
    assert_eq!(
        turns_of(&built, "root")[0].sent,
        std::slice::from_ref(&spawn.id)
    );
}

#[test]
fn codex_subagent_titles_describe_tasks_instead_of_nicknames() {
    let home = Home::new();
    home.codex("root", json!({}), &[]);
    for (id, path, prompt, expected) in [
        (
            "path-title",
            Some("/root/audit_parser"),
            Some("Review parsing"),
            "audit_parser",
        ),
        (
            "prompt-title",
            None,
            Some("Review tool classification\nand result correlation"),
            "Review tool classification and result correlation",
        ),
        ("empty-title", None, None, "Codex subagent"),
    ] {
        let records = prompt
            .map(|text| vec![codex_user(ts(23, 2), text)])
            .unwrap_or_default();
        home.codex(id, json!({"parent_thread_id":"root","thread_source":"subagent","agent_nickname":"Socrates","agent_path":path}), &records);
        let built = home.build_query();
        assert_eq!(built.sessions[id].name, expected);
        assert_eq!(built.sessions[id].parent.as_deref(), Some("root"));
        // Reopening the persisted index must produce the same task title.
        assert_eq!(home.build_query().sessions[id].name, expected);
    }
}

#[test]
fn codex_task_paths_take_precedence_over_nicknames_for_spawn_matching() {
    let home = Home::new();
    home.codex("root", json!({}), &[
        codex_user(ts(23, 0), "Coordinate"),
        codex_line(ts(23, 1), "response_item", json!({"type":"function_call","name":"spawn_agent","call_id":"task","arguments":"{\"task_name\":\"audit_parser\"}"})),
        codex_line(ts(23, 2), "response_item", json!({"type":"function_call","name":"spawn_agent","call_id":"nickname","arguments":"{\"task_name\":\"Socrates\"}"})),
    ]);
    home.codex("child", json!({"parent_thread_id":"root","thread_source":"subagent","agent_path":"/root/audit_parser","agent_nickname":"Socrates"}), &[codex_user(ts(23,3), "Audit the parser")]);
    let built = home.build_query();
    let spawn = only(&built, "spawn", "root", "child");
    assert!(!spawn.ambiguous);
    assert_eq!(spawn.at, at(23, 1));
    assert_eq!(built.sessions["child"].name, "audit_parser");
}

#[test]
fn codex_orphan_subagent_keeps_its_task_title_without_inventing_a_parent() {
    let home = Home::new();
    home.codex(
        "orphan",
        json!({"thread_source":"subagent","agent_nickname":"Socrates"}),
        &[codex_user(ts(23, 1), "Audit the parser")],
    );
    let built = home.build_query();
    assert_eq!(built.sessions["orphan"].name, "Audit the parser");
    assert!(built.sessions["orphan"].parent.is_none());
}

#[test]
fn overview_prompt_sender_follows_the_recorded_turn_instead_of_session_ancestry() {
    const ROOT: &str = "11111111-1111-4111-8111-111111111111";
    let home = Home::new();
    home.lines(
        "codex/session_index.jsonl",
        &[json!({"id":ROOT,"thread_name":"Coordinator label","updated_at":"2026-09-24T23:01:00Z"})],
    );
    home.codex(ROOT, json!({}), &[
        codex_user(ts(23, 0), "Coordinate"),
        codex_line(ts(23, 1), "response_item", json!({"type":"function_call","name":"spawn_agent","call_id":"task","arguments":"{\"task_name\":\"audit_parser\",\"prompt\":\"Audit the parser\"}"})),
    ]);
    let initial = codex_user(ts(23, 2), "Audit the parser");
    for (records, expected) in [
        (vec![initial.clone()], ROOT),
        (
            vec![
                initial,
                codex_reply(ts(23, 3), "Audit complete"),
                codex_user(ts(23, 4), "A direct follow-up from you"),
            ],
            "you",
        ),
    ] {
        home.codex("child", json!({"parent_thread_id":ROOT,"thread_source":"subagent","agent_path":"/root/audit_parser","agent_nickname":"Socrates"}), &records);
        let inputs = crate::inputs::inputs(&home.options)
            .unwrap()
            .into_iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .collect::<Vec<_>>();
        let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
        let prepared = prepare_sources(&home.options, &mut cache, &inputs, NOW).unwrap();
        let child = prepared.rows.iter().find(|row| row.key == "child").unwrap();
        assert_eq!(child.parent.as_deref(), Some(ROOT));
        let overview = serde_json::to_value(&child.overview).unwrap();
        assert_eq!(
            overview["home"]["detail_from"], expected,
            "ancestry must not replace the latest prompt's sender"
        );
        if expected == ROOT {
            let sender = prepared.rows.iter().find(|row| row.key == ROOT).unwrap();
            assert_eq!(sender.name, "Coordinator label");
            assert_eq!(
                overview["home"]["detail_from_name"], sender.name,
                "the source-qualified sender label must survive a bounded recent list"
            );
        } else {
            assert!(
                overview["home"]["detail_from_name"].is_null(),
                "a direct user turn must not inherit the parent's name"
            );
        }
        drop(cache);
        assert_staged_owner_parity(&home, &inputs);
    }
}

#[test]
fn codex_native_spawn_calls_place_their_exact_task() {
    let home = Home::new();
    home.codex("root", json!({}), &[
        codex_user(ts(23,0), "Coordinate"),
        codex_line(ts(23,1), "event_msg", json!({"type":"item_completed","item":{"type":"DynamicToolCall","id":"native-spawn","namespace":"collaboration","tool":"spawn_agent","arguments":{"task_name":"audit_parser"},"success":true,"status":"completed","content_items":[]}})),
    ]);
    home.codex("child", json!({"parent_thread_id":"root","thread_source":"subagent","agent_path":"/root/audit_parser","agent_nickname":"Socrates"}), &[codex_user(ts(23,2), "Audit the parser")]);
    let built = home.build_query();
    let spawn = only(&built, "spawn", "root", "child");
    assert!(!spawn.ambiguous);
    assert_eq!(spawn.at, at(23, 1));
    assert_eq!(
        turns_of(&built, "root")[0].sent,
        std::slice::from_ref(&spawn.id)
    );
}

/// Replay must enter the shared turn/slot builder before it replaces native
/// entries with placements. Compare the public persisted artifacts, including
/// native preview recipes, rather than just checking parent headers.
fn assert_staged_owner_parity(home: &Home, inputs: &[crate::Input]) {
    let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
    let full = prepare_sources(&home.options, &mut cache, inputs, NOW).unwrap();
    let serialized = serde_json::to_vec(&full.relationships).unwrap();
    let mut staged: relation_stage::Context = serde_json::from_slice(&serialized).unwrap();
    for row in &full.rows {
        staged.children.insert(
            row.key.clone(),
            serde_json::from_value(
                row.query.as_ref().unwrap()["rows"][&row.key]["summary"]["children"].clone(),
            )
            .unwrap(),
        );
    }
    for input in inputs {
        let replay = prepare_staged_sources(
            &home.options,
            &mut cache,
            std::slice::from_ref(input),
            &staged,
            false,
            NOW,
        )
        .unwrap();
        for row in &replay.rows {
            let expected = full
                .rows
                .iter()
                .find(|candidate| candidate.key == row.key)
                .unwrap();
            assert_eq!(
                serde_json::to_value(row).unwrap(),
                serde_json::to_value(expected).unwrap(),
                "{} catalog/Query",
                row.key
            );
            assert_eq!(
                format!("{:?}", row.trace),
                format!("{:?}", expected.trace),
                "{} trace",
                row.key
            );
            assert_eq!(
                serde_json::to_value(&row.overview).unwrap(),
                serde_json::to_value(&expected.overview).unwrap(),
                "{} native preview recipes",
                row.key
            );
            let actual_slots = &replay
                .transcripts
                .iter()
                .find(|projection| projection.key == row.key)
                .unwrap()
                .slots;
            let expected_slots = &full
                .transcripts
                .iter()
                .find(|projection| projection.key == row.key)
                .unwrap()
                .slots;
            assert_eq!(
                serde_json::to_value(actual_slots).unwrap(),
                serde_json::to_value(expected_slots).unwrap(),
                "{} source-qualified slots",
                row.key
            );
        }
    }
}

#[test]
fn staged_codex_native_spawn_and_relay_replay_matches_shared_builder() {
    let home = Home::new();
    home.codex("root", json!({}), &[
        codex_user(ts(4,0), "Coordinate"),
        codex_line(ts(4,1), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"spawn-worker","arguments":"{\"task_name\":\"worker\",\"message\":\"Inspect\"}"})),
        codex_line(ts(4,1), "response_item", json!({"type":"function_call_output","call_id":"spawn-worker","output":"{\"task_name\":\"worker\"}"})),
        codex_line(ts(4,3), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"send_message","call_id":"send-worker","arguments":"{\"target\":\"root/worker\",\"message\":\"Follow up\"}"})),
        codex_line(ts(4,3), "response_item", json!({"type":"function_call_output","call_id":"send-worker","output":"sent"})),
    ]);
    home.codex("worker", json!({"parent_thread_id":"root","thread_source":"subagent","agent_path":"root/worker","agent_nickname":"Socrates"}), &[
        codex_user(ts(4,2), "Inspect"),
        codex_reply(ts(4,4), "Complete"),
    ]);
    assert_staged_owner_parity(
        &home,
        &[
            crate::Input {
                root: crate::InputRoot::Codex,
                path: "sessions/2026/09/24/rollout-root.jsonl".into(),
            },
            crate::Input {
                root: crate::InputRoot::Codex,
                path: "sessions/2026/09/24/rollout-worker.jsonl".into(),
            },
        ],
    );
}

#[test]
fn staged_claude_unmatched_send_and_orphan_receipt_keep_stubs_and_native_recipes() {
    let home = Home::new();
    home.top(
        "sender",
        &[
            human("sender", ts(5, 0), "Coordinate"),
            assistant(
                "sender",
                ts(5, 1),
                vec![tool(
                    "send-absent",
                    "SendMessage",
                    json!({"to":"missing","message":"Native outgoing brief"}),
                )],
            ),
            result(
                "sender",
                ts(5, 1),
                "send-absent",
                "sent",
                false,
                json!({"success":true,"message":"sent"}),
            ),
        ],
    );
    home.top("receiver", &[
        human("receiver", ts(5,0), "Receive"),
        json!({"type":"user","timestamp":ts(5,2),"sessionId":"receiver","origin":{"kind":"peer","from":"unknown","name":"missing-worker","body":"Native receipt"},"message":{"role":"user","content":"<cross-session-message from=\"unknown\" from-name=\"missing-worker\">Native receipt</cross-session-message>"}}),
    ]);
    assert_staged_owner_parity(
        &home,
        &[
            crate::Input {
                root: crate::InputRoot::Claude,
                path: "projects/-work-proj/sender.jsonl".into(),
            },
            crate::Input {
                root: crate::InputRoot::Claude,
                path: "projects/-work-proj/receiver.jsonl".into(),
            },
        ],
    );
}

#[test]
fn resumable_rich_codex_family_survives_owner_loss_and_late_native_append() {
    let home = Home::new();
    let mut records = vec![codex_user(ts(6, 0), "Coordinate a large native family")];
    for n in 0..80 {
        let task = format!("part{n:03}");
        records.push(codex_line(ts(6,1), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":format!("spawn-{n}"),"arguments":json!({"task_name":task,"message":"Inspect"}).to_string()})));
        home.codex(&format!("child-{n:03}"), json!({"parent_thread_id":"root","thread_source":"subagent","agent_path":format!("root/{task}"),"agent_nickname":format!("worker-{n}")}), &[
            codex_user(ts(6,2), "Inspect"), codex_reply(ts(6,4), "Complete"),
        ]);
    }
    records.push(codex_line(ts(6,3), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"send_message","call_id":"far-relay","arguments":"{\"target\":\"root/part079\",\"message\":\"Follow up\"}"})));
    home.codex("root", json!({}), &records);
    home.codex("grandchild", json!({"parent_thread_id":"child-079","thread_source":"subagent","agent_path":"root/part079/deep","agent_nickname":"deep"}), &[
        codex_user(ts(6,3), "Deep work"),
        codex_line(ts(6,4), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"send_message","call_id":"deep-relay","arguments":"{\"target\":\"root/part000\",\"message\":\"Across the true root\"}"})),
    ]);
    let mut evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let mut restarted = false;
    let mut appended = false;
    let mut complete = false;
    for _ in 0..128 {
        let done = evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap();
        let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let phase: Option<String> = db
            .query_row("SELECT phase FROM catalog_semantic_stage", [], |row| {
                row.get(0)
            })
            .optional()
            .unwrap();
        if !done {
            assert!(
                crate::source_projection_ready(
                    &home.options,
                    "codex",
                    "sessions/2026/09/24/rollout-root.jsonl"
                )
                .unwrap()
                .is_none(),
                "partial epoch became archive-ready"
            );
        }
        if phase.as_deref() == Some("relations") && !restarted {
            evidence.retire();
            evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
            restarted = true;
        }
        if phase.as_deref() == Some("publication") && !appended {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(
                    home.options
                        .codex_home
                        .join("sessions/2026/09/24/rollout-root.jsonl"),
                )
                .unwrap();
            writeln!(file, "{}", codex_reply(ts(6, 5), "Late native completion")).unwrap();
            appended = true;
        } else if done {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        complete && restarted && appended,
        "durable rich family did not qualify after owner loss and append"
    );
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("oracle-index.json");
    let oracle = home.build_at(&oracle_options, NOW);
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    for handoff in oracle
        .handoffs
        .iter()
        .filter(|edge| matches!(edge.kind, "spawn" | "relay"))
    {
        for owner in [Some(&handoff.from), handoff.to.as_ref()]
            .into_iter()
            .flatten()
        {
            let metadata: String = db
                .query_row(
                    "SELECT metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
                    rusqlite::params![owner, handoff.id],
                    |row| row.get(0),
                )
                .unwrap();
            let value: Value = serde_json::from_str(&metadata).unwrap();
            assert_eq!(value["from"], json!(handoff.from));
            assert_eq!(value["to"], json!(handoff.to));
            assert_eq!(value["status"], json!(handoff.status));
            assert_eq!(value["ambiguous"], json!(handoff.ambiguous));
            assert_eq!(value["unmatched"], json!(handoff.unmatched));
        }
    }
    for turn in &oracle.turns {
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_trace_turns WHERE session_key=?1 AND turn_id=?2",
                rusqlite::params![turn.sid, turn.id],
                |row| row.get(0),
            )
            .unwrap();
        let value: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(value["start"], json!(turn.start));
        assert_eq!(value["sent"], json!(turn.sent));
        assert_eq!(value["end"], json!(turn.end));
    }
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='root' AND lifecycle='current'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(
        row["query"]["rows"]["root"]["summary"]["children"]
            .as_array()
            .unwrap()
            .len(),
        80
    );
    assert!(
        crate::source_projection_ready(
            &home.options,
            "codex",
            "sessions/2026/09/24/rollout-root.jsonl"
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn resumable_claude_peer_component_preserves_exact_messages_and_learned_name() {
    let home = Home::new();
    let mut sender = vec![human("coordinator", ts(7, 0), "Coordinate peers")];
    for n in 0..80 {
        let call = format!("send-{n:03}");
        let message = format!("message-{n:03}");
        let receiver = format!("peer-{n:03}");
        sender.push(assistant(
            "coordinator",
            ts(7, 1),
            vec![tool(
                &call,
                "SendMessage",
                json!({"to":receiver,"message":format!("native brief {n}")}),
            )],
        ));
        sender.push(result(
            "coordinator",
            ts(7, 1),
            &call,
            "sent",
            false,
            json!({"success":true,"msg_id":message}),
        ));
        let mut records = vec![peer(
            &receiver,
            ts(7, 2),
            &message,
            "Coordinator",
            100,
            &format!("native brief {n}"),
        )];
        if n == 79 {
            records.push(assistant(
                &receiver,
                ts(7, 3),
                vec![tool(
                    "legacy-return",
                    "SendMessage",
                    json!({"to":"Coordinator","message":"Return by learned unique name"}),
                )],
            ));
            records.push(result(
                &receiver,
                ts(7, 3),
                "legacy-return",
                "sent",
                false,
                json!({"success":true,"message":"sent"}),
            ));
        }
        home.top(&receiver, &records);
    }
    home.top("coordinator", &sender);
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("oracle-index.json");
    let oracle = home.build_at(&oracle_options, NOW);
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let mut complete = false;
    for _ in 0..128 {
        if evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        complete,
        "cross-batch native Claude peer component remained pending"
    );
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let expected = oracle
        .handoffs
        .iter()
        .filter(|edge| edge.kind == "relay")
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 81);
    assert!(expected.iter().all(|edge| !edge.unmatched));
    assert_eq!(db.query_row("SELECT count(DISTINCT edge_id) FROM session_trace_edges WHERE json_extract(metadata,'$.kind')='relay'",[],|row|row.get::<_,i64>(0)).unwrap(),81,"a receipt became an extra orphan edge");
    for edge in expected {
        for owner in [Some(&edge.from), edge.to.as_ref()].into_iter().flatten() {
            let metadata: String = db
                .query_row(
                    "SELECT metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
                    rusqlite::params![owner, edge.id],
                    |row| row.get(0),
                )
                .unwrap();
            let value: Value = serde_json::from_str(&metadata).unwrap();
            assert_eq!(value["from"], json!(edge.from));
            assert_eq!(value["to"], json!(edge.to));
            assert_eq!(value["unmatched"], json!(edge.unmatched));
        }
    }
}

#[test]
fn catalog_runtime_staged_parent_turn_survives_bidirectional_message_pages() {
    assert_staged_bidirectional_message_pages(80);
}

#[test]
fn catalog_runtime_staged_publication_replays_large_incident_relationships() {
    assert_staged_bidirectional_message_pages(360);
}

fn assert_staged_bidirectional_message_pages(rounds: i64) {
    let home = Home::new();
    let ts = |hour: i64, minute: i64| {
        format!(
            "2026-09-{:02}T{:02}:{minute:02}:00.000Z",
            24 + hour / 24,
            hour % 24
        )
    };
    let mut alpha = vec![human("alpha", ts(6, 0), "Coordinate a conversation")];
    // Offset the two native message pages so the same send is observed with
    // different incident subsets. Its preceding receipt still owns its turn.
    let mut beta = vec![peer("beta", ts(6, 1), "prelude", "Outside", 101, "Start")];
    for n in 0..rounds {
        let minute = 7 + n / 10;
        let second = (n % 10) * 4;
        let forward = format!("forward-{n:03}");
        let backward = format!("backward-{n:03}");
        let send = format!("send-{n:03}");
        let reply = format!("reply-{n:03}");
        alpha.push(assistant(
            "alpha",
            ts(minute, second),
            vec![tool(
                &send,
                "SendMessage",
                json!({"to":"beta","message":"Forward"}),
            )],
        ));
        alpha.push(result(
            "alpha",
            ts(minute, second),
            &send,
            "sent",
            false,
            json!({"success":true,"msg_id":forward}),
        ));
        beta.push(peer(
            "beta",
            ts(minute, second + 1),
            &forward,
            "Alpha",
            102,
            "Forward",
        ));
        beta.push(assistant(
            "beta",
            ts(minute, second + 2),
            vec![tool(
                &reply,
                "SendMessage",
                json!({"to":"alpha","message":"Backward"}),
            )],
        ));
        beta.push(result(
            "beta",
            ts(minute, second + 2),
            &reply,
            "sent",
            false,
            json!({"success":true,"msg_id":backward}),
        ));
        alpha.push(peer(
            "alpha",
            ts(minute, second + 3),
            &backward,
            "Beta",
            103,
            "Backward",
        ));
    }
    home.top("alpha", &alpha);
    home.top("beta", &beta);
    for n in 0..70 {
        let id = format!("independent-{n:03}");
        home.top(&id, &[human(&id, ts(6, 0), "Independent history")]);
    }
    let oracle = assert_queued_identity_matches_oracle(&home, &home.options);
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    for turn in &oracle.turns {
        for edge in &turn.sent {
            let handoff = oracle.handoffs.iter().find(|h| h.id == *edge).unwrap();
            for owner in [Some(turn.sid.as_str()), handoff.to.as_deref()]
                .into_iter()
                .flatten()
            {
                let actual: String = db
                    .query_row(
                        "SELECT metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
                        rusqlite::params![owner, edge],
                        |row| row.get(0),
                    )
                    .unwrap();
                let actual: Value = serde_json::from_str(&actual).unwrap();
                assert_eq!(
                    actual["source_turn"],
                    json!(turn.id),
                    "{owner}: {edge} recorded sender turn"
                );
            }
        }
    }
}

#[test]
fn late_native_claude_receipt_requalifies_previously_unmatched_sender() {
    let home = Home::new();
    home.top(
        "sender",
        &[
            human("sender", ts(7, 0), "Send before the receipt exists"),
            assistant(
                "sender",
                ts(7, 1),
                vec![tool(
                    "send-late",
                    "SendMessage",
                    json!({"to":"receiver","message":"Native late receipt"}),
                )],
            ),
            result(
                "sender",
                ts(7, 1),
                "send-late",
                "sent",
                false,
                json!({"success":true,"msg_id":"late-message"}),
            ),
        ],
    );
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let drain = || {
        for _ in 0..128 {
            if evidence
                .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
                .unwrap()
            {
                return;
            }
        }
        panic!("late native receipt publication did not finish");
    };
    drain();
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let before: String = db.query_row("SELECT metadata FROM session_trace_edges WHERE session_key='sender' AND json_extract(metadata,'$.kind')='relay'", [], |row| row.get(0)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&before).unwrap()["unmatched"],
        json!(true)
    );
    home.top(
        "receiver",
        &[peer(
            "receiver",
            ts(7, 2),
            "late-message",
            "Sender",
            100,
            "Native late receipt",
        )],
    );
    evidence.request_metadata_audit();
    drain();
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("late-receipt-oracle.json");
    let oracle = home.build_at(&oracle_options, NOW);
    let edge = oracle
        .handoffs
        .iter()
        .find(|edge| edge.kind == "relay")
        .unwrap();
    assert!(!edge.unmatched);
    assert_eq!(edge.to.as_deref(), Some("receiver"));
    for owner in [&edge.from, edge.to.as_ref().unwrap()] {
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
                rusqlite::params![owner, edge.id],
                |row| row.get(0),
            )
            .unwrap();
        let actual: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(actual["from"], json!(edge.from));
        assert_eq!(actual["to"], json!(edge.to));
        assert_eq!(actual["unmatched"], json!(false));
    }
    evidence.retire();
}

fn assert_queued_identity_matches_oracle(home: &Home, options: &Options) -> QueryData {
    let mut oracle_options = options.clone();
    oracle_options.cache = options.cache.with_extension("oracle.json");
    let oracle = home.build_at(&oracle_options, NOW);
    let mut inputs = crate::inputs(options).unwrap();
    inputs.retain(|input| input.path.ends_with(".jsonl"));
    let paths = inputs
        .iter()
        .map(|input| input.full_path(options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
    let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    // Seed one bounded operation. The ordinary producer discovers and drains
    // the remaining history; appending every page to this same unfinished
    // component would turn the per-operation source limit into a history cap.
    if let Some(page) = paths.chunks(64).next() {
        prepare_semantic_index_sources(options, &mut cache, &inputs[..page.len()], NOW).unwrap();
        cache.end_scan();
        cache.release_published_indexes();
        assert_eq!(
            cache.begin_semantic_stage(page, &runtime).unwrap(),
            crate::events::Outcome::Written
        );
    }
    let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
    crate::viewer::lock(&evidence.work).events = Some(cache);
    let mut complete = false;
    for _ in 0..512 {
        if evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        complete,
        "queued identity closure did not finish bounded steps"
    );
    let db = rusqlite::Connection::open(EventCache::path(&options.cache)).unwrap();
    for (key, session) in &oracle.sessions {
        if session.stub {
            continue;
        }
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        let row: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(row["parent"], json!(session.parent), "{key} parent");
        assert_eq!(row["tokens"], json!(session.tokens), "{key} owner usage");
        assert_eq!(row["cost"], json!(session.cost), "{key} owner cost");
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_trace_headers WHERE session_key=?1",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        let header: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(header["state"], json!(session.state), "{key} native state");
    }
    for edge in oracle
        .handoffs
        .iter()
        .filter(|edge| matches!(edge.kind, "spawn" | "relay"))
    {
        for owner in [Some(&edge.from), edge.to.as_ref()].into_iter().flatten() {
            if oracle.sessions[owner].stub {
                continue;
            }
            let metadata: String = db
                .query_row(
                    "SELECT metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
                    rusqlite::params![owner, edge.id],
                    |row| row.get(0),
                )
                .unwrap();
            let value: Value = serde_json::from_str(&metadata).unwrap();
            assert_eq!(value["from"], json!(edge.from));
            assert_eq!(value["to"], json!(edge.to));
            assert_eq!(value["status"], json!(edge.status));
            assert_eq!(value["unmatched"], json!(edge.unmatched));
            assert_eq!(value["ambiguous"], json!(edge.ambiguous));
        }
    }
    for turn in &oracle.turns {
        if oracle.sessions[&turn.sid].stub {
            continue;
        }
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_trace_turns WHERE session_key=?1 AND turn_id=?2",
                rusqlite::params![turn.sid, turn.id],
                |row| row.get(0),
            )
            .unwrap();
        let value: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(value["start"], json!(turn.start));
        assert_eq!(value["sent"], json!(turn.sent));
        assert_eq!(value["end"], json!(turn.end));
    }
    oracle
}

fn companion_fixture_before_relations(home: &Home) -> (EventCache, relation_stage::Batch) {
    let inputs = crate::inputs(&home.options).unwrap();
    assert!(inputs.len() > 64);
    let paths = inputs
        .iter()
        .map(|input| {
            input
                .full_path(&home.options)
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let runtime = crate::native_evidence::query_runtime_generation(&home.options).unwrap();
    let mut cache = EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
    for paths in paths.chunks(64) {
        assert_eq!(
            cache.begin_semantic_stage(paths, &runtime).unwrap(),
            crate::events::Outcome::Written
        );
    }
    for _ in 0..128 {
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        if batch.phase == relation_stage::Phase::Relations {
            return (cache, batch);
        }
        if batch.paths.is_empty() {
            assert!(matches!(
                batch.phase,
                relation_stage::Phase::Indexing | relation_stage::Phase::Identity
            ));
            continue;
        }
        let inputs = batch
            .paths
            .iter()
            .map(|path| crate::Input {
                root: crate::InputRoot::Claude,
                path: std::path::Path::new(path)
                    .strip_prefix(&home.options.claude_home)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            })
            .collect::<Vec<_>>();
        let prepared = match batch.phase {
            relation_stage::Phase::Indexing => {
                prepare_semantic_index_sources(&home.options, &mut cache, &inputs, NOW).unwrap()
            }
            relation_stage::Phase::Identity => prepare_semantic_identity_sources(
                &home.options,
                &mut cache,
                &inputs,
                &batch.context,
                NOW,
            )
            .unwrap(),
            _ => panic!("fixture reached publication before native relationship matching"),
        };
        assert_eq!(
            cache.record_semantic_batch(&batch, &prepared).unwrap(),
            crate::events::Outcome::Written
        );
        cache.end_scan();
        cache.release_published_indexes();
    }
    panic!("fixture identity frontier did not reach native matching");
}

/// Exercise a valid selected native matching batch at an existing durable
/// frontier. Choosing the duplicate receipt explicitly avoids having canonical
/// source ordering mask the incident-edge/certified-companion interaction.
fn record_selected_companion_batch(
    home: &Home,
    cache: &mut EventCache,
    mut batch: relation_stage::Batch,
    seed: &str,
    actors: &[&str],
    selected: &[(&str, Kind, &str)],
) -> PreparedSources {
    let inputs = actors
        .iter()
        .map(|id| crate::Input {
            root: crate::InputRoot::Claude,
            path: format!("projects/-work-proj/{id}.jsonl"),
        })
        .collect::<Vec<_>>();
    let full = prepare_sources(&home.options, cache, &inputs, NOW).unwrap();
    let mut context = full.relationships.clone();
    context.epoch = Some(batch.epoch);
    context.relations.clear();
    let mut locations = Vec::new();
    for (owner, kind, id) in selected {
        let source = full
            .rows
            .iter()
            .find(|row| row.key == *owner)
            .unwrap()
            .sources[0]
            .clone();
        let index = cache.selected_index(&source.path).unwrap();
        let (at, found) = index
            .events
            .iter()
            .enumerate()
            .find(|(_, event)| event.k == *kind && event.id.as_deref() == Some(*id))
            .unwrap();
        locations.push(relation_stage::Location {
            source,
            offset: found.o,
            block: found.b,
            kind: found.k,
            occurrence: index.events[..at]
                .iter()
                .filter(|event| event.o == found.o && event.b == found.b && event.k == found.k)
                .count(),
        });
    }
    let seed_path = home
        .options
        .claude_home
        .join(format!("projects/-work-proj/{seed}.jsonl"))
        .to_string_lossy()
        .into_owned();
    context.companions = inputs
        .iter()
        .map(|input| {
            input
                .full_path(&home.options)
                .to_string_lossy()
                .into_owned()
        })
        .filter(|path| path != &seed_path)
        .collect();
    context.matching_locations = Some(locations);
    batch.paths = vec![seed_path.clone()];
    batch.context = context;
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let after: String = db
        .query_row(
            "SELECT relation_after FROM catalog_semantic_sources WHERE source_path=?1",
            [&seed_path],
            |row| row.get(0),
        )
        .unwrap();
    let end: String = db
        .query_row(
            "SELECT max(event_key) FROM catalog_semantic_events WHERE source_path=?1",
            [&seed_path],
            |row| row.get(0),
        )
        .unwrap();
    batch.relation_cursor = Some(json!({"path":seed_path,"after":after,"end":end}).to_string());
    let prepared =
        prepare_staged_sources(&home.options, cache, &inputs, &batch.context, true, NOW).unwrap();
    assert_eq!(
        cache.record_semantic_batch(&batch, &prepared).unwrap(),
        crate::events::Outcome::Written
    );
    cache.end_scan();
    prepared
}

fn finish_companion_fixture_against_oracle(home: &Home, oracle: &QueryData) {
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let mut complete = false;
    for _ in 0..128 {
        if evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        complete,
        "native companion fixture did not finish its durable frontier"
    );
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    for edge in oracle.handoffs.iter().filter(|edge| edge.kind == "relay") {
        for owner in [Some(&edge.from), edge.to.as_ref()].into_iter().flatten() {
            let actual: String = db
                .query_row(
                    "SELECT metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2",
                    rusqlite::params![owner, edge.id],
                    |row| row.get(0),
                )
                .unwrap();
            let actual: Value = serde_json::from_str(&actual).unwrap();
            assert_eq!(actual["from"], json!(edge.from));
            assert_eq!(actual["to"], json!(edge.to));
            assert_eq!(actual["status"], json!(edge.status));
            assert_eq!(actual["unmatched"], json!(edge.unmatched));
            assert_eq!(actual["ambiguous"], json!(edge.ambiguous));
        }
    }
    for owner in [
        "zzz-sender",
        "aaa-canonical",
        "bbb-shadow",
        "aaa-selected",
        "bbb-other",
    ] {
        let Some(session) = oracle.sessions.get(owner) else {
            continue;
        };
        let expected: BTreeSet<_> = oracle
            .handoffs
            .iter()
            .filter(|edge| {
                edge.kind == "relay" && (edge.from == owner || edge.to.as_deref() == Some(owner))
            })
            .map(|edge| edge.id.clone())
            .collect();
        let actual = db.prepare("SELECT edge_id FROM session_trace_edges WHERE session_key=?1 AND json_extract(metadata,'$.kind')='relay' ORDER BY edge_id").unwrap().query_map([owner], |row| row.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<BTreeSet<_>>>().unwrap();
        assert_eq!(
            actual, expected,
            "{owner} incident edges gained an orphan or lost native evidence"
        );
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
                [owner],
                |row| row.get(0),
            )
            .unwrap();
        let row: summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
        assert_eq!(row.parent, session.parent);
        assert_eq!(row.sources.len(), 1);
        assert_eq!(crate::catalog::source_state(&row.sources[0]), "cached");
        assert!(
            crate::source_projection_ready(
                &home.options,
                "claude",
                &format!("projects/-work-proj/{owner}.jsonl")
            )
            .unwrap()
            .is_some()
        );
        let reply = crate::session_transcript_range(
            &home.options,
            "fixture",
            &format!("sid={owner}&after=0&limit=100"),
            None,
        );
        assert_eq!(
            reply.status, 200,
            "qualified native companion bytes for {owner}"
        );
        if owner == "zzz-sender" {
            // Resolved SendMessage slots render as handoff references. Their
            // native message bytes belong to the generation-qualified preview.
            let transcript: Value = serde_json::from_slice(&reply.body).unwrap();
            let outgoing = oracle
                .handoffs
                .iter()
                .filter(|edge| edge.kind == "relay" && edge.from == owner)
                .collect::<Vec<_>>();
            let briefs: BTreeSet<_> = outgoing.iter().map(|edge| edge.brief.as_str()).collect();
            let expected = if oracle.sessions.contains_key("aaa-canonical") {
                BTreeSet::from(["Canonical outgoing message"])
            } else {
                BTreeSet::from(["First outgoing message", "Second outgoing message"])
            };
            assert_eq!(briefs, expected, "independent native message oracle");
            for edge in &outgoing {
                assert!(
                    transcript["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["k"] == "h" && entry["id"] == edge.id),
                    "missing native send placement {}: {transcript}",
                    edge.id
                );
            }
            let generation = transcript["relationship_context"]["previews"][owner]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("missing native sender preview generation: {transcript}")
                });
            let ids = outgoing.iter().map(|edge| &edge.id).collect::<Vec<_>>();
            let query = format!(
                "sid={owner}&generation={generation}&scope=retained_history&events={}",
                crate::viewer::percent_encode(&json!(ids).to_string())
            );
            let details = crate::session_home_details_with_mode(
                &home.options,
                "fixture",
                &query,
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
            );
            assert_eq!(
                details.status,
                200,
                "{}",
                String::from_utf8_lossy(&details.body)
            );
            let details: Value = serde_json::from_slice(&details.body).unwrap();
            assert_eq!(details["incomplete"], false, "{details}");
            for edge in outgoing {
                assert_eq!(
                    details["events"][&edge.id]["brief"],
                    json!(edge.brief),
                    "{} native message bytes: {details}",
                    edge.id
                );
            }
        }
    }
}

#[test]
fn staged_duplicate_receipt_preserves_certified_sender_to_canonical_owner() {
    let home = Home::new();
    home.top(
        "aaa-canonical",
        &[
            peer(
                "aaa-canonical",
                ts(17, 2),
                "shared-message",
                "Coordinator",
                100,
                "Canonical receipt",
            ),
            assistant(
                "aaa-canonical",
                ts(17, 3),
                vec![tool(
                    "learned-return",
                    "SendMessage",
                    json!({"to":"Coordinator","message":"Return by learned canonical name"}),
                )],
            ),
            result(
                "aaa-canonical",
                ts(17, 3),
                "learned-return",
                "sent",
                false,
                json!({"success":true}),
            ),
        ],
    );
    home.top(
        "bbb-shadow",
        &[peer(
            "bbb-shadow",
            ts(17, 4),
            "shared-message",
            "ForbiddenShadow",
            100,
            "Later duplicate receipt",
        )],
    );
    home.top(
        "zzz-sender",
        &[
            human("zzz-sender", ts(17, 0), "Coordinate"),
            assistant(
                "zzz-sender",
                ts(17, 1),
                vec![tool(
                    "single-send",
                    "SendMessage",
                    json!({"to":"aaa-canonical","message":"Canonical outgoing message"}),
                )],
            ),
            result(
                "zzz-sender",
                ts(17, 1),
                "single-send",
                "sent",
                false,
                json!({"success":true,"msg_id":"shared-message"}),
            ),
        ],
    );
    for n in 0..70 {
        home.top(
            &format!("filler-{n:03}"),
            &[human(
                &format!("filler-{n:03}"),
                ts(17, 0),
                "Independent fixture owner",
            )],
        );
    }
    let mut options = home.options.clone();
    options.cache = home.root.join("duplicate-companion-oracle.json");
    let oracle = home.build_at(&options, NOW);
    let canonical = by_brief(&oracle, "Canonical outgoing message");
    assert_eq!(canonical.to.as_deref(), Some("aaa-canonical"));
    assert!(!canonical.unmatched);
    let learned = by_brief(&oracle, "Return by learned canonical name");
    assert_eq!(learned.to.as_deref(), Some("zzz-sender"));
    assert!(!learned.unmatched);
    let (mut cache, batch) = companion_fixture_before_relations(&home);
    let prepared = record_selected_companion_batch(
        &home,
        &mut cache,
        batch,
        "bbb-shadow",
        &["aaa-canonical", "bbb-shadow", "zzz-sender"],
        &[
            ("bbb-shadow", Kind::Xsm, "shared-message"),
            ("aaa-canonical", Kind::Xsm, "shared-message"),
            ("zzz-sender", Kind::Tool, "single-send"),
        ],
    );
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let (done, cursor): (bool,String) = db.query_row("SELECT relation_done,relation_after FROM catalog_semantic_sources WHERE source_path=?1", [home.options.claude_home.join("projects/-work-proj/zzz-sender.jsonl").to_string_lossy().as_ref()], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert!(
        done && !cursor.is_empty(),
        "complete native sender companion was not consumed"
    );
    let edge: String = db
        .query_row(
            "SELECT metadata FROM catalog_semantic_relations WHERE relation_id=?1",
            [&canonical.id],
            |row| row.get(0),
        )
        .unwrap();
    let edge: Value = serde_json::from_str(&edge).unwrap();
    assert_eq!(edge["from"], json!(canonical.from));
    assert_eq!(edge["to"], json!(canonical.to));
    assert_eq!(edge["status"], json!(canonical.status));
    let source = &prepared.relationships.endpoints["zzz-sender"].row.sources[0];
    assert_eq!(crate::catalog::source_state(source), "cached");
    assert!(
        crate::source_projection_ready(
            &home.options,
            "claude",
            "projects/-work-proj/zzz-sender.jsonl"
        )
        .unwrap()
        .is_none()
    );
    drop(db);
    drop(cache);
    finish_companion_fixture_against_oracle(&home, &oracle);
}

#[test]
fn staged_partial_native_companion_keeps_cursor_and_endpoint_pending() {
    let home = Home::new();
    home.top(
        "aaa-selected",
        &[peer(
            "aaa-selected",
            ts(18, 2),
            "first-message",
            "Coordinator",
            100,
            "First receipt",
        )],
    );
    home.top(
        "bbb-other",
        &[peer(
            "bbb-other",
            ts(18, 4),
            "second-message",
            "Coordinator",
            100,
            "Second receipt",
        )],
    );
    home.top(
        "zzz-sender",
        &[
            human("zzz-sender", ts(18, 0), "Coordinate"),
            assistant(
                "zzz-sender",
                ts(18, 1),
                vec![tool(
                    "first-send",
                    "SendMessage",
                    json!({"to":"aaa-selected","message":"First outgoing message"}),
                )],
            ),
            result(
                "zzz-sender",
                ts(18, 1),
                "first-send",
                "sent",
                false,
                json!({"success":true,"msg_id":"first-message"}),
            ),
            assistant(
                "zzz-sender",
                ts(18, 3),
                vec![tool(
                    "second-send",
                    "SendMessage",
                    json!({"to":"bbb-other","message":"Second outgoing message"}),
                )],
            ),
            result(
                "zzz-sender",
                ts(18, 3),
                "second-send",
                "sent",
                false,
                json!({"success":true,"msg_id":"second-message"}),
            ),
        ],
    );
    for n in 0..70 {
        home.top(
            &format!("filler-{n:03}"),
            &[human(
                &format!("filler-{n:03}"),
                ts(18, 0),
                "Independent fixture owner",
            )],
        );
    }
    let mut options = home.options.clone();
    options.cache = home.root.join("partial-companion-oracle.json");
    let oracle = home.build_at(&options, NOW);
    assert_eq!(
        by_brief(&oracle, "First outgoing message").to.as_deref(),
        Some("aaa-selected")
    );
    assert_eq!(
        by_brief(&oracle, "Second outgoing message").to.as_deref(),
        Some("bbb-other")
    );
    let (mut cache, batch) = companion_fixture_before_relations(&home);
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM catalog_semantic_endpoints WHERE session_key='zzz-sender'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let (done, cursor): (bool,String) = db.query_row("SELECT relation_done,relation_after FROM catalog_semantic_sources WHERE source_path=?1", [home.options.claude_home.join("projects/-work-proj/zzz-sender.jsonl").to_string_lossy().as_ref()], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert!(!done && cursor.is_empty());
    record_selected_companion_batch(
        &home,
        &mut cache,
        batch,
        "aaa-selected",
        &["aaa-selected", "zzz-sender"],
        &[
            ("aaa-selected", Kind::Xsm, "first-message"),
            ("zzz-sender", Kind::Tool, "first-send"),
        ],
    );
    let after: (bool,String) = db.query_row("SELECT relation_done,relation_after FROM catalog_semantic_sources WHERE source_path=?1", [home.options.claude_home.join("projects/-work-proj/zzz-sender.jsonl").to_string_lossy().as_ref()], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(
        after,
        (done, cursor),
        "partial native call roster consumed its companion cursor"
    );
    let after: String = db
        .query_row(
            "SELECT metadata FROM catalog_semantic_endpoints WHERE session_key='zzz-sender'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, metadata,
        "partial native call roster refined its companion endpoint"
    );
    assert!(
        crate::source_projection_ready(
            &home.options,
            "claude",
            "projects/-work-proj/zzz-sender.jsonl"
        )
        .unwrap()
        .is_none()
    );
    drop(db);
    drop(cache);
    finish_companion_fixture_against_oracle(&home, &oracle);
}

#[test]
fn staged_identity_reprepares_copy_resume_split_across_source_pages() {
    let home = Home::new();
    let original = vec![
        uuid(
            human("000-original", ts(8, 0), "Before resume"),
            "copied-user",
        ),
        uuid(
            assistant(
                "000-original",
                ts(8, 1),
                vec![tool(
                    "copy-send",
                    "SendMessage",
                    json!({"to":"absent","message":"Only one native send"}),
                )],
            ),
            "copied-send",
        ),
        uuid(
            result(
                "000-original",
                ts(8, 1),
                "copy-send",
                "sent",
                false,
                json!({"success":true}),
            ),
            "copied-result",
        ),
    ];
    let mut old = original.clone();
    old.push(json!({"type":"continued-in","continuedInSessionId":"zzz-resumed","sessionId":"000-original","timestamp":ts(8,2)}));
    home.top("000-original", &old);
    let mut copied = original;
    copied.push(uuid(
        human("zzz-resumed", ts(8, 3), "After resume"),
        "resumed-user",
    ));
    home.top("zzz-resumed", &copied);
    for n in 0..70 {
        home.top(
            &format!("middle-{n:03}"),
            &[human(&format!("middle-{n:03}"), ts(8, 0), "Independent")],
        );
    }
    let oracle = assert_queued_identity_matches_oracle(&home, &home.options);
    assert!(oracle.sessions.contains_key("000-original"));
    assert!(!oracle.sessions.contains_key("zzz-resumed"));
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='000-original'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(row["sources"].as_array().unwrap().len(), 2);
    assert_eq!(row["native_ids"], json!(["000-original", "zzz-resumed"]));
    assert_eq!(db.query_row("SELECT count(DISTINCT edge_id) FROM session_trace_edges WHERE json_extract(metadata,'$.kind')='relay'",[],|row|row.get::<_,i64>(0)).unwrap(),1,"received/copied policy emitted duplicate send");
}

#[test]
fn staged_native_key_collisions_match_global_harness_and_reserved_id_contract() {
    let home = Home::new();
    home.top("same", &[human("same", ts(9, 0), "Claude identity")]);
    home.top(
        "codex:same",
        &[human("codex:same", ts(9, 0), "Reserved native identity")],
    );
    for n in 0..70 {
        home.top(
            &format!("middle-{n:03}"),
            &[human(&format!("middle-{n:03}"), ts(9, 0), "Independent")],
        );
    }
    home.codex("same", json!({}), &[codex_user(ts(9, 1), "Codex identity")]);
    let oracle = assert_queued_identity_matches_oracle(&home, &home.options);
    assert!(oracle.sessions.contains_key("claude:same"));
    assert!(oracle.sessions.contains_key(":codex:same"));
    assert!(oracle.sessions.contains_key("codex:same"));
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    for (key, harness, native) in [
        ("claude:same", "claude", "same"),
        (":codex:same", "codex", "same"),
        ("codex:same", "claude", "codex:same"),
    ] {
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        let row: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(row["harness"], json!(harness));
        assert_eq!(row["native_ids"], json!([native]));
    }
}

#[test]
fn staged_deep_claude_spawn_uses_received_true_root_liveness() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(10, 0), "Start nested work"),
            assistant(
                "root",
                ts(10, 1),
                vec![tool("outer-call", "Agent", json!({"prompt":"Outer work"}))],
            ),
            result(
                "root",
                ts(10, 1),
                "outer-call",
                "launched",
                false,
                json!({"status":"async_launched"}),
            ),
        ],
    );
    home.agent(
        "root",
        "outer",
        "outer-call",
        &[
            user("root", ts(10, 2), "Outer work"),
            assistant(
                "root",
                ts(10, 3),
                vec![tool("inner-call", "Agent", json!({"prompt":"Inner work"}))],
            ),
            result(
                "root",
                ts(10, 3),
                "inner-call",
                "launched",
                false,
                json!({"status":"async_launched"}),
            ),
        ],
    );
    home.agent(
        "outer",
        "inner",
        "inner-call",
        &[user("outer", ts(10, 4), "Inner work")],
    );
    home.live(77, "root", "busy", json!({}));
    for n in 0..70 {
        home.top(
            &format!("middle-{n:03}"),
            &[human(&format!("middle-{n:03}"), ts(10, 0), "Independent")],
        );
    }
    let (_copy, options) = copy_with_facts(&home);
    let oracle = assert_queued_identity_matches_oracle(&home, &options);
    assert_eq!(oracle.sessions["inner"].state, "work");
    assert_eq!(only(&oracle, "spawn", "outer", "inner").status, "work");
}

#[test]
fn staged_claude_ambiguity_and_received_copy_policy_match_shared_builder() {
    let home = relay_home();
    let beta = home
        .options
        .claude_home
        .join("projects/-work-proj/beta.jsonl");
    let mut resumed = fs::read_to_string(&beta)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    resumed.push(uuid(
        human("zzz-beta", ts(6, 1), "After received copy"),
        "after-copy",
    ));
    home.top("zzz-beta", &resumed);
    let mut original = fs::OpenOptions::new().append(true).open(&beta).unwrap();
    writeln!(original,"{}",json!({"type":"continued-in","continuedInSessionId":"zzz-beta","sessionId":"beta","timestamp":ts(6,1)})).unwrap();
    home.top(
        "logged-root",
        &[human("logged-root", ts(5, 0), "Unresolved exact call")],
    );
    for owner in ["call-a", "call-b"] {
        home.top(
            owner,
            &[
                human(owner, ts(5, 0), "Duplicate native call"),
                assistant(
                    owner,
                    ts(5, 1),
                    vec![tool(
                        "duplicate-native-call",
                        "Agent",
                        json!({"prompt":"Ambiguous native ID"}),
                    )],
                ),
            ],
        );
    }
    home.agent(
        "logged-root",
        "ambiguous-child",
        "duplicate-native-call",
        &[user("logged-root", ts(5, 2), "Unresolved child")],
    );
    for n in 0..70 {
        home.top(
            &format!("middle-{n:03}"),
            &[human(&format!("middle-{n:03}"), ts(5, 0), "Independent")],
        );
    }
    let oracle = assert_queued_identity_matches_oracle(&home, &home.options);
    assert!(by_brief(&oracle, "to a twin").unmatched);
    assert_eq!(
        by_brief(&oracle, "by unique name").to.as_deref(),
        Some("beta")
    );
    assert_eq!(
        oracle
            .handoffs
            .iter()
            .filter(|edge| edge.kind == "relay" && edge.brief == "hello beta")
            .count(),
        1
    );
    assert_eq!(
        only(&oracle, "spawn", "logged-root", "ambiguous-child").from,
        "logged-root"
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
    let model: Value = semantic_values(&built);
    assert_eq!(model["sessions"]["stuck"]["activity"], Value::Null);
    assert_eq!(
        model["sessions"]["running"]["activity"],
        json!(["Bash", "make", 0, at(23, 3)])
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
    let built = home.build_query();
    let content = fs::read_to_string(&path).unwrap();
    let offset = |needle: &str| -> u64 {
        let at = content.find(needle).unwrap();
        u64::try_from(content[..at].rfind('\n').map_or(0, |line| line + 1)).unwrap()
    };
    // All four texts are in the one turn, in order.
    assert_eq!(
        assistant_offsets(&built, "chatty"),
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
    assert_eq!(assistant_offsets(&built, "run").len(), 2);
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
    crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
    options.facts = Some(path);
    (copy, options)
}

/// Every session's last transcript page, as served.
fn pages(built: &QueryData) -> Vec<String> {
    built
        .tx
        .keys()
        .map(|sid| crate::tx::tests::page(built, sid, &crate::tx::Anchor::Last, NOW).unwrap())
        .collect()
}

fn assert_mirrors(home: &Home) {
    let original = home.build_query();
    let (_copy, options) = copy_with_facts(home);
    let mirrored = home.build_at(&options, NOW);
    assert_eq!(semantic_values(&mirrored), semantic_values(&original));
    assert_eq!(mirrored.facts, original.facts);
    assert_eq!(mirrored.machine_id, original.machine_id);
    assert_eq!(pages(&mirrored), pages(&original), "the transcripts");
}

#[cfg(unix)]
#[test]
fn copied_inputs_and_facts_preserve_native_query_semantics() {
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
        &[
            codex_line(
                ts(18, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"copied-owned-turn"}),
            ),
            codex_user(ts(18, 0), "run the suite"),
        ],
    );
    hold_lock(&home, "root");
    // Never an input.
    home.write("claude/sessions/secret.key", "NEVER");

    let original = home.build_query();
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
        codex_rollouts: None,
        codex_provisional_rollouts: None,
        mirror_observation_id: None,
        mirror_sequence: None,
        repos: BTreeMap::new(),
        offline_since: None,
        runs: BTreeMap::new(),
        reported_runs: Vec::new(),
        native_names: Vec::new(),
        process_ancestors: BTreeMap::new(),
    };
    crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
    let mut options = home.options.clone();
    options.facts = Some(path.clone());
    let live = home.build_at(&options, NOW);
    assert_eq!(live.machine_id, "laptop");
    assert_eq!(live.sessions["reader"].state, "work");
    assert!(pages(&live).concat().contains("~/notes.md"));
    // Offline: the same machine, nothing running.
    crate::write_facts(
        &path,
        &facts.offline(1_790_000_000_000),
        &crate::Options::default().source,
    )
    .unwrap();
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
    let first = home.build_query();
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
    let second = home.build_query();
    assert_eq!(second.sessions["run-one"].reported_runs.len(), 1);
    assert_eq!(second.sessions["run-one"].cost_check[0].ok, Some(true));
    assert_eq!(second.sessions["run-two"].reported_runs.len(), 1);
    assert_eq!(second.sessions["run-two"].cost_check[0].ok, Some(false));

    let served_json = semantic_values(&second);
    let served = served_json.to_string();
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
fn recorded_codex_selection_distinguishes_absent_and_empty_without_pruning() {
    let home = Home::new();
    home.codex(
        "retained",
        json!({}),
        &[codex_user(ts(18, 0), "retained fixture")],
    );
    let source = home
        .root
        .join("codex/sessions/2026/09/24/rollout-retained.jsonl");
    let bytes = fs::read(&source).unwrap();
    let path = home.root.join("facts.json");
    let mut facts = crate::local_facts(&home.options).unwrap();
    facts.codex_rollouts = None;
    crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
    let mut options = home.options.clone();
    options.facts = Some(path.clone());
    assert!(
        home.build_at(&options, NOW)
            .sessions
            .contains_key("retained")
    );
    facts.codex_rollouts = Some(BTreeSet::new());
    crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
    assert!(
        !home
            .build_at(&options, NOW)
            .sessions
            .contains_key("retained")
    );
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn archived_codex_root_links_are_not_viewing_inputs() {
    let home = Home::new();
    let outside = Home::new();
    outside.codex(
        "outside-archive",
        json!({}),
        &[codex_user(ts(18, 0), "private fixture")],
    );
    fs::create_dir_all(&home.options.codex_home).unwrap();
    std::os::unix::fs::symlink(
        outside.options.codex_home.join("sessions"),
        home.options.codex_home.join("archived_sessions"),
    )
    .unwrap();
    assert!(crate::inputs(&home.options).unwrap().is_empty());
    assert!(!home.build_query().sessions.contains_key("outside-archive"));
}

#[test]
fn archived_codex_rollouts_keep_history_and_input_copy_parity() {
    let home = Home::new();
    home.codex(
        "archived-root",
        json!({}),
        &[codex_user(ts(18, 0), "archive fixture")],
    );
    let active = home
        .root
        .join("codex/sessions/2026/09/24/rollout-archived-root.jsonl");
    let before = home.build_query();
    let before_pages = pages(&before);
    let archived = home
        .root
        .join("codex/archived_sessions/rollout-archived-root.jsonl");
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::rename(&active, &archived).unwrap();
    let bytes = fs::read(&archived).unwrap();
    let after = home.build_query();
    assert!(after.sessions.contains_key("archived-root"));
    assert_eq!(before_pages, pages(&after));
    assert!(
        crate::inputs(&home.options)
            .unwrap()
            .iter()
            .any(|input| input.path == "archived_sessions/rollout-archived-root.jsonl")
    );
    assert_mirrors(&home);
    assert_eq!(fs::read(&archived).unwrap(), bytes);
    assert!(!active.exists());
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
        ("codex", "archived_sessions/rollout-x.jsonl"),
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
        ("codex", "archived_sessions/.jsonl"),
        ("codex", "archived_sessions/auth.json"),
        ("codex", "archived_sessions/r.jsonl.zst"),
        ("codex", "archived_sessions/r.jsonl.seal/r.jsonl"),
        ("claude", "projects/p/x.jsonl.seal/y.jsonl"),
        (
            "claude",
            "projects/p/x.jsonl.seal/subagents/agent-a.meta.json",
        ),
        ("codex", "sessions/2026/r.jsonl.seal/r.jsonl"),
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
    let built = home.build_query();
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

/// The active native Analytics aggregator over this fixture's complete Query evidence.
fn week(built: &QueryData) -> Value {
    let activity = native_activity(built, NOW);
    let rows = crate::analytics::rows([("testbox", &activity)]);
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
    let built = home.build_query();
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
    let built = home.build_query();
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
            json!({"type":"function_call","name":"request_user_input","call_id":id,
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
            codex_line(
                ts(13, 0),
                "event_msg",
                json!({"type":"task_started","turn_id":"open-question-turn"}),
            ),
            codex_user(ts(13, 0), "Go"),
            ask(13, 1, "open", "Still there?"),
            reply(13, 1, "open", "{\"accepted\":true}"),
        ],
    );
    hold_lock(&home, "cx-open");
    let built = home.build_query();
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

#[test]
fn wait_edges_keep_only_exact_targets_and_recorded_intervals_with_turn_identity() {
    let home = Home::new();
    home.codex("root",json!({}), &[
        codex_user(ts(10,0),"Run the child"),
        codex_line(ts(10,1),"response_item",json!({"type":"function_call","name":"wait","call_id":"targeted", "arguments":"{\"receiver_ids\":[\"child\",\"unknown\"]}"})),
        codex_line(ts(10,2),"response_item",json!({"type":"function_call_output","call_id":"targeted","output":"{}"})),
        codex_line(ts(10,3),"response_item",json!({"type":"function_call","name":"wait_agent","call_id":"untargeted", "arguments":"{\"timeout_ms\":1000}"})),
        codex_line(ts(10,4),"response_item",json!({"type":"function_call_output","call_id":"untargeted","output":"{}"})),
        codex_reply(ts(10,5),"Finished"),
    ]);
    home.codex(
        "child",
        json!({"parent_thread_id":"root"}),
        &[codex_user(ts(10, 0), "Work")],
    );
    let built = home.build_query();
    let waits = &built.sessions["root"].wait_edges;
    assert_eq!(waits.len(), 2);
    assert_eq!(waits[0].targets, vec!["child"]);
    assert_eq!((waits[0].start, waits[0].end), (at(10, 1), Some(at(10, 2))));
    assert!(waits[1].targets.is_empty());
    let turn = &turns_of(&built, "root")[0];
    assert_eq!(waits[0].turn.as_deref(), Some(turn.id.as_str()));
    assert_eq!(turn.end.at, Some(at(10, 5)));
}

#[cfg(unix)]
#[test]
fn local_ancestry_is_exact_and_environment_overrides_it_without_retaining_other_variables() {
    let home = Home::new();
    home.top("ancestor", &[human("ancestor", ts(10, 0), "launch")]);
    home.live(10, "ancestor", "busy", json!({}));
    home.codex("explicit", json!({}), &[codex_user(ts(10, 0), "parent")]);
    home.codex("child", json!({}), &[codex_user(ts(10, 1), "child")]);
    hold_lock(&home, "child");
    let mut fields = vec!["0"; 20];
    fields[1] = "10";
    fields[19] = "777";
    home.write(
        "proc/4242/stat",
        &format!("4242 (codex with parentheses) {}\n", fields.join(" ")),
    );
    let ancestor = home.build_query();
    assert_eq!(
        ancestor.sessions["child"].parent.as_deref(),
        Some("ancestor")
    );
    let captured = crate::local_facts(&home.options).unwrap();
    assert_eq!(captured.process_ancestors[&4242], vec![10]);
    home.write(
        "proc/4242/environ",
        "SEMON_PARENT=codex:explicit\0TOKEN=secret-never-retained\0",
    );
    let explicit = home.build_query();
    assert_eq!(
        explicit.sessions["child"].parent.as_deref(),
        Some("explicit")
    );
    assert_eq!(
        explicit.sessions["child"].parent_source.as_deref(),
        Some("environment")
    );
    let captured = crate::local_facts(&home.options).unwrap();
    assert_eq!(
        captured.runs[&4242],
        BTreeMap::from([("SEMON_PARENT".into(), "codex:explicit".into())])
    );
}

#[test]
fn process_ancestry_links_every_harness_pair_and_survives_process_exit() {
    for (parent_harness, child_harness) in
        [("claude", "codex"), ("codex", "claude"), ("codex", "codex")]
    {
        let home = Home::new();
        let mut facts = crate::Facts {
            version: crate::FACTS_VERSION,
            hostname: "machine".into(),
            process_ancestors: BTreeMap::from([(20, vec![10])]),
            ..Default::default()
        };
        for (id, harness, pid) in [("parent", parent_harness, 10), ("child", child_harness, 20)] {
            if harness == "claude" {
                home.top(id, &[human(id, ts(10, 0), "secret prompt never cached")]);
                home.live(pid, id, "busy", json!({}));
                facts.proc_starts.insert(pid, 777);
            } else {
                home.codex(
                    id,
                    json!({}),
                    &[codex_user(ts(10, 0), "secret prompt never cached")],
                );
                facts.codex_locks.insert(id.into(), pid);
            }
        }
        let path = home.root.join("facts.json");
        crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
        let mut options = home.options.clone();
        options.facts = Some(path.clone());
        let built = home.build_at(&options, NOW);
        assert_eq!(
            built.sessions["child"].parent.as_deref(),
            Some("parent"),
            "{parent_harness}->{child_harness}"
        );
        assert_eq!(
            built.sessions["child"].parent_source.as_deref(),
            Some("ancestry")
        );
        let journal =
            fs::read_to_string(options.cache.with_extension("parent-links.json")).unwrap();
        assert!(!journal.contains("secret prompt"));
        facts.proc_starts.clear();
        facts.codex_locks.clear();
        facts.process_ancestors.clear();
        crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
        assert_eq!(
            home.build_at(&options, NOW).sessions["child"]
                .parent
                .as_deref(),
            Some("parent")
        );
    }
}

#[test]
fn a_general_marker_overrides_environment_and_native_parent_and_cycles_stay_unlinked() {
    let home = Home::new();
    home.codex(
        "native",
        json!({}),
        &[codex_user(ts(10, 0), "native parent")],
    );
    home.codex(
        "environment",
        json!({}),
        &[codex_user(ts(10, 0), "env parent")],
    );
    home.codex(
        "marked",
        json!({}),
        &[codex_user(ts(10, 0), "marker parent")],
    );
    home.codex(
        "child",
        json!({"parent_thread_id":"native"}),
        &[codex_user(ts(10, 1), "Semon-Parent: codex:marked\nWork")],
    );
    home.top(
        "claude-child",
        &[human(
            "claude-child",
            ts(10, 1),
            "Semon-Parent: codex:marked\nWork",
        )],
    );
    home.codex(
        "cycle-a",
        json!({}),
        &[codex_user(ts(10, 0), "Semon-Parent: codex:cycle-b\nWork")],
    );
    home.codex(
        "cycle-b",
        json!({}),
        &[codex_user(ts(10, 0), "Semon-Parent: codex:cycle-a\nWork")],
    );
    let facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "machine".into(),
        codex_locks: BTreeMap::from([("child".into(), 20)]),
        runs: BTreeMap::from([(
            20,
            BTreeMap::from([("SEMON_PARENT".into(), "codex:environment".into())]),
        )]),
        ..Default::default()
    };
    let path = home.root.join("facts.json");
    crate::write_facts(&path, &facts, &crate::Options::default().source).unwrap();
    let mut options = home.options.clone();
    options.facts = Some(path);
    let built = home.build_at(&options, NOW);
    assert_eq!(built.sessions["child"].parent.as_deref(), Some("marked"));
    assert_eq!(
        built.sessions["claude-child"].parent.as_deref(),
        Some("marked")
    );
    assert_eq!(
        built.sessions["child"].parent_source.as_deref(),
        Some("marker")
    );
    assert!(built.sessions["cycle-a"].parent.is_none());
    assert!(built.sessions["cycle-b"].parent.is_none());
    let tree = crate::collect(&options).unwrap();
    let marked = tree.iter().find(|node| node.id == "marked").unwrap();
    assert!(
        marked.children.iter().any(
            |node| node.id == "claude-child" && node.parent_source.as_deref() == Some("marker")
        )
    );
    assert!(marked.children.iter().any(|node| node.id == "child"));
    assert!(tree.iter().any(|node| node.id == "cycle-a"));
    assert!(tree.iter().any(|node| node.id == "cycle-b"));
}

fn native_marker_query(home: &Home) -> QueryData {
    build_query(
        &home.options,
        &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
        &mut false,
        &mut Texts::default(),
        NOW,
    )
    .unwrap()
}

#[test]
fn native_query_restart_uses_verified_markers_and_launches_without_direct_scans() {
    let home = Home::new();
    let mut parent = vec![human("parent", ts(10, 0), "parent task")];
    parent.extend((0..1024).map(|_| json!({"type":"progress","padding":"x".repeat(1024)})));
    parent.push(assistant(
        "parent",
        ts(10, 1),
        vec![tool(
            "launch",
            "Bash",
            json!({"command":"codex exec < /private/prompt"}),
        )],
    ));
    let path = home.top("parent", &parent);
    let original = fs::read(&path).unwrap();
    home.codex(
        "child",
        json!({}),
        &[codex_user(
            ts(10, 2),
            "Semon-Parent: claude:parent\nSemon-Handoff: /private/prompt",
        )],
    );
    let marker_reads = crate::FIRST_MARKER_READS.with(std::cell::Cell::get);
    let direct_reads = DIRECT_SOURCE_READS.with(std::cell::Cell::get);
    for _ in 0..2 {
        let before = crate::native_record::READ_BYTES.with(std::cell::Cell::get);
        let built = home.build_query();
        assert_eq!(built.sessions["child"].parent.as_deref(), Some("parent"));
        assert_eq!(only(&built, "spawn", "parent", "child").at, at(10, 1));
        assert!(crate::native_record::READ_BYTES.with(std::cell::Cell::get) - before < 16 * 1024);
        assert_eq!(
            crate::FIRST_MARKER_READS.with(std::cell::Cell::get),
            marker_reads
        );
        assert_eq!(DIRECT_SOURCE_READS.with(std::cell::Cell::get), direct_reads);
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn native_query_rewrites_refuse_selected_bodies_and_stale_catalog_publication() {
    let _clock = crate::events::fixed_change_time_for_test(0);
    for after_derivation in [false, true] {
        for (from, to) in [
            ("EVIDENCE-A", "EVIDENCE-B"),
            ("Bash", "Task"),
            ("10:01:00", "10:03:00"),
        ] {
            let home = Home::new();
            let path = home.top(
                "parent",
                &[
                    human("parent", ts(10, 0), "EVIDENCE-A"),
                    assistant(
                        "parent",
                        ts(10, 1),
                        vec![tool(
                            "launch",
                            "Bash",
                            json!({"command":"codex exec < /private/prompt"}),
                        )],
                    ),
                ],
            );
            home.codex(
                "child",
                json!({}),
                &[codex_user(
                    ts(10, 2),
                    "Semon-Parent: claude:parent\nSemon-Handoff: /private/prompt",
                )],
            );
            home.build_query();
            let mut cache =
                EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
            let generation = cache.session_catalog_generation();
            let mutation: Box<dyn FnOnce()> = Box::new(move || {
                let before = fs::metadata(&path).unwrap();
                let original = fs::read_to_string(&path).unwrap();
                assert!(original.contains(from));
                let rewritten = original.replace(from, to);
                assert_eq!(rewritten.len(), original.len());
                fs::write(&path, rewritten).unwrap();
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(before.modified().unwrap())
                    .unwrap();
                assert_eq!(
                    crate::events::Stat::from_metadata(&before).unwrap(),
                    crate::events::Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap()
                );
            });
            if after_derivation {
                AFTER_QUERY_DERIVATION.with(|hook| *hook.borrow_mut() = Some(mutation));
            } else {
                AFTER_SCAN.with(|hook| *hook.borrow_mut() = Some(mutation));
            }
            if let Ok(data) = build_query(
                &home.options,
                &mut cache,
                &mut false,
                &mut Texts::default(),
                NOW,
            ) {
                // Native Query stores ask offsets, so a content-only rewrite
                // may first be observed when that selected body is requested.
                let records = data.sources.reader();
                assert!(
                    data.ask_briefs
                        .values()
                        .next()
                        .unwrap()
                        .read(&records)
                        .is_err(),
                    "{after_derivation}: {from}"
                );
                assert!(records.finish().is_err());
            }
            assert_eq!(cache.session_catalog_generation(), generation);
        }
    }
}

#[test]
fn native_handoff_restart_reads_candidates_and_repairs_locations_without_event_replay() {
    let home = Home::new();
    let mut parent = vec![human("parent", ts(10, 0), "parent task")];
    parent.extend((0..1024).map(|_| json!({"type":"progress","padding":"x".repeat(1024)})));
    parent.push(assistant(
        "parent",
        ts(10, 1),
        vec![tool(
            "launch",
            "Bash",
            json!({"command":"codex exec < /private/SECRET-PROMPT"}),
        )],
    ));
    let path = home.top("parent", &parent);
    let original = fs::read(&path).unwrap();
    home.codex(
        "child",
        json!({}),
        &[codex_user(
            ts(10, 2),
            "Semon-Parent: claude:parent\nSemon-Handoff: /private/SECRET-PROMPT",
        )],
    );
    for _ in 0..2 {
        let bytes = crate::native_record::READ_BYTES.with(std::cell::Cell::get);
        let native = native_marker_query(&home);
        assert_eq!(native.sessions["child"].parent.as_deref(), Some("parent"));
        assert_eq!(
            native
                .handoffs
                .iter()
                .find(|handoff| handoff.to.as_deref() == Some("child"))
                .unwrap()
                .at,
            at(10, 1)
        );
        assert!(
            crate::native_record::READ_BYTES.with(std::cell::Cell::get) - bytes < 16 * 1024,
            "native evidence reads must not scan the unrelated parent prefix"
        );
    }
    let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let indexed = crate::events::PARSED.with(std::cell::Cell::get);
    let rows: Vec<(String, String)> = connection
        .prepare("SELECT key,value FROM meta WHERE key GLOB 'native_launch_calls:*' ORDER BY key")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(rows.iter().all(|(_, value)| !value.contains("SECRET")
        && !value.contains("command")
        && !value.contains("private")));
    for mutation in 0..4 {
        let key = rows
            .iter()
            .find(|(key, _)| key.ends_with("0000000000"))
            .unwrap()
            .0
            .clone();
        match mutation {
            0 => {
                connection
                    .execute("DELETE FROM meta WHERE key=?1", [&key])
                    .unwrap();
            }
            1 => {
                connection
                    .execute("UPDATE meta SET value='{' WHERE key=?1", [&key])
                    .unwrap();
            }
            2 => {
                connection
                    .execute(
                        "UPDATE meta SET value=json_set(value,'$.version',99) WHERE key=?1",
                        [&key],
                    )
                    .unwrap();
            }
            _ => {
                connection
                    .execute(
                        "UPDATE meta SET value=json_set(value,'$.calls[0].budget',1) WHERE key=?1",
                        [&key],
                    )
                    .unwrap();
            }
        }
        let native = native_marker_query(&home);
        assert_eq!(
            native
                .handoffs
                .iter()
                .find(|handoff| handoff.to.as_deref() == Some("child"))
                .unwrap()
                .at,
            at(10, 1)
        );
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), indexed);
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn native_parent_marker_restart_reads_only_its_location_and_repairs_metadata_without_event_replay()
{
    let home = Home::new();
    home.top("parent", &[human("parent", ts(10, 0), "parent task")]);
    let marker = human(
        "child",
        ts(10, 1),
        "Semon-Parent: claude:parent\nSemon-Handoff: /private/SECRET-PROMPT\nSECRET-BODY",
    );
    let mut records = vec![json!({"type":"progress","padding":"x".repeat(1024)}); 1024];
    records.push(marker.clone());
    records.push(assistant("child", ts(10, 2), vec![text("answer")]));
    let path = home.top("child", &records);
    let original = fs::read(&path).unwrap();
    let raw = crate::FIRST_MARKER_READS.with(std::cell::Cell::get);
    let bytes = || crate::native_marker::READ_BYTES.with(std::cell::Cell::get);
    for iteration in 0..2 {
        let before = bytes();
        let query = native_marker_query(&home);
        assert_eq!(query.sessions["child"].parent.as_deref(), Some("parent"));
        assert_eq!(
            query.sessions["child"].parent_source.as_deref(),
            Some("marker")
        );
        assert_eq!(
            bytes() - before,
            (if iteration == 0 { 3 } else { 2 }) * (marker.to_string().len() as u64 + 1)
        );
        assert_eq!(crate::FIRST_MARKER_READS.with(std::cell::Cell::get), raw);
    }
    let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let key = format!("native_parent_marker:{}", path.display());
    let encoded: String = connection
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(!encoded.contains("SECRET"));
    assert!(!encoded.contains("private"));
    let indexed = crate::events::PARSED.with(std::cell::Cell::get);
    for mutation in 0..4 {
        match mutation {
            0 => {
                connection
                    .execute("DELETE FROM meta WHERE key=?1", [&key])
                    .unwrap();
            }
            1 => {
                connection
                    .execute("UPDATE meta SET value='{' WHERE key=?1", [&key])
                    .unwrap();
            }
            2 => {
                connection
                    .execute(
                        "UPDATE meta SET value=json_set(value,'$.version',99) WHERE key=?1",
                        [&key],
                    )
                    .unwrap();
            }
            _ => {
                connection
                    .execute(
                        "UPDATE meta SET value=json_set(value,'$.ledger','mismatch') WHERE key=?1",
                        [&key],
                    )
                    .unwrap();
            }
        }
        assert_eq!(
            native_marker_query(&home).sessions["child"]
                .parent
                .as_deref(),
            Some("parent")
        );
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), indexed);
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn native_handoff_context_rewrite_with_identical_stat_refuses_catalog_publication() {
    let _clock = crate::events::fixed_change_time_for_test(0);
    for eof in [false, true] {
        let home = Home::new();
        let path = home.top(
            "parent",
            &[
                human("parent", ts(10, 0), "task"),
                assistant(
                    "parent",
                    ts(10, 1),
                    vec![tool(
                        "launch",
                        "Bash",
                        json!({"command":"codex exec < /private/prompt"}),
                    )],
                ),
            ],
        );
        if eof {
            let mut bytes = fs::read(&path).unwrap();
            bytes.pop();
            fs::write(&path, bytes).unwrap();
        }
        home.codex(
            "child",
            json!({}),
            &[codex_user(
                ts(10, 2),
                "Semon-Parent: claude:parent\nSemon-Handoff: /private/prompt",
            )],
        );
        native_marker_query(&home);
        let mut cache = EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
        let generation = cache.session_catalog_generation();
        AFTER_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let before = fs::metadata(&path).unwrap();
                let original = fs::read_to_string(&path).unwrap();
                assert!(original.contains("10:01:00"));
                fs::write(&path, original.replace("10:01:00", "10:03:00")).unwrap();
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(before.modified().unwrap())
                    .unwrap();
                assert_eq!(
                    crate::events::Stat::from_metadata(&before).unwrap(),
                    crate::events::Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap()
                );
            }))
        });
        assert!(
            build_query(
                &home.options,
                &mut cache,
                &mut false,
                &mut Texts::default(),
                NOW
            )
            .is_err()
        );
        assert_eq!(cache.session_catalog_generation(), generation);
    }
}

#[test]
fn native_parent_markers_keep_valid_eof_and_first_agent_output_semantics() {
    for eof in [false, true] {
        let home = Home::new();
        home.top("parent", &[human("parent", ts(10, 0), "parent task")]);
        home.codex("child", json!({}), &[]);
        let path = home
            .options
            .codex_home
            .join("sessions/2026/09/24/rollout-child.jsonl");
        let marker = codex_user(
            ts(10, 1),
            "Semon-Parent: claude:parent\nSemon-Handoff: /private/prompt",
        )
        .to_string();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        if eof {
            let split = marker.len() / 2;
            write!(file, "{}", &marker[..split]).unwrap();
            assert!(
                native_marker_query(&home).sessions["child"]
                    .parent
                    .is_none()
            );
            write!(file, "{}", &marker[split..]).unwrap();
        } else {
            writeln!(file, "{marker}").unwrap();
        }
        let original = fs::read(&path).unwrap();
        assert_eq!(
            native_marker_query(&home).sessions["child"]
                .parent
                .as_deref(),
            Some("parent")
        );
        assert_eq!(
            native_marker_query(&home).sessions["child"]
                .parent
                .as_deref(),
            Some("parent")
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        if eof {
            writeln!(file).unwrap();
            assert_eq!(
                native_marker_query(&home).sessions["child"]
                    .parent
                    .as_deref(),
                Some("parent")
            );
        }
        home.codex(
            "stopped",
            json!({}),
            &[
                codex_reply(ts(10, 0), "output before marker"),
                codex_user(ts(10, 1), "Semon-Parent: claude:parent"),
            ],
        );
        assert!(
            native_marker_query(&home).sessions["stopped"]
                .parent
                .is_none()
        );
    }
}

#[test]
fn native_parent_marker_rewrite_with_identical_stat_refuses_catalog_publication_including_eof() {
    let _clock = crate::events::fixed_change_time_for_test(0);
    for eof in [false, true] {
        let home = Home::new();
        home.top("parent", &[human("parent", ts(10, 0), "parent task")]);
        let marker = human(
            "child",
            ts(10, 1),
            "Semon-Parent: claude:parent\nSemon-Handoff: /private/OLD",
        );
        let path = home.top("child", &[marker]);
        if eof {
            let mut original = fs::read(&path).unwrap();
            original.pop();
            fs::write(&path, original).unwrap();
        }
        native_marker_query(&home);
        let mut cache = EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
        let generation = cache.session_catalog_generation();
        AFTER_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let before = fs::metadata(&path).unwrap();
                let original = fs::read_to_string(&path).unwrap();
                assert!(original.contains("/private/OLD"));
                fs::write(&path, original.replace("/private/OLD", "/private/NEW")).unwrap();
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(before.modified().unwrap())
                    .unwrap();
                assert_eq!(
                    crate::events::Stat::from_metadata(&before).unwrap(),
                    crate::events::Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap()
                );
            }))
        });
        assert!(
            build_query(
                &home.options,
                &mut cache,
                &mut false,
                &mut Texts::default(),
                NOW
            )
            .is_err()
        );
        assert_eq!(cache.session_catalog_generation(), generation);
    }
}

#[test]
fn model_comparison_uses_indexed_per_message_models_for_launched_work() {
    let home = Home::new();
    home.top(
        "lead",
        &[
            human("lead", ts(10, 0), "Launch"),
            assistant(
                "lead",
                ts(10, 1),
                vec![tool(
                    "launch",
                    "Agent",
                    json!({"description":"Implement", "prompt":"Work"}),
                )],
            ),
        ],
    );
    let mut reply = assistant("lead", ts(10, 2), vec![text("Done")]);
    reply["message"]["model"] = json!("claude-sonnet-4-6");
    reply["message"]["usage"] = json!({"input_tokens":123, "output_tokens":45});
    home.agent("lead", "child", "launch", &[reply]);
    let built = home.build_query();
    let analytics = week(&built);
    let group = &analytics["models"]["groups"][0];
    assert_eq!(group["model"], "claude-sonnet-4-6");
    assert_eq!(group["n"], 1);
    assert_eq!(group["tokens"]["input"], 123);
    assert_eq!(group["tokens"]["output"], 45);
    assert_eq!(group["band"], "unknown");
    assert!(group["first_pass_acceptance"].is_null());
    assert!(group["median_model_ms"].is_null());
}

#[test]
fn analytics_answered_question_history_has_an_explicit_bound() {
    let home = Home::new();
    let mut records = vec![human("many-questions", ts(12, 0), "Ask")];
    for index in 0..4100 {
        let id = format!("q-{index}");
        records.push(assistant(
            "many-questions",
            ts(12, 1),
            vec![tool(
                &id,
                "AskUserQuestion",
                json!({"questions":[question("Pick?", &["Yes"], false)]}),
            )],
        ));
        records.push(result(
            "many-questions",
            ts(12, 2),
            &id,
            "",
            false,
            json!({"answers":{"Pick?":"Yes"}}),
        ));
    }
    home.top("many-questions", &records);
    let built = home.build_query();
    let activity = &native_activity(&built, NOW)["many-questions"];
    assert_eq!(activity.answered.len(), 4096);
    assert_eq!(activity.answered.capacity(), activity.answered.len());
    assert!(activity.waits_truncated);
    assert_eq!(week(&built)["waits_truncated"], json!(["many-questions"]));
}

#[test]
fn codex_root_human_reply_is_a_new_result_but_harness_context_is_not() {
    let home = Home::new();
    home.codex(
        "human-root",
        json!({}),
        &[
            codex_user(ts(12, 0), "Fix the bug"),
            codex_reply(ts(12, 1), "Fixed the bug"),
        ],
    );
    home.codex(
        "context-root",
        json!({}),
        &[
            codex_user(
                ts(12, 0),
                "<environment_context>machine</environment_context>",
            ),
            codex_reply(ts(12, 1), "Context read"),
        ],
    );
    home.codex(
        "missing-parent",
        json!({}),
        &[
            codex_user(ts(12, 0), "Semon-Parent: claude:missing\nWork"),
            codex_reply(ts(12, 1), "Done"),
        ],
    );
    let built = home.build_query();
    let results: Vec<_> = built
        .handoffs
        .iter()
        .filter(|h| h.ask == Some("result"))
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(
        (results[0].from.as_str(), results[0].status),
        ("human-root", "new")
    );
    assert!(matches!(built.tx["human-root"].slots[0].kind, SlotKind::U));
}

#[cfg(unix)]
#[test]
fn acknowledged_async_codex_question_does_not_count_continued_work_as_waiting() {
    let home = Home::new();
    home.codex("async", json!({}), &[
        codex_user(ts(12, 0), "Go"),
        codex_line(ts(12, 1), "response_item", json!({"type":"function_call","name":"request_user_input_async","call_id":"q", "arguments":"{\"questions\":[{\"title\":\"Which?\"}]}"})),
        codex_line(ts(12, 2), "response_item", json!({"type":"function_call_output","call_id":"q","output":"{\"accepted\":true}"})),
    ]);
    hold_lock(&home, "async");
    let built = home.build_query();
    assert_eq!(by_brief(&built, "Which?").status, "done");
    assert_eq!(week(&built)["current"]["wait_ms"], MINUTE);
}

#[test]
fn a_permission_wait_keeps_the_pending_tool_live_and_counts_the_known_wait() {
    let home = Home::new();
    home.top(
        "permission",
        &[
            human("permission", ts(16, 0), "touch it"),
            assistant(
                "permission",
                ts(16, 1),
                vec![tool("pending", "Bash", json!({"command":"touch file"}))],
            ),
        ],
    );
    home.live(
        39,
        "permission",
        "waiting",
        json!({"waitingFor":"permission prompt", "statusUpdatedAt":at(16, 2)}),
    );
    let built = home.build_query();
    let tx = &built.tx["permission"];
    assert_eq!(tx.errors, 0);
    assert!(tx.slots.iter().any(|slot| matches!(
        slot.kind,
        SlotKind::Tool {
            shown: Shown::Live,
            ..
        }
    )));
    let turns = turns_of(&built, "permission");
    assert_eq!((turns[0].end.st, turns[0].end.why), ("wait", "permission"));
    assert_eq!(built.sessions["permission"].waiting_since, Some(at(16, 2)));
    assert_eq!(week(&built)["current"]["wait_ms"], NOW - at(16, 2));
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
    let week = week(&home.build_query());
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
    let built = home.build_query();
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
        &crate::tx::tests::page(&built, &stub, &crate::tx::Anchor::Last, at(5, 30)).unwrap(),
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

/// A subagent's send that isn't its hand-back, as the receiving session gets
/// it (Claude Code 2.1: `senderTaskId`, no `handback`, no `msg_id`).
fn agent_send(sid: &str, time: String, from: &str, body: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,
        "origin":{"kind":"peer","from":from,"senderTaskId":from,"name":"general-purpose"},
        "message":{"role":"user","content":format!("Another Claude session sent a message:\n<agent-message from=\"{from}\">{body}</agent-message>")}})
}

/// A subagent's hand-back, as its parent gets it.
fn hand_back(sid: &str, time: String, from: &str, body: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,
        "origin":{"kind":"peer","from":from,"senderTaskId":from,"handback":true,"body":body},
        "message":{"role":"user","content":format!("<agent-message from=\"{from}\">{body}</agent-message>")}})
}

/// A parent's message, as its running or resumed subagent gets it.
fn coordinator(sid: &str, time: String, body: &str) -> Value {
    json!({"type":"user","timestamp":time,"sessionId":sid,"isMeta":true,"origin":{"kind":"coordinator"},
        "message":{"role":"user","content":format!("The coordinator sent a message while you were working:\n{body}")}})
}

/// The lead's `SendMessage` to agent `to` at 20:`minute`, with `outcome`.
fn send_to_agent(id: &str, minute: i64, to: &str, message: &str, outcome: Value) -> [Value; 2] {
    [
        assistant(
            "lead",
            ts(20, minute),
            vec![tool(id, "SendMessage", json!({"to":to,"message":message}))],
        ),
        result("lead", ts(20, minute), id, "sent", false, outcome),
    ]
}

fn resumed(agent: &str) -> Value {
    json!({"success":true,"message":format!("Resuming agent {agent} in the background"),"resumedAgentId":agent})
}

/// A subagent's progress send, its hand-back and its agent-end notification
/// are one relay and one result, each drawn once: the notification is the
/// run's status, not a message or an entry.
#[test]
fn a_progress_send_a_hand_back_and_an_agent_end_are_one_relay_and_one_result() {
    let mut child = send_to_main("p1", 2, "halfway there", queued_reply());
    child.push(child_says(4, "all done"));
    let built = background_home(
        vec![("aw", "tw", child)],
        vec![
            agent_send("lead", ts(20, 3), "aw", "halfway there"),
            hand_back("lead", ts(20, 5), "aw", "the final answer"),
            notification("lead", ts(20, 6), "tw", "completed", "the notified answer"),
        ],
    )
    .build_query();
    let spawn = only(&built, "spawn", "lead", "aw");
    assert_eq!(
        (spawn.status, spawn.result.as_deref(), spawn.done),
        ("done", Some("the final answer"), Some(at(20, 5)))
    );
    assert_eq!(built.sessions["aw"].state, "done");
    let relay = only(&built, "relay", "aw", "lead");
    assert_eq!(
        (relay.brief.as_str(), relay.unmatched, relay.at),
        ("halfway there", false, at(20, 2))
    );
    assert_eq!(received_in(&built, "lead", &relay.id), 1);
    // Nothing else between them: no second relay, result or entry.
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.from == "aw" || handoff.to.as_deref() == Some("aw"))
            .count(),
        2
    );
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.result.is_some())
            .count(),
        1
    );
    let slots = &built.tx["lead"].slots;
    assert_eq!(received_in(&built, "lead", &spawn.id), 1);
    assert!(
        slots
            .iter()
            .all(|slot| !matches!(slot.kind, SlotKind::BgEnd { .. }))
    );
    assert!(built.sessions.values().all(|session| !session.stub));
}

/// A subagent's send to its sibling is a relay between them, not a receipt
/// of either spawn; its send to `main` is still paired with its receipt in
/// the parent.
#[test]
fn a_subagent_send_to_a_sibling_is_a_relay_not_a_receipt() {
    let mut aa = vec![
        assistant(
            "lead",
            ts(20, 2),
            vec![tool(
                "to-b",
                "SendMessage",
                json!({"to":"ab","message":"sibling note"}),
            )],
        ),
        result(
            "lead",
            ts(20, 2),
            "to-b",
            "sent",
            false,
            json!({"success":true,"message":"sent","pin":{"id":"ab"}}),
        ),
    ];
    aa.extend(send_to_main("to-lead", 3, "note for lead", queued_reply()));
    aa.push(child_says(5, "aa done"));
    let ab = vec![
        agent_send("lead", ts(20, 4), "aa", "sibling note"),
        child_says(6, "noted"),
    ];
    let built = background_home(
        vec![("aa", "ta", aa), ("ab", "tb", ab)],
        vec![
            agent_send("lead", ts(20, 4), "aa", "note for lead"),
            hand_back("lead", ts(20, 7), "aa", "aa done"),
        ],
    )
    .build_query();
    let sibling = only(&built, "relay", "aa", "ab");
    assert_eq!(
        (sibling.brief.as_str(), sibling.unmatched),
        ("sibling note", false)
    );
    assert_eq!(received_in(&built, "ab", &sibling.id), 1);
    assert_eq!(
        turns_of(&built, "ab")[1].start.as_deref(),
        Some(sibling.id.as_str())
    );
    let up = only(&built, "relay", "aa", "lead");
    assert_eq!(up.brief, "note for lead");
    assert_eq!(received_in(&built, "lead", &up.id), 1);
    assert_eq!(
        only(&built, "spawn", "lead", "aa").result.as_deref(),
        Some("aa done")
    );
    assert_eq!(only(&built, "spawn", "lead", "ab").result.as_deref(), None);
    assert!(built.sessions.values().all(|session| !session.stub));
}

/// A parent's message that reaches its subagent mid-run is a relay from the
/// parent, paired with the parent's send, and an entry of the child's
/// current turn, not a turn of its own.
#[test]
fn a_parent_message_mid_run_is_inline_in_the_subagents_turn() {
    let child = vec![
        assistant(
            "lead",
            ts(20, 2),
            vec![tool("ls", "Bash", json!({"command":"ls"}))],
        ),
        result("lead", ts(20, 2), "ls", "src", false, json!({})),
        coordinator("lead", ts(20, 3), "also check the tests"),
        child_says(4, "checked"),
    ];
    let mut receipts = Vec::from(send_to_agent(
        "to-aw",
        2,
        "aw",
        "also check the tests",
        json!({"success":true,"message":"Message queued for delivery to aw at its next tool round."}),
    ));
    receipts.push(hand_back("lead", ts(20, 5), "aw", "checked"));
    receipts.push(notification(
        "lead",
        ts(20, 6),
        "tw",
        "completed",
        "checked",
    ));
    let built = background_home(vec![("aw", "tw", child)], receipts).build_query();
    let relay = only(&built, "relay", "lead", "aw");
    assert_eq!(
        (relay.brief.as_str(), relay.unmatched, relay.at),
        ("also check the tests", false, at(20, 2))
    );
    assert_eq!(received_in(&built, "lead", &relay.id), 1);
    assert_eq!(received_in(&built, "aw", &relay.id), 1);
    assert!(
        turns_of(&built, "lead")
            .iter()
            .any(|turn| turn.sent.contains(&relay.id))
    );
    let turns = turns_of(&built, "aw");
    assert_eq!(turns.len(), 1);
    let slot = built.tx["aw"]
        .slots
        .iter()
        .find(|slot| matches!(&slot.kind, SlotKind::H(id) if id == &relay.id))
        .unwrap();
    assert_eq!(slot.turn.as_deref(), Some(turns[0].id.as_str()));
    assert_eq!(built.sessions["aw"].state, "done");
    assert!(built.sessions.values().all(|session| !session.stub));
}

/// A subagent that handed back, then was resumed by its parent: the resume
/// starts a new turn and the child works again until that run ends.
fn resume_home(second_end: bool) -> Home {
    let mut child = vec![
        child_says(2, "first answer"),
        coordinator("lead", ts(20, 5), "now fix it"),
        child_says(6, "fixing"),
    ];
    let mut receipts = vec![
        hand_back("lead", ts(20, 3), "aw", "first answer"),
        notification("lead", ts(20, 4), "tw", "completed", "first answer"),
    ];
    receipts.extend(send_to_agent(
        "resume",
        5,
        "aw",
        "now fix it",
        resumed("aw"),
    ));
    if second_end {
        child.push(child_says(7, "fixed"));
        receipts.push(hand_back("lead", ts(20, 8), "aw", "fixed"));
        receipts.push(notification("lead", ts(20, 9), "tw", "completed", "fixed"));
    }
    let home = background_home(vec![("aw", "tw", child)], receipts);
    home.live(51, "lead", "busy", json!({}));
    home
}

#[test]
fn a_resume_after_a_hand_back_starts_a_turn_and_works_until_its_run_ends() {
    let built = resume_home(false).build_query();
    let relay = only(&built, "relay", "lead", "aw");
    assert_eq!(relay.brief, "now fix it");
    let turns = turns_of(&built, "aw");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].start.as_deref(), Some(relay.id.as_str()));
    assert_eq!(built.sessions["aw"].state, "work");
    let spawn = only(&built, "spawn", "lead", "aw");
    // Still the previous run's end until this one ends.
    assert_eq!(
        (spawn.status, spawn.result.as_deref()),
        ("work", Some("first answer"))
    );

    let built = resume_home(true).build_query();
    assert_eq!(built.sessions["aw"].state, "done");
    let spawn = only(&built, "spawn", "lead", "aw");
    assert_eq!(
        (spawn.status, spawn.result.as_deref(), spawn.done),
        ("done", Some("fixed"), Some(at(20, 8)))
    );
    assert_eq!(turns_of(&built, "aw").len(), 2);
}

/// Three runs of one subagent: the spawn's result is the latest run's
/// hand-back. The second run's notification arrives after the parent
/// already resumed the third, and still belongs to the second.
#[test]
fn a_subagent_resumed_twice_returns_its_latest_run() {
    let child = vec![
        child_says(2, "one"),
        coordinator("lead", ts(20, 5), "second task"),
        child_says(7, "two"),
        coordinator("lead", ts(20, 9), "third task"),
        child_says(11, "three"),
    ];
    let mut receipts = vec![
        hand_back("lead", ts(20, 3), "aw", "one"),
        notification("lead", ts(20, 4), "tw", "completed", "notified one"),
    ];
    receipts.extend(send_to_agent("r2", 5, "aw", "second task", resumed("aw")));
    receipts.push(hand_back("lead", ts(20, 8), "aw", "two"));
    receipts.extend(send_to_agent("r3", 9, "aw", "third task", resumed("aw")));
    receipts.push(notification(
        "lead",
        ts(20, 10),
        "tw",
        "completed",
        "notified two",
    ));
    receipts.push(hand_back("lead", ts(20, 12), "aw", "three"));
    receipts.push(notification(
        "lead",
        ts(20, 13),
        "tw",
        "completed",
        "notified three",
    ));
    let built = background_home(vec![("aw", "tw", child)], receipts).build_query();
    let spawn = only(&built, "spawn", "lead", "aw");
    assert_eq!(
        (spawn.status, spawn.result.as_deref(), spawn.done),
        ("done", Some("three"), Some(at(20, 12)))
    );
    assert_eq!(built.sessions["aw"].state, "done");
    let turns = turns_of(&built, "aw");
    assert_eq!(turns.len(), 3);
    for (turn, brief) in [(1, "second task"), (2, "third task")] {
        let relay = by_brief(&built, brief);
        assert_eq!(
            (relay.kind, relay.from.as_str(), relay.to.as_deref()),
            ("relay", "lead", Some("aw"))
        );
        assert_eq!(turns[turn].start.as_deref(), Some(relay.id.as_str()));
    }
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.result.is_some())
            .count(),
        1
    );
}

/// A legacy `<agent-message>` tag with no `origin` keeps the old pairing: a
/// send's receipt is its relay and the last unclaimed one is the hand-back.
/// A tag quoted in a notification's result is not a receipt.
#[test]
fn legacy_tag_receipts_pair_as_before_and_quoted_tags_are_not_receipts() {
    let mut child = send_to_main("n1", 2, "legacy note", queued_reply());
    child.push(child_says(4, "legacy done"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![
            agm_line(3, &[("ap", "legacy note")]),
            agm_line(5, &[("ap", "[Subagent hand-back] legacy done")]),
            user(
                "lead",
                ts(20, 6),
                "<task-notification><tool-use-id>bg</tool-use-id><status>completed</status><result>log: <agent-message from=\"ap\">quoted, not sent</agent-message></result></task-notification>",
            ),
        ],
    )
    .build_query();
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("[Subagent hand-back] legacy done")
    );
    let relay = only(&built, "relay", "ap", "lead");
    assert_eq!(relay.brief, "legacy note");
    assert_eq!(received_in(&built, "lead", &relay.id), 1);
    assert!(
        built
            .handoffs
            .iter()
            .all(|handoff| handoff.result.as_deref() != Some("quoted, not sent"))
    );
}

/// A notification that a subagent run this session started has ended is a
/// status, not a message: a reply after it still answers your message.
#[test]
fn an_agent_end_notification_does_not_interrupt_a_reply() {
    let home = Home::new();
    home.top(
        "notified-agent",
        &[
            human("notified-agent", ts(16, 0), "delegate it"),
            assistant(
                "notified-agent",
                ts(16, 1),
                vec![tool("ag", "Agent", json!({"prompt":"go"}))],
            ),
            result(
                "notified-agent",
                ts(16, 1),
                "ag",
                "launched",
                false,
                json!({"status":"async_launched","agentId":"ag1"}),
            ),
            notification(
                "notified-agent",
                ts(16, 3),
                "ag",
                "completed",
                "agent result",
            ),
            assistant(
                "notified-agent",
                ts(16, 4),
                vec![text("The agent finished.")],
            ),
        ],
    );
    home.agent(
        "notified-agent",
        "ag1",
        "ag",
        &[
            user("notified-agent", ts(16, 1), "go"),
            assistant("notified-agent", ts(16, 2), vec![text("working")]),
        ],
    );
    home.live(41, "notified-agent", "idle", json!({}));
    let built = home.build_query();
    let result = by_brief(&built, "The agent finished.");
    assert_eq!(
        (result.kind, result.ask, result.from.as_str()),
        ("toyou", Some("result"), "notified-agent")
    );
    assert_eq!(
        only(&built, "spawn", "notified-agent", "ag1")
            .result
            .as_deref(),
        Some("agent result")
    );
}

/// A Codex child's result is only what it sent up to its parent, not its
/// message to its own child or to a sibling.
#[test]
fn a_codex_childs_result_is_its_message_up() {
    let home = Home::new();
    home.codex(
        "root",
        json!({}),
        &[
            codex_user(ts(4, 0), "Coordinate"),
            codex_line(ts(4, 1), "response_item", json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"c1","arguments":"{\"task_name\":\"worker\",\"message\":\"go\"}"})),
            codex_line(ts(4, 1), "response_item", json!({"type":"function_call_output","call_id":"c1","output":"{\"task_name\":\"worker\"}"})),
            codex_line(ts(4, 9), "response_item", json!({"type":"agent_message","author":"root/worker","recipient":"root","content":[{"type":"input_text","text":"worker finished"}]})),
        ],
    );
    home.codex(
        "child",
        json!({"parent_thread_id":"root","thread_source":"subagent","agent_nickname":"worker","agent_path":"root/worker"}),
        &[
            codex_user(ts(4, 1), "go"),
            codex_line(ts(4, 10), "response_item", json!({"type":"agent_message","author":"root/worker","recipient":"root/worker/helper","content":[{"type":"input_text","text":"down to my helper"}]})),
            codex_line(ts(4, 11), "response_item", json!({"type":"agent_message","author":"root/worker","recipient":"root/other","content":[{"type":"input_text","text":"side note"}]})),
            codex_reply(ts(4, 12), "done"),
        ],
    );
    let built = home.build_query();
    assert_eq!(
        only(&built, "spawn", "root", "child").result.as_deref(),
        Some("worker finished")
    );
}

/// The child's own hand-back call, at 20:`minute`.
fn child_hands_back(id: &str, minute: i64, message: &str) -> Value {
    assistant(
        "lead",
        ts(20, minute),
        vec![tool(id, "SubagentHandback", json!({"message":message}))],
    )
}

/// A send to the name the spawning call gave the agent pairs with the
/// message it received, though its result doesn't say where it went; a
/// second send to that name that never arrived is the agent's too, never a
/// relay to a stub.
#[test]
fn a_send_to_the_name_its_spawning_call_gave_pairs_with_its_message() {
    let home = Home::new();
    let sent = json!({"success":true,"message":"Message sent to reviewer"});
    let mut lead = vec![
        human("lead", ts(20, 0), "Review it"),
        assistant(
            "lead",
            ts(20, 1),
            vec![tool(
                "tw",
                "Agent",
                json!({"name":"reviewer","prompt":"brief tw"}),
            )],
        ),
        result(
            "lead",
            ts(20, 1),
            "tw",
            "launched",
            false,
            json!({"status":"async_launched","agentId":"aw"}),
        ),
    ];
    lead.extend(send_to_agent(
        "first",
        2,
        "reviewer",
        "look at the diff",
        sent.clone(),
    ));
    lead.extend(send_to_agent(
        "second",
        6,
        "reviewer",
        "never arrived",
        sent,
    ));
    home.top("lead", &lead);
    home.agent(
        "lead",
        "aw",
        "tw",
        &[
            user("lead", ts(20, 1), "brief tw"),
            coordinator("lead", ts(20, 3), "look at the diff"),
            child_says(4, "looking"),
        ],
    );
    let built = home.build_query();
    let relay = only(&built, "relay", "lead", "aw");
    assert_eq!(
        (relay.brief.as_str(), relay.unmatched, relay.at),
        ("look at the diff", false, at(20, 2))
    );
    assert_eq!(received_in(&built, "lead", &relay.id), 1);
    assert_eq!(received_in(&built, "aw", &relay.id), 1);
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.kind == "relay")
            .count(),
        1
    );
    assert!(built.sessions.values().all(|session| !session.stub));
}

/// A parent's message with no send in these logs is still drawn, from its
/// own text without the CLI's preamble, and its preview reads the same.
#[test]
fn an_unpaired_parent_message_is_drawn_from_its_own_text() {
    let home = background_home(
        vec![(
            "aw",
            "tw",
            vec![
                coordinator("lead", ts(20, 3), "only in the child"),
                child_says(4, "ok"),
            ],
        )],
        Vec::new(),
    );
    let built = home.build_query();
    let relay = only(&built, "relay", "lead", "aw");
    assert_eq!(
        (relay.brief.as_str(), relay.unmatched, relay.at),
        ("only in the child", false, at(20, 3))
    );
    assert_eq!(received_in(&built, "aw", &relay.id), 1);
    assert_eq!(received_in(&built, "lead", &relay.id), 0);
    let turns = turns_of(&built, "aw");
    assert_eq!(turns.len(), 1);
    assert_bounded_causal_previews(&home, &built, turns[0]);
    assert_eq!(
        overview_text(
            &json!({"kind":"received_down"}),
            &coordinator("lead", ts(20, 3), "only in the child"),
            None
        ),
        json!("only in the child")
    );
}

/// The child hands back, the parent resumes it, and only then does the
/// parent record the hand-back: it is the first run's, and the resumed run
/// is still working.
#[test]
fn a_hand_back_recorded_after_the_resume_belongs_to_the_run_that_handed_back() {
    let mut receipts = Vec::from(send_to_agent(
        "resume",
        4,
        "aw",
        "now fix it",
        resumed("aw"),
    ));
    receipts.push(hand_back("lead", ts(20, 6), "aw", "first done"));
    let home = background_home(
        vec![(
            "aw",
            "tw",
            vec![
                child_hands_back("hb1", 2, "first done"),
                coordinator("lead", ts(20, 5), "now fix it"),
                child_says(7, "fixing"),
            ],
        )],
        receipts,
    );
    home.live(52, "lead", "busy", json!({}));
    let built = home.build_query();
    assert_eq!(built.sessions["aw"].state, "work");
    let spawn = only(&built, "spawn", "lead", "aw");
    assert_eq!(
        (spawn.status, spawn.result.as_deref()),
        ("work", Some("first done"))
    );
    let turns = turns_of(&built, "aw");
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns[1].start.as_deref(),
        Some(only(&built, "relay", "lead", "aw").id.as_str())
    );
}

/// Without a hand-back in the child, a message that arrives before the
/// parent records the hand-back reached the open run: inline, and that
/// hand-back ends the run.
#[test]
fn a_message_before_the_only_hand_back_is_inline_and_the_hand_back_ends_the_run() {
    let mut receipts = Vec::from(send_to_agent(
        "more",
        4,
        "aw",
        "also this",
        json!({"success":true,"message":"Message queued for delivery to aw at its next tool round."}),
    ));
    receipts.push(hand_back("lead", ts(20, 8), "aw", "done both"));
    let home = background_home(
        vec![(
            "aw",
            "tw",
            vec![
                child_says(2, "working"),
                coordinator("lead", ts(20, 5), "also this"),
                child_says(7, "done both"),
            ],
        )],
        receipts,
    );
    home.live(53, "lead", "busy", json!({}));
    let built = home.build_query();
    assert_eq!(turns_of(&built, "aw").len(), 1);
    assert_eq!(built.sessions["aw"].state, "done");
    assert_eq!(
        only(&built, "spawn", "lead", "aw").result.as_deref(),
        Some("done both")
    );
}

/// A resumed run that is killed after the run before handed back without a
/// notification: the kill ends the resumed run, which isn't working.
#[test]
fn a_killed_resumed_run_does_not_show_as_working() {
    let mut receipts = vec![hand_back("lead", ts(20, 3), "aw", "first done")];
    receipts.extend(send_to_agent("again", 4, "aw", "try again", resumed("aw")));
    receipts.push(notification(
        "lead",
        ts(20, 8),
        "tw",
        "killed",
        "stopped by you",
    ));
    let home = background_home(
        vec![(
            "aw",
            "tw",
            vec![
                child_hands_back("hb1", 2, "first done"),
                coordinator("lead", ts(20, 5), "try again"),
                child_says(6, "retrying"),
            ],
        )],
        receipts,
    );
    home.live(54, "lead", "busy", json!({}));
    let built = home.build_query();
    assert_eq!(built.sessions["aw"].state, "err");
    let spawn = only(&built, "spawn", "lead", "aw");
    assert_eq!(
        (spawn.status, spawn.result.as_deref(), spawn.done),
        ("err", Some("stopped by you"), Some(at(20, 8)))
    );
}

/// A resumed run that ended only in the child's log (`SubagentHandback`) is
/// still the latest end: its message is the result.
#[test]
fn a_run_ended_only_by_the_childs_hand_back_is_the_latest_result() {
    let mut receipts = vec![
        hand_back("lead", ts(20, 3), "aw", "one"),
        notification("lead", ts(20, 4), "tw", "completed", "one"),
    ];
    receipts.extend(send_to_agent("r2", 5, "aw", "second", resumed("aw")));
    let built = background_home(
        vec![(
            "aw",
            "tw",
            vec![
                child_says(2, "one"),
                coordinator("lead", ts(20, 5), "second"),
                child_hands_back("hb2", 7, "two by hand"),
            ],
        )],
        receipts,
    )
    .build_query();
    let spawn = only(&built, "spawn", "lead", "aw");
    assert_eq!(
        (spawn.status, spawn.result.as_deref(), spawn.done),
        ("done", Some("two by hand"), Some(at(20, 7)))
    );
}

/// A hand-back from a subagent this session spawned is that spawn's result,
/// not a message: a reply after it still answers your message.
#[test]
fn an_own_subagents_hand_back_does_not_interrupt_a_reply() {
    let home = Home::new();
    home.top(
        "handed-agent",
        &[
            human("handed-agent", ts(16, 0), "delegate it"),
            assistant(
                "handed-agent",
                ts(16, 1),
                vec![tool("ag", "Agent", json!({"prompt":"go"}))],
            ),
            result(
                "handed-agent",
                ts(16, 1),
                "ag",
                "launched",
                false,
                json!({"status":"async_launched","agentId":"ag1"}),
            ),
            hand_back("handed-agent", ts(16, 3), "ag1", "agent result"),
            assistant(
                "handed-agent",
                ts(16, 4),
                vec![text("The agent handed back.")],
            ),
        ],
    );
    home.agent(
        "handed-agent",
        "ag1",
        "ag",
        &[
            user("handed-agent", ts(16, 1), "go"),
            assistant("handed-agent", ts(16, 2), vec![text("working")]),
        ],
    );
    home.live(42, "handed-agent", "idle", json!({}));
    let built = home.build_query();
    let result = by_brief(&built, "The agent handed back.");
    assert_eq!(
        (result.kind, result.ask, result.from.as_str()),
        ("toyou", Some("result"), "handed-agent")
    );
}

/// A lead with two subagents: `aw` sends progress to `main` and gets a
/// message from the lead mid-run; `mid` spawns `leaf`, whose progress send
/// and hand-back arrive at `mid`, an intermediate agent.
fn down_and_nested_home() -> Home {
    let home = Home::new();
    let mut lead = vec![
        human("lead", ts(20, 0), "Start"),
        assistant(
            "lead",
            ts(20, 1),
            vec![
                tool("ta", "Agent", json!({"prompt":"brief aw"})),
                tool("tm", "Agent", json!({"prompt":"brief mid"})),
            ],
        ),
        result(
            "lead",
            ts(20, 1),
            "ta",
            "launched",
            false,
            json!({"status":"async_launched","agentId":"aw"}),
        ),
        result(
            "lead",
            ts(20, 1),
            "tm",
            "launched",
            false,
            json!({"status":"async_launched","agentId":"mid"}),
        ),
        agent_send("lead", ts(20, 3), "aw", "aw note"),
    ];
    lead.extend(send_to_agent(
        "to-aw",
        4,
        "aw",
        "also this",
        json!({"success":true,"message":"Message queued for delivery to aw at its next tool round."}),
    ));
    lead.extend([
        hand_back("lead", ts(20, 7), "aw", "aw done"),
        notification("lead", ts(20, 8), "ta", "completed", "aw done"),
        hand_back("lead", ts(20, 12), "mid", "mid done"),
    ]);
    home.top("lead", &lead);
    let mut aw = vec![user("lead", ts(20, 1), "brief aw")];
    aw.extend(send_to_main("aw-up", 2, "aw note", queued_reply()));
    aw.extend([
        coordinator("lead", ts(20, 5), "also this"),
        child_says(6, "aw done"),
    ]);
    home.agent("lead", "aw", "ta", &aw);
    home.agent(
        "lead",
        "mid",
        "tm",
        &[
            user("lead", ts(20, 1), "brief mid"),
            assistant(
                "lead",
                ts(20, 2),
                vec![tool("t-leaf", "Agent", json!({"prompt":"leaf brief"}))],
            ),
            result(
                "lead",
                ts(20, 2),
                "t-leaf",
                "launched",
                false,
                json!({"status":"async_launched","agentId":"leaf"}),
            ),
            agent_send("lead", ts(20, 5), "leaf", "leaf note"),
            hand_back("lead", ts(20, 9), "leaf", "leaf done"),
            child_says(11, "mid done"),
        ],
    );
    let mut leaf = vec![user("lead", ts(20, 2), "leaf brief")];
    leaf.extend(send_to_main("leaf-up", 4, "leaf note", queued_reply()));
    leaf.push(child_says(8, "leaf done"));
    home.agent("lead", "leaf", "t-leaf", &leaf);
    home
}

fn assert_down_and_nested(built: &QueryData) {
    let up = only(built, "relay", "aw", "lead");
    assert_eq!(up.brief, "aw note");
    assert_eq!(received_in(built, "lead", &up.id), 1);
    let down = only(built, "relay", "lead", "aw");
    assert_eq!((down.brief.as_str(), down.unmatched), ("also this", false));
    assert_eq!(turns_of(built, "aw").len(), 1);
    let nested = only(built, "relay", "leaf", "mid");
    assert_eq!(nested.brief, "leaf note");
    assert_eq!(received_in(built, "mid", &nested.id), 1);
    assert_eq!(
        only(built, "spawn", "mid", "leaf").result.as_deref(),
        Some("leaf done")
    );
    assert_eq!(
        only(built, "spawn", "lead", "aw").result.as_deref(),
        Some("aw done")
    );
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.kind == "relay")
            .count(),
        3
    );
    assert!(built.sessions.values().all(|session| !session.stub));
}

/// The staged catalog draws the same agent messages as the shared builder:
/// one relay per message, no `relay:tool:` stub beside a parent's message,
/// and a receipt at an intermediate agent pairs there.
#[test]
fn staged_agent_messages_match_the_shared_builder_without_duplicate_relations() {
    let home = down_and_nested_home();
    let oracle = assert_queued_identity_matches_oracle(&home, &home.options);
    assert_down_and_nested(&oracle);
    let ids: BTreeSet<&str> = oracle
        .handoffs
        .iter()
        .map(|handoff| handoff.id.as_str())
        .collect();
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    for key in ["lead", "aw", "mid", "leaf"] {
        let edges = db
            .prepare("SELECT edge_id FROM session_trace_edges WHERE session_key=?1")
            .unwrap()
            .query_map([key], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let unique: BTreeSet<&String> = edges.iter().collect();
        assert_eq!(unique.len(), edges.len(), "{key}: duplicate staged edges");
        for edge in &edges {
            assert!(
                ids.contains(edge.as_str()),
                "{key}: staged edge {edge} the shared builder doesn't draw"
            );
        }
    }
}

#[test]
fn staged_owner_replay_keeps_agent_messages_and_inline_turns() {
    let home = down_and_nested_home();
    assert_down_and_nested(&home.build_query());
    let mut inputs = crate::inputs(&home.options).unwrap();
    inputs.retain(|input| input.path.ends_with(".jsonl"));
    assert_eq!(inputs.len(), 4);
    assert_staged_owner_parity(&home, &inputs);
}

/// A lead that starts background subagents at 20:01, each (agent id,
/// Agent call id, its lines after its brief), then gets `receipts`.
fn background_home(agents: Vec<(&str, &str, Vec<Value>)>, receipts: Vec<Value>) -> Home {
    let home = Home::new();
    let calls = agents
        .iter()
        .map(|(_, call, _)| tool(call, "Agent", json!({"prompt":format!("brief {call}")})))
        .collect();
    let mut lead = vec![
        human("lead", ts(20, 0), "Start background workers"),
        assistant("lead", ts(20, 1), calls),
    ];
    for (agent, call, _) in &agents {
        lead.push(result(
            "lead",
            ts(20, 1),
            call,
            "launched",
            false,
            json!({"status":"async_launched","agentId":agent}),
        ));
    }
    lead.extend(receipts);
    home.top("lead", &lead);
    for (agent, call, mut lines) in agents {
        lines.insert(0, user("lead", ts(20, 1), &format!("brief {call}")));
        home.agent("lead", agent, call, &lines);
    }
    home
}

/// One parent line at 20:`minute` holding `<agent-message>` tags.
fn agm_line(minute: i64, tags: &[(&str, &str)]) -> Value {
    let content: String = tags
        .iter()
        .map(|(from, body)| format!("<agent-message from=\"{from}\">{body}</agent-message>\n"))
        .collect();
    user("lead", ts(20, minute), &content)
}

fn child_says(minute: i64, said: &str) -> Value {
    assistant("lead", ts(20, minute), vec![text(said)])
}

fn queued_reply() -> Option<Value> {
    Some(json!({"success":true,"message":"Message queued for the main conversation's next turn."}))
}

/// A subagent's send to `main` at 20:`minute`, and its result: `None` for
/// one not back yet, a string for a refused one.
fn send_to_main(id: &str, minute: i64, message: &str, reply: Option<Value>) -> Vec<Value> {
    let mut lines = vec![assistant(
        "lead",
        ts(20, minute),
        vec![tool(
            id,
            "SendMessage",
            json!({"to":"main","message":message}),
        )],
    )];
    if let Some(reply) = reply {
        let refused = reply.is_string();
        let content = if refused {
            "Permission denied"
        } else {
            "queued"
        };
        lines.push(result("lead", ts(20, minute), id, content, refused, reply));
    }
    lines
}

/// How often `sid`'s transcript draws handoff `id`.
fn received_in(built: &QueryData, sid: &str, id: &str) -> usize {
    built.tx[sid]
        .slots
        .iter()
        .filter(|slot| matches!(&slot.kind, SlotKind::H(found) if found == id))
        .count()
}

/// Two progress sends to `main` (each received a minute later), then the
/// subagent's answer and its hand-back.
fn progress_home(hand_back: bool) -> Home {
    let mut child = send_to_main("p1", 2, "first progress note", queued_reply());
    child.extend(send_to_main(
        "p2",
        4,
        "second progress note",
        queued_reply(),
    ));
    let mut receipts = vec![
        agm_line(3, &[("ap", "first progress note")]),
        agm_line(5, &[("ap", "second progress note")]),
    ];
    if hand_back {
        child.push(child_says(6, "the final answer"));
        receipts.push(agm_line(
            7,
            &[("ap", "[Subagent hand-back] the final answer")],
        ));
    }
    background_home(vec![("ap", "tp", child)], receipts)
}

#[test]
fn progress_sends_to_main_are_relays_and_only_the_last_receipt_hands_back() {
    let built = progress_home(true).build_query();
    let spawn = only(&built, "spawn", "lead", "ap");
    assert_eq!(
        (spawn.status, spawn.result.as_deref(), spawn.done),
        (
            "done",
            Some("[Subagent hand-back] the final answer"),
            Some(at(20, 7))
        )
    );
    let lead = turns_of(&built, "lead");
    for (minute, brief) in [(2, "first progress note"), (4, "second progress note")] {
        // One handoff per message: the relay, sent by the subagent's turn
        // and starting the parent's turn at its receipt, drawn there once.
        let relay = by_brief(&built, brief);
        assert_eq!(
            built
                .handoffs
                .iter()
                .filter(|handoff| handoff.brief == brief)
                .count(),
            1
        );
        assert_eq!(
            (
                relay.kind,
                relay.from.as_str(),
                relay.to.as_deref(),
                relay.at
            ),
            ("relay", "ap", Some("lead"), at(20, minute))
        );
        assert!(
            turns_of(&built, "ap")
                .iter()
                .any(|turn| turn.sent.contains(&relay.id))
        );
        assert_eq!(
            lead.iter()
                .filter(|turn| turn.start.as_deref() == Some(relay.id.as_str()))
                .count(),
            1,
            "{brief}"
        );
        assert_eq!(received_in(&built, "lead", &relay.id), 1, "{brief}");
    }
    assert!(built.sessions.values().all(|session| !session.stub));
}

#[test]
fn progress_before_a_hand_back_is_not_the_spawns_result() {
    let built = progress_home(false).build_query();
    let spawn = only(&built, "spawn", "lead", "ap");
    assert_eq!((spawn.result.as_deref(), spawn.done), (None, None));
    for brief in ["first progress note", "second progress note"] {
        let relay = by_brief(&built, brief);
        assert_eq!(received_in(&built, "lead", &relay.id), 1, "{brief}");
    }
    assert_eq!(
        built
            .handoffs
            .iter()
            .filter(|handoff| handoff.kind == "relay" && handoff.from == "ap")
            .count(),
        2
    );
}

/// A subagent resumed after its first hand-back: its first send's receipt
/// isn't in these logs. Neither hand-back is taken for a send, and the
/// second is the result.
#[test]
fn a_resumed_subagents_hand_backs_are_not_taken_for_its_sends() {
    let mut child = send_to_main("r1", 2, "first run note", queued_reply());
    child.push(child_says(3, "first run done"));
    child.push(user("lead", ts(20, 5), "carry on"));
    child.extend(send_to_main("r2", 6, "second run note", queued_reply()));
    child.push(child_says(8, "second run done"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![
            agm_line(4, &[("ap", "[Subagent hand-back] first run done")]),
            agm_line(7, &[("ap", "second run note")]),
            agm_line(9, &[("ap", "[Subagent hand-back] second run done")]),
        ],
    )
    .build_query();
    let first = by_brief(&built, "first run note");
    assert_eq!(received_in(&built, "lead", &first.id), 0);
    let second = by_brief(&built, "second run note");
    assert_eq!(received_in(&built, "lead", &second.id), 1);
    let spawn = only(&built, "spawn", "lead", "ap");
    assert_eq!(
        (spawn.result.as_deref(), spawn.done),
        (
            Some("[Subagent hand-back] second run done"),
            Some(at(20, 9))
        )
    );
}

/// A send whose receipt is missing doesn't take the hand-back as its own:
/// the spawn keeps its result.
#[test]
fn fewer_receipts_than_sends_keep_the_hand_back() {
    let mut child = send_to_main("n1", 2, "note one", queued_reply());
    child.extend(send_to_main("n2", 4, "note two", queued_reply()));
    child.push(child_says(6, "done"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![
            agm_line(3, &[("ap", "note one")]),
            agm_line(7, &[("ap", "[Subagent hand-back] done")]),
        ],
    )
    .build_query();
    let spawn = only(&built, "spawn", "lead", "ap");
    assert_eq!(spawn.result.as_deref(), Some("[Subagent hand-back] done"));
    let one = by_brief(&built, "note one");
    assert_eq!(received_in(&built, "lead", &one.id), 1);
    let two = by_brief(&built, "note two");
    assert_eq!(received_in(&built, "lead", &two.id), 0);
}

/// Two subagents of one parent, their receipts interleaved: each pairs and
/// hands back on its own.
#[test]
fn interleaved_subagents_pair_their_own_receipts() {
    let mut ap = send_to_main("pa", 2, "note from ap", queued_reply());
    ap.push(child_says(5, "ap done"));
    let mut aq = send_to_main("pq", 3, "note from aq", queued_reply());
    aq.push(child_says(6, "aq done"));
    let built = background_home(
        vec![("ap", "tp", ap), ("aq", "tq", aq)],
        vec![
            agm_line(4, &[("aq", "note from aq")]),
            agm_line(5, &[("ap", "note from ap")]),
            agm_line(7, &[("aq", "[Subagent hand-back] aq done")]),
            agm_line(8, &[("ap", "[Subagent hand-back] ap done")]),
        ],
    )
    .build_query();
    for (agent, note, handed) in [
        ("ap", "note from ap", "[Subagent hand-back] ap done"),
        ("aq", "note from aq", "[Subagent hand-back] aq done"),
    ] {
        assert_eq!(
            only(&built, "spawn", "lead", agent).result.as_deref(),
            Some(handed)
        );
        let relay = only(&built, "relay", agent, "lead");
        assert_eq!(relay.brief, note);
        assert_eq!(received_in(&built, "lead", &relay.id), 1, "{agent}");
    }
}

/// A refused send to `main` reached no one, as any refused send: no
/// receiver, the address as written, and it claims no receipt.
#[test]
fn a_refused_send_to_main_reached_no_one() {
    let mut child = send_to_main(
        "denied",
        2,
        "refused note",
        Some(json!("Error: permission denied")),
    );
    child.push(child_says(3, "done anyway"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![agm_line(4, &[("ap", "[Subagent hand-back] done anyway")])],
    )
    .build_query();
    let relay = by_brief(&built, "refused note");
    assert_eq!(
        (relay.status, relay.to.as_deref(), relay.target.as_deref()),
        ("err", None, Some("main"))
    );
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("[Subagent hand-back] done anyway")
    );
}

/// A send whose result isn't written yet can already be received: its
/// receipt is progress, not the hand-back.
#[test]
fn a_pending_sends_receipt_is_not_the_hand_back() {
    let built = background_home(
        vec![("ap", "tp", send_to_main("wait", 2, "pending note", None))],
        vec![agm_line(3, &[("ap", "pending note")])],
    )
    .build_query();
    let spawn = only(&built, "spawn", "lead", "ap");
    assert_eq!((spawn.result.as_deref(), spawn.done), (None, None));
}

/// A hand-back keyed by the spawning call's id (`senderTaskId`) joins the
/// tag receipts keyed by the agent id.
#[test]
fn a_hand_back_by_call_id_joins_receipts_by_agent_id() {
    let mut child = send_to_main("k1", 2, "keyed note", queued_reply());
    child.push(child_says(4, "done"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![
            agm_line(3, &[("ap", "keyed note")]),
            json!({"type":"user","timestamp":ts(20, 5),"sessionId":"lead",
                "origin":{"kind":"peer","handback":true,"from":"worker","senderTaskId":"tp","body":"handed back by call id"},
                "message":{"role":"user","content":"x"}}),
        ],
    )
    .build_query();
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("handed back by call id")
    );
    let relay = by_brief(&built, "keyed note");
    assert_eq!(received_in(&built, "lead", &relay.id), 1);
}

/// One parent line can batch tags, another agent's among them: each of the
/// agent's tags is read as its own, in place.
#[test]
fn batched_tags_are_read_in_place() {
    let mut child = send_to_main("b1", 2, "batched one", queued_reply());
    child.extend(send_to_main("b2", 3, "batched two", queued_reply()));
    child.push(child_says(4, "batched done"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![agm_line(
            5,
            &[
                ("zz", "someone else"),
                ("ap", "batched one"),
                ("ap", "batched two"),
                ("ap", "[Subagent hand-back] batched done"),
            ],
        )],
    )
    .build_query();
    for brief in ["batched one", "batched two"] {
        let relay = by_brief(&built, brief);
        assert_eq!(received_in(&built, "lead", &relay.id), 1, "{brief}");
    }
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("[Subagent hand-back] batched done")
    );
}

/// A receipt whose body isn't the send's text (escaped here) still pairs
/// by time: it was written between the send and the child's last event.
/// The hand-back, written after that, stays the result.
#[test]
fn an_escaped_receipt_pairs_by_time() {
    let mut child = send_to_main("lt", 2, "a < b", queued_reply());
    child.push(child_says(4, "compared"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![
            agm_line(3, &[("ap", "a &lt; b")]),
            agm_line(5, &[("ap", "[Subagent hand-back] compared")]),
        ],
    )
    .build_query();
    let relay = by_brief(&built, "a < b");
    assert_eq!(received_in(&built, "lead", &relay.id), 1);
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("[Subagent hand-back] compared")
    );
}

/// A send whose receipt isn't in these logs doesn't take a receipt written
/// after the child's last event: that one is the hand-back.
#[test]
fn a_receipt_after_the_childs_last_event_stays_the_hand_back() {
    let mut child = send_to_main("lost", 2, "unreceived note", queued_reply());
    child.push(child_says(4, "wrapped up"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![agm_line(5, &[("ap", "[Subagent hand-back] wrapped up")])],
    )
    .build_query();
    let relay = by_brief(&built, "unreceived note");
    assert_eq!(received_in(&built, "lead", &relay.id), 0);
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("[Subagent hand-back] wrapped up")
    );
}

/// Two sends with the same text claim their receipts in order.
#[test]
fn identical_sends_claim_receipts_in_order() {
    let mut child = send_to_main("same1", 2, "same note", queued_reply());
    child.extend(send_to_main("same2", 4, "same note", queued_reply()));
    child.push(child_says(6, "same done"));
    let built = background_home(
        vec![("ap", "tp", child)],
        vec![
            agm_line(3, &[("ap", "same note")]),
            agm_line(5, &[("ap", "same note")]),
            agm_line(7, &[("ap", "[Subagent hand-back] same done")]),
        ],
    )
    .build_query();
    let mut relays: Vec<&Handoff> = built
        .handoffs
        .iter()
        .filter(|handoff| handoff.kind == "relay" && handoff.from == "ap")
        .collect();
    relays.sort_by_key(|handoff| handoff.at);
    assert_eq!(
        relays.iter().map(|relay| relay.at).collect::<Vec<_>>(),
        [at(20, 2), at(20, 4)]
    );
    let drawn: Vec<&str> = built.tx["lead"]
        .slots
        .iter()
        .filter_map(|slot| match &slot.kind {
            SlotKind::H(id) if relays.iter().any(|relay| &relay.id == id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(drawn, [relays[0].id.as_str(), relays[1].id.as_str()]);
    assert_eq!(
        only(&built, "spawn", "lead", "ap").result.as_deref(),
        Some("[Subagent hand-back] same done")
    );
}

#[test]
fn a_changed_session_reuses_other_sessions_with_stable_source_references() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "root prompt")]);
    home.top("other", &[human("other", ts(0, 0), "other prompt")]);
    let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
    let mut dirty = false;
    let mut texts = Texts::default();
    SESSION_DERIVATIONS.with(|counts| counts.borrow_mut().clear());
    SESSION_DESCRIPTIONS.with(|counts| counts.borrow_mut().clear());
    let first = build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    append_records(
        &home,
        "root",
        &[assistant("root", ts(0, 1), vec![text("root updated")])],
    );
    // Adding a file before existing files changes their positions in the
    // scan. A reused transcript must still read its original source path.
    home.top("aaa", &[human("aaa", ts(0, 0), "new session")]);
    let second = build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    assert!(Arc::ptr_eq(&first.tx["other"], &second.tx["other"]));
    assert!(!Arc::ptr_eq(&first.tx["root"], &second.tx["root"]));
    SESSION_DERIVATIONS.with(|counts| {
        assert_eq!(
            *counts.borrow(),
            BTreeMap::from([
                ("root".to_owned(), 2),
                ("other".to_owned(), 1),
                ("aaa".to_owned(), 1),
            ])
        )
    });
    SESSION_DESCRIPTIONS.with(|counts| {
        assert_eq!(
            *counts.borrow(),
            BTreeMap::from([
                ("root".to_owned(), 2),
                ("other".to_owned(), 1),
                ("aaa".to_owned(), 1),
            ])
        )
    });
    assert_equivalent(&second, &home.build_query(), NOW);
    let other = second.tx["other"]
        .slots
        .iter()
        .find_map(|slot| slot.file.as_ref())
        .unwrap();
    assert!(other.path.ends_with("other.jsonl"));
    fs::remove_file(home.root.join("claude/projects/-work-proj/other.jsonl")).unwrap();
    build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    assert!(!texts.derived_sessions.contains_key("other"));
}

#[test]
fn rewritten_growing_logs_invalidate_cached_prompt_text() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "old prompt")]);
    let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
    let mut dirty = false;
    let mut texts = Texts::default();
    let first = build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    assert_eq!(by_brief(&first, "old prompt").kind, "ask");

    // Rewrite in place to a longer record: growing size alone must never
    // make the offset memoizer reuse the previous line's contents.
    home.top(
        "root",
        &[human(
            "root",
            ts(0, 0),
            "replacement prompt longer than the old prompt",
        )],
    );
    let resumed = build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    assert_eq!(
        by_brief(&resumed, "replacement prompt longer than the old prompt").kind,
        "ask"
    );
    assert_equivalent(&resumed, &home.build_query(), NOW);
}

#[derive(Clone, Debug)]
enum EquivalenceStep {
    Append {
        sid: String,
        records: Vec<Value>,
    },
    PartialStart {
        sid: String,
        prefix: String,
    },
    PartialFinish {
        sid: String,
        suffix: String,
        record: Value,
    },
    NewTop {
        sid: String,
        records: Vec<Value>,
    },
    ChildBeforeSpawn {
        parent: String,
        agent: String,
        tool_id: String,
        records: Vec<Value>,
    },
    SpawnLine {
        parent: String,
        records: Vec<Value>,
    },
    ToolUse {
        sid: String,
        record: Value,
    },
    ToolResult {
        sid: String,
        record: Value,
    },
    Truncate {
        sid: String,
        records: Vec<Value>,
    },
    PidWrite {
        pid: u32,
        sid: String,
    },
    PidRemove {
        pid: u32,
    },
    PidDies {
        pid: u32,
    },
    CopyResume {
        source: String,
        target: String,
        continued: Value,
        records: Vec<Value>,
    },
    LockReleased {
        id: String,
    },
    CodexPair {
        parent: String,
        child: String,
        records: Vec<Value>,
    },
    CodexAppend {
        sid: String,
        records: Vec<Value>,
    },
}

#[derive(Clone, Copy)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

const EQUIVALENCE_STEPS: usize = 20;
const EQUIVALENCE_FILES: usize = 10;
const EQUIVALENCE_KINDS: usize = 17;

fn equivalence_root(seed: u64) -> Value {
    uuid(
        human("root", ts(0, 0), &format!("root for seed {seed}")),
        &format!("eq-{seed}-0"),
    )
}

fn identified(seed: u64, next_record: &mut u32, record: Value) -> Value {
    *next_record += 1;
    uuid(record, &format!("eq-{seed}-{}", *next_record))
}

fn generated_time(step: usize, offset: usize) -> String {
    ts(18 + (step / 10) as i64, ((step * 3 + offset) % 60) as i64)
}

fn generated_records(
    rng: &mut SplitMix64,
    seed: u64,
    step: usize,
    sid: &str,
    next_record: &mut u32,
) -> Vec<Value> {
    let count = (rng.next() % 3 + 1) as usize;
    (0..count)
        .map(|offset| {
            let time = generated_time(step, offset);
            let body = format!("seed {seed}, step {step}, record {offset}");
            let record = match rng.next() % 3 {
                0 => human(sid, time, &body),
                1 => user(sid, time, &body),
                _ => assistant(sid, time, vec![text(&body)]),
            };
            identified(seed, next_record, record)
        })
        .collect()
}

fn choose_sid(rng: &mut SplitMix64, histories: &BTreeMap<String, Vec<Value>>) -> String {
    let sessions: Vec<&String> = histories.keys().collect();
    sessions[(rng.next() as usize) % sessions.len()].clone()
}

fn note_generated_step(
    step: &EquivalenceStep,
    histories: &mut BTreeMap<String, Vec<Value>>,
    log_files: &mut usize,
    live_pids: &mut BTreeSet<u32>,
    lock_held: &mut bool,
) {
    match step {
        EquivalenceStep::Append { sid, records }
        | EquivalenceStep::SpawnLine {
            parent: sid,
            records,
        } => histories
            .get_mut(sid)
            .unwrap()
            .extend(records.iter().cloned()),
        EquivalenceStep::PartialFinish { sid, record, .. }
        | EquivalenceStep::ToolUse { sid, record }
        | EquivalenceStep::ToolResult { sid, record } => {
            histories.get_mut(sid).unwrap().push(record.clone());
        }
        EquivalenceStep::NewTop { sid, records } => {
            histories.insert(sid.clone(), records.clone());
            *log_files += 1;
        }
        EquivalenceStep::ChildBeforeSpawn { .. } => *log_files += 1,
        EquivalenceStep::Truncate { sid, records } => {
            histories.insert(sid.clone(), records.clone());
        }
        EquivalenceStep::CopyResume {
            source,
            target,
            continued,
            records,
        } => {
            histories.get_mut(source).unwrap().push(continued.clone());
            histories.insert(target.clone(), records.clone());
            *log_files += 1;
        }
        EquivalenceStep::PidWrite { pid, .. } => {
            live_pids.insert(*pid);
        }
        EquivalenceStep::PidRemove { pid } | EquivalenceStep::PidDies { pid } => {
            live_pids.remove(pid);
        }
        EquivalenceStep::LockReleased { .. } => *lock_held = false,
        EquivalenceStep::CodexPair { .. } => *log_files += 2,
        EquivalenceStep::PartialStart { .. } | EquivalenceStep::CodexAppend { .. } => {}
    }
}

fn generated_steps(seed: u64) -> Vec<EquivalenceStep> {
    let mut rng = SplitMix64(seed);
    let mut next_record = 0;
    let mut next_top = 0;
    let mut next_agent = 0;
    let mut log_files = if cfg!(unix) { 2 } else { 1 };
    let mut histories = BTreeMap::from([("root".to_owned(), vec![equivalence_root(seed)])]);
    let mut live_pids = BTreeSet::new();
    let mut lock_held = cfg!(unix);
    let mut steps = Vec::with_capacity(EQUIVALENCE_STEPS);
    let mut pending = None;

    for index in 0..EQUIVALENCE_STEPS {
        if let Some(step) = pending.take() {
            note_generated_step(
                &step,
                &mut histories,
                &mut log_files,
                &mut live_pids,
                &mut lock_held,
            );
            steps.push(step);
            continue;
        }

        let first_kind = (rng.next() % EQUIVALENCE_KINDS as u64) as usize;
        let mut selected = None;
        for offset in 0..EQUIVALENCE_KINDS {
            let kind = (first_kind + offset) % EQUIVALENCE_KINDS;
            let paired_step_fits = index + 1 < EQUIVALENCE_STEPS;
            let candidate = match kind {
                0 => {
                    let sid = choose_sid(&mut rng, &histories);
                    Some((
                        EquivalenceStep::Append {
                            records: generated_records(
                                &mut rng,
                                seed,
                                index,
                                &sid,
                                &mut next_record,
                            ),
                            sid,
                        },
                        None,
                    ))
                }
                1 if paired_step_fits => {
                    let sid = choose_sid(&mut rng, &histories);
                    let record = identified(
                        seed,
                        &mut next_record,
                        user(
                            &sid,
                            generated_time(index, 0),
                            &format!("partial from seed {seed}, step {index}"),
                        ),
                    );
                    let line = record.to_string();
                    let split = line.len() / 2;
                    Some((
                        EquivalenceStep::PartialStart {
                            sid: sid.clone(),
                            prefix: line[..split].to_owned(),
                        },
                        Some(EquivalenceStep::PartialFinish {
                            sid,
                            suffix: line[split..].to_owned(),
                            record,
                        }),
                    ))
                }
                2 if log_files < EQUIVALENCE_FILES => {
                    let sid = format!("session-{next_top}");
                    next_top += 1;
                    Some((
                        EquivalenceStep::NewTop {
                            records: generated_records(
                                &mut rng,
                                seed,
                                index,
                                &sid,
                                &mut next_record,
                            ),
                            sid,
                        },
                        None,
                    ))
                }
                3 if log_files < EQUIVALENCE_FILES && paired_step_fits => {
                    let parent = choose_sid(&mut rng, &histories);
                    let agent = format!("a{next_agent}");
                    next_agent += 1;
                    let tool_id = format!("spawn-{seed}-{index}");
                    let records = vec![identified(
                        seed,
                        &mut next_record,
                        user(
                            &parent,
                            generated_time(index, 0),
                            &format!("child brief for seed {seed}, step {index}"),
                        ),
                    )];
                    // A child can relay to its parent before the spawn result
                    // provides the join; the global link digest must then change.
                    let send_id = format!("child-send-{seed}-{index}");
                    let mut records = records;
                    records.extend([
                        identified(seed, &mut next_record, assistant(&parent, generated_time(index, 1),
                            vec![tool(&send_id, "SendMessage", json!({"to":"main","message":"child progress","type":"message"}))])),
                        identified(seed, &mut next_record, result(&parent, generated_time(index, 2),
                            &send_id, "queued", false, json!({"success":true}))),
                    ]);
                    let spawn = vec![
                        identified(
                            seed,
                            &mut next_record,
                            assistant(
                                &parent,
                                generated_time(index, 1),
                                vec![tool(
                                    &tool_id,
                                    "Agent",
                                    json!({"description":"generated child","prompt":"generated brief"}),
                                )],
                            ),
                        ),
                        identified(
                            seed,
                            &mut next_record,
                            result(
                                &parent,
                                generated_time(index, 2),
                                &tool_id,
                                "launched",
                                false,
                                json!({"status":"async_launched","agentId":agent}),
                            ),
                        ),
                    ];
                    Some((
                        EquivalenceStep::ChildBeforeSpawn {
                            parent: parent.clone(),
                            agent,
                            tool_id,
                            records,
                        },
                        Some(EquivalenceStep::SpawnLine {
                            parent,
                            records: spawn,
                        }),
                    ))
                }
                4 if paired_step_fits => {
                    let sid = choose_sid(&mut rng, &histories);
                    let tool_id = format!("tool-{seed}-{index}");
                    let call = identified(
                        seed,
                        &mut next_record,
                        assistant(
                            &sid,
                            generated_time(index, 0),
                            vec![tool(
                                &tool_id,
                                "Bash",
                                json!({"command":"printf generated"}),
                            )],
                        ),
                    );
                    let result_record = identified(
                        seed,
                        &mut next_record,
                        result(
                            &sid,
                            generated_time(index, 1),
                            &tool_id,
                            "generated result",
                            false,
                            json!({}),
                        ),
                    );
                    Some((
                        EquivalenceStep::ToolUse {
                            sid: sid.clone(),
                            record: call,
                        },
                        Some(EquivalenceStep::ToolResult {
                            sid,
                            record: result_record,
                        }),
                    ))
                }
                5 => {
                    let eligible: Vec<(&String, &Vec<Value>)> = histories
                        .iter()
                        .filter(|(_, records)| records.len() >= 2)
                        .collect();
                    if eligible.is_empty() {
                        None
                    } else {
                        let (sid, records) = eligible[(rng.next() as usize) % eligible.len()];
                        let keep = 1 + (rng.next() as usize % (records.len() - 1));
                        Some((
                            EquivalenceStep::Truncate {
                                sid: sid.clone(),
                                records: records[..keep].to_vec(),
                            },
                            None,
                        ))
                    }
                }
                6 => {
                    let sid = choose_sid(&mut rng, &histories);
                    let pid = 10_000 + (rng.next() % 50_000) as u32;
                    Some((EquivalenceStep::PidWrite { pid, sid }, None))
                }
                7 if !live_pids.is_empty() => {
                    let pids: Vec<u32> = live_pids.iter().copied().collect();
                    Some((
                        EquivalenceStep::PidRemove {
                            pid: pids[(rng.next() as usize) % pids.len()],
                        },
                        None,
                    ))
                }
                8 if !live_pids.is_empty() => {
                    let pids: Vec<u32> = live_pids.iter().copied().collect();
                    Some((
                        EquivalenceStep::PidDies {
                            pid: pids[(rng.next() as usize) % pids.len()],
                        },
                        None,
                    ))
                }
                9 if log_files < EQUIVALENCE_FILES => {
                    let source = choose_sid(&mut rng, &histories);
                    let target = format!("copy-{next_top}");
                    next_top += 1;
                    let continued = identified(
                        seed,
                        &mut next_record,
                        json!({"type":"continued-in","continuedInSessionId":target,"sessionId":source,"timestamp":generated_time(index, 0)}),
                    );
                    let mut records = histories[&source].clone();
                    records.push(identified(
                        seed,
                        &mut next_record,
                        human(
                            &target,
                            generated_time(index, 1),
                            &format!("copied resume for seed {seed}, step {index}"),
                        ),
                    ));
                    Some((
                        EquivalenceStep::CopyResume {
                            source,
                            target,
                            continued,
                            records,
                        },
                        None,
                    ))
                }
                10 if cfg!(unix) && lock_held => Some((
                    EquivalenceStep::LockReleased {
                        id: "lock".to_owned(),
                    },
                    None,
                )),
                11 if log_files < EQUIVALENCE_FILES && paired_step_fits => {
                    let sender = choose_sid(&mut rng, &histories);
                    let receiver = format!("peer-{next_top}");
                    next_top += 1;
                    let msg_id = format!("message-{seed}-{index}");
                    let call_id = format!("send-{seed}-{index}");
                    let body = format!("relay seed {seed} step {index}");
                    Some((
                        EquivalenceStep::NewTop {
                            sid: receiver.clone(),
                            records: vec![identified(
                                seed,
                                &mut next_record,
                                peer(
                                    &receiver,
                                    generated_time(index, 1),
                                    &msg_id,
                                    &sender,
                                    123,
                                    &body,
                                ),
                            )],
                        },
                        Some(EquivalenceStep::Append {
                            sid: sender.clone(),
                            records: vec![
                                identified(
                                    seed,
                                    &mut next_record,
                                    assistant(
                                        &sender,
                                        generated_time(index, 0),
                                        vec![tool(
                                            &call_id,
                                            "SendMessage",
                                            json!({"to":receiver,"message":body,"type":"message"}),
                                        )],
                                    ),
                                ),
                                identified(
                                    seed,
                                    &mut next_record,
                                    result(
                                        &sender,
                                        generated_time(index, 1),
                                        &call_id,
                                        "sent",
                                        false,
                                        json!({"success":true,"msg_id":msg_id}),
                                    ),
                                ),
                            ],
                        }),
                    ))
                }
                12 if paired_step_fits => {
                    let sid = choose_sid(&mut rng, &histories);
                    let call_id = format!("question-{seed}-{index}");
                    Some((
                        EquivalenceStep::ToolUse {
                            sid: sid.clone(),
                            record: identified(
                                seed,
                                &mut next_record,
                                assistant(
                                    &sid,
                                    generated_time(index, 0),
                                    vec![tool(
                                        &call_id,
                                        "AskUserQuestion",
                                        json!({"questions":[question("Ship generated change?", &["Yes", "No"], false)]}),
                                    )],
                                ),
                            ),
                        },
                        Some(EquivalenceStep::ToolResult {
                            sid: sid.clone(),
                            record: identified(
                                seed,
                                &mut next_record,
                                result(
                                    &sid,
                                    generated_time(index, 1),
                                    &call_id,
                                    "Yes",
                                    false,
                                    json!({"answers":{"Ship generated change?":"Yes"}}),
                                ),
                            ),
                        }),
                    ))
                }
                13 if paired_step_fits => {
                    let sid = choose_sid(&mut rng, &histories);
                    Some((
                        EquivalenceStep::Append {
                            sid: sid.clone(),
                            records: vec![identified(
                                seed,
                                &mut next_record,
                                assistant(
                                    &sid,
                                    generated_time(index, 0),
                                    vec![text("Generated work finished. Please review.")],
                                ),
                            )],
                        },
                        Some(EquivalenceStep::Append {
                            sid: sid.clone(),
                            records: vec![identified(
                                seed,
                                &mut next_record,
                                human(&sid, generated_time(index, 1), "Continue after review"),
                            )],
                        }),
                    ))
                }
                14 if log_files < EQUIVALENCE_FILES && paired_step_fits => {
                    let source = choose_sid(&mut rng, &histories);
                    let target = format!("lineage-{next_top}");
                    next_top += 1;
                    let bridge = format!("bridge-{seed}-{index}");
                    Some((
                        EquivalenceStep::Append {
                            sid: source.clone(),
                            records: vec![identified(
                                seed,
                                &mut next_record,
                                json!({"type":"bridge-session","bridgeSessionId":bridge,"sessionId":source}),
                            )],
                        },
                        Some(EquivalenceStep::NewTop {
                            sid: target.clone(),
                            records: vec![
                                identified(
                                    seed,
                                    &mut next_record,
                                    json!({"type":"bridge-session","bridgeSessionId":bridge,"sessionId":target}),
                                ),
                                identified(
                                    seed,
                                    &mut next_record,
                                    json!({"type":"user","sessionId":target,"session_id":source,"timestamp":generated_time(index, 1),
                                    "origin":{"kind":"human"},"message":{"role":"user","content":"generated clear/resume"}}),
                                ),
                            ],
                        }),
                    ))
                }
                15 if log_files + 2 <= EQUIVALENCE_FILES && paired_step_fits => {
                    let parent = format!("codex-{next_top}");
                    let child = format!("codex-child-{next_top}");
                    next_top += 1;
                    let call_id = format!("codex-spawn-{seed}-{index}");
                    let name = format!("worker-{index}");
                    Some((
                        EquivalenceStep::CodexPair {
                            parent: parent.clone(),
                            child: child.clone(),
                            records: vec![
                                codex_user(
                                    generated_time(index, 0),
                                    "Coordinate generated Codex child",
                                ),
                                codex_line(
                                    generated_time(index, 1),
                                    "response_item",
                                    json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":call_id,
                                    "arguments":json!({"task_name":name,"message":"generated Codex work"}).to_string()}),
                                ),
                                codex_line(
                                    generated_time(index, 1),
                                    "response_item",
                                    json!({"type":"function_call_output","call_id":call_id,"output":json!({"task_name":name}).to_string()}),
                                ),
                                codex_line(
                                    generated_time(index, 1),
                                    "response_item",
                                    json!({"type":"function_call","namespace":"collaboration","name":"send_message",
                                        "call_id":format!("relay-{call_id}"), "arguments":json!({"target":format!("{parent}/{name}"),"message":"generated Codex relay"}).to_string()}),
                                ),
                                codex_line(
                                    generated_time(index, 1),
                                    "response_item",
                                    json!({"type":"function_call_output","call_id":format!("relay-{call_id}"),"output":"sent"}),
                                ),
                            ],
                        },
                        Some(EquivalenceStep::CodexAppend {
                            sid: parent.clone(),
                            records: vec![codex_line(
                                generated_time(index, 2),
                                "response_item",
                                json!({"type":"agent_message","author":format!("{parent}/{name}"),"recipient":parent,
                                "content":[{"type":"input_text","text":"generated Codex hand-back"}]}),
                            )],
                        }),
                    ))
                }
                16 if paired_step_fits => {
                    let sid = choose_sid(&mut rng, &histories);
                    let call_id = format!("background-{seed}-{index}");
                    Some((
                        EquivalenceStep::ToolUse {
                            sid: sid.clone(),
                            record: identified(
                                seed,
                                &mut next_record,
                                assistant(
                                    &sid,
                                    generated_time(index, 0),
                                    vec![tool(
                                        &call_id,
                                        "Bash",
                                        json!({"command":"sleep 1","run_in_background":true}),
                                    )],
                                ),
                            ),
                        },
                        Some(EquivalenceStep::Append {
                            sid: sid.clone(),
                            records: vec![
                                identified(
                                    seed,
                                    &mut next_record,
                                    result(
                                        &sid,
                                        generated_time(index, 1),
                                        &call_id,
                                        "running",
                                        false,
                                        json!({"backgroundTaskId":call_id}),
                                    ),
                                ),
                                identified(
                                    seed,
                                    &mut next_record,
                                    notification(
                                        &sid,
                                        generated_time(index, 2),
                                        &call_id,
                                        "completed",
                                        "background done",
                                    ),
                                ),
                            ],
                        }),
                    ))
                }
                _ => None,
            };
            if let Some((step, next)) = candidate {
                selected = Some((step, next));
                break;
            }
        }

        let (step, next) = selected.unwrap_or_else(|| {
            let sid = choose_sid(&mut rng, &histories);
            (
                EquivalenceStep::Append {
                    records: generated_records(&mut rng, seed, index, &sid, &mut next_record),
                    sid,
                },
                None,
            )
        });
        note_generated_step(
            &step,
            &mut histories,
            &mut log_files,
            &mut live_pids,
            &mut lock_held,
        );
        steps.push(step);
        pending = next;
    }

    assert!(pending.is_none());
    steps
}

fn append_bytes(home: &Home, relative: &str, content: &str) {
    let path = home.root.join(relative);
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
}

fn append_records(home: &Home, sid: &str, records: &[Value]) {
    let content = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    append_bytes(
        home,
        &format!("claude/projects/-work-proj/{sid}.jsonl"),
        &content,
    );
}

fn apply_equivalence_step(home: &Home, step: &EquivalenceStep) {
    match step {
        EquivalenceStep::Append { sid, records }
        | EquivalenceStep::SpawnLine {
            parent: sid,
            records,
        } => append_records(home, sid, records),
        EquivalenceStep::PartialStart { sid, prefix } => append_bytes(
            home,
            &format!("claude/projects/-work-proj/{sid}.jsonl"),
            prefix,
        ),
        EquivalenceStep::PartialFinish { sid, suffix, .. } => append_bytes(
            home,
            &format!("claude/projects/-work-proj/{sid}.jsonl"),
            &format!("{suffix}\n"),
        ),
        EquivalenceStep::NewTop { sid, records } => {
            home.top(sid, records);
        }
        EquivalenceStep::ChildBeforeSpawn {
            parent,
            agent,
            tool_id,
            records,
        } => home.agent(parent, agent, tool_id, records),
        EquivalenceStep::ToolUse { sid, record } | EquivalenceStep::ToolResult { sid, record } => {
            append_records(home, sid, std::slice::from_ref(record));
        }
        EquivalenceStep::Truncate { sid, records } => {
            home.lines(&format!("claude/projects/-work-proj/{sid}.jsonl"), records);
        }
        EquivalenceStep::PidWrite { pid, sid } => home.live(*pid, sid, "busy", json!({})),
        EquivalenceStep::PidRemove { pid } => {
            fs::remove_file(home.root.join(format!("claude/sessions/{pid}.json"))).unwrap();
            fs::remove_file(home.root.join(format!("proc/{pid}/stat"))).unwrap();
        }
        EquivalenceStep::PidDies { pid } => {
            fs::remove_file(home.root.join(format!("proc/{pid}/stat"))).unwrap();
        }
        EquivalenceStep::CopyResume {
            source,
            target,
            continued,
            records,
        } => {
            append_records(home, source, std::slice::from_ref(continued));
            home.top(target, records);
        }
        EquivalenceStep::CodexPair {
            parent,
            child,
            records,
        } => {
            home.codex(parent, json!({}), records);
            let name = records[1]["payload"]["arguments"].as_str().unwrap();
            let name: Value = serde_json::from_str(name).unwrap();
            let name = name["task_name"].as_str().unwrap();
            home.codex(
                child,
                json!({"parent_thread_id":parent,"thread_source":"subagent",
                "agent_nickname":name,"agent_path":format!("{parent}/{name}")}),
                &[
                    codex_user(generated_time(0, 1), "generated Codex work"),
                    codex_reply(generated_time(0, 2), "done"),
                ],
            );
        }
        EquivalenceStep::CodexAppend { sid, records } => {
            let content = records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            append_bytes(
                home,
                &format!("codex/sessions/2026/09/24/rollout-{sid}.jsonl"),
                &content,
            );
        }
        EquivalenceStep::LockReleased { id } => {
            #[cfg(unix)]
            {
                let lock_path = home
                    .root
                    .join(format!("codex/thread-writer-locks/{id}.lock"));
                assert!(lock_path.exists());
                // The writer-lock file may remain after the process releases its flock.
                home.write("proc/locks", "");
            }
            #[cfg(not(unix))]
            {
                let _ = id;
                unreachable!("Codex writer locks are only generated on Unix");
            }
        }
    }
}

fn assert_equivalent(resumed: &QueryData, fresh: &QueryData, now: i64) {
    assert_eq!(semantic_values(resumed), semantic_values(fresh));
    assert_eq!(format!("{:?}", resumed.tx), format!("{:?}", fresh.tx));
    assert_eq!(resumed.facts, fresh.facts);
    assert_eq!(
        format!("{:?}", native_activity(resumed, now)),
        format!("{:?}", native_activity(fresh, now))
    );
    assert_eq!(resumed.home, fresh.home);
    assert_eq!(resumed.machine_id, fresh.machine_id);
}

fn build_with_cache(
    options: &Options,
    cache: &mut EventCache,
    dirty: &mut bool,
    texts: &mut Texts,
    now: i64,
) -> QueryData {
    if options.facts.is_none() {
        cache
            .refresh_reported_runs(&options.claude_json, now, dirty)
            .unwrap();
    }
    let built = build_query(options, cache, dirty, texts, now).unwrap();
    invariants(&built, options.all || options.scan_window);
    built
}

fn with_equivalence_context<T>(
    seed: u64,
    index: usize,
    steps: &[EquivalenceStep],
    action: impl FnOnce() -> T,
) -> T {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
    match outcome {
        Ok(value) => value,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic");
            panic!("seed={seed}, step={index}, steps={steps:#?}\n{detail}");
        }
    }
}

fn replay(seed: u64) {
    let home = Home::new();
    home.top("root", &[equivalence_root(seed)]);
    #[cfg(unix)]
    {
        home.codex("lock", json!({}), &[codex_user(ts(0, 0), "locked fixture")]);
        hold_lock(&home, "lock");
    }
    let steps = generated_steps(seed);
    let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
    let mut dirty = false;
    let mut texts = Texts::default();

    // Seed the long-lived event cache before the first append, so step zero
    // reads the persisted ledger's offsets in its restarted-cache build.
    with_equivalence_context(seed, 0, &steps, || {
        build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    });

    for (index, step) in steps.iter().enumerate() {
        with_equivalence_context(seed, index, &steps, || {
            apply_equivalence_step(&home, step);
            let resumed = home.build_at(&home.options, NOW);
            let long_lived =
                build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
            let mut fresh_options = home.options.clone();
            fresh_options.cache = home.root.join(format!("fresh-{index}.json"));
            let fresh = home.build_at(&fresh_options, NOW);

            assert_equivalent(&resumed, &fresh, NOW);
            assert_equivalent(&long_lived, &fresh, NOW);

            let unchanged =
                build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
            assert_equivalent(&long_lived, &unchanged, NOW);
        });
    }
}

#[test]
fn append_resumed_build_matches_fresh_build_for_seeded_sequences() {
    for seed in 0..64 {
        replay(seed);
    }
}

#[test]
#[ignore = "extended seeded append-resume equivalence run"]
fn append_resumed_build_matches_fresh_build_for_1000_seeds() {
    for seed in 0..1000 {
        replay(seed);
    }
}

#[test]
#[ignore = "set SEMON_EQUIV_SEED=<n> to replay one generated case"]
fn replay_equivalence_seed_from_env() {
    let seed = std::env::var("SEMON_EQUIV_SEED")
        .expect("set SEMON_EQUIV_SEED to the seed to replay")
        .parse()
        .expect("SEMON_EQUIV_SEED must be an unsigned integer");
    replay(seed);
}

#[test]
fn unchanged_native_queries_keep_recorded_facts_across_clock_changes() {
    let home = Home::new();
    home.top(
        "clock",
        &[
            human("clock", ts(23, 58), "start"),
            assistant(
                "clock",
                ts(23, 59),
                vec![tool(
                    "clock-pending",
                    "Bash",
                    json!({"command":"printf clock"}),
                )],
            ),
        ],
    );
    home.live(42, "clock", "busy", json!({}));

    let mut options = home.options.clone();
    options.all = false;
    options.since = Duration::from_secs(30 * 24 * 60 * 60);
    let first = home.build_at(&options, NOW);
    assert_equivalent(&first, &home.build_at(&options, NOW), NOW);
    let later = home.build_at(&options, NOW + 60_000);
    assert_equivalent(&first, &later, NOW + 60_000);
    assert!(
        first
            .sessions
            .values()
            .any(|session| session.activity.is_some())
    );
}

#[test]
fn resumed_process_start_invalidates_cached_background_liveness() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(1, 0), "Start a worker"),
            assistant(
                "root",
                ts(1, 1),
                vec![tool(
                    "shell",
                    "Bash",
                    json!({"command":"sleep 60","run_in_background":true}),
                )],
            ),
            result(
                "root",
                ts(1, 1),
                "shell",
                "Running in background",
                false,
                json!({}),
            ),
        ],
    );
    home.live(30, "root", "busy", json!({"startedAt":at(1, 0)}));
    let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
    let mut dirty = false;
    let mut texts = Texts::default();
    let before = build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    let live = |built: &QueryData| {
        built.tx["root"]
            .slots
            .iter()
            .find_map(|slot| match &slot.kind {
                SlotKind::Tool { bg: Some(bg), .. } => Some(bg.live),
                _ => None,
            })
            .unwrap()
    };
    assert!(live(&before));
    home.live(30, "root", "busy", json!({"startedAt":at(1, 2)}));
    let after = build_with_cache(&home.options, &mut cache, &mut dirty, &mut texts, NOW);
    assert!(!live(&after));
    assert!(!Arc::ptr_eq(&before.tx["root"], &after.tx["root"]));
    assert_equivalent(&after, &home.build_query(), NOW);
}

#[test]
fn native_claude_copied_usage_keeps_observations_without_assigning_owner() {
    let home = Home::new();
    let parent = include_str!(
        "../../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/resumed-transcript.jsonl"
    );
    let child = include_str!(
        "../../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/forked-transcript.jsonl"
    );
    home.write("claude/projects/fixture/native-claude-parent.jsonl", parent);
    let alone = home.build_query();
    assert_eq!(
        alone.sessions["native-claude-parent"].tokens_by_model["claude-sonnet-4-6"].input,
        10
    );
    assert!(
        alone.sessions["native-claude-parent"]
            .claude_usage
            .is_none()
    );
    let path = home.write("claude/projects/fixture/native-claude-child.jsonl", child);
    let assert_evidence = |built: &QueryData| {
        let parent = &built.sessions["native-claude-parent"];
        let child = &built.sessions["native-claude-child"];
        assert!(parent.parent.is_none() && child.parent.is_none());
        let p = parent.claude_usage.as_ref().unwrap();
        let c = child.claude_usage.as_ref().unwrap();
        assert_eq!((p.observed.input, p.observed.output), (10, 6));
        assert_eq!((c.observed.input, c.observed.output), (15, 9));
        assert_eq!(p.exclusive, crate::Tokens::default());
        assert_eq!((c.exclusive.input, c.exclusive.output), (5, 3));
        assert_eq!(p.shared, c.shared);
        assert_eq!(
            p.shared.values().map(|t| t.input).sum::<u64>() + c.exclusive.input,
            15
        );
        assert_eq!(
            p.shared.values().map(|t| t.output).sum::<u64>() + c.exclusive.output,
            9
        );
        assert!(p.fresh.is_none() && c.fresh.is_none());
        assert!(p.shared_owner.is_none() && c.shared_owner.is_none());
        assert!(parent.tokens_by_model.is_empty());
        assert_eq!(child.tokens_by_model["claude-sonnet-4-6"].input, 5);
        assert!(parent.cost.usd.is_none() && child.cost.usd.is_none());
    };
    assert_evidence(&home.build_query());
    assert_evidence(&home.build_query()); // Durable SQLite restart.
    let mut cold = home.options.clone();
    cold.cache = home.root.join("cold/index.json");
    assert_evidence(&home.build_at(&cold, NOW));
    assert_eq!(fs::read_to_string(path).unwrap(), child);
    // Removing the counterpart changes ownership availability, never invents
    // lineage or remembers a now-unobservable original branch owner.
    fs::remove_file(
        home.root
            .join("claude/projects/fixture/native-claude-child.jsonl"),
    )
    .unwrap();
    assert!(
        home.build_query().sessions["native-claude-parent"]
            .claude_usage
            .is_none()
    );
}

#[test]
fn persisted_descriptions_reuse_restart_and_revalidate_changed_sources() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "secret prompt body")]);
    home.top("other", &[human("other", ts(0, 0), "other prompt")]);
    let first = home.build_query();
    SESSION_DESCRIPTIONS.with(|counts| counts.borrow_mut().clear());
    let restarted = home.build_query();
    assert_equivalent(&first, &restarted, NOW);
    SESSION_DESCRIPTIONS.with(|counts| assert!(counts.borrow().is_empty()));

    append_records(
        &home,
        "root",
        &[assistant(
            "root",
            ts(0, 1),
            vec![text("secret answer body")],
        )],
    );
    let updated = home.build_query();
    SESSION_DESCRIPTIONS
        .with(|counts| assert_eq!(*counts.borrow(), BTreeMap::from([("root".into(), 1)])));
    let mut fresh = EventCache::default();
    let oracle = build_with_cache(
        &home.options,
        &mut fresh,
        &mut false,
        &mut Texts::default(),
        NOW,
    );
    assert_equivalent(&updated, &oracle, NOW);

    let connection =
        rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    let rows: Vec<String> = connection
        .prepare("SELECT description FROM session_descriptions")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert!(!row.contains("secret prompt body"));
        assert!(!row.contains("secret answer body"));
        let value: Value = serde_json::from_str(&row).unwrap();
        assert!(!value["sources"].as_array().unwrap().is_empty());
        for forbidden in [
            "state",
            "activity",
            "waiting_for",
            "waiting_since",
            "alive",
            "turns",
            "handoffs",
            "tx",
        ] {
            assert!(value.get(forbidden).is_none(), "persisted {forbidden}");
        }
    }

    fs::remove_file(home.root.join("claude/projects/-work-proj/other.jsonl")).unwrap();
    assert!(!home.build_query().sessions.contains_key("other"));
}

#[test]
fn incompatible_or_damaged_description_rebuilds_without_losing_observations() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "prompt")]);
    let first = home.build_query();
    let connection =
        rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    connection
        .execute("UPDATE session_descriptions SET version = version + 1", [])
        .unwrap();
    SESSION_DESCRIPTIONS.with(|counts| counts.borrow_mut().clear());
    assert_equivalent(&first, &home.build_query(), NOW);
    SESSION_DESCRIPTIONS.with(|counts| assert_eq!(counts.borrow().get("root"), Some(&1)));
    connection
        .execute("UPDATE session_descriptions SET description = '{}'", [])
        .unwrap();
    SESSION_DESCRIPTIONS.with(|counts| counts.borrow_mut().clear());
    assert_equivalent(&first, &home.build_query(), NOW);
    SESSION_DESCRIPTIONS.with(|counts| assert_eq!(counts.borrow().get("root"), Some(&1)));
}

#[test]
#[ignore = "measurement workload: 512 sessions × 128 assistant usage records"]
fn persisted_description_restart_measurement() {
    let home = Home::new();
    for index in 0..512 {
        let id = format!("session-{index:04}");
        let mut records = vec![human(&id, ts(0, 0), "prompt")];
        for message in 0..128 {
            records.push(assistant_usage(
                &id,
                ts(0, 1),
                &format!("message-{index}-{message}"),
                "claude-sonnet-4-5",
                json!({"input_tokens": 1000, "output_tokens": 200, "cache_read_input_tokens": 500}),
            ));
        }
        home.top(&id, &records);
    }
    home.build_query();
    for reuse in [false, true, false, true, false, true] {
        if !reuse {
            rusqlite::Connection::open(home.options.cache.with_extension("sqlite3"))
                .unwrap()
                .execute("DELETE FROM session_descriptions", [])
                .unwrap();
        }
        SESSION_DESCRIPTIONS.with(|counts| counts.borrow_mut().clear());
        let start = std::time::Instant::now();
        let built = home.build_query();
        eprintln!(
            "summary_reuse={reuse} total_ms={} sessions={}",
            start.elapsed().as_millis(),
            built.sessions.len()
        );
        if reuse {
            SESSION_DESCRIPTIONS.with(|counts| assert!(counts.borrow().is_empty()));
        }
    }
}

fn read_catalog(home: &Home) -> (String, Vec<summary::CatalogRow>) {
    let connection =
        rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    let generation = connection
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let rows = connection
        .prepare("SELECT metadata FROM session_catalog ORDER BY session_key")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect();
    (generation, rows)
}

#[test]
fn catalog_observation_preserves_native_query_projections() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(0, 0), "native prompt"),
            assistant(
                "root",
                ts(0, 1),
                vec![tool("spawn", "Agent", json!({"prompt":"child task"}))],
            ),
        ],
    );
    home.agent(
        "root",
        "child",
        "spawn",
        &[user("root", ts(0, 2), "child prompt")],
    );
    home.codex("codex-root", json!({}), &[]);
    home.codex("codex-fork", json!({"forked_from_id":"codex-root"}), &[]);
    home.live(42, "root", "working", json!({}));
    let projection = || {
        let catalog = read_catalog(&home);
        let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let mut statement = connection
            .prepare("SELECT metadata FROM session_slots ORDER BY session_key,slot")
            .unwrap();
        let slots: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        (catalog.0, serde_json::to_string(&catalog.1).unwrap(), slots)
    };
    let observe = || {
        let pids = observe_catalog(
            &home.options,
            &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
            &mut false,
            &mut Texts::default(),
            NOW,
        )
        .unwrap();
        assert!(pids.contains(&42));
    };
    home.build_query();
    let original = projection();
    observe();
    assert_eq!(projection(), original);
    append_records(
        &home,
        "root",
        &[assistant("root", ts(0, 3), vec![text("new source answer")])],
    );
    observe();
    let appended = projection();
    assert_ne!(appended.0, original.0);
    home.build_query();
    assert_eq!(projection(), appended);
    let reply = crate::session_transcript_range(
        &home.options,
        "fixture",
        "sid=root&after=0&limit=100",
        None,
    );
    assert_eq!(reply.status, 200);
    assert!(String::from_utf8_lossy(&reply.body).contains("new source answer"));
    fs::remove_file(
        home.root
            .join("claude/projects/-work-proj/root/subagents/agent-child.jsonl"),
    )
    .unwrap();
    observe();
    let removed = projection();
    assert!(
        read_catalog(&home)
            .1
            .iter()
            .any(|row| row.lifecycle == summary::CatalogLifecycle::Retained)
    );
    home.build_query();
    assert_eq!(projection(), removed);
    let mut inputs = crate::inputs::inputs(&home.options).unwrap();
    inputs.retain(|input| input.path.ends_with(".jsonl"));
    prepare_sources(
        &home.options,
        &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
        &inputs,
        NOW,
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn unsupported_portable_path_preserves_native_tree_and_refuses_lossy_query_aliases() {
    use std::os::unix::ffi::OsStringExt;
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "prompt")]);
    let projects = home.root.join("claude/projects");
    fs::rename(
        projects.join("-work-proj"),
        projects.join(std::ffi::OsString::from_vec(b"-work-\xff".to_vec())),
    )
    .unwrap();
    // Native tree discovery supports exact non-UTF-8 paths. Portable Catalog
    // and Query must refuse rather than publish a lossy source alias.
    let tree_options = Options {
        cache: home.root.join("tree-index.json"),
        ..home.options.clone()
    };
    let tree = crate::collect(&tree_options).unwrap();
    assert_eq!(tree[0].id, "root");
    assert_eq!(tree[0].last_activity.as_deref(), Some(ts(0, 0).as_str()));
    let query = || {
        build_query(
            &home.options,
            &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
            &mut false,
            &mut Texts::default(),
            NOW,
        )
    };
    assert!(query().is_err());
    let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let ready: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key='catalog_generation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ready, 0);
    assert!(query().is_err());
    let exact_source = fs::read_dir(&projects)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("root.jsonl");
    let original = fs::read(&exact_source).unwrap();
    assert!(query().is_err());
    assert_eq!(fs::read(&exact_source).unwrap(), original);
    assert_eq!(crate::collect(&tree_options).unwrap()[0].id, "root");
}

#[test]
fn complete_catalog_is_untrimmed_stable_and_preserves_native_source_and_parent() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(0, 0), "private prompt"),
            assistant(
                "root",
                ts(0, 1),
                vec![tool(
                    "spawn",
                    "Agent",
                    json!({"prompt":"private tool body"}),
                )],
            ),
        ],
    );
    home.agent(
        "root",
        "child",
        "spawn",
        &[user("root", ts(0, 2), "private child prompt")],
    );
    let built = home.build_query();
    let (generation, rows) = read_catalog(&home);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let expected = &built.sessions[&row.key];
        assert_eq!(row.parent, expected.parent);
        assert_eq!(row.name, expected.name);
        assert_eq!(row.harness, expected.harness);
        assert_eq!(row.start, Some(expected.start));
        assert_eq!(row.last, Some(expected.last));
        assert!(!row.native_ids.is_empty());
        assert_eq!(row.sources.len(), 1);
        let source = &row.sources[0];
        assert_eq!(source.size, fs::metadata(&source.path).unwrap().len());
        assert!(source.offset > 0);
        let json = serde_json::to_string(row).unwrap();
        assert!(!json.contains("private prompt"));
        assert!(!json.contains("private tool body"));
        assert!(!json.contains("private child prompt"));
    }
    home.build_query();
    assert_eq!(generation, read_catalog(&home).0);
    let mut window = home.options.clone();
    window.all = false;
    window.since = Duration::from_secs(1);
    home.build_at(&window, NOW);
    assert_eq!(generation, read_catalog(&home).0);
    assert_eq!(read_catalog(&home).1.len(), 2);

    fs::remove_file(
        home.root
            .join("claude/projects/-work-proj/root/subagents/agent-child.jsonl"),
    )
    .unwrap();
    home.build_query();
    let (next, rows) = read_catalog(&home);
    assert_ne!(generation, next);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().find(|row| row.key == "root").unwrap().lifecycle,
        super::summary::CatalogLifecycle::Current
    );
    assert_eq!(
        rows.iter().find(|row| row.key != "root").unwrap().lifecycle,
        super::summary::CatalogLifecycle::Retained
    );
}

#[test]
fn scan_window_never_replaces_a_complete_catalog_and_unknown_times_remain_unknown() {
    let home = Home::new();
    home.top("root", &[json!({"type":"user","sessionId":"root","message":{"role":"user","content":"prompt without a timestamp"}})]);
    home.build_query();
    let (generation, rows) = read_catalog(&home);
    assert_eq!(rows[0].start, None);
    assert_eq!(rows[0].last, None);
    let mut window = home.options.clone();
    window.scan_window = true;
    home.build_at(&window, NOW + 60_000);
    assert_eq!(generation, read_catalog(&home).0);
    assert_eq!(read_catalog(&home).1.len(), 1);
}

#[test]
fn isolated_catalog_publication_fences_competing_generations_and_pending_sources() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "prompt")]);
    home.top("other", &[human("other", ts(0, 0), "other prompt")]);
    home.build_query();
    let input = |id: &str| crate::inputs::Input {
        root: crate::inputs::InputRoot::Claude,
        path: format!("projects/-work-proj/{id}.jsonl"),
    };
    home.top(
        "root",
        &[
            human("root", ts(0, 0), "prompt"),
            assistant("root", ts(0, 1), vec![text("new answer")]),
        ],
    );
    let mut root_cache =
        EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
    let prepared = prepare_sources(&home.options, &mut root_cache, &[input("root")], NOW).unwrap();
    let before = read_catalog(&home);
    home.top(
        "other",
        &[
            human("other", ts(0, 0), "other prompt"),
            assistant("other", ts(0, 2), vec![text("other answer")]),
        ],
    );
    let mut other_cache =
        EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
    let other = prepare_sources(&home.options, &mut other_cache, &[input("other")], NOW).unwrap();
    assert_eq!(
        root_cache.publish_closed_catalog(&prepared).unwrap(),
        Some(crate::events::Outcome::Written),
        "a closed component must commit while unrelated evidence stays pending"
    );
    assert_ne!(
        read_catalog(&home).0,
        before.0,
        "closed publication advances its fenced generation"
    );
    let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_invalidations WHERE source_path=?1",
            [input("other")
                .full_path(&home.options)
                .to_string_lossy()
                .as_ref()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1, "unrelated source remains durable pending work");
    assert_eq!(
        other_cache.publish_closed_catalog(&other).unwrap(),
        Some(crate::events::Outcome::Conflict),
        "a prepared component must retry after a competing publication advances the generation"
    );
    observe_catalog(
        &home.options,
        &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
        &mut false,
        &mut Texts::default(),
        NOW,
    )
    .unwrap();
    assert_eq!(
        root_cache.publish_closed_catalog(&prepared).unwrap(),
        Some(crate::events::Outcome::Conflict),
        "a newer complete generation fences an older component"
    );
    let (_, rows) = read_catalog(&home);
    assert_eq!(
        rows.iter().find(|row| row.key == "other").unwrap().last,
        Some(at(0, 2))
    );
    assert_eq!(
        rows.iter().find(|row| row.key == "root").unwrap().last,
        Some(at(0, 1))
    );
}

#[test]
fn interrupted_catalog_publication_retains_one_generation_and_recovers_on_restart() {
    let home = Home::new();
    for id in ["aaa", "zzz"] {
        home.top(id, &[human(id, ts(0, 0), "prompt")]);
    }
    home.build_query();
    let before = read_catalog(&home);
    let connection =
        rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER interrupt_catalog BEFORE UPDATE ON session_catalog WHEN NEW.session_key='zzz' BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
    for id in ["aaa", "zzz"] {
        append_records(
            &home,
            id,
            &[assistant(id, ts(0, 1), vec![text("new answer")])],
        );
    }
    home.build_query();
    let retained = read_catalog(&home);
    assert_eq!(before.0, retained.0);
    assert_eq!(
        serde_json::to_string(&before.1).unwrap(),
        serde_json::to_string(&retained.1).unwrap()
    );
    let aliases: i64 = connection
        .query_row("SELECT COUNT(*) FROM session_catalog_sources", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(aliases, 2);
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_invalidations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 2);
    let native = observe_catalog(
        &home.options,
        &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
        &mut false,
        &mut Texts::default(),
        NOW,
    );
    assert!(
        native.is_err(),
        "a rejected publication cannot acknowledge a complete native observation"
    );
    connection
        .execute_batch("DROP TRIGGER interrupt_catalog")
        .unwrap();
    observe_catalog(
        &home.options,
        &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
        &mut false,
        &mut Texts::default(),
        NOW,
    )
    .unwrap();
    home.build_query();
    let recovered = read_catalog(&home);
    assert_ne!(before.0, recovered.0);
    assert!(recovered.1.iter().all(|row| row.last == Some(at(0, 1))));
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_invalidations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0);
}

#[test]
fn partial_lineage_source_disappearance_preserves_natural_history_context() {
    let home = Home::new();
    home.top(
        "root",
        &[uuid(
            human("root", ts(0, 0), "original root prompt"),
            "root-event",
        )],
    );
    home.top("cleared", &[json!({"type":"user","uuid":"cleared-event","timestamp":ts(0,1),"sessionId":"cleared","session_id":"root","origin":{"kind":"human"},"message":{"role":"user","content":"original cleared prompt"}})]);
    home.build_query();
    let (_, before) = read_catalog(&home);
    let root = before.iter().find(|row| row.key == "root").unwrap();
    assert_eq!(root.sources.len(), 2);
    let removed = root
        .sources
        .iter()
        .find(|source| source.native_id == "cleared")
        .unwrap()
        .clone();
    let before_identity = crate::session_catalog_history_identity(&home.options, "fixture", "root")
        .unwrap()
        .unwrap();
    assert_eq!(before_identity.source_refs.len(), 2);
    let original = fs::read(&removed.path).unwrap();
    let before_body = crate::session_transcript_range(
        &home.options,
        "fixture",
        "sid=root&scope=retained_history&after=0&limit=100",
        None,
    );
    assert_eq!(before_body.status, 200);
    let before_body: Value = serde_json::from_slice(&before_body.body).unwrap();
    let original_entry = before_body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "original cleared prompt")
        .unwrap()
        .clone();
    let original_mcp = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![("fixture".into(), home.options.clone())]),
    ))
    .call_selected_at(
        "fixture",
        "root",
        "read_transcript",
        &json!({"id":"root","after":0,"limit":100}),
        None,
        crate::SessionSourceReadMode::LocalThenProvider,
        crate::model::now_ms(),
    )
    .unwrap();
    fs::remove_file(&removed.path).unwrap();
    let current = home.build_query();
    assert!(current.sessions.contains_key("root"));
    let (_, after) = read_catalog(&home);
    let root = after.iter().find(|row| row.key == "root").unwrap();
    assert_eq!(root.lifecycle, super::summary::CatalogLifecycle::Current);
    assert_eq!(root.sources.len(), 1);
    let history = crate::session_catalog_history_identity(&home.options, "fixture", "root")
        .unwrap()
        .unwrap();
    assert_eq!(
        history.source_refs.len(),
        2,
        "a current canonical owner must retain its disappeared native source for natural History reads"
    );
    assert!(
        history
            .source_refs
            .iter()
            .any(|source| source.source.native_id == "cleared"
                && source.source.prefix_sha256 == removed.prefix_sha256)
    );
    let ready = crate::source_projection_ready(
        &home.options,
        "claude",
        "projects/-work-proj/cleared.jsonl",
    )
    .unwrap()
    .unwrap();
    assert_eq!(ready.lifecycle, "retained");
    assert_eq!(ready.source.prefix_sha256, removed.prefix_sha256);
    struct Archive {
        bytes: Vec<u8>,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl crate::SessionSourceReader for Archive {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            assert_eq!(source.native_id, "cleared");
            self.calls.fetch_add(1, Ordering::Relaxed);
            let generation = format!("{:x}", sha2::Sha256::digest(&self.bytes));
            assert!(expected.is_none_or(|expected| expected == generation));
            let end = (offset as usize + max).min(self.bytes.len());
            Ok(crate::SessionSourceRange {
                generation,
                length: self.bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(&self.bytes[offset as usize..end]),
                cached: false,
            })
        }
    }
    use sha2::Digest;
    let archive = Archive {
        bytes: original,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let read = || {
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let reply = crate::session_transcript_range_with_mode(
            &home.options,
            "fixture",
            "sid=root&scope=retained_history&after=0&limit=100",
            Some(&archive),
            crate::SessionSourceReadMode::LocalThenProvider,
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
        serde_json::from_slice::<Value>(&reply.body).unwrap()
    };
    let retained = read();
    assert_eq!(
        retained["session"]["summary_context"],
        json!({"state":"incomplete","scope":"current_observation"})
    );
    let entry = retained["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "original cleared prompt")
        .unwrap();
    assert_eq!(entry["entry_id"], original_entry["entry_id"]);
    assert_eq!(entry["provenance"], original_entry["provenance"]);
    assert!(archive.calls.load(Ordering::Relaxed) > 0);
    assert!(!removed.path.exists());
    let current_identity = crate::session_catalog_identity(&home.options, "fixture", "root")
        .unwrap()
        .unwrap();
    assert_eq!(current_identity.source_refs.len(), 1);
    assert_eq!(current_identity.native_id.as_deref(), Some("root"));
    let calls = archive.calls.load(Ordering::Relaxed);
    let current = crate::session_transcript_range_with_mode(
        &home.options,
        "fixture",
        "sid=root&scope=current&after=0&limit=100",
        Some(&archive),
        crate::SessionSourceReadMode::LocalThenProvider,
    );
    assert_eq!(current.status, 200);
    let current: Value = serde_json::from_slice(&current.body).unwrap();
    assert_eq!(
        current["identity"]["source_refs"].as_array().unwrap().len(),
        1
    );
    assert_eq!(current["entries"].as_array().unwrap().len(), 1);
    assert_eq!(current["entries"][0]["text"], "original root prompt");
    assert_ne!(
        current["projection"]["generation"],
        retained["projection"]["generation"]
    );
    assert_eq!(archive.calls.load(Ordering::Relaxed), calls);
    let history_cursor = retained["projection"]["generation"].as_str().unwrap();
    assert_eq!(
        crate::session_transcript_range_with_mode(
            &home.options,
            "fixture",
            &format!("sid=root&scope=current&after=1&limit=1&generation={history_cursor}"),
            Some(&archive),
            crate::SessionSourceReadMode::LocalThenProvider,
        )
        .status,
        409,
        "History cursor must not select Current recipes"
    );
    // Current work can advance while the original disappeared path stays part
    // of natural history after another complete observation and cold restart.
    let root_path = home
        .options
        .claude_home
        .join("projects/-work-proj/root.jsonl");
    let mut file = fs::OpenOptions::new().append(true).open(root_path).unwrap();
    writeln!(
        file,
        "{}",
        uuid(
            assistant("root", ts(0, 2), vec![text("new current answer")]),
            "new-current-answer"
        )
    )
    .unwrap();
    drop(file);
    home.build_query();
    home.build_query();
    assert!(
        crate::overview_repair_candidates(&home.options, 8)
            .unwrap()
            .contains(&"root".into())
    );
    let mut metadata_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![("fixture".into(), home.options.clone())]),
    ));
    assert_eq!(
        metadata_query
            .call_catalog(
                "list_sessions",
                &json!({}),
                None,
                crate::SessionSourceReadMode::LocalThenProvider
            )
            .unwrap_err()
            .code,
        "unavailable",
        "a current fragment cannot claim complete history facts"
    );
    assert!(
        crate::repair_session_overview(
            &home.options,
            "fixture",
            "root",
            Some(&archive),
            crate::SessionSourceReadMode::LocalThenProvider
        )
        .unwrap()
    );
    assert!(
        !crate::overview_repair_candidates(&home.options, 8)
            .unwrap()
            .contains(&"root".into())
    );
    let reads = archive.calls.load(Ordering::Relaxed);
    assert!(
        !crate::repair_session_overview(
            &home.options,
            "fixture",
            "root",
            Some(&archive),
            crate::SessionSourceReadMode::LocalThenProvider
        )
        .unwrap()
    );
    assert_eq!(archive.calls.load(Ordering::Relaxed), reads);
    let metadata = metadata_query
        .call_catalog(
            "list_sessions",
            &json!({}),
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
        )
        .unwrap();
    assert_eq!(metadata["sessions"][0]["id"], "root");
    assert_eq!(
        metadata["sessions"][0]["last_activity"],
        events::parse_ms(&ts(0, 2)).unwrap()
    );
    assert_eq!(
        archive.calls.load(Ordering::Relaxed),
        reads,
        "qualified history summaries must not fetch transcript bodies"
    );
    home.build_query();
    assert!(
        !crate::overview_repair_candidates(&home.options, 8)
            .unwrap()
            .contains(&"root".into()),
        "an unchanged current observation must retain exact history metadata"
    );
    let causal = crate::session_causal_trace(
        &home.options,
        "fixture",
        &format!(
            "sid=root&scope=retained_history&turn={}",
            crate::viewer::percent_encode(
                before_body["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["text"] == "original cleared prompt")
                    .unwrap()["turn"]
                    .as_str()
                    .unwrap()
            )
        ),
    );
    assert_eq!(
        causal.status,
        200,
        "{}",
        String::from_utf8_lossy(&causal.body)
    );
    // Turn navigation must use the same materialized history projection as
    // transcript ranges, including the prefix whose local source disappeared.
    let historical_turn = before_body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "original cleared prompt")
        .unwrap()["turn"]
        .as_str()
        .unwrap();
    let anchor = crate::session_turn_anchor(
        &home.options,
        "fixture",
        &format!(
            "sid=root&scope=retained_history&turn={}",
            crate::viewer::percent_encode(historical_turn)
        ),
    );
    assert_eq!(
        anchor.status,
        200,
        "{}",
        String::from_utf8_lossy(&anchor.body)
    );
    let anchor: Value = serde_json::from_slice(&anchor.body).unwrap();
    assert!(anchor["slot"].is_u64(), "{anchor}");
    let absent = crate::session_turn_anchor(
        &home.options,
        "fixture",
        "sid=root&scope=retained_history&turn=not-a-recorded-turn",
    );
    assert_eq!(absent.status, 200);
    let absent: Value = serde_json::from_slice(&absent.body).unwrap();
    assert!(absent["slot"].is_null());
    assert_eq!(
        archive.calls.load(Ordering::Relaxed),
        reads,
        "turn anchor fetched transcript bytes"
    );
    let advanced = read();
    let entries = advanced["entries"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["text"] == "original cleared prompt")
            .count(),
        1
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["text"] == "new current answer")
    );
    // A cold MCP consumer must page the same composite custody: neither a
    // current-only fragment nor a stale whole-session summary is sufficient.
    let mut query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![("fixture".into(), home.options.clone())]),
    ));
    let mut position = 0;
    let mut paged = Vec::new();
    loop {
        let page = query
            .call_selected_at(
                "fixture",
                "root",
                "read_transcript",
                &json!({"id":"root","after":position,"limit":1}),
                Some(&archive),
                crate::SessionSourceReadMode::LocalThenProvider,
                crate::model::now_ms(),
            )
            .unwrap();
        let items = page["entries"].as_array().unwrap();
        if items.is_empty() {
            break;
        }
        assert_eq!(items.len(), 1);
        paged.push(items[0].clone());
        let next = page["to"].as_u64().unwrap();
        assert!(next > position, "composite paging did not advance: {page}");
        position = next;
        assert!(paged.len() <= 3, "composite paging repeated entries");
    }
    assert_eq!(
        &paged[..2],
        original_mcp["entries"].as_array().unwrap().as_slice(),
        "composite paging changed the original native entry identities"
    );
    assert_eq!(paged.len(), 3);
    assert_eq!(paged[2]["text"], "new current answer");
    assert!(!removed.path.exists());
    let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let body_leaks:i64=connection.query_row("SELECT count(*) FROM session_history_slots WHERE instr(metadata,'original cleared prompt')>0 OR instr(metadata,'new current answer')>0",[],|row|row.get(0)).unwrap();
    assert_eq!(body_leaks, 0);
    let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT metadata FROM session_history_slots WHERE session_key='root' AND slot>=0 AND slot<2 ORDER BY slot",[],|row|row.get(3)).unwrap();
    assert!(
        plan.contains("SEARCH session_history_slots USING PRIMARY KEY"),
        "{plan}"
    );
    let ranged = || {
        let mut query=connection.prepare("SELECT metadata FROM session_history_slots WHERE session_key='root' AND slot>=0 AND slot<2 ORDER BY slot").unwrap();
        let rows: Vec<String> = query
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        (rows, query.get_status(rusqlite::StatementStatus::VmStep))
    };
    let baseline = ranged();
    // Isolate ranged storage work: unrelated retained recipe rows, without
    // native-file discovery or a producer build, grow from zero to 20,000.
    connection.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_history_slots(session_key,slot,metadata) SELECT 'unrelated-'||printf('%05d',x),0,metadata FROM n,session_history_slots WHERE session_key='root' AND slot=0").unwrap();
    assert_eq!(ranged(), baseline);
    let grown = read();
    assert_eq!(grown["entries"], advanced["entries"]);
    assert_eq!(grown["projection"], advanced["projection"]);
    assert_eq!(
        grown["observation"]["source_bytes"],
        advanced["observation"]["source_bytes"]
    );
    eprintln!(
        "partial-lineage ranged isolation: unrelated recipe owners 0->20000, fixed slots 0..2, VM steps {}, response bytes {}, selected source bytes {}",
        baseline.1,
        serde_json::to_vec(&grown).unwrap().len(),
        grown["observation"]["source_bytes"]
    );
}

#[test]
fn partial_lineage_unsupported_recipes_remain_visible_and_block_eviction() {
    let home = Home::new();
    let publish = || {
        let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
        let complete = (0..128).any(|_| {
            evidence
                .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
                .unwrap()
        });
        evidence.retire();
        assert!(complete, "the ordinary producer must finish this fixture");
    };
    home.top("root", &[human("root", ts(0, 0), "current root prompt")]);
    let removed=home.top("cleared", &[json!({"type":"user","uuid":"old-event","timestamp":ts(0,1),"sessionId":"cleared","session_id":"root","origin":{"kind":"human"},"message":{"role":"user","content":"unsupported old native prompt"}})]);
    publish();
    let config = home.options.source.config(&home.options.cache).unwrap();
    let connection = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    connection
        .execute(
            "UPDATE main.session_slot_projections SET version=version+1 WHERE namespace=semon_namespace() AND session_key='root'",
            [],
        )
        .unwrap();
    fs::remove_file(removed).unwrap();
    publish();
    // Exercise the product page too so a failure records the existing
    // adapter's sanitized metadata diagnostic before identity maps it.
    let page = crate::session_catalog_page(
        &home.options,
        "fixture",
        "sid=root&scope=retained_history&limit=1",
    );
    assert_eq!(page.status, 200, "retained identity metadata page");
    let history = crate::session_catalog_history_identity(&home.options, "fixture", "root")
        .unwrap()
        .unwrap();
    assert_eq!(history.source_refs.len(), 2);
    let current = crate::session_catalog_identity(&home.options, "fixture", "root")
        .unwrap()
        .unwrap();
    assert_eq!(current.source_refs.len(), 1);
    assert!(
        crate::source_projection_ready(
            &home.options,
            "claude",
            "projects/-work-proj/cleared.jsonl"
        )
        .unwrap()
        .is_none()
    );
    let reply = crate::session_transcript_range(
        &home.options,
        "fixture",
        "sid=root&scope=retained_history&after=0&limit=100",
        None,
    );
    assert_eq!(reply.status, 200);
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(body["projection"]["history_incomplete"], true);
    assert_eq!(body["content_observation"]["state"], "incomplete");
    assert!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["text"] != "unsupported old native prompt")
    );
}

#[test]
fn source_disappearance_stays_pending_until_catalog_retirement_commits() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "prompt")]);
    home.build_query();
    let before = read_catalog(&home).0;
    let connection =
        rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER interrupt_retirement BEFORE UPDATE OF lifecycle ON session_catalog WHEN NEW.lifecycle='retained' AND OLD.lifecycle='current' BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
    let source = home.root.join("claude/projects/-work-proj/root.jsonl");
    fs::remove_file(&source).unwrap();
    let built = home.build_query();
    assert!(!built.sessions.contains_key("root"));
    assert_eq!(before, read_catalog(&home).0);
    let reason: String = connection
        .query_row(
            "SELECT reason FROM session_catalog_invalidations WHERE source_path=?1",
            [source.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "removed");
    connection
        .execute_batch("DROP TRIGGER interrupt_retirement")
        .unwrap();
    home.build_query();
    let (generation, rows) = read_catalog(&home);
    assert_ne!(before, generation);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].lifecycle,
        super::summary::CatalogLifecycle::Retained
    );
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_invalidations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0);
}

#[test]
fn native_catalog_does_not_acknowledge_a_rejected_generation() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "prompt")]);
    home.build_query();
    let options = home.options.clone();
    let path = home.root.join("claude/projects/-work-proj/root.jsonl");
    AFTER_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(
                file,
                "{}",
                assistant("root", ts(0, 1), vec![text("new answer")])
            )
            .unwrap();
            observe_catalog(
                &options,
                &mut EventCache::open_scoped(&options.cache, &options.source).unwrap(),
                &mut false,
                &mut Texts::default(),
                NOW,
            )
            .unwrap();
        }));
    });
    assert!(
        observe_catalog(
            &home.options,
            &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
            &mut false,
            &mut Texts::default(),
            NOW,
        )
        .is_err(),
        "a newer committed source generation must be retried"
    );
    let (_, catalog) = read_catalog(&home);
    assert_eq!(catalog[0].last, Some(at(0, 1)));
}

#[test]
fn a_slow_catalog_builder_cannot_overwrite_newer_committed_source_generation() {
    let home = Home::new();
    home.top("root", &[human("root", ts(0, 0), "prompt")]);
    home.build_query();
    let options = home.options.clone();
    let path = home.root.join("claude/projects/-work-proj/root.jsonl");
    AFTER_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(
                file,
                "{}",
                assistant("root", ts(0, 1), vec![text("new answer")])
            )
            .unwrap();
            observe_catalog(
                &options,
                &mut EventCache::open_scoped(&options.cache, &options.source).unwrap(),
                &mut false,
                &mut Texts::default(),
                NOW,
            )
            .unwrap();
        }));
    });
    let older = home.build_query();
    assert_eq!(older.sessions["root"].last, at(0, 0));
    let (_, catalog) = read_catalog(&home);
    assert_eq!(catalog[0].last, Some(at(0, 1)));
}

#[test]
fn a_slow_catalog_builder_cannot_overwrite_newer_native_metadata_without_log_changes() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(0, 0), "prompt"),
            assistant(
                "root",
                ts(0, 1),
                vec![tool("spawn", "Agent", json!({"prompt":"task"}))],
            ),
        ],
    );
    home.agent(
        "root",
        "child",
        "spawn",
        &[user("root", ts(0, 2), "child prompt")],
    );
    home.build_query();
    let options = home.options.clone();
    let meta = home
        .root
        .join("claude/projects/-work-proj/root/subagents/agent-child.meta.json");
    AFTER_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            fs::write(
                meta,
                json!({"description":"new native metadata label", "toolUseId":"spawn"}).to_string(),
            )
            .unwrap();
            observe_catalog(
                &options,
                &mut EventCache::open_scoped(&options.cache, &options.source).unwrap(),
                &mut false,
                &mut Texts::default(),
                NOW,
            )
            .unwrap();
        }));
    });
    assert!(
        build_query(
            &home.options,
            &mut EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap(),
            &mut false,
            &mut Texts::default(),
            NOW
        )
        .is_err()
    );
    let (_, catalog) = read_catalog(&home);
    assert_eq!(
        catalog.iter().find(|row| row.key == "child").unwrap().name,
        "new native metadata label"
    );
}

#[test]
fn catalog_native_and_parent_indexes_keep_ambiguity_and_remove_retired_sources() {
    let home = Home::new();
    home.top(
        "root",
        &[
            human("root", ts(0, 0), "prompt"),
            assistant(
                "root",
                ts(0, 1),
                vec![tool("spawn", "Agent", json!({"prompt":"task"}))],
            ),
        ],
    );
    home.agent(
        "root",
        "child",
        "spawn",
        &[user("root", ts(0, 2), "child prompt")],
    );
    home.build_query();
    let connection =
        rusqlite::Connection::open(home.options.cache.with_extension("sqlite3")).unwrap();
    let children: Vec<String> = connection.prepare("SELECT session_key FROM session_catalog WHERE parent_key=?1 ORDER BY last_ms DESC,session_key ASC").unwrap()
        .query_map(["root"], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(children, ["child"]);

    let (generation, mut rows) = read_catalog(&home);
    // The index must represent ambiguity even if a future adapter observes
    // one native identifier at several separate physical session owners.
    for row in &mut rows {
        row.native_ids = vec!["ambiguous-native".into()];
        for source in &mut row.sources {
            source.native_id = "ambiguous-native".into();
        }
    }
    let mut cache = EventCache::open(&home.options.cache, &home.options.source).unwrap();
    cache.publish_session_catalog(&rows, Some(&generation));
    let owners: Vec<String> = connection.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE harness=?1 AND native_id=?2 ORDER BY session_key").unwrap()
        .query_map(["claude", "ambiguous-native"], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(owners, ["child", "root"]);
    let wrong_harness: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_sources WHERE harness=?1 AND native_id=?2",
            ["codex", "ambiguous-native"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(wrong_harness, 0);
    let native_plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM session_catalog_sources WHERE harness=?1 AND native_id=?2", ["claude", "ambiguous-native"], |row| row.get(3)).unwrap();
    assert!(
        native_plan.contains("session_catalog_native"),
        "{native_plan}"
    );
    let parent_plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT session_key FROM session_catalog WHERE parent_key=?1 ORDER BY last_ms DESC,session_key ASC LIMIT 30", ["root"], |row| row.get(3)).unwrap();
    assert!(
        parent_plan.contains("session_catalog_parent"),
        "{parent_plan}"
    );

    fs::remove_file(
        home.root
            .join("claude/projects/-work-proj/root/subagents/agent-child.jsonl"),
    )
    .unwrap();
    home.build_query();
    let retired: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_sources WHERE session_key='child' AND lifecycle='current'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired, 0);
    let retained:i64=connection.query_row("SELECT count(*) FROM session_catalog_sources WHERE session_key='child' AND lifecycle='retained'",[],|row|row.get(0)).unwrap();
    assert_eq!(
        retained, 1,
        "disappeared source identity remains explicit retained provenance"
    );
    let current: i64 = connection.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE session_key='root' AND native_id='root'", [], |row| row.get(0)).unwrap();
    assert_eq!(current, 1);
}

#[test]
fn native_name_updates_do_not_change_transcripts_and_remain_source_qualified() {
    let home = Home::new();
    let other = Home::new();
    let id = "00000000-0000-0000-0000-000000000077";
    home.codex(id, json!({}), &[]);
    other.codex(id, json!({}), &[]);
    home.top(id, &[human(id, ts(0, 0), "Claude prompt")]);
    let before = home.build_query();
    home.lines(
        "codex/session_index.jsonl",
        &[json!({"id":id,"thread_name":"Meaningful name","updated_at":"2026-10-10T00:00:00Z"})],
    );
    let after = home.build_query();
    let native = |built: &QueryData| {
        built
            .sessions
            .iter()
            .find(|(_, s)| s.harness == "codex")
            .map(|(key, s)| (key.clone(), s.name.clone()))
            .unwrap()
    };
    assert_eq!(native(&after).1, "Meaningful name");
    assert_eq!(native(&before).0, native(&after).0);
    assert_ne!(native(&other.build_query()).1, "Meaningful name");
    assert!(
        after
            .sessions
            .values()
            .filter(|s| s.harness == "claude")
            .all(|s| s.name != "Meaningful name")
    );
    let sid = native(&after).0;
    let tx_before = crate::tx::tests::page(&before, &sid, &crate::tx::Anchor::Last, NOW);
    let tx_after = crate::tx::tests::page(&after, &sid, &crate::tx::Anchor::Last, NOW);
    assert_eq!(tx_before.unwrap(), tx_after.unwrap());
}

#[test]
fn failed_send_bookmarks_resolve_indexed_real_owners_without_source_bytes() {
    let home = relay_home();
    home.build_query();
    fs::remove_dir_all(&home.options.claude_home).unwrap();
    let resolve = |query: &str| crate::session_bookmark(&home.options, "fixture", query);
    let response = resolve("sid=unsent%3Anobody-9f");
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(value["unavailable"], 0);
    assert_eq!(
        value["matches"],
        json!([{"source_key":"fixture","catalog_key":"alpha","harness":"claude"}])
    );
    assert!(
        !String::from_utf8(response.body)
            .unwrap()
            .contains("lost one")
    );
    let connection = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let metadata: Vec<String> = connection
        .prepare("SELECT metadata FROM session_trace_edges WHERE session_key='alpha'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        metadata.iter().all(|edge| !edge.contains("nobody-9f")),
        "native failed-send aliases remain private to bookmark normalization"
    );
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM session_trace_bookmarks WHERE target='nobody-9f' ORDER BY session_key LIMIT 3", [], |row| row.get(3)).unwrap();
    assert!(
        plan.contains("SEARCH") && plan.contains("session_trace_bookmarks"),
        "{plan}"
    );
    for query in [
        "sid=alpha&sid=alpha",
        "sid=alpha&machine=elsewhere",
        "sid=alpha&scope=current",
    ] {
        assert_eq!(resolve(query).status, 400);
    }
    let value: Value = serde_json::from_slice(&resolve("sid=unsent%3Amissing").body).unwrap();
    assert_eq!(value["matches"], json!([]));
}

#[test]
fn semantic_index_own_writes_and_persisted_stage_resume_with_dirty_backlog() {
    let mut home = acknowledged_semantic_family();
    let inputs = crate::inputs(&home.options).unwrap();
    let paths = inputs
        .iter()
        .map(|input| {
            input
                .full_path(&home.options)
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let mut facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "synthetic-stage".into(),
        codex_rollouts: Some(
            inputs
                .iter()
                .filter(|input| input.root == crate::InputRoot::Codex)
                .map(|input| input.path.clone())
                .collect(),
        ),
        ..Default::default()
    };
    home.options.facts = Some(home.root.join("facts.json"));
    let intent = crate::SessionFactsWrite::begin(&home.options, &facts).unwrap();
    crate::write_facts(
        home.options.facts.as_ref().unwrap(),
        &facts,
        &home.options.source,
    )
    .unwrap();
    intent.complete().unwrap();
    let runtime = crate::native_evidence::query_runtime_generation(&home.options).unwrap();
    let mut cache = EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
    for page in paths.chunks(64) {
        assert_eq!(
            cache.begin_semantic_stage(page, &runtime).unwrap(),
            crate::events::Outcome::Written
        );
    }
    drop(cache);
    let mut phases = BTreeSet::new();
    let mut injected = false;
    let mut complete = false;
    for step in 0..512 {
        // A fresh owner each time must accept the previous batch's own Index
        // writes rather than reject its ledger/claim witnesses after restart.
        let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
        let done = evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap();
        evidence.retire();
        drop(evidence);
        let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let phase: Option<String> = db
            .query_row("SELECT phase FROM catalog_semantic_stage", [], |row| {
                row.get(0)
            })
            .optional()
            .unwrap();
        if let Some(phase) = phase {
            phases.insert(phase);
        }
        if !injected && phases.contains("identity") {
            // Interrupt an existing persistent stage with both a Raw intent
            // and an actual hosted runtime update. Neither journal is cleared.
            let input = &inputs[0];
            let path = input.full_path(&home.options);
            let from = fs::metadata(&path).unwrap().len();
            let intent =
                crate::SessionSourceWrite::begin(&home.options, input, None, from).unwrap();
            let line = format!(
                "{}\n",
                codex_reply(ts(14, 3), "Synthetic persisted backlog update")
            );
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(line.as_bytes()).unwrap();
            file.sync_all().unwrap();
            intent.complete(from + line.len() as u64).unwrap();
            facts.codex_locks.insert("ack-root".into(), 42);
            facts.proc_starts.insert(42, 100);
            let intent = crate::SessionFactsWrite::begin(&home.options, &facts).unwrap();
            crate::write_facts(
                home.options.facts.as_ref().unwrap(),
                &facts,
                &crate::Options::default().source,
            )
            .unwrap();
            intent.complete().unwrap();
            injected = true;
        }
        if done {
            complete = true;
            eprintln!(
                "semantic-restart-backlog steps={} phases={phases:?}",
                step + 1
            );
            break;
        }
    }
    assert!(
        injected && complete,
        "persistent stage/backlog did not recover: {phases:?}"
    );
    assert!(
        phases.contains("indexing")
            && phases.contains("identity")
            && phases.contains("relations")
            && phases.contains("publication")
    );
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let pending: i64 = db
        .query_row(
            "SELECT count(*) FROM session_catalog_invalidations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0);
}

#[test]
fn semantic_large_native_source_own_index_writes_survive_every_owner_restart() {
    let mut home = acknowledged_semantic_family();
    // Exceed the producer's 4 MiB scan budget with individually bounded native
    // records. The one-line fixtures cannot exercise resumed Index writes.
    let record = codex_user(ts(14, 0), &"Synthetic retained native text. ".repeat(3072));
    home.codex("ack-root", json!({}), &vec![record; 64]);
    let inputs = crate::inputs(&home.options).unwrap();
    let paths = inputs
        .iter()
        .map(|input| {
            input
                .full_path(&home.options)
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let large_path = inputs
        .iter()
        .find(|input| input.path.ends_with("rollout-ack-root.jsonl"))
        .unwrap()
        .full_path(&home.options);
    let large_size = fs::metadata(&large_path).unwrap().len();
    assert!(large_size > 4 * 1024 * 1024);
    let facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "synthetic-large-stage".into(),
        codex_rollouts: Some(inputs.iter().map(|input| input.path.clone()).collect()),
        ..Default::default()
    };
    home.options.facts = Some(home.root.join("facts.json"));
    let intent = crate::SessionFactsWrite::begin(&home.options, &facts).unwrap();
    crate::write_facts(
        home.options.facts.as_ref().unwrap(),
        &facts,
        &home.options.source,
    )
    .unwrap();
    intent.complete().unwrap();
    let runtime = crate::native_evidence::query_runtime_generation(&home.options).unwrap();
    let mut cache = EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
    for page in paths.chunks(64) {
        assert_eq!(
            cache.begin_semantic_stage(page, &runtime).unwrap(),
            crate::events::Outcome::Written
        );
    }
    drop(cache);
    let mut phases = BTreeSet::new();
    let mut partial_index = false;
    let mut complete = false;
    for step in 0..1024 {
        let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
        let done = evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap_or_else(|error| panic!("large-source step={step} phases={phases:?}: {error}"));
        evidence.retire();
        drop(evidence);
        let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let phase: Option<String> = db
            .query_row("SELECT phase FROM catalog_semantic_stage", [], |row| {
                row.get(0)
            })
            .optional()
            .unwrap();
        if let Some(phase) = phase {
            phases.insert(phase);
        }
        let resume: Option<i64> = db
            .query_row(
                "SELECT resume_at FROM files WHERE path=?1",
                [large_path.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
            .flatten();
        partial_index |= resume.is_some_and(|offset| offset > 0 && (offset as u64) < large_size);
        if done {
            complete = true;
            eprintln!(
                "semantic-large-source-restart steps={} bytes={large_size} phases={phases:?}",
                step + 1
            );
            break;
        }
    }
    assert!(
        partial_index,
        "fixture never exercised a resumed Index write"
    );
    assert!(complete, "large native source did not recover: {phases:?}");
    assert!(
        phases.contains("indexing")
            && phases.contains("identity")
            && phases.contains("relations")
            && phases.contains("publication")
    );
}

fn acknowledged_semantic_family() -> Home {
    let home = Home::new();
    home.codex(
        "ack-root",
        json!({}),
        &[codex_user(ts(14, 0), "Coordinate")],
    );
    for n in 0..80 {
        home.codex(&format!("ack-child-{n:03}"),json!({"parent_thread_id":"ack-root","thread_source":"subagent","agent_path":format!("root/part{n:03}")}),&[codex_user(ts(14,1),"Work")]);
    }
    home
}

fn first_acknowledged_owner(
    home: &Home,
) -> (
    std::sync::Arc<crate::native_evidence::NativeEvidence>,
    i64,
    summary::CatalogSource,
) {
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    for _ in 0..128 {
        assert!(
            !evidence
                .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
                .unwrap(),
            "family completed before exposing its first acknowledged owner page"
        );
        let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let acknowledged:Option<(i64,String)>=db.query_row("SELECT epoch,source_path FROM catalog_semantic_stage CROSS JOIN catalog_semantic_sources WHERE resetting=0 AND ack_done=1 AND EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE publication_done=0) ORDER BY source_path LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?))).optional().unwrap();
        if let Some((epoch, path)) = acknowledged {
            let metadata:String=db.query_row("SELECT c.metadata FROM session_catalog_sources s JOIN session_catalog c USING(session_key) WHERE s.source_path=?1 AND s.lifecycle='current'",[&path],|row|row.get(0)).unwrap();
            let row: summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
            let source = row
                .sources
                .into_iter()
                .find(|source| source.path.to_string_lossy() == path)
                .unwrap();
            let relative = source
                .path
                .strip_prefix(&home.options.codex_home)
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                crate::source_projection_ready(&home.options, "codex", relative)
                    .unwrap()
                    .is_none(),
                "acknowledged owner made the partial collection archive-ready"
            );
            return (evidence, epoch, source);
        }
    }
    panic!("no active stage with an acknowledged real owner publication page");
}

#[test]
fn semantic_announced_mutation_of_acknowledged_page_invalidates_old_epoch() {
    let home = acknowledged_semantic_family();
    let (evidence, old_epoch, old_source) = first_acknowledged_owner(&home);
    let relative = old_source
        .path
        .strip_prefix(&home.options.codex_home)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let input = crate::Input {
        root: crate::InputRoot::Codex,
        path: relative.clone(),
    };
    let from = fs::metadata(&old_source.path).unwrap().len();
    let intent = crate::SessionSourceWrite::begin(&home.options, &input, None, from).unwrap();
    let line = format!(
        "{}\n",
        codex_reply(
            ts(14, 2),
            "Announced completion after acknowledged owner page"
        )
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&old_source.path)
        .unwrap();
    file.write_all(line.as_bytes()).unwrap();
    file.sync_all().unwrap();
    intent.complete(from + line.len() as u64).unwrap();
    let mut cache = EventCache::open_scoped(&home.options.cache, &home.options.source).unwrap();
    assert_ne!(
        cache.finish_semantic_stage(old_epoch).unwrap(),
        crate::events::Outcome::Written,
        "old epoch accepted a newly announced revision of an acknowledged source"
    );
    drop(cache);
    let mut reset = false;
    let mut complete = false;
    for _ in 0..256 {
        let done = evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap();
        let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let active: Option<i64> = db
            .query_row("SELECT epoch FROM catalog_semantic_stage", [], |row| {
                row.get(0)
            })
            .optional()
            .unwrap();
        if active.is_some_and(|epoch| epoch != old_epoch) {
            reset = true;
        }
        if active == Some(old_epoch) {
            assert!(!done, "stale acknowledged epoch completed");
            assert!(
                crate::source_projection_ready(&home.options, "codex", &relative)
                    .unwrap()
                    .is_none()
            );
        }
        if done {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        reset && complete,
        "announced acknowledged-page mutation did not reset and qualify a new epoch"
    );
    let ready = crate::source_projection_ready(&home.options, "codex", &relative)
        .unwrap()
        .unwrap();
    assert_eq!(ready.source.offset, from + line.len() as u64);
    assert_ne!(ready.source.prefix_sha256, old_source.prefix_sha256);
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("ack-oracle-index.json");
    let oracle = home.build_at(&oracle_options, NOW);
    let key = &old_source.native_id;
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
            [key],
            |row| row.get(0),
        )
        .unwrap();
    let row: summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
    assert_eq!(row.parent, oracle.sessions[key].parent);
    assert_eq!(row.last, oracle.facts[key].last);
    for turn in oracle.turns.iter().filter(|turn| &turn.sid == key) {
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_trace_turns WHERE session_key=?1 AND turn_id=?2",
                rusqlite::params![key, turn.id],
                |row| row.get(0),
            )
            .unwrap();
        let value: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(value["end"], json!(turn.end));
        assert_eq!(value["sent"], json!(turn.sent));
    }
}

#[test]
fn semantic_unannounced_mutation_after_ack_rejects_original_native_generation() {
    let home = acknowledged_semantic_family();
    let (evidence, _, old_source) = first_acknowledged_owner(&home);
    let relative = old_source
        .path
        .strip_prefix(&home.options.codex_home)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let original = crate::query_source::Sources::capture_focused(
        &home.options,
        std::slice::from_ref(&old_source),
    )
    .unwrap();
    original.reader().verify_all().unwrap();
    assert!(
        crate::session_source_proof(&home.options, "codex", &relative)
            .unwrap()
            .is_some()
    );
    let source_path = old_source.path.to_string_lossy();
    let revision: Option<i64> = db
        .query_row(
            "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
            [source_path.as_ref()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    let bytes = fs::read_to_string(&old_source.path).unwrap();
    assert!(bytes.contains("Work"));
    assert_eq!(crate::catalog::source_state(&old_source), "cached");
    // Change bytes inside the consumed prefix without invoking a journal or
    // producer. Readiness is a consumed-generation certificate, not an atomic
    // filesystem snapshot or permission to archive the changed current file.
    fs::write(
        &old_source.path,
        bytes.replacen("Work", "Changed original bytes", 1),
    )
    .unwrap();
    assert_eq!(
        revision,
        db.query_row(
            "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
            [source_path.as_ref()],
            |row| row.get::<_, i64>(0)
        )
        .optional()
        .unwrap()
    );
    assert!(
        crate::source_projection_ready(&home.options, "codex", &relative)
            .unwrap()
            .is_none(),
        "partial stage lost strict global archive barrier"
    );
    assert_eq!(crate::catalog::source_state(&old_source), "stale");
    assert!(
        matches!(
            crate::session_source_proof(&home.options, "codex", &relative),
            Err(crate::CatalogIdentityError::Unavailable) | Ok(None)
        ),
        "native control/archive source proof accepted changed original generation"
    );
    assert!(
        original.reader().verify_all().is_err(),
        "original consumed-byte archive proof accepted altered prefix bytes"
    );
    evidence.retire();
}

#[test]
fn cold_semantic_family_discovers_older_disconnected_native_key_collisions() {
    let home = Home::new();
    for id in ["same", "codex:same"] {
        let path = home.top(
            id,
            &[human(id, ts(15, 0), "Older disconnected Claude identity")],
        );
        fs::File::open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
            )
            .unwrap();
    }
    for n in 0..80 {
        let id = format!("native-child-{n:03}");
        home.codex(&id,json!({"parent_thread_id":"same","thread_source":"subagent","agent_path":format!("root/part{n:03}")}),&[codex_user(ts(15,1),"Related native work")]);
        fs::File::open(
            home.options
                .codex_home
                .join(format!("sessions/2026/09/24/rollout-{id}.jsonl")),
        )
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(2)),
        )
        .unwrap();
    }
    home.codex(
        "same",
        json!({}),
        &[codex_user(ts(15, 0), "Native Codex family")],
    );
    fs::File::open(
        home.options
            .codex_home
            .join("sessions/2026/09/24/rollout-same.jsonl"),
    )
    .unwrap()
    .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(3)))
    .unwrap();
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("cold-key-oracle.json");
    let oracle = home.build_at(&oracle_options, NOW);
    assert!(
        oracle.sessions.contains_key("claude:same")
            && oracle.sessions.contains_key(":codex:same")
            && oracle.sessions.contains_key("codex:same")
    );
    // Enter through ordinary cold discovery. Prequeueing every source would
    // mask missing collision hints in the oldest disconnected source cohort.
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let mut complete = false;
    for _ in 0..128 {
        if evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        complete,
        "cold family with older disconnected native collisions remained pending"
    );
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let actual=db.prepare("SELECT session_key FROM session_catalog WHERE lifecycle='current' ORDER BY session_key").unwrap().query_map([],|row|row.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<BTreeSet<_>>>().unwrap();
    assert_eq!(
        actual,
        oracle
            .sessions
            .iter()
            .filter(|(_, session)| !session.stub)
            .map(|(key, _)| key.clone())
            .collect::<BTreeSet<_>>()
    );
    for (key, session) in &oracle.sessions {
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        let row: summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
        assert_eq!(row.harness, session.harness);
        assert_eq!(row.parent, session.parent, "{key} qualified native parent");
        assert_eq!(
            row.query.as_ref().unwrap()["rows"][key]["summary"]["parent"],
            json!(oracle.facts[key].parent),
            "{key} Query parent binding"
        );
    }
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key=':codex:same'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
    assert_eq!(
        row.query.as_ref().unwrap()["rows"][":codex:same"]["summary"]["children"]
            .as_array()
            .unwrap()
            .len(),
        80
    );
}

#[test]
fn staged_claude_learned_names_ignore_pinned_sends_and_later_duplicate_receipts() {
    let home = Home::new();
    home.top("alpha",&[
        human("alpha",ts(16,0),"Send native messages"),
        assistant("alpha",ts(16,1),vec![tool("pin-call","SendMessage",json!({"to":"pin-owner","message":"Pinned native send"})),tool("ordinary-call","SendMessage",json!({"to":"aaa-first","message":"Ordinary native send"}))]),
        result("alpha",ts(16,1),"pin-call","sent",false,json!({"success":true,"msg_id":"pin-message","pin":{"id":"pin-owner","name":"pin"}})),
        result("alpha",ts(16,1),"ordinary-call","sent",false,json!({"success":true,"msg_id":"duplicate-message"})),
    ]);
    home.top(
        "aaa-first",
        &[peer(
            "aaa-first",
            ts(16, 2),
            "duplicate-message",
            "FirstLearned",
            100,
            "Ordinary native send",
        )],
    );
    home.top(
        "zzz-second",
        &[peer(
            "zzz-second",
            ts(16, 2),
            "duplicate-message",
            "ForbiddenSecond",
            100,
            "Ordinary native send",
        )],
    );
    home.top(
        "pin-owner",
        &[peer(
            "pin-owner",
            ts(16, 2),
            "pin-message",
            "ForbiddenPinned",
            100,
            "Pinned native send",
        )],
    );
    let mut legacy = vec![human("legacy", ts(16, 0), "Use learned names")];
    for (call, name, body) in [
        ("first-legacy", "FirstLearned", "Eligible first receipt"),
        (
            "second-legacy",
            "ForbiddenSecond",
            "Ineligible duplicate receipt",
        ),
        ("pin-legacy", "ForbiddenPinned", "Ineligible pinned send"),
    ] {
        legacy.push(assistant(
            "legacy",
            ts(16, 3),
            vec![tool(call, "SendMessage", json!({"to":name,"message":body}))],
        ));
        legacy.push(result(
            "legacy",
            ts(16, 3),
            call,
            "sent",
            false,
            json!({"success":true}),
        ));
    }
    home.top("legacy", &legacy);
    for n in 0..70 {
        let id = format!("middle-{n:03}");
        home.top(&id, &[human(&id, ts(16, 0), "Independent")]);
    }
    // The oracle assertions make the exact native-first and PIN eligibility
    // premises explicit before testing the staged reducer against them.
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("learned-name-premise-oracle.json");
    let oracle = home.build_at(&oracle_options, NOW);
    assert_eq!(
        by_brief(&oracle, "Eligible first receipt").to.as_deref(),
        Some("alpha")
    );
    assert!(by_brief(&oracle, "Ineligible duplicate receipt").unmatched);
    assert!(by_brief(&oracle, "Ineligible pinned send").unmatched);
    assert_queued_identity_matches_oracle(&home, &home.options);
}

#[test]
fn cold_codex_family_qualifies_pending_copilot_native_collision() {
    let home = Home::new();
    let native = "01234567-89ab-4cde-8123-456789abcdef";
    let copilot = home.lines(
        &format!("copilot/session-state/{native}/events.jsonl"),
        &[json!({"type":"session.start","id":"copilot-start","timestamp":ts(0,0),"data":{"version":1,"copilotVersion":"1.0.91","sessionId":native}})],
    );
    fs::File::options()
        .write(true)
        .open(&copilot)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs((BASE / 1000) as u64)),
        )
        .unwrap();
    for n in 0..80 {
        let child = format!("copilot-collision-child-{n:03}");
        home.codex(&child, json!({"parent_thread_id":native,"thread_source":"subagent","agent_path":format!("{native}/part{n:03}")}), &[codex_user(ts(1,0), "Child")]);
    }
    home.codex(native, json!({}), &[codex_user(ts(1, 0), "Codex root")]);
    let mut oracle_options = home.options.clone();
    oracle_options.cache = home.root.join("copilot-collision-oracle.json");
    let oracle = home.build_at(&oracle_options, NOW);
    let codex_key = format!("codex:{native}");
    let copilot_key = format!("copilot:{native}");
    assert_eq!(oracle.sessions[&codex_key].harness, "codex");
    assert_eq!(oracle.sessions[&copilot_key].harness, "copilot");
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let mut complete = false;
    let mut staged = false;
    for _ in 0..128 {
        let done = evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
            .unwrap();
        let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        staged |= db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap();
        if done {
            complete = true;
            break;
        }
    }
    evidence.retire();
    assert!(
        complete && staged,
        "normal cold family did not use and finish durable staging"
    );
    let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
    let keys = db.prepare("SELECT session_key FROM session_catalog WHERE lifecycle='current' ORDER BY session_key").unwrap().query_map([],|row|row.get::<_,String>(0)).unwrap().collect::<Result<BTreeSet<_>,_>>().unwrap();
    assert_eq!(
        keys,
        oracle
            .sessions
            .iter()
            .filter(|(_, session)| !session.stub)
            .map(|(key, _)| key.clone())
            .collect()
    );
    for n in 0..80 {
        let child = format!("copilot-collision-child-{n:03}");
        let parent:Option<String>=db.query_row("SELECT parent_key FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",[&child],|row|row.get(0)).unwrap();
        assert_eq!(parent.as_deref(), Some(codex_key.as_str()), "{child}");
    }
    for (harness, key) in [("codex", &codex_key), ("copilot", &copilot_key)] {
        let binding:String=db.query_row("SELECT session_key FROM session_catalog_sources WHERE harness=?1 AND native_id=?2 AND lifecycle='current'",rusqlite::params![harness,native],|row|row.get(0)).unwrap();
        assert_eq!(&binding, key);
        let reply = crate::session_transcript_range(
            &home.options,
            "fixture",
            &format!("sid={key}&limit=10"),
            None,
        );
        assert_eq!(
            reply.status, 200,
            "{harness} source-qualified Query binding"
        );
    }
}

#[test]
fn staged_current_native_key_ignores_deselected_or_rewritten_cached_codex_header() {
    for (rewritten, retain) in [(false, true), (false, false), (true, false)] {
        let home = Home::new();
        let native = "stale-native";
        home.codex(
            native,
            json!({}),
            &[codex_user(ts(0, 0), "Old native identity")],
        );
        let old_relative = format!("sessions/2026/09/24/rollout-{native}.jsonl");
        let old_evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
        let mut old_complete = false;
        for _ in 0..16 {
            if old_evidence
                .refresh_catalog(&mut crate::viewer::lock(&old_evidence.work), NOW)
                .unwrap()
            {
                old_complete = true;
                break;
            }
        }
        old_evidence.retire();
        assert!(
            old_complete,
            "old Current native publication did not qualify"
        );
        let old_db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
        let old_binding:String=old_db.query_row("SELECT session_key FROM session_catalog_sources WHERE harness='codex' AND native_id=?1 AND lifecycle='current'",[native],|row|row.get(0)).unwrap();
        assert_eq!(
            old_binding, native,
            "fixture requires an old Current identity membership"
        );
        drop(old_db);
        let retained = if retain {
            let proof = crate::session_source_proof(&home.options, "codex", &old_relative)
                .unwrap()
                .unwrap();
            let ready = crate::source_projection_ready(&home.options, "codex", &old_relative)
                .unwrap()
                .unwrap();
            crate::retain_source_event_index(&home.options, &proof, &ready).unwrap();
            assert!(crate::source_event_index_retained(&home.options, &proof, &ready).unwrap());
            let db = rusqlite::Connection::open(EventCache::path(&home.options.cache)).unwrap();
            let archive_key = format!(
                "archive_event_index:{}",
                home.options.codex_home.join(&old_relative).display()
            );
            let marker: String = db
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [&archive_key],
                    |row| row.get(0),
                )
                .unwrap();
            Some((proof, archive_key, marker))
        } else {
            None
        };
        if rewritten {
            home.lines(&format!("codex/{old_relative}"),&[
                json!({"type":"session_meta","timestamp":ts(0,1),"payload":{"id":"rewritten-native","cwd":"/work/proj","originator":"codex_exec","thread_source":"user"}}),
                codex_user(ts(0,1),"Replacement native identity"),
            ]);
        }
        home.top(
            native,
            &[human(native, ts(1, 0), "Current Claude identity")],
        );
        let mut current = BTreeSet::new();
        if rewritten {
            current.insert(old_relative.clone());
        }
        for n in 0..80 {
            let child = format!("cached-header-child-{n:03}");
            home.codex(&child,json!({"parent_thread_id":"cached-header-family","thread_source":"subagent","agent_path":format!("cached-header-family/part{n:03}")}),&[codex_user(ts(1,0),"Child")]);
            current.insert(format!("sessions/2026/09/24/rollout-{child}.jsonl"));
        }
        home.codex(
            "cached-header-family",
            json!({}),
            &[codex_user(ts(1, 0), "Current family")],
        );
        current.insert("sessions/2026/09/24/rollout-cached-header-family.jsonl".to_owned());
        let mut facts = crate::local_runtime_facts(&home.options).unwrap();
        facts.codex_rollouts = Some(current);
        let facts_path = home.root.join("selected-facts.json");
        crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
        let mut options = home.options.clone();
        options.facts = Some(facts_path);
        let mut oracle_options = options.clone();
        oracle_options.cache = home.root.join("selected-native-oracle.json");
        let oracle = home.build_at(&oracle_options, NOW);
        assert_eq!(oracle.sessions[native].harness, "claude");
        assert!(!oracle.sessions.contains_key(&format!("claude:{native}")));
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        let mut complete = false;
        for _ in 0..128 {
            if evidence
                .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
                .unwrap()
            {
                complete = true;
                break;
            }
        }
        evidence.retire();
        assert!(
            complete,
            "cached native identity did not requalify (rewritten={rewritten}, retain={retain})"
        );
        let db = rusqlite::Connection::open(EventCache::path(&options.cache)).unwrap();
        let keys = db.prepare("SELECT session_key FROM session_catalog WHERE lifecycle='current' ORDER BY session_key").unwrap().query_map([],|row|row.get::<_,String>(0)).unwrap().collect::<Result<BTreeSet<_>,_>>().unwrap();
        assert_eq!(
            keys,
            oracle
                .sessions
                .iter()
                .filter(|(_, session)| !session.stub)
                .map(|(key, _)| key.clone())
                .collect(),
            "rewritten={rewritten}, retain={retain}"
        );
        let old_header: Option<String> = db
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                [format!(
                    "native_codex_header:{}",
                    home.options.codex_home.join(&old_relative).display()
                )],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        if rewritten || retain {
            let header: Value = serde_json::from_str(&old_header.unwrap()).unwrap();
            assert_eq!(
                header["header"]["id"],
                if rewritten {
                    "rewritten-native"
                } else {
                    native
                }
            );
        } else {
            assert!(
                old_header.is_none(),
                "unretained excluded header remains authoritative"
            );
            let old_path = home.options.codex_home.join(&old_relative);
            assert!(
                old_path.is_file(),
                "withdrawal must preserve the native file"
            );
            let ledger: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM files WHERE path=?1)",
                    [old_path.to_string_lossy().as_ref()],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                !ledger,
                "unretained excluded source kept its EventIndex ledger"
            );
            let old_current:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE harness='codex' AND native_id=?1 AND lifecycle='current')",[native],|row|row.get(0)).unwrap();
            assert!(!old_current, "withdrawn native membership stayed Current");
        }
        if let Some((proof, archive_key, marker)) = retained {
            let preserved: String = db
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [archive_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                preserved, marker,
                "retained consumed-generation EventIndex proof changed"
            );
            let history = crate::session_catalog_history_identity(&options, "fixture", native)
                .unwrap()
                .unwrap();
            assert_eq!(history.harness, "codex");
            assert_eq!(history.source_refs.len(), 1);
            assert!(
                history
                    .source_refs
                    .iter()
                    .any(|source| source.source == proof.source),
                "historical recipes lost the retained native source proof"
            );
            let current = crate::session_catalog_identity(&options, "fixture", native)
                .unwrap()
                .unwrap();
            assert_eq!(current.harness, "claude");
            assert_eq!(current.source_refs.len(), 1);
            assert!(
                current
                    .source_refs
                    .iter()
                    .all(|source| source.source.root == "claude")
            );
            let old_row: String = db
                .query_row(
                    "SELECT metadata FROM session_history_catalog WHERE session_key=?1",
                    [native],
                    |row| row.get(0),
                )
                .unwrap();
            let old_row: summary::CatalogRow = serde_json::from_str(&old_row).unwrap();
            assert_eq!(old_row.harness, "codex");
            assert_eq!(old_row.parent, None);
            assert_eq!(old_row.sources.len(), 1);
            assert_eq!(
                old_row.sources[0].path,
                home.options.codex_home.join(&old_relative)
            );
            let transcript = crate::session_transcript_range(
                &options,
                "fixture",
                &format!("sid={native}&scope=retained_history&after=0&limit=100"),
                None,
            );
            assert_eq!(
                transcript.status, 200,
                "original harness history recipe read"
            );
            let transcript = String::from_utf8_lossy(&transcript.body);
            assert!(transcript.contains("Old native identity"));
            assert!(!transcript.contains("Current Claude identity"));
            let list = crate::session_catalog_page(
                &options,
                "fixture",
                "scope=retained_history&harness=claude&limit=100",
            );
            assert_eq!(list.status, 200);
            let list: Value = serde_json::from_slice(&list.body).unwrap();
            let item = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["key"] == native)
                .unwrap();
            assert_eq!(item["harness"], json!("claude"));
            assert!(
                item["source_refs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|source| source["source"]["root"] == "claude")
            );
            let recipes: i64 = db
                .query_row(
                    "SELECT count(*) FROM session_history_slots WHERE session_key=?1",
                    [native],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(recipes > 0, "retained native recipes were removed");
        }
        let binding:String=db.query_row("SELECT session_key FROM session_catalog_sources WHERE harness='claude' AND native_id=?1 AND lifecycle='current'",[native],|row|row.get(0)).unwrap();
        assert_eq!(binding, native);
    }
}

#[test]
fn staged_native_key_long_reserved_prefix_chain_matches_shared_identity_oracle() {
    let home = Home::new();
    home.top("same", &[human("same", ts(0, 0), "Claude collision")]);
    home.codex(
        "same",
        json!({}),
        &[codex_user(ts(0, 0), "Codex collision")],
    );
    for n in 0..80 {
        home.codex(
            &format!("{}codex:same", ":".repeat(n)),
            json!({}),
            &[codex_user(ts(0, 0), "Reserved native identity")],
        );
    }
    let oracle = assert_queued_identity_matches_oracle(&home, &home.options);
    assert_eq!(
        oracle.sessions[&format!("{}codex:same", ":".repeat(80))].harness,
        "codex"
    );
    assert_eq!(oracle.sessions["claude:same"].harness, "claude");
    for n in 0..80 {
        let key = format!("{}codex:same", ":".repeat(n));
        assert_eq!(oracle.sessions[&key].harness, "codex");
    }
}

#[test]
fn different_harness_current_preserves_unsupported_original_history_custody() {
    let home = Home::new();
    let native = "unsupported-original";
    let mut original_records = vec![codex_user(ts(19, 0), "Original retained native body")];
    original_records
        .extend((0..130).map(|n| codex_reply(ts(19, 0), &format!("Original retained reply {n}"))));
    home.codex(native, json!({}), &original_records);
    let drain = |options: &Options| {
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        for _ in 0..128 {
            if evidence
                .refresh_catalog(&mut crate::viewer::lock(&evidence.work), NOW)
                .unwrap()
            {
                evidence.retire();
                return;
            }
        }
        panic!("selected native custody fixture did not complete");
    };
    drain(&home.options);
    let relative = format!("sessions/2026/09/24/rollout-{native}.jsonl");
    let proof = crate::session_source_proof(&home.options, "codex", &relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&home.options, "codex", &relative)
        .unwrap()
        .unwrap();
    crate::retain_source_event_index(&home.options, &proof, &ready).unwrap();
    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let marker_key = format!(
        "archive_event_index:{}",
        home.options.codex_home.join(&relative).display()
    );
    let marker: String = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [&marker_key],
            |row| row.get(0),
        )
        .unwrap();
    // The original recipes still exist, but their historical header cannot
    // certify a V5 source cohort. A new Current owner must retain this custody
    // without granting an empty or rebound historical observation.
    db.execute(
        "UPDATE main.session_slot_projections SET version=4,source_generation=NULL WHERE namespace=semon_namespace() AND session_key=?1",
        [native],
    )
    .unwrap();
    let header: (String,u32,Option<String>,i64) = db.query_row("SELECT generation,version,source_generation,total FROM session_slot_projections WHERE session_key=?1", [native], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
    let slots = db
        .prepare("SELECT slot,metadata FROM session_slots WHERE session_key=?1 ORDER BY slot")
        .unwrap()
        .query_map([native], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!slots.is_empty());
    let old: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key=?1",
            [native],
            |row| row.get(0),
        )
        .unwrap();
    let mut old: Value = serde_json::from_str(&old).unwrap();
    old["lifecycle"] = json!("retained");
    home.top(
        native,
        &[human(native, ts(19, 1), "New Current Claude body")],
    );
    let mut facts = crate::local_runtime_facts(&home.options).unwrap();
    facts.codex_rollouts = Some(BTreeSet::new());
    let facts_path = home.root.join("custody-facts.json");
    crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
    let mut options = home.options.clone();
    options.facts = Some(facts_path);
    drain(&options);
    let current = crate::session_catalog_identity(&options, "fixture", native)
        .unwrap()
        .unwrap();
    assert_eq!(current.harness, "claude");
    assert!(
        current
            .source_refs
            .iter()
            .all(|source| source.source.root == "claude")
    );
    assert_eq!(
        crate::session_catalog_history_identity(&options, "fixture", native),
        Err(crate::CatalogIdentityError::Unavailable)
    );
    let saved: String = db
        .query_row(
            "SELECT metadata FROM session_history_catalog WHERE session_key=?1",
            [native],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&saved).unwrap(), old);
    let saved_header: String = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("history_original_header:{native}")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Option<(String, u32, Option<String>, i64)>>(&saved_header).unwrap(),
        Some(header.clone())
    );
    let (generation,version,total,incomplete): (String,u32,i64,bool) = db.query_row("SELECT generation,version,total,incomplete FROM session_history_projections WHERE session_key=?1", [native], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
    assert_eq!(
        (generation, version, total),
        (header.0.clone(), header.1, header.3)
    );
    assert!(incomplete);
    let saved = db
        .prepare(
            "SELECT slot,metadata FROM session_history_slots WHERE session_key=?1 ORDER BY slot",
        )
        .unwrap()
        .query_map([native], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        saved, slots,
        "unsupported original recipe bytes were rewritten or discarded"
    );
    assert_eq!(
        db.query_row(
            "SELECT value FROM meta WHERE key=?1",
            [&marker_key],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        marker
    );
    // A second native publication must preserve the unqualified original custody.
    append_records(
        &home,
        native,
        &[assistant(
            native,
            ts(19, 2),
            vec![text("Second Current Claude publication")],
        )],
    );
    drain(&options);
    let republished = crate::session_catalog_identity(&options, "fixture", native)
        .unwrap()
        .unwrap();
    assert_ne!(republished.generation, current.generation);
    assert_eq!(republished.harness, "claude");
    assert!(
        republished
            .source_refs
            .iter()
            .all(|source| source.source.root == "claude")
    );
    let transcript = crate::session_transcript_range(
        &options,
        "fixture",
        &format!("sid={native}&after=0&limit=100"),
        None,
    );
    assert_eq!(transcript.status, 200);
    let transcript = String::from_utf8_lossy(&transcript.body);
    assert!(transcript.contains("New Current Claude body"));
    assert!(transcript.contains("Second Current Claude publication"));
    assert!(!transcript.contains("Original retained native body"));
    let repeated_metadata: String = db
        .query_row(
            "SELECT metadata FROM session_history_catalog WHERE session_key=?1",
            [native],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&repeated_metadata).unwrap(),
        old
    );
    let repeated_header: String = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("history_original_header:{native}")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repeated_header, saved_header);
    assert_eq!(
        serde_json::from_str::<Option<(String, u32, Option<String>, i64)>>(&repeated_header)
            .unwrap(),
        Some(header.clone())
    );
    let (generation, version, total, incomplete): (String, u32, i64, bool) = db
        .query_row(
            "SELECT generation,version,total,incomplete FROM session_history_projections WHERE session_key=?1",
            [native],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!((generation, version, total), (header.0, header.1, header.3));
    assert!(incomplete);
    assert_eq!(
        db.query_row(
            "SELECT value FROM meta WHERE key=?1",
            [&marker_key],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        marker
    );
    let repeated = db
        .prepare(
            "SELECT slot,metadata FROM session_history_slots WHERE session_key=?1 ORDER BY slot",
        )
        .unwrap()
        .query_map([native], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(repeated, slots);
    assert_eq!(
        crate::session_catalog_history_identity(&options, "fixture", native),
        Err(crate::CatalogIdentityError::Unavailable)
    );
}
