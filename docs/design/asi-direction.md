# Agent Session Intelligence direction

Status: accepted direction (2026-11-08; federation D1–D8 accepted through the
PM's structured decision process on 2026-11-09; Viewer scope amended 2026-11-09
and clarified by the owner on 2026-11-12).
Federation supersedes option B's central-derivation default. This document
supersedes earlier product focus statements wherever they conflict. Acceptance
specifies the target architecture; it does not claim the safeguards are implemented.

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

1. **Storage.** A durable, multi-machine store of normalized session events:
   sessions, turns, tool calls and results, errors, tokens, cost, waits,
   handoffs and lineage. Native raw remains at the origin by default; body-free
   analytics is precomputed there and published across machines. Receiver-held
   facts remain durable when raw is unavailable and cannot be assumed rebuildable.
   Read-only bodies are fetched on demand under origin policy; explicit replicas
   retain supported mirror custody.
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
| Session bodies off-machine | Federation default: raw stays at origin; the operator's receiver fetches redacted typed viewing pages only on demand. Origin authorizes and audits every read, including cache hits; no offline grants. Encrypted cache: 256 MiB global, 64 MiB/source, 15-minute absolute TTL; stream-through opt-in. Explicit raw replica custody remains supported. Hub never holds or serves bodies. Redaction remains best-effort. |
| Metadata boundary | D7 accepts plain typed names and paths between origin and the operator's receiver. **PM interpretation, open to user correction:** this does not change the settled Hub rule; paths become HMAC tokens at Hub export and Hub gets no plain paths. Body exclusions, credential stripping and source policy remain. |
| Semantic labels (L1) | In v1, computed at origin. Labels follow methodology §3–§4 (labeller, taxonomy version, confidence and exact generation-bound evidence IDs). Model-provider evidence egress needs per-source opt-in naming provider/model; receiver enrollment grants none. Only categorical labels export; free-text goal/mechanism/reason fields are bodies. Multi-source derivatives obey all contributor policies. |
| Analytics UI spec | Adopt the session activity viewer spec (semon#532) as the analytics UI direction. |
| Consumers | Agents (MCP/SQL) and humans (dashboards) equally, over one shared schema. |
| Viewer | Amended 2026-11-12 (owner decision; research in [t3-viewer-reuse-research.md](t3-viewer-reuse-research.md)). The Viewer provides analytics and standalone read-only browsing of Semon's durable session history. Preserve Semon's sidebar/navigation and native Catalog and transcript ownership. Use T3's session-log presentation as a reference for legibility and evidence navigation, without a T3 service, state or import dependency. Improve transcript usability where it supports those tasks; cosmetic pixel matching alone is not a goal. No composer, control, runtime or SSH features. Review any copied code/assets for license obligations and preserve notices. It keeps the authenticated server, shell and security boundary. Existing catalog/transcript code remains the starting point, not removal scope. Reimplement borrowed patterns in Semon's Preact UI under the [ownership contract](viewer-ownership.md), with no React/T3 runtime or state. Bodies stay on the machine or the operator's receiver, and Hub stays analytics-only. An "Open in T3" link is optional and appears only for a verified mapping. Pixel/visual comparison stays removed from CI (semon#536/#537). Proposed, not yet authorized: a Storybook comparison on synthetic fixtures of the current transcript against a T3-style presentation, run under [the AI design workflow](ai-design-workflow.md). |
| Control, runtime, SSH | Retired in step 2: `semon-control`, `semon-runtime` and `semon-ssh` code and active entry points are removed. Push/receive over TLS covers multi-machine collection. Retirement deletes no user data, credentials, history or installed services. |

## Architecture

Federation accepted 2026-11-09 replaces option B as the default. The
[federation contract](asi-federation.md) owns governance, transport, cache and
authority details; [store](asi-store.md) (#544), [query](asi-query.md) (#545)
and #546 specify their respective surfaces. The user accepts mixed rules versions
and possible permanent evidence loss when origins never return.

- **Layers and producers.** Origin raw/native archives feed a per-namespace SQLite
  index. Activity, L0, StepFacts and recorded prices have one derivation there;
  `semon-asi` projects admitted generations into Parquet for DuckDB. Origins publish
  body-free facts to the receiver through immutable objects and atomic namespace
  heads. Imported facts are durable evidence, not disposable caches. Labels and
  other non-rebuildable state live in owned durable SQLite stores.
- **Origin governance.** Sensitivity tags, secret/PII detection and versioned
  policy govern egress locally. Every derivative obeys the intersection of its
  contributors' policies. L0/L1 classification stays at origin; hosted model
  evidence needs separate consent. Policy, redaction, source and computation
  versions accompany provenance. Plain typed names/paths are permitted to the
  operator receiver under D7, with the Hub-boundary interpretation recorded above.
- **Read-only viewing.** Origins open outbound-only authenticated channels to the
  self-hosted receiver. Only bounded read/append-subscription operations are
  available; no control, runtime, SSH, remote execution or Encrypted Relay.
  Origins pin native proofs and render redacted typed pages with exact evidence
  mappings. Receiver presentation never treats redacted bytes as original proofs.
  Machine/namespace/user access and origin authorization/audit apply on every read,
  including hits in the encrypted bounded cache. Offline means new bodies are
  unavailable even if cached; analytics remains queryable. Cache is never archive.
- **Custody and optional replicas.** Origin archive retention stays unchanged
  (indefinite default, optional explicit retention). Supported mirror and #572
  custody remain. Raw replicas are opt-in per source; central analytics authority
  may transfer only for a whole namespace with complete eligible source cohorts,
  preserving unaffected facts and labels. Mixed replica/pull-only machines keep
  origin analytics authority. Replica bodies still require online origin approval.
- **Versions and loss.** Re-derive affected sessions when the origin returns with
  complete inputs; retain old rows and show per-origin/version/freshness coverage
  meanwhile. Retired origins may stay frozen permanently. Recorded origin costs
  remain the current valuation; a separately named central valuation is later work.
  Refuse old formats, preserve/export frozen data, and never automatically reset
  custody. No multi-version exception or compatibility adapters.
- **Hub gets analytics only.** A receiver or standalone origin exports body-free
  facts/categories to a tenant-scoped Hub store. Paths are HMAC tokens at this
  boundary; the exporting store holds the key, Hub never does. Hub never receives
  bodies, serves transcripts, runs L1 or resolves body handles.
- **Query reach.** Sandboxed read-only DuckDB SQL and schema work through MCP/CLI,
  authenticated Viewer and tenant Hub, with no separate network SQL endpoint.
  SQL uses admitted analytics heads and never pulls bodies. Local FTS and remote
  bounded view come first; receiver search stays unavailable until federated
  coverage/audit exists. Hub search/view refuse before I/O.
- **Performance and boundaries.** Keep the local ingest/analytics/search budgets
  and measure connected publication and online-view costs separately in CI.
  `semon-asi` owns schema, transforms, publication and query; `semon-sessions`
  owns native parsing/index/StepFacts. Hub depends only on `semon-asi`.

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
3. Finish the ASI foundations: origin archive/index/StepFacts, the dimensional
   model over methodology §1.1 and pure analytics projection. Add governed
   origin-to-receiver publication with source/version coverage. Keep supported
   mirror custody. The PM applies the [lane-change plan](asi-federation.md#lane-change-plan-for-pm-application)
   after docs merge approval; existing PRs do not absorb channel/cache scope.
4. Ship read-only SQL/schema and local FTS through MCP/CLI, replacing paused
   `find`; add authenticated online remote view only after origin policy/audit,
   exact page mappings and receiver cache/access gates. Receiver search is deferred.
5. Rebuild Analytics on durable storage; shape the Viewer into the analytics UI
   plus read-only history browsing (Viewer decision above).
   Pixel baselines and visual tooling are removed by semon#536/#537 (Source) and
   semon-hub#377 (Hub).
6. Hub follow-ups: shrink browser suites that exercise display/control/runtime,
   delete `ui/ssh.mjs` and reconcile viewer/session redesign docs with this
   direction. These follow-ups remain separate from the landed Semon retirement.
