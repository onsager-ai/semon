# Current native session storage contract (#477 / #478 / #479)

Status: implementation in progress, not full #477 acceptance. SQLite is the
explicit public backend. The private PostgreSQL adapter is being qualified
through the ordinary native ingest, Catalog, navigation, selected reads and
restart flow. Its existence and component results do not enable a public PG
factory or establish complete backend parity. Hub #313 and the full current-format
migration/cutover/rollback qualification remain required.

#380 closed on 2026-10-29 with final Source `f1a6c2ab` and Hub `ade4a935`;
its unmeasured performance follow-up is #510. This integration includes later
Source main through `893dd56f`. #477 still requires independent acceptance.
This work grants no MAIN, deployment, publication or real-data migration authority.

The WIP PG captured-body port is `StoreConfig::call_captured_query_at`.
It names one acknowledged immutable publication and requires the request's
`CatalogReaderFactory`; generation addresses never grant Raw access. The port
also requires the external Machine key and the request's `SelectedReadControl`.
It enters that control before metadata/binding SQL and retains it through the
namespace/canonical-key factory, leased proof rendering and final check. Machine
labels use the external key independently of namespace storage authorization.
The port
uses ordinary Query envelopes and shared body/field rendering with leased Native
proofs. It accepts a complete (`all=true`) capture and refuses recent-window
Options. Live Query continues to reject expired runtime context, including PID
changes; it does not automatically select this capture path. This port's exact
CI qualification remains pending.

`StoreConfig::captured_source_proof_at` prepares the exact source proof for a
selected captured publication before entering the body-reader lease. It checks
the configured source binding and does not grant Raw access. Hosted consumers
must retain their physical/generation, cancellation and final authority checks;
the public interface alone is not retained-prefix acceptance.

PG workspace listing now has a WIP native global SQL selection path for explicit
authorized namespaces sharing one pool. Namespace/epoch/admission/head and
revocation checks include sources without matching rows. Keyset ordering is
last descending, key ascending, external Machine key ascending. Machine keys
remain independent of the authorized storage namespaces. Bounded metadata search
uses the existing Unicode lowercase field rules. Workspace RLS additions apply
only to SELECT on the required Catalog/authority metadata tables; ordinary
single-Source write policies and Native proof/Raw boundaries remain unchanged.
Only selected rows enter shared per-Source presentation. SQLite retains its
formal union path; mixed or different-pool PG workspaces refuse explicitly.
The existing source and metadata limits remain, and exact CI qualification of
the new SQL, isolation and presentation path is pending.

The PG native physical contract is now v11. It includes nullable `bytea`
`query_recipe` cells on current Catalog rows, retained History headers and
immutable selected proof generations for typed Catalog transfer. These cells
are inactive paired storage: production publication and read paths remain
inline and unchanged. The v11 marker and native shape hash refuse v10 stores;
there is no migration or dual-read path. Shared Catalog version 5 and PG
publication envelope v1 remain unchanged, with no format activation in this
prerequisite.

## One current SQLite contract

All current tables use the namespace-leading physical contract. Namespace-local
`CHECK(singleton=1)` control rows remain legitimate; they are not the removed
singleton schema. Unknown, old or corrupt stores refuse without conversion,
reset, deletion, lazy reopen or fallback to memory/another backend.

`StoreConfig` separates the physical path, logical source namespace and expected
stream epoch. A path is neither an identity nor an access grant. Admission
validates the actual current physical contract and owner. NativeOwnership and
Semantic NativeReaderBinding carry the admitted namespace/epoch through private
multi-source publication and reads. The restricted public admission gate remains
subject to its actual qualification; private multi-source tests do not remove it.
SQLite public admission validates existing current custody. It cannot adopt an
unbound store or install the retired enrollment runtime. SQLite unenrollment
refuses with `NamespaceUpgradePending` until current-format revocation is
qualified; it never drops custody tables to restore unbound native writers.

Catalog publication requires the current owner and expected generation. There is
no unbound writer/history/search mode, transaction snapshot fallback or old Catalog
assignment/import runtime. The obsolete assignment API has been removed. Fixtures
must obtain real current admission and use namespace-qualified current DML.
The SourceJournal has one mandatory-namespace path for claims, discovery,
recovery intents, receipts, cursor advancement and exact operation replay.

