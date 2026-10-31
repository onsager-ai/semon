# Incremental native Catalog publication

NativeEvidence owns discovery, dependency expansion, native derivation and
publication. Viewer schedules that producer; CLI, MCP and selected Query reads
reuse it. The producer opens the existing SQLite Event Index without restoring
historical FileIndexes. Raw remains authoritative for native content and facts
retain their independent source-selection and runtime authority.

First-release budgets and measurement semantics are defined in the
[native foundation performance contract](native-foundation-performance.md).

## Durable source revisions

Receiver writes use `SessionSourceWrite` before changing Raw and complete it after
fsync under the receiver publication fence. The existing
`session_catalog_invalidations` journal records exact paths and monotonically
increasing revisions. `session_source_mutations` stores only generation-bound
append/relocation proofs or content-free runtime fingerprints. Begin withdraws
prior append trust durably. A crash leaves pending work and conservative
verification, never an acknowledged unindexed change. No second parser, broker
or metadata database participates.

A verified append binds the old and new descriptor identities, both clocks,
lengths and receiver generation. The parser resumes at its committed complete
record offset. Persisted 64 KiB chunk proofs include a SHA-256 continuation at the
last complete chunk boundary. The continuation reconstructs and checks the old
full-prefix digest before computing the new prefix. Only the old boundary chunk
and appended bytes are read and hashed. Missing, corrupt or ambiguous evidence,
replacement and untrusted local changes retain full-prefix verification or repair.
An interrupted Index commit is retried through its ledger CAS.

Whole Claude process JSON and agent sidecars journal their native dependencies.
Sidecar intents fence the underlying log before Raw mutation. Whole JSON inputs
retain their pre-write revision until dependency replay, so dependent logs are
not published twice around that replay. Facts writes journal actual served
changes, including native names, lock holders, process starts, runs, ancestors,
repositories and source inventory. Heartbeat sequence and naming observation
timestamps alone do not stale runtime summaries.

Every receiver-owned facts file used by a producer follows `SessionFactsWrite`:
take the exclusive facts SourceLock and persist invalidation before publication,
then complete the intent after writing the facts and its authority projection.
This includes the reference viewer's normalized `ReceivedMachines` copy; the
incoming mirror file is transport input, while that copy is a producer input.
Its observed-input mode retains metadata discovery: a facts journal does not
qualify the reference receiver's unjournaled Raw mutations as push-driven custody.
An identical normalized heartbeat preserves the copy and creates no new work.
The reference viewer holds its discovery mutex before the facts lock; producers
do not take that discovery mutex. A failed publication leaves durable dirty work
and does not grant a successful journaled notification.

Runtime recipes use per-row fingerprints of those dependencies. Local observations
use the same native process/lock/name inputs and existing allowlisted reported-run
parser. Hosted upgrade witnesses preserve qualified legacy recipes across verified
facts and PID writes without rewriting unrelated metadata. PID witnesses are
persisted before Raw changes and replayed idempotently; changes outside the
witnessed input invalidate their qualification. These fingerprints never grant
runtime or control authority.

## Qualified publication and restart

An existing qualified collection resumes persisted dirty revisions even when its
owner has no in-memory snapshot. Push-driven sources require no historical path
discovery. Untrusted local observations use bounded metadata audits: filesystem
pages examine at most 512 entries and accept at most 64 candidates; current-source
keyset pages inspect at most 64 rows. Audits open no Event Index payload for an
unchanged source. Agent sidecars are checked alongside their discovered log.

The host distinguishes exact journaled notifications from unknown writes.
`changed_journaled` drains durable revisions; `changed` requests a bounded metadata
audit. Explicit recovery demand coalesces an audit; ordinary qualified reads use
`watch`. Read interest checks only persisted pending revisions and backfill state,
so a replacement owner resumes an interrupted publication even when its rows
already answer metadata reads. No pending work means no cold preparation. An audit advances `catalog_observed_at` only after discovery, current-row
checks and claimed revisions complete. Component generations advance independently,
so a partial drain cannot claim a newly observed whole cohort. SQLite journal
mutations reserve the writer before reading evidence, avoiding read-to-write
upgrade races while WAL readers remain available.

Dirty sources expand through indexed current owners, parents/children, handoffs,
aliases, relocation candidates and native identity dependencies. Newly derived
endpoints expand the same component. Claude `session_id`, `continued-in` and
shared bridge identities join their indexed current owners, including newly parsed
links; reverse links share the existing dependency table. Source removal qualifies
retired native IDs through the disappearance journal. The existing native model
derives only that component. Retained rows cannot replace current producer evidence.

