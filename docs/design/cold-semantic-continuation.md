# Cold semantic continuation

Status: design checkpoint for #477 / the #493 configured-storage port. No
production implementation or acceptance is claimed by this document.

Inspected base: `d24bbf15bab96dffa7583e3125cd02c252fc9378`.

## Problem and required behavior

The configured producer currently treats its durable component as a single
64-source preparation/publication unit. The ordinary producer instead retains
component metadata and prepares and publishes bounded owner groups. This loses
supported native behavior when a cold Codex family has 80 children and one parent.

Each owner in that family has one physical source. The per-owner 64-source
ceiling and each prepared batch's 64-source ceiling remain valid. The family
contains 81 distinct owners and must be resumable across those batches; family
cardinality is not a per-owner source count. This distinction does not permit
paging an indivisible old-owner lifecycle change around its existing limit.

Both original regressions must remain intact:

- `cold_codex_family_crossing_batches_preserves_every_native_parent` creates
  80 children plus their parent and requires all 80 native parent links.
- `resumable_catalog_keeps_current_old_family_outside_viewer_scan_window`
  creates the same 81 sources outside the viewer window. It requires 81 Current
  owners, the parent's complete 80-child Query summary, exact consumed source
  offsets, and qualified original projections only after the family completes.

These are different from changing an already published 64-owner family by
adding a 65th source. The existing
`configured_semantic_domain52_high_degree_raw_component_refuses_without_partial_publication`
must still refuse with `semantic_affected_owner_source_budget`, before changing
any namespace, Current/History projection, proof, or claim. Its two seeds are an
existing parent and the new child. Subtracting seeds from the closure count
would count only 63 additional sources and incorrectly admit this operation.

The fix must preserve the existing newest-first cold page and its timing gates.
It must not defer that page until all historical component metadata is indexed.
It must use the existing native parser, identity/relationship reducer, Query
derivation, custody transfer, and publication effects.

## Current implementation inventory

Paths below are relative to `crates/semon-sessions/src/`.

| Concern | Ordinary implementation | Missing configured continuation |
| --- | --- | --- |
| Source discovery | `events/store/semantic_stage.rs::frontier_page` retains keyset cursors and enrolls bounded metadata pages | SQLite `frontier_page` has cursors, but `staged_paths` rejects the accumulated set at 65. PostgreSQL `semantic_stage::Stage.paths` is one bounded vector. |
| Old-owner authority | Shared `model/relation_stage.rs::affected_owners` validates a bounded recorded owner closure | SQLite `Stage::batch` unions every staged seed with `Owners::affected` and rejects the total. No durable distinction exists between baseline owners and the cold run's own partial publication. PostgreSQL `next` uses the same whole-unit union. |
| Global source witness | Ordinary `witness_rejection` checks indexed persisted source fields and claim changes through SQL | SQLite `reduction_witnesses` decodes at most 65 indexed ledgers and refuses source 65. It also separately bounds negative withdrawal authorities. This cannot simply become an unbounded ledger loop. |
| Native key allocation | Ordinary `native_keys_page` uses durable harness/native cursors and collision occupancy work | SQLite `native_keys_page` loads 65 distinct pairs and refuses above 64. PostgreSQL `record` allocates identities from the one prepared cohort and stores them in its singleton payload. |
| Identity preparation | Ordinary `owner_closure` keeps one complete physical owner group together, then fills a page of at most 64 paths | SQLite `Stage::batch` selects 64 pending paths without the equivalent complete-owner grouping. PostgreSQL advances the phase for its entire path vector after one `record`. |
| Parent/root/cost reduction | Ordinary `reduce_with` advances bounded metadata pages | SQLite already uses `ScopedReduction` with this shared algorithm. It must remain the sole reducer. The configured phase planner does not expose its completed results as bounded contexts. |
| Context loading | Ordinary `load_context` loads selected owners, parent descriptors, bounded child keys and family costs; publication incident edges replay separately | SQLite loads every staged endpoint with `LIMIT 65` and refuses above 64. Its publication context derives children from that complete endpoint map. PostgreSQL embeds the entire context in `Stage` and `save` caps endpoints at 64. |
| Relationship work | Ordinary relation pages retain selected-actor cursors and bounded companions | SQLite has `relation_page` and per-source `relation_after`, but the surrounding context is whole-component. PostgreSQL persists a single prepared relationship context. Both must retain the existing selector and companion semantics. |
| Publication selection | Ordinary `semantic_next` selects unpublished complete owner groups, unions at most 64 physical sources, and reduces groups if context bytes overflow | SQLite selects pending source paths and expects the complete stage in final publication. PostgreSQL publishes its whole `Stage.paths`. |
| Retirement | Shared lifecycle derivation uses exact prior/retired-owner and authority witnesses | SQLite `retirement_plan` calls `affected` over all staged paths and requires all old owners in the one prepared model. PostgreSQL passes whole-stage `prior_owners` to publication. Retirement needs a complete bounded lifecycle unit, never an arbitrary owner page. |
| Model receipt | SQLite `record` persists a model witness under the stage epoch and exact claims | `storage_semantic_model_batches` is one row per namespace. A page identity is absent. PostgreSQL `Stage.model` is also one whole-stage witness. |
| Publication receipt | Ordinary `published` marks only selected owners/paths and advances the stage head | SQLite `storage_semantic_publications` has immutable primary key `(namespace, epoch)`, and replay looks up only that epoch. It can represent one publication, not several pages in the same continuation. PostgreSQL marks the entire stage inactive on its first publication and retains its acknowledgement per epoch. |
| ACK and completion | Ordinary `published` ACKs exact selected claims; `semantic_finish` completes only when all work is finished | SQLite `publish` requires `staged == batch.paths`, then marks **all** staged sources ACKed and **all** endpoints published. `finish` deletes all stage rows in one transaction. These operations need selection/completion and bounded cleanup boundaries. |

