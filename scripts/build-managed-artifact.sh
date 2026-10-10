#!/bin/sh
# Build an independently reviewable artifact; never publish or deploy it.
set -eu
out=${1:?Usage: build-managed-artifact.sh OUTPUT_DIRECTORY}
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
cargo build --locked --release -p semon-control --bin semon-guest -p semon-cli --bin semon
cp scripts/managed-codex/package.json scripts/managed-codex/package-lock.json "$stage/"
npm --prefix "$stage" ci --ignore-scripts --no-audit --no-fund
cp -a "$stage/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl" "$stage/codex"
cp "$stage/node_modules/@openai/codex/package.json" "$stage/codex/codex-package.json"
cp target/release/semon-guest "$stage/guest"
cp target/release/semon "$stage/semon"
printf '%s\n' 'Built from:' > "$stage/provenance.txt"
git rev-parse HEAD >> "$stage/provenance.txt"
tar -C "$stage" -czf "$out/managed-guest.tar.gz" guest semon codex provenance.txt
sha256sum "$out/managed-guest.tar.gz" > "$out/managed-guest.sha256"
