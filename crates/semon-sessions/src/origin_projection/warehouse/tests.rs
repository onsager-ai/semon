use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use semon_asi::{Store, Versions, project, publish_cycle};
use serde_json::json;

use super::*;
use crate::origin_projection::SqliteAnalyticsInput;

const NAMESPACE: &str = "fixture";
const BUDGET: InputBudget = InputBudget {
    max_sessions: 1,
    max_steps: 1,
};

struct Fixture {
    root: PathBuf,
    options: Options,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("semon-p4-{}-{name}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let dir = &root;
        let options = Options {
            claude_home: dir.join("claude"),
            claude_json: dir.join("claude.json"),
            codex_home: dir.join("codex"),
            copilot_home: dir.join("copilot"),
            cache: dir.join("state/index.json"),
            proc_root: dir.join("proc"),
            annotations: dir.join("annotations"),
            all: true,
            ..Options::default()
        };
        // Runtime data is irrelevant even when present. An empty ordinary
        // home still lets the legacy oracle publish its native rows.
        fs::create_dir_all(&options.proc_root).unwrap();
        Self { root, options }
    }

    fn seed(&self, id: &str) -> PathBuf {
        let path = self
            .options
            .claude_home
            .join(format!("projects/-work-fixture/{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "{}\n",
                json!({
                    "type":"user", "uuid":format!("{id}-prompt"), "sessionId":id,
                    "timestamp":"2026-11-24T00:00:00Z",
                    "message":{"role":"user","content":"PRIVATE-P4-FIXTURE"}
                })
            ),
        )
        .unwrap();
        append(&path, id, 1);
        path
    }

