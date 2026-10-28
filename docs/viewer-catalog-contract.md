Catalog-backed `ViewerCore` requests wait for the first persisted Catalog page
after validating their arguments and source authority. One absolute 20-second
deadline covers the whole request, including multi-source location. If
publication has not arrived, the request returns an
explicit retryable `503 catalog_unavailable`. Waiting uses the existing producer
and publication condition variable; it does not acquire the producer's source
lock. A producer may remain blocked by a native writer after the request ends.
Native HTML page reads never wait for publication; they retain their existing
unavailable/retry response, including when a Work fence is held.

This admission applies to `/api/sessions`, `/api/session-locate`,
`/api/session-transcript`, `/api/session-entry`, `/api/session-identity`,
`/api/session-anchor`, `/api/session-annotations`, `/api/session-trace`,
`/api/session-home`, `/api/session-observe` and `/api/session-resolve`.
`/api/viewer/home` and `/api/viewer/analytics` retain explicit per-source partial
or unavailable observations. Source inventory, capabilities, chrome and tree
metadata do not require a Catalog publication. Nonblocking library read
primitives remain usable independently of a live viewer producer.

First publication is an admission signal, not a promise that every selected
session or optional projection exists. Existing partial-data, generation,
retained-history and authorization checks still determine each reply.

Codex native observations may include `turn_lifecycle` with the exactly owned
thread/turn, last durable phase, source record offset and consumed-prefix
generation. The Event Index supplies this fact through the existing Catalog
Query publication and Home summary. Writer `pid`/`alive`, remote freshness and
the last turn outcome are independent. Missing terminal evidence never implies
successful completion. See [the supported carriers and limitations](design/codex-turn-lifecycle.md).

Recorded spawn/relationship status also admits `idle` and `off`: a child whose
writer remains held after a terminal can be idle, and missing lifecycle is an
unknown observation (`off`). A parent's selected range retains these child
observations without rejecting its own transcript. Arbitrary states, foreign
edges and source/generation mismatches remain invalid. Optional unsupported
turn-lifecycle metadata degrades only that observation to unknown; other
independently validated selected details remain readable.

Archive source discovery is an optional, separate read contract. A host advertises
`source_candidates: true` only when it implements both
`GET /api/session-source-candidates?machine=&limit=&cursor=` (maximum 60 hints) and
`GET /api/session-source-candidate?machine=&candidate=&generation=`. A hint carries
an opaque candidate key, logical root/path, nullable native name hint, verified
SHA256 generation and nullable archive verification timestamp. It is not a
canonical session row, native session identity or control grant; archive time is
not native activity time.

Selection replies echo source, candidate and generation and report updating,
ready or unavailable, with retryability and an optional reason. Only ready may
carry a canonical catalog key. The Viewer polls serially with bounded requests
and backoff, then verifies that key through the typed retained-history metadata
endpoint before opening the parsed session. Ambiguous sources remain unavailable
with an explanation. Source changes, selection changes and disposal invalidate
late deliveries. No complete-model restore or control authority is inferred.

Partial discovery reports collection `completeness: {state: 'partial'}` and
`freshness: 'updating'`. Source metadata remains readable while the native owner
is provisional. A typed identity may declare `owner_qualification: 'provisional'`
but must carry `native_id: null`; provenance and declared native IDs remain
available for explanation. Both the native observer and control presentation
reject provisional and historical authority. Independent read-only runtime phase
observation remains available.

Metadata search requires explicit `metadata_search: true` and the advertised `q`
filter. Literal user input is URL-encoded once and normalized by the server. The
result documents Unicode lowercase substring matching over names, keys,
repositories, branches, models and harnesses, with at most 512 candidates and
2 MiB verification per request. Budget-limited results can be empty with a
continuation cursor. The Viewer presents that partial scan separately from an
incomplete index, retains the query across source switches, and never falls back
to full transcript or cross-source search.