The ordinary implementation is the algorithm reference, not an alternate store
to open from PostgreSQL. In particular, use its typed physical ports to share
decisions; do not introduce a second parser, Event Index, SQLite facade, or
backend-specific relationship reducer.

## Smallest cohesive shared contract

Keep `Batch.paths` as the native admission unit: at most 64 physical sources,
with the existing smaller relation actor selection and companion accounting.
The durable continuation's component metadata may span more than one batch.
Do not raise native admission, selector capacity, metadata, or source byte caps.

Introduce one typed continuation/page contract next to the existing semantic
owner decisions and relation-stage types. Its responsibilities are selection,
authority classification, and exact receipts; the existing preparation and
publication code remains responsible for deriving and writing content.

The minimum durable state is:

1. **Continuation identity:** namespace and admitted incarnation, a stable run
   identifier, original Catalog head, current Catalog head, recipe, and existing
   source/runtime authority binding. Keep the existing mutable holder epoch
   separate: crash resume changes ownership without changing committed page
   identities. A semantic reset invalidates pending page requests.
2. **Source/owner progress:** the existing indexed source, frontier, owner-source,
   endpoint, relation, publication and ACK records and their exact witnesses.
   Retain these as paged metadata, rather than embedding the whole component in
   a serialized context. Record which exact owner/source projection was emitted
   by an acknowledged page of this continuation.
3. **Pending page:** stable continuation/page identity, phase, holder epoch,
   selected complete owner groups and exact paths, base Catalog head, source
   revision, runtime binding, and exact prepared model/claims witness. There is
   only one pending writer page per continuation. Selection is deterministic;
   retrying it cannot select a different source cohort under the same receipt.
4. **Committed page receipt:** the exact request witness, resulting Catalog
   generation and acknowledgement, written atomically with its publication and
   exact source ACKs. Key it by continuation and page, rather than epoch alone.
   Bind existing proof references to this same exact receipt; never replace or
   mutate an immutable publication receipt to make room for the next page.

Use typed page selection results for indexing, complete-owner identity groups,
relation actors/companions, complete lifecycle units, and publication owner
groups. These are variants of one existing stage, not independently scheduled
queues. Physical ports supply bounded indexed rows, advance cursors and persist
the shared decision inside the admitted transaction. Metadata byte accounting
must include child keys, endpoint descriptors, claims and receipt payloads.

For SQLite, extend the existing namespace-leading stage/progress family and
publication receipt keys. For PostgreSQL, the existing
`semon_pg_native_catalog_stage_items` family can hold bounded typed progress
items/pages; stop using `seq=0` as a container for the complete source/context
cohort. Keep `seq=0` as its bounded control record if useful. Load only the
selected metadata page, verify its stored digest and namespace/incarnation,
and enforce the same decision contract as SQLite.

These are current-format changes. Update authored DDL, shape validation,
producers, consumers, recipe and fixtures together under the pre-release policy;
do not add migration readers or silently convert unsupported profiles.

## Baseline owner authority and cold provenance

Capture continuation provenance before its first partial publication. The cold
path in `native_evidence.rs` can call `publish_partial_catalog_fenced` before
creating a semantic stage, so flags on a later stage are insufficient.

