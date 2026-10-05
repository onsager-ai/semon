#!/usr/bin/env python3
"""Disposable stable Codex terminal attachment; synthetic provider, no user credentials."""

import argparse, importlib.util, os, pty, fcntl, struct, termios, select, threading, time, subprocess, tempfile, json, http.server, re, signal
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "probe", Path(__file__).with_name("codex-control.py")
)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Terminal:
    def __init__(self, argv, env, cwd):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        self.data = bytearray()
        self.p = subprocess.Popen(
            argv,
            env=env,
            cwd=cwd,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            start_new_session=True,
        )
        os.close(slave)
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        while True:
            try:
                part = os.read(self.master, 65536)
            except OSError:
                return
            if not part:
                return
            self.data.extend(part)
            if b"\x1b[6n" in part:
                os.write(self.master, b"\x1b[1;1R")

    def text(self):
        return re.sub(
            r"\x1b\[[0-?]*[ -/]*[@-~]", "", self.data.decode(errors="replace")
        )

    def wait(self, text, timeout=12):
        stop = time.monotonic() + timeout
        while time.monotonic() < stop:
            if text in self.text():
                return
            if self.p.poll() is not None:
                break
            time.sleep(0.05)
        raise TimeoutError("terminal did not show " + text + "\n" + self.text()[-2500:])

    def key(self, keys):
        os.write(self.master, keys)

    def close(self):
        if self.p.poll() is None:
            os.killpg(self.p.pid, signal.SIGKILL)
            self.p.wait()
        os.close(self.master)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--codex", required=True)
    ap.add_argument("--output", type=Path, required=True)
    a = ap.parse_args()
    assert subprocess.check_output([a.codex, "--version"], text=True).strip() == m.PIN
    report = {
        "version": m.PIN,
        "provider": "synthetic localhost; native terminal and app-server",
        "cases": {},
        "frames": [],
    }
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), m.Provider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(
            prefix="semon-terminal-", dir=Path.home()
        ) as temp:
            root = Path(temp)
            home = root / "home"
            work = root / "work"
            home.mkdir()
            work.mkdir()
            env = {
                k: v
                for k, v in os.environ.items()
                if not any(
                    x in k for x in ["TOKEN", "AUTH", "API_KEY", "CODEX", "OPENAI"]
                )
            }
            env.update(CODEX_HOME=str(home), TERM="xterm-256color", RUST_LOG="error")
            (home / "config.toml").write_text(
                f'model="gpt-6.1-sol"\nmodel_provider="synthetic"\n[model_providers.synthetic]\nname="Synthetic"\nbase_url="http://127.0.0.1:{server.server_port}/v1"\nwire_api="responses"\nrequires_openai_auth=false\n[projects.{json.dumps(str(work))}]\ntrust_level="trusted"\n'
            )
            terminal = None
            native = None
            clients = []
            try:
                daemon = subprocess.run(
                    [a.codex, "app-server", "daemon", "start"],
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                assert daemon.returncode == 0, daemon.stderr
                sock = Path(json.loads(daemon.stdout)["socketPath"])
                terminal = Terminal(
                    [
                        a.codex,
                        "--no-alt-screen",
                        "-C",
                        str(work),
                        "-a",
                        "on-request",
                        "TUI_WARMUP",
                    ],
                    env,
                    work,
                )
                terminal.wait("SYNTHETIC_ACK")
                c = m.Client(a.codex, env, sock, "default-observer", report["frames"])
                clients.append(c)
                rows = c.rpc("thread/list", {})["result"]["data"]
                tid = next(t["id"] for t in rows if t["preview"] == "TUI_WARMUP")
                c.rpc("thread/resume", {"threadId": tid})
                m.Provider.pending = m.tool(
                    "exec_command",
                    {
                        "cmd": m.COMMAND,
                        "sandbox_permissions": "require_escalated",
                        "justification": "Terminal attachment fixture",
                    },
                )
                report["cases"]["default_turn"] = c.rpc(
                    "turn/start",
                    {
                        "threadId": tid,
                        "input": [{"type": "text", "text": "DEFAULT_APPROVAL"}],
                    },
                )
                req = c.event("item/commandExecution/requestApproval")
                terminal.wait("Press enter to confirm or esc to cancel")
                report["cases"]["default_existing_terminal"] = {
                    "thread": tid,
                    "request": req,
                    "terminal": terminal.text()[-4500:],
                }
                terminal.key(b"\x1b")
                resolved = c.event("serverRequest/resolved")
                ended = c.event("turn/completed")
                report["cases"]["default_terminal_escape"] = {
                    "resolved": resolved,
                    "turn": ended,
                }
                terminal.close()
                terminal = None
                c.close()
                clients.remove(c)
                subprocess.run(
                    [a.codex, "app-server", "daemon", "stop"],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=15,
                    check=True,
                )
                sock = root / "foreground.sock"
                log = (root / "native.log").open("w")
                native = subprocess.Popen(
                    [a.codex, "app-server", "--listen", "unix://" + str(sock)],
                    env=env,
                    stdout=log,
                    stderr=log,
                    start_new_session=True,
                )
                for _ in range(200):
                    if sock.exists():
                        break
                    time.sleep(0.05)
                c = m.Client(a.codex, env, sock, "foreground-owner", report["frames"])
                clients.append(c)
                tid = c.rpc(
                    "thread/start",
                    {
                        "cwd": str(work),
                        "approvalPolicy": "on-request",
                        "sandbox": "read-only",
                    },
                )["result"]["thread"]["id"]
                c.rpc(
                    "turn/start",
                    {
                        "threadId": tid,
                        "input": [{"type": "text", "text": "FOREGROUND_WARMUP"}],
                    },
                )
                c.event("turn/completed")
                terminal = Terminal(
                    [
                        a.codex,
                        "--remote",
                        "unix://" + str(sock),
                        "-C",
                        str(work),
                        "--no-alt-screen",
                        "resume",
                        tid,
                    ],
                    env,
                    work,
                )
                terminal.wait("SYNTHETIC_ACK")
                m.Provider.pending = m.tool(
                    "exec_command",
                    {
                        "cmd": m.COMMAND,
                        "sandbox_permissions": "require_escalated",
                        "justification": "Shared server terminal fixture",
                    },
                )
                c.rpc(
                    "turn/start",
                    {
                        "threadId": tid,
                        "input": [{"type": "text", "text": "FOREGROUND_APPROVAL"}],
                    },
                )
                req = c.event("item/commandExecution/requestApproval")
                terminal.wait("Press enter to confirm or esc to cancel")
                report["cases"]["remote_unix_terminal"] = {
                    "request": req,
                    "terminal": terminal.text()[-4500:],
                }
                c.send({"id": req["id"], "result": {"decision": "decline"}})
                c.event("serverRequest/resolved")
                terminal.key(b"y\r")
                c.event("turn/completed")
                report["cases"]["viewer_wins_then_terminal_late"] = {
                    "history": c.rpc(
                        "thread/read", {"threadId": tid, "includeTurns": True}
                    )
                }
                report["cases"]["completed"] = True
            finally:
                report["provider_calls"] = [
                    {
                        "model": b.get("model"),
                        "output_feedback": [
                            x
                            for x in b.get("input", [])
                            if x.get("type")
                            in ("function_call_output", "custom_tool_call_output")
                        ],
                    }
                    for b in m.Provider.calls
                ]
                if terminal:
                    report["terminal_at_end"] = terminal.text()[-6000:]
                    terminal.close()
                for c in clients:
                    c.close()
                if native and native.poll() is None:
                    os.killpg(native.pid, 9)
                    native.wait()
                try:
                    subprocess.run(
                        [a.codex, "app-server", "daemon", "stop"],
                        env=env,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        timeout=15,
                    )
                except subprocess.TimeoutExpired:
                    pass
            report = json.loads(json.dumps(report).replace(str(root), "<FIXTURE>"))
    finally:
        server.shutdown()
        a.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
