#!/usr/bin/env python3
"""Write disclosure-safe numeric operations aggregates for private runs."""

from __future__ import annotations

import argparse
import json
import math
import os
import stat
import sys
from pathlib import Path
from typing import Any

import real_run

MAX_FILE_BYTES = 10_000_000
LATENCY_FIELDS = {
    "event_compute_or_http_ms": "request_rtt_ms",
    "rule_gate_compute_ms": "rule_gate_compute_ms",
    "projected_online_latency_ms": "projected_online_latency_ms",
    "logical_queue_wait_ms": "logical_queue_wait_ms",
}


class OperationsError(ValueError):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _read(path: Path, root: Path) -> bytes:
    try:
        path = real_run._private_input(path, root)
        before = path.stat()
        if before.st_size >= MAX_FILE_BYTES:
            raise OperationsError("input_too_large")
        real_run._thermal_checkpoint()
        with path.open("rb") as source:
            data = source.read(MAX_FILE_BYTES - 1)
        after = path.stat()
        real_run._thermal_checkpoint()
    except OperationsError:
        raise
    except (OSError, real_run.RealRunError):
        raise OperationsError("private_input_invalid") from None
    if len(data) != before.st_size or (after.st_size, after.st_mtime_ns) != (before.st_size, before.st_mtime_ns):
        raise OperationsError("private_input_changed")
    return data


def _json(path: Path, root: Path, *, lines: bool = False) -> Any:
    data = _read(path, root)
    try:
        return real_run._read_jsonl_bytes(data, "private_jsonl_invalid") if lines else json.loads(data)
    except (json.JSONDecodeError, UnicodeDecodeError, real_run.RealRunError):
        raise OperationsError("private_json_invalid") from None


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        number = float(value)
    except (OverflowError, ValueError):
        return None
    return number if math.isfinite(number) and number >= 0 else None


def _distribution(values: list[float]) -> dict[str, Any]:
    return {"count": len(values), "p50_ms": real_run.benchmark._percentile(values, 0.50),
            "p95_ms": real_run.benchmark._percentile(values, 0.95)}


def _usage(summary: dict[str, Any], calls: list[dict[str, Any]], requests: list[dict[str, Any]]) -> dict[str, Any]:
    successful = [call for call in requests if call.get("failed") is None]
    if not requests:
        return {"status": "not_applicable", "input_tokens": 0, "output_tokens": 0}
    if not successful:
        return {"status": "incomplete", "input_tokens": None, "output_tokens": None}
    totals = [0, 0]
    sessions = summary.get("sessions")
    valid = isinstance(sessions, list) and bool(sessions)
    if valid:
        for session in sessions:
            item = session.get("summary", {}).get("usage") if isinstance(session, dict) else None
            pair = (item.get("input_tokens"), item.get("output_tokens")) if isinstance(item, dict) else (None, None)
            if any(isinstance(token, bool) or not isinstance(token, int) or token < 0 for token in pair):
                valid = False
                break
            totals[0] += pair[0]
            totals[1] += pair[1]
    if not valid:
        totals = [0, 0]
        valid = True
        for call in successful:
            pair = (call.get("input_tokens"), call.get("output_tokens"))
            if any(isinstance(token, bool) or not isinstance(token, int) or token < 0 for token in pair):
                valid = False
                break
            totals[0] += pair[0]
            totals[1] += pair[1]
    if not valid:
        return {"status": "incomplete", "input_tokens": None, "output_tokens": None}
    return {"status": "partial" if len(successful) != len(requests) else "complete",
            "input_tokens": totals[0], "output_tokens": totals[1]}


