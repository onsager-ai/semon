# Agent Session Intelligence direction

Status: accepted direction (2026-11-08; decisions extended the same day). This document supersedes earlier product
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
| Storage engine | Extend the SQLite Session Event Index first (event/content tables, FTS5). Design the schema so it can be exported to a columnar store (Parquet/DuckDB) once analytical volume calls for it. |
| PostgreSQL track (#477) | Paused. The merged code stays behind its feature flag with its CI workflow. The Raw-plus-projection-cache model, namespace-bound storage port and runtime custody owners carry over to the SQLite ASI store. |
| Hub | Hub is hosted Semon only: multi-tenant tenancy, workspace authorization and hosted views over Semon. ASI logic, schema and analytics live in Semon. Hub receives metadata rows and labels only, never session bodies, and runs no L1 model classification. Each tenant workspace gets its own ASI database. |
| Session bodies off-machine | Opt-in per source. Without opt-in, only metadata and aggregates leave the machine (see [Architecture](#architecture)). Opted-in bodies are redacted by default before they leave. |
| Semantic labels (L1) | In v1. Labels follow methodology §3–§4 (rows with labeller, taxonomy version, confidence and evidence step IDs). Classifiers run where the bodies are, on the origin machine by default. Sending evidence to a model provider needs its own per-source opt-in that names the provider and model; the bodies opt-in does not grant it. Categorical labels travel as metadata; free-text label fields (goal, mechanism) count as bodies. |
| Analytics UI spec | Adopt the session activity viewer spec (semon#532) as the analytics UI direction. |
| Consumers | Agents (MCP/SQL) and humans (dashboards) equally, over one shared schema. |
| Viewer | Shrink to an analytics UI: keep the authenticated server, shell and security boundary; drop transcript-browsing polish. Pixel/visual comparison is removed from CI by semon#536/#537. New UI work is dashboards and query results. |
| Control, runtime, SSH | Retire `semon-control`, `semon-runtime` and `semon-ssh` code. Push/receive over TLS covers multi-machine collection. Retirement deletes no user data, credentials, history or installed services. |

## Architecture

Decided 2026-11-08. The specs (#544 storage, #545 query, #546 L1 labels) own the
details.

- **Layers.** Native Raw, then the per-namespace ingest index (today's Session
  Event Index; internal and rebuildable), then the public ASI projection, then
  query. The public projection is the methodology §1.1 tables, held in typed
  tables in their own database and read-only to queries. Internal ingest tables
  may change freely; the public schema is the versioned contract. State that
  can't be rebuilt (labels, adjudications, links, work items, interventions)
  lives in a separate durable file that derived-cache rebuilds never touch.
- **Metadata push by default.** The origin machine derives the L0 projection
  (sessions, turns, steps, cost, compactions) and pushes typed rows, not Raw.
  Rows carry the origin's projector, rules and price versions, and the Raw
  prefix digest they came from. The receiver validates rows against the schema,
  stores them in the pushing machine's namespace, and asks for a re-push when
  versions move; it never repairs rows in place. Repo, branch, model and tool
  names cross in plain text. File paths cross only as keyed HMAC tokens; the key
  is shared by an operator's workspace and is never held by a receiver or Hub.
  Message, command, tool-output and other body text never crosses without
  opt-in. Opted-in sources also send redacted Raw through the mirror path. That
  Raw feeds only body tables, FTS5 and L1 evidence packets; the ASI rows still
  come from the origin.
- **Raw archive.** The machine with custody of the bodies (the origin, or a
  receiver for opted-in sources) keeps compressed, sealed Raw under explicit
  retention, so derived layers can be re-derived when rules, prices or the
  projector change. Derived rows older than the archive are frozen History,
  marked with the versions they were computed under.
- **Versioning.** Every derived row records its projector, rules and price
  versions. A version bump re-derives only the affected rows. Labels are
  append-only: a new taxonomy or labeller adds rows, and views pick the current
  ones.
- **Query reach.** Read-only SQL and FTS5 search are available through MCP/CLI
  on the machine, through the authenticated Viewer server for its own
  namespaces, and through Hub per tenant. There is no separate network SQL
  endpoint.
- **Crates.** A `semon-asi` library crate (public schema, projection store and
  query engine) is split out of `semon-sessions` as part of #544. `semon-sessions`
  keeps native parsing and the ingest index. Hub depends on `semon-asi`.

## Unchanged invariants

- Native session identity, lineage and provenance semantics.
- Source integrity, authorization and custody; push-side redaction.
- Harness parser behavior and sanitized fixtures.
- Pre-release compatibility policy: evolve contracts directly, without
  migration layers.

## Sequencing

1. Adopt this direction in README.md and AGENTS.md; update
   `session-foundation-retirement.md` so the temporary pauses become retirements.
2. Retire control/runtime/SSH code and their CI jobs.
3. Specify the ASI storage schema by adopting methodology §1.1 in SQLite
   (normalized events and content, FTS5, retention beyond the current 60-day
   Analytics window, multi-machine keys, body opt-in and redaction). Start from
   the #477 carry-over: https://github.com/onsager-ai/semon/issues/477#issuecomment-6719032404.
4. Ship the query surface: read-only SQL plus FTS through MCP/CLI, replacing the
   paused `find`.
5. Rebuild Analytics on durable storage; shrink the Viewer into the analytics UI.
   Pixel baselines and visual tooling are removed by semon#536/#537 (Source) and
   semon-hub#377 (Hub).
6. Hub follow-ups: shrink browser suites that exercise display/control/runtime,
   delete paused `ui/ssh.mjs` with the SSH retirement, and reconcile the
   viewer/session redesign docs with this direction.
