"""Synthetic-only tests for the private real-session metrics contract."""

import json
import unittest

from real_metrics import (
    EXPECTED_REPETITIONS,
    MetricsError,
    _protocol_conformance,
    condition_name,
    score_records,
)


def _gold(activity, *, failure="none", retry="none", anomaly=False, phase="active"):
    return {
        "activity": activity,
        "failure_category": failure,
        "retry_loop": retry,
        "anomaly": anomaly,
        "phase": phase,
    }


def _reference(event_id, session_id, order, gold, *, harness="claude", complete=True, eligible_count=2):
    return {
        "id": event_id,
        "session_id": session_id,
        "harness": harness,
        "event_order": order,
        "complete_session": complete,
        "eligible_event_count": eligible_count if complete else None,
        "gold": gold,
    }


def _prediction(event_id, repetition, values, *, strategy="rules", mode="per-event", context="prefix"):
    return {
        "id": event_id,
        "repetition": repetition,
        "strategy": strategy,
        "mode": mode,
        "context": context,
        "predictions": values,
    }


def _operations_row(*, event_count, prediction_rows, request_count, failed_request_count,
                    http_count=None, http_unknown=0, http_p95=None,
                    projected_count=None, projected_unknown=0, projected_p95=None,
                    wait_count=None, wait_unknown=0):
    http_count = request_count - http_unknown if http_count is None else http_count
    projected_count = prediction_rows - projected_unknown if projected_count is None else projected_count
    wait_count = prediction_rows - wait_unknown if wait_count is None else wait_count
    return {
        "status": "measured",
        "event_count": event_count,
        "prediction_rows": prediction_rows,
        "request_count": request_count,
        "failed_request_count": failed_request_count,
        "request_rtt_ms": {
            "count": http_count, "p50_ms": http_p95, "p95_ms": http_p95,
            "unknown_count": http_unknown,
        },
        "latency_ms": {
            "projected_online_latency_ms": {
                "count": projected_count, "p50_ms": projected_p95,
                "p95_ms": projected_p95, "unknown_rows": projected_unknown,
            },
            "logical_queue_wait_ms": {
                "count": wait_count, "p50_ms": 5.0, "p95_ms": 10.0,
                "unknown_rows": wait_unknown,
            },
        },
    }


