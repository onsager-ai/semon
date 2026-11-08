use super::*;
use serde_json::json;

fn table() -> Arc<pricing::EffectiveTable> {
    // Synthetic, pinned reference data; independent of pricing refresh or logs.
    let price = json!({"price": {
        "input": 1.0, "output": 2.0, "cache_read": 0.25,
        "cache_write_5m": 1.25, "cache_write_1h": 2.0,
        "web_search_per_1k": 10.0
    }, "provenance": {}});
    Arc::new(
        serde_json::from_value(json!({
            "version": 1, "fetched_at": 0, "fingerprint": "synthetic-prices-v1",
            "providers": {
                "anthropic": {"synthetic-model": price.clone(), "partial-model": {
                    "price": {"input": 1.0, "cache_read": 0.25}, "provenance": {}
                }},
                "openai": {"synthetic-model": price}
            }
        }))
        .unwrap(),
    )
}

fn position(source: &str, offset: u64) -> SourcePosition {
    SourcePosition {
        source: source.into(),
        offset,
    }
}

fn message(model: &str, tokens: ModelTokens, timestamp: Option<i64>) -> MessageUsage {
    MessageUsage {
        model: Some(model.into()),
        billing: super::super::BillingUsage {
            input: tokens.input,
            output: tokens.output,
            cache_read: tokens.cache_read,
            cache_write_5m: tokens.cache_write,
            prompt_size: tokens.input + tokens.cache_read + tokens.cache_write,
            timestamp,
            ..Default::default()
        },
        model_tokens: tokens,
        ..Default::default()
    }
}

fn step(id: &str, source: &str, offset: u64, at: Option<i64>) -> UsageStep {
    UsageStep::claude(
        id.into(),
        position(source, offset),
        message(
            "synthetic-model",
            ModelTokens {
                input: 100,
                output: 10,
                ..Default::default()
            },
            at,
        ),
    )
}

fn compact(source: &str, offset: u64, at: Option<i64>) -> (String, Signal) {
    (
        source.into(),
        Signal {
            k: SignalKind::Compact,
            o: offset,
            t: at,
            at: 0,
            n: None,
            v: None,
        },
    )
}

fn activity(steps: Vec<UsageStep>, signals: Vec<(String, Signal)>) -> SessionActivity {
    derive("session".into(), steps, &Shared::new(), signals, table())
}

fn claude_id(id: &str) -> ActivityStepId {
    ActivityStepId::Claude(id.into())
}

#[test]
fn synthetic_claude_fixture_matches_step_golden() {
    let mut index = super::super::FileIndex::default();
    let mut positions = BTreeMap::new();
    let mut offset = 0;
    for line in include_str!("../../../tests/fixtures/session-activity/claude.jsonl").lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(id) = crate::field(&record["message"], "id") {
            positions.insert(id.to_owned(), offset);
        }
        super::super::claude(&mut index, &record, offset);
        offset += line.len() as u64 + 1;
    }
    // The two content blocks are one usage row; boundary + compact summary
    // are one normalized compaction, not two activity observations.
    assert_eq!(index.usage().len(), 3);
    assert_eq!(index.usage()["first"].record_ids.len(), 2);
    assert_eq!(index.usage()["first"].model_tokens.output, 2);
    let result = activity(
        index
            .usage()
            .iter()
            .map(|(id, usage)| {
                UsageStep::claude(id.clone(), position("source", positions[id]), usage.clone())
            })
            .collect(),
        index
            .signals
            .iter()
            .cloned()
            .map(|signal| ("source".into(), signal))
            .collect(),
    );
    let snapshot = json!({
        "rules_version": result.rules_version,
        "pricing_fingerprint": result.pricing_fingerprint,
        "steps": result.steps.iter().map(|step| json!({
            "id": match &step.id { ActivityStepId::Claude(id) => id, _ => unreachable!() },
            "context_tokens": step.context_tokens,
            "band": format!("{:?}", step.band),
            "elapsed_ms": step.elapsed_ms,
            "compactions": step.compactions
        })).collect::<Vec<_>>(),
        "observed_steps": result.aggregate.totals.observed.steps,
        "exclusive_steps": result.aggregate.totals.exclusive.steps,
        "shared_steps": result.aggregate.totals.shared.steps,
        "compactions": result.aggregate.compactions,
        "thrash": result.aggregate.thrash
    });
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/session-activity/expected.json"
    ))
    .unwrap();
    assert_eq!(snapshot, expected);
}

