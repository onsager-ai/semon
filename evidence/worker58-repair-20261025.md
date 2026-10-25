# Worker58: bounded high-degree lifecycle fixture repair

SOL_USAGE_UNKNOWN_BOUNDED_EXCEPTION. One approved attempt, no counter reset or
automatic retry. Implementation delivered for Root-owned fresh CI and Manager
review. **No new test pass or full #477 acceptance is claimed.**

## Classification and actual path

Fixture repair only. The only Rust change is inside
`configured_semantic_domain52_high_degree_raw_component_refuses_without_partial_publication`
in `crates/semon-sessions/src/events/store/configured.rs`. No production behavior,
limit, schema or algorithm changed. No reserved implementation region was edited.

The configured producer calls `storage/sqlite/semantic.rs`, not the singleton
publication implementation in `events/store/semantic_stage.rs`. Worker55's
historical report is preserved, but its reachability claim is superseded by the
reported failing CI and this source audit:

- `events.rs:1354-1407`: real ingest scans Raw and obtains its durable receipt.
  `events.rs:1950-1971` delegates begin/batch to the admitted store;
  `events.rs:2001-2011` prepares with the shared native model and records its
  witness before publication. The existing test helpers use these methods.
- `storage/sqlite/semantic.rs:1331-1342`: begin refuses an empty input or more
  than **64 input paths**, among other argument checks, with
  `StorageError::InvalidArguments`. Its display is `Invalid storage arguments.`
  (`storage.rs:382`). Supplying all 65 paths cannot reach any closure guard.
- `storage/sqlite/semantic.rs:1488-1489`: before closure enrollment, a false
  `reduction_witnesses` produces `StorageError::Conflict`. Its display is
  `Source ledger or observed revision changed.` (`storage.rs:383`).
  `reduction_witnesses` (`semantic.rs:253-281`) returns false for more than 64
  indexed sources, more than 64 withdrawals, stale withdrawal authority, or an
  indexed source's claim/serialized ledger differing from the current values.
  Thus that display alone does not prove an actual ledger mutation.
- Real progressive discovery exists: `semantic.rs:448-517` pages namespace-bound
  path-owner/native/header/dependency indexes and enrolls discovered paths with
  real source-journal claims. `semantic.rs:1948-1990` queues these frontiers from
  real indexed native identity. Staging one parent does not constrain the
  eventual discovered component to one source. With 65 indexed members, the
  explicit `rows.len() > 64` witness branch at 257-258 returns the generic
  Conflict **before** closure qualification. This accounts for the earlier
  fixture's failure mode without claiming that its CI log identified the exact
  witness subcondition. The earlier log did not distinguish these subconditions.
- `model/relation_stage.rs:152-186`: the typed `LifecycleStore` traverses recorded
  Current ownership; `affected_owners` refuses a 65th recorded owner with
  `semantic_affected_owner_budget` at 172-173. The SQLite implementation is
  `storage/sqlite/semantic_owners.rs:156-235`; it validates exact Current source
  membership and traverses Current parents/children, trace and native dependency
  indexes. No relationship reduction is invented by the fixture.
- **Guard required by the new oracle:** `semantic.rs:1495-1513` unions legal
  stage seeds with every source of the typed prior-owner closure and refuses
  more than 64 sources with `semantic_affected_owner_source_budget`. This is
  distinct from both input admission and the 65-recorded-owner guard. The new
  child is not a prior published owner; the fixture has 64 prior owners plus one
  new owner/source, and exercises the combined closure **source** budget.

The singleton `semantic_stage.rs:1817-1821` has a separate input-budget error;
its `witness_rejection` at 1209-1235 uses stage revision plus claim/stat fields,
and its publication lookup at 2081-2100 has an owner-source budget. None of those
singleton guards establishes which guard this configured test reaches.

## Valid producer precondition and stronger oracle

The test first publishes a real foreign Claude owner. It then writes and ingests
only `budget-00` and its 63 real Codex children, stages the legal 64 inputs, runs
`semantic_batch` / `prepare_semantic_model_batch`, publishes through the normal
admitted producer, and finishes the stage. Existing `ready` and `publish`
helpers retain their fault-injected transaction rollback and same-ACK checks.