Publication atomically checks the collection generation, exact source ledgers,
claims, native ownership, source membership, role invariants and relationship
closure. The receiver facts read fence spans preparation and publication; facts
writes cannot miss an about-to-be-published new owner. Only claimed revisions are acknowledged. Unrelated pending revisions
remain pending and do not block a closed component. Slot recipes, history
extensions and Catalog metadata share that transaction. Search publication reuses
bounded posting inserts and updates gram counts through a selected source-key range;
old posting removal aggregates shared gram decrements for the same bounded cohort
before replacing its current and retained-history documents. Both aggregates use
the cohort keys as the outer loop and indexed source-key lookups; unrelated gram
members are not scanned. Counts, incomplete coverage and documents roll back with
publication. It does not execute a separate insertion program for every gram. A concurrent Raw change
or competing publication cannot satisfy a stale claim. An untrusted append that
lands after scanning requeues only its affected paths.

Published Event Index payloads and native description derivations are released.
The worker retains scoped metadata and the existing queue/lifetime behavior;
this change adds no workers. Selected reads reuse the same producer and existing
source authorization, generation and read-cohort fences.

## Progressive initial ingestion and recovery

The following origin extension remains unqualified. A WIP SQLite milestone
implements only owned discovery and its first ordinary publication; cumulative
completion, receipt-based prior-owner exclusion and PostgreSQL remain pending.
The scoped oversized-component implementation must retain the initial producer
origin before its first partial Current publication. A stage created only after
the newest cohort has published cannot distinguish that cohort from an existing
Current family. This distinction is required: an existing 64-owner family plus a
new physical source still refuses with `semantic_affected_owner_source_budget`.
An initial 81-owner family may progress through bounded source and owner pages.

The initial origin belongs to the existing semantic stage. Initial discovery
enrolls each admitted page and its exact claims in that stage in the discovery
transaction; no whole-inventory copy or in-memory inventory is permitted. The
stage seals discovery before the ordinary newest-cohort publication. That
publication must present the held stage epoch and capture its accepted owners,
physical source cohorts, claims and ledgers before acknowledging them, together
with the resulting live head, in the publication transaction. A backfill marker
or a matching pathname is not an origin witness.

Only Current owners that still match those accepted origin observations may be
excluded from the pre-origin affected-owner baseline. Source availability, native
identity, aliases, collisions, relationship reduction and complete owner capture
still qualify through their existing ports. Derived reset may preserve origin
provenance but cannot preserve an exception whose admission, runtime, source or
publication witness has changed. Competing publications cannot advance the held
stage's live generation. Discovery recovery must fence the origin before changing
its admitted membership. The collection remains partial until every durable queue
and final witness qualifies.

Physical source batches, individual owner closures, prior owners outside the
origin, context payloads and witness pages retain their existing limits. The
cumulative durable stage may span more than one page. PostgreSQL stores such
pages in the existing stage-item family; its stage header and each metadata page
retain their existing payload limits. This origin extension and its first-page
cost remain unqualified until the exact final Source/Hub checks pass.

### Proposed shared origin checkpoint

This subsection is an implementation plan, not an implemented storage format.
SQLite and PostgreSQL must implement the same transitions before integration.
It does not explain the independently observed ordinary-discovery first-page
timings or change their two-second, five-second or sixty-second gates.

The shared private value types are:

```rust,ignore
struct OriginId { initial_stage_epoch: i64 }
enum OriginPhase { Discovering, Sealed, Draining, Invalid, Finished }
struct OriginHeader {
    id: OriginId,
    phase: OriginPhase,
    admission: u64,
    stream: String,
    sealed_revision: Option<i64>,
    first_publication: Option<String>,
}
struct OriginMember {
    path: String,
    admission_claim: Option<i64>,
}
struct OriginPublication {
    origin: OriginId,
    held_epoch: i64,
    expected_generation: Option<String>,
    group_digest: String,
}
struct OriginOwnerWitness {
    key: String,
    metadata_digest: String,
    publication: String,
}
struct OriginSourceWitness {
    path: String,
    accepted_claim: Option<i64>,
    // One scalar representation of the existing Ledger fields.
    accepted_ledger: Ledger,
    acknowledged: bool,
}
```

