# Agent Session Intelligence direction

Status: accepted direction (2026-11-08; Viewer scope clarified 2026-11-12;
centralized default selected by the owner **2026-11-14**).
[Centralized ASI](asi-centralized.md) is the current architecture contract for one
owner's own machines. It supersedes federation D1/D2/D3/D6/D8 and the P1 raw-replica
exclusion; D4/D5 and the Hub boundary remain. Acceptance specifies the target;
it does not claim receiver ASI integration has shipped.

## Decision

Semon's product focus is **Agent Session Intelligence (ASI)**: durable storage,
query and analytics over harness-native agent sessions. Two-way harness control
is not a product goal; T3 Code covers it. Semon is a backend that T3 Code and
other agents query, not a competing control front end. Analytics is the core,
and read-only viewing of the raw session logs is its foundation: analytics
evidence links into it, so it is not optional polish. Semon therefore keeps
standalone, read-only browsing of the history it stores (amended 2026-11-09,
see the Viewer decision), so it never depends on T3 Code.

The method (data model, storage schema, measurement layers, labels, ledgers,
evidence grades and interventions) is defined in the
[ASI methodology](asi-methodology.md). This document owns product scope and
sequencing. Progress is tracked in the pinned roadmap issue, #549.

## Pillars

1. **Storage.** Durable native session history across one owner's machines.
   Origins push selected redacted logs through the existing mirror. The receiver
   indexes them, materializes ASI and serves retained transcripts locally under
   its own read authorization and audit. Preserve facts and frozen data when
   complete source evidence is unavailable. [Central contract C1, M, I](asi-centralized.md#decision-trace).
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
| Storage engine | SQLite for ingest (the Session Event Index), durable labels and FTS5. DuckDB over Parquet analytics partitions for analytics and the public SQL dialect, from day 1 (amended 2026-11-09). |
| PostgreSQL track (#477) | Paused. The merged code stays behind its feature flag with its CI workflow. The Raw-plus-projection-cache model, namespace-bound storage port and runtime custody owners carry over to the SQLite ASI store. |
| Hub | Hub is hosted Semon only: multi-tenant tenancy, workspace authorization and hosted views over Semon. ASI logic, schema and analytics live in Semon. Hub receives body-free analytics partitions and categorical labels only, never Raw or session bodies, and runs no L1 model classification. Each tenant workspace has its own analytics store. |
| Session bodies off-machine | Centralized default: selected redacted native logs are durably mirrored to the owner's receiver. Origin pairing/source consent authorizes export; receiver authorizes and audits reads locally, including while origins are offline. No zero-findings scan gate for own-machine reads. Hub never holds bodies. Redaction is best effort. [C1/C5/M](asi-centralized.md#decision-trace). |
| Metadata boundary | Plain names/paths may reach the operator's receiver with source consent. Hub receives only body-free analytics/categories, with paths tokenized at export. [C1/M/I](asi-centralized.md#decision-trace). |
| Semantic labels (L1) | A separate attributable layer. Provider/model evidence egress still requires explicit source consent; receiver enrollment grants none. Only categorical labels reach Hub; free-text evidence remains body data. This amendment adds no L1 execution mechanism. [C1/I](asi-centralized.md#decision-trace). |
| Analytics UI spec | Adopt the session activity viewer spec (semon#532) as the analytics UI direction. |
| Consumers | Agents (MCP/SQL) and humans (dashboards) equally, over one shared schema. |
| Viewer | Amended 2026-11-12 (owner decision; research in [t3-viewer-reuse-research.md](t3-viewer-reuse-research.md)). The Viewer provides analytics and standalone read-only browsing of Semon's durable session history. Preserve Semon's sidebar/navigation and native Catalog and transcript ownership. Use T3's session-log presentation as a reference for legibility and evidence navigation, without a T3 service, state or import dependency. Improve transcript usability where it supports those tasks; cosmetic pixel matching alone is not a goal. No composer, control, runtime or SSH features. Review any copied code/assets for license obligations and preserve notices. It keeps the authenticated server, shell and security boundary. Existing catalog/transcript code remains the starting point, not removal scope. Reimplement borrowed patterns in Semon's Preact UI under the [ownership contract](viewer-ownership.md), with no React/T3 runtime or state. Bodies stay on the machine or the operator's receiver, and Hub stays analytics-only. An "Open in T3" link is optional and appears only for a verified mapping. Pixel/visual comparison stays removed from CI (semon#536/#537). Proposed, not yet authorized: a Storybook comparison on synthetic fixtures of the current transcript against a T3-style presentation, run under [the AI design workflow](ai-design-workflow.md). |
| Control, runtime, SSH | Retired in step 2: `semon-control`, `semon-runtime` and `semon-ssh` code and active entry points are removed. Push/receive over TLS covers multi-machine collection. Retirement deletes no user data, credentials, history or installed services. |

## Architecture

The owner selected centralized storage on **2026-11-14**. The
[central contract](asi-centralized.md) owns architecture, custody/privacy,
identity/provenance, reads/audit, the four-action journey and implementation order.
The [federation design](asi-federation.md) is superseded history, retained for a
future multi-user or third-party receiver discussion, not a dormant runtime mode.

- **Ingest and derivation.** Reuse mirror export consent, sender redaction, native
  source parsing, archive custody and SQLite → Parquet/DuckDB derivation on the
  receiver. Body/FTS indexes remain private and separate from analytics. [C1/M/I].
- **Read-only access.** Receiver-local SQL/search/evidence and authenticated
  loopback Viewer use retained received history and local authorization/audit.
  Origin availability gates new uploads, not reads. Hub remains body-free. [C1/I].
- **Preservation.** Keep native identity/lineage/provenance, recorded costs without
  silent repricing, one current format and frozen data. No automatic deletion or
  compatibility layers. Duplicates count once only when proven; received machines
  have generated IDs joined only by explicit owner link (central contract C6). [C1/C6/M/I]
- **At rest and UX.** Recommend full-disk encryption and encrypted backups without
  a blocking setup check or app encryption. Target four setup actions and two
  long-running processes; service install is a counted opt-in. [C3/C4].

Trace labels refer to the central contract's [decision table](asi-centralized.md#decision-trace).
Receiver ASI integration remains planned; existing Viewer aggregation and mirror
custody do not establish delivered receiver SQL/search/materialization.

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
3. Follow the [central implementation plan](asi-centralized.md#implementation-plan):
   received identity/custody, received-machine materialization, receiver
   SQL/search/evidence, then four-action lifecycle and consent updates.
4. After the switch, remove federation runtime following a dependency pass;
   preserve enrollment, mTLS, renewal, mirror, archive custody and ASI
   projection/store/query. Retain only the historical design rationale.
5. Rebuild Analytics on durable storage; shape the Viewer into the analytics UI
   plus read-only history browsing (Viewer decision above).
   Pixel baselines and visual tooling are removed by semon#536/#537 (Source) and
   semon-hub#377 (Hub).
6. Hub follow-ups: shrink browser suites that exercise display/control/runtime,
   delete `ui/ssh.mjs` and reconcile viewer/session redesign docs with this
   direction. These follow-ups remain separate from the landed Semon retirement.
