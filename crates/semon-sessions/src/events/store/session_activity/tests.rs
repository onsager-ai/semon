use super::super::tests::{catalog_native_fixture, catalog_write, opened, scratch};
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
    let mut new = (*old).clone();
    new.fingerprint = "synthetic-reprice".into();
    let new = Arc::new(new);
    let tx = store.connection.transaction().unwrap();
    assert!(reprice_step(&tx, &new).unwrap());
    tx.commit().unwrap();
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
