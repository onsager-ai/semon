# Worker61: bounded late-incident re-admission

SOL_USAGE_UNKNOWN_BOUNDED_EXCEPTION. One Manager-approved attempt, no counter
reset or automatic retry. Implementation is delivered for fresh Gateway-owned
CI and Manager review; no execution pass or full #477 acceptance is claimed.

## Intake and revision

Actual repository: `/home/marvin/Documents/Codex/2026-10-24/session-url-semon-484-hub-318/semon`.
Branch: `origin/task/477-raw-sql-backends-20261024`.
Verified clean local HEAD and actual `ls-remote` base before editing:
`e11454d5c334847f13d59a359134abd83d1327a3`.

Worker60 start prerequisite was checked read-only: the selected task's existing
`state.json` reported status `running`, request `selected60-repair-001`, phase
`model`. No controller or peer process was invoked or contacted.

WIP implementation commit `530e36dd08371848c9144b00020e9df8faf9147b`, parent
`e11454d5c334847f13d59a359134abd83d1327a3`, was pushed within 15 minutes.
`ls-remote origin refs/heads/task/477-raw-sql-backends-20261024` independently
returned that full SHA. The final evidence commit follows it normally; the
Worker result records the verified final remote SHA. Author and committer are
Marvin Zhang <3393101+tikazyq@users.noreply.github.com>.

## D-1: capacity is distinct from changed authority

Before: `storage/sqlite/semantic.rs:253` returned `bool` for both capacity and
claim/ledger/withdrawal-authority validity. All callers treated false as a
generic Conflict. After: `ReductionWitness` at lines 253-299 distinguishes
Current, Changed, SourceBudget and WithdrawalBudget. Its conversion returns
the exact `semantic_component_source_budget` or
`semantic_component_withdrawal_budget` refusal for capacity. Changed remains
false and follows the callers' existing Conflict behavior.

All four consumers use that conversion: publication at line 728, batch at
1506, publication-model record at 1740, and identity/relation record at 1806
(the latter is a shared consumer). No bound increased. Input admission,
affected-owner source union, owner count, native-key count and endpoint budgets
retain their independent guards and names. In particular, the existing
fixture58 test and its exact `semantic_affected_owner_source_budget` oracle are
unchanged.

## D-2: invalidate and rediscover through the existing producer

Before, the non-indexing old-owner source union at old line 1515 permanently
refused additional paths with `semantic_affected_owner_late_incident_pending`.
There is a necessary correction to the proposed discovery premise: an ingested
new child has no Current owner yet. `Owners.affected` therefore cannot discover
it solely by traversing the previously published Current graph. Replacing only
that refusal would leave the requested real-child test blocked or omit the
child. Completed header/dependency frontiers also need to be revisited.

The localized fix at `semantic.rs:1533-1592` handles this within the same batch
transaction. After the unchanged ledger/claim witness and bounded Current
closure, it captures real source-journal claims and restarts when a non-indexing
stage sees an added closure path, an unindexed admitted path, or an advance of
the namespace's source-journal revision. It calls the existing `reset` at line
203 and uses the existing 64-work-unit `reset_page` at line 209 on following
batches. That clears derived staging inputs/endpoints/frontiers and regenerates
the existing native header/dependency discovery from real indexed sources.
It does not implement another parser, owner reducer or projection algorithm.

The reset increments the durable epoch, explicitly deletes the old
`storage_semantic_model_batches` witness, and returns an empty Indexing batch.
The in-memory epoch changes only after commit. A fault before commit rolls
back enrollment, epoch and model invalidation together. Held prepared models
then fail the existing epoch checks before publication effects. Published
Current/History/slots/proofs and Raw are untouched by the reset. No proof
generation, schema or authority contract changed.

Re-admission stays bounded by the original 64-source/64-owner closure, 1 MiB
owner metadata and paged discovery/reset limits. A newly discovered component
can still refuse a real capacity guard; it never publishes a truncated cohort.
An unrelated source-journal advance conservatively causes one bounded reset
as well. The existing reset completion captures the new journal revision;
there is no automatic retry loop or increased component budget.

Residual refusal: line 1539 retains `semantic_affected_owner_late_incident_pending`
for an additional recorded-closure path whose Raw carrier is missing, cannot
be inspected, or is not a file. Existing staged negative observations keep
their existing authority rules. Real claims come from `source_journal::claims`;
`None` is a legitimate already-settled claim, not unavailable authority. SQL
claim-read or epoch-reset failures propagate their existing storage errors
and roll back, rather than fabricating claims or treating failure as absence.

## Added real-producer oracles (not executed locally)

All three tests are appended inside the existing domain52 test module in
`events/store/configured.rs`. Existing tests and assertions are unchanged.

