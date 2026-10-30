#!/usr/bin/env python3
"""Run selected reviewed real-session conditions in a private sequential matrix."""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path

import benchmark
import real_run


MAX_REQUESTS = 4_096
PROGRESS_INTERVAL = 25
CONDITIONS = tuple(real_run.CONDITIONS)


def _append_jsonl(fd: int, row: dict[str, object]) -> None:
    pending = (json.dumps(row, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    while pending:
        pending = pending[os.write(fd, pending):]
    os.fsync(fd)


def _open_private_log(path: Path) -> int:
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_APPEND, 0o600)
    os.fchmod(fd, 0o600)
    return fd


def _matrix_directory() -> Path:
    root = real_run._private_root()
    path = root / f"matrix-{datetime.now(timezone.utc):%Y%m%dT%H%M%S%fZ}"
    path.mkdir(mode=0o700, exist_ok=False)
    os.chmod(path, 0o700)
    return path


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--all", action="store_true", help="run all nine conditions in defined order")
    selection.add_argument("--condition", choices=CONDITIONS, action="append", help="condition to run; repeat to select several")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    conditions = CONDITIONS if args.all else tuple(args.condition or ())
    if len(set(conditions)) != len(conditions):
        parser.error("each condition may be selected only once")

    progress_fd = completed_fd = -1
    original_post_json = benchmark._post_json
    lock = threading.Lock()
    counts: dict[str, int | str] = {"condition": "", "calls": 0, "failures": 0}

    def tracked_post_json(url: str, api_key: str, payload: dict, timeout: float):
        real_run._thermal_checkpoint()
        failed = False
        try:
            return original_post_json(url, api_key, payload, timeout)
        except BaseException:
            failed = True
            raise
        finally:
            try:
                real_run._thermal_checkpoint()
            finally:
                with lock:
                    counts["calls"] = int(counts["calls"]) + 1
                    counts["failures"] = int(counts["failures"]) + int(failed)
                    calls = int(counts["calls"])
                    if calls % PROGRESS_INTERVAL == 0:
                        condition = str(counts["condition"])
                        _append_jsonl(progress_fd, {"condition": condition, "calls": calls, "failures": counts["failures"]})
                        print(f"condition={condition} calls={calls} failures={counts['failures']}", flush=True)

    try:
        root = real_run._private_root()
        real_run._thermal_checkpoint()
        real_run._verify_reference_freezes(root)
        directory = _matrix_directory()
        progress_fd = _open_private_log(directory / "progress.jsonl")
        completed_fd = _open_private_log(directory / "completed.jsonl")
        _append_jsonl(progress_fd, {"status": "reference_verified"})
        benchmark._post_json = tracked_post_json
        for condition in conditions:
            with lock:
                counts.update(condition=condition, calls=0, failures=0)
            try:
                result = real_run.run_condition(
                    condition,
                    max_requests=MAX_REQUESTS,
                    llm_model=real_run.HAIKU_MODEL,
                )
            except (real_run.RealRunError, OSError):
                with lock:
                    calls, failures = int(counts["calls"]), int(counts["failures"])
                _append_jsonl(progress_fd, {"condition": condition, "status": "failed", "calls": calls, "failures": failures})
                print(f"condition={condition} failed=1", file=sys.stderr, flush=True)
                return 2
            with lock:
                calls, failures = int(counts["calls"]), int(counts["failures"])
            _append_jsonl(completed_fd, {
                "condition": condition,
                "output_dir": str(result["output_dir"]),
                "calls": calls,
                "failures": failures,
            })
            _append_jsonl(progress_fd, {"condition": condition, "status": "complete", "calls": calls, "failures": failures})
            print(
                f"condition={condition} calls={calls} failures={failures} "
                f"events={result['event_count']} sessions={result['session_count']} "
                f"planned_requests={result['planned_request_count']}",
                flush=True,
            )
        return 0
    except (real_run.RealRunError, OSError):
        print("matrix_failed=1", file=sys.stderr, flush=True)
        return 2
    finally:
        benchmark._post_json = original_post_json
        if progress_fd >= 0:
            os.close(progress_fd)
        if completed_fd >= 0:
            os.close(completed_fd)


if __name__ == "__main__":
    raise SystemExit(main())
