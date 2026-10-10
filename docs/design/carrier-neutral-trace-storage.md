---
depends_on: []
identity_impact: replaces the product's scope, storage substrate, and transport
---

# Carrier-neutral trace storage and lossy reactivation

Historical architecture, superseded by the
[Session foundation retirement](session-foundation-retirement.md). Reactivation,
capability negotiation and cue retrieval are not current delivery requirements.
Existing storage/identity/forensic contracts remain migration obligations; the
periodic capture installer and source unit templates are retired.

**Semon captures an agent's own working memory from one carrier, stores it in a
carrier-neutral form addressed by content, and re-activates it into a different
carrier — accepting that every re-activation is lossy and making the loss
explicit rather than hiding it. The telemetry pipeline is abandoned: OTLP,
ClickHouse, the collector, and the analytics queries are removed. Only the
Codex/Claude record parsers survive, re-scoped as carrier adapters.**

## Implementation status

This document states the intended carrier interface, not the current API.
Capture and forensic retention are implemented, with ordered occurrences
specified separately in [trace-identity-and-occurrences.md](trace-identity-and-occurrences.md).
Capability references, declared capability sets, reactivation with loss reports,
and cue retrieval remain unfinished. The three field classes below are distinct
from the store's three tables/regions; the occurrence table does not implement
capability references. The unresolved reactivation behavior and cue mechanism
remain design decisions before those interfaces can ship.

## Constraints

| Tag | Constraint |
|---|---|
| **MUST** | A trace's identity is the content hash of its canonical semantic form. Two captures of the same work from different carriers that reduce to the same semantics are the same trace. |
| **MUST** | Every trace separates **transferable semantics** from **carrier-incidental detail**. The split is structural, not a convention — carrier-incidental fields live in a distinct region that reactivation may drop. |
| **MUST** | Every ecphory emits a **loss report**: what was applied, what was approximated, what was dropped and why. A reactivation that silently discards content is a defect. |
| **MUST** | A carrier adapter implements `capture` and `reactivate` and declares its **capability set**. Anything outside that set is unrepresentable and must be reported, never coerced. |
| **MUST** | The subject of a trace is the *work*, never a person. No user entity, no user profile, no per-user memory. |
| **WHY** | Carriers are not isomorphic. Claude Code has hooks, skills, subagents, and permission modes; Codex has none. A trace saying "the pre-push hook rejected the commit" has no referent in a carrier without hooks. Any design assuming lossless transfer is falsified by the first asymmetric feature. |
| **WHY** | Content addressing rather than timestamps, because the same work re-encountered through a second carrier must converge on one trace instead of accumulating duplicates. Time ordering is a query concern, not an identity concern. |
| **MAY** | Retain raw carrier records beside the canonical trace for forensics and re-derivation, provided they never feed identity or retrieval. |
| **MAY** | Ship carrier adapters out-of-process, so a new carrier can be supported without a Semon release. |
| **OPEN** | **Cue matching mechanism.** Lexical full-text may suffice for v1; semantic retrieval needs embeddings and a model dependency. Undecided, and it materially changes the dependency surface. |
| **OPEN** | **Implementation language.** Python keeps the surviving parsers as they are; Rust matches the rest of the portfolio and yields one distributable binary. Depends on whether Semon ships to others or stays local tooling. |
| **OPEN** | **Whether reactivation writes or proposes.** Injecting memory into a live carrier session versus emitting a document its operator applies. Materially different blast radius. |
| **DECIDED** | **Trace granularity — closed 2026-09-20.** Identity does not range over session, time, repository or order; those are *occurrence* facts, recorded in a separate ordered region. A trace stays content-addressed over its semantic core alone, and the log renders by joining occurrences to traces. See [trace-identity-and-occurrences.md](trace-identity-and-occurrences.md). |
| **NOT** | Not an observability or cost-analytics product. Token counts, latency, and spend are carrier-incidental. |
| **NOT** | Not a memory store *about users*. That is Mem0, Letta, Zep, and Engram's problem, and taking it would force a user entity into the schema. |
| **NOT** | Not a distributed or hosted service in v1. Local-first, single-file store, no daemon. |
| **NOT** | Not a general log aggregator. A record with no reactivation value is not a trace and does not belong. |

## What survives

*(measured — read from the tree at `71a0813`)*

**Survives, re-scoped as a carrier adapter.** `scripts/codex_tailer.py:201` `normalize_record` and its helpers — `message_text:86`, `token_usage:121`, `parse_timestamp:60`, `repo_from_url:106`, `repo_from_cwd:115`, `infer_success:134`. Roughly 200 lines of format archaeology against Codex session JSONL. This is the only hard-won asset in the repo, and under the new model its carrier-specificity is a feature: this *is* what a carrier adapter is.

**Survives, generic.** Incremental tailing — `default_state_path:21`, `load_state:28`, `save_state:44`, `candidate_files:481`, `process_file:488`. Cursor-based resumption is needed whatever the substrate.

**Removed.** `otlp_payload:391`, `export:454`, `any_value:381` — OTLP transport. `clickhouse/schema.sql` entire, including the `events_from_otel_logs` materialized view. `otel-collector-config.yaml`. `docker-compose.yml`. `analysis/queries.sql` — five cost and usage queries belonging to the product being abandoned.

**Retired.** The former `systemd/devlog-codex-tailer.*` periodic capture templates and installer are removed from the source tree. Existing installed services are untouched and require explicit inventory and historical-data export before uninstall.

## The trace schema

*(derived from the constraints above)*

A trace has three regions, and the boundary between them is the core of the design.

**Semantic core** — participates in the content hash. What the agent was trying to do, what it did, what happened, what it concluded. Carrier-independent by construction: a fact that cannot survive translation to a carrier with a different feature set is not core.

