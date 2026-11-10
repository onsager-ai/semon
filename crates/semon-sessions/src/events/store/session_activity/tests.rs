use super::super::tests::{
    catalog_fixture_projection, catalog_native_fixture, catalog_write, opened, scratch,
};
use super::*;
use crate::SqliteAnalyticsInput;
use crate::analytics_writer::{AnalyticsWriterConfig, start};
use crate::events::session_activity::classify::Change;
use crate::events::session_activity::{ActivityStepId, Ownership, TimedInput, UsageOwner};
use semon_asi::{AnalyticsInput, Reconciliation, SessionChange};
use std::{
    thread,
    time::{Duration, Instant},
};

fn wait_for_writer_manifest(root: &std::path::Path) -> semon_asi::Manifest {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(manifest) = semon_asi::read_manifest(root).unwrap() {
            return manifest;
        }
        assert!(
            Instant::now() < deadline,
            "analytics writer did not publish"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn analytics_writer_publishes_admitted_fixture_rows() {
    let root = scratch("analytics-writer-publishes");
    let (options, rows, slots) = catalog_native_fixture(&root, 1, false);
    let index = crate::events::EventCache::path(&options.cache);
    let (mut cache, _) = opened(&index);
    catalog_write(&mut cache, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    drop(cache);

    let store_root = root.join("analytics-store");
    let handle = start(AnalyticsWriterConfig::new(
        "fixture-machine",
        &index,
        &store_root,
    ));
    handle.notify();
    let manifest = wait_for_writer_manifest(&store_root);
    assert!(manifest.checkpoints.contains_key("fixture-machine"));
    assert!(
        !semon_asi::read_live_map(&store_root, &manifest)
            .unwrap()
            .is_empty()
    );
    handle.stop_and_join();
    fs::remove_dir_all(root).unwrap();
}

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

fn same_stem_contributor(options: &crate::Options, compaction_id: Option<&str>) {
    let path = options
        .claude_home
        .join("projects/-work-proj-copy/native-cl-00.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut compact = serde_json::json!({
        "type":"system", "subtype":"compact_boundary", "sessionId":"native-cl-00",
        "timestamp":"2026-09-24T00:01:30Z",
        "compactMetadata":{"trigger":"auto","preTokens":155000}
    });
    if let Some(id) = compaction_id {
        compact["uuid"] = serde_json::json!(id);
    }
    let assistant = serde_json::json!({
        "type":"assistant", "uuid":"assistant-contributor", "sessionId":"native-cl-00",
        "timestamp":"2026-09-24T00:02:00Z",
        "message":{"id":"message-contributor","role":"assistant","model":"claude-sonnet-4-5",
            "content":[{"type":"text","text":"Synthetic continuation"}],
            "usage":{"input_tokens":100,"output_tokens":25}}
    });
    fs::write(path, format!("{compact}\n{assistant}\n")).unwrap();
}

#[test]
fn same_stem_claude_native_ids_admit_distinct_portable_anchors() {
    let mut prior_anchors = None;
    for label in ["native-anchors-first-root", "native-anchors-moved-root"] {
        let root = scratch(label);
        let (options, _, _) = catalog_native_fixture(&root, 1, false);
        same_stem_contributor(&options, Some("compact-contributor"));
        let (rows, slots) = catalog_fixture_projection(&options);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sources.len(), 2);
        let path = crate::events::EventCache::path(&options.cache);
        let (mut store, _) = opened(&path);
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        let mut input = SqliteAnalyticsInput::open(&path, "fixture-machine").unwrap();
        let batch = input.read_admitted(None).unwrap();
        let session = batch
            .changes
            .iter()
            .find_map(|change| match change {
                SessionChange::Admitted(session) => Some(session),
                _ => None,
            })
            .expect("same-stem contributors with native IDs must be admitted");
        let uid = "fixture-machine/claude/native-cl-00";
        let mut anchors: Vec<_> = session
            .steps
            .iter()
            .map(|step| (step.step_uid.clone(), step.turn_uid.clone()))
            .collect();
        anchors.sort();
        assert_eq!(
            anchors,
            vec![
                (
                    format!("{uid}/s/claude/message-contributor"),
                    format!("{uid}/t/message-contributor")
                ),
                (
                    format!("{uid}/s/claude/message-native-cl-00"),
                    format!("{uid}/t/user-native-cl-00")
                ),
            ],
            "prefer the initiating UUID, then the step's native message ID"
        );
        assert_eq!(session.compactions.len(), 1);
        let compact_uid = session.compactions[0].compaction_uid.clone();
        assert_eq!(compact_uid, format!("{uid}/c/compact-contributor"));
        for anchor in anchors
            .iter()
            .flat_map(|(step, turn)| [step, turn])
            .chain([&compact_uid])
        {
            assert!(!anchor.contains("%2F"));
            assert!(!anchor.contains("projects"));
            assert!(!anchor.contains(label));
        }
        let current = (anchors, compact_uid);
        if let Some(prior) = &prior_anchors {
            assert_eq!(&current, prior, "source relocation must preserve all UIDs");
        }
        prior_anchors = Some(current);
        drop(input);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn copied_native_compactions_collapse_or_withhold_only_the_conflicting_session() {
    for conflict in ["none", "timestamp", "step", "unknown-first", "unknown-last"] {
        let root = scratch(&format!("analytics-copied-compaction-{conflict}"));
        let (options, rows, _) = catalog_native_fixture(&root, 2, false);
        let original = &rows
            .iter()
            .find(|row| row.harness == "claude")
            .unwrap()
            .sources[0]
            .path;
        let prefix = fs::read_to_string(original).unwrap();
        let compact = serde_json::json!({
            "type":"system", "subtype":"compact_boundary", "uuid":"compact-shared",
            "sessionId":"native-cl-00", "timestamp":"2026-09-24T00:01:30Z",
            "compactMetadata":{"trigger":"auto","preTokens":155000}
        });
        fs::write(original, format!("{prefix}{compact}\n")).unwrap();
        // The copy overlaps the original only in the shared compaction record.
        // Its other records stay distinct: identical turn/event records would
        // collide in the causal trace tables before reaching the adapter.
        same_stem_contributor(&options, Some("compact-shared"));
        if conflict == "timestamp" {
            let copy = options
                .claude_home
                .join("projects/-work-proj-copy/native-cl-00.jsonl");
            let text = fs::read_to_string(&copy).unwrap();
            fs::write(
                &copy,
                text.replacen("2026-09-24T00:01:30Z", "2026-09-24T00:01:31Z", 1),
            )
            .unwrap();
        }
        let (rows, slots) = catalog_fixture_projection(&options);
        let row = rows.iter().find(|row| row.harness == "claude").unwrap();
        assert_eq!(row.sources.len(), 2);
        let path = crate::events::EventCache::path(&options.cache);
        let (mut store, _) = opened(&path);
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        let declared: i64 = store
            .connection
            .query_row(
                "SELECT compactions FROM session_activity_projections WHERE session_key=?1",
                [&row.key],
                |record| record.get(0),
            )
            .unwrap();
        assert_eq!(
            declared, 2,
            "both physical contributors must reach the adapter"
        );
        let persisted = |ordinal: i64| -> serde_json::Value {
            let metadata: String = store
                .connection
                .query_row(
                    "SELECT metadata FROM session_activity_compactions WHERE session_key=?1 AND ordinal=?2",
                    params![row.key, ordinal],
                    |record| record.get(0),
                )
                .unwrap();
            serde_json::from_str(&metadata).unwrap()
        };
        let mut first = persisted(0);
        assert!(!first["step_id"].is_null());
        // Which contributor persisted first is an ingest detail. Make the two
        // native facts agree on their step so only the injected conflict differs.
        let mut second = persisted(1);
        second["step_id"] = first["step_id"].clone();
        if conflict == "step" {
            let activity = read(&store.connection, row);
            second["step_id"] = activity
                .steps
                .iter()
                .map(|step| serde_json::to_value(&step.id).unwrap())
                .find(|id| *id != first["step_id"])
                .expect("the disjoint contributor has a different known step");
            assert!(!second["step_id"].is_null());
            assert_ne!(first["step_id"], second["step_id"]);
        }
        let known_timestamp = first["timestamp"].clone();
        if conflict == "unknown-first" {
            first["timestamp"] = serde_json::Value::Null;
        } else if conflict == "unknown-last" {
            second["timestamp"] = serde_json::Value::Null;
        }
        store
            .connection
            .execute(
                "UPDATE session_activity_compactions SET metadata=?2 WHERE session_key=?1 AND ordinal=0",
                params![row.key, first.to_string()],
            )
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE session_activity_compactions SET metadata=?2 WHERE session_key=?1 AND ordinal=1",
                params![row.key, second.to_string()],
            )
            .unwrap();
        let mut input = SqliteAnalyticsInput::open(&path, "fixture-machine").unwrap();
        let batch = input.read_admitted(None).unwrap();
        let uid = "fixture-machine/claude/native-cl-00";
        let session = batch.changes.iter().find_map(|change| match change {
            SessionChange::Admitted(session) if session.session_uid == uid => Some(session),
            _ => None,
        });
        if matches!(conflict, "none" | "unknown-first" | "unknown-last") {
            let session =
                session.expect("identical native facts collapse without blocking admission");
            assert_eq!(session.compactions.len(), 1);
            assert_eq!(
                session.compactions[0].compaction_uid,
                format!("{uid}/c/compact-shared")
            );
            assert!(session.compactions[0].step_uid.is_some());
            assert_eq!(
                serde_json::to_value(session.compactions[0].timestamp).unwrap(),
                known_timestamp
            );
            let projected = semon_asi::project(session).unwrap();
            assert_eq!(projected.tables["fact_compaction"].len(), 1);
            assert_eq!(projected.tables["fact_step"].len(), session.steps.len());
        } else {
            assert!(session.is_none());
            assert!(batch.changes.contains(&SessionChange::Pending {
                session_uid: uid.into()
            }));
            assert!(!batch.changes.contains(&SessionChange::SourceRestored {
                session_uid: uid.into()
            }));
        }
        assert!(batch.changes.iter().any(|change| matches!(
            change, SessionChange::Admitted(session) if session.session.native_session_id == "native-cx-01"
        )), "another session is admitted even when a compaction identity conflicts");
        drop(input);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn copied_timestampless_compactions_merge_unknown_and_known_steps_in_either_order() {
    for known_first in [false, true] {
        let root = scratch(&format!("analytics-compatible-compaction-{known_first}"));
        let (options, rows, _) = catalog_native_fixture(&root, 2, false);
        let original = &rows
            .iter()
            .find(|row| row.harness == "claude")
            .unwrap()
            .sources[0]
            .path;
        let prefix = fs::read_to_string(original).unwrap();
        let user = prefix.lines().next().unwrap();
        let compact = serde_json::json!({
            "type":"system", "subtype":"compact_boundary", "uuid":"compact-shared",
            "sessionId":"native-cl-00",
            "compactMetadata":{"trigger":"auto","preTokens":155000}
        });
        // No usage in this contributor: without time, its compaction cannot
        // acquire the other contributor's step from cross-source offsets.
        let separator = serde_json::json!({
            "type":"user", "uuid":"user-separator", "sessionId":"native-cl-00",
            "timestamp":"2026-09-24T00:01:45Z",
            "message":{"role":"user","content":"Synthetic continuation"}
        });
        let mut distinct = compact.clone();
        distinct["uuid"] = serde_json::json!("compact-distinct");
        fs::write(
            original,
            format!("{user}\n{compact}\n{separator}\n{distinct}\n"),
        )
        .unwrap();
        same_stem_contributor(&options, Some("compact-shared"));
        let copy = options
            .claude_home
            .join("projects/-work-proj-copy/native-cl-00.jsonl");
        let text = fs::read_to_string(&copy).unwrap();
        let assistant = text.lines().nth(1).unwrap();
        fs::write(&copy, format!("{compact}\n{assistant}\n")).unwrap();
        if !known_first {
            // '-' sorts before '/', '0' after it. Change only the directory
            // order so the real producer persists each ordering of the pair.
            let later = options.claude_home.join("projects/-work-proj0-copy");
            fs::rename(copy.parent().unwrap(), later).unwrap();
        }
        let (rows, slots) = catalog_fixture_projection(&options);
        let row = rows.iter().find(|row| row.harness == "claude").unwrap();
        assert_eq!(row.sources.len(), 2);
        let path = crate::events::EventCache::path(&options.cache);
        let (mut store, _) = opened(&path);
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        let physical: Vec<crate::events::session_activity::Compaction> = store
            .connection
            .prepare("SELECT metadata FROM session_activity_compactions WHERE session_key=?1 ORDER BY ordinal")
            .unwrap()
            .query_map([&row.key], |record| record.get::<_, String>(0))
            .unwrap()
            .map(|record| serde_json::from_str(&record.unwrap()).unwrap())
            .collect();
        assert_eq!(physical.len(), 3, "both real contributors are persisted");
        assert_eq!(physical[0].step_id.is_some(), known_first);
        assert_eq!(
            physical[if known_first { 1 } else { 2 }].step_id.is_some(),
            !known_first
        );
        assert!(physical.iter().all(|compact| compact.timestamp.is_none()));
        let mut input = SqliteAnalyticsInput::open(&path, "fixture-machine").unwrap();
        let batch = input.read_admitted(None).unwrap();
        let uid = "fixture-machine/claude/native-cl-00";
        let session = batch
            .changes
            .iter()
            .find_map(|change| match change {
                SessionChange::Admitted(session) if session.session_uid == uid => Some(session),
                _ => None,
            })
            .expect("compatible unknown/known associations admit the session");
        assert_eq!(session.compactions.len(), 2);
        assert_eq!(
            session.compactions[0].compaction_uid,
            format!("{uid}/c/compact-shared")
        );
        assert_eq!(
            session.compactions[0].step_uid.as_deref(),
            Some("fixture-machine/claude/native-cl-00/s/claude/message-contributor")
        );
        assert_eq!(session.compactions[0].timestamp, None);
        assert_eq!(
            session.compactions[1].compaction_uid,
            format!("{uid}/c/compact-distinct")
        );
        drop(input);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn truncated_compaction_rows_fail_integrity_even_when_ordinals_remain_contiguous() {
    for remove_all in [false, true] {
        let root = scratch("analytics-truncated-compactions");
        let (options, rows, _) = catalog_native_fixture(&root, 1, false);
        let source = &rows[0].sources[0].path;
        let mut records = fs::read_to_string(source).unwrap();
        for (id, second) in [("compact-first", 30), ("compact-last", 32)] {
            let compact = serde_json::json!({
                "type":"system", "subtype":"compact_boundary", "uuid":id,
                "sessionId":"native-cl-00", "timestamp":format!("2026-09-24T00:01:{second:02}Z"),
                "compactMetadata":{"trigger":"auto","preTokens":155000}
            });
            records.push_str(&format!("{compact}\n"));
            // Native ingest coalesces adjacent boundary/summary records.
            // A real prompt separates these two independent compactions.
            let prompt = serde_json::json!({
                "type":"user", "uuid":format!("prompt-{id}"), "sessionId":"native-cl-00",
                "timestamp":format!("2026-09-24T00:01:{:02}Z", second + 1),
                "message":{"role":"user","content":"Synthetic continuation"}
            });
            records.push_str(&format!("{prompt}\n"));
        }
        fs::write(source, records).unwrap();
        let (rows, slots) = catalog_fixture_projection(&options);
        let path = crate::events::EventCache::path(&options.cache);
        let (mut store, _) = opened(&path);
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        let mut input = SqliteAnalyticsInput::open(&path, "fixture-machine").unwrap();
        let complete = input.read_admitted(None).unwrap();
        let session = complete
            .changes
            .iter()
            .find_map(|change| match change {
                SessionChange::Admitted(session) => Some(session),
                _ => None,
            })
            .unwrap();
        assert_eq!(session.compactions.len(), 2);
        assert_eq!(
            session
                .compactions
                .iter()
                .map(|compact| compact.compaction_uid.as_str())
                .collect::<Vec<_>>(),
            vec![
                "fixture-machine/claude/native-cl-00/c/compact-first",
                "fixture-machine/claude/native-cl-00/c/compact-last"
            ]
        );
        let removed = store
            .connection
            .execute(
                if remove_all {
                    "DELETE FROM session_activity_compactions WHERE session_key=?1"
                } else {
                    "DELETE FROM session_activity_compactions WHERE session_key=?1 AND ordinal=1"
                },
                [&rows[0].key],
            )
            .unwrap();
        assert_eq!(removed, if remove_all { 2 } else { 1 });
        assert!(
            matches!(
                input.read_admitted(None),
                Err(rusqlite::Error::InvalidQuery)
            ),
            "truncated evidence cannot emit either Admitted or SourceRestored"
        );
        drop(input);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ambiguous_record_anchor_is_pending_without_blocking_other_sessions() {
    use std::io::Write;

    let root = scratch("native-anchors-pending");
    let (options, _, _) = catalog_native_fixture(&root, 1, false);
    same_stem_contributor(&options, Some("compact-contributor"));
    let healthy_path = options
        .claude_home
        .join("projects/-work-proj/native-cl-02.jsonl");
    let mut healthy_record = serde_json::json!({
        "type":"assistant", "uuid":"assistant-healthy", "sessionId":"native-cl-02",
        "timestamp":"2026-09-24T00:03:00Z",
        "message":{"id":"message-healthy","role":"assistant","model":"claude-sonnet-4-5",
            "content":[{"type":"text","text":"Synthetic answer"}],
            "usage":{"input_tokens":20,"output_tokens":10}}
    });
    fs::write(&healthy_path, format!("{healthy_record}\n")).unwrap();
    let (rows, slots) = catalog_fixture_projection(&options);
    assert_eq!(rows.len(), 2);
    let path = crate::events::EventCache::path(&options.cache);
    let (mut store, _) = opened(&path);
    catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    let mut input = SqliteAnalyticsInput::open(&path, "fixture-machine").unwrap();
    let first = input.read_admitted(None).unwrap();
    let pending_uid = "fixture-machine/claude/native-cl-00";
    assert!(first.changes.iter().any(|change| matches!(
        change, SessionChange::Admitted(session) if session.session_uid == pending_uid
    )));

    // This generation has valid steps, but its compaction has no native ID
    // and shares its filename stem with another physical contributor.
    same_stem_contributor(&options, None);
    healthy_record["uuid"] = serde_json::json!("assistant-healthy-next");
    healthy_record["message"]["id"] = serde_json::json!("message-healthy-next");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&healthy_path)
            .unwrap(),
        "{healthy_record}"
    )
    .unwrap();
    let (rows, slots) = catalog_fixture_projection(&options);
    catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    let batch = input.read_admitted(Some(&first.checkpoint)).unwrap();
    assert_eq!(batch.reconciliation, Reconciliation::Incremental);
    assert!(batch.changes.contains(&SessionChange::Pending {
        session_uid: pending_uid.into()
    }));
    assert!(!batch.changes.contains(&SessionChange::SourceRestored {
        session_uid: pending_uid.into()
    }));
    assert!(!batch.changes.iter().any(|change| matches!(
        change, SessionChange::Admitted(session) if session.session_uid == pending_uid
    )));
    let healthy = batch
        .changes
        .iter()
        .find_map(|change| match change {
            SessionChange::Admitted(session) => Some(session),
            _ => None,
        })
        .expect("the other changed session must still be admitted");
    assert_eq!(healthy.session.native_session_id, "native-cl-02");
    assert_eq!(healthy.steps.len(), 2);

    // A fresh namespace read must also carry an explicit Pending inventory
    // entry rather than silently omitting the unrepresentable session.
    let full = input.read_admitted(None).unwrap();
    assert!(full.changes.contains(&SessionChange::Pending {
        session_uid: pending_uid.into()
    }));
    assert_eq!(
        full.changes
            .iter()
            .filter(|change| matches!(change, SessionChange::Admitted(_)))
            .count(),
        1
    );

    same_stem_contributor(&options, Some("compact-contributor"));
    let (rows, slots) = catalog_fixture_projection(&options);
    catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    let restored = input.read_admitted(Some(&batch.checkpoint)).unwrap();
    assert!(restored.changes.contains(&SessionChange::SourceRestored {
        session_uid: pending_uid.into()
    }));
    assert!(restored.changes.iter().any(|change| matches!(
        change, SessionChange::Admitted(session) if session.session_uid == pending_uid
    )));
    drop(input);
    drop(store);
    fs::remove_dir_all(root).unwrap();
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
fn missing_member_retains_complete_activity_until_its_source_witness_returns() {
    fn table_rows(
        connection: &Connection,
        table: &str,
        order: &str,
        key: &str,
    ) -> Vec<Vec<rusqlite::types::Value>> {
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM {table} WHERE session_key=?1 ORDER BY {order}"
            ))
            .unwrap();
        let columns = statement.column_count();
        statement
            .query_map([key], |row| {
                (0..columns)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn header(
        connection: &Connection,
        key: &str,
    ) -> (String, String, u32, String, i64, i64, String, String) {
        connection
            .query_row(
                "SELECT generation,source_generation,rules_version,pricing_fingerprint,total,compactions,summary,preview FROM session_activity_projections WHERE session_key=?1",
                [key],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap()
    }

    fn catalog_sources(connection: &Connection, key: &str) -> Vec<(String, String, String)> {
        let mut statement = connection
            .prepare("SELECT source_path,native_id,lifecycle FROM session_catalog_sources WHERE session_key=?1 ORDER BY source_path")
            .unwrap();
        statement
            .query_map([key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    let root = scratch("activity-missing-source");
    let (options, _, _) = catalog_native_fixture(&root, 1, false);
    let missing_path = options
        .claude_home
        .join("projects/-work-proj-copy/native-cl-00.jsonl");
    fs::create_dir_all(missing_path.parent().unwrap()).unwrap();
    let missing_records = [
        serde_json::json!({
            "type":"system", "subtype":"compact_boundary", "uuid":"compact-cleared",
            "sessionId":"native-cl-00", "session_id":"native-cl-00", "timestamp":"2026-09-24T00:01:30Z",
            "compactMetadata":{"trigger":"auto","preTokens":155000}
        }),
        serde_json::json!({
            "type":"assistant", "uuid":"assistant-cleared", "sessionId":"native-cl-00",
            "session_id":"native-cl-00", "timestamp":"2026-09-24T00:02:00Z",
            "message":{"id":"message-cleared","role":"assistant","model":"claude-sonnet-4-5",
                "content":[{"type":"tool_use","id":"commit-call","name":"Bash",
                    "input":{"command":"git commit -m fixture"}}],
                "usage":{"input_tokens":100,"output_tokens":25}}
        }),
        serde_json::json!({
            "type":"user", "uuid":"result-cleared", "sessionId":"native-cl-00",
            "session_id":"native-cl-00", "timestamp":"2026-09-24T00:02:01Z",
            "message":{"role":"user","content":[{"type":"tool_result",
                "tool_use_id":"commit-call","content":"[main abc1234] fixture"}]}
        }),
    ]
    .map(|record| record.to_string())
    .join("\n")
        + "\n";
    fs::write(&missing_path, &missing_records).unwrap();

    let (mut rows, slots) = catalog_fixture_projection(&options);
    assert_eq!(rows.len(), 1, "the same-ID Claude files form one session");
    let row_index = rows.iter().position(|row| row.harness == "claude").unwrap();
    assert_eq!(rows[row_index].sources.len(), 2);
    assert!(
        rows[row_index]
            .sources
            .iter()
            .all(|source| source.native_id == "native-cl-00")
    );
    let key = rows[row_index].key.clone();
    let missing_source = missing_path.to_string_lossy().into_owned();
    {
        let projection = rows[row_index].session_activity.as_mut().unwrap();
        assert!(
            projection
                .activity
                .steps
                .iter()
                .any(|step| step.position.source == missing_source),
            "the missing file contributes its own usage step"
        );
        assert!(!projection.activity.compactions.is_empty());
        // Change facts point to their tool result record, not the usage step.
        assert!(projection.activity.facts.iter().any(|fact| {
            fact.position.source == missing_source
                && matches!(
                    &fact.change,
                    Change::Commit { branch, sha }
                        if branch == "main" && sha == "abc1234"
                )
        }));
    }

    let row = &rows[row_index];
    let path = crate::events::EventCache::path(&options.cache);
    let (mut store, _) = opened(&path);
    assert_eq!(
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto)
            .unwrap()
            .0,
        Outcome::Written
    );
    let complete = read(&store.connection, row);
    let mut input = SqliteAnalyticsInput::open(&path, "fixture-machine").unwrap();
    let first = input.read_admitted(None).unwrap();
    assert_eq!(first.reconciliation, Reconciliation::ReplaceNamespace);
    let admitted_uid = first
        .changes
        .iter()
        .find_map(|change| match change {
            SessionChange::Admitted(session)
                if session.session.native_session_id == "native-cl-00" =>
            {
                Some(session.session_uid.clone())
            }
            _ => None,
        })
        .expect("the complete source cohort is admitted");
    let checkpoint = first.checkpoint;
    let original_header = header(&store.connection, &key);
    assert_eq!(
        source_missing(&store.connection, &key).unwrap(),
        Some(false)
    );
    let original_membership = catalog_sources(&store.connection, &key);
    assert_eq!(original_membership.len(), 2);
    assert!(
        original_membership
            .iter()
            .all(|(_, native_id, lifecycle)| native_id == "native-cl-00" && lifecycle == "current")
    );
    let present_source = original_membership
        .iter()
        .find(|(source_path, _, _)| source_path != &missing_source)
        .unwrap()
        .0
        .clone();
    assert_ne!(present_source, missing_source);
    assert!(
        original_membership
            .iter()
            .any(|(source_path, _, _)| source_path == &missing_source)
    );
    let original_steps = table_rows(
        &store.connection,
        "session_activity_steps",
        "generation,ordinal",
        &key,
    );
    let original_compactions = table_rows(
        &store.connection,
        "session_activity_compactions",
        "generation,ordinal",
        &key,
    );
    let original_facts = table_rows(
        &store.connection,
        "session_activity_facts",
        "generation,identity",
        &key,
    );
    let original_witness: String = store
        .connection
        .query_row(
            "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
            [&key],
            |record| record.get(0),
        )
        .unwrap();
    assert!(!original_compactions.is_empty());
    assert_eq!(original_facts.len(), 1);
    let start = changes(&store.connection);

    fs::remove_file(&missing_path).unwrap();
    let (partial_rows, partial_slots) = catalog_fixture_projection(&options);
    let partial = partial_rows.iter().find(|row| row.key == key).unwrap();
    assert_eq!(partial.sources.len(), 1, "Catalog membership reflects disk");
    let seen = partial_rows
        .iter()
        .flat_map(|row| row.sources.iter())
        .map(|source| source.path.to_string_lossy().into_owned())
        .collect::<std::collections::BTreeSet<_>>();
    // Selected fixture projection leaves unseen ledgers alone. Reconcile this
    // fixture's complete current inventory as the native full scan does.
    let mut event_cache = crate::events::EventCache::open_scoped(&options.cache).unwrap();
    event_cache.begin_scan().unwrap();
    event_cache.begin_full_scan().unwrap();
    let mut dirty = false;
    event_cache.retain(&seen, &mut dirty);
    event_cache.end_scan();
    drop(event_cache);
    assert!(dirty, "the full scan retires the disappeared source ledger");
    assert_eq!(
        catalog_write(
            &mut store,
            &partial_rows,
            &partial_slots,
            false,
            CatalogWriter::Auto,
        )
        .unwrap()
        .0,
        Outcome::Written,
        "the current Catalog membership must publish after source retirement"
    );

    assert_eq!(source_missing(&store.connection, &key).unwrap(), Some(true));
    let unavailable = input.read_admitted(Some(&checkpoint)).unwrap();
    assert_eq!(unavailable.reconciliation, Reconciliation::Incremental);
    assert!(unavailable.changes.iter().any(|change| {
        matches!(change, SessionChange::SourceMissing { session_uid, retained_at_origin: true } if session_uid == &admitted_uid)
    }));
    let missing_membership = catalog_sources(&store.connection, &key);
    assert_eq!(missing_membership.len(), 2);
    assert!(
        missing_membership
            .iter()
            .any(|(source_path, native_id, lifecycle)| {
                source_path == &missing_source
                    && native_id == "native-cl-00"
                    && lifecycle == "retained"
            })
    );
    assert!(
        missing_membership
            .iter()
            .any(|(source_path, native_id, lifecycle)| {
                source_path == &present_source
                    && native_id == "native-cl-00"
                    && lifecycle == "current"
            })
    );
    assert!(
        page(
            &store.connection,
            partial,
            None,
            &crate::pricing::snapshot().fingerprint,
            0,
            256,
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(read(&store.connection, row), complete);
    assert_eq!(header(&store.connection, &key), original_header);
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_steps",
            "generation,ordinal",
            &key,
        ),
        original_steps
    );
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_compactions",
            "generation,ordinal",
            &key,
        ),
        original_compactions
    );
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_facts",
            "generation,identity",
            &key,
        ),
        original_facts
    );
    let witness: String = store
        .connection
        .query_row(
            "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
            [&key],
            |record| record.get(0),
        )
        .unwrap();
    assert_eq!(witness, original_witness);
    let missing_changes = changes(&store.connection);
    assert_eq!(missing_changes.len(), start.len() + 1);
    assert_eq!(
        missing_changes.last(),
        Some(&(key.clone(), "changed".into()))
    );

    assert_eq!(
        catalog_write(
            &mut store,
            &partial_rows,
            &partial_slots,
            false,
            CatalogWriter::Auto,
        )
        .unwrap()
        .0,
        Outcome::Written
    );
    assert_eq!(changes(&store.connection), missing_changes);

    // Withdraw the entire session, then return the exact admitted generation
    // without activity. Exact source proof is enough to clear missing state.
    let withdraw_session = |store: &mut SqliteStore| {
        store.connection.execute_batch(NO_MEMBERS).unwrap();
        let tx = store.connection.transaction().unwrap();
        withdraw(&tx, None).unwrap();
        tx.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key=?1", [&key]).unwrap();
        tx.commit().unwrap();
    };
    withdraw_session(&mut store);
    assert_eq!(source_missing(&store.connection, &key).unwrap(), Some(true));
    let tx = store.connection.transaction().unwrap();
    tx.execute(
        "UPDATE session_catalog SET lifecycle='current',metadata=?2 WHERE session_key=?1",
        params![key, json(row).unwrap()],
    )
    .unwrap();
    remove_mismatched(&tx, row).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        source_missing(&store.connection, &key).unwrap(),
        Some(false)
    );
    assert_eq!(header(&store.connection, &key), original_header);
    assert_eq!(count(&changes(&store.connection), &key, "removed"), 0);

    // Lose a member again and withdraw fully. Returning both identities with
    // changed source generations is only metadata proof, not a replacement.
    assert_eq!(
        catalog_write(
            &mut store,
            &partial_rows,
            &partial_slots,
            false,
            CatalogWriter::Auto,
        )
        .unwrap()
        .0,
        Outcome::Written
    );
    withdraw_session(&mut store);
    assert_eq!(source_missing(&store.connection, &key).unwrap(), Some(true));

    fs::write(&missing_path, &missing_records).unwrap();
    let (recovered_rows, recovered_slots) = catalog_fixture_projection(&options);
    let recovered = recovered_rows.iter().find(|row| row.key == key).unwrap();
    assert_eq!(recovered.sources.len(), 2);
    assert!(!witness_has_missing_member(row, recovered));
    assert_ne!(
        crate::retention::source_generation(&recovered.sources).unwrap(),
        original_header.1
    );
    let mut metadata_rows = recovered_rows.clone();
    metadata_rows
        .iter_mut()
        .find(|row| row.key == key)
        .unwrap()
        .session_activity = None;
    assert_eq!(
        catalog_write(
            &mut store,
            &metadata_rows,
            &recovered_slots,
            false,
            CatalogWriter::Auto,
        )
        .unwrap()
        .0,
        Outcome::Written
    );
    assert_eq!(source_missing(&store.connection, &key).unwrap(), Some(true));
    assert_eq!(header(&store.connection, &key), original_header);
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_steps",
            "generation,ordinal",
            &key
        ),
        original_steps
    );
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_compactions",
            "generation,ordinal",
            &key
        ),
        original_compactions
    );
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_facts",
            "generation,identity",
            &key
        ),
        original_facts
    );
    let witness: String = store
        .connection
        .query_row(
            "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
            [&key],
            |record| record.get(0),
        )
        .unwrap();
    assert_eq!(witness, original_witness);
    assert_eq!(count(&changes(&store.connection), &key, "removed"), 0);
    let before_replacement = changes(&store.connection);
    assert_eq!(
        catalog_write(
            &mut store,
            &recovered_rows,
            &recovered_slots,
            false,
            CatalogWriter::Auto,
        )
        .unwrap()
        .0,
        Outcome::Written
    );

    assert_eq!(
        source_missing(&store.connection, &key).unwrap(),
        Some(false)
    );
    assert!(
        catalog_sources(&store.connection, &key)
            .iter()
            .all(|(_, native_id, lifecycle)| native_id == "native-cl-00" && lifecycle == "current")
    );
    let recovered_page = read(&store.connection, recovered);
    assert_ne!(recovered_page.generation, complete.generation);
    let usage_step = recovered_page
        .steps
        .iter()
        .position(|step| step.position.source == missing_source)
        .unwrap();
    let (_, usage) = step_at(
        &store.connection,
        recovered,
        &recovered_page.generation,
        usage_step as u64,
    )
    .unwrap()
    .unwrap();
    let Usage::Claude(usage) = usage else {
        panic!("expected usage from the returned Claude source");
    };
    assert_eq!(usage.model_tokens.input, 100);
    assert_eq!(usage.model_tokens.output, 25);
    assert_eq!(
        table_rows(
            &store.connection,
            "session_activity_facts",
            "generation,identity",
            &key,
        )
        .len(),
        original_facts.len()
    );
    let recovered_changes = changes(&store.connection);
    assert_eq!(recovered_changes.len(), before_replacement.len() + 1);
    assert_eq!(count(&recovered_changes, &key, "removed"), 0);
    assert_eq!(
        recovered_changes.last(),
        Some(&(key.clone(), "changed".into()))
    );
    let witness: String = store
        .connection
        .query_row(
            "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
            [&key],
            |record| record.get(0),
        )
        .unwrap();
    let witness: crate::model::summary::CatalogRow = serde_json::from_str(&witness).unwrap();
    assert_eq!(witness.sources, recovered.sources);
    let restored = input.read_admitted(Some(&unavailable.checkpoint)).unwrap();
    assert!(restored.changes.iter().any(|change| {
        matches!(change, SessionChange::SourceRestored { session_uid } if session_uid == &admitted_uid)
    }));
    assert!(restored.changes.iter().any(|change| {
        matches!(change, SessionChange::Admitted(session) if session.session_uid == admitted_uid)
    }));

    drop(input);
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
    store
        .connection
        .execute(
            "UPDATE session_activity_steps SET pricing_fingerprint='mixed-price' WHERE session_key=?1 AND generation=?2 AND ordinal=0",
            params![row.key, initial.generation],
        )
        .unwrap();
    assert!(
        step_facts(&store.connection, row, &initial.generation)
            .unwrap()
            .is_none()
    );
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
        (vec![], vec![]),
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
fn step_facts_keep_private_tool_details_and_usage_ownership() {
    use crate::events::session_activity::classify::{self, LocatedObservation};
    let root = scratch("step-facts");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let usage = row
        .session_activity
        .as_ref()
        .unwrap()
        .inputs
        .values()
        .next()
        .unwrap()
        .clone();
    let mut usage = match usage {
        Usage::Claude(usage) => usage,
        Usage::Codex(_) => unreachable!(),
    };
    usage.billing.timestamp = Some(20);
    let step = UsageStep::claude(
        "facts-message".into(),
        SourcePosition {
            source: row.sources[0].path.to_string_lossy().into_owned(),
            offset: 20,
        },
        usage,
    );
    let mut read_observation = classify::call(
        "functions.Read",
        &serde_json::json!({"file_path":"/local/secret.rs"}),
        Some(step.id.clone()),
        "read".into(),
    );
    classify::finish(
        &mut read_observation,
        &serde_json::json!("ok"),
        false,
        Some(10),
        Some(15),
        "read-result".into(),
    );
    let mut shell = classify::call(
        "Bash",
        &serde_json::json!({"command":"git status"}),
        Some(step.id.clone()),
        "shell".into(),
    );
    classify::finish(
        &mut shell,
        &serde_json::json!(""),
        false,
        Some(16),
        Some(19),
        "shell-result".into(),
    );
    let projection = Projection::derive(
        row.key.clone(),
        vec![step.clone()],
        &Default::default(),
        vec![],
        vec![
            LocatedObservation {
                position: SourcePosition {
                    source: step.position.source.clone(),
                    offset: 11,
                },
                timestamp: Some(10),
                result_timestamp: Some(15),
                result_offset: Some(12),
                tool: Some(classify::tool_evidence(
                    "functions.Read",
                    &serde_json::json!({"file_path":"/local/secret.rs"}),
                )),
                observation: read_observation,
            },
            LocatedObservation {
                position: SourcePosition {
                    source: step.position.source.clone(),
                    offset: 13,
                },
                timestamp: Some(16),
                result_timestamp: Some(19),
                result_offset: Some(14),
                tool: Some(classify::tool_evidence(
                    "Bash",
                    &serde_json::json!({"command":"git status"}),
                )),
                observation: shell,
            },
            LocatedObservation {
                position: SourcePosition {
                    source: step.position.source.clone(),
                    offset: 18,
                },
                timestamp: Some(18),
                result_timestamp: Some(40),
                result_offset: Some(25),
                tool: None,
                observation: classify::call("Agent", &serde_json::json!({}), None, "future".into()),
            },
        ],
        (
            vec![],
            vec![TimedInput {
                position: SourcePosition {
                    source: step.position.source.clone(),
                    offset: 1,
                },
                timestamp: Some(1),
            }],
        ),
        crate::pricing::snapshot(),
    );
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &projection).unwrap();
    tx.commit().unwrap();
    let page = read(&store.connection, row);
    let facts = step_facts(&store.connection, row, &page.generation)
        .unwrap()
        .unwrap();
    assert_eq!(facts.len(), 1);
    let (_, id, facts) = &facts[0];
    assert_eq!(id, &step.id);
    assert_eq!(facts.usage_owner, UsageOwner::Exclusive);
    assert_eq!(facts.model_ms, Some(1));
    assert_eq!(facts.tool_ms, Some(8));
    assert_eq!(facts.tool_calls.len(), 2);
    assert_eq!(facts.tool_calls[0].tool, "Read");
    assert_eq!(
        facts.tool_calls[0].target_path.as_deref(),
        Some("/local/secret.rs")
    );
    assert_eq!(facts.tool_calls[0].ok, Some(true));
    assert_eq!(facts.tool_calls[1].target_path, None);
    let public = items(
        &store.connection,
        row,
        None,
        &crate::pricing::snapshot().fingerprint,
        false,
        None,
        1,
    )
    .unwrap()
    .unwrap();
    assert!(public.items[0].get("facts").is_none());
    assert!(public.items[0].get("target_path").is_none());
    let copied = Projection::derive(
        row.key.clone(),
        vec![step.clone()],
        &[("facts-message".into(), "other-session".into())]
            .into_iter()
            .collect(),
        vec![],
        vec![],
        (vec![], vec![]),
        crate::pricing::snapshot(),
    );
    let tx = store.connection.transaction().unwrap();
    publish(&tx, row, &copied).unwrap();
    tx.commit().unwrap();
    let copied_page = read(&store.connection, row);
    let copied_facts = step_facts(&store.connection, row, &copied_page.generation)
        .unwrap()
        .unwrap();
    assert_eq!(copied_facts[0].2.usage_owner, UsageOwner::Copied);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_repricing_resumes_and_never_serves_mixed_prices() {
    let root = scratch("activity-reprice");
    let (options, mut rows, _) = catalog_native_fixture(&root, 1, true);
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
        (Vec::new(), Vec::new()),
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
fn invalidating_a_mismatched_projection_keeps_the_last_complete_cohort_pending() {
    let root = scratch("change-log-removed");
    let (options, rows, _) = catalog_native_fixture(&root, 2, true);
    let (mut store, _) = opened(&crate::events::EventCache::path(&options.cache));
    let row = rows.iter().find(|row| row.harness == "claude").unwrap();
    let start = changes(&store.connection).len();
    let header = read(&store.connection, row);
    let mut moved = row.clone();
    moved.sources[0].offset += 1;
    let tx = store.connection.transaction().unwrap();
    remove_mismatched(&tx, &moved).unwrap();
    tx.commit().unwrap();
    let logged = changes(&store.connection);
    assert_eq!(logged.len(), start + 1);
    assert_eq!(logged.last(), Some(&(row.key.clone(), "changed".into())));
    assert_eq!(read(&store.connection, row), header);
    let tx = store.connection.transaction().unwrap();
    remove_mismatched(&tx, &moved).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        changes(&store.connection).len(),
        start + 1,
        "unchanged invalidation"
    );
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
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT harness,native_session_id FROM silver_session_identity WHERE session_key=?1",
                [&row.key],
                |record| Ok((record.get::<_, String>(0)?, record.get::<_, String>(1)?)),
            )
            .unwrap(),
        (row.harness.clone(), row.native_ids[0].clone())
    );
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
