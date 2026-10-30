#!/usr/bin/env python3
"""Reproject the selected private snapshots with the final pre-label adapter."""
import json

import real_pipeline as pipeline
from real_select import ROOT, read_private


def main():
    pipeline.thermal_checkpoint()
    selection = json.loads(read_private(ROOT / "selection.json"))
    metadata = json.loads(read_private(ROOT / "task-metadata.json"))
    target = ROOT / "final-capture"
    if target.exists():
        raise ValueError("final capture already exists; preserve it and use a new protocol")
    target.mkdir(mode=0o700)
    packet_dir = ROOT / "annotation-input"
    packet_dir.mkdir(mode=0o700, exist_ok=False)
    bundle = []
    final = []
    for index, item in enumerate(selection["selected"], 1):
        manifest = json.loads(read_private(item["manifest"]))
        prepared = pipeline.prepare_one(manifest["source_path"], harness=item["harness"],
                                        closed_confirmed=True, private_root=target)
        new_manifest = json.loads(read_private(prepared["manifest"]))
        if new_manifest["source_sha256"] != manifest["source_sha256"]:
            raise ValueError("selected source changed before final capture")
        rows = [json.loads(line) for line in read_private(prepared["projection_copy"]).splitlines() if line]
        cleaned = [{key: row[key] for key in (
            "id", "session_id", "event_order", "harness", "native_format", "arrival_ms", "projection",
        )} for row in rows]
        bundle.extend(cleaned)
        packet = packet_dir / f"session-{index:02d}.jsonl"
        packet.write_text("".join(json.dumps(row) + "\n" for row in cleaned))
        packet.chmod(0o600)
        final.append({**prepared, **metadata[item["session_id"]], "packet": str(packet)})
    path = ROOT / "review-bundle.jsonl"
    path.write_text("".join(json.dumps(row) + "\n" for row in bundle))
    path.chmod(0o600)
    path = ROOT / "final-selection.json"
    path.write_text(json.dumps(final, indent=2) + "\n")
    path.chmod(0o600)
    pipeline.thermal_checkpoint()
    print(json.dumps({"events": len(bundle), "sessions": len(final),
                      "distinct_tasks": len({row["task_key"] for row in final})}))


if __name__ == "__main__":
    main()
