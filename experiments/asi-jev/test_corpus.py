import hashlib
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "experiments" / "asi-jev" / "data"
FIXTURE_REVISION = "893dd56fdc63fd53b7febd597c1381570c01f64b"

ACTIVITIES = {
    "task_request",
    "progress",
    "tool_action",
    "wait",
    "failure",
    "recovery",
    "completion",
    "unknown",
}
FAILURE_CATEGORIES = {
    "none",
    "tool",
    "permission",
    "environment",
    "model",
    "transport",
    "malformed",
    "unknown",
}
RETRY_LOOPS = {"none", "retry", "loop"}
PHASES = {"start", "active", "blocked", "recovery", "ending", "unknown"}


def digest(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def read_jsonl(path: Path):
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


class CorpusIntegrityTests(unittest.TestCase):
    def test_fixture_rows_match_pinned_source_lines(self):
        events = read_jsonl(DATA / "events.jsonl")
        for event in events:
            if event["origin"] != "sanitized_fixture":
                continue

            source = event["source"]
            self.assertEqual(source["repository"], "onsager-ai/semon")
            self.assertEqual(source["revision"], FIXTURE_REVISION)
            original_path = ROOT / source["file"]
            source_lines = original_path.read_bytes().splitlines(keepends=True)
            line = source_lines[source["line"] - 1]
            self.assertTrue(line.endswith(b"\n"), source["file"])
            self.assertFalse(line.endswith(b"\r\n"), source["file"])
            self.assertEqual(digest(line), source["line_sha256"])

            raw = event["native"]["raw"].encode("utf-8")
            self.assertEqual(raw, line[:-1])
            self.assertEqual(digest(raw), event["native"]["raw_sha256"])

    def test_synthetic_rows_match_companion_raw_lines(self):
        events = read_jsonl(DATA / "events.jsonl")
        synthetic_lines = (DATA / "synthetic-adversarial.jsonl").read_bytes().splitlines(keepends=True)
        for event in events:
            if event["origin"] != "synthetic_adversarial":
                continue

            source = event["source"]
            self.assertEqual(source["file"], "experiments/asi-jev/data/synthetic-adversarial.jsonl")
            line = synthetic_lines[source["line"] - 1]
            self.assertTrue(line.endswith(b"\n"))
            self.assertFalse(line.endswith(b"\r\n"))
            self.assertEqual(digest(line), source["line_sha256"])

            raw = event["native"]["raw"].encode("utf-8")
            self.assertEqual(raw, line[:-1])
            self.assertEqual(digest(raw), event["native"]["raw_sha256"])

    def test_labels_and_session_order_follow_schema(self):
        events = read_jsonl(DATA / "events.jsonl")
        ids = set()
        last_order = {}
        for event in events:
            self.assertNotIn(event["id"], ids)
            ids.add(event["id"])
            self.assertIn(event["origin"], {"sanitized_fixture", "synthetic_adversarial"})
            self.assertIn(event["split"], {"development", "heldout_challenge"})
            self.assertIn(event["origin"], {"synthetic_adversarial"} if event["split"] == "heldout_challenge" else {"sanitized_fixture"})
            self.assertIsInstance(event["annotation_note"], str)
            self.assertNotIn("annotation_note", event["gold"])

            context = event["context"]
            self.assertIn("session_id", context)
            self.assertIsInstance(context["event_order"], int)
            previous = last_order.get(context["session_id"])
            if previous is not None:
                self.assertGreater(context["event_order"], previous)
            last_order[context["session_id"]] = context["event_order"]

            self.assertEqual(set(event["gold"]), {"streaming", "offline"})
            for view in event["gold"].values():
                self.assertIn(view["activity"], ACTIVITIES)
                self.assertIn(view["failure_category"], FAILURE_CATEGORIES)
                self.assertIn(view["retry_loop"], RETRY_LOOPS)
                self.assertIsInstance(view["anomaly"], bool)
                self.assertIn(view["phase"], PHASES)


if __name__ == "__main__":
    unittest.main()
