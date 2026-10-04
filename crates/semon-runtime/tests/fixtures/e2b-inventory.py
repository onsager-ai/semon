"""Offline private-process fixture. Contains only synthetic credentials."""
import json
import os
from pathlib import Path
import sys
import time

root = Path(__file__).parent
(root / "pid").write_text(str(os.getpid()))
mode = (root / "mode").read_text()
if mode == "hang":
    time.sleep(60)
request = json.load(sys.stdin)
assert request["api_key"] == "synthetic-provider-only-key"
assert "synthetic-provider-only-key" not in " ".join(sys.argv)
for name in ("SEMON_TEST_COORDINATOR_SECRET", "E2B_API_KEY", "E2B_ACCESS_TOKEN",
             "OPENAI_API_KEY", "CODEX_ACCESS_TOKEN", "E2B_API_URL", "E2B_DEBUG"):
    assert name not in os.environ
if "SEMON_TEST_NETWORK_GUARD" in os.environ:
    assert os.environ["SEMON_TEST_NETWORK_GUARD"] == "keep"
(root / "request-checked").touch()
if mode == "oversize":
    sys.stdout.write("x" * (3 * 1024 * 1024))
    time.sleep(60)
if mode == "malformed":
    sys.stderr.write("synthetic-vendor-secret")
    print("synthetic-vendor-secret")
    sys.exit(0)
labels = request["scope"] | {"semon_version": "1", "semon_owner": "owner-1",
                             "semon_workspace": "workspace-1", "semon_epoch": "1"}
resource = {"id": "sandbox-1", "labels": labels, "state": "paused"}
response = {"version": 1, "status": "complete", "resources": [resource]}
if mode == "foreign":
    labels["semon_owner"] = "foreign-owner"
if mode == "wrong_scope":
    labels["semon_session"] = "foreign-session"
if mode == "vendor_metadata":
    labels["private"] = "synthetic-vendor-secret"
if mode == "bad_id":
    resource["id"] = "../sandbox"
if mode == "wrong_state":
    resource["state"] = "deleted"
if mode == "wrong_version":
    response["version"] = 2
if mode == "no_resources":
    response.pop("resources")
if mode == "mixed":
    response["status"] = "incomplete"
if mode in ("incomplete", "unavailable"):
    response = {"version": 1, "status": mode}
print(json.dumps(response))
if mode == "nonzero":
    sys.exit(1)
