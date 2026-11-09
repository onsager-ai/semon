# Agent Session Intelligence direction

Status: accepted direction (2026-11-08; decisions extended the same day; architecture amended 2026-11-09). This document supersedes earlier product
focus statements wherever they conflict.

## Decision

Semon's product focus is **Agent Session Intelligence (ASI)**: durable storage,
query and analytics over harness-native agent sessions. Session display and
two-way harness control are no longer product goals. T3 Code covers them, and
Semon should be a backend that T3 Code and other agents query, not a competing
front end.

The method (data model, storage schema, measurement layers, labels, ledgers,
evidence grades and interventions) is defined in the
[ASI methodology](asi-methodology.md). This document owns product scope and
sequencing. Progress is tracked in the pinned roadmap issue, #549.

## Pillars

1. **Storage.** A durable, multi-machine store of normalized session events:
   sessions, turns, tool calls and results, errors, tokens, cost, waits,
   handoffs and lineage, bodies included. Native logs remain the source of truth;
   the store is derived and rebuildable, respecting source availability, custody
   and redaction. Push/receive is the collection path across machines.
2. **Query.** A documented schema served as read-only SQL, full-text search and
   the existing focused reads, exposed equally through MCP/CLI for agents and an
   analytics UI for humans.
3. **Analytics.** Long-horizon aggregates: spend by repo/model/harness over time,
   tool failure and retry rates, human wait and intervention time, subagent fan-out,
   stall patterns. Later: derived summaries and outcome classification as a
   separate, attributable layer.

## Decisions

| Area | Decision |
| --- | --- |
| Storage engine | SQLite for ingest (the Session Event Index), durable labels and FTS5. DuckDB over Parquet gold partitions for analytics and the public SQL dialect, from day 1 (amended 2026-11-09). |
| PostgreSQL track (#477) | Paused. The merged code stays behind its feature flag with its CI workflow. The Raw-plus-projection-cache model, namespace-bound storage port and runtime custody owners carry over to the SQLite ASI store. |
| Hub | Hub is hosted Semon only: multi-tenant tenancy, workspace authorization and hosted views over Semon. ASI logic, schema and analytics live in Semon. Hub receives body-free gold partitions and categorical labels only, never Raw or session bodies, and runs no L1 model classification. Each tenant workspace has its own gold store. |
| Session bodies off-machine | Redacted Raw goes to the operator's own receiver by default (amended 2026-11-09). It never goes to Hub. Transparency comes from open, validated code, and redaction is best-effort. |
| Semantic labels (L1) | In v1. Labels follow methodology §3–§4 (rows with labeller, taxonomy version, confidence and evidence step IDs). Classifiers run where the bodies are: the receiver, or the machine itself when it has none. Sending evidence to a model provider needs its own per-source opt-in that names the provider and model; pushing to a receiver does not grant it. Categorical labels travel as metadata; free-text label fields (goal, mechanism) count as bodies. |
| Analytics UI spec | Adopt the session activity viewer spec (semon#532) as the analytics UI direction. |
| Consumers | Agents (MCP/SQL) and humans (dashboards) equally, over one shared schema. |
| Viewer | Shrink to an analytics UI: keep the authenticated server, shell and security boundary; drop transcript-browsing polish. Pixel/visual comparison is removed from CI by semon#536/#537. New UI work is dashboards and query results. |
| Control, runtime, SSH | Retired in step 2: `semon-control`, `semon-runtime` and `semon-ssh` code and active entry points are removed. Push/receive over TLS covers multi-machine collection. Retirement deletes no user data, credentials, history or installed services. |

## Architecture

Decided 2026-11-08 and amended 2026-11-09 (central derivation inside the
operator's trust boundary, and DuckDB gold). The specs (#544 storage, #545
query, #546 L1 labels) own the details.

- **Medallion layers.**
  - Bronze is admitted Raw: native logs, plus their redacted mirrors on a
    receiver, kept immutably with redaction provenance.
  - Silver is the per-namespace ingest index (today's Session Event Index,
    with the activity projection). It is internal and rebuildable from bronze.
  - Gold is the public dimensional model: Kimball facts and dimensions over
    methodology §1.1, held as Parquet partitions and queried with DuckDB. Gold
    is rebuildable from silver and is the versioned public contract.
  - State that can't be rebuilt (labels, adjudications, links, work items,
    interventions) lives in a separate durable SQLite file.
- **Central derivation inside the operator's trust boundary.**
  - Machines push redacted Raw to the operator's own receiver through the
    mirror path, and the receiver derives silver and gold for every machine.
  - A single machine runs the same pipeline locally.
  - Exactly one store derives each source's gold. Re-derivation after a
    rules, price or transform change needs only the receiver's bronze.
- **Hub gets gold only.**
  - A receiver (or a single machine) exports body-free gold partitions and
    categorical labels to Hub.
  - File paths cross to Hub only as HMAC tokens keyed per operator
    workspace. The receiver holds the key and Hub never does.
  - Body columns never cross.
- **Raw archive.** Bronze is kept immutably, including bytes a source rewrite
  replaces, and indefinitely by default. That keeps gold re-derivable after a
  harness prunes its logs.
- **Versioning.** Every gold row records its transform, rules and price
  versions. A bump re-derives only the affected partitions. Labels are
  append-only: a new taxonomy or labeller adds rows, and views pick the
  current ones.
- **Query reach.**
  - Read-only DuckDB SQL over gold, sandboxed: no external access or
    extensions, locked configuration, memory, thread and time limits.
  - FTS5 search over silver bodies.
  - Both through MCP/CLI on the machine, through the authenticated Viewer
    server for its own store, and through Hub per tenant (gold only). There is
    no separate network SQL endpoint.
- **Performance from day 1.** Ingest, freshness, dashboard, group-by and
  search budgets are measured in CI on synthetic data at 1M, 10M and 100M rows.
- **Crates.** A `semon-asi` library crate (gold schema, transforms, partition
  store and query engine) is split out of `semon-sessions` as part of #544.
  `semon-sessions` keeps native parsing and the ingest index. Hub depends on
  `semon-asi`.

## Unchanged invariants

- Native session identity, lineage and provenance semantics.
- Source integrity, authorization and custody; push-side redaction.
- Harness parser behavior and sanitized fixtures.
- Pre-release compatibility policy: evolve contracts directly, without
  migration layers.

## Sequencing

1. Adopt this direction in README.md and AGENTS.md; update
   `session-foundation-retirement.md` so the temporary pauses become retirements
   (landed in semon#541).
2. Retire control/runtime/SSH code and their CI jobs (landed in semon#541).
   The browser, design-document and Hub Session display/control/runtime follow-ups
   remain in step 6.
3. Specify the ASI store: medallion layers, the gold dimensional model over
   methodology §1.1, central derivation on the receiver, retention beyond the
   current 60-day Analytics window, multi-machine keys and Hub gold export. Start from
   the #477 carry-over: https://github.com/onsager-ai/semon/issues/477#issuecomment-6719032404.
4. Ship the query surface: read-only SQL plus FTS through MCP/CLI, replacing the
   paused `find`.
5. Rebuild Analytics on durable storage; shrink the Viewer into the analytics UI.
   Pixel baselines and visual tooling are removed by semon#536/#537 (Source) and
   semon-hub#377 (Hub).
6. Hub follow-ups: shrink browser suites that exercise display/control/runtime,
   delete `ui/ssh.mjs` and reconcile viewer/session redesign docs with this
   direction. These follow-ups remain separate from the landed Semon retirement.