#[test]
fn exact_context_thresholds_include_both_cache_categories() {
    let steps = [49_999, 50_000, 150_000, 150_001]
        .into_iter()
        .enumerate()
        .map(|(i, context)| {
            UsageStep::claude(
                format!("{i}"),
                position("s", i as u64),
                message(
                    "synthetic-model",
                    ModelTokens {
                        input: context - 30,
                        cache_read: 10,
                        cache_write: 20,
                        output: 999,
                    },
                    Some(i as i64),
                ),
            )
        })
        .collect();
    let result = activity(steps, vec![]);
    assert_eq!(
        result
            .steps
            .iter()
            .map(|step| step.band)
            .collect::<Vec<_>>(),
        [
            ContextBand::Under50k,
            ContextBand::From50kTo150k,
            ContextBand::From50kTo150k,
            ContextBand::Over150k,
        ]
    );
    assert_eq!(
        result.aggregate.by_band[&ContextBand::From50kTo150k]
            .exclusive
            .steps,
        2
    );
}

#[test]
fn repeated_claude_rows_use_existing_merge_order_and_shared_time_is_not_exclusive() {
    let shared = Shared::from([("copied".into(), "exact-shared-key".into())]);
    let result = derive(
        "session".into(),
        [
            step("copied", "s", 0, Some(0)),
            step("next", "s", 10, Some(100)),
            step("next", "s", 20, Some(200)),
        ],
        &shared,
        [],
        table(),
    );
    assert_eq!(result.steps.len(), 2);
    assert_eq!(result.steps[1].position.offset, 20);
    assert_eq!(result.steps[0].elapsed_ms, Some(200));
    assert_eq!(result.aggregate.totals.shared.elapsed_ms.known, 200);
    assert_eq!(
        result.aggregate.totals.exclusive.elapsed_ms,
        Measured {
            known: 0,
            unknown_steps: 1
        }
    );
    assert_eq!(
        result.aggregate.by_band[&ContextBand::Under50k]
            .shared
            .steps,
        1
    );
}

#[test]
fn price_changes_reprice_without_changing_activity_rules() {
    let first = activity(vec![step("id", "s", 0, Some(0))], vec![]);
    let mut changed = serde_json::to_value(table().as_ref()).unwrap();
    changed["fingerprint"] = json!("synthetic-prices-v2");
    changed["providers"]["anthropic"]["synthetic-model"]["price"]["input"] = json!(2.0);
    let second = derive(
        "session".into(),
        [step("id", "s", 0, Some(0))],
        &Shared::new(),
        [],
        Arc::new(serde_json::from_value(changed).unwrap()),
    );
    assert_eq!(first.rules_version, second.rules_version);
    assert_eq!(first.steps[0].id, second.steps[0].id);
    assert_ne!(first.pricing_fingerprint, second.pricing_fingerprint);
    assert!(second.steps[0].list_cost_usd.unwrap() > first.steps[0].list_cost_usd.unwrap());
}

#[test]
fn exact_copy_evidence_stays_shared_and_unrelated_message_ids_stay_exclusive() {
    use crate::claude_usage::{Records, shared};
    use std::collections::BTreeSet;
    let records =
        |uuid: &str| Records::from([("same-message".into(), BTreeSet::from([uuid.into()]))]);
    let first = records("same-record");
    let copied = records("same-record");
    let unrelated = records("different-record");
    let owners = shared([
        ("first", &first),
        ("copied", &copied),
        ("unrelated", &unrelated),
    ]);
    let derive_owner = |owner: &str| {
        derive(
            owner.into(),
            [step("same-message", owner, 0, Some(0))],
            owners.get(owner).unwrap_or(&Shared::new()),
            [],
            table(),
        )
    };
    let a = derive_owner("first");
    let b = derive_owner("copied");
    let c = derive_owner("unrelated");
    assert_eq!(a.steps[0].id, b.steps[0].id);
    assert_ne!(a.session_key, b.session_key);
    assert_eq!(a.steps[0].ownership, b.steps[0].ownership);
    assert!(matches!(a.steps[0].ownership, Ownership::Shared(_)));
    assert_eq!(a.aggregate.totals.exclusive.steps, 0);
    assert_eq!(a.aggregate.totals.shared.steps, 1);
    assert_eq!(a.aggregate.totals.exclusive.list_cost_usd.known, 0.0);
    assert!(!a.aggregate.exclusive_cost_complete);
    assert_eq!(c.steps[0].ownership, Ownership::Exclusive);
    assert!(c.aggregate.exclusive_cost_complete);
}