    fn seed_codex(&self, id: &str, parent: Option<&str>) {
        let path = self
            .options
            .codex_home
            .join(format!("sessions/rollout-{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let records = [
            json!({"type":"session_meta", "timestamp":"2026-11-24T00:00:00Z", "payload":{
                "id":id, "cwd":"/work/synthetic", "originator":"codex_exec",
                "thread_source":if parent.is_some() { "subagent" } else { "user" },
                "parent_thread_id":parent, "agent_path":parent.map(|_| "root/worker")
            }}),
            json!({"type":"turn_context", "timestamp":"2026-11-24T00:00:01Z", "payload":{"model":"gpt-6-sol"}}),
            json!({"type":"token_usage_record", "timestamp":"2026-11-24T00:01:00Z", "payload":{
                "thread_id":id, "session_id":"session", "turn_id":"turn", "response_id":"response",
                "usage":{"input_tokens":7,"output_tokens":0,"total_tokens":7,"cached_input_tokens":0,
                         "cache_write_input_tokens":0,"reasoning_output_tokens":0}
            }}),
        ];
        let mut file = fs::File::create(path).unwrap();
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
    }

    fn admit(&self) {
        admit_warehouse(&self.options, || false).unwrap();
    }

    fn open(&self) -> WarehouseAnalyticsInput {
        WarehouseAnalyticsInput::open(&self.options, NAMESPACE).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn append(path: &Path, id: &str, index: u32) {
    let native_id = format!("{id}-{index}");
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(
        file,
        "{}",
        json!({
            "type":"assistant", "uuid":native_id, "sessionId":id,
            "timestamp":format!("2026-11-24T00:{index:02}:00Z"),
            "message":{"id":native_id,"role":"assistant","model":"claude-sonnet-4-5",
                       "content":[{"type":"text","text":"PRIVATE-P4-FIXTURE"}],
                       "usage":{"input_tokens":10,"output_tokens":5}}
        })
    )
    .unwrap();
}

fn drain(
    input: &mut impl AnalyticsInput<Error = rusqlite::Error>,
) -> BTreeMap<String, semon_asi::AdmittedSession> {
    let mut checkpoint = None;
    let mut sessions = BTreeMap::new();
    for _ in 0..16 {
        let batch = input.read_admitted(checkpoint.as_deref(), BUDGET).unwrap();
        checkpoint = Some(batch.checkpoint);
        for change in batch.changes {
            match change {
                SessionChange::Admitted(session) => {
                    assert!(
                        sessions
                            .insert(session.session_uid.clone(), *session)
                            .is_none()
                    );
                }
                SessionChange::SourceRestored { .. } => {}
                other => panic!("unexpected change {other:?}"),
            }
        }
        if batch.complete {
            assert_eq!(
                batch.reconciliation,
                Reconciliation::ReplaceNamespace {
                    session_uids: sessions.keys().cloned().collect(),
                }
            );
            return sessions;
        }
    }
    panic!("inventory did not terminate");
}

fn logical_rows(
    rows: &BTreeMap<String, semon_asi::AdmittedSession>,
) -> Vec<semon_asi::ProjectedSession> {
    rows.values()
        .map(|session| project(session).unwrap())
        .collect()
}

#[test]
fn cold_start_warehouse_matches_semantic_origin() {
    let fixture = Fixture::new("cold");
    fixture.seed("a");
    fixture.seed_codex("codex-fixture", None);
    let capture = crate::test_hooks::register(&fixture.options.cache, |point| {
        assert_ne!(
            point, "runtime_captured",
            "warehouse admission captured runtime"
        );
    });
    fixture.admit();
    drop(capture);
    let path = crate::analytics_index_path(&fixture.options.cache);
    let db = Connection::open(&path).unwrap();
    let (catalog, journal, captures): (i64, i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM session_catalog),(SELECT count(*) FROM silver_change),
                (SELECT count(*) FROM fact_process_observation)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((catalog, journal, captures), (0, 0, 0));

    let actual = drain(&mut fixture.open());
    assert_eq!(actual.len(), 2);
    assert_eq!(actual["fixture/codex/codex-fixture"].steps.len(), 1);
    assert!(actual.values().all(|session| session.steps.len() == 1));
    // Exercise the real writer and its manifest checkpoint through several batches.
    let mut input = fixture.open();
    let mut store = Store::open(&fixture.root.join("warehouse-analytics")).unwrap();
    assert!(
        !publish_cycle(
            &mut store,
            &mut input,
            NAMESPACE,
            Versions::default(),
            BUDGET
        )
        .unwrap()
        .complete
    );
    assert!(
        publish_cycle(
            &mut store,
            &mut input,
            NAMESPACE,
            Versions::default(),
            BUDGET
        )
        .unwrap()
        .complete
    );
    assert_eq!(store.live_map().len(), 2);
    let projected = logical_rows(&actual);
    assert!(!format!("{projected:?}").contains("PRIVATE-P4-FIXTURE"));

    crate::refresh_analytics_catalog(&fixture.options, || false).unwrap();
    let expected = drain(&mut SqliteAnalyticsInput::open(&path, NAMESPACE).unwrap());
    assert_eq!(projected, logical_rows(&expected));
}

#[test]
fn both_harnesses_preserve_evidence_turns_and_usage() {
    use semon_asi::input::UsageOwner::{Copied, Exclusive};

    let fixture = Fixture::new("usage");
    let first = fixture.seed("a");
    let second = fixture.seed("b");
    for (file, session) in [(&first, "a"), (&second, "b")] {
        let mut file = OpenOptions::new().append(true).open(file).unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type":"assistant", "uuid":"copy-uuid", "sessionId":session,
                "timestamp":"2026-11-24T00:02:00Z", "message":{
                    "id":"copied-message", "role":"assistant", "model":"claude-sonnet-4-5",
                    "content":[{"type":"text","text":"PRIVATE-P4-FIXTURE"}],
                    "usage":{"input_tokens":20,"output_tokens":5}
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type":"system", "subtype":"compact_boundary", "uuid":"compact-audit",
                "sessionId":session, "timestamp":"2026-11-24T00:03:00Z",
                "compactMetadata":{"trigger":"auto","preTokens":155000}
            })
        )
        .unwrap();
    }
    fixture.seed_codex("codex-root", None);
    fixture.seed_codex("codex-child", Some("codex-root"));
    fixture.admit();
    let actual = drain(&mut fixture.open());
    crate::refresh_analytics_catalog(&fixture.options, || false).unwrap();
    let path = crate::analytics_index_path(&fixture.options.cache);
    let expected = drain(&mut SqliteAnalyticsInput::open(&path, NAMESPACE).unwrap());
    assert_eq!(actual.len(), 4);
    assert_eq!(logical_rows(&actual), logical_rows(&expected));
    let repeated = drain(&mut fixture.open());
    for (uid, session) in &actual {
        // The semantic digest includes Catalog role metadata. Compare the
        // portable source evidence, and check the warehouse digest separately.
        assert_eq!(
            session.source_witness.sources,
            expected[uid].source_witness.sources
        );
        assert_eq!(session.source_witness, repeated[uid].source_witness);
        assert_eq!(session.steps, expected[uid].steps);
        assert_eq!(session.compactions, expected[uid].compactions);
    }

    let claude = &actual["fixture/claude/a"];
    assert_eq!(claude.steps[0].usage_owner, Exclusive);
    assert_eq!(claude.steps[1].usage_owner, Copied);
    assert_eq!(claude.steps[0].turn_uid, "fixture/claude/a/t/a-prompt");
    assert_eq!(claude.steps[1].turn_uid, claude.steps[0].turn_uid);
    assert_eq!(claude.compactions.len(), 1);
    let child = &actual["fixture/codex/codex-child"];
    let root = &actual["fixture/codex/codex-root"];
    assert_eq!(
        child.session.parent_session_uid.as_deref(),
        Some(root.session_uid.as_str())
    );
    assert_eq!(child.steps[0].turn_uid, "fixture/codex/codex-child/t/turn");
    assert_eq!(child.steps[0].usage_owner, Exclusive);
    let family_costs = |rows: &BTreeMap<String, semon_asi::AdmittedSession>| {
        crate::causal_trace::family_costs(rows.values().map(|session| {
            let cost = session
                .steps
                .iter()
                .filter(|step| step.usage_owner == Exclusive)
                .map(|step| step.cost.usd_est)
                .sum::<Option<f64>>();
            (
                session.session_uid.as_str(),
                session.session.parent_session_uid.as_deref(),
                cost,
            )
        }))
        .into_iter()
        .map(|(uid, cost)| (uid.to_owned(), cost))
        .collect::<BTreeMap<_, _>>()
    };
    let totals = family_costs(&actual);
    assert_eq!(totals, family_costs(&expected));
    assert_eq!(
        totals[&root.session_uid],
        Some(root.steps[0].cost.usd_est.unwrap() + child.steps[0].cost.usd_est.unwrap())
    );
}

#[test]
fn claude_agent_without_sidecar_keeps_storage_parent() {
    let fixture = Fixture::new("storage-parent");
    let parent = fixture.seed("parent");
    let child = parent
        .parent()
        .unwrap()
        .join("parent/subagents/agent-child.jsonl");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(&child, []).unwrap();
    append(&child, "child", 2);

    fixture.admit();
    let actual = drain(&mut fixture.open());
    assert_eq!(actual.len(), 2);
    let child = &actual["fixture/claude/child"];
    assert_eq!(
        child.session.parent_session_uid.as_deref(),
        Some("fixture/claude/parent")
    );
    assert_eq!(child.session.parent_method, ParentMethod::Native);

    crate::refresh_analytics_catalog(&fixture.options, || false).unwrap();
    let path = crate::analytics_index_path(&fixture.options.cache);
    let expected = drain(&mut SqliteAnalyticsInput::open(&path, NAMESPACE).unwrap());
    assert_eq!(logical_rows(&actual), logical_rows(&expected));
}

#[test]
fn watch_rescan_pins_a_drain_and_sees_next_append() {
    let fixture = Fixture::new("watch");
    fixture.seed("a");
    let path = fixture.seed("b");
    fixture.admit();
    let mut input = fixture.open();
    let first = input.read_admitted(None, BUDGET).unwrap();
    assert!(!first.complete);
    assert_eq!(input.read_admitted(None, BUDGET).unwrap(), first);
    append(&path, "b", 2);
    fixture.admit();
    let pinned = input
        .read_admitted(Some(&first.checkpoint), BUDGET)
        .unwrap();
    assert!(pinned.complete);
    let [
        SessionChange::SourceRestored { .. },
        SessionChange::Admitted(session),
    ] = pinned.changes.as_slice()
    else {
        panic!("unexpected watch continuation {:?}", pinned.changes);
    };
    assert_eq!(
        (session.session_uid.as_str(), session.steps.len()),
        ("fixture/claude/b", 1)
    );
    drop(input);
    let updated = drain(&mut fixture.open());
    assert_eq!(updated["fixture/claude/b"].steps.len(), 2);
}

#[test]
fn concurrent_admission_matches_single_run() {
    let fixture = Fixture::new("race");
    let path = fixture.seed("race");
    fixture.admit();
    let count = || {
        drain(&mut fixture.open())["fixture/claude/race"]
            .steps
            .len()
    };
    assert_eq!(count(), 1);
    append(&path, "race", 2);
    let (parsed, ready) = mpsc::channel();
    let (release, continue_scan) = mpsc::channel();
    std::thread::scope(|scope| {
        let options = &fixture.options;
        let stale = scope.spawn(move || {
            events::BEFORE_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    parsed.send(()).unwrap();
                    continue_scan.recv_timeout(Duration::from_secs(20)).unwrap();
                }))
            });
            admit_warehouse(options, || false).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(20)).unwrap();
        append(&path, "race", 3);
        fixture.admit();
        assert_eq!(count(), 3);
        release.send(()).unwrap();
        stale.join().unwrap();
        assert_eq!(count(), 3);
    });
    let baseline = Fixture::new("baseline");
    let path = baseline.seed("race");
    append(&path, "race", 2);
    append(&path, "race", 3);
    baseline.admit();
    assert_eq!(
        logical_rows(&drain(&mut fixture.open())),
        logical_rows(&drain(&mut baseline.open()))
    );
}

