#!/usr/bin/env python3
"""Classify PR changes for the Rust CI gate and the Viewer UI tier."""

import os
import subprocess
import sys
from pathlib import Path

# Inputs that only the Viewer browser suites exercise in full.
VIEWER_PREFIXES = (
    b"ui/",
    b"tests/ui/",
    b".github/scripts/",
    b".github/workflows/ui.yml",
    b"crates/semon-sessions/src/viewer",
    b"crates/semon-sessions/tests/viewer_server.rs",
)
# The Viewer binary does not link this crate; a classifier test guards that.
VIEWER_UNLINKED_PREFIXES = (b"crates/semon-asi/",)


def changed_paths(base: str, head: str, repository: Path | None = None) -> list[bytes]:
    prefix = ["git"] if repository is None else ["git", "-C", str(repository)]
    merge_base = subprocess.run(
        [*prefix, "merge-base", base, head], check=True, capture_output=True
    ).stdout.strip()
    result = subprocess.run(
        [
            *prefix,
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            merge_base.decode("ascii"),
            head,
            "--",
        ],
        check=True,
        capture_output=True,
    )
    return [path for path in result.stdout.split(b"\0") if path]


def is_docs(path: bytes) -> bool:
    # Rust embeds docs/shell.md and tests its Classes contract against the CSS.
    return path != b"docs/shell.md" and (path.endswith(b".md") or path.startswith(b"docs/"))


def run_expensive(paths: list[bytes]) -> bool:
    return not paths or not all(is_docs(path) for path in paths)


def viewer_tier(paths: list[bytes]) -> str:
    if not paths or any(path.startswith(VIEWER_PREFIXES) for path in paths):
        return "full"
    if all(is_docs(path) or path.startswith(VIEWER_UNLINKED_PREFIXES) for path in paths):
        return "none"
    return "smoke"


def classify(event: str, base: str, head: str) -> tuple[bool, str]:
    if event != "pull_request":
        return True, "full"
    paths = changed_paths(base, head)
    return run_expensive(paths), viewer_tier(paths)


def should_run(event: str, base: str, head: str) -> bool:
    return classify(event, base, head)[0]


def main() -> None:
    expensive, tier = classify(
        os.environ["GITHUB_EVENT_NAME"],
        os.environ.get("BASE_SHA", ""),
        os.environ.get("HEAD_SHA", ""),
    )
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"run_expensive={'true' if expensive else 'false'}\n")
        output.write(f"viewer_tier={tier}\n")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, subprocess.CalledProcessError, UnicodeError) as error:
        print(f"CI change classification failed: {error}", file=sys.stderr)
        raise
