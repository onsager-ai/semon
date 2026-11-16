#!/usr/bin/env python3
"""Create disposable current-format push pairing for transport CI only."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import sys

root, token_path = map(Path, sys.argv[1:])
root.mkdir(mode=0o700)
enrollment_dir = root / "enrollment"
enrollment_dir.mkdir(mode=0o700)
token = secrets.token_hex(32)
machine_id = "machine-6754a9c1e83f27b4"
registry = {
    "format": 9, "revision": 1, "latest": 0,
    "machines": [{"id": machine_id, "directory": machine_id, "owner_name": None}],
    "machine_audit": [],
    "read_tokens": {"tokens": [], "audit": [], "public_url": None},
    "simple_pair": {"pending": None, "burns": [], "pause_until": 0, "audit": [], "tokens": [{
        "credential_id": secrets.token_hex(32), "enrollment_id": machine_id,
        "kind": "push", "sha256": list(hashlib.sha256(token.encode()).digest()),
        "generation": 1, "valid_from": 0, "valid_until": 18446744073709551615,
        "revoked_at": None,
    }]},
}
for path, value in [(enrollment_dir / "registry.json", json.dumps(registry)),
                    (enrollment_dir / "registry.lock", ""), (token_path, token)]:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(value)
