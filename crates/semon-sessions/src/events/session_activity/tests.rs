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
            native_record: None,
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
fn codex_call_provenance_requires_exact_identity() {
    let payload = native_payload();
    let step = UsageStep::codex(
        &payload,
        "accepted",
        position("s", 100),
        "synthetic-model".into(),
        None,
    )
    .unwrap();
    let identity = step.id.clone();
    let mut result = activity(vec![step], vec![]);
    let observation = |offset, id| classify::LocatedObservation {
        position: position("s", offset),
        timestamp: None,
        result_timestamp: None,
        result_offset: None,
        tool: None,
        observation: classify::call(
            "exec_command",
            &json!({"cmd":"true"}),
            id,
            format!("call-{offset}"),
        ),
    };
    apply_observations(
        &mut result,
        vec![
            observation(5, None),
            observation(20, Some(identity.clone())),
            observation(10, Some(identity)),
        ],
    );
    assert_eq!(result.steps[0].call_position, Some(position("s", 10)));
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

fn assert_usd(actual: Option<f64>, expected: Option<f64>) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => assert!((actual - expected).abs() < 1e-12),
        (actual, expected) => assert_eq!(actual, expected),
    }
}

#[test]
fn components_price_independently_and_list_cost_needs_all_of_them() {
    let tokens = ModelTokens {
        input: 100,
        output: 10,
        cache_read: 200,
        cache_write: 50,
    };
    // Components: fresh input, cache read, cache write, output.
    for (model, known_list, known) in [
        (
            "synthetic-model",
            Some(0.0002325),
            [Some(1e-4), Some(5e-5), Some(6.25e-5), Some(2e-5)],
        ),
        ("partial-model", None, [Some(1e-4), Some(5e-5), None, None]),
        ("unpriced-model", None, [None; 4]),
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
        assert_usd(step.list_cost_usd, known_list);
        assert_usd(step.fresh_input_usd, known[0]);
        assert_usd(step.cache_read_usd, known[1]);
        assert_usd(step.cache_write_usd, known[2]);
        assert_usd(step.output_usd, known[3]);
        if known.iter().all(Option::is_some) {
            let sum: f64 = known.iter().flatten().sum();
            assert!((step.list_cost_usd.unwrap() - sum).abs() < 1e-12);
        }
        assert_eq!(result.pricing_fingerprint, "synthetic-prices-v1");
        assert_eq!(
            result.aggregate.exclusive_cost_complete,
            known_list.is_some()
        );
        let exclusive = &result.aggregate.totals.exclusive;
        let components = analytics::Analytics::of(&result).counts.component_cost_usd;
        assert_eq!(
            exclusive.list_cost_usd.unknown_steps,
            u64::from(known_list.is_none())
        );
        // Each component total is its own measurement, known or unknown.
        for (total, expected) in [
            (&components.fresh_input_usd, known[0]),
            (&components.cache_read_usd, known[1]),
            (&components.cache_write_usd, known[2]),
            (&components.output_usd, known[3]),
        ] {
            assert_usd(Some(total.known), Some(expected.unwrap_or(0.0)));
            assert_eq!(total.unknown_steps, u64::from(expected.is_none()));
        }
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
    let step = &no_cache.steps[0];
    assert_eq!(step.list_cost_usd, None);
    assert_eq!(step.fresh_input_usd, None);
    // No tokens is a known zero, not an unpriced component.
    assert_eq!(step.cache_read_usd, Some(0.0));
    assert_eq!(step.cache_write_usd, Some(0.0));
    assert_eq!(step.output_usd, Some(0.0));
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
            result_timestamp: Some(7_200_000),
            result_offset: Some(offset + 1),
            tool: None,
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
            result_timestamp: None,
            result_offset: Some(offset + 1),
            tool: None,
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
    for member in [&mut parent, &mut child] {
        member.steps[0].activity = Some(classify::Activity::Wait);
        member.steps[0].trigger = Some(Trigger::Polling);
    }

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
    let waiting = &family.counts.by_activity[&classify::Activity::Wait];
    assert_eq!(waiting.steps, 2);
    assert_eq!(waiting.elapsed_ms.known, 25);
    assert_eq!(family.counts.by_trigger[&Trigger::Polling].steps, 2);
    let classifier = &family.counts.coverage.classifier;
    assert_eq!(classifier.unlabelled_activity_steps, 0);
    assert_eq!(
        classifier.activities_not_computed,
        [classify::Activity::Recover]
    );
    let timing = &family.counts.coverage.timing;
    assert_eq!(timing.known_steps + timing.unknown_steps, 2);
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
                result_timestamp: None,
                result_offset: Some(12),
                tool: None,
                observation,
            }],
        );
        assert_eq!(activity.facts.len(), 1);
        assert!(matches!(activity.facts[0].change, Change::Commit { .. }));
        assert_eq!(activity.facts[0].step_id, exact.then_some(id));
    }
}

