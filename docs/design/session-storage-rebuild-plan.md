# Session storage rebuild plan (#477)

Status: plan for the rebuild from `main` decided on 2026-11-06 (PR #493).
It replaces the receipt-bearing narrow line. The binding decisions are:

- The SQL session store is a **projection cache over Raw** (#478, 2026-11-06).
  Each projection unit carries a cheap tag: Raw identity, consumed-prefix
  digest and projector version. A reader checks the tag. On a mismatch, an
  unknown version or a stale generation it refuses honestly (and the
  producer rebuilds from Raw); it never serves mismatched data.
- Each database has one version stamp. Per-connection physical schema
  re-proving, presentation receipts and roots, publication receipts,
  observation grants, page-read receipt authentication, SQLite guard triggers,
  capability UDFs and per-connection temp views are not brought over.
- State that cannot be rebuilt from local Raw keeps strict integrity:
  transactions, namespace binding and, on PostgreSQL, RLS.
- Pre-release policy (AGENTS.md) applies: no compatibility layer for older
  Semon stores. Unsupported stores are refused or rebuilt from Raw; user data
  is never deleted. Native identity, lineage, provenance, parser behaviour and
  sanitized fixtures are preserved.

Every slice keeps main's gates green: `rust.yml` (fmt, clippy, test, including
the cold first-page gate
`viewer::tests::initial_catalog_page_precedes_global_native_observation`
against its unchanged 2 s deadline) and `ui.yml` with main's outcome. No gate,
threshold, fixture size or build/test profile is loosened and no test is
skipped. Timing is compared against main on the same runner from a diagnostic
branch; instrumentation never lands on the rebuild branch. The first-page
target is within about 15% of main.

## Slice 1: namespace-bound ports, SQLite adapter, projection tag, version stamp

Scope:

- `crates/semon-sessions/src/storage.rs`: the backend-neutral port. A
  `Namespace` is a logical source binding, never a path or a credential. Every
  port operation takes an explicit `&Namespace`. Distinct capability types
  separate reading (`CatalogReader`: Catalog pages, metadata search, retained
  History and selected identities), Event index/ledger writing (`IndexWriter`)
  and Catalog publication (`CatalogPublisher`). A capability bound to one
  namespace refuses another namespace before any I/O, so a refusal has no
  partial effect.
- `storage/sqlite.rs`: the SQLite adapter. One database per namespace
  placement, keeping main's single-connection writer (`EventCache` /
  `SqliteStore`) and its read-only snapshot readers. The writer binds the
  database's namespace through its own connection; readers and writers refuse a
  database bound to another namespace.
- Version stamp: one `storage_stamp` metadata row per database, written in
  the initialization transaction (format, backend, schema, parser, projector).
  `SCHEMA_VERSION` moves to 35, so main's existing obsolete-store path rebuilds
  pre-stamp stores from Raw and focused readers refuse them.
- Projection tag: a Catalog unit's published sources carry the Raw identity
  (device, inode) and the consumed-prefix digest (offset, prefix and tail
  SHA-256) that main's ledger and byte-proof checks qualify against native
  bytes; the projector version is the database's Catalog version. Readers
  check every Current unit on a page against the ledger in the same snapshot.
  A ledger shorter than the publication, an altered prefix at the published
  boundary or another projector version refuses the read with a retryable
  `projection_stale` result; the producer's invalidation then rebuilds the
  unit from Raw. An append-only advance of the ledger keeps the published
  prefix readable. A content-identical copy at another Raw identity is the
  same prefix; when the ledger has moved past the boundary at another
  identity, the selected read's byte proof qualifies the prefix against the
  native bytes, as on main.
- The viewer, union, Query and bookmark readers call the port.

Acceptance checks:

