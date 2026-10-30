# Worker140 / PG57 — unfinished major business-flow WIP

## Worker144 / global144 / PG61 — same major B, NOT COMPLETE

Custody: started with clean local/remote e00cce06113de58b3f09c4caf43dbe41463ec309,
which is Root's normal merge of 522af2f and Worker140 final 17d66662. No reset,
other worktree mutation, nested harness, public PG factory or MAIN merge occurred.

Implemented WIP on task/477-pg-vertical-20261026:
- Step 0: ordinary UUID/versioned Copilot and rollout Codex fixtures. CI
  40265716159/e22143bc passed the formerly failing SQLite oracle, PG model parity,
  and restart; failed an append source assertion that incorrectly used row 0.
  This now finds the exact source path. No native validation was weakened.
- Step 1 PARTIAL: publication uses semantic_owner::begin and a durable monotonic
  catalog_stage epoch under the SourceState lock. This is NOT the full semantic
  staging producer; begin_semantic_stage/batch/record/publish/finish/select/replay
  trait defaults remain unavailable and are STILL BLOCKING the ordinary queued
  semantic flow (events.rs:1160-1217). Do not accept the stage row as that owner.
- Step 2 WIP: ordinary IndexStore publication ports now call one transaction with
  native135 binding, immutable Current/recipe/trace/search effects, history
  preservation, expected-head CAS, exact claim ACK, full-value operation replay,
  durable native proof references and change hints. The ordinary port derives
  operation IDs from the complete request (including trace witnesses/overview,
  recipes, base, claims and closed mode). Removal of an unmentioned Current row
  explicitly refuses; general partial/cohort closure is NOT complete. Explicit
  mismatched caller-supplied OpID qualification remains missing.
- Step 3 PARTIAL: session_slots reads bounded exact-source recipes. Explicit
  pending remains remove_files, resolve_catalog_input, relocate_codex and
  catalog_component_paths. No claim is made that those are off the full ordinary
  call path: semantic execution has NOT reached them. qualified_catalog observes
  a committed op; it is not complete backend/public selection qualification.
- Step 4 PARTIAL: acknowledged immutable generation + expiring lease can feed the
  ordinary Catalog list renderer. Completeness is explicitly partial. Bounded
  request-owned selected_reader adds native-page hash proof verification to an
  independently authorized host reader and ordinary native_field::preview.
  No local Raw file is opened by the PG reader. Proofs are retained indefinitely;
  there is no GC capability. CatalogSnapshot search, selected_details and
  transcript_observation still explicitly refuse; QueryCatalogStore is unported.
  This is not complete selected Query/body/field integration.
- Step 5 PARTIAL: existing scenario now attempts publish, exact ACK replay,
  competing stale CAS refusal, ordinary list, scalar field parity, restart,
  append/republish and old selected proof read. Existing process-wide SQLite
  rejection and Raw checks remain. Full list parity, restart selected route,
  explicit mismatched OpID, concurrent CAS, restricted-role collision on the new
  reader and complete cancellation/oversize publication matrix are still missing.

Observed CI: 40265909533/8a8ef8e2 failed private publish visibility (fixed).
40266184604/51f0bfa1 compiled and ran through publication, identical ACK replay and
stale competitor refusal, then failed list rendering at businessflow.rs:134.
The adapter supplied an invalid completeness value; fixed to current `partial`.
40266455652/eccaf779, 40266562039/132348ec, 40266653453/6ee60ee5 were still running
at observation. Final code 16eeca2382823d6a07459e08502d93a70eaf077c was normally
pushed and full remote SHA verified. No final-green evidence exists. Formatting
has not been applied from CI artifacts and fmt/strict Clippy/default/full gates
remain pending. Root must collect exact-head results, not infer green from a
prefix assertion. These WIP paths need further implementation and review.

Interfaces for Root: only private PG modules/current native TABLES additions;
no PgPool API or public backend enum change. New tables catalog_operations,
catalog_proofs, catalog_changes, catalog_stage and catalog_leases are current
manifest entries and inherit namespace forced RLS. Preserve immutable bytea Tree
address fix. Reconcile A/C via Root only. Full477/current transfer/catch-up/fenced
cutover/qualified rollback/Hub313 stay open; #380 before #477 MAIN unchanged.

Thermal log remains unavailable. No local cargo/build/check/test/clippy/fmt,
lockgen/update, Node/typecheck, browser/image/corpus/SQL benchmark ran. CI only;
stop CPU work at >=85 C or unavailable readings. Bounded named-file reads and
sanitized small diagnostic artifacts only. This bounded attempt is FAIL, not an
external authorization blocker, and grants no automatic retry/escalation.


Worker140 is NOT complete and is not accepted. No full #477 claim is made.
This bounded start did not connect admitted Catalog/list/selected reads. Continue
the same major business flow; do not independently accept the helpers below.

## Custody and actual changes

Started after Worker139 was observed running. Dedicated worktree was clean and
local/origin both matched `14d7331ec0b24bfeda203b90b411cbfdaa071082`.
This already included preserved SemanticOwner draft `7788afff` and native recovery.
No other worktree was mutated. Public backend selection remains closed.

