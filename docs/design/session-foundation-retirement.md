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
native archives, generic non-SSH Push/mirror and passive runtime coordination.
Agent handoff and peer events named “relay” remain session semantics. Hub vault
and credential encryption remains active. Independent VM recovery (#255) is
future scope, not a prerequisite. Preserve writer exclusion and durable uncertain
operation reconciliation without adding speculative recovery infrastructure.

## Collection retirement scope

> **Current scope (2026-11-08).** The [ASI direction](asi-direction.md) retires
> control delivery, managed runtime and SSH. The `semon-control`, `semon-runtime`
> and `semon-ssh` crates, their active CLI/HTTP/MCP/UI entry points, managed E2B
> workflows and SSH qualification scripts are removed in step 2, landed in this
> branch. Multi-machine collection uses non-SSH Push/receive over TLS.

Broad full-text `find` remains paused until the ASI query surface lands. Retirement
never deletes user data, credentials, native history or installed services, and
performs no live operational cleanup.

Local native collection, non-SSH Push/receive, Session Event Index and Catalog,
focused Query reads, native identity/lineage/handoffs, passive questions and
approvals recorded in logs, Archive/Unarchive, enrollment/authorization, export,
and passive runtime observation (process and lock facts) remain supported.
Metadata navigation and selected-text navigation remain available; full-text
search is not currently exposed. See the [ASI direction](asi-direction.md) for
sequencing and Hub follow-ups.

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
Hub build owners, Docker and CI use that binary owner.
Hub's sole Semon pin is its submodule gitlink; verify the exact checked-out pair
with `scripts/verify-source-pair.sh`. Do not add another source pin.

## Model retirement follow-up — #469

Audited main `5f34607070ff2e7cd5ef4f1e41a5ca9c2935f9f8` already uses native
Viewer/Query consumers and rejects the retired Model/transcript/analytics routes.
#385's older route/owner/warming checklist is historical, not a second migration
plan. The follow-up removes its remaining executable test oracle and exclusive
serialization/transport preparation. Shared native derivation and independently
preserved responses remain. On 2026-10-22 the user explicitly chose to drop
compatibility and remove the retired JSON event-cache importer: facts found only
in that file are no longer imported, and any existing cache or interrupted
`.importing` file is left untouched because deletion remains an operator action.
See the [inventory, reference provenance and acceptance reconciliation](model-oracle-retirement.md).
Final integration, consumer/release qualification and the separate transcript
proof repair remain under their existing owners. No merge or deployment is implied.

## Derived-index cleanup

The user's October 23 correction authorizes automatic discard and reindex of
recognized obsolete Semon-owned derived SQLite indexes during normal writable
producer open. The [storage contract](session-source-reader.md#current-semon-storage-contract)
defines recognition, current schema initialization and refusal of future,
foreign or ambiguous stores. Reset is transactional and safe for concurrent
producers and WAL readers; it does not migrate old rows. Focused reads never
reset storage, and physical-corruption recovery remains unchanged.

This narrow cache invalidation does not authorize live operational cleanup.
Native logs, archive objects, credentials, unrelated files and retired JSON
caches are preserved. History without accessible supported native source bytes
cannot be rebuilt, and old archive markers do not become new custody authority.

## Remaining acceptance

- Catalog discovery and deterministic relationship closure are bounded. Publish
  changed sources incrementally, reconcile disappearance/rewrite and distinguish
  current from retained history under generation fences. Unchanged sources reuse
  the existing Event Index without repeated full-history parsing.
- Local/stdio and hosted MCP preserve list_sessions, get_session, read_transcript
  and stalls through the shared native Query. Broad full-text `find` remains paused
  until the ASI query surface lands. Hosted requests admit native sources without
  ViewerCore, warming or workspace-model preparation.
- Focused history reads retrieve only selected sources/ranges and bounded
  relationship context, verify identity/generation/integrity, avoid tenant locks
  during remote I/O and unrelated restoration, coalesce duplicate reads and bound
  cache admission. Unavailable, incomplete and stale evidence fails explicitly.
- Native Home/recent, navigation/tree, timeline/analytics, machine/runtime,
  transcript expansion and embedded consumers retain their behavior. The
  server-rendered no-JavaScript session reader and its legacy URL aliases are retired. Full-workspace Model ownership/routes, transaction/delta/SSE, compat=1
  and warming are retired. Preserve structural and consumer regression evidence.
- Preserve session names, selection/navigation, native tool rendering, collapsed
  Thinking, scroll/focus, responsive/accessibility behavior and current/retained
  history. Control delivery, managed runtime and SSH are retired. Preserve the
  approved visual references and applicable tests.
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

The preceding integration snapshot is historical. Current native ownership and
the #469 follow-up are reconciled above; final adoption still requires
exact-commit validation. A PR's existence or old passing CI is not delivery.
Merge, release publication and deployment remain separate authorized actions.
