#!/usr/bin/env python3
"""Offline protocol fixture. Never talks to OpenAI or runs a model/task."""
import json
import os
from pathlib import Path
import sys
import threading
import time

root = Path(__file__).resolve().parent
mode_file = root / "mode"
mode = mode_file.read_text().strip() if mode_file.exists() else "success"
if "--version" in sys.argv:
    print("codex-cli " + ("wrong" if mode == "wrong_version" else "0.159.0-alpha.3"))
    sys.exit(0)
assert "--stdio" in sys.argv and "app-server" in sys.argv
assert "OPENAI_API_KEY" not in os.environ and "CODEX_ACCESS_TOKEN" not in os.environ
assert not any(key.startswith("SEMON_HUB_") for key in os.environ)
assert not any(key in os.environ for key in ("GH_TOKEN", "GITHUB_TOKEN", "E2B_API_KEY", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN"))
home = Path(os.environ["CODEX_HOME"])
(root / "pid").write_text(str(os.getpid()))
with (root / "spawns").open("a") as file:
    file.write("spawn\n")
lock = threading.Lock()


def emit(value):
    with lock:
        print(json.dumps(value), flush=True)


def complete():
    # Synthetic opaque managed credential, never a real token.
    if mode != "no_credential":
        (home / "auth.json").write_text(json.dumps({
            "auth_mode": "chatgpt", "OPENAI_API_KEY": None,
            "tokens": {"access_token": "fixture-private-access", "refresh_token": "fixture-private-refresh", "id_token": "fixture-id", "account_id": "fixture-account"},
        }))
    emit({"method": "account/login/completed", "params": {"loginId": "other-login", "success": True, "error": None}})
    emit({"method": "account/login/completed", "params": {"loginId": "fixture-login", "success": mode != "failed", "error": "fixture-sensitive-error" if mode == "failed" else None}})


def approve():
    while not (root / "approve").exists():
        time.sleep(0.05)
    complete()


for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        emit({"id": message["id"], "result": {"userAgent": "offline-fixture"}})
    elif method == "account/login/start":
        assert message["params"] == {"type": "chatgptDeviceCode"}
        if mode == "oversize":
            print("x" * 65537, flush=True)
        elif mode == "rejected":
            emit({"id": message["id"], "error": {"message": "fixture-sensitive-error"}})
        else:
            if mode != "pending":
                complete()  # Completion may arrive before the RPC receipt.
            emit({"id": message["id"], "result": {"type": "chatgptDeviceCode", "loginId": "fixture-login", "verificationUrl": "https://evil.test/device" if mode == "bad_url" else "https://auth.openai.com/codex/device", "userCode": "FIXT-1234"}})
            if mode == "pending":
                threading.Thread(target=approve, daemon=True).start()
    elif method == "account/read":
        assert message["params"] == {"refreshToken": False}
        emit({"id": message["id"], "result": {"account": {"type": "apiKey"} if mode == "account_api_key" else {"type": "chatgpt", "email": "fixture@example.test", "planType": "plus"}, "requiresOpenaiAuth": True}})
    elif method != "initialized":
        raise AssertionError("Unexpected RPC: " + method)
