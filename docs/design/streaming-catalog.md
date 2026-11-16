# Streaming Catalog maintenance

Status: draft for owner approval. Owner direction: full streaming rewrite,
2026-11-16. This document proposes a replacement maintenance algorithm; it does
not authorize implementation, deployment, source deletion or live cache recovery.
Code references use repository paths and lines at `fe77ddb1`.

## Problem and goal

Continuous appends repeatedly invalidate the current semantic stage between
indexing, identity, relationships and publication. A cold receiver can therefore
serve `503 catalog_unavailable` indefinitely although indexing makes progress.
Busy local logs have the same failure mode. Preserve today's identity, lineage,
provenance, merged-copy, turn, usage and selected-read outputs while allowing Raw,
Index and Catalog to advance independently.

The history is instructive: [#445](https://github.com/onsager-ai/semon/pull/445)
recorded live starvation despite a passing static-backlog regression;
[#453](https://github.com/onsager-ai/semon/pull/453) stopped unrelated journaled
runtime changes resetting history; [#455](https://github.com/onsager-ai/semon/pull/455)
paged oversized incident relationships; [#473](https://github.com/onsager-ai/semon/pull/473)
retained published prefix proofs and pinned selected-read snapshots. None makes
the multi-phase source claim stable under continuous append traffic.

The investigation considered finite pinned-Index epochs and receiver watermarks.
The owner chose the full streaming replacement instead. There is no receiver-only
watermark, pending-tail storage, new broker or storage-backend migration here.
The reference receiver's ordinary file writes are not assumed to emit
`SessionSourceWrite` evidence: see `crates/semon-sessions/src/source_change.rs:391`.

## Contract and non-goals

- Keep public identity `(scope, source_key, catalog_key)` and current collision
  qualification, canonical ownership and cross-machine presentation rules.
- Keep inferred relationships, current stub behavior and copied-event suppression.
  These become revisable views; they are not weakened into candidate-only output.
- Preserve parsers, supported native formats, source authorization, custody,
  retained provenance and source-backed text. No body text enters analytics/Hub.
- A partial Catalog remains provisional. A list being readable does not establish
  complete discovery, unique native ownership or archive-eviction readiness.
- No public revision-history browser, new URL grammar, runtime/control feature,
  changes to receiver ACK meaning, or automatic old-store conversion.

The governing contracts are [scoped publication](scoped-catalog-producer.md),
[retained projections](retained-native-projections.md),
[native resolution](catalog-native-resolution.md),
[public identity](session-identity-and-urls.md) and
[source readers](session-source-reader.md). This design supersedes the semantic
stage portions of scoped publication at cutover, not those independent contracts.

## Evidence and projection model

M, P and N classify information, not file types. A single native record can add
an immutable observation, resolve a slot and revise several published values.

| Class | Durable representation | Rule |
| --- | --- | --- |
| M: immutable observations | Source-incarnation, native record position/block, observation kind, captured input revision and provenance | A qualified append adds observations; old evidence is never edited into a different observation. |
| P: pending then final | Slot identity, pending state, decisive observation reference, interpretation revision | Resolve once only where native occurrence scope actually makes the answer final. |
| N: revisable conclusions | Derived key, revision, inputs, output membership, explicit replacement/retraction | New evidence can remove an old conclusion and replace all its contributions. |

Observation identity is internal and does not replace a native session identity
or Catalog key. It includes the source incarnation and native position, not
arrival order. Observations extracted from live facts/sidecars additionally bind
their independent authority revision. Rewrites start another incarnation rather
than pretending to be append observations.

Keep raw candidate evidence for reused tool IDs, receipts, markers, usage and
lineage edges. Current parser materializations may still update earlier rows;
the immutable observation log records the triggering native evidence separately.
`DirtyRows` already identifies changed earlier rows and appended ranges
(`crates/semon-sessions/src/events.rs:286`), but is transient today. It becomes the
input to durable derivation deltas, not a substitute for those deltas.

Ordinary unique call/result pairs and first decisive marker observations are P.
A copied or reused native identifier is not proof of uniqueness. Its selected
attachment belongs to N. `OriginTurn::merge`
(`crates/semon-sessions/src/model/relation_stage.rs:210`) remains the P merge rule
within one fixed sender-turn interpretation; repartitioning turns creates a new
interpretation rather than merging contradictory resolved witnesses.

### Durable records

The following are logical tables in the existing Event Index database. Exact
SQL names can follow local conventions; their keys and transaction contracts are
part of this proposal. They contain metadata and source references, not a second
persistent copy of prompt, result or message bodies.

| Record | Key and content |
| --- | --- |
| Source revision | `(source incarnation, revision)`; complete consumed ledger, parser contract, independent sidecar/facts dependencies |
| Observation | `(source incarnation, native position, kind, ordinal)`; immutable evidence or source-backed field reference |
| Delta | Machine-local increasing sequence; before/after source revision, added/changed/withdrawn observation references, changed selector keys |
| Dependency membership | `(selector kind, scoped selector value, member key, validity revision)` plus reverse index by member |
| Dirty work | `(projection kind, key, target revision)`; cause sequence, durable keyset cursor and private result reference |
| Derived revision | `(projection kind, key, revision)`; normalized result, input references, output contribution identities |
| Publication | Generation, immutable manifest root, interpreted source-vector root, discovery/readiness state |

The before-image includes old selector memberships. Retraction must dirty readers
of the old name/parent/root as well as readers of the new value. A delta that only
records the latest Index row cannot do that safely after a crash.

### Revision admission and coherent cuts

Ingest and semantic derivation have separate durable positions. The Index can
advance continuously; the semantic consumer applies a finite admitted batch of
committed deltas in machine-local sequence order. The current publication plus
that batch defines a finite interpreted source vector V. Unchanged sources retain
their previously interpreted revisions. Newer deltas wait for a later batch.

This is a projection input cut, not a global semantic epoch: no all-source scan,
Index freeze, live-generation equality, phase barrier or empty-journal condition
is required. There is one semantic consumer per machine database, enforced by
transactional work ownership/CAS, using the existing producer scheduling. It can
resume in another process. Ingest writers do not need to wait for that consumer.

For a cold Catalog, admit the first bounded discovered cohort, then publish its
complete semantic interpretation as a provisional list. Undiscovered sources
are absent from V, not falsely proved absent from the machine. A later discovery
is another delta. The full-builder oracle receives exactly that same inventory.

Dependency expansion and every reducer read versioned observations at V, never
the advancing latest Index. They can discover additional affected keys, but cannot
admit newer source revisions into this work. A second append to an already dirty
source does not reset its in-flight target or cursor. Do not coalesce away a target
that has started; later targets may coalesce before admission if their before/after
memberships and native evidence are preserved.

The semantic consumer's ordered cut is required to avoid publishing an ancestry
from V1 with usage deduplication from V2. This deliberately chooses simple ordered
publication over speculative parallel reducers with conflicting cuts. Independent
machine databases continue independently.

## Dependency indexes and incremental maintenance

Every reducer declares selectors it reads, including negative/uniqueness queries.
A lookup returning zero candidates still registers a dependency; arrival of the
first candidate must wake it. Register the dependency in the same transaction
that commits the reducer result. Index both selector-to-consumer and
consumer-to-selector; rebuild a key by replacing its complete membership set.

| Selector key | Scope and members | Dirty consumers |
| --- | --- | --- |
| Native lineage component | Machine, harness, native node; continuation/bridge edges, source membership, directed copy pair | Canonical owner, copied-event mask, all displaced owner keys |
| Catalog spelling | Machine, raw native ID and every reserved/qualified spelling examined | Collision qualification and exact-key/native maps |
| Call ID | Machine, harness, native occurrence scope, call ID; source-scoped and fallback candidate ranges | Attachments, parent/spawn placement, results and turns |
| Relay `msg_id` | Machine and protocol's actual ID scope; ordered send/receipt candidates | Endpoint selection, stubs, placement and sender-turn provenance |
| Name/task name | Machine, root/parent where current rule requires it, exact normalized name; siblings/calls/path aliases | Unique-name relays and spawn placement |
| Heuristic candidate range | Existing prompt/path/cwd/time/body-match predicate and root; source-backed evidence references | Launch selection and two-pass receipt pairing |
| Usage pair | Machine, harness, `(message ID, assistant UUID)` and per-message report key | Shared/exclusive attribution, owner usage and cost |
| Parent/root family | Machine, selected child/parent/root keys; reverse parent/children and contribution edges | Ancestry, root membership, descendant aggregates |
| Turn partition | Canonical owner, event ordering key and incident relation/run boundary | Merged event order, affected turns, slots, annotations and summary |
| External input | Machine, source/sidecar/facts selector and authority revision | Role, selection, names, process state, source availability and summaries |
| Presentation owner | Authorized configured-machine set revision and served spelling | Existing received-copy dropping/conflicts; no public identity merge |

Do not scope a globally searched fallback more narrowly than today's semantics.
For example, Claude global `toolUseId` fallback needs a machine-wide call bucket
in addition to the explicit storage-parent bucket. Body matching stays origin
local; dependency metadata uses evidence references and existing private matching
access, never exports matching text to analytics.

### Work and fan-out bounds

A changed record can affect an arbitrarily large lineage, a popular name or an
entire family. The bound is **per step**, not a claim of constant total fan-out.
For V with finite indexed evidence, the affected set is finite. Enumerate it by
indexed keyset pages of at most 64 members/relations and 1 MiB of decoded metadata.
Persist the selector, V, last composite key and any accumulator in the same
transaction as each page's dirty insertions. No OFFSET paging or whole-bucket
`GROUP_CONCAT`, JSON vector decode or recursive in-memory closure.

Use the existing ~4 MiB complete-record ingestion slice. One native record may
cross that bound; preserve its existing parser limits and valid-EOF behavior.
Derived variable fields use source references and chunked private output so one
large owner does not have to fit a single metadata row. Reject a native record
outside supported parser limits explicitly; do not call truncated input success.

Retraction uses contribution identities: remove `(producer key, prior revision,
output key)` then install the replacement revision's contributions exactly once.
Do not subtract floating-point displayed totals repeatedly. Re-reduce affected
usage/cost facts in the existing deterministic ordering and rounding contract.
A contribution consumed by another projection dirties that projection, and each
edge is processed once per target revision, even if several causes reach it.

### The eight non-monotonic cases

All cases below use the admitted V and publish replacements atomically. Listed
fan-outs are total affected sets, traversed with the page limits above.

1. **Canonical merges and copied-event suppression.** Persist native nodes and
   continuation/bridge edges; recompute connected membership for touched old and
   new components, preserving the current timestamp/continuation ordering and
   first-file owner. Retraction after source withdrawal may split a component,
   so union-find alone is insufficient: traverse surviving adjacency at V.
   Recompute suppression only for directed continuation copy pairs using existing
   UUID/eligible-ID rules, then replace owner/event memberships. Retract displaced
   Catalog keys, source maps, recipes, usage and incident placements together.
   Fan-out: both old components and their events, copy pairs and incident edges;
   traverse adjacency and event identities with durable cursors, then propagate
   to turns and usage. No full-machine scan. Current rules:
   `crates/semon-sessions/src/model.rs:1728`, `:2223`, `:2251`, `:2435`.

2. **Collision keys and cross-machine duplicate owners.** Recompute the current
   harness qualification and reserved-colon chain for the affected spelling
   bucket, including names newly reserved or released by rekeying. Keep a durable
   worklist of old/new spellings until that chain closes; retract every former
   key and dependent source/native lookup. Do not adopt unconditional new keys.
   Fan-out: colliding native identities and reachable reserved spellings, paged
   by `(spelling, harness, native identity)`. Cross-machine presentation retains
   configured-source ordering, conflicts and received-copy dropping: recompute
   the served-key bucket from each authorized published machine snapshot and
   bind the result to that machine-generation vector. This request-owned plan
   cannot hold back source-local publication. Public scoped URLs stay distinct.
   Current rules: `crates/semon-sessions/src/model.rs:2101` and
   `crates/semon-sessions/src/union.rs:499`.

3. **Uniqueness-based parent/spawn inference.** Recompute every consumer of a
   changed explicit call bucket, fallback global call bucket or `(parent, task)`
   sibling/call bucket. Count candidates with a bounded accumulator while keeping
   their deterministic winner/order information; do not finalize after seeing
   one page. Preserve explicit storage-parent precedence and exactly-one rules.
   Retract placement if a second candidate makes it ambiguous. Fan-out: all
   consumers and candidates in those buckets, followed by old/new parent families
   and affected turn partitions. Current rules:
   `crates/semon-sessions/src/model.rs:3310`, `:3887`, `:3906`.

4. **Parent precedence and launch matching.** Preserve separate native-header,
   marker and observed-process assertions. Resolve the first decisive marker
   slot with the native valid-EOF rule, then recompute effective parent precedence
   and the existing prompt/path/cwd/time launch candidate selection. Replace old
   call placement and ancestry, including both prior and replacement families.
   Fan-out: consumers of the changed marker/process and indexed candidate ranges;
   paged candidate ranking and old/new ancestor walks. Cycles use today's model
   treatment, verified by the oracle, rather than a new guessed parent rule.
   Current rules: `crates/semon-sessions/src/native_marker.rs:19` and
   `crates/semon-sessions/src/model.rs:3968`, `:4734`.

5. **Relay endpoints, duplicate receipts and heuristic pairing.** Recompute
   exact `msg_id` buckets in today's native model order, then the existing
   name/body/time/order passes for affected candidate groups. Preserve first
   receipt selection under duplicates; do not freeze the first arrival. Internal
   relation identities come from native observations; exported stub keys and
   relation placement retain current semantics. When a real endpoint wins,
   retract stub membership only when no surviving relation needs it. Fan-out:
   all sends/receipts in the ID or matching group, name/path candidates and their
   incident turn partitions; page both ranking passes and retain pass state.
   A late candidate can require replaying the whole affected matching group.
   Current rules: `crates/semon-sessions/src/model.rs:4150`, `:4288`, `:4330`,
   `:4504`, `:4635`, `:4674`.

6. **Reused tool IDs and replacement results.** Keep call/completion occurrence
   observations and recompute attachment for their exact native ID scope in
   native order. Ordinary unambiguous completion resolves P; Copilot duplicate
   IDs and Codex connector replacement create new N attachment revisions,
   including explicit removal of a formerly attached reply. Update earlier
   event interpretations, result references and dependent run/turn summaries.
   Fan-out: occurrences/consumers in the ID bucket and affected owner partitions,
   paged by native source/position; no latest-row offset filter. Current rules:
   `crates/semon-sessions/src/events.rs:4741`, `:5931`, `:6943` and
   `crates/semon-sessions/src/events/store.rs:4380`.

7. **Latest results, titles, state and turns.** Derive selected scalar values
   and run completions from ordered observation references with today's
   precedence, including resumed activity and hand-back fallbacks. Merge native
   events in the current event-time order. Dirty from the earliest changed
   ordering/relationship boundary; recompute forward until the partition state
   and following boundary agree with the prior revision, or through owner end.
   Retract replaced turns, slots, last-turn flags, incoming placements and
   source-turn witnesses together. Fan-out: potentially every turn/event of an
   owner, and relations consuming its source-turn witnesses. Use persisted merge
   cursors and partition checkpoints; never load all owner FileIndexes. Persist
   ordered source runs for a paged external merge rather than one cursor per
   unlimited source in memory. Current rules:
   `crates/semon-sessions/src/model.rs:3433`, `:3576`, `:5031`, `:5710`,
   `:6210`, `:6405`, `:6549`.

8. **Usage ownership and hierarchical totals.** Recompute latest per-message
   reports, then copy attribution for every changed `(message ID, UUID)` bucket.
   Preserve unknown original ownership; arrival order never chooses an original
   branch. Retract old exclusive/shared contributions, recompute owner totals and
   propagate changed values through old/new parents and roots. Parent moves
   retract old family contributions even when token values are unchanged.
   Fan-out: all copies of the pair, affected owners, ancestor paths and changed
   family membership. Page pair membership and deterministic reductions; cache
   intermediate contribution revisions, not rounded cumulative subtraction.
   Current rules: `crates/semon-sessions/src/model.rs:2553`, `:2883`,
   `crates/semon-sessions/src/claude_usage.rs:26` and
   `crates/semon-sessions/src/events/store/semantic_stage.rs:914`.

## Durability, publication and selected reads

### Index transaction

One `BEGIN IMMEDIATE` transaction performs source-incarnation/ledger CAS, Index
advance, new immutable evidence, before/after delta, dependency membership changes
and durable dirty-selector insertion. Commit all or none. The existing
`write_one_dirty` transaction is the starting seam
(`crates/semon-sessions/src/events/store.rs:4402`). Cover batch ingestion,
removal, relocation, usage/tree repair and every other Index mutation too.
A crash after Index advance must never leave semantic work only in memory.

Raw writes remain outside SQLite. Failed parsing/verification leaves durable
source invalidation pending. Acknowledgement removes only the captured claim
when still equal; a newer claim survives. Unjournaled local changes discovered
after parsing enqueue another observation rather than silently claiming EOF.
Complete-record checkpoints bind consumed bytes, not physical live length.

### Projection transactions

Each work page reads a fixed V, writes private derived revisions, registers its
read dependencies, inserts downstream dirty keys and advances its cursor in one
transaction. CAS uses the task's target and cursor, not the latest physical stat.
Two producers cannot consume the same page twice. Work claims do not require a
wall-clock lease for correctness: a replacement resumes the committed cursor.

A target's transitive dirty work must finish before that target becomes public.
Otherwise a rekey could expose an old source map or a parent move could expose
new ancestry with old family totals. There is no public partially derived row.
Unchanged public rows remain available while private work proceeds.

### Bounded publication

A large dependency closure cannot require one giant final write transaction.
Build a persistent, paged manifest of Catalog rows, relationships, source vectors,
activity, usage, search membership and recipe revision references. Changed pages
use copy-on-write; unchanged pages retain their prior references. Stage retractions
as removal from the candidate manifest, not deletion from the current publication.

Each page is built and validated in bounded transactions. When the work graph at
V is closed and its candidate manifest is complete, one short transaction CASes
the prior publication pointer, installs the new manifest root/generation and
advances the semantic delta position. All projections resolve through that root.
The pointer swap contains no owner-sized loop, source vector or recursive reduce.
The existing in-place Catalog/search/recipe writers must be adapted to this
revision indirection; merely adding a new generation counter is insufficient.

A source vector is a normalized paged relation with a digest/root, not a JSON
array that grows beyond today's 64-source metadata cap. Hashes bind the exact
ordered typed entries and parser/interpretation revision; they are not authority.
Readiness and archive chain evidence remain bound to the same publication.

Crash before pointer swap: old publication remains usable; private pages resume.
Crash after swap: committed root and consumer position agree; retry is idempotent.
A failed pointer CAS reloads committed work ownership/publication state; it does
not restart parsing or reinterpret already committed observations at a newer V.
Retire unreferenced private/replaced metadata in bounded pages after reference
checks. Never collect native bytes, archives or retained provenance as work GC.

### Interpretation retention

A recipe references the derived event/slot interpretation revision it published,
not the mutable latest `events` row. A call at offset A resolved by a result at B
must retain both its earlier pending interpretation and the newer result-bearing
interpretation while referenced. Keep immutable recipe field/range references,
attachment selection, turn membership and proof bindings for those revisions.
Source bytes remain source-backed. Filtering latest rows by `offset <= old_end`
cannot recover the earlier interpretation.

Selected reads resolve Catalog identity, manifest, recipe revisions, source
membership and proof pages in one SQLite read snapshot, held through lazy byte
rendering. Reuse the request-owned proof snapshot from #473; retain explicit
continuation/field/error generation checks. An unrelated publication cannot mix
new Index interpretation into that read. A later request gets the new root;
a stale continuation keeps the current documented refusal behavior.

Old referenced proofs and derived interpretations survive Index advancement.
Retained-history roots and active SQLite snapshots also count as references;
reclaim only unreferenced metadata with bounded GC. An unsupported recipe or
missing original source stays explicitly unavailable, never repaired by a
reader inventing evidence. This adds no public historical-version selection.

## Liveness and invalidation

After any usable generation exists, ordinary reads keep using the last good
root while work is pending. First publication consumes a finite admitted input
cut and finite dependency closure. Appends after admission do not invalidate that
cut, enlarge its live inventory or reset its work. With finite supported inputs,
continued scheduling and available source evidence, it completes in finitely
many bounded steps while ingestion continues. No global quiet interval is needed.

Use FIFO admission of committed deltas with bounded source fairness; selected
read interests may prioritize work without starving ordinary sources. A hot
source cannot replace an already started target. The consumer may lag if input
arrives faster than available compute; the guarantee is progress and continued
readability, not zero lag or a universal wall-clock first-list deadline. Discovery
completion remains separate from a usable generation and cannot be asserted while
inventory/relevant source work is pending.

| Input or limit | Replacement behavior |
| --- | --- |
| 64-source owner closure | Remove as a semantic availability limit. Keep 64 as a physical preparation/page budget. Page owner membership, dependency traversal, ordered transcript merge, proof references, native-resolution validation and eviction-readiness checks; no complete owner must fit in RAM or a 64-element serialized vector. |
| Sidecars/whole JSON/PID inputs | Record immutable qualified replacement observations and dirty their reverse dependencies, including unpublished owners. Source role changes create scoped reinterpretation work; no machine-wide stage reset. |
| Facts/names/process/runtime | Compare actual served inputs, preserve heartbeat deduplication, invalidate indexed selectors and their negative consumers. Root-selection changes can affect many sources but enumerate them durably. Current live state remains independently qualified and never becomes retained runtime authority. |
| Rewrites/truncation/replacement | Refuse the affected source revision, withdraw append trust, invalidate its dependent keys and start qualified source-incarnation repair. Do not serve changed bytes under old recipes. Unaffected owners remain available. |
| Authorization/custody withdrawal | Apply denial immediately at read admission and before/after byte I/O, even while semantic work is pending. A last good row is not permission to read revoked content. |
| Relocation/archive/sealing | Reuse qualified identity/prefix/chain evidence; update source mappings and their dependents. Do not treat a stale mirror as a second current recording. |

The first row requires reader changes as well as producer changes. Existing
native resolution loads one row with at most 64 references
(`docs/design/catalog-native-resolution.md:23`); source readiness has the same
vector bound (`docs/design/retained-native-projections.md:122`). Replace those
embedded-vector assumptions with indexed paged validation and publication-bound
qualification summaries. A small list/selected page must not scan an entire large
owner merely to establish its publication root. Complete Query retains explicit
work budgets and can refuse an oversized request; that must not hide the session
from Catalog or break normal paged transcript browsing.

Physical source-prefix verification, source incarnation/CAS, authorization,
custody and sidecar/facts qualification remain. Remove only equalities whose
meaning was “all phases must still see the latest live generation.” Local
uncooperative writers require verified pinned consumed bytes; receiver append
custody is an optimization when actually available. Producer record reads must
qualify the admitted prefix while Raw grows, rather than repeat exact-live-stat
checks after proving that prefix. Public selected reader physical-I/O fences
remain as documented in `session-source-reader.md:68`; this design does not
promise successful byte reads through arbitrarily frequent physical changes.

A genuinely changed consumed prefix can invalidate in-flight dependent work.
Confine restart/refusal to that source and its transitive affected keys. Repeated
rewrites of the same dependency can prevent that dependency becoming current;
they cannot reset independent published owners or block all machine work.

## Replacement and reuse

| Remove at cutover | Current code anchor |
| --- | --- |
| Global semantic stage tables, phase flags, key epochs, stage reset/ACK lifecycle and exact cross-phase claims | `crates/semon-sessions/src/events/store/semantic_stage.rs:28`, `:826`, `:1573`, `:2201`, `:2294`, `:2555` |
| Stage selection/preparation/publication orchestration and ordinary-publication exclusion while stage active | `crates/semon-sessions/src/native_evidence.rs:1066`, `:1619`; `crates/semon-sessions/src/events/store.rs:3377` |
| Phase-oriented Batch/Context capture and whole-owner rescans | `crates/semon-sessions/src/model/relation_stage.rs:57`; `crates/semon-sessions/src/model.rs:8308` |
| Exact captured/live CatalogSource substitution as a later-phase admission fence | `crates/semon-sessions/src/model/relation_stage.rs:340` |
| Whole physical owner >64 refusal and giant embedded source-vector assumptions | `crates/semon-sessions/src/events/store/semantic_stage.rs:550`; Catalog/recipe/native-resolution readers |

Keep parsers and normalized native semantics in `events.rs`, `model.rs`,
`native_marker.rs` and `claude_usage.rs`; extract reusable pure reducers rather
than copy their rules into SQL ad hoc. Keep Index transactions, `DirtyRows`,
ledgers, chunk proofs/SHA continuation, append/relocation qualification, source
journals and `OriginTurn::merge`. Keep current source locks and facts authority.
The full builder remains a test oracle after cutover, not an alternate production
path. Stage-only DTOs disappear; useful provenance types move beside reducers.

The current-schema change updates producers, readers, recipe contract, fixtures
and docs together. Old Semon stores are refused without mutation; there is no
stage migration chain, automatic reset or compatibility reader. Rebuild only from
available authorized native/archive evidence through explicit recovery. Sole
retained facts/recipes are not disposable because the new schema differs.

## Correctness oracle and acceptance

The main safety net is differential equality with today's full native builder at
**the same finite source vector V and external observation vector**. Quiescing a
live fixture directory after the streaming run is insufficient: that gives the
oracle different bytes. Capture immutable fixture prefixes, qualified valid-EOF
records, sidecar/facts revisions, configured-machine order and fixed `now`.

Compare semantic outputs, allowing only internal revision IDs, manifest layout
and diagnostic counters to differ: Catalog keys/rows/order/filtering, canonical
native maps and resolution states, lineage/copy masks, parent/root graph,
relationships/stubs/placements/OriginTurn, ordered transcript entries/turns/slots,
usage/shared ownership/cost and current/retained source provenance. Render selected
recipes against the same authorized bytes to compare interpretation, not only
counts. Treat provisional inventory identically on both sides.

Run over sanitized harness fixtures and existing collision, continuation, relay,
spawn, marker, turn, usage and duplicate-ID cases. Generate randomized legal
append/discovery/inter-machine orders, partial lines, qualified EOF markers,
late earlier timestamps, results followed by duplicate IDs and second uniqueness
candidates. Check after every published target, not only at final convergence.
Compare multiple orders at the same final V to catch arrival-order ownership.

Add state-machine/crash tests around Index commit, delta/dirty commit, work-page
commit, manifest preparation, pointer swap and bounded GC. Restart with a fresh
producer between any two steps; inject competing writers and verify CAS/refusal.
Assert no lost dirty work, no double contributions, coherent old readers, and
newer source claims surviving old-target publication.

Liveness CI appends after every bounded step on both receiver and local paths.
Include >64 source owners, >64 incident relationships, large ID/name buckets,
large source backlogs and unrelated facts churn. Require a first readable list
within a fixture-derived finite step bound while appends continue, then require
successive publication targets. Count visited/decoded rows, payload bytes and
source loads per step; do not substitute a timeout-only assertion for bounds.
Exercise source withdrawal/rewrite/interior tamper with restored mtime, truncation,
replacement and sidecar-role changes; require scoped refusal and unaffected-owner
progress. Measure representative two-machine first-list latency separately.

## Independently mergeable implementation plan

All phases require ordinary Rust CI (`cargo fmt --all --check`, locked all-target
Clippy with warnings denied, and `cargo test --locked`) plus their focused tests.
No local cargo build/check/test/clippy, Node tests or project-wide typechecks;
local work follows the host thermal rule. Dispatch pinned upstream conformance
when parser integration changes need it. UI workflow is manual only when visible
reader behavior changes need browser/screenshot evidence. No phase claims runtime
acceptance from static fixtures alone.

### 1. Differential oracle and dependency indexes

Scope: introduce the finite-V fixture driver, exact output normalization and all
selector/reverse/negative dependency indexes. Instrument existing derivation to
record dependency reads, including absent candidates; audit every Index writer.
Index advance plus dependency maintenance is transactional. No production
maintenance switch yet; main uses the existing stage.

Files: `crates/semon-sessions/src/model/tests.rs`, `events/store.rs`,
`events/store/semantic_stage.rs`, proposed `model/differential_tests.rs` and
`events/store/catalog_dependencies.rs`; current schema tests and contract docs.
Tests: existing full-builder parity, eight N examples, old/new selector membership,
negative lookup wakeup, SQL plans and paged high-fanout enumeration. This is the
first independently useful foundation; it does not unblock staging.

### 2. Immutable evidence, durable deltas and interpretation references

Scope: extract source-position evidence alongside the existing parser, persist
before/after deltas and initial dirty work in every Index transaction. Retain
interpretation inputs and proof references needed by admitted V. Add restartable
ordered semantic-consumer position behind an internal test/config switch.

Files: `events.rs`, `events/store.rs`, `source_change.rs`, `source_chunks.rs`,
`events/store/chunk_proofs/published.rs`, proposed `events/store/catalog_delta.rs`
and dependency module. Tests: late result/duplicate ID before-images, every writer
path, crash atomicity, claim survival, rewind/truncation and qualified-prefix reads.
Main still reads the old path; new writes are current-schema metadata for the
candidate path, with no old-version conversion. No staging availability claim.

### 3. Incremental identity, copies and attachment reducers

Scope: implement M/P reduction plus N cases 1, 2 and 6, with source membership,
collision spelling worklists, copy masks and paged component split/merge. Extract
shared native rules. Add private derived revisions and bounded work cursors.

Files: `model.rs`, `model/relation_stage.rs`, `events.rs`, dependency/delta modules,
proposed `model/catalog_identity.rs` and `events/store/catalog_projection.rs`.
Tests: differential identity/copy/attachment outputs at each V, >64 source component,
reserved-spelling cascades, retraction and cursor restart. Main continues the
old production path; incomplete new projections cannot serve API responses.

### 4. Relationships, turns, usage and bounded manifests

Scope: implement N cases 3–5 and 7–8, family contributions, paged external event
merge and source vectors. Build candidate manifest pages and atomic pointer
publication. Cover all existing output projections, including activity/search,
retained custody and chain evidence. Complete shadow comparisons against the
full builder; the internal switch can select the new producer in CI only.

Files: `model.rs`, `model/relation_stage.rs`, `native_marker.rs`, `claude_usage.rs`,
`events/store.rs`, projection/dependency modules; proposed
`model/catalog_relations.rs`, `model/catalog_turns.rs` and
`events/store/catalog_publication.rs`; current activity/search publication modules.
Tests: all eight N cases across append orders, property/crash suite, turn-provenance
changes, old/new family moves, large buckets and a final pointer-swap cost bound.
Main stays on the old path; this phase establishes a complete candidate algorithm,
not an early product with weakened relationship or usage semantics.

### 5. Reader integration, cutover and stage deletion

Scope: make Catalog/list, native resolution, recipes, Query and source-readiness
consumers use one publication root and paged source membership. Pin interpretation
revisions through byte I/O. Route NativeEvidence to streaming work, remove the
internal switch and old production stage in the same PR. Update current schema,
recipes and affected contracts together; retain full builder only for tests.

Files: `native_evidence.rs`, `events/store.rs`, `events/store/semantic_stage.rs`
(deleted), `model/relation_stage.rs`, `catalog.rs`, `query_source.rs`,
`query/transcript.rs`, `origin_projection.rs`, `union.rs`, proof/publication modules
and the five governing design docs. Reader submodule names must be confirmed in
implementation; this is a scope inventory, not permission to omit their callers.
Tests: complete differential suite, selected old-snapshot/new-result races,
retained originals, scoped rewrite refusal, >64 source list/transcript/resolution,
continuous-append first publication on receiver and local paths, fresh-process
restart and unsupported-store refusal without mutation.

**This is the earliest phase that unblocks staging on the normal supported path.**
The CI-only switch in phase 4 is diagnostic, not a staging rollout or a second
production contract. Phase 5 is releasable only when the continuous-append and
output-parity gates pass for its exact head; no implementation phase retains the
old stage after cutover as a compatibility fallback.

### 6. Production-scale qualification and bounded cleanup

Scope: qualify two-machine cold receipt, busy local logs, long owners and hot-key
fan-out under representative contention. Tune existing page scheduling only on
measured evidence; remove unused stage-only helpers/tests/docs references and
verify GC bounds. Report lag, first-list time and memory/IO separately.

Files: focused native evidence/model/store regression tests, performance fixture
harnesses and `docs/design/native-foundation-performance.md`; task-owned reducers
only for demonstrated qualification fixes. Tests: fixed-work scaling against
unrelated history, sustained producer/restart workload, retained-root GC and
selected byte-read refusal behavior. Main already uses streaming; qualification
must not weaken assertions, remove supported native behavior or introduce a
second delivery path. Staging deployment remains a separate authorized action.

## Owner questions and risks

No identity/key redesign, weaker inference or new provisional row type is needed
for this plan. Those would change the requested outputs and are excluded.
The remaining product decisions can be settled before their implementation phase:

1. What representative receiver/local workload and first-list/freshness targets
   qualify the rollout? Recommend retaining the existing foundation gates plus
   a continuously appending two-machine case with an explicit input rate/backlog;
   an arbitrary universal latency promise is not supportable.
2. For an installation whose old store contains sole retained recipes/facts but
   lacks recoverable originals, what operator workflow should accompany refusal
   at cutover? Recommend keeping it unavailable and untouched, with recovery
   requirements stated explicitly. Automatic deletion/conversion is excluded.

The main risks are missing reverse or negative dependencies, hidden parser
mutations, accidental arrival-order selection, and turning one “bounded” operation
into a whole-owner JSON decode or final transaction. Differential testing is the
primary output safeguard; work counters and crash injection address bounds and
atomicity. Repeated defects in the same class should trigger review of reducer
ownership/dependency declarations, not another isolated guard.

Revision retention and persistent manifests add write amplification and GC work.
Hot semantic closures can still be expensive; this design pages that work rather
than claiming it disappears. Old SQLite readers can hold WAL pages until their
request ends; retain existing cancellation/deadline ownership and measure this.
A single unsupported or repeatedly rewritten source can remain unavailable while
unrelated owners progress. Source-backed transcript I/O keeps its independent
physical fences, so Catalog liveness does not promise uninterrupted body reads.

Estimated scope remains roughly 15–25 production files and 3,000–6,000 production
lines plus substantial fixtures/property tests, subject to the reader/manifest
audit in phases 1–2. This is a design estimate, not an implementation result.
