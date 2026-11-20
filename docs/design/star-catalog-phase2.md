# Star Catalog phase 2 implementation plan

Proposed for owner review before production edits. Phase 2 writes the origin
warehouse alongside Index advancement. Viewer and ASI cutover, Hub export and
semantic-stage retirement remain in phases 3 and 4. This plan targets the primary
SQLite Index. The paused PostgreSQL projection has a separate transaction in
`storage/postgres/index.rs` (`index_source` → `write_index`). [Production plan](star-catalog.md#production-plan)

## Write transactions

Code paths below are relative to `crates/semon-sessions/src`.

| Admission | Transaction and planned change |
| --- | --- |
| Local append, first scan, bounded checkpoint or replacement | `events.rs` `scan_file` parses outside the write lock, captures the consumed prefix and checks source, header, marker and sidecar evidence. `EventCache::commit` → `events/store.rs` `commit_file` → `SqliteStore::write_one_dirty` takes `BEGIN IMMEDIATE` and compares the expected ledger. Write the warehouse through `write_index_changes_measured`, after generation and ledger checks and before commit. |
| Cold batch | `events.rs` `EventCache::flush_cold_writes` → `events/store.rs` `commit_initial_files` takes one `BEGIN IMMEDIATE` for up to 64 new complete files. It checks absent ledgers and exact stats, then calls `write_index_changes_measured` for each source. Use the same warehouse writer and roll back the whole batch on failure. |
| Authorized archive replay | `events.rs` `scan_archive` uses `archive.rs` `Archive::with_native` to pin verified bytes and checks native identity, consumed boundary and prefix/tail and metadata proofs. It reaches `EventCache::commit` and the same `write_one_dirty` transaction. Exact committed ledger reuse produces no duplicate facts. |
| Source withdrawal or verified Codex move | In `events/store.rs` `SqliteStore::remove`, retire the current selector and affected memberships with the ledger check, including the branch that keeps archived Index rows. In `events/store/relocation.rs` `reuse`, move the carrier mapping with the verified ledger and proofs, keeping the incarnation and its fact keys. |

Each record admission commits the `files` ledger (complete offset, prefix and tail),
proofs, native rows and existing local projections together with facts, current
source selection, dimensions and the maintenance cursor. The Index transaction
also records the existing Catalog invalidation when required. A failed proof,
lost ledger comparison or failed write leaves the prior snapshot visible.
A crash before commit leaves both offset and warehouse at the prior boundary.
After commit, reopening sees both at the new boundary, even if the producer never
acknowledged it. Identical replay is a no-op. A different payload at the same
fact key aborts the write. Incomplete trailing records never advance the offset.
The separate scan checkpoint and Raw mutation journal can still lead or lag their
consumers under the existing progress rules.

Keep `events/store.rs` `publish_catalog_with_writer` as the existing Catalog
publication during development. It still publishes its own snapshots and chain
witnesses later. Archive admission in `events.rs` `archive_consumed` →
`archive.rs` `admit_native` uses an archive lock outside the SQLite transaction.
Local fact admission cannot imply archival custody. `origin_projection.rs`
`read_cohort_generations` and selected readers keep their current publication,
chain and byte checks. `selected_read.rs` supplies request limits, and selected
repair or provider reads supply no new warehouse admission path. [Source reader](session-source-reader.md) · [Retained projections](retained-native-projections.md)

## Schema and recovery

Use the [prototype SQL](../../crates/semon-sessions/src/model/tests/star_catalog/schema.sql)
as the starting point. Persist these tables in the Event Index:

- `fact_session_observation`, `fact_record`, `fact_step_observation`,
  `fact_step_billing`, `fact_tool_call`, `fact_tool_result` and
  `fact_relation_observation`.
- `fact_relay_send`, `fact_relay_result`, `fact_relay_receipt` and
  `fact_subagent_event` for qualified event inputs. Split the prototype's mutable
  attached reply columns into immutable result observations at the reply's
  position, with call scope, flags and correlation. Recover the attached event
  shape in a read join. Preserve unknown or ambiguous results and late binding.
- `fact_claude_call`, `fact_agent_observation`, `fact_parent_observation`,
  `fact_process_observation`, `fact_environment_observation` and
  `fact_ancestor_observation`. The last three share a qualified capture identity
  and can provide retained parent evidence after the process exits.
- `dim_session`, `lineage_group`, `lineage_member`, `lineage_node`,
  `bridge_spawn_placement` and `maintenance_cursor`, with indexes that support
  the bounded updates below. `affected_spawn_key` is transaction scratch space.

Replace the prototype integer source ID with an Index-scoped key bound to the
origin namespace, harness, logical source and incarnation. Index the current
incarnation separately from historical facts and retain a ledger-bound source
reference for reads. Record keys include byte offset, block and observation kind
plus a deterministic discriminator where a block emits multiple observations.
Keep ordinal for ordering. A header, sidecar or process capture uses its own
qualified observation identity. A changed sidecar creates a new observation or
records qualified absence without minting a new log incarnation. An unqualified
sidecar supplies no new authority. Keep native correlation IDs nullable and
scoped. Preserve origin-only locators and allowlisted metadata. Bodies and hashes
of body content are excluded from warehouse and export rows.

Reuse the durable label producer for `fact_label` and the current native usage
and price contracts. Store compaction observations from native signals at source position
(`fact_compaction` in the target contract). Keep Copilot cumulative shutdown
observations at shutdown position and model, with latest-snapshot semantics.
Keep conformed harness, tool, repo, model, machine and price meanings and derive
UTC dates at read time. Phase 2 needs metadata and native billing inputs for
parity. Public dimension/export cutover remains in phase 4.

Promote the prototype's read-time SQL for `dim_lineage`, `bridge_session_owner`,
`membership`, `catalog_keys`, `current_usage`, `shared_messages`, `usage_totals`,
`session_list`, `spawn_placement`, `session_records`, `tool_answers`,
`parent_selection`, `relay_endpoints` and the family/shared-usage and pricing
views in `family.sql` and `shared_family.sql`. Use `detail.sql` as a bound
selected query. Make these available to phase 2 parity tests on the current
schema. `fact_step`, `fact_turn` and `fact_session` remain logical views of
invocations and aggregates. Bind effective prices at read time and use
`model/relay_match.rs` and `model/turns.rs` for selected pairing and causal
state. The synthetic loader, its title/body map, `index_advance` (a stand-in for
`files`), the injected `qualified_turn_runtime` table and `full_recompute.sql`
are test-only. Persist neither chosen parent/pairing/turn nor live runtime state.

Bump `events/store.rs` `SCHEMA_VERSION` from 49 to 50 on this base. Create the
required tables and indexes with the fresh schema and qualify their shape in
`qualify_contract` alongside the parser/cache version. Bump `CACHE_VERSION` if
the persisted parser contract changes. Producers and focused readers refuse an
unsupported index before mutation. Update `unsupported_message` and the operator
docs with recovery instructions that preserve the refused DB and its sidecars.

The host runs recovery itself (owner decision, 2026-11-20): when it finds an
unsupported Index it starts the rebuild with the verification below, serves no
history from the refused Index, and reports rebuild progress or the recovery
error. There is no separate operator command. Require an independently
authorized inventory of the expected sources and generations, including any archive-only or retained history
in scope. Directory discovery alone cannot prove completeness. Qualify each
original through existing local, standalone archive or host custody rules.
Replay the authorized originals through the same parser and Index transactions
into a separate current Index. Keep it out of service until every expected
source and consumed boundary is accounted for. Missing, inaccessible,
ambiguous or unverifiable originals, including unknown coverage, return a
recovery error. Preserve the old DB, WAL and SHM, originals, archives and any
non-rebuildable evidence. There is no old-schema reader, copy of old derived
rows, import layer or automatic reset. The host switches to the new Index only
after this validation passes. [Current storage contract](session-source-reader.md#current-semon-storage-contract)

## Observation producers

Emit metadata from the existing parser decisions at each admitted position.
Carry the new observations to the writer with the parsed batch. The folded
`FileIndex` and its dirty-row journal can revise an earlier call when a result
arrives. Use the result's original byte/block location and native correlation
when emitting facts, before the fold can hide the occurrence. Use admitted
complete-record positions, including gaps needed for causal turns.

| Input | Production outputs to use |
| --- | --- |
| Claude project and subagent logs | `events.rs` `parse` → `claude` / `claude_events` supplies record identity, UUID/message usage, native billing, calls/results, compaction signals and session-ID, continuation and bridge assertions. Send, receipt and agent direction/finality, denial, pin and result flags feed the relay and subagent facts. Reuse the native parser's marker stop/precedence and `native_handoff.rs` `Index::observe` for launch candidates. `model.rs` `scan` qualifies the project/subagent layout and storage parent. `native_sidecar.rs` `ClaudeSidecar::observe` supplies independently verified call binding and allowlisted metadata for session and agent observations. |
| Codex rollouts and archived rollouts | `native_header.rs` `CodexHeader::from_record` / `native_id` supplies identity and native session/parent metadata, preserving role and guardian-review constraints. `events.rs` `codex` and `events/usage.rs` `CodexNativeUsage::observe` supply records, turn and compaction signals, token snapshots and native billing observations with current deduplication. Response items, tool/connector calls and completions, and agent send/receipt events supply tool, relay and subagent facts. Keep structured exits and unknown status distinct. Preserve source positions when model-level deduplication skips a record. |
| Copilot CLI workspaces | `events.rs` `copilot` supplies the `session.start` identity/model, resume and model changes, record IDs/parents, assistant tool requests, execution completions, subagent lifecycle crumbs and compaction. Feed session, record, tool and compaction observations. Pair the final `session.shutdown` usage and its native observation through `copilot::snapshot`; do not sum successive shutdowns or treat subagent crumbs as qualified cross-session parent links. |

`inputs.rs` `InputRoot` recognizes these three native log roots. Use the same
source allowlist and parser for live and archive inputs. Machine observations
come from `model.rs` `pid_files` and `facts.rs` `MachineFacts` (`proc_start`,
`codex_lock_pids`, `ancestors`, `run`). Capture only allowlisted process/parent
metadata. Bind it to the origin, native identity, source incarnation, process
start and observation clock. Use the existing qualified local or recorded
machine input. Fence ancestry and environment acquisition against PID reuse.
Add a short SQLite write in `events/store.rs` at runtime capture, after source
admission. Check the current Index namespace and expected source incarnation
inside the transaction, then commit the process, environment and ancestor rows
together. A changed binding leaves the capture uncommitted. This advances no
native byte receipt.
Apply liveness and expiry from qualified current runtime at read time. Stored
parent evidence alone grants neither current runtime authority nor body access.

The synthetic loader supplies fixture IDs, parsed JSON and mostly prequalified
runtime. Production must acquire and verify the byte and custody witnesses and
perform actual process-start qualification. Existing provisional EOF header,
marker and handoff evidence keeps its native validation and precedence, with
its own physical witness. It never advances the complete-record receipt or
supplies an unchecked body reference.

## Append cost and source replacement

Bind the old `maintenance_cursor` inside the source transaction and aggregate
only the new suffix through source/position indexes. Update timestamp extrema,
latest title/model positions, first prompt and continuation presence from that
suffix. Retain marker-stop state across appends. Reconsider owner rank only if
first time or continuation rank changes. Probe indexed endpoints on new edges
or physical-source admission, including absent IDs and bridge nodes. Merge by
size and relabel only the smaller side. Compare the two prior owners after a
union. Recompute spawn placement only for changed parent/name keys and revoke
ambiguous matches. Read parent precedence, result pairing, shared usage and
family closure on demand.

Replacement changes the current incarnation in the same transaction as ledger
replacement. Old facts remain attributable. Recompute current membership only
within the old affected component and any components the new evidence reaches,
using active edges. A component can split when an edge loses its last current
assertion. Apply the same rule to withdrawal and changes to sidecar-derived
keys. Preserve retained evidence and original-custody rules. Component repair on
these paths is outside the ordinary append budget. A verified move keeps the
existing incarnation and edges.

Use the [phase 1 benchmark](star-catalog.md#production-plan) as the baseline.
Budget about 0.8 ms mean per append for dimension maintenance on the same 1 GB /
20k-session workload (794 µs measured). Measure parsing and proof acquisition,
fact writes and maintenance separately on the production path. Keep the formal
A/B/C gate of last-window p50 ≤ 3× first-window p50 with the same 10,000 appends
per case. Report p50/p99 and ingest total. Keep the existing fresh/warm read and
query-plan gates. The spike timings cover the fixture workload and provide a
review baseline for production costs.

## Comparison and acceptance

Run the existing Builder and Catalog reader against the same authorized fixture
source set and qualified runtime capture as the warehouse. Let the existing
publication finish, or compare using its exact committed source witnesses, so a
lagging Catalog publication cannot look like a mismatch. Compare canonical IDs,
parents, relays, usage, turns and record positions. Use a fresh current Index in
both arms. Existing production consumers remain on their current reader until
their phase switches them. Compare in tests with no warehouse read fallback,
old-schema branch or new compatibility writer. Retire the old publication path
with its final consumer in phase 4. Keep independent fixture expectations and
full-recompute checks that still exercise the current contract.

| Acceptance | Production-path test |
| --- | --- |
| Supported native inputs | Drive cold, bounded and incremental admission with sanitized Claude, Codex and Copilot fixtures. Assert emitted metadata and position keys and compare selected parity after each commit. Include malformed records, partial EOF, usage deduplication/compaction, noisy or changing headers and sidecars, late results and Copilot's repeated shutdown. Run pinned Codex fixture conformance in CI. |
| Idempotent replay | Reopen and re-admit the same prefix, duplicate a parse attempt and race two writers on one prior ledger. Verify identical facts, cursor and dimensions, one winning advancement, and rollback on a differing payload at a used key. |
| Rollback and restart | Inject failure after fact/dimension writes and before commit on the single-source and cold-batch paths. Check readers and reopen against the original ledger, proofs, cursor, selectors and rows. Simulate loss of acknowledgement after commit and assert all of them advance together. |
| Replacement and incarnation | Rewrite or truncate at the same path, reuse native IDs and call IDs, and replace an assertion that splits a lineage. Verify current facts come from the new incarnation, old facts keep their witnesses, stale bodies refuse, and unrelated components do zero maintenance. Cover withdrawal with and without retained Index rows and proof-qualified Codex relocation followed by append. |
| Late endpoint or parent | Admit continuation/bridge endpoints, spawn candidates, native markers and relay receipts before and after their counterparts. Assert component union, owner change, parent precedence/cycle handling and duplicate-spawn revocation. Exercise changed or expired process-start evidence, PID reuse and retained parent observations with no live authority. |
| Custody and unsupported state | Exercise missing or changed sources, stale/ambiguous chain or provider bindings and unauthorized evidence. Assert withheld body access and no advancement from refused input. Refuse schema 49, future and foreign stores without mutation. Rebuild separately from verified local and archive originals, then repeat with missing or incomplete inventory and assert a recovery error with no new active Index. |
| Bounded maintenance | Compare randomized splits and interleavings against independent full recomputation after each append. Run the scale cases and assert query plans and unrelated-component work. Use CI for builds, tests and benchmarks. |

## Suggested implementation PRs

1. Prepare parser observation batches and fixture parity, including sidecar and
   runtime capture tests. Keep the new output in test use until its writer lands.
2. Land schema, write transactions, component maintenance and replacement and
   relocation handling together with unsupported-store refusal and recovery.
   Gate on the production-path tests and benchmark before read cutover.

## Owner decisions

Decided by the owner on 2026-11-20:

- **Scope.** Primary SQLite only. The paused PostgreSQL projection is not part
  of phase 2.
- **Recovery interface.** Host-managed rebuild, with the verification in
  [Schema and recovery](#schema-and-recovery). The new Index stays separate
  until it is complete and validated.
- **Durable runtime evidence.** Record qualified process/environment/ancestry
  observations when they change, with a capture clock; qualify live state on
  reads.
- **Replacement cost.** Recompute only the affected components, synchronously
  in the admission transaction, until measurements justify a separate design.
- **Rebuild inventory and Viewer state.** Decided by the Manager on 2026-11-20
  under the owner's delegation. The host builds the expected inventory from
  sources it can authorize today: the configured native roots (local homes, and
  each received machine's directory on a receiver), the standalone archive
  manifest, and receiver custody records. Nothing is read from the refused
  Index. A source that one of those records references but that is missing or
  fails verification is a recovery error. A source no record references is
  not history Semon can vouch for, the same as on a fresh install. While the
  rebuild runs, Catalog and transcript reads answer 503 `history_rebuilding`
  with `Retry-After` and progress (sources done of total), and the Viewer shows
  "Rebuilding history (N of M sources)". Nothing is served from the refused
  Index.
- **Copilot.** Deferred. Phase 2 covers Claude and Codex only. The Copilot
  producer row above is kept as the plan for a later PR, which must land before
  phase 3 switches Catalog reads, so Copilot sessions do not drop out.
