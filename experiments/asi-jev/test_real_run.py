"""Synthetic-only contract tests for the private real-session runner."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import real_run as runner


def projection_row(
    event_id: str = "event-1", *, event_order: int = 0, arrival_ms: float | None = 0.0,
) -> dict:
    return {
        "id": event_id,
        "session_id": "opaque-session-1",
        "event_order": event_order,
        "harness": "codex",
        "native_format": "codex-jsonl-v1",
        "arrival_ms": arrival_ms,
        "projection": {
            "event_kind": "error",
            "native_record_kind": "response_item/error",
            "role": "assistant",
            "tool_name": None,
            "result_status": "error",
            "exit_status": None,
            "stop_reason": "abort",
            "reason": "synthetic reason",
            "error_message": "synthetic error",
            "visible_text": "synthetic visible text",
        },
    }


def gold_row(event_id: str = "event-1") -> dict:
    return {
        "id": event_id,
        "gold": {
            "streaming": {
                "activity": "failure",
                "failure_category": "model",
                "retry_loop": "none",
                "anomaly": False,
                "phase": "blocked",
            },
            "offline": {
                "activity": "failure",
                "failure_category": "model",
                "retry_loop": "none",
                "anomaly": False,
                "phase": "blocked",
            },
        },
    }


class RealRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.private_root = Path(self.temp_dir.name) / "private"
        self.root_patch = patch.object(runner, "PRIVATE_ROOT", self.private_root)
        self.root_patch.start()
        self.thermal_patch = patch.object(runner.real_pipeline, "thermal_checkpoint", return_value=41_000)
        self.thermal_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.addCleanup(self.thermal_patch.stop)
        self.addCleanup(self.temp_dir.cleanup)
        self.private_root.mkdir(mode=0o700)

    def _write_jsonl(self, name: str, rows: list[dict]) -> Path:
        path = self.private_root / name
        path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
        os.chmod(path, 0o600)
        return path

    def test_event_uses_only_sanitized_projection_fields(self) -> None:
        row = projection_row()
        labels = {"event-1": runner._validate_gold_rows([gold_row()], {"event-1"})["event-1"]}
        event = runner._events_from_rows([row], labels)[0]
        self.assertEqual(event.event_id, "event-1")
        self.assertEqual(event.event_kind, "error:assistant")
        self.assertIn("[native_record_kind: response_item/error]", event.evidence_text)
        self.assertIn("[stop_reason: abort]", event.evidence_text)
        self.assertIn("[reason: synthetic reason]", event.evidence_text)
        self.assertIn("[error_message: synthetic error]", event.evidence_text)
        self.assertIn("synthetic visible text", event.evidence_text)
        self.assertEqual(event.timestamp, 0.0)

    def test_private_arrival_times_drive_four_target_500ms_batches(self) -> None:
        arrivals = [0.0, 100.0, 200.0, 499.0, 500.0, None]
        rows = [
            projection_row(f"event-{index}", event_order=index, arrival_ms=arrival)
            for index, arrival in enumerate(arrivals)
        ]
        event_ids = {row["id"] for row in rows}
        labels = runner._validate_gold_rows(
            [gold_row(event_id) for event_id in event_ids], event_ids,
        )
        events = runner._events_from_rows(rows, labels)

        self.assertEqual(
            [event.timestamp for event in events],
            [0.0, 0.1, 0.2, 0.499, 0.5, None],
        )
        context = runner.benchmark.format_context(events[1], events, "prefix", 8192)
        self.assertNotIn("arrival_ms", context)
        self.assertNotIn("0.1", context)
        batches = runner.benchmark.build_microbatches(events, 4, 500.0)
        self.assertEqual([len(batch) for batch, _ in batches], [4, 2])
        self.assertIsNone(batches[1][1][events[5].index])

    def test_freezes_projection_before_complete_reference_and_keeps_event_metadata_only(self) -> None:
        projection_path = self._write_jsonl("candidates.jsonl", [projection_row()])
        reference_path = self._write_jsonl("labels.jsonl", [gold_row()])

        with self.assertRaises(runner.RealRunError) as missing_attestation:
            runner.freeze_projection(projection_path, reviewed_sanitized_content=False)
        self.assertEqual(missing_attestation.exception.code, "review_attestation_required")

        runner.freeze_projection(projection_path, reviewed_sanitized_content=True)
        projection_manifest, _ = runner._projection_freeze(self.private_root)
        self.assertEqual(projection_manifest["event_count"], 1)

        runner.freeze_reference(reference_path)
        event_reference = self.private_root / runner.REFERENCE_EVENTS_PATH
        row = json.loads(event_reference.read_text(encoding="utf-8"))
        self.assertEqual(row["id"], "event-1")
        self.assertEqual(row["event_order"], 0)
        self.assertTrue(row["complete_session"])
        self.assertEqual(row["eligible_event_count"], 1)
        self.assertEqual(row["gold"]["activity"], "failure")
        self.assertEqual(row["causal_gold"]["activity"], "failure")
        self.assertNotIn("visible_text", row)
        self.assertEqual(event_reference.stat().st_mode & 0o777, 0o600)

    def test_reference_rejects_missing_labels_and_projection_payload_extras(self) -> None:
        with self.assertRaises(runner.RealRunError):
            runner._validate_gold_rows([], {"event-1"})

        unsafe = projection_row()
        unsafe["native"] = {"raw": "synthetic raw data"}
        with self.assertRaises(runner.RealRunError):
            runner._validate_projection_rows([unsafe])


if __name__ == "__main__":
    unittest.main()
