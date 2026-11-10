# Origin analytics projection

Implementation plan for #557, following the accepted [store](asi-store.md),
[federation](asi-federation.md), and [governance](asi-governance-api.md) contracts.
#593 owns subsequent federated publication. Storage owns #578–#581 and the query
and FTS work; this lane does not edit those stack branches.

## Input and projection on main

`semon-asi` owns a typed index-input trait and pure whole-session projection.
`semon-sessions` implements the trait over its internal activity readers. Read
index incarnation, change sequence, catalog, complete admitted activity generation,
StepFacts and provenance in one SQLite read transaction. The adapter owns strict
incarnation/sequence checkpoint encoding and validation; incarnation changes cause
a full namespace read. A pending or unavailable generation is distinct from an
empty admitted session or an explicit removal.

The ASI session UID is `(configured machine namespace, native harness, native
session ID)`, percent-encoded component-wise. The Catalog key only disambiguates
source-local rows and is never substituted for the native ID: collisions and
parent links resolve through the recorded native identity. A Catalog row with no
single native identity is unavailable rather than guessed. `codex-run` is a
worker when its recorded native parent resolves; otherwise its role remains solo.

Project steps, ordered tool calls, turns, sessions, compactions and dimensions
from admitted SQLite evidence. Preserve native UID, lineage, source witness and
exclusive/copied usage semantics. Copy recorded labels, triggers, cost components
and pricing fingerprint; do not classify or price again. Preserve all billing
inputs and unknown values. `dim_price` must describe the exact snapshot that
priced the admitted steps; investigate persistence of that snapshot rather than
substituting today's rates. Raw target paths remain local typed paths. Export is
a separate governed projection requiring the complete contributing source cohort;
free text is a body and model-provider egress requires per-source opt-in.

Current page/StepFacts readers admit current source/rules/price only. Any extension
for retained generations must explicitly validate complete generation, source
witness, recorded versions and row consistency. Do not bypass current admission
or add readers for older Semon formats. Retain previous published facts while a
replacement is pending or unavailable.

## Versions and coverage

Add the typed per-session rules, transform, price and taxonomy provenance and
coverage metadata needed by the current schema. Distinguish source completeness
from computation current/pending/failed/frozen. Missing or disconnected alone does
not imply frozen. Report session/step counts, exclusive-usage denominators and
unknown/unclassified coverage by recorded versions; do not claim a uniform refresh
from a scalar manifest version. A transform bump reprojects admitted facts; rules
and price refresh remain origin derivation work, queued in bounded batches.

Preserve the last complete admitted generation after partial or total source loss.
Format mismatches refuse explicitly while preserving custody; no automatic reset,
conversion, compatibility layer or deletion of frozen data is permitted.
An older Event Index schema is an unsupported persistent format, even when its
object names look familiar. Producers leave it unchanged; a later, explicit
operator recovery may only proceed after independently establishing the retained
source cohort and custody. A Catalog source-generation change publishes `Pending`
until a complete replacement is admitted; it never emits a removal or deletes the
previous complete facts. `Removed` is reserved for a Catalog identity that is no
longer present.

## Integration after the storage stack merges

Rebase onto main after #578–#581 land, then integrate the publisher and local
consumer lifecycle. Publication uses `Update`: checkpoints merge by namespace;
every touched session supplies all its fact rows, including a valid empty result.
Only explicit removals leave the live map. To mark an existing session missing,
set `source_missing` without touching/removing its rows; `source_restored` clears
the marker. Missing markers must refer to live sessions.

Dimensions replace whole tables. Omission retains the previous dimension; an
explicitly empty typed batch clears it. Build complete dimension state including
entries needed by frozen sessions. Clarify `Versions` documentation: manifest
versions are writer targets, while untouched sessions retain recorded versions.

A reader loads the manifest once, its matching live map and exactly its named
files. Filter session facts by `(session_uid, _gen)`; labels are append-only.
There are no snapshot leases. Bound setup, execution and fetch well below the
10-minute retirement window and do not retain reusable snapshots beyond it.
Use the existing analytics lock for the single writer. DuckDB reads the published
projection; it performs no second derivation or central repricing.

Wire local origin consumers first, consistent with namespace authority; imported
receiver heads are not rebuilt from local indexes. Implement the #557 export/reset
surface with explicit custody/ownership checks and no automatic reset. Transport,
channels, cache, replicas and query/FTS implementation remain outside this slice.

## Verification and delivery

Use synthetic/sanitized fixture goldens and compare incremental with full-refresh
live rows: delayed results, late records, rewrite, midnight crossing, removal,
incarnation change, copied usage, mixed versions and source loss/restoration.
Verify complete-cohort admission and preservation of recorded cost and unknowns.
Use CI for compilation, Clippy, tests and repository gates; no local cargo/node
builds, tests or project-wide typechecks.

- Before CPU-bound local work, run `thermal now`. Exit 75 means 85 °C or above,
  or an unreadable sensor: stop and use CI. Exit 69 means an unsupported machine,
  so the rule doesn't apply.
- Run heavy commands as `thermal gate -- <cmd>`. It starts below 75 °C and kills
  at 88 °C, or on two readings at or above 85 °C. `heat-gate`/`cargo-gate` still work.
- Don't use ~/.cache/thermal-watch/temps.log any more; it's frozen and its service
  is gone.
- Unchanged: no local cargo/node test suites or project-wide typechecks; use CI.
  Bounded reads, one file under about 10 MB.

Open a draft PR from main with independently compilable work. Stack-dependent
integration waits for merge. Obtain independent review and exact-head pull_request
CI against main before reporting merge-ready; PM retains merge approval.
