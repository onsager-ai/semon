# Analytics without live runtime observation

Status: Chief PM approved the design on 2026-11-24. Open an implementation PR
once the hp-code-2 materialize measurements are available.

## Problem

`semon analytics materialize` refreshes the live Catalog before draining the
analytics writer, on the first pass and every `--watch` pass.
[#735](https://github.com/onsager-ai/semon/pull/735) made each refresh reuse
one runtime capture so process churn would stop invalidating work in flight.
An older capture can still publish a removed Codex name after a newer refresh
observed its absence ([#736](https://github.com/onsager-ai/semon/issues/736)).

Four review rounds across #735 and [#737](https://github.com/onsager-ai/semon/pull/737)
ran into the same problem: ordering live captures and writes across producers.
The last attempt started the Viewer's live Catalog producer from materialize.
Its final review found four failures. The aggregate digest missed changed
process ancestry, owner qualification missed unheld Codex writer lock files,
runtime requalification held the SQLite writer lock during filesystem scans,
and two live producers could still restore a withdrawn name.
[#737 closing review](https://github.com/onsager-ai/semon/pull/737#issuecomment-7106590448)

## Analytics inputs

Native evidence enters through the Index. Its admitted bytes support records,
usage, tools and compactions, with source incarnations, committed prefixes and
proofs for evidence links. The [current SQLite adapter](../../crates/semon-sessions/src/origin_projection.rs)
joins Catalog identity and native lineage to matching activity and pricing
projections in a read snapshot. Catalog and activity generations qualify
those joins. `_gen` marks the separate analytics publication.

| Output | Required inputs |
| --- | --- |
| `fact_session` | Origin namespace, committed native session identity and start, source qualification, step coverage and exclusive usage, recorded pricing and transform versions. |
| `fact_step` | Committed session, turn and step identities, native anchors, times, usage and activity, committed repo identity, recorded rates and price version. |
| `fact_turn` | Native step grouping, timestamp bounds and step count. Initiator is currently unknown. |
| `fact_tool_call` | Admitted native call and result metadata attached to the step. |
| `fact_compaction` | Admitted native compaction event and record anchor, with a qualified step association when available. |
| `fact_label` | Durable categorical label assertions with target, author and taxonomy provenance. |
| `dim_session` | Committed harness, native identity and parent lineage, role, start, branch and repo. |
| `dim_machine`, `dim_harness`, `dim_repo` | Origin namespace, native harness and committed repository identity, respectively. |
| `dim_tool`, `dim_model`, `dim_date`, `dim_price` | Classified tool and model metadata, UTC dates from admitted timestamps, and recorded rates and versions, respectively. |

No published column consumes processes, PIDs, harness writer locks, runtime
files, Codex display names, or reported runs from `~/.claude.json`. Observed
parents are excluded, status stays unknown, and reported cost and unproven end
times stay null. A ledger-qualified native sidecar that proves identity or
lineage is source evidence. Warehouse queries must preserve that distinction
because Viewer parent selection can also consult runtime observations. Body
text stays origin-local and out of analytics and the Hub.
[#737 field audit](https://github.com/onsager-ai/semon/pull/737) · [ASI direction](asi-direction.md)

## Proposal

Materialize reads source-qualified, committed warehouse facts and identity from
one SQLite snapshot per input batch. Use committed source prefixes and a
warehouse change boundary for inventory and incremental work, including
replacement, withdrawal and late links. Keep the existing distinction between
pending, missing and admitted evidence so a partial scan cannot erase prior
facts. The analytics writer keeps its own store lock, checkpoints and publication
writes. It takes no runtime capture, starts no Catalog producer and writes no
Catalog or runtime metadata.

The [phase 2 producer](star-catalog-phase2.md#write-transactions) already writes
warehouse facts and native proofs with Index advancement. It supplies the
storage and parser path, but a warehouse read alone cannot discover new logs or
advance their committed prefixes. To preserve standalone use, reuse the Index's
native discovery, parsing and admission through a runtime-free entry point with
no semantic stage. On cold start, that entry point establishes the qualified
Index and storage binding and drains the initial authorized inventory. One-shot
materialize drains analytics through the resulting committed boundary. In watch
mode, each pass advances native ingestion in bounded batches and consumes the
new commits. A later append belongs to a later pass. Source admission and its
integrity and custody checks belong to the Index, even if the CLI invokes it
before running the analytics drain.

## Concurrent admission

The Viewer and materialize can admit the same growing source into the same
Index. `Ledger::resumes` checks the file identity and hashes of the consumed
prefix and tail before reusing it, unless the scanner already has qualified
append evidence. `SqliteStore::write_one_dirty` compares the committed `files`
ledger with the scanner's expected ledger under `BEGIN IMMEDIATE`. It writes the
new ledger, proofs and warehouse facts in that transaction. `commit_initial_files`
requires an absent ledger and matching file stat. A losing scan reads the
newer ledger before trying again. While Catalog publication still uses source
claims, `publish_catalog_with_writer` compares each claimed revision against
`session_catalog_invalidations` and acknowledges only the matching revision, so
old work cannot clear a newer claim. Phase 4 retires those semantic claims with
their consumers and keeps the Index ledger checks.
[Scanner and ledger](../../crates/semon-sessions/src/events.rs) · [Index transactions and claims](../../crates/semon-sessions/src/events/store.rs)

A pending scan never erases already-admitted facts. A losing transaction leaves
the committed facts visible, and `SessionChange::Pending` leaves the published
analytics rows in place. Neither the Viewer's Index admission nor materialize's
admission takes a runtime capture in this path. Process churn cannot invalidate
these batches, and there are no captured runtime values to reconcile or publish
out of order. This removes the failure class seen in #735 and #737 from
materialize.
[Analytics publication](../../crates/semon-asi/src/publish.rs)

The implementation PR must include `concurrent_admission_matches_single_run`.
Race two admission runs over the same growing native source using separate
processes or database connections. Assert that admitted facts increase
monotonically, stale work never overwrites newer data, both runs complete
without lock timeouts, and the analytics published afterward match a single-run
baseline over the same evidence.

## Phase 4

Make this the analytics slice of the [phase 4 cutover](star-catalog-phase3.md#phase-4).
It needs the phase 2 writer and phase 3 identity and source-resolution contracts,
then a warehouse input adapter with committed change tracking and native-only
lineage. Verify stable UIDs and evidence links, causal turns, shared/exclusive
and family totals, costs, and label provenance. Keep source qualification and
Index compare-and-swap checks. Remove the old stage and its generation fences
when the remaining CLI, MCP and export callers have moved.

## Decisions

Chief PM approved both choices on 2026-11-24.

- **Index admission.** Materialize invokes shared runtime-free admission. This
  preserves one-command cold start and fresh watch without a Viewer. A separately
  managed Index producer would give materialize a read-only process boundary and
  add setup and explicit stale-coverage handling.
- **Cutover.** Switch in phase 4. An earlier adapter over native evidence and
  committed identity and lineage would still need runtime-free admission and
  fact parity, then be replaced in phase 4. Collect the cold and warm one-shot
  timings and watch freshness measurements on hp-code-2 before implementation.

Close [#736](https://github.com/onsager-ai/semon/issues/736) after verifying that
neither materialize mode can capture or publish Codex names. Any stale-name
ordering issue reproduced between other Catalog writers belongs to the Catalog.