It asserts 64 prepared rows, 64 Current Catalog owners, 63 children of
`budget-00`, and 64 Current proof references. Only then does it write/ingest
`budget-64`, capture the current runtime and begin a new stage on **two** paths:
the existing parent and the new child. It asserts exactly two staged sources
and zero indexed sources before the refusing call. Consequently the initial
witness check has no indexed-source overflow or ledger comparison to reject;
the parent's 64-owner recorded closure plus the new child requires 65 paths at
the source-union guard. This is source-grounded reachability reasoning; execution
of this new oracle remains pending CI.

Changed assertions:

1. Retained `error.to_string().contains("budget")`; additionally require exact
   `semantic_affected_owner_source_budget`. A generic error, invalid admission,
   another budget guard, or a successful/empty batch fails the test.
2. Retained exact whole-database `shared_state` equality immediately around the
   refusing `semantic_batch(64)` call, including every foreign/global table.
3. Replaced the old empty-Current zero-total-Catalog assertion under the
   Manager-approved populated-prior shift. After the 64-owner publication,
   capture all source-a `session_*` tables except
   `session_catalog_invalidations`, all `storage_proof_*` tables and
   `storage_semantic_publications`. Compare the exact ordered SQLite values
   after new ingest/begin and again after refusal. This covers the complete
   prior Current/History/source mappings, slots, trace/search, immutable proof
   generations/sources/references and publication witnesses. Invalidations are
   deliberately not in the prior-publication snapshot because new ingestion
   must add one; they remain in the whole-DB before/after refusal comparison.
4. Assert zero Catalog rows for the new `budget-64` owner. Existing owners must
   remain byte-identical rather than being erased to recover a zero-row result.
5. Compare every source-b namespace row with the snapshot after real foreign
   publication and the 64-owner publication. This is nonempty foreign coverage.
6. Removed the loop's `phase != Publication` assertion: the first post-begin
   batch must now return the exact source-budget error directly. Any returned
   batch, including Publication, fails `unwrap_err` before any model/publish
   call can occur for the 65th member. The first 64-member publication is an
   explicitly checked prerequisite, not forbidden partial publication.

No fabricated PreparedSources, Catalog/endpoints/slots, alternative algorithm,
test-only production branch or manually inserted ownership rows are used.
All previous tests are retained; this modifies only the one existing test.

## Revision and verification

Repository: `semon`; authorized branch:
`origin/task/477-raw-sql-backends-20261024`.

Verified clean local and actual remote base before editing:
`e4c13d3841d25ccb8e73aa012d8cffd520eb7a4a`.

Implementation commit: `b16695476a67304555792b811f846eb3c155099a`.
Parent: `e4c13d3841d25ccb8e73aa012d8cffd520eb7a4a`.
Pushed to `origin` (`https://github.com/onsager-ai/semon.git`); actual branch
`refs/heads/task/477-raw-sql-backends-20261024` verified by `ls-remote` at the full
implementation SHA above. A following handoff commit adds this file and only
collapses the local helper signature; the final full remote HEAD is recorded
in the Worker JSON delivery result.

Exact semantic diff: `git diff e4c13d3841d25ccb8e73aa012d8cffd520eb7a4a b16695476a67304555792b811f846eb3c155099a -- crates/semon-sessions/src/events/store/configured.rs`.

`git diff --check` passed. No local build, check, test, clippy, formatter,
lock generation, Node/typecheck, browser/image or corpus CPU work ran; no CI
polling or dispatch was performed. Thermal telemetry was absent at intake.
Stop CPU-bound work at >=85 C; absent telemetry prohibits starting it. Reads
and searches were limited to individual named files under 10 MB.

**Pending fresh Root-owned CI on the delivered revision:** all 70 configured
tests (including execution of this exact guard oracle), 32 SQLite tests,
3 journal tests, formatting, and reachability/results of the 90-group. Static
reasoning and whitespace checks do not establish test acceptance. Fresh Manager
audit/verdict is required; no historical green result is relabeled.

## Remaining full objective

Full #477 remains unfinished: D3/D4 independent review; incidents, selectors and
recovery; S3/S4; complete PostgreSQL parity/no-hidden-SQLite and remaining PG
work; bounded migration/catch-up/fenced cutover/rollback; Hub #313; cold timing;
full UI binding and applicable performance/security/browser/visual gates.
Source #484 / Hub #318, Hub #321, Sessions-default #279 and Source #483 remain
the #380 predecessor gate. No #477 main merge until #380 is complete. Current
Semon schema only; no historical Semon compatibility or upgrade ladder.
No merge, deployment, publication, live PostgreSQL or user migration was done.