Keep the existing newest-first preparation and first publication path. Attach
its continuation identity and exact receipt in that same admitted publication
transaction, using already prepared source/model witnesses. Do not insert a
global native scan, owner snapshot, or full-component reduction ahead of the
first page. A later semantic stage adopts this durable continuation only after
qualifying the receipt and live head. If the first preparation already needs a
stage, create that same continuation before indexing its subsequent pages.

Classify a Current owner as produced by the continuation only when its exact
Current owner/source/model witness matches an acknowledged receipt in the live
continuation's publication chain. Membership by session key, path, backfill
state, or `published=1` alone is insufficient. A row that differs, a publication
from another writer, or an owner without that receipt remains baseline Current.
The existence of a cold continuation grants no Raw or withdrawal authority.

Apply that classification in indexed physical owner/neighbor lookups before
their existing overflow sentinel, rather than first decoding an entire cold
family through `affected_owners` and filtering afterward. Persist the exact
owner/source witness at publication so qualification does not require scanning
all old page receipts for each neighbor. Reuse existing staged source/owner
records for this relationship; add only the receipt identity needed to bind
them to their committed publication.

For a genuinely baseline affected component, retain the current operation:
collect its full recorded affected-owner closure, include its initiating native
sources, and refuse if the complete lifecycle unit exceeds 64 sources or the
existing owner/metadata bounds. Do this before committing discovery cursors,
admitting peer Raw, or modifying publication state. Do not partition a baseline
65-source lifecycle unit across pages. Disjoint new cold owners may progress in
their own pages; they do not make an oversized baseline lifecycle unit legal.

Owners emitted by the same qualified cold continuation do not become newly
baseline owners merely because an earlier page became visible. Reuse their
durable staged native identity/relationship descriptors and exact source
witnesses. This allows an 81-source cold component to finish without reparsing
all previously published sources on every page. It also permits the parent's
final Query summary to include all 80 child keys through the existing bounded
metadata context, without admitting 80 child bodies into its native batch.

The full graph must still be discovered and reduced. Excluding an acknowledged
cold owner from **baseline retirement enrollment** must not exclude it from
parent/root/cost reduction, relationship selectors, incident replay, final child
summaries, source readiness, or complete-cohort qualification.

## Page execution, replay and completion

Index and identity pages use the existing parser and complete-owner grouping.
Native key collision allocation uses the existing durable keyset/occupancy
algorithm. Parent/root/family-cost work uses `reduce_with`; relationship matching
uses existing selectors and actor/companion cursors. Extract physical ports from
ordinary `load_context` and owner-group selection where needed so both adapters
use the same selection and normalization decisions.

Replace whole-component decoded witness loops with persisted indexed witness
checks, following the ordinary producer's ledger-field/claim rejection. Every
page must detect a changed previously observed source, not only changes to its
selected paths. Negative withdrawal authorities retain their explicit bounds
and live authority checks; paging is not a way around the withdrawal limit.

Publication selects complete unpublished owners under both native-source and
metadata budgets. The selected page must exactly cover its model, claims and
withdrawals, and independently match staged identity and completed reduction.
Baseline retirement is indivisible within its admitted lifecycle unit. A
successful publication updates only the selected owners and source claims,
records its receipt, and advances the live continuation head atomically.

An exact replay finds its own page receipt, verifies the captured request and
existing authority/receipt qualification, and returns its recorded ACK without
writing a successor page or consuming a newer claim. A different model, source
set, runtime, generation, page identity or prepared witness is a conflict with
zero effects. Preserve existing backend replay guarantees, including the
PostgreSQL test that finds an acknowledged old epoch after successor publication;
do not accidentally impose singleton-current-head lookup on those receipts.

Keep `Deferred` custody progress distinct from success and conflict. A deferred
page keeps its exact selection and witness; it does not ACK paths, advance its
page sequence, mark another owner published, or qualify a partial original.

Completion requires all discovery frontiers, native keys, identities, copied
event qualification, parent/root/cost reduction, relation cursors, owner
publications and exact ACKs to be complete under the live witness. The empty
Publication batch remains the explicit completion request. Listing or source
readiness must not infer completion from the first acknowledged page.

## Resume, reset and cleanup

Crash resume restores durable page progress and the current head, then takes a
new holder epoch through the existing owner transition. No in-memory model is
authority. Fence any retained pending page/model against that holder transition
using the same exact witness rules as the current producer.

Source, parser, membership, custody, runtime and external Catalog changes retain
the existing reset/conflict decisions. Journal-qualified unrelated Facts
activity may advance the existing runtime witness without discarding unaffected
work; unjournaled changes and negative custody keep conservative resets.
Reset cannot confer cold provenance on changed Current state. A previous cold
receipt is reusable only while its exact live chain and owner/source witnesses
qualify; otherwise treat that state as baseline and apply the existing bound.