Namespace, runtime and live generation remain in the existing admitted stage
binding; these values do not introduce another native identity or authority.
`OriginId` is the epoch allocated when that same stage was first created. It is
immutable across holder takeover and derived reset. The existing held epoch
still changes on takeover/reset and fences old callers; no second epoch counter
is allocated. A publication group digest hashes its origin, held epoch, expected
head, canonical selected keys, exact claims and prepared-model witness. Exact
receipt replay precedes the now-stale head/claim CAS, as in existing publication.

Only an owned native discovery producer may create the origin. The proposed
private operations are `enroll_origin_page`, `seal_origin`,
`activate_origin(runtime)`, `publish_origin_group`, `origin_owner_matches` and
`invalidate_origin`. They take the existing admitted native transaction or
`NativeOwnership`; no public journal method accepts a caller-constructed token
as authority. Generic configured journals continue their existing discovery
behavior. Origin enrollment validates the enrolled semantic profile, namespace,
admission and stream. Runtime is unbound during metadata discovery and is bound
once by the actual producer after sealing, before any Current publication.

#### Persistent rows and pages

SQLite keeps the existing `catalog_semantic_stage` row and adds nullable origin
header fields: `origin_epoch`, `origin_phase`, `origin_admission`, `origin_stream`,
`origin_sealed_revision` and `origin_first_generation`. Existing `generation`,
`runtime`, `epoch` and reduction cursors keep their current jobs. The SQLite WIP
represents an unactivated runtime using explicit `origin_runtime_bound=0` while
retaining the existing non-null runtime column. Its placeholder contents convey
no authority; publication requires activation and the held stage binding.
PostgreSQL should encode that same logical distinction as an optional bound
runtime in its header. Existing `catalog_semantic_sources` rows gain immutable
origin membership/admission-claim fields; the normal mutable claim and derived
progress fields are separate. The source row is enrolled in the same discovery
transaction that records its backfill membership and initial invalidation.

Two small witness relations belong to that same stage family:

```sql
-- Conceptual DDL; final constraints/guards require schema review.
catalog_semantic_origin_owners(
  namespace, origin_epoch, session_key, metadata_digest, publication,
  PRIMARY KEY(namespace, origin_epoch, session_key));
catalog_semantic_origin_sources(
  namespace, origin_epoch, source_path, accepted_claim,
  acknowledged, dev, ino, size, mtime_ns, ctime_ns, resume_at,
  head_sha256, tail_sha256,
  PRIMARY KEY(namespace, origin_epoch, source_path));
```

Allocation also proves the Current baseline is empty in that same owned
transaction: no Current Catalog rows, Current source membership, native Current
observation grants or Current proof references. An absent/corrupt derived head
with any such evidence is recovery/refusal, never a new origin; Current and
History remain preserved.

The owner witness references the already required native Catalog observation
grant and canonical metadata digest. It does not duplicate the Catalog row or
mint another grant. `Ledger` is exactly the existing stat/offset/prefix/tail
value; it is stored only as scalar columns, without a redundant serialized
Ledger blob. Those columns permit indexed SQL mismatch detection without loading
every ledger. This is a non-authorizing comparison witness, never a Raw recipe,
lease, source grant or substitute for current canonical qualification. The
current owner row's complete source cohort supplies owner-to-source membership;
its existing membership check must still pass. Source witnesses are deduplicated
by physical path within the origin.

PostgreSQL uses the existing `catalog_stage_items` family. `seq=0` remains a
bounded header; other rows are typed items for source progress, endpoints,
frontiers, relations, origin owners, origin sources and publication receipts.
The physical `stage_epoch` key is the stable lifecycle root (`OriginId` for an
origin stage); the header records the current held epoch. Takeover changes the
held epoch by CAS and never copies all item rows into another epoch.

Add typed item kind, key digest and full key to this existing family, with a
namespace/root/kind/digest index. Full namespace and key bytes qualify every
digest match; collisions refuse explicitly. Work pages seek by digest and
retain that opaque cursor, while each returned `Batch.paths` is canonically
sorted. Do not search an unbounded collection of JSON pages for a path. A typed
item can hold one source/owner or a bounded metadata page; every row payload
remains at most 1 MiB and every fetch returns at most 64 items. A full `Stage`
with all sources/endpoints is never serialized or reconstructed. Existing
publication epochs and the current header supply epoch allocation under the
namespace lock; this adds neither a parallel lifecycle nor a second allocator.
The `seq` value is only an item ordinal allocated under that same header lock;
it is not an origin/holder epoch. Conceptual item mappings are:

