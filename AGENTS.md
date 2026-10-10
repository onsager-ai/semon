# Semon

Harness-native session collection, synchronization, browsing, queries and runtime
integration, forming the foundation for Session Intelligence. Experimental trace
capture and encrypted Relay are being retired in stages; see
docs/design/session-foundation-retirement.md. Preserve their historical-data
access and existing retention/security boundaries during migration.
The former OTLP/Collector/ClickHouse pipeline is intentionally gone.

## Repository invariants

- Transferable trace semantics and raw forensic records are separate regions.
  Ordinary trace reads cannot expose raw bytes or their carrier labels.
- Preserve canonical identity and occurrence/provenance semantics. An identity
  change must update the relevant fixtures and documented contract together.
- Preserve documented forensic permissions, exposure and retention. Full raw
  records may contain credentials. Deletion remains an explicit operator action;
  do not introduce automatic pruning or maximal deletion defaults.
- Preserve native session/control parser behavior and the shared, sanitized
  harness fixtures. Experimental trace collectors are retired; their historical
  repair/capture contracts live at the pinned revision in
  docs/trace-capture-retirement.md.
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
- AI-assisted UI/UX: docs/design/ai-design-workflow.md and ui-design skill.
  User-level Stitch/pen.dev tooling is optional; keep vendor skills and credentials
  outside the checkout and preserve the existing visual/implementation contract.
- Relay/protocol changes: docs/mirror-protocol.md and
  docs/encrypted-remote-sessions.md.

Pass an exact ref or isolated checkout path when delegating.

For native capability mapping, use harness-operations; Codex-specific reference
routing is in .agents/adapters/codex.md. These adapters grant no extra scope.

<!-- agent-config:begin -->
## Shared agent conventions (generated)

- **authority:** Use the user's task and established session scope to determine authority. Requests to implement or fix work authorize necessary edits, validation and reviewable branch/PR preparation unless the task limits them. Reuse authorization already given; do not request it again for the same action and scope. Ask only for an unresolved decision or an action outside that scope, explaining the concrete reason. Shared procedures grant no additional authority; PR preparation does not authorize merging, production release, unrelated infrastructure changes or new paid resources.
- **checks:** Run checks appropriate to the affected behavior and the repository's declared gates. Report commands, actual results, blocked prerequisites and remaining scope. Test evidence is not an additional runtime permission or deployment flag: do not invent a checklist, report file or approval requirement to block the testing requested by the user. Preserve actual implementation, configuration, custody and authority checks. A quick check does not replace a declared merge gate.
- **discovery:** Before editing a module, locate applicable ancestor/module instruction files and load only relevant references. Shared workflows and their dependencies are checked in under .agents/skills; Claude discovery copies are generated under .claude/skills.
- **ownership:** Edit repo-owned contracts and local skills at their canonical paths. Shared skills, Claude projections, this managed section and synchronization tooling are generated: change the upstream source or manifest selection and regenerate; do not hand-edit generated copies.
- **workflow-policy:** Repository policy owns contribution process, scope and gates; shared methods do not impose an SDD spec requirement where the repo has none. Native tool names in shared workflows illustrate operations: use equivalent available connected tools, and report unavailable capabilities. No global skill install or personal MCP setup is required for discovery.
- **human-decisions:** When a concrete decision remains for a human, use the current harness's supported structured question tool, following its native instructions, tool contract and mode restrictions. Resolve tool names and mechanics through the matching harness-operations reference where available. Do not leave the decision only in a plain-text question, final response, or "Human decides" checklist. State the decision, relevant context, options and tradeoffs in the tool call; wait for an explicit answer before dependent work and reconcile it into the spec or decision record. Continue independent authorized work and do not re-ask settled decisions. If no permitted question tool is available, state that limitation and the unresolved decision, keep dependent work blocked, and use the repository's established human handoff channel. Silence, elapsed time and a recommended option are not approval.
<!-- agent-config:end -->
