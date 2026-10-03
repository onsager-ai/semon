#!/usr/bin/env python3
"""Isolated native Copilot lifecycle probe; never reads a personal home."""

import argparse, importlib.util, pathlib, subprocess, json, http.server, threading, hashlib, collections


def main():
    spec = importlib.util.spec_from_file_location(
        "probe", pathlib.Path(__file__).with_name("copilot-offline.py")
    )
    probe = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(probe)
    parser = argparse.ArgumentParser(
        description="Probe pinned Copilot resume, cumulative mock usage and persisted shell denial in a new private root."
    )
    parser.add_argument("--copilot", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument(
        "--expected-version", choices=["1.0.90", "1.0.91"], default="1.0.91"
    )
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(mode=0o700, exist_ok=False)
    env = probe.probe_environment(root)
    requests = []
    mode = "initial"

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(
                json.dumps(
                    {"object": "list", "data": [{"id": "gpt-4", "object": "model"}]}
                ).encode()
            )

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["content-length"])))
            requests.append(request)
            if mode == "denial" and not any(
                m.get("role") == "tool" for m in request.get("messages", [])
            ):
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "fixture-denied-call",
                            "type": "function",
                            "function": {
                                "name": "bash",
                                "arguments": json.dumps(
                                    {
                                        "command": "printf fixture-denial; : > SHOULD-NOT-EXIST",
                                        "description": "disposable denial probe",
                                    }
                                ),
                            },
                        }
                    ],
                }
                finish = "tool_calls"
            else:
                delta = {
                    "role": "assistant",
                    "content": "SYNTHETIC_COPILOT_LIFECYCLE_ACK",
                }
                finish = "stop"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            chunks = [
                {"choices": [{"delta": delta, "index": 0, "finish_reason": None}]},
                {"choices": [{"delta": {}, "index": 0, "finish_reason": finish}]},
                {
                    "choices": [],
                    "usage": {
                        "prompt_tokens": 11,
                        "completion_tokens": 3,
                        "total_tokens": 14,
                        "prompt_tokens_details": {"cached_tokens": 2},
                    },
                },
            ]
            for chunk in chunks:
                data = {
                    "id": f"fixture-msg-{len(requests)}",
                    "object": "chat.completion.chunk",
                    "created": 1,
                    "model": "gpt-4",
                    **chunk,
                }
                self.wfile.write(("data: " + json.dumps(data) + "\n\n").encode())
            self.wfile.write(b"data: [DONE]\n\n")

    exe = str(args.copilot.resolve(strict=True))
    version = subprocess.run(
        [exe, "--no-auto-update", "--version"],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    if version.splitlines()[0] != f"GitHub Copilot CLI {args.expected_version}.":
        raise RuntimeError(f"Expected pinned Copilot {args.expected_version}")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    env.update(
        COPILOT_PROVIDER_BASE_URL=f"http://127.0.0.1:{server.server_port}/v1",
        COPILOT_MODEL="gpt-4",
    )
    common = [
        exe,
        "--no-auto-update",
        "--disable-builtin-mcps",
        "--no-custom-instructions",
        "--no-remote",
        "--no-remote-export",
        "--allow-all-tools",
        "--output-format",
        "json",
    ]

    def run(label, extra):
        p = subprocess.run(
            common + extra + ["-p", "SEMON_SYNTHETIC_" + label],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        (root / (label + "-stdout.jsonl")).write_text(p.stdout)
        (root / (label + "-stderr.txt")).write_text(p.stderr)
        p.check_returncode()

    def snapshot():
        return {
            p.parent.name: p.read_bytes()
            for p in (root / "home/session-state").glob("*/events.jsonl")
        }

    try:
        run("initial", [])
        initial = snapshot()
        assert len(initial) == 1
        sid = next(iter(initial))
        (root / "initial.events.jsonl").write_bytes(initial[sid])
        mode = "resume"
        run("resume", ["--resume=" + sid])
        resumed = snapshot()
        assert len(resumed) == 1
        assert resumed[sid].startswith(initial[sid])
        mode = "denial"
        run("denial", ["--deny-tool=shell"])
        denied = snapshot()
        assert len(denied) == 2
        assert not (root / "SHOULD-NOT-EXIST").exists()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    resumed_rows = [json.loads(line) for line in resumed[sid].splitlines()]
    assert sum(row["type"] == "session.resume" for row in resumed_rows) == 1
    snapshots = [
        row["data"]["modelMetrics"]["gpt-4"]["usage"]
        for row in resumed_rows
        if row["type"] == "session.shutdown"
    ]
    assert [
        (row["inputTokens"], row["outputTokens"], row["cacheReadTokens"])
        for row in snapshots
    ] == [(11, 3, 2), (22, 6, 4)]
    denial_id = next(identity for identity in denied if identity != sid)
    denial_rows = [json.loads(line) for line in denied[denial_id].splitlines()]
    starts = [
        row["data"] for row in denial_rows if row["type"] == "tool.execution_start"
    ]
    completions = [
        row["data"] for row in denial_rows if row["type"] == "tool.execution_complete"
    ]
    assert len(starts) == len(completions) == 1
    assert (
        starts[0]["toolCallId"] == completions[0]["toolCallId"] == "fixture-denied-call"
    )
    assert (
        completions[0]["success"] is False
        and completions[0]["error"]["code"] == "denied"
    )
    requested = [
        call
        for row in denial_rows
        if row["type"] == "assistant.message"
        for call in row["data"].get("toolRequests", [])
    ]
    assert len(requested) == 1 and requested[0]["toolCallId"] == starts[0]["toolCallId"]
    assert requested[0]["arguments"] == starts[0]["arguments"]
    report = {
        "initial_sha256": hashlib.sha256(initial[sid]).hexdigest(),
        "version": args.expected_version,
        "binary_sha256": hashlib.sha256(pathlib.Path(exe).read_bytes()).hexdigest(),
        "confidence": "native headless persistence, localhost mock provider; synthetic usage, no native billing claim",
        "resume_same_session": True,
        "resume_prefix_byte_identical": True,
        "denied_marker_absent": True,
        "request_count": len(requests),
        "sessions": [],
    }
    for sid, data in denied.items():
        rows = [json.loads(line) for line in data.splitlines()]
        report["sessions"].append(
            {
                "session": sid,
                "sha256": hashlib.sha256(data).hexdigest(),
                "event_types": dict(collections.Counter(r["type"] for r in rows)),
                "shutdown_metrics": [
                    r["data"].get("modelMetrics")
                    for r in rows
                    if r["type"] == "session.shutdown"
                ],
                "tool_events": [r for r in rows if r["type"].startswith("tool.")],
                "approval_events": [r["type"] for r in rows if "approval" in r["type"]],
            }
        )
    (root / "requests.json").write_text(json.dumps(requests))
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
