#!/usr/bin/env python3
"""Qualify synthetic custody and offline historical access using the clean pin.

Only private disposable fixtures run a signed loopback historical receiver.
No installed data is scanned or changed.
"""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile

from relay_history_build import PIN, project, verify_source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--legacy-source", type=Path, required=True)
    parser.add_argument("--history-binary", type=Path,
                        help="qualify this retained/downloaded binary instead of the newly built reader")
    args = parser.parse_args()
    source = args.legacy_source.resolve(strict=True)
    try:
        verify_source(source)
    except ValueError as error:
        parser.error(str(error))
    with tempfile.TemporaryDirectory(prefix="semon-relay-custody-qualification-") as scratch:
        scratch = Path(scratch)
        project(source, scratch)
        env = os.environ.copy()
        env.setdefault("CARGO_BUILD_JOBS", "4")
        env["SEMON_RELAY_QUALIFIED_SOURCE"] = PIN
        if args.history_binary:
            env["SEMON_RELAY_HISTORY_BINARY"] = str(args.history_binary.resolve(strict=True))
        manifest = str(scratch / "Cargo.toml")
        subprocess.run(["cargo", "clippy", "--locked", "--manifest-path", manifest,
                        "--bin", "semon-relay-history", "--test", "custody", "--",
                        "--no-deps", "-D", "warnings"], check=True, env=env)
        subprocess.run(["cargo", "test", "--locked", "--manifest-path", manifest,
                        "--test", "custody"], check=True, env=env)
    verify_source(source)
    print(f"Synthetic custody/decryption qualification passed using clean {PIN}.")
    if args.history_binary:
        binary_digest = hashlib.sha256(args.history_binary.read_bytes()).hexdigest()
        print("Qualified standalone reader SHA-256: " + binary_digest)


if __name__ == "__main__":
    main()
