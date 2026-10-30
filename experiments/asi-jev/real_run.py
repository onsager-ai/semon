#!/usr/bin/env python3
"""Private-only runner for the reviewed real-session ASI evaluation."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import stat
import sys
import tempfile
import threading
import time
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import benchmark
import real_pipeline


MAX_FILE_BYTES = 10_000_000
MAX_TEXT_CHARS = 1_200
REMOTE_REPETITIONS = 3
MAX_SESSION_WORKERS = 4
DEFAULT_MAX_REQUESTS = 4_096
MAX_CONTEXT_BYTES = 8_192
PRIVATE_ROOT = real_pipeline.PRIVATE_ROOT
PROTOCOL_PATH = Path(__file__).with_name("REAL_PROTOCOL.md")
PIPELINE_PATH = Path(__file__).with_name("real_pipeline.py")
RUNNER_PATH = Path(__file__)

REAL_COMPLETION_DESCRIPTION = "The assistant reports the current requested work or turn complete; a stop marker alone is insufficient."
REAL_ENDING_DESCRIPTION = "Native evidence establishes session termination; a turn stop or last captured row alone is insufficient."
REAL_ANOMALY_DESCRIPTION = "The event is malformed, unexpected, or shows an anomalous repeated pattern; an ordinary tool failure alone is insufficient."

PROJECTION_PATH = "frozen-projection.jsonl"
REFERENCE_PATH = "frozen-reference.jsonl"
REFERENCE_EVENTS_PATH = "real-event-reference.jsonl"
PROJECTION_MANIFEST_PATH = "manifests/real-projection.json"
REFERENCE_MANIFEST_PATH = "manifests/real-reference.json"

SAFE_HARNESSES = {"claude", "codex"}
SAFE_FORMATS = {"claude-jsonl-v1", "codex-jsonl-v1"}
SAFE_EVENT_KINDS = {
    "user_message", "assistant_message", "tool_use", "tool_result", "error",
    "reasoning", "tool_status", "turn_status", "status",
}
SAFE_CLAUDE_RECORD_KINDS = {"user", "assistant", "tool_result"}
SAFE_CODEX_RECORD_KINDS = {
    f"{root}/{payload}"
    for root in ("response_item", "event_msg")
    for payload in real_pipeline.CODEX_PAYLOAD_TYPES
}
SAFE_NATIVE_RECORD_KINDS = SAFE_CLAUDE_RECORD_KINDS | SAFE_CODEX_RECORD_KINDS
SAFE_ROLES = {"user", "assistant"}
SAFE_STATUSES = {
    "success", "error", "running", "pending", "complete", "cancelled",
    "blocked", "unknown",
}
SAFE_STOP_REASONS = {
    "end_turn", "tool_use", "max_tokens", "stop_sequence", "refusal",
    "abort", "cancelled", "turn_complete",
}
TOOL_NAME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9_.:-]{0,79}$")
CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")

CONDITIONS: dict[str, tuple[str, str, str]] = {
    "rules-prefix": ("rules", "per-event", "prefix"),
    "rules-offline": ("rules", "per-event", "offline"),
    "jev-prefix": ("jev", "per-event", "prefix"),
    "jev-microbatch-prefix": ("jev", "microbatch", "prefix"),
    "jev-triggered-prefix": ("jev", "event-triggered", "prefix"),
    "jev-offline": ("jev", "per-event", "offline"),
    "llm-prefix": ("llm", "per-event", "prefix"),
    "llm-triggered-prefix": ("llm", "event-triggered", "prefix"),
    "llm-offline": ("llm", "per-event", "offline"),
}
HAIKU_MODEL = "wokey/claude-haiku-5-5"


class RealRunError(Exception):
    """Safe error code for the private runner."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":"),
        allow_nan=False,
    ).encode("utf-8")


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _private_root() -> Path:
    root = Path(PRIVATE_ROOT)
    if root.is_symlink():
        raise RealRunError("private_root_symlink")
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(root, 0o700)
    if not stat.S_ISDIR(root.stat().st_mode):
        raise RealRunError("private_root_invalid")
    return root


def _private_input(path: str | Path, root: Path) -> Path:
    candidate = Path(path).absolute()
    try:
        info = candidate.lstat()
        resolved = candidate.resolve(strict=True)
        resolved.relative_to(root.resolve())
    except (OSError, ValueError):
        raise RealRunError("input_must_be_in_private") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise RealRunError("input_not_regular")
    if info.st_size > MAX_FILE_BYTES:
        raise RealRunError("input_too_large")
    try:
        os.chmod(candidate, 0o600)
    except OSError:
        raise RealRunError("input_permissions_unavailable") from None
    return candidate