| Logical record | SQLite | PostgreSQL item kind/key |
| --- | --- | --- |
| Origin and held state | Existing stage row | `seq=0`, header with stable root/current held epoch |
| Admitted membership and claim | Existing source row plus immutable origin fields | `source`, full physical path |
| Accepted owner comparison | Origin-owner witness row | `origin-owner`, full session key |
| Accepted source comparison | Origin-source witness row | `origin-source`, full physical path |
| Selected-group replay | Group-qualified stage publication receipt | `publication`, canonical group digest |
| Existing derived work | Existing sources/endpoints/frontiers/relations | Typed item with its complete existing logical key |

For example, a PostgreSQL source lookup qualifies namespace bytes, stable root,
`kind='source'`, the existing key digest and the full path. A page selects one
kind with `digest > cursor ORDER BY digest LIMIT 64`; a digest collision or
unexpected full key refuses. SQLite uses its existing namespace/path or
namespace/session-key indexes with the equivalent keyset predicate. Neither
adapter discovers membership by scanning payloads of unrelated stage items.

The existing semantic publication receipt key must distinguish selected groups
within a held stage epoch. Use the deterministic group digest, retaining the
held epoch as a fence and indexed receipt qualifier. One receipt per entire
stage cannot acknowledge a second group. First ordinary publication records its
group receipt in the same stage family; later semantic publications use their
existing proof/presentation authority and selected-group receipts.

#### State and first-publication CAS

1. Under the existing discovery writer reservation/namespace lock, verify an
   absent Catalog head and no competing active stage, allocate the existing
   stage epoch, and enter `Discovering`. Each discovery transaction appends only
   its admitted page and exact claims. A replay checks the existing operation
   receipt and does not append a second membership. No whole-inventory copy,
   hash pass or payload preparation occurs here.
2. The final discovery transaction seals the exact accumulated membership and
   its journal revision. A sealed origin cannot admit another discovery page.
   The producer activates the sealed stage with its actual runtime/admission
   binding. The existing newest64 derivation remains the first builder.
3. The first publisher presents the held epoch and origin id. In the publication
   transaction, require `Sealed`, the exact held binding, no prior origin
   publication and the still-absent expected head. Require every selected path
   to be an enrolled member. Run all ordinary native source, role, membership,
   ledger, physical-generation, claim, proof and presentation checks unchanged.
4. Capture only this group's accepted owner digests and source witnesses from
   those validated rows/ledgers/claims. Publish Current and the existing native
   grants, write the group receipt, ACK only its exact claims, and move the same
   stage to `Draining` with its resulting live generation. All effects commit
   atomically. Any receipt/capture failure rolls back publication and ACK.
5. Later groups require the current held epoch and live head, complete selected
   owner closures and the ordinary current claims. They publish/ACK only their
   selected paths/owners, advance only those progress flags and witnesses, and
   update the stage's live head in that same transaction. Unselected pending
   work is untouched. Empty publication requests finish only after all durable
   queues and final witnesses qualify.

The SQLite first-publication header CAS is conceptually the following guarded
update, inside the transaction that already validates/updates the Catalog head:

```sql
UPDATE catalog_semantic_stage
SET generation=:new_head, origin_phase='draining',
    origin_first_generation=:new_head
WHERE namespace=:namespace AND epoch=:held_epoch
  AND origin_epoch=:origin_epoch AND origin_phase='sealed'
  AND origin_admission=:admission AND origin_stream=:stream
  AND runtime=:bound_runtime AND generation IS NULL
  AND origin_first_generation IS NULL AND resetting=0;
```

Require exactly one changed row; otherwise roll back every publication effect.
Subsequent group CAS requires `origin_phase='draining'` and exact live generation
instead of the absent head. The PostgreSQL equivalent locks the existing source
state row, validates the decoded header/binding, then updates the stable-root
`seq=0` item with `WHERE namespace_digest=... AND namespace=:full_namespace AND
stage_epoch=:origin_epoch AND seq=0 AND active AND payload_hash=:old_header_hash`.
The new payload contains the exact held epoch and live head. Require one row and
retain the full bytes/hash validation. This CAS and Catalog-head CAS are in the
same transaction. Header revision/watermark alone cannot substitute for either.

