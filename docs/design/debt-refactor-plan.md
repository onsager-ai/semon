# Refactor what the warehouse keeps

Status: proposed sequence, 2026-11-17. The first extraction accompanies this plan;
remaining entries need review before implementation. Baseline:
`fe77ddb157581d818efb5822b6520b5e063391aa`.

The owner chose to reduce debt in code that survives the catalog warehouse.
The production plan in `origin/spike/star-catalog:docs/design/star-catalog.md`
at `852a1807695763fc820cf4cb8dc4788cf536f921` is the retirement boundary,
particularly phases 1–4. This is preparation for
that work, not an alternative warehouse architecture or a reason to delay its
availability fix. Keep native identity, source admission/proofs, custody,
origin-local bodies, selected-source semantics and the existing Viewer shell.

## What survives

Paths below are relative to `crates/semon-sessions/src/`. Classification is by
responsibility, not a promise to retain each current function or struct.
Large line counts include substantial inline tests; moving tests alone would
make the file shorter without reducing production coupling.

| Flagged file | Survives / useful existing seams | Deleted or replaced in warehouse phases 3–4 | Extracted in phase 1 |
| --- | --- | --- | --- |
| `events.rs` (~10.9k) | `FileIndex`, ledgers and source revisions, native event/signal parsing (`claude`, `codex`), typed usage observations, incremental parse/write accounting and fixture tests | EventCache semantic publication forwarding, old Catalog publication/repair orchestration and stage-only cache state with final callers; ordinary source admission and CAS stay | None owned here; parser observations feed the extracted matcher |
| `events/store.rs` (~11.7k) | Source admission transaction, ledgers, native event/usage row codecs, current-schema qualification, source chunks/proofs, archive evidence, origin-local body/FTS and usage repair | `semantic_stage.rs`, old Catalog list/detail rows and publication batches, derived session journals, publication epochs, phase cursors and exact-generation stage fences; superseded activity projection with final consumers | No standalone store split required; expose role-qualified matcher observations as facts in the phase-1 contract |
| `model.rs` (~9.2k) | Native selected-source body decoding (`Texts`, prompt/tool/result readers), transcript representations/render helpers, source identity/provenance rules and their fixtures; retain parent precedence semantics when moving their implementation | `Builder` list/detail derivation and global assembly, `model/relation_stage.rs`, staged endpoint/origin maps and semantic-index preparation after consumers switch; selected reads use the new source resolver | Causal `groups`/`split` partitioning, read context and end-state logic; role-qualified relay observations and existing greedy matcher over selected source bodies. Do not pre-split these independently |
| `catalog.rs` (~9.7k) | Public identity/source proof contracts, authorization/read-scope distinctions, native relative source resolution, request validation and current/history semantics | `rows`, `read_page_snapshot` and list/detail SQL/adapters over published `CatalogRow`, ready/publication fences and their stage-only tests. Public entrypoints may keep names with new implementations | None |
| `viewer.rs` (~9.2k) | HTTP server, access/security headers, receive authentication, shell/assets, route dispatch, bounded ASI transport, `MachineView` tree snapshots and refresh worker lifecycle | Semantic-ready waits and producer/publication assumptions inside Catalog refresh/read paths; replace those calls together with warehouse reads, not the server or lock ownership wholesale | None |
| `native_evidence.rs` (~4.9k) | Per-machine `RuntimeStore`/source ownership, `SessionSources`, input discovery, passive runtime observations, source snapshots and change detection, cancellation/retirement | `begin_catalog_stage`, `advance_semantic_stage`, semantic conflict handling, staged preparation/continuation and old query-generation cache dependencies; search scheduling changes with the new admission path | Native qualified runtime/source context remains input to the phase-1 parity contract |
| `union.rs` | Configured machine authority, received-machine following, identity/collision qualification, cross-machine routing, account/admin extras and host chrome | `wait_for_catalog_read` semantic publication wait and generation-specific Catalog assembly; replace list/detail delegation and old QueryData assembly when consumers switch | None |
| `lib.rs` | Public façade/types, native homes/options, process/marker helpers, `collect_native_tree`, tree rendering, allowlisted input access and harness fixtures | `refresh_native_catalog`'s semantic-ready loop and exported old publication adapters with their final callers | None; native marker/process facts support parent qualification |
| `origin_projection.rs` (misplaced finding) | Namespace/source witness admission, qualified read-only cohort generation and custody checks as requirements | Current `SqliteAnalyticsInput` activity/StepFacts adapter, silver-journal replay/checkpoint machinery and published Catalog dependencies are replaced in phase 4 | None |

