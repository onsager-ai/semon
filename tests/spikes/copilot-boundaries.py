#!/usr/bin/env python3
"""Pinned private-root interactive/compaction/cancellation persistence probe."""

import fcntl, termios, struct, pty, select, time, signal, os, argparse, importlib.util, pathlib, subprocess, json, http.server, threading, hashlib, collections


def main():
    spec = importlib.util.spec_from_file_location(
        "probe", pathlib.Path(__file__).with_name("copilot-offline.py")
    )
    probe = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(probe)
    parser = argparse.ArgumentParser(
        description="Pinned isolated Copilot interactive, compaction and cancellation persistence against a localhost mock."
    )
    parser.add_argument("--copilot", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument(
        "--expected-version", choices=["1.0.90", "1.0.91"], default="1.0.91"
    )
    parser.add_argument(
        "--mode", choices=["interactive", "compaction", "cancel"], required=True
    )
    args = parser.parse_args()
    probe_sha256 = hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
    root = args.output.resolve()
    root.mkdir(mode=0o700, exist_ok=False)
    env = probe.probe_environment(root)
    requests = []

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
            if args.mode == "cancel" and not any(
                message.get("role") == "tool" for message in request.get("messages", [])
            ):
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "fixture-cancel-call",
                            "type": "function",
                            "function": {
                                "name": "bash",
                                "arguments": json.dumps(
                                    {
                                        "command": ": > CANCEL-READY; sleep 20; printf SYNTHETIC_FINISHED",
                                        "description": "Disposable cancellation probe",
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

    def snapshot():
        return {
            p.parent.name: p.read_bytes()
            for p in (root / "home/session-state").glob("*/events.jsonl")
        }

    def complete_rows(data):
        # A live writer can leave its final frame incomplete during polling.
        framed = data if data.endswith(b"\n") else data[: data.rfind(b"\n") + 1]
        return [json.loads(line) for line in framed.splitlines()]

    if args.mode == "cancel":
        process = subprocess.Popen(
            common + ["-p", "SEMON_SYNTHETIC_CANCEL_235"],
            cwd=root,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                rows = [
                    row for data in snapshot().values() for row in complete_rows(data)
                ]
                if (
                    any(row["type"] == "tool.execution_start" for row in rows)
                    and (root / "CANCEL-READY").is_file()
                ):
                    break
                if process.poll() is not None:
                    raise RuntimeError("Exited before persisted tool start")
                time.sleep(0.1)
            else:
                raise RuntimeError("No persisted tool start before cancellation")
            os.killpg(process.pid, signal.SIGINT)
            stdout, stderr = process.communicate(timeout=10)
            (root / "stdout.jsonl").write_bytes(stdout)
            (root / "stderr.txt").write_bytes(stderr)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            server.shutdown()
            server.server_close()
            thread.join()
    else:
        common = common[:-2]
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
        env["TERM"] = "xterm-256color"

        def attach_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        process = subprocess.Popen(
            common + ["-i", "SEMON_SYNTHETIC_INTERACTIVE_235"],
            cwd=root,
            env=env,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            preexec_fn=attach_terminal,
        )
        os.close(slave)
        terminal = bytearray()
        completed = False
        trusted = False
        compacting = False
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    try:
                        chunk = os.read(master, 65536)
                        terminal.extend(chunk)
                        replies = {
                            b"\x1b[>q": b"\x1bP>|XTerm(390)\x1b\\",
                            b"\x1b[6n": b"\x1b[1;1R",
                            b"\x1b[c": b"\x1b[?1;2c",
                            b"\x1b[>c": b"\x1b[>0;390;0c",
                            b"\x1b[?u": b"\x1b[?0u",
                            b"\x1b[?2026$p": b"\x1b[?2026;2$y",
                            b"\x1b[?12$p": b"\x1b[?12;2$y",
                        }
                        for query, reply in replies.items():
                            if query in chunk:
                                os.write(master, reply)
                    except OSError:
                        break
                if not trusted and b"Confirm folder trust" in terminal:
                    os.write(master, b"\r")
                    trusted = True
                if any(
                    b"SYNTHETIC_COPILOT_LIFECYCLE_ACK" in data
                    for data in snapshot().values()
                ):
                    if args.mode == "interactive":
                        completed = True
                        break
                    if not compacting:
                        (root / "before-compact.events.jsonl").write_bytes(
                            next(iter(snapshot().values()))
                        )
                        os.write(master, b"/compact\r")
                        compacting = True
                    if any(
                        b"session.compaction_complete" in data
                        for data in snapshot().values()
                    ):
                        completed = True
                        break
                if process.poll() is not None:
                    break
            if completed:
                os.write(master, b"/exit\r")
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGINT)
                    process.wait(timeout=5)
            else:
                os.killpg(process.pid, signal.SIGINT)
                process.wait(timeout=5)
            (root / "terminal.txt").write_bytes(terminal)
            if not completed:
                raise RuntimeError(
                    "Expected persisted interactive/compaction completion"
                )
            (root / "interactive-status.json").write_text(
                json.dumps(
                    {
                        "persisted_ack": completed,
                        "exit": process.returncode,
                        "requests": len(requests),
                    }
                )
            )
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            os.close(master)
            server.shutdown()
            server.server_close()
            thread.join()
    report = {
        "version": args.expected_version,
        "probe_sha256": probe_sha256,
        "binary_sha256": hashlib.sha256(pathlib.Path(exe).read_bytes()).hexdigest(),
        "requests": len(requests),
        "mode": args.mode,
        "sessions": [],
    }
    saved = snapshot()
    if len(saved) != 1:
        raise RuntimeError("Expected one physical persisted session")
    for sid, data in saved.items():
        rows = [json.loads(line) for line in data.splitlines()]
        if args.mode == "cancel":
            starts = [
                row["data"] for row in rows if row["type"] == "tool.execution_start"
            ]
            aborts = [row["data"] for row in rows if row["type"] == "abort"]
            if len(starts) != 1 or starts[0]["toolCallId"] != "fixture-cancel-call":
                raise RuntimeError("Missing exact cancelled start")
            if not aborts or aborts[-1]["reason"] not in (
                "user_initiated",
                "user_abort",
            ):
                raise RuntimeError("Missing persisted user abort")
            if any(row["type"] == "tool.execution_complete" for row in rows):
                raise RuntimeError("Unexpected completed tool in cancellation fixture")
        if args.mode == "compaction":
            before = (root / "before-compact.events.jsonl").read_bytes()
            if not data.startswith(before):
                raise RuntimeError("Compaction rewrote consumed events")
            complete = [
                row["data"]
                for row in rows
                if row["type"] == "session.compaction_complete"
            ]
            if len(complete) != 1 or complete[0]["success"] is not True:
                raise RuntimeError("Missing successful compaction")
            checkpoint = pathlib.Path(complete[0]["checkpointPath"]).resolve()
            if not checkpoint.is_relative_to(root) or not checkpoint.is_file():
                raise RuntimeError("Checkpoint outside private root or absent")
            (root / "checkpoint.md").write_bytes(checkpoint.read_bytes())
        report["source_sha256"] = hashlib.sha256(data).hexdigest()
        (root / "boundary.events.jsonl").write_bytes(data)
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