Completed exact group-receipt replay may return its acknowledgement without
effects before checking a now-stale held epoch/head, matching the existing replay
contract. An unfinished stale-holder request conflicts and writes nothing.

Admission claims are historical discovery evidence, not a frozen post-seal
journal. Initial indexing legitimately advances claims. After sealing, native
index commits, source changes and claim capture keep their ordinary CAS/restart
semantics; origin never blesses a newer claim using a watermark or membership.
The first accepted publication records the actual validated claims before ACK,
including an explicit acknowledged bit to distinguish an accepted null claim
from a missing receipt. For an already published origin source, a changed
claim/ledger/runtime/admission disables its old comparison witness. Only a new
ordinary validated publication can record a successor witness; provenance alone
cannot authorize that publication or bypass the prior-owner refusal.

An origin owner may be excluded from the pre-origin baseline only while its
canonical Current digest and existing native grant match its witness, its
complete Current membership matches the row, all incident source witnesses
still match live ledgers, and the held origin binding is valid. Perform mismatch
and membership selection through indexed metadata predicates, then load only
the selected bounded page. Do not enumerate all origin owners into memory.
Missing/corrupt/inconsistent origin state provides no exclusion. Preserve
Current/History/Raw and either use the ordinary bounded refusal or report a
conflict; never fall back to an inferred `draining` origin.

An origin exception also requires every selected/replayed physical path to
belong to the sealed initial inventory. A post-seal new child cannot be appended
to that inventory or inherit an exception from its parent's receipt. Work that
needs such a path must use the ordinary prior-owner closure/refusal semantics;
its pending journal claim is preserved. For ordinary work, the complete union of
selected changed sources and affected prior sources still must fit 64 before
Raw preparation. In particular, finishing an initial 64-owner origin never
allows a later 65th source to reuse its exception.

Derived reset/takeover may retain immutable membership and accepted provenance,
but must invalidate old held batches/model witnesses and recheck every
exception. Recovery fences/invalidates the origin before replacing admitted
membership. A concurrent publisher cannot adopt or advance its live head. A
new admission, stream or incompatible runtime cannot inherit the exception.

#### Operation bounds and implementation checkpoints

| Operation | Resident/operation bound | Durable progress |
| --- | --- | --- |
| Discovery | Existing 512 examined entries, at most 64 accepted paths | Append the same accepted page and claims; seal in final transaction |
| Index/identity | At most 64 physical sources, existing 4 MiB source slices and complete-owner closure rules | Per-source progress and existing ledger witnesses |
| Relationship preparation | Existing actor/seed and event limits, at most 64 physical companions, 1 MiB context | Frontier/event/relation cursors; no all-endpoint map |
| Witness/context reads | At most 64 metadata items per page, 1 MiB per payload/context | Keyset cursor; no uncapped loop inside one producer tick |
| Publication | At most 64 selected owners and 64 distinct physical sources; existing metadata/projection byte limits | Selected-group receipt, flags, ACK and live head in one transaction |
| Prior owners outside origin | Existing complete 64-owner/source and 1 MiB closure refusal | No origin-based increase or truncation |
| Reset/cleanup | Existing 64 metadata-item mutation budget | Preserve origin root/provenance separately from derived reset cursors |

Receipt records contain keys/digests and source witnesses, not another copy of
all model metadata. PostgreSQL packs them by both item count and bytes; it may
write several bounded typed pages for an already bounded publication group.
An individual oversized item refuses, and a multi-item page is shortened before
the payload limit. Neither `Batch.paths`, a context map, a page nor its payload
limit is enlarged to fit a cumulative stage. Parent child names and incident
placements use the existing bounded metadata/replay ports, not Raw loading of
all children.

Implementation checkpoints, each requiring review before the next:

1. Review these shared types, lifecycle-root/held-epoch distinction, native-only
   discovery hook, keyed item layout and deterministic selected receipt key.
2. Implement SQLite owned discovery plus atomic first-publication transition,
   including rollback, crash/replay, stale holder and generic-journal refusal
   controls. This alone is not an integrable oversized-component fix.
3. Implement SQLite cumulative queues, selected-owner context/publication,
   receipt matching, selected ACK and bounded reset/recovery. Require unchanged
   cold81 and old-window completion, every parent/parent-summary assertion,
   unchanged Domain52 64-prior-plus-new refusal and zero-effects assertions.