`events/store/session_activity.rs` and its analytics projection are not attractive
pre-rewrite cleanup targets: the plan removes the superseded activity publication.
Keep upstream harness format support, exact usage validation, source proofs/CAS
and useful refusal tests. The names “legacy” and “migration” in usage/parser code
do not establish old-Semon compatibility: cumulative versus explicit counters are
supported native observations, and receipt-checked representation repair serves
the current index. No compatibility-only deletion was established in the first
extraction. Do not add aliases for retired Semon formats or migrations; the
warehouse switch removes temporary comparison paths with their final consumers.
No source history or live index cleanup is authorized by this plan.

## Ranked PRs

Each PR is a behavior-preserving move with explicit imports and existing owners.
Keep each under roughly 2,000 moved lines, splitting the entry further if needed.
The verification column adds focused coverage to the common checks below.

| Rank / timing | Files and move | Reason / coupling made visible | Risk and focused CI verification |
| --- | --- | --- | --- |
| 1 — before phase 1, first PR | Move the usage records, `ModelTokens::add` and `CodexNativeUsage` decode/observe implementation from `events.rs` into `events/usage.rs` (~182 existing lines). Re-export existing names from `events`; use `pub(super)` for fields already accessible throughout the parent subtree | First bounded seam in a god/hotspot file; separates source-local usage records from cache orchestration. Names the parser → normalized `events/store/native_usage.rs` → pricing dependency that currently shares one enormous file; tolmap reports native_usage↔model co-change | Low: unchanged expressions, derives and serde fields; only module paths/visibility change. Rust parser/cache replay, normalized native usage restore/lost-store and pricing tests |
| 2 — parallel with phase 1 | Extract `events/store.rs` native row codecs (`read_event`, `read_usage`, `put_event`, `put_usage` and tightly shared codecs) into `events/store/rows.rs`; leave transaction orchestration in the parent. Keep semantic Catalog codecs out | God/hotspot store mixes native serialization and retiring publication; explicit native row boundary supports phase-2 fact writes. Related store↔native_record and store↔pricing changes become reviewable together | Medium: SQL column/parameter ordering and full struct destructuring must remain exact. Store round-trip, append/replacement, rollback, normalized usage and bounded-input tests |
| 3 — parallel with phase 1; coordinate source edits | Move complete selected-source record text helpers (`block_of`, prompt/assistant/tool/result decoding and associated pure helpers) from `model.rs` into `model/native_text.rs`. Keep existing signatures through private imports/re-exports; leave Builder/relay/grouping untouched | Separates surviving source rendering from the builder being deleted. Makes the model↔native_record/native_handoff cluster distinct from semantic publication | Medium: native body and tool formats are an upstream contract. Selected transcript/tool/field tests, sanitized harness fixtures and source replacement/refusal tests; manual Codex conformance if decoder ownership changes |
| 4 — parallel with phase 1 | Extract native home resolution, marker parsing and process/lock helpers from `lib.rs` into narrowly named existing-domain modules (`native_homes.rs`, existing `native_marker.rs`/`parent_links.rs` where appropriate), one seam per PR. Keep the public façade and `collect_native_tree` orchestration | Hotspot root mixes public exports with native environment evidence. Exposes tree_state↔lib and model↔native_tree ownership without moving the retiring builder | Medium: machine homes, process-start identity and marker precedence must stay exact. Native home, native tree, parent/marker and cross-harness identity fixtures |
| 5 — after phase 3 removes publication waits; parallel with phase 4 | Split `viewer.rs` HTTP transport/security/receive adapter from `MachineView` refresh implementation; split `union.rs` machine selection/identity routing from account chrome only if still oversized. Keep `ViewerCore`, `NativeEvidence` and refresh-pool ownership unchanged | Surviving hotspots; tx↔union (14 co-changes), slot_projection↔union (9), storage/runtime↔union identify request/ownership boundaries. Do not preserve a dependency on retiring slot recipes just to make a static edge | Medium: locks, request authentication, selected source lifetime and host chrome. HTTP/auth, multi-machine collision/received-source, refresh and selected read tests; full manual UI fixture/browser workflow plus actual screenshot review under Viewer ownership rules |
| 6 — after phase 4, not a prerequisite | Move the replacement warehouse analytics-input adapter under an `analytics/input.rs` owner; split `native_evidence.rs` surviving source discovery/snapshots from per-machine lifecycle only after stage deletion reveals the final size | `origin_projection.rs` has 9/16 edges (56.25%) to another district, and co-changes with ASI projection/publish/store. Fix ownership in its replacement; renaming the old journal adapter now would be discarded. Evidence↔SQLite and source-change↔custody form a surviving cluster | Medium: analytics/namespace/custody boundary. ASI/CLI/MCP admitted-input, source witness/refusal, retained history and read-only cohort tests; no temporary compatibility façade |

