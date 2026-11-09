#!/usr/bin/env python3
"""Classify whether a pull request needs the expensive Rust and Viewer suites."""

import os
import subprocess
import sys
from pathlib import Path


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


def should_run(event: str, base: str, head: str) -> bool:
    if event != "pull_request":
        return True
    paths = changed_paths(base, head)
    return not paths or any(
        not (path.endswith(b".md") or path.startswith(b"docs/")) for path in paths
    )


def main() -> None:
    run_expensive = should_run(
        os.environ["GITHUB_EVENT_NAME"],
        os.environ.get("BASE_SHA", ""),
        os.environ.get("HEAD_SHA", ""),
    )
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"run_expensive={'true' if run_expensive else 'false'}\n")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, subprocess.CalledProcessError, UnicodeError) as error:
        print(f"CI change classification failed: {error}", file=sys.stderr)
        raise