fn native_payload() -> serde_json::Value {
    json!({"thread_id":"thread", "session_id":"native-session", "turn_id":"turn", "response_id":"response",
        "usage": {"input_tokens":100, "output_tokens":10, "total_tokens":110,
            "cached_input_tokens":20, "cache_write_input_tokens":5, "reasoning_output_tokens":2}})
}

#[test]
fn codex_requires_accepted_complete_tuple_and_never_adds_duplicate_or_conflict() {
    let payload = native_payload();
    let mut conflict = payload.clone();
    conflict["usage"]["output_tokens"] = json!(900);
    let rows = [
        (&payload, "accepted"),
        (&payload, "duplicate"),
        (&conflict, "conflict"),
        (&payload, "unavailable"),
    ]
    .into_iter()
    .enumerate()
    .filter_map(|(i, (payload, outcome))| {
        UsageStep::codex(
            payload,
            outcome,
            position("s", i as u64),
            "synthetic-model".into(),
            Some(i as i64),
        )
    })
    .collect();
    let result = activity(rows, vec![]);
    assert_eq!(result.steps.len(), 1);
    assert_eq!(
        result.steps[0].id,
        ActivityStepId::Codex([
            "thread".into(),
            "native-session".into(),
            "turn".into(),
            "response".into(),
        ])
    );
    assert_eq!(result.steps[0].context_tokens, 105);
    assert_eq!(result.steps[0].tokens.input, 80);
    assert_eq!(result.steps[0].tokens.output, 10);
    let mut other = payload.clone();
    other["thread_id"] = json!("different-thread");
    let rows = [&payload, &other]
        .into_iter()
        .enumerate()
        .filter_map(|(i, payload)| {
            UsageStep::codex(
                payload,
                "accepted",
                position("s", i as u64),
                "synthetic-model".into(),
                None,
            )
        })
        .collect();
    assert_eq!(
        activity(rows, vec![]).steps.len(),
        2,
        "response ID alone is not an identity"
    );
    for key in ["thread_id", "session_id", "turn_id", "response_id"] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(key);
        assert!(
            UsageStep::codex(
                &invalid,
                "accepted",
                position("s", 0),
                "synthetic-model".into(),
                None
            )
            .is_none()
        );
    }
    let mut invalid = payload;
    invalid["usage"]
        .as_object_mut()
        .unwrap()
        .remove("total_tokens");
    assert!(
        UsageStep::codex(
            &invalid,
            "accepted",
            position("s", 0),
            "synthetic-model".into(),
            None
        )
        .is_none()
    );
}

#[test]
fn missing_times_ties_and_one_hour_cap_preserve_unknown_coverage() {
    let result = activity(
        vec![
            step("missing", "a", 0, None),
            step("later", "a", 3, Some(7_200_000)),
            step("tie-b", "b", 0, Some(0)),
            step("tie-a2", "a", 2, Some(0)),
            step("tie-a1", "a", 1, Some(0)),
        ],
        vec![],
    );
    assert_eq!(
        result
            .steps
            .iter()
            .map(|step| step.id.clone())
            .collect::<Vec<_>>(),
        ["tie-a1", "tie-a2", "tie-b", "later", "missing"].map(claude_id)
    );
    assert_eq!(
        result
            .steps
            .iter()
            .map(|step| step.elapsed_ms)
            .collect::<Vec<_>>(),
        [Some(0), Some(0), Some(HOUR_MS), None, None]
    );
    assert_eq!(
        result.aggregate.totals.exclusive.elapsed_ms,
        Measured {
            known: HOUR_MS,
            unknown_steps: 2
        }
    );
    assert_eq!(
        activity(vec![step("one", "s", 0, None)], vec![]).steps[0].elapsed_ms,
        None
    );
    let extreme = activity(
        vec![
            step("a", "s", 0, Some(i64::MIN)),
            step("b", "s", 1, Some(i64::MAX)),
        ],
        vec![],
    );
    assert_eq!(extreme.steps[0].elapsed_ms, Some(HOUR_MS));
}

