#!/usr/bin/env python3
"""Select whole private session projections before blind annotation or scoring."""
import json
import re
from collections import Counter
from pathlib import Path

import real_pipeline as pipeline

# Conservative second pass: omit entire sessions with remaining sensitive-topic
# text, even when it might be a harmless discussion rather than a credential.
SENSITIVE = re.compile(
    r"\b(?:password|passwd|credential|authorization|bearer|api[ _-]?key|"
    r"private[ _-]?key|access[ _-]?token|refresh[ _-]?token)\b", re.I,
)
ERROR_CUE = re.compile(r"\b(?:error|failed|denied|timeout|traceback)\b", re.I)
WORKER_ROOT = re.compile(r'"(?:original_contract|management_state|user_messages)"|^\s*(?:you are|role:)\s+(?:an?\s+)?(?:worker|reviewer)\b', re.I)
ROOT = Path(__file__).with_name("private")


def read_private(path):
    pipeline.thermal_checkpoint()
    path = Path(path).resolve()
    if not path.is_relative_to(ROOT.resolve()) or path.stat().st_size >= 10_000_000:
        raise ValueError("private input is outside the bounded local directory")
    value = path.read_text()
    pipeline.thermal_checkpoint()
    return value


def main():
    pipeline.thermal_checkpoint()
    candidates = json.loads(read_private(ROOT / "prepared-candidates.json"))["candidates"]
    task_metadata = json.loads(read_private(ROOT / "task-metadata.json"))
    eligible = []
    excluded = Counter()
    for candidate in candidates:
        rows = [json.loads(line) for line in read_private(candidate["projection_copy"]).splitlines() if line]
        metadata = task_metadata.get(candidate["session_id"])
        if not metadata or metadata["role"] not in {"manager", "worker"}:
            excluded["not_manager_or_worker"] += 1
            continue
        text = "\n".join(json.dumps(row["projection"]) for row in rows)
        if SENSITIVE.search(text):
            excluded["remaining_sensitive_context"] += 1
            continue
        if not 7 <= len(rows) <= 60:
            excluded["short_capture"] += 1
            continue
        cue_count = sum(bool(ERROR_CUE.search(json.dumps(row["projection"]))) for row in rows)
        eligible.append((candidate, rows, cue_count))
    chosen = []
    task_keys = set()
    for harness, count in (("codex", 8), ("claude-code", 12)):
        pool = [item for item in eligible if item[0]["harness"] == harness]
        pool.sort(key=lambda item: (-item[2], -len(item[1]), item[0]["session_id"]))
        selected = []
        for item in pool:
            key = task_metadata[item[0]["session_id"]]["task_key"]
            if key in task_keys:
                continue
            selected.append(item)
            task_keys.add(key)
            if len(selected) == count:
                break
        if len(selected) < count:
            raise ValueError("insufficient eligible whole sessions; prepare more candidates")
        chosen.extend(selected)
    bundle = []
    session_files = ROOT / "review-sessions"
    session_files.mkdir(mode=0o700, exist_ok=True)
    for candidate, rows, _ in chosen:
        cleaned = [{key: row[key] for key in (
            "id", "session_id", "event_order", "harness", "native_format", "arrival_ms", "projection",
        )} for row in rows]
        bundle.extend(cleaned)
        path = session_files / (candidate["session_id"] + ".jsonl")
        path.write_text("".join(json.dumps(row) + "\n" for row in cleaned))
        path.chmod(0o600)
    if not 300 <= len(bundle) <= 600:
        raise ValueError("whole-session selection outside predeclared target range")
    path = ROOT / "review-bundle.jsonl"
    path.write_text("".join(json.dumps(row) + "\n" for row in bundle))
    path.chmod(0o600)
    selection = {
        "selected": [item[0] for item in chosen], "exclusions": dict(excluded),
        "eligible_events": len(bundle), "complete_captured_sessions": len(chosen),
        "selection": "whole standalone Manager/Worker sessions; 12 Claude/8 Codex; distinct original tasks; error cues, then event count, then opaque ID",
    }
    path = ROOT / "selection.json"
    path.write_text(json.dumps(selection, indent=2) + "\n")
    path.chmod(0o600)
    pipeline.thermal_checkpoint()
    print(json.dumps({"events": len(bundle), "sessions": len(chosen), "exclusions": dict(excluded)}))


if __name__ == "__main__":
    main()
