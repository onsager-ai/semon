#!/usr/bin/env python3
"""Write a private packet of the highest-error events from frozen runs."""

from __future__ import annotations

import argparse
import json
import os
import stat
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

import benchmark
import real_metrics
import real_operations
import real_run


MAX_CONTEXT_BYTES = 8_192
OUTPUT_RELATIVE = Path("failure-cases/packet.jsonl")


class CasesError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _root(private_root: str | Path | None) -> Path:
    if private_root is None:
        return real_run._private_root()
    root = Path(private_root).absolute()
    if root.is_symlink() or not root.is_dir():
        raise CasesError("private_root_invalid")
    try:
        os.chmod(root, 0o700)
    except OSError:
        raise CasesError("private_root_invalid") from None
    return root


def _reference(root: Path) -> tuple[list[benchmark.Event], dict[str, dict[str, Any]]]:
    real_run._thermal_checkpoint()
    try:
        real_run._verify_reference_freezes(root)
    except real_run.RealRunError as exc:
        raise CasesError(exc.code) from None
    finally:
        real_run._thermal_checkpoint()
    try:
        projection = real_operations._json(root / real_run.PROJECTION_PATH, root, lines=True)
        rows = real_operations._json(root / real_run.REFERENCE_EVENTS_PATH, root, lines=True)
        projection = real_run._validate_projection_rows(projection)
        refs = {row["id"]: row for row in rows}
        if len(refs) != len(rows) or not rows:
            raise CasesError("reference_invalid")
        labels = real_run._validate_gold_rows([
            {"id": row["id"], "gold": {"streaming": row["causal_gold"], "offline": row["gold"]}}
            for row in rows
        ], {row["id"] for row in projection})
        events = real_run._events_from_rows(projection, labels)
        metadata = {row["id"]: row for row in projection}
        if any(any(ref[key] != metadata[event_id][key]
                   for key in ("session_id", "event_order", "harness"))
               for event_id, ref in refs.items()):
            raise CasesError("reference_metadata_mismatch")
        return events, refs
    except real_operations.OperationsError as exc:
        raise CasesError(exc.code) from None
    except real_run.RealRunError as exc:
        raise CasesError(exc.code) from None


def _run_directory(path: Path, root: Path) -> Path:
    try:
        info = path.lstat()
        resolved = path.resolve(strict=True)
        resolved.relative_to(root.resolve())
    except (OSError, ValueError):
        raise CasesError("run_directory_invalid") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise CasesError("run_directory_invalid")
    return resolved


