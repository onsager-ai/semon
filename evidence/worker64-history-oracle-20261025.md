# Worker64: late-child History oracle

This is a bounded test-oracle repair.  It does not accept Worker61, the
lifecycle slice, or the full #477 migration.  Fresh Gateway-owned CI and a
fresh Manager review remain required.

## Classification

The prior `session_history_catalog == 4` assertion was an oracle defect, not
evidence of a producer defect.  `publish_history_rows` treats History as
retained custody:

* a completed original is unchanged (`storage/catalog.rs:462-466`);
* an old row from another harness retains its sources after the earlier
  original-custody preflight (`472-482`);
* for a same-harness row, only old source paths absent from the current row and
  not relocated produce a composite History row (`486-511`);
* otherwise its History is cleared (`494-496`).

The domain61 fixture has an empty History set after the first all-Codex
three-owner publication (now explicitly asserted at
`events/store/configured.rs:7076-7087`).  Its four final Current rows are
still all Codex.  The expected History map is now derived for each final key
from the captured pre-incident Current metadata and final ordinary producer
model (`7167-7216`): an unchanged-source key removes any History entry; a
disappeared, unrelocated source produces the exact composite metadata with
Query removed and deduplicated native IDs.  The existing CI observation was
one History row, not four; the strengthened test retains that exact expected
cardinality only after the derived-map comparison (`7224-7239`).  It further
requires the retained row to contain a genuinely disappeared old source and
to retain a History projection header and inspectable slot prefix.  No History
rows, Catalog rows, endpoints, projections or proofs are fabricated.

The changed oracle retains the required Current count of 4, three current
children of `late-0`, four current proof refs, foreign namespace equality,
held-model `Conflict` with zero effects, reset-fault atomicity, indexed late
child discovery, and no intermediate loss of prior publication state.

## Late-reset audit (read-only)

1. On an admitted non-indexing journal advance, the re-entry condition reads
   the journal revision or unindexed source state (`storage/sqlite/semantic.rs:1545-1563`),
   calls `reset`, deletes the durable model witness, commits them together,
   and returns a new Indexing epoch (`1563-1594`).  `reset` increments the
   durable profile epoch and fences/clears staging state (`203-206`).  The
   test's held-model conflict and whole-state fault rollback stay in place at
   `7104-7122`.
2. A finite advance is bounded: `reset_page` deletes/re-enrols at most 64
   units at a time (`209-247`), and, when complete, stores the current source
   journal revision and clears `resetting` (`249-250`).  The next stable batch
   therefore fails the `revision <> current` predicate at `1552-1556` and
   proceeds through normal indexing/frontier work (`1599-1625`).  No
   one-finite-advance livelock was found.
3. Capacity remains typed: more than 64 indexed sources or withdrawals returns
   the corresponding budget refusal (`253-280`).  Ledger, claim or withdrawal
   mismatch returns `ReductionWitness::Changed` (`282-299`), whose consumers
   retain their existing `Conflict` treatment rather than accepting a budget
   error.
4. A foreign/unrelated namespace does not alter this namespace's journal
   revision query.  An unrelated path in the same namespace is conservative:
   it can cause one reset for each new revision, but a stable finite revision
   is captured at reset completion.  Continuous same-namespace appends can
   repeatedly fence work; this remains an open liveness limitation, not a
   proven defect in the bounded single-advance path, and no redesign was made.
5. The reset occurs before later durable publication phases.  The new test
   keeps the injected reset-effect whole-database equality check (`7104-7108`),
   and source inspection shows the reset transaction commits before any
   `publish_history_rows`/Current effects; those begin later in the publisher
   (`semantic.rs:944-1000`).  No partial Current, History or GC effect was
   introduced by this repair.

## Scope and contacts

Changed only:

* `crates/semon-sessions/src/events/store/configured.rs` — domain61
  real-producer test oracle and a local `BTreeSet` import.
* this evidence file.

Read-only contacts: `storage/catalog.rs` (History rule),
`storage/sqlite/history.rs` (History store), and
`storage/sqlite/semantic.rs` (Worker61 reset/re-entry and publication call).
`model.rs`, relation staging, proof generation, PG-owned source/receipt work,
S2 selected-facts/history work, source journal, UI, Cargo, Hub and #380 were
not edited.

## Verification and delivery

Thermal telemetry remained unavailable; no CPU-bound work, local Cargo build,
check, test, clippy, formatting, Node/typecheck, browser work or CI polling
ran. `git diff --check` passed.  Required Gateway CI on the final SHA remains:
73 configured tests, the 90-group, 32 SQLite, 3 journal and formatting.

Pending full scope: native incident candidates and cross-harness keys,
nonprimitive selectors/frontier, Facts-less local authority, recovery, S3/S4,
full PostgreSQL parity, migration/cutover, Hub #313, cold timing, full UI
binding, and the unfinished #380 predecessor gate.  #477 main merge remains
held.
