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

## Temporary collection scope

The October 15, 2026 recovery candidate from Source `c402e481dbe23bcf03841e47b5f7eccd14c0e426` prioritizes collection, synchronization, monitoring and readable history (Source #380/#437 and Hub #267). Source #421's later unfinished framework remains preserved separately; this recovery does not adopt it. Two-way harness control, managed runtime delivery, broad cross-session full-text retrieval and all SSH capabilities are paused, rather than retired or replaced.

The active Source CLI, local HTTP API and shared collection reader offer no session creation/resume, send, approval/question answer, steering, interrupt or reconnect journey. `semon control`, `serve_with_control`, fresh managed guest and private SSH controller binaries refuse before input, listener, credential or controller initialization. Authenticated stale HTTP writes receive HTTP 503 with `capability_unavailable`; unauthenticated requests retain HTTP 403. The native control status endpoint returns no live authority without preparing a selected source. Default private E2B stdio workers refuse create/bootstrap/dispatch, mutable RPC and qualification delivery before SDK admission; inspect/inspect_bootstrap and existing RPC snapshot reconciliation remain available. The legacy guest launch entry refuses before executing its retained launcher. Hub owns the matching authorized product/API/MCP/provider boundaries.

`find` is absent from CLI/MCP advertisement. Public Query (ordinary, selected and Catalog), CLI and MCP stale calls receive `capability_unavailable`, including malformed search arguments, before source discovery, body reads or provider admission. The existing metadata `q` index remains navigation by name/key/repository and recorded fields, not a full-text index. Deterministic paging, direct lookup, source/relationship indexes and generation-verified transcript/tool reads remain supported. Find data/tables are not deleted and no replacement search service/index is introduced. In-session navigation over already selected rendered text remains a reader function.

Public SSH discovery, connection checks, all mirror/native bootstrap variants, controller install/observe/stop and execution transport refuse with `capability_unavailable` before validation, private credential files, sockets or SSH process admission. Execution capability flags are all false. Pure host-key/target validation and receipt/identity reconstruction/application remain available for accepted history; they grant no transport. There is no active Source SSH CLI/MCP/UI route. The fixture installer and SSH qualification scripts also refuse before reading private input, credentials or creating a fixture. The embedded Python transport bodies remain private retained implementation, not public product entry points. SSH onboarding, connection testing, remote installation/bootstrap/update, tunnels and SSH-mediated collection/execution are outside the active configuration. Ordinary local collection and non-SSH Push/receive retain their existing admission and custody checks.

Collection startup needs no model/compute/SSH credentials or SSH connectivity. Passive questions/approvals, tool outcomes, native identity, lineage/handoffs, native Archive/Unarchive, collection setup/enrollment/revocation, authorization, export and explicit data administration remain in scope. Existing credentials, operations and uncertain-result reconciliation remain intact. No installed process is terminated and no unknown write is replayed.

Retained optional control/runtime engines and search matchers keep their independent semantic/security fixtures. Private test-only Query helpers qualify the existing search oracle; they are unavailable to product callers. The `ssh-controller-qualification` example retains the controller fixture implementation, while the default binary's refusal is separately tested. Real-SSH OpenSSH/native-controller jobs and fixture commands are explicitly deferred under the October 15 SSH scope decision; their workflow definitions remain in Git history, not the active workflow; deferred jobs are not passing qualification. Offline transport error/cancellation/output limits, pure receipt semantics, supervisor isolation/lifetime and non-SSH checkpoint custody tests remain active. Reactivating real-SSH transport qualification requires a future scope decision and adaptation of the preserved oracles to an isolated fixture owner; no product bypass is provided. The paused native local product-control workflow is retired from the active tree; the probe scripts and historical receipts remain, and do not qualify the collection candidate. Candidate Rust, UI, browser and exact-pair evidence must be recorded separately from trusted-main, published-image, staging, PG and SSH release qualification. Existing approved visual references, security checks and thresholds remain unchanged.

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
  and stalls through the shared native Query. Broad find is temporarily unavailable. Hosted requests admit native
  sources without ViewerCore, warming or workspace-model preparation.
- Focused history reads retrieve only selected sources/ranges and bounded
  relationship context, verify identity/generation/integrity, avoid tenant locks
  during remote I/O and unrelated restoration, coalesce duplicate reads and bound
  cache admission. Unavailable, incomplete and stale evidence fails explicitly.
- Native Home/recent, navigation/tree, timeline/analytics, machine/runtime,
  transcript expansion and embedded consumers retain their behavior. The
  server-rendered no-JavaScript session reader and its legacy URL aliases are retired. Full-workspace Model ownership/routes, transaction/delta/SSE, compat=1
  and warming are retired. Preserve structural and consumer regression evidence.
- Preserve session names, selection/navigation, the paused control boundary, tool
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

The preceding integration snapshot is historical. Current native ownership and
the #469 follow-up are reconciled above; final adoption still requires
exact-commit validation. A PR's existence or old passing CI is not delivery.
Merge, release publication and deployment remain separate authorized actions.
