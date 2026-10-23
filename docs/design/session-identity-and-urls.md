# Session identity and URLs

Public identity is `(scope, source_key, catalog_key)`. Scope is one standalone
installation or the authenticated Hub workspace. The source key is the existing
opaque machine key, and the Catalog key is the exact source-local key. Hostnames,
machine labels, harness names and native IDs are provenance, never authority.
SQLite files, partition placement and cache layout are not public namespaces.

`(workspace A, laptop, same)` and `(workspace A, desktop, same)` are different
sessions. Copying or synchronizing a recording to desktop creates another public
identity even if its harness and native UUID are unchanged. No URL lookup merges
or deduplicates those recordings. A Claude and a Codex recording may share a native
UUID; their Catalog keys distinguish source-local recordings. Workspace B cannot
use workspace A's source keys to read its recordings. An installation or workspace
move needs an explicit future migration policy; labels do not provide continuity.

## Grammar

The disjoint `/session/` prefix is available in the standalone tiny_http shell and
Hub's authenticated fallback router:

* Transcript: `/session/<source_key>/<catalog_key>`.
* Transcript anchor: `/session/<source_key>/<catalog_key>#<encoded turn>`.
  The historical `#turn=<encoded turn>` spelling remains readable.
* Trace: `/session/<source_key>/<catalog_key>/trace/<turn>`.
  The turn is mandatory. There is no turnless trace destination.

Split raw paths on literal `/`, then percent-decode each segment exactly once
as UTF-8. A decoded slash belongs to the opaque identifier and is never split
again or used as a filesystem path. For example, `source&key` and `session/key`
format as `/session/source%26key/session%2Fkey`; `%252F` means the literal key
`%2F`, not `/`. Formatters escape every UTF-8 byte except RFC 3986 unreserved
characters, using uppercase hexadecimal. Literal plus is not form-encoded space
in a path. Noncanonical valid encodings are accepted without changing identity;
new links always use canonical escaping. Trailing slashes and extra segments
are malformed. Empty, `.` and `..`, controls, invalid UTF-8 and malformed percent
escapes are rejected. Existing selected-reader keys have a 4096-byte bound.

Canonical queries contain only nonidentity controls, at most once each:
`harness` (the existing browsing filter), `repo`, `no_repo=1`, `model`, `q`,
`all_machines=1`, `native=1`, `after`, `generation`, `field_chunk`, `part`.
The filter never resolves or authorizes the selected session. `repo` and
`no_repo` conflict. Ordinals are nonnegative safe integers; continuation after
zero requires a generation. Field and part reads require after and generation,
and conflict with each other. The existing parts are script/output/attachment.
All deeper selected-read generation, range and custody validation still applies.
Unknown keys, duplicate keys, invalid encodings and identity parameters
`machine`, `source`, `sid`, `catalog_key` are malformed on canonical addresses.
Browsing context and native paging/expansion links remain usable on reload and
history navigation. API requests still explicitly carry the resolved source and
Catalog key under their existing selected-read contracts.

## Resolution and compatibility

Rust `session_route` and UI `lib/sessionRoute` implement the same grammar and
metadata resolver. Both consume the vectors in
`tests/fixtures/session-route-vectors.json`; their tests run in the ordinary Rust
and UI PR suites. A resolver receives only already-authorized, unrevoked metadata.
It cannot grant a read, make native IDs authoritative or silently choose another
source. Outcomes are valid, malformed (400), not_found (404), unauthorized under
the host's existing denial policy, and legacy-only ambiguous or mismatch.
An unavailable publication remains unavailable; it is not evidence of absence
or uniqueness. Selected reads retain their generation and custody fences.

Legacy `/s/<native|codex|claude>/<key>` and
`/trace/<native|codex|claude>/<key>/<turn>` are compatibility inputs only.
An optional single `machine` binds the exact source. Without it, lookup examines
only authorized sources. Zero exact matches is not_found; multiple matches show
canonical source choices without content or automatic selection. One match with
`native` or its actual harness gives HTTP 308 (JS normalization uses replaceState).
A different harness is a mismatch with no content or redirect. Duplicate machine
parameters are malformed. Exact Catalog keys are required; labels and native-ID
aliases cannot redirect to a different session. Allowed controls survive the
redirect. Trace turns stay in the required trace segment, fragments survive HTTP
redirect inheritance, and old session `?turn=` targets normalize to `#<turn>`.

Retire legacy input only after all first-party producers use this grammar, the
cross-host direct/history/native matrix passes, and an explicit product decision
sets a compatibility window and communicates bookmark replacement. No retirement,
data rewrite, database merge or deletion is authorized by this change.
