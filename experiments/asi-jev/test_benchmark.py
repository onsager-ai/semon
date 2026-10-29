import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from benchmark import (
    ABSTAIN,
    DIMENSIONS,
    BenchmarkError,
    Event,
    ProviderFailure,
    _metric_for,
    build_microbatches,
    context_events,
    extract_native_evidence,
    format_context,
    load_events,
    make_jev_request,
    parse_jev_response,
    phase_transition_metrics,
    rule_classify,
)


def _gold(activity="progress", failure_category="none", retry_loop="none", anomaly=False, phase="active"):
    return {"activity": activity, "failure_category": failure_category, "retry_loop": retry_loop, "anomaly": anomaly, "phase": phase}


def _event(index, *, session="s1", order=None, timestamp=None, kind="assistant", text="visible", gold=None):
    return Event(
        index=index,
        event_id=f"event-{index}",
        split="development",
        origin="sanitized_fixture",
        harness="claude-code",
        native_format="claude-code-jsonl-transcript",
        native_sha256=hashlib.sha256(f"raw-{index}".encode()).hexdigest(),
        session_id=session,
        event_order=index + 1 if order is None else order,
        timestamp=timestamp,
        gold={"streaming": _gold(), "offline": _gold()},
        event_kind=kind,
        evidence_text=text,
        malformed_native=False,
        oversized_native=False,
    )


def _row(raw, *, raw_hash=None):
    return {
        "id": "native-1",
        "split": "development",
        "origin": "sanitized_fixture",
        "source": {"file": "fixture.jsonl", "line": 1},
        "context": {"session_id": "session-1", "event_order": 3, "timestamp": "2026-10-03T13:21:03.003Z"},
        "native": {"harness": "claude-code", "format": "claude-code-jsonl-transcript", "raw": raw,
                   "raw_sha256": raw_hash or hashlib.sha256(raw.encode()).hexdigest()},
        "gold": {"streaming": _gold("task_request", phase="start"), "offline": _gold("task_request", phase="start")},
        "annotation_note": "NEVER_SEND_annotation",
    }


