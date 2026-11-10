//! Synthetic native fixtures through the real SQLite adapter and Parquet store.

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use arrow::array::{Int64Array, StringArray};
use arrow::util::display::array_value_to_string;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use semon_asi::{
    AdmittedBatch, Clock, InputBudget, PublishProgress, Store, Versions, publish_cycle,
};

use super::*;

const NAMESPACE: &str = "fixture-machine";

#[derive(Clone)]
struct TestClock(Arc<AtomicI64>);

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn clock() -> TestClock {
    TestClock(Arc::new(AtomicI64::new(100)))
}

fn budget(sessions: usize, steps: usize) -> InputBudget {
    InputBudget {
        max_sessions: sessions,
        max_steps: steps,
    }
}

fn fixture(
    name: &str,
    count: usize,
) -> (
    PathBuf,
    crate::Options,
    Vec<crate::model::summary::CatalogRow>,
    SqliteStore,
) {
    let root = scratch(name);
    let (options, rows, slots) = catalog_native_fixture(&root, count, false);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    (root, options, rows, store)
}

struct ObservedInput {
    inner: SqliteAnalyticsInput,
    last: Option<AdmittedBatch>,
    reconciliations: usize,
    removed: BTreeSet<String>,
}

impl ObservedInput {
    fn open(options: &crate::Options) -> Self {
        Self {
            inner: SqliteAnalyticsInput::open(
                &crate::events::EventCache::path(&options.cache),
                NAMESPACE,
            )
            .unwrap(),
            last: None,
            reconciliations: 0,
            removed: BTreeSet::new(),
        }
    }
}

impl AnalyticsInput for ObservedInput {
    type Error = rusqlite::Error;

    fn read_admitted(
        &mut self,
        checkpoint: Option<&str>,
        budget: InputBudget,
    ) -> rusqlite::Result<AdmittedBatch> {
        let batch = self.inner.read_admitted(checkpoint, budget)?;
        let mut sessions = BTreeSet::new();
        let mut admitted_steps = 0;
        for change in &batch.changes {
            let uid = match change {
                SessionChange::Admitted(session) => {
                    admitted_steps += session.steps.len();
                    &session.session_uid
                }
                SessionChange::Pending { session_uid }
                | SessionChange::SourceMissing { session_uid, .. }
                | SessionChange::SourceRestored { session_uid } => session_uid,
                SessionChange::Removed { session_uid } => {
                    self.removed.insert(session_uid.clone());
                    session_uid
                }
            };
            sessions.insert(uid);
        }
        assert!(sessions.len() <= budget.max_sessions);
        assert!(
            admitted_steps <= budget.max_steps || sessions.len() == 1,
            "only a single whole session may exceed the step budget"
        );
        if matches!(
            batch.reconciliation,
            Reconciliation::ReplaceNamespace { .. }
        ) {
            self.reconciliations += 1;
        }
        self.last = Some(batch.clone());
        Ok(batch)
    }
}

fn chunk(
    store: &mut Store<TestClock>,
    input: &mut ObservedInput,
    budget: InputBudget,
) -> PublishProgress {
    let generation = store.manifest().generation;
    let progress = publish_cycle(store, input, NAMESPACE, Versions::default(), budget).unwrap();
    assert_eq!(progress.generation, generation + 1);
    assert_eq!(
        store.manifest().checkpoints[NAMESPACE],
        input.last.as_ref().unwrap().checkpoint
    );
    progress
}

fn drain(store: &mut Store<TestClock>, input: &mut ObservedInput, budget: InputBudget) -> usize {
    for count in 1..=256 {
        if chunk(store, input, budget).complete {
            return count;
        }
    }
    panic!("synthetic inventory did not finish");
}

type Rows = BTreeMap<String, BTreeMap<Vec<Option<String>>, usize>>;

