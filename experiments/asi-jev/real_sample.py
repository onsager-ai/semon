#!/usr/bin/env python3
"""Prepare a private, deterministic user spot-check packet from frozen inputs."""

from __future__ import annotations

import argparse
import json
import os
import random
import sys
from collections import Counter
from pathlib import Path
from typing import Any

import benchmark
import real_run


SEED = 20261030
SAMPLE_SIZE = 50
MAX_CONTEXT_BYTES = 8_192
MAX_FILE_BYTES = 10_000_000
OUTPUT_DIR = "spot-check"


class SpotCheckError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _checkpoint() -> None:
    try:
        real_run._thermal_checkpoint()
    except real_run.RealRunError as exc:
        raise SpotCheckError(exc.code) from None


def _read(path: Path, root: Path) -> bytes:
    _checkpoint()
    try:
        _, data = real_run._read_private_file(path, root)
        return data
    except real_run.RealRunError as exc:
        raise SpotCheckError(exc.code) from None
    finally:
        _checkpoint()


def _load(root: Path) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    _checkpoint()
    try:
        real_run._verify_reference_freezes(root)
    except real_run.RealRunError as exc:
        raise SpotCheckError(exc.code) from None
    finally:
        _checkpoint()

    try:
        projection_rows = real_run._validate_projection_rows(real_run._read_jsonl_bytes(
            _read(root / real_run.PROJECTION_PATH, root), "invalid_projection_jsonl",
        ))
        reference_rows = real_run._read_jsonl_bytes(
            _read(root / real_run.REFERENCE_EVENTS_PATH, root), "invalid_reference_jsonl",
        )
    except real_run.RealRunError as exc:
        raise SpotCheckError(exc.code) from None
    return projection_rows, reference_rows


def _events(
    projection_rows: list[dict[str, Any]],
    reference_rows: list[dict[str, Any]],
) -> tuple[list[benchmark.Event], dict[str, dict[str, Any]]]:
    required = {
        "id", "session_id", "event_order", "harness", "complete_session",
        "eligible_event_count", "gold", "causal_gold",
    }
    labels: dict[str, dict[str, dict[str, Any]]] = {}
    references: dict[str, dict[str, Any]] = {}
    for row in reference_rows:
        if set(row) != required or not isinstance(row["id"], str) or row["id"] in references:
            raise SpotCheckError("reference_event_invalid")
        references[row["id"]] = row
        labels[row["id"]] = {"offline": row["gold"], "streaming": row["causal_gold"]}

    projection_by_id = {row["id"]: row for row in projection_rows}
    if set(references) != set(projection_by_id):
        raise SpotCheckError("reference_coverage_mismatch")
    for event_id, reference in references.items():
        projection = projection_by_id[event_id]
        if any(reference[key] != projection[key] for key in ("session_id", "event_order", "harness")):
            raise SpotCheckError("reference_metadata_mismatch")

    try:
        labels = real_run._validate_gold_rows(
            [{"id": event_id, "gold": {"streaming": value["streaming"], "offline": value["offline"]}}
             for event_id, value in labels.items()],
            set(projection_by_id),
        )
        events = real_run._events_from_rows(projection_rows, labels)
    except real_run.RealRunError as exc:
        raise SpotCheckError(exc.code) from None
    return events, references


def _select_ids(references: list[dict[str, Any]]) -> list[str]:
    by_id = {row["id"]: row for row in references}
    if len(by_id) != len(references):
        raise SpotCheckError("duplicate_reference_id")
    ordered_ids = sorted(by_id)
    strata = (
        [event_id for event_id in ordered_ids if by_id[event_id]["harness"] == "claude"],
        [event_id for event_id in ordered_ids if by_id[event_id]["harness"] == "codex"],
        [event_id for event_id in ordered_ids if by_id[event_id]["gold"]["anomaly"] is True],
        [event_id for event_id in ordered_ids if by_id[event_id]["gold"]["failure_category"] != "none"],
        [event_id for event_id in ordered_ids if by_id[event_id]["gold"]["retry_loop"] == "retry"],
        [event_id for event_id in ordered_ids if by_id[event_id]["gold"]["retry_loop"] == "loop"],
    )
    rng = random.Random(SEED)
    queues = []
    for stratum in strata:
        rng.shuffle(stratum)
        queues.append(stratum)

    target = min(SAMPLE_SIZE, len(ordered_ids))
    selected: list[str] = []
    seen: set[str] = set()
    while len(selected) < target:
        added = False
        for queue in queues:
            while queue and queue[0] in seen:
                queue.pop(0)
            if queue and len(selected) < target:
                event_id = queue.pop(0)
                selected.append(event_id)
                seen.add(event_id)
                added = True
        if not added:
            break

    remaining = [event_id for event_id in ordered_ids if event_id not in seen]
    rng.shuffle(remaining)
    selected.extend(remaining[:target - len(selected)])
    return selected