fn observe(
    id: &str,
    offset: u64,
    name: &str,
    command: &str,
    output: serde_json::Value,
    error: bool,
) -> classify::LocatedObservation {
    let mut observation = classify::call(
        name,
        &json!({"command":command, "file_path":"file"}),
        Some(claude_id(id)),
        "call-hash".into(),
    );
    classify::finish(
        &mut observation,
        &output,
        error,
        None,
        None,
        "result".into(),
    );
    classify::LocatedObservation {
        position: position("s", offset),
        timestamp: None,
        result_timestamp: None,
        result_offset: Some(offset + 1),
        tool: None,
        observation,
    }
}

#[test]
fn step_activity_trigger_retry_and_coverage_flow_through_analytics() {
    use classify::{Activity, WasteCategory, WasteKind};
    let mut result = activity(
        vec![
            step("s1", "s", 0, Some(0)),
            step("s2", "s", 100, Some(1_000)),
            step("s3", "s", 200, Some(2_000)),
            step("s4", "s", 300, Some(3_000)),
        ],
        vec![],
    );
    apply_observations(
        &mut result,
        vec![
            observe("s1", 10, "Bash", "curl synthetic", json!("boom"), true),
            observe("s2", 110, "Bash", "curl synthetic", json!("fine"), false),
            observe("s3", 210, "Read", "", json!("body"), false),
            observe(
                "s3",
                220,
                "Bash",
                "git commit -m synthetic",
                json!("[main abc1234] synthetic"),
                false,
            ),
        ],
    );
    let activities: Vec<_> = result.steps.iter().map(|step| step.activity).collect();
    // Unknown commands prove nothing; a step takes the first match among its
    // calls; a step with no tool call has no label.
    assert_eq!(activities, [None, None, Some(Activity::Integrate), None]);
    assert_eq!(result.steps[0].waste, Some(WasteKind::OtherFailure));
    assert_eq!(result.steps[1].waste, Some(WasteKind::Retry));
    assert_eq!(
        result.steps[1].waste_category,
        Some(WasteCategory::Unmapped)
    );
    let triggers: Vec<_> = result.steps.iter().map(|step| step.trigger).collect();
    assert_eq!(
        triggers,
        [
            Some(Trigger::NoneVisible),
            Some(Trigger::ToolError),
            None,
            None
        ]
    );
    let analytics = analytics::Analytics::of(&result);
    let counts = &analytics.counts;
    assert_eq!(counts.by_activity.len(), 1);
    assert_eq!(counts.by_activity[&Activity::Integrate].steps, 1);
    assert_eq!(
        counts.by_activity[&Activity::Integrate].elapsed_ms.known,
        1_000
    );
    assert_eq!(counts.coverage.classifier.unlabelled_activity_steps, 3);
    assert_eq!(
        counts
            .by_activity
            .values()
            .map(|totals| totals.steps)
            .sum::<u64>()
            + counts.coverage.classifier.unlabelled_activity_steps,
        counts.totals.exclusive.steps
    );
    assert_eq!(counts.by_trigger[&Trigger::ToolError].steps, 1);
    assert_eq!(counts.by_trigger[&Trigger::NoneVisible].steps, 1);
    assert_eq!(counts.waste_by_kind[&WasteKind::Retry], 1);
    let classifier = &counts.coverage.classifier;
    assert_eq!(classifier.rules_version, ACTIVITY_RULES_VERSION);
    assert_eq!(classifier.activities_not_computed, [Activity::Recover]);
    assert_eq!(
        classifier.triggers_not_computed,
        [
            analytics::TriggerNotComputed::InstructionChange,
            analytics::TriggerNotComputed::MissingInput
        ]
    );
    assert_eq!(
        classifier.levers_not_computed,
        [analytics::LeverNotComputed::TaskSpecification]
    );
}

