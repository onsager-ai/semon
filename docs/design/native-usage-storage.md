# Native usage facts and bounded projections

## Baseline and data flow

The inspected Source base is `e0c0b1685511bb59e7f5644e979a33c20ae10095`.
Hub base `290d55b6f3bdb9ac17e011dbf47460e8e1e3ca1c` pins that exact Source
commit. Both default branches were `main`. Existing foundation and native
formatter work was inspected. This change addresses the usage/metadata failure
within #383/#384 and Hub #122/#124, not those entire trackers.

Native bytes -> the existing Event Index parser -> file ledger, event rows and
usage facts/identities -> incremental aggregates -> Tree metadata, Catalog and
overview publications -> their native, CLI/MCP and authorized Hub consumers.
No second database or parser is introduced. Analytics consumes overview/activity,
not Tree's metadata. Hub keeps ownership of authorization and source unions.

| State | Authority and growth |
| --- | --- |
| Native bytes and file ledger | Original evidence; stat, consumed offset, prefix/tail SHA and block proofs |
| `events`, `signals`, `usage`, `usage_identity`, `codex_usage` | Existing content-free Index facts; selected-history cardinality |
| `native_usage_sources`, `native_usage_current` | Physical source incarnation and current receipt mapping |
| `native_usage_facts`, `native_usage_identities` | Per-observation evidence and source-local first accepted identities; linear history |
| `native_usage_dimensions` | Incremental model and cost model/day accumulators; actual dimension cardinality |
| Tree generations, models, tools and usage models | Derived, indexed dimensions; historical closure is retained |
| Tree root, migration roots | Scalar accumulators and versions; no complete normalized usage histories |
| Catalog, descriptions, overview/activity | Existing fenced, derived publications for their public read contracts |

Before this change, `files.codex_native_usage` held sorted `seen`, totals,
`by_model` and insertion-ordered `events`. Tree repeated that full structure.
The 1 MiB writer ceiling deleted an oversized Tree root. On restart, unchanged
repair wrote it again and unconditionally invalidated the source; a saved
semantic claim then failed `witness_claim`. The witness was correct. Neither
raising the limit nor changing its expected revision resolves that cycle.

## Usage identity, availability and billing

Explicit per-response reports are decoded by the existing shared Codex rules.
The exact native four-ID tuple is scoped to a physical source incarnation, not
globally unique across files. A normal append retains the incarnation; verified
rewrite/truncate/replacement allocates another. A qualified relocation rebinds
its current path without changing the original facts' provenance.

Every observation stores the typed payload, native byte offset, length and SHA,
nullable model/time, and an explicit accounting outcome. Repeated identical
reports and conflicting reports remain facts. First valid observation wins,
matching the native contract; payload conflicts are recorded as `conflict`.
Replaying an offset requires identical payload AND record evidence, otherwise the
transaction aborts. Identity set insertion alone never conceals a payload conflict.

Tree and model eligibility are separate scopes. Tree accepts a valid explicit
report before model attribution; model/cost accounting starts once a model has
been observed. A fork establishes explicit availability with zero totals before
its first report, suppressing inherited cumulative counters. Missing explicit
usage, explicit zero, unknown model and unknown time remain distinct.

`codex_usage` still contains deltas of legacy cumulative counters. It is not an
explicit per-response table and is never combined with explicit facts by SQL SUM.
Claude keeps last-message usage and exact assistant UUID/message memberships;
cross-file copied usage continues through the existing lineage resolver. Equal
message IDs without the required matching record UUID are not copies.

Raw counters remain JSON unsigned integers; no SQLite signed narrowing, SQL SUM
or floating-point token conversion is added. Costs still apply the existing
per-event billing function, model normalization, timestamp and uncertainty rules.
The accumulator is persisted separately from the raw counters, with model/day
rows updated only when touched. The current pricing table and derivation rules
are unchanged. A future pricing policy change is semantic and must version/rebuild
its affected cost publications; it must not be treated as representation repair.
Raw model, timestamp and billing counters remain available for that derivation.

## Tree and Analytics consumers

Tree format 3 stores scalar line/usage state plus the receipt and parser
version. Current activity/path labels, models, historical tools, closed IDs and
retained Claude usage-model labels live in indexed rows. Large current labels
remain complete without inflating the root record. A cold Tree read restores models and currently open
tools, with targeted lookup for a requested historical tool. Copied Claude
memberships are maintained with existing usage-identity insertion/deletion and a
bounded old-database bootstrap. Only copied reports needed by the selected owner
cohort are loaded for attribution, rather than all historical UUIDs/messages.

A Tree snapshot binds the file ledger, physical incarnation and complete
projection generation. Its targeted follow-up reads recheck the ledger/header.
Warm Tree proof capture reads the ledger-bound proof header; existing focused
readers obtain and authenticate only the reached proof pages and native blocks.
It never substitutes an empty proof or relaxes the source fence.

The root is bounded by current scalar values rather than report count. Models,
open tools and copied evidence can legitimately grow with the selected result;
those results are not claimed to be fixed-size. A current label can also be
large; its native contents are preserved in a directly indexed value. Historical closed tools remain
in the indexed generation. Explicit all-time cost outputs retain their existing
model/day response dimensions. Selected full transcript/semantic Index reads
still have their existing event and Claude-message cardinality; this work does
not claim to finish the general focused-query tracker.

