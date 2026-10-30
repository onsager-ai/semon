#!/usr/bin/env python3
"""Blind local annotation interface for reviewed real-session projections."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import stat
import sys
from pathlib import Path
from typing import Any, Iterator

import benchmark
import real_pipeline
import real_run


MAX_FILE_BYTES = 10_000_000
MAX_CONTEXT_BYTES = 8_192
MAX_INPUT_BYTES = 64_000
MAX_OUTPUT_BYTES = 10_000_000
PROJECTION_PATH = "frozen-projection.jsonl"
PROJECTION_MANIFEST_PATH = "manifests/real-projection.json"
STATE_PATH = "annotations/labeling-manifest.json"
ANNOTATORS = ("A", "B")
PROTOCOL = {
    "completion": "The assistant reports the current requested work or turn complete; a stop marker alone is insufficient.",
    "ending": "Native evidence establishes session termination; a turn stop or last captured row alone is insufficient.",
    "anomaly": "The event is malformed, unexpected, or shows an anomalous repeated pattern; an ordinary tool failure alone is insufficient.",
}
UNKNOWN_GOLD = {
    "activity": "unknown", "failure_category": "unknown", "retry_loop": "none",
    "anomaly": False, "phase": "unknown",
}


class LabelError(Exception):
    """Safe error code for the local annotation interface."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _checkpoint() -> None:
    try:
        real_run._thermal_checkpoint()
    except Exception as exc:
        code = getattr(exc, "code", None)
        raise LabelError("thermal_limit" if code == "thermal_limit" else "thermal_unavailable") from None


def _json(data: bytes, code: str) -> Any:
    def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise LabelError(code)
            result[key] = value
        return result

    try:
        return json.loads(data, object_pairs_hook=unique)
    except (json.JSONDecodeError, UnicodeDecodeError):
        raise LabelError(code) from None


def _canonical(value: Any) -> bytes:
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")
    except (TypeError, ValueError):
        raise LabelError("annotation_invalid") from None


def _root(private_root: str | Path | None) -> Path:
    root = Path(private_root if private_root is not None else real_pipeline.PRIVATE_ROOT).absolute()
    if root.is_symlink():
        raise LabelError("private_root_invalid")
    try:
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(root, 0o700)
        if not stat.S_ISDIR(root.stat().st_mode):
            raise LabelError("private_root_invalid")
    except OSError:
        raise LabelError("private_root_unavailable") from None
    return root


def _read(path: Path, root: Path) -> bytes:
    _checkpoint()
    try:
        _, data = real_run._read_private_file(path, root)
        return data
    except real_run.RealRunError as exc:
        raise LabelError(exc.code) from None
    finally:
        _checkpoint()


def _write(path: Path, data: bytes, root: Path) -> None:
    _checkpoint()
    try:
        real_run._write_private(path, data, root)
    except real_run.RealRunError as exc:
        raise LabelError(exc.code) from None
    finally:
        _checkpoint()


def _projection(root: Path) -> tuple[list[dict[str, Any]], str, str]:
    manifest_bytes = _read(root / PROJECTION_MANIFEST_PATH, root)
    manifest = _json(manifest_bytes, "projection_manifest_invalid")
    if not isinstance(manifest, dict):
        raise LabelError("projection_manifest_invalid")
    _checkpoint()
    try:
        hashes = real_run._freeze_hashes()
    except real_run.RealRunError as exc:
        raise LabelError(exc.code) from None
    finally:
        _checkpoint()
    expected = {
        "schema", "reviewed_sanitized_content", "source_projection_sha256",
        "projection_sha256", "event_count", "session_count", *hashes,
    }
    if (set(manifest) != expected or manifest.get("schema") != 1
            or manifest.get("reviewed_sanitized_content") is not True
            or any(manifest.get(key) != value for key, value in hashes.items())):
        raise LabelError("projection_freeze_changed")
    data = _read(root / PROJECTION_PATH, root)
    projection_hash = hashlib.sha256(data).hexdigest()
    if (manifest.get("projection_sha256") != projection_hash
            or manifest.get("source_projection_sha256") != projection_hash):
        raise LabelError("projection_freeze_changed")
    try:
        rows = real_run._validate_projection_rows(
            real_run._read_jsonl_bytes(data, "projection_jsonl_invalid"),
        )
    except real_run.RealRunError as exc:
        raise LabelError(exc.code) from None
    if (manifest.get("event_count") != len(rows)
            or manifest.get("session_count") != len({row["session_id"] for row in rows})):
        raise LabelError("projection_freeze_changed")
    rows.sort(key=lambda row: (row["session_id"], row["event_order"], row["id"]))
    return rows, projection_hash, hashlib.sha256(manifest_bytes).hexdigest()


