#!/usr/bin/env python3
"""Classify PR changes for Rust CI and Viewer UI tiers."""

import os
import subprocess
import sys
from pathlib import Path


VIEWER_EXACT = {
    b".github/workflows/ui.yml",
    b"crates/semon-sessions/tests/viewer_server.rs",
    b"ui/package.json",
    b"ui/package-lock.json",
    b"tests/ui/package.json",
    b"tests/ui/package-lock.json",
}


def changed_paths(base: str, head: str, repository: Path | None = None) -> list[bytes]:
    if base and base == head:
        return []
    prefix = ["git"] if repository is None else ["git", "-C", str(repository)]
    merge_base = subprocess.run(
        [*prefix, "merge-base", base, head], check=True, capture_output=True
    ).stdout.strip()
    result = subprocess.run(
        [*prefix, "diff", "--name-only", "--no-renames", "-z", merge_base.decode("ascii"), head, "--"],
        check=True,
        capture_output=True,
    )
    return [path for path in result.stdout.split(b"\0") if path]


def is_docs_only(path: bytes) -> bool:
    # Rust embeds this document and tests its Classes contract against the CSS.
    return path != b"docs/shell.md" and (path.endswith(b".md") or path.startswith(b"docs/"))


def is_viewer_path(path: bytes) -> bool:
    return (
        path in VIEWER_EXACT
        or path.startswith((b"ui/", b"tests/ui/", b".github/scripts/", b"crates/semon-sessions/src/viewer"))
    )


def classify_paths(paths: list[bytes]) -> str:
    if any(is_viewer_path(path) for path in paths):
        return "full"
    if all(is_docs_only(path) or path.startswith(b"crates/semon-asi/") for path in paths):
        return "none"
    return "smoke"


def should_run(event: str, base: str, head: str) -> bool:
    if event != "pull_request":
        return True
    paths = changed_paths(base, head)
    return not paths or any(not is_docs_only(path) for path in paths)


def main() -> None:
    event = os.environ["GITHUB_EVENT_NAME"]
    paths = changed_paths(os.environ.get("BASE_SHA", ""), os.environ.get("HEAD_SHA", "")) if event == "pull_request" else []
    tier = classify_paths(paths) if event == "pull_request" else "full"
    run_expensive = should_run(event, os.environ.get("BASE_SHA", ""), os.environ.get("HEAD_SHA", ""))
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"run_expensive={'true' if run_expensive else 'false'}\n")
        output.write(f"viewer_tier={tier}\n")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, subprocess.CalledProcessError, UnicodeError) as error:
        print(f"CI change classification failed: {error}", file=sys.stderr)
        raise