Ordinary source-write and enqueue entry points now open the admitted backend and
its SourceJournal directly. Empty Current publication initializes both search
coverage scopes through the owned publisher. Retained proof references require
the namespace-leading Catalog and History families before payload reads or GC.
The old open/qualification/initialization runtime has been removed. Fixtures
use current admission and the owned reader/writer context; persisted parser
oracles restore the selected index from the formal backend. Unavailable,
damaged and unsupported stores are refused without replacement or memory
fallback. Broad fixture qualification remains subject to exact-revision CI.

Ordinary host scans reconcile a ledger CAS conflict within the existing three
attempt limit, loading the winner's committed rows before parsing again. Each
attempt has an immutable operation identity; explicit ingest retains its caller's
exact replay identity. A rejected append continuation requires full prefix
verification before the existing ledger can be used again.

Current SQLite remains a supported alternative backend. External Claude/Codex Raw
formats and their existing event parsers remain supported independently of the
removed database schema/runtime compatibility.

## Transactions, recovery and resource bounds

Every operation receives an explicit opaque logical namespace. The host registry
binds it to authorized Raw roots and storage coordinates. Connections, SQL,
lock codes and SQL callbacks remain adapter details. Schema/role admission and
controlled DDL invalidation must preserve the current physical contract; business
operations retain namespace, epoch, generation/CAS and resource checks.

| Operation | Required outcome |
| --- | --- |
| Admit read snapshot | Pin namespace, epoch, Catalog/source vectors, recipes and proofs; release leases on every exit. |
| Commit parsed source batch | Event families, consumed ledger, proof generation and continuation commit atomically against the expected ledger. |
| Claim source work | CAS the durable revision and claim generation; stale workers cannot publish, complete or ACK replacement work. |
| Publish Catalog | Source witnesses, claims, Catalog/recipes, retained custody and outbox commit together; failed CAS publishes nothing. |
| Journal Raw mutation | Persist intent before bytes, qualify durable bytes before completion, and retain exact OpID reconciliation after a lost reply. |
| Export/checkpoint | Bounded typed pages with immutable snapshot identity, epoch/revision cursor and manifest; no arbitrary SQL or body replay. |

Never hold a SQL write transaction across Raw I/O or semantic derivation. Lock
order is admission/fence, source claims in stable logical order, Index ledger and
proofs, Catalog/retained references, then outbox/checkpoint. SQL commit does not
prove Raw durability. Uncertain effects are reconciled by the same durable OpID;
a later cursor or peer result cannot manufacture the original ACK.

PostgreSQL semantic withdrawal qualifies Facts/Source metadata before opening
the SQL writer, under the existing shared Facts and sorted Source locks. The
operation retains those locks through transaction return, while SQL rechecks
persisted authority and exact stage/claim witnesses. Completed publication replay
retains its original acknowledgement semantics. This excludes cooperating
SourceChange writers in the same advisory lock domain; it does not fence arbitrary
external writers or add non-Unix exclusion. The correction and its before-COMMIT
lock-lifetime regressions remain subject to exact-revision CI qualification.

Busy, conflict, refusal and uncertain commit remain distinct outcomes. Diagnostics
are closed and exclude credentials, paths and native contents. Engine errors
never authorize changing engines. Preserve actual before/after append evidence,
Raw/Facts/history custody, byte/WAL/mtime nonmutation checks and reader leases.
Current publication cohorts remain 2..64 rows with a 1 MiB payload bound.

SQLite native ingestion propagates typed `Busy` after the existing 100 ms
reservation timeout. It does not promise that every competing scan succeeds.
The ordinary viewer records a failed refresh and schedules its next check;
committed rows remain the next scan's base. The concurrent-writer fixture permits
only that typed refusal during its existing scan rounds, then requires a final
scan after both writers finish and exact persisted equality with a cold read.
It neither retries within a scan nor extends the reservation deadline.

An unchanged large source that needs a Tree representation repair queues that
repair before beginning native ingestion. The projection worker owns no ingest
OpID or receipt. It must not leave an unexecuted ingest intent behind to block
the semantic publication recovery fence.

Current acceptance preserves the existing two-second cold first-page gate,
five-second publication wait and sixty-second background completion gate.
Historical relaxed thresholds do not authorize weakening these checks. Wall
time includes scheduling, filesystem and database work; investigate failures
with stage measurements while preserving 60 correctly ordered selected sessions,
partial publication before global observation, bounded native observation,
isolation and cancellation. Foreground Catalog/Query remains two seconds.
Discovery barriers, provisional authority, ownership, archive eviction and
source/VM/byte/security bounds remain unchanged.

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
Path-keyed live proofs alone are not sufficient. Current and retained references remain required for both adapters; private PG
qualification must prove the same lifecycle before backend acceptance.

