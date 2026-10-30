"""Synthetic tests for local metadata inventory and root envelope parsing."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import real_prepare


class RealPrepareTests(unittest.TestCase):
    def test_inventory_excludes_recent_large_subagent_and_non_owned_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            claude = base / "claude"
            codex = base / "codex"
            private = base / "private"
            now = time.time_ns()

            def source(path: Path, size: int, age_ns: int) -> None:
                path.parent.mkdir(parents=True, exist_ok=True)
                with path.open("wb") as stream:
                    stream.truncate(size)
                timestamp = now - age_ns
                os.utime(path, ns=(timestamp, timestamp))

            source(claude / "small.jsonl", 2, real_prepare.MIN_AGE_NS + 2_000_000_000)
            source(claude / "larger.jsonl", 4, real_prepare.MIN_AGE_NS + 2_000_000_000)
            source(claude / "recent.jsonl", 1, real_prepare.MIN_AGE_NS - 2_000_000_000)
            source(claude / "limit.jsonl", real_prepare.MAX_BYTES, real_prepare.MIN_AGE_NS + 2_000_000_000)
            source(claude / "subagents" / "nested.jsonl", 1,
                   real_prepare.MIN_AGE_NS + 2_000_000_000)
            source(codex / "session.jsonl", 3, real_prepare.MIN_AGE_NS + 2_000_000_000)
            (claude / "alias.jsonl").symlink_to(claude / "small.jsonl")

            with patch.object(real_prepare.pipeline, "thermal_checkpoint", return_value=40_000):
                result = real_prepare.inventory_sources(
                    (("claude-code", claude), ("codex", codex)), private_root=private,
                )

            rows = json.loads((private / "inventory.json").read_text(encoding="utf-8"))
            self.assertEqual(result["files"], 3)
            self.assertEqual([row["harness"] for row in rows], ["claude-code", "claude-code", "codex"])
            self.assertEqual([row["size"] for row in rows], [2, 4, 3])
            self.assertEqual(result["by_harness"], {"claude-code": 2, "codex": 1})
            self.assertNotIn("recent.jsonl", " ".join(row["path"] for row in rows))
            self.assertNotIn("nested.jsonl", " ".join(row["path"] for row in rows))
            self.assertNotIn("limit.jsonl", " ".join(row["path"] for row in rows))
            self.assertNotIn("alias.jsonl", " ".join(row["path"] for row in rows))

            with patch.object(real_prepare.pipeline, "thermal_checkpoint", return_value=40_000), \
                    patch.object(real_prepare.os, "getuid", return_value=os.getuid() + 1):
                foreign_owner = real_prepare.inventory_sources(
                    (("claude-code", claude),), private_root=base / "foreign-private",
                )
            self.assertEqual(foreign_owner["files"], 0)

    def test_root_user_envelopes_hash_cwd_and_objective_as_canonical_array(self) -> None:
        cwd = "/synthetic/project"
        objective = "inspect a synthetic fixture"
        envelope = json.dumps({
            "original_contract": {"cwd": cwd, "objective": objective},
            "user_messages": ["synthetic user request"],
            "task_role": "worker",
        })
        canonical = json.dumps([cwd, objective], ensure_ascii=False, sort_keys=True,
                               separators=(",", ":"))
        expected_key = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
        records = (
            ("claude-code", {
                "type": "user",
                "message": {"role": "user", "content": [{"type": "text", "text": envelope}]},
            }),
            ("codex", {
                "payload": {"role": "user", "content": [{"type": "input_text", "text": envelope}]},
            }),
        )
        for harness, record in records:
            with self.subTest(harness=harness):
                texts = real_prepare._user_texts(record, harness)
                self.assertEqual(len(texts), 1)
                self.assertEqual(real_prepare._task_facts(texts[0]), (expected_key, "worker"))

    def test_task_facts_ignores_json_without_root_contract_keys(self) -> None:
        self.assertEqual(real_prepare._task_facts('{"cwd":"/synthetic","objective":"fixture"}'),
                         (None, None))


if __name__ == "__main__":
    unittest.main()