class RealMetricsTests(unittest.TestCase):
    def test_condition_names_cover_all_protocol_modes(self):
        self.assertEqual(condition_name("rules", "per-event"), "Rules")
        self.assertEqual(condition_name("jev", "per-event"), "Jev per-event")
        self.assertEqual(condition_name("jev", "microbatch"), "Jev microbatch")
        self.assertEqual(condition_name("jev", "event-triggered"), "Rules+triggeredJev")
        self.assertEqual(condition_name("llm", "per-event"), "Haiku per-event")
        self.assertEqual(condition_name("llm", "event-triggered"), "Rules+triggeredHaiku")

    def test_repetitions_are_averaged_inside_unique_event_denominator(self):
        refs = [
            _reference("event-a", "session-a", 1, _gold("task_request", phase="start")),
            _reference("event-b", "session-a", 2, _gold("progress", phase="active")),
        ]
        predictions = []
        for repetition, event_a_prediction in enumerate(("task_request", "task_request", "progress"), 1):
            predictions.append(_prediction("event-a", repetition, {
                "activity": event_a_prediction, "failure_category": "none", "retry_loop": "none",
                "anomaly": False, "phase": "start",
            }))
            predictions.append(_prediction("event-b", repetition, {
                "activity": "progress", "failure_category": "none", "retry_loop": "none",
                "anomaly": False, "phase": "active",
            }))
        result = score_records(refs, predictions, draws=40)
        score = result["conditions"]["Rules"]["prefix"]["metrics"]
        self.assertEqual(score["event_count"], 2)
        self.assertEqual(score["prediction_rows"], 6)
        self.assertAlmostEqual(score["dimensions"]["activity"]["accuracy"]["estimate"], 5 / 6)
        self.assertEqual(score["dimensions"]["activity"]["per_class"]["task_request"]["positive_events"], 1)
        self.assertEqual(result["protocol_conformance"]["status"], "insufficient_evidence")

    def test_protocol_conformance_requires_frozen_draws_seed_and_repeat_sets(self):
        predictions = {
            key: {"event": {repetition: {} for repetition in repetitions}}
            for key, repetitions in EXPECTED_REPETITIONS.items()
        }
        conforming = _protocol_conformance(predictions, 2000, 20261030)
        self.assertEqual(conforming["status"], "pass")
        self.assertTrue(conforming["bootstrap_draws_match"])
        self.assertTrue(conforming["bootstrap_seed_match"])

        predictions[("Jev per-event", "prefix")] = {"event": {1: {}, 2: {}}}
        nonconforming = _protocol_conformance(predictions, 2000, 20261030)
        self.assertEqual(nonconforming["status"], "insufficient_evidence")
        self.assertFalse(nonconforming["repetition_sets"]["Jev per-event|prefix"]["matches"])
        self.assertEqual(_protocol_conformance(predictions, 20, 123)["status"], "insufficient_evidence")

    def test_harness_points_are_descriptive_and_average_repetitions_per_event(self):
        refs = [
            _reference("claude-a", "claude-session", 1, _gold("task_request")),
            _reference("claude-b", "claude-session", 2, _gold("progress")),
            _reference("codex-a", "codex-session", 1, _gold("task_request"), harness="codex"),
            _reference("codex-b", "codex-session", 2, _gold("progress"), harness="codex"),
        ]
        for row in refs:
            row["causal_gold"] = row["gold"]
        predictions = []
        for repetition in (1, 2):
            predictions.extend([
                _prediction("claude-a", repetition, _gold("task_request" if repetition == 1 else "progress")),
                _prediction("claude-b", repetition, _gold("progress")),
                _prediction("codex-a", repetition, _gold("progress")),
                _prediction("codex-b", repetition, _gold("wait")),
            ])
        result = score_records(refs, predictions, draws=20)
        score = result["conditions"]["Rules"]["prefix"]
        strata = score["harness_descriptive_points"]
        self.assertEqual(strata["estimate_type"], "descriptive_point_estimate")
        self.assertEqual(strata["confidence_intervals"], "not_computed")
        self.assertEqual(strata["bootstrap_scope"], "pooled_condition_context_metrics_only")
        self.assertTrue(strata["harness_role_confounded"])
        self.assertEqual(strata["claude"]["unique_event_count"], 2)
        self.assertEqual(strata["claude"]["session_count"], 1)
        self.assertEqual(strata["codex"]["unique_event_count"], 2)
        self.assertEqual(strata["codex"]["session_count"], 1)
        self.assertEqual(strata["claude"]["dimensions"]["activity"]["accuracy"], 0.75)
        self.assertEqual(strata["claude"]["dimensions"]["activity"]["per_class"]["task_request"]["positive_events"], 1)
        self.assertEqual(
            strata["claude"]["dimensions"]["activity"]["per_class"]["task_request"]["recall"],
            0.5,
        )
        self.assertEqual(strata["codex"]["dimensions"]["activity"]["accuracy"], 0.0)
        self.assertEqual(
            strata["codex"]["dimensions"]["activity"]["per_class"]["progress"]["precision"],
            0.0,
        )
        self.assertNotIn("event_bootstrap_ci95", strata["claude"]["dimensions"]["activity"])
        self.assertIsNotNone(score["causal_reference_harness_descriptive_points"])

    def test_session_alert_budget_uses_complete_sessions_and_session_bootstrap(self):
        refs = [
            _reference("a1", "s1", 1, _gold("progress", anomaly=False)),
            _reference("a2", "s1", 2, _gold("progress", anomaly=False)),
            _reference("b1", "s2", 1, _gold("progress", anomaly=False)),
            _reference("b2", "s2", 2, _gold("progress", anomaly=False)),
        ]
        predictions = []
        for repetition in (1, 2, 3):
            for event_id in ("a1", "a2", "b1", "b2"):
                predictions.append(_prediction(event_id, repetition, {
                    "activity": "progress", "failure_category": "none", "retry_loop": "none",
                    "anomaly": event_id in {"b1", "b2"}, "phase": "active",
                }))
        result = score_records(refs, predictions, draws=40)
        budget = result["conditions"]["Rules"]["prefix"]["false_alert_budget"]
        self.assertEqual(budget["status"], "measured")
        self.assertEqual(budget["session_count"], 2)
        self.assertEqual(budget["mean_false_positive_alerts_per_session"]["estimate"], 1.0)
        self.assertEqual(budget["fraction_sessions_over_one_false_positive_alert"]["estimate"], 0.5)

    def test_sessions_over_one_uses_repetition_averaged_session_count(self):
        refs = [
            _reference("a1", "s1", 1, _gold("progress", anomaly=False)),
            _reference("a2", "s1", 2, _gold("progress", anomaly=False)),
        ]
        predictions = []
        for repetition in (1, 2, 3):
            for event_id in ("a1", "a2"):
                predictions.append(_prediction(event_id, repetition, {
                    "activity": "progress", "failure_category": "none", "retry_loop": "none",
                    "anomaly": repetition == 1, "phase": "active",
                }))
        result = score_records(refs, predictions, draws=20)
        budget = result["conditions"]["Rules"]["prefix"]["false_alert_budget"]
        self.assertEqual(budget["mean_false_positive_alerts_per_session"]["estimate"], 2 / 3)
        self.assertEqual(budget["fraction_sessions_over_one_false_positive_alert"]["estimate"], 0.0)

    def test_binary_failure_counts_every_non_none_category_as_failure(self):
        refs = [
            _reference("e1", "s1", 1, _gold("failure", failure="unknown")),
            _reference("e2", "s1", 2, _gold("progress", failure="none")),
        ]
        predictions = [
            _prediction("e1", 1, {"activity": "failure", "failure_category": "unknown", "retry_loop": "none", "anomaly": False, "phase": "active"}),
            _prediction("e2", 1, {"activity": "progress", "failure_category": "none", "retry_loop": "none", "anomaly": False, "phase": "active"}),
        ]
        result = score_records(refs, predictions, draws=20)
        binary = result["conditions"]["Rules"]["prefix"]["metrics"]["dimensions"]["failure_detection"]
        self.assertEqual(binary["per_class"]["failure"]["positive_events"], 1)
        self.assertEqual(binary["per_class"]["failure"]["recall"]["estimate"], 1.0)

    def test_rules_prefix_offline_comparison_is_reported(self):
        refs = [
            _reference("e1", "s1", 1, _gold("task_request")),
            _reference("e2", "s1", 2, _gold("progress")),
        ]
        predictions = [
            _prediction("e1", 1, _gold("task_request"), context="prefix"),
            _prediction("e2", 1, _gold("progress"), context="prefix"),
            _prediction("e1", 1, _gold("progress"), context="offline"),
            _prediction("e2", 1, _gold("progress"), context="offline"),
        ]
        result = score_records(refs, predictions, draws=20)
        comparison = result["paired_prefix_vs_offline_activity_macro_f1"]["Rules"]
        self.assertEqual(comparison["event_count"], 2)
        self.assertIn("estimate", comparison)

    def test_independent_annotations_report_raw_agreement_and_kappa(self):
        refs = [
            _reference("e1", "s1", 1, _gold("task_request", phase="start")),
            _reference("e2", "s1", 2, _gold("progress", phase="active")),
        ]
        predictions = [
            _prediction(event_id, 1, {"activity": gold["activity"], "failure_category": "none", "retry_loop": "none", "anomaly": False, "phase": gold["phase"]})
            for event_id, gold in (("e1", refs[0]["gold"]), ("e2", refs[1]["gold"]))
        ]
        annotations_a = [{"id": row["id"], "gold": row["gold"]} for row in refs]
        annotations_b = [{"id": row["id"], "gold": row["gold"]} for row in refs]
        result = score_records(refs, predictions, annotation_a_rows=annotations_a, annotation_b_rows=annotations_b, draws=20)
        agreement = result["independent_annotation_agreement"]["dimensions"]["activity"]
        self.assertEqual(agreement["event_count"], 2)
        self.assertEqual(agreement["raw_agreement"], 1.0)
        self.assertEqual(agreement["cohen_kappa"], 1.0)

    def test_acceptance_requires_full_offline_and_causal_annotation_coverage(self):
        refs = [
            _reference("e1", "s1", 1, _gold("task_request", phase="start")),
            _reference("e2", "s1", 2, _gold("progress", phase="active")),
        ]
        for row in refs:
            row["causal_gold"] = row["gold"]
        predictions = [
            _prediction(row["id"], 1, row["gold"], strategy="jev")
            for row in refs
        ]
        complete_a = [
            {"id": row["id"], "gold": row["gold"], "causal_gold": row["causal_gold"]}
            for row in refs
        ]
        complete_b = [dict(row) for row in complete_a]

        def agreement_gate(rows_a, rows_b):
            result = score_records(
                refs,
                predictions,
                annotation_a_rows=rows_a,
                annotation_b_rows=rows_b,
                adjudication_status="frozen",
                draws=20,
            )
            return result["acceptance"]["Jev per-event"]["gates"]["independent_annotation_agreement"]

        self.assertEqual(agreement_gate(complete_a, complete_b), "pass")
        self.assertEqual(agreement_gate(complete_a, complete_b[:1]), "insufficient_evidence")
        missing_causal = [dict(row) for row in complete_a]
        missing_causal[1].pop("causal_gold")
        self.assertEqual(agreement_gate(missing_causal, complete_b), "insufficient_evidence")
        extra_event = [*complete_b, {"id": "not-in-reference", "gold": _gold("progress"), "causal_gold": _gold("progress")}]
        self.assertEqual(agreement_gate(complete_a, extra_event), "insufficient_evidence")

    def test_nested_runner_summaries_aggregate_sessions_and_unknown_waits(self):
        refs = [
            _reference("e1", "s1", 1, _gold("progress")),
            _reference("e2", "s1", 2, _gold("progress")),
        ]
        predictions = [
            _prediction(event_id, 1, _gold("progress"), strategy="jev")
            for event_id in ("e1", "e2")
        ]
        session_summary = {
            "request_count": 2,
            "request_failure_count": 1,
            "latency": {
                "provider_http_rtt": {"p95_ms": 30.0},
                "projected_online_event_latency": {"p95_ms": 100.0},
                "queue_wait_unknown_event_rows": 0,
            },
            "usage": {"input_tokens": 11, "output_tokens": 6},
            "cost": {"basis": "published_list_price_estimate", "estimate_usd": 0.1},
        }
        second_summary = {
            "request_count": 1,
            "request_failure_count": 0,
            "latency": {
                "provider_http_rtt": {"p95_ms": 40.0},
                "projected_online_event_latency": {"p95_ms": 200.0},
                "queue_wait_unknown_event_rows": 0,
            },
            "usage": {"input_tokens": 13, "output_tokens": 7},
            "cost": {"basis": "published_list_price_estimate", "estimate_usd": 0.2},
        }
        summary = {
            "strategy": "jev",
            "mode": "per-event",
            "context": "prefix",
            "sessions": [{"summary": session_summary}, {"summary": second_summary}],
        }
        result = score_records(refs, predictions, summary_rows=[summary], draws=20)
        operations = result["conditions"]["Jev per-event"]["prefix"]["operations"]
        self.assertEqual(operations["request_count"], 3)
        self.assertEqual(operations["request_failure_count"], 1)
        self.assertEqual(operations["input_tokens"], 24)
        self.assertEqual(operations["output_tokens"], 13)
        self.assertAlmostEqual(operations["jev_list_estimate_usd"], 0.3)
        self.assertEqual(operations["provider_http_p95_ms_worst_session"], 40.0)
        self.assertEqual(operations["projected_async_p95_ms_worst_session"], 200.0)
        self.assertIsNone(operations["provider_http_p95_ms_pooled"])
        self.assertIsNone(operations["projected_async_p95_ms_pooled"])
        self.assertEqual(operations["queue_wait_unknown_event_rows"], 0)

        second_summary["latency"]["queue_wait_unknown_event_rows"] = 1
        result = score_records(refs, predictions, summary_rows=[summary], draws=20)
        operations = result["conditions"]["Jev per-event"]["prefix"]["operations"]
        self.assertIsNone(operations["projected_async_p95_ms_worst_session"])
        self.assertEqual(operations["queue_wait_unknown_event_rows"], 1)

        second_summary["latency"].pop("queue_wait_unknown_event_rows")
        result = score_records(refs, predictions, summary_rows=[summary], draws=20)
        operations = result["conditions"]["Jev per-event"]["prefix"]["operations"]
        self.assertIsNone(operations["projected_async_p95_ms_worst_session"])
        self.assertIsNone(operations["queue_wait_unknown_event_rows"])

    def test_pooled_operations_drive_acceptance_and_match_run_counts(self):
        refs = [
            _reference("e1", "s1", 1, _gold("progress"), eligible_count=1),
            _reference("e2", "s2", 1, _gold("progress"), eligible_count=1),
        ]
        predictions = [
            _prediction(event_id, repetition, _gold("progress"), strategy="jev")
            for event_id in ("e1", "e2")
            for repetition in (1, 2, 3)
        ]
        summary = {
            "strategy": "jev", "mode": "per-event", "context": "prefix",
            "sessions": [
                {"summary": {"request_count": 2, "request_failure_count": 0, "latency": {
                    "provider_http_rtt": {"p95_ms": 1200.0},
                    "projected_online_event_latency": {"p95_ms": 3000.0},
                    "queue_wait_unknown_event_rows": 0,
                }}},
                {"summary": {"request_count": 1, "request_failure_count": 0, "latency": {
                    "provider_http_rtt": {"p95_ms": 1400.0},
                    "projected_online_event_latency": {"p95_ms": 2500.0},
                    "queue_wait_unknown_event_rows": 0,
                }}},
            ],
        }
        operations_row = _operations_row(
            event_count=2, prediction_rows=6, request_count=3,
            failed_request_count=0, http_p95=900.0, projected_p95=2000.0,
        )
        operations_file = {"schema": 1, "conditions": {"jev-prefix": operations_row}}
        result = score_records(
            refs, predictions, summary_rows=[summary], operations=operations_file, draws=20,
        )
        measured = result["conditions"]["Jev per-event"]["prefix"]["operations"]
        self.assertEqual(measured["provider_http_p95_ms_worst_session"], 1400.0)
        self.assertEqual(measured["provider_http_p95_ms_pooled"], 900.0)
        self.assertEqual(measured["projected_async_p95_ms_worst_session"], 3000.0)
        self.assertEqual(measured["projected_async_p95_ms_pooled"], 2000.0)
        criteria = result["acceptance"]["Jev per-event"]["criteria"]
        self.assertEqual(criteria["provider_http_p95_ms"]["bound"], 900.0)

        without_operations = score_records(refs, predictions, summary_rows=[summary], draws=20)
        operations = without_operations["conditions"]["Jev per-event"]["prefix"]["operations"]
        self.assertIsNone(operations["provider_http_p95_ms_pooled"])
        self.assertEqual(
            without_operations["acceptance"]["Jev per-event"]["criteria"]["provider_http_p95_ms"]["status"],
            "not_measured",
        )

        mismatched = {"schema": 1, "conditions": {"jev-prefix": _operations_row(
            event_count=2, prediction_rows=6, request_count=4,
            failed_request_count=0, http_p95=900.0, projected_p95=2000.0,
        )}}
        with self.assertRaises(MetricsError):
            score_records(refs, predictions, summary_rows=[summary], operations=mismatched, draws=20)

    def test_unknown_queue_timing_blocks_pooled_projected_latency(self):
        refs = [
            _reference("e1", "s1", 1, _gold("progress"), eligible_count=1),
            _reference("e2", "s2", 1, _gold("progress"), eligible_count=1),
        ]
        predictions = [
            _prediction(event_id, repetition, _gold("progress"), strategy="jev")
            for event_id in ("e1", "e2")
            for repetition in (1, 2, 3)
        ]
        operations = {"schema": 1, "conditions": {"jev-prefix": _operations_row(
            event_count=2, prediction_rows=6, request_count=3,
            failed_request_count=0, http_p95=900.0,
            projected_count=5, projected_unknown=1, projected_p95=2000.0,
            wait_count=5, wait_unknown=1,
        )}}
        result = score_records(refs, predictions, operations=operations, draws=20)
        measured = result["conditions"]["Jev per-event"]["prefix"]["operations"]
        self.assertEqual(measured["provider_http_p95_ms_pooled"], 900.0)
        self.assertEqual(measured["queue_wait_unknown_event_rows"], 1)
        self.assertIsNone(measured["projected_async_p95_ms_pooled"])
        self.assertIsNone(measured["input_tokens"])
        self.assertIsNone(measured["jev_list_estimate_usd"])

    def test_aggregate_output_contains_no_private_event_or_session_keys(self):
        refs = [_reference("private-event-key", "private-session-key", 1, _gold("progress"), eligible_count=1)]
        predictions = [_prediction("private-event-key", 1, {"activity": "progress", "failure_category": "none", "retry_loop": "none", "anomaly": False, "phase": "active"})]
        encoded = json.dumps(score_records(refs, predictions, draws=20))
        self.assertNotIn("private-event-key", encoded)
        self.assertNotIn("private-session-key", encoded)

    def test_complete_session_event_count_must_match_reference_rows(self):
        refs = [_reference("e1", "s1", 1, _gold("progress"), eligible_count=2)]
        predictions = [_prediction("e1", 1, {"activity": "progress", "failure_category": "none", "retry_loop": "none", "anomaly": False, "phase": "active"})]
        with self.assertRaises(MetricsError):
            score_records(refs, predictions, draws=20)


if __name__ == "__main__":
    unittest.main()