**Carrier envelope** — excluded from the hash, retained for provenance. Which carrier produced it, session and message identifiers, token counts, cost, latency, tool-call framing. The existing `devlog.events` table is *entirely* this region, which is the clearest evidence the current storage models the wrong thing.

**Capability references** — the interesting region, and the reason two regions are not enough. Content that is semantically load-bearing *and* names a carrier feature: "the pre-push hook blocked me", "I delegated this to a subagent", "the permission classifier denied the edit". These cannot be dropped without losing meaning, and cannot be transferred literally to a carrier lacking the feature. Each carries a declared capability tag, so a target carrier can be asked whether it can honour it and answer honestly.

Stripped at capture: anything reconstructable from the semantic core, anything whose only value is aggregate analytics, and raw payloads already held in the optional forensic region.

## The fidelity fork: reconstruction

**Position taken: reconstruction. Replay is rejected.**

*(derived — the argument is deductive, not measured)*

Replay requires a lossless mapping between every pair of carriers. That is combinatorially unmanageable and, more decisively, falsified by a single asymmetric feature — and asymmetric features are the normal case (*measured*: Claude Code's hooks, skills, subagents, and permission modes have no Codex equivalent). Under replay, every such asymmetry is filed as an engineering defect to minimize, producing a permanent backlog of bugs that are actually category errors.

Tulving's encoding specificity principle sharpens it: retrieval succeeds only where the cue overlaps the encoding context. If the receiving carrier cannot reproduce the encoding context, a byte-exact transfer does not reproduce the memory — it reproduces a *record* of the memory. Bartlett points the same way: remembering is reconstruction under the receiving system's schema, not playback of a stored copy.

**Consequence — how loss is measured and surfaced.** Every ecphory returns a report with three lists:

- **applied** — capability references the target honoured natively.
- **approximated** — references the target substituted for, each naming the original capability, the substitute, and the adapter's confidence that intent survived.
- **dropped** — references the target could not represent at all, retained verbatim so the operator sees exactly what did not make the journey.

A trace re-activated into a carrier that drops a third of its capability references has not failed; it has produced a correct result the operator must be able to see. **The report is part of the return value, not a log line.** An adapter returning an empty dropped-list while discarding content is the one defect this design does not tolerate.

## Carrier abstraction

*(derived)*

The minimum interface is three operations, not two:

```
capabilities() -> CapabilitySet
capture(source) -> [Trace]
reactivate(trace, target) -> Reactivation { applied, approximated, dropped }
```

`capabilities()` is what makes honest loss reporting possible: reactivation compares the trace's capability references against the target's declared set *before* attempting the write, so the dropped-list is computed rather than discovered through failure.

When a target cannot accept part of a trace, the adapter must never coerce silently. It has two legitimate moves: substitute, recording it under `approximated`; or decline, recording it under `dropped`. Partial reactivation is the expected outcome and must succeed — refusing a whole trace because one capability is unrepresentable would make cross-carrier transfer useless in exactly the cases it exists for.

## Positioning boundary

*(assumed — competitor behaviour is from public positioning, not from use)*

Mem0, Letta, Zep, and Engram store an agent's memory **about a user**: preferences, history, facts accumulated across conversations. The memory is scoped to a person and the agent is its reader.

Semon stores an agent's **own working memory**: what it attempted in this codebase, which approach failed, what it concluded about the architecture — scoped to the work, and portable to a *different* agent in a *different* carrier.

The constraint that follows, and the reason this is not marketing copy: **there is no user entity in the schema, and there must never be one.** Any field identifying a person is carrier-incidental at best and out of scope at worst. When a design question arises — "should traces be per-user?" — the answer is already determined: no, per-work. It settles retrieval too: cues describe work context (repository, subsystem, failure mode), never identity.

## Vocabulary critique

The proposed terms assessed rather than accepted:

- **`trace`** — keep, with a known collision. In distributed tracing a "trace" is a request span tree, and this project is adjacent to that world; leaving OTel reduces but does not remove the confusion. Accepted because an engram *is* a memory trace and no better word exists.
- **`cue`** — keep without reservation. Tulving's own term, precise, unclaimed.
- **`carrier`** — keep, but **retire `substrate`**. The scope statement says "substrate-independent" while the vocabulary says "carrier"; two words for one concept will diverge in the docs within a month. Pick `carrier` and rewrite the scope sentence.
- **`ecphory`** — keep as the noun for the reactivation event; it is Semon's real coinage and genuinely unclaimed. Do **not** make it the everyday verb — the verb is "ecphorize", which nobody will type. Use `reactivate` on the CLI and API surface; reserve `ecphory` for the domain concept and the report type.
- **`store`** — weakest of the five. Both noun and verb, and it names a database rather than a role. `engram` and `mneme` are correctly banned. No strong alternative identified; flagged rather than invented.

## Architecture consequence

*(derived)*

The current stack is three moving parts — a Python tailer, an OTel collector, ClickHouse in Docker — bridging a telemetry transport to a columnar warehouse. All three exist to serve aggregate analytics over append-only events.

Content-addressed storage with cue retrieval needs none of them. One embedded store with full-text search covers v1, removing the daemon, the container runtime, and the collector, and reducing the product to a binary plus a file. Whether that store also needs vector search is the open question above, and it should not be answered before there are real cues to test against.

The strongest argument for removal is not simplicity but honesty: OTel's data model flattens every record into a string attribute bag, structurally incapable of expressing the semantic-core / carrier-envelope / capability-reference split this design rests on. Keeping OTel would mean encoding that split in naming conventions inside a flat map — which is exactly how the current `devlog.*` namespace works, and exactly why it cannot carry the new model.