#[test]
fn trigger_follows_refill_then_the_nearest_preceding_step_in_source() {
    use classify::{Activity, WasteKind};
    let fresh = || {
        activity(
            (0..3)
                .map(|n| step(&format!("s{n}"), "s", n * 100, Some(n as i64)))
                .collect(),
            vec![],
        )
        .steps
    };
    type Setup = fn(&mut ActivityStep);
    let cases: [(Setup, Trigger); 17] = [
        (
            |p| p.waste = Some(WasteKind::PathNotFound),
            Trigger::ToolError,
        ),
        (|p| p.waste = Some(WasteKind::Timeout), Trigger::ToolError),
        (|p| p.empty_search = true, Trigger::EmptyResult),
        (|p| p.verify_failure = true, Trigger::TestOrCiFailure),
        (
            |p| p.waste = Some(WasteKind::ExternalFailure),
            Trigger::ExternalError,
        ),
        (|p| p.waste = Some(WasteKind::Sleep), Trigger::Polling),
        (
            |p| p.waste = Some(WasteKind::ChecksPending),
            Trigger::Polling,
        ),
        (|p| p.activity = Some(Activity::Wait), Trigger::Polling),
        (
            |p| p.activity = Some(Activity::Coordinate),
            Trigger::WorkerResult,
        ),
        (
            |p| p.waste = Some(WasteKind::OrchestrationWait),
            Trigger::WorkerResult,
        ),
        (
            |p| {
                p.waste = Some(WasteKind::OrchestrationWait);
                p.activity = Some(Activity::Wait);
            },
            Trigger::Polling,
        ),
        (|_| {}, Trigger::NoneVisible),
        // Repeat markers are not failures of the preceding call.
        (|p| p.waste = Some(WasteKind::Retry), Trigger::NoneVisible),
        (
            |p| {
                p.waste = Some(WasteKind::PathNotFound);
                p.verify_failure = true;
            },
            Trigger::ToolError,
        ),
        (
            |p| {
                p.empty_search = true;
                p.verify_failure = true;
            },
            Trigger::EmptyResult,
        ),
        (
            |p| {
                p.verify_failure = true;
                p.waste = Some(WasteKind::ExternalFailure);
            },
            Trigger::TestOrCiFailure,
        ),
        (
            |p| {
                p.waste = Some(WasteKind::ExternalFailure);
                p.activity = Some(Activity::Wait);
            },
            Trigger::ExternalError,
        ),
    ];
    for (index, (setup, expected)) in cases.into_iter().enumerate() {
        let mut steps = fresh();
        setup(&mut steps[0]);
        steps[1].waste = Some(WasteKind::Timeout);
        assert_eq!(Trigger::of(&steps, 1), Some(expected), "case {index}");
    }
    let mut steps = fresh();
    steps[0].waste = Some(WasteKind::PathNotFound);
    // Refill follows compaction whatever precedes it; no waste or verify
    // failure means no trigger; the first step has nothing visible before it.
    steps[1].waste = Some(WasteKind::Refill);
    assert_eq!(Trigger::of(&steps, 1), Some(Trigger::Compaction));
    assert_eq!(Trigger::of(&steps, 2), None);
    steps[2].verify_failure = true;
    assert_eq!(Trigger::of(&steps, 2), Some(Trigger::NoneVisible));
    steps[0].verify_failure = true;
    assert_eq!(Trigger::of(&steps, 0), Some(Trigger::NoneVisible));
    // Only the same native source supplies evidence.
    let mut steps = fresh();
    steps[0].waste = Some(WasteKind::PathNotFound);
    steps[1].position.source = "other".into();
    steps[1].waste = Some(WasteKind::Timeout);
    steps[2].waste = Some(WasteKind::Timeout);
    assert_eq!(Trigger::of(&steps, 1), Some(Trigger::NoneVisible));
    assert_eq!(Trigger::of(&steps, 2), Some(Trigger::ToolError));
}

fn priced_steps(count: usize) -> Vec<UsageStep> {
    (0..count)
        .map(|i| step(&format!("s{i:04}"), "s", i as u64, Some(i as i64 * 10)))
        .collect()
}

fn model_step(
    id: &str,
    offset: u64,
    model: &str,
    tokens: ModelTokens,
    at: Option<i64>,
) -> UsageStep {
    UsageStep::claude(
        id.into(),
        position("source", offset),
        message(model, tokens, at),
    )
}

