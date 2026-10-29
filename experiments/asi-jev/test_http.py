"""In-process tests for the benchmark's remote request plumbing."""

from __future__ import annotations

import os
import sys
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import benchmark as runner


EXPECTED = {"activity": "unknown", "failure_category": "unknown", "retry_loop": "none", "anomaly": False, "phase": "unknown"}


def event(index: int, kind: str, text: str) -> runner.Event:
    gold = dict(EXPECTED)
    return runner.Event(index, f"e{index}", "development", "fixture", "codex", "jsonl", "0" * 64,
                        f"session-{index}", 0, None, {"streaming": gold, "offline": dict(gold)},
                        kind, text, False, False)


def args(mode: str) -> Namespace:
    return Namespace(strategy="jev", mode=mode, context="prefix", repeats=3, jev_model="typesafe/jev-1.14.0",
                      llm_model="", max_requests=10, batch_size=2, batch_window_ms=100.0,
                      max_context_bytes=1024, timeout=1.0, llm_endpoint="/v1/chat/completions",
                      input_price_per_1m=None, output_price_per_1m=None)


def response(payload: dict) -> dict:
    answers = {}
    for question_id, question in payload["questions"].items():
        if question["type"] == "noul":
            answers[question_id] = {"type": "noul", "noul": 0.1}
        else:
            labels = list(question["criteria"])
            selected = "unknown" if "unknown" in labels else "none"
            answers[question_id] = {
                "type": "choice", "choice": selected, "confidence": 1.0,
                "probabilities": {label: float(label == selected) for label in labels},
            }
    return {"model": "jev-1.14.0", "answers": answers, "usage": {"input_tokens": 10, "output_tokens": 2}}


class RemoteRunTests(unittest.TestCase):
    def setUp(self) -> None:
        self.credentials = patch.dict(os.environ, {"ASI_API_BASE": "https://mock.invalid", "ASI_API_KEY": "test-key"})
        self.credentials.start()
        self.addCleanup(self.credentials.stop)

    def test_per_event_parses_all_dimensions_and_accounts_for_failed_abstention(self) -> None:
        calls = []

        def fake_post(url: str, key: str, payload: dict, timeout: float):
            calls.append(payload)
            self.assertEqual(url, "https://mock.invalid/v1/systemone")
            if len(calls) == 1:
                raise runner.ProviderFailure("timeout", 37.5, 1234)
            return response(payload), 12.0, 567

        with patch.object(runner, "_post_json", side_effect=fake_post):
            rows, summary, request_rows, failures = runner.run_benchmark(args("per-event"), [event(0, "assistant", "progress")], "a" * 64)

        self.assertEqual(len(calls), 3)
        self.assertEqual(len(request_rows), 3)
        self.assertEqual(failures[0]["code"], "timeout")
        self.assertEqual(request_rows[0]["rtt_ms"], 37.5)
        self.assertEqual(rows[0]["request_rtt_ms"], 37.5)
        self.assertEqual(rows[0]["predictions"], {key: None for key in runner.DIMENSIONS})
        self.assertEqual(rows[1]["predictions"], EXPECTED)
        self.assertEqual(set(rows[1]["validated_answers"]), set(runner.DIMENSIONS))
        metric = summary["metrics"]["overall"]["activity"]
        self.assertEqual(metric["count"], 3)
        self.assertAlmostEqual(metric["accuracy"], 2 / 3)
        self.assertAlmostEqual(metric["coverage"], 2 / 3)
        self.assertIsNone(summary["cost"]["estimate_usd"])
        self.assertTrue(summary["cost"]["unknown_cost_for_failed_requests"])
        self.assertAlmostEqual(summary["cost"]["successful_response_subtotal_estimate_usd"], 20 * 0.042 / 1_000_000)

    def test_triggered_mode_measures_gate_and_adds_http_rtt_only_when_called(self) -> None:
        calls = []

        def fake_post(url: str, key: str, payload: dict, timeout: float):
            calls.append(payload)
            return response(payload), 12.5, 567

        events = [event(0, "user_message", "Please do this task."), event(1, "tool_result", "[tool is_error: true]")]
        with patch.object(runner, "_post_json", side_effect=fake_post):
            rows, _, _, _ = runner.run_benchmark(args("event-triggered"), events, "b" * 64)

        self.assertEqual(len(calls), 3)
        bypass = next(row for row in rows if row["event_index"] == 0)
        triggered = next(row for row in rows if row["event_index"] == 1)
        self.assertFalse(bypass["model_attempted"])
        self.assertIsNone(bypass["request_rtt_ms"])
        self.assertGreater(bypass["rule_gate_compute_ms"], 0)
        self.assertAlmostEqual(bypass["projected_online_latency_ms"], bypass["rule_gate_compute_ms"])
        self.assertTrue(triggered["model_attempted"])
        self.assertEqual(triggered["request_rtt_ms"], 12.5)
        self.assertGreater(triggered["rule_gate_compute_ms"], 0)
        self.assertAlmostEqual(triggered["projected_online_latency_ms"], 12.5 + triggered["rule_gate_compute_ms"])


if __name__ == "__main__":
    unittest.main()