The ordinary PG IndexStore now reads/saves real session descriptions, reads the
actual Catalog head and append evidence, and retains a durable private namespace
runtime incarnation across EventCache reopen. This incarnation is NOT a complete
semantic staging epoch owner or publication admission. Full-value key checks,
namespace RLS, 64-row/1 MiB bounds and transactional refusal remain in place.
Unknown current schema contract refuses; no historical schema upgrade was added.

One isolated CI scenario now constructs ordinary Claude, Codex and Copilot native
inputs, runs the SQLite ordinary model oracle first, then rejects SQLite opens
process-wide while PG ingests and the ordinary `model::prepare_sources` runs.
The scenario asserts model/recipe parity, separately checks the namespace-bound
runtime witness, reopens EventCache, appends real input and continues, verifies
Raw preservation, namespace-isolated descriptions, oversized atomic refusal and
cancellation. This is currently ONLY the ingest-to-model prefix, not the required
full business-flow qualification. Its runtime assertions have not yet passed.

CI exposed an inherited PG tree witness caller still using the old two-tuple.
It now compares the complete current Tree4 tool set, nullable names and closed
state against independently framed SQLite JSON rows. CI also exposed SQLSTATE
42P17 during schema creation: inherited generated Tree addresses used stable
`convert_to` expressions. Fixed bytea domain literals retain the exact same bytes
while making the generated expressions immutable. Both fixes need final CI.

Formatting was produced in CI and applied from exact artifacts; no local fmt ran.
Inherited SemanticOwner/SQLite draft formatting in the artifacts was retained;
Root must reconcile those formatting-only overlaps with A/C.

## Actual verification

- `40263670989`, code `6a6612b2ef97e1a8c684bc37ab6460e28b204258`:
  failed compilation (missing NativeEventStore trait import and inherited Tree4
  caller). Superseded run cancellation requested. Neither failure is a pass.
- `40263891432`, code `64bf8b473c749d9694f1c523160ec8ddffc486dd`:
  superseded before acceptance; cancellation requested.
- `40264013651`, code `35b117bd92f936058fbc288e1549271426b8e916`:
  compiled tests; ordinary model and existing Catalog scenarios failed at fixture
  schema provisioning, SQLSTATE 42P17 / sanitized `Unavailable`. Superseded run
  cancellation requested. No business-flow/runtime pass established.
- `40264221008`, code `f65cf1b94db7dca70a858321d638296023addbf2`:
  superseded formatting/diagnostics revision, cancellation requested.
- `40264313371`, code `44ead665c98dc39e7f6ecaaadb013f72ef1f0a7e`:
  ordinary scenario FAILED at `businessflow.rs:69` in the initial SQLite
  three-harness oracle `prepare_sources`, reporting InvalidData. PG business
  phase was therefore NOT reached. Fixture/model failure is unresolved; do not
  remove the parity requirement or treat this as a PG pass. The job remains
  IN PROGRESS at handover, includes immutable-prefix correction. Must inspect
  focused scenario, existing PG focused/full, fmt, strict Clippy and default
  driver gates. No green claim.
- Exact audit revision `e3ae5f6ad948946e11b6ff2bd42e7750c99fc338` has run
  `40264421172`, still in progress at the last observation. Its code matches
  the failed-prefix code above; no green exact-head evidence exists.

## Blocking remaining work

`index_store.rs` still explicitly refuses remove_files, session_slots,
publish_session_catalog_fenced, publish_partial_catalog,
publish_closed_catalog_fenced, resolve_catalog_input, relocate_codex,
qualified_catalog and catalog_component_paths. Semantic staging methods still
inherit the unavailable trait defaults. Required full semantic epoch ownership,
native-proof Current/history ownership, exact publication OpID replay, head CAS
and claim ACK integration remain unfinished. Existing native lost-ACK handling
was preserved, not substituted for missing Catalog operation reconciliation.

Admitted CatalogSnapshot, Query/list and selected body/field ports, retained
ReaderLease and proof lifetime qualification are missing. The new scenario does
not test these or complete the required lost-ACK/CAS/restricted-role/retained-proof
business-phase matrix. Existing component tests remain; no thresholds weakened.
No additional public PG enum/factory or SQLite/memory fallback was introduced.

Full #477/#478/#479, current-format SQLite↔PG transfer/catch-up/fenced cutover/
qualified rollback, Hub #313 and final Source/Hub qualification remain open.
#380 must complete before any #477 MAIN merge. No merge/deploy/publication/live
PG/user-data/credential/permission authority was exercised.

## Resume limits

All task code has normal WIP commits pushed to existing origin branch
`task/477-pg-vertical-20261026`. Code SHA `44ead665c98dc39e7f6ecaaadb013f72ef1f0a7e`
was verified against `git ls-remote`; the audit commit succeeds that code revision.
Final external delivery JSON records the resulting full remote branch SHA.
No nested harness, controller or agent was launched.

Temperature log `/home/marvin/.cache/thermal-watch/temps.log` remains absent.
No local CPU-bound work, cargo build/check/test/clippy/fmt/update/lockgen, Node
tests, project typecheck, browser/render, corpus analysis or SQL benchmark ran.
Continue CI-only. Check temperatures before and after CPU-bound work, stop at
85 °C or above, and never start without current readings. Read/search one named
file under approximately 10 MiB, at most 150 lines/4000 tokens; bounded logs only,
strip ANSI and apply safe allowlists, never print Raw, DSNs or credentials.
