# Semon

Local-first, carrier-neutral agent memory, capture, relay and session tooling.
The former OTLP/Collector/ClickHouse pipeline is intentionally gone.

## Repository invariants

- Transferable trace semantics and raw forensic records are separate regions.
  Ordinary trace reads cannot expose raw bytes or their carrier labels.
- Preserve canonical identity and occurrence/provenance semantics. An identity
  change must update the relevant fixtures and documented contract together.
- Preserve documented forensic permissions, exposure and retention. Full raw
  records may contain credentials. Deletion remains an explicit operator action;
  do not introduce automatic pruning or maximal deletion defaults.
- Preserve golden capture/adapter behavior for Claude and Codex inputs.
- Vendored tiny_http remains governed by vendor/tiny_http/SEMON-PATCH.md.
- Viewer UI sources live in ui/src/; generated bundles embedded by Rust are
  produced by ui/build.mjs. Edit sources and regenerate; do not hand-edit bundles.
  Preserve the shared chrome/host ownership and security boundaries in ui/README.md.

## Checks and completion

Run the affected checks and report their actual outcomes:

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- --no-deps -D warnings
cargo test --locked
```

Installer changes also require `sh -n scripts/install-user-timer.sh`.
Viewer changes require applicable fixture/browser suites and reviewed pixel
baselines. Do not loosen existing thresholds or remove required checks.
Use Node 22+ and the foundation lane's locked tooling: `npm --prefix ui ci
--no-audit --no-fund`, then `npm --prefix ui run typecheck`, `npm --prefix ui test`,
`npm --prefix ui run check:bundle` and `npm --prefix ui run sizes`.
The Rust and UI workflow files supply exact CI setup and aggregate semantics.

## Conditional reading and workflows

Before editing a module, locate applicable ancestor/module instruction files.

- Storage/identity: docs/design/carrier-neutral-trace-storage.md,
  docs/design/trace-identity-and-occurrences.md.
- Forensic access/retention: docs/design/forensic-retention-and-exposure.md.
- Harness changes: the affected adapter's fixtures and harness-compatibility skill.
- Viewer changes: ui/README.md, tests/ui/README.md and viewer-verification skill.
- Relay/protocol changes: docs/mirror-protocol.md and
  docs/encrypted-remote-sessions.md.

Read only the references relevant to the task. Pass an exact ref or isolated
checkout path when delegating. Report skipped/blocked checks and remaining scope.
