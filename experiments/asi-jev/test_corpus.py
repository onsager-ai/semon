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
                if view["phase"] == "ending":
                    self.assertEqual(event["origin"], "synthetic_adversarial")
                    self.assertEqual(context["session_scope"], "complete_session")
                if event["origin"] == "sanitized_fixture":
                    self.assertNotEqual(view["phase"], "ending")

            native = event["native"]
            try:
                native_root = json.loads(native["raw"])
            except json.JSONDecodeError:
                native_root = None
            if isinstance(native_root, dict) and native_root.get("type") == "future_event":
                for view in event["gold"].values():
                    self.assertEqual(view["activity"], "unknown")
                    self.assertEqual(view["failure_category"], "none")
                    self.assertTrue(view["anomaly"])

    def test_annotation_corrections_preserve_all_other_event_fields(self):
        versions = {
            "v1": read_jsonl(EVIDENCE / "events-v1.jsonl"),
            "v2": read_jsonl(EVIDENCE / "events-v2.jsonl"),
            "v3": read_jsonl(DATA / "events.jsonl"),
        }
        corrections_path = DATA / "annotation-corrections.json"
        correction_doc = json.loads(corrections_path.read_text(encoding="utf-8"))
        self.assertIn("no label was adjusted using model predictions", correction_doc["correction_basis"])
        for evidence_file in correction_doc["evidence_files"]:
            evidence_path = ROOT / evidence_file["path"]
            self.assertEqual(digest(evidence_path.read_bytes()), evidence_file["sha256"])

        corrections = {entry["id"]: entry for entry in correction_doc["corrections"]}
        by_version = {
            version: {event["id"]: event for event in events}
            for version, events in versions.items()
        }
        ids = list(by_version["v1"])
        self.assertEqual(ids, list(by_version["v2"]))
        self.assertEqual(ids, list(by_version["v3"]))

        changed_ids = set()
        for before_name, after_name in (("v1", "v2"), ("v2", "v3")):
            for event_id in ids:
                before = by_version[before_name][event_id]
                after = by_version[after_name][event_id]
                before_other = {k: v for k, v in before.items() if k not in {"gold", "annotation_note"}}
                after_other = {k: v for k, v in after.items() if k not in {"gold", "annotation_note"}}
                self.assertEqual(before_other, after_other)
                if (before["gold"], before["annotation_note"]) != (after["gold"], after["annotation_note"]):
                    changed_ids.add(event_id)
                    self.assertIn(event_id, corrections)

        self.assertEqual(changed_ids, set(corrections))
        for event_id, correction in corrections.items():
            for version in ("v1", "v2", "v3"):
                event = by_version[version][event_id]
                self.assertEqual(event["gold"], correction[version]["gold"])
                self.assertEqual(event["annotation_note"], correction[version]["annotation_note"])
            final = by_version["v3"][event_id]
            self.assertEqual(correction["source"]["file"], final["source"]["file"])
            self.assertEqual(correction["source"]["line"], final["source"]["line"])
            self.assertEqual(correction["source"]["raw_sha256"], final["native"]["raw_sha256"])

            raw = json.loads(final["native"]["raw"])
            evidence = correction["source_evidence"]
            self.assertEqual(raw.get("type"), evidence["root_type"])
            if "payload_type" in evidence:
                self.assertEqual(raw["payload"]["type"], evidence["payload_type"])
            if "attachment_type" in evidence:
                self.assertEqual(raw["attachment"]["type"], evidence["attachment_type"])
            if "operation" in evidence:
                self.assertEqual(raw["operation"], evidence["operation"])
            if "message_stop_reason" in evidence:
                self.assertEqual(raw["message"]["stop_reason"], evidence["message_stop_reason"])


if __name__ == "__main__":
    unittest.main()