class BenchmarkTests(unittest.TestCase):
    def test_native_hash_is_checked_and_causal_prompt_excludes_metadata(self):
        raw = '{"type":"user","message":{"role":"user","content":"VISIBLE_USER_TEXT"}}'
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "events.jsonl"
            path.write_text(json.dumps(_row(raw), separators=(",", ":")) + "\n", encoding="utf-8")
            events, _ = load_events(path)
            event = events[0]
            self.assertEqual(event.native_sha256, hashlib.sha256(raw.encode()).hexdigest())
            context = format_context(event, events, "prefix", 8192)
            request, _ = make_jev_request(events, {event.index: context}, "jev-latest")
            request_text = json.dumps(request)
            self.assertIn("VISIBLE_USER_TEXT", request_text)
            self.assertNotIn("NEVER_SEND", request_text)
            self.assertNotIn("fixture.jsonl", request_text)

            bad = _row(raw, raw_hash="0" * 64)
            path.write_text(json.dumps(bad) + "\n", encoding="utf-8")
            with self.assertRaises(BenchmarkError):
                load_events(path)

    def test_only_allowlisted_roles_and_bounded_tool_results_are_extracted(self):
        system_raw = json.dumps({"type": "assistant", "message": {"role": "system", "content": "SECRET_POLICY"}})
        _, system_text, _, _ = extract_native_evidence(system_raw, "claude-code", "claude-code-jsonl-transcript")
        self.assertEqual(system_text, "")

        result_raw = json.dumps({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "call-1", "is_error": True, "content": "Exit code 1: command failed"}
        ]}})
        kind, text, malformed, _ = extract_native_evidence(result_raw, "claude-code", "claude-code-jsonl-transcript")
        self.assertIn("tool_result", kind)
        self.assertIn("call-1", text)
        self.assertIn("Exit code 1", text)
        self.assertFalse(malformed)
        tool_event = _event(0, order=14, kind=kind, text=text)
        prediction = rule_classify(tool_event, [tool_event])
        self.assertEqual(prediction["activity"], "failure")
        self.assertEqual(prediction["failure_category"], "tool")

    def test_prefix_and_offline_context_are_session_scoped_and_keep_target_under_cap(self):
        first = _event(0, order=3, text="FIRST")
        target = _event(1, order=14, text="TARGET_TEXT" * 30)
        future = _event(2, order=20, text="FUTURE_SECRET")
        other_session = _event(3, session="s2", order=1, text="OTHER_SESSION")
        events = [first, target, future, other_session]
        self.assertEqual([item.index for item in context_events(target, events, "prefix")], [0, 1])
        self.assertEqual([item.index for item in context_events(target, events, "offline")], [0, 1, 2])
        prefix = format_context(target, events, "prefix", 90)
        offline = format_context(target, events, "offline", 90)
        self.assertLessEqual(len(prefix.encode("utf-8")), 90)
        self.assertIn("TARGET", prefix)
        self.assertNotIn("FUTURE_SECRET", prefix)
        self.assertLessEqual(len(offline.encode("utf-8")), 90)
        self.assertIn("TARGET", offline)
        self.assertNotIn("OTHER_SESSION", offline)

    def test_offline_cap_keeps_target_in_native_order_with_newest_whole_future_row(self):
        events = [
            _event(0, order=3, text="BEFORE_1_" + "a" * 40),
            _event(1, order=14, text="BEFORE_2_" + "b" * 40),
            _event(2, order=20, text="TARGET_MIDDLE_" + "c" * 40),
            _event(3, order=29, text="FUTURE_1_" + "d" * 40),
            _event(4, order=37, text="FUTURE_SENTINEL_" + "e" * 40),
        ]
        context = format_context(events[2], events, "offline", 400)
        self.assertLessEqual(len(context.encode("utf-8")), 400)
        self.assertIn("omitted", context)
        self.assertIn("TARGET_MIDDLE_", context)
        self.assertIn("FUTURE_SENTINEL_", context)
        self.assertLess(context.index("TARGET_MIDDLE_"), context.index("FUTURE_SENTINEL_"))

    def test_microbatches_split_by_session_size_and_timestamp_window(self):
        events = [
            _event(0, session="s1", timestamp=0.0),
            _event(1, session="s1", timestamp=0.1),
            _event(2, session="s1", timestamp=1.0),
            _event(3, session="s2", timestamp=0.05),
        ]
        batches = build_microbatches(events, batch_size=2, window_ms=500)
        self.assertEqual([[event.index for event in batch] for batch, _ in batches], [[0, 1], [2], [3]])
        self.assertAlmostEqual(batches[0][1][0], 100.0)
        self.assertEqual(batches[0][1][1], 0.0)
        self.assertAlmostEqual(batches[1][1][2], 500.0)

    def test_retry_loop_requires_three_matching_events_and_completion_stays_ending(self):
        first = _event(0, order=3, kind="assistant:tool_use", text="[sanitized tool command: npm test]")
        second = _event(1, order=14, kind="assistant:tool_use", text="[sanitized tool command: npm test]")
        third = _event(2, order=21, kind="assistant:tool_use", text="[sanitized tool command: npm test]")
        self.assertEqual(rule_classify(second, [first, second])["retry_loop"], "none")
        self.assertEqual(rule_classify(third, [first, second, third])["retry_loop"], "loop")

        failed = _event(3, order=28, kind="tool_result", text="[tool exit_code: 1] failed")
        completed = _event(4, order=35, kind="assistant:completion", text="Task completed")
        prediction = rule_classify(completed, [first, second, third, failed, completed])
        self.assertEqual(prediction["activity"], "completion")
        self.assertEqual(prediction["phase"], "ending")

    def test_first_user_event_is_start_even_with_source_line_order(self):
        event = _event(0, order=3, kind="user", text="Please inspect this")
        prediction = rule_classify(event, [event])
        self.assertEqual(prediction["activity"], "task_request")
        self.assertEqual(prediction["phase"], "start")

    def test_jev_response_is_strict_and_missing_outputs_are_not_fallbacks(self):
        with self.assertRaises(ProviderFailure):
            parse_jev_response({"model": "jev-1.14.0"}, {"q": (0, "activity")})

        choices = {str(label): 1.0 / len(DIMENSIONS["activity"]) for label in DIMENSIONS["activity"]}
        response = {
            "model": "jev-1.14.0",
            "answers": {
                "activity": {"type": "choice", "choice": "progress", "confidence": 0.7, "probabilities": choices},
                "bad_retry": {"type": "choice", "choice": "loop", "confidence": 0.8},
            },
            "usage": {"input_tokens": 12, "output_tokens": 5},
        }
        predictions, errors, details, model, inputs, outputs = parse_jev_response(
            response, {"activity": (0, "activity"), "missing": (0, "phase"), "bad_retry": (0, "retry_loop")}
        )
        self.assertEqual(predictions[0]["activity"], "progress")
        self.assertIn("missing_answer", errors[0].values())
        self.assertEqual(errors[0]["retry_loop"], "invalid_answer")
        self.assertEqual(details[0]["activity"]["confidence"], 0.7)
        self.assertEqual(model, "jev-1.14.0")
        self.assertEqual((inputs, outputs), (12, 5))

    def test_abstention_counts_as_wrong_and_is_explicit_in_confusion(self):
        rows = [
            {"gold": _gold(activity="task_request"), "predictions": {"activity": None}},
            {"gold": _gold(activity="tool_action"), "predictions": {"activity": "tool_action"}},
        ]
        metrics = _metric_for(rows, "activity")
        self.assertEqual(metrics["count"], 2)
        self.assertEqual(metrics["accuracy"], 0.5)
        self.assertEqual(metrics["coverage"], 0.5)
        self.assertEqual(metrics["confusion"]["task_request"][ABSTAIN], 1)

    def test_phase_transition_boundary_scores_adjacent_events_per_session(self):
        first = _event(0, order=3)
        second = _event(1, order=14)
        rows = [
            {"event_index": 0, "repetition": 1, "split": "development", "origin": "sanitized_fixture", "harness": "claude-code",
             "gold": _gold(phase="start"), "predictions": {"phase": "start"}},
            {"event_index": 1, "repetition": 1, "split": "development", "origin": "sanitized_fixture", "harness": "claude-code",
             "gold": _gold(phase="active"), "predictions": {"phase": "active"}},
        ]
        metric = phase_transition_metrics(rows, [first, second])["overall"]
        self.assertEqual(metric["tp"], 1)
        self.assertEqual(metric["recall"], 1.0)
        self.assertEqual(metric["coverage"], 1.0)


if __name__ == "__main__":
    unittest.main()