- `rust.yml` fmt, clippy and test pass; `ui.yml` matches main.
- New conformance tests in `storage::conformance`: namespace collision
  between two sources observing the same native path, wrong-namespace refusal
  with no partial success, capability separation, reopen keeps the binding,
  version stamp refusal, and tag refusal for an altered prefix, a shortened
  ledger and a wrong projector version; an append-only advance and a
  content-identical copy stay readable; an in-place Raw rewrite is rebuilt.
- One same-runner timing run against main: isolated first page for
  unrelated=0/256/2048 and the full-suite first page.

Main tests that guard it: `initial_catalog_page_precedes_global_native_observation`,
`journal_append_and_warm_restart_ignore_unrelated_history`,
`native_codex_archive_unarchive_reuses_the_indexed_generation`,
`obsolete_producer_contracts_rebuild_once_and_keep_focused_reads_read_only`,
`selected_record_windows_reuse_qualified_bytes_and_reject_rebinding`,
`query::tests::recovery_tests::`, the focused Query identity-collision tests
and the Catalog/viewer suites.

Known limit carried into slice 2: production producers still open
`EventCache` directly, so ordinary local databases stay unbound until the
cohort intents route the producer through `IndexWriter`. An in-place rewrite
that grows the file past the published boundary is detected by main's native
source-state check, not yet by the ledger tag; slice 2 adds a ledger epoch.

## Slice 2: cohort intents and the strict custody set

Scope:

- Cohort-batched crash-recovery intents: persist every host ingest intent of a
  cohort before Raw mutation and keep it queued until the cohort's Raw checks
  pass; qualify durable bytes before completion/ACK; reconcile interrupted or
  uncertain writes idempotently.
- The strict custody set, state that cannot be rebuilt from local Raw:
  reported Facts and usage from other machines, durable change-delivery
  epochs/heads/cursors, custody and generation markers, Raw intents and
  completions, claims, staging checkpoints, and History whose Raw has been
  withdrawn. Withdrawing Raw first hands the affected History over to this set.
- Route the producer through `IndexWriter`/`CatalogPublisher`, binding every
  production database's namespace, and add a ledger epoch to the tag.

Acceptance checks: the four ported recovery tests (kill/fault around Raw
writes and SQL commits replay without loss or duplicate logical sessions),
partial-publication withdrawal custody, child-process exit during an
uncommitted rebuild, and all slice 1 checks.

Main tests that guard it: `query::tests::recovery_tests::`,
`native_relocation_history_witness_survives_failed_publication_and_an_append`,
`catalog_statement_journal_batch_probe_preserves_catalog_feed_and_transaction_rollback`,
`events::store::reprice::tests::`, the `catalog_changes` suite and the
retained-history Catalog tests.

## Slice 3: PostgreSQL adapter with RLS

Scope: a complete PostgreSQL session-data adapter behind the same ports
(namespace on every call, the same tag and stamp), selected explicitly at
startup; PostgreSQL mode creates and reads no hidden SQLite session store.
Namespace isolation is enforced by row-level security for a restricted role.
The transaction foundation and ownership work from PR #487 and PR #485 is the
porting reference.

Acceptance checks: the slice 1 and 2 conformance suites run against both
engines; a real restricted-role PostgreSQL proves RLS; no SQLite file is
created in PostgreSQL mode.

Main tests that guard it: the whole SQLite suite stays green; the PostgreSQL
job is added without loosening any existing job.

## Slice 4: remaining #477 test plan

Scope: the #477 Test list on both backends — cold ingest, trusted append,
replacement/truncation, relocation, restart, current/retained and selected
transcript/field tests; concurrent Raw/Index/Catalog advancement with altered
prefixes, stale generations and unauthorized source access refused without
partial success; kill/fault injection around Raw writes, SQL commits,
migration checkpoints and cutover; multi-source collisions, stable provenance
and bounded global lists; and the performance measurements (cold/progressive
ingestion, idle work, append read/hash bytes, long-session pages, database
size/write amplification, lock/pool waits, multi-machine contention).

Acceptance checks: every #477 Test item has a named passing test or
measurement on both backends, and all current performance gates still pass.
