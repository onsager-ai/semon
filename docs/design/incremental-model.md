# Incremental model reuse

The model keeps its full rebuild as the oracle. Transcript/activity reuse remains in memory. Stable session descriptions now also survive restart in the SQLite event store; they are validated after the source scan before reuse.

Each build scans complete-line ledgers and reruns lineage and cross-session joins. It records link digests in one pass, using source paths, byte offsets and handoff ids. Session descriptions are reused when their member ledgers, source metadata, reported cost snapshots, repository facts and initial names match. Activity, turns and transcript slots are reused when membership, description, direct and root liveness, and the link digest match. Sessions with no timestamp always recompute their clock fallback. Unseen sessions are evicted.

Transcript slots own shared source metadata (`Arc<SlotFile>`), and transcripts are shared `Arc<Transcript>` values. Reordering the file scan cannot change what a cached slot reads. Cached values hold neither source-array positions nor handoff-array positions. Serve-time age and expiry, window trimming, parent selection, transcript marks, model version and analytics are computed again on every build.

Global handoffs, including asks, questions and inferred replies to the reader, remain rebuilt conservatively because their placement participates in subsequent joins. This preserves existing ordering and duplicate rules. Further indexing of those joins remains conditional on measurements; the per-pass timings expose their cost.

The append parser records touched event and signal rows, changed usage ids and each table's append boundary. It relinquishes the event cache's extra ownership before `Arc::make_mut`, so an ordinary append does not clone its index. Readers that still hold an index retain their snapshot through copy on write. SQLite commits these dirty rows with the new ledger under the existing conditional transaction; a moved ledger retries from the committed index, and a replacement clears its previous rows. This changes neither parser output nor the parser cache version.

The normal test job compares restarted, long-lived and fresh builds over 64 deterministic seeds of 20 steps. It includes partial writes, rewritten parents, copy resumes, pid and writer-lock changes, child files before spawn records, peer and parent relays, questions, bridge/session-id joins, Codex spawns and background calls. The comparison covers JSON, transcript indices, facts, analytics, version and text ordering. Separate tests check reuse, eviction and unique-Arc append mutation. A 1,000-seed run remains available as an ignored test.

# Prompt image metadata

Each source file generation owns a bounded image metadata cache. An entry stores only image refs and encoded JSON string ranges, with at most 16 entries and 256 KiB of metadata per file. It stores no prompt text, base64 strings or decoded images.

The first page reads the source prompt; repeated pages read its text and structure while seeking past the image strings. Image requests seek to the selected string range, decode its JSON escapes and base64, and check the requested content version and raster signature. Rewritten files receive a new cache generation. Prompts too large to fit the metadata budget keep the ordinary uncached image path.

### Model transport deltas

The viewer opts in with `/api/model?delta=1&since=VERSION`. A matching current
version still returns 304. A known earlier version returns a JSON envelope
`{delta:1,from,version,set,remove,collections}` when it is smaller than the full
model. `set` replaces changed top-level values and `remove` deletes fields.
Collections patch `sessions` and `tx` by key, and `handoffs` and `turns` by `id`;
each contains `set` and `remove`, plus an explicit `order` when an array's order
changes. Machine membership, busy intervals and other aggregate fields are
replaced when they change. The browser reconstructs a complete model before its
existing update routine preserves scroll, focus, open tools and transcript pages.
A mismatched browser baseline requests a full model.

Each ViewerCore retains at most eight versions and 32 MiB of serialized models,
and admits no individual model over 8 MiB. A reconnect, restart, evicted version,
oversized model or patch bigger than a full response receives the full model.
Requests without `delta=1` keep the previous full-response contract. Account,
navigation and admin fields are never stored in shared history; each patch uses
only the current request's extras. The protocol reduces response bytes; global
join work and constructing the current model still use the existing build path.


## Persisted descriptions (#320, first slice)

SQLite schema 7 adds `session_descriptions`, one versioned row per session key.
The payload is an explicit typed allowlist: source paths and complete-line
ledger identities, names/labels, native history metadata, usage and cost,
repository/branch, invariant timestamps and recorded busy intervals. It never
serializes `Session` as a whole. Message, prompt, answer and tool bodies,
transcript slots, handoff text, process status, waiting state and current
liveness are excluded. `reported_runs` remains authoritative observed data in
its existing table, carried across parser/schema rebuilds; summary copies do
not replace that custody.

