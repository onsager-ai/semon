"""Synthetic tests for disclosure-safe run operations aggregation."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import real_operations


class RealOperationsTests(unittest.TestCase):
    def _run(self, root: Path, condition: str = "jev-prefix") -> Path:
        strategy, mode, context = real_operations.real_run.CONDITIONS[condition]
        directory = root / "runs" / ("synthetic-" + condition)
        directory.mkdir(parents=True)
        summary = {
            "condition": condition, "strategy": strategy, "mode": mode, "context": context,
            "event_count": 1, "execution_wall_seconds": 4.0,
            "request_count": 2, "request_failure_count": 1,
            "run_id": "private-run-id", "requested_model": "private-model-name",
            "sessions": [{"summary": {"usage": {"input_tokens": 11, "output_tokens": 5}}}],
        }
        calls = [
            {"request": True, "rtt_ms": 10.0, "failed": None, "input_tokens": 11,
             "output_tokens": 5, "response_id": "private-response-id",
             "provider_usage": {"private": "response details"}},
            {"request": True, "rtt_ms": 30.0, "failed": "synthetic-failure"},
            {"request": False},
        ]
        rows = [
            {"id": "private-event-id", "strategy": strategy, "mode": mode, "context": context,
             "request_rtt_ms": 10.0, "rule_gate_compute_ms": 2.0,
             "projected_online_latency_ms": 14.0, "logical_queue_wait_ms": 2.0},
            {"id": "another-private-event-id", "strategy": strategy, "mode": mode, "context": context,
             "request_rtt_ms": None, "rule_gate_compute_ms": 4.0,
             "projected_online_latency_ms": None, "logical_queue_wait_ms": None},
        ]
        (directory / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
        (directory / "calls.json").write_text(json.dumps(calls), encoding="utf-8")
        (directory / "predictions.jsonl").write_text(
            "".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8",
        )
        return directory

    def test_pools_failed_request_rtt_and_reports_only_numeric_aggregates(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "private"
            directory = self._run(root)
            with patch.object(real_operations.real_run, "_thermal_checkpoint"):
                result = real_operations.aggregate_runs([directory], root=root)
            condition = result["conditions"]["jev-prefix"]
            self.assertEqual(condition["request_count"], 2)
            self.assertEqual(condition["failed_request_count"], 1)
            self.assertEqual(condition["request_rtt_ms"]["count"], 2)
            self.assertEqual(condition["request_rtt_ms"]["p50_ms"], 20.0)
            self.assertAlmostEqual(condition["request_rtt_ms"]["p95_ms"], 29.0)
            self.assertEqual(condition["latency_ms"]["logical_queue_wait_ms"]["unknown_rows"], 1)
            self.assertEqual(condition["latency_ms"]["rule_gate_compute_ms"]["p50_ms"], 3.0)
            self.assertEqual(condition["prediction_rows"], 2)
            self.assertEqual(condition["prediction_rows_per_second"], 0.5)
            self.assertEqual(condition["usage_tokens"]["status"], "partial")
            self.assertEqual(condition["jev_list_cost_status"], "partial_estimate")
            self.assertEqual(condition["haiku_billed_cost_status"], "not_applicable")
            encoded = json.dumps(result)
            for private_value in (
                "private-event-id", "another-private-event-id", "private-response-id",
                "private-run-id", "private-model-name", "response details",
            ):
                self.assertNotIn(private_value, encoded)

    def test_haiku_billed_cost_is_unknown_and_duplicate_conditions_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "private"
            directory = self._run(root, "llm-prefix")
            with patch.object(real_operations.real_run, "_thermal_checkpoint"):
                result = real_operations.aggregate_runs([directory], root=root)
                with self.assertRaises(real_operations.OperationsError) as duplicate:
                    real_operations.aggregate_runs([directory, directory], root=root)
            condition = result["conditions"]["llm-prefix"]
            self.assertEqual(condition["haiku_billed_cost_usd"], None)
            self.assertEqual(condition["haiku_billed_cost_status"], "unknown")
            self.assertEqual(duplicate.exception.code, "duplicate_condition")


if __name__ == "__main__":
    unittest.main()
