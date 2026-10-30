#!/usr/bin/env python3
"""Build private metadata inventories and prepare bounded local candidates."""
import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

import real_pipeline as pipeline
from real_select import ROOT as PRIVATE_ROOT


MAX_BYTES = 10_000_000
MIN_AGE_NS = 24 * 60 * 60 * 1_000_000_000
ROOT = PRIVATE_ROOT.resolve()
TASK_ROLE_RE = re.compile(
    r'\b(?:task_role|execution_role|run_role|role)\s*["\']?\s*[:=]\s*["\']?'
    r'(manager|worker|preflight|acceptance)\b|'
    r'\byou\s+are\s+(?:an?\s+)?(manager|worker|preflight|acceptance)\b', re.I,
)
TASK_ROLES = {"manager", "worker", "preflight", "acceptance"}


def inventory_sources(roots=None, *, private_root=ROOT):
    pipeline.thermal_checkpoint()
    cutoff = time.time_ns() - MIN_AGE_NS
    rows = []
    if roots is None:
        roots = (("claude-code", Path.home() / ".claude/projects"),
                 ("codex", Path.home() / ".codex/sessions"))
    for harness, source_root in roots:
        if source_root.is_symlink() or not source_root.is_dir():
            continue
        for base, dirs, files in os.walk(source_root, followlinks=False):
            dirs[:] = [name for name in dirs if name.lower() != "subagents"
                       and not (Path(base) / name).is_symlink()]
            for name in files:
                if not name.lower().endswith(".jsonl"):
                    continue
                path = Path(base) / name
                try:
                    info = path.lstat()
                except OSError:
                    continue
                if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                        or info.st_size >= MAX_BYTES or info.st_mtime_ns >= cutoff):
                    continue
                rows.append({"harness": harness, "path": str(path),
                             "size": info.st_size, "mtime_ns": info.st_mtime_ns})
    rows.sort(key=lambda row: (row["harness"], row["size"], row["path"]))
    private_root = pipeline._ensure_private_root(private_root)
    pipeline._atomic_private_write(
        private_root / "inventory.json",
        (json.dumps(rows, ensure_ascii=False, indent=2) + "\n").encode("utf-8"),
    )
    pipeline.thermal_checkpoint()
    return {"files": len(rows), "by_harness": dict(sorted(Counter(r["harness"] for r in rows).items()))}


def _private_json(path, *, parent=ROOT):
    path = Path(path)
    parent = Path(parent).resolve()
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_size >= MAX_BYTES
            or not path.resolve().is_relative_to(parent)):
        raise ValueError("private_input_invalid")
    return json.loads(path.read_text(encoding="utf-8"))


def _user_texts(record, harness):
    if harness == "claude-code":
        message = record.get("message")
        if record.get("type") != "user" or not isinstance(message, dict) or message.get("role") != "user":
            return ()
        content = message.get("content")
    else:
        payload = record.get("payload")
        if not isinstance(payload, dict) or payload.get("role") != "user":
            return ()
        content = payload.get("content")
    if isinstance(content, str):
        return (content,)
    if not isinstance(content, list):
        return ()
    return tuple(block["text"] for block in content
                 if isinstance(block, dict) and block.get("type") in {"text", "input_text"}
                 and isinstance(block.get("text"), str))


def _task_facts(text):
    if not text.lstrip().startswith("{"):
        return None, None
    try:
        envelope = json.loads(text)
    except json.JSONDecodeError:
        return None, None
    if not isinstance(envelope, dict) or not {"original_contract", "user_messages"} <= envelope.keys():
        return None, None
    contract = envelope.get("original_contract")
    if isinstance(contract, str) and contract.lstrip().startswith("{"):
        try:
            contract = json.loads(contract)
        except json.JSONDecodeError:
            contract = None
    task_key = None
    if isinstance(contract, dict):
        cwd, objective = contract.get("cwd"), contract.get("objective")
        if isinstance(cwd, str) and cwd and isinstance(objective, str) and objective:
            canonical = json.dumps([cwd, objective], ensure_ascii=False,
                                   sort_keys=True, separators=(",", ":"))
            task_key = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    serialized = json.dumps(envelope, ensure_ascii=False)
    match = TASK_ROLE_RE.search(serialized)
    role = next((part.lower() for part in match.groups() if part), None) if match else None
    return task_key, role if role in TASK_ROLES else None


