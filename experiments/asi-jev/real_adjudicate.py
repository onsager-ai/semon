#!/usr/bin/env python3
"""Blind third-person adjudication for frozen private annotations."""

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

import real_label


ANNOTATORS = ("A", "B")
STATE_PATH = "annotations/adjudication-manifest.json"
STREAM_PATH = "annotations/C-streaming.jsonl"
OFFLINE_PATH = "annotations/C-offline.jsonl"
REFERENCE_PATH = "adjudicated-reference.jsonl"
UNKNOWN_GOLD = dict(real_label.UNKNOWN_GOLD)


class AdjudicationError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _checkpoint() -> None:
    try:
        real_label._checkpoint()
    except Exception as exc:
        raise AdjudicationError(getattr(exc, "code", "thermal_unavailable")) from None


def _annotation_rows(data: bytes) -> list[dict[str, Any]]:
    rows = []
    for line in data.splitlines():
        if line.strip():
            row = real_label._json(line, "annotation_log_invalid")
            if not isinstance(row, dict):
                raise AdjudicationError("annotation_log_invalid")
            rows.append(row)
    return rows


def _inputs(root: Path) -> tuple[list[dict[str, Any]], str, str, dict[str, dict[str, dict[str, Any]]], dict[str, str]]:
    rows, projection_hash, projection_manifest_hash = real_label._projection(root)
    label_state = real_label._state(root, projection_hash, projection_manifest_hash)
    if not label_state["streaming_sha256"]:
        raise AdjudicationError("streaming_not_frozen")
    ids = {row["id"] for row in rows}
    streams: dict[str, dict[str, dict[str, Any]]] = {}
    offlines: dict[str, dict[str, dict[str, Any]]] = {}
    hashes: dict[str, str] = {}
    stream_hashes = {}
    for annotator in ANNOTATORS:
        stream_data = real_label._read(root / f"annotations/{annotator}-streaming.jsonl", root)
        offline_data = real_label._read(root / f"annotations/{annotator}-offline.jsonl", root)
        stream_records = _annotation_rows(stream_data)
        offline_records = _annotation_rows(offline_data)
        streams[annotator] = real_label._stream_labels(stream_records, ids)
        offlines[annotator] = real_label._offline_rows(offline_records, ids, streams[annotator])
        if set(streams[annotator]) != ids or set(offlines[annotator]) != ids:
            raise AdjudicationError("source_annotation_coverage_incomplete")
        hashes[f"{annotator}-streaming"] = hashlib.sha256(stream_data).hexdigest()
        hashes[f"{annotator}-offline"] = hashlib.sha256(offline_data).hexdigest()
        stream_hashes[annotator] = hashes[f"{annotator}-streaming"]
    if (label_state.get("streaming_annotation_sha256") != stream_hashes
            or label_state["streaming_sha256"] != hashlib.sha256(real_label._canonical(stream_hashes)).hexdigest()):
        raise AdjudicationError("source_stream_freeze_changed")
    return rows, projection_hash, projection_manifest_hash, {
        "streaming": streams, "offline": offlines,
    }, hashes


def _state(root: Path, projection_hash: str, projection_manifest_hash: str, source_hashes: dict[str, str]) -> dict[str, Any]:
    path = root / STATE_PATH
    initial = {
        "schema": 1,
        "projection_sha256": projection_hash,
        "projection_manifest_sha256": projection_manifest_hash,
        "source_annotation_sha256": source_hashes,
        "causal_sha256": None,
        "offline_sha256": None,
        "reference_sha256": None,
    }
    try:
        data = real_label._read(path, root)
    except real_label.LabelError as exc:
        if exc.code != "input_must_be_in_private":
            raise AdjudicationError(exc.code) from None
        if any((root / item).exists() or (root / item).is_symlink() for item in (STREAM_PATH, OFFLINE_PATH, REFERENCE_PATH)):
            raise AdjudicationError("adjudication_state_missing")
        real_label._write(path, real_label._canonical(initial) + b"\n", root)
        data = real_label._read(path, root)
    state = real_label._json(data, "adjudication_manifest_invalid")
    if (not isinstance(state, dict) or set(state) != set(initial)
            or any(state.get(key) != initial[key] for key in (
                "schema", "projection_sha256", "projection_manifest_sha256", "source_annotation_sha256",
            ))):
        raise AdjudicationError("source_annotation_freeze_changed")
    return state


