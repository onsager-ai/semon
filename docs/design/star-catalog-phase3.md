# Star Catalog phase 3 implementation plan

Switch the primary SQLite Catalog and Viewer to the committed facts written by
[phase 2](star-catalog-phase2.md), including Copilot ([#724](https://github.com/onsager-ai/semon/pull/724)).
The list, navigation, selected transcript and evidence links switch in one
production PR. A source can keep appending while the Viewer serves its last
committed prefix. This implements [Viewer and ASI reads](star-catalog.md#viewer-and-asi-reads)
and item 3 of the [production plan](star-catalog.md#production-plan). ASI and MCP
switch in phase 4. The paused PostgreSQL projection is outside this SQLite cutover.

## Current read paths

| Surface | Current path and dependency |
| --- | --- |
| Viewer HTTP and navigation | `ViewerCore::respond_with_deadline_mode`, `catalog`, `native_session_redirect` and `session_page` in [`union.rs`](../../crates/semon-sessions/src/union.rs) route requests and select the configured source. `wait_for_catalog_read` calls `MachineView::wait_until_initial_catalog` or tests `page_ready`, then records producer interest. [`viewer.rs`](../../crates/semon-sessions/src/viewer.rs) waits for the first readable publication through `wait_for_initial_catalog`. [`catalog.rs`](../../crates/semon-sessions/src/catalog.rs) `sqlite_page_ready` checks `catalog_version` and `catalog_generation`. |
| Catalog lists and identity | `catalog.rs` `read_page` already opens a SQLite read transaction. `read_page_snapshot` and `rows` use published Catalog rows, a publication generation, and (for `q`) a search projection tied to that generation. `catalog_selection_read`, reached by `session_catalog_identity` and `catalog_selection_in_snapshot`, implements identity as an exact `sid` page. Exact pages also call `causal_trace::selected_details` and `transcript_range::observation`. `session_source_proof` maps paths through the published row and its source vector. |
| Viewer transcript and evidence | [`transcript_range.rs`](../../crates/semon-sessions/src/transcript_range.rs) `read`, `session_entry_field_with_gate`, and the turn/activity anchor routes qualify a Catalog selection and its published projection. [`native_resolution.rs`](../../crates/semon-sessions/src/native_resolution.rs), [`overview.rs`](../../crates/semon-sessions/src/overview.rs) `session_home_details_with_gate` and [`causal_trace.rs`](../../crates/semon-sessions/src/causal_trace.rs) are part of the same navigation audit. Trace the host entry points for locate, resolve, identity, home, observe, trace, anchors, transcript, entry expansion and annotations through `ViewerCore::catalog`, including any calls outside that dispatcher. |
| Query and MCP | [`query/catalog.rs`](../../crates/semon-sessions/src/query/catalog.rs) `sqlite_read` requires a complete Catalog, matching `slot_projection_catalog_generation` and no invalidations. `sqlite_recheck` fences the observation. [`query/transcript.rs`](../../crates/semon-sessions/src/query/transcript.rs) `read_inner` and `virtual_read` use selected Catalog/slot recipes and recheck their publication or observation. Keep the phase 4 callers on their existing path until their cutover. Viewer-facing calls into shared code must use the warehouse selection in phase 3. |

In [#715](https://github.com/onsager-ai/semon/issues/715), pushes arrive about every
three seconds and a running session repeatedly returns 409 `stale_projection`
with `fence_reason: local_source_revision`, `local_revision_reason: size_grew`
and `stage: stat_grew`. `transcript_range.rs`'s `Local::read_range` compares the
live carrier stat with the selected ledger. Its exact-revision branch rejects
ordinary growth. `stale` returns a resynchronize response and the retry in
[`catalogViewer.tsx`](../../ui/src/app/catalogViewer.tsx) can encounter the same
race on the next attempt. Removing the wait for a published row alone leaves
this loop in place. Phase 3 qualifies a committed prefix for the body read as
well as selecting metadata from committed facts.

## List, identity and relationships

- Open a short read transaction on the current, source-qualified Index. Read
  the page, counts and coverage together. Use the admitted source selectors,
  ledger and inventory to distinguish committed, pending and unavailable data.
  A source awaiting admission contributes no rows and is explicitly pending
  under the existing coverage contract. Preserve current-format refusal and the
  host-managed rebuild response. Keep observation demand asynchronous so the
  producer keeps advancing without the HTTP request waiting for publication.
- In [`warehouse.sql`](../../crates/semon-sessions/src/events/store/warehouse.sql),
  `bridge_session_owner`, `catalog_keys` and `dim_session` supply identity;
  `session_list` shows how to join titles, models, `parent_selection` and
  `usage_totals`. `current_usage`, `shared_messages` and `current_copilot_usage`
  select native observations, including the latest whole Copilot shutdown.
  Treat the shadow views as starting points. The `session_list` status literals
  are placeholders, and its global materialized CTEs and the correlated Copilot
  latest-shutdown expression need plan checks on the actual production queries.
  Preserve shared/exclusive accounting and pricing for the fields the existing
  UI presents. Use `family_edges`, `family_members`, `family_usage` and
  `family_shared_usage` for selected family fields as needed, without
  multiplying usage through tool or relay joins.
- Keep canonical collision-key reservation, native aliases and current versus
  retained-history qualification. For exact identity, start with the indexed
  native-ID probes in [`warehouse_detail.sql`](../../crates/semon-sessions/src/events/store/warehouse_detail.sql).
  Resolve `(scope,source_key,catalog_key)` before joining the selected component.
  Use `parent_selection` and `relay_endpoints` for metadata and the existing
  selected-source matchers where body/time qualification is required. Compare
  list parent fields too. For prompt-dependent parents, measure indexed cwd/time
  candidate selection and run prompt comparison at the origin; keep the bounded
  parent parity requirement in the list gate.
- The owner chose warehouse-owned retained identity on 2026-11-21. Record a
  qualified original component and its complete recipe/source generation in an
  indexed binding for `(retained_history, source_key, catalog_key)`. Withdrawal
  or replacement freezes that binding. Reusing the key leaves the original
  binding intact and lists show the current owner. Exact History reads return
  `retained_identity_unverified` when the binding is missing or its witness
  fails qualification. Phase 3 does not read `session_history_*`.
- Qualification starts with the new schema. The host rebuild replays authorized
  originals without importing old Index rows or a custody manifest. Pre-cutover
  retained keys without a verified warehouse binding get the same explicit
  refusal. The old Index stays in place under Infra custody.
- Preserve list filters, including metadata search. Audit [`catalog_search.rs`](../../crates/semon-sessions/src/catalog_search.rs)
  for reads of published metadata and generation checks. Feed required search
  fields from qualified Index/warehouse data and preserve coverage, bounded
  candidate work and continuation frontiers without a semantic-ready check.
  Keep ordinary lists free of transcript or native-log scans. Resolve title
  positions through the existing authorized metadata/field path when needed.
- Capture runtime evidence once per source per request. Use the current
  [`RuntimeObservations`](../../crates/semon-sessions/src/native_evidence/runtime.rs)
  and `catalog.rs` `relative_status` rules alongside phase 2's
  `warehouse::runtime_binding`/`write_runtime` observations. Durable observations
  can establish historical context. Live state requires the existing process
  start, expiry, authority and source qualification, including received-machine
  facts and selected parent/child overrides. PID reuse or stale observations
  cannot make a row live. Runtime churn must not restart unrelated history reads.

## Pagination and committed transcript prefixes

- Keep the focused list order `last DESC, catalog_key ASC` and strict keyset
  continuation, including ties and bounded search's inclusive frontier where
  applicable. [`catalog_union.rs`](../../crates/semon-sessions/src/catalog_union.rs)
  `read_union` merges in `last DESC, catalog_key ASC, source_key ASC` order by
  opening one authorized source database at a time. Keep its tie-boundary rule
  (`source.key > cursor.after.source` includes an equal local tuple), search
  frontier and coverage aggregation. Machine keys come from configured authority;
  duplicate native IDs and matching display labels do not collapse across
  machines. A union composes independently committed source snapshots.
- Bind list continuations to scope, workspace and source set, filters, order and
  the committed view. Use the ordered view's committed revision evidence in
  place of semantic publication stamps. Keep explicit restart when changes can
  move rows across the keyset boundary. Each request uses one SQLite snapshot
  per source for rows and relationships. Transcript continuations use the
  [stateless prefix cursor](#transcript-cursor-decision).
- Resolve selected component members through `lineage_member` in keyset pages.
  Use an indexed component-local form of `session_records` suppression and
  `tool_answers` joins. Merge ordered source positions with a stable tie-break
  and bound pages by source positions and bytes. Late answers, copied Claude
  positions, relay endpoints and the context needed by the existing causal turn
  partitioner must all come from the page's SQLite snapshot and respect the
  committed-prefix bounds. Use the same native turn/entry addresses for anchor
  lookup, highlighted evidence, scroll and Back.
- Remove the lineage's 64-member vector from the phase 3 read path end to end.
  Today `catalog.rs` checks it for source proofs and relative status,
  `native_resolution.rs` checks it for resolution, `transcript_range.rs`
  `with_source_reader` and `query_source.rs` `Sources::capture_focused_snapshot`
  reject oversize inputs, and `query/catalog.rs` has its own phase 4 guard.
  Inspect history and selected relationship helpers for further vector limits.
  Page members from indexed rows and acquire proof state only for sources and
  boundary context touched by the page. Keep byte/block budgets. The separate
  `catalog_union.rs` limit of 64 configured machine inputs is a workspace fan-in
  bound and stays in place.
- Open a fresh SQLite read snapshot for each transcript page. Verify the
  selected member-set witness and touched sources' incarnations and committed
  prefixes from the cursor. Limit positions and evidence lookups to the
  qualified prefixes. When a page first touches another member, resolve its
  prefix within the continuation's committed view and apply the keyset boundary
  before returning entries. Keep prefix state bounded by the sources a page
  touches, including any required boundary context.
  Reuse the authorized Index chunk proofs, native source readers and
  archive/provider readers. See the request pin and byte budgets in the
  [source reader contract](session-source-reader.md#published-proof-retention-and-request-snapshots).
  Its `LocalOrPinnedProvider` path can verify blocks against an older ledger
  but still rejects a later stat change within the request. Qualify verified
  growth across block reads and the final recheck in the same reader. Verify
  covering blocks and requalify identity and custody before exposing bytes.
  Return an explicit restart when membership, incarnation, custody or prefix
  integrity changes. The next fresh request or refresh uses the latest
  committed state.

## Performance and benchmark gate

- **Receiver list, about 12 s at `limit=40`.** The current request can spend time
  in readiness and producer waits before SQL. Its SQLite snapshot also reads
  runtime and recorded authority and qualifies selected metadata, including
  per-member title checks. These are concrete paths to time; the receiver split
  has not been measured. In CI on a receiver-shaped, sanitized multi-source
  fixture, take baseline and candidate HTTP timings for `/api/sessions?limit=40`
  at idle, during appends and with pending sources, on fresh and warm connections.
  Split wait/queue, SQL and row count, runtime/custody, title/presentation and
  response time. Remove the publication wait, capture runtime/custody once,
  use set-based page queries and avoid per-member metadata probes. Inspect plans
  and rows visited if SQL still dominates. Keep HTTP measurements alongside the
  standard query benchmarks so fast SQL alone cannot hide the original delay.
- **Production case A, about 16 ms per append.** The phase 2 report places most
  of the cost in the existing Index write bucket. `ProductionScale::measure_appends`
  in [`observations.rs`](../../crates/semon-sessions/src/model/tests/star_catalog/observations.rs)
  reports `parse_ms`, `index_ms`, `fact_write_ms` and `maintenance_ms`. The fact
  and maintenance timers sit inside the Index transaction, so derive the
  exclusive Index remainder or report nonoverlapping spans. Measure proof/ledger
  work, existing Index writes and commit separately if the remainder reproduces.
  Compare writer-alone and concurrent-reader samples on the same runner. Keep
  cost independent of historical file length and lineage size for ordinary
  appends. Fix an attributable regression in the phase 3 path before cutover;
  record any existing Index bottleneck separately with its measured dominant
  work. The 16 ms observation is a baseline, not a new pass threshold.
- On the cutover head, manually run [Star Catalog scale](../../.github/workflows/star-catalog.yml).
  Exercise the actual SQLite reader with the 1 GB/20k-session corpus. Report
  fresh/warm list and selected-detail times and separate ingest and maintenance.
  The review targets are 500 ms list and 50 ms detail. The CI ceilings are 5 s
  and 500 ms respectively. Keep deterministic `EXPLAIN QUERY PLAN` assertions
  against correlated scalar subqueries and recursive lineage reads in either
  query, and global detail session/owner scans or materialization. Prove that
  unrelated sessions do not increase selected record or suppression work.
- Keep all three append cases at 10,000 appends of 1–5 records each. Case A uses
  a million-record Claude file with `compact_boundary` records, B uses 50
  continuation-linked files with appends to the newest, and C uses a 300-file
  bridge group with appends spread over members. Report count, total time and
  first/last 1,000 p50/p99 in microseconds. Every case must meet last-window p50
  ≤ 3× first-window p50. Keep randomized split/interleaving comparison against
  independent recomputation.

## Comparison and acceptance

Use the old reader as a temporary fixture oracle. Compare against the same
admitted source witnesses and qualified runtime sample, with the oracle caught
up for parity runs. Keep independent expected answers for continuous writes,
where the oracle can be behind by design. Remove the comparison plumbing at
cutover where it is only needed for Viewer development. Existing phase 4
consumers retain their current implementation until they switch. Production
Viewer routes have one SQLite read path, with no fallback to the oracle.

| Acceptance | Production-path test |
| --- | --- |
| Full row, relationship and turn parity | Compare the complete visible list/detail fields and canonical addresses on sanitized Claude, Codex and Copilot inputs. Include aliases, retained history, usage/cost and parent/family fields, late parent and relay endpoints, marker precedence and cycles, copied positions, late results, causal boundaries and end states. Cover the result and entry expansion and following a trace link to the same highlighted turn. |
| Runtime qualification | Cover current and expired observations, PID/start mismatch and PID reuse, received facts, process/ancestry and parent/child overrides, and unrelated runtime churn during repeated reads. |
| Collision keys and pagination | Exercise cross-harness collisions, reserved native key forms, identical timestamps and IDs across machines, every existing filter and a bounded search frontier. Traverse small pages and assert order, scope, counts and no omissions or duplicates. Change filters, inventory or custody and assert the appropriate refusal or explicit restart. |
| Transcript cursor and membership | Use a component with more than 64 sources, including evidence beyond member 64. Follow the cursor through fresh SQLite snapshots, expand an entry and navigate from an evidence link. Assert that cursor state grows with sources touched by a page and that continuations need no server-held state. Commit proof-qualified appends before and during body fetch, including to a member first touched on a later page. Assert that each continuation stays within the committed prefixes and that a fresh request sees later commits. Check explicit restart for incarnation change, replacement, same-size rewrite, truncation, custody change and changed membership, including a late parent link. Withhold bytes on invalid proofs. |
| `/api/sessions` with a concurrent writer | Use the real HTTP server and the supported admission path. Start with committed rows and keep appending complete native records while issuing repeated HTTP list requests. Every request returns 200 with coherent committed rows, counts and relationships. Assert no `catalog_unavailable`, no exposure of an incomplete record, and observation of newly committed additions while the writer is still active. Coordinate overlap with test barriers/hooks and assert the committed revision counter advances while a list read snapshot is open and again during the repeated request window. Count successful Index commits, not writes to the file. Bound the test so a blocked writer fails. |
| Running transcript regression | Reproduce [#715](https://github.com/onsager-ai/semon/issues/715) with repeated `/api/session-transcript` requests during real appends, including growth during body reads. Require every fresh request and append-safe continuation to return 200 with only its committed prefix. Verify the view opens, stays readable and shows new commits on the normal refresh path. |
| Pending coverage and refusal | Hold back admission for a configured source while another keeps committing. Verify committed rows and explicit partial/pending coverage without waiting for quiescence. Check recovery and authorization refusals separately from valid empty results. |

## Viewer verification

Use [viewer-verification](../../.agents/skills/viewer-verification/SKILL.md), the
[Viewer ownership checklist](viewer-ownership.md#refactoring-review-checklist)
and the [UI design workflow](ai-design-workflow.md#baseline-scope-and-review).
Preserve the native Catalog/sidebar and transcript state owners, route and
source scope, stable row/entry keys, focus and scroll. The intentional visible
change is that a live selection opens and remains usable while data arrives.
The existing layout, chrome and shared design contract govern the comparison.
If wire or state handling changes, update `ui/src/` and regenerate bundles via
[`ui/README.md`](../../ui/README.md). Verify architecture, types, source/security,
bundle freshness and sizes in CI.

Manually dispatch the full [Viewer UI workflow](../../.github/workflows/ui.yml)
on the cutover head and check the actual `viewer browser checks` aggregate.
Use its locked fixture tooling and synthetic native/received inputs. Cover list
filters and pagination, single/multi-machine navigation, running and finished
transcripts, pending/unavailable and retained sources, entry expansion, evidence
deep links, Back and scroll restoration. Include live-update, transport,
interaction/touch, accessibility and lifecycle coverage from the workflow. Inspect
uploaded `viewer-ui-*` screenshots for the affected states at matching desktop
and phone viewports in light and dark, with the affected touch states covered
by the workflow. Compare base and head under the same fixture, route, viewport
and theme where available. Record the inspected images, expected change and
reference disposition in the cutover PR. Classify implementation, reference and
fixture defects and any remaining product decisions per the workflow. CI makes
no pixel comparison. Use [the fixture guide](../../tests/ui/README.md) for suite
selection and artifact names and report any unrun or blocked coverage. Run
pinned Codex conformance in CI for changed native read semantics.

## Suggested implementation PRs

0. **Warehouse-owned retained-history identity.** Advance the Index schema
   directly, certify and retain original component bindings, and test refusal,
   withdrawal, replacement and cross-harness key reuse. The host rebuild ships
   with cutover.
1. **Warehouse list and identity queries.** Add the fixture-only read adapters,
   source qualification, filters/search, coverage and union/keyset tests. Capture
   the list and append baselines and add production-query plan checks. The
   Viewer still uses the existing reader while the comparison matures.
2. **Selected component and proof reads.** Add the selected metadata, causal
   rendering and position/byte adapters, member paging, stateless prefix cursors
   and source/refusal tests. Compare transcript, anchors and entry fields in CI
   without wiring a second production route. Land this after the shared
   identity and scope contract in PR 1.
3. **Atomic Catalog and Viewer cutover.** Switch the list, identity, navigation,
   selected metadata/transcript, anchors and evidence links together. Remove the
   Viewer semantic-ready waits and dead Viewer-only code. Audit readiness through
   the HTTP server and host entry points, including the pushed receiver. Gate on
   the concurrent-writer and live-transcript tests, benchmark and Viewer review.

## Phase 4

Switch ASI SQL, analytics inputs, CLI/MCP list, search and focused reads and
metadata export to the warehouse and source resolver. Check causal turns,
shared/exclusive and family totals, costs and label provenance. Keep body/FTS
access origin-local. Remove the semantic stage, activity projection, relation
publication, generation fences, phase cursors, row recipes, derived journals
and any remaining temporary oracle with their final consumers. Retain native
integrity checks, authorization, custody, snapshot isolation and Index CAS.
Update the ASI contract and host rebuild guidance. Phase 3 delivers the read
cutover and leaves operator data and retained source evidence under existing
custody.

## Transcript cursor decision

On 2026-11-21, the owner chose a stateless prefix cursor. The cursor carries
an incarnation and committed-prefix proof for each touched source, plus a
witness of the selected member set. Each page opens a fresh SQLite snapshot
and reads within those prefix bounds. Later commits appear on the next fresh
request or refresh. An incarnation change, replacement, rewrite, truncation,
custody change or membership change, including one caused by a late parent
link, requires an explicit restart. Cursor state is bounded by the sources a
page touches. The server holds no cursor state between requests.

### Prefix receipts in the selected-read fixture adapter

The Index writes one `warehouse_prefix` receipt when an admitted source
first commits or advances its consumed offset. The receipt records the source
incarnation ID, committed offset and ledger at that boundary. It contains no
source text. Receipts commit with the facts and native proof pages. Keep receipts
from the last 24 hours plus the newest receipt per source incarnation. Normal
ingest collects expired superseded receipts in bounded batches and records
the highest removed revision. The reader explicitly restarts a cursor older
than 24 hours or at or below that revision. Collection can invalidate a
recent cursor when a long-idle source advances and loses its old receipt.

A first page records the highest receipt revision in its snapshot. A later page
can use that revision to recover the same bound for a member it has not read
yet. The cursor also carries a digest of the selected member identities and
carrier paths and the prefix receipts for the sources read by the previous page.
The Index incarnation, canonical selection and current source authority qualify
these values on each request. Covering blocks come from the current proof tree
in the page's SQLite snapshot. A receipt and current proof may be combined only
within the same source incarnation. The reader caps returned data at the
receipt offset. It re-verifies read blocks if the local carrier grows during
body fetch and rechecks the named file and descriptor before returning the page.

This fixture adapter covers current local record positions, copied-record
suppression, exact-address text lookup and prefix-bounded tool result reads.
The retained adapter pages original records through an authorized provider
using the warehouse binding and revalidates the previous page's source
references. The current adapter exposes native prompt anchors and exact-ID
relay receipt addresses for selected tool results. It uses the shared native
launch matcher on a qualified Codex marker and the parent's committed launch
candidates. The list compares Codex prompt titles, reads qualified top-level
process status, and prices winning usage facts and legacy Codex counters.
Schema 54 preserves the versions of native signals that can change when a
later record enriches an earlier position. The selected fixture adapter reads
those signal versions and reply facts within the cursor's committed view. It
compares native entry IDs against the current reader. Schema 55 indexes
prompt and gap boundaries. Selected records carry the latest unsuppressed
prompt across member files and pages, bounded by the original receipts. A gap
resets that prompt at the preceding native timestamp. The adapter resolves
unique native event and tool-call IDs to proof-checked records and carries the
turn start's canonical address across files. Relay and launch turn placement,
operation anchors and the full activity anchor contract still need parity.

For current reads, a host may supply an authorized provider for a committed
source whose local carrier is absent and whose path remains in the authorized
inventory. The provider authorizes the exact selected receipt. The reader pins
one provider generation and length for the request and reauthorizes previous
provider sources on continuation. The cursor carries a digest of each pinned
generation. The adapter restarts if source custody changes, including when a
local carrier appears during provider reads. Pages that can use the provider
are limited to 32 positions so previous and new reads share the 64-source
request budget. A changed or invalid existing local carrier still restarts.

The selected family pricing adapter qualifies every member against the
current inventory and prices each descendant with the same effective rates as
the list. Inclusive dollars remain unknown when a member has unknown or shared
cost. Shared dollars count each connected Claude invocation once, using the
native billing fact of the winner in the existing family fold order. A fixture
compares inclusive dollars with the current reader and checks shared winner and
unknown-price coverage.

Full causal presentation, remaining anchors, relay body/time matching, and
parent/child runtime overrides still need comparison before cutover.