1. `configured_semantic_domain61_late_child_restarts_complete_native_cohort`
   (line 6990): publish a real foreign Claude owner; write/ingest/publish a
   Codex parent with two children; stage only the parent and reach a fully
   prepared three-owner publication through the actual typed model. Then
   write/ingest a fourth Raw source whose native parent is the published parent.
   The next batch must reach the reset fault hook with whole-database rollback,
   then return Indexing under a strictly newer epoch with no durable old model
   witness. Publishing the held model must return Conflict with zero effects.
   Every subsequent batch must preserve the exact prior Current/History/slots/
   proof/publication state and all foreign rows until final publication.
   Header discovery must actually index the new child. The completed four-owner
   model must equal the ordinary native oracle for every Catalog field,
   relationship field, slots/repair, PIDs and lineage dependencies. Final
   Current and History have all four owners, the parent has three Current
   children, and Current proof references cover all four. Publication uses the
   existing fault rollback and exact same-ACK oracle.
2. `configured_semantic_domain61_progressive_index_overflow_is_capacity_not_conflict`
   (line 7085): use the existing admitted primitive staging fixture and real
   Raw ingestion/model indexing, with 65 duplicate rollouts of a single native
   identity. Begin with one legal input. Real header-id discovery enrolls the
   rest; no Catalog, endpoint, dependency or PreparedSources payload is manually
   populated. This profile is required to reach the indexed-source guard: the
   Current-owner profile honestly refuses the separate source-union guard
   earlier. Require exactly 65 indexed source rows, the exact
   `semantic_component_source_budget` error, whole failed-transaction equality,
   and exact prior publication-state equality. Prior publication in this
   primitive-only fixture is empty; the unchanged fixture58 and the late-child
   test separately cover nonempty prior custody and foreign publication.
3. `configured_semantic_domain61_real_ledger_change_remains_conflict`
   (line 7135): hold a real prepared model, rewrite/ingest that same Raw path,
   and require the original Conflict display plus exact whole-database equality.
   A capacity refusal must not absorb a genuine ledger/claim change.

The first test uses the existing ordinary `prepare_sources` oracle, with only
its established configured runtime normalization. No fabricated projection,
negative observation, endpoint or row was added. The overflow test uses the
existing schema-only primitive fixture setup; its semantic source rows are
produced exclusively through ingestion, discovery and the shared typed model.

## Ownership and merge contacts

Only these source files changed:

- `storage/sqlite/semantic.rs`: witness classification and its existing
  publish/record consumers; the bounded closure enrollment/reset block in batch;
  one new rollback fault hook. Existing `reset`/`reset_page` are called unchanged.
  This is a shared-file contact, so integrate these exact hunks without replacing
  PG59 or S2-60 changes on their branches. No proof generation/presentation,
  Snapshot hook, selected renderer or runtime-facts seam was edited.
- `events/store/configured.rs`: three appended tests only, inside domain52.
  Production NativeOwnership, SourceObservationStore and pending reconciliation
  are unchanged. Existing domain52/fixture58 bodies are unchanged.
- `model.rs` and `model/relation_stage.rs`: read-only contacts to existing typed
  preparation, LifecycleAuthority and bounded closure. No edits.
- No `semantic_owners.rs`, source_journal, PG, Cargo, UI, perf/cold, Viewer,
  Hub, #380, AGENTS or skill edits. No other worktree or out/ writes.

Exact implementation diff:
`git diff e11454d5c334847f13d59a359134abd83d1327a3 530e36dd08371848c9144b00020e9df8faf9147b -- crates/semon-sessions/src/storage/sqlite/semantic.rs crates/semon-sessions/src/events/store/configured.rs`.
The same reviewable patch is retained at
`evidence/worker61-late-incident.diff.patch`.

## Verification and remaining objective

`git diff --check` passed. Full compile, formatting and execution are unverified.
No local cargo build/check/test/clippy/fmt/update/lock generation, Node checks,
typechecks, formatter, browser, image or corpus work ran. No CI polling or
dispatch ran. `/home/marvin/.cache/thermal-watch/temps.log` was absent at intake
and after editing. Stop CPU-bound work at >=85 C; absent telemetry prohibits
starting it. Reads/searches were bounded to named files under 10 MB.

Fresh Gateway CI must run on the final delivered SHA: all 70 prior plus 3 new
configured tests, the 90 catalog/storage group, 32 SQLite and 3 journal tests,
query/facts groups and format, plus applicable aggregate Rust gates. The
existing base 0095 green report is historical input only and is not this
revision's evidence. Fresh Manager audit must confirm the real producer
reachability and rollback assertions above; static reasoning is insufficient.

Full #477 remains unfinished: local authority without Facts; native incident
candidates and cross-harness keys; nonprimitive selectors/frontier; recovery;
S3/S4; full PostgreSQL parity and no-hidden-SQLite; supported-current-store
migration, non-rebuildable facts/retained custody, catch-up, exclusive fenced
cutover and qualified rollback; Hub #313/RLS; cold timing; full UI binding and
performance/security/browser/visual gates. Current Semon schema only; no
historical schema compatibility or upgrade ladder. Source #484 / Hub #318,
Hub #321, Sessions-default #279 and Source #483 remain the #380 predecessor
completion gate. #380 is not complete; #477 main merge remains held. No merge,
deployment, publication, live PG, user-data migration or deletion was done.