#[test]
fn prices_and_carry_use_one_snapshot_and_keep_partial_prices_unknown() {
    let tokens = ModelTokens {
        input: 100,
        output: 10,
        cache_read: 200,
        cache_write: 50,
    };
    for (model, known_list, known_carry) in [
        ("synthetic-model", Some(0.0002325), Some(0.00005)),
        ("partial-model", None, Some(0.00005)),
        ("unpriced-model", None, None),
    ] {
        let result = activity(
            vec![UsageStep::claude(
                "id".into(),
                position("s", 0),
                message(model, tokens.clone(), Some(0)),
            )],
            vec![],
        );
        let step = &result.steps[0];
        match (step.list_cost_usd, known_list) {
            (Some(actual), Some(expected)) => assert!((actual - expected).abs() < 1e-12),
            (actual, expected) => assert_eq!(actual, expected),
        }
        assert_eq!(step.carry_cost_usd, known_carry);
        assert_eq!(result.pricing_fingerprint, "synthetic-prices-v1");
        assert_eq!(
            result.aggregate.exclusive_cost_complete,
            known_list.is_some()
        );
        assert_eq!(
            result
                .aggregate
                .totals
                .exclusive
                .list_cost_usd
                .unknown_steps,
            u64::from(known_list.is_none())
        );
    }
    let no_cache = activity(
        vec![UsageStep::claude(
            "id".into(),
            position("s", 0),
            message(
                "unpriced-model",
                ModelTokens {
                    input: 1,
                    ..Default::default()
                },
                None,
            ),
        )],
        vec![],
    );
    assert_eq!(no_cache.steps[0].list_cost_usd, None);
    assert_eq!(no_cache.steps[0].carry_cost_usd, Some(0.0));
}

#[test]
fn compactions_attach_to_following_step_or_last_preceding_and_thrash_in_three() {
    let result = activity(
        (0..4)
            .map(|i| step(&format!("s{i}"), "s", i * 10 + 10, Some(i as i64 * 100)))
            .collect(),
        vec![
            compact("s", 0, Some(-1)),
            compact("s", 11, Some(1)),
            compact("s", 21, Some(101)),
            compact("s", 99, Some(999)),
            compact("s", 99, Some(999)),
        ],
    );
    assert_eq!(result.aggregate.compactions, 4);
    assert_eq!(
        result
            .steps
            .iter()
            .map(|step| step.compactions)
            .collect::<Vec<_>>(),
        [1, 1, 1, 1]
    );
    assert_eq!(
        result
            .compactions
            .iter()
            .map(|compact| compact.step_id.clone())
            .collect::<Vec<_>>(),
        ["s0", "s1", "s2", "s3"].map(|id| Some(claude_id(id)))
    );
    assert!(result.aggregate.thrash);
    assert_eq!(
        result.aggregate.thrash_windows,
        [
            ["s0", "s1", "s2"].map(claude_id),
            ["s1", "s2", "s3"].map(claude_id),
        ]
    );
    let sparse = activity(
        (0..4)
            .map(|i| step(&format!("s{i}"), "s", i * 10, Some(i as i64 * 100)))
            .collect(),
        vec![
            compact("s", 0, Some(0)),
            compact("s", 31, Some(301)),
            compact("s", 32, Some(302)),
        ],
    );
    assert_eq!(sparse.aggregate.compactions, 3);
    assert!(
        !sparse.aggregate.thrash,
        "three across four steps is outside the rolling window"
    );
}

#[test]
fn compactions_without_times_keep_source_order_and_unplaced_evidence() {
    let result = activity(
        vec![step("first", "s", 10, None), step("last", "s", 30, None)],
        vec![
            compact("s", 0, None),
            compact("s", 20, None),
            compact("s", 40, None),
            compact("other-source", 0, None),
        ],
    );
    assert_eq!(result.aggregate.compactions, 4);
    assert!(
        !result.aggregate.thrash,
        "fewer than three steps is not a three-step window"
    );
    assert_eq!(result.steps[0].compactions, 1);
    assert_eq!(result.steps[1].compactions, 2);
    assert_eq!(
        result
            .compactions
            .iter()
            .filter(|compact| compact.step_id.is_none())
            .count(),
        1
    );
    assert!(
        result
            .compactions
            .iter()
            .all(|compact| compact.timestamp.is_none())
    );
    let no_steps = activity(vec![], vec![compact("s", 1, Some(2))]);
    assert_eq!(no_steps.aggregate.compactions, 1);
    assert_eq!(no_steps.compactions[0].step_id, None);
    assert!(!no_steps.aggregate.thrash);
}

