# Worker140 / PG57 — unfinished major business-flow WIP

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
  IN PROGRESS at handover, includes immutable-prefix correction. Must inspect
  focused scenario, existing PG focused/full, fmt, strict Clippy and default
  driver gates. No green claim.

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
