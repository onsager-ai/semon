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

## Qualified string fields

Recipe version 2 indexes plain Claude user, assistant and thinking strings during
complete observation. It stores only an allowlisted field pointer, scalar-safe
raw offsets/checkpoints and native event identity. It does not persist body text.
Structured user content with multiple blocks, attachments, Codex records and tool
formatting wrappers remain unsupported by this field path.

Selected previews decode at most 4 KiB of text from at most 4 KiB + 11 raw bytes.
A clipped entry exposes `field: {name: "text", chunks, complete}`. The scoped
`/api/session-entry` endpoint requires `sid`, `after`, `limit=1`, the observed
projection `generation`, and a `field_chunk` ordinal. Each ordinal addresses one
persisted scalar-safe checkpoint interval, capped at 64 KiB + 11 raw bytes and
128 KiB decoded text. It never reads or decodes earlier field bytes. A generation
change returns the normal 409 resynchronization response; unqualified fields
return 422 and unavailable source ranges return 503.

`selected_entry` advertises this qualified endpoint; `large_native_records`
remains false because native formats outside this allowlist remain incomplete.
Producer indexing still rereads qualified source records during a complete
rebuild, and repeated slots can duplicate producer parsing. This work is outside
the bounded request path and remains a measured background-refresh concern.

## Common tool fields and host dispatch

Recipe version 3 adds string Claude `tool_result.content` and Codex
`function_call_output.output` (including an object with a string `output`). A
qualified result field feeds the shared native tool renderer; the small native
call supplies its name, argument preview and recorded outcome. Call context that
exceeds the request record budget remains visibly incomplete. Codex formatting
frames, provider truncation markers and embedded JSON strings that require
another decoding layer remain unqualified pending the producer mapping contract.
The result descriptor and expansion response use `field.name: "out"`.

Native slot UUIDs are persisted as metadata so a failed byte observation retains
entry identity and provenance. Useful already-loaded content can remain visible
with its incomplete observation overlay. The bounded response also reports
`content_observation` separately from local source/facts/native-selection
freshness. Its source generation hashes fingerprint opaque reader generation
identifiers, rather than purporting to hash returned content.

Hosts can opt into `session_transcript_range_with_mode` or
`session_entry_field_with_mode` with `LocalThenProvider`. Only a missing original
local source falls through to the request-owned provider. A changed/replaced
local source, permission denial or missing/corrupt sealed segment must resynchronize
and does not silently fall through. The host must authorize before and after the
complete call because its callback is not invoked for qualified local bytes.
Default provider-only behavior remains available.

`session_catalog_capabilities` and `SessionReadEndpoints` provide pure capability
advertisement for an already-authorized source without creating a ViewerCore or
restoring/discovering sources. The public default Viewer exposes the stable
catalog-only `local` source alias; legacy internal identities remain unchanged.

### Configured source inventory

`GET /api/session-sources?limit=60&cursor=...` returns API 1 `items` with
`source_key` and `label`, plus an opaque `next_cursor` or null. Limits are 1–100;
clients percent-encode the returned cursor once. A changed configuration returns
409 `stale_cursor`. Labels describe configured source keys, not runtime health.
The original single-machine Viewer advertises `local` as its explicit catalog
source key. Duplicate or empty configured keys fail explicitly.

`SessionSourceInventory` is a reusable metadata-only keyset registry. Hosts build
it when their authorized configuration changes, then reuse it for paged reads.
ViewerCore builds the fixed configuration index during construction; focused
source lookup uses the same ordered index and never initializes a native model.
The received-directory compatibility mode still discovers machine metadata in
`follow()` on each request. Removing that directory-wide discovery from the cold
and warm journey remains separate work; this registry does not claim that mode
is bounded by the returned page.

### Producer demand without a ViewerCore

A host retains `SessionCatalogObserver` per configured source and calls `changed()`
after an authorized native push, including when no browser exists. Focused reads
may call `demand()` to queue initial or recovery observation. Both use the existing
bounded refresh pool, coalescing and publication revision checks; neither call
waits for a native history build. `close()` stops and drains work before source
configuration or custody is replaced. The host owns the observer lifetime and
must authorize options before construction.

The existing producer still observes the complete selected machine in the
background. This activation seam does not qualify bounded dirty dependency
recomputation, nor does it initialize unrelated machine models. First useful
content before an initial projection exists remains explicitly updating or
unavailable until that publication completes.
