"""Synthetic-only tests for the local user spot-check packet."""

from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import real_run
import real_sample


def projection_row(index: int, harness: str) -> dict:
    session_id = f"session-{harness}"
    return {
        "id": f"event-{index:02}", "session_id": session_id,
        "event_order": index if harness == "claude" else index - 30,
        "harness": harness, "native_format": f"{harness}-jsonl-v1",
        "arrival_ms": float(index),
        "projection": {
            "event_kind": "assistant_message",
            "native_record_kind": "assistant" if harness == "claude" else "response_item/message",
            "role": "assistant", "tool_name": None, "result_status": None,
            "exit_status": None, "stop_reason": None, "reason": None,
            "error_message": None, "visible_text": f"synthetic event {index}",
        },
    }


def gold_row(event_id: str, *, anomaly: bool = False, failure: str = "none", retry: str = "none") -> dict:
    labels = {
        "activity": "failure" if failure != "none" else "progress",
        "failure_category": failure, "retry_loop": retry,
        "anomaly": anomaly, "phase": "active",
    }
    return {"id": event_id, "gold": {"streaming": labels, "offline": labels}}


class RealSampleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "private"
        self.root.mkdir(mode=0o700)
        self.root_patch = patch.object(real_run, "PRIVATE_ROOT", self.root)
        self.root_patch.start()
        self.thermal_patch = patch.object(real_run.real_pipeline, "thermal_checkpoint", return_value=41_000)
        self.thermal_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.addCleanup(self.thermal_patch.stop)
        self.addCleanup(self.temporary.cleanup)

    def _freeze(self) -> None:
        projections = [projection_row(i, "claude" if i < 30 else "codex") for i in range(60)]
        labels = []
        for index, row in enumerate(projections):
            labels.append(gold_row(
                row["id"], anomaly=index == 1,
                failure="tool" if index == 2 else "none",
                retry="loop" if index == 3 else "retry" if index == 4 else "none",
            ))
        projection_path = self.root / "source-projection.jsonl"
        reference_path = self.root / "source-reference.jsonl"
        projection_path.write_text("".join(json.dumps(row) + "\n" for row in projections), encoding="utf-8")
        reference_path.write_text("".join(json.dumps(row) + "\n" for row in labels), encoding="utf-8")
        os.chmod(projection_path, 0o600)
        os.chmod(reference_path, 0o600)
        real_run.freeze_projection(projection_path, reviewed_sanitized_content=True)
        real_run.freeze_reference(reference_path)

    def test_sample_is_deterministic_stratified_bounded_and_pending(self) -> None:
        self._freeze()
        summary = real_sample.prepare_sample(private_root=self.root)
        self.assertEqual(summary["sample_count"], 50)
        self.assertEqual(set(summary["by_harness"]), {"claude", "codex"})
        self.assertEqual(summary["rare_label_counts"]["anomaly_true"], 1)
        self.assertEqual(summary["rare_label_counts"]["failure_not_none"], 1)
        self.assertEqual(summary["rare_label_counts"]["retry_loop"], 1)
        self.assertEqual(summary["rare_label_counts"]["retry"], 1)

        output = self.root / "spot-check"
        packet = [json.loads(line) for line in (output / "packet.jsonl").read_text().splitlines()]
        template = [json.loads(line) for line in (output / "template.jsonl").read_text().splitlines()]
        manifest = json.loads((output / "manifest.json").read_text())
        self.assertEqual([row["id"] for row in packet], [row["id"] for row in template])
        self.assertEqual(len({row["id"] for row in packet}), 50)
        self.assertEqual(
            [row["id"] for row in packet],
            real_sample._select_ids(list(reversed([
                json.loads(line) for line in
                (self.root / real_run.REFERENCE_EVENTS_PATH).read_text().splitlines()
            ]))),
        )
        self.assertTrue(all(len(row["context"].encode("utf-8")) <= 8_192 for row in packet))
        self.assertTrue(all(set(row) == {"id", "harness", "context", "gold", "causal_gold"} for row in packet))
        self.assertTrue(all(row["gold"] is None and row["causal_gold"] is None for row in template))
        self.assertEqual(manifest["user_review"]["status"], "pending")
        self.assertFalse(manifest["user_review"]["template_is_returned_review"])
        self.assertFalse(manifest["user_review"]["agreement_counted"])
        self.assertEqual(output.stat().st_mode & 0o777, 0o700)
        for name in ("packet.jsonl", "template.jsonl", "manifest.json"):
            self.assertEqual((output / name).stat().st_mode & 0o777, 0o600)

        with self.assertRaises(real_sample.SpotCheckError) as duplicate:
            real_sample.prepare_sample(private_root=self.root)
        self.assertEqual(duplicate.exception.code, "output_exists")


if __name__ == "__main__":
    unittest.main()