def _condition_run(
    run_dir: Path, root: Path, references: dict[str, dict[str, Any]],
    events: list[benchmark.Event], events_by_id: dict[str, benchmark.Event],
) -> tuple[str, list[dict[str, Any]]]:
    run_dir = _run_directory(run_dir, root)
    try:
        summary = real_operations._json(run_dir / "summary.json", root)
        rows = real_operations._json(run_dir / "predictions.jsonl", root, lines=True)
    except real_operations.OperationsError as exc:
        raise CasesError(exc.code) from None
    if not isinstance(summary, dict):
        raise CasesError("summary_invalid")
    condition = summary.get("condition")
    if not isinstance(condition, str) or condition not in real_run.CONDITIONS:
        raise CasesError("condition_invalid")
    strategy, mode, context_mode = real_run.CONDITIONS[condition]
    if (summary.get("strategy"), summary.get("mode"), summary.get("context")) != (strategy, mode, context_mode):
        raise CasesError("condition_mismatch")
    if summary.get("event_count") != len(references):
        raise CasesError("event_count_mismatch")
    repetitions = summary.get("remote_repetitions")
    if isinstance(repetitions, bool) or not isinstance(repetitions, int) or repetitions < 1:
        raise CasesError("repetitions_invalid")

    grouped: dict[str, dict[int, dict[str, Any]]] = defaultdict(dict)
    for row in rows:
        event_id, repetition = row.get("id"), row.get("repetition")
        if (not isinstance(event_id, str) or event_id not in references or isinstance(repetition, bool)
                or not isinstance(repetition, int) or repetition < 1
                or (row.get("strategy"), row.get("mode"), row.get("context")) != (strategy, mode, context_mode)
                or not isinstance(row.get("predictions"), dict)
                or repetition in grouped[event_id]):
            raise CasesError("prediction_row_invalid")
        grouped[event_id][repetition] = row
    expected_repetitions = set(range(1, repetitions + 1))
    if (set(grouped) != set(references)
            or any(set(trials) != expected_repetitions for trials in grouped.values())):
        raise CasesError("prediction_coverage_mismatch")

    ranked: list[tuple[float, str, dict[str, Any]]] = []
    for event_id, trials in grouped.items():
        gold = references[event_id]["gold"]
        mismatched: set[str] = set()
        wrong_count = 0
        trial_predictions = []
        for repetition in sorted(trials):
            predictions = trials[repetition]["predictions"]
            trial_mismatches = {
                dimension for dimension in benchmark.DIMENSIONS
                if real_metrics._label(predictions.get(dimension), dimension, allow_abstain=True)
                != real_metrics._label(gold[dimension], dimension)
            }
            mismatched.update(trial_mismatches)
            wrong_count += len(trial_mismatches)
            trial_predictions.append({"repetition": repetition, "predictions": predictions})
        if not mismatched:
            continue
        context = benchmark.format_context(
            events_by_id[event_id], events, context_mode, MAX_CONTEXT_BYTES,
        )
        if len(context.encode("utf-8")) > MAX_CONTEXT_BYTES:
            raise CasesError("context_limit_exceeded")
        row = {
            "condition": condition, "id": event_id,
            "gold": references[event_id]["gold"],
            "causal_gold": references[event_id]["causal_gold"],
            "trial_predictions": trial_predictions,
            "mismatched_dimensions": [dimension for dimension in benchmark.DIMENSIONS if dimension in mismatched],
            "context_mode": context_mode, "context": context,
        }
        ranked.append((wrong_count / repetitions, event_id, row))

    ranked.sort(key=lambda item: (-item[0], item[1]))
    return condition, [row for _, _, row in ranked[:5]]


def _write_new(path: Path, data: bytes, root: Path) -> None:
    if path.parent.resolve() != (root / OUTPUT_RELATIVE.parent).resolve():
        raise CasesError("output_path_invalid")
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
    except FileExistsError:
        raise CasesError("output_exists") from None
    except OSError:
        raise CasesError("private_write_failed") from None


def make_packet(run_dirs: list[str | Path], *, private_root: str | Path | None = None) -> dict[str, Any]:
    root = _root(private_root)
    output = root / OUTPUT_RELATIVE
    output_dir = output.parent
    if output.exists() or output.is_symlink() or output_dir.is_symlink():
        raise CasesError("output_exists")
    events, references = _reference(root)
    events_by_id = {event.event_id: event for event in events}
    results: dict[str, list[dict[str, Any]]] = {}
    for path in run_dirs:
        real_run._thermal_checkpoint()
        condition, cases = _condition_run(Path(path).absolute(), root, references, events, events_by_id)
        real_run._thermal_checkpoint()
        if condition in results:
            raise CasesError("duplicate_condition")
        results[condition] = cases

    try:
        output_dir.mkdir(mode=0o700)
        os.chmod(output_dir, 0o700)
    except FileExistsError:
        try:
            if output_dir.is_symlink() or not output_dir.is_dir():
                raise CasesError("output_path_invalid")
            os.chmod(output_dir, 0o700)
        except OSError:
            raise CasesError("private_write_failed") from None
    except OSError:
        raise CasesError("private_write_failed") from None
    rows = [row for condition in real_run.CONDITIONS for row in results.get(condition, [])]
    data = b"".join(
        (json.dumps(row, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
        for row in rows
    )
    _write_new(output, data, root)
    real_run._thermal_checkpoint()
    return {
        "selected_cases": len(rows),
        "by_condition": {key: len(results[key]) for key in real_run.CONDITIONS if key in results},
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-dir", type=Path, action="append", required=True)
    args = parser.parse_args(argv)
    try:
        result = make_packet(args.run_dir)
    except CasesError as exc:
        print(json.dumps({"error_code": exc.code}), file=sys.stderr)
        return 2
    except real_run.RealRunError as exc:
        print(json.dumps({"error_code": exc.code}), file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