def _read_private_file(path: str | Path, root: Path) -> tuple[Path, bytes]:
    candidate = _private_input(path, root)
    try:
        with candidate.open("rb") as source:
            data = source.read(MAX_FILE_BYTES + 1)
    except OSError:
        raise RealRunError("input_unavailable") from None
    if len(data) > MAX_FILE_BYTES:
        raise RealRunError("input_too_large")
    return candidate, data


def _write_private(path: Path, data: bytes, root: Path) -> None:
    resolved_parent = path.parent.resolve()
    try:
        resolved_parent.relative_to(root.resolve())
    except ValueError:
        raise RealRunError("output_outside_private") from None
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(path.parent, 0o700)
    try:
        fd, temporary_name = tempfile.mkstemp(prefix=".real-run-", dir=path.parent)
        temporary_path = Path(temporary_name)
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "wb") as output:
                output.write(data)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary_path, path)
            os.chmod(path, 0o600)
        except Exception:
            try:
                temporary_path.unlink(missing_ok=True)
            except OSError:
                pass
            raise
    except OSError:
        raise RealRunError("private_write_failed") from None


def _read_json(path: Path, root: Path) -> tuple[dict[str, Any], bytes]:
    _, data = _read_private_file(path, root)
    try:
        value = json.loads(data)
    except (json.JSONDecodeError, UnicodeDecodeError):
        raise RealRunError("invalid_private_manifest") from None
    if not isinstance(value, dict):
        raise RealRunError("invalid_private_manifest")
    return value, data


def _read_jsonl_bytes(data: bytes, code: str) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for line in data.splitlines():
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except (json.JSONDecodeError, UnicodeDecodeError):
            raise RealRunError(code) from None
        if not isinstance(row, dict):
            raise RealRunError(code)
        rows.append(row)
    return rows


def _text(value: Any, *, optional: bool = True) -> str | None:
    if value is None and optional:
        return None
    if (not isinstance(value, str) or len(value) > MAX_TEXT_CHARS
            or real_pipeline.CONTROL_RE.search(value)):
        raise RealRunError("unsafe_projection_text")
    return value


def _identity(value: Any, code: str) -> str:
    if (not isinstance(value, str) or not value or len(value) > 256
            or CONTROL_RE.search(value)):
        raise RealRunError(code)
    return value


