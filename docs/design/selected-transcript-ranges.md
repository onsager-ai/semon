# Selected native transcript ranges

This staging contract extends #319 after the versioned metadata catalog. It
preserves native parsing/rendering and does not instantiate a global event
cache, ownership plan or selected-file index on the request path.

## Storage and observation

Schema 10 stores content-free native slot recipes by `(session_key,slot)` and
per-session projection version, generation and total. Recipes explicitly cover
all native slot variants. They retain native offsets, block/turn identities,
reply references and necessary source context, excluding message/tool bodies.
Live and inferred unfinished outcomes become unknown in stored recipes;
background process liveness is never retained. A complete untrimmed build
publishes recipes inside the existing source-ledger and catalog-generation CAS.
Retired sessions are removed atomically. Slot generation includes selected source
revisions and excludes unrelated sessions. Native original files remain the
source of rendered bodies; projections are rebuildable.

## Typed request boundary

`session_transcript_range(options,source_key,query,reader)` takes a request-borrowed
optional provider reader. Hosts choose the authorized configuration/source;
readers and credentials never enter workspace/cache state. The local reader
opens sealed sources and checks their recorded identity/stat before and after
bounded I/O. Providers retain their own source-generation and authorization
checks. No raw source HTTP endpoint exists.

`GET /api/session-transcript?machine=&sid=&limit=1..100` defaults to the newest
bounded window. `after` is an inclusive slot address, including explicit zero.
The returned range is `[first,end)` and `next` is the next inclusive address or
null. A requested `generation` mismatch returns 409 `stale_projection` with
`resynchronize:true`. Missing selections return404; unavailable projections503.
Only requested recipes plus one preceding/following context slot are selected
through the primary key. Responses include typed scoped identity, the same
nullable catalog session metadata, native TX entries and per-entry provenance.
Native UUID/id and logical source identity provide append-stable `entry_id`;
without native IDs the fallback uses native byte offset/block. Ordinals are range
addresses, never the only DOM identity. Source edits can invalidate fallback
identities. Source disappearance remains visible even if a host supplies verified
retained native bytes.

`GET /api/session-identity?machine=&sid=` returns `{api:1,identity}` without
transcript construction. `GET /api/session-capabilities?machine=` advertises
`catalog-v1` and stable source scope without a model. Multiple configured sources
require an explicit machine key. No global source inventory is supplied yet.
Recognized bounded mode must not silently request the compatibility global model.

## Bounded native bodies and remaining qualification

A page caps requested source bytes at1MiB, provider calls at256, and any assembled
native record at128KiB. Repeated lines share the request-local native parser
cache. Oversized/missing/changed/undecodable source records produce explicitly
incomplete entries; there is no whole-file or global fallback. Metadata recipes
are capped at64KiB each when read. Provider whole-request cancellation/deadlines
remain host responsibility. These caps are protection, not a performance budget.

Large native previews/field expansion are not yet qualified by this slice.
`selected_entry`, `attachment`, `large_native_records` and
`relationship_context` are advertised false. Relationship context is explicitly
incomplete: turn IDs are references, not a reconstructed full turn/handoff graph.
Bounded JSON string spans/checkpoints will enable large-body reads without
concatenating the whole native record or decoding a field prefix per chunk.
Structured/encoded body formats require separate native fixture qualification.
Current coherent publication still builds all background native histories; dirty
component publication remains a separate dependency. Metadata-only q/global
union pagination are also unfinished. This slice does not satisfy all #319
acceptance criteria or claim a complete default Viewer journey.
