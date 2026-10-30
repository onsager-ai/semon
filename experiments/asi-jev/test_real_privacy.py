"""Synthetic tests for the private annotation redaction transform."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import real_privacy


def row(event_id: str, session_id: str, order: int, text: str) -> dict[str, object]:
    return {
        "id": event_id,
        "session_id": session_id,
        "event_order": order,
        "harness": "claude",
        "native_format": "claude-jsonl-v1",
        "arrival_ms": order * 100,
        "projection": {
            "event_kind": "user_message",
            "native_record_kind": "user",
            "visible_text": text,
            "reason": "keep this full field",
        },
    }


def jsonl(rows: list[dict[str, object]]) -> bytes:
    return b"".join((json.dumps(item, ensure_ascii=False) + "\n").encode() for item in rows)


class PrivacyTransformTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "private"
        (self.root / "annotation-input").mkdir(parents=True, mode=0o700)
        self.rows = [
            row("opaque-01", "opaque-session-01", 4, "whole sensitive sentence"),
            row("opaque-02", "opaque-session-02", 9, "another sensitive sentence"),
        ]
        self.packet_names = ["session-01.jsonl", "session-02.jsonl"]
        self.packet_bytes = {}
        for name, item in zip(self.packet_names, self.rows, strict=True):
            data = jsonl([item])
            self.packet_bytes[name] = data
            (self.root / "annotation-input" / name).write_bytes(data)
            os.chmod(self.root / "annotation-input" / name, 0o600)
        self.bundle_bytes = jsonl(self.rows)
        (self.root / "review-bundle.jsonl").write_bytes(self.bundle_bytes)
        os.chmod(self.root / "review-bundle.jsonl", 0o600)
        self.plan_path = self.root / "privacy-plan.json"
        self.write_plan([
            {"packet": "session-01.jsonl", "event_order": 4, "field": "visible_text"},
            {"packet": "session-02.jsonl", "event_order": 9, "field": "reason"},
        ])
        self.checkpoints = patch("real_privacy._checkpoint", lambda: None)
        self.checkpoints.start()

    def tearDown(self) -> None:
        self.checkpoints.stop()
        self.temp.cleanup()

    def write_plan(self, redactions: list[dict[str, object]]) -> None:
        self.plan_path.write_text(json.dumps({"redactions": redactions}) + "\n")
        os.chmod(self.plan_path, 0o600)

    def test_replaces_whole_fields_and_preserves_exact_backup(self) -> None:
        counts = real_privacy.apply_plan(self.plan_path, private_root=self.root)
        self.assertEqual(counts, {"redactions": 2, "packets": 2, "bundle_rows": 2})
        backup = self.root / real_privacy.BACKUP_NAME
        self.assertEqual(backup.read_bytes(), self.bundle_bytes)

        first = json.loads((self.root / "annotation-input" / self.packet_names[0]).read_text())
        second = json.loads((self.root / "annotation-input" / self.packet_names[1]).read_text())
        self.assertEqual(first["projection"]["visible_text"], real_privacy.PLACEHOLDER)
        self.assertEqual(first["projection"]["reason"], "keep this full field")
        self.assertEqual(second["projection"]["visible_text"], "another sensitive sentence")
        self.assertEqual(second["projection"]["reason"], real_privacy.PLACEHOLDER)

        bundle = [json.loads(line) for line in (self.root / "review-bundle.jsonl").read_text().splitlines()]
        self.assertEqual(bundle[0]["projection"]["visible_text"], real_privacy.PLACEHOLDER)
        self.assertEqual(bundle[1]["projection"]["reason"], real_privacy.PLACEHOLDER)
        manifest = json.loads((self.root / real_privacy.MANIFEST_NAME).read_text())
        self.assertEqual(manifest["bundle_before_sha256"], hashlib.sha256(self.bundle_bytes).hexdigest())
        self.assertEqual(manifest["redaction_count"], 2)
        self.assertEqual(stat_mode(backup), 0o600)

    def test_refuses_after_projection_freeze_without_writing(self) -> None:
        freeze = self.root / "manifests" / "real-projection.json"
        freeze.parent.mkdir(mode=0o700)
        freeze.write_text("synthetic freeze marker")
        with self.assertRaises(real_privacy.PrivacyError):
            real_privacy.apply_plan(self.plan_path, private_root=self.root)
        self.assertFalse((self.root / real_privacy.BACKUP_NAME).exists())
        self.assertEqual((self.root / "review-bundle.jsonl").read_bytes(), self.bundle_bytes)

    def test_refuses_packet_bundle_mismatch_before_any_write(self) -> None:
        path = self.root / "annotation-input" / self.packet_names[0]
        changed = row("opaque-01", "opaque-session-01", 4, "different text")
        path.write_bytes(jsonl([changed]))
        with self.assertRaises(real_privacy.PrivacyError):
            real_privacy.apply_plan(self.plan_path, private_root=self.root)
        self.assertFalse((self.root / real_privacy.BACKUP_NAME).exists())
        self.assertEqual((self.root / "review-bundle.jsonl").read_bytes(), self.bundle_bytes)

    def test_rejects_duplicate_plan_entries(self) -> None:
        entry = {"packet": "session-01.jsonl", "event_order": 4, "field": "visible_text"}
        self.write_plan([entry, entry])
        with self.assertRaises(real_privacy.PrivacyError):
            real_privacy.apply_plan(self.plan_path, private_root=self.root)
        self.assertFalse((self.root / real_privacy.BACKUP_NAME).exists())


def stat_mode(path: Path) -> int:
    return path.stat().st_mode & 0o777


if __name__ == "__main__":
    unittest.main()
