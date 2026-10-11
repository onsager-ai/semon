# Initial scoped native publication

The focused catalog producer opens the durable event store without restoring every
FileIndex or reported run. An exact source batch uses the existing native model
builder, captures its allowlisted catalog and untrimmed slot recipes, and publishes
those together. Historical reported-run snapshots are selected by indexed native
identity; a write does not read back unrelated snapshots.

A partial publication verifies the collection generation, exact source ledgers
including ctime, source journal claims, and existing source/native ownership in one
transaction. Every affected existing current owner must have its entire current
source closure in the batch. Missing closure, newer claims, and competing publications
reject the write. Only the claimed revisions are acknowledged. Unrelated owners
and pending work are preserved. The collection epoch advances without hashing all
catalog metadata or counting all file ledgers. History extension and source/slot
CAS use the same transaction as full publication.

A first focused catalog demand discovers allowed paths, orders their filesystem
metadata by modification time, and prepares at most 64 native sources. It publishes
before the complete compatibility/background model observes unrelated native
history. Collection `completeness.state` is `partial`, `freshness` is `updating`, and
cursors bind the opaque 64-hex-character publication epoch. Existing databases
without the additive completeness marker are treated as complete. A successful
full reconciliation publishes `complete` and changes the cursor generation.

A partial collection returns `owner_qualification: "provisional"` and null
`native_id` in identity responses, preserving the declared `native_ids` and source
references as read context. It cannot qualify native control selection or an eviction
readiness proof. Explicit History identities remain readable. This conservative gate prevents an undiscovered lineage or harness
collision from granting native control authority under a provisional canonical key.
It does not replace a host's independent durable runtime ownership.

The complete background scan captures only durable ledger metadata before source
discovery for disappearance reconciliation. It removes a disappeared file only if
that observed ledger still matches, preserving newly committed concurrent sources.
Selected batches neither restore unrelated FileIndexes nor prune membership.

An already complete Catalog can republish up to 64 changed isolated sources
without reloading unrelated event indexes. Before native reads, the producer
captures a new filesystem/runtime snapshot and compares it with the last
acknowledged snapshot. Discovery membership, sidecars and runtime inputs must be
unchanged. Both snapshots are retained as observations; a write arriving after
capture remains visible to the next refresh.

The write transaction rechecks completeness, canonical/native identity, source
membership and isolation against persisted parent/handoff/bookmark metadata.
Human asks and questions remain part of the selected session. It uses the
existing partial-publication ledger, generation and revision fences, acknowledges
only its claims, and rolls back if any unrelated source revision remains pending.
It preserves complete qualification only after those checks succeed. Identity or
relationship changes and initial discovery still use complete reconciliation.
The changed-source regression loads one event index with zero or 256 unrelated
sources; the former complete path loaded 257 in the latter case. This is event-row
reuse evidence; filesystem snapshot work and cold reconciliation still scale.

## Qualified workload

The production MachineView fixture holds a recent 60-session page constant and
adds 0, 256, and 2,048 older single-record Claude sources. Older source mtimes are
explicitly preserved. Before the full background observation starts, the real
producer and catalog reader returned that same page in 101, 103, and 133 ms.
Only 60, 64, and 64 native file ledgers existed then. Response sizes were 53,736,
53,960, and 53,960 bytes; the additional 224 bytes are a continuation cursor.
These single-run server fixture numbers include metadata discovery, native batch
preparation, publication and response generation. They exclude browser, provider,
long-transcript, and large-result costs and are not complete journey budgets.

An explicit 60-source preparation fixture returned identical 54,090-byte pages
with 0 and 2,048 unrelated files; neither unrelated file set was indexed. A separate
20,000-run fixture verifies exact historical snapshot restoration and bounded write
readback. Tests also reject newer journal claims without acknowledging unrelated
work and verify full reconciliation restores Current identity qualification.

## Remaining work

Filesystem discovery and complete background reconciliation still grow with total
history. A source batch parses its selected native history; it does not bound initial
source parsing by requested list metadata. Progressive per-source parsing, a durable
bounded dirty dependency worker, separately qualified native owners, canonical alias
resynchronization, and authorized immutable-provider History backfill remain open.
The current partial seam must not be advertised as a complete incremental producer
or as qualified archived backfill. Metadata modification time is a discovery hint,
not native transcript chronology; partial lists are not exhaustive. An import that
writes old history with new filesystem mtimes can put unrelated old sources in the
first batch while the requested recent page still waits for full reconciliation.
The adversarial connection-to-session workload must retain that behavior and pass
its fixed 60-row page, 473-slot transcript and large-field assertions before any
complete journey improvement is claimed.
