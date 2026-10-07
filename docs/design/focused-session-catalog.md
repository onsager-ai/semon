# Focused session catalog reads

`GET /api/sessions` reads a persisted metadata catalog before constructing any
conversation graph or opening the event cache. This is an additive contract;
`/api/model` remains the compatibility full-model path. Hosts can call
`session_catalog_page(options, machine_key, query)` before constructing a core.
The host selects the authorized tenant, machine configuration and credential
policy. Public source references contain logical roots and relative paths.

## Request and response

Select a stable caller machine key with `machine`. Multiple configured machines
require an explicit key; display hostnames are not authority. `limit` is 1–100
(default 60). Exact `harness` and `repo` filters use dedicated indexes.
`sid` selects one canonical catalog key through the primary key and cannot be
combined with filters or a cursor. An absent selected key returns 404.
`q` returns `unsupported_filter`; this endpoint does not silently search or
return an unfiltered result for a requested text search.

The response includes `api:1`, `machine`, `machine_info`, `capabilities`, the
catalog `generation`, `observed_at`, `items` and `next_cursor`. Machine metadata
and session summaries are explicitly cached. No process, runtime or harness
readiness is invented from persistent metadata. Nullable source timestamps
remain nullable. Capabilities advertise exact filters, direct selected metadata,
ordering and pagination; full-text search, runtime status and global union paging
remain unsupported. The list is ordered by last observation descending and
canonical key ascending. A machine-local catalog includes native sessions and
subagents, without whole-model union copy deduplication.

Each item retains canonical and native identities, parent provenance, stable
summary fields and `source_refs`. Each reference contains logical `root`, relative
`path`, native ID, complete consumed offset and prefix/tail SHA-256 identities.
Absolute source paths are removed from the response. The prefix hash covers all
consumed bytes `[0,offset)`; the tail covers at most the final 4 KiB. Neither
proves bytes after that boundary or substitutes for host archive authorization
and independently verified object generation.

Each source is checked against the current configured root and native input
allowlist. Codex also checks its current manifest authority. Safe descriptor
metadata distinguishes cached, changed/stale, partial/incomplete and unavailable
sources, including refused symlinks or disappearance. The page does not read or
hash log bodies. Matching stat information does not establish fresh process or
native metadata observations. A changed configured root fails the page without
returning summaries from the old configuration.

Cursors bind protocol version, machine, exact filters, catalog generation and
last/key position. Generation and rows are read in one SQLite transaction.
Changing the scope returns 400 `cursor_scope_mismatch`; publishing a different
catalog returns 409 `stale_cursor`. Both explicitly request cursor reset.
Unavailable catalogs return retryable 503; read-only lookup never creates a DB.
A ViewerCore request schedules observation on its existing background worker;
OnRead mode requires explicit observation and reports manual refresh. First
catalog demand can publish even before any full model was served. Background
observation still builds the full compatible model: its global CPU/memory is a
remaining limitation, distinct from the bounded first response.

## Measured workload

`cargo run --release --locked -p semon-sessions --example catalog-workload --
1000 4` seeds four deterministic machine catalogs, closes the writer, then
measures full-model and catalog reads in independent fresh child processes.
Each session contains two synthetic user records. All source timestamps tie;
the first 60 canonical keys on machine 0 stay identical while unrelated history
grows. Core construction is included. No source text, credentials or private
host policy is used. The cache is persistent and warm from the seed process;
these are process-restart reads, not an initial unindexed import.

Measurements on this shared Linux x86-64 container, optimized Rust 1.99 build.
Measured executable SHA-256:
`fd055f5ff0ab9013394a0fd00a49a2adf4a145e8f301f53c72859ae58014bb86`.
This checkpoint predates direct `sid` lookup and its capability advertisement;
those additions do not change the measured page query but change response bytes
by one byte. Re-run the example for the final reviewed revision rather than
assuming these are exact final-bundle measurements:

| Sessions per machine | Total sessions | Model first ms | Catalog first ms | Model bytes | Catalog bytes | Model first RSS KiB | Catalog first RSS KiB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 400 | 90.69 | 5.58 | 361605 | 49254 | 21472 | 6484 |
| 1000 | 4000 | 685.04 | 3.53 | 3610605 | 49254 | 149456 | 6568 |
| 5000 | 20000 | 3843.68 | 4.65 | 18050605 | 49254 | 663320 | 6712 |

The model response is the compatibility endpoint's complete result; the catalog
response is the requested 60 metadata rows. This is an endpoint comparison for
the same first-list intent, not an equivalence claim for transcript or runtime
semantics. The default UI still consumes the full model until the separate
paged-list/selected-conversation consumer lands. No browser latency improvement
is claimed by this server contract alone.

