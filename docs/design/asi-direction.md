# Agent Session Intelligence direction

Status: accepted direction (2026-11-08). This document supersedes earlier product
focus statements wherever they conflict.

## Decision

Semon's product focus is **Agent Session Intelligence (ASI)**: durable storage,
query and analytics over harness-native agent sessions. Session display and
two-way harness control are no longer product goals. T3 Code covers them, and
Semon should be a backend that T3 Code and other agents query, not a competing
front end.

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
| Consumers | Agents (MCP/SQL) and humans (dashboards) equally, over one shared schema. |
| Viewer | Shrink to an analytics UI: keep the authenticated server, shell and security boundary; drop transcript-browsing polish and the pixel baselines that only cover it. New UI work is dashboards and query results. |
| Control, runtime, SSH | Retire `semon-control`, `semon-runtime` and `semon-ssh` code. Push/receive over TLS covers multi-machine collection. Retirement deletes no user data, credentials, history or installed services. |

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
3. Specify the ASI storage schema (normalized events and content, FTS5,
   retention beyond the current 60-day Analytics window, multi-machine keys).
4. Ship the query surface: read-only SQL plus FTS through MCP/CLI, replacing the
   paused `find`.
5. Rebuild Analytics on durable storage; shrink the Viewer into the analytics UI
   and prune the obsolete baselines.
