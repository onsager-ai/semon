import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).parents[1] / "scripts" / "codex_tailer.py"
SPEC = importlib.util.spec_from_file_location("codex_tailer", SCRIPT)
tailer = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(tailer)


class TailerTest(unittest.TestCase):
    def setUp(self):
        self.exported = []
        self.export_patch = mock.patch.object(
            tailer,
            "export",
            side_effect=lambda endpoint, events, timeout: self.exported.extend(events),
        )
        self.export_patch.start()

    def tearDown(self):
        self.export_patch.stop()

    def args(self, root):
        return tailer.parser().parse_args(
            [
                "--sessions",
                str(root / "sessions"),
                "--history",
                str(root / "history.jsonl"),
                "--state",
                str(root / "state.json"),
                "--endpoint",
                "http://127.0.0.1:4318/v1/logs",
            ]
        )

    def test_normalizes_and_resumes(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            sessions = root / "sessions" / "2026" / "07"
            sessions.mkdir(parents=True)
            path = sessions / "rollout-test.jsonl"
            records = [
                {
                    "timestamp": "2026-07-31T01:02:03Z",
                    "type": "session_meta",
                    "payload": {
                        "session_id": "session-1",
                        "cwd": "/work/devlog",
                        "git": {"repository_url": "git@github.com:onsager-ai/devlog.git"},
                    },
                },
                {
                    "timestamp": "2026-07-31T01:02:04Z",
                    "type": "event_msg",
                    "payload": {"type": "user_message", "message": "hello"},
                },
                {
                    "timestamp": "2026-07-31T01:02:05Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 10,
                                "output_tokens": 4,
                                "reasoning_output_tokens": 2,
                            }
                        },
                    },
                },
            ]
            path.write_text(
                "".join(json.dumps(record) + "\n" for record in records),
                encoding="utf-8",
            )
            args = self.args(root)
            self.assertEqual(tailer.run_once(args), 3)
            self.assertEqual(tailer.run_once(args), 0)
            self.assertEqual(len(self.exported), 3)
            self.assertEqual([item["kind"] for item in self.exported],
                             ["session_meta", "user_prompt", "api_request"])
            self.assertEqual(self.exported[2]["repo"], "devlog")
            self.assertEqual(self.exported[2]["tokens_in"], 10)
            self.assertEqual(self.exported[2]["tokens_out"], 6)

    def test_partial_line_waits_for_completion(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            sessions = root / "sessions"
            sessions.mkdir()
            path = sessions / "rollout-test.jsonl"
            record = {
                "timestamp": "2026-07-31T01:02:03Z",
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "hello"},
            }
            encoded = json.dumps(record)
            path.write_text(encoded, encoding="utf-8")
            args = self.args(root)
            self.assertEqual(tailer.run_once(args), 0)
            with path.open("a", encoding="utf-8") as handle:
                handle.write("\n")
            self.assertEqual(tailer.run_once(args), 1)


if __name__ == "__main__":
    unittest.main()
