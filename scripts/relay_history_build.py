"""Build only historical tooling against the fixed, clean legacy source."""
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

PIN = "0648a99f977971fb177a5dda7a445c4941d64b57"
ROOT = Path(__file__).resolve().parents[1]


def git(source, *args):
    return subprocess.check_output(["git", "-C", str(source), *args], text=True).strip()


def verify_source(source):
    if git(source, "rev-parse", "HEAD") != PIN or git(source, "status", "--porcelain"):
        raise ValueError(f"legacy source must be clean at {PIN}")


def project(source, scratch):
    verify_source(source)
    def quote(path):
        return json.dumps(str(path))

    (scratch / "Cargo.toml").write_text(f'''[package]
name = "semon-relay-custody-qualification"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
semon-relay = {{ path = {quote(source / "crates/semon-relay")} }}
semon-relay-export = {{ path = {quote(ROOT / "crates/semon-relay-export")} }}
age = "=0.12.1"
hex = "0.4.3"
libc = "0.2.189"
serde_json = {{ version = "1.0.150", features = ["preserve_order", "raw_value"] }}
sha2 = "0.10.9"

[patch.crates-io]
tiny_http = {{ path = {quote(source / "vendor/tiny_http")} }}

[[bin]]
name = "pinned-relay"
path = {quote(source / "crates/semon-relay/src/main.rs")}

[[bin]]
name = "custody-exporter"
path = {quote(ROOT / "crates/semon-relay-export/src/main.rs")}

[[bin]]
name = "semon-relay-history"
path = {quote(ROOT / "scripts/relay-history.rs")}

[[test]]
name = "custody"
path = {quote(ROOT / "tests/retirement/relay-custody.rs")}
''')
    shutil.copyfile(source / "Cargo.lock", scratch / "Cargo.lock")
    subprocess.run(["cargo", "metadata", "--format-version", "1",
                    "--manifest-path", str(scratch / "Cargo.toml")],
                   check=True, stdout=subprocess.DEVNULL)
    legacy_packages = tomllib.loads((source / "Cargo.lock").read_text())["package"]
    qualified_packages = tomllib.loads((scratch / "Cargo.lock").read_text())["package"]

    def registries(packages):
        return {(p["name"], p["version"], p.get("checksum"), p.get("source"))
                for p in packages if p.get("source", "").startswith("registry+")}
    if not registries(qualified_packages) <= registries(legacy_packages):
        raise SystemExit("qualification changed historical registry pins")
