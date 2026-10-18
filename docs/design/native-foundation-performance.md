# Native foundation first-release performance contract

Issue [#380](https://github.com/onsager-ai/semon/issues/380) separates usable native
reads from complete historical indexing. These are provisional engineering
budgets, not claims of demonstrated production capability.

| Scenario | Budget | Priority |
| --- | --- | --- |
| Warm restart with an existing qualified Catalog | 2 s to usable | P0 |
| Initial ingestion, recent sessions browsable | 5 s | P0 |
| Focused query for a known, explicitly selected session | 2 s without global indexing | P0 |
| Qualified Push/append | Work independent of unrelated history width | P0 |
| Complete cold indexing with 2,048 unrelated sessions | 30 s in required CI | P1 |

## Interactive readiness

Start the clock before admitting the existing producer or opening the replacement
native evidence owner. Stop after a successful native response with expected
sessions/content and generation fences. Listener startup, queue admission,
Index writes and an empty response are not usability witnesses.

`viewer::tests::initial_catalog_page_precedes_global_native_observation` uses the
existing single-worker pool, 60 recent sessions and 0/256/2,048 older sessions.
It witnesses the recent page at first publication, then holds subsequent work.
Foreground Catalog and explicitly source-selected `read_transcript` calls must
return expected content within the existing 2 s read bound while the worker is
held. This debug fixture covers backend readiness, not browser first paint or
cold filesystem caches. Its first-page bound stays tighter than the proposed 5 s.

Partial pages report `completeness.state=partial` and `freshness=updating`.
Published source bytes can be verified and browsed; discovery is not yet proof
of unique ownership. Identity remains `Provisional`, with no qualified single
`native_id`. Control selection and `source_projection_ready` (archive eviction)
must refuse qualification. Explicit source selection scopes a read; it grants no
control authority. Unscoped multi-machine native ID resolution cannot assume a
partial collection excludes duplicates. Existing duplicate-owner and incomplete
relationship tests remain gates.

`catalog::tests::journal_append_and_warm_restart_ignore_unrelated_history`
measures replacement-owner initialization, persisted refresh and a successful
Catalog response as warm restart to usable, separately from append publication.
This is a process-warm debug fixture, not a fresh production process measurement.

## Incremental efficiency

Keep the selected qualified append identical at 0/256/2,048 unrelated sessions.
Require one loaded Index, one parsed record, zero discovered paths, one derived
source, one published row, no full historical-prefix hashing and no resident
Index payload after publication. Logical reads, parsing and source hashing must
equal the first size's work. Fresh owners consume persisted revisions without
rebuilding unrelated history. Existing interrupted-intent, continuation-proof,
generation/Raw fencing, semantic component, Archive/Unarchive identity and
no-duplicate-transcript oracles remain required.

Work counters establish bounded selected work, not constant whole-store size or
process RSS. Initial discovery, oversized components and metadata-only audits
retain their own bounds and refusal behavior. Worker counts and released payloads
alone cannot establish a memory guarantee.

## Progressive correctness and full completion

Use the existing Event Index journals, semantic queues and publication CAS.
Global completeness requires completed discovery, acknowledged exact source
revisions, retired active semantic stages after indexing/identity/relationship/
owner/ACK work, and final runtime/source/publication witnesses. Unreachable stage
garbage collection differs from pending semantic work. Partial publication
cannot acknowledge unrelated revisions. No parallel indexing architecture or
new compatibility model is permitted.

Hub's `committed_append_and_known_archive_ignore_unrelated_native_headers` keeps
the original 0/256/2,048 fixture, including the selected source (2,049 total in
the largest case). Its clock starts before `observer.demand()`. The strict
`source_projection_ready` witness remains. Success additionally requires every
file and current Catalog row, zero invalidations, zero active semantic stages and
complete Catalog/backfill markers. Timeout fails with indexed/pending counts;
partial publication cannot satisfy completion. Original append, native header,
Archive, source-custody and cleanup assertions remain.

## Measurement and optimization policy

Required CI runs focused fixtures with `SEMON_BUILD_TIMING=0`, then retains the
existing instrumented full suite. Record exact Source/Hub heads and tested merge
trees; report the two executions separately. Cold completion emits latency,
indexed/published sources, pending revisions, active stages and throughput.
The user removed the 10 s objective on 2026-10-14; it is neither a gate nor a
follow-up tuning target. Existing process I/O deltas and Index/WAL sizes remain.
Process `VmHWM` includes fixture setup and other tests; kernel counters and WAL
extents are not device I/O or producer live heap.

Retire the 10 s global-completion release gate because it conflates interactive
availability with asynchronous historical work. Repeated required-CI failures
did not demonstrate unusable already-published reads. The provisional 30 s gate
still requires complete indexing under identical resources. It does not relax
source integrity, relationships, cancellation, restart, retention, authorization
or durability. Preserve measured latency and the
[producer measurements](scoped-catalog-producer.md), without a target-specific
10 s indicator.

Further SQLite/cache/WAL or statement tuning requires measured evidence affecting
interactive responsiveness, incremental complexity, resource bounds,
correctness/durability or the 30 s budget. Expand diagnostics only for a specific
unresolved question.

Physical/clipped query windows, alias/virtual/composite-history preparation,
cold identity/uniqueness, received-copy/composer authority and final
native/browser/PG/SSH/image qualification remain
separate correctness and release questions. Passing this contract does not close
them. Exact-pair results and remaining blockers belong in #380 and its integration
PRs. Identify production-representative measurements separately from synthetic
debug fixtures and instrumented CI results.
