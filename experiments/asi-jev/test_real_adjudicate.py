"""Synthetic blinding and freeze tests for real_adjudicate.py."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import real_adjudicate
import real_label
import real_run


def gold(activity: str) -> dict[str, object]:
    return {
        "activity": activity,
        "failure_category": "none",
        "retry_loop": "none",
        "anomaly": False,
        "phase": "active",
    }


def jsonl(rows: list[dict[str, object]]) -> bytes:
    return b"".join(real_label._canonical(row) + b"\n" for row in rows)


class AdjudicationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "private"
        self.root.mkdir(mode=0o700)
        (self.root / "annotations").mkdir(mode=0o700)
        self.rows = [
            self.event("opaque-01", "opaque-session-01", 0, "current event"),
            self.event("opaque-02", "opaque-session-01", 1, "future event"),
            self.event("opaque-03", "opaque-session-02", 0, "other session"),
        ]
        self.write_projection()
        self.streams = {
            "A": {"opaque-01": gold("progress"), "opaque-02": gold("progress"), "opaque-03": gold("wait")},
            "B": {"opaque-01": gold("wait"), "opaque-02": gold("progress"), "opaque-03": gold("wait")},
        }
        self.offlines = {
            "A": {"opaque-01": gold("failure"), "opaque-02": gold("progress"), "opaque-03": gold("wait")},
            "B": {"opaque-01": gold("completion"), "opaque-02": gold("progress"), "opaque-03": gold("wait")},
        }
        self.write_source_annotations()
        self.checkpoints = patch("real_label._checkpoint", lambda: None)
        self.checkpoints.start()

    def tearDown(self) -> None:
        self.checkpoints.stop()
        self.temp.cleanup()

    @staticmethod
    def event(event_id: str, session_id: str, order: int, text: str) -> dict[str, object]:
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
        data = jsonl(self.rows)
        digest = hashlib.sha256(data).hexdigest()
        manifest = {
            "schema": 1,
            "reviewed_sanitized_content": True,
            "source_projection_sha256": digest,
            "projection_sha256": digest,
            "event_count": len(self.rows),
            "session_count": len({row["session_id"] for row in self.rows}),
            **real_run._freeze_hashes(),
        }
        projection_path.write_bytes(data)
        manifest_path.write_bytes(real_label._canonical(manifest) + b"\n")
        os.chmod(projection_path, 0o600)
        os.chmod(manifest_path, 0o600)

    def write_source_annotations(self) -> None:
        hashes = {}
        for annotator in ("A", "B"):
            stream_rows = [{"id": row["id"], "gold": self.streams[annotator][row["id"]]} for row in self.rows]
            stream_data = jsonl(stream_rows)
            offline_rows = [{
                "id": row["id"], "gold": self.offlines[annotator][row["id"]],
                "causal_gold": self.streams[annotator][row["id"]],
            } for row in self.rows]
            offline_data = jsonl(offline_rows)
            for frame, data in (("streaming", stream_data), ("offline", offline_data)):
                path = self.root / "annotations" / f"{annotator}-{frame}.jsonl"
                path.write_bytes(data)
                os.chmod(path, 0o600)
                hashes[f"{annotator}-{frame}"] = hashlib.sha256(data).hexdigest()
            hashes[annotator] = hashes[f"{annotator}-streaming"]
        projection_hash = hashlib.sha256((self.root / real_label.PROJECTION_PATH).read_bytes()).hexdigest()
        projection_manifest = (self.root / real_label.PROJECTION_MANIFEST_PATH).read_bytes()
        labeling_state = {
            "schema": 1,
            "projection_sha256": projection_hash,
            "projection_manifest_sha256": hashlib.sha256(projection_manifest).hexdigest(),
            "streaming_sha256": hashlib.sha256(real_label._canonical({a: hashes[a] for a in ("A", "B")})).hexdigest(),
            "streaming_annotation_sha256": {a: hashes[a] for a in ("A", "B")},
        }
        (self.root / real_label.STATE_PATH).write_bytes(real_label._canonical(labeling_state) + b"\n")
        os.chmod(self.root / real_label.STATE_PATH, 0o600)

    def test_causal_then_offline_adjudication_and_reference(self) -> None:
        originals = {
            path.name: path.read_bytes()
            for path in (self.root / "annotations").glob("[AB]-*.jsonl")
        }
        causal = real_adjudicate.next_causal(private_root=self.root)
        self.assertEqual(causal["id"], "opaque-01")
        self.assertEqual(causal["label_a"], gold("progress"))
        self.assertEqual(causal["label_b"], gold("wait"))
        self.assertIn("current event", causal["context"])
        self.assertNotIn("future event", causal["context"])
        self.assertNotIn("other session", causal["context"])
        self.assertEqual(real_adjudicate.next_offline(private_root=self.root), {"status": "causal_incomplete"})

        causal_done = real_adjudicate.submit_causal(gold("recovery"), private_root=self.root)
        self.assertEqual(causal_done, {"status": "causal_complete"})
        session = real_adjudicate.next_offline(private_root=self.root)
        self.assertEqual(session["session_id"], "opaque-session-01")
        self.assertEqual([item["id"] for item in session["events"]], ["opaque-01", "opaque-02"])
        self.assertEqual(session["events"][0]["offline_a"], gold("failure"))
        self.assertEqual(session["events"][0]["offline_b"], gold("completion"))
        self.assertNotIn("offline_a", session["events"][1])
        self.assertEqual(session["events"][0]["streaming_label"], gold("recovery"))

        with self.assertRaises(real_adjudicate.AdjudicationError):
            real_adjudicate.submit_offline(
                "opaque-session-01", {}, reviewed=True, private_root=self.root,
            )
        complete = real_adjudicate.submit_offline(
            "opaque-session-01", {"opaque-01": real_adjudicate.UNKNOWN_GOLD},
            reviewed=True, private_root=self.root,
        )
        self.assertEqual(complete, {"status": "adjudication_complete", "events": 3})
        reference = [json.loads(line) for line in (self.root / real_adjudicate.REFERENCE_PATH).read_text().splitlines()]
        self.assertEqual([row["id"] for row in reference], ["opaque-01", "opaque-02", "opaque-03"])
        self.assertEqual(reference[0]["gold"]["streaming"], gold("recovery"))
        self.assertEqual(reference[0]["gold"]["offline"], real_adjudicate.UNKNOWN_GOLD)
        self.assertEqual(reference[1]["gold"]["offline"], gold("progress"))
        self.assertEqual(reference[2]["gold"]["offline"], gold("wait"))
        self.assertEqual(originals, {
            path.name: path.read_bytes()
            for path in (self.root / "annotations").glob("[AB]-*.jsonl")
        })

    def test_frozen_causal_decisions_cannot_be_overwritten(self) -> None:
        real_adjudicate.next_causal(private_root=self.root)
        real_adjudicate.submit_causal(gold("recovery"), private_root=self.root)
        with self.assertRaises(real_adjudicate.AdjudicationError):
            real_adjudicate.submit_causal(gold("failure"), private_root=self.root)

    def test_source_hash_change_is_rejected(self) -> None:
        real_adjudicate.next_causal(private_root=self.root)
        path = self.root / "annotations" / "A-offline.jsonl"
        path.write_bytes(path.read_bytes() + b"\n")
        with self.assertRaises(real_adjudicate.AdjudicationError):
            real_adjudicate.next_causal(private_root=self.root)

    def test_incomplete_offline_source_coverage_is_rejected(self) -> None:
        path = self.root / "annotations" / "B-offline.jsonl"
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        path.write_bytes(jsonl(rows[:-1]))
        with self.assertRaises(real_adjudicate.AdjudicationError):
            real_adjudicate.next_causal(private_root=self.root)

    def test_explicit_full_label_override_is_used(self) -> None:
        real_adjudicate.next_causal(private_root=self.root)
        real_adjudicate.submit_causal(gold("recovery"), private_root=self.root)
        real_adjudicate.next_offline(private_root=self.root)
        real_adjudicate.submit_offline(
            "opaque-session-01", {"opaque-01": gold("recovery")},
            reviewed=True, private_root=self.root,
        )
        reference = [json.loads(line) for line in (self.root / real_adjudicate.REFERENCE_PATH).read_text().splitlines()]
        self.assertEqual(reference[0]["gold"]["offline"], gold("recovery"))


if __name__ == "__main__":
    unittest.main()
