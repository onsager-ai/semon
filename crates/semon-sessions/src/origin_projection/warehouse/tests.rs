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
    root: tempfile::TempDir,
    options: Options,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
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

    fn admit(&self) {
        admit_warehouse(&self.options, || false).unwrap();
    }

    fn open(&self) -> WarehouseAnalyticsInput {
        WarehouseAnalyticsInput::open(&self.options, NAMESPACE).unwrap()
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
    let fixture = Fixture::new();
    fixture.seed("a");
    fixture.seed("b");
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
    assert!(actual.values().all(|session| session.steps.len() == 1));
    // Exercise the real writer and its manifest checkpoint through several batches.
    let mut input = fixture.open();
    let mut store = Store::open(&fixture.root.path().join("warehouse-analytics")).unwrap();
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
fn watch_rescan_pins_a_drain_and_sees_next_append() {
    let fixture = Fixture::new();
    fixture.seed("a");
    let path = fixture.seed("b");
    fixture.admit();
    let mut input = fixture.open();
    let first = input.read_admitted(None, BUDGET).unwrap();
    assert!(!first.complete);
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
    let fixture = Fixture::new();
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
    let baseline = Fixture::new();
    let path = baseline.seed("race");
    append(&path, "race", 2);
    append(&path, "race", 3);
    baseline.admit();
    assert_eq!(
        logical_rows(&drain(&mut fixture.open())),
        logical_rows(&drain(&mut baseline.open()))
    );
}
