#!/usr/bin/env python3
"""Native mock transcript probe; copied records do not prove logical lineage."""

import argparse, hashlib, http.server, threading, subprocess, os, json, pathlib


def main():
    parser = argparse.ArgumentParser(
        description="Isolated pinned Claude resume/fork probe with a localhost synthetic model."
    )
    parser.add_argument("--claude", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(mode=0o700, exist_ok=False)
    for directory in ["work", "config", "tmp"]:
        (root / directory).mkdir(mode=0o700)

    requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            x = json.loads(self.rfile.read(int(self.headers["content-length"])))
            requests.append(
                {"path": self.path, "model": x.get("model"), "stream": x.get("stream")}
            )
            model = x.get("model", "claude-sonnet-4-6")
            msg = {
                "id": f"fixture-msg-{len(requests)}",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{"type": "text", "text": "SYNTHETIC_CLAUDE_ACK"}],
                "stop_reason": "end_turn",
                "stop_sequence": None,
                "usage": {
                    "input_tokens": 5,
                    "output_tokens": 3,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
            }
            self.send_response(200)
            if x.get("stream"):
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                start = dict(msg)
                start["content"] = []
                start["stop_reason"] = None
                start["usage"] = dict(msg["usage"], output_tokens=0)
                events = [
                    {"type": "message_start", "message": start},
                    {
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {"type": "text", "text": ""},
                    },
                    {
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": "SYNTHETIC_CLAUDE_ACK"},
                    },
                    {"type": "content_block_stop", "index": 0},
                    {
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                        "usage": {"output_tokens": 3},
                    },
                    {"type": "message_stop"},
                ]
                for e in events:
                    self.wfile.write(
                        (
                            "event: " + e["type"] + "\ndata: " + json.dumps(e) + "\n\n"
                        ).encode()
                    )
            else:
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(msg).encode())

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {
        k: os.environ[k]
        for k in ["PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TERM"]
        if k in os.environ
    }
    env.update(
        CLAUDE_CONFIG_DIR=str(root / "config"),
        TMPDIR=str(root / "tmp"),
        ANTHROPIC_BASE_URL=f"http://127.0.0.1:{server.server_port}",
        ANTHROPIC_API_KEY="fixture-localhost-only",
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1",
        DISABLE_AUTOUPDATER="1",
        DISABLE_TELEMETRY="1",
        DISABLE_ERROR_REPORTING="1",
    )
    cli = str(args.claude.resolve(strict=True))
    try:
        version = subprocess.run(
            [cli, "--version"], env=env, capture_output=True, text=True, check=True
        )
        if not version.stdout.startswith("2.1.288 "):
            raise RuntimeError("Expected pinned Claude 2.1.288")
        p = subprocess.run(
            [
                cli,
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                "claude-sonnet-4-6",
                "SEMON_SYNTHETIC_CLAUDE_235",
            ],
            cwd=root / "work",
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        p.check_returncode()
        initial = next((root / "config/projects").glob("*/*.jsonl"))
        sid = initial.stem
        initial_bytes = initial.read_bytes()
        (root / "initial-transcript.jsonl").write_bytes(initial_bytes)
        for case, extra in [
            ("resume", ["--resume", sid]),
            ("fork", ["--resume", sid, "--fork-session"]),
        ]:
            q = subprocess.run(
                [
                    cli,
                    "--print",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--model",
                    "claude-sonnet-4-6",
                    *extra,
                    "SEMON_SYNTHETIC_" + case.upper() + "_235",
                ],
                cwd=root / "work",
                env=env,
                capture_output=True,
                text=True,
                timeout=60,
            )
            q.check_returncode()
            if case == "resume":
                resumed = initial.read_bytes()
                if not resumed.startswith(initial_bytes):
                    raise RuntimeError("Resume mutated consumed prefix")
                (root / "resumed-transcript.jsonl").write_bytes(resumed)
            else:
                children = [
                    x
                    for x in (root / "config/projects").glob("*/*.jsonl")
                    if x != initial
                ]
                if len(children) != 1:
                    raise RuntimeError("Expected one distinct fork transcript")
                forked = children[0].read_bytes()
                (root / "forked-transcript.jsonl").write_bytes(forked)
            (root / (case + ".stdout.jsonl")).write_text(q.stdout)
            (root / (case + ".stderr.txt")).write_text(q.stderr)
            print(
                {
                    "case": case,
                    "exit": q.returncode,
                    "saved_files": [
                        x.name for x in (root / "config/projects").glob("*/*.jsonl")
                    ],
                    "stderr_tail": q.stderr[-500:],
                }
            )
        (root / "stdout.jsonl").write_text(p.stdout)
        (root / "stderr.txt").write_text(p.stderr)
        (root / "report.json").write_text(
            json.dumps(
                {
                    "version": version.stdout.strip(),
                    "binary_sha256": hashlib.sha256(
                        pathlib.Path(cli).read_bytes()
                    ).hexdigest(),
                    "sources": {
                        name: hashlib.sha256((root / name).read_bytes()).hexdigest()
                        for name in [
                            "initial-transcript.jsonl",
                            "resumed-transcript.jsonl",
                            "forked-transcript.jsonl",
                        ]
                    },
                },
                indent=2,
            )
            + "\n"
        )
        print(
            {
                "version": version.stdout.strip(),
                "exit": p.returncode,
                "requests": requests,
                "saved_files": [
                    str(x.relative_to(root)) for x in (root / "config").rglob("*.jsonl")
                ],
                "stderr_tail": p.stderr[-1000:],
            }
        )
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
