#!/usr/bin/env python3
"""CI-only native SQL/path inventory and explicit remaining-path guard."""
import argparse
from pathlib import Path
import re

parser = argparse.ArgumentParser()
parser.add_argument("--discover", action="store_true")
args = parser.parse_args()
root = Path("crates/semon-sessions/src")
document = Path("docs/design/session-storage-contract.md").read_text()
listed = set(re.findall(r"\| `([^`]+\.rs)` \| direct \|", document))
found = set()
for path in sorted(root.rglob("*.rs")):
    if path.stat().st_size > 10 * 1024 * 1024:
        raise SystemExit(f"Refuse oversized source: {path}")
    source = path.read_text()
    # Includes test-only paths: they can otherwise hide new direct driver opens.
    if re.search(r"\brusqlite\b|\bEventCache::path\b|\.with_extension\(\"sqlite3\"\)", source):
        relative = path.relative_to(root).as_posix()
        if relative == "storage/sqlite.rs" or relative.startswith("storage/sqlite/"):
            continue
        found.add(relative)
        print(f"| `{relative}` | direct | Remaining SQL/path coupling (including tests); see ownership below. |")
unexpected = found - listed
stale = listed - found
if not args.discover and (unexpected or stale):
    raise SystemExit(f"Storage inventory mismatch; unlisted={sorted(unexpected)}, stale={sorted(stale)}")
if not args.discover:
    print("Native storage boundary matches the explicit remaining-path inventory.")