// Compare actual live Parquet rows, including dimensions, ignoring only the
// physical publication generation. Dead rows stay in immutable files.
fn live_rows(store: &Store<TestClock>) -> Rows {
    let mut result = Rows::new();
    for table in semon_asi::schema::TABLES {
        if table.kind == semon_asi::schema::Kind::Append {
            continue;
        }
        let rows = result.entry(table.name.into()).or_default();
        for file in store.manifest().files_of(table.name) {
            let reader = ParquetRecordBatchReaderBuilder::try_new(
                fs::File::open(store.analytics_dir().join(&file.path)).unwrap(),
            )
            .unwrap()
            .build()
            .unwrap();
            for batch in reader {
                let batch = batch.unwrap();
                for index in 0..batch.num_rows() {
                    if table.kind == semon_asi::schema::Kind::Session {
                        let uid = batch
                            .column_by_name("session_uid")
                            .unwrap()
                            .as_any()
                            .downcast_ref::<StringArray>()
                            .unwrap();
                        let generation = batch
                            .column_by_name("_gen")
                            .unwrap()
                            .as_any()
                            .downcast_ref::<Int64Array>()
                            .unwrap();
                        if store.live_map().get(uid.value(index)) != Some(&generation.value(index))
                        {
                            continue;
                        }
                    }
                    let row = batch
                        .schema()
                        .fields()
                        .iter()
                        .zip(batch.columns())
                        .filter(|(field, _)| field.name() != "_gen")
                        .map(|(_, array)| {
                            (!array.is_null(index))
                                .then(|| array_value_to_string(array, index).unwrap())
                        })
                        .collect();
                    *rows.entry(row).or_default() += 1;
                }
            }
        }
    }
    result
}

fn uid(row: &crate::model::summary::CatalogRow) -> String {
    format!("{NAMESPACE}/{}/{}", row.harness, row.native_ids[0])
}

fn append_steps(row: &crate::model::summary::CatalogRow, count: usize) {
    use std::io::Write;
    let mut source = fs::OpenOptions::new()
        .append(true)
        .open(&row.sources[0].path)
        .unwrap();
    for index in 0..count {
        writeln!(source, "{}", serde_json::json!({"type":"user", "uuid":format!("late-user-{index}"), "sessionId":row.native_ids[0], "timestamp":"2026-09-24T00:02:00Z", "message":{"role":"user", "content":"Synthetic late prompt"}})).unwrap();
        writeln!(source, "{}", serde_json::json!({"type":"assistant", "uuid":format!("late-assistant-{index}"), "sessionId":row.native_ids[0], "timestamp":"2026-09-24T00:02:01Z", "message":{"id":format!("late-message-{index}"), "role":"assistant", "model":"claude-sonnet-4-5", "content":[{"type":"text", "text":"Synthetic late response"}], "usage":{"input_tokens":30, "output_tokens":7}}})).unwrap();
    }
}

