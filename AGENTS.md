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

Core Rust checks:

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

- Storage/identity: docs/design/carrier-neutral-trace-storage.md,
  docs/design/trace-identity-and-occurrences.md.
- Forensic access/retention: docs/design/forensic-retention-and-exposure.md.
- Harness changes: the affected adapter's fixtures and harness-compatibility skill.
- Viewer changes: ui/README.md, docs/design/viewer-ownership.md,
  docs/design/design-contract.md,
  tests/ui/README.md and viewer-verification skill. Viewer refactors must follow
  the ownership contract and complete its refactoring review checklist.
- Relay/protocol changes: docs/mirror-protocol.md and
  docs/encrypted-remote-sessions.md.

Pass an exact ref or isolated checkout path when delegating.

For native capability mapping, use harness-operations; Codex-specific reference
routing is in .agents/adapters/codex.md. These adapters grant no extra scope.

<!-- agent-config:begin -->
## Shared agent conventions (generated)

- **authority:** Opening, updating or merging a pull request requires authority from the task or declared repository policy. Shared procedures grant no authority themselves; opening or updating authority does not authorize merging.
- **checks:** Run checks appropriate to the affected behavior. Report commands, actual results, blocked prerequisites and remaining scope. A quick check does not replace a declared merge gate.
- **discovery:** Before editing a module, locate applicable ancestor/module instruction files and load only relevant references. Shared workflows and their dependencies are checked in under .agents/skills; Claude discovery copies are generated under .claude/skills.
- **ownership:** Edit repo-owned contracts and local skills at their canonical paths. Shared skills, Claude projections, this managed section and synchronization tooling are generated: change the upstream source or manifest selection and regenerate; do not hand-edit generated copies.
- **workflow-policy:** Repository policy owns contribution process, scope and gates; shared methods do not impose an SDD spec requirement where the repo has none. Native tool names in shared workflows illustrate operations: use equivalent available connected tools, and report unavailable capabilities. No global skill install or personal MCP setup is required for discovery.
- **human-decisions:** When a concrete decision remains for a human, use the current harness's supported structured question tool, following its native instructions, tool contract and mode restrictions. Resolve tool names and mechanics through the matching harness-operations reference where available. Do not leave the decision only in a plain-text question, final response, or "Human decides" checklist. State the decision, relevant context, options and tradeoffs in the tool call; wait for an explicit answer before dependent work and reconcile it into the spec or decision record. Continue independent authorized work and do not re-ask settled decisions. If no permitted question tool is available, state that limitation and the unresolved decision, keep dependent work blocked, and use the repository's established human handoff channel. Silence, elapsed time and a recommended option are not approval.
<!-- agent-config:end -->
