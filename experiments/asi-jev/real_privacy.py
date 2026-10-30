#!/usr/bin/env python3
"""Apply an approved whole-field redaction plan to local annotation inputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path
from typing import Any

import real_pipeline
import real_run


MAX_FILE_BYTES = 10_000_000
PACKET_RE = re.compile(r"session-[0-9]{2}\.jsonl\Z")
FIELDS = {"visible_text", "reason", "error_message"}
PLACEHOLDER = "[privacy review redacted]"
BACKUP_NAME = "preprivacy-review-bundle.jsonl"
MANIFEST_NAME = "privacy-transform.json"


class PrivacyError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _checkpoint() -> None:
    try:
        real_pipeline.thermal_checkpoint()
    except Exception as exc:
        code = getattr(exc, "code", None)
        raise PrivacyError("thermal_limit" if code == "thermal_limit" else "thermal_unavailable") from None


def _root(private_root: str | Path | None) -> Path:
    root = Path(private_root if private_root is not None else real_pipeline.PRIVATE_ROOT).absolute()
    if root.is_symlink():
        raise PrivacyError("private_root_invalid")
    if not root.is_dir():
        try:
            root.mkdir(mode=0o700, parents=True, exist_ok=True)
        except OSError:
            raise PrivacyError("private_root_invalid") from None
    try:
        os.chmod(root, 0o700)
    except OSError:
        raise PrivacyError("private_root_invalid") from None
    return root


def _read(path: Path, root: Path) -> bytes:
    _checkpoint()
    try:
        _, data = real_run._read_private_file(path, root)
        return data
    except real_run.RealRunError as exc:
        raise PrivacyError(exc.code) from None
    finally:
        _checkpoint()


def _write(path: Path, data: bytes, root: Path) -> None:
    _checkpoint()
    try:
        real_run._write_private(path, data, root)
    except real_run.RealRunError as exc:
        raise PrivacyError(exc.code) from None
    finally:
        _checkpoint()


def _parse(data: bytes, code: str) -> Any:
    def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result = {}
        for key, value in pairs:
            if key in result:
                raise PrivacyError(code)
            result[key] = value
        return result

    try:
        return json.loads(data, object_pairs_hook=unique)
    except (json.JSONDecodeError, UnicodeDecodeError):
        raise PrivacyError(code) from None


def _jsonl(data: bytes) -> list[dict[str, Any]]:
    rows = [_parse(line, "invalid_jsonl") for line in data.splitlines() if line.strip()]
    if any(not isinstance(row, dict) for row in rows):
        raise PrivacyError("invalid_jsonl")
    return rows


def _encode(rows: list[dict[str, Any]]) -> bytes:
    return b"".join((json.dumps(row, ensure_ascii=False) + "\n").encode("utf-8") for row in rows)


def _exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def apply_plan(plan_path: str | Path, *, private_root: str | Path | None = None) -> dict[str, int]:
    root = _root(private_root)
    frozen = root / "manifests" / "real-projection.json"
    bundle_path = root / "review-bundle.jsonl"
    backup_path = root / BACKUP_NAME
    manifest_path = root / MANIFEST_NAME
    if _exists(frozen):
        raise PrivacyError("projection_already_frozen")
    if _exists(backup_path) or _exists(manifest_path):
        raise PrivacyError("privacy_transform_already_applied")

    plan_bytes = _read(Path(plan_path).absolute(), root)
    plan = _parse(plan_bytes, "plan_invalid")
    if not isinstance(plan, dict) or set(plan) != {"redactions"} or not isinstance(plan["redactions"], list):
        raise PrivacyError("plan_invalid")
    redactions = plan["redactions"]
    seen = set()
    for item in redactions:
        if not isinstance(item, dict) or set(item) != {"packet", "event_order", "field"}:
            raise PrivacyError("plan_invalid")
        packet, order, field = item["packet"], item["event_order"], item["field"]
        if (not isinstance(packet, str) or not PACKET_RE.fullmatch(packet)
                or not isinstance(order, int) or isinstance(order, bool) or order < 0
                or not isinstance(field, str) or field not in FIELDS):
            raise PrivacyError("plan_invalid")
        key = (packet, order, field)
        if key in seen:
            raise PrivacyError("plan_duplicate_entry")
        seen.add(key)
    if not redactions:
        raise PrivacyError("plan_empty")

    bundle_bytes = _read(bundle_path, root)
    bundle_rows = _jsonl(bundle_bytes)
    bundle_by_id = {}
    for row in bundle_rows:
        event_id = row.get("id")
        if not isinstance(event_id, str) or event_id in bundle_by_id:
            raise PrivacyError("bundle_id_invalid")
        bundle_by_id[event_id] = row

    packet_dir = root / "annotation-input"
    try:
        info = packet_dir.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise PrivacyError("packet_directory_invalid")
        packet_dir.resolve().relative_to(root.resolve())
    except (OSError, ValueError):
        raise PrivacyError("packet_directory_invalid") from None

    packet_rows: dict[str, list[dict[str, Any]]] = {}
    for name in sorted({item["packet"] for item in redactions}):
        data = _read(packet_dir / name, root)
        rows = _jsonl(data)
        ids = [row.get("id") for row in rows]
        if any(not isinstance(event_id, str) for event_id in ids) or len(set(ids)) != len(ids):
            raise PrivacyError("packet_id_invalid")
        for row in rows:
            if bundle_by_id.get(row["id"]) != row:
                raise PrivacyError("packet_bundle_mismatch")
        packet_rows[name] = rows

    for item in redactions:
        rows = packet_rows[item["packet"]]
        matches = [
            row for row in rows
            if isinstance(row.get("event_order"), int)
            and not isinstance(row.get("event_order"), bool)
            and row["event_order"] == item["event_order"]
        ]
        if len(matches) != 1:
            raise PrivacyError("redaction_event_missing")
        projection = matches[0].get("projection")
        field = item["field"]
        if not isinstance(projection, dict) or not isinstance(projection.get(field), str):
            raise PrivacyError("redaction_field_invalid")

    counts = {field: 0 for field in sorted(FIELDS)}
    for item in redactions:
        rows = packet_rows[item["packet"]]
        row = next(row for row in rows if row["event_order"] == item["event_order"])
        bundle_row = bundle_by_id[row["id"]]
        row["projection"][item["field"]] = PLACEHOLDER
        bundle_row["projection"][item["field"]] = PLACEHOLDER
        counts[item["field"]] += 1

    updated_bundle = _encode(bundle_rows)
    manifest = {
        "schema": 1,
        "plan_sha256": hashlib.sha256(plan_bytes).hexdigest(),
        "bundle_before_sha256": hashlib.sha256(bundle_bytes).hexdigest(),
        "bundle_after_sha256": hashlib.sha256(updated_bundle).hexdigest(),
        "redaction_count": len(redactions),
        "packet_count": len(packet_rows),
        "field_counts": counts,
    }

    _write(backup_path, bundle_bytes, root)
    for name, rows in packet_rows.items():
        _write(packet_dir / name, _encode(rows), root)
    _write(bundle_path, updated_bundle, root)
    _write(manifest_path, (json.dumps(manifest, sort_keys=True) + "\n").encode(), root)
    return {"redactions": len(redactions), "packets": len(packet_rows), "bundle_rows": len(bundle_rows)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        print(json.dumps(apply_plan(args.plan), sort_keys=True))
        return 0
    except PrivacyError as exc:
        print(f"error: {exc.code}", file=sys.stderr)
        return 2
    except Exception:
        print("error: privacy_transform_failed", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