The dependency fingerprint is SHA-256 over the existing description inputs:
file membership and verified ledger revisions, native metadata, initial names,
repository facts, shared-usage ownership and observed cost reports. Only the
fingerprint and source references are persisted, not the inputs' debug strings.
A current source scan is mandatory. Missing/inaccessible sources cannot be
restored by a summary. Clock fallback descriptions are never persisted. Live
status, relationship joins, transcript construction and serving ages still run
through their existing paths. The in-memory cache continues to avoid SQLite
lookups on subsequent builds.

Changed descriptions publish as one SQLite transaction after the complete model
build succeeds. An interrupted/failed publication retains the prior generation;
a damaged payload, incompatible summary version or changed dependency misses
the cache and uses the full description builder. The serving snapshot is
unaffected by publication failure. `model::summary::VERSION` must be bumped when
description/identity/pricing derivation changes; the parser version and verified
source ledger also remain part of compatibility.

This is a reusable restart boundary, not completion of #320: event loading,
source discovery/validation, global relationship joins and transcript indices
still scale with complete history. Serving useful cached list content before
those passes, bounded background projection refresh, coherent freshness/progress
and focused query integration remain follow-up work with #319. `/api/model`
shape and current freshness semantics are unchanged.


### Complete metadata catalog for focused reads

A successful build without `scan_window` also publishes `session_catalog`: one
source-backed session row, indexed by last observed source timestamp and session
key, with harness, repository and combined filter indexes. Rows contain stable
identity/native IDs, names, usage/cost, observed timestamps, and the same logged
parent rule as the compatibility model. Every source reference retains its
path, native ID, stat stamp and complete-line prefix/tail hashes. Unknown source
timestamps stay null; they never persist the model's clock fallback. Stubs have
no independent source authority and are excluded from the catalog.

The complete catalog is taken before the serving window trims history. A
scan-window build cannot replace it. Changed rows, removed source-backed
sessions, catalog version and deterministic SHA-256 generation publish together
in one transaction. Unchanged metadata keeps the generation across restart and
live status changes; `catalog_observed_at` records the last successful complete
observation independently. Parser rebuilding clears catalog readiness while
retaining authoritative observed reports. Optional description/catalog writes
use a zero lock wait and cannot add a writer-lock delay to startup or refresh.
An interrupted transaction exposes the earlier complete generation; a later
successful build retries publication.

This makes a bounded read-only SQL page possible before loading the event
indexes or rebuilding the model. #319 owns page filters/cursors and serving-time
source-access/generation checks, freshness and HTTP integration. It must validate
selected sources under the current configured roots: a persisted path or a
catalog generation is never authorization. Background full refresh still runs
the existing global builder; dirty-lineage refresh and bounded transcript/detail
reads remain subsequent work.


Name provenance audit: the description builder uses native metadata only
(`customTitle`/`agentName` log records, agent `.meta.json`
`description`/`agentType`, Codex `agent_nickname`/guardian kind, or a PID record's
explicit `name`), followed by repository/branch/file-slug labels. It never
extracts a name from the first user message, a prompt, an answer or a tool body.
The persisted allowlist is captured at the description boundary, before any
handoff/transcript body reads. Native label metadata is retained as metadata;
the related transcript body is always reread through its source reference.

Catalog publication captures the committed catalog generation before building
and verifies it, source membership, and each committed ledger's complete-prefix
identity inside its transaction. This also rejects competing native-metadata
updates that do not advance a log ledger. If another builder has added,
removed or advanced a source, publication returns conflict and retains the
newer catalog rather than rolling back its generation.


### Restart-style preparation measurement, 2026-10-07

Reproduce with `cargo test -p semon-sessions --locked --lib
persisted_description_restart_measurement -- --ignored --nocapture`. The
workload is 512 Claude sessions, each with one prompt and 128 unique billed
assistant usage records (65,536 usage records). Each iteration constructs a new
`EventCache` and `Texts`; OS caches are warm. This measures application cache
restart, not an OS-cold server/browser journey. The generated workload contains
no live processes, cross-session links, archived providers or expanded tools.