The target GC API must bound pages, bytes and elapsed work per call, record a
resumable cursor, recheck published and admitted-reader references before delete,
and recover incomplete steps after restart. Existing proof GC is reference-bound
and source-mutation driven, not yet a qualified general bounded GC API. Readers
never manufacture missing proofs. Unchanged consumed prefixes may remain
readable across append; interior edits, truncation, replacement, missing authority
or generation mismatch refuse before publishing partial content. Selected I/O
keeps existing byte/deadline/cancellation and covering-block bounds.

## Snapshot/export and migration requirements (undelivered)

`StoreConfig::transfer_inbox` provides bounded capture, staging and authoritative
page application on SQLite and PostgreSQL. `capture(operation)` advances one
durable page and returns capture progress or a manifest; `begin`, `stage_page`,
`checkpoint` and `staged_page` retain exact replay state. `apply_page` commits
authoritative rows, its applied cursor and receipt together. Sequence zero
completes an actually empty manifest. Current materialization includes reported
Facts/stamps, native file/event/usage pages and Catalog content with original
publication receipts, proof generations and presentations. These mappings remain
partially qualified; physical row IDs are adapter-local.
Source journal revision/backfill, exact selected-demand marker, invalidation
claims and source membership now have fixed typed pages and authoritative target
effects, independent of Catalog presence. This does not cover all pending state:
Raw intents/completions, unfinished producer stages and feed floors remain gaps.
The source freeze and incoming-target markers fence ordinary writes, including
PG lifecycle writes. Core-row tests exercise pagination, absent-Raw SQLite
reads and reverse transfer after new PG writes.

These partial manifests are not full-source completeness witnesses and cannot
authorize an ownership flip. Complete current/retained graph export, pending work
and the remaining authoritative families still need implementation and
qualification. `admit_catalog_from(source, manifest, options)` obtains typed
authority from the completed frozen source and checks the exact fully applied
target before executing a genuine Catalog import publication. Host Options must
name the same logical namespace and stream. For PostgreSQL with canonical Facts,
the existing ordinary producer prepares the Facts selection head outside SQL row
locks before the final import transaction rechecks the target checkpoint. Original
producer receipts remain unchanged. This Catalog-scoped operation and its ordinary
destination admission are separate: the import receipt records the target epoch,
and imported publication bindings authorize local leases while their original
producer coordinates and proof IDs remain immutable. Namespace/stream and the
fully applied frozen-source checkpoint are still checked. This operation's
reader path still requires qualification; copied content alone is not an ACK.
Imported Query observations retain their original admission-bound fingerprint.
Each read qualifies the exact row against its immutable selected proof and
original publication receipt, verifies the presentation entry/root, and requires
the import grant to name the destination's current admission and stream. Only
the retained-view lifecycle display tag may differ from the original row.
The original observation binding then checks the current authoritative runtime;
no copied witness is rewritten into destination runtime authority. Missing or
forged provenance, changed runtime, foreign namespace and replaced destination
admission refuse. The per-row lookup uses the namespace/session proof references
and selects one exact metadata match before decoding bounded proof cells.
SQLite's imported Catalog/query closure may be listed under its qualified
generation marker while `catalog_completeness` remains `partial`; this does not
claim complete source migration. A real native component publication preserves
that listing eligibility only from the exact qualified base after its affected
query/projection closure and claim acknowledgements complete with no remaining
invalidations. Its locally minted generation grant names the current admission.
Actual SQLite publishers also mint exact-row observation grants; PostgreSQL
publishers mint pointers verified against their bounded original native operation.
These local authority records are never transported and carried imported rows
receive no new observation grants. Imported immutable receipt epochs reserve the
SQLite producer's next epoch above those receipts without fabricating a stage or
ACK. Runtime changes preserve Catalog stale-status responses independently of
provenance validation. Exact-revision CI qualification is pending.
Native authority-record reclamation remains subject to qualification. Stage
grants share the lifetime of immutable semantic receipts, and PostgreSQL
observation pointers share retained native operation receipts. Direct SQLite
publication instead keeps one namespace generation marker and at most two
observation grants per session key, owned by its latest Catalog and History
metadata. Publication reconciles only touched owners in the same transaction;
copied metadata cannot mint a local grant. Existing read snapshots retain their
prior grant versions through SQLite WAL, while leased immutable proof bodies
retain their separate references and leases. These local grants are not
transported and do not authorize rows absent from Catalog. Exact-revision
retention tests and the general bounded GC contract remain acceptance work.
Source quiesce,
unknown Raw-intent reconciliation before freezing, reader drain, full catch-up,
final qualification and qualified reverse transfer remain undelivered. The
physical inventory tests reject unclassified tables; classification alone does
not establish a working row codec or safe migration.

