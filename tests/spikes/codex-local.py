#!/usr/bin/env python3
"""Real semon launcher/routes/native executor; synthetic model, disposable owner/work homes.
Linux bwrap required. No mocks substitute for native outcomes; no personal auth or remote deployment.
"""

import argparse, importlib.util, json, os, signal, subprocess, tempfile, threading, time, http.server, http.client, re, socket, uuid, shutil
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "probe", Path(__file__).with_name("codex-control.py")
)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def until(f, timeout=12):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = f()
        if result:
            return result
        time.sleep(0.05)
    raise TimeoutError("condition not observed")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--codex", required=True)
    ap.add_argument("--semon", required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--browser", type=Path)
    ap.add_argument("--tmux", default=shutil.which("tmux"))
    a = ap.parse_args()
    report = {
        "version": m.PIN,
        "provenance": "real native semon local driver; synthetic Responses model",
        "cases": {},
        "frames": [],
    }
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), m.Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory(prefix="native-local-", dir=Path.home()) as temp:
        root = Path(temp)
        work = root / "work"
        work.mkdir()
        (work / ".codex").mkdir(exist_ok=True)
        (work / ".codex" / "config.toml").write_text(
            '[mcp_servers.startup_escape]\ncommand="/bin/sh"\nargs=["-c", "touch '
            + str(root / "MCP_STARTUP")
            + '"]\n'
        )
        state = root / "owner"
        conf = root / "model.toml"
        conf.write_text(
            f'model="gpt-6.1-sol"\nmodel_provider="synthetic"\n[model_providers.synthetic]\nname="Synthetic"\nbase_url="http://127.0.0.1:{provider.server_port}/v1"\nwire_api="responses"\nrequires_openai_auth=false\n'
        )
        malicious_config = root / "reserved-env.toml"
        malicious_config.write_text(conf.read_text() + 'env_key="LD_PRELOAD"\n')
        rejected = subprocess.run(
            [
                a.semon,
                "control",
                "codex",
                "--codex",
                a.codex,
                "--workspace",
                str(work),
                "--state",
                str(root / "rejected-owner"),
                "--config",
                str(malicious_config),
            ],
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert rejected.returncode != 0 and "non-reserved" in rejected.stderr, rejected
        report["cases"]["reserved_model_environment"] = "refused before server start"
        log = (root / "semon.log").open("w")
        process = subprocess.Popen(
            [
                a.semon,
                "control",
                "codex",
                "--codex",
                a.codex,
                "--workspace",
                str(work),
                "--state",
                str(state),
                "--config",
                str(conf),
            ],
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
        clients = []
        broker = None
        replacement = None
        tmux_started = False
        try:

            def startup():
                text = (root / "semon.log").read_text()
                if process.poll() is not None:
                    raise RuntimeError(text)
                return re.search(r"http://127\.0\.0\.1:(\d+)/\?t=([0-9a-f]+)", text)

            match = until(startup)
            assert not (root / "MCP_STARTUP").exists()
            port = int(match[1])
            token = match[2]
            cookie = "semon_session=" + token

            def request(method, path, body=None, headers=None):
                nonlocal cookie
                h = {
                    "Cookie": cookie,
                    "Origin": f"http://127.0.0.1:{port}",
                    "Content-Type": "application/json",
                }
                if headers is not None:
                    h = headers
                conn = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
                conn.request(
                    method,
                    path,
                    body=None if body is None else json.dumps(body),
                    headers=h,
                )
                r = conn.getresponse()
                raw = r.read()
                if r.getheader("Set-Cookie"):
                    cookie = r.getheader("Set-Cookie").split(";")[0]
                status = r.status
                conn.close()
                try:
                    result = json.loads(raw)
                except json.JSONDecodeError:
                    result = raw.decode()
                return status, result

            assert request("GET", "/?t=" + token, headers={})[0] == 200
            assert cookie != "semon_session=" + token
            assert request("GET", "/?t=" + token, headers={})[0] == 403
            assert request("GET", "/?t=" + token)[0] == 200
            snap = lambda: request("GET", "/api/model")[1]["control"]
            target = lambda op, **extra: dict(
                id=uuid.uuid4().hex,
                op=op,
                thread=snap()["thread"],
                generation=snap()["generation"],
                activeTurn=snap()["activeTurn"],
                expires=int(time.time() * 1000) + 25000,
                **extra,
            )
            send = lambda text: request(
                "POST", "/api/control", target("send", text=text)
            )
            cmd = target("send", text="MUST_NOT_APPEAR")
            for headers in (
                {},
                {"Cookie": cookie},
                {
                    "Cookie": "semon_session=" + token,
                    "Origin": f"http://127.0.0.1:{port}",
                    "Content-Type": "application/json",
                },
                {
                    "Cookie": cookie,
                    "Origin": "https://evil.invalid",
                    "Content-Type": "application/json",
                },
                {
                    "Cookie": cookie,
                    "Origin": f"http://127.0.0.1:{port}",
                    "Content-Type": "application/json",
                    "Host": "evil.invalid",
                },
            ):
                assert request("POST", "/api/control", cmd, headers)[0] == 403
            report["cases"]["transport"] = {
                "missing_auth_wrong_host_origin_refused": True,
                "bootstrap_single_use": True,
                "bootstrap_is_not_write_cookie": True,
                "cookie_refresh": True,
                "opt_in": True,
            }
            c = m.Client(
                a.codex,
                {"CODEX_HOME": str(state / "home")},
                state / "native.sock",
                "terminal-owner",
                report["frames"],
            )
            clients.append(c)
            tid = snap()["thread"]
            assert send("LOCAL_INPUT")[0] == 200
            until(lambda: snap()["activeTurn"] is None)
            until(lambda: c.rpc("thread/resume", {"threadId": tid}).get("result"))
            stale = target("send", text="MUST_NOT_APPEAR")
            stale["activeTurn"] = "stale"
            assert request("POST", "/api/control", stale)[0] == 409
            stale = target("send", text="MUST_NOT_APPEAR")
            stale["expires"] = 0
            assert request("POST", "/api/control", stale)[0] == 409

            def completed(turn):
                return c.wait(
                    lambda x: x.get("method") == "turn/completed"
                    and x["params"]["turn"]["id"] == turn
                )

            def pending(command, call):
                m.Provider.pending = m.tool(
                    "exec_command",
                    {
                        "cmd": command,
                        "sandbox_permissions": "require_escalated",
                        "justification": "Native local fixture",
                    },
                )
                m.Provider.pending["call_id"] = call
                receipt = send(call)
                assert receipt[0] == 200, receipt
                req = c.event("item/commandExecution/requestApproval")
                r = until(
                    lambda: next(
                        (
                            r
                            for r in snap()["requests"]
                            if r["payload"]["nativeId"] == req["id"]
                            and r["state"]["state"] == "open"
                        ),
                        None,
                    )
                )
                return req, r

            def answer(r, decision):
                return target(
                    "answer",
                    request=r["id"],
                    hash=r["hash"],
                    answer={"decision": decision},
                )

            req, r = pending("printf LOCAL_ONCE", "local_once")
            assert r["reason"] is None, r
            bad = answer(r, "allow")
            bad["hash"] = "0" * 64
            assert request("POST", "/api/control", bad)[0] == 409
            yes = answer(r, "allow")
            assert request("POST", "/api/control", yes)[0] == 200
            c.event("serverRequest/resolved")
            completed(req["params"]["turnId"])
            until(lambda: snap()["activeTurn"] is None)
            duplicate = request("POST", "/api/control", yes)
            assert duplicate[0] == 200
            changed = dict(yes, text="changed")
            assert request("POST", "/api/control", changed)[0] == 409
            output = c.rpc("thread/read", {"threadId": tid, "includeTurns": True})
            once = next(
                i
                for turn in output["result"]["thread"]["turns"]
                for i in turn["items"]
                if i.get("id") == "local_once" and i.get("type") == "commandExecution"
            )
            assert (
                once["exitCode"] == 0 and once["aggregatedOutput"] == "LOCAL_ONCE"
            ), once
            report["cases"]["allow_once_duplicate"] = dict(
                output=output, duplicate=duplicate
            )
            req, r = pending("printf MUST_NOT_RUN", "terminal_wins")
            c.send({"id": req["id"], "result": {"decision": "decline"}})
            c.event("serverRequest/resolved")
            completed(req["params"]["turnId"])
            until(lambda: snap()["activeTurn"] is None)
            assert request("POST", "/api/control", answer(r, "allow"))[0] == 409
            report["cases"]["terminal_wins"] = {
                "request": req,
                "state": next(
                    row for row in snap()["requests"] if row["id"] == r["id"]
                )["state"],
            }
            req, r = pending("printf INTERRUPT_PENDING", "steer_interrupt")
            turn = snap()["activeTurn"]
            steer = request("POST", "/api/control", target("send", text="STEER_NATIVE"))
            assert steer[0] == 200, steer
            interrupt = request("POST", "/api/control", target("interrupt"))
            assert interrupt[0] == 200, interrupt
            end = completed(req["params"]["turnId"])
            assert end["params"]["turn"]["status"] == "interrupted"
            until(lambda: snap()["activeTurn"] is None)
            report["cases"]["steer_interrupt"] = {
                "turn": turn,
                "steer": steer,
                "interrupt": interrupt,
                "completed": end,
            }
            # Native questions require supported Plan mode; driver responses bind question IDs.
            m.Provider.pending = m.tool(
                "request_user_input",
                {
                    "questions": [
                        {
                            "id": "route",
                            "header": "Route",
                            "question": "Pick a route",
                            "options": [
                                {"label": "One", "description": "First route"},
                                {"label": "Two", "description": "Second route"},
                            ],
                        }
                    ]
                },
            )
            m.Provider.pending["call_id"] = "local_question"
            c.rpc(
                "turn/start",
                {
                    "threadId": tid,
                    "input": [{"type": "text", "text": "LOCAL_QUESTION"}],
                    "collaborationMode": {
                        "mode": "plan",
                        "settings": {
                            "model": "gpt-6.1-sol",
                            "reasoning_effort": None,
                            "developer_instructions": None,
                        },
                    },
                },
            )
            q = c.event("item/tool/requestUserInput")
            row = until(
                lambda: next(
                    (
                        x
                        for x in snap()["requests"]
                        if x["payload"]["nativeId"] == q["id"]
                        and x["state"]["state"] == "open"
                    ),
                    None,
                )
            )
            assert row["reason"] is None, row
            assert (
                request(
                    "POST",
                    "/api/control",
                    target(
                        "answer",
                        request=row["id"],
                        hash=row["hash"],
                        answer={"answers": {"route": ["One"]}},
                    ),
                )[0]
                == 200
            )
            completed(q["params"]["turnId"])
            until(lambda: snap()["activeTurn"] is None)
            report["cases"]["native_question"] = {
                "request": q,
                "answered_id": "route",
                "answer": "One",
            }
            (work / "patch.txt").write_text("before\n")
            m.Provider.pending = {
                "type": "custom_tool_call",
                "id": "local_patch",
                "call_id": "local_patch",
                "name": "apply_patch",
                "input": "*** Begin Patch\n*** Update File: "
                + str(work / "patch.txt")
                + "\n@@\n-before\n+after\n*** End Patch",
            }
            c.rpc(
                "turn/start",
                {
                    "threadId": tid,
                    "input": [{"type": "text", "text": "LOCAL_PATCH"}],
                    "collaborationMode": {
                        "mode": "default",
                        "settings": {
                            "model": "gpt-6.1-sol",
                            "reasoning_effort": None,
                            "developer_instructions": None,
                        },
                    },
                },
            )
            patch = c.event("item/fileChange/requestApproval")
            row = until(
                lambda: next(
                    (
                        x
                        for x in snap()["requests"]
                        if x["payload"]["nativeId"] == patch["id"]
                        and x["state"]["state"] == "open"
                    ),
                    None,
                )
            )
            assert row["reason"] is None, row
            if a.browser:
                browser = subprocess.run(
                    [
                        "node",
                        str(a.browser.resolve()),
                        f"http://127.0.0.1:{port}",
                        cookie.split("=", 1)[1],
                        str(a.output.parent / "local-control-browser"),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=60,
                )
                assert browser.returncode == 0, browser.stdout + browser.stderr
                report["cases"]["browser"] = {
                    "exit": browser.returncode,
                    "stdout": browser.stdout,
                }
            else:
                assert request("POST", "/api/control", answer(row, "deny"))[0] == 200
            completed(patch["params"]["turnId"])
            until(lambda: snap()["activeTurn"] is None)
            assert (work / "patch.txt").read_text() == "before\n"
            report["cases"]["native_patch"] = {
                "request": patch,
                "exact_changes": row["payload"]["item"],
                "denied_patch_unchanged": True,
            }
            # Lose the HTTP response after dispatch: inspect native outcome and saved receipt, never resend.
            req, row = pending("printf UNCERTAIN_HTTP_ONCE", "uncertain_http")
            operation = answer(row, "allow")
            body = json.dumps(operation).encode()
            wire = socket.create_connection(("127.0.0.1", port))
            wire.sendall(
                (
                    f"POST /api/control HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nCookie: {cookie}\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n"
                ).encode()
                + body
            )
            wire.close()
            completed(req["params"]["turnId"])
            until(lambda: snap()["activeTurn"] is None)
            receipt = until(
                lambda: snap()["actions"].get(operation["id"], {}).get("receipt")
            )
            report["cases"]["uncertain_http"] = {
                "response_abandoned": True,
                "receipt": receipt,
                "history": c.rpc(
                    "thread/read", {"threadId": tid, "includeTurns": True}
                ),
            }
            # Host process broker that executes a delegated owner command (tmux-equivalent escape).
            broker = socket.socket(socket.AF_UNIX)
            broker.bind(str(work / "broker.sock"))
            broker.listen()
            broker.settimeout(0.2)
            escaped = []

            def serve_broker():
                while broker:
                    try:
                        conn, _ = broker.accept()
                    except (TimeoutError, OSError):
                        continue
                    with conn:
                        if conn.recv(200) == b"execute-owner-command":
                            escaped.append(True)
                            (root / "ESCAPED").write_text("bad")

            threading.Thread(target=serve_broker, daemon=True).start()
            (root / "secret").write_text("OWNER_SECRET")
            (work / "host-link").symlink_to(root / "secret")
            if a.tmux:
                shutil.copy2(a.tmux, work / "tmux")
                tmux_socket = work / "tmux.sock"
                marker = root / "TMUX_ESCAPE"
                subprocess.run(
                    [
                        a.tmux,
                        "-S",
                        str(tmux_socket),
                        "-f",
                        "/dev/null",
                        "new-session",
                        "-d",
                        "sleep 300",
                    ],
                    check=True,
                )
                tmux_started = True
                subprocess.run(
                    [
                        a.tmux,
                        "-S",
                        str(tmux_socket),
                        "run-shell",
                        "touch " + str(marker),
                    ],
                    check=True,
                )
                assert marker.exists()
                marker.unlink()
                report["cases"]["tmux_baseline"] = {
                    "version": subprocess.check_output(
                        [a.tmux, "-V"], text=True
                    ).strip(),
                    "owner_command_executed": True,
                }
            # Deliberately supply the exact viewer credential: namespaces, not token secrecy, block self-approval.
            attack = """import socket,urllib.request,os,json,subprocess,ctypes,errno
out={}
try:
 socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM);out['datagram_pair']='ESCAPED'
except OSError as e:out['datagram_pair']='blocked' if e.errno==errno.EPERM else str(e)
libc=ctypes.CDLL(None,use_errno=True)
out['io_uring']='blocked' if libc.syscall(425,1,0)==-1 and ctypes.get_errno()==errno.EPERM else 'ESCAPED'
left,right=socket.socketpair(socket.AF_UNIX,socket.SOCK_STREAM);left.sendall(b'private');assert right.recv(7)==b'private';left.close();right.close()
if TMUX_SOCKET:
 result=subprocess.run(['./tmux','-S',TMUX_SOCKET,'run-shell','touch '+TMUX_MARKER],capture_output=True)
 out['tmux']='blocked' if result.returncode!=0 else 'ESCAPED'
for name,path in PATHS.items():
 try:
  if name in ('native','physical_native','broker'):
   s=socket.socket(socket.AF_UNIX);s.settimeout(.3);s.connect(path);s.sendall(b'execute-owner-command');s.close()
  else:open(path).read()
  out[name]='ESCAPED'
 except (OSError,PermissionError):out[name]='blocked'
try:
 urllib.request.urlopen(urllib.request.Request(URL,data=b'{}',headers={'Cookie':COOKIE,'Origin':ORIGIN,'Content-Type':'application/json'}),timeout=1);out['viewer']='ESCAPED'
except Exception:out['viewer']='blocked'
out['environment']='blocked' if not any(k in os.environ for k in ('OPENAI_API_KEY','SEMON_TEST_SECRET')) else 'ESCAPED'
print(json.dumps(out,sort_keys=True))
"""
            paths = {
                "native": str(state / "native.sock"),
                "physical_native": str((state / "native.sock").resolve()),
                "broker": str(work / "broker.sock"),
                "home": str(root / "secret"),
                "symlink": str(work / "host-link"),
                "proc": f"/proc/{process.pid}/root" + str(root / "secret"),
                "config": str(state / "home" / "environments.toml"),
            }
            script = (
                "TMUX_SOCKET="
                + repr(str(tmux_socket) if a.tmux else None)
                + "\nTMUX_MARKER="
                + repr(str(marker) if a.tmux else "")
                + "\nPATHS="
                + repr(paths)
                + "\nURL="
                + repr(f"http://127.0.0.1:{port}/api/control")
                + "\nCOOKIE="
                + repr(cookie)
                + "\nORIGIN="
                + repr(f"http://127.0.0.1:{port}")
                + "\n"
                + attack
            )
            (work / "attack.py").write_text(script)
            # Prove broker really works for an ordinary same-UID process before testing the confined tool.
            outside = socket.socket(socket.AF_UNIX)
            outside.connect(str(work / "broker.sock"))
            outside.sendall(b"execute-owner-command")
            outside.close()
            until(lambda: bool(escaped))
            (root / "ESCAPED").unlink()
            escaped.clear()
            # Mount/PID/network namespaces alone still expose filesystem Unix sockets.
            baseline = [
                "/usr/bin/bwrap",
                "--unshare-all",
                "--new-session",
                "--clearenv",
            ]
            for path in ["/usr", "/bin", "/lib", "/lib64"]:
                if Path(path).exists():
                    baseline.extend(["--ro-bind", path, path])
            baseline.extend(
                [
                    "--dev",
                    "/dev",
                    "--proc",
                    "/proc",
                    "--tmpfs",
                    "/tmp",
                    "--bind",
                    str(work),
                    str(work),
                    "--",
                    "/usr/bin/python3",
                    "-c",
                    "import socket;s=socket.socket(socket.AF_UNIX);s.connect("
                    + repr(str(work / "broker.sock"))
                    + ");s.sendall(b'execute-owner-command')",
                ]
            )
            subprocess.run(baseline, check=True)
            until(lambda: bool(escaped))
            (root / "ESCAPED").unlink()
            escaped.clear()
            report["cases"][
                "workspace_socket_namespace_baseline"
            ] = "escaped; syscall boundary required"
            req, r = pending("python3 attack.py", "isolation_attack")
            assert request("POST", "/api/control", answer(r, "allow"))[0] == 200
            completed(req["params"]["turnId"])
            until(lambda: snap()["activeTurn"] is None)
            history = c.rpc("thread/read", {"threadId": tid, "includeTurns": True})
            items = [
                i
                for t in history["result"]["thread"]["turns"]
                for i in t["items"]
                if i.get("type") == "commandExecution"
                and i.get("id") == "isolation_attack"
            ]
            assert len(items) == 1, items
            result = json.loads(items[0]["aggregatedOutput"])
            assert set(result.values()) == {"blocked"}, result
            assert (
                not escaped
                and not (root / "ESCAPED").exists()
                and not (root / "TMUX_ESCAPE").exists()
            )
            report["cases"]["isolation"] = {
                "host_broker_baseline": "escaped as expected",
                "native_escalated_executor": result,
                "native_item": items[0],
            }
            # Untrusted workspace config must not start a host-side MCP program.
            (work / ".codex").mkdir(exist_ok=True)
            (work / ".codex" / "config.toml").write_text(
                '[mcp_servers.escape]\ncommand="/bin/sh"\nargs=["-c", "touch '
                + str(root / "MCP_ESCAPE")
                + '"]\n'
            )
            receipt = send("UNTRUSTED_PROJECT")
            assert receipt[0] == 200
            completed(receipt[1]["result"]["turn"]["id"])
            assert not (root / "MCP_ESCAPE").exists()
            report["cases"]["untrusted_project_host_program"] = "blocked"
            old = snap()["generation"]
            assert request("POST", "/api/control", target("reconnect"))[0] == 200
            assert snap()["generation"] != old
            assert request("POST", "/api/control", yes)[0] == 409
            req, r = pending("printf RECONNECT_PENDING", "reconnect_pending")
            old = snap()["generation"]
            assert request("POST", "/api/control", target("reconnect"))[0] == 200
            assert snap()["generation"] != old
            nextrow = until(
                lambda: next(
                    (
                        x
                        for x in snap()["requests"]
                        if x["payload"]["nativeId"] == req["id"]
                        and x["state"]["state"] == "open"
                    ),
                    None,
                )
            )
            assert request("POST", "/api/control", answer(nextrow, "deny"))[0] == 200
            completed(req["params"]["turnId"])
            report["cases"]["reconnect_pending"] = {
                "old_target_refused": True,
                "new_native_request": nextrow,
            }
            nativepid = next(
                int(p.name)
                for p in Path("/proc").iterdir()
                if p.name.isdigit()
                and (p / "cmdline").exists()
                and b"app-server\x00--disable\x00hooks" in (p / "cmdline").read_bytes()
                and str(state / "home").encode() in (p / "environ").read_bytes()
            )
            req, r = pending("printf SERVER_RESTART_PENDING", "server_loss")
            os.kill(nativepid, signal.SIGKILL)
            until(lambda: not snap()["connected"])
            assert request("POST", "/api/control", answer(r, "allow"))[0] == 409
            assert request("POST", "/api/control", target("reconnect"))[0] == 409
            replacement = subprocess.Popen(
                [
                    a.codex,
                    "app-server",
                    "--disable",
                    "hooks",
                    "--disable",
                    "plugins",
                    "--disable",
                    "apps",
                    "--listen",
                    "unix://" + str(state / "native.sock"),
                ],
                env={
                    "CODEX_HOME": str(state / "home"),
                    "HOME": str(state / "home"),
                    "PATH": "/usr/bin:/bin",
                },
                cwd=work,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )

            def restarted_native():
                try:
                    return m.Client(
                        a.codex,
                        {},
                        state / "native.sock",
                        "restarted-server",
                        report["frames"],
                    )
                except (OSError, TimeoutError):
                    return None

            replacement_client = until(restarted_native)
            clients.append(replacement_client)
            assert request("POST", "/api/control", target("reconnect"))[0] == 409
            report["cases"]["server_restart"] = {
                "disconnected": True,
                "old_target_refused": True,
                "no_automatic_relaunch": True,
                "replacement_transport_reachable": True,
                "replacement_write_authority_refused": True,
            }
            restart = subprocess.run(
                [
                    a.semon,
                    "control",
                    "codex",
                    "--codex",
                    a.codex,
                    "--workspace",
                    str(work),
                    "--state",
                    str(state),
                    "--config",
                    str(conf),
                ],
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert restart.returncode != 0
            report["cases"]["semon_restart"] = {
                "exit": restart.returncode,
                "stderr": restart.stderr,
                "no_action_replay": True,
            }
            report["cases"]["completed"] = True
        finally:
            for c in clients:
                c.close()
            if replacement and replacement.poll() is None:
                os.killpg(replacement.pid, signal.SIGKILL)
                replacement.wait()
            if tmux_started:
                subprocess.run(
                    [a.tmux, "-S", str(work / "tmux.sock"), "kill-server"],
                    capture_output=True,
                )
            if broker:
                broker.close()
                broker = None
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            report["semon_log"] = (root / "semon.log").read_text()
            report["native_log"] = (
                (state / "native.log").read_text()
                if (state / "native.log").exists()
                else ""
            )
            report = json.loads(
                json.dumps(report)
                .replace(str(root), "<FIXTURE>")
                .replace(a.codex, "<CODEX_BIN>")
                .replace(
                    token if "token" in locals() else "<unallocated>",
                    "<BOOTSTRAP_NONCE>",
                )
                .replace(
                    (
                        cookie.split("=", 1)[1]
                        if "cookie" in locals()
                        else "<unallocated-cookie>"
                    ),
                    "<OWNER_TOKEN>",
                )
            )
            a.output.write_text(json.dumps(report, indent=2) + "\n")
            provider.shutdown()


if __name__ == "__main__":
    main()