Invalidate pending models/pages atomically, then reclaim private stage rows
through the existing bounded reset/cleanup mechanism. Preserve published
Current, History, original recipes, proof ownership, source journal claims and
all committed page receipts needed by replay/proof references. Do not garbage
collect immutable publication evidence merely because its active run finished.
Existing reference/custody ownership must determine eventual receipt reclamation;
this change does not add a new retention policy or delete native data.

Normal completion also needs bounded private progress cleanup. Keep enough
control state to distinguish cleanup in progress from a new run, and never
advertise full completion while exact dirty claims remain. Preserve namespace
guards and incarnation checks on every cleanup page.

## Remaining implementation sequence

1. Extract shared complete-owner page selection, selected context loading and
   page decision/witness types from the ordinary stage. Keep its current output
   and newest-first behavior unchanged. Add focused contract tests for page
   selection, exact receipts and full baseline closure refusal.
2. Add durable continuation identity/provenance and page receipt keys to the
   admitted publication family. Carry the same identity through the existing
   first partial publication and a later stage; update proof/replay consumers
   and authored profile validation together.
3. Port SQLite source/key/identity/context/publication planning to that shared
   page contract. Replace global decoded ledger checks with indexed rejection;
   make retirement page scope exact, update only selected ACK/published flags,
   and page completion cleanup. Preserve existing selector capacity checks.
4. Port PostgreSQL's singleton Stage payload/phase progression to bounded
   progress items and the same shared decisions/receipts. Retain its current
   capability/factory closed boundaries; no SQLite opens or public capability
   expansion are part of this port.
5. Add crash/resume and stale-page/fault cases at each phase and around page
   publication, then run the acceptance matrix below in CI. Review both SQL
   adapters for namespace, authority, custody, replay and exact ACK parity
   before integrating the configured port.

Do not land an intermediate production change that only removes a source,
endpoint, witness or receipt capacity check. Such a change would either move the
failure to a later phase or weaken the existing authority/refusal contract.

## Acceptance and execution limits

Run applicable checks in CI. On the current workstation, no local cargo
build/check/test/clippy/fmt, Node tests, or project-wide typechecks are authorized.
Check `/home/marvin/.cache/thermal-watch/temps.log` before and after CPU-bound
work; do not begin without current readings and stop at 85 °C or higher. The
inspected tail is stale, so this checkpoint used only bounded reads and edits.
This work does not authorize E2B, SSH, cloud agents or runtime qualification.

Required evidence, with original fixture counts and thresholds:

- Both 81-source regressions named above, plus
  `cold_codex_parent_context_survives_separate_unrelated_batches`. Preserve the
  80-child parent summary, native offsets, eventual original readiness, and
  incomplete-family readiness refusal.
- The existing domain52 high-degree refusal with 64 published owners, one new
  child and two seeds. Compare all namespaces and Current/History/slots/proofs
  before/after the refused transaction, including after restart.
- `configured_semantic_domain94_name_members_source_overflow_rolls_back_before_raw`:
  64 independently published hinted owners plus an initiator, unavailable peer
  bodies, exact capacity error, unchanged cursor/state, repeat and restart.
  Metadata-only selector capacity must still refuse before Raw preparation.
- Existing domain52 changed-authority/foreign-epoch zero-effect cases,
  domain61 late-child reset, domain94 late-member fencing, ordinary source
  witness changes, namespace isolation, and PostgreSQL old-request replay and
  factory/capability refusal suites.
- New page fault cases: crash after first cold partial receipt; restart with a
  new holder during each phase; crash before/after exact page ACK; duplicate old
  page after a successor; source/claim/runtime mutation between pages; deferred
  custody; incomplete cleanup. Compare the final native results with the
  ordinary producer and assert each native batch stays at most 64 sources.
- Existing newest-first and unrelated-source counters, including
  `cold_catalog_search_newest_first_page_bypasses_unrelated_pending_paths`,
  `cold_scoped_native_publication_is_visible_before_unrelated_source_reads`,
  `initial_catalog_page_precedes_global_native_observation`, all
  `cold_catalog_search_` cases and
  `journal_append_and_warm_restart_ignore_unrelated_history`.
  Preserve the current first-page two-second gate and all other source/test
  timing thresholds. Keep both the isolated ordinary CI timing gate and the
  full-parallel suite; the `SEMON_BUILD_TIMING=1` diagnostic is not acceptance.
- Repository Rust format, clippy and complete test gates from
  `.github/workflows/rust.yml`; existing PostgreSQL qualification must run in
  its authorized CI environment. Passing only the new cold tests is insufficient.

No new CI or runtime result was produced for this design-only checkpoint.