A migration snapshot must include typed Event/ledger pages and all supported
retained and non-rebuildable metadata: Facts/reported usage, custody and generation
markers, Raw intents/completions, claims, staged work/checkpoints, Current/History,
recipes, versioned proofs and references, change epoch/head/scoped floors.
Each page needs namespace, snapshot/version identity, bounded rows/bytes,
continuation and digest. Checkpoint advancement and applied operation IDs commit
together at the destination. Duplicate pages must be safe to replay.

Semantic withdrawal also owns original recipe custody. A staged publication
may contain only the exact validated retired owners after an ordinary source
removal has already been acknowledged. It preserves their original recipes
through the existing bounded custody algorithm before retiring Current; a
`Deferred` custody page leaves Current and claims unchanged. This uses the same
retirement witness, namespace, epoch and publication CAS as nonempty publication.

Completed semantic producers intentionally delete their temporary model batches.
Migration preserves their original publication receipts exactly and validates
the admitted persisted presentation, Native proof and ledger graph from an
authorized frozen consistent snapshot. It does not re-execute historical
producer computations or require a permanent archive of completed staging
inputs. Unfinished staging/checkpoints remain authoritative transfer state.
An adapter-local physical source ID is not logical source authority: retained
SQLite prefixes bind the original ledger/header/root, and their physical page
addresses may be translated when the optional source-ID hint is absent. This
does not reconstruct missing proofs or grant Usage, Raw or publication authority.
Ordinary reader admission still requires a genuine target publication effect;
copying receipts/content cannot invent a source head, operation, ACK or lease.

Catch-up follows the captured durable change revision and validates counts,
identities, source witnesses, retained custody and proof reachability against
that same snapshot. Hints alone do not export every authoritative mutation;
a complete mutation journal/export API is still required. Cutover needs an
exclusive durable fencing generation and quiesced old writers. Rollback is
qualified only while reverse catch-up/retained authoritative state is verified.
No real user data migration/deletion, deployment, paid resources or control-plane
replacement is authorized here. Only the current supported format is in scope for transfer; there is no old-schema
import, automatic upgrade or historical runtime compatibility.

## Verification and remaining qualification

Storage contract CI compiles actual production callers and runs the current
namespace/admission/runtime contracts. The explicit current-owner group executes
the 13 configured recovery scenarios, five current history-custody scenarios,
search, navigation and SourceJournal tests before the longer native suite. These
include lost ACK, exact replay, receipt/cursor/restart, fault rollback, foreign
namespace invisibility, epoch refusal, retained metadata and indexed budgets.
An effect-hook failure and a real SQLite trigger failure are distinct boundaries;
neither substitutes for the other. Authored tests are not a passing result.

Acceptance requires green checks on the delivered revision, including formatting,
boundary inventory, strict linting and ordinary native business behavior. PG must
also pass the same semantic contracts under actual restricted-role RLS and the
process-wide SQLite-deny guard, with no implicit SQLite/memory fallback. Complete
transfer, catch-up, fenced cutover, qualified rollback and the Hub #313 paired
integration remain open until exact-revision evidence proves them. Component
acceptance or compile success alone is not full #477 acceptance.

## Direct SQLite inventory

This exact inventory is checked against source by the CI boundary script. Entries
are remaining direct dependencies to close or explicitly qualify; inclusion does
not grant a backend-neutral qualification. PostgreSQL adapter code and test-only
SQLite oracles are tracked separately in the writer inventory.