def _run_row(run_dir: Path, root: Path) -> tuple[str, dict[str, Any]]:
    try:
        info = run_dir.lstat()
        directory = run_dir.resolve(strict=True)
        directory.relative_to(root.resolve())
    except (OSError, ValueError):
        raise OperationsError("run_directory_invalid") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise OperationsError("run_directory_invalid")
    summary = _json(directory / "summary.json", root)
    if not isinstance(summary, dict):
        raise OperationsError("summary_invalid")
    condition = summary.get("condition")
    if not isinstance(condition, str) or condition not in real_run.CONDITIONS:
        raise OperationsError("condition_invalid")
    strategy, mode, context = real_run.CONDITIONS[condition]
    if (summary.get("strategy"), summary.get("mode"), summary.get("context")) != (strategy, mode, context):
        raise OperationsError("condition_mismatch")
    events, wall = summary.get("event_count"), _number(summary.get("execution_wall_seconds"))
    if isinstance(events, bool) or not isinstance(events, int) or events < 1 or wall is None or wall <= 0:
        raise OperationsError("summary_invalid")

    calls = _json(directory / "calls.json", root)
    if not isinstance(calls, list) or any(not isinstance(call, dict) for call in calls):
        raise OperationsError("calls_invalid")
    if any(not isinstance(call.get("request"), bool) for call in calls):
        raise OperationsError("calls_invalid")
    requests = [call for call in calls if call["request"]]
    failed = [call for call in requests if call.get("failed") is not None]
    if (summary.get("request_count") != len(requests)
            or summary.get("request_failure_count") != len(failed)):
        raise OperationsError("calls_summary_mismatch")
    rtts = []
    for call in requests:
        if call.get("rtt_ms") is not None:
            value = _number(call["rtt_ms"])
            if value is None:
                raise OperationsError("calls_invalid")
            rtts.append(value)

    rows = _json(directory / "predictions.jsonl", root, lines=True)
    if not rows:
        raise OperationsError("predictions_empty")
    latency = {name: [] for name in LATENCY_FIELDS}
    unknown = dict.fromkeys(LATENCY_FIELDS, 0)
    for row in rows:
        if (row.get("strategy"), row.get("mode"), row.get("context")) != (strategy, mode, context):
            raise OperationsError("prediction_condition_mismatch")
        for name, field in LATENCY_FIELDS.items():
            raw = row.get(field)
            if raw is None:
                unknown[name] += 1
            else:
                value = _number(raw)
                if value is None:
                    raise OperationsError("prediction_latency_invalid")
                latency[name].append(value)

    usage = _usage(summary, calls, requests)
    jev_cost, jev_status = None, "not_applicable"
    if strategy == "jev":
        jev_status = "unknown"
        if usage["status"] in {"complete", "partial"}:
            rate = real_run.benchmark.JEV_LIST_PRICE_INPUT_PER_MILLION
            jev_cost = usage["input_tokens"] * rate / 1_000_000
            jev_status = "partial_estimate" if failed else "estimate"
    return condition, {
        "status": "measured", "event_count": events, "prediction_rows": len(rows),
        "execution_wall_seconds": wall, "prediction_rows_per_second": len(rows) / wall,
        "request_count": len(requests), "failed_request_count": len(failed),
        "request_rtt_ms": {**_distribution(rtts), "unknown_count": len(requests) - len(rtts)},
        "latency_ms": {name: {**_distribution(values), "unknown_rows": unknown[name]}
                       for name, values in latency.items()},
        "usage_tokens": usage, "jev_list_cost_usd_estimate": jev_cost,
        "jev_list_cost_status": jev_status, "haiku_billed_cost_usd": None,
        "haiku_billed_cost_status": "unknown" if strategy == "llm" else "not_applicable",
    }


def aggregate_runs(run_dirs: list[str | Path], *, root: Path | None = None) -> dict[str, Any]:
    root = real_run._private_root() if root is None else Path(root).resolve()
    conditions = {}
    for path in run_dirs:
        real_run._thermal_checkpoint()
        condition, result = _run_row(Path(path).absolute(), root)
        real_run._thermal_checkpoint()
        if condition in conditions:
            raise OperationsError("duplicate_condition")
        conditions[condition] = result
    if not conditions:
        raise OperationsError("run_directory_required")
    return {"schema": 1, "conditions": {key: conditions[key] for key in real_run.CONDITIONS if key in conditions}}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-dir", action="append", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        root = real_run._private_root()
        output = args.output.absolute()
        if output.is_symlink() or output.resolve().is_relative_to(root.resolve()):
            raise OperationsError("output_path_invalid")
        result = aggregate_runs(args.run_dir, root=root)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n",
                          encoding="utf-8")
        output.chmod(0o644)
    except OperationsError as error:
        print(json.dumps({"error_code": error.code}), file=sys.stderr)
        return 2
    except real_run.RealRunError as error:
        print(json.dumps({"error_code": error.code}), file=sys.stderr)
        return 2
    except OSError:
        print(json.dumps({"error_code": "output_write_failed"}), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
