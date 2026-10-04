#!/usr/bin/env python3
"""Coordinator-only, read-only inventory worker for the pinned official SDK.

One bounded JSON request over private stdin; one sanitized response on stdout.
The embedding must enforce a process deadline, recheck authority and protect IPC.
Never invoke with a credential in argv or capture requests in logs/trace records.
"""
import importlib.metadata
import json
import logging
import re
import sys
import time

SDK_VERSION = "2.52.0"
KEYS = ("semon_version", "semon_deployment", "semon_owner", "semon_workspace",
        "semon_session", "semon_operation", "semon_epoch")
SCOPE_KEYS = ("semon_deployment", "semon_session", "semon_operation")
IDENTIFIER = re.compile(r"[A-Za-z0-9_.-]{1,128}\Z")


def identifier(value):
    return isinstance(value, str) and value not in (".", "..") and bool(IDENTIFIER.fullmatch(value))


def inventory(request, sandbox, query, clock=time.monotonic):
    """Consume all SDK pages; partial or failed listing cannot report absence."""
    if (not isinstance(request, dict) or set(request) != {"version", "scope", "api_key"}
            or type(request["version"]) is not int or request["version"] != 1
            or not isinstance(request["scope"], dict)
            or set(request["scope"]) != set(SCOPE_KEYS)
            or not all(identifier(value) for value in request["scope"].values())
            or not isinstance(request["api_key"], str)
            or not 1 <= len(request["api_key"]) <= 4096):
        return {"version": 1, "status": "unavailable"}
    deadline = clock() + 20
    resources = []
    try:
        paginator = sandbox.list(query=query(metadata=request["scope"]), limit=100,
                                 api_key=request["api_key"], request_timeout=5)
        pages = 0
        while paginator.has_next:
            remaining = deadline - clock()
            if pages >= 32 or remaining <= 0:
                return {"version": 1, "status": "incomplete"}
            page = paginator.next_items(request_timeout=min(5, remaining))
            pages += 1
            for item in page:
                if len(resources) >= 1000 or not identifier(item.sandbox_id):
                    return {"version": 1, "status": "incomplete"}
                metadata = item.metadata
                if (not isinstance(metadata, dict)
                        or any(metadata.get(key) != value for key, value in request["scope"].items())):
                    return {"version": 1, "status": "incomplete"}
                labels = {key: metadata[key] for key in KEYS if key in metadata}
                if not all(identifier(value) for value in labels.values()):
                    return {"version": 1, "status": "incomplete"}
                # Do not relay vendor metadata, URLs, account identity or errors.
                state = getattr(item.state, "value", item.state)
                resources.append({"id": item.sandbox_id, "labels": labels,
                                  "state": state if state in ("running", "paused") else "unknown"})
        if clock() >= deadline:
            return {"version": 1, "status": "incomplete"}
        return {"version": 1, "status": "complete", "resources": resources}
    except Exception:
        # Even a failure after several pages is not a complete empty inventory.
        return {"version": 1, "status": "unavailable"}


def validate_credential(request, sandbox, query, authentication_error=(), clock=time.monotonic):
    """Check list API access without provisioning or returning account resources.

    This is basic connection validation, not template/create/lifecycle entitlement.
    """
    if (not isinstance(request, dict)
            or set(request) != {"version", "method", "scope", "api_key"}
            or type(request["version"]) is not int or request["version"] != 1
            or request["method"] != "validate_credential"
            or not isinstance(request["scope"], dict)
            or set(request["scope"]) != {"semon_deployment", "semon_owner"}
            or not all(identifier(value) for value in request["scope"].values())
            or not isinstance(request["api_key"], str)
            or not 1 <= len(request["api_key"]) <= 4096):
        return {"version": 1, "status": "unavailable"}
    deadline = clock() + 5
    try:
        paginator = sandbox.list(query=query(metadata=request["scope"]), limit=1,
                                 api_key=request["api_key"], request_timeout=5)
        # Exactly one authenticated request. Listing pagination/state is not
        # interpreted as absence; resource and account data never join the result.
        paginator.next_items(request_timeout=5)
        if clock() >= deadline:
            return {"version": 1, "status": "unavailable"}
        return {"version": 1, "status": "credential_valid"}
    except authentication_error:
        return {"version": 1, "status": "credential_rejected"}
    except Exception as error:
        if getattr(error, "status_code", None) == 403:
            return {"version": 1, "status": "credential_rejected"}
        return {"version": 1, "status": "unavailable"}


def main():
    result = {"version": 1, "status": "unavailable"}
    # SDK/vendor errors may contain request values; this worker emits only its
    # fixed protocol. Preserve inherited proxy/CA settings and TLS verification.
    logging.disable(logging.CRITICAL)
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536 or importlib.metadata.version("e2b") != SDK_VERSION:
            raise ValueError("unsupported request or SDK")
        request = json.loads(raw)
        from e2b import Sandbox, SandboxQuery
        from e2b.exceptions import AuthenticationException
        if isinstance(request, dict) and request.get("method") == "validate_credential":
            result = validate_credential(request, Sandbox, SandboxQuery, AuthenticationException)
        else:
            result = inventory(request, Sandbox, SandboxQuery)
    except Exception:
        pass
    sys.stdout.write(json.dumps(result, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
