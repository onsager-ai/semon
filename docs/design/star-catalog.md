# Primary star-schema Catalog spike

Status: architecture prototype, 2026-11-17. No production wiring. This replaces
semantic publication, rather than optimizing its generation fence. The existing
ASI warehouse names and grains in [asi-store.md](asi-store.md#analytics-model)
remain the starting point. The SQLite Index becomes their primary origin store;
DuckDB and Hub receive metadata projections. Native bodies and FTS stay local.

## Contract

Append native observations, never a chosen owner, parent, copy classification,
relay receiver or turn number. Read a SQLite snapshot to answer the current
question. A later observation can change a query answer without invalidating a
stored fact. Facts identify `(machine, source, incarnation, byte offset, block,
observation kind)`; native IDs remain scoped by harness and source authority.
They also carry an ordinal for deterministic native ordering. Incarnation is
essential: replacing a source is not appending to the same source.

Type 1 dimensions and bridges hold revisable answers separately from facts.
Natural-key maintenance selects latest model, first prompt position and timestamp
extrema. Component-local maintenance selects lineage membership and canonical
and usage owners. These answers are replaced atomically when new evidence arrives;
they are never written back into facts. SCD2 is reserved for machine names and
prices. Neither arbitrary arrival order nor a guessed original copy owner becomes
truth. Cross-owner shared usage remains a set-based read over current membership.

This deliberately revises the downstream ASI contract: `usage_owner`, selected
turn, exclusive totals, classified activity and outcome are view columns. The
old `fact_step` model-invocation grain remains a logical view over finer usage
observations, because streaming blocks can revise the observed usage of one
invocation. `fact_session` and `fact_turn` become aggregate views. Updating an
accumulating snapshot would violate the append-only requirement.

## Facts and parser producers

Every table has the source-position key above; nullable native correlation IDs
are evidence, not globally unique primary keys. Columns below are metadata;
prompt, result and argument bodies are source references, never analytics text.
Current parser locations refer to main fe77ddb1.

| Table | Native grain and columns | Producer today |
| --- | --- | --- |
| `fact_session_observation` | Header/sidecar assertion: native ID, harness, native parent, role, nickname, agent path, repository identity, branch, title reference, observation time | `model.rs:1346` Claude layout; `model.rs:1447` Codex header; `events.rs:6438` Codex; `events.rs:6841` Copilot |
| `fact_record` | Complete native record: source position, record UUID, native message ID, kind, timestamp, body reference | `events.rs:6007`, `events.rs:6438`, `events.rs:6841`; `events.rs:4197` time |
| `fact_step_observation` | Assistant usage block: message ID, UUID, model key, token classes, native reported totals, timestamp | `events.rs:6024` Claude events; `events.rs:6438` Codex token counts |
| `fact_tool_call` | Call occurrence: native call ID, tool key, argument reference, timestamp | `events.rs:6024`, `events.rs:6438`, `events.rs:6841` |
| `fact_tool_result` | Completion occurrence: native call ID, native occurrence scope, result reference, error/exit observation, timestamp | `events.rs:4741` attachment today; `events.rs:5931` connector completion; `events.rs:6943` Copilot completion |
| `fact_relation_observation` | One assertion: continuation, session_id alias, bridge, native parent, marker parent, launch candidate, send or receipt; endpoint IDs, method, correlation ID, name, source refs | `events.rs:6114` links; `model.rs:3310` Claude parents; `model.rs:3892` Codex; `native_marker.rs:19`; `model.rs:4288` relays |
| `fact_runtime_observation` | Captured process/lock/sidecar observation: native ID, process start, status, observation clock, expiry/qualification | `model.rs:1033`, `model.rs:2435`; runtime facts input remains independently qualified |
| `fact_compaction` | One native compaction observation: timestamp, native identity/source ref | `events.rs:6024`, `events.rs:6438` |
| `fact_label` | Existing one label assertion: target, field, enum value, labeller, taxonomy version, confidence, time | Existing ASI durable label producer; preserve its ownership |

`fact_record` is an origin metadata locator, not a raw JSON staging table. The
prototype uses a reduced schema with `fact_relation_observation`,
`fact_step_observation`, `fact_tool_call`, `fact_tool_result`, and `fact_record`.
It loads synthetic native JSON directly, independently of the builder oracle.
It does not prove all parser extraction, custody or runtime integration.

## Conformed dimensions

| Dimension | Kind and recomputation |
| --- | --- |
| `dim_session` | Type 1, natural key `(machine,harness,native ID,source incarnation)`. Re-read only this key's observations: first/last time, latest model, title/prompt source reference, native role/nickname/path and outbound-continuation presence used in deterministic ordering. A title body may be served locally from its reference; it is not exported to analytics. |
| `dim_lineage` | Type 1, source → component ID (minimum physical source key). Traverse only the arriving source's undirected continuation/session-ID edges with a visited set, then replace rows for that component. The executable spike covers actual sources; bridge pseudo-nodes remain a fidelity gap. |
| `bridge_session_owner` | Type 1, source → canonical owner and usage owner. Rank the affected component's sources by the builder's timestamp/continuation/native-ID order, replacing all member rows in the append transaction. Both owner columns currently agree; shared-copy original attribution stays unknown. |
| `bridge_spawn_placement` | Type 1, child → parent source/call position/time. Recompute grouped unique call/name and child/name matches for the appended parent and, when a child arrives, its native parent. This immediate family group is bounded independently of ancestor depth. A second name candidate revokes placement; a late parent fills it. |
| `dim_machine` | SCD2 names, with observed/effective times and an explicit source authority; new name closes the previous effective interval. Identity does not change with name. |
| `dim_harness`, `dim_tool`, `dim_date` | Type 1 lookup values from their own natural key. Date is a deterministic UTC calendar projection. |
| `dim_repo` | Type 1 sanitized repository identity/display name, using current ASI sanitization. Local path remains origin scoped. |
| `dim_model` | Type 1 immutable provider/model identity; no fabricated history. |
| `dim_price` | Existing SCD2 price/version contract and conflict refusal, unchanged. |

Recompute local dimensions from all facts for the appended key, in the ingestion
transaction. Discover the affected lineage component once through indexed edges,
then replace its dimension/bridge rows in that same transaction. The prototype's
`maintain.sql` uses connection-local work tables to share this visited set between
statements. Component merges include both previously separate components. A child
arriving before its parent is repaired when the parent arrives; the test compares
all persisted derived columns and the final read rows against in-order ingestion.
Repeating maintenance produces identical rows. The prototype stores title source
positions and reads bodies from its fixture reader only after SQL chooses that
position. It stores no title text in SQLite.

Catalog key collision qualification remains a set-based read because reserved IDs
can be in unrelated components. Title/model selection, native-parent lookup,
current usage and shared/exclusive sums use joins, grouped sets and window
functions. Copy suppression is a distinct UUID set anti-join; tool result matching
joins a grouped unique-call set. No list/detail view contains a correlated scalar
subquery or recursive lineage closure. Collision prefix escaping is the only
remaining recursive read and its owner/reserved sets are materialized once.

## Eight non-monotonic cases

These retain the C inventory's numbering. SQL is illustrative except for the
executable views under the prototype's test support directory.

1. **Canonical merges and suppression.** Undirected continuation/session_id/bridge
   edges define components, scoped to one machine and Claude harness. A recursive
   CTE with `UNION` (visited set) terminates cycles during append maintenance. Rank actual sources by first
   timestamp, presence of outbound continuation first on a tie, then native ID,
   exactly as `model.rs:1757`. Missing IDs/bridge pseudo-nodes must connect components
   but can never win ownership (not yet covered by this reduced extractor).
   Persist rank 1 in the Type 1 owner bridge during component maintenance. Suppress a target
   record only with explicit source→target continuation and matching UUID, else
   the supported tool/message identity (`model.rs:2251`). Do not suppress bridge
   records merely because they have equal text.

   ```sql
   WITH RECURSIVE reach(start,node) AS (
     SELECT node,node FROM nodes UNION
     SELECT r.start,e.dst FROM reach r JOIN undirected_edges e ON e.src=r.node
   ) SELECT start, first_value(node) OVER
     (PARTITION BY start ORDER BY first_ts, continuation_rank, native_id)
     FROM reach JOIN actual_sources USING(node);
   ```

2. **Collision keys and received owners.** Build a `catalog_key` view preserving
   today's unqualified ID when unique, otherwise harness qualification with
   leading colons until the candidate is outside the reserved native-key set
   (`model.rs:2101`). Resolve received duplicate presentation by configured source
   order (`union.rs:499`) at the union boundary, not by upload order. SQL sketch:
   `row_number() over(partition by presentation_id order by configured_rank,
   source_key)`; retain every source-scoped identity for explicit lookup. The
   prototype covers cross-harness collisions, not the cross-machine union.

3. **Unique parent/spawn placement.** Native parent assertions join the current
   owner bridge. For name-based placement recompute the revisable bridge from
   grouped candidates: `GROUP BY parent,name HAVING count(DISTINCT call_position)=1` joined
   to `GROUP BY parent,name HAVING count(DISTINCT child)=1`. A second candidate
   revokes placement immediately. Claude storage-parent exact call outranks
   global unique-ID fallback. Parent graph cycle handling preserves the current
   builder rule; it must be tested before production replacement.

4. **Parent precedence/launch inference.** Rank marker/native/process assertions
   according to current builder precedence, then native source order:
   `row_number() over(partition by child order by method_priority,ordinal)`.
   Marker parsing still stops at the first decisive complete record or preceding
   agent output, including the currently supported qualified JSON-at-EOF case.
   Prompt/cwd/path/time launch matching needs origin body reads; see below.

5. **Relay endpoints and pairing.** Sends and receipts are separate facts. Exact
   correlation joins select the current earliest receipt under deterministic
   model/source order: `row_number() over(partition by send_position order by
   source_rank,receipt_ordinal)`. Name uniqueness is a grouped query. Unresolved
   endpoints are projected stubs with current stable-ID encoding, never stored
   session dimensions. Later receipts remove a stub from the view. Retain all
   duplicate observations rather than overwriting an endpoint. Body/time pairing
   needs the origin-local algorithm described below.

6. **Reused call IDs/results.** Preserve each occurrence. Join by native scope and
   occurrence; Copilot duplicated call IDs yield no successful attachment:
   `HAVING count(DISTINCT call_position)=1`. Codex connector precedence orders
   completion candidates in a view. Late answers append `fact_tool_result`,
   without changing `fact_tool_call`. No accumulated result/outcome on facts.

7. **Latest state, results and turns.** Latest fields use ranked observations;
   `row_number() over(partition by native_key order by ordinal desc)`. Live status
   also qualifies current process evidence, independently from log progress.
   Parent/child completion overrides remain selected-session query logic. Native
   boundaries form `fact_turn` views with window sums; merged causal turns require
   the read-time renderer below. No persisted last-turn flag or derived end state.

8. **Shared usage and family totals.** Choose the current observation per native
   invocation/message, then detect matching `(message ID,UUID)` across distinct
   current owners. Connected groups sharing any UUID for the same message use the
   existing `claude_usage.rs:26` algorithm, expressed as a recursive CTE. Two UUIDs
   can connect three owners transitively; simple per-UUID counts alone are
   insufficient. Exclusive sums exclude these groups; shared totals remain
   separately reported with unknown original owner. No earliest-arrival owner.
   Recursive parent joins compute family totals from this read snapshot, using a
   visited set for cycles. Usage/cost is never multiplied by joining tool rows.

## Viewer and ASI reads

`/api/sessions` starts a short read transaction; select owner/key view, join local
session dimensions, compute usage and current parents, order by requested list
sort and key, and apply keyset pagination. Counts and page rows share a snapshot.
Runtime evidence is captured once per request with its current qualification.
No ready flag waits for every file to stop appending. A source not yet ingested
is absent/pending under the existing coverage contract, not a claimed empty
complete corpus. SQL availability alone does not imply complete native coverage.

Single-session reads resolve `(scope,source_key,catalog_key)` in the same view,
then query membership and fact positions for that component only. Join result
facts at read time and anti-join copied positions. Merge ordered source positions
and fetch actual bodies through existing authorized Index/archive readers.
A read cursor pins an SQLite snapshot plus source-incarnation/prefix proofs;
verified append is allowed, replacement/custody change is refused. The old
64-source vector limit must disappear from the reader and membership pagination,
not merely from the semantic stage. Full turns can be expensive independently
of list latency; bound transcript pages by source positions.

ASI reads the same `fact_step`/`fact_turn`/`fact_session` logical views and conformed
dimensions. Export only permitted metadata, never title bodies, prompt hashes,
arguments or raw tool output. Labels/classification requiring bodies execute at
the origin and export only their allowed enums and provenance. The existing ASI
activity projection must be retired after parity, not maintained as a second
semantic authority.

## Atomicity and progress

The parser inserts all facts and recomputes affected local dimensions and lineage/
spawn bridges in the same SQLite transaction as Index complete-record advancement
and source proof. There is no global semantic stage, generation fence, dependency
graph or restart-until-quiescent loop. A visited set bounds the traversal by the
finite affected lineage component; grouped spawn maintenance reads the affected
immediate family. One transaction completes against that admitted snapshot even
when other sources keep appending. Thus continuous appends cannot starve this
append by invalidating its generation. Very large components can still increase
write latency; this is a size bound, not a constant-time claim. A
crash before commit exposes neither; a crash after commit exposes both. Unique
source-position keys make replay idempotent; a conflicting payload for an existing
key is an integrity error. A result arriving later is a new position/fact.
The prototype explicitly tests rollback and repeated dimension recomputation.

A source replacement appends a new incarnation and updates its current-source
selector transactionally, retaining native history according to existing custody
and retention rules. Old facts remain attributable; current queries select the
admitted incarnation. Do not erase unavailable/unrebuildable evidence. Sidecars,
process snapshots and remote custody changes have their own admitted observations;
a JSONL-only fact stream cannot represent them completely.

Remove semantic-stage phase cursors, global exact-generation restart fences,
identity/relation publication epochs, full Catalog row recipes and derived
session change journals once all consumers query facts. Keep source integrity,
incarnation/CAS, authorization, native prefix checks and SQLite snapshot isolation.
#699's dependency edges, dirty closure, revision propagation, replacement/retraction
queue and bounded semantic epochs are unnecessary. Its bounded source ingestion
and atomic Index advancement remain useful independently.

## Rules that do not become plain SQL

* Marker/launch prompt+time heuristics: metadata candidate joins can use cwd/time
  indexes, but prompt comparison must remain origin local. Proposal: preserve the
  current matcher as selected-session read-time code over candidate references;
  do not persist its chosen parent or invent a body hash analytics column. Full
  list parent parity needs a measured bounded candidate strategy before rollout.
* Two-pass body/time relay pairing: independent nearest-neighbor joins do not
  preserve the global greedy matching order. Proposal: read candidates in current
  deterministic order and run the existing two-pass matcher for the selected
  component. Expose exact-ID metadata joins directly. No claim that a single
  window function is equivalent. Keep this out of list fields where unused.
* Turn partitioning: merged turns depend on chosen relationships and late events.
  Proposal: native-boundary SQL views for analytics input, with existing causal
  partitioning run over selected transcript pages plus required boundary context.
  Full causal analytics parity is a separate gate; replacing it with native turns
  silently would change the product. No stored turn repair/dependency system.

## Mergeable phases and staging

1. **This spike:** executable reduced SQLite model, independent full-builder
   oracle, explicitly manual scale job; no production behavior. Measure list
   closure cost and identify fidelity gaps before choosing indexes.
2. **Parser facts:** append observations in the Index transaction, local-key
   dimensions and source-incarnation selector. Fixture parity and crash tests;
   still read the old Catalog during this short implementation phase. This is a
   validation step, not a permanent dual writer compatibility contract.
3. **Query Catalog list and identity:** complete all list fields, parent selection,
   runtime qualification and union ownership; compare full corpus fixtures,
   continuous-append availability, query cost and pagination. **Earliest staging
   unblock**, provided single-session reads have a fact-backed position resolver
   and can construct their selected model without requiring old publication.
   A list-only switch that opens an empty/stale detail view is not sufficient.
4. **Selected transcript/relationships:** finish body/time heuristics, results,
   causal turns and proof-backed cursor pagination, remove old semantic stage,
   epochs/fences/recipes. Remove temporary comparison producers with their users.
5. **ASI views/export:** replace activity-generation dependency with the same
   warehouse views, align classifications/costs/labels, remove obsolete projections
   and update asi-store.md. Benchmark broader analytic workloads separately.

The prototype's benchmark uses roughly 1 GB of generated native JSONL and 20,000
sessions, a deep parent chain, continuation components and common task names.
It reports ingestion including dimension maintenance, aggregate maintenance time
and mean microseconds per append, then fresh-connection and repeated list/detail
queries with explicit secondary indexes enabled. SQLite automatic indexes remain
at their default enabled setting for joins against materialized grouped sets;
this differs from the initial 154 s list / 36 s detail run, which disabled them.
The preexisting ingestion/query timing line formats are retained. Fresh connection is a
SQLite page-cache cold measurement; it is **not** a claim of cold OS disk cache.
The fixture corpus lives in a separate file and never becomes a SQLite body
column. The test asserts the corpus size and row count and prints actual query
plans/timings; the workflow uploads the full log. No required PR gate runs it.