Analytics retains its sixty-day activity projection for the maximum thirty-day
request and comparison windows. Its cost-day series, busy intervals and answered
questions already follow that horizon; unknown usage/time, time zones, comparison,
lineage and unpriced model rules are retained. Cost and incomplete-usage day
selection uses ordered range reads at this horizon. Ordinary overview/Analytics reads
use those publications and do not reconstruct native identity/event histories.
No parallel Analytics service or UI is introduced.

## Transactions, versions and invalidation

The additive schema is version 30; parser cache stays 24. Explicit usage root
format is 1 and Tree format is 3. Source incarnation, source receipt, projection
generation, migration cursor, dirty revision and Catalog generation are distinct.
Older writers are rejected by the existing schema/parser checks. No global
parser reset, raw deletion or published-generation clearing performs migration.

Receipt CAS, native observations/identity outcomes, dimension deltas, Tree state
and Index checkpoint are committed together. Stage progress/failure writes also
check the current schema/parser and the selected migration receipt/cursor. A
stale slice cannot overwrite a successor's state. Temporary generations are
private until the source ledger/stat and migration cursor still match at commit.

Equal receipts with unchanged independent Claude sidecar interpretation mean
representation repair and do not advance dirty revision. Actual append, rewrite,
removal, sidecar changes, receiver selection/facts and relationship dependencies
retain their invalidation owners. Semantic claim, source membership, ledger,
runtime/facts and publication CAS checks are unchanged. Last-good Catalog rows
retain only their existing compatible, authorized source evidence. A partial
publication does not gain global completeness or destructive authority.

The existing timing counters distinguish `tree_repair`, `representation_repair`
and `invalidation`. Persisted migration state/cursor and semantic phase/epoch
show progress; existing constant semantic rejection tags distinguish claim,
ledger, runtime and publication failures. Diagnostics do not print raw content
or identity collections.

## Bounded migration and recovery

Schema upgrade only installs additive tables. The producer discovers at most 64
ledger rows per keyset step and advances one round-robin source slice. A slice
processes at most 512 complete native records and targets 1 MiB. One native
record may cross that target, with a hard 32 MiB record ceiling: the explicit
maximum is less than 33 MiB of records plus the fixed reader/proof buffers.
Records exceeding the cap are explicitly `blocked_evidence`, never truncated.

The migration uses the existing sealed native file, focused proof reader and
shared decoders. It never zips sorted IDs with events, parses a growing legacy
JSON on each slice, uses growing OFFSET, or manufactures an ID/event association.
Private Tree work tables carry only exact usage/identity state needed between
slices. Their completed work is deleted in indexed batches of at most 512 rows
per work table. Copied-membership bootstrap also advances at most 512 identities
per source step. The replacement dimensions and root become visible together.

The transaction persists cursor, last model, scalar roots, source incarnation,
private projection generation, state and fair scheduling clock. Restart/replay
resumes that checkpoint. A verified concurrent append keeps the incarnation and
processed prefix; rewrite resets the private projection. Final receipt/stat checks
reject changed raw bytes. Cancellation checkpoints occur during reading and
before commit; an interrupted transaction leaves the previous checkpoint intact.

Missing raw data or insufficient proof keeps compatibility data and a precise
blocked state. It cannot be claimed migrated or interpreted as an empty source.
The compatibility accumulator exists only until that source's migration succeeds;
normalized writers no longer restore or serialize its complete `seen`/`events`.
An in-memory reader without a store retains its own uncheckpointed identities;
they are discarded only after a durable commit. Once compact totals depend on
SQLite identities, losing that store returns an explicit unavailable error until
it is reopened, rather than appending against incomplete deduplication state.
Large foreground metadata repairs are deferred to the worker. Real appends can
advance their Index receipt while Tree remains explicitly unavailable, allowing
the worker to resume instead of deadlocking on a stale receipt.

Malformed compact roots enter the same repair path. An oversized current scalar
records a receipt-bound `blocked_metadata` condition and retains prior data;
it is not silently deleted and repeatedly repaired. The 1 MiB limit is unchanged.
Small foreground compatibility repair has a fixed 1 MiB source-byte ceiling.

## Reproducible verification

The pre-fix synthetic test at `d649527d`, CI run `39223137561`, failed after 8,192
reports and restart: unchanged processing advanced revision 1 to 2. Its eight-report
control completed. No private production logs were used.

CI `39232839035` verified old-schema migration, duplicate/conflict/unknown/zero and
incarnation behavior, append/rewrite/missing-source recovery, and Tree restoration.
It measured roots of 616/633 bytes for 8/8,192 Codex reports; the old 8,192-report
accumulator exceeds 1 MiB. Codex migration completed in 32 steps including work
cleanup, at most 512 native records per slice, without changing revision 1.
Claude 8,192-report repair completed in 48 steps with a 510-byte root. Tree tests
at 8/4,096 historical tools restored two active tools with zero full Index loads.

The regression also saves a real semantic epoch, corrupts the derived root during
identity work, releases memory and requires indexing, identity, relations and
publication to complete under the same epoch/revision. Separate existing stale
claim/ledger/runtime/publication tests remain mandatory. The final PR records the
exact final run and paired Hub results; earlier successful runs are supporting
evidence, not qualification of later untested edits.

The three-scale check (8, 512 and 8,192 reports) measured three restored usage
dimension rows, zero restored native identities/events and zero full Index loads
for Tree. Appending one report changed exactly 24 SQLite rows at every scale,
inserted exactly one immutable fact, and used the identity table primary key.
Cost outputs matched exactly before and after serialization. The existing Rust
workflow owns complete workspace/feature Clippy, tests and CLI/native contracts;
the additional focused workflow owns the new synthetic measurements.
