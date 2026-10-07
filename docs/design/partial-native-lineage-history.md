# Partial native lineage history

A canonical session may remain current while one native path in its lineage
leaves the local observation. Keeping absent whole catalog rows alone is
insufficient: replacing the current row and transcript recipes used to discard
that path's original provenance and body addressing.

Schema 13 separates the current projection from producer-materialized history
context. `session_history_catalog`, `session_history_projections`, and
`session_history_slots` are allocated only for canonical owners with retained
paths. Ordinary sessions reuse the existing current tables. History metadata
keeps current name, filters, ordering, and relationship fields, and extends its
source references and native identifiers with the original disappeared paths.
Current identity and native identifiers continue to use the current catalog.
Usage/cost summary metrics remain those of the current observation; mixed
owners explicitly expose `summary_context` as incomplete rather than claiming
retained-source totals were recomputed. Per-source retained usage projections
are necessary to qualify complete historical totals.
Neither observation lifecycle nor history identity grants control authority.

The full native producer publishes the history context before replacing current
rows, inside the existing source-ledger, membership, collection-generation, and
revision-CAS transaction. Source mappings for disappeared paths become retained.
The source-bound recipe version must match; metadata is never reinterpreted
across unsupported versions. Stable native source/event/block/recipe identities
prevent duplicate retained entries, without comparing message text. Existing
current ordering is preserved; retained records are inserted by their recorded
native timestamps, preserving their prior order on ties.

History requests select the materialized history header and SQL slot range.
They never reconstruct the union or load native file indexes on a request. The
history collection metadata participates in the collection generation, while
the selected history projection has its own source- and recipe-bound generation.
Readiness looks up the retained exact path and its history header under the same
coherence and pending-invalidation checks as current source readiness. Hosted
readers still establish archive access and strong original-generation custody.
Missing original bytes are not restored. Body text, credentials, provider object
locations, and live process authority are not persisted by these tables.

## Qualification and remaining limits

The baseline native `session_id` continuation fixture actually failed: the
canonical owner remained current, but natural history identity dropped from two
source references to one. The corrected fixture exercises original body reads
through the request-owned provider, current native identity remaining singular,
new current output after repeated cold builds, stable entry identity and
provenance, and absence of restored files or persisted message bodies.
Unsupported recipe versions preserve visible source provenance, mark history
content incomplete, and return not-ready for eviction.

This slice preserves independent and same-file native recipes for disappeared
paths. A retained recipe with a changed foreign-file generation is incomplete;
an existing-source recipe that depends on a disappeared foreign source is also
incomplete. No current source is silently rebound to an older recipe. Supporting
those edges requires versioned per-slot dependency references and a renderer
that can select multiple qualified generations of the same logical path. Native
copy deduplication across different source paths is not inferred from text.
Rewrites at the same still-present path do not become implicit old-version
history. The native producer continues to use its full build and full catalog
generation hashing; bounded dirty-closure production remains separate work.

Selected history metadata has a 1 MiB byte cap; eviction readiness retains its
64-source and eight-owner context caps. Large lineage source-context reads still
need indexed per-range dependency retrieval. These limits are visible failures,
not evidence that arbitrary lineage or background work is bounded.

The isolated storage growth check holds a native selected history range constant
while adding 20,000 unrelated retained recipe rows. It checks unchanged SQL VM
steps, selected native entries, projection generation, response source bytes,
and zero event-cache loads. This is a ranged-storage isolation check, not a
complete cold-start or producer-capacity benchmark.
