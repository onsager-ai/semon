# Native session storage contract (#478, implementation checkpoints)

Status: partial implementation. #477 is not complete. Change delivery,
Catalog/Query metadata, bounded native navigation and source observations now
have semantic storage interfaces, with SQLite adapters, explicit `StoreConfig`
and `LogicalSourceNamespace`. The existing parser, identity and semantic
projection algorithms are unchanged. The full native commit unit remains pending.
PostgreSQL (#479) has private pool, schema and native-row prerequisites. Its
complete session backend and Hub adoption/migration (#313) remain undelivered.

MERGE HOLD: do not merge before #380 completes (Source #484 / Hub #318,
Hub #321, Sessions-default #279, Source #483). Their independent browser,
visual, cold-start, performance and security acceptance remains required.

The user relaxed the progressive cold-start first-page budget on 2026-10-26.
The 0/256/2048-history producer now permits eight seconds for its first 60
selected sessions. Foreground Catalog/Query checks keep their existing two-second
budget; the 60-second background completion boundary, ordering, worker ownership,
provisional authority and archive-eviction checks are unchanged. Older two-second
cold-start results below describe the earlier gate and do not qualify this revision.
The initial five-second revision passed the isolated producer check but measured
5.82 seconds under the full parallel PostgreSQL regression; the eight-second
revision requires a fresh full run.

The streamed Tree witness repair passed scoped independent review at d794ef2a:
43 focused PostgreSQL tests, 1142 full library tests and every Storage contract
group passed, including the actual encoded-byte and cancellation cases. This
qualifies the witness component; full backend and migration acceptance remain open.

## Worker 21 repair: preserve the metadata physical contract

Metadata conformance fixtures now use the same authored singleton metadata DDL
as the factory, including `value TEXT NOT NULL`. The strict physical-shape
guard remains unchanged. Configured Event preparation preserves that two-column
singleton table rather than appending a non-key namespace column, which the
strict guard correctly rejects. Its native metadata writes remain fenced by
the sole enrolled Source, registered writer and per-table ownership checks;
Catalog epochs and custody records remain outside that writer's key set.
Composite metadata Event preparation explicitly refuses with
`NamespaceUpgradePending`; this repair does not qualify public metadata.
The existing stale-interpretation readiness regression now also checks durable
replay after reopen and exact preservation of the factory's metadata DDL.
CI and independent acceptance of this repair are still required.

The prior usage CI failure was in `native_events()` before ingest, at line
1880 of the formatter's actual output. Event preparation appended the malformed
metadata column, then opening the configured IndexStore repeated SourceJournal
preparation and failed its metadata contract check. This was not an observed
post-ingest copied-layout-cache or unknown-key failure.

The earlier cold-page performance failure remains unresolved. Layout probes
are on that producer/read path: `write_file` and `Readers::new` select copied
usage statements, while metadata reads select `metadata_sql`. Copied layout
validation caches only autocommit decisions; initialization qualifies inside
transactions, so a connection without a committed cache can repeat the full
family validation during subsequent transactional writes/reads. This is a
concrete #477 cost to measure, not proof that it caused the recorded 2.37-second
failure. No timing limit was raised. The #380 visual binding gaps are separate;
API/performance failures are not waived as visual overlap.

## Worker 20 increment: metadata admission and ownership

Composite change-stream DDL now initializes only the store-global epoch. The
real first-Source admission transaction validates the bound namespace and
registry, validates both floor values, and fills missing Source floors before
installing durable fences. Repeated admission does not reset progress. Public
second admission is still refused. Private two-Source lifecycle tests use the
same registry DDL and initializer; they do not expose a second public writer.

The configured factory selects the authored store-global epoch UPDATE and
cache-version INSERT. `writer-inventory.json` explicitly lists store keys,
Source keys and Source prefixes. Store.rs observation/recovery/backfill,
generation, slot/search/publication, contract qualification/init and reported
stamp metadata use authored physical variants. Archive GLOB membership selects
core and metadata ownership independently. Unknown composite keys, invalid
Source bindings and malformed metadata namespace primary keys refuse. Source
Journal revision remains separate from the physical store stream incarnation.
These statement ports do not qualify their still-fenced publication consumers.

Published proof preservation now selects Current and history references by
their own physical Catalog families, independently of metadata, and addresses
main tables with explicit namespace joins. Nonempty singleton history in a
composite Current Catalog refuses before payload reads or proof copy/GC,
because its publisher cannot establish a Source owner yet. This is an explicit
remaining boundary, not qualified history publication. The connected proof
fixture checks a same-key malformed foreign owner is never decoded, actual
retained proof copy, unchanged foreign metadata and zero-write history refusal.

Three exact `events::store::reprice::tests::native_usage_metadata_` tests cover
real admission rollback/idempotence/reopen/warm-writer fencing; private Source
floor/key isolation across rollback/restart, equal-path native append/cleanup,
foreign archive poison, corrupt namespace shape and unknown keys; and proof
reference ownership. The assembly fixture also compares actual configured
factory column/index/trigger SQL and checks foreign change rows on insert,
update and delete. All new checks, formatting and performance results await
exact-SHA CI and independent review. No local CPU checks ran.

Metadata/global/usage qualification stays 0. Semantic stage/dependency/mutation
keys and consumers, full publication and historical ownership, immutable proof
generations/reader leases/GC, supported-current migration, PostgreSQL and Hub313
remain required. The #380 merge hold remains unchanged.

## Worker 19 increment: complete authored DDL composition

`authored_storage_ddl(PhysicalLayout)` now selects all 21 authored dependency
chunks for default initialization/rebuild, configured creation and both copied
usage/repricing fixtures. Physical flags select whole families; incompatible
Tree/copied/core/native/queue combinations return `DdlProfileError` before any
execution. The existing public default and configured-core chunk selections are
preserved. Composite profiles remain private fixtures, with qualification 0.

The complete assembly includes real history, overview, search, slots, support
and semantic-stage dependencies. A profile selects current Catalog/source rows
and trace headers only; it does not port the singleton history, trace edges,
support or semantic-stage owners. Authored change-stream variants scope current
Catalog trigger effects to the changed row, keep the epoch in store namespace
`''`. Source floor enrollment was subsequently moved to real admission in
Worker20. Change delivery consumers and the historical publisher remain unqualified.

The copied-usage guard additionally validates its files foreign keys and the
complete non-partial peer index before payload DML. CI now explicitly registers
`native_usage_authored_assembly_preserves_factories_and_complete_dependencies`
and `native_usage_copied_family_rejects_corrupt_constraints_before_payload_dml`
under `events::store::reprice::tests`. They cover unchanged factory DDL bytes,
every authored table/index/trigger, foreign-key consistency, incompatible
profiles, corrupt/missing indexes and foreign parents, and zero-write refusal.
The two existing copied fixtures retain their append, equal-ID collision,
restart, rollback and foreign-source oracles; foreign change rows and the
composite Catalog refusal's zero payload decodes are now checked too.

All results, formatting and performance qualification are pending exact-pin CI
and independent review. No local CPU checks ran because temperature readings
are unavailable. The Worker19 code read found an unscoped current/history reference UNION in
`chunk_proofs/published.rs::preserve`. Worker20 ports its selectors and adds
explicit refusal for unowned singleton history; historical publication remains
unqualified.
Public multi-source/current/publisher gates, configured host repair, immutable
proof/reader lifetime work, supported-current migration, PostgreSQL and Hub
adoption remain pending. The #380 merge hold is unchanged.

## Worker 17 checkpoint: copied usage and pricing owner keys

This is a WIP implementation increment; exact-pin CI and independent key review
are pending. It is not public multi-source activation or completion of #478.
`storage_ddl()` still selects the singleton usage, Tree, metadata and pricing
families. Writer/current/publisher/global and second-source qualification stay 0.
No user store is migrated or reset.

The copied-usage family now has separately authored singleton and composite DDL
for `usage_identity`, `usage` and `codex_usage`, including namespace-leading
keys, peer indexes and foreign keys. Its statement selector inspects that family,
independently of core/native usage or metadata layout. Actual child readers,
message/record and Codex writes/deletes, Tree shared-token reads and remove/clear
loops use the selected physical owner. Composite Tree copy insert/delete triggers
join peers only inside the changed row's namespace. The existing singleton
core-collision fixture now explicitly assembles its copied-usage dependency.

Pricing pending owners have `(namespace,owner_kind,owner_key)` keys. Every runtime
queue read, delete and existence check selects its queue's own layout. Progress
reads select metadata and queue layouts independently. Authored Catalog/core
triggers scope file/session updates and pending owners to `NEW.namespace`, while
a composite metadata join binds the pricing target to the reserved empty store
namespace. Catalog layout and metadata layout are separately selected in the
private complete usage/Tree/repair fixture. Catalog semantic producers and their
physical activation remain unqualified; these trigger definitions do not enable
publication. Native source joins still require the qualified composite core/native
pair; arbitrary mixed native/core activation is not claimed.

Repair preparation receives an explicitly opened Raw carrier and a
`NativeReaderBinding` proof snapshot, rather than opening an unbound database.
The snapshot validates admission, source namespace, current contract and stream
epoch inside its read transaction and transfers its lifetime to the proof reader.
There is no Options or alternate-backend retry in preparation. The existing
Options repair entry refuses enrolled stores before Raw access and retains its
local source lock/allowlist and receipt/CAS checks. A configured host repair
semantic entrypoint, bound writer/claim admission and complete producer guards
are still unexposed and unqualified; the private input is not a public authority
or a claim that configured repair is delivered.

New exact CI test registrations:

- `events::store::reprice::tests::native_usage_copied_layout_is_independent_of_core_and_native_layout`
- `events::store::reprice::tests::native_usage_copied_tree_pricing_collision_restart_and_rollback`
- `events::store::configured::tests::configured_repair_proof_snapshot_uses_admission_and_refuses_unbound_fallback`

The full-family file-backed fixture exercises actual file/child writers, copied
peer insert/delete, token reads, native facts, Tree generations, repair rows,
pricing triggers and source pending selection/completion. It collides IDs and
paths across sources, poisons a source-owned pricing target, changes rows under a
different connection binding, compares the other namespace's full family rows,
rolls back a repricing+clear transaction and reopens before committed work.
Catalog/metadata shapes cover four combinations; a second fixture varies copied
usage independently of composite core/native usage. These are private fixtures,
not admitted public two-source ingestion or full repair recovery qualification.

Coverage correction: `native_usage::namespace_tests` and
`native_usage::file_namespace_tests` are distinct modules. The workflow now
explicitly selects both; the latter owns the existing file-backed append,
replay/restart/conflicting-replay regression. No prior execution is inferred
from the other filter. The original Catalog convergence test retains all 256
iterations and epoch/revision/phase assertions unchanged.

No local cargo/build/test/clippy/fmt, Node/typecheck, browser or corpus analysis
ran: thermal readings are unavailable. CI owns compilation, formatting, storage
and full Rust/Viewer/security/performance gates. Remaining work includes the
configured repair entry, production family assembly/guards, full semantic and
Catalog publication boundary, retained reader lease/GC, supported-current bounded
migration/catch-up/cutover/rollback, PostgreSQL #479 and Hub #313. The #380 merge
hold remains unchanged.

## Worker 16 checkpoint: usage history reconciliation and repair readiness

This is a bounded WIP increment, requiring exact-pin CI and fresh acceptance.
The ordinary merge retains both `fe08864f` and component `43399c01` histories.
The production boundary remains the bound `semon_namespace()` selector API.
Component15's file-backed same-ID/path append, exact replay, reopen and conflicting
replay rollback test is adapted to that boundary. Parent relocation and retained
incarnation coverage is kept. All these tests are selected by the existing
`native_usage_` CI filter; configured regressions retain their existing filter.

A concrete non-convergence mechanism was found in `tree_state::ready`: its old
JSON/version predicate could mark a root complete even though `read_tree_summary`
refused its dimension or interpretation witness. That sent the next consumer
back to the same repair queue. Readiness now uses the consumer's bounded full
validation. A regression changes interpretation while retaining valid JSON and a
complete generation, requires not-ready, rewrites the summary and requires stable
readiness. The original Catalog test still has its 256-iteration limit and the
same epoch, revision and phase assertions. Passing that test is **not yet verified**.

Repair selection, progress CAS, failure/reset/completion and queue writes now use
the repair table's own shape selector. The store clock is read independently
through metadata ownership. Tree generations, headers, models, labels, tools,
usage models, work rows, restoration, witnesses and snapshot header/tool reads
have authored scoped statements. Namespace-local generation IDs use an explicit
allocation inside the writer transaction. Native repricing selects current source
incarnations and reads/replaces fact-derived dimensions within the bound source;
current facts, receipts and byte/row bounds stay intact. Private tests collide
Tree IDs and paths, verify restored dimensions/witnesses and rollback, and reprice
colliding source IDs while checking the other source and original facts.

`native_usage::schema(bool)` centralizes usage and repair DDL selection;
`tree_state::schema(bool)` selects the Tree tables. Candidate PKs, unique keys,
indexes and foreign keys lead with namespace. `storage_ddl()` still selects the
singleton family: activating it before the remaining coupled owners would be
incorrect. Candidate Tree DDL does not yet include composite copied-usage
triggers. No database is reset or converted, and no public qualification changes.

Exact remaining activation work for this family:

- `usage_identity`, `usage`, `codex_usage` schema/DML, the two copied-identity
  triggers, `Snapshot::shared_usage` token read and store remove/clear loops.
- `reprice.rs` pending owner row keys and SELECT/DELETE/EXISTS operations, plus
  Catalog and core pricing trigger queue ownership. Current source/fact/dimension
  operations are ported, but mixed-layout conformance remains pending.
- `native_usage/repair.rs::prepare` currently opens a legacy proof snapshot and
  uses local Options source admission. A configured repair API must bind an
  explicit authorized source and proof snapshot before exposing it.
- Production configured DDL and guards for the whole family, public two-source
  ingestion, retained-reader lifetime/lease/GC, and semantic/dependency/mutation
  owners. Metadata and global writer qualification remain **0**.

CI compilation, formatting, all applicable tests and performance are pending.
No local cargo/formatter/browser work ran because thermal readings are absent.
Full #478/#477, PG #479, Hub #313 and migration/cutover are unfinished; the #380
merge hold remains in force.

## Worker 14 checkpoint: Tree completion witness and metadata ownership

Fresh operation IDs over unchanged Raw still receive their own durable completion.
In the same completion transaction, preserving a current Tree now requires its
file identity, ledger, complete generation, parser version and stored dimension
digest to agree, together with pricing and semantic-recipe interpretation
witnesses and the observed sidecar/header dependencies. An unchanged verified
Tree avoids header, generation and label writes. Missing/stale headers, damaged
labels or a changed pricing interpretation retire the bad header and repair a
new generation. The original state-equality oracle remains; an audit also checks
that no Tree writes occur. These regressions require CI; no local execution is
claimed. The digest scans the same labels/models/open-tools dimensions used by
ordinary restoration, not historical usage identities or closed tool history.

`catalog_semantic_recipe`, Catalog generation, semantic progress/epoch and
pricing work/completion belong to a logical source. A's completed recipe cannot
qualify B's older stage. The configured source revision comes from the existing
namespace-keyed SourceJournal, not a second metadata counter. The existing
`catalog_changes_epoch` remains the physical-store stream incarnation; page
`source_key`, source generation, namespace admission and per-source stream floors
provide the logical-source coordinates. `cache_version`, Catalog/search format
versions, the pricing target and the repair scheduling clock are store-wide.
The empty reserved namespace remains unavailable to logical sources.

Authored metadata statements now cover semantic recipe/header frontier/progress
operations, SourceJournal seeds, reader epoch checks, change-stream floor reads
and updates, pricing work/completion and the repair clock. The configured core
pricing DDL has a separately authored composite-meta alternative whose target
join binds the reserved store scope. A private fixture checks stale recipes,
independent source epochs/revisions, rollback, unique store keys, unknown-key
refusal and a poisoned non-owner pricing target. This does not qualify the
semantic stage's physical tables or the Catalog publisher.

C2 also has scoped operational helpers and private composite DDL for the five
native usage tables (sources/current/facts/identities/dimensions), including
namespace-leading PKs, indexes, unique keys and foreign keys. A private fixture
collides source IDs, paths and observation identities, reconciles exact replay,
rolls back relocation/dimension writes and preserves an earlier incarnation's
facts. Production usage DDL stays singleton until the Tree/repair/pricing and
retained-reader owners are ported. Usage and meta qualification remain **0**.

Meta activation still depends on the Catalog publication/recovery/archive and
qualification queries in `store.rs`, the Catalog pricing triggers and initial
change-stream metadata DDL, plus complete key/owner qualification. Those are
listed in `writer-inventory.json`. This does not block further C2 key ports.
Public second-source admission, global qualification, publication and `current()`
remain fenced. No database was reset or migrated. Full #478/#477, PG #479 and
Hub #313 remain incomplete; the #380 merge hold stays in effect. Exact-pin CI,
formatting, performance and independent acceptance remain pending.

## Worker 13 checkpoint: assembled DDL repair and C1b source-proof subunit

The five semantic indexes on `files`/`events` are now a separate authored DDL
chunk. Cold initialization and rebuilds use their original definitions; fresh
configured creation selects namespace-leading definitions with the same
expressions and partial predicates. No schema guard is relaxed. A regression
inspects every core index through `sqlite_schema`/`pragma_index_xinfo` before
calling the Event factory, also checking the scope of file-pricing triggers.
This covers the complete DDL assembly that the earlier private core fixture
omitted. Existing configured ingestion/replay/Busy/effect/batch tests remain
unchanged and must pass again in exact-pin CI; no local execution is claimed.

C1b is split at source-owned metadata operations versus store-global owners.
The source proof helpers now select authored scoped SQL for native headers,
sidecars, parent markers, summaries/failures, archive markers, chunk pages and
Merkle witnesses, retained proof copy/cleanup/manifests, launch pages, relocation
evidence and the Tree readiness metadata join. The bounded cleanup predicate
and all its bindings are preserved. Metadata shape is checked independently of
`files`: a composite core does not imply composite metadata. There is no SQL
rewriting, fallback retry, alternate engine or new parser.

The candidate physical meta DDL is `(namespace,key,value)` with
`PRIMARY KEY(namespace,key)` and no namespace default. Its two semantic JSON
indexes lead with namespace. A private fixture runs the actual chunk and launch
proof writers/readers with equal paths and keys in two namespaces, two proof
pages and Merkle witnesses, retained-generation copying, source deletion and
rollback. It verifies the other namespace's complete metadata rows are unchanged.
This fixture deliberately does not qualify Catalog membership or a full native
multi-source ingest. The shared Catalog reference used by the retention test
only exercises the metadata copy's namespace predicate.

`SOURCE_METADATA_PREFIXES` explicitly classifies the converted proof key
families. Worker 14 corrected the recipe to source ownership and extended the
explicit store/source key lists as described above.
The reserved store scope is the empty string, which `LogicalSourceNamespace`
rejects; it is bound through the key classifier, never a column default. Scoped
inserts refuse unclassified keys. Tests check unknown-key refusal, same-key
source isolation and a unique store-global pricing target across source bindings.
**This is not a complete inventory of global keys.**

The public fresh store still assembles singleton meta, and preparation retains
its transitional first-source default. Meta qualification remains **0**, as do
global writer qualification, public second-source admission and publication.
Activating composite meta or removing that default before completing the next
list would break unported global operations. Supported-current bounded migration
remains pending; no original, proof, fact or existing database is converted/reset.

Exact remaining statement groups for the next C1b unit (names identify the
existing authored statements rather than claiming a corpus-wide port):

- `store.rs`: Catalog observation/recovery/backfill/generation/slot/search and
  publication metadata; `qualify_contract`, `qualify`, `qualify_producer`,
  `init`, `current`; archive GLOB validation and `current_file_count` membership
  join; `claude_json_stamp`. These include arbitrary-key global readers/writers.
- `semantic_stage.rs`: metadata statements are ported; the stage/dependency
  physical row keys and namespace joins remain unqualified.
- `reprice.rs`: Catalog pricing trigger joins, pending owner keys and native
  source/fact/dimension queries; metadata work/completion is ported. Configured
  core pricing triggers have an authored composite-meta alternative.
- `native_usage/repair.rs`: native repair/Tree table scope remains C2; the clock
  increment and timestamp metadata subquery are ported.
- `changes.rs`: initial epoch/floors DDL and Catalog publication triggers remain;
  read/compaction metadata is scoped. SourceJournal's legacy-only unbound writes
  keep their existing refusal fence; configured writes use the source journal.

Other direct owners remain in the unchanged unqualified path inventory and need
CI inventory/conformance, including selected-reader lifetime qualification.
Do not set meta to 1 until all relevant callers are ported and the public fresh
fixture passes. C2 usage/Tree/dependency/mutation keys, full migration/cutover,
PG #479 and Hub #313 remain pending. PG stays serial until C1b/C2 key-schema
review. Per-statement schema probes need performance CI; no performance pass is
claimed. The two memory-before-persist retry regressions remain required before
exposing those methods, and the #380 merge hold remains.

## Worker 12 checkpoint: composite core Event rows (C1a WIP)

Fresh stores created explicitly with `StoreConfig::create_native_store` have
namespace-leading physical primary, unique and secondary keys on `files`,
`events` and `signals`. Child foreign keys include namespace. Creation requires
a previously absent physical path and a host-chosen stream epoch; any existing
file refuses, without repair, copying or reset. Enrollment and bounded Catalog
assignment remain separate explicit operations. Old singleton Event stores
return `NamespaceUpgradePending` before Event preparation changes native state.
Their facts, originals and retained proofs are preserved; the required bounded
supported-current migration is still pending.

The shared file/Event/Signal writers and row decoders now select authored SQL
for that physical family shape. Configured statements explicitly scope the
namespace, including ledger/read queries, file ID allocation, upserts, tail cuts,
replacement deletes and the Tree writer's file lookup. This keeps one parser,
identity and projection pipeline. The attached file-pricing indexes and triggers
also scope file IDs by namespace. No SQL is rewritten at runtime, no view UPSERT
is used, and a failed statement never selects another schema or engine.

`storage_event_tables.qualification` is per-family: 1 for these three composite
tables and 0 for the 22 remaining singleton families, including `meta`.
Operation and reconciliation tables retain their namespace-leading keys. Global
writer qualification remains zero, public second-source admission remains
closed, and Catalog publication remains fenced. **Full Manager C1 is not yet
finished: namespace-composite `meta` and connected proof/metadata helpers remain
C1b.** This is a primary schema/writer increment, not full #478/#477 acceptance.

CI-only regressions enumerate physical keys/indexes/foreign keys and honest
per-table qualification, exercise private equal-path/equal-ID core writes,
append/upsert, scoped row reads, deletes and rollback, and preserve all existing
configured ingestion/restart/reconciliation/Busy/effect/batch tests. The private
fixture bypasses public admission only for the qualified core; it does not claim
a full two-namespace parser/usage/proof writer. No local tests or formatter ran.

Immediate continuation: port fresh `meta` DDL plus every connected metadata
statement in `store.rs`, `chunk_proofs{,/published}.rs`, `launch_calls.rs`,
`relocation.rs`, `tree_state.rs` and `native_timing.rs` together. Preserve global
Catalog epoch/custody facts and original metadata without in-place row copying.
Then port usage/proof/Tree/relocation/dependency/mutation families (C2) and run the
full public-path two-source fixture before reconsidering PG parallel readiness.
The two memory-before-persist retry regressions remain pending and must precede
exposing those currently refused methods. Ingestion now clears its operation
context after both scan outcomes, before receipt reads or error propagation;
same-handle Busy and success checks assert that cleanup.

## Worker 11 checkpoint: request identity and recoverable errors (partial)

Configured native operation IDs now bind a length-delimited SHA-256 request
digest containing namespace, exact UTF-8 path, harness and `CACHE_VERSION`.
Non-UTF-8 public paths refuse rather than alias through lossy conversion.
The completion lookup precedes Raw I/O. A completed ID with the same request
returns its original consumed boundary and observation revision even after Raw
append, replacement or disappearance. `EventReceipt.replayed` distinguishes that
result from new work; it never acknowledges a new Raw intent. New work requires
a fresh operation ID. A different request returns `Conflict`, including while
an intent is incomplete. Earlier pre-release operation tables without request
binding refuse `NamespaceUpgradePending`; this does not copy or reset rows.

Every `store_failed` caller now propagates a typed error in explicit mode.
`save_runs`, reported-run refresh and retention return results through their
Facts/model callers. Tree reads also propagate configured decode/refusal errors
instead of taking the rebuild path. There is no permanent `configured_failure`
latch, store loss or in-memory-success fallback on these configured failures.
Legacy unbound Options busy/reopen behavior remains transitional. CI covers
same-handle Busy recovery, retention refusal followed by successful reconciliation,
append/replay/restart, fresh-ID advancement, mismatched harness and absent Raw.
Existing effect rollback and batch tests remain enabled.

**At the Worker 11 pin the namespace-composite port was unfinished.** All 25
native tables then had singleton keys, including metadata; see C1a above for
the current three-table increment. No two-source qualification
or connected Catalog publication is claimed, and no public gate is opened.
The inventory retains all 43 unqualified owners and qualification zero.
Reading the shared writer confirms that its transaction reaches relocation,
usage source allocation, all native proof writers, copied-usage Tree triggers,
dependencies, metadata, source invalidation and mutation retirement. The Tree
snapshot opens a configured connection and rechecks its exact ledger and
projection; these queries also need namespace keys. A namespace-column-only
change would leave path/ID joins and copied-usage triggers crossing sources.
Complete those helpers, retained proofs and every configured read/write together
with the fresh composite schema and private two-namespace collision fixtures.
The shared parser/identity algorithm has not changed. PG remains serial pending
this contract; semantic stage/publication and the remaining original scope are
still outstanding. This checkpoint is WIP, not full #478/#477 delivery.

## Persistence inventory and port status

Paths below are relative to `crates/semon-sessions/src`. The CI guard scans
each Rust file for driver references and physical index-path derivation, including
test-only uses. Every remaining direct file must appear in this table. Removing
an entry requires porting its SQL/path ownership; moving files alone is not a port.
The adapters live in `storage/sqlite.rs` and `storage/sqlite/`.
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
| `events/store/configured.rs` | direct | Configured composite files/events/signals transaction; 22 native families remain singleton and global qualification stays zero. |
| `events/store/configured_selected60.rs` | direct | Test-only committed-publication assertions for selected facts, retained-generation witnesses, independent private-witness binding, tamper-before-Raw refusal, negative reference/lease GC and real native multi-window range oracles; writer qualification remains zero. |
| `events/store.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/native_commit.rs` | direct | SQLite physical native commit stages and independent production parser oracle; shared ordering and relationship semantics live in events/native_commit.rs. |
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
| `storage/postgres/native_bundle/tests.rs` | direct | Real-parser SQLite metadata oracle for the connected private partial bundle; no ingest ACK or public qualification. |
| `storage/postgres/tree_writer/tests.rs` | direct | Independent SQLite oracle for private Tree scalar publication; public qualification remains zero. |
| `storage/postgres/native_core/tests.rs` | direct | Real-parser SQLite oracle for the private PG core row family; no ingest ACK or public qualification. |
| `storage/postgres/native_usage/tests.rs` | direct | Shared-parser SQLite oracle for the partial PG stateful usage family; no complete backend qualification. |
| `storage/postgres/native_usage_identity/tests.rs` | direct | Test-only shared-parser SQLite oracle for PG copied-identity effects; no PG backend or no-SQLite qualification. |
| `storage/postgres/reported_facts.rs` | direct | Test-only real reported-source SQLite producer/oracle; the private PG effects use no SQLite connection or path. Public backend qualification remains zero. |
| `storage/postgres/tree_checkpoint/tests.rs` | direct | Test-only real-parser SQLite oracle for the private PG checkpoint family; public backend qualification remains zero. |
| `storage/postgres/tree_writer/tests.rs` | direct | Independent SQLite oracle in private PG Tree tests; no production backend qualification. |
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
`StoreConfig::session_bookmark` path is exercised against a real native Catalog
publication. It still returns the explicit `503 catalog_unavailable` refusal
from an unported composed Catalog helper; it does not fall back to unbound SQL.

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

## Configured source-observation writer checkpoint

This is a functional source discovery/invalidation and revision-CAS family,
not the requested Event/ledger commit family or a finished #478 backend.
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

## Configured singleton Event/ledger WIP

`StoreConfig::native_events()` now explicitly prepares and opens a singleton
native ingest family. `ingest(operation_id, path, harness)` uses the existing
EventCache/parser, ledger identity and projection writer. The caller authorizes
Raw separately and retains its 64-character hex operation ID across retries.
An intent with the observed source revision and expected ledger commits before
Raw is opened. The observation revision is not a worker lease. Source reads,
parsing and physical qualification run outside the DB writer transaction.

One Immediate transaction validates namespace, current contract, epoch, expected
ledger and observed revision; commits all native row/proof/header/tree/launch,
usage, relocation-advance, dependency, invalidation and mutation-retirement
effects; and records the operation completion and target ledger. Retry of a
completed operation reconciles its receipt without Raw I/O or applying effects
again, including after restart. Conflict, busy, scope/refusal and uncertain
commit are typed errors. Configured errors no longer become legacy busy/memory
success. Missing configured stores fail before Raw is opened. Legacy Options
fallback behavior is retained.

This increment deliberately binds native table ownership to the sole admitted
namespace. Metadata retains its authored singleton shape; Event preparation
refuses composite metadata before writing. It does not append a namespace column
or adopt existing rows. Shared-database multi-source writers are still
undelivered. All family mutation guards require the admitted writer connection;
metadata guards also protect Catalog publication and epoch markers. The global
writer qualification remains zero and `current()` still refuses enrolled stores.
Catalog publication/staging, cross-path relocation admission, Raw mutation ACK,
retained-run writers, complete row-key multiplexing and migration remain pending.
Configured ingestion uses conservative native prefix verification instead of the
unported append-trust/relocation optimizations. Original proof retention still
runs in the same native writer transaction; tree snapshots carry explicit
configured admission rather than reopening an unbound reader.

CI-only tests were added for actual configured ingestion, persisted intent and
completion, uncertain-reply reconciliation after restart without Raw, every
native effect rollback, ledger versus observation CAS, busy, namespace/epoch
refusal, old-writer fences and the configured error-mapping regression. No local
checks ran: thermal readings are unavailable. Exact-head CI and independent
acceptance are pending. This WIP does not deliver full #478/#477; PostgreSQL
#479 and Hub #313 remain undelivered. #380 still gates merging.

## Semantic stage/publication S1: unfinished implementation

The authored 21-chunk assembly now has two private physical groups: the complete
semantic stage (12 tables and namespace-leading primary/secondary keys), and
its relocation/dependency/mutation input witnesses. The production default and
configured factory still select their original profiles. The composite stage
requires composite metadata, Catalog and input witnesses; unsupported combinations
are rejected before creating a configured file. Its invalidation/backfill keys
are also composite. Descriptions, history and slot owners are not qualified by
either new flag. Facts118 adds reported facts to this private authored Source
family and its registered OLD/NEW namespace guards; public qualification stays 0.

Facts118 shares the production reported-run `(session_id,start)` merge plan and
allowlisted row validation between SQLite and the private PG family. Missing
reported input retains every earlier run. A save writes its observations and
current file stamp atomically. PG fresh schema 5 adds
`semon_pg_native_reported_runs` and `semon_pg_native_reported_stamp`; Root must
coordinate this fresh contract with the independent Proof117 tables. There is
no schema upgrade chain. Opaque IDs and namespace bytes retain NUL/full UTF-8;
indexed SHA-256 keys always retain a full-equality collision guard. Unsigned
counters remain exact in the shared codec, including `u64::MAX`.

The PG family uses existing schema/STORE/publication admission, then locks the
Source revision before effects. Its durable operation receipt binds the exact
request; retry reuses that identity. Named reads use a SQL keyset and charge row
length before fetching and decoding the body under the existing transaction
cancellation/deadline. All records and stamps remain content-free metadata.
This private family is not a PgIndexStore or a selectable public PG backend.
Complete migration snapshots, catch-up, fenced cutover/rollback, Hub #313 and
fresh independent acceptance remain required. Facts118 CI is recorded separately
from historical evidence; this paragraph is not an acceptance claim.

The input-witness profile is connected to the existing configured parser/ingest
transaction. Native lineage/bridge and Codex identity/parent dependency writes,
mutation retirement and relocation-proof advancement select that family's
authored scoped SQL. The connection validates all three input-table primary
keys, namespace types and lookup indexes before Raw admission. Committed layout
decisions are cached per connection/schema cookie; transient DDL is not cached.
The existing sole-Source guards also enforce OLD/NEW namespace ownership on
these composite rows, without raising their qualification from zero.

CI fixtures exercise real ingest, append, forced rollback after mutation
retirement, reopen/retry/replay, and colliding foreign input witnesses. The
foreign rows are private pre-enrollment fixtures, not two publicly admitted
Sources. Additional fixtures cover corrupt/mixed input layouts, cache rollback,
the complete authored stage key/index set and explicit unported-stage refusal.
No local test, formatter or performance measurement ran; exact-head CI and
independent review remain required.

S1 is NOT complete: ConfiguredIndex begin/batch/record/publish/finish/select
still return NamespaceUpgradePending. The existing stage algorithm and its
remaining dependency/frontier/reset/ACK SQL, whole Catalog publication/history/
slot/search/proof owners and typed Source publication transaction still need
ports. The two planned configured semantic end-to-end/publication tests are
not implemented or claimed. The composite stage's active probe refuses rather
than allowing the old singleton algorithm to read its payload. This is a WIP
prerequisite, not end-to-end Catalog acceptance. Immutable snapshots/leases/GC,
remaining #478 ports, #479 PostgreSQL and Hub #313 remain outstanding; global,
metadata, usage and publisher qualification remain zero and #380 holds merging.

Worker23 added factory validation of all twelve stage tables against the authored
schema (columns, defaults, constraints and indexes), invalidation/backfill and
input-witness column validation, and checks for all three event guards on every
event-owned table. Malformed stage state refuses before journal preparation in
both configured factories. Committed schema-cookie caches retain their rollback
rules. The refusal fixture now represents an absent journal separately from an
empty journal, preserving its table-presence and zero-write oracle.

Worker23 also warms the existing copied-usage layout cache through the actual
producer and selected factories before their first transaction. A factory-path
test covers cache presence and rollback invalidation. The cleanup error test now
uses valid metadata and an SQLite authorizer to inject the same real DELETE
prepare failure for ordinary and measured calls, preserving VM-unknown, no-record
and no-status-read assertions. This does not establish the cause of the parallel
Catalog completion stall: the unchanged 0/256/2048-row and full-suite 2 s/60 s
gates still require CI evidence. No Worker24 was dispatched by this Worker.

These corrections do not implement S1 begin/batch/record/publication/finish/select
or its three planned end-to-end/isolation/stale-publisher tests. S1 remains
unfinished and the configured pipeline still cannot run end to end. No public
qualification was raised. CI-only checks and fresh independent acceptance remain
pending; this increment must not be reported as full S1 or #477 delivery.

Worker24 implements a private indexing sub-unit in `storage/sqlite/semantic.rs`.
The test-only factory requires the real admitted Source, physical epoch, authored
composite stage schema and a versioned recipe/profile. Its typed owner rechecks
admission and recipe inside every operation. ConfiguredIndex begin, bounded
indexing batch and primitive source record now use that capability, the existing
IndexStore types, parser-written ledgers/headers and Source journal claims. Stage
and frontier DML explicitly binds namespace. Record compares the captured stage,
Catalog generation, claim and ledger before writes; claims remain pending until
publication. The existing unbound ownership guard remains; its role-proof body
is also reachable through the validated admitted capability.

This is still partial S1A. The private fixture replaces only empty stage schema;
native data is ingested through the actual configured native writer. The private
profile retains the sole-Source singleton native families; it does not resolve
shared-store multi-Source native ownership. The isolation test uses separate
admitted A/B stores, exercises rollback/reopen, and rejects B on A's store. It
does not prove two admitted Sources sharing one SQL store. Stale record and stale
admission tests are added, but a competing Catalog-generation begin test remains
pending the typed publication port. No test pass is claimed before CI.

Namespace-scoped event paging, native-key/frontier reduction and full reset are
still unported. Batch refuses once primitive indexing finishes. Record refuses
withdrawals and nonempty native-parent/lineage dependency inputs rather than
discarding them. Identity/relations, publish, finish, select and replay remain
unavailable; the public factory still refuses semantic work. No Current, global,
metadata, usage or multi-Source qualification changed. S1A/S1/#477 are not complete.

Worker25 removes the test-only constructor restriction. A crate-private
`SemanticStaging` capability validates the admitted Source, native writer profile
and existing authored composite stage schema before any stage support DDL. It is
absent from Options/config parsing and the public factory still uses no semantic
capability. The earlier primitive-only internal constructor remains limited to
its existing behavior; the explicit reduction capability enables the next phases.
These are internal runtime seams, not public backend qualification or a store
creation/migration path. Existing singleton stage/native data is never converted.

The primitive Codex adapter now has namespace-bound bounded event/key/frontier
pages and reset pages. Reset and reduction validate the admitted owner and epoch/
Catalog generation inside IMMEDIATE transactions. The old parent/root/cost
schedule is extracted into one shared semantic reducer, with separate authored
unbound and namespace-bound physical operations. Primitive Codex identity records
validate complete source witnesses and native keys before inserting endpoints;
the shared reducer then produces roots/cost state and Relations batches. Empty
primitive relation records retain all Source claims. Publication still refuses.

Three additional CI cases cover the runtime capability/public-factory separation,
real ingest through indexing/identity/shared reduction/relations, and reduction/
reset fault rollback plus stale-epoch/generation refusal. The earlier three cases
are unchanged. No local execution or CI pass is claimed by Worker25; Root owns
exact-head CI/formatting and independent review.

The reduction capability is deliberately bounded to primitive Codex components
of at most 64 sources/identities. Existing Current owner closure, cross-harness
native-key collisions, non-Codex relationship events/frontiers, native-parent/
lineage dependencies, withdrawals, transcript/PID/projection and relation rows
have named refusals. Their ports are not waived. Same-DB two-Source success still
needs composite ownership of the Writer's singleton meta/native families
(S1A-2). Competing Catalog-generation stale begin needs the typed publisher
(S1B). Publish/finish/select/replay, atomic ACK/change hints and full S1 acceptance
are still outstanding. All public qualifications remain zero and #380 still
holds #477 merge.

Resume constraints: one writer on the existing task branch; Worker count 25,
without reset. Thermal readings are absent: no local cargo/build/check/test/
clippy/fmt, Node tests/typechecks/formatters, browser/screenshots or image work;
use CI. Check temperatures before/after CPU work and stop at 85 C or higher;
do not start CPU work without current readings. Bound each read/search to one
named file under 10 MB. Preserve unrelated work, disable Git automatic GC and
maintenance, and do not merge, deploy, publish or move live data.

Worker26 adds a candidate private fresh-store native ownership profile. It
selects the existing authored composite core, meta, copied usage, native usage,
Tree, support, stage and pricing queue families together. It never converts a
singleton store or adopts native rows. Explicit crate-private admission captures
one namespace's admission generation and physical epoch. The same capability
supplies the existing configured Writer, Source journal and semantic Stage; the
EventCache parser is unchanged. Public admission and Options remain closed.

Native inventory, intent/completion and all writer effects have namespace guards.
The meta guard allows only native/proof keys, checks both old and new keys on
updates, and preserves physical globals. Private batch runtime witnesses also
include namespace/admission, so a batch from A cannot match B's equal epoch and
Catalog generation. Header frontier reads bind the actual composite meta owner.
Unported Catalog/history/trace/description writers remain durably fenced.

Four CI-only tests now exercise real A/B ingest in one database, primitive
indexing/reduction/record, retained claims, rollback/reopen/completion replay,
cross-namespace batch and stale admission refusal, and closed public factories.
These are candidate assertions, not a CI pass or independent acceptance claim.
The old six semantic cases and all budgets/guards remain. All public writer,
multi-Source, Current, global, metadata and usage qualifications remain zero.

The physical storage_writer_inventory registry remains a store-wide unqualified
schema inventory; source writer ownership is in the composite storage_event_tables
inventory. Unported singleton history/slots/search/trace families stay fenced.
Lazy proof/repair reader qualification for multiple Sources still requires its
own typed reader port. Full-domain reduction, typed publication/ACK/change hints,
finish/select/replay, competing Catalog generation tests, immutable retained
snapshots/leases/GC/Raw custody, all other owner ports, PostgreSQL, supported-store
migration/cutover/rollback and Hub #313 remain required. The cold full-suite gate
and #380 image acceptance remain open; #477 merge stays held.

Worker29 adds a private initial-Current publication candidate, not full S1B
qualification. The admitted composite owner supplies an IMMEDIATE transaction
that checks Source admission/physical inventory, stage epoch/runtime, Catalog
base generation, journal revision, exact claims, staged completion and current
Raw/role witnesses before writes. Current rows and their existing scoped change
triggers, an append-only proof/claim inventory, exact-revision ACKs and the bound
head commit together. Four fault points cover rows, proof, ACK and head. The
component-generation hash recipe is shared with the legacy publisher; its
History/search/slot publication body has NOT been ported or duplicated.

Private finish removes only the matching namespace's working stage after all
publication/ACK flags are complete. Native-key selection and bounded no-relation
endpoint replay now have scoped implementations. Identical publication retry
is read-only, and a new stage over unchanged Raw retains the generation digest.
The unbound transitional head still refuses enrolled stores. Public factories,
configuration and qualification flags remain closed.

This candidate refuses replacement of differing existing Current metadata until
retained History is ported. It also refuses transcripts, PID/relations, lineage,
native-parent and withdrawal payloads; non-native-key selectors and incident
relation replay remain unported. This is therefore NOT the complete existing
publication/recipe algorithm and NOT S1B acceptance. Generic partial/closed/full
Catalog writers, slot/search/history/description/trace owners and the typed
multi-Source proof/repair reader remain fenced. Previously authored private
profiles with unconditional Catalog fences are not upgraded implicitly.

Five new explicit CI cases cover actual two-Source ingest/stage/publication,
competing epochs and stale begin/publish zero-write snapshots, rollback at each
commit step, Raw replay idempotence and preservation of the unbound head refusal.
No local checks ran; CI and independent acceptance are pending. Worker count is
29 without reset. Worker28 cold witness/instrumentation, PG/Cargo, UI and Hub are
untouched. Full-domain S1B, S2-S4, #479 integration, migration/cutover/rollback and
Hub #313 remain required; #380 completion still gates #477 merge.

### Worker32: shared original custody and slot publication, S1B still unfinished

The private admitted publisher now expands a published Current path to every
Current source of that owner within the same namespace, instead of refusing
existing owners. The existing reducer consumes those sources again; unchanged
Raw can reach publication after finish. This does not port Claude lineage,
incident edges or the complete native owner-closure domain.

`storage::catalog::OriginalHistoryStore` carries typed original metadata,
projection headers, bounded recipe-copy/checkpoint operations and completion.
The original custody algorithm (64 records / 1 MiB per step, oversized byte
parts, changed-witness reset and immutable original header) is shared by the
legacy unbound publisher and the admitted namespace adapter. The legacy policy
still selects cross-harness replacements; the private scoped policy also
preserves differing Current metadata before overwriting it. Deferred custody
commits only bounded original-copy progress; Current, generation and claims
remain unchanged until custody finishes. This is original custody, not S2
immutable history for every successive published generation.

`SlotPublicationStore` similarly shares the existing slot serialization,
source-generation/digest recipe and conditional replacement. Its SQLite
adapter owns distinct authored unbound/scoped statements. The admitted path
bounds projection owners, slots and encoded bytes, requires source membership,
and binds projections into its publication proof and changed-generation recipe.
These are private transaction ports; full model/transcript preparation through
the configured backend is still missing. Public factories still expose no
semantic capability and all public qualification flags remain zero.

Two explicit CI cases were added: real A/B ingest followed by A append,
replacement, retained metadata, exact claim ACK, new hints, fault rollback and
stale-publisher whole-DB equality; and complete unbound publication comparing
all durable rows/digests against frozen pre-extraction custody/slot SQL, including
cross-harness replacement and nonempty native recipes. Existing custody paging
and replay tests remain in the CI list. No observed pass is claimed here.

Search publication, complete Current/source/native-dependency effects, composite
History composition, complete selectors/incident replay, nonprimitive identity,
PID/lineage/native-parent/withdrawal and full producer integration remain open.
The full legacy publication algorithm has therefore not yet been extracted.
S1B is unfinished. S2-S4, PG integration without SQLite, migration/cutover/
rollback, Hub #313 and fresh independent acceptance remain required. #380 still
blocks #477 main merge. Thermal readings are absent: no local checks ran.

### Worker34: authored publication families and shared effects (unqualified)

The five Worker32 configured publication failures have a concrete common
physical mismatch: `authored_storage_ddl` selected singleton History and slot
schemas even for the fully composite private semantic profile, while the new
admitted effects explicitly queried `main.*` with `namespace`. Fresh semantic
profiles now select independently authored namespace-keyed History, slot and
search DDL. Other profiles retain their original DDL. Publication checks these
family keys before payload effects; existing private profiles are not upgraded.
Test-only diagnostics print a publication phase and optional SQLite extended
code, never the SQLite message, SQL, paths or content. Production error mapping
is unchanged.

`CurrentPublicationStore` owns authored Current/source/dependency statements;
one `publish_current_sources` loop retains the original retirement, dependency
clearing, PID/parent derivation and source insertion order. The legacy batch
Current statement has moved into the adapter. `SearchPublicationStore` supplies
cohort/member/removal/document/posting/count effects to the existing search
algorithm, preserving its normalization, field limits, tokenization, posting
order and per-cohort counts. `CompositeHistoryStore` supplies physical effects
to the original missing-source recipe composition. Both publishers use these
algorithms. Only test code retains frozen original publication paths for the
independent durable-row/digest comparison. The comparison requires nonempty
Current/source, native-dependency, search and original-recipe families.

Scoped Current, History, slots, search, proof, ACK and change hints stay in the
same fenced IMMEDIATE transaction. The new after-search fault is covered by
whole-database rollback and the existing stale/replacement tests. Additional
private two-Source coverage checks search documents/counts, replay and foreign
namespace equality. CI results remain pending; these fixtures are not the
required full configured model/Query integration test.

S1B remains unfinished: the real model preparation path still needs a coherent
typed runtime/producer seam; record still refuses nonprimitive projection
payloads, and exact staged primitive rows remain required at publication.
Full affected-owner lifecycle retirement, complete selectors/incident replay,
nonprimitive owner closure and the multi-Source proof/repair reader remain.
Public qualification is 0 and the public factory retains `semantic=None`.
Immutable generations/leases/GC/Raw custody, remaining owners, public backend
qualification, full PG parity/no hidden SQLite, migration/cutover/rollback and
Hub #313 remain required. #380 must complete before #477 merges.

### Worker36: real model feed and admitted metadata Query (private WIP)

`model::prepare_semantic_batch` now dispatches the existing index/identity/staged
model producers and returns an opaque `SemanticModelBatch`. EventCache's private
runtime seam records that output before publish. Identity staging accepts real
metadata/transcript preparation; publication staging captures the exact encoded
model rows, slot recipes, repair flags and claims in an epoch-owned witness.
Publish retains the independent identity/source checks and requires exact final
Query/recipe agreement with that witness before any publication effect. Metadata
and transcript recipes use the existing Current/History/slot/search effect ports.
No alternate parser or derivation algorithm is introduced.

Standalone Claude Top inputs without any candidate incident events now share
this path with Codex. The existing relationship reducer candidate predicate
rejects Claude messages/launches needing incident closure. PID, parent, lineage,
relations and withdrawals retain refusals. Replacing a same-key source preserves
original History and retires old source memberships through the shared port;
changing affected owner keys refuses `semantic_affected_owner_retirement_pending`.
Search readiness-marker orchestration, overview/trace effects and complete
selected/proof/repair readers remain pending.

A private Query listing entrypoint accepts the existing QueryCatalogStore port;
filtering/rendering/window and post-read validation use the ordinary Query path.
SQLite uses separately authored namespace selectors for the same cohort algorithm.
The private listing generation is ready only if the component has model Query
rows and recipes and covers every admitted file and Current owner. Global
`catalog_completeness` stays `partial`; general readers cannot consult the
private listing marker as complete-domain readiness. Public factories have semantic=None;
all public qualifications remain 0. This is not a usable backend.

The replacement CI oracle remains unchanged. Static tracing identifies begin's
actual-epoch comparison: after finish, the original handle is quiescent (None),
while a competitor has created the next epoch. A quiescent same-generation handle
may take over that successor only by atomically advancing its epoch. Same-runtime
takeover retains valid reduction/model progress; a different runtime uses the
existing bounded reset. An old epoch or generation refuses before writes.

Two explicit CI tests exercise real sanitized Codex + Claude ingest/model output,
nonempty Query metadata and recipes, namespace-bound Query listings, replay,
Claude replacement History, first model-record fault and every publication fault,
and a full-model stale publisher with whole-database equality/digests. The frozen
legacy oracle and prior replacement/stale/fault/search tests are retained.
No local tests, formatting or CPU-bound checks ran; Root owns exact-head CI and
format adoption. S1C is partial; S2 remains deferred and #380 holds #477 merge.

### Worker38: fenced model preparation and listing custody (unverified WIP)

The observed model publication failure was not an older-schema predicate.
Configured native scanning required an ingestion operation ID even when the
model only needed the committed Index. The swallowed scan error became
`selected source cannot be qualified`. Configured preparation without an ingest
operation now loads the typed, namespace-bound committed Index/ledger/proofs;
it refuses changed physical evidence or incomplete metadata and never reparses
or opens a new ingestion intent. The existing producer verifies captured chunks,
sidecars and native markers, derives the same model, and retains publication's
independent Raw/claim/epoch/generation checks. No PreparedSources are fabricated.

A private ownership-backed reader binding serves only the existing Query
listing algorithm. General Catalog/navigation, selected/proof and search
factories refuse this binding. Open-question text also refuses before an
Options-derived selected reader could run. Public single-Source admission and
all qualification versions remain unchanged at zero.

Current Semon schema/parser versions are required by NativeOwnership, including
the private reader binding. Non-current schema refuses with the typed
NamespaceUpgradePending error; no compatibility ladder or fallback was added.
Frozen original publication parity remains a derivation/effect oracle.

CI registers t1-t5 for fresh same-runtime takeover, different-runtime reset,
active old-epoch zero-write operations, quiescent post-finish takeover and
whole-database rollback. The real Codex/Claude tests now compare production
derivation with the unbound producer and check persisted metadata/recipe/slot
contents plus exact Query facts, including after replacement/History retention.
t6 checks listing-specific readiness and unsupported-reader refusals. An
additional case refuses changed Raw and non-current schema without row writes.
These are test additions, not execution evidence: exact-head CI, formatting and
independent review are still required. Full selected readers, incident/owner
retirement, PID/lineage/relations/withdrawals, S2-S4, PG, Hub313 and #380 remain
unqualified or pending; #477 merge is held on #380 completion.

### Worker40: private Query trace family and runtime fence (unverified WIP)

The private listing cohort requires trace headers, turns and edges as well as
Current/History metadata. Its fresh semantic profile now authors the complete
namespace-keyed trace family, including bookmarks and namespace-leading indexes.
Admission validates exact ordered column/PK shapes, strict rowid policy and
index shapes before any enrollment write. Unsupported physical profiles refuse;
there is no adoption, empty-table shortcut, in-place reset or upgrade chain.

`TracePublicationStore` exposes typed header, turn, edge and bookmark effects.
The existing trace derivation/digest/publication algorithm serves both physical
profiles through the SQLite adapter. The private semantic publisher writes these
effects under its existing Raw/ledger/claim/admission/epoch/generation fence,
before listing readiness. Full trace metadata and its content-generation header
join the durable model witness, publication proof and generation calculation;
repeat detection also checks the persisted trace header. Selected Query recipe
metadata remains unqualified and is not written through an unscoped fallback.

Private runtime observations use `source-runtime-v1:SHA256(JSON(tuple))`, with
the tuple `(semon-source-runtime-v1, namespace, admission, stream_epoch, runtime)`.
The runtime input is the existing captured/rechecked aggregate native runtime
fingerprint. This conservatively invalidates a private Source's summaries on an
aggregate runtime change. Database coordinates and Options-derived SQLite files
are absent. The configured IndexStore supplies a validated typed binding; the
listing reader derives the same binding from its validated ownership capability.
An ordinary reopen retains the witness; a different admission or stream fences
it. The unbound producer retains its existing row-runtime observation contract.

The parity oracle asserts the exact private runtime tuple independently, then
compares every other semantic field and all recipe/trace witness bytes with the
unbound producer. Added tests cover persisted trace facts, equal native IDs and
relative paths across Sources, stable reopen and changed admission/runtime,
nonempty unsupported physical-profile refusal, and trace-effect rollback.
These tests have not run. CI and fresh Manager review are required. All public
qualifications remain 0/None; generic catalog completeness remains partial.
Affected-owner retirement, complete History/slot/search lifetimes, native
PID/lineage/relations/withdrawals/incidents, S2-S4, complete PG selection,
migration/cutover/rollback, Hub #313 and the #380 merge hold remain outstanding.


### Worker43: typed private semantic owners (unverified WIP)

The admitted private stage now records real native PID, native-parent and
lineage observations, typed endpoints and relation rows through `OwnerStore`.
The existing model performs identity and matching; its Relations phase now uses
the existing matching selection rather than final replay. The shared parent /
root / family-cost reducer is unchanged. Relation comparison and physical-sender
turn-witness merging are one shared method used by the ordinary publisher and
configured adapter. Bounded incident pages use namespace-leading from/to indexes
and normalize parent/root observations before the same model replays them.

The existing current stage and support profiles remain distinct from singleton
storage. Stage DDL, columns, primary keys and indexes are validated before
admission; the dependency family additionally requires its exact composite
columns, STRICT/WITHOUT ROWID and namespace-leading lookup index. Unsupported
nonempty profiles refuse without adoption, reset or rewrite.

The durable model witness includes native PID sets, endpoints, parent/lineage
observations, full recorded relations, withdrawals and message selectors, as well
as trace and slot witnesses. It joins the existing proof digest under the same
admission, epoch, runtime, source-revision, ledger, Raw and claim transaction.
Typed withdrawal observations reuse the native CurrentSources authority and Raw
stat checks. Each stage validates their durable authority; final publication
requires an exact re-observation. A fully withdrawn owner becomes retained only
in its own Source, preserving its Catalog metadata, source provenance, slots,
trace and retained search positions. Current search memberships retire through
the existing typed search effects. Other affected-owner moves remain refused.

Four configured CI gates cover real two-Source equal absolute Raw paths/native
IDs, parent/relation/PID collisions, Claude continuations, independently computed
runtime-qualified unbound parity, exact stored relation/dependency rows, stale
publication holders, stage/publication rollback faults, unsupported nonempty
owner profiles, and explicit withdrawal retention. Tests and formatting have not
run; all public qualifications stay 0/None and completeness remains partial.

Remaining domain work includes event incident candidate projection, cross-harness
native keys, remaining frontier/selectors and high-degree matching/cursors,
recovery intents and non-withdrawal ownership retirement. Full retained History
and proof/reader/GC lifecycle remains unqualified. S2 is a separately reserved
increment; S3/S4, injectable PG/full PG, migration/catch-up/cutover/rollback and
Hub #313 remain outstanding. #380 completion still holds #477 main merge.

## S2 private immutable generations and reader custody

The private, owned composite SQLite producer writes immutable selected metadata,
original slot recipes and path/ledger references in the same transaction as its
publication proof, Current/History effects and claim ACK. Generation identity is
SHA-256 over a domain-separated tuple of logical namespace, admission, stream
epoch and the real producer publication proof digest. Existing durable publication
identity reconciles uncertain write ACKs; lease expiry never implies completion.

A host opens one private `ReaderRuntime`, cloning it for concurrent requests.
Explicit runtime takeover advances a durable epoch and fences every old reader.
Writer batch advancement does not change this reader epoch. A reader first names
an exact generation, then acquires a token-bound lease (maximum 60 seconds, 64
leases per namespace). Renewal checks admission, stream and runtime ownership;
release is explicit and fenced. Retrying an uncertain acquire uses the same token
and expected generation, so it cannot accidentally bind to a newer Current.
Expired/fenced lease cleanup is bounded and cannot authorize a remote write.

`ProofReaderLease` exposes the pinned snapshot and private record/recipe-page
reads. The adapter uses existing `Sources`, `Focused`, `Records::finish` and native
record parsing, with database coordinates supplied only by NativeOwnership.
Options and SessionSourceReader retain their existing, independent Raw authority.
Reads check their lease before I/O and freshly before response publication;
providers reauthorize through the existing contract. All page records are buffered
and all touched blocks rechecked before success. The response carries exactly the
pinned proof-generation root. This qualification cohort admits eight recipes per
page, 1 MiB per native record and 2 MiB of returned record JSON; over-budget work
refuses rather than replaying history. Host deadline/cancellation is checked at
each provider-neutral range boundary.

The existing outgoing-proof preservation algorithm now includes the union of
Current, actual retained-source ledger references and live epoch-valid leases.
Publication collects at most 16 unreferenced generation rows; proof mutation
collects at most 16 obsolete manifest generations per pass. GC never deletes Raw.
Reader handles hold no SQL connection between calls, so continued L1 reads after
L2/L3 do not rely solely on a long MVCC transaction. Each individual call still
uses a short coherent metadata/proof snapshot.

This is a private connected implementation increment, not complete #478 or #477.
Tests are authored under the existing configured CI prefix and have not run
locally. Exact-head CI, formatting/clippy, PostgreSQL parity, full-domain selected
presentation and retained lifecycle qualification, hosted custody/RLS, migration,
performance and fresh independent review remain required. Public qualifications
remain zero. #380 completion still precedes any #477 main merge.

### Bounded affected-owner lifecycle (Worker52, unqualified)

The private semantic publisher now uses the shared typed `LifecycleStore`
closure traversal. A Source's recorded Current path owners, parents, trace
incident endpoints, PID peers and lineage peers enroll their complete Raw
membership before the existing native parser/identity/matcher runs. The bound
is 64 owners, 64 sources and 1 MiB of old owner metadata. This is recorded
ownership traversal, not a second relationship reducer.

The typed model captures exact outgoing Catalog observations and the physical
Facts identity plus the existing recorded runtime fingerprint. Record and
publication compare those observations under the Source/epoch/generation and
claim fences. Non-withdrawal retirement requires positive replacement Raw for
every outgoing path; only Current and its source mappings retire. History,
slots, retained recipes, trace headers and retained-history search remain in
custody. The existing S2 publish/collect and proof-ref union are unchanged.
Facts replacement or changed runtime refuses with Conflict, including ACK
replay; hostname alone is not authority.

Path retargeting with a retained verifiable old carrier and explicit collector
selection uses the existing withdrawal and history composition policy. Missing
or tampered old Raw is an explicit reader/producer refusal, never reconstructed
bytes. Local authority without recorded Facts, late-discovered incident owners,
over-budget components and transitions without a complete positive replacement
remain explicit pending/refusal classes. Public qualification remains 0/None,
completeness partial, and selected factories closed. This increment requires
exact-head CI and fresh scoped review; it does not complete D3/D4 or #477.

### Private PG STORE authority qualification (UNIT115, pending CI/review)

The host provisioning LOGIN owns the immutable STORE owner row and resources;
Source LOGINs receive Source-table/sequence access and SELECT only on STORE
metadata. Neither a Source namespace nor a GUC establishes STORE control.
Transactions acquire the schema STORE advisory lock before SourceState, file or
UsageSource locks. Source publication holds a shared advisory lock; host metadata
mutation takes its exclusive form, then the STORE state row FOR UPDATE, and
never requests SourceState. PostgreSQL requires UPDATE privilege for FOR SHARE,
so Source does not take a STORE row lock or receive UPDATE privilege. Independently
authorized host mutations must use this lock protocol; arbitrary host SQL that
ignores it is outside the injected control contract.

Warm admission checks STORE shape, validated checks, keys, defaults, policies,
forced RLS, effective privileges, actual table owners and immutable owner/epoch.
Mutation reauthorizes its host role and uses revision CAS with a durable STORE
receipt. These receipts cannot acknowledge Source ingestion. The distinct-login
CI fixture exercises the normal injected pool and Source observation conformance;
STORE fault hooks cover before state write, after state write and after receipt.
This component is pending exact-head CI and independent review. Writer/backend
qualification remains zero, and complete native bundle, read/repricing integration,
no-SQLite proof and migration/cutover/rollback are still required.

## Worker114 connected private native file increment

The PG partial bundle consumes the same native_commit Input/Effects sequence
as SQLite in one borrowed transaction: exact Core CAS, Usage enrollment and
facts, complete Tree scalar/checkpoint publication, dependencies/invalidation,
shared Codex header/Claude sidecar/parent marker plans, archive metadata
retirement and source-mutation retirement. Current PG physical schema includes
source_mutations and relocation_candidates; pending relocation candidates
refuse before family effects because relocation proof advancement is unported.

This is WIP, not a successful native ingest. The returned PartialFile explicitly
names missing immutable chunk-proof generations/reader lifetimes, launch calls
and EOF markers, relocation qualification, and complete durable ingest receipt/Raw
qualification. It has no conversion to EventReceipt. Public qualification stays
zero; no public backend factory or IndexStore implementation is activated.
Meaningful real-parser, independent SQLite metadata, connected rollback and
stale-CAS tests are registered in the existing PG filter; exact CI is required.


## Worker117: private native file page persistence (WIP)

The native SQLite chunk and launch writers and private PG file bundle consume
`events/native_pages` encoding plans. The current PG layout is version 5, created
fresh only. It stores lossless namespace bytes, content-addressed page blobs,
exact Usage-source/ledger/header-bound native generations, immutable page
references and a transactionally updated path-digest pointer with full-path
collision checks. Old generations are retained; this adapter has no GC.

The private reader checks namespace, exact ledger, generation binding, byte
limits, cancellation/deadline, payload digests and chunk Merkle witnesses before
returning a hash. Launch reconstruction shares the production SQLite validator,
including EOF and its complete-index fingerprint. SQL snapshots grant no Raw
access. A native generation is not a Catalog publication generation.

This increment is unqualified WIP. Catalog Current/history references, actual
reader/pool lease ownership, GC ownership and selected publication integration
remain unported. No complete native ingest receipt, public PG backend, full
no-SQLite path, migration, cutover or rollback is claimed. History-noise resource
measurements and complete page/lease adversarial qualification remain required.
Exact CI and independent review determine acceptance; public qualification is 0.

## Current schema contract (2026-10-26 amendment)

Semon supports one current schema contract per engine. The #477 transfer,
catch-up, cutover and rollback paths exchange that current contract only.
Old Semon schema readers, writers, importers, upgrade chains, dual writes and
compatibility-only tests are outside scope and must not be retained to support
older installations. Unsupported old state is refused explicitly without
resetting, deleting or silently replacing retained data. Raw and non-rebuildable
current facts/history retain their existing preservation requirements.
This supersedes any earlier proposal to import older singleton/composite
profiles. Current producer/consumer correctness and integrity checks still apply.

## Private published Tree scalar reader (scalar119)

The private PG Tree checkpoint restores the actual version-3 `TreeSummaryRecord`
through the same strict current-format codec as SQLite `read_tree_summary`.
Both witnesses are mandatory. PG qualifies the exact namespace, full file ledger,
UsageSource receipt/parser/checkpoint, complete nonpending generation and header,
then checks persisted dimensions and current store pricing, Source recipe and File
pricing fingerprint before decoding the admitted scalar. No missing scalar can
be returned as a default-valued complete Current summary.

Deferred tool/usage calls retain the captured PG checkpoint and host cancellation.
Each call uses the current selected request scope and a fresh SQL snapshot to
reauthorize the scalar digest, dimensions and interpretation before payload.
A changed Current generation refuses explicitly; this component does not promise
retained L1 reader leases. Scalar and variable metadata are length checked and
charged before retrieval/decoding; dimension queries retain keyset LIMIT 1 reads.

`Snapshot::restore(pool, namespace, path, ledger, host_cancellation)` keeps its
existing private signature and now requires real `tree_writer::publish` state.
The future native117 caller must supply the committed full ledger and request
cancellation only after Usage/Core/Tree publication completes. No NativeBundle
caller hook exists in this base, and none is claimed here. Checkpoint fixtures
now publish through the production PG Tree writer instead of importing a header
without a scalar. Public backend qualification remains zero, with complete
IndexStore/reader/proof lifetimes, recovery, migration and Hub #313 still pending.
The #380 predecessor completion requirement before MAIN remains unchanged.