At `79b7529` (schema 6 catalog included), alternating forced-description misses
and compatible committed hits yielded:

| Pair | Miss total ms | Hit total ms | Miss sessions ms | Hit sessions ms |
| --- | ---: | ---: | ---: | ---: |
| 1 | 4265 | 3272 | 2441 | 938 |
| 2 | 2552 | 2454 | 1244 | 460 |
| 3 | 2878 | 1321 | 1449 | 282 |

Every full-model response was 614,977 bytes. The whole workload process peaked
at 77,508 KiB RSS. The complete model still loads all event metadata, so this
slice does not claim a memory or response-size reduction. The original
`83e2325` plus the same measurement-only fixture yielded repeated totals
2158/2793/2669/3308/3153/4103 ms and 74,376 KiB whole-process RSS; the earlier
schema-5 description-only slice (`441b333`) yielded paired misses
1926/1724/1509 ms and hits 885/895/1013 ms (76,224 KiB RSS). These runs shared a
host with parallel builds: the consistent description-phase savings are useful
evidence, while total latency varies too much to calibrate a production budget.

The embedded Viewer bundle remains the base revision's bundle, SHA-256
`f8e53117df754dde006726c1b87f09864480e875fc13c1cab07bf5fbdcbbf059`.
No guest artifact was used. UI connection/session integration, cold focused list
pages under increasing unrelated history and multi-machine/archive journeys
require the consuming query/UI/host workstreams; this evidence does not stand
in for those gates.


### Indexed native identity and relationship lookups

Catalog version 2 / SQLite schema 7 adds a scalar `parent_key` index and a
`session_catalog_sources` mapping from `(harness, native_id)` to physical
source-backed session keys. Several source aliases may resolve to one owner;
several owners remain explicit ambiguity, never a uniqueness overwrite or a
first-match choice. The mapping records source paths and identities only.
Exact session-key reads already use the catalog primary key.

Native ownership and parent/child queries no longer need to scan/parse every
row's JSON arrays. The `(harness, native_id, session_key)` covering index serves
ownership; `(parent_key, last_ms DESC, session_key ASC)` serves child pages.
Source mappings update only for changed rows (or a version migration), and
retired mappings disappear in the same transaction as catalog row retirement.
The catalog derivation version participates in the generation hash, so migration
invalidates earlier cursors even when stable display metadata is unchanged.

A schema-6 catalog remains stored through additive migration. Version-2 readers
must wait for the next successful complete publication before assuming the new
indexes are populated. Source authority and generation CAS remain identical;
these indexes grant no archive access, native operation authority or source
body access. Substring text search is not implemented by this slice and retains
its separate query-contract/measurement workstream.

### Durable source observation work

SQLite schema 8 adds `session_catalog_invalidations`, keyed by physical source
path. Event append/rewrite commits and confirmed source removals record a
store-wide monotonic pending revision, `changed`/`removed` reason, and observation time in
the same transaction as the event ledger. Conflicting writers and interrupted
transactions leave both the event index and pending revision unchanged. No
source body or runtime liveness enters this journal. A covering source-path
index maps work to existing catalog owners without parsing unrelated rows.

A successful complete catalog publication acknowledges pending observations
only after its generation, source membership and consumed-prefix checks pass,
in the same transaction as publishing metadata and retiring absent rows. A
failed publication leaves the old coherent catalog and pending observations,
including source disappearance, intact across restart. Acknowledgement retains
the revision counter so a later observation cannot reuse an older work claim.
Parser-version rebuilding
clears this rebuildable work together with derived views, preserving observed
reports. Additive schema migration preserves the committed catalog.

This is a durable observation foundation, not yet a bounded partial-refresh
scheduler. Stat/source-access checks remain required for reads: unobserved
external changes and native metadata changes cannot be inferred from an empty
journal. Parent/child and native ownership indexes identify immediate context,
but shared Claude usage records, resume bridges, spawn/relay peers and repository
metadata can affect distant sessions. Partial publication must prove the entire
affected dependency closure, including new links and removed sources, before
acknowledging a claimed revision. Until that contract is implemented, coherent
refresh still uses the full rebuild oracle; global discovery, joins and catalog
generation work remain explicit scaling limitations. The journal does not grant
source authority or make cached runtime state current.