#[test]
fn compaction_timestamp_ties_follow_source_offsets() {
    let result = activity(
        vec![step("a", "s", 10, Some(1)), step("b", "s", 30, Some(1))],
        vec![compact("s", 20, Some(1))],
    );
    assert_eq!(result.compactions[0].step_id, Some(claude_id("b")));
}

#[test]
fn codex_normalized_compaction_variants_remain_one_signal() {
    let mut index = super::super::FileIndex::default();
    for (offset, record) in [json!({"type":"compacted"}),
        json!({"type":"event_msg", "payload":{"type":"context_compacted"}}),
        json!({"type":"event_msg", "payload":{"type":"item_completed", "item":{"type":"ContextCompaction"}}}),
    ].iter().enumerate() {
        super::super::codex(&mut index, record, offset as u64);
    }
    let result = activity(
        vec![],
        index
            .signals
            .into_iter()
            .map(|signal| ("s".into(), signal))
            .collect(),
    );
    assert_eq!(result.aggregate.compactions, 1);
}

#[test]
fn step_waste_precedence_keeps_empty_search_and_exact_uncapped_waits() {
    use classify::{LocatedObservation, WasteCategory, WasteKind};
    let mut result = activity(
        vec![
            step("first", "s", 0, Some(0)),
            step("next", "s", 100, Some(7_200_000)),
        ],
        vec![],
    );
    let mut observations = Vec::new();
    for (offset, name, command, output, error) in [
        (10, "Read", "", json!("body"), false),
        (20, "Read", "", json!("body"), false),
        (
            30,
            "Bash",
            "cat absent",
            json!("No such file or directory"),
            true,
        ),
        (40, "Agent", "", json!("done"), false),
    ] {
        let input = if name == "Read" {
            json!({"file_path":"file"})
        } else {
            json!({"command":command})
        };
        let mut observation =
            classify::call(name, &input, Some(claude_id("first")), "call-hash".into());
        classify::finish(
            &mut observation,
            &output,
            error,
            Some(0),
            Some(7_200_000),
            "result-hash".into(),
        );
        observation.empty_search = offset == 30;
        observations.push(LocatedObservation {
            position: position("s", offset),
            timestamp: Some(7_200_000),
            result_offset: Some(offset + 1),
            observation,
        });
    }
    apply_observations(&mut result, observations);
    assert_eq!(result.steps[0].waste, Some(WasteKind::OrchestrationWait));
    assert_eq!(
        result.steps[0].waste_category,
        Some(WasteCategory::Orchestration)
    );
    assert!(result.steps[0].empty_search);
    assert_eq!(result.steps[0].elapsed_ms, Some(3_600_000));
    assert_eq!(result.steps[0].waits[0].completed_at_ms, 7_200_000);
    assert!(result.steps[1].waste.is_none());
}

#[test]
fn failure_kinds_rank_and_signals_count_without_waste() {
    use classify::{LocatedObservation, WasteKind};
    let mut result = activity(
        vec![
            step("first", "s", 0, Some(0)),
            step("second", "s", 100, Some(1)),
            step("third", "s", 200, Some(2)),
        ],
        vec![],
    );
    let mut observations = Vec::new();
    // (step, offset, waste, verify_failure, silent_exit)
    for (id, offset, waste, verify, silent) in [
        ("first", 10, Some(WasteKind::MergeConflict), false, false),
        ("first", 20, Some(WasteKind::ChecksPending), false, false),
        ("first", 30, Some(WasteKind::Timeout), false, false),
        ("second", 40, None, true, false),
        ("second", 50, None, true, true),
        (
            "third",
            60,
            Some(WasteKind::ToolInputRejected),
            false,
            false,
        ),
    ] {
        let mut observation = classify::call(
            "Agent",
            &serde_json::Value::Null,
            Some(claude_id(id)),
            "call-hash".into(),
        );
        observation.waste = waste;
        observation.verify_failure = verify;
        observation.silent_exit = silent;
        observations.push(LocatedObservation {
            position: position("s", offset),
            timestamp: None,
            result_offset: Some(offset + 1),
            observation,
        });
    }
    apply_observations(&mut result, observations);
    // Checks-pending ranks with orchestration waits, ahead of other failures.
    assert_eq!(result.steps[0].waste, Some(WasteKind::ChecksPending));
    assert_eq!(result.steps[1].waste, None);
    assert!(result.steps[1].verify_failure && result.steps[1].silent_exit);
    assert_eq!(result.steps[2].waste, Some(WasteKind::ToolInputRejected));
    let counts = analytics::Analytics::of(&result).counts;
    assert_eq!(counts.verify_failure_steps, 1);
    assert_eq!(counts.silent_exit_steps, 1);
    assert_eq!(counts.waste_by_kind[&WasteKind::ChecksPending], 1);
    assert!(!counts.waste_by_kind.contains_key(&WasteKind::Timeout));
}

