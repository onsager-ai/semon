"""Synthetic privacy and envelope tests for the local real-session pipeline."""

from __future__ import annotations

import json
import hashlib
import os
import stat
import tempfile
import time
import unittest
from collections import Counter
from pathlib import Path
from unittest.mock import patch

import real_pipeline


def _write_closed_log(path: Path, records: list[dict[str, object]]) -> bytes:
    raw = b"".join(
        (json.dumps(record, separators=(",", ":")) + "\n").encode("utf-8")
        for record in records
    )
    path.write_bytes(raw)
    old_time = time.time() - real_pipeline.MIN_SOURCE_AGE_SECONDS - 60
    os.utime(path, (old_time, old_time))
    return raw


class RealPipelineTests(unittest.TestCase):
    def test_serialized_task_envelope_is_not_visible_user_text(self) -> None:
        from collections import Counter
        counts = Counter()
        text = json.dumps({
            "original_contract": {"objective": "synthetic private instruction"},
            "user_messages": ["synthetic system and orchestration instructions"],
        })
        self.assertEqual(real_pipeline._sanitize_text(text, counts),
                         "[task instructions redacted]")
        self.assertEqual(counts["orchestration_context"], 1)

    def test_claude_projection_keeps_allowlisted_text_and_redacts_sensitive_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            source = base / "synthetic-claude.jsonl"
            private_root = base / "private"
            raw = _write_closed_log(source, [
                {
                    "type": "queue-operation",
                    "sessionId": "synthetic-claude-session",
                    "operation": "synthetic-start-metadata",
                },
                {
                    "type": "queue-operation",
                    "sessionId": "synthetic-claude-session",
                    "operation": "synthetic-queued-metadata",
                },
                {
                    "type": "user",
                    "sessionId": "synthetic-claude-session",
                    "uuid": "synthetic-user-event",
                    "parentUuid": None,
                    "timestamp": "2026-10-29T00:00:00.250Z",
                    "cwd": "/home/synthetic/project",
                    "message": {
                        "role": "user",
                        "content": [{
                            "type": "text",
                            "text": (
                                "Please inspect /home/synthetic/project and contact "
                                "synthetic@example.invalid. token=synthetic-secret-value "
                                '"MAGPIE_API_KEY": "synthetic-short-secret" '
                                "'PASSWORD'='tiny-secret' \"env\":{\"CUSTOM\":\"synthetic-env-value\"} "
                                "<system-reminder>hidden system prompt</system-reminder> "
                                "```\nAGENTS.md\nSYNTHETIC_ENV=hidden\n```"
                            ),
                        }],
                    },
                },
                {
                    "type": "assistant",
                    "sessionId": "synthetic-claude-session",
                    "uuid": "synthetic-assistant-event",
                    "message": {
                        "role": "assistant",
                        "stop_reason": "end_turn",
                        "content": [
                            {"type": "text", "text": "The synthetic review is complete."},
                            {
                                "type": "tool_use",
                                "name": "Read",
                                "input": {"file_path": "/home/synthetic/private.txt"},
                            },
                        ],
                    },
                },
                {
                    "type": "user",
                    "sessionId": "synthetic-claude-session",
                    "uuid": "synthetic-tool-result-event",
                    "timestamp": "2026-10-29T00:00:02.000Z",
                    "message": {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "is_error": True,
                            "exit_code": 17,
                            "content": "Synthetic command failed at /tmp/synthetic/output",
                        }],
                    },
                },
            ])

            with patch.object(real_pipeline, "thermal_checkpoint", return_value=41_000):
                result = real_pipeline.prepare_one(
                    source,
                    harness="claude",
                    closed_confirmed=True,
                    private_root=private_root,
                )

            rows = [json.loads(line) for line in Path(result["projection_copy"]).read_text().splitlines()]
            candidate_text = Path(result["projection_copy"]).read_text()
            self.assertEqual(Path(result["raw_copy"]).read_bytes(), raw)
            self.assertEqual(len(rows), 3)
            self.assertEqual(rows[1]["projection"]["stop_reason"], "end_turn")
            self.assertEqual(rows[1]["projection"]["tool_name"], "Read")
            self.assertEqual(rows[2]["projection"]["result_status"], "error")
            self.assertEqual(rows[2]["projection"]["exit_status"], 17)
            self.assertEqual([row["arrival_ms"] for row in rows], [0.0, None, 1750.0])
            self.assertNotIn("arrival_ms", rows[0]["projection"])
            self.assertEqual(rows[0]["evidence_text"], rows[0]["projection"]["visible_text"])
            self.assertNotIn("synthetic@example.invalid", candidate_text)
            self.assertNotIn("/home/synthetic", candidate_text)
            self.assertNotIn("synthetic-secret-value", candidate_text)
            self.assertNotIn("synthetic-short-secret", candidate_text)
            self.assertNotIn("tiny-secret", candidate_text)
            self.assertNotIn("synthetic-env-value", candidate_text)
            self.assertNotIn("hidden system prompt", candidate_text)
            self.assertNotIn("SYNTHETIC_ENV", candidate_text)
            self.assertNotIn("AGENTS.md", candidate_text)
            self.assertNotIn("private.txt", candidate_text)
            self.assertNotIn("gold", rows[0])
            self.assertNotIn("timestamp", rows[0].get("context", {}))
            self.assertEqual(stat.S_IMODE(Path(result["raw_copy"]).stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(Path(result["projection_copy"]).stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(private_root.stat().st_mode), 0o700)
            first_native_line = raw.splitlines(keepends=True)[2]
            self.assertEqual(rows[0]["native_sha256"], hashlib.sha256(first_native_line).hexdigest())
            manifest = json.loads(Path(result["manifest"]).read_text())
            self.assertEqual(manifest["selection_cues"]["eligible_projected_rows"], 3)
            self.assertTrue(manifest["selection_cues"]["native_session_start_valid"])

    def test_codex_reason_and_error_are_separate_from_visible_text(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            source = base / "synthetic-codex.jsonl"
            private_root = base / "private"
            _write_closed_log(source, [
                {
                    "type": "session_meta",
                    "timestamp": "2026-10-29T00:00:00Z",
                    "payload": {"id": "synthetic-codex-session"},
                },
                {
                    "type": "response_item",
                    "timestamp": "2026-10-29T00:00:02.500Z",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "message": {
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": "Synthetic work started."}],
                        },
                    },
                },
                {
                    "type": "response_item",
                    "timestamp": "2026-10-29T00:00:03Z",
                    "payload": {
                        "type": "agent_reasoning",
                        "text": "synthetic hidden reasoning must stay out",
                    },
                },
                {
                    "type": "event_msg",
                    "timestamp": "2026-10-29T00:00:04Z",
                    "payload": {
                        "type": "error",
                        "reason": "Synthetic transport issue at /home/synthetic/workspace",
                        "error": {"message": "Authorization: Bearer synthetic-bearer-value"},
                    },
                },
                {
                    "type": "response_item",
                    "timestamp": "2026-10-29T00:00:05Z",
                    "payload": {
                        "type": "custom_tool_call_output",
                        "call_id": "synthetic-tool-call",
                        "output": "Synthetic tool output at /home/synthetic/result",
                        "exit_code": 7,
                    },
                },
            ])

            with patch.object(real_pipeline, "thermal_checkpoint", return_value=42_000):
                result = real_pipeline.prepare_one(
                    source,
                    harness="codex",
                    closed_confirmed=True,
                    private_root=private_root,
                )

            rows = [json.loads(line) for line in Path(result["projection_copy"]).read_text().splitlines()]
            self.assertEqual(len(rows), 3)
            error_projection = rows[1]["projection"]
            self.assertEqual(error_projection["event_kind"], "error")
            self.assertIn("Synthetic transport issue", error_projection["reason"])
            self.assertEqual(error_projection["error_message"], "[redacted-auth]")
            self.assertEqual(error_projection["visible_text"], "")
            self.assertNotIn("synthetic-bearer-value", Path(result["projection_copy"]).read_text())
            self.assertNotIn("synthetic hidden reasoning", Path(result["projection_copy"]).read_text())
            self.assertEqual(result["exclusion_counts"]["internal_reasoning"], 1)
            result_projection = rows[2]["projection"]
            self.assertEqual(result_projection["event_kind"], "tool_result")
            self.assertEqual(result_projection["result_status"], "error")
            self.assertEqual(result_projection["exit_status"], 7)
            self.assertIn("Synthetic tool output", result_projection["visible_text"])
            self.assertEqual(rows[0]["context"]["event_order"], 0)
            self.assertEqual(rows[1]["context"]["event_order"], 1)
            self.assertEqual(rows[2]["context"]["event_order"], 2)
            self.assertEqual([row["arrival_ms"] for row in rows], [2500.0, 4000.0, 5000.0])
            self.assertNotIn("arrival_ms", rows[0]["projection"])

    def test_uncertain_visible_payload_excludes_the_session(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            source = base / "synthetic-unknown-block.jsonl"
            _write_closed_log(source, [
                {
                    "type": "user",
                    "sessionId": "synthetic-unknown-session",
                    "parentUuid": None,
                    "message": {"role": "user", "content": [{"type": "opaque_blob", "data": "synthetic"}]},
                },
            ])
            with patch.object(real_pipeline, "thermal_checkpoint", return_value=43_000):
                with self.assertRaises(real_pipeline.PipelineError) as raised:
                    real_pipeline.prepare_one(
                        source,
                        harness="claude",
                        closed_confirmed=True,
                        private_root=base / "private",
                    )
            self.assertEqual(raised.exception.code, "session_unsanitizable")

    def test_claude_start_with_parent_is_not_treated_as_a_complete_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            source = base / "synthetic-fork.jsonl"
            _write_closed_log(source, [
                {
                    "type": "user",
                    "sessionId": "synthetic-fork-session",
                    "parentUuid": "synthetic-parent-event",
                    "message": {"role": "user", "content": "synthetic text"},
                },
            ])
            with patch.object(real_pipeline, "thermal_checkpoint", return_value=43_000):
                with self.assertRaises(real_pipeline.PipelineError) as raised:
                    real_pipeline.prepare_one(
                        source,
                        harness="claude",
                        closed_confirmed=True,
                        private_root=base / "private",
                    )
            self.assertEqual(raised.exception.code, "forked_or_partial_session")

    def test_missing_claude_ancestry_and_nested_codex_subagents_are_excluded(self) -> None:
        for harness, record, expected in (
            ("claude", {"type": "user", "sessionId": "synthetic"}, "forked_or_partial_session"),
            ("codex", {"type": "session_meta", "payload": {
                "id": "synthetic", "source": {"subagent": {"thread_spawn": {}}},
            }}, "subagent_session"),
        ):
            with self.subTest(harness=harness):
                with self.assertRaises(real_pipeline.PipelineError) as raised:
                    real_pipeline._locate_native_start([(record, b"synthetic")], harness)
                self.assertEqual(raised.exception.code, expected)

    def test_codex_custom_tool_invocation_and_task_start_keep_native_kinds(self) -> None:
        for native_kind, expected in (("custom_tool_call", "tool_use"), ("task_started", "turn_status")):
            projection = real_pipeline._project_record({
                "type": "response_item" if native_kind == "custom_tool_call" else "event_msg",
                "payload": {"type": native_kind, "name": "synthetic_tool"},
            }, "codex", Counter())
            self.assertEqual(projection["event_kind"], expected)
            self.assertTrue(projection["native_record_kind"].endswith("/" + native_kind))

    def test_prepare_one_requires_explicit_closed_source_attestation(self) -> None:
        with self.assertRaises(real_pipeline.PipelineError) as raised:
            real_pipeline.prepare_one("synthetic.jsonl", harness="claude")
        self.assertEqual(raised.exception.code, "closed_source_confirmation_required")


if __name__ == "__main__":
    unittest.main()