Catalog warm p50/p95 across 20 requests: 4.43/9.82 ms, 3.33/6.17 ms,
3.27/5.32 ms respectively. Full-model warm observations use one request
(20.84,194.16,1033.44 ms), so they are not percentile estimates. Catalog cold
process read counters `(rchar,syscr)` were `(98574,36)`, `(102670,37)`,
`(106766,38)`; model counters were `(1489569,1889)`, `(12739809,18104)`,
`(69250785,91760)`. These are Linux process counters, including SQLite and
configuration reads; they are not physical storage bytes. Shared scheduling,
filesystem cache and allocator state affect timing. Do not calibrate universal
performance budgets from these three samples.

A targeted SQLite-work test also holds the first four selected metadata rows
constant while increasing unrelated rows from 100 to 20000: both execute 52 VM
steps. A cursor at key 19000 in the equal-timestamp group executes 57 steps.
Two direct indexed ranges (equal timestamp/key, then older timestamp) avoid a
mixed-direction predicate scanning earlier keys. Tests also verify generation
changes, unchanged-byte source replacement, deletion, symlink refusal, root
changes, filter/machine binding and initial background publication. They assert
that catalog requests never open the full event cache.

## Remaining work

The default Viewer must adopt this list contract and selected conversation
queries without eagerly loading the global model. Selected native projection,
bounded transcript/source range reads, archive adapters, required relationship
context, text filtering, coherent global union pagination and bounded background
publication are separate pending work. Existing compatibility behavior remains
available. No issue acceptance criterion is closed solely by this endpoint.

### Scoped identity for selected clients

`session_catalog_identity(options, source_key, canonical_catalog_key)` is a typed
primary-key lookup over the same catalog snapshot and source-scope validation as
`/api/sessions?sid=...`. Hosts choose the authorized configuration and stable
source key. It returns `None` only for an absent selected key; invalid arguments,
unavailable observations and changed configured source authority are separate
errors. Error text contains no native paths or provider details.

`CatalogSessionIdentity` carries the source and catalog keys, harness, native
IDs, logical source observations, cached machine label, generation, nullable
observation timestamp and `{state}` freshness. `native_id` is populated only
when all source references identify one distinct native session. A continuation
with multiple native IDs retains them and returns `native_id: null`; clients
must not choose the first ID as a control target. Source disappearance remains
visible. This cached identity supplies neither runtime readiness nor mutation
authority. No global event-cache/model construction or source-body read occurs.

Recorded facts and native source-selection observations remain independent of
source-byte availability. Missing, malformed or unsupported recorded facts make
the composite catalog/identity observation unavailable and its machine label
unknown; retained source bytes can still be inspected. A legacy Codex facts file
without a rollout manifest makes that selected identity incomplete. An explicit
empty manifest selects no current native sources. The compatibility model's
legacy facts fallback is unchanged. Clients must disable native controls when
composite identity observation is incomplete or unavailable.

### Explicit retained history reads

`scope=retained_history` selects read-only retained history intent; the default
is `scope=current`. Cursors bind this intent as well as machine and filters.
`session_catalog_history_identity` resolves the same immutable source references
with typed `read_scope=retained_history`; the existing
`session_catalog_identity` remains the strict current-native identity service.

An omitted Codex native source can remain readable in history scope while
`native_selection.state=retired` explicitly denies any assertion of current
native selection. Unavailable machine observations are separate from known
retained body observations. Current-native identity still refuses retirement,
and source-root substitution still fails both scopes. Hosts must independently
authorize retained history and archive access, and source readers must verify
original immutable generations before supplying bytes. History identity grants
no control, credential or runtime authority.

This scope cannot resurrect deleted projection rows. Durable retention of
catalog, consumed-source provenance and native slot/span recipes across local
source disappearance/eviction is a separate producer requirement. Without those
rows, a selected history request remains not-found or unavailable rather than
silently rebuilding unrelated workspace history or granting native authority.

Current lists use lifecycle-prefixed indexes and omit retained projection rows.
Retained-history lists use the existing order/filter indexes over both lifecycles.
Selected identity is an exact primary-key lookup in either scope: local byte
absence or a retained projection does not itself revoke current native authority.
Current identity still independently requires the recorded native selection;
history read intent never grants native control authority.

A fixed one-item current page with 1 versus 20,001 unrelated retained rows returns
the same item with equal SQLite VM work. The focused catalog tests also verify
that retained-history pagination exposes retained rows and that an exact current
identity remains available independently of projection lifecycle. These SQL
measurements do not establish complete browser journey budgets.
