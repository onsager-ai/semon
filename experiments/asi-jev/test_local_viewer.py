"""Synthetic tests for the private static ASI/JEv viewer generator."""

from __future__ import annotations

import json
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import local_viewer


def _write_json(path: Path, value: object) -> bytes:
    data = (json.dumps(value, ensure_ascii=True) + "\n").encode("ascii")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_bytes(data)
    os.chmod(path, 0o600)
    return data


def _write_jsonl(path: Path, rows: list[dict[str, object]]) -> bytes:
    data = b"".join((json.dumps(row, ensure_ascii=True) + "\n").encode("ascii") for row in rows)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_bytes(data)
    os.chmod(path, 0o600)
    return data


def _fixture(root: Path, *, private: bool) -> None:
    (root / "viewer").mkdir(parents=True)
    (root / "viewer" / "index.html").write_text("<!doctype html><title>Local fixture</title>\n")
    (root / "viewer" / "app.js").write_text("document.documentElement.dataset.fixture = 'yes';\n")
    (root / "viewer" / "style.css").write_text("body { color: #123; }\n")

    result_conditions: dict[str, dict[str, object]] = {}
    operation_conditions: dict[str, dict[str, object]] = {}
    improvements: dict[str, dict[str, object]] = {}
    for spec in local_viewer.CONDITIONS:
        metrics = {
            "event_count": 2,
            "session_count": 1,
            "dimensions": {dimension: {"accuracy": 0.5} for dimension in local_viewer.DIMENSIONS},
        }
        result_conditions.setdefault(spec.results_group, {})[spec.context] = {
            "metrics": metrics,
            "false_alert_budget": {"events": 2},
            "phase_boundary": {"boundaries": 1},
        }
        operation_conditions[spec.id] = {"status": "measured", "event_count": 2}
        if spec.acceptance_group:
            improvements.setdefault(spec.results_group, {})[spec.context] = {
                "estimate": 0.1,
                "event_count": 2,
                "session_count": 1,
                "event_bootstrap_ci95": [0.0, 0.2],
            }

    _write_json(root / "REAL_RESULTS.json", {
        "schema_version": 1,
        "conditions": result_conditions,
        "acceptance": {name: {"status": "pass"} for name in improvements},
        "paired_activity_macro_f1_improvement_vs_rules": improvements,
        "reference_counts": {"unique_events": 2, "sessions": 1},
    })
    _write_json(root / "REAL_OPERATIONS.json", {"schema": 1, "conditions": operation_conditions})
    trials = []
    for condition_id in local_viewer.FIRST_RUN_LABELS:
        trials.append({
            "trial": condition_id,
            "summary": {
                "event_count": 58,
                "metrics": {"overall": {"activity": {"accuracy": 0.6, "macro_f1": 0.4}}},
            },
        })
    _write_json(root / "results" / "comparison.json", {"conditions": trials})

    if not private:
        return

    private_root = root / "private"
    private_root.mkdir(mode=0o700)
    os.chmod(private_root, 0o700)
    session_id = "c" * 32
    event_ids = ("a" * 32, "b" * 32)
    golds = (
        {"activity": "start", "anomaly": False, "failure_category": "none", "phase": "start", "retry_loop": "none"},
        {"activity": "active", "anomaly": False, "failure_category": "none", "phase": "active", "retry_loop": "none"},
    )
    projections = []
    references = []
    for order, (event_id, gold) in enumerate(zip(event_ids, golds, strict=True)):
        projections.append({
            "id": event_id,
            "session_id": session_id,
            "event_order": order,
            "harness": "codex",
            "native_format": "codex-jsonl-v1",
            "arrival_ms": order * 10,
            "projection": {
                "event_kind": "user_message",
                "native_record_kind": "response_item/message",
                "visible_text": f"PRIVATE_SENTINEL_{order}",
            },
        })
        references.append({
            "id": event_id,
            "session_id": session_id,
            "event_order": order,
            "harness": "codex",
            "gold": gold,
            "causal_gold": gold,
        })
    _write_jsonl(private_root / "frozen-projection.jsonl", projections)
    _write_jsonl(private_root / "real-event-reference.jsonl", references)
    _write_jsonl(private_root / "spot-check" / "packet.jsonl", [{
        "id": event_ids[0],
        "harness": "codex",
        "context": "PRIVATE_REVIEW_SENTINEL",
        "gold": golds[0],
        "causal_gold": golds[0],
    }])

    completed = []
    for spec in local_viewer.CONDITIONS:
        run_dir = private_root / "runs" / f"20261030T000000000000Z-{spec.id}"
        prediction_rows = []
        for event_id, gold in zip(event_ids, golds, strict=True):
            for repetition in range(1, local_viewer.EXPECTED_REPETITIONS[spec.id] + 1):
                activity = (
                    "start" if event_id == event_ids[0] and repetition == 1
                    else "active" if repetition > 1
                    else None
                )
                prediction_rows.append({
                    "id": event_id,
                    "harness": "codex",
                    "repetition": repetition,
                    "predictions": {
                        **gold,
                        "activity": activity,
                    },
                })
        _write_jsonl(run_dir / "predictions.jsonl", prediction_rows)
        completed.append({"condition": spec.id, "output_dir": str(run_dir), "calls": 0, "failures": 0})
    _write_jsonl(private_root / "matrix-20261030T000000000000Z" / "completed.jsonl", completed)


class LocalViewerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.root = self.base / "experiment"
        self.root.mkdir()
        self.checkpoint = patch.object(local_viewer, "thermal_checkpoint", return_value=40_000)
        self.checkpoint.start()

    def tearDown(self) -> None:
        self.checkpoint.stop()
        self.temp.cleanup()

    def test_aggregate_only_uses_public_inputs_and_contains_no_private_sentinels(self) -> None:
        _fixture(self.root, private=False)
        output = self.base / "aggregate"
        with patch.object(local_viewer, "EXPERIMENT_ROOT", self.root), \
                patch.object(local_viewer, "_load_private_references", side_effect=AssertionError("private reference read")), \
                patch.object(local_viewer, "_load_private_predictions", side_effect=AssertionError("private prediction read")):
            local_viewer.build(output, aggregate_only=True)

        review = json.loads((output / "review.json").read_text())
        results = json.loads((output / "results.json").read_text())
        self.assertEqual(review, {"schema": 1, "aggregate_only": True, "packet_id": "aggregate-only", "events": []})
        self.assertEqual(results["counts"], {"events": 2, "sessions": 1})
        self.assertEqual(results["sessions"], [])
        self.assertEqual(results["confusion"], {})
        self.assertFalse(results["confusion_supported"])
        rendered = (output / "review.json").read_text() + (output / "results.json").read_text()
        for sentinel in ("PRIVATE_SENTINEL", "PRIVATE_REVIEW_SENTINEL", "a" * 32, "c" * 32, str(self.root / "private"), "sha256:"):
            self.assertNotIn(sentinel, rendered)

    def test_private_build_preserves_projection_order_and_averages_repetition_confusion(self) -> None:
        _fixture(self.root, private=True)
        source_files = [
            self.root / "REAL_RESULTS.json",
            self.root / "REAL_OPERATIONS.json",
            self.root / "results" / "comparison.json",
            self.root / "private" / "frozen-projection.jsonl",
            self.root / "private" / "real-event-reference.jsonl",
            self.root / "private" / "spot-check" / "packet.jsonl",
        ]
        before = {path: path.read_bytes() for path in source_files}
        output = self.root / "private" / "viewer"
        with patch.object(local_viewer, "EXPERIMENT_ROOT", self.root):
            local_viewer.build(output)

        review = json.loads((output / "review.json").read_text())
        results = json.loads((output / "results.json").read_text())
        self.assertTrue(results["complete"])
        self.assertEqual(results["counts"], {"events": 2, "sessions": 1})
        self.assertEqual(review["events"][0]["context"], "PRIVATE_REVIEW_SENTINEL")
        self.assertEqual(review["events"][0]["reference"]["activity"], "start")
        self.assertEqual(results["sessions"][0]["key"], "Session01")
        condition_rows = {row["id"]: row for row in results["conditions"]}
        self.assertIsNotNone(condition_rows["jev-prefix"]["acceptance"])
        self.assertIsNone(condition_rows["jev-offline"]["acceptance"])
        self.assertEqual(condition_rows["jev-prefix"]["improvement"]["event_bootstrap_ci95"], [0.0, 0.2])
        self.assertEqual(results["first_run"]["conditions"][0]["id"], "rules-offline")
        self.assertEqual(
            {row["id"] for row in results["first_run"]["conditions"]},
            {"rules-offline", "rules-prefix", "jev-triggered-prefix", "jev-microbatch-prefix",
             "jev-prefix", "llm-prefix", "jev-offline", "llm-offline"},
        )
        first, second = results["sessions"][0]["events"]
        self.assertEqual([first["order"], second["order"]], [0, 1])
        self.assertEqual(first["context"], '{"event_kind":"user_message","native_record_kind":"response_item/message","visible_text":"PRIVATE_SENTINEL_0"}')
        self.assertEqual(len(first["predictions"]["rules-prefix"]), 1)
        activity = results["confusion"]["jev-prefix"]["activity"]
        self.assertEqual(activity["row_labels"], ["start", "active"])
        self.assertEqual(activity["labels"], ["start", "active", "<abstain>"])
        self.assertAlmostEqual(activity["matrix"][0][0], 1.0 / 3)
        self.assertAlmostEqual(activity["matrix"][0][1], 2.0 / 3)
        self.assertAlmostEqual(activity["matrix"][1][1], 2.0 / 3)
        self.assertAlmostEqual(activity["matrix"][1][2], 1.0 / 3)
        anomaly = results["confusion"]["rules-prefix"]["anomaly"]
        self.assertEqual(anomaly["row_labels"], ["false"])
        self.assertEqual(anomaly["matrix"], [[2.0, 0.0]])
        self.assertEqual({path: path.read_bytes() for path in source_files}, before)
        self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
        for path in output.iterdir():
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_missing_condition_is_explicit_and_makes_private_output_incomplete(self) -> None:
        _fixture(self.root, private=True)
        completed_path = next((self.root / "private").glob("matrix-*/completed.jsonl"))
        records = [json.loads(line) for line in completed_path.read_text().splitlines()]
        records = [row for row in records if row["condition"] != "rules-offline"]
        _write_jsonl(completed_path, records)
        output = self.root / "private" / "viewer"
        with patch.object(local_viewer, "EXPERIMENT_ROOT", self.root):
            local_viewer.build(output)
        results = json.loads((output / "results.json").read_text())
        condition = next(row for row in results["conditions"] if row["id"] == "rules-offline")
        self.assertFalse(results["complete"])
        self.assertFalse(condition["available"])
        self.assertEqual(condition["unavailable"], "saved_condition_missing")
        self.assertIn("rules-offline", results["unavailable_conditions"])

    def test_cli_rejects_aggregate_output_outside_private_before_building(self) -> None:
        with patch.object(local_viewer, "EXPERIMENT_ROOT", self.root), \
                patch.object(local_viewer, "build") as builder:
            self.assertEqual(local_viewer.main([
                "--aggregate-only", "--output", str(self.base / "public-site"),
            ]), 2)
            builder.assert_not_called()

    def test_safe_output_and_size_limits_reject_source_overwrite(self) -> None:
        _fixture(self.root, private=False)
        source = self.root / "REAL_RESULTS.json"
        before = source.read_bytes()
        with patch.object(local_viewer, "EXPERIMENT_ROOT", self.root):
            with self.assertRaises(local_viewer.ViewerError):
                local_viewer.build(self.root)
            with self.assertRaises(local_viewer.ViewerError):
                local_viewer.build(self.root / "private")
        self.assertEqual(source.read_bytes(), before)

        oversized_root = self.base / "oversized"
        oversized_root.mkdir()
        oversized = oversized_root / "large.json"
        with oversized.open("wb") as target:
            target.truncate(local_viewer.MAX_FILE_BYTES)
        with self.assertRaises(local_viewer.ViewerError):
            local_viewer._read_bounded(oversized, base=oversized_root)


if __name__ == "__main__":
    unittest.main()