4. Implement PostgreSQL typed stage items and the identical transitions with
   independent adapter/conformance oracles. Do not retain a second all-stage
   JSON implementation or different origin semantics.
5. Run cross-backend corruption/missing-witness, post-seal mutation, collision,
   takeover, runtime/admission replacement and competing-publication controls;
   then exact final Source/Hub checks and original first-page/background gates.

No local cargo (including formatting), Node tests, project typechecks or other
CPU-bound validation is authorized while thermal readings are unavailable.
Stop CPU-bound work at 85 C. Validation remains CI-only; the runtime hold grants
no E2B, SSH, cloud execution, deployment or publication authority.

A new machine discovers source metadata in bounded pages. Discovered paths and
backfill state are persisted in SQLite, and committed source offsets survive
restart. Filesystem enumeration itself restarts after process loss; discovered
paths are deduplicated and indexed history is not reparsed. The first publication
favors the newest 64 discovered candidates. Subsequent publications drain the
journal in bounded source batches instead of constructing a global model.

Cold parsing checkpoints after approximately 4 MiB per source, at a complete
record boundary. One native record can cross that budget. New, fully consumed sources share one Index transaction for at most 64 sources;
parsing occurs before the writer reservation, and every ledger and physical
generation is rechecked before any row commits. Partial-source checkpoints
retain their individual transaction. Each commit atomically publishes its Event
rows, native dependencies and consumed offsets; a separately fenced
continuation proof allows the next slice to reuse the prefix. A crash between
those writes safely verifies the affected prefix. Partial Catalog publications
are visible between slices, and cache payloads are released after publication.
Completion checks the publication generation and an empty pending journal.

Partial collections retain provisional owner qualification and cannot grant
native control selection or payload-eviction readiness. Native archived paths do
not change this distinction. Existing collections without a backfill marker are
qualified rather than automatically cold-started.

Broad discovery is reserved for initial ingestion or failed closure/repair.
Recovery records its reason and resumes bounded discovery and prior-current-path
keyset pages, including disappeared sources that disk enumeration cannot find.
Supported disappearance, replacement, inventory and runtime changes ordinarily
use source revisions. Unavailable scoped stores retain conservative repair/refusal semantics; they
do not authorize a second compatibility model.

## Native Codex relocation

Active and archived rollout paths retain the same explicitly verified native
identity. A validated relocation rebinds the existing file ID, Event rows,
chunk proofs and native metadata to the selected destination. Custody proofs can
avoid historical Raw verification; local moves conservatively verify the selected
prefix. Intervening appended complete records are parsed from the old offset.
The Index transaction also records a content-free Catalog relocation witness.
Publication checks the old Catalog generation against the verified prefix and
the destination against its current Index ledger, including any intervening
verified append. The witness survives failed publication and restart, and is
removed only after the relevant source revisions are acknowledged. History
projection uses this witness to avoid retaining the former path as a second copy
of the same transcript. Replacement or mismatched evidence withdraws that trust.
Facts still select the authoritative current path. Stale mirrored copies may
remain and cannot create duplicate current sources or grant control authority.
Native archived state is distinct from retained-history and Hub object-storage
lifecycle. The wire contract is in [the mirror protocol](../mirror-protocol.md).

## Instrumented evidence and limits

Synthetic tests hold one selected append constant with 0, 256 and 2,048 unrelated
single-record sessions. Baseline f8b49cf loaded 1, 257 and 2,049 Event Indexes on
append and again on fresh-owner restart. A fresh-process rerun measured append
publication at 21.359, 474.664 and 3,750.150 ms; restart took 17.701, 477.791 and
3,733.778 ms.

The source-journal implementation loaded one Index and parsed one 194-byte record
in all three cases. It discovered zero paths, derived one source and published
one row. Logical source reads were 2,512 bytes and source SHA work 2,702 bytes in
all cases. Publication took 12.437, 10.022 and 19.302 ms. Fresh-owner restart loaded
zero Indexes; zero indexed payloads remained resident after publication. These
are individual debug-build fixture measurements, not production latency budgets.
Whole-fixture peak RSS, including historical ingestion, was 152,996 KiB for the
baseline and 38,068 KiB for that rerun. Filesystem output accounting was higher
for the new implementation, so these results do not establish improved cold-write
amplification. A subsequent Hub cold run with cohort posting removal completed
0, 256 and 2,048 unrelated sessions in 24, 826 and 6,894 ms, respectively. The
former 10-second completion assertion has been superseded by the provisional
30-second complete-indexing gate; these individual samples do not establish a
production or contention latency guarantee.

