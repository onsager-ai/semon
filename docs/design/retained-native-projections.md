# Retained native projections

A missing local source is an observation failure, rather than proof that a
session never existed. Catalog format 3 records `current` or `retained`
lifecycle. Full native publication still checks the captured collection
revision, committed source membership and every complete consumed-prefix ledger
before changing the projection. Sources absent from that complete publication
retain their metadata and source mappings; their lifecycle changes atomically
with the collection generation. Native event-cache ledgers can retire without
removing the original catalog provenance or versioned transcript recipes.

Lifecycle describes the read projection. It does not establish or revoke
credential custody, runtime ownership, harness authorization or current native
selection. Exact Current identity may use qualified retained metadata when
current native facts and independent durable ownership still bind it. Retired
native manifests remain outside Current scope. Explicit retained-history reads
can inspect past identity without gaining current control authority.

Current list indexes start with lifecycle, followed by their filter and keyset
order. Separate unprefixed indexes support history across both observation
states. Current exact-key lookup remains a primary-key operation. Source maps
carry the same lifecycle and have indexed current native/path lookups. Default
current lists therefore do not scan unrelated retained history.

A retained recipe keeps its immutable slot generation and supported recipe
version. Each header also records SHA-256 of the typed source-reference vector;
this is source/projection agreement, not a complete archive-object hash. Source
references preserve the original physical observation and SHA-256 of the full
consumed prefix `[0,offset)` plus its final up-to-4-KiB window. Neither marker
binds bytes after the consumed offset. A host must independently authorize and
verify an archived generation before supplying a requested source range.

`source_projection_ready(options, root, path)` provides a bounded read-only
pre-eviction guard. It never creates an index, reads bodies, opens the global
EventCache or builds a model. It checks exact allowlisted path mappings, the
supported recipe version, per-session source digest and complete publication
marker. It prefers current observations, then qualified retained observations;
pending source observations, ambiguous references, missing headers and unsupported/stale versions return
not-ready. At most eight owner metadata records are considered. Their source
vectors are necessary relationship context for the selected source.

The result is historical source provenance, not permission to remove bytes or
control a native session. Before removing local bytes, an authorized host must
verify the remote original and the actual bytes against the reference, recheck
readiness under its lifecycle gates, and defer eviction if no qualified snapshot
exists. A consumed prefix may be shorter than the archived object; that remains
an explicit incomplete observation, rather than a claim that the whole object
was indexed. Archive locations, credentials and tenant policy do not enter the
public read model.

On parser-cache reset, observations become retained while rebuildable event
rows are cleared. Original references and recipes survive. Unsupported recipe
versions remain unavailable until an authorized producer rebuilds them from the
original source; they are never silently interpreted as current versions.
Native source reappearance can reactivate its catalog key through normal source
CAS publication. This retains the latest observation per canonical key, rather
than introducing arbitrary historical revision browsing. Explicit original raw
record retention remains the host/source contract.

The existing complete background builder and collection hash still perform
global producer work. This change protects provenance and bounds indexed reads;
it does not claim that the subsequent dirty-lineage producer is complete.

Verification at the schema-11 checkpoint includes schema-10 coherent/stale
header migration, parser reset with absent source, native source reappearance,
interrupted lifecycle retirement with the dirty observation still pending, and
a provider-rendered native range after a complete rebuild removed the local
source ledger. The exact-path guard rejects missing indexes without creating
them, stale collection markers, missing source digests and unsupported recipe
versions.

An isolated indexed SQL check holds one selected current row and a three-row
request constant while unrelated retained rows grow from 1 to 20,001. The
returned metadata is identical and the lifecycle/harness/repo covering order
requires 18 SQLite VM steps in both cases. This qualifies the new current index
shape, not complete journey latency, browser work, memory or the background
producer. The reader scope/filter integration is a separate dependent change.
