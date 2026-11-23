# Analytics without live runtime observation

Status: proposed for review. Open an implementation PR only after this design
has been reviewed and the hp-code-2 materialize measurements are available.

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

## Phase 4 and decisions

Make this the analytics slice of the [phase 4 cutover](star-catalog-phase3.md#phase-4).
It needs the phase 2 writer and phase 3 identity and source-resolution contracts,
then a warehouse input adapter with committed change tracking and native-only
lineage. Verify stable UIDs and evidence links, causal turns, shared/exclusive
and family totals, costs, and label provenance. Keep source qualification and
Index compare-and-swap checks. Remove the old stage and its generation fences
when the remaining CLI, MCP and export callers have moved.

- **Who advances the Index?** Prefer standalone materialize invoking shared
  runtime-free admission. This preserves one-command cold start and fresh watch
  without a Viewer. A read-only command backed by a separately managed Index
  producer gives a cleaner process boundary, but adds setup and must explicitly
  report stale or missing coverage when that producer is absent.
- **When to switch?** Prefer phase 4. If it slips and the hp-code-2 measurements
  justify an earlier cut, a runtime-free adapter over native evidence and
  committed identity and lineage could ship first. That still needs independent
  admission and fact parity, and adds an adapter that phase 4 would replace.
  Review the design and compare cold and warm one-shot time and watch freshness
  on hp-code-2 before choosing.

Close [#736](https://github.com/onsager-ai/semon/issues/736) after verifying that
neither materialize mode can capture or publish Codex names. Any stale-name
ordering issue reproduced between other Catalog writers belongs to the Catalog.
