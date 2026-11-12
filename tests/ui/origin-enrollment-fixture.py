#!/usr/bin/env python3
"""Disposable transport-CI custody. Coupled to origin_custody.rs, never an import API."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

base, receiver, token_path, endpoint, sample = sys.argv[1:]
base, receiver, token_path, sample = map(Path, (base, receiver, token_path, sample))
base.mkdir(mode=0o700, parents=True, exist_ok=False)
record = json.loads((receiver / 'enrollment/registry.json').read_text())['enrollments'][0]
name = hashlib.sha256(endpoint.encode()).hexdigest()
directory = base / name
directory.mkdir(mode=0o700)
def write(name, data):
    fd = os.open(directory / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as output:
        output.write(data)
def encoded(data):
    return base64.urlsafe_b64encode(data).decode().rstrip('=')
def packed(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()
# Test fixtures contain only admitted native files; enumerate them without source bodies.
sources = sorted(str(path) for root in ('claude', 'codex', 'copilot') for path in (sample / root).rglob('*') if path.is_file())
consent = packed(dict(receiver_id=record['receiver_id'], enrollment_id=record['enrollment_id'], machine='laptop', sources=sources))
secret = token_path.read_bytes()
scope = dict(receiver_id=record['receiver_id'], machine='laptop', binding={'state': 'inapplicable'}, kinds=[['mirror_push', False]], client_certificate_pins=[], valid_until=18446744073709551615)
config = dict(version=1, endpoint=endpoint, receiver_id=record['receiver_id'], enrollment_id=record['enrollment_id'], machine='laptop', server_roots='public_webpki', scope=scope, credential_file='mirror-push.secret', private_key_file='origin-key.pk8', consent_file='source-consent.json', consent_sha256=hashlib.sha256(consent).hexdigest(), credential_sha256=hashlib.sha256(secret).hexdigest(), client_certificate_sha256=None)
key = subprocess.check_output(['openssl', 'genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256', '-outform', 'DER'], stderr=subprocess.DEVNULL)
# genpkey's DER EC output is SEC1; explicitly convert to PKCS#8 for ring.
key = subprocess.check_output(['openssl', 'pkcs8', '-topk8', '-nocrypt', '-inform', 'DER', '-outform', 'DER'], input=key)
write('origin-key.pk8', key)
public = subprocess.check_output(['openssl', 'pkey', '-inform', 'DER', '-in', str(directory / 'origin-key.pk8'), '-pubout', '-outform', 'DER'])[-65:]
signature = subprocess.check_output(['openssl', 'dgst', '-sha256', '-sign', str(directory / 'origin-key.pk8'), '-keyform', 'DER'], input=packed(config))
# P-256 ASN.1 signature is a short SEQUENCE containing two INTEGERs.
assert signature[0] == 0x30 and signature[2] == 2
size = signature[3]
r = signature[4:4 + size]
position = 4 + size
assert signature[position] == 2
s = signature[position + 2:position + 2 + signature[position + 1]]
fixed = int.from_bytes(r).to_bytes(32) + int.from_bytes(s).to_bytes(32)
write('mirror-push.secret', secret)
write('source-consent.json', consent)
write('enrollment.json', packed(dict(configuration=config, public_key=encoded(public), signature=encoded(fixed))))