Only PR 1 is recommended to land before phase 1; it gives that work a stable
usage module at small review cost. It is not a correctness prerequisite. PRs 2–4
can proceed independently, coordinating overlapping source ranges with the
warehouse author. Do not block phase 1 on generic debt reduction. PRs 5–6 wait
for deletion rather than moving code the rewrite removes.

The causal grouper and relay matcher already belong to warehouse phase 1.
Duplicating those extractions here risks incompatible read contexts; verify them
with full-builder parity, late/out-of-order evidence and revocation tests in that
phase, not a second general-purpose abstraction.

## Findings that should not become cleanup PRs yet

The reported district cycle closes through CLI `main.rs` → ASI `coverage.rs`
and `body_query/tests.rs` → `facts.rs`. The latter serializes `RepoId` for a
fixture; the former is an ordinary consumer of a public ASI reader. The report
says `crosses_package: false`, with no package-edge or mutual-pair evidence.
This is a cycle between clustered file districts, not a demonstrated Rust crate
cycle. Do not introduce a shared contract crate or distort production ownership
to remove it. Re-evaluate after the warehouse replaces body-query admission;
only extract a fixture helper if actual test reuse warrants one.

The semantic_stage↔native_evidence cluster (13 co-changes),
catalog↔relation_stage and activity projection↔query/MCP clusters mostly describe
one publication pipeline being retired. They justify coordinated deletion, not
new modules. tx↔union and slot_projection↔union describe selected request custody;
read their real call paths before deciding whether a split needs direct imports.
Co-change alone does not justify an abstraction or a new dependency edge.

## Verification and map evidence

For every implementation PR, CI runs `cargo fmt --all --check`,
`cargo clippy --all-targets --locked -- --no-deps -D warnings` and
`cargo test --locked`, including the ordinary native-storage boundary and fixture
gates in `rust.yml`. Local builds, cargo check/test/clippy, Node tests and project
wide typechecks are prohibited on this machine. Changed-file rustfmt is allowed;
run it and report its actual exit status. Keep hooks enabled and never bypass
checks. Viewer changes additionally use the repository's manual UI workflow;
manual dispatch is not a replacement for Rust PR CI.

Run `tolmap check . --base "$B" --head HEAD --report-only --format json`, where
`B=$(git merge-base origin/main HEAD)`, on the task checkout. This builds the
branch map without updating registrations. Read its changed edges, fixed-base
modularity and outside-graph paths; advisory exit 0 is not a correctness result.
For comparable debt reports, build both merge base and head cold with explicit
`--name semon --out <separate temporary directory>`, obtain unfiltered JSON with
`tolmap debt --map <directory>/semon.json --format json`, then run
`tolmap debt diff <before.json> <after.json>`. Do not compare a warm library map
to a cold branch map. Report changed district memberships alongside the edge
report: a renamed/repartitioned district is not itself a regression.

Baseline `tolmap debt --repo onsager-ai/semon` reports 5 god files, 8 hotspots,
125 hidden-coupling pairs, 1 misplaced file and 1 district cycle across 178 Rust
files/1,377 kept edges. `overview`, `where` and `deps` helped locate seams; exact
callers and privacy still require source inspection. The two districts named
`events` and `query & events` and the `semon-push` district containing `facts.rs`
are clustered dependencies, not module or crate boundaries. Rust `use`, fully
qualified paths and re-exports are mapped (up to four hops); methods and macros
are incomplete, `mod` declarations are not edges, tests are included, and only
Rust is covered. All counts are lower bounds. A façade split correctly leaves
many caller edges pointing to `events.rs`; expect a small structural improvement,
not an automatic reduction of every debt counter.

First-PR measurements (cold base/head maps, before the extraction commit):
178 → 179 files, 1,377 → 1,390 kept edges; `events.rs` fan-in 96.95 → 85.72,
complexity 965 → 962. Five god files and eight hotspots remain; hidden coupling
125 → 126, adding analytics↔events as type references resolve into the new
module instead. The unchanged `origin_projection.rs` misplaced flag disappears
because districts repartition, not because its ownership was fixed. Six districts
become seven: `events` splits into `events & store` and `events & session_activity`
(the new usage module joins the latter), `query & events` becomes `query`, and
`handoff & native_tree` becomes `handoff & model`. `native_evidence.rs` moves from
`storage & postgres` to `events & store`; `origin_projection.rs` stays in
`semon-asi`. These boundaries fit some dependencies but remain broader than
actual Rust owners. Fixed-base working-tree `check` crosses one `events` district,
with modularity 0.380548 → 0.381652 (delta +0.001104); it lists the design document
outside the Rust graph. Added static edges include analytics/comparison/model
tests → usage and usage → lib; several removed edges have `static_import: false`
and reflect graph blending/pruning, not deleted runtime behavior. Re-run the
committed-head check for the PR body; historical co-change metrics can change
when the extraction commit enters history.
