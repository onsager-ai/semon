#!/usr/bin/env python3
"""Create disposable current-format mirror enrollment for transport CI only."""
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
enrollment_id = secrets.token_hex(32)
registry = {
    "format": 2,
    "latest": 0,
    "next_sequence": 2,
    "tombstones": [],
    "revision": 1,
    "enrollments": [{
        "enrollment_id": enrollment_id,
        "receiver_id": "ci-receiver",
        "machine": "laptop",
        "sequence": 1,
        "origin_key": [4] * 65,
        "endpoint": "https://receiver.example:443",
        "binding": {"state": "inapplicable"},
        "client_certificate_pins": [],
        "credential_kinds": [{
            "kind": "mirror_push", "client_cert_required": False,
            "generations": {"current": 1, "retiring": None},
            "available": True, "enable_authorization": None,
        }],
        "revoked_at": None,
    }],
    "credentials": [{
        "credential_id": secrets.token_hex(32), "enrollment_id": enrollment_id,
        "kind": "mirror_push", "sha256": list(hashlib.sha256(token.encode()).digest()),
        "generation": 1, "valid_from": 0, "valid_until": 18446744073709551615,
        "revoked_at": None,
    }],
    "pairing_receipts": [],
}
for path, value in [(enrollment_dir / "registry.json", json.dumps(registry)),
                    (enrollment_dir / "registry.lock", ""), (token_path, token)]:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(value)