def _validate_projection_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not rows:
        raise RealRunError("empty_projection")
    seen_ids: set[str] = set()
    orders_by_session: dict[str, set[int]] = {}
    cleaned: list[dict[str, Any]] = []
    allowed_row_keys = {
        "id", "session_id", "event_order", "harness", "native_format",
        "arrival_ms", "projection",
    }
    allowed_projection_keys = {
        "event_kind", "native_record_kind", "role", "tool_name", "result_status",
        "exit_status", "stop_reason", "reason", "error_message", "visible_text",
        "evidence_text",
    }
    for row in rows:
        if set(row) != allowed_row_keys:
            raise RealRunError("unsafe_projection_schema")
        event_id = _identity(row.get("id"), "projection_id_invalid")
        session_id = _identity(row.get("session_id"), "projection_session_invalid")
        if event_id in seen_ids:
            raise RealRunError("duplicate_projection_id")
        seen_ids.add(event_id)
        event_order = row.get("event_order")
        if not isinstance(event_order, int) or isinstance(event_order, bool) or event_order < 0:
            raise RealRunError("projection_order_invalid")
        orders = orders_by_session.setdefault(session_id, set())
        if event_order in orders:
            raise RealRunError("duplicate_projection_order")
        orders.add(event_order)
        harness = row.get("harness")
        native_format = row.get("native_format")
        if (not isinstance(harness, str) or harness not in SAFE_HARNESSES
                or not isinstance(native_format, str) or native_format not in SAFE_FORMATS):
            raise RealRunError("projection_format_invalid")
        if native_format != f"{harness}-jsonl-v1":
            raise RealRunError("projection_format_mismatch")
        arrival_ms = row.get("arrival_ms")
        if arrival_ms is not None:
            if not isinstance(arrival_ms, (int, float)) or isinstance(arrival_ms, bool):
                raise RealRunError("projection_arrival_invalid")
            try:
                arrival_ms = float(arrival_ms)
            except (OverflowError, ValueError):
                raise RealRunError("projection_arrival_invalid") from None
            if not math.isfinite(arrival_ms) or arrival_ms < 0:
                raise RealRunError("projection_arrival_invalid")
        projection = row.get("projection")
        if not isinstance(projection, dict) or not set(projection).issubset(allowed_projection_keys):
            raise RealRunError("unsafe_projection_schema")
        if "event_kind" not in projection:
            raise RealRunError("projection_kind_missing")
        if "native_record_kind" not in projection:
            raise RealRunError("projection_native_kind_missing")
        event_kind = projection.get("event_kind")
        if not isinstance(event_kind, str) or event_kind not in SAFE_EVENT_KINDS:
            raise RealRunError("projection_kind_invalid")
        native_record_kind = projection.get("native_record_kind")
        if (not isinstance(native_record_kind, str)
                or native_record_kind not in SAFE_NATIVE_RECORD_KINDS):
            raise RealRunError("projection_native_kind_invalid")
        if ((harness == "claude" and native_record_kind not in SAFE_CLAUDE_RECORD_KINDS)
                or (harness == "codex" and native_record_kind not in SAFE_CODEX_RECORD_KINDS)):
            raise RealRunError("projection_native_kind_mismatch")
        role = projection.get("role")
        if role is not None and (not isinstance(role, str) or role not in SAFE_ROLES):
            raise RealRunError("projection_role_invalid")
        tool_name = projection.get("tool_name")
        if tool_name is not None and (
            not isinstance(tool_name, str) or not TOOL_NAME_RE.fullmatch(tool_name)
            or tool_name.lower() in {"authorization", "password", "token", "secret"}
        ):
            raise RealRunError("projection_tool_invalid")
        result_status = projection.get("result_status")
        if (result_status is not None
                and (not isinstance(result_status, str) or result_status not in SAFE_STATUSES)):
            raise RealRunError("projection_status_invalid")
        exit_status = projection.get("exit_status")
        if exit_status is not None and (
            not isinstance(exit_status, int) or isinstance(exit_status, bool)
            or not -255 <= exit_status <= 255
        ):
            raise RealRunError("projection_exit_status_invalid")
        stop_reason = projection.get("stop_reason")
        if (stop_reason is not None
                and (not isinstance(stop_reason, str) or stop_reason not in SAFE_STOP_REASONS)):
            raise RealRunError("projection_stop_reason_invalid")
        visible_text = projection.get("visible_text")
        evidence_text = projection.get("evidence_text")
        if visible_text is not None and evidence_text is not None and visible_text != evidence_text:
            raise RealRunError("projection_text_conflict")
        if visible_text is None and evidence_text is None:
            text_value = ""
        else:
            text_value = _text(visible_text if visible_text is not None else evidence_text) or ""
        cleaned_projection = {
            "event_kind": event_kind,
            "native_record_kind": native_record_kind,
            "role": role,
            "tool_name": tool_name,
            "result_status": result_status,
            "exit_status": exit_status,
            "stop_reason": stop_reason,
            "reason": _text(projection.get("reason")),
            "error_message": _text(projection.get("error_message")),
            "visible_text": text_value,
        }
        cleaned.append({
            "id": event_id,
            "session_id": session_id,
            "event_order": event_order,
            "harness": harness,
            "native_format": native_format,
            "arrival_ms": arrival_ms,
            "projection": cleaned_projection,
        })
    return cleaned


def _validate_gold_rows(rows: list[dict[str, Any]], event_ids: set[str]) -> dict[str, dict[str, dict[str, Any]]]:
    if not rows:
        raise RealRunError("empty_reference")
    labels: dict[str, dict[str, dict[str, Any]]] = {}
    if set(benchmark.DIMENSIONS) != {
        "activity", "failure_category", "retry_loop", "anomaly", "phase",
    }:
        raise RealRunError("taxonomy_mismatch")
    for row in rows:
        if set(row) != {"id", "gold"}:
            raise RealRunError("unsafe_reference_schema")
        event_id = _identity(row.get("id"), "reference_id_invalid")
        if event_id in labels:
            raise RealRunError("duplicate_reference_id")
        gold = row.get("gold")
        if not isinstance(gold, dict) or set(gold) != {"streaming", "offline"}:
            raise RealRunError("reference_frames_invalid")
        frames: dict[str, dict[str, Any]] = {}
        for frame in ("streaming", "offline"):
            values = gold[frame]
            if not isinstance(values, dict) or set(values) != set(benchmark.DIMENSIONS):
                raise RealRunError("reference_labels_incomplete")
            checked: dict[str, Any] = {}
            for dimension, allowed in benchmark.DIMENSIONS.items():
                value = values[dimension]
                if dimension == "anomaly":
                    valid = isinstance(value, bool)
                else:
                    valid = isinstance(value, str) and value in allowed
                if not valid:
                    raise RealRunError("reference_label_invalid")
                checked[dimension] = value
            frames[frame] = checked
        labels[event_id] = frames
    if set(labels) != event_ids:
        raise RealRunError("reference_coverage_mismatch")
    return labels


def _thermal_checkpoint() -> None:
    try:
        millidegrees = real_pipeline.thermal_checkpoint()
    except Exception as exc:
        code = getattr(exc, "code", None)
        raise RealRunError("thermal_limit" if code == "thermal_limit" else "thermal_unavailable") from None
    if not isinstance(millidegrees, int) or millidegrees >= real_pipeline.THERMAL_LIMIT_MILLIDEGREES:
        raise RealRunError("thermal_limit")