| Path | Dependency | Current boundary |
| --- | --- | --- |
| `storage.rs` | direct | Transitional local Options.cache coordinate bridge only; native ports remain open. |
| `catalog_observer.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `catalog_search.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `causal_trace.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `codex_lifecycle.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/chunk_proofs/published.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/chunk_proofs.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/launch_calls.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/native_usage/repair.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/native_usage.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/relocation.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/reprice.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/semantic_plan.rs` | direct | Shared physical owner planning through metadata ports; admitted SQLite effects remain adapter-owned. |
| `events/store/semantic_stage.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/tree_state.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/configured.rs` | direct | Current namespace-native Event/Facts writer and admitted owner qualification. Public backend qualification remains incomplete. |
| `events/store/history_custody_tests.rs` | direct | Current admitted publisher custody tests retain row/byte budgets, rollback, original recipes and immutable history. |
| `events/store/configured_selected60.rs` | direct | Test-only committed-publication assertions for selected facts, retained-generation witnesses, independent private-witness binding, tamper-before-Raw refusal, negative reference/lease GC and real native multi-window range oracles; writer qualification remains zero. |
| `events/store.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `events/store/native_commit.rs` | direct | SQLite physical native commit stages and independent production parser oracle; shared ordering and relationship semantics live in events/native_commit.rs. |
| `events/store/native_decode.rs` | direct | SQLite row adapter for shared current native decoders consumed by SQLite and private PostgreSQL native readers; full backend qualification remains incomplete. |
| `events.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `facts/native_names.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `facts/source_authority.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `history_projection.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `mcp.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `model/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `model.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_evidence.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_header.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_sidecar.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_timing.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_tree/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `overview.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `overview_repair.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `pricing/runtime.rs` | direct | Repricing registry retains admitted SQLite ownership; registry lifecycle fixtures open current stores. |
| `query/tests/recovery_tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/tests/retained_context_tests.rs` | direct | Test-only retained Query endpoint metadata, physical-owner grant and handoff fixture reads SQLite Catalog/History directly. |
| `query/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/transcript.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query_projection.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query_source.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `received.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `retention.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `selected_read.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `storage/postgres/catalog_effects/tests.rs` | direct | Catalog124 real parser/model and ordinary SQLite Current/recipe producer oracle runs before private PG immutable effects; no complete Catalog or no-SQLite backend qualification. |
| `storage/postgres/catalog_effects/tests/query_recipe.rs` | direct | Test-only admitted SQLite producer and transfer oracle for original immutable compact payloads and private PG Current/History recipe reads; no public factory or production recipe activation. |
| `storage/postgres/native_bundle/tests.rs` | direct | Real-parser SQLite metadata oracle for the connected private partial bundle; no ingest ACK or public qualification. |
| `storage/postgres/tree_writer/tests.rs` | direct | Independent SQLite oracle for private Tree scalar publication; public qualification remains zero. |
| `storage/postgres/native_core/tests.rs` | direct | Real-parser SQLite oracle for the private PG core row family; no ingest ACK or public qualification. |
| `storage/postgres/native_ingest_tests.rs` | direct | Independent ordinary SQLite parser oracle for the private PostgreSQL native EventCache integration; no complete backend qualification. |
| `storage/postgres/native_schema.rs` | direct | Test-only SQLite parser oracle and EventCache path cleanup for native tree values; production schema verification uses PostgreSQL. |
| `storage/postgres/native_usage/tests.rs` | direct | Shared-parser SQLite oracle for the partial PG stateful usage family; no complete backend qualification. |
| `storage/postgres/native_usage_identity/tests.rs` | direct | Test-only shared-parser SQLite oracle for PG copied-identity effects; no PG backend or no-SQLite qualification. |
| `storage/postgres/semantic_events.rs` | direct | Private PG event projection reuses the ordinary qualifier through its existing rusqlite result type; error-type decoupling remains required. It reads PG native rows and does not open SQLite. Full backend qualification remains incomplete. |
| `storage/postgres/tests.rs` | direct | Test-only EventCache pathname check proves cold PostgreSQL Catalog waiting creates no SQLite session store; the readiness path uses PostgreSQL metadata. |
| `storage/postgres/tree_checkpoint/tests.rs` | direct | Test-only real-parser SQLite oracle for the private PG checkpoint family; public backend qualification remains zero. |
| `storage/postgres/tree_writer/tests.rs` | direct | Independent SQLite oracle in private PG Tree tests; no production backend qualification. |
| `storage/transfer/reported_tests.rs` | direct | Test-only admitted SQLite context for authoritative Facts round trips; production SQL stays in adapters. Full transfer qualification remains incomplete. |
| `storage/postgres/transfer.rs` | direct | Test-only admitted SQLite seed for the exact selected pending marker; journal/claims are read and resumed through ordinary Source observation ports in both directions. Full pending-state transfer remains incomplete. |
| `storage/postgres/transfer/catalog.rs` | direct | Test-only real SQLite ingest/publication oracle and scoped connection for carried imported-row grant corruption injection and restoration; no public PostgreSQL factory or full transfer qualification. |
| `test_hooks.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `transcript_range.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `viewer.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
