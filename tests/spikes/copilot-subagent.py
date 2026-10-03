#!/usr/bin/env python3
"""Native mock task probe; child usage is already included in session totals."""

import argparse, importlib.util, pathlib, subprocess, json, http.server, threading, hashlib, collections


def main():
    spec = importlib.util.spec_from_file_location(
        "probe", pathlib.Path(__file__).with_name("copilot-offline.py")
    )
    probe = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(probe)
    parser = argparse.ArgumentParser(
        description="Pinned isolated Copilot task/subagent persistence against a localhost mock."
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
            if len(requests) == 1:
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "fixture-task-call",
                            "type": "function",
                            "function": {
                                "name": "task",
                                "arguments": json.dumps(
                                    {
                                        "name": "fixture-child",
                                        "prompt": "Respond with SYNTHETIC_CHILD_ACK only. Do not use tools.",
                                        "agent_type": "explore",
                                        "description": "Disposable synthetic subagent probe",
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

    version = subprocess.run(
        [str(args.copilot.resolve(strict=True)), "--no-auto-update", "--version"],
        env=env,
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    if not version.startswith(f"GitHub Copilot CLI {args.expected_version}.\n"):
        raise RuntimeError("Unexpected Copilot version")
    exe = str(args.copilot.resolve(strict=True))
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
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    env.update(
        COPILOT_PROVIDER_BASE_URL=f"http://127.0.0.1:{server.server_port}/v1",
        COPILOT_MODEL="gpt-4",
    )

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
        run("subagent", [])
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    report = {
        "version": args.expected_version,
        "binary_sha256": hashlib.sha256(pathlib.Path(exe).read_bytes()).hexdigest(),
        "requests": len(requests),
        "sessions": [],
    }
    saved = snapshot()
    if len(saved) != 1:
        raise RuntimeError("Expected one physical persisted session")
    for sid, data in saved.items():
        rows = [json.loads(line) for line in data.splitlines()]
        starts = [row["data"] for row in rows if row["type"] == "subagent.started"]
        completed = [row["data"] for row in rows if row["type"] == "subagent.completed"]
        child = [
            row
            for row in rows
            if row["data"].get("parentToolCallId") == "fixture-task-call"
        ]
        if len(starts) != 1 or len(completed) != 1:
            raise RuntimeError("Missing unique subagent lifecycle")
        if (
            starts[0]["toolCallId"] != "fixture-task-call"
            or completed[0]["toolCallId"] != "fixture-task-call"
        ):
            raise RuntimeError("Subagent call IDs do not join")
        if not any(row["type"] == "assistant.message" for row in child):
            raise RuntimeError("Missing exact child assistant edge")
        if completed[0]["totalTokens"] != 14:
            raise RuntimeError("Unexpected child mock usage")
        shutdown = [
            row["data"]["modelMetrics"]["gpt-4"]
            for row in rows
            if row["type"] == "session.shutdown"
        ]
        if len(shutdown) != 1 or shutdown[0]["requests"]["count"] != 3:
            raise RuntimeError("Unexpected cumulative request count")
        if shutdown[0]["usage"]["inputTokens"] != 33:
            raise RuntimeError("Unexpected cumulative usage")
        report["source_sha256"] = hashlib.sha256(data).hexdigest()
        (root / "subagent.events.jsonl").write_bytes(data)
        report["sessions"].append(
            {
                "session": sid,
                "types": dict(collections.Counter(row["type"] for row in rows)),
                "structural_fields": [
                    {
                        "type": row["type"],
                        "keys": sorted(row["data"]),
                        "ids": {
                            k: v
                            for k, v in row["data"].items()
                            if k
                            in [
                                "toolCallId",
                                "parentToolCallId",
                                "parentAgentTaskId",
                                "agentId",
                                "agentTaskId",
                                "sessionId",
                                "parentSessionId",
                            ]
                        },
                    }
                    for row in rows
                ],
            }
        )
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
