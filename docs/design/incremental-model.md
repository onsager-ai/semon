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

SQLite schema 6 adds `session_descriptions`, one versioned row per session key.
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