#[test]
fn next_inventory_reflects_replacement_and_committed_withdrawal() {
    let fixture = Fixture::new("withdrawal");
    let path = fixture.seed("replace");
    fixture.admit();
    let mut store = Store::open(&fixture.root.join("warehouse-analytics")).unwrap();
    let publish = |store: &mut Store| {
        let mut input = fixture.open();
        assert!(
            publish_cycle(store, &mut input, NAMESPACE, Versions::default(), BUDGET)
                .unwrap()
                .complete
        );
    };
    publish(&mut store);
    let initial = drain(&mut fixture.open());
    let uid = "fixture/claude/replace";
    let replacement = path.with_extension("tmp");
    fs::write(&replacement, []).unwrap();
    append(&replacement, "replace", 2);
    fs::rename(&replacement, &path).unwrap();
    fixture.admit();
    let replaced = drain(&mut fixture.open());
    assert_eq!(replaced[uid].steps.len(), 1);
    assert_ne!(
        replaced[uid].steps[0].step_uid,
        initial[uid].steps[0].step_uid
    );
    assert_ne!(replaced[uid].source_witness, initial[uid].source_witness);
    publish(&mut store);

    // The regular Index owns withdrawals. Source disappearance alone does
    // not let the reader erase or mark a still-admitted warehouse identity.
    let namespace = Namespace::new("local").unwrap();
    let mut producer = sqlite::open_index(&fixture.options, &namespace).unwrap();
    producer.begin_full_scan().unwrap();
    fs::remove_file(path).unwrap();
    fixture.admit();
    assert_eq!(drain(&mut fixture.open()).len(), 1);
    producer.retain(&BTreeSet::new(), &mut false);
    publish(&mut store);
    assert!(store.live_map().contains_key(uid));
    assert!(store.manifest().source_missing.contains_key(uid));
}
