#!/usr/bin/env python3
"""Private, bounded managed SDK calls. Credentials never appear in command text.

The Hub owns durable create/dispatch claims. Unknown results are not retries.
"""
import hashlib
import importlib.metadata
import json
import logging
import re
import shlex
import sys

LABELS = {"semon_version", "semon_deployment", "semon_owner", "semon_workspace",
          "semon_session", "semon_operation", "semon_epoch"}
IDENTIFIER = re.compile(r"[A-Za-z0-9_.-]{1,128}\Z")


def owned(request, info):
    return all(info.metadata.get(k) == v for k, v in request["labels"].items())


def managed(request, sandbox):
    if (not isinstance(request, dict) or request.get("version") != 1
            or not isinstance(request.get("api_key"), str)
            or not 1 <= len(request["api_key"]) <= 4096
            or set(request.get("labels", {})) != LABELS
            or not all(isinstance(v, str) and IDENTIFIER.fullmatch(v)
                       for v in request["labels"].values())):
        return {"error": "invalid_request"}
    method = request.get("method")
    key = request["api_key"]
    if method == "create":
        # One call only. Failure/timeout may have created compute.
        vm = sandbox.create(template="base", timeout=900, metadata=request["labels"],
                            api_key=key, request_timeout=20, retries=0)
        return {"id": vm.sandbox_id}
    identifier = request.get("id")
    if not isinstance(identifier, str) or not IDENTIFIER.fullmatch(identifier):
        return {"error": "invalid_request"}
    info = sandbox.get_info(identifier, api_key=key, request_timeout=10)
    if not owned(request, info):
        return {"error": "ownership_mismatch"}
    if getattr(info.state, "value", info.state) != "running":
        return {"error": "compute_not_running"}
    vm = sandbox.connect(identifier, api_key=key, request_timeout=10, retries=0)
    if method == "bootstrap":
        # Artifact is an operator-reviewed immutable semon-guest build, verified
        # before use. No provider key or user prompt enters the command string.
        url, digest = request.get("artifact_url", ""), request.get("artifact_sha256", "")
        if not url.startswith("https://") or not re.fullmatch(r"[0-9a-f]{64}", digest):
            return {"error": "artifact_not_configured"}
        api_key = request.get("model_key")
        if not isinstance(api_key, str) or not 1 <= len(api_key) <= 4096:
            return {"error": "model_auth_required"}
        # Never repeat bootstrap into an existing state after uncertainty. The
        # native launcher also refuses reused homes and unsupported isolation.
        setup = "\n".join([
            "set -eu", "test ! -e /var/lib/semon/bootstrap-intent",
            "install -d -m 700 /var/lib/semon /workspace/project /opt/semon",
            "touch /var/lib/semon/bootstrap-intent",
            "apt-get update -qq && apt-get install -y -qq bubblewrap ca-certificates curl",
            "cd /opt/semon",
            "curl --fail --silent --show-error --location " + shlex.quote(url) + " -o artifact.tar.gz",
            "echo " + shlex.quote(digest + "  artifact.tar.gz") + " | sha256sum --check --status",
            "tar -xzf artifact.tar.gz --no-same-owner guest semon codex",
            "chmod 700 guest semon",
        ])
        result = vm.commands.run(setup, user="root", timeout=120)
        if result.exit_code != 0:
            return {"error": "bootstrap_failed"}
        # Protected root-owned input file is never mounted in the executor.
        payload = json.dumps({"api_key": api_key, "method": request.get("model_method")}, separators=(",", ":"))
        vm.files.write("/var/lib/semon/launch.json", payload, user="root")
        vm.commands.run("chmod 600 /var/lib/semon/launch.json", user="root", timeout=10)
        vm.commands.run("exec 3< /var/lib/semon/launch.json; rm /var/lib/semon/launch.json; exec /opt/semon/guest <&3", user="root", background=True)
        ready = vm.commands.run("for i in $(seq 1 40); do test ! -S /var/lib/semon/control.sock || exit 0; sleep 0.5; done; exit 1", user="root", timeout=25)
        if ready.exit_code != 0:
            return {"error": "native_launch_unavailable"}
        push_token, push_url = request.get("push_token"), request.get("push_url")
        if not isinstance(push_token, str) or not isinstance(push_url, str) or not push_url.startswith("https://"):
            return {"error": "mirror_enrollment_missing"}
        vm.files.write("/var/lib/semon/push-token", push_token, user="root")
        vm.commands.run("chmod 600 /var/lib/semon/push-token", user="root", timeout=10)
        vm.commands.run("exec /opt/semon/semon push --codex-home /var/lib/semon/session/home --to " + shlex.quote(push_url) + " --token-file /var/lib/semon/push-token --state /var/lib/semon/push-state --watch", user="root", background=True)
        return {"started": True}
    if method == "rpc":
        # Fixed bridge; untrusted text is carried only in a protected file. The
        # agent cannot reach this socket or E2B envd through its confined executor.
        payload = request.get("payload")
        if not isinstance(payload, dict) or payload.get("op") not in ("snapshot", "send", "reconnect", "renew", "end"):
            return {"error": "unsupported_control"}
        if payload.get("op") == "end":
            evidence = vm.commands.run("test ! -f /var/lib/semon/ended.json || cat /var/lib/semon/ended.json", user="root", timeout=10)
            if evidence.exit_code == 0 and evidence.stdout.strip():
                return json.loads(evidence.stdout)
        nonce = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        path = "/var/lib/semon/request-" + nonce + ".json"
        vm.files.write(path, json.dumps(payload), user="root")
        try:
            vm.commands.run("chmod 600 " + path, user="root", timeout=10)
            receive = "while True:\n b=s.recv(65536)\n if not b: break\n out.extend(b)\n if len(out)>2097152: raise ValueError()"
            bridge = ("import socket,sys; s=socket.socket(socket.AF_UNIX); s.settimeout(30); "
                      "s.connect('/var/lib/semon/control.sock'); s.sendall(sys.stdin.buffer.read()); "
                      "s.shutdown(socket.SHUT_WR); out=bytearray(); "
                      "exec(" + repr(receive) + "); sys.stdout.buffer.write(out)")
            result = vm.commands.run("python3 -c " + shlex.quote(bridge) + " < " + path,
                                     user="root", timeout=35)
            if result.exit_code != 0:
                return {"error": "transport_unknown"}
            return json.loads(result.stdout)
        finally:
            vm.commands.run("rm -f " + path, user="root", timeout=10)
    return {"error": "unsupported_method"}


def main():
    logging.disable(logging.CRITICAL)
    result = {"error": "provider_outcome_unknown"}
    try:
        raw = sys.stdin.buffer.read(131073)
        if len(raw) > 131072 or importlib.metadata.version("e2b") != "2.52.0":
            raise ValueError()
        from e2b import Sandbox
        result = managed(json.loads(raw), Sandbox)
    except Exception:
        pass  # Vendor errors/command output may contain credentials or input.
    sys.stdout.write(json.dumps(result, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
