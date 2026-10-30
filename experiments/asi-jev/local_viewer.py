#!/usr/bin/env python3
"""Build a private, local-only static viewer from saved ASI/JEv artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from real_pipeline import PipelineError, thermal_checkpoint


EXPERIMENT_ROOT = Path(__file__).resolve().parent
MAX_FILE_BYTES = 10_000_000
DIMENSIONS = ("activity", "anomaly", "failure_category", "phase", "retry_loop")
PROJECTION_KEYS = {
    "event_kind", "native_record_kind", "role", "tool_name", "result_status",
    "exit_status", "stop_reason", "reason", "error_message", "visible_text",
    "evidence_text",
}
OUTPUT_FILES = {"index.html", "app.js", "style.css", "review.json", "results.json"}
SAFE_CONDITION_ID = re.compile(r"[a-z0-9-]{1,80}\Z")


@dataclass(frozen=True)
class Condition:
    id: str
    label: str
    context: str
    family: str
    results_group: str
    acceptance_group: str | None


CONDITIONS = (
    Condition("rules-prefix", "Rules", "prefix", "rules", "Rules", None),
    Condition("rules-offline", "Rules", "offline", "rules", "Rules", None),
    Condition("jev-prefix", "Jev per-event", "prefix", "jev", "Jev per-event", "Jev per-event"),
    Condition("jev-microbatch-prefix", "Jev microbatch", "prefix", "jev", "Jev microbatch", "Jev microbatch"),
    Condition("jev-triggered-prefix", "Rules + triggered Jev", "prefix", "jev", "Rules+triggeredJev", "Rules+triggeredJev"),
    Condition("jev-offline", "Jev per-event", "offline", "jev", "Jev per-event", "Jev per-event"),
    Condition("llm-prefix", "Haiku per-event", "prefix", "haiku", "Haiku per-event", "Haiku per-event"),
    Condition("llm-triggered-prefix", "Rules + triggered Haiku", "prefix", "haiku", "Rules+triggeredHaiku", "Rules+triggeredHaiku"),
    Condition("llm-offline", "Haiku per-event", "offline", "haiku", "Haiku per-event", "Haiku per-event"),
)
EXPECTED_REPETITIONS = {spec.id: (1 if spec.family == "rules" else 3) for spec in CONDITIONS}

FIRST_RUN_LABELS = {
    "ci-rules-per-event-offline": "Rules per-event (offline)",
    "ci-rules-per-event-prefix": "Rules per-event (prefix)",
    "matrix-20261029-jev-event-triggered-prefix": "Jev event-triggered (prefix)",
    "matrix-20261029-jev-microbatch-prefix": "Jev microbatch (prefix)",
    "matrix-20261029-jev-per-event-prefix": "Jev per-event (prefix)",
    "matrix-20261029-llm-per-event-prefix": "Haiku per-event (prefix)",
    "ordered-20261029-jev-per-event-offline": "Jev per-event (offline)",
    "ordered-20261029-llm-per-event-offline": "Haiku per-event (offline)",
}

FIRST_RUN_IDS = {
    "ci-rules-per-event-offline": "rules-offline",
    "ci-rules-per-event-prefix": "rules-prefix",
    "matrix-20261029-jev-event-triggered-prefix": "jev-triggered-prefix",
    "matrix-20261029-jev-microbatch-prefix": "jev-microbatch-prefix",
    "matrix-20261029-jev-per-event-prefix": "jev-prefix",
    "matrix-20261029-llm-per-event-prefix": "llm-prefix",
    "ordered-20261029-jev-per-event-offline": "jev-offline",
    "ordered-20261029-llm-per-event-offline": "llm-offline",
}


class ViewerError(ValueError):
    """A safe, concise error for invalid viewer inputs or output paths."""


def _is_within(path: Path, parent: Path) -> bool:
    return path == parent or parent in path.parents


def _overlaps(path: Path, protected: Path) -> bool:
    return _is_within(path, protected) or _is_within(protected, path)


def _absolute(path: Path) -> Path:
    return Path(os.path.abspath(path))


def _check_components(path: Path, *, base: Path) -> Path:
    base = _absolute(base)
    path = _absolute(path)
    try:
        relative = path.relative_to(base)
    except ValueError:
        raise ViewerError("input_outside_allowed_root") from None

    current = base
    try:
        base_info = current.lstat()
    except OSError:
        raise ViewerError("input_root_invalid") from None
    if stat.S_ISLNK(base_info.st_mode) or not stat.S_ISDIR(base_info.st_mode):
        raise ViewerError("input_root_invalid")

    for index, part in enumerate(relative.parts):
        current = current / part
        try:
            info = current.lstat()
        except OSError:
            raise ViewerError("input_missing") from None
        if stat.S_ISLNK(info.st_mode):
            raise ViewerError("input_symlink")
        if index < len(relative.parts) - 1 and not stat.S_ISDIR(info.st_mode):
            raise ViewerError("input_path_invalid")
    return path


def _read_bounded(path: Path, *, base: Path) -> bytes:
    path = _check_components(path, base=base)
    thermal_checkpoint()
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError:
        raise ViewerError("input_unreadable") from None
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise ViewerError("input_not_regular")
        if before.st_size >= MAX_FILE_BYTES:
            raise ViewerError("input_too_large")
        with os.fdopen(fd, "rb", closefd=False) as source:
            data = source.read(MAX_FILE_BYTES)
        after = os.fstat(fd)
    finally:
        os.close(fd)
    thermal_checkpoint()
    if len(data) != before.st_size or (after.st_size, after.st_mtime_ns) != (before.st_size, before.st_mtime_ns):
        raise ViewerError("input_changed_during_read")
    return data


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ViewerError("duplicate_json_key")
        result[key] = value
    return result


def _parse_json(data: bytes) -> Any:
    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise ViewerError("invalid_json") from None


def _parse_jsonl(data: bytes) -> list[dict[str, Any]]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        raise ViewerError("invalid_jsonl") from None
    rows: list[dict[str, Any]] = []
    for line in text.splitlines():
        if not line.strip():
            raise ViewerError("invalid_jsonl")
        value = _parse_json(line.encode("utf-8"))
        if not isinstance(value, dict):
            raise ViewerError("invalid_jsonl")
        rows.append(value)
    return rows


def _read_json(path: Path, *, base: Path) -> Any:
    return _parse_json(_read_bounded(path, base=base))


def _read_jsonl(path: Path, *, base: Path) -> list[dict[str, Any]]:
    return _parse_jsonl(_read_bounded(path, base=base))


def _number(value: Any) -> int | float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return value if value == value and value not in (float("inf"), float("-inf")) else None


def _first_run(comparison: dict[str, Any]) -> dict[str, Any]:
    conditions = comparison.get("conditions")
    if not isinstance(conditions, list):
        raise ViewerError("first_run_conditions_invalid")
    output: list[dict[str, Any]] = []
    event_counts: list[int] = []
    for row in conditions:
        if not isinstance(row, dict):
            raise ViewerError("first_run_condition_invalid")
        condition_id = row.get("trial")
        if not isinstance(condition_id, str) or not SAFE_CONDITION_ID.fullmatch(condition_id):
            raise ViewerError("first_run_condition_invalid")
        summary = row.get("summary")
        metrics = summary.get("metrics") if isinstance(summary, dict) else None
        overall = metrics.get("overall") if isinstance(metrics, dict) else None
        activity = overall.get("activity") if isinstance(overall, dict) else None
        if not isinstance(activity, dict):
            raise ViewerError("first_run_metrics_invalid")
        event_count = summary.get("event_count")
        if isinstance(event_count, bool) or not isinstance(event_count, int) or event_count < 0:
            raise ViewerError("first_run_event_count_invalid")
        event_counts.append(event_count)
        output.append({
            "id": FIRST_RUN_IDS.get(condition_id, condition_id),
            "label": FIRST_RUN_LABELS.get(condition_id, condition_id),
            "activity_accuracy": _number(activity.get("accuracy")),
            "activity_macro_f1": _number(activity.get("macro_f1")),
        })
    if not event_counts or len(set(event_counts)) != 1:
        raise ViewerError("first_run_event_count_mismatch")
    return {"events": event_counts[0], "sessions": 14, "conditions": output}


def _condition_metadata(
    real_results: dict[str, Any], operations: dict[str, Any],
) -> tuple[list[dict[str, Any]], bool]:
    result_groups = real_results.get("conditions")
    operation_groups = operations.get("conditions")
    acceptance = real_results.get("acceptance")
    improvements = real_results.get("paired_activity_macro_f1_improvement_vs_rules")
    if not isinstance(result_groups, dict) or not isinstance(operation_groups, dict):
        raise ViewerError("aggregate_conditions_invalid")
    acceptance = acceptance if isinstance(acceptance, dict) else {}
    improvements = improvements if isinstance(improvements, dict) else {}

    output: list[dict[str, Any]] = []
    complete = True
    for spec in CONDITIONS:
        group = result_groups.get(spec.results_group)
        mode_data = group.get(spec.context) if isinstance(group, dict) else None
        metrics = mode_data.get("metrics") if isinstance(mode_data, dict) else None
        dimensions = metrics.get("dimensions") if isinstance(metrics, dict) else None
        operation = operation_groups.get(spec.id)
        available = isinstance(dimensions, dict) and isinstance(operation, dict)
        unavailable = None if available else "public_aggregate_missing"
        if not available:
            complete = False
        paired_group = improvements.get(spec.results_group)
        paired_mode = paired_group.get(spec.context) if isinstance(paired_group, dict) else None
        output.append({
            "id": spec.id,
            "label": spec.label,
            "context": spec.context,
            "family": spec.family,
            "dimensions": dimensions if isinstance(dimensions, dict) else {},
            "acceptance": (
                acceptance.get(spec.acceptance_group)
                if spec.acceptance_group and spec.context == "prefix"
                else None
            ),
            "operations": operation if isinstance(operation, dict) else {},
            "false_alert_budget": mode_data.get("false_alert_budget") if isinstance(mode_data, dict) else None,
            "phase_boundary": mode_data.get("phase_boundary") if isinstance(mode_data, dict) else None,
            "improvement": paired_mode if isinstance(paired_mode, dict) else None,
            "available": available,
            "unavailable": unavailable,
        })
    return output, complete


def _validate_gold(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != set(DIMENSIONS):
        raise ViewerError("reference_labels_invalid")
    for label in value.values():
        if label is not None and not isinstance(label, (str, bool)):
            raise ViewerError("reference_label_invalid")
    return value


def _validate_projection(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or not set(value).issubset(PROJECTION_KEYS):
        raise ViewerError("projection_invalid")
    if not {"event_kind", "native_record_kind"}.issubset(value):
        raise ViewerError("projection_invalid")
    for field_value in value.values():
        if field_value is not None and not isinstance(field_value, (str, int, float, bool)):
            raise ViewerError("projection_invalid")
        if isinstance(field_value, str) and len(field_value) > 1_200:
            raise ViewerError("projection_invalid")
    return value


def _private_root(root: Path) -> Path:
    private = root / "private"
    try:
        info = private.lstat()
    except OSError:
        raise ViewerError("private_root_missing") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise ViewerError("private_root_invalid")
    return private


def _load_private_references(root: Path) -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]], bytes]:
    private = _private_root(root)
    projection_rows = _read_jsonl(private / "frozen-projection.jsonl", base=private)
    reference_rows = _read_jsonl(private / "real-event-reference.jsonl", base=private)
    packet_path = private / "spot-check" / "packet.jsonl"
    packet_bytes = _read_bounded(packet_path, base=private)
    packet_rows = _parse_jsonl(packet_bytes)
    if not projection_rows or not reference_rows:
        raise ViewerError("frozen_references_empty")

    events: dict[str, dict[str, Any]] = {}
    sessions: dict[str, dict[str, Any]] = {}
    for row in projection_rows:
        event_id = row.get("id")
        session_id = row.get("session_id")
        order = row.get("event_order")
        harness = row.get("harness")
        if (not isinstance(event_id, str) or not isinstance(session_id, str)
                or not session_id or isinstance(order, bool) or not isinstance(order, int) or order < 0
                or harness not in {"claude", "codex"}):
            raise ViewerError("projection_identity_invalid")
        if event_id in events:
            raise ViewerError("duplicate_projection_id")
        projection = _validate_projection(row.get("projection"))
        events[event_id] = {
            "id": event_id, "session_id": session_id, "order": order,
            "harness": harness, "context": projection,
        }
        session = sessions.setdefault(session_id, {"harness": harness, "events": []})
        if session["harness"] != harness or any(item["order"] == order for item in session["events"]):
            raise ViewerError("projection_session_mismatch")
        session["events"].append(events[event_id])

    references: dict[str, dict[str, Any]] = {}
    for row in reference_rows:
        event_id = row.get("id")
        if not isinstance(event_id, str) or event_id in references:
            raise ViewerError("reference_identity_invalid")
        references[event_id] = row
    if set(events) != set(references):
        raise ViewerError("projection_reference_coverage_mismatch")

    for event_id, event in events.items():
        reference = references[event_id]
        if (reference.get("session_id") != event["session_id"]
                or reference.get("event_order") != event["order"]
                or reference.get("harness") != event["harness"]):
            raise ViewerError("projection_reference_mismatch")
        event["reference"] = _validate_gold(reference.get("gold"))
        event["causal_reference"] = _validate_gold(reference.get("causal_gold"))

    review_events: list[dict[str, Any]] = []
    seen_review_ids: set[str] = set()
    for row in packet_rows:
        event_id = row.get("id")
        harness = row.get("harness")
        context = row.get("context")
        causal_gold = _validate_gold(row.get("causal_gold"))
        if (not isinstance(event_id, str) or event_id in seen_review_ids
                or event_id not in events or harness != events[event_id]["harness"]
                or not isinstance(context, str)):
            raise ViewerError("review_packet_invalid")
        if causal_gold != events[event_id]["causal_reference"]:
            raise ViewerError("review_reference_mismatch")
        seen_review_ids.add(event_id)
        review_events.append({"id": event_id, "harness": harness, "context": context, "reference": causal_gold})

    for session in sessions.values():
        session["events"].sort(key=lambda item: item["order"])
    return review_events, events, packet_bytes


def _latest_matrix(private: Path) -> Path | None:
    candidates: list[Path] = []
    try:
        entries = list(private.iterdir())
    except OSError:
        raise ViewerError("private_root_unreadable") from None
    for entry in entries:
        if not entry.name.startswith("matrix-"):
            continue
        try:
            info = entry.lstat()
        except OSError:
            continue
        if stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode):
            candidates.append(entry)
    return max(candidates, key=lambda path: path.name) if candidates else None


def _run_directory(raw_path: Any, *, root: Path, private: Path) -> Path:
    if not isinstance(raw_path, str) or not raw_path or len(raw_path) > 4_096:
        raise ViewerError("saved_run_invalid")
    runs = private / "runs"
    try:
        runs_info = runs.lstat()
    except OSError:
        raise ViewerError("saved_run_invalid") from None
    if stat.S_ISLNK(runs_info.st_mode) or not stat.S_ISDIR(runs_info.st_mode):
        raise ViewerError("saved_run_invalid")
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        candidate = root / candidate
    candidate = _absolute(candidate)
    checked = _check_components(candidate, base=runs)
    try:
        info = checked.lstat()
    except OSError:
        raise ViewerError("saved_run_invalid") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise ViewerError("saved_run_invalid")
    return checked


def _prediction_label(value: Any) -> str | bool | None:
    if value is None or isinstance(value, bool):
        return value
    if isinstance(value, str) and len(value) <= 256:
        return value
    raise ViewerError("prediction_labels_invalid")


def _read_condition_predictions(
    run_dir: Path,
    *,
    runs_root: Path,
    expected_events: dict[str, dict[str, Any]],
    expected_repetitions: int,
) -> tuple[dict[str, list[dict[str, Any]]], str | None, int]:
    try:
        rows = _read_jsonl(run_dir / "predictions.jsonl", base=runs_root)
    except ViewerError:
        return {}, "predictions_unreadable", expected_repetitions
    grouped: dict[str, dict[int, dict[str, Any]]] = {}
    issue: str | None = None
    for row in rows:
        event_id = row.get("id")
        if not isinstance(event_id, str) or event_id not in expected_events:
            issue = issue or "prediction_event_mismatch"
            continue
        if row.get("harness") != expected_events[event_id]["harness"]:
            issue = issue or "prediction_harness_mismatch"
            continue
        repetition = row.get("repetition")
        if (isinstance(repetition, bool) or not isinstance(repetition, int)
                or not 1 <= repetition <= expected_repetitions):
            issue = issue or "prediction_repetition_invalid"
            continue
        raw_labels = row.get("predictions")
        if not isinstance(raw_labels, dict) or set(raw_labels) != set(DIMENSIONS):
            issue = issue or "prediction_labels_invalid"
            continue
        try:
            labels = {dimension: _prediction_label(raw_labels[dimension]) for dimension in DIMENSIONS}
        except ViewerError:
            issue = issue or "prediction_labels_invalid"
            continue
        values = grouped.setdefault(event_id, {})
        if repetition in values:
            issue = issue or "duplicate_prediction"
            continue
        values[repetition] = labels

    expected_repetition_numbers = set(range(1, expected_repetitions + 1))
    output: dict[str, list[dict[str, Any]]] = {}
    for event_id in expected_events:
        event_predictions = grouped.get(event_id, {})
        if set(event_predictions) != expected_repetition_numbers:
            issue = issue or "prediction_coverage_mismatch"
        output[event_id] = [
            {"repetition": repetition, "labels": event_predictions[repetition]}
            for repetition in sorted(event_predictions)
        ]
    if set(grouped) != set(expected_events):
        issue = issue or "prediction_coverage_mismatch"
    return output, issue, expected_repetitions


def _load_private_predictions(
    root: Path, events: dict[str, dict[str, Any]],
) -> tuple[dict[str, dict[str, list[dict[str, Any]]]], dict[str, str | None], dict[str, int]]:
    private = _private_root(root)
    all_predictions: dict[str, dict[str, list[dict[str, Any]]]] = {spec.id: {} for spec in CONDITIONS}
    statuses: dict[str, str | None] = {spec.id: "saved_condition_missing" for spec in CONDITIONS}
    repetition_counts: dict[str, int] = dict(EXPECTED_REPETITIONS)
    latest = _latest_matrix(private)
    if latest is None:
        return all_predictions, statuses, repetition_counts
    completed_path = latest / "completed.jsonl"
    try:
        completed_rows = _read_jsonl(completed_path, base=private)
    except ViewerError:
        for spec in CONDITIONS:
            statuses[spec.id] = "completed_records_unavailable"
        return all_predictions, statuses, repetition_counts

    expected = {spec.id for spec in CONDITIONS}
    completed: dict[str, dict[str, Any]] = {}
    duplicates: set[str] = set()
    unexpected_record = False
    for row in completed_rows:
        condition_id = row.get("condition")
        if not isinstance(condition_id, str) or condition_id not in expected:
            unexpected_record = True
            continue
        if condition_id in completed:
            duplicates.add(condition_id)
        else:
            completed[condition_id] = row

    runs_root = private / "runs"
    for spec in CONDITIONS:
        if unexpected_record:
            statuses[spec.id] = "unexpected_completed_record"
            continue
        if spec.id in duplicates:
            statuses[spec.id] = "duplicate_completed_record"
            continue
        record = completed.get(spec.id)
        if record is None:
            continue
        try:
            run_dir = _run_directory(record.get("output_dir"), root=root, private=private)
        except ViewerError:
            statuses[spec.id] = "saved_run_invalid"
            continue
        if not run_dir.name.endswith("-" + spec.id):
            statuses[spec.id] = "saved_run_condition_mismatch"
            continue
        predictions, issue, repetitions = _read_condition_predictions(
            run_dir,
            runs_root=runs_root,
            expected_events=events,
            expected_repetitions=EXPECTED_REPETITIONS[spec.id],
        )
        all_predictions[spec.id] = predictions
        repetition_counts[spec.id] = repetitions
        statuses[spec.id] = issue
    return all_predictions, statuses, repetition_counts


def _label_text(value: Any) -> str:
    if value is None:
        return "<abstain>"
    return str(value).lower() if isinstance(value, bool) else str(value)


def _confusion(
    events: dict[str, dict[str, Any]],
    predictions: dict[str, dict[str, list[dict[str, Any]]]],
    repetition_counts: dict[str, int],
) -> dict[str, Any]:
    output: dict[str, Any] = {}
    for spec in CONDITIONS:
        condition_output: dict[str, Any] = {}
        repeats = max(1, repetition_counts.get(spec.id, 0))
        for dimension in DIMENSIONS:
            row_labels: list[str] = []
            columns: list[str] = []
            matrix_counts: dict[str, dict[str, float]] = {}
            for event_id, event in events.items():
                reference = _label_text(event["reference"][dimension])
                if reference not in row_labels:
                    row_labels.append(reference)
                if reference not in columns:
                    columns.append(reference)
                row_counts = matrix_counts.setdefault(reference, {})
                by_repetition = {
                    item["repetition"]: item["labels"][dimension]
                    for item in predictions.get(spec.id, {}).get(event_id, [])
                }
                for repetition in range(1, repeats + 1):
                    predicted = _label_text(by_repetition.get(repetition))
                    if predicted not in columns:
                        columns.append(predicted)
                    row_counts[predicted] = row_counts.get(predicted, 0.0) + (1.0 / repeats)
            if "<abstain>" not in columns:
                columns.append("<abstain>")
            condition_output[dimension] = {
                "labels": columns,
                "row_labels": row_labels,
                "matrix": [[matrix_counts.get(row, {}).get(column, 0.0) for column in columns] for row in row_labels],
            }
        output[spec.id] = condition_output
    return output


def _safe_output(output: Path, *, root: Path, aggregate_only: bool) -> Path:
    output = _absolute(output)
    private = _absolute(root / "private")
    private_outputs = {
        _absolute(private / "viewer"),
        _absolute(private / "viewer-aggregate"),
    }
    if not aggregate_only and output not in private_outputs:
        raise ViewerError("private_output_must_be_dedicated")
    if output not in private_outputs and _overlaps(output, private):
        raise ViewerError("output_overlaps_private_corpus")

    protected = (
        root / "REAL_RESULTS.json",
        root / "REAL_OPERATIONS.json",
        root / "results",
        root / "viewer",
    )
    if any(_overlaps(output, _absolute(path)) for path in protected):
        raise ViewerError("output_overwrites_source")

    _reject_output_symlinks(output)
    try:
        info = output.lstat()
    except FileNotFoundError:
        info = None
    except OSError:
        raise ViewerError("output_path_invalid") from None
    if info is not None:
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise ViewerError("output_path_invalid")
        try:
            for child in output.iterdir():
                child_info = child.lstat()
                if child.name not in OUTPUT_FILES or stat.S_ISLNK(child_info.st_mode) or not stat.S_ISREG(child_info.st_mode):
                    raise ViewerError("output_directory_not_dedicated")
                if child_info.st_nlink != 1:
                    raise ViewerError("output_file_linked")
        except OSError:
            raise ViewerError("output_path_invalid") from None
    try:
        output.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(output, 0o700)
    except OSError:
        raise ViewerError("output_path_unwritable") from None
    return output


def _reject_output_symlinks(path: Path) -> None:
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        try:
            info = current.lstat()
        except FileNotFoundError:
            continue
        except OSError:
            raise ViewerError("output_path_invalid") from None
        if stat.S_ISLNK(info.st_mode):
            raise ViewerError("output_symlink")


def _json_bytes(value: Any) -> bytes:
    try:
        encoded = (json.dumps(value, ensure_ascii=True, indent=2, allow_nan=False) + "\n").encode("ascii")
    except (TypeError, ValueError, UnicodeEncodeError):
        raise ViewerError("output_json_invalid") from None
    if len(encoded) >= MAX_FILE_BYTES:
        raise ViewerError("output_too_large")
    return encoded


def _write_private(path: Path, data: bytes) -> None:
    if len(data) >= MAX_FILE_BYTES:
        raise ViewerError("output_too_large")
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags, 0o600)
    except OSError:
        raise ViewerError("output_file_unwritable") from None
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise ViewerError("output_file_invalid")
        os.fchmod(fd, 0o600)
        view = memoryview(data)
        while view:
            written = os.write(fd, view)
            view = view[written:]
        os.fsync(fd)
    finally:
        os.close(fd)


def _load_public(root: Path) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    real_results = _read_json(root / "REAL_RESULTS.json", base=root)
    operations = _read_json(root / "REAL_OPERATIONS.json", base=root)
    comparison = _read_json(root / "results" / "comparison.json", base=root)
    if not all(isinstance(item, dict) for item in (real_results, operations, comparison)):
        raise ViewerError("public_aggregate_invalid")
    return real_results, operations, comparison


def _asset_bytes(root: Path) -> dict[str, bytes]:
    assets: dict[str, bytes] = {}
    for name in ("index.html", "app.js", "style.css"):
        assets[name] = _read_bounded(root / "viewer" / name, base=root)
    return assets


def build(output: Path, *, aggregate_only: bool = False) -> Path:
    """Build the local static viewer, using only public aggregates when requested."""
    root = _absolute(EXPERIMENT_ROOT)
    thermal_checkpoint()
    target = _safe_output(Path(output), root=root, aggregate_only=aggregate_only)
    real_results, operations, comparison = _load_public(root)
    condition_rows, public_complete = _condition_metadata(real_results, operations)
    first_run = _first_run(comparison)

    if aggregate_only:
        reference_counts = real_results.get("reference_counts")
        reference_counts = reference_counts if isinstance(reference_counts, dict) else {}
        event_count = reference_counts.get("unique_events", 0)
        session_count = reference_counts.get("sessions", 0)
        event_count = event_count if isinstance(event_count, int) and not isinstance(event_count, bool) else 0
        session_count = session_count if isinstance(session_count, int) and not isinstance(session_count, bool) else 0
        review = {"schema": 1, "aggregate_only": True, "packet_id": "aggregate-only", "events": []}
        results = {
            "schema": 1,
            "complete": public_complete,
            "aggregate_only": True,
            "counts": {"events": event_count or 0, "sessions": session_count or 0},
            "conditions": condition_rows,
            "sessions": [],
            "confusion": {},
            "confusion_supported": False,
            "unavailable_conditions": [row["id"] for row in condition_rows if not row["available"]],
            "first_run": first_run,
        }
    else:
        review_events, events, packet_bytes = _load_private_references(root)
        predictions, private_statuses, repetition_counts = _load_private_predictions(root, events)
        sessions: list[dict[str, Any]] = []
        session_by_id: dict[str, dict[str, Any]] = {}
        for session_id, private_session in _sessions_from_events(events).items():
            session = {
                "key": f"Session{len(sessions) + 1:02d}",
                "harness": private_session["harness"],
                "events": [],
            }
            for event in private_session["events"]:
                session_event = {
                    "id": event["id"],
                    "order": event["order"],
                    "context": json.dumps(event["context"], ensure_ascii=True, separators=(",", ":")),
                    "reference": event["reference"],
                    "causal_reference": event["causal_reference"],
                    "predictions": {
                        spec.id: predictions[spec.id].get(event["id"], [])
                        for spec in CONDITIONS
                    },
                }
                session["events"].append(session_event)
                session_by_id[event["id"]] = session
            sessions.append(session)

        unavailable: list[str] = []
        for row in condition_rows:
            private_issue = private_statuses.get(row["id"])
            if private_issue:
                row["available"] = False
                row["unavailable"] = private_issue
            if not row["available"]:
                unavailable.append(row["id"])
        results = {
            "schema": 1,
            "complete": public_complete and not unavailable,
            "aggregate_only": False,
            "counts": {"events": len(events), "sessions": len(sessions)},
            "conditions": condition_rows,
            "sessions": sessions,
            "confusion": _confusion(events, predictions, repetition_counts),
            "confusion_supported": True,
            "unavailable_conditions": unavailable,
            "first_run": first_run,
        }
        review = {
            "schema": 1,
            "aggregate_only": False,
            "packet_id": "sha256:" + hashlib.sha256(packet_bytes).hexdigest(),
            "events": review_events,
        }

    assets = _asset_bytes(root)
    thermal_checkpoint()
    for name, data in assets.items():
        _write_private(target / name, data)
    _write_private(target / "review.json", _json_bytes(review))
    _write_private(target / "results.json", _json_bytes(results))
    thermal_checkpoint()
    return target


def _sessions_from_events(events: dict[str, dict[str, Any]]) -> dict[str, dict[str, Any]]:
    sessions: dict[str, dict[str, Any]] = {}
    for event in events.values():
        session = sessions.setdefault(event["session_id"], {"harness": event["harness"], "events": []})
        session["events"].append(event)
    for session in sessions.values():
        session["events"].sort(key=lambda item: item["order"])
    return sessions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aggregate-only", action="store_true", help="build from public aggregates only")
    parser.add_argument("--output", type=Path, help="dedicated output directory (default: private/viewer)")
    args = parser.parse_args(argv)
    output = args.output or (EXPERIMENT_ROOT / "private" / "viewer")
    try:
        # Interactive builds stay in the two ignored viewer directories.
        # The callable aggregate-only builder also supports temporary CI roots.
        permitted = {_absolute(EXPERIMENT_ROOT / "private" / name)
                     for name in ("viewer", "viewer-aggregate")}
        if _absolute(output) not in permitted:
            raise ViewerError("output_not_dedicated_private_viewer")
        build(output, aggregate_only=args.aggregate_only)
    except (ViewerError, PipelineError, OSError) as exc:
        print(f"local viewer build failed: {exc}", file=sys.stderr)
        return 2
    print(f"local viewer generated at {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