Five trusted appends to 4 MiB and 16 MiB sources parsed five records and performed
no full-prefix rehash. Logical reads were 133,754 and 128,312 bytes; source SHA
work was 263,412 and 252,528 bytes. Corrupt continuation state and interrupted Raw
intents exercised conservative full-prefix verification. SHA counters distinguish
full-prefix verification from total source hashing; neither counts metadata
fingerprint hashing or physical block I/O.

A 16,816,826-byte cold source progressed across fresh owners in four steps of
approximately 4.2 MiB, parsing each of its 513 records once without rehashing a
committed prefix. Steps took approximately 330–394 ms and released indexed
payloads. Archive/Unarchive tests preserve the indexed file ID and load only the
selected Index at all three unrelated-history sizes, parsing zero records on a
move and one on move-plus-append. Local metadata audit work still scales with
inventory size; local runtime tests changed one Index at each history size.

Oversized semantic components use durable source indexing, complete owner identity, relationship and owner publication queues in the existing Event Index. Physical preparation remains bounded to 64 source files, with smaller relationship seed batches reserving space for selected native companions. Indexing retains primitive source identity and lineage links until the selected owner's complete physical closure can be derived; it does not accept partial copied or continued owner observations. Native keys use the shared harness collision contract before authoritative capture. Indexed ambiguity selectors and bounded parent, root and family-cost reductions retain graph semantics across batches. Owner publication replays complete incident native placements before the shared turn and slot derivation; it never combines independently derived partial slots. Native locations bind the exact source generation, offset and block. Candidate metadata comes from normalized Event Index records in bounded row and byte pages, using the existing continuation-only copy policy. Relation metadata contains no prompt, message or tool-result bodies. A replacement owner resumes these queues with the same source claims, runtime and publication fences. The collection stays globally partial until all queued work and final witnesses qualify, preserving strict archive readiness. Retired stage records are reclaimed in bounded metadata pages; retained source and archive records are preserved. These changes are unqualified until the exact final Source/Hub CI pair passes. An individual owner whose physical source closure exceeds 64 remains explicitly unavailable under the existing physical preparation budget.

A relationship batch may also advance a selected Claude companion when the shared matching observation covers its entire physical owner and every eligible native candidate. Qualification, retention of all incident edges and cursor advancement share the existing publication CAS. Candidate inspection uses one shared 64-event budget; partial companion observations remain pending. A Current native key reused by a different harness preserves the original retained owner, harness and recipes independently. Explicit retained-history identity reads select that original owner only with a qualified header and source cohort; list filters and ordering continue to use Current. Unsupported original headers and raw recipes retain custody without becoming qualified. A selected-owner transfer that exceeds 64 recipe rows or 1 MiB refuses the transaction rather than truncating old custody; this does not change normal same-harness history composition.

Schema 26 adds compact derived search memberships without changing native ledgers, recipes or source authority. A source/gram row carries independent nullable Current and retained-history ordering timestamps, with separate partial ordering indexes; zero remains a valid timestamp. Format 2 reads both the existing postings and compact memberships. The additive upgrade promotes the supported search-format marker without rebuilding legacy history or changing completeness and backfill flags. Normal publication replaces only its bounded selected keys; an update to one scope preserves the other scope before removing the selected legacy representation. Each old scope is qualified against the existing 1,024-gram budget, and a compact owner is first checked against their combined bound. Mixed representation for one key fails closed. Search merges the two indexed representations under one 513-position lookahead and the existing 512-candidate and 2 MiB verification budgets. Scope counts, documents, coverage, filters, ordering and continuation semantics remain independent and unchanged. This storage change addresses measured search page-cache spills; its performance remains unqualified until the exact final Source/Hub pair passes the provisional 30-second complete-indexing gate. The original 10-second
result remains a nonblocking optimization benchmark.

Initial source discovery requires total
inventory metadata work. One selected Index load can grow with that session's
history, and one complete native record may exceed the cold byte budget.
Resident payload counts are not a process RSS guarantee. Worker lifetime, actual
RSS and cross-machine contention need separate production-scale qualification;
endpoint timings do not establish browser journey performance. No deadline,
security assertion or worker count is relaxed by these measurements.
