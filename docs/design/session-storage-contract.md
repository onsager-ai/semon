# Native session storage contract (#478, first increment)

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
| `catalog.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `catalog_observer.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `catalog_search.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `catalog_union.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
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
| `native_resolution.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_sidecar.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_timing.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `native_tree/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `overview.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `overview_repair.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/catalog.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/tests/recovery_tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/tests.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query/transcript.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query_projection.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `query_source.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `received.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `retention.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `selected_read.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
| `session_bookmark.rs` | direct | Remaining SQL/path coupling (including tests); see ownership below. |
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
