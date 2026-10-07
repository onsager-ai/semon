
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
