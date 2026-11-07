#!/usr/bin/env python3
"""Refuse session-store namespace or version-stamp writes outside the port.

The storage port (crates/semon-sessions/src/storage/) owns the per-database
namespace binding and version stamp; the SQLite index writer in
events/store.rs writes them through its own connection. Any other source that
names these metadata keys bypasses the namespace-bound capability types.
See docs/design/session-storage-rebuild-plan.md.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCES = ROOT / "crates"
OWNERS = (
    pathlib.Path("crates/semon-sessions/src/storage.rs"),
    pathlib.Path("crates/semon-sessions/src/storage"),
    pathlib.Path("crates/semon-sessions/src/events/store.rs"),
)
KEYS = re.compile(r"'storage_(?:namespace|stamp)'")


def owned(relative: pathlib.Path) -> bool:
    return any(relative == owner or owner in relative.parents for owner in OWNERS)


def main() -> int:
    refused = []
    for path in sorted(SOURCES.rglob("*.rs")):
        relative = path.relative_to(ROOT)
        if owned(relative):
            continue
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if KEYS.search(line):
                refused.append(f"{relative}:{number}")
    if refused:
        print("session-store binding keys outside the storage port:", file=sys.stderr)
        for location in refused:
            print(f"  {location}", file=sys.stderr)
        return 1
    print("native storage boundary: binding keys are owned by the storage port")
    return 0


if __name__ == "__main__":
    sys.exit(main())