#[test]
fn analytics_unions_exact_wait_intervals_and_preserves_missing_coverage() {
    use classify::{WaitInterval, WasteCategory, WasteKind};

    let mut result = activity(
        vec![
            step("exclusive", "s", 0, Some(0)),
            step("shared", "s", 10, Some(1_000)),
            step("missing", "s", 20, Some(2_000)),
        ],
        vec![],
    );
    result.steps[0].waste = Some(WasteKind::OrchestrationWait);
    result.steps[0].waste_category = Some(WasteCategory::Orchestration);
    result.steps[0].waits = [(0, 100), (100, 200), (250, 300), (20, 50), (0, 100)]
        .into_iter()
        .map(|(started_at_ms, completed_at_ms)| WaitInterval {
            started_at_ms,
            completed_at_ms,
        })
        .collect();
    result.steps[1].ownership = Ownership::Shared("shared-observation".into());
    result.steps[1].waste = Some(WasteKind::OrchestrationWait);
    result.steps[1].waste_category = Some(WasteCategory::Orchestration);
    result.steps[1].waits = vec![WaitInterval {
        started_at_ms: 200,
        completed_at_ms: 250,
    }];
    result.steps[2].waste = Some(WasteKind::OrchestrationWait);
    result.steps[2].waste_category = Some(WasteCategory::Orchestration);

    let counts = analytics::Analytics::of(&result).counts;
    assert_eq!(counts.waste_by_kind[&WasteKind::OrchestrationWait], 3);
    assert_eq!(counts.waste_by_category[&WasteCategory::Orchestration], 3);
    assert_eq!(counts.wait_time_ms.observed.known, 300);
    assert_eq!(counts.wait_time_ms.exclusive.known, 250);
    assert_eq!(counts.wait_time_ms.shared.known, 50);
    assert_eq!(counts.wait_time_ms.observed.unknown_steps, 1);
    assert_eq!(counts.wait_time_ms.exclusive.unknown_steps, 1);
    assert_eq!(counts.wait_time_ms.shared.unknown_steps, 0);
    assert_eq!(counts.waits_without_interval.observed, 1);
    assert_eq!(counts.waits_without_interval.exclusive, 1);
    assert_eq!(counts.waits_without_interval.shared, 0);
}

#[test]
fn interval_union_merges_overlap_containment_adjacency_and_duplicates() {
    let intervals = vec![(0, 100), (20, 50), (0, 100), (100, 150), (160, 180)];
    assert_eq!(analytics::interval_union_ms(&intervals), 170);
}

#[test]
fn waste_cost_keeps_unpriced_steps_unknown() {
    use classify::{WasteCategory, WasteKind};

    let mut result = derive(
        "session".into(),
        [
            step("priced", "s", 0, Some(0)),
            UsageStep::claude(
                "unpriced".into(),
                position("s", 10),
                message(
                    "unpriced-model",
                    ModelTokens {
                        input: 100,
                        output: 10,
                        ..Default::default()
                    },
                    Some(1_000),
                ),
            ),
        ],
        &Shared::new(),
        [],
        table(),
    );
    for step in &mut result.steps {
        step.waste = Some(WasteKind::ExternalFailure);
        step.waste_category = Some(WasteCategory::External);
    }
    let priced_cost = result.steps[0].list_cost_usd.unwrap();
    let counts = analytics::Analytics::of(&result).counts;
    let by_kind = &counts.waste_cost_by_kind[&WasteKind::ExternalFailure];
    let by_category = &counts.waste_cost_by_category[&WasteCategory::External];
    for totals in [by_kind, by_category] {
        assert_eq!(totals.steps, 2);
        assert_eq!(totals.list_cost_usd.known, priced_cost);
        assert_eq!(totals.list_cost_usd.unknown_steps, 1);
        assert_eq!(totals.elapsed_ms.known, 1_000);
        assert_eq!(totals.elapsed_ms.unknown_steps, 1);
    }
    let shared = Shared::from([("unpriced".into(), "shared-unpriced".into())]);
    let mut result = derive(
        "shared-session".into(),
        [UsageStep::claude(
            "unpriced".into(),
            position("s", 0),
            message(
                "unpriced-model",
                ModelTokens {
                    input: 100,
                    output: 10,
                    ..Default::default()
                },
                Some(0),
            ),
        )],
        &shared,
        [],
        table(),
    );
    result.steps[0].waste = Some(WasteKind::ExternalFailure);
    let counts = analytics::Analytics::of(&result).counts;
    assert!(counts.waste_cost_by_kind.is_empty());
    assert_eq!(counts.waste_by_kind[&WasteKind::ExternalFailure], 1);
}

