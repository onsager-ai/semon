//! Synthetic native fixtures through the real SQLite adapter and Parquet store.

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use arrow::array::{Int64Array, StringArray, TimestampMillisecondArray};
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
                            if array.is_null(index) {
                                None
                            } else if let Some(times) =
                                array.as_any().downcast_ref::<TimestampMillisecondArray>()
                            {
                                // Arrow's optional chrono-tz feature is off;
                                // compare the exact stored milliseconds.
                                Some(times.value(index).to_string())
                            } else {
                                Some(array_value_to_string(array, index).unwrap())
                            }
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
    let last_uid = input
        .last
        .as_ref()
        .unwrap()
        .changes
        .iter()
        .find_map(|change| match change {
            SessionChange::Admitted(session) => Some(&session.session_uid),
            _ => None,
        })
        .unwrap();
    let removed = rows.iter().find(|row| uid(row) == *last_uid).unwrap();
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
    let upper = rows.iter().max_by_key(|row| &row.key).unwrap();
    remove_identity(&mut native, &upper.key);
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
    assert!(!store.live_map().contains_key(&uid(upper)));
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
fn bounded_inventory_replays_new_session_behind_keyset_cursor() {
    let (root, options, _, mut native) = fixture("bounded-add-behind-cursor", 4);
    let mut store = Store::open_with_clock(&root.join("chunked"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    let selected = budget(1, 8_192);
    for _ in 0..2 {
        assert!(!chunk(&mut store, &mut input, selected).complete);
    }
    let cursor: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    assert_eq!(cursor["cursor"]["phase"], "inventory");
    assert!("a-new" < cursor["cursor"]["after"].as_str().unwrap());
    let snapshot = cursor["cursor"]["snapshot"].as_i64().unwrap();
    let new_uid = format!("{NAMESPACE}/claude/a-new");
    let path = options.claude_home.join("projects/-work-proj/a-new.jsonl");
    fs::write(path, [
        serde_json::json!({"type":"user", "uuid":"a-new-user", "sessionId":"a-new", "timestamp":"2026-09-24T00:03:00Z", "message":{"role":"user", "content":"Synthetic session behind the cursor"}}).to_string(),
        serde_json::json!({"type":"assistant", "uuid":"a-new-assistant", "sessionId":"a-new", "timestamp":"2026-09-24T00:03:01Z", "message":{"id":"a-new-message", "role":"assistant", "model":"claude-sonnet-4-5", "content":[{"type":"text", "text":"Synthetic answer"}], "usage":{"input_tokens":10, "output_tokens":5}}}).to_string(),
    ].join("\n") + "\n").unwrap();
    republish(&mut native, &options);
    let added_after_snapshot: bool = native
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM silver_change WHERE session_key='a-new' AND seq>?1)",
            [snapshot],
            |row| row.get(0),
        )
        .unwrap();
    assert!(added_after_snapshot);

    // The bounded inventory cannot revisit this key. Its complete UID set
    // includes the new identity, but only replay from S may publish its facts.
    for _ in 0..8 {
        let progress = chunk(&mut store, &mut input, selected);
        let batch = input.last.as_ref().unwrap();
        assert!(!batch.changes.iter().any(|change| {
            matches!(change, SessionChange::Admitted(session) if session.session_uid == new_uid)
        }));
        assert!(!store.live_map().contains_key(&new_uid));
        if let Reconciliation::ReplaceNamespace { session_uids } = &batch.reconciliation {
            assert!(session_uids.contains(&new_uid));
            assert!(!progress.complete);
            break;
        }
    }
    assert_eq!(input.reconciliations, 1);
    let cursor: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    assert_eq!(cursor["cursor"]["phase"], "replay");
    assert_eq!(cursor["cursor"]["after"].as_i64(), Some(snapshot));
    let mut replayed_new = false;
    for _ in 0..16 {
        let progress = chunk(&mut store, &mut input, selected);
        let batch = input.last.as_ref().unwrap();
        assert_eq!(batch.reconciliation, Reconciliation::Incremental);
        replayed_new |= batch.changes.iter().any(|change| {
            matches!(change, SessionChange::Admitted(session) if session.session_uid == new_uid)
        });
        if progress.complete {
            break;
        }
    }
    assert!(input.last.as_ref().unwrap().complete);
    assert!(replayed_new);
    assert!(store.live_map().contains_key(&new_uid));
    assert!(store.manifest().source_missing.is_empty());
    let mut expected = Store::open_with_clock(&root.join("one-shot"), clock()).unwrap();
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
fn bounded_inventory_replays_source_missing_and_restored_between_chunks() {
    let (root, options, rows, mut native) = fixture("bounded-availability", 6);
    let mut store = Store::open_with_clock(&root.join("chunked"), clock()).unwrap();
    let mut expected = Store::open_with_clock(&root.join("one-shot"), clock()).unwrap();
    for target in [&mut store, &mut expected] {
        drain(
            target,
            &mut ObservedInput::open(&options),
            InputBudget::default(),
        );
    }
    let retained = live_rows(&store);
    new_incarnation(&mut native);
    let mut input = ObservedInput::open(&options);
    let selected = budget(1, 8_192);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    let withdrawn = rows.iter().find(|row| row.key == "native-cl-02").unwrap();
    let withdrawn_uid = uid(withdrawn);
    assert_eq!(withdrawn.sources.len(), 1);
    let source = &withdrawn.sources[0].path;
    let original = fs::read(source).unwrap();
    fs::remove_file(source).unwrap();
    let (available, slots) = catalog_fixture_projection(&options);
    assert!(available.iter().all(|row| row.key != withdrawn.key));
    let seen = available
        .iter()
        .flat_map(|row| &row.sources)
        .map(|source| source.path.to_string_lossy().into_owned())
        .collect();
    // Match the existing missing-member fixture: a full scan must retire the
    // absent source ledger before a complete catalog publication can withdraw
    // its session. Omitting a still-current ledger would refuse publication.
    let mut cache = crate::events::EventCache::open_scoped(&options.cache).unwrap();
    cache.begin_scan().unwrap();
    cache.begin_full_scan().unwrap();
    let mut dirty = false;
    cache.retain(&seen, &mut dirty);
    cache.end_scan();
    drop(cache);
    assert!(dirty, "the missing source must leave the current ledger");
    assert_eq!(
        catalog_write(&mut native, &available, &slots, false, CatalogWriter::Auto)
            .unwrap()
            .0,
        Outcome::Written
    );
    assert_eq!(
        session_activity::source_missing(&native.connection, &withdrawn.key).unwrap(),
        Some(true)
    );
    assert!(!chunk(&mut store, &mut input, selected).complete);
    assert!(input.last.as_ref().unwrap().changes.iter().any(|change| {
        matches!(change, SessionChange::SourceMissing { session_uid, retained_at_origin: true } if session_uid == &withdrawn_uid)
    }));
    assert!(store.manifest().source_missing.contains_key(&withdrawn_uid));
    assert_eq!(
        live_rows(&store),
        retained,
        "source loss keeps frozen facts"
    );

    // Restore the source bytes after its inventory key has passed, and let the
    // native producer admit the complete current cohort again.
    // The eventual SourceRestored observation must therefore come from replay.
    fs::write(source, original).unwrap();
    let (restored, slots) = catalog_fixture_projection(&options);
    assert_eq!(
        catalog_write(&mut native, &restored, &slots, false, CatalogWriter::Auto)
            .unwrap()
            .0,
        Outcome::Written
    );
    assert_eq!(
        session_activity::source_missing(&native.connection, &withdrawn.key).unwrap(),
        Some(false)
    );
    let mut restored_by_replay = false;
    for _ in 0..32 {
        let progress = chunk(&mut store, &mut input, selected);
        let batch = input.last.as_ref().unwrap();
        if matches!(
            batch.reconciliation,
            Reconciliation::ReplaceNamespace { .. }
        ) {
            assert!(store.manifest().source_missing.contains_key(&withdrawn_uid));
        }
        if batch.changes.iter().any(|change| {
            matches!(change, SessionChange::SourceRestored { session_uid } if session_uid == &withdrawn_uid)
        }) {
            assert_eq!(batch.reconciliation, Reconciliation::Incremental);
            restored_by_replay = true;
        }
        if progress.complete {
            break;
        }
    }
    assert!(input.last.as_ref().unwrap().complete);
    assert_eq!(input.reconciliations, 1);
    assert!(restored_by_replay);
    assert!(input.removed.is_empty());
    assert!(store.manifest().source_missing.is_empty());
    assert_eq!(
        drain(
            &mut expected,
            &mut ObservedInput::open(&options),
            InputBudget::default()
        ),
        1
    );
    assert_eq!(live_rows(&store), live_rows(&expected));
    assert_eq!(
        store.manifest().source_missing,
        expected.manifest().source_missing
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_inventory_incarnation_change_abandons_old_partial_reconciliation() {
    let (root, options, _, mut native) = fixture("bounded-incarnation", 5);
    let mut store = Store::open_with_clock(&root.join("analytics-store"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    let selected = budget(1, 8_192);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    let old: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    new_incarnation(&mut native);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    let new: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    assert_ne!(old["incarnation"], new["incarnation"]);
    assert_eq!(
        new["cursor"]["after"],
        serde_json::json!("native-cl-00"),
        "restart begins at the first bounded journal page"
    );
    assert_eq!(old["cursor"]["after"], serde_json::json!("native-cl-02"));
    assert_eq!(input.reconciliations, 0);
    assert!(store.manifest().source_missing.is_empty());
    drain(&mut store, &mut input, selected);
    assert_eq!(input.reconciliations, 1);
    assert!(store.manifest().source_missing.is_empty());
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
fn bounded_drain_finishes_at_its_endpoint_while_the_journal_keeps_advancing() {
    let (root, options, rows, native) = fixture("bounded-moving-journal", 4);
    let mut store = Store::open_with_clock(&root.join("store"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    let selected = budget(1, 8_192);
    assert!(!chunk(&mut store, &mut input, selected).complete);
    for row in &rows {
        native
            .connection
            .execute(
                "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
                [&row.key],
            )
            .unwrap();
    }
    // Finish inventory and capture the replay endpoint after all changes that
    // happened during inventory. Reopening the adapter preserves this bound.
    for _ in 0..3 {
        assert!(!chunk(&mut store, &mut input, selected).complete);
    }
    let checkpoint: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    let endpoint = checkpoint["cursor"]["through"].as_i64().unwrap();
    assert_eq!(checkpoint["cursor"]["phase"], "replay");
    for (page, row) in rows.iter().enumerate() {
        native
            .connection
            .execute(
                "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
                [&row.key],
            )
            .unwrap();
        input = ObservedInput::open(&options);
        assert_eq!(chunk(&mut store, &mut input, selected).complete, page == 3);
    }
    let checkpoint: Value = serde_json::from_str(&store.manifest().checkpoints[NAMESPACE]).unwrap();
    assert_eq!(
        checkpoint["cursor"],
        serde_json::json!({"phase":"incremental", "after":endpoint})
    );
    // Writes beyond the bound are retained for a subsequent request, which
    // captures a new endpoint and is finite even if more changes arrive.
    assert_eq!(drain(&mut store, &mut input, selected), 4);
    let expected = live_rows(&store);
    for row in &rows {
        native
            .connection
            .execute(
                "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
                [&row.key],
            )
            .unwrap();
    }
    assert!(!chunk(&mut store, &mut input, selected).complete);
    for (page, row) in rows.iter().enumerate().skip(1) {
        native
            .connection
            .execute(
                "INSERT INTO silver_change(session_key,kind) VALUES(?1,'changed')",
                [&row.key],
            )
            .unwrap();
        assert_eq!(chunk(&mut store, &mut input, selected).complete, page == 3);
    }
    assert_eq!(live_rows(&store), expected);
    assert_eq!(drain(&mut store, &mut input, selected), 3);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_pending_catalog_inventory_needs_no_journal_entries() {
    let root = scratch("bounded-pending-only");
    let (options, rows, slots) = catalog_native_fixture(&root, 4, false);
    let mut pending = rows.clone();
    for row in &mut pending {
        row.session_activity = None;
    }
    let (mut native, _) = opened(&crate::events::EventCache::path(&options.cache));
    catalog_write(&mut native, &pending, &slots, false, CatalogWriter::Auto).unwrap();
    let journal: i64 = native
        .connection
        .query_row("SELECT count(*) FROM silver_change", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        journal, 0,
        "the producer deliberately waits for query-ready activity"
    );
    let mut store = Store::open_with_clock(&root.join("store"), clock()).unwrap();
    let mut input = ObservedInput::open(&options);
    let mut seen = BTreeSet::new();
    for page in 0..4 {
        let progress = chunk(&mut store, &mut input, budget(1, 1));
        let batch = input.last.as_ref().unwrap();
        assert_eq!(progress.complete, page == 3);
        assert_eq!(batch.changes.len(), 1);
        let SessionChange::Pending { session_uid } = &batch.changes[0] else {
            panic!("a never-admitted Catalog identity must be pending");
        };
        assert!(seen.insert(session_uid.clone()));
        assert!(store.live_map().is_empty());
        assert!(store.manifest().source_missing.is_empty());
        if let Reconciliation::ReplaceNamespace { session_uids } = &batch.reconciliation {
            assert_eq!(session_uids, &rows.iter().map(uid).collect());
        }
    }
    assert_eq!(input.reconciliations, 1);
    catalog_write(&mut native, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    drain(&mut store, &mut input, budget(1, 1));
    assert_eq!(store.live_map().len(), 4);
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
fn bounded_writer_continues_one_chunk_per_interval_without_fresh_notifications() {
    let (root, options, _, _native) = fixture("bounded-writer", 4);
    let store_root = root.join("writer-store");
    let time = Arc::new(clock());
    let mut config = crate::AnalyticsWriterConfig::new(
        NAMESPACE,
        crate::events::EventCache::path(&options.cache),
        &store_root,
    );
    config.input_budget = budget(1, 8_192);
    let handle = crate::analytics_writer::start_with_clock(config, Arc::clone(&time));
    handle.notify();
    handle.wait_for_publishes(1);
    handle.wait_for_idle_wait();
    let first = semon_asi::read_manifest(&store_root).unwrap().unwrap();
    assert_eq!(
        semon_asi::read_live_map(&store_root, &first).unwrap().len(),
        1
    );
    assert!(first.source_missing.is_empty());

    let previous_wait = handle.idle_waits();
    time.0.store(2_099, Ordering::SeqCst);
    handle.wake_for_clock();
    handle.wait_for_idle_wait_after(previous_wait);
    assert_eq!(
        handle.published(),
        1,
        "the existing 2 s cadence still applies to continuations"
    );
    for count in 2..=4 {
        time.0
            .store(100 + (count - 1) as i64 * 2_000, Ordering::SeqCst);
        handle.wake_for_clock();
        handle.wait_for_publishes(count);
        handle.wait_for_idle_wait();
    }
    let final_manifest = semon_asi::read_manifest(&store_root).unwrap().unwrap();
    assert_eq!(
        semon_asi::read_live_map(&store_root, &final_manifest)
            .unwrap()
            .len(),
        4
    );
    let checkpoint: Value = serde_json::from_str(&final_manifest.checkpoints[NAMESPACE]).unwrap();
    assert_eq!(checkpoint["cursor"]["phase"], "incremental");
    let previous_wait = handle.idle_waits();
    time.0.store(10_000, Ordering::SeqCst);
    handle.wake_for_clock();
    handle.wait_for_idle_wait_after(previous_wait);
    assert_eq!(
        handle.published(),
        4,
        "completed inventory leaves no publication demand"
    );
    handle.stop_and_join();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_writer_request_waits_for_complete_inventory() {
    let (root, options, _, _native) = fixture("bounded-writer-request", 6);
    let store_root = root.join("writer-store");
    let time = Arc::new(clock());
    let mut config = crate::AnalyticsWriterConfig::new(
        NAMESPACE,
        crate::events::EventCache::path(&options.cache),
        &store_root,
    );
    config.input_budget = budget(1, 8_192);
    let handle = crate::analytics_writer::start_with_clock(config, Arc::clone(&time));
    std::thread::scope(|scope| {
        // Cancellation bounds scoped-thread cleanup even if an assertion
        // fails before the fake clock reaches the final publication.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let handle_ref = &handle;
        let waiter = scope.spawn(move || {
            handle_ref.publish_and_wait_until(|| std::time::Instant::now() >= deadline)
        });
        for count in 1..=6 {
            let compact_hold = (count == 6).then(|| handle.hold_compact());
            if count > 1 {
                time.0
                    .store(100 + (count - 1) as i64 * 2_000, Ordering::SeqCst);
                handle.wake_for_clock();
            }
            handle.wait_for_publishes(count);
            if count < 6 {
                handle.wait_for_idle_wait();
                assert_eq!(
                    handle.unsettled_requests(),
                    1,
                    "partial publication and compaction must not settle materialization"
                );
            } else {
                handle.wait_for_compact_hold();
                assert_eq!(
                    handle.unsettled_requests(),
                    1,
                    "final compaction must settle first"
                );
            }
            drop(compact_hold);
        }
        assert_eq!(waiter.join().unwrap(), crate::AnalyticsWriterStatus::Idle);
    });
    let manifest = semon_asi::read_manifest(&store_root).unwrap().unwrap();
    assert_eq!(
        semon_asi::read_live_map(&store_root, &manifest)
            .unwrap()
            .len(),
        6
    );
    handle.stop_and_join();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_writer_error_between_chunks_belongs_to_its_request() {
    let (root, options, rows, native) = fixture("bounded-writer-error", 4);
    let store_root = root.join("writer-store");
    let time = Arc::new(clock());
    let mut config = crate::AnalyticsWriterConfig::new(
        NAMESPACE,
        crate::events::EventCache::path(&options.cache),
        &store_root,
    );
    config.input_budget = budget(1, 8_192);
    let handle = crate::analytics_writer::start_with_clock(config, Arc::clone(&time));
    let mut ordered: Vec<_> = rows.iter().collect();
    ordered.sort_by(|left, right| left.key.cmp(&right.key));
    let key = &ordered[1].key;
    let metadata: String = native
        .connection
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key=?1",
            [key],
            |row| row.get(0),
        )
        .unwrap();
    std::thread::scope(|scope| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let handle_ref = &handle;
        let first = scope.spawn(move || {
            handle_ref.publish_and_wait_until(|| std::time::Instant::now() >= deadline)
        });
        handle.wait_for_publishes(1);
        handle.wait_for_idle_wait();
        let second = scope.spawn(move || {
            handle_ref.publish_and_wait_until(|| std::time::Instant::now() >= deadline)
        });
        handle.wait_for_requests(2);
        native
            .connection
            .execute(
                "UPDATE session_catalog SET metadata='{}' WHERE session_key=?1",
                [key],
            )
            .unwrap();
        time.0.store(2_100, Ordering::SeqCst);
        handle.wake_for_clock();
        assert!(matches!(
            first.join().unwrap(),
            crate::AnalyticsWriterStatus::Refused { .. }
        ));
        assert_eq!(
            handle.unsettled_requests(),
            1,
            "later request must keep its own outcome"
        );
        assert_eq!(handle.published(), 1);
        native
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=?1 WHERE session_key=?2",
                params![metadata, key],
            )
            .unwrap();
        for count in 2..=4 {
            time.0.store(100 + count as i64 * 2_000, Ordering::SeqCst);
            handle.wake_for_clock();
            handle.wait_for_publishes(count);
            handle.wait_for_idle_wait();
            if count < 4 {
                assert_eq!(handle.unsettled_requests(), 1);
            }
        }
        assert_eq!(second.join().unwrap(), crate::AnalyticsWriterStatus::Idle);
    });
    assert_eq!(handle.input_attempts(), 5);
    handle.stop_and_join();
    let manifest = semon_asi::read_manifest(&store_root).unwrap().unwrap();
    assert_eq!(
        semon_asi::read_live_map(&store_root, &manifest)
            .unwrap()
            .len(),
        4
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_writer_cancellation_mid_drain_preserves_the_partial_checkpoint() {
    let (root, options, _, _native) = fixture("bounded-writer-cancel", 4);
    let store_root = root.join("writer-store");
    let time = Arc::new(clock());
    let mut config = crate::AnalyticsWriterConfig::new(
        NAMESPACE,
        crate::events::EventCache::path(&options.cache),
        &store_root,
    );
    config.input_budget = budget(1, 8_192);
    let handle = crate::analytics_writer::start_with_clock(config, time);
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let handle_ref = &handle;
        let cancelled_ref = &cancelled;
        let waiter = scope.spawn(move || {
            handle_ref.publish_and_wait_until(|| {
                cancelled_ref.load(Ordering::SeqCst) || std::time::Instant::now() >= deadline
            })
        });
        handle.wait_for_publishes(1);
        handle.wait_for_idle_wait();
        cancelled.store(true, Ordering::SeqCst);
        assert_eq!(
            waiter.join().unwrap(),
            crate::AnalyticsWriterStatus::Stopped
        );
        assert_eq!(handle.unsettled_requests(), 0);
    });
    handle.stop_and_join();
    let manifest = semon_asi::read_manifest(&store_root).unwrap().unwrap();
    assert_eq!(manifest.generation, 1);
    assert!(manifest.source_missing.is_empty());
    let checkpoint: Value = serde_json::from_str(&manifest.checkpoints[NAMESPACE]).unwrap();
    assert_eq!(checkpoint["cursor"]["phase"], "inventory");
    let mut resumed = Store::open_with_clock(&store_root, clock()).unwrap();
    assert_eq!(
        drain(
            &mut resumed,
            &mut ObservedInput::open(&options),
            budget(1, 8_192)
        ),
        3
    );
    assert_eq!(resumed.live_map().len(), 4);
    drop(resumed);
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
        serde_json::json!({"phase":"replay", "after":-1, "through":current}),
        serde_json::json!({"phase":"replay", "after":current, "through":current}),
        serde_json::json!({"phase":"replay", "after":current, "through":current-1}),
        serde_json::json!({"phase":"replay", "after":current, "through":current+11}),
        serde_json::json!({"phase":"replay", "after":current, "through":current+1}),
        serde_json::json!({"phase":"replay", "after":current+1, "through":current+10}),
        serde_json::json!({"phase":"replay", "after":0}),
    ] {
        let mut forged = original.clone();
        forged["cursor"] = cursor;
        invalid.push(forged.to_string());
    }
    for cursor in [
        serde_json::json!({"phase":"inventory", "snapshot":current, "through":"native-cx-03", "after":"native-cl-01"}),
        serde_json::json!({"phase":"inventory", "snapshot":current, "through":"zzzz", "after":"native-cl-00"}),
        serde_json::json!({"phase":"inventory", "snapshot":current, "through":"native-cl-00", "after":"native-cx-03"}),
        serde_json::json!({"phase":"inventory", "snapshot":current+1, "through":"native-cx-03", "after":"native-cl-00"}),
        serde_json::json!({"phase":"inventory", "snapshot":-1, "through":"native-cx-03", "after":"native-cl-00"}),
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
