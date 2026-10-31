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

## Turn IDs

Turn IDs used by transcript anchors and trace routes are opaque model IDs, not
public session keys. A turn that starts at a handoff keeps that handoff ID. A
handoff-free turn uses the nonempty native UUID (`Event.u`) on its first grouped
event when present. That ID is a domain-separated stable hash of the structured
tuple `("native-start", canonical session key, UUID)`, so opaque values and
delimiters cannot alias. The current hash is the existing 64-bit FNV-1a stable
ID encoding (`t` followed by 16 lowercase hexadecimal digits); as with any
fixed-width hash, collisions remain possible. If the first grouped event has no
nonempty UUID, the existing `<session key>:<native file ID>:<byte offset>` ID
remains. The turn's `file` and `offset` fields continue to identify its native
start location.

Distinct UUID-bearing handoff-free turn starts within one canonical session
need distinct native UUIDs for distinct IDs. Repeated start UUIDs are not merged or
disambiguated and can violate turn or trace uniqueness. UUID-less turns can
still collide when physical files share a native file ID and starting offset.
Paths are not identity inputs. These repeated-UUID and colliding UUID-less
cases have no qualified unique-ID contract. UUID-bearing handoff-free IDs
change under this contract update; old Semon versions' IDs are not retained as
a compatibility format.

## Grammar

The disjoint `/session/` prefix is available in the standalone tiny_http shell and
Hub's authenticated fallback router:

* Transcript: `/session/<source_key>/<catalog_key>`.
* Transcript anchor: `/session/<source_key>/<catalog_key>#<encoded turn>`.
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
`all_machines=1`, `after`, `generation`, `field_chunk`, `part`.
The filter never resolves or authorizes the selected session. `repo` and
`no_repo` conflict. Ordinals are nonnegative safe integers; continuation after
zero requires a generation. Field and part reads require after and generation,
and conflict with each other. The existing parts are script/output/attachment.
All deeper selected-read generation, range and custody validation still applies.
Unknown keys, duplicate keys, invalid encodings and identity parameters
`machine`, `source`, `sid`, `catalog_key` are malformed on canonical addresses.
Browsing context remains usable on reload and history navigation. Standalone HTTP consumes a successfully authenticated `t` bootstrap credential
before route parsing, and the frontend removes it before parsing or history
normalization. Duplicate or invalid URL credentials fail authentication, including
with a cookie. Existing Host, cookie, one-time control nonce and write protections
remain in force. Public route parsers and Hub never accept `t` as a route control.
API requests still explicitly carry the resolved source and
Catalog key under their existing selected-read contracts.

## Resolution

Rust `session_route` and UI `lib/sessionRoute` implement the same grammar and
metadata resolver. Both consume the vectors in
`tests/fixtures/session-route-vectors.json`; their tests run in the ordinary Rust
and UI PR suites. A resolver receives only already-authorized, unrevoked metadata.
It cannot grant a read, make native IDs authoritative or silently choose another
source. Outcomes are valid, malformed (400), not_found (404), unauthorized under
the host's existing denial policy, and ambiguous (404; duplicate candidates for
one source and key).
An unavailable publication remains unavailable; it is not evidence of absence
or uniqueness. For syntactically valid direct routes, the reader shell
remains available while a source observation is pending; selected APIs own its
retry state and content waits for exact resolution. Hub validates workspace and
source authority before returning the shell. Selected reads retain their
generation and custody fences.

The viewer requires JavaScript. The server-rendered `native=1` reader, the
`compat=1` switch, the `/timeline` alias, the `/m/<id>` client alias, the legacy
`/s/<harness>/<key>` and `/trace/<harness>/<key>/<turn>` bookmarks and the
`#turn=<id>` fragment spelling are retired without redirects; they receive the
ordinary not-found or malformed-address response. Old bookmarks do not resolve.
