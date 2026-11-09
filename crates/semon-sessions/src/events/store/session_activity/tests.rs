use super::super::tests::{
    catalog_fixture_projection, catalog_native_fixture, catalog_write, opened, scratch,
};
use super::*;
use crate::events::session_activity::{ActivityStepId, Ownership};

fn read(c: &Connection, row: &crate::model::summary::CatalogRow) -> Page {
    page(
        c,
        row,
        None,
        &crate::pricing::snapshot().fingerprint,
        0,
        256,
    )
    .unwrap()
    .unwrap()
}

#[test]
fn activity_catalog_publication_rolls_back_header_steps_and_catalog_together() {
    let root = scratch("activity-atomic");
    let (options, mut rows, slots) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter_mut().find(|row| row.harness == "claude").unwrap();
    let key = row.key.clone();
    let before = read(&store.connection, row);
    let generation = store.session_catalog_generation().unwrap();
    row.session_activity.as_mut().unwrap().activity.steps[0]
        .tokens
        .output += 1;
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_activity BEFORE INSERT ON main.session_activity_steps BEGIN SELECT RAISE(ABORT,'activity failure'); END;").unwrap();
    assert!(catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).is_err());
    assert_eq!(store.session_catalog_generation().unwrap(), generation);
    assert_eq!(
        read(
            &store.connection,
            rows.iter().find(|row| row.key == key).unwrap()
        ),
        before
    );
    store
        .connection
        .execute_batch("DROP TRIGGER fail_activity")
        .unwrap();
    assert_eq!(
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto)
            .unwrap()
            .0,
        Outcome::Written
    );
    assert_ne!(
        read(
            &store.connection,
            rows.iter().find(|row| row.key == key).unwrap()
        )
        .generation,
        before.generation
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_pages_require_exact_sources_generation_rules_and_prices() {
    let root = scratch("activity-generation");
    let (options, rows, slots) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let initial = read(&store.connection, row);
    let price = crate::pricing::snapshot().fingerprint.clone();
    assert_eq!(initial.total, 1);
    assert!(matches!(initial.steps[0].id, ActivityStepId::Claude(_)));
    assert_eq!(initial.steps[0].ownership, Ownership::Exclusive);
    assert!(
        page(&store.connection, row, Some("stale"), &price, 0, 1)
            .unwrap()
            .is_none()
    );
    assert!(
        page(&store.connection, row, None, "stale-prices", 0, 1)
            .unwrap()
            .is_none()
    );
    let mut moved = row.clone();
    moved.sources[0].offset += 1;
    assert!(
        page(&store.connection, &moved, None, &price, 0, 1)
            .unwrap()
            .is_none()
    );
    store
        .connection
        .execute(
            "UPDATE session_activity_projections SET rules_version=0 WHERE session_key=?1",
            [&row.key],
        )
        .unwrap();
    assert!(
        page(&store.connection, row, None, &price, 0, 1)
            .unwrap()
            .is_none()
    );
    assert!(
        repairs(&store.connection)
            .unwrap()
            .contains(&row.sources[0].path.to_string_lossy().into_owned())
    );
    assert_eq!(
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto)
            .unwrap()
            .0,
        Outcome::Written
    );
    assert_eq!(read(&store.connection, row), initial);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_anchor_admits_only_current_exact_records() {
    let root = scratch("activity-anchor");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let page = read(&store.connection, row);
    let anchor = crate::transcript_range::StepAnchor {
        generation: page.generation.clone(),
        ordinal: 0,
        id: page.steps[0].id.clone(),
    };
    let resolved =
        crate::transcript_range::resolve_step_slot(&store.connection, row, false, &anchor).unwrap();
    assert_eq!(resolved["available"], true);
    assert_eq!(resolved["basis"], "native_record_id");
    let selected = resolved["slot"].as_i64().unwrap();
    store
        .connection
        .execute(
            "INSERT INTO session_slots(session_key,slot,metadata) SELECT session_key,slot+1000,metadata FROM session_slots WHERE session_key=?1 AND slot=?2",
            params![row.key, selected],
        )
        .unwrap();
    let repeated =
        crate::transcript_range::resolve_step_slot(&store.connection, row, false, &anchor).unwrap();
    assert_eq!(repeated["slot"], selected);
    let mut stale = anchor.clone();
    stale.generation = "stale".into();
    assert_eq!(
        crate::transcript_range::resolve_step_slot(&store.connection, row, false, &stale).unwrap()
            ["reason"],
        "stale_activity"
    );
    let mut changed = row.clone();
    changed.sources[0].offset += 1;
    assert_eq!(
        crate::transcript_range::resolve_step_slot(&store.connection, &changed, false, &anchor)
            .unwrap()["reason"],
        "source_generation_mismatch"
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn streamed_claude_anchor_selects_lowest_exact_record() {
    use std::io::Write;

    let root = scratch("streamed-activity-anchor");
    let (options, _, _) = catalog_native_fixture(&root, 2, false);
    let path = options
        .claude_home
        .join("projects/-work-proj/native-cl-00.jsonl");
    let record = serde_json::json!({
        "type": "assistant", "uuid": "assistant-second", "sessionId": "native-cl-00",
        "timestamp": "2026-09-24T00:01:01Z",
        "message": {"id": "message-native-cl-00", "role": "assistant",
            "model": "claude-sonnet-4-5", "content": [{"type": "text", "text": "More"}],
            "usage": {"input_tokens": 10, "output_tokens": 6}}
    });
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{record}"
    )
    .unwrap();
    let (rows, slots) = catalog_fixture_projection(&options);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let page = read(&store.connection, row);
    let anchor = crate::transcript_range::StepAnchor {
        generation: page.generation,
        ordinal: 0,
        id: page.steps[0].id.clone(),
    };
    let (_, usage) = step_at(&store.connection, row, &anchor.generation, 0)
        .unwrap()
        .unwrap();
    let Usage::Claude(usage) = usage else {
        panic!("expected Claude usage");
    };
    assert!(usage.record_ids.contains("assistant-native-cl-00"));
    assert!(usage.record_ids.contains("assistant-second"));
    let mut matching = store
        .connection
        .prepare("SELECT slot, metadata FROM session_slots WHERE session_key=?1 ORDER BY slot")
        .unwrap();
    let matches = matching
        .query_map([&row.key], |record| {
            Ok((record.get::<_, i64>(0)?, record.get::<_, String>(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .filter_map(|(slot, metadata)| {
            let saved: crate::slot_projection::SavedSlot = serde_json::from_str(&metadata).unwrap();
            saved
                .native_event_id
                .filter(|id| usage.record_ids.contains(id))
                .map(|id| (slot, id))
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 2);
    assert_ne!(matches[0].1, matches[1].1);
    let resolved =
        crate::transcript_range::resolve_step_slot(&store.connection, row, false, &anchor).unwrap();
    assert_eq!(resolved["basis"], "native_record_id");
    assert_eq!(resolved["slot"], matches[0].0);
    drop(matching);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_anchor_requires_a_rendered_exact_call() {
    use crate::slot_projection::{File, Outcome, Recipe, SavedSlot};
    let root = scratch("codex-activity-anchor");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "codex").unwrap();
    let identity = ["thread", "session", "turn", "response"].map(str::to_owned);
    let position = SourcePosition {
        source: row.sources[0].path.to_string_lossy().into_owned(),
        offset: 42,
    };
    let usage = UsageStep::normalized_codex(
        identity.clone(),
        position.clone(),
        CodexUsageEvent {
            model: "synthetic".into(),
            tokens: Default::default(),
            timestamp: None,
        },
    );
    let mut projection = Projection::derive(
        row.key.clone(),
        vec![usage],
        &Default::default(),
        vec![],
        vec![],
        crate::pricing::snapshot(),
    );
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &projection).unwrap();
    tx.commit().unwrap();
    let page = read(&store.connection, row);
    let mut anchor = crate::transcript_range::StepAnchor {
        generation: page.generation,
        ordinal: 0,
        id: ActivityStepId::Codex(identity),
    };
    assert_eq!(
        crate::transcript_range::resolve_step_slot(&store.connection, row, false, &anchor).unwrap()
            ["reason"],
        "no_exact_record"
    );
    projection.activity.steps[0].call_position = Some(position);
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &projection).unwrap();
    tx.commit().unwrap();
    anchor.generation = read(&store.connection, row).generation;
    let slot = SavedSlot {
        recipe: Recipe::Tool {
            outcome: Outcome::Ok,
            name: "exec_command".into(),
            result_source: false,
            reply: None,
            item: None,
            background: None,
        },
        file: Some(File {
            path: row.sources[0].path.clone(),
            cwd: None,
        }),
        offset: 42,
        block: 0,
        t: None,
        turn: None,
        first: false,
        field: None,
        native_event_id: None,
        images: vec![],
    };
    store
        .connection
        .execute(
            "INSERT INTO session_slots(session_key,slot,metadata) VALUES(?1,1000,?2)",
            params![row.key, serde_json::to_string(&slot).unwrap()],
        )
        .unwrap();
    let resolved =
        crate::transcript_range::resolve_step_slot(&store.connection, row, false, &anchor).unwrap();
    assert_eq!(resolved["basis"], "call_identity");
    assert_eq!(resolved["slot"], 1000);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_repricing_resumes_and_never_serves_mixed_prices() {
    let root = scratch("activity-reprice");
    let (options, mut rows, _) = catalog_native_fixture(&root, 2, true);
    let row = rows.iter_mut().find(|row| row.harness == "claude").unwrap();
    let original = row.session_activity.as_ref().unwrap().clone();
    let usage = (0..260)
        .map(|n| {
            UsageStep::claude(
                format!("m-{n:03}"),
                SourcePosition {
                    source: row.sources[0].path.to_string_lossy().into_owned(),
                    offset: n,
                },
                match original.inputs.values().next().unwrap() {
                    Usage::Claude(used) => used.clone(),
                    _ => unreachable!(),
                },
            )
        })
        .collect();
    let old = crate::pricing::snapshot();
    let projection = Projection::derive(
        row.key.clone(),
        usage,
        &Default::default(),
        Vec::new(),
        Vec::new(),
        old.clone(),
    );
    let path = crate::events::EventCache::path(&options.cache);
    let (mut store, _) = opened(&path);
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &projection).unwrap();
    tx.commit().unwrap();
    let before = read(&store.connection, row);
    let published = changes(&store.connection);
    assert_eq!(published.last(), Some(&(row.key.clone(), "changed".into())));
    let mut new = (*old).clone();
    new.fingerprint = "synthetic-reprice".into();
    let new = Arc::new(new);
    let tx = store.connection.transaction().unwrap();
    assert!(reprice_step(&tx, &new).unwrap());
    tx.commit().unwrap();
    assert_eq!(changes(&store.connection), published, "partial reprice");
    assert!(
        page(&store.connection, row, None, &old.fingerprint, 0, 256)
            .unwrap()
            .is_none()
    );
    assert!(
        page(&store.connection, row, None, &new.fingerprint, 0, 256)
            .unwrap()
            .is_none()
    );
    drop(store);
    let (mut store, _) = opened(&path);
    for _ in 0..3 {
        let tx = store.connection.transaction().unwrap();
        reprice_step(&tx, &new).unwrap();
        tx.commit().unwrap();
    }
    let repriced = changes(&store.connection);
    assert_eq!(repriced.len(), published.len() + 1, "final batch only");
    assert_eq!(repriced.last(), Some(&(row.key.clone(), "changed".into())));
    let after = page(&store.connection, row, None, &new.fingerprint, 0, 256)
        .unwrap()
        .unwrap();
    assert_eq!(after.total, 260);
    assert_eq!(after.steps.len(), 256);
    assert_ne!(after.generation, before.generation);
    assert!(
        page(
            &store.connection,
            row,
            Some(&before.generation),
            &new.fingerprint,
            0,
            1
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        page(
            &store.connection,
            row,
            Some(&after.generation),
            &new.fingerprint,
            256,
            256
        )
        .unwrap()
        .unwrap()
        .steps
        .len(),
        4
    );
    // The final batch recomputes every price-dependent summary value.
    let summary: String = store
        .connection
        .query_row(
            "SELECT summary FROM session_activity_projections WHERE session_key=?1",
            [&row.key],
            |record| record.get(0),
        )
        .unwrap();
    let summary: activity::analytics::Analytics = serde_json::from_str(&summary).unwrap();
    assert_eq!(summary.pricing_fingerprint, new.fingerprint);
    assert_eq!(summary.counts.totals.exclusive.steps, 260);
    let pricing = &summary.counts.coverage.pricing;
    assert_eq!(
        pricing.full_steps + pricing.partial_steps + pricing.unpriced_steps,
        260
    );
    let preview: String = store
        .connection
        .query_row(
            "SELECT preview FROM session_activity_projections WHERE session_key=?1",
            [&row.key],
            |record| record.get(0),
        )
        .unwrap();
    let preview: activity::analytics::Preview = serde_json::from_str(&preview).unwrap();
    assert_eq!((preview.steps_total, preview.bins.len()), (260, 52));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_observations_survive_restart_and_facts_share_publication_generation() {
    use crate::events::session_activity::classify::{self, Change, ChangeFact, WasteKind};
    let root = scratch("activity-evidence");
    let (options, mut rows, _) = catalog_native_fixture(&root, 2, true);
    let path = crate::events::EventCache::path(&options.cache);
    let (mut store, _) = opened(&path);
    let record = serde_json::json!({"type":"assistant","message":{"id":"synthetic-message","role":"assistant","content":[{"type":"tool_use","id":"synthetic-call","name":"Agent","input":{"prompt":"private-marker"}}]}});
    let mut index = FileIndex::default();
    crate::events::claude(&mut index, &record, 0);
    classify::capture(
        &mut index,
        "claude",
        &record,
        0,
        &serde_json::to_vec(&record).unwrap(),
    );
    let ledger = Ledger {
        stat: Stat {
            dev: 1,
            ino: 2,
            size: 1000,
            modified_ns: 1,
            changed_ns: Some(1),
        },
        offset: 1000,
        prefix: [1; 32],
        tail: [2; 32],
    };
    store
        .write_one("synthetic.jsonl", None, None, &ledger, &index)
        .unwrap();
    drop(store);
    let (mut store, _) = opened(&path);
    let (_, restored) = store.read_one("synthetic.jsonl").unwrap().unwrap();
    assert_eq!(
        restored.events[0].activity_observation,
        index.events[0].activity_observation
    );
    let metadata: String = store
        .connection
        .query_row(
            "SELECT y FROM events JOIN files USING(file_id) WHERE path='synthetic.jsonl'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!metadata.contains("private-marker"));
    let row = rows.iter_mut().find(|row| row.harness == "claude").unwrap();
    let projection = row.session_activity.as_mut().unwrap();
    projection.activity.steps[0].waste = Some(WasteKind::OrchestrationWait);
    projection.activity.steps[0].waste_category = Some(WasteKind::OrchestrationWait.category());
    projection.activity.facts.push(ChangeFact {
        identity: "synthetic-fact".into(),
        timestamp: Some(100),
        step_id: Some(projection.activity.steps[0].id.clone()),
        position: SourcePosition {
            source: row.sources[0].path.to_string_lossy().into_owned(),
            offset: 0,
        },
        record_digest: "synthetic-digest".into(),
        change: Change::Commit {
            branch: "topic".into(),
            sha: "abc1234".into(),
        },
    });
    let projection = projection.clone();
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &projection).unwrap();
    tx.commit().unwrap();
    let read = read(&store.connection, row);
    assert_eq!(read.steps[0].waste, Some(WasteKind::OrchestrationWait));
    let (generation, metadata): (String, String) = store
        .connection
        .query_row(
            "SELECT generation,metadata FROM session_activity_facts WHERE session_key=?1",
            [&row.key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(generation, read.generation);
    assert_eq!(
        serde_json::from_str::<ChangeFact>(&metadata).unwrap(),
        projection.activity.facts[0]
    );
    let tx = store.connection.transaction().unwrap();
    let mut changed = projection.clone();
    changed.activity.facts.clear();
    publish(&tx, row, &changed).unwrap();
    tx.rollback().unwrap();
    let count: i64 = store
        .connection
        .query_row(
            "SELECT count(*) FROM session_activity_facts WHERE session_key=?1",
            [&row.key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "rollback preserves facts with the old header");
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

fn changes(c: &Connection) -> Vec<(String, String)> {
    c.prepare("SELECT session_key,kind FROM silver_change ORDER BY seq")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn the_change_log_is_empty_before_activity_is_query_ready_and_has_a_row_after() {
    let root = scratch("change-log-ready");
    let (store, _) = opened(&root.join("fresh.sqlite3"));
    assert!(changes(&store.connection).is_empty());
    drop(store);
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    assert!(changes(&store.connection).contains(&(row.key.clone(), "changed".into())));
    let orphans: i64 = store
        .connection
        .query_row(
            "SELECT count(*) FROM silver_change WHERE session_key NOT IN (SELECT session_key FROM session_activity_projections)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "a change names only query-ready sessions");
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rewriting_activity_logs_a_change_only_when_it_differs() {
    let root = scratch("change-log-rewrite");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let projection = row.session_activity.as_ref().unwrap();
    // The fixture row may differ from the published one in catalog fields.
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, projection).unwrap();
    tx.commit().unwrap();
    let start = changes(&store.connection).len();
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, projection).unwrap();
    tx.commit().unwrap();
    assert_eq!(changes(&store.connection).len(), start, "unchanged");
    let mut rewritten = projection.clone();
    rewritten.activity.steps[0].tokens.output += 1;
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &rewritten).unwrap();
    tx.commit().unwrap();
    let logged = changes(&store.connection);
    assert_eq!(logged.len(), start + 1);
    assert_eq!(logged.last(), Some(&(row.key.clone(), "changed".into())));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn removing_a_mismatched_projection_logs_a_removal_once() {
    let root = scratch("change-log-removed");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let start = changes(&store.connection).len();
    let mut moved = row.clone();
    moved.sources[0].offset += 1;
    let tx = store.connection.transaction().unwrap();
    remove_mismatched(&tx, &moved).unwrap();
    tx.commit().unwrap();
    let logged = changes(&store.connection);
    assert_eq!(logged.len(), start + 1);
    assert_eq!(logged.last(), Some(&(row.key.clone(), "removed".into())));
    let tx = store.connection.transaction().unwrap();
    remove_mismatched(&tx, &moved).unwrap();
    tx.commit().unwrap();
    assert_eq!(changes(&store.connection).len(), start + 1, "nothing left");
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

fn publish_row(
    store: &mut SqliteStore,
    row: &crate::model::summary::CatalogRow,
    projection: &Projection,
) {
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, projection).unwrap();
    tx.commit().unwrap();
}

#[test]
fn catalog_changes_log_a_change_without_rewriting_unchanged_activity() {
    let root = scratch("change-log-metadata");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let projection = row.session_activity.as_ref().unwrap();
    publish_row(&mut store, row, projection);
    let mut variants = Vec::new();
    for edit in 0..12 {
        let mut next = row.clone();
        match edit {
            0 => next.branch = Some("another-branch".into()),
            1 => next.parent_source = Some("another-method".into()),
            2 => next.start = Some(next.start.unwrap_or(0) + 1),
            3 => next.repo = Some("another-repo".into()),
            4 => next.overview.as_mut().unwrap().set_state("synthetic-state"),
            5 => next.model = "another-model".into(),
            6 => next.effort = Some("high".into()),
            7 => next.tokens[0] += 1.0,
            8 => next.cost.usd = Some(next.cost.usd.unwrap_or(0.0) + 1.0),
            9 => next.name = "another-name".into(),
            10 => next.harness = "another-harness".into(),
            _ => next.native_ids.push("another-native-id".into()),
        }
        variants.push(next);
    }
    for (field, value) in [
        ("reported_runs", serde_json::json!([{"cost_usd":2.0}])),
        ("cost_check", serde_json::json!([{"reported_usd":2.0}])),
        ("claude_usage", serde_json::json!({"complete":false})),
        ("copilot", serde_json::json!({"usage":null})),
        ("incomplete_usage_days", serde_json::json!([123])),
        ("incomplete_usage_time", serde_json::json!(true)),
        ("lane", serde_json::json!("changed")),
        ("role", serde_json::json!("changed")),
    ] {
        let mut next = row.clone();
        let mut overview = serde_json::to_value(next.overview.as_ref().unwrap()).unwrap();
        overview["home"][field] = value;
        next.overview = Some(serde_json::from_value(overview).unwrap());
        variants.push(next);
    }
    for next in &variants {
        let before = changes(&store.connection).len();
        let generation: String = store
            .connection
            .query_row(
                "SELECT generation FROM session_activity_projections WHERE session_key=?1",
                [&row.key],
                |r| r.get(0),
            )
            .unwrap();
        publish_row(&mut store, next, projection);
        let logged = changes(&store.connection);
        assert_eq!(logged.len(), before + 1);
        assert_eq!(logged.last(), Some(&(row.key.clone(), "changed".into())));
        publish_row(&mut store, next, projection);
        assert_eq!(changes(&store.connection).len(), before + 1, "repeat");
        let kept: String = store
            .connection
            .query_row(
                "SELECT generation FROM session_activity_projections WHERE session_key=?1",
                [&row.key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, generation, "steps are not rewritten");
        publish_row(&mut store, row, projection);
    }
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

fn count(logged: &[(String, String)], key: &str, kind: &str) -> usize {
    logged
        .iter()
        .filter(|(k, found)| k == key && found == kind)
        .count()
}

fn digest(c: &Connection, key: &str) -> Option<String> {
    c.query_row(
        "SELECT digest FROM silver_session_state WHERE session_key=?1",
        [key],
        |r| r.get(0),
    )
    .optional()
    .unwrap()
}

const NO_MEMBERS: &str = "CREATE TEMP TABLE IF NOT EXISTS catalog_members (session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_members;";

#[test]
fn a_withdrawn_session_is_logged_changed_and_again_when_it_returns() {
    let root = scratch("change-log-return");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let projection = row.session_activity.as_ref().unwrap();
    publish_row(&mut store, row, projection);
    let settled = digest(&store.connection, &row.key);
    let start = changes(&store.connection);
    store.connection.execute_batch(NO_MEMBERS).unwrap();
    let retire = |store: &mut SqliteStore| {
        let tx = store.connection.transaction().unwrap();
        withdraw(&tx, None).unwrap();
        tx.execute(
            "UPDATE session_catalog SET lifecycle='retained' WHERE lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
    };
    retire(&mut store);
    let withdrawn = changes(&store.connection);
    assert_eq!(
        count(&withdrawn, &row.key, "changed"),
        count(&start, &row.key, "changed") + 1
    );
    assert_eq!(count(&withdrawn, &row.key, "removed"), 0);
    assert_eq!(
        digest(&store.connection, &row.key).as_deref(),
        Some("retained")
    );
    let kept: i64 = store
        .connection
        .query_row(
            "SELECT count(*) FROM session_activity_projections WHERE session_key=?1",
            [&row.key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1, "analytics state is kept");
    retire(&mut store);
    assert_eq!(
        changes(&store.connection),
        withdrawn,
        "a repeat logs nothing"
    );
    publish_row(&mut store, row, projection);
    let returned = changes(&store.connection);
    assert_eq!(
        count(&returned, &row.key, "changed"),
        count(&withdrawn, &row.key, "changed") + 1
    );
    assert_eq!(digest(&store.connection, &row.key), settled);
    publish_row(&mut store, row, projection);
    assert_eq!(changes(&store.connection), returned);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scoped_withdrawal_searches_the_session_key_index_and_touches_one_session() {
    let root = scratch("change-log-scoped");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    store.connection.execute_batch(NO_MEMBERS).unwrap();
    let (log, mark) = withdraw_sql(true);
    for sql in [log, mark] {
        let plan = store
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(["key"], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("SEARCH session_catalog"), "{plan}");
        assert!(!plan.contains("SCAN session_catalog"), "{plan}");
    }
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let start = changes(&store.connection);
    let tx = store.connection.transaction().unwrap();
    withdraw(&tx, Some(&row.key)).unwrap();
    tx.commit().unwrap();
    let logged = changes(&store.connection);
    assert_eq!(logged.len(), start.len() + 1);
    assert_eq!(logged.last(), Some(&(row.key.clone(), "changed".into())));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_focused_publication_leaves_the_catalog_digest_for_the_catalog_publication() {
    let root = scratch("change-log-focused");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let path = crate::events::EventCache::path(&options.cache);
    let (mut store, _) = opened(&path);
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    publish_row(&mut store, row, row.session_activity.as_ref().unwrap());
    let settled = digest(&store.connection, &row.key);
    drop(store);
    // The focused derivation sees newer Catalog fields and newer activity.
    let mut focused = row.clone();
    focused.branch = Some("focused-branch".into());
    focused.session_activity.as_mut().unwrap().activity.steps[0]
        .tokens
        .output += 1;
    let start = {
        let (store, _) = opened(&path);
        changes(&store.connection)
    };
    publish_selected(&options, &focused, None).unwrap();
    let (mut store, _) = opened(&path);
    let after_focused = changes(&store.connection);
    assert_eq!(
        count(&after_focused, &row.key, "changed"),
        count(&start, &row.key, "changed") + 1,
        "its activity rewrite is logged"
    );
    assert_eq!(digest(&store.connection, &row.key), settled);
    // The analytics checkpoint has read everything so far. The Catalog
    // publication that follows must still be logged.
    publish_row(
        &mut store,
        &focused,
        focused.session_activity.as_ref().unwrap(),
    );
    let published = changes(&store.connection);
    assert_eq!(
        count(&published, &row.key, "changed"),
        count(&after_focused, &row.key, "changed") + 1
    );
    assert_ne!(digest(&store.connection, &row.key), settled);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_change_of_repository_identity_alone_is_logged() {
    let root = scratch("change-log-origin");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let projection = row.session_activity.as_ref().unwrap();
    publish_row(&mut store, row, projection);
    for (origin, logs) in [
        (Some("github.com/a/app"), 1),
        (Some("github.com/a/app"), 0),
        (Some("github.com/b/app"), 1),
        (None, 1),
    ] {
        let before = count(&changes(&store.connection), &row.key, "changed");
        let mut next = row.clone();
        next.repo_id = origin.map(|origin| crate::RepoId::Remote(origin.into()));
        publish_row(&mut store, &next, projection);
        let after = count(&changes(&store.connection), &row.key, "changed");
        assert_eq!(after - before, logs, "{origin:?}");
        let fresh = Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
        let saved: Option<String> = fresh
            .query_row(
                "SELECT repo_id FROM silver_session_state WHERE session_key=?1",
                [&row.key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            saved.map(|json| serde_json::from_str::<crate::RepoId>(&json).unwrap()),
            next.repo_id
        );
    }
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_identity_and_digest_roll_back_when_change_logging_fails() {
    let root = scratch("change-log-repo-atomic");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let projection = row.session_activity.as_ref().unwrap();
    let mut initial = row.clone();
    initial.repo_id = Some(crate::RepoId::Remote("a.test/team/app".into()));
    publish_row(&mut store, &initial, projection);
    let before = digest(&store.connection, &row.key);
    let logged = changes(&store.connection);
    let mut changed = initial.clone();
    changed.repo_id = Some(crate::RepoId::Path("/srv/private/app.git".into()));
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_change BEFORE INSERT ON main.silver_change BEGIN SELECT RAISE(ABORT,'change failure'); END;").unwrap();
    let tx = store.connection.transaction().unwrap();
    assert!(publish(&tx, &changed, projection).is_err());
    tx.rollback().unwrap();
    assert_eq!(digest(&store.connection, &row.key), before);
    assert_eq!(changes(&store.connection), logged);
    let fresh = Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
    let saved: String = fresh
        .query_row(
            "SELECT repo_id FROM silver_session_state WHERE session_key=?1",
            [&row.key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<crate::RepoId>(&saved).unwrap(),
        initial.repo_id.unwrap()
    );
    drop(fresh);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
