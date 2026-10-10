#!/usr/bin/env python3
"""Build the fixed offline Relay history reader, then export declared custody.

Builds may fetch locked Cargo dependencies. The compiled reader uses no network
or installed services; only the explicit custody artifact and new output are read.
Use --build-only --artifact PATH to retain a standalone reader and its receipt.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from relay_history_build import PIN, ROOT, git, project, verify_source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--legacy-source", type=Path, required=True)
    parser.add_argument("--custody", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument("--artifact", type=Path)
    args = parser.parse_args()
    if args.build_only:
        if not args.artifact or args.custody or args.out:
            parser.error("--build-only requires --artifact and excludes --custody/--out")
    elif not args.custody or not args.out or args.artifact:
        parser.error("export requires --custody and --out; --artifact requires --build-only")
    source = args.legacy_source.resolve(strict=True)
    try:
        verify_source(source)
    except ValueError as error:
        parser.error(str(error))
    if args.artifact:
        parent = args.artifact.parent.resolve(strict=True)
        args.artifact = parent / args.artifact.name
        if args.artifact.is_relative_to(source) or args.artifact.is_relative_to(ROOT):
            parser.error("retain binary/receipt outside both source checkouts")
        for path in (args.artifact, args.artifact.with_name(args.artifact.name + ".json")):
            if path.exists() or path.is_symlink():
                parser.error("binary and receipt destinations must be new")
    with tempfile.TemporaryDirectory(prefix="semon-relay-history-build-") as scratch:
        scratch = Path(scratch)
        project(source, scratch)
        env = os.environ.copy()
        env.setdefault("CARGO_BUILD_JOBS", "4")
        manifest = str(scratch / "Cargo.toml")
        subprocess.run(["cargo", "build", "--locked", "--release", "--manifest-path", manifest,
                        "--bin", "semon-relay-history"], check=True, env=env)
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
             "--manifest-path", manifest], env=env))
        binary = Path(metadata["target_directory"]) / "release/semon-relay-history"
        verify_source(source)
        if args.build_only:
            artifact = args.artifact
            parent = artifact.parent
            binary_bytes = binary.read_bytes()
            descriptor = os.open(artifact, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o755)
            with os.fdopen(descriptor, "wb") as output:
                output.write(binary_bytes)
                output.flush()
                os.fsync(output.fileno())
            receipt = {
                "format": "semon.relay-history-build", "version": 1,
                "legacy_source": PIN, "tool_source": git(ROOT, "rev-parse", "HEAD"),
                "tool_source_dirty": bool(git(ROOT, "status", "--porcelain")),
                "binary_sha256": hashlib.sha256(binary_bytes).hexdigest(),
                "qualified_lock_sha256": hashlib.sha256((scratch / "Cargo.lock").read_bytes()).hexdigest(),
                "qualification": "build only; deployment decryption and recovery not verified",
            }
            descriptor = os.open(artifact.with_name(artifact.name + ".json"),
                                 os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(descriptor, "w") as output:
                json.dump(receipt, output, indent=2)
                output.write("\n")
                output.flush()
                os.fsync(output.fileno())
            descriptor = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
            print("Standalone offline reader and build receipt retained; preserve the legacy pin too.")
            return 0
        result = subprocess.run([str(binary), "--custody", str(args.custody), "--out", str(args.out)], env=env)
        verify_source(source)
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
