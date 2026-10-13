# Incremental native Catalog publication

NativeEvidence owns discovery, dependency expansion, native derivation and
publication. Viewer schedules that producer; CLI, MCP and selected Query reads
reuse it. The producer opens the existing SQLite Event Index without restoring
historical FileIndexes. Raw remains authoritative for native content and facts
retain their independent source-selection and runtime authority.

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
it does not execute a separate insertion program for every gram. A concurrent Raw change
or competing publication cannot satisfy a stale claim. An untrusted append that
lands after scanning requeues only its affected paths.

Published Event Index payloads and native description derivations are released.
The worker retains scoped metadata and the existing queue/lifetime behavior;
this change adds no workers. Selected reads reuse the same producer and existing
source authorization, generation and read-cohort fences.

## Progressive initial ingestion and recovery

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
use source revisions. The compatibility producer remains available when a scoped
store is unavailable, with its original conservative semantics.

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
append and again on fresh-owner restart. Append publication took 6.725, 337.024
and 2,717.895 ms; restart took 5.024, 332.998 and 2,709.485 ms.

The source-journal implementation loaded one Index and parsed one 194-byte record
in all three cases. It discovered zero paths, derived one source and published
one row. Logical source reads were 2,512 bytes and source SHA work 2,702 bytes in
all cases. Publication took 11.599, 9.440 and 15.579 ms. Fresh-owner restart loaded
zero Indexes; zero indexed payloads remained resident after publication. These
are individual debug-build fixture measurements, not production latency budgets.

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

The existing semantic component limit remains 64 sources. Oversized connected
components are not yet qualified for resumable semantic publication; they can
exhaust the bounded recovery path. Initial source discovery requires total
inventory metadata work. One selected Index load can grow with that session's
history, and one complete native record may exceed the cold byte budget.
Resident payload counts are not a process RSS guarantee. Worker lifetime, actual
RSS and cross-machine contention need separate production-scale qualification;
endpoint timings do not establish browser journey performance. No deadline,
security assertion or worker count is relaxed by these measurements.
