#!/usr/bin/env python3
"""Read-only protocol smoke for the pinned Codex binary; never starts a login.

Usage: python3 tests/spikes/codex-auth-protocol.py /absolute/path/to/codex
Only a temporary, empty Codex home is used. No tokens or model calls are made.
"""

import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import time

VERSION = "0.159.0-alpha.3"


def main():
    executable = str(Path(sys.argv[1]).resolve())
    with tempfile.TemporaryDirectory(prefix="semon-codex-protocol-") as temporary:
        root = Path(temporary)
        home = root / "home"
        home.mkdir(mode=0o700)
        env = dict(os.environ, CODEX_HOME=str(home))
        # This test must not use inherited credentials or a user's home.
        for name in ("OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"):
            env.pop(name, None)

        def run(*arguments):
            result = subprocess.run(
                [executable, *arguments], env=env, capture_output=True, timeout=30
            )
            if result.returncode:
                raise RuntimeError("pinned protocol command failed; output withheld")
            return result.stdout.decode()

        assert run("--version").strip() == f"codex-cli {VERSION}"
        help_text = run("login", "--help")
        for flag in ("--device-auth", "--with-api-key", "--with-access-token"):
            assert flag in help_text
        schema = root / "schema"
        run("app-server", "generate-json-schema", "--out", str(schema))
        parameters = json.loads((schema / "v2/LoginAccountParams.json").read_bytes())
        kinds = {
            item["properties"]["type"]["enum"][0]: item
            for item in parameters["oneOf"]
        }
        assert "chatgptDeviceCode" in kinds and "apiKey" in kinds
        assert "FOR OPENAI INTERNAL USE ONLY" in kinds["chatgptAuthTokens"]["description"]
        responses = json.loads((schema / "v2/LoginAccountResponse.json").read_bytes())
        device = next(
            item for item in responses["oneOf"]
            if item["properties"]["type"]["enum"] == ["chatgptDeviceCode"]
        )
        assert set(device["required"]) == {"type", "loginId", "userCode", "verificationUrl"}

        process = subprocess.Popen(
            [executable, "app-server", "--stdio"], env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, bufsize=0,
        )
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        try:
            def send(message):
                process.stdin.write((json.dumps(message) + "\n").encode())

            send({"id": 1, "method": "initialize", "params": {
                "clientInfo": {"name": "semon-auth-protocol-smoke", "version": "0.1.0"}
            }})
            received = {}
            pending = b""
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline and 2 not in received:
                if not selector.select(timeout=0.5):
                    continue
                chunk = os.read(process.stdout.fileno(), 65536)
                if not chunk:
                    break
                pending += chunk
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    message = json.loads(line)
                    if message.get("id") == 1:
                        assert "result" in message
                        received[1] = True
                        send({"method": "initialized", "params": {}})
                        send({"id": 2, "method": "account/read", "params": {"refreshToken": False}})
                    elif message.get("id") == 2:
                        result = message["result"]
                        assert result["account"] is None
                        assert result["requiresOpenaiAuth"] is True
                        received[2] = True
            assert received == {1: True, 2: True}, "read-only handshake did not complete"
        finally:
            selector.close()
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            process.stdin.close()
            process.stdout.close()

        print(f"PASS: Codex {VERSION}; schema routes; initialize; unauthenticated account/read")
        for name in ("LoginAccountParams", "LoginAccountResponse", "AccountLoginCompletedNotification"):
            digest = hashlib.sha256((schema / f"v2/{name}.json").read_bytes()).hexdigest()
            print(f"{name}.json sha256 {digest}")


if __name__ == "__main__":
    main()