def _state(root: Path, projection_hash: str, manifest_hash: str) -> dict[str, Any]:
    path = root / STATE_PATH
    initial = {
        "schema": 1, "projection_sha256": projection_hash,
        "projection_manifest_sha256": manifest_hash,
        "streaming_sha256": None, "streaming_annotation_sha256": None,
    }
    try:
        data = _read(path, root)
    except LabelError as exc:
        if exc.code != "input_must_be_in_private":
            raise
        _write(path, _canonical(initial) + b"\n", root)
        data = _read(path, root)
    state = _json(data, "labeling_manifest_invalid")
    if (not isinstance(state, dict) or set(state) != set(initial)
            or state.get("schema") != 1
            or state.get("projection_sha256") != projection_hash
            or state.get("projection_manifest_sha256") != manifest_hash):
        raise LabelError("projection_freeze_changed")
    return state


@contextlib.contextmanager
def _log(root: Path, annotator: str, frame: str) -> Iterator[tuple[int, list[dict[str, Any]], bytes]]:
    if annotator not in ANNOTATORS or frame not in {"streaming", "offline"}:
        raise LabelError("annotation_arguments_invalid")
    directory = root / "annotations"
    _checkpoint()
    try:
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(directory, 0o700)
        info = directory.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise LabelError("annotation_storage_invalid")
        directory.resolve().relative_to(root.resolve())
    except LabelError:
        raise
    except (OSError, ValueError):
        raise LabelError("annotation_storage_invalid") from None
    finally:
        _checkpoint()

    path = directory / f"{annotator}-{frame}.jsonl"
    fd = -1
    _checkpoint()
    try:
        fd = os.open(path, os.O_RDWR | os.O_APPEND | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
        os.fchmod(fd, 0o600)
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise LabelError("annotation_log_invalid")
        fcntl.flock(fd, fcntl.LOCK_EX)
    except LabelError:
        if fd >= 0:
            os.close(fd)
        raise
    except OSError:
        if fd >= 0:
            os.close(fd)
        raise LabelError("annotation_storage_unavailable") from None
    finally:
        _checkpoint()

    try:
        data = bytearray()
        os.lseek(fd, 0, os.SEEK_SET)
        while len(data) <= MAX_FILE_BYTES:
            _checkpoint()
            try:
                chunk = os.read(fd, min(65_536, MAX_FILE_BYTES + 1 - len(data)))
            except OSError:
                raise LabelError("annotation_read_failed") from None
            finally:
                _checkpoint()
            if not chunk:
                break
            data.extend(chunk)
        if len(data) > MAX_FILE_BYTES:
            raise LabelError("annotation_log_too_large")
        records = []
        for line in data.splitlines():
            if line.strip():
                record = _json(line, "annotation_log_invalid")
                if not isinstance(record, dict):
                    raise LabelError("annotation_log_invalid")
                records.append(record)
        yield fd, records, bytes(data)
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


def _append(fd: int, size: int, rows: list[dict[str, Any]]) -> None:
    data = b"".join(_canonical(row) + b"\n" for row in rows)
    if size + len(data) > MAX_FILE_BYTES:
        raise LabelError("annotation_log_too_large")
    offset = 0
    while offset < len(data):
        _checkpoint()
        try:
            offset += os.write(fd, data[offset:offset + 65_536])
        except OSError:
            raise LabelError("annotation_write_failed") from None
        finally:
            _checkpoint()


def _label(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != set(benchmark.DIMENSIONS):
        raise LabelError("annotation_labels_incomplete")
    result = {}
    for dimension, allowed in benchmark.DIMENSIONS.items():
        item = value[dimension]
        if dimension == "anomaly":
            if not isinstance(item, bool):
                raise LabelError("annotation_label_invalid")
        elif not isinstance(item, str) or item not in allowed:
            raise LabelError("annotation_label_invalid")
        result[dimension] = item
    return result


def _stream_labels(records: list[dict[str, Any]], ids: set[str]) -> dict[str, dict[str, Any]]:
    labels = {}
    for row in records:
        event_id = row.get("id")
        if (set(row) != {"id", "gold"} or not isinstance(event_id, str)
                or event_id not in ids or event_id in labels):
            raise LabelError("annotation_log_invalid")
        labels[event_id] = _label(row["gold"])
    return labels


def _stream_snapshot(
    root: Path, state: dict[str, Any], rows: list[dict[str, Any]],
) -> tuple[dict[str, Any], dict[str, dict[str, dict[str, Any]]] | None]:
    ids = {row["id"] for row in rows}
    labels_by_annotator = {}
    hashes = {}
    for annotator in ANNOTATORS:
        with _log(root, annotator, "streaming") as (_, records, data):
            labels = _stream_labels(records, ids)
        if len(labels) != len(ids):
            return state, None
        labels_by_annotator[annotator] = labels
        hashes[annotator] = hashlib.sha256(data).hexdigest()
    stream_hash = hashlib.sha256(_canonical(hashes)).hexdigest()
    if state["streaming_sha256"] is not None:
        if (state["streaming_sha256"] != stream_hash
                or state["streaming_annotation_sha256"] != hashes):
            raise LabelError("annotation_freeze_changed")
    else:
        state = {**state, "streaming_sha256": stream_hash, "streaming_annotation_sha256": hashes}
        _write(root / STATE_PATH, _canonical(state) + b"\n", root)
    return state, labels_by_annotator


def _events(rows: list[dict[str, Any]]) -> list[benchmark.Event]:
    return real_run._events_from_rows(rows, {
        row["id"]: {"streaming": dict(UNKNOWN_GOLD), "offline": dict(UNKNOWN_GOLD)}
        for row in rows
    })


def _check_output(packet: dict[str, Any]) -> dict[str, Any]:
    if len(_canonical(packet)) > MAX_OUTPUT_BYTES:
        raise LabelError("annotation_output_too_large")
    return packet


def _stream_packet(rows: list[dict[str, Any]], target: dict[str, Any]) -> dict[str, Any]:
    prefix = [row for row in rows if row["session_id"] == target["session_id"] and row["event_order"] <= target["event_order"]]
    events = _events(prefix)
    target_event = next(event for event in events if event.event_id == target["id"])
    return _check_output({
        "status": "ready", "id": target["id"], "session_id": target["session_id"],
        "event_order": target["event_order"],
        "context": benchmark.format_context(target_event, events, "prefix", MAX_CONTEXT_BYTES),
        "protocol": PROTOCOL,
    })


def next_stream(annotator: str, *, private_root: str | Path | None = None) -> dict[str, Any]:
    if annotator not in ANNOTATORS:
        raise LabelError("annotation_arguments_invalid")
    root = _root(private_root)
    rows, projection_hash, manifest_hash = _projection(root)
    state = _state(root, projection_hash, manifest_hash)
    with _log(root, annotator, "streaming") as (_, records, _):
        labels = _stream_labels(records, {row["id"] for row in rows})
    target = next((row for row in rows if row["id"] not in labels), None)
    if target is None:
        _stream_snapshot(root, state, rows)
        return {"status": "stream_complete"}
    if state["streaming_sha256"] is not None:
        raise LabelError("annotation_freeze_changed")
    return _stream_packet(rows, target)


def submit_stream(
    annotator: str, label: Any, *, private_root: str | Path | None = None,
) -> dict[str, Any]:
    if annotator not in ANNOTATORS:
        raise LabelError("annotation_arguments_invalid")
    checked = _label(label)
    root = _root(private_root)
    rows, projection_hash, manifest_hash = _projection(root)
    state = _state(root, projection_hash, manifest_hash)
    if state["streaming_sha256"] is not None:
        raise LabelError("stream_labels_frozen")
    ids = {row["id"] for row in rows}
    with _log(root, annotator, "streaming") as (fd, records, data):
        labels = _stream_labels(records, ids)
        target = next((row for row in rows if row["id"] not in labels), None)
        if target is None:
            raise LabelError("stream_complete")
        _append(fd, len(data), [{"id": target["id"], "gold": checked}])
    next_target = next((row for row in rows if row["id"] not in labels and row["id"] != target["id"]), None)
    if next_target is None:
        _stream_snapshot(root, state, rows)
        return {"status": "stream_complete"}
    return _stream_packet(rows, next_target)


def _offline_rows(records: list[dict[str, Any]], ids: set[str], streaming: dict[str, dict[str, Any]]) -> dict[str, dict[str, Any]]:
    labels = {}
    for row in records:
        event_id = row.get("id")
        if (set(row) != {"id", "gold", "causal_gold"} or not isinstance(event_id, str) or event_id not in ids
                or event_id in labels or event_id not in streaming):
            raise LabelError("annotation_log_invalid")
        labels[event_id] = _label(row["gold"])
        if _label(row["causal_gold"]) != streaming[event_id]:
            raise LabelError("annotation_freeze_changed")
    return labels


def _session_groups(rows: list[dict[str, Any]]) -> list[tuple[str, list[dict[str, Any]]]]:
    groups: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        groups.setdefault(row["session_id"], []).append(row)
    return list(groups.items())


def _offline_packet(
    session_id: str, rows: list[dict[str, Any]], streaming: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    events = []
    for row in rows:
        projection = row["projection"]
        kind = projection["event_kind"]
        if projection.get("role") is not None:
            kind = f"{kind}:{projection['role']}"
        events.append({
            "id": row["id"], "event_order": row["event_order"],
            "harness": row["harness"], "native_format": row["native_format"],
            "event_kind": kind,
            "evidence_text": real_run._event_evidence(projection),
            "streaming_label": streaming[row["id"]],
        })
    return _check_output({"status": "ready", "session_id": session_id, "events": events, "protocol": PROTOCOL})


def _next_offline(
    root: Path, annotator: str, rows: list[dict[str, Any]], streaming: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    ids = {row["id"] for row in rows}
    with _log(root, annotator, "offline") as (_, records, _):
        labels = _offline_rows(records, ids, streaming)
    groups = _session_groups(rows)
    for session_id, session in groups:
        session_ids = {row["id"] for row in session}
        count = len(session_ids & set(labels))
        if count and count != len(session_ids):
            raise LabelError("offline_session_partial")
        if not count:
            return _offline_packet(session_id, session, streaming)
    return {"status": "offline_complete"}


def next_offline(annotator: str, *, private_root: str | Path | None = None) -> dict[str, Any]:
    if annotator not in ANNOTATORS:
        raise LabelError("annotation_arguments_invalid")
    root = _root(private_root)
    rows, projection_hash, manifest_hash = _projection(root)
    state = _state(root, projection_hash, manifest_hash)
    state, labels = _stream_snapshot(root, state, rows)
    if labels is None:
        return {"status": "streaming_incomplete"}
    streaming = labels[annotator]
    return _next_offline(root, annotator, rows, streaming)


def submit_offline(
    annotator: str, session_id: str, changed_labels: Any, *, reviewed: bool,
    private_root: str | Path | None = None,
) -> dict[str, Any]:
    if annotator not in ANNOTATORS or not reviewed or not isinstance(changed_labels, dict):
        raise LabelError("offline_review_required")
    root = _root(private_root)
    rows, projection_hash, manifest_hash = _projection(root)
    state = _state(root, projection_hash, manifest_hash)
    state, labels = _stream_snapshot(root, state, rows)
    if labels is None:
        raise LabelError("streaming_incomplete")
    streaming = labels[annotator]
    ids = {row["id"] for row in rows}
    changes = {}
    for event_id, label in changed_labels.items():
        if not isinstance(event_id, str) or event_id not in ids:
            raise LabelError("offline_changed_id_invalid")
        changes[event_id] = _label(label)

    with _log(root, annotator, "offline") as (fd, records, data):
        saved = _offline_rows(records, ids, streaming)
        groups = _session_groups(rows)
        current = None
        for candidate_session_id, session in groups:
            session_ids = {row["id"] for row in session}
            count = len(session_ids & set(saved))
            if count and count != len(session_ids):
                raise LabelError("offline_session_partial")
            if not count:
                current = (candidate_session_id, session)
                break
        if current is None:
            raise LabelError("offline_complete")
        current_session_id, session = current
        if session_id != current_session_id:
            raise LabelError("offline_session_mismatch")
        session_ids = {row["id"] for row in session}
        if not set(changes).issubset(session_ids):
            raise LabelError("offline_changed_id_invalid")
        append_rows = [{
            "id": row["id"], "gold": changes.get(row["id"], streaming[row["id"]]),
            "causal_gold": streaming[row["id"]],
        } for row in session]
        _append(fd, len(data), append_rows)
    return _next_offline(root, annotator, rows, streaming)


def _stdin_json() -> Any:
    try:
        data = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
    except OSError:
        raise LabelError("annotation_input_unavailable") from None
    if len(data) > MAX_INPUT_BYTES:
        raise LabelError("annotation_input_too_large")
    return _json(data, "annotation_input_invalid")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("next", "submit", "offline-next", "offline-submit"):
        command = commands.add_parser(name)
        command.add_argument("--annotator", choices=ANNOTATORS, required=True)
        if name == "offline-submit":
            command.add_argument("--session-id", required=True)
            command.add_argument("--reviewed", action="store_true", help="attest that the complete session was reviewed")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "next":
            packet = next_stream(args.annotator)
        elif args.command == "submit":
            packet = submit_stream(args.annotator, _stdin_json())
        elif args.command == "offline-next":
            packet = next_offline(args.annotator)
        else:
            packet = submit_offline(
                args.annotator, args.session_id, _stdin_json(), reviewed=args.reviewed,
            )
        print(json.dumps(packet, ensure_ascii=False, sort_keys=True, allow_nan=False))
        return 0
    except LabelError as exc:
        print(f"error: {exc.code}", file=sys.stderr)
        return 2
    except Exception:
        print("error: annotation_failed", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