def _hash_source(path: Path) -> str:
    try:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_FILE_BYTES:
            raise RealRunError("implementation_file_invalid")
        digest = hashlib.sha256()
        with path.open("rb") as source:
            while True:
                chunk = source.read(64 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
        return digest.hexdigest()
    except OSError:
        raise RealRunError("implementation_file_unavailable") from None


def _freeze_hashes() -> dict[str, str]:
    protocol_hash = _hash_source(PROTOCOL_PATH)
    pipeline_hash = _hash_source(PIPELINE_PATH)
    runner_hash = _hash_source(RUNNER_PATH)
    privacy_hash = _hash_source(Path(__file__).with_name("real_privacy.py"))
    combined = hashlib.sha256(
        f"{pipeline_hash}\0{runner_hash}\0{privacy_hash}".encode("ascii")
    ).hexdigest()
    return {
        "protocol_sha256": protocol_hash,
        "benchmark_sha256": _hash_source(Path(benchmark.__file__)),
        "projection_pipeline_sha256": pipeline_hash,
        "projection_adapter_sha256": runner_hash,
        "privacy_transform_sha256": privacy_hash,
        "projection_implementation_sha256": combined,
    }


def _manifest_path(root: Path, relative: str) -> Path:
    return root / relative


def _projection_freeze(root: Path) -> tuple[dict[str, Any], str]:
    manifest, manifest_bytes = _read_json(
        _manifest_path(root, PROJECTION_MANIFEST_PATH), root,
    )
    manifest_hash = _sha256(manifest_bytes)
    del manifest_bytes
    if manifest.get("schema") != 1 or manifest.get("reviewed_sanitized_content") is not True:
        raise RealRunError("projection_freeze_invalid")
    expected_hashes = _freeze_hashes()
    if any(manifest.get(key) != value for key, value in expected_hashes.items()):
        raise RealRunError("projection_freeze_changed")
    _, projection_bytes = _read_private_file(root / PROJECTION_PATH, root)
    actual_hash = _sha256(projection_bytes)
    if actual_hash != manifest.get("projection_sha256"):
        raise RealRunError("projection_freeze_changed")
    return manifest, manifest_hash


def freeze_projection(path: str | Path, *, reviewed_sanitized_content: bool) -> dict[str, Any]:
    if not reviewed_sanitized_content:
        raise RealRunError("review_attestation_required")
    _thermal_checkpoint()
    root = _private_root()
    manifest_path = _manifest_path(root, PROJECTION_MANIFEST_PATH)
    if manifest_path.exists():
        raise RealRunError("projection_already_frozen")
    _, source_bytes = _read_private_file(path, root)
    rows = _validate_projection_rows(_read_jsonl_bytes(source_bytes, "invalid_projection_jsonl"))
    projection_hash = _sha256(source_bytes)
    hashes = _freeze_hashes()
    _thermal_checkpoint()
    # Freeze exact bytes. Validation uses only the allowlisted projection fields.
    _write_private(root / PROJECTION_PATH, source_bytes, root)
    manifest = {
        "schema": 1,
        "reviewed_sanitized_content": True,
        "source_projection_sha256": projection_hash,
        "projection_sha256": projection_hash,
        "event_count": len(rows),
        "session_count": len({row["session_id"] for row in rows}),
        **hashes,
    }
    _write_private(manifest_path, _canonical_json(manifest) + b"\n", root)
    return {"event_count": manifest["event_count"], "session_count": manifest["session_count"]}


def _projection_metadata(rows: list[dict[str, Any]]) -> tuple[set[str], list[dict[str, Any]]]:
    cleaned = _validate_projection_rows(rows)
    ids = {row["id"] for row in cleaned}
    metadata = [
        {
            "id": row["id"],
            "session_id": row["session_id"],
            "event_order": row["event_order"],
            "harness": row["harness"],
        }
        for row in cleaned
    ]
    return ids, metadata


def freeze_reference(path: str | Path) -> dict[str, Any]:
    _thermal_checkpoint()
    root = _private_root()
    reference_manifest_path = _manifest_path(root, REFERENCE_MANIFEST_PATH)
    if reference_manifest_path.exists():
        raise RealRunError("reference_already_frozen")
    projection_manifest, projection_manifest_hash = _projection_freeze(root)
    _, projection_bytes = _read_private_file(root / PROJECTION_PATH, root)
    projection_rows = _read_jsonl_bytes(projection_bytes, "invalid_projection_jsonl")
    event_ids, event_metadata = _projection_metadata(projection_rows)
    del projection_rows, projection_bytes
    _, reference_bytes = _read_private_file(path, root)
    reference_rows = _read_jsonl_bytes(reference_bytes, "invalid_reference_jsonl")
    labels = _validate_gold_rows(reference_rows, event_ids)
    reference_hash = _sha256(reference_bytes)
    eligible_counts = Counter(item["session_id"] for item in event_metadata)
    event_reference_bytes = b"".join(
        _canonical_json({
            **metadata,
            "complete_session": True,
            "eligible_event_count": eligible_counts[metadata["session_id"]],
            "gold": labels[metadata["id"]]["offline"],
            "causal_gold": labels[metadata["id"]]["streaming"],
        }) + b"\n"
        for metadata in event_metadata
    )
    _thermal_checkpoint()
    _write_private(root / REFERENCE_PATH, reference_bytes, root)
    _write_private(root / REFERENCE_EVENTS_PATH, event_reference_bytes, root)
    manifest = {
        "schema": 1,
        "projection_manifest_sha256": projection_manifest_hash,
        "projection_sha256": projection_manifest["projection_sha256"],
        "reference_sha256": reference_hash,
        "event_reference_sha256": _sha256(event_reference_bytes),
        "event_count": len(labels),
        "session_count": len({item["session_id"] for item in event_metadata}),
        "frames": ["streaming", "offline"],
        "labels_complete": True,
    }
    _write_private(reference_manifest_path, _canonical_json(manifest) + b"\n", root)
    return {"event_count": manifest["event_count"], "session_count": manifest["session_count"]}


def _verify_reference_freezes(root: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    projection_manifest, projection_manifest_hash = _projection_freeze(root)
    reference_manifest, _ = _read_json(
        _manifest_path(root, REFERENCE_MANIFEST_PATH), root,
    )
    if reference_manifest.get("schema") != 1 or reference_manifest.get("labels_complete") is not True:
        raise RealRunError("reference_freeze_invalid")
    if reference_manifest.get("projection_manifest_sha256") != projection_manifest_hash:
        raise RealRunError("reference_freeze_changed")
    if reference_manifest.get("projection_sha256") != projection_manifest.get("projection_sha256"):
        raise RealRunError("reference_freeze_changed")
    _, reference_bytes = _read_private_file(root / REFERENCE_PATH, root)
    if _sha256(reference_bytes) != reference_manifest.get("reference_sha256"):
        raise RealRunError("reference_freeze_changed")
    _, event_reference_bytes = _read_private_file(root / REFERENCE_EVENTS_PATH, root)
    if _sha256(event_reference_bytes) != reference_manifest.get("event_reference_sha256"):
        raise RealRunError("reference_freeze_changed")
    return projection_manifest, reference_manifest


def _event_evidence(projection: dict[str, Any]) -> str:
    parts: list[str] = []
    for key, label in (
        ("native_record_kind", "native_record_kind"),
        ("role", "role"),
        ("tool_name", "tool"),
        ("result_status", "result_status"),
    ):
        value = projection.get(key)
        if value is not None:
            parts.append(f"[{label}: {value}]")
    if projection.get("exit_status") is not None:
        parts.append(f"[tool exit_code: {projection['exit_status']}]")
        if projection["exit_status"] != 0:
            parts.append("[tool is_error: true]")
    if projection.get("stop_reason") is not None:
        parts.append(f"[stop_reason: {projection['stop_reason']}]")
    for key in ("reason", "error_message"):
        value = projection.get(key)
        if value:
            parts.append(f"[{key}: {value}]")
    if projection.get("result_status") == "error":
        parts.append("[tool is_error: true]")
    if projection.get("visible_text"):
        parts.append(projection["visible_text"])
    return "\n".join(parts)


def _events_from_rows(
    projection_rows: list[dict[str, Any]],
    labels: dict[str, dict[str, dict[str, Any]]],
) -> list[benchmark.Event]:
    cleaned = _validate_projection_rows(projection_rows)
    if {row["id"] for row in cleaned} != set(labels):
        raise RealRunError("reference_coverage_mismatch")
    cleaned.sort(key=lambda row: (row["session_id"], row["event_order"], row["id"]))
    events: list[benchmark.Event] = []
    for index, row in enumerate(cleaned):
        projection = row["projection"]
        # Event requires a hash, so retain a digest of the sanitized projection,
        # never a hash derived from or associated with native log bytes.
        native_hash = _sha256(_canonical_json(projection))
        kind = projection["event_kind"]
        if projection.get("role") is not None:
            kind = f"{kind}:{projection['role']}"
        evidence = _event_evidence(projection)
        events.append(benchmark.Event(
            index=index,
            event_id=row["id"],
            split="real_private",
            origin="reviewed_projection",
            harness=row["harness"],
            native_format=row["native_format"],
            native_sha256=native_hash,
            session_id=row["session_id"],
            event_order=row["event_order"],
            timestamp=(row["arrival_ms"] / 1000.0) if row["arrival_ms"] is not None else None,
            gold=labels[row["id"]],
            event_kind=kind,
            evidence_text=evidence,
            malformed_native=False,
            oversized_native=False,
        ))
    return events


def _plan_requests(strategy: str, mode: str, events: list[benchmark.Event]) -> int:
    if strategy == "rules":
        return 0
    if mode == "event-triggered":
        triggered = 0
        for event in events:
            causal = benchmark.context_events(event, events, "prefix")
            if benchmark.should_trigger_model(benchmark.rule_classify(event, causal)):
                triggered += 1
        return triggered * REMOTE_REPETITIONS
    if mode == "microbatch":
        batch_count = len(benchmark.build_microbatches(events, 4, 500.0))
    else:
        batch_count = len(events)
    return batch_count * REMOTE_REPETITIONS


def _benchmark_args(
    strategy: str,
    mode: str,
    context: str,
    *,
    max_requests: int,
    max_context_bytes: int,
    timeout: float,
    llm_model: str,
) -> argparse.Namespace:
    return argparse.Namespace(
        strategy=strategy,
        mode=mode,
        context=context,
        repeats=1 if strategy == "rules" else REMOTE_REPETITIONS,
        jev_model="typesafe/jev-latest",
        llm_model=llm_model,
        max_requests=max_requests,
        batch_size=4,
        batch_window_ms=500.0,
        max_context_bytes=max_context_bytes,
        timeout=timeout,
        llm_endpoint="/v1/chat/completions",
        input_price_per_1m=None,
        output_price_per_1m=None,
    )


def _response_usage(response: Any) -> dict[str, int]:
    usage = response.get("usage") if isinstance(response, dict) else None
    if not isinstance(usage, dict):
        return {}
    names = {
        "input_tokens": ("input_tokens", "prompt_tokens"),
        "output_tokens": ("output_tokens", "completion_tokens"),
    }
    captured: dict[str, int] = {}
    for output_name, keys in names.items():
        for key in keys:
            value = usage.get(key)
            if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
                captured[output_name] = value
                break
    return captured


def _response_id(response: Any) -> str | None:
    value = response.get("id") if isinstance(response, dict) else None
    if isinstance(value, str) and value and len(value) <= 256 and not CONTROL_RE.search(value):
        return value
    return None


def _response_model(response: Any) -> str | None:
    value = response.get("model") if isinstance(response, dict) else None
    if isinstance(value, str) and value and len(value) <= 256 and not CONTROL_RE.search(value):
        return value
    return None


def _run_remote_sessions(
    args: argparse.Namespace,
    events: list[benchmark.Event],
    corpus_sha256: str,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]]]:
    groups = benchmark._session_events(events)
    sessions = list(groups.items())
    thread_state = threading.local()
    ledger_lock = threading.Lock()
    request_ledger: list[dict[str, Any]] = []
    original_post_json = benchmark._post_json

    def audited_post_json(url: str, api_key: str, payload: dict[str, Any], timeout: float):
        session_id = getattr(thread_state, "session_id", None)
        request_index = getattr(thread_state, "request_index", None)
        if session_id is not None and request_index is not None:
            thread_state.request_index = request_index + 1
            request_started = _utc_now()
            try:
                response, rtt_ms, payload_bytes = original_post_json(url, api_key, payload, timeout)
            except Exception as exc:
                failure_code = getattr(exc, "code", "request_failed")
                record = {
                    "session_id": session_id,
                    "request_index": request_index,
                    "status": "failed",
                    "failure_code": failure_code if isinstance(failure_code, str) else "request_failed",
                    "request_started_at_utc": request_started,
                    "response_received_at_utc": _utc_now(),
                    "request_model": payload.get("model") if isinstance(payload.get("model"), str) else None,
                }
                with ledger_lock:
                    request_ledger.append(record)
                raise
            record = {
                "session_id": session_id,
                "request_index": request_index,
                "status": "response",
                "request_started_at_utc": request_started,
                "response_received_at_utc": _utc_now(),
                "request_model": payload.get("model") if isinstance(payload.get("model"), str) else None,
                "response_id": _response_id(response),
                "response_model": _response_model(response),
                "usage": _response_usage(response),
                "rtt_ms": rtt_ms,
                "request_body_bytes": payload_bytes,
            }
            with ledger_lock:
                request_ledger.append(record)
            return response, rtt_ms, payload_bytes
        return original_post_json(url, api_key, payload, timeout)

    def run_one(session_id: str, session_events: list[benchmark.Event]):
        thread_state.session_id = session_id
        thread_state.request_index = 0
        return benchmark.run_benchmark(args, session_events, corpus_sha256)

    benchmark._post_json = audited_post_json
    try:
        with ThreadPoolExecutor(max_workers=MAX_SESSION_WORKERS) as pool:
            futures = [pool.submit(run_one, session_id, session_events) for session_id, session_events in sessions]
            results = [future.result() for future in futures]
    except benchmark.BenchmarkError:
        raise RealRunError("benchmark_failed") from None
    except Exception:
        raise RealRunError("remote_run_failed") from None
    finally:
        benchmark._post_json = original_post_json

    combined_rows: list[dict[str, Any]] = []
    combined_calls: list[dict[str, Any]] = []
    combined_failures: list[dict[str, Any]] = []
    session_summaries: list[dict[str, Any]] = []
    ledger_by_session: dict[str, list[dict[str, Any]]] = {}
    for item in request_ledger:
        ledger_by_session.setdefault(item["session_id"], []).append(item)
    for (session_id, session_events), result in zip(sessions, results, strict=True):
        rows, summary, calls, failures = result
        session_ledger = sorted(
            ledger_by_session.get(session_id, []), key=lambda item: item["request_index"],
        )
        request_index = 0
        for call in calls:
            if call.get("request") is not True:
                continue
            if request_index >= len(session_ledger):
                raise RealRunError("response_ledger_incomplete")
            ledger_row = session_ledger[request_index]
            if ledger_row["status"] == "response":
                call["request_started_at_utc"] = ledger_row["request_started_at_utc"]
                call["response_received_at_utc"] = ledger_row["response_received_at_utc"]
                call["response_id"] = ledger_row["response_id"]
                call["provider_response_model"] = ledger_row["response_model"]
                call["provider_usage"] = ledger_row["usage"]
            else:
                call["request_started_at_utc"] = ledger_row["request_started_at_utc"]
                call["response_received_at_utc"] = ledger_row["response_received_at_utc"]
                call["provider_failure_code"] = ledger_row["failure_code"]
            request_index += 1
        if request_index != len(session_ledger):
            raise RealRunError("response_ledger_mismatch")
        combined_rows.extend(rows)
        combined_calls.extend(calls)
        combined_failures.extend(failures)
        session_summaries.append({
            "event_count": len(session_events),
            "summary": summary,
            "failure_count": len(failures),
        })
    return combined_rows, combined_calls, combined_failures, session_summaries


