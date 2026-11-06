#!/usr/bin/env python3
"""CI-only native SQL/path inventory and explicit remaining-path guard."""
import argparse
from pathlib import Path
import re
import json

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
    for name in ["catalog.rs", "query.rs"]:
        source = (root / "storage/sqlite" / name).read_text()
        if "EventCache::path" in source or "options.cache" in source:
            raise SystemExit(f"Native reader bypasses configured admission: {name}")
    inventory = json.loads((root / "storage/sqlite/writer-inventory.json").read_text())
    if set(inventory["unqualified_paths"]) != listed or inventory["writer_qualification"] != 0:
        raise SystemExit("Namespace writer qualification must match the unported path inventory")
    # SQLite namespace and writer authority live in the namespace-bound port
    # types, not in schema triggers or connection capability flags. Refuse a
    # reintroduced guard family or retired capability function anywhere in the
    # crate, including tests (tests may only create the legacy names as
    # refusal fixtures through LEGACY_GUARD_FIXTURE).
    retired = re.compile(
        r"CREATE TRIGGER (?:IF NOT EXISTS )?\{?(?:storage_(?:owned|semantic|source|event|catalog)_guard_|storage_event_scope_|storage_private_unported_)"
        r"|\bsemon_(?:event_writer|semantic_writer|source_backfill_writer)\b"
    )
    for path in sorted(root.rglob("*.rs")):
        source = path.read_text()
        for line_number, line in enumerate(source.splitlines(), 1):
            if retired.search(line) and "LEGACY_GUARD_FIXTURE" not in line:
                raise SystemExit(
                    f"Retired SQLite guard or capability flag reintroduced: {path}:{line_number}"
                )
    print("Native storage boundary matches the explicit remaining-path inventory.")