def _jsonl(rows: list[dict[str, Any]]) -> bytes:
    return b"".join(
        (json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
        for row in rows
    )


def _write_new(path: Path, data: bytes, root: Path) -> None:
    if len(data) > MAX_FILE_BYTES:
        raise SpotCheckError("output_too_large")
    try:
        path.parent.resolve().relative_to(root.resolve())
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
    except FileExistsError:
        raise SpotCheckError("output_exists") from None
    except (OSError, ValueError):
        raise SpotCheckError("private_write_failed") from None


def prepare_sample(*, private_root: str | Path | None = None) -> dict[str, Any]:
    root = Path(private_root if private_root is not None else real_run.PRIVATE_ROOT).absolute()
    if root.is_symlink() or not root.is_dir():
        raise SpotCheckError("private_root_invalid")
    try:
        os.chmod(root, 0o700)
    except OSError:
        raise SpotCheckError("private_root_invalid") from None
    output_dir = root / OUTPUT_DIR
    if output_dir.exists() or output_dir.is_symlink():
        raise SpotCheckError("output_exists")

    projection_rows, reference_rows = _load(root)
    _checkpoint()
    events, references = _events(projection_rows, reference_rows)
    selected = _select_ids(reference_rows)
    events_by_id = {event.event_id: event for event in events}
    packet = []
    template = []
    for event_id in selected:
        reference = references[event_id]
        context = benchmark.format_context(
            events_by_id[event_id], events, "offline", MAX_CONTEXT_BYTES,
        )
        if len(context.encode("utf-8")) > MAX_CONTEXT_BYTES:
            raise SpotCheckError("context_limit_exceeded")
        packet.append({
            "id": event_id, "harness": reference["harness"], "context": context,
            "gold": reference["gold"], "causal_gold": reference["causal_gold"],
        })
        # Null labels keep the blank return template from counting as a review.
        template.append({"id": event_id, "gold": None, "causal_gold": None})

    by_harness = dict(sorted(Counter(row["harness"] for row in packet).items()))
    rare_counts = {
        "anomaly_true": sum(row["gold"]["anomaly"] is True for row in packet),
        "failure_not_none": sum(row["gold"]["failure_category"] != "none" for row in packet),
        "retry_loop": sum(row["gold"]["retry_loop"] == "loop" for row in packet),
        "retry": sum(row["gold"]["retry_loop"] == "retry" for row in packet),
    }
    summary = {
        "sample_count": len(packet), "target_count": SAMPLE_SIZE,
        "by_harness": by_harness, "rare_label_counts": rare_counts,
    }
    manifest = {
        "schema": 1, "seed": SEED, "sample_count": len(packet),
        "target_count": SAMPLE_SIZE, "context_mode": "offline",
        "max_context_bytes": MAX_CONTEXT_BYTES,
        "selection": "stratum_round_robin_then_seeded_fill",
        "stratum_counts": {**by_harness, **rare_counts},
        "user_review": {
            "status": "pending", "template": "template.jsonl",
            "template_is_returned_review": False, "agreement_counted": False,
        },
    }
    try:
        output_dir.mkdir(mode=0o700)
        os.chmod(output_dir, 0o700)
    except FileExistsError:
        raise SpotCheckError("output_exists") from None
    except OSError:
        raise SpotCheckError("private_write_failed") from None
    _write_new(output_dir / "packet.jsonl", _jsonl(packet), root)
    _write_new(output_dir / "template.jsonl", _jsonl(template), root)
    _write_new(
        output_dir / "manifest.json",
        (json.dumps(manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8"),
        root,
    )
    _checkpoint()
    return summary


def main(argv: list[str] | None = None) -> int:
    argparse.ArgumentParser(description=__doc__).parse_args(argv)
    try:
        summary = prepare_sample()
    except SpotCheckError as exc:
        print(json.dumps({"error_code": exc.code}), file=sys.stderr)
        return 2
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
