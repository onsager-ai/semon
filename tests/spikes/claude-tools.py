#!/usr/bin/env python3
"""Pinned native tool persistence and model-switch probe using synthetic localhost replies."""

import argparse, hashlib, http.server, threading, subprocess, os, json, pathlib


def main():
    parser = argparse.ArgumentParser(
        description="Isolated Claude 2.1.288 Read/Edit/Bash and model-switch persistence probe."
    )
    parser.add_argument("--claude", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    probe_sha256 = hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
    root = args.output.resolve()
    root.mkdir(mode=0o700, exist_ok=False)
    for directory in ["work", "config", "tmp"]:
        (root / directory).mkdir(mode=0o700)

    (root / "work/fixture.txt").write_text("SYNTHETIC_BEFORE\n")
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
            if len(requests)==1:
                msg['content']=[
                    {'type':'tool_use','id':'fixture-read','name':'Read','input':{'file_path':str(root/'work/fixture.txt')}},
                    {'type':'tool_use','id':'fixture-bash-one','name':'Bash','input':{'command':'printf SYNTHETIC_BASH_ONE','description':'synthetic command success'}},
                    {'type':'tool_use','id':'fixture-bash-two','name':'Bash','input':{'command':'printf SYNTHETIC_BASH_TWO; : > BASH-TWO-DONE; exit 1','description':'synthetic command failure'}},
                ]
                msg['stop_reason']='tool_use'
            elif len(requests)==2:
                msg['content']=[{'type':'tool_use','id':'fixture-edit','name':'Edit','input':{'file_path':str(root/'work/fixture.txt'),'old_string':'SYNTHETIC_BEFORE','new_string':'SYNTHETIC_AFTER'}}]
                msg['stop_reason']='tool_use'
            self.send_response(200)
            if x.get("stream"):
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                start = dict(msg)
                start["content"] = []
                start["stop_reason"] = None
                start["usage"] = dict(msg["usage"], output_tokens=0)
                events=[{'type':'message_start','message':start}]
                for index,block in enumerate(msg['content']):
                    opening=dict(block)
                    if block['type']=='tool_use':opening['input']={}
                    else:opening['text']=''
                    events.append({'type':'content_block_start','index':index,'content_block':opening})
                    delta={'type':'input_json_delta','partial_json':json.dumps(block['input'])} if block['type']=='tool_use' else {'type':'text_delta','text':block['text']}
                    events.append({'type':'content_block_delta','index':index,'delta':delta})
                    events.append({'type':'content_block_stop','index':index})
                events.extend([{'type':'message_delta','delta':{'stop_reason':msg['stop_reason'],'stop_sequence':None},'usage':{'output_tokens':3}},{'type':'message_stop'}])
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
                "--permission-mode", "default", "--allowedTools", "Read", "Edit", "Bash", "--", "SEMON_SYNTHETIC_CLAUDE_235",
            ],
            cwd=root / "work",
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        (root / "initial-stdout.jsonl").write_text(p.stdout)
        (root / "initial-stderr.txt").write_text(p.stderr)
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
                    "claude-haiku-4-5" if case == "resume" else "claude-sonnet-4-6",
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
        initial_rows = [json.loads(line) for line in initial_bytes.splitlines()]
        calls = [block for row in initial_rows if row.get("type") == "assistant"
                 for block in row.get("message", {}).get("content", [])
                 if isinstance(block, dict) and block.get("type") == "tool_use"]
        results = [block for row in initial_rows if row.get("type") == "user"
                   for block in row.get("message", {}).get("content", [])
                   if isinstance(block, dict) and block.get("type") == "tool_result"]
        expected = {"fixture-read", "fixture-bash-one", "fixture-bash-two", "fixture-edit"}
        if {block["id"] for block in calls} != expected or len(calls) != 4:
            raise RuntimeError("Expected four exact native tool calls")
        if {block["tool_use_id"] for block in results} != expected or len(results) != 4:
            raise RuntimeError("Expected four exact native tool results")
        failures = {block["tool_use_id"] for block in results if block.get("is_error")}
        if failures != {"fixture-bash-two"}:
            raise RuntimeError("Unexpected persisted tool error flags")
        if (root / "work/fixture.txt").read_text() != "SYNTHETIC_AFTER\n":
            raise RuntimeError("Native Edit did not update disposable fixture")
        models = {row.get("message", {}).get("model") for row in map(json.loads, resumed.splitlines()) if row.get("type") == "assistant"}
        if models != {"claude-sonnet-4-6", "claude-haiku-4-5"}:
            raise RuntimeError("Expected persisted model switch")
        (root / "stdout.jsonl").write_text(p.stdout)
        (root / "stderr.txt").write_text(p.stderr)
        (root / "report.json").write_text(
            json.dumps(
                {
                    "version": version.stdout.strip(),
                    "probe_sha256": probe_sha256,
                    "requests": requests,
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