#[test]
fn family_fold_sums_session_wait_unions_and_waste_costs() {
    use classify::{WaitInterval, WasteCategory, WasteKind};

    let mut parent = activity(vec![step("parent", "p", 0, Some(0))], vec![]);
    parent.steps[0].waste = Some(WasteKind::OrchestrationWait);
    parent.steps[0].waste_category = Some(WasteCategory::Orchestration);
    parent.steps[0].waits = vec![WaitInterval {
        started_at_ms: 0,
        completed_at_ms: 20,
    }];
    parent.steps[0].elapsed_ms = Some(10);
    parent.steps[0].list_cost_usd = Some(0.2);
    let mut child = activity(vec![step("child", "c", 0, Some(0))], vec![]);
    child.steps[0].waste = Some(WasteKind::OrchestrationWait);
    child.steps[0].waste_category = Some(WasteCategory::Orchestration);
    child.steps[0].waits = vec![WaitInterval {
        started_at_ms: 0,
        completed_at_ms: 30,
    }];
    child.steps[0].elapsed_ms = Some(15);
    child.steps[0].list_cost_usd = Some(0.3);

    let family = analytics::families(
        [
            ("parent", None, Some(&parent)),
            ("child", Some("parent"), Some(&child)),
        ]
        .into_iter(),
    )
    .remove("parent")
    .unwrap();
    assert_eq!(family.counts.wait_time_ms.observed.known, 50);
    let waste_totals = &family.counts.waste_cost_by_kind[&WasteKind::OrchestrationWait];
    assert_eq!(waste_totals.steps, 2);
    assert_eq!(waste_totals.elapsed_ms.known, 25);
    assert_eq!(waste_totals.list_cost_usd.known, 0.5);
    assert_eq!(
        family.levers_not_computed,
        vec![analytics::LeverNotComputed::TaskSpecification]
    );
    assert_eq!(
        family.counts.waste_cost_by_category[&WasteCategory::Orchestration].steps,
        2
    );
}

#[test]
fn fact_links_require_a_qualified_step_in_this_projection() {
    use super::classify::{Change, LocatedObservation};
    for exact in [false, true] {
        let mut activity = derive(
            "synthetic".into(),
            [],
            &Default::default(),
            [],
            crate::pricing::snapshot(),
        );
        let id = ActivityStepId::Claude("message".into());
        if exact {
            let used = MessageUsage {
                model: Some("claude-sonnet-4-6".into()),
                ..Default::default()
            };
            activity = derive(
                "synthetic".into(),
                [UsageStep::claude(
                    "message".into(),
                    SourcePosition {
                        source: "synthetic".into(),
                        offset: 10,
                    },
                    used,
                )],
                &Default::default(),
                [],
                crate::pricing::snapshot(),
            );
        }
        let mut observation = classify::call(
            "Bash",
            &serde_json::json!({"command":"git commit -m synthetic"}),
            Some(id.clone()),
            "hash".into(),
        );
        classify::finish(
            &mut observation,
            &serde_json::json!("[topic abc1234] synthetic"),
            false,
            None,
            None,
            "result".into(),
        );
        apply_observations(
            &mut activity,
            vec![LocatedObservation {
                position: SourcePosition {
                    source: "synthetic".into(),
                    offset: 11,
                },
                timestamp: None,
                result_offset: Some(12),
                observation,
            }],
        );
        assert_eq!(activity.facts.len(), 1);
        assert!(matches!(activity.facts[0].change, Change::Commit { .. }));
        assert_eq!(activity.facts[0].step_id, exact.then_some(id));
    }
}
