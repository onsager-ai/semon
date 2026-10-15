# Native Session Foundation

Tracking: Semon #380, #381–#387 and Hub #122–#124, #226–#227.

## First-release product boundary

Semon is pre-release. Encrypted Relay and Canonical Trace Store are abandoned
experiments. Remove their product implementation, exclusive dependencies, capture
and service templates, CLI commands, tests, fixtures and active documentation.
Historical ciphertext, Store schemas, raw carrier records and deployment
inventory are not compatibility or migration gates. Standalone exporters and
legacy decrypt readers have no supported product purpose and are removed.

Code retirement does not authorize deleting user files, native history, databases,
logs or keys; stopping installed services; resetting staging; or changing deployed
infrastructure. Operational cleanup requires separate authority.

Retain normal machine identity, native sessions, Session Event Index, verified
native archives, generic Push/mirror, SSH onboarding and runtime coordination.
Agent handoff and peer events named “relay” remain session semantics. Hub vault
and credential encryption remains active. Independent VM recovery (#255) is
future scope, not a prerequisite. Preserve writer exclusion and durable uncertain
operation reconciliation without adding speculative recovery infrastructure.

## Ownership

`native sources → Event Index and Catalog → focused Query/Transcript/Entry → Viewer/CLI/MCP`

Semon owns reusable native parsing, identity, lineage, generation-bound evidence,
indexed byte proofs, headers, sidecars, parent/launch markers, incremental source
observation and retained archive generations. Hub owns tenant/workspace
authorization, forced PostgreSQL RLS, credential generations, provider adaptation,
cancellation, scheduling and actual worker/resource lifetime. Request credentials
and readers never enter shared caches or persistent projections.

The product binary belongs to `semon-cli`. Its normal/build dependency graph and
the workspace exclude the retired Store, Relay, capture and exporter packages.
Hub build owners, Docker, SSH, managed artifacts and CI use that binary owner.
Hub's sole Semon pin is its submodule gitlink; verify the exact checked-out pair
with `scripts/verify-source-pair.sh`. Do not add another source pin.

## Remaining acceptance

- Catalog discovery and deterministic relationship closure are bounded. Publish
  changed sources incrementally, reconcile disappearance/rewrite and distinguish
  current from retained history under generation fences. Unchanged sources reuse
  the existing Event Index without repeated full-history parsing.
- Local/stdio and hosted MCP preserve list_sessions, get_session, read_transcript,
  find and stalls through the shared native Query. Hosted requests admit native
  sources without ViewerCore, warming or workspace-model preparation.
- Focused history reads retrieve only selected sources/ranges and bounded
  relationship context, verify identity/generation/integrity, avoid tenant locks
  during remote I/O and unrelated restoration, coalesce duplicate reads and bound
  cache admission. Unavailable, incomplete and stale evidence fails explicitly.
- Home/recent, navigation/tree, timeline/analytics, machine/runtime, transcript
  expansion, embedded routes and native/no-JavaScript consumers migrate before
  deleting full-workspace model ownership, /api/model, model transaction/delta/SSE,
  compat=1, startup/push warming, global hydration and redundant caches.
- Preserve session names, selection/navigation, composer disabled states, tool
  rendering, collapsed Thinking, scroll/focus, responsive/accessibility behavior
  and current/retained history. Preserve the approved visual references and tests.
- Measure baseline versus candidate on small/large unrelated histories, multiple
  machines, retained archives and concurrent refresh/read/cancellation/deletion.
  Report actual source/range reads, archive GETs/bytes, model builds, latency,
  memory/cache occupancy and capacity contention. No unmeasured speedup or
  permanent viewer_busy fix is implied.
- Run repository-owned Rust, feature, restricted-role PostgreSQL, Node/TypeScript,
  browser/visual/native, CLI/MCP, architecture, bundle, exact-pair and production
  build/image checks. Record passed/failed/pending/skipped/unavailable separately.
  Older stack or local receipts do not qualify final integration CI/images.

## Integration evidence

The consolidated integration starts at Semon main
`263a02da278716ab2309b8e34a463419e5c781e5`, reusing Source #388–#420 at
`c28e6bba86c46ff47d88bfa770bba3986108a7cf`. Hub starts at main
`e4a6b6f86085910cfe34d31eb94457696e56808f`, reusing #251/#257–#260 at
`fb5444dfd6e206b3057c7bf9bb46fb51de9a3642`; this preserves main's #212 sync
progress and source custody. Unique native implementations and assertions are
retained. Migration-only #392/#396/#400/#401 are intentionally discarded under
the product boundary above. No shared stack branch is rewritten.

The integration remains unmerged. Catalog bounded discovery, selected hosted
Query archive access and remaining Viewer consumer migration require completion
and exact-commit validation. A PR's existence or old passing CI is not delivery.
Merge, release publication and deployment remain separate authorized actions.
