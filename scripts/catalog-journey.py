#!/usr/bin/env python3
"""Measure complete embedded-Viewer journeys against increasing native history.

Requires Linux /proc, Node, and the repository Playwright installation. The
binary must contain the tested UI bundle; this runner never substitutes one.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import time


def process_sample(pid):
    """Whole-process counters, including background producer work."""
    values = {}
    try:
        for name in ("status", "io"):
            for line in Path(f"/proc/{pid}/{name}").read_text().splitlines():
                key, _, value = line.partition(":")
                if key in ("VmRSS", "VmHWM", "rchar", "wchar", "read_bytes", "write_bytes"):
                    values[key] = int(value.split()[0])
        stat = Path(f"/proc/{pid}/stat").read_text().split()
        values["cpu_ticks"] = int(stat[13]) + int(stat[14])
        return values
    except FileNotFoundError:
        return values


def timestamp(milliseconds):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(milliseconds / 1000))


def seed(home, public_root, additional, scalar):
    fixture = subprocess.run(
        ["node", str(public_root / "tests/ui/fixture.mjs"), str(home), "--extras"],
        capture_output=True, text=True, check=True,
    )
    now = int(fixture.stdout.strip())
    backlog = next((home / "claude").rglob("backlog.jsonl"))
    with backlog.open("a") as stream:
        stream.write(json.dumps({
            "type": "assistant", "sessionId": "backlog", "uuid": "semon-served-scalar",
            "timestamp": timestamp(now - 1000),
            "message": {
                "role": "assistant", "content": [{"type": "text", "text": scalar}],
                "model": "claude-sonnet-5", "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        }) + "\n")
    unrelated = home / "claude/projects/unrelated"
    unrelated.mkdir(exist_ok=True)
    # Fixed padding fills the requested 60-row page at every workload. Added
    # sessions sort later, so the displayed first page and backlog stay fixed.
    for index in range(64 + additional):
        sid = f"journey-unrelated-{index:06}"
        with (unrelated / f"{sid}.jsonl").open("w") as stream:
            for turn in range(8):
                stream.write(json.dumps({
                    "type": "user", "sessionId": sid, "uuid": f"{sid}-{turn}",
                    "parentUuid": None, "cwd": "/synthetic/unrelated",
                    "timestamp": timestamp(now - 172800000 + turn * 1000),
                    "message": {"role": "user", "content": "Unrelated native history. " * 128},
                }) + "\n")
    return now


def start_server(binary, home, now):
    env = {**os.environ, "SEMON_TEST_NOW": str(now)}
    command = [binary, "sessions", "--serve", "--listen", "127.0.0.1:0"]
    for option, path in (
        ("claude-home", home / "claude"), ("claude-json", home / ".claude.json"),
        ("codex-home", home / "codex"), ("copilot-home", home / "copilot"),
        ("proc-root", home / "proc"), ("cache", home / "index.json"),
    ):
        command.extend([f"--{option}", str(path)])
    server = subprocess.Popen(
        command, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    auth = {}

    def capture_url():
        for line in server.stdout:
            match = re.search(r"(http://127.0.0.1:\d+)/\?t=([a-f0-9]+)", line)
            if match:
                auth.update(base=match[1], token=match[2])

    threading.Thread(target=capture_url, daemon=True).start()
    deadline = time.monotonic() + 10
    while not auth:
        if server.poll() is not None or time.monotonic() > deadline:
            server.terminate()
            server.wait(timeout=10)
            raise RuntimeError("Server did not announce its listener")
        time.sleep(0.02)
    return server, auth


def journey(args, root, home, now, additional, phase, scalar, binary_sha):
    server, auth = start_server(args.binary, home, now)
    evidence = root / f"{additional}-{phase}.json"
    manifest = root / f"{additional}-{phase}-auth.json"
    browser = None
    try:
        # Create with private permissions before writing the ephemeral token.
        with os.fdopen(os.open(manifest, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as stream:
            json.dump({
                **auth, "source_key": "local", "sid": "backlog", "scalar": scalar,
                "phase": phase, "harness": "claude", "source_revision": args.source_revision,
                "binary_sha256": binary_sha, "evidence_path": str(evidence),
            }, stream)
        before = process_sample(server.pid)
        peak = before.get("VmRSS", 0)
        started = time.monotonic()
        with (root / f"{additional}-{phase}-browser.log").open("w") as log:
            browser = subprocess.Popen(
                ["node", args.driver, str(manifest), str(evidence)],
                stdout=log, stderr=subprocess.STDOUT,
            )
            while browser.poll() is None:
                peak = max(peak, process_sample(server.pid).get("VmRSS", 0))
                if time.monotonic() - started > 180:
                    raise RuntimeError("Browser journey exceeded its bounded test timeout")
                time.sleep(0.05)
        after = process_sample(server.pid)
        result = {
            "unrelated_sessions": additional, "fixed_padding_sessions": 64,
            "phase": phase, "browser_exit": browser.returncode,
            "source_revision": args.source_revision, "binary_sha256": binary_sha,
            "wall_ms": (time.monotonic() - started) * 1000,
            "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "server_before": before, "server_after": after, "peak_rss_kib": peak,
            "claude_native_files": len(list((home / "claude").rglob("*.jsonl"))),
            "native_bytes": sum(path.stat().st_size for path in (home / "claude").rglob("*.jsonl")),
            "evidence": str(evidence),
        }
        return result
    finally:
        if browser and browser.poll() is None:
            browser.kill()
            browser.wait()
        server.terminate()
        server.wait(timeout=10)
        manifest.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--public-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--driver")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--counts", default="0,256,2048")
    args = parser.parse_args()
    args.driver = args.driver or str(args.public_root / "ui/tests/catalog-journey.mjs")
    root = args.output or Path(tempfile.mkdtemp(prefix="semon-catalog-journeys-"))
    root.mkdir(parents=True, exist_ok=True)
    binary_sha = hashlib.sha256(Path(args.binary).read_bytes()).hexdigest()
    scalar = "Actual native scalar field.\n\n" + "Recorded source text. " * 5000
    results = []
    for additional in map(int, args.counts.split(",")):
        if additional < 0:
            raise ValueError("History counts must be nonnegative")
        home = root / f"history-{additional}"
        if home.exists():
            raise RuntimeError("Use a fresh output directory; first import needs an empty projection")
        home.mkdir()
        now = seed(home, args.public_root, additional, scalar)
        for phase in ("first-import", "persisted-restart"):
            result = journey(args, root, home, now, additional, phase, scalar, binary_sha)
            results.append(result)
            (root / "server-results.json").write_text(json.dumps(results, indent=2))
            if result["browser_exit"]:
                raise RuntimeError("Browser journey failed; inspect the local browser log")
    print(json.dumps({"output": str(root), "results": results}, indent=2))


if __name__ == "__main__":
    main()
