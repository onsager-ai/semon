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
incarnation/snapshot/cursor checkpoint encoding and validation; incarnation
changes restart a bounded namespace inventory. Each chunk has its own coherent
read transaction; chunks do not share a long-lived transaction. A pending or unavailable generation is distinct from an
empty admitted session or an explicit removal.

The ASI session UID is `(configured machine namespace, native harness, native
session ID)`, percent-encoded component-wise. The Catalog key only disambiguates
source-local rows and is never substituted for the native ID: collisions and
parent links resolve through the recorded native identity. A multi-ID Claude
continuation lineage uses its recorded owner when the Catalog key is one of that
lineage's native IDs; any other multi-ID row is unavailable rather than guessed.
`codex-run` is a worker when its recorded native parent resolves; otherwise its
role remains solo. Native Catalog parent links have no `parent_source`.
An agreeing environment/marker observation preserves that native provenance;
`parent_source` records observed evidence only when it establishes or replaces
the parent, including marker-only Codex spawns. Catalog projector 9 and SQLite
schema 47 fence this producer contract; older indexes are refused unchanged.
Observed `ancestry`, `environment` and `marker` links, and unknown source values,
are omitted from ASI parentage (`parent_method = none`): they prove neither
native lineage nor workspace/time inference. See the [parent mapping](asi-store.md#identity).

Project steps, ordered tool calls, turns, sessions, compactions and dimensions
from admitted SQLite evidence. Preserve native UID, lineage, source witness and
exclusive/copied usage semantics. Fallback turn and compaction anchors use the
zero-based physical record ordinal captured by native ingest and persisted with
StepFacts/compaction evidence under the same admitted generation. Native IDs take
precedence; byte offsets and activity/compaction row ordinals never substitute
for record ordinals. Validated filename stems keep fallback anchors stable on
relocation; duplicate or ambiguous source contributors refuse projection.
Repeated compaction IDs merge unknown timestamps/step associations with known
values in either order, preserving first persisted ordinal position. Different
known values withhold only that session as `Pending`. Raw row counts and
contiguous ordinals are validated before deduplication.
Copy recorded labels, triggers, cost components
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
longer present. The source-admission transition is a `changed` journal entry in
the same Catalog transaction even when no new activity rows are available, so
incremental and full readers both retain the prior admitted generation while
observing the new physical witness as pending.

## Integration after the storage stack merges

`semon-sessions::analytics_writer` now supplies the first local lifecycle
primitive for this plan. A caller explicitly starts it with a configured
namespace, read-only SQLite index path and analytics store root; no Viewer, MCP,
push/receive or CLI path starts it, and the owner still needs to choose the host
configuration and enablement surface. It coalesces catalog-publication
notifications into at-most-one cycle per interval, opens a fresh read-only
`SqliteAnalyticsInput` for each cycle, and uses the current native activity-rules
version and pricing snapshot fingerprint as manifest targets. It preserves the
adapter's admitted input boundary: the adapter bounds inventory and incremental
chunks using the configured input budget, and the writer schedules continuation
at the same interval until the input reports completion.

Lock contention is an observable no-writer state. Unsupported or integrity input
refuses visibly without mutating the SQLite index, advancing its checkpoint,
resetting state or deleting data; a later notification makes one fresh read
attempt. Direct store I/O and Parquet `External` I/O back off, as do operational
SQLite busy/locked/I/O/full/cannot-open errors. Parquet page-read I/O surfaced as
an Arrow error latches refusal until the next notification; the writer does not
recover its lost typed I/O source. A store error reloads the manifest before
retry because a rename may already have published the generation. Reload success
preserves the resumed operation's retry count. Every publication attempt advances
the interval deadline, including rename-visible failures. Refusals consume only
the pre-operation notification snapshot, leaving later notifications pending.
The service attempts bounded compaction once after publication. Its slow
retirement cadence can run ahead of an interval-gated publication while keeping
that publication pending, but cannot bypass recovery barriers. Shutdown never
schedules retirement.

Rebase onto main after #578–#581 land, then integrate the publisher and local
consumer lifecycle. Publication uses `Update`: checkpoints merge by namespace;
every touched session supplies all its fact rows, including a valid empty result.
Only explicit removals leave the live map. To mark an existing session missing,
set `source_missing` without touching/removing its rows; `source_restored` clears
the marker. Missing markers must refer to live sessions.

Dimensions replace whole tables. Omission retains the previous dimension; an
explicitly empty typed batch clears it. Build complete dimension state including
entries needed by frozen sessions from the named dimension files only, never by
scanning facts. Retain unreferenced dimensions until bounded maintenance can
prune them. Clarify `Versions` documentation: manifest versions are writer
targets, while untouched sessions retain recorded versions.

Shared repository keys (remote identities and `unknown`) carry no machine-local
path; only a repository with its own local identity projects `repo_path`, using
the path identity its key is bound to rather than a per-checkout path.
Price dimensions merge by logical rate key, including price version and all rate
selectors. An unchanged rate merges the minimum `learned_at`, minimum `valid_from`
and minimum non-null `valid_to` independently; the end is null only when every
observation is open. Closed intervals never reopen, independent of arrival order
or batch boundaries; conflicting rates under
one key refuse the cycle without advancing the checkpoint. Distinct price
versions remain separate SCD2 rows. See the [store contract](asi-store.md) for
the complete key and observation merge contract.

The admitted input now takes a caller-selected positive session/journal and step
budget (defaults: 64 sessions/journal entries and 8,192 admitted steps). Inventory
pages Catalog/activity keys through its starting maximum key, recording sequence
S for catch-up. Metadata-only `Pending` identities need no journal entry.
The silver journal is append-only within an incarnation; production does not
prune or rewrite its rows. Any pruning or resequencing MUST change the
incarnation so readers restart inventory. Sequence gaps are allowed, and cursors
ahead of the current journal maximum are refused.
Whole-session atomicity allows a single oversize session. Partial chunks
publish independently and checkpoint their cursor. Only the final inventory
chunk carries `ReplaceNamespace { session_uids }`, with the complete current UID
set, not merely that chunk's changes. It marks prior live sessions absent from
that exact namespace inventory as `source_missing`; rows remain live and other
namespaces are untouched. Catch-up replays changes after S, including sessions
added, removed or changed between inventory chunks. Completion means the cursor
has caught the current journal maximum. Crash/restart uses the manifest cursor;
an incarnation change abandons partial inventory and starts over. See the
[resumable input contract](asi-store.md#resumable-admitted-input) for validation,
consistency proof and the remaining O(sessions) UID/live-map/dimension costs.

`publish_cycle` accepts the budget, writes one chunk as one generation and returns
completion to its caller. The caller may drain or yield between chunks; the 2 s
discovery cadence does not require an entire inventory in one tick. The existing
local writer selects `AnalyticsWriterConfig.input_budget` and continues incomplete
chunks at its configured interval, including after recovery, without another
external notification. This delivery remains only part of #557. The
adapter and publisher share `semon-asi`'s canonical UID component encoder. A
later `SourceRestored` clears the marker.

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