#[test]
fn preview_bins_equal_step_counts_and_never_exceeds_64_bins() {
    assert_eq!(analytics::Preview::of(&[]), analytics::Preview::default());
    // (steps, steps per bin, bins): ceil(n / 64) steps per bin.
    for (count, per_bin, bins) in [
        (1, 1, 1),
        (63, 1, 63),
        (64, 1, 64),
        (65, 2, 33),
        (1000, 16, 63),
    ] {
        let preview = analytics::Preview::of(&activity(priced_steps(count), vec![]).steps);
        assert_eq!(preview.steps_total, count as u64);
        assert_eq!(preview.binned_steps, count as u64);
        assert_eq!(preview.bins.len(), bins, "{count} steps");
        for (index, bin) in preview.bins.iter().enumerate() {
            let first = index * per_bin;
            let last = ((index + 1) * per_bin).min(count) - 1;
            let span = (bin.first_step, bin.last_step);
            assert_eq!(span, (first as u64, last as u64));
            assert_eq!(bin.list_cost_usd.unknown_steps, 0);
            assert_eq!(bin.max_context_tokens, 100);
        }
    }
}

#[test]
fn preview_bins_summarize_context_cost_waste_and_compactions_without_shared_steps() {
    use classify::{WasteCategory, WasteKind};
    let mut usage = Vec::new();
    for i in 0..70u64 {
        let model = if i == 3 {
            "unpriced-model"
        } else {
            "synthetic-model"
        };
        let tokens = ModelTokens {
            input: i * 100,
            output: 1,
            ..Default::default()
        };
        usage.push(model_step(
            &format!("s{i:02}"),
            i,
            model,
            tokens,
            Some(i as i64 * 10),
        ));
    }
    let shared = Shared::from([("s02".into(), "shared-key".into())]);
    let mut result = derive("session".into(), usage, &shared, vec![], table());
    result.steps[0].waste = Some(WasteKind::Retry);
    result.steps[1].waste = Some(WasteKind::Timeout);
    result.steps[1].compactions = 2;
    result.steps[2].compactions = 5;
    let preview = analytics::Preview::of(&result.steps);
    assert_eq!((preview.steps_total, preview.binned_steps), (70, 69));
    // 69 exclusive steps, two per bin; the shared step (ordinal 2) is skipped.
    assert_eq!(preview.bins.len(), 35);
    let first = &preview.bins[0];
    assert_eq!((first.first_step, first.last_step), (0, 1));
    assert_eq!(first.max_context_tokens, 100);
    assert_eq!(first.last_context_tokens, 100);
    assert_eq!(first.waste_by_category.len(), 2);
    assert_eq!(first.waste_by_category[&WasteCategory::Unmapped], 1);
    assert_eq!(first.waste_by_category[&WasteCategory::Tooling], 1);
    assert_eq!(first.compactions, 2);
    let second = &preview.bins[1];
    assert_eq!((second.first_step, second.last_step), (3, 4));
    assert_eq!(second.max_context_tokens, 400);
    assert_eq!(second.last_context_tokens, 400);
    // The unpriced step 3 stays unknown beside the priced step 4.
    assert_eq!(second.list_cost_usd.unknown_steps, 1);
    assert!((second.list_cost_usd.known - 0.000402).abs() < 1e-12);
    assert!(second.waste_by_category.is_empty());
    assert_eq!(second.compactions, 0);
}

#[test]
fn coverage_counts_exclusive_pricing_timing_and_classifier_gaps() {
    let with_input = |input| ModelTokens {
        input,
        output: 10,
        ..Default::default()
    };
    let zero = ModelTokens::default();
    let shared = Shared::from([("d-shared".into(), "shared-key".into())]);
    let result = derive(
        "session".into(),
        [
            model_step("a-full", 0, "synthetic-model", with_input(100), Some(0)),
            // The output rate is missing: input priced, output not.
            model_step("b-partial", 1, "partial-model", with_input(100), Some(10)),
            model_step("c-unpriced", 2, "unpriced-model", with_input(100), None),
            // Shared steps are outside exclusive coverage.
            model_step("d-shared", 3, "unpriced-model", with_input(100), None),
            // No tokens: no component is missing a price.
            model_step("e-empty", 4, "unpriced-model", zero, None),
        ],
        &shared,
        [],
        table(),
    );
    let coverage = analytics::Analytics::of(&result).counts.coverage;
    assert_eq!(coverage.pricing.full_steps, 2);
    assert_eq!(coverage.pricing.partial_steps, 1);
    assert_eq!(coverage.pricing.unpriced_steps, 1);
    // Only the first step has a successor timestamp.
    assert_eq!(coverage.timing.known_steps, 1);
    assert_eq!(coverage.timing.unknown_steps, 3);
    assert_eq!(coverage.classifier.unlabelled_activity_steps, 4);
    assert_eq!(coverage.classifier.rules_version, ACTIVITY_RULES_VERSION);
}