def task_metadata():
    pipeline.thermal_checkpoint()
    candidates = _private_json(ROOT / "prepared-candidates.json")
    metadata = {}
    for candidate in candidates.get("candidates", []):
        raw_path = Path(candidate["raw_copy"])
        raw_root = (ROOT / "raw").resolve()
        info = raw_path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_size >= MAX_BYTES
                or not raw_path.resolve().is_relative_to(raw_root)):
            raise ValueError("raw_copy_invalid")
        pipeline.thermal_checkpoint()
        keys, roles = set(), set()
        with raw_path.open(encoding="utf-8") as source:
            for line in source:
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(record, dict):
                    continue
                for text in _user_texts(record, candidate["harness"]):
                    key, role = _task_facts(text)
                    if key:
                        keys.add(key)
                    if role:
                        roles.add(role)
        session_id = candidate["session_id"]
        task_key = next(iter(keys)) if len(keys) == 1 else None
        role = next(iter(roles)) if len(roles) == 1 else None
        metadata[session_id] = {
            "task_key": task_key,
            "role": role if task_key else "other",
        }
        pipeline.thermal_checkpoint()
    private_root = pipeline._ensure_private_root(ROOT)
    pipeline._atomic_private_write(
        private_root / "task-metadata.json",
        (json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8"),
    )
    roles = Counter(row["role"] for row in metadata.values())
    return {"sessions": len(metadata), "distinct_tasks": len({row["task_key"] for row in metadata.values() if row["task_key"]}),
            "roles": dict(sorted(roles.items()))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--inventory", action="store_true")
    mode.add_argument("--task-metadata", action="store_true")
    parser.add_argument("--harness", choices=("claude-code", "codex"))
    parser.add_argument("--start", type=int)
    parser.add_argument("--end", type=int)
    args = parser.parse_args()
    try:
        if args.inventory:
            result = inventory_sources()
        elif args.task_metadata:
            result = task_metadata()
        else:
            if args.harness is None or args.start is None or args.end is None:
                parser.error("preparation requires --harness, --start, and --end")
            pipeline.thermal_checkpoint()
            inventory = _private_json(ROOT / "inventory.json")
            state_path = ROOT / "prepared-candidates.json"
            state = _private_json(state_path) if state_path.exists() else {"candidates": [], "exclusions": {}}
            excluded = Counter(state["exclusions"])
            candidates = [row for row in inventory if row["harness"] == args.harness][args.start:args.end]
            for row in candidates:
                pipeline.thermal_checkpoint()
                if row["size"] >= MAX_BYTES or Path(row["path"]).stat().st_size >= MAX_BYTES:
                    excluded["source_size_limit"] += 1
                    continue
                if subprocess.run(["fuser", "-s", row["path"]], stdout=subprocess.DEVNULL,
                                  stderr=subprocess.DEVNULL).returncode != 1:
                    excluded["open_or_unconfirmed"] += 1
                    continue
                try:
                    prepared = pipeline.prepare_one(row["path"], harness=args.harness, closed_confirmed=True)
                except pipeline.PipelineError as error:
                    if error.code.startswith("thermal"):
                        raise
                    excluded[error.code] += 1
                    continue
                state["candidates"].append({"harness": args.harness, **prepared})
                state["exclusions"] = dict(excluded)
                pipeline._atomic_private_write(state_path, (json.dumps(state, indent=2) + "\n").encode())
            state["exclusions"] = dict(excluded)
            pipeline._atomic_private_write(state_path, (json.dumps(state, indent=2) + "\n").encode())
            pipeline.thermal_checkpoint()
            result = {"prepared": len(state["candidates"]), "exclusions": dict(excluded)}
        print(json.dumps(result, sort_keys=True))
    except pipeline.PipelineError as error:
        print(json.dumps({"error_code": error.code}), file=sys.stderr)
        return 2
    except (OSError, ValueError, KeyError, json.JSONDecodeError):
        print(json.dumps({"error_code": "private_input_invalid"}), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