fn republish(store: &mut SqliteStore, options: &crate::Options) {
    let (rows, slots) = catalog_fixture_projection(options);
    catalog_write(store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
}

fn remove_identity(store: &mut SqliteStore, key: &str) {
    let tx = store.connection.transaction().unwrap();
    tx.execute("DELETE FROM session_catalog WHERE session_key=?1", [key])
        .unwrap();
    tx.execute(
        "DELETE FROM session_activity_projections WHERE session_key=?1",
        [key],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO silver_change(session_key,kind) VALUES(?1,'removed')",
        [key],
    )
    .unwrap();
    tx.commit().unwrap();
}

fn new_incarnation(store: &mut SqliteStore) {
    // Simulate an independently rebuilt synthetic index. User data is never used.
    let tx = store.connection.transaction().unwrap();
    tx.execute(
        "UPDATE meta SET value=lower(hex(randomblob(16))) WHERE key='incarnation'",
        [],
    )
    .unwrap();
    tx.execute("DELETE FROM silver_change", []).unwrap();
    tx.execute("INSERT INTO silver_change(session_key,kind) SELECT session_key,'changed' FROM session_catalog WHERE lifecycle='current' ORDER BY session_key", []).unwrap();
    tx.commit().unwrap();
}

#[test]
fn bounded_cold_live_rows_equal_one_shot_with_oversize_and_exact_fit() {
    let (root, options, rows, mut native) = fixture("bounded-cold", 6);
    let large = rows.iter().find(|row| row.key == "native-cl-00").unwrap();
    append_steps(large, 4);
    republish(&mut native, &options);
    let mut expected = Store::open_with_clock(&root.join("expected"), clock()).unwrap();
    let mut full = ObservedInput::open(&options);
    assert!(chunk(&mut expected, &mut full, InputBudget::default()).complete);
    let batch = full.last.as_ref().unwrap();
    let total_steps = batch
        .changes
        .iter()
        .map(|change| match change {
            SessionChange::Admitted(session) => session.steps.len(),
            _ => 0,
        })
        .sum();
    assert!(batch.changes.iter().any(
        |change| matches!(change, SessionChange::Admitted(session) if session.steps.len() == 5)
    ));
    let expected_rows = live_rows(&expected);
    for (index, selected) in [
        budget(1, 1),
        budget(2, 2),
        budget(64, total_steps),
        InputBudget::default(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut store =
            Store::open_with_clock(&root.join(format!("chunked-{index}")), clock()).unwrap();
        let mut input = ObservedInput::open(&options);
        let chunks = drain(&mut store, &mut input, selected);
        assert_eq!(input.reconciliations, 1);
        assert_eq!(store.live_map().len(), 6);
        assert!(store.manifest().source_missing.is_empty());
        assert_eq!(live_rows(&store), expected_rows, "budget {selected:?}");
        if index < 2 {
            assert!(chunks > 1);
        }
        if index >= 2 {
            assert_eq!(chunks, 1, "exact fit and default fit in one chunk");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_inventory_restart_resumes_manifest_and_defers_absence_until_final() {
    for stop_after in [1, 2] {
        let (root, options, rows, mut native) = fixture("bounded-interrupt", 5);
        let selected = budget(1, 8_192);
        let time = clock();
        let analytics = root.join("interrupted");
        let mut store = Store::open_with_clock(&analytics, time.clone()).unwrap();
        let mut expected = Store::open_with_clock(&root.join("expected"), time.clone()).unwrap();
        drain(
            &mut store,
            &mut ObservedInput::open(&options),
            InputBudget::default(),
        );
        drain(
            &mut expected,
            &mut ObservedInput::open(&options),
            InputBudget::default(),
        );
        let absent = &rows[0];
        let absent_uid = uid(absent);
        remove_identity(&mut native, &absent.key);
        new_incarnation(&mut native);
        let mut input = ObservedInput::open(&options);
        for _ in 0..stop_after {
            assert!(!chunk(&mut store, &mut input, selected).complete);
            assert_eq!(input.reconciliations, 0);
            assert!(store.manifest().source_missing.is_empty());
            assert!(store.live_map().contains_key(&absent_uid));
        }
        let committed = store.manifest().checkpoints[NAMESPACE].clone();
        let unpublished = input
            .inner
            .read_admitted(Some(&committed), selected)
            .unwrap();
        drop(input);
        drop(store); // crash after a read, before committing the next chunk
        let mut store = Store::open_with_clock(&analytics, time.clone()).unwrap();
        assert_eq!(store.manifest().checkpoints[NAMESPACE], committed);
        let mut input = ObservedInput::open(&options);
        assert_eq!(
            input
                .inner
                .read_admitted(Some(&committed), selected)
                .unwrap(),
            unpublished
        );
        drain(&mut store, &mut input, selected);
        assert_eq!(input.reconciliations, 1);
        assert_eq!(store.manifest().source_missing.len(), 1);
        assert_eq!(
            store.manifest().source_missing[&absent_uid].detected_at,
            100
        );
        drain(
            &mut expected,
            &mut ObservedInput::open(&options),
            InputBudget::default(),
        );
        assert_eq!(live_rows(&store), live_rows(&expected));
        assert_eq!(
            store.manifest().checkpoints,
            expected.manifest().checkpoints
        );
        time.0.store(900, Ordering::SeqCst);
        assert!(chunk(&mut store, &mut input, selected).complete);
        new_incarnation(&mut native);
        drain(&mut store, &mut input, selected);
        assert_eq!(
            store.manifest().source_missing[&absent_uid].detected_at,
            100,
            "absence is detected once, even across another inventory"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn bounded_inventory_replays_late_records_additions_and_removals() {
    let (root, options, rows, mut native) = fixture("bounded-concurrent", 6);
    new_incarnation(&mut native);
    let mut store = Store::open_with_clock(&root.join("chunked"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    let selected = budget(1, 8_192);
    for _ in 0..3 {
        assert!(!chunk(&mut store, &mut input, selected).complete);
    }
    let changed = rows
        .iter()
        .find(|row| row.harness == "claude" && store.live_map().contains_key(&uid(row)))
        .unwrap();
    let removed = rows
        .iter()
        .find(|row| row.key != changed.key && store.live_map().contains_key(&uid(row)))
        .unwrap();
    append_steps(changed, 2);
    let path = options
        .claude_home
        .join("projects/-work-proj/new-native.jsonl");
    fs::write(path, [
        serde_json::json!({"type":"user", "uuid":"new-user", "sessionId":"new-native", "timestamp":"2026-09-24T00:03:00Z", "message":{"role":"user", "content":"Synthetic new session"}}).to_string(),
        serde_json::json!({"type":"assistant", "uuid":"new-assistant", "sessionId":"new-native", "timestamp":"2026-09-24T00:03:01Z", "message":{"id":"new-message", "role":"assistant", "model":"claude-sonnet-4-5", "content":[{"type":"text", "text":"Synthetic answer"}], "usage":{"input_tokens":10, "output_tokens":5}}}).to_string(),
    ].join("\n") + "\n").unwrap();
    republish(&mut native, &options);
    remove_identity(&mut native, &removed.key);
    let mut final_inventory_was_incomplete = false;
    for _ in 0..64 {
        let progress = chunk(&mut store, &mut input, selected);
        let batch = input.last.as_ref().unwrap();
        if let Reconciliation::ReplaceNamespace { session_uids } = &batch.reconciliation {
            assert!(session_uids.contains(&format!("{NAMESPACE}/claude/new-native")));
            assert!(!session_uids.contains(&uid(removed)));
            final_inventory_was_incomplete = !progress.complete;
        }
        if progress.complete {
            break;
        }
    }
    assert!(input.last.as_ref().unwrap().complete);
    assert!(
        final_inventory_was_incomplete,
        "inventory completion must still request journal replay after S"
    );
    assert!(input.removed.contains(&uid(removed)));
    assert!(!store.live_map().contains_key(&uid(removed)));
    assert!(
        store
            .live_map()
            .contains_key(&format!("{NAMESPACE}/claude/new-native"))
    );
    assert!(store.manifest().source_missing.is_empty());
    let mut expected = Store::open_with_clock(&root.join("final-full"), clock()).unwrap();
    assert_eq!(
        drain(
            &mut expected,
            &mut ObservedInput::open(&options),
            InputBudget::default()
        ),
        1
    );
    assert_eq!(live_rows(&store), live_rows(&expected));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_inventory_incarnation_change_abandons_old_partial_reconciliation() {
    let (root, options, _, mut native) = fixture("bounded-incarnation", 5);
    let mut store = Store::open_with_clock(&root.join("analytics-store"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    let selected = budget(1, 8_192);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    let old: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    new_incarnation(&mut native);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    let new: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    assert_ne!(old["incarnation"], new["incarnation"]);
    assert_eq!(
        old["cursor"]["after"], new["cursor"]["after"],
        "restart begins at the first bounded journal page"
    );
    assert_eq!(input.reconciliations, 0);
    assert!(store.manifest().source_missing.is_empty());
    drain(&mut store, &mut input, selected);
    assert_eq!(input.reconciliations, 1);
    let mut expected = Store::open_with_clock(&root.join("expected"), clock()).unwrap();
    drain(
        &mut expected,
        &mut ObservedInput::open(&options),
        InputBudget::default(),
    );
    assert_eq!(live_rows(&store), live_rows(&expected));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_incremental_pages_preserve_sequence_order_and_idempotence() {
    let (root, options, rows, native) = fixture("bounded-incremental", 4);
    let mut store = Store::open_with_clock(&root.join("store"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    drain(&mut store, &mut input, InputBudget::default());
    let expected = live_rows(&store);
    let order = [3, 1, 2, 0, 1, 3];
    for &index in &order {
        native
            .connection
            .execute(
                "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
                [&rows[index].key],
            )
            .unwrap();
    }
    for (page, indices) in order.chunks(2).enumerate() {
        let checkpoint = store.manifest().checkpoints[NAMESPACE].clone();
        let selected = budget(2, 8_192);
        let first = input
            .inner
            .read_admitted(Some(&checkpoint), selected)
            .unwrap();
        assert_eq!(
            first,
            input
                .inner
                .read_admitted(Some(&checkpoint), selected)
                .unwrap()
        );
        assert_eq!(first.reconciliation, Reconciliation::Incremental);
        let actual: Vec<_> = first
            .changes
            .iter()
            .filter_map(|change| match change {
                SessionChange::Admitted(session) => Some(session.session_uid.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            actual,
            indices
                .iter()
                .map(|&index| uid(&rows[index]))
                .collect::<Vec<_>>()
        );
        let progress = chunk(&mut store, &mut input, selected);
        assert_eq!(progress.complete, page == 2);
        assert_eq!(live_rows(&store), expected);
    }
    assert!(chunk(&mut store, &mut input, budget(1, 1)).complete);
    assert!(input.last.as_ref().unwrap().changes.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_incremental_coalesces_duplicates_and_defers_without_skipping() {
    let (root, options, rows, native) = fixture("bounded-deduplicate", 6);
    let mut store = Store::open_with_clock(&root.join("store"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    drain(&mut store, &mut input, InputBudget::default());
    let expected = live_rows(&store);
    let claude: Vec<_> = rows.iter().filter(|row| row.harness == "claude").collect();
    assert_eq!(claude.len(), 3);
    for index in [0, 1, 0, 2] {
        native
            .connection
            .execute(
                "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
                [&claude[index].key],
            )
            .unwrap();
    }
    // Page [A, B, A] coalesces to [B, A]. B exactly fills the step
    // budget; A's last entry remains beyond the committed cursor.
    for (page, index) in [1, 0, 2].into_iter().enumerate() {
        let progress = chunk(&mut store, &mut input, budget(3, 1));
        let admitted: Vec<_> = input
            .last
            .as_ref()
            .unwrap()
            .changes
            .iter()
            .filter_map(|change| match change {
                SessionChange::Admitted(session) => Some(session.session_uid.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(admitted, vec![uid(claude[index])]);
        assert_eq!(progress.complete, page == 2);
        assert_eq!(live_rows(&store), expected);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_empty_inventory_completes_and_checkpoints_zero() {
    let (root, options, _, _native) = fixture("bounded-empty", 0);
    let mut input = ObservedInput::open(&options);
    let first = input.read_admitted(None, budget(1, 1)).unwrap();
    assert!(first.complete);
    assert!(first.changes.is_empty());
    assert_eq!(
        first.reconciliation,
        Reconciliation::ReplaceNamespace {
            session_uids: BTreeSet::new()
        }
    );
    let second = input
        .read_admitted(Some(&first.checkpoint), budget(1, 1))
        .unwrap();
    assert!(second.complete);
    assert!(second.changes.is_empty());
    assert_eq!(second.reconciliation, Reconciliation::Incremental);
    assert_eq!(second.checkpoint, first.checkpoint);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_input_refuses_malformed_forged_and_cross_namespace_cursors() {
    let (root, options, rows, native) = fixture("bounded-invalid", 4);
    let mut input = ObservedInput::open(&options);
    let valid = input.inner.read_admitted(None, budget(1, 8_192)).unwrap();
    let original: Value = serde_json::from_str(&valid.checkpoint).unwrap();
    let current: i64 = native
        .connection
        .query_row("SELECT max(seq) FROM silver_change", [], |row| row.get(0))
        .unwrap();
    // Leave real gaps, so cursors must name an actual sequence, not merely be
    // numerically below the high-water mark.
    native
        .connection
        .execute(
            "INSERT INTO silver_change(seq,session_key,kind) VALUES(?1,?2,'changed')",
            params![current + 10, rows[0].key],
        )
        .unwrap();
    let mut invalid = vec![
        String::new(),
        "{}".into(),
        "not-a-checkpoint".into(),
        format!("{}:1", original["incarnation"].as_str().unwrap()),
    ];
    for (field, value) in [
        ("namespace", serde_json::json!("another-machine")),
        ("incarnation", serde_json::json!("bad")),
        ("extra", serde_json::json!(true)),
    ] {
        let mut forged = original.clone();
        forged[field] = value;
        invalid.push(forged.to_string());
    }
    for cursor in [
        serde_json::json!({"phase":"unknown", "after":1}),
        serde_json::json!({"phase":"inventory", "snapshot":current, "after":0}),
        serde_json::json!({"phase":"inventory", "snapshot":current, "after":current}),
        serde_json::json!({"phase":"inventory", "snapshot":current, "after":current+1}),
        serde_json::json!({"phase":"inventory", "snapshot":current+1, "after":1}),
        serde_json::json!({"phase":"incremental", "after":-1}),
        serde_json::json!({"phase":"incremental", "after":current+11}),
        serde_json::json!({"phase":"incremental", "after":current+1}),
        serde_json::json!({"phase":"incremental", "after":"1"}),
        serde_json::json!({"phase":"incremental", "after":1, "snapshot":current}),
        serde_json::json!({"phase":"incremental", "after":1, "extra":true}),
    ] {
        let mut forged = original.clone();
        forged["cursor"] = cursor;
        invalid.push(forged.to_string());
    }
    invalid.push(format!("{{\"namespace\":\"{NAMESPACE}\",\"namespace\":\"{NAMESPACE}\",\"incarnation\":{},\"cursor\":{{\"phase\":\"incremental\",\"after\":1}}}}", original["incarnation"]));
    for forged in invalid {
        assert!(
            matches!(
                input.inner.read_admitted(Some(&forged), budget(1, 1)),
                Err(rusqlite::Error::InvalidQuery)
            ),
            "accepted forged cursor: {forged}"
        );
    }
    for invalid_budget in [budget(0, 1), budget(1, 0)] {
        assert!(matches!(
            input.inner.read_admitted(None, invalid_budget),
            Err(rusqlite::Error::InvalidQuery)
        ));
    }
    fs::remove_dir_all(root).unwrap();
}