@contextlib.contextmanager
def _decisions(root: Path, frame: str) -> Iterator[tuple[int, list[dict[str, Any]], bytes]]:
    path = root / (STREAM_PATH if frame == "streaming" else OFFLINE_PATH)
    directory = path.parent
    fd = -1
    _checkpoint()
    try:
        if directory.is_symlink() or not directory.is_dir():
            raise AdjudicationError("annotation_directory_invalid")
        fd = os.open(path, os.O_RDWR | os.O_APPEND | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
        os.fchmod(fd, 0o600)
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise AdjudicationError("decision_log_invalid")
        fcntl.flock(fd, fcntl.LOCK_EX)
    except AdjudicationError:
        if fd >= 0:
            os.close(fd)
        raise
    except OSError:
        if fd >= 0:
            os.close(fd)
        raise AdjudicationError("decision_log_unavailable") from None
    finally:
        _checkpoint()
    try:
        data = bytearray()
        os.lseek(fd, 0, os.SEEK_SET)
        while len(data) <= real_label.MAX_FILE_BYTES:
            _checkpoint()
            try:
                chunk = os.read(fd, min(65_536, real_label.MAX_FILE_BYTES + 1 - len(data)))
            except OSError:
                raise AdjudicationError("decision_log_read_failed") from None
            finally:
                _checkpoint()
            if not chunk:
                break
            data.extend(chunk)
        if len(data) > real_label.MAX_FILE_BYTES:
            raise AdjudicationError("decision_log_too_large")
        yield fd, _annotation_rows(bytes(data)), bytes(data)
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


def _append(fd: int, size: int, rows: list[dict[str, Any]]) -> bytes:
    data = b"".join(real_label._canonical(row) + b"\n" for row in rows)
    if size + len(data) > real_label.MAX_FILE_BYTES:
        raise AdjudicationError("decision_log_too_large")
    offset = 0
    while offset < len(data):
        _checkpoint()
        try:
            offset += os.write(fd, data[offset:offset + 65_536])
        except OSError:
            raise AdjudicationError("decision_log_write_failed") from None
        finally:
            _checkpoint()
    return data


def _decision_map(records: list[dict[str, Any]], ids: set[str]) -> tuple[dict[str, dict[str, Any]], list[str]]:
    labels = {}
    order = []
    for row in records:
        event_id = row.get("id")
        if set(row) != {"id", "gold"} or not isinstance(event_id, str) or event_id not in ids or event_id in labels:
            raise AdjudicationError("decision_log_invalid")
        labels[event_id] = real_label._label(row["gold"])
        order.append(event_id)
    return labels, order


def _groups(rows: list[dict[str, Any]]) -> list[tuple[str, list[dict[str, Any]]]]:
    groups: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        groups.setdefault(row["session_id"], []).append(row)
    return list(groups.items())


def _causal_parts(rows: list[dict[str, Any]], source: dict[str, dict[str, Any]]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    agree, disagree = [], []
    for row in rows:
        (agree if source["A"][row["id"]] == source["B"][row["id"]] else disagree).append(row)
    return agree, disagree


def _causal_log(
    root: Path, state: dict[str, Any], rows: list[dict[str, Any]], source: dict[str, dict[str, Any]],
) -> tuple[dict[str, dict[str, Any]], dict[str, Any] | None, bytes]:
    agree, disagree = _causal_parts(rows, source)
    ids = {row["id"] for row in rows}
    with _decisions(root, "streaming") as (fd, records, data):
        if (state["causal_sha256"] is not None
                and hashlib.sha256(data).hexdigest() != state["causal_sha256"]):
            raise AdjudicationError("causal_freeze_changed")
        labels, order = _decision_map(records, ids)
        agree_rows = [{"id": row["id"], "gold": source["A"][row["id"]]} for row in agree]
        agree_ids = [row["id"] for row in agree_rows]
        if order[:min(len(order), len(agree_ids))] != agree_ids[:len(order)] or len(order) > len(agree_ids) + len(disagree):
            raise AdjudicationError("causal_decisions_invalid")
        if len(order) < len(agree_ids):
            payload = agree_rows[len(order):]
            data += _append(fd, len(data), payload)
            labels.update({row["id"]: row["gold"] for row in payload})
            order.extend(row["id"] for row in payload)
        for row in agree_rows:
            if labels.get(row["id"]) != row["gold"]:
                raise AdjudicationError("causal_agreement_changed")
        decided = order[len(agree_ids):]
        disagree_ids = [row["id"] for row in disagree]
        if decided != disagree_ids[:len(decided)]:
            raise AdjudicationError("causal_decisions_out_of_order")
        target = disagree[len(decided)] if len(decided) < len(disagree) else None
        if state["causal_sha256"] is not None and hashlib.sha256(data).hexdigest() != state["causal_sha256"]:
            raise AdjudicationError("causal_freeze_changed")
        return labels, target, data


def _save_state(root: Path, state: dict[str, Any]) -> dict[str, Any]:
    real_label._write(root / STATE_PATH, real_label._canonical(state) + b"\n", root)
    return state


def _freeze_causal(root: Path, state: dict[str, Any], data: bytes) -> dict[str, Any]:
    digest = hashlib.sha256(data).hexdigest()
    if state["causal_sha256"] is not None:
        if state["causal_sha256"] != digest:
            raise AdjudicationError("causal_freeze_changed")
        return state
    return _save_state(root, {**state, "causal_sha256": digest})


def _causal_packet(rows: list[dict[str, Any]], target: dict[str, Any], source: dict[str, dict[str, Any]]) -> dict[str, Any]:
    packet = real_label._stream_packet(rows, target)
    packet["label_a"] = source["A"][target["id"]]
    packet["label_b"] = source["B"][target["id"]]
    return real_label._check_output(packet)


def _next_causal(root: Path, state: dict[str, Any], rows: list[dict[str, Any]], source: dict[str, dict[str, Any]]) -> dict[str, Any]:
    _, target, data = _causal_log(root, state, rows, source)
    if target is None:
        _freeze_causal(root, state, data)
        return {"status": "causal_complete"}
    return _causal_packet(rows, target, source)


def next_causal(*, private_root: str | Path | None = None) -> dict[str, Any]:
    root = real_label._root(private_root)
    rows, projection_hash, manifest_hash, sources, source_hashes = _inputs(root)
    state = _state(root, projection_hash, manifest_hash, source_hashes)
    return _next_causal(root, state, rows, sources["streaming"])


def submit_causal(label: Any, *, private_root: str | Path | None = None) -> dict[str, Any]:
    checked = real_label._label(label)
    root = real_label._root(private_root)
    rows, projection_hash, manifest_hash, sources, source_hashes = _inputs(root)
    state = _state(root, projection_hash, manifest_hash, source_hashes)
    if state["causal_sha256"] is not None:
        raise AdjudicationError("causal_decisions_frozen")
    _, target, data = _causal_log(root, state, rows, sources["streaming"])
    if target is None:
        raise AdjudicationError("causal_complete")
    with _decisions(root, "streaming") as (fd, _, current):
        _append(fd, len(current), [{"id": target["id"], "gold": checked}])
    return _next_causal(root, state, rows, sources["streaming"])


def _offline_parts(
    rows: list[dict[str, Any]], source: dict[str, dict[str, Any]],
) -> tuple[list[dict[str, Any]], list[tuple[str, list[dict[str, Any]]]]]:
    agrees, disputes = [], []
    for row in rows:
        (agrees if source["A"][row["id"]] == source["B"][row["id"]] else disputes).append(row)
    sessions = []
    for session_id, session in _groups(rows):
        current = [row for row in session if row in disputes]
        if current:
            sessions.append((session_id, current))
    return agrees, sessions


def _offline_log(
    root: Path, state: dict[str, Any], rows: list[dict[str, Any]], source: dict[str, dict[str, Any]],
) -> tuple[dict[str, dict[str, Any]], tuple[str, list[dict[str, Any]]] | None, bytes]:
    agrees, sessions = _offline_parts(rows, source)
    ids = {row["id"] for row in rows}
    with _decisions(root, "offline") as (fd, records, data):
        if (state["offline_sha256"] is not None
                and hashlib.sha256(data).hexdigest() != state["offline_sha256"]):
            raise AdjudicationError("offline_freeze_changed")
        labels, order = _decision_map(records, ids)
        agree_rows = [{"id": row["id"], "gold": source["A"][row["id"]]} for row in agrees]
        agree_ids = [row["id"] for row in agree_rows]
        if order[:min(len(order), len(agree_ids))] != agree_ids[:len(order)] or len(order) > len(agree_ids) + sum(len(session) for _, session in sessions):
            raise AdjudicationError("offline_decisions_invalid")
        if len(order) < len(agree_ids):
            payload = agree_rows[len(order):]
            data += _append(fd, len(data), payload)
            labels.update({row["id"]: row["gold"] for row in payload})
            order.extend(row["id"] for row in payload)
        for row in agree_rows:
            if labels.get(row["id"]) != row["gold"]:
                raise AdjudicationError("offline_agreement_changed")
        decided = order[len(agree_ids):]
        dispute_ids = [row["id"] for _, session in sessions for row in session]
        if decided != dispute_ids[:len(decided)]:
            raise AdjudicationError("offline_decisions_out_of_order")
        for _, session in sessions:
            session_ids = [row["id"] for row in session]
            count = len(set(session_ids) & set(decided))
            if count and count != len(session_ids):
                raise AdjudicationError("offline_session_partial")
            if count == 0:
                target = (session[0]["session_id"], session)
                break
        else:
            target = None
        if state["offline_sha256"] is not None and hashlib.sha256(data).hexdigest() != state["offline_sha256"]:
            raise AdjudicationError("offline_freeze_changed")
        return labels, target, data


def _offline_packet(
    session_id: str, session: list[dict[str, Any]], streaming: dict[str, dict[str, Any]],
    offline: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    packet = real_label._offline_packet(session_id, session, streaming)
    for event in packet["events"]:
        event_id = event["id"]
        a, b = offline["A"][event_id], offline["B"][event_id]
        event["offline_disagreement"] = a != b
        if a != b:
            event["offline_a"] = a
            event["offline_b"] = b
    packet["offline_override_policy"] = "Submit one explicit full five-dimension label for every disputed event; use taxonomy unknown values when evidence is insufficient."
    return real_label._check_output(packet)


def _reference(root: Path, state: dict[str, Any], rows: list[dict[str, Any]], streaming: dict[str, dict[str, Any]], offline: dict[str, dict[str, Any]], offline_data: bytes) -> dict[str, Any]:
    state = state if state["offline_sha256"] is not None else _save_state(
        root, {**state, "offline_sha256": hashlib.sha256(offline_data).hexdigest()},
    )
    records = [
        {"id": row["id"], "gold": {"streaming": streaming[row["id"]], "offline": offline[row["id"]]}}
        for row in rows
    ]
    data = b"".join(real_label._canonical(record) + b"\n" for record in records)
    if len(data) > real_label.MAX_FILE_BYTES:
        raise AdjudicationError("reference_too_large")
    digest = hashlib.sha256(data).hexdigest()
    if state["reference_sha256"] is not None and state["reference_sha256"] != digest:
        raise AdjudicationError("reference_freeze_changed")
    path = real_label._root(root) / REFERENCE_PATH
    if path.exists() or path.is_symlink():
        if state["reference_sha256"] is None:
            raise AdjudicationError("reference_already_exists")
        current = real_label._read(path, root)
        if hashlib.sha256(current).hexdigest() != digest:
            raise AdjudicationError("reference_already_exists")
    else:
        state = _save_state(root, {**state, "reference_sha256": digest})
        real_label._write(path, data, root)
    return {"status": "adjudication_complete", "events": len(records)}


def next_offline(*, private_root: str | Path | None = None) -> dict[str, Any]:
    root = real_label._root(private_root)
    rows, projection_hash, manifest_hash, sources, source_hashes = _inputs(root)
    state = _state(root, projection_hash, manifest_hash, source_hashes)
    causal, causal_target, causal_data = _causal_log(root, state, rows, sources["streaming"])
    if causal_target is not None:
        return {"status": "causal_incomplete"}
    state = _freeze_causal(root, state, causal_data)
    _, offline_target, offline_data = _offline_log(root, state, rows, sources["offline"])
    if offline_target is None:
        return _reference(root, state, rows, causal, _offline_decisions_from_data(offline_data, rows), offline_data)
    return _offline_packet(offline_target[0], _session_rows(rows, offline_target[0]), causal, sources["offline"])


def _offline_decisions_from_data(data: bytes, rows: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    return _decision_map(_annotation_rows(data), {row["id"] for row in rows})[0]


def _session_rows(rows: list[dict[str, Any]], session_id: str) -> list[dict[str, Any]]:
    return [row for row in rows if row["session_id"] == session_id]


def submit_offline(
    session_id: str, overrides: Any, *, reviewed: bool, private_root: str | Path | None = None,
) -> dict[str, Any]:
    if not reviewed or not isinstance(overrides, dict):
        raise AdjudicationError("offline_review_required")
    root = real_label._root(private_root)
    rows, projection_hash, manifest_hash, sources, source_hashes = _inputs(root)
    state = _state(root, projection_hash, manifest_hash, source_hashes)
    causal, causal_target, causal_data = _causal_log(root, state, rows, sources["streaming"])
    if causal_target is not None:
        raise AdjudicationError("causal_freeze_incomplete")
    state = _freeze_causal(root, state, causal_data)
    labels, target, data = _offline_log(root, state, rows, sources["offline"])
    if target is None:
        raise AdjudicationError("offline_complete")
    current_id, disputes = target
    if session_id != current_id:
        raise AdjudicationError("offline_session_mismatch")
    dispute_ids = {row["id"] for row in disputes}
    if set(overrides) != dispute_ids:
        raise AdjudicationError("offline_override_coverage")
    checked = {}
    for event_id, label in overrides.items():
        if not isinstance(event_id, str) or event_id not in dispute_ids:
            raise AdjudicationError("offline_override_invalid")
        checked[event_id] = real_label._label(label)
    decisions = [{"id": row["id"], "gold": checked[row["id"]]} for row in disputes]
    with _decisions(root, "offline") as (fd, _, current):
        _append(fd, len(current), decisions)
    return next_offline(private_root=root)


def _stdin_json() -> Any:
    data = sys.stdin.buffer.read(real_label.MAX_INPUT_BYTES + 1)
    if len(data) > real_label.MAX_INPUT_BYTES:
        raise AdjudicationError("annotation_input_too_large")
    return real_label._json(data, "annotation_input_invalid")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("next")
    commands.add_parser("submit")
    commands.add_parser("offline-next")
    offline = commands.add_parser("offline-submit")
    offline.add_argument("--session-id", required=True)
    offline.add_argument("--reviewed", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.command == "next":
            packet = next_causal()
        elif args.command == "submit":
            packet = submit_causal(_stdin_json())
        elif args.command == "offline-next":
            packet = next_offline()
        else:
            packet = submit_offline(args.session_id, _stdin_json(), reviewed=args.reviewed)
        print(json.dumps(packet, ensure_ascii=False, sort_keys=True, allow_nan=False))
        return 0
    except (AdjudicationError, real_label.LabelError) as exc:
        print(f"error: {getattr(exc, 'code', 'adjudication_failed')}", file=sys.stderr)
        return 2
    except Exception:
        print("error: adjudication_failed", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