def run_condition(
    condition: str,
    *,
    max_requests: int = DEFAULT_MAX_REQUESTS,
    max_context_bytes: int = MAX_CONTEXT_BYTES,
    timeout: float = 45.0,
    llm_model: str = "",
) -> dict[str, Any]:
    if condition not in CONDITIONS:
        raise RealRunError("condition_invalid")
    if max_requests < 0 or not 1 <= max_context_bytes <= MAX_CONTEXT_BYTES or timeout <= 0:
        raise RealRunError("run_arguments_invalid")
    strategy, mode, context = CONDITIONS[condition]
    # Real-only prompt clarifications; the archived runner/source stays intact.
    benchmark.DESCRIPTIONS = {dimension: dict(criteria) for dimension, criteria in benchmark.DESCRIPTIONS.items()}
    benchmark.DESCRIPTIONS["activity"]["completion"] = REAL_COMPLETION_DESCRIPTION
    benchmark.DESCRIPTIONS["phase"]["ending"] = REAL_ENDING_DESCRIPTION
    benchmark.DESCRIPTIONS["anomaly"][True] = REAL_ANOMALY_DESCRIPTION
    if strategy == "llm" and llm_model != HAIKU_MODEL:
        raise RealRunError("explicit_haiku_model_required")

    _thermal_checkpoint()
    root = _private_root()
    projection_manifest, reference_manifest = _verify_reference_freezes(root)
    _, projection_bytes = _read_private_file(root / PROJECTION_PATH, root)
    projection_rows = _validate_projection_rows(
        _read_jsonl_bytes(projection_bytes, "invalid_projection_jsonl"),
    )
    event_ids = {row["id"] for row in projection_rows}
    del projection_rows, projection_bytes
    _, reference_bytes = _read_private_file(root / REFERENCE_PATH, root)
    reference_rows = _read_jsonl_bytes(reference_bytes, "invalid_reference_jsonl")
    labels = _validate_gold_rows(reference_rows, event_ids)
    del reference_rows, reference_bytes
    _, projection_bytes = _read_private_file(root / PROJECTION_PATH, root)
    projection_rows = _read_jsonl_bytes(projection_bytes, "invalid_projection_jsonl")
    events = _events_from_rows(projection_rows, labels)
    del projection_rows, projection_bytes
    _thermal_checkpoint()

    _thermal_checkpoint()
    planned_requests = _plan_requests(strategy, mode, events)
    _thermal_checkpoint()
    if planned_requests > max_requests:
        raise RealRunError("request_cap_too_low")
    if strategy != "rules":
        try:
            benchmark._api_credentials()
        except Exception:
            raise RealRunError("remote_credentials_unavailable") from None

    benchmark_args = _benchmark_args(
        strategy, mode, context,
        max_requests=max_requests,
        max_context_bytes=max_context_bytes,
        timeout=timeout,
        llm_model=llm_model,
    )
    _thermal_checkpoint()
    execution_started = time.perf_counter()
    if strategy == "rules":
        try:
            rows, session_summary, calls, failures = benchmark.run_benchmark(
                benchmark_args, events, projection_manifest["projection_sha256"],
            )
        except Exception:
            raise RealRunError("benchmark_failed") from None
        session_summaries = [{
            "event_count": len(events),
            "summary": session_summary,
            "failure_count": len(failures),
        }]
    else:
        rows, calls, failures, session_summaries = _run_remote_sessions(
            benchmark_args, events, projection_manifest["projection_sha256"],
        )
    execution_wall_seconds = time.perf_counter() - execution_started
    _thermal_checkpoint()

    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    runs_dir = root / "runs"
    runs_dir.mkdir(mode=0o700, exist_ok=True)
    os.chmod(runs_dir, 0o700)
    output_dir = runs_dir / f"{run_id}-{condition}"
    output_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    os.chmod(output_dir, 0o700)
    summary = {
        "schema": 1,
        "run_id": run_id,
        "condition": condition,
        "strategy": strategy,
        "mode": mode,
        "context": context,
        "requested_model": (
            "deterministic-rules" if strategy == "rules"
            else HAIKU_MODEL if strategy == "llm"
            else "typesafe/jev-latest"
        ),
        "event_count": len(events),
        "session_count": len({event.session_id for event in events}),
        "planned_request_count": planned_requests,
        "request_count": sum(call.get("request") is True for call in calls),
        "request_failure_count": sum(call.get("failed") is not None for call in calls),
        "failure_record_count": len(failures),
        "session_concurrency": 1 if strategy == "rules" else MAX_SESSION_WORKERS,
        "remote_repetitions": 1 if strategy == "rules" else REMOTE_REPETITIONS,
        "execution_wall_seconds": execution_wall_seconds,
        "scored_event_rows_per_second": len(rows) / execution_wall_seconds,
        "projection_sha256": projection_manifest["projection_sha256"],
        "reference_sha256": reference_manifest["reference_sha256"],
        "projection_implementation_sha256": projection_manifest["projection_implementation_sha256"],
        "sessions": session_summaries,
        "failures": failures,
    }
    _thermal_checkpoint()
    predictions_bytes = b"".join(_canonical_json(row) + b"\n" for row in rows)
    _write_private(output_dir / "predictions.jsonl", predictions_bytes, root)
    _write_private(output_dir / "calls.json", _canonical_json(calls) + b"\n", root)
    _write_private(output_dir / "summary.json", _canonical_json(summary) + b"\n", root)
    _thermal_checkpoint()
    return {
        "condition": condition,
        "event_count": len(events),
        "session_count": summary["session_count"],
        "planned_request_count": planned_requests,
        "output_dir": output_dir,
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    projection = commands.add_parser("freeze-projection")
    projection.add_argument("path", type=Path)
    projection.add_argument(
        "--reviewed", action="store_true",
        help="Manager attests that the sanitized projection content was reviewed locally",
    )
    reference = commands.add_parser("freeze-reference")
    reference.add_argument("path", type=Path)
    run = commands.add_parser("run")
    run.add_argument("--condition", required=True, choices=tuple(CONDITIONS))
    run.add_argument("--max-requests", type=int, default=DEFAULT_MAX_REQUESTS)
    run.add_argument("--max-context-bytes", type=int, default=MAX_CONTEXT_BYTES)
    run.add_argument("--timeout", type=float, default=45.0)
    run.add_argument("--llm-model", default="")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "freeze-projection":
            result = freeze_projection(args.path, reviewed_sanitized_content=args.reviewed)
        elif args.command == "freeze-reference":
            result = freeze_reference(args.path)
        else:
            result = run_condition(
                args.condition,
                max_requests=args.max_requests,
                max_context_bytes=args.max_context_bytes,
                timeout=args.timeout,
                llm_model=args.llm_model,
            )
        print(json.dumps({"status": "ok", **{
            key: value for key, value in result.items() if key != "output_dir"
        }}, sort_keys=True))
        return 0
    except RealRunError as exc:
        print(f"real-run: {exc.code}", file=sys.stderr)
        return 2
    except OSError:
        print("real-run: filesystem_error", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
