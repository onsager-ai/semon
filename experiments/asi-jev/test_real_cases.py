"""Synthetic-only tests for private benchmark failure cases."""

from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import real_cases
import real_run


def projection(event_id: str, order: int) -> dict:
    return {
        "id": event_id, "session_id": "session-1", "event_order": order,
        "harness": "codex", "native_format": "codex-jsonl-v1", "arrival_ms": float(order * 10),
        "projection": {
            "event_kind": "assistant_message", "native_record_kind": "response_item/message",
            "role": "assistant", "tool_name": None, "result_status": None,
            "exit_status": None, "stop_reason": None, "reason": None,
            "error_message": None, "visible_text": f"synthetic event {event_id}",
        },
    }


def reference(event_id: str) -> dict:
    labels = {
        "activity": "progress", "failure_category": "none", "retry_loop": "none",
        "anomaly": False, "phase": "active",
    }
    return {"id": event_id, "gold": {"streaming": labels, "offline": labels}}


class RealCasesTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "private"
        self.root.mkdir(mode=0o700)
        self.root_patch = patch.object(real_run, "PRIVATE_ROOT", self.root)
        self.thermal_patch = patch.object(real_run, "_thermal_checkpoint")
        self.root_patch.start()
        self.thermal_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.addCleanup(self.thermal_patch.stop)
        self.addCleanup(self.temp.cleanup)

    def _freeze(self) -> None:
        projection_path, label_path = self.root / "projection-input.jsonl", self.root / "labels-input.jsonl"
        projections = [projection("event-1", 0), projection("event-2", 1)]
        labels = [reference("event-1"), reference("event-2")]
        projection_path.write_text("".join(json.dumps(row) + "\n" for row in projections), encoding="utf-8")
        label_path.write_text("".join(json.dumps(row) + "\n" for row in labels), encoding="utf-8")
        os.chmod(projection_path, 0o600)
        os.chmod(label_path, 0o600)
        real_run.freeze_projection(projection_path, reviewed_sanitized_content=True)
        real_run.freeze_reference(label_path)

    def _run(self, condition: str, predictions: list[dict]) -> Path:
        strategy, mode, context = real_run.CONDITIONS[condition]
        directory = self.root / "runs" / condition
        directory.mkdir(parents=True)
        summary = {
            "condition": condition, "strategy": strategy, "mode": mode, "context": context,
            "remote_repetitions": 3, "event_count": 2,
        }
        (directory / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
        (directory / "predictions.jsonl").write_text(
            "".join(json.dumps(row) + "\n" for row in predictions), encoding="utf-8",
        )
        return directory

    def _predictions(self, condition: str, errors: dict[tuple[str, int], set[str]]) -> list[dict]:
        strategy, mode, context = real_run.CONDITIONS[condition]
        rows = []
        for event_id in ("event-1", "event-2"):
            for repetition in (1, 2, 3):
                values = {
                    "activity": "progress", "failure_category": "none", "retry_loop": "none",
                    "anomaly": False, "phase": "active",
                }
                for dimension in errors.get((event_id, repetition), set()):
                    values[dimension] = "failure" if dimension == "activity" else "blocked"
                rows.append({
                    "id": event_id, "repetition": repetition, "strategy": strategy,
                    "mode": mode, "context": context, "predictions": values,
                })
        return rows

    def test_ranks_unique_events_groups_trials_and_uses_condition_context(self) -> None:
        self._freeze()
        prefix = self._run("jev-prefix", self._predictions("jev-prefix", {
            ("event-1", 1): {"activity", "phase"},
            ("event-1", 2): {"activity", "phase"},
            ("event-1", 3): {"activity", "phase"},
            ("event-2", 1): {"phase"}, ("event-2", 2): {"phase"},
        }))
        offline = self._run("jev-offline", self._predictions("jev-offline", {
            ("event-1", 1): {"phase"},
        }))

        summary = real_cases.make_packet([prefix, offline], private_root=self.root)
        output = self.root / real_cases.OUTPUT_RELATIVE
        rows = [json.loads(line) for line in output.read_text(encoding="utf-8").splitlines()]
        prefix_rows = [row for row in rows if row["condition"] == "jev-prefix"]
        offline_rows = [row for row in rows if row["condition"] == "jev-offline"]
        self.assertEqual(summary["by_condition"], {"jev-prefix": 2, "jev-offline": 1})
        self.assertEqual([row["id"] for row in prefix_rows], ["event-1", "event-2"])
        self.assertEqual(len(prefix_rows[0]["trial_predictions"]), 3)
        self.assertEqual(prefix_rows[0]["mismatched_dimensions"], ["activity", "phase"])
        self.assertEqual(prefix_rows[0]["context_mode"], "prefix")
        self.assertNotIn("synthetic event event-2", prefix_rows[0]["context"])
        self.assertEqual(offline_rows[0]["context_mode"], "offline")
        self.assertIn("synthetic event event-2", offline_rows[0]["context"])
        self.assertTrue(all(len(row["context"].encode("utf-8")) <= 8_192 for row in rows))
        self.assertEqual(output.parent.stat().st_mode & 0o777, 0o700)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        encoded = output.read_text(encoding="utf-8")
        self.assertNotIn("native_sha256", encoded)
        with self.assertRaises(real_cases.CasesError) as overwrite:
            real_cases.make_packet([prefix], private_root=self.root)
        self.assertEqual(overwrite.exception.code, "output_exists")


if __name__ == "__main__":
    unittest.main()
