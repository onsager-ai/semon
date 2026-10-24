# Native session storage contract (#478, implementation checkpoints)

Status: partial implementation. #477 is not complete. This revision moves only
change delivery behind `storage::SessionMetadataStore`, with an implemented
SQLite adapter, explicit `StoreConfig`, and `LogicalSourceNamespace`. The
existing parser, identity and semantic projection algorithms are unchanged.
PostgreSQL (#479) and Hub adoption/migration (#313) are not implemented.

MERGE HOLD: do not merge before #380 completes (Source #484 / Hub #318,
Hub #321, Sessions-default #279, Source #483). Their independent browser,
visual, cold-start, performance and security acceptance remains required.

## Persistence inventory and port status

Paths below are relative to `crates/semon-sessions/src`. The CI guard scans
each Rust file for driver references and physical index-path derivation, including
test-only uses. Every remaining direct file must appear in this table. Removing
an entry requires porting its SQL/path ownership; moving files alone is not a port.
The adapter lives in `storage/sqlite.rs` and `storage/sqlite/changes.rs`.
The file inventory below was discovered by CI run 40101653609 at Source
`1d953b2e`; the rejecting guard checks additions and stale entries on every PR.

| Path | Status | Responsibility |
| --- | --- | --- |
| `catalog_changes.rs` | ported | Public change API and data types; no SQL or physical path access. |
| `storage.rs` | direct | Transitional local Options.cache coordinate bridge only; native ports remain open. |
| `catalog.rs` | ported | Semantic snapshot reads; SQL and SQL fixtures in storage/sqlite/catalog*.rs. |
| `catalog_observer.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `catalog_search.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `catalog_union.rs` | ported | Workspace cursor generation uses a backend-neutral semantic snapshot; cross-source PG querying remains future work. |
| `causal_trace.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `codex_lifecycle.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/chunk_proofs/published.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/chunk_proofs.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/launch_calls.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/native_usage/repair.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/native_usage.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/relocation.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/reprice.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/semantic_stage.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/tree_state.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `facts/native_names.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `facts/source_authority.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `history_projection.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `mcp.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `model/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `model.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_evidence/runtime.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_evidence.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_header.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_resolution.rs` | ported | Shared identity/Raw-path derivation consumes bounded semantic navigation observations; SQLite and unchanged fixture oracles live in storage/sqlite/navigation.rs. |
| `native_sidecar.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_timing.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_tree/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `overview.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `overview_repair.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/catalog.rs` | ported | Cohort, search-slot and final-witness reads use the Query metadata contract. |
| `query/tests/recovery_tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/transcript.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query_projection.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query_source.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `received.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `retention.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `selected_read.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `session_bookmark.rs` | ported | Shared request/identity/JSON algorithm uses configured semantic navigation reads; SQLite target lookup lives in storage/sqlite/navigation.rs. |
| `source_change.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `test_hooks.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `transcript_range.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `viewer.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |

Authoritative ownership that must be ported, even where access is indirect:

* `events/store.rs` and children own Event rows and ledgers, source claims,
  invalidations, Raw intents, native metadata/usage, reported observations,
  receiver custody/generation markers, staged work, checkpoints, Catalog/Current,
  retained History, slots/recipes, proof generations and publication changes.
* Catalog, native resolution/bookmarks/read capabilities, selected reads and
  transcript/field readers still own SQLite selection snapshots and recipes.
* Query and Query projections still own selected Event reads and projections.
  The existing single model/parser must remain the only semantic implementation.
* Producer helpers (overview repair, source change/inventory, retention, native
  evidence, refresh and tree staging) still resolve the existing index and its
  checkpoints. Reported Facts/usage and retained original custody are not
  disposable derived caches.
* Byte proofs and their request-owned snapshots remain in
  `events/store/chunk_proofs{,/published}.rs`. Their current lifetime is described
  below; they have not been moved behind the new boundary.
* Runtime pricing uses the same store and must join backend-neutral publication
  ownership rather than independently reopen SQLite.

## Target semantic boundary

Every operation receives an explicit opaque logical source/access namespace.
The authorized host registry binds that namespace to Raw roots and storage
coordinates separately. A physical database location is never a Machine identity,
session key or access grant. Driver connections, SQL, lock codes and arbitrary
SQL callbacks stay inside adapters. The eventual native contract will expose:

| Operation | Atomicity / result |
| --- | --- |
| Admit read snapshot | Pin epoch, Catalog identity/source vectors, recipes and proof references; explicit release on every exit. |
| Commit parsed source batch | Event rows, consumed ledger, byte-proof generation and durable continuation commit together, conditional on the expected source ledger. |
| Claim source work | CAS durable revision and claim generation; stale workers cannot publish, complete or acknowledge replacement work. |
| Publish Catalog | Expected collection generation, full source witnesses, claims, Catalog/recipes, retained custody and change outbox commit together. A failed CAS publishes nothing. |
| Journal Raw mutation | Durable intent before bytes; qualified durable bytes before completion; completion/ACK only after custody and generation fences. |
| Export / checkpoint | Bounded typed pages, immutable snapshot identity, epoch/revision cursor and manifest; no arbitrary SQL or body replay. |

Read snapshots are explicit request-owned objects. Writes are explicit semantic
transactions; compaction in this increment is one such complete operation.
Native write-transaction APIs are still to be implemented, not represented by
empty default methods. Lock order is namespace admission/fence, source claims
in stable logical order, Index ledger/proofs, Catalog/retained references, then
outbox/checkpoint. Never hold a driver write transaction during Raw I/O or
semantic derivation. Adapters may use different lock syntax but must preserve
the same CAS and atomicity outcomes.

Busy is a retryable capacity/lock result; conflict means the expected claim or
witness lost; refusal means authority, version, bound or generation failed.
Errors carry closed diagnostics without paths, credentials or native contents.
The new metadata API distinguishes busy, unavailable, namespace/epoch refusal,
invalid arguments and uncertain commit. Legacy catalog APIs map these into their
existing public error set. No engine error grants permission to change engines.

Native writes must use durable operation identities: persist intent plus
operation ID and result in the same transaction as effects, then reconcile an
uncertain reply by that ID before retry. Raw durability is qualified separately;
database commit does not imply fsync of Raw. Current source journals/claim
recovery remain authoritative until ported. Compaction is already idempotent by
namespace/scope/epoch/acknowledgement: an uncertain commit is reconciled from
the scoped floor, then retried; a duplicate may report zero removed rows.

## Proof generations, leases and recovery

Published recipes must name an immutable proof generation, including namespace,
logical source, parser version, exact consumed ledger and proof root. Index L2
must not overwrite the only obtainable proof for published L1. Current and
retained-history references protect every still-published generation, without a
two-version assumption. Admitted readers pin their selected snapshot/proofs
until their last lease releases, including error/cancellation/unwind paths.

The existing SQLite implementation preserves outgoing exact-ledger proof
namespaces when Current or History references them, within the Index mutation
transaction. Request-owned read transactions retain their WAL snapshots through
lazy proof I/O; last reader drop rolls back/closes. Restart reconstructs durable
publication references; crashed readers have no surviving connection lease.
Path-keyed live proofs alone are not sufficient. This increment preserves this
implementation and does not claim backend-neutral proof leases are implemented.

The target GC API must bound pages, bytes and elapsed work per call, record a
resumable cursor, recheck published and admitted-reader references before delete,
and recover incomplete steps after restart. Existing proof GC is reference-bound
and source-mutation driven, not yet a qualified general bounded GC API. Readers
never manufacture missing proofs. Unchanged consumed prefixes may remain
readable across append; interior edits, truncation, replacement, missing authority
or generation mismatch refuse before publishing partial content. Selected I/O
keeps existing byte/deadline/cancellation and covering-block bounds.

## Snapshot/export and migration requirements (undelivered)

A migration snapshot must include typed Event/ledger pages and all supported
retained and non-rebuildable metadata: Facts/reported usage, custody and generation
markers, Raw intents/completions, claims, staged work/checkpoints, Current/History,
recipes, versioned proofs and references, change epoch/head/scoped floors.
Each page needs namespace, snapshot/version identity, bounded rows/bytes,
continuation and digest. Checkpoint advancement and applied operation IDs commit
together at the destination. Duplicate pages must be safe to replay.

Catch-up follows the captured durable change revision and validates counts,
identities, source witnesses, retained custody and proof reachability against
that same snapshot. Hints alone do not export every authoritative mutation;
a complete mutation journal/export API is still required. Cutover needs an
exclusive durable fencing generation and quiesced old writers. Rollback is
qualified only while reverse catch-up/retained authoritative state is verified.
No real user data migration/deletion, deployment, paid resources or control-plane
replacement is authorized here. The user's supported-current-store migration is
an explicit bounded exception to pre-release historical compatibility policy.

## Current configuration and limits

`StoreConfig` selects the implemented SQLite backend explicitly and separates its
database path from namespace and optional expected stream epoch. Missing or
unavailable configured storage is an error: the adapter never creates/resets it,
uses an in-memory store, changes engines or loads a Hub/PG dependency. The first
authorized discovery may omit the epoch; the host must persist it and pin later
admissions. A changed epoch refuses instead of adopting another store.

SQLite in this increment still uses one source per database. Namespace equality
fences operations within one admitted binding; it is not durable same-database
multi-source isolation. A host must not admit two namespaces to the same database.
The existing local change API remains a transitional Options.cache bridge with
external host authorization. Global native backend selection, durable namespace
admission and same-database isolation remain unimplemented. Other existing native
paths can still reopen SQLite; the old complete EventCache can still fall back
to memory on operational failure. Those are explicit blockers to #478/#477
completion, not properties of the new metadata adapter.

## Verification and remaining work

CI runs the adapter's existing atomic/scoped delivery, compaction, restart and
indexed-work regressions plus a backend-parameterized metadata conformance
fixture. It checks coherent snapshots while publishers/compaction advance,
namespace mismatch, replaced epoch and missing-store refusal without creation.
This metadata harness is reusable by PG but does not qualify native parity.

Still required for #478: complete all native ports and explicit backend lifecycle,
shared-database namespace isolation with identical native IDs/relative paths,
native fixture identities/lineage/counts/turns/recipes/source refs/error parity,
Raw/Index/Catalog crash recovery and stale claims, L1/L2 proof/GC/restart and
altered-source safety, bounded export/checkpoints/GC, no-SQLite instrumentation
outside the adapter, and green exact-revision repository CI with fresh acceptance.
#479 still requires a complete PG adapter with no hidden SQLite and the same
native conformance suite. Hub #313 still requires hosted adoption, restricted-role
RLS, bounded supported-current-store migration/catch-up, fenced cutover, qualified
rollback and all existing browser/performance/security/visual gates.

## Increment 2 checkpoint: native reads and closed writer admission

The Catalog page/selection/source-ledger SQL now lives in
`storage/sqlite/catalog.rs`, behind the semantic `CatalogSnapshot` contract.
Catalog identity, source-root validation, status rendering, ordering and cursor
rules remain in the one shared `catalog.rs` implementation. Existing Catalog
SQL fixture mechanics moved to the adapter tree while retaining their original
Rust test module names and assertions. Query cohort, search-slot and final
publication/witness reads use `storage::query::QueryCatalogStore`; SQL lives in
`storage/sqlite/query.rs`. This removes two direct entries: 47 -> 45. Other
Query/selected-body helpers and SQL called by the adapter remain unported.

`StoreConfig::admit_namespace` explicitly binds an existing supported-current
store to one opaque namespace and a pinned change-stream epoch. It atomically
persists the namespace, admission generation, admitted_at, format version and
an exhaustive bounded table inventory. Missing epoch/binding, changed epoch,
unadmitted reads and mismatched namespaces refuse. Public `open` requires the
pinned epoch and operations require durable admission. Existing local APIs use
a crate-private unbound-local bridge; they cannot access an enrolled store.

Writer qualification is deliberately zero. There are NO namespace columns or
qualified native row writers yet. Enrollment installs durable INSERT/UPDATE/
DELETE refusal triggers for every existing non-internal table (including meta),
so even already-open native writers cannot mutate the admitted source. Native
producer/reader qualification refuses the enrolled store before reset/rebuild.
This preserves existing rows and non-rebuildable facts but pauses native writes;
this API is staging infrastructure, not an operational backend/cutover.

Second-namespace admission always refuses `MultiSourceUnqualified`; changing a
mutable database qualification flag cannot enable it. The adapter-owned JSON
unqualified-path inventory is cross-checked against the exact CI allowlist.
The stored per-table inventory remains version zero. No reader-only multi-source
admission or combined access-set list is enabled over unqualified writers.
Metadata reads of the admitted first source retain snapshots; metadata writes
also refuse until the namespace row/writer upgrade exists.

The supported-current-store row assignment, bounded resumable row migration,
namespace-qualified Event/ledger/Catalog/recipe keys and writers, admitted access
sets and same-store native collision/concurrency tests are STILL UNIMPLEMENTED.
The atomic admission stage is not a substitute for those requirements. Existing
unbound native fixtures are exercised through the ported Catalog/Query read
contracts in CI, along with admission/fencing tests. This does not establish
same-database two-source parity or complete backend-parameterized native parity.
The #380 closure lane is Source #486 / Hub #322; its inherited UI fixes are not
part of this storage work. The existing merge hold and all final gates remain.

## Slice 1a WIP: configured native admission and reversible staging

D1: Catalog page and Query metadata reads now resolve database coordinates from
`StoreConfig` through one adapter-owned reader admission path. Namespace and
pinned epoch are checked in the same read transaction, before Catalog/Query
payload SQL. The registry and epoch checks themselves necessarily read SQL
metadata. Mismatched, unadmitted and stale bindings have closed typed refusals.
Public `StoreConfig::catalog_page` and bounded `query_metadata` exercise this
configured path. Query metadata does not fetch question text or replace the
complete Query tools. Options supplies source/runtime authority only.

The only Options-to-database bridge is explicitly named
`NativeReaderBinding::TransitionalUnboundLocal`, constructed in `storage.rs`.
Its validation refuses every enrolled store inside the read snapshot. Native
Catalog/Query adapter files no longer derive paths from Options.cache, and CI
checks that prohibition. Existing unbound native tests retain their oracles.
This resolves the admission/epoch bypass while the gate remains closed; it does
NOT qualify row-level namespace isolation. Catalog SQL still has no namespace
columns or predicates, and only the one admitted frozen source may be read.

D3: `StoreConfig::unenroll_namespace` reverts only the read-only first-source
preparation stage. It checks the full admission witness, epoch, zero table/writer
qualification and the exact bounded fence inventory under one writer lock before
removing anything. Native tables and rows are untouched. Missing/tampered fences,
new tables or qualified state refuse without partial release. A small durable
sequence record remains so re-enrollment advances the admission generation and
an older witness cannot un-enroll the newer stage. The primitive restores legacy
writability; it is not migration rollback after qualified/new-backend writes.

New shared conformance probes cover configured Catalog/Query refusal before
payload access, and SQLite tests cover the unreachable enrolled Options bypass,
explicit database selection despite a decoy Options path, un-enroll row retention,
warm-writer restoration, re-enrollment generation fencing and tampered-fence
refusal. These checks run in CI only. Exact-final results are left to Gateway.

Still missing from slice 1a: namespace columns/composite keys/keyset indexes and
qualified Catalog/recipe/change writers, namespace SQL predicates, bounded
resumable row assignment, and adapter-level equal-ID/path isolation tests.
The direct-file allowlist remains 45; no file was cosmetically removed to claim
additional backend coverage. The multi-source qualification remains zero and the
second-source gate stays closed. The full #478/#477 criteria and #380 merge hold
remain unchanged; all #479 and Hub #313 implementation is still outstanding.

## Slice 1b WIP: Catalog namespace assignment

`assign_catalog_namespace(max_rows)` implements the second, explicit admission
phase for the 18 names in the adapter's `QUALIFIED_CATALOG_TABLES` constant.
Each call copies at most 64 rows / 8 MiB, with a 2 MiB individual-row admission
limit and a durable primary-key checkpoint. Row inserts and checkpoint updates
commit together. No OFFSET or Raw replay is used. Missing/ambiguous bindings and
changed epochs refuse before checkpoint creation. Oversized/unsupported rows
refuse without reset. The first-source store stays frozen during assignment.

Each destination preserves authored columns/checks and expression/partial
indexes while adding namespace to the primary key and index prefix. At table
completion the source is renamed to a fenced `storage_original_*` backup; no
native table/row is dropped. New canonical tables require the adapter's scoped
`semon_namespace()` connection function on every mutation. Existing unqualified
connections cannot write them. The adapter-private row writer supplies namespace
independently of row images and is used by assignment and private table tests.
Event/ledger/Raw/claim writer families are still unqualified and fenced; the
store-level multi-source qualification remains zero. Native producer publication
integration and its claims/operation identities are not completed by this writer.

Configured read snapshots refuse partial assignment. Once all tables complete,
connection-local views add explicit namespace predicates to native Query/helper
SELECTs; indexed Catalog keyset ranges address main with a namespace predicate
and namespace-leading indexes. Authored source identities and stored metadata
remain unchanged. Helpers that insist on INDEXED BY against a scoped view still
need qualification; their availability is not claimed here. Catalog change
triggers preserve scope semantics with namespace-local revision allocation.
Metadata compaction and full native selected-read/proof integration remain open.

Un-enroll now refuses as soon as Catalog assignment starts. Safe reverse row /
schema assignment is not implemented; the preserved originals and checkpoints
are custody, not permission to select stale state after later writes. This is
not qualified operational migration rollback or cutover.

CI tests add interrupted/restarted assignment with counts/metadata/originals
preserved, absent/ambiguous binding refusal without checkpoints, and table-level
same-ID/same-relative-path get/list/keyset isolation through the guarded adapter.
The second namespace is created only in a privileged private test fixture;
production admission of that namespace still refuses. These are NOT same-DB
native two-source ingest tests. No fixtures, thresholds or visual approvals
changed. The exact-path allowlist remains 45. SourceProofObservation replaces the
complex source-proof tuple at both reviewed lint sites, without a lint allow.
Exact-final CI and fresh independent inspection remain required.

### Slice 1c: production head port and explicit blocked publication unit

The real `SqliteStore::publish_catalog_with_writer` now obtains its base
Catalog generation and records its committed generation/version through the
semantic `CatalogPublicationHead` port. The SQLite implementation borrows the
publisher's existing IMMEDIATE transaction; it neither commits independently nor
copies parsing, identity, projection, CAS or ACK derivation. Current-contract
validation still precedes the port. The explicit `transitional_head` implementation
rejects enrolled stores before reading a publication base or running publication
DML; native `current()` also continues to refuse enrolled stores elsewhere.

This is a production **head** port, not an operational namespace-qualified
publication writer. Inspection of the real publisher shows why Catalog-only
unfencing cannot implement that writer: the same transaction reads source ledger
and invalidation claims, verifies source bytes, preserves history, publishes
search/overview/trace/slot rows, updates global meta generations/completeness,
clears invalidations, acknowledges relocation, and completes semantic stage
receipts. `catalog_native_dependencies`, `session_catalog_invalidations`, the
semantic-stage/frontier/source/input/key tables, relocation ACK dependencies,
ledger and meta are still unqualified/fenced. Their connected extraction remains
required before this transaction may run for an admitted namespace. The public
multi-source gate remains version 0. No additional fences have been lifted.

A real warm-native-publisher regression test first publishes through the normal
unbound production pipeline, then enrolls/assigns that same DB and proves that
publication is explicitly refused with the original rows, generation and change
hints preserved. This is a refusal/coherence test, not successful admitted
publication or two-source native-ingest conformance. The 45-file remaining direct
SQL/path inventory is unchanged: two generation SQL operations moved, but the
native writer file still owns other SQL and is not falsely removed.

Scoped TEMP views now name native columns explicitly, excluding namespace.
Catalog range qualification is carried from the one snapshot validation instead
of rechecking assignment readiness for every keyset range. The indexed
source-proof lookup uses main tables, a namespace-composite join and the scoped
index. Unported search/history qualification/selected trace/transcript helpers
return a typed NamespaceUpgradePending error before legacy indexed SQL; admitted
change compaction remains refused. These helpers still require ports.

The assignment regression now proves the existing epoch row before testing its
meta fence, verifies INSERT/UPDATE/DELETE refusal and all three original-table
fences after renames, checks the supported schema has no child foreign keys, and
checks scoped positional column order. Production schema inspection found no
REFERENCES declarations (the existing FK occurrence is a test-only boundary
fixture); no generic FK compatibility mechanism was introduced.

Remaining full scope is unchanged: operational connected publication and
Event/ledger qualification; same-store native multi-source ingest; claims/Raw
intents and operation reconciliation; immutable proof/reader leases and GC;
export/checkpoint/catch-up/fenced cutover/qualified rollback; EventCache fallback
and no-SQLite instrumentation; #479 PostgreSQL; Hub #313 RLS and migration.
Exact-head CI and independent review remain required. #380 remains a merge hold.

### Read-port increment: bookmark targets and native aliases

Two real consumers now use semantic navigation snapshot operations:
`session_bookmark.rs` and `native_resolution.rs`. Their driver/path entries are
removed from the exact inventory (45 -> 43). Options enters through the named
transitional unbound binding; StoreConfig enters through durable admitted
namespace/epoch validation. Both use one bookmark algorithm and one native
resolution derivation, including the unchanged Raw-home/input-path checks.
Neither consumer receives SQL, driver types, a database filename or an arbitrary
SQL callback. The existing native alias fixture and every oracle moved into the
SQLite adapter, where the fixture calls the same observation reader and shared
derivation as production.

The adapter reads publication version/generation/completeness, at most two owner
keys and at most 1 MiB of selected metadata within the admitted read snapshot.
Qualified reads use main tables, namespace predicates and namespace-composite
native-binding indexes; no INDEXED BY is applied to a TEMP view. An optional
expected generation is compared before owner/metadata SQL. Bookmark target
lookups remain distinct, ordered and bounded to three keys. Missing or
unsupported configured state is an explicit error; the public unbound native
resolver retains its unavailable observation for unavailable local storage.

Configured tests cover namespace isolation against private second-source table
rows, epoch/generation refusal, unadmitted/mismatched namespaces, refusal of the
unbound bridge on enrolled stores, and preservation of originals. They do not
open the public multi-source gate or constitute full native two-source ingest.
Additional tests preserve unbound ambiguity/provisional/current/history/index
oracles, target bounds and generation refusal before owner SQL.

The connected Event/ledger/meta/claims/stage/invalidation/relocation publication
unit, the remaining 43 inventory paths and indirect owners, claims/Raw intents,
operation IDs, immutable proofs/reader leases/GC, export/checkpoint/catch-up/
fenced cutover/qualified rollback, EventCache fallback and no-SQLite
instrumentation remain. #479 PostgreSQL and Hub #313 remain wholly outstanding.
The gate stays version 0; #478/#477 completion and the #380 merge hold are unchanged.

### Read-hardening increment: qualified navigation and workspace cursor generation

Implementation commit: `4a29569b705147612e522246de2ca99822b2a841` on
`task/477-raw-sql-backends-20261024`.

The qualified navigation tests now cover RetainedHistory resolution through
`storage_ns_session_catalog_native`, metadata preference for
`session_history_catalog` with fallback to `session_catalog`, the 1 MiB selected
metadata limit, the two-key native ambiguity limit, the three-key bookmark
limit, and the 4096-byte bookmark target bound. Query-plan assertions verify
the current and retained namespace-composite covering indexes. Foreign namespace
and enrolled unbound reads assert their specific typed refusals. The configured
`StoreConfig::session_bookmark` path is exercised end to end against a real native
Catalog publication.

Workspace cursor generation now goes through
`NativeNavigationStore::catalog_generation`. The Options bridge and explicit
`StoreConfig::catalog_generation` API share this adapter operation. Each lookup
validates namespace admission and stream epoch in the same SQLite read snapshot
as the Catalog generation. `catalog_union.rs` no longer opens SQLite or derives
the physical path. Its existing merge, keyset, filter, authorization and JSON
logic is unchanged; this is not the backend-native PostgreSQL cross-source
query required by #479.

The direct-SQL inventory shrinks from 43 to 42 by removing only
`catalog_union.rs`. Tests cover a matching cursor generation, changed Catalog
generation refusal, missing-store refusal during cursor generation lookup, and
retain the existing source-set-change and union fixture checks. CI remains the
only build/test/format path; this increment has not been locally built or tested.

The connected Event/ledger/meta/claims/stage/invalidation/relocation publication
unit remains the next bounded production boundary. Also outstanding are the
remaining 42 paths and indirect owners, `catalog_observer` invalidation helpers,
retained bookmark helpers, claims/Raw intents, operation IDs, proofs/reader
leases/GC, export/checkpoint/catch-up/fenced cutover/qualified rollback,
no-SQLite instrumentation, all of #479 PostgreSQL and Hub #313 RLS/snapshot/
migration. The multi-source gate remains 0, and the #380 merge hold is unchanged.

## Configured source-observation writer checkpoint (attempt 9)

This is a functional source discovery/invalidation and revision-CAS family,
not the requested Event/ledger commit family or a finished #478 backend. Worker
attempt 9 follows the explicit Manager escalation from cumulative count 8;
max_auto_rework remains 2. The one-dispatch Sol usage-unknown exception applies.
The #380 merge hold and global `WRITER_QUALIFICATION_VERSION = 0` remain.

Inspection corrected the proposed Event/ledger isolation premise.
`commit_initial_files` and `write_one_dirty` call
`write_index_changes_measured`, which also updates relocation, native usage,
headers/sidecars, chunk/launch/parent/tree proofs, native dependencies, archive
metadata, invalidation revisions and mutation retirement in the same native
transaction. Qualifying just `files`, `events`, `signals`, `usage_identity`,
`usage` and `codex_usage` would break this unit. None of these writes is unfenced
here. The prerequisite source-observation family is implemented instead; the
original requested Event/ledger goal remains pending.

`StoreConfig::source_observations()` creates the real `EventCache` with an
explicit configured `IndexStore`. Its public semantic `SourceObservationStore`
operations are discover, claims, pending, backfill_state and invalidate. The
existing Options producer's discovery and invalidation transactions reuse the
same SQLite adapter operations. There is no second parser or derivation, SQL
callback, consumer connection, implicit database creation, repair, memory store
or unbound fallback in the configured route. Existing default Options behavior
continues through its unbound adapter. This is an observation writer API, not a
configured full ingest API.

Bounded assignment now includes `session_catalog_invalidations` and
`catalog_backfill_sources` (20 tables, extending the earlier 18-table checkpoint).
Both have namespace-composite keys, fenced immutable originals and adapter
namespace guards. The pending queue has a covering
`(namespace, revision, source_path)` index. The separate namespace-keyed
`storage_source_journal` retains the revision high watermark and backfill state;
initialization preserves the old counter even when its newest claim was already
acknowledged. Its family marker is `SOURCE_JOURNAL_QUALIFICATION_VERSION = 1`.
The global writer gate remains zero. A family marker is not CI acceptance.

Discovery commits new path membership, invalidations, counter and progress
state atomically. Repeated discovery does not mint a new claim for an existing
path. Invalidate checks every expected claim under the same write transaction
before advancing any revision; a stale claim changes nothing. It acknowledges
no work and publishes no Catalog. Every operation rechecks durable admission,
namespace, expected stream epoch and completed assignment inside its own
transaction. Busy and uncertain commit remain explicit storage errors. Callers
must reconcile durable observations after a lost reply; operation IDs and the
full recovery protocol remain pending.

All other configured IndexStore methods explicitly refuse with
`NamespaceUpgradePending`, including inherited optional methods that otherwise
return empty success, Event commits, staging, Catalog/meta publication,
relocation, repair, retained runs and backfill completion. A configured error
keeps the configured store attached and never switches to the unbound producer.
Catalog generation still reads store-global meta: namespace-qualified publication
and generation must precede any store-level multi-source admission.

New CI-only tests exercise the public configured API, the existing unbound
producer, one-row assignment/reopen, preserved originals, idempotent discovery,
acknowledged-counter preservation, stale-claim atomicity, restart, an injected
mid-batch rollback, a concurrent writer lock, live epoch replacement, typed
admission/namespace/epoch/bound errors, missing-store refusal without creation,
and explicit unported Event/publication/stage refusals. A private foreign-row
fixture proves observation isolation, not same-store multi-source native ingest.
The query-plan assertion uses the production pending SQL constant. Existing
native unbound fixtures and assertions were not changed. Tests and formatting
have not run locally; exact-head CI and independent code review are required.
The direct-SQL inventory remains 42: events/store.rs still owns other SQL, so
there is no allowlist reduction for this partial extraction.

Remaining connected units, in order: Event/ledger and its usage/proof/dependency/
relocation prerequisites; remaining source invalidation/Raw/discovery producers;
semantic stage and the connected Catalog/meta publication unit
(`publish_catalog_with_writer`) with change hints and proof; relocation and ACK;
retained run metadata; all remaining direct-SQL inventory paths and indirect
owners; Raw intents and operation IDs; immutable proofs, reader leases and GC;
export/checkpoint/catch-up, fenced cutover and qualified rollback; no-SQLite
instrumentation; all #479 PostgreSQL operations and shared parity; Hub #313 RLS,
snapshot and migration. The earlier read-test follow-ups remain pending.

Functional code pin for this checkpoint:
`3fd39e0c15ffd0ca90b32a5e00efc1bd5b8d1dd1` (initial WIP
`0dedac22a7a1ba15ac0a3f9fd74f833fb40fefc8`). The final delivery/report commit
may follow with documentation only. No exact-head CI result or independent
acceptance is claimed for either pin.
