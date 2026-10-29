import hashlib
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "experiments" / "asi-jev" / "data"
EVIDENCE = ROOT / "experiments" / "asi-jev" / "evidence"
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

    def test_annotation_corrections_preserve_all_other_event_fields(self):
        old_events = read_jsonl(EVIDENCE / "events-v1.jsonl")
        new_events = read_jsonl(DATA / "events.jsonl")
        corrections_path = DATA / "annotation-corrections.json"
        correction_doc = json.loads(corrections_path.read_text(encoding="utf-8"))
        self.assertEqual(
            digest((EVIDENCE / "events-v1.jsonl").read_bytes()),
            correction_doc["evidence_file"]["sha256"],
        )
        corrections = {entry["id"]: entry for entry in correction_doc["corrections"]}
        self.assertEqual(
            set(corrections),
            {
                "fixture-native-claude-parent-014-17",
                "fixture-native-claude-parent-021-18",
                "fixture-native-claude-parent-023-19",
            },
        )
        self.assertEqual(len(old_events), len(new_events))

        for old, new in zip(old_events, new_events, strict=True):
            self.assertEqual(old["id"], new["id"])
            correction = corrections.get(old["id"])
            if correction is None:
                self.assertEqual(old, new)
                continue

            self.assertEqual(correction["source"]["file"], new["source"]["file"])
            self.assertEqual(correction["source"]["line"], new["source"]["line"])
            self.assertEqual(correction["source"]["raw_sha256"], new["native"]["raw_sha256"])
            self.assertEqual(correction["old"]["gold"], old["gold"])
            self.assertEqual(correction["old"]["annotation_note"], old["annotation_note"])
            self.assertEqual(correction["new"]["gold"], new["gold"])
            self.assertEqual(correction["new"]["annotation_note"], new["annotation_note"])

            old_without_labels = {key: value for key, value in old.items() if key not in {"gold", "annotation_note"}}
            new_without_labels = {key: value for key, value in new.items() if key not in {"gold", "annotation_note"}}
            self.assertEqual(old_without_labels, new_without_labels)
            self.assertEqual(old["native"]["raw"], new["native"]["raw"])
            self.assertEqual(old["native"]["raw_sha256"], new["native"]["raw_sha256"])

            raw = json.loads(new["native"]["raw"])
            evidence = correction["source_evidence"]
            self.assertEqual(raw.get("type"), evidence["native_type"])
            if evidence["native_type"] == "attachment":
                self.assertEqual(raw["attachment"]["type"], evidence["attachment_type"])
            else:
                self.assertEqual(raw.get("operation"), evidence["operation"])


if __name__ == "__main__":
    unittest.main()
