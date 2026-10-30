"""Synthetic privacy and blinding checks for real_label.py."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import real_label
import real_run


def label(activity: str = "progress") -> dict[str, object]:
    return {
        "activity": activity,
        "failure_category": "none",
        "retry_loop": "none",
        "anomaly": False,
        "phase": "active",
    }


class RealLabelTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "private"
        self.root.mkdir(mode=0o700)
        self.rows = [
            self.row("opaque-a0", "opaque-session-a", 0, "first event"),
            self.row("opaque-a1", "opaque-session-a", 1, "future event"),
            self.row("opaque-b0", "opaque-session-b", 0, "other session"),
        ]
        self.write_projection()
        self.checkpoints = patch("real_label._checkpoint", lambda: None)
        self.checkpoints.start()

    def tearDown(self) -> None:
        self.checkpoints.stop()
        self.temp.cleanup()

    @staticmethod
    def row(event_id: str, session_id: str, order: int, text: str) -> dict[str, object]:
        return {
            "id": event_id,
            "session_id": session_id,
            "event_order": order,
            "harness": "claude",
            "native_format": "claude-jsonl-v1",
            "arrival_ms": order * 10,
            "projection": {
                "event_kind": "user_message",
                "native_record_kind": "user",
                "role": "user",
                "visible_text": text,
            },
        }

    def write_projection(self) -> None:
        projection_path = self.root / real_label.PROJECTION_PATH
        manifest_path = self.root / real_label.PROJECTION_MANIFEST_PATH
        projection_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        manifest_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        data = b"".join(real_run._canonical_json(row) + b"\n" for row in self.rows)
        projection_hash = hashlib.sha256(data).hexdigest()
        manifest = {
            "schema": 1,
            "reviewed_sanitized_content": True,
            "source_projection_sha256": projection_hash,
            "projection_sha256": projection_hash,
            "event_count": len(self.rows),
            "session_count": len({row["session_id"] for row in self.rows}),
            **real_run._freeze_hashes(),
        }
        projection_path.write_bytes(data)
        manifest_path.write_bytes(real_run._canonical_json(manifest) + b"\n")
        os.chmod(projection_path, 0o600)
        os.chmod(manifest_path, 0o600)

    def label_stream(self, annotator: str, activity: str) -> None:
        for _ in self.rows:
            packet = real_label.next_stream(annotator, private_root=self.root)
            self.assertEqual(packet["status"], "ready")
            real_label.submit_stream(
                annotator, label(activity), private_root=self.root,
            )

    def test_streaming_prefix_stops_at_current_target(self) -> None:
        first = real_label.next_stream("A", private_root=self.root)
        self.assertEqual(first["id"], "opaque-a0")
        self.assertIn("first event", first["context"])
        self.assertNotIn("future event", first["context"])
        self.assertNotIn("other session", first["context"])
        self.assertIn("completion", first["protocol"])

        second = real_label.submit_stream("A", label(), private_root=self.root)
        self.assertEqual(second["id"], "opaque-a1")
        self.assertIn("first event", second["context"])
        self.assertIn("future event", second["context"])
        self.assertNotIn("other session", second["context"])

    def test_offline_is_locked_until_both_blind_streams_are_complete(self) -> None:
        self.label_stream("A", "progress")
        self.assertEqual(
            real_label.next_offline("A", private_root=self.root),
            {"status": "streaming_incomplete"},
        )

        self.label_stream("B", "wait")
        packet = real_label.next_offline("A", private_root=self.root)
        self.assertEqual(packet["session_id"], "opaque-session-a")
        self.assertEqual([item["id"] for item in packet["events"]], ["opaque-a0", "opaque-a1"])
        self.assertEqual(packet["events"][0]["streaming_label"], label("progress"))
        self.assertTrue(all(item["streaming_label"] == label("progress") for item in packet["events"]))
        self.assertNotIn("other session", json.dumps(packet))

        with self.assertRaises(real_label.LabelError):
            real_label.submit_offline(
                "A", "opaque-session-b", {}, reviewed=True, private_root=self.root,
            )

        next_packet = real_label.submit_offline(
            "A", "opaque-session-a", {"opaque-a0": label("completion")}, reviewed=True,
            private_root=self.root,
        )
        self.assertEqual(next_packet["session_id"], "opaque-session-b")
        offline_path = self.root / "annotations" / "A-offline.jsonl"
        records = [json.loads(line) for line in offline_path.read_text().splitlines()]
        self.assertEqual(records[0]["gold"], label("completion"))
        self.assertEqual(records[0]["causal_gold"], label("progress"))
        self.assertEqual(records[1]["gold"], label("progress"))
        self.assertEqual(records[1]["causal_gold"], label("progress"))

    def test_projection_change_after_first_packet_is_rejected(self) -> None:
        real_label.next_stream("A", private_root=self.root)
        projection_path = self.root / real_label.PROJECTION_PATH
        projection_path.write_bytes(projection_path.read_bytes() + b"\n")
        with self.assertRaises(real_label.LabelError):
            real_label.next_stream("A", private_root=self.root)

    def test_stream_labels_require_all_five_dimensions(self) -> None:
        with self.assertRaises(real_label.LabelError):
            real_label.submit_stream("A", {"activity": "progress"}, private_root=self.root)


if __name__ == "__main__":
    unittest.main()
