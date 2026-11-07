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

Implemented in slice 2:

- The native producer opens main's single scoped writer through the port's
  `open_index`, which binds the database to the host's source namespace (the
  local source's empty key binds as `local`) and refuses a database bound to
  another namespace before any index or Catalog write. `SessionSources`,
  received machine views and `SessionCatalogObserver` (now constructed with its
  source key) carry the namespace; key-less producer constructors are
  test-only. Publication stays on that one writer connection.
- Ledger epoch: a ledger written without append changes takes a fresh
  database-wide epoch, and publication records each source's epoch. Past the
  published boundary at the same Raw identity, a changed epoch is a
  `ledger_epoch` mismatch, not an append. `SCHEMA_VERSION` 36.
- Intents: local cold ingest never writes Raw; it only reads native logs, so
  it has no Raw crash window to journal. A crash before the cohort's index
  commit leaves nothing durable and the next scan rebuilds from Raw. Main's
  existing mechanisms already cover the real windows and are kept: the cold
  index cohort (at most 64 sources) commits in one transaction with a
  per-source generation fence (a mid-apply fault rolls the whole cohort back,
  one changed source refuses it, and replay of a committed cohort is a
  conflict, not a duplicate:
  `initial_index_batch_rolls_back_evidence_and_revisions_and_rechecks_each_generation`);
  host Raw writes journal a durable intent before touching bytes and complete
  after fsync (`SessionSourceWrite`), and an interrupted write grants no append
  trust and is replayed conservatively
  (`receiver_append_chain_reads_only_boundary_and_recovers_interrupted_or_corrupt_proofs`).
  No in-repo host batches Raw writes, so per-cohort host intents are not added.

Carried to slice 3/4: custody writers that open their own connection
(`SessionSourceWrite`, `SessionFactsWrite`, `retain_source_event_index`, the
CLI `collect`/`FactsSource` index) do not yet take a namespace; the
partial-publication withdrawal custody test; per-connection `qualify_contract`
and the direct SQLite readers (page-ready, union generation, native
resolution, in-snapshot transcript reads) behind the reader capability.

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

Implementation checkpoint (2026-11-07): partial slice, not a selectable
PostgreSQL session backend yet.

- Carried fixes: invalid host source keys are retained as lazy configuration
  errors and refused before opening an index. `SessionSourceWrite`,
  `SessionFactsWrite`, `collect`, `FactsSource`/`local_facts`,
  `retain_source_event_index` and `session_source_proof` take an explicit source
  key. Custody journal transactions bind/check their own connection; read-only
  proofs and retention check the requested namespace in their snapshot.
- Optional `postgres` feature: an injectable `PgProjectionStore` provides
  repeatable-read projection transactions, namespace binding, version stamps,
  ledger CAS, namespace-wide replacement epochs, tag publication and tag checks.
  The host provisions `postgres_projection_schema()` with a separate owner role.
  The one database version stamp is read-only for the application role.
  All tables have forced RLS; the adapter refuses superuser, BYPASSRLS and table
  owner application clients. Namespace context is transaction-local. This module
  opens no local database and reads no credentials.
- CI: `postgres.yml` uses a free PostgreSQL 16 service container and a restricted
  non-owner application role. The storage conformance module exercises SQLite
  and the PostgreSQL transaction subset, direct SQL foreign-row read/write
  refusals, context reuse, ledger conflicts/rollback and unchanged local files.
  The unchanged-local-files assertion covers this projection API only; it is
  not qualification of a complete PostgreSQL mode.
- Part 2 index checkpoint: `PgProjectionStore::index_source` reuses the native
  parser and commits normalized events/extras/signals, metadata, consumed ledger
  and immutable block-proof generations in one restricted-role transaction.
  Records and proof nodes use batched inserts. Idle scans preserve the epoch;
  append preserves the old selected-prefix proof and replacement gets a new
  epoch. Exact block reads qualify a root through indexed sibling witnesses.
  The PostgreSQL schema stamp is now version 2; unsupported old projection
  schemas are refused rather than migrated. There is no native body in SQL.
- Part 2 qualification remains scoped to that index API. PostgreSQL lifecycle
  coverage includes cold ingest, idle, append, restart, replacement, retained
  prefix proofs, altered-proof refusal and foreign-namespace RLS. Shared parser
  comparisons cover normalized rows and consumed proofs; they do not establish
  Catalog or selected transcript parity. Proof collection, bounded incremental
  SQL updates and complete producer integration remain pending.
- Part 3 storage checkpoint: schema 3 adds typed bounded Catalog publication
  under generation/source CAS, Current and retained-History metadata, versioned
  source-bound recipe windows and metadata substring postings. Reads check the
  row's source tag against its publication and committed ledger in one
  repeatable-read transaction. Publication and withdrawal have savepoint rollback
  on refusal, so callers cannot commit a partially applied operation. New tables
  have forced namespace RLS. No receipts, grants, triggers, capability functions
  or temporary views are introduced.
- Part 3 custody primitives: revision-CAS reported Facts and staging checkpoints,
  namespace custody epoch, transactional scoped change heads/cursors and bounded
  acknowledgement compaction. Raw withdrawal first captures the committed Catalog
  and recipes, retires Current and emits change hints atomically. Unsupported
  composite History handoff refuses without replacing older custody. This does
  not yet implement the host Raw intent/completion or generation-marker ports.
- Part 3 qualification is a storage API subset, not full producer/viewer parity.
  Tests cover cold publication, append/restart, replacement/truncation, stale
  publication, late-refusal rollback, recipe source/version generations, a
  prepared-but-unpublished append during withdrawal, Facts/checkpoint restart
  and rollback, change cursors/compaction and direct SQL RLS on the added tables.
  The same typed row and recipes are compared with SQLite's publisher. The
  PostgreSQL lifecycle root has no SQLite session files; its explicit SQLite
  comparison uses a separate root. The measured ingest-through-typed-Catalog
  first page is reported separately and is not a viewer first-page measurement.
- Index writes no longer delete/reinsert all normalized rows: an upsert updates
  only differing values and removes truncated tails. Serialization and SQL input
  still cover the complete index; bounded delta preparation and proof-generation
  collection remain pending.
- Part 4 host-port checkpoint: `PgProjectionStore::publish_sources` indexes a
  bounded allowlisted cohort, captures PostgreSQL indexes/claims, reuses native
  Catalog derivation without an attached SQL store, and publishes recipes under
  Catalog/source CAS. Runtime observation capture explicitly bypasses SQLite for
  this derivation. This is explicit-cohort production, not machine discovery or
  global lineage qualification; served completeness and search coverage remain
  partial. The host retains source custody through this blocking native work.
- `catalog_reply` reuses the existing Catalog query parser, response presentation,
  source-root validation and runtime fences. PostgreSQL filters and keyset cursors
  cover harness/repo/no-repo/model/metadata search. `transcript_reply` reads identity
  and recipe windows in one PostgreSQL snapshot and uses the existing native
  renderer through an independently authorized host `SessionSourceReader`.
  Generation mismatches refuse; native text remains source-backed. Causal range
  context is explicitly incomplete, and selected details/field/annotation routes
  are not implemented on this host port. Native proof verification remains the
  host reader's obligation; no hidden SQLite proof connection is opened.
- Schema 5 adds forced-RLS Raw intent revisions and completed host generation
  markers. Commit intent before Raw mutation, retain the host's exclusive custody
  fence, then complete after fsync and independent byte qualification. Restart
  retains pending state, stale completion refuses, and uncommitted intents roll
  back. Each token also carries a PostgreSQL-generated random intent ID, so a
  token returned from an aborted transaction cannot complete a later reused revision. Pending writes refuse Current reads and producer publication; intents
  advance the namespace epoch so a racing repeatable-read publisher refuses.
  These primitives do not yet replace `SessionSourceWrite` in runtime callers.
- Part 4 qualification covers the shared producer, actual Catalog response,
  provider-backed selected prompt, append/restart and Raw custody/RLS. The
  sanitized Codex comparison matches shared Catalog metadata/filter results and
  selected rendered entry content with SQLite. A Linux inotify watch over a real
  test-owned SQLite sentinel detects hidden read-only opens and mutations; a
  separate PostgreSQL root also checks that no SQLite session file is created.
  This remains host API qualification, not end-to-end startup mode. The shared-producer-to-Catalog-response measurement
  is separate from ViewerCore startup and must not be called the PostgreSQL
  viewer cold first-page number.
- Part 5 runtime checkpoint: a source-owned `RuntimeStore` now owns namespace
  validation, SQLite writer opening and native-index fallback, page readiness,
  ViewerCore Catalog dispatch, and selected SQLite snapshot opening. SQLite keeps
  the existing single writer and selected snapshot/proof lifetime. A private,
  test-only PostgreSQL injection supplies the same NativeEvidence and ViewerCore
  owners with a source-owned reactor; it serves Catalog, a source-backed selected
  transcript, and ordinary Query through PostgreSQL indexes. The shared native
  Query derivation also bypasses SQLite runtime observations for this path.
- The injected producer advances bounded native discovery (64 inputs / 512
  entries), retaining refused cohorts for retry. It does not yet reconcile
  cross-cohort lineage, removals, empty-source publication or latest-first cold
  discovery. Native Query currently discovers and indexes its complete input set;
  bounded incremental preparation remains pending. Native source locks span
  PostgreSQL producer and Query derivation. The selected local reader loads
  current proof generations and refuses an older selected prefix; lazy retained
  prefix proof reads remain pending. Unported ViewerCore and focused/Catalog
  Query operations refuse before any SQLite access.
- CI's private-injection runtime probe uses the sanitized Codex parser fixture,
  ViewerCore Catalog and transcript routes, ordinary Query listing, foreign-source
  read/write refusal, and the inotify SQLite sentinel. PostgreSQL conformance and
  strict feature Clippy passed at `866cb6a22` (run 42311367916). The first-page
  sample at the preceding runtime checkpoint was 70.1 ms (run 42310636276).
  This measures real ViewerCore with private backend injection; it is not yet
  qualification or timing of selectable startup configuration.
- Follow-up runtime adoption (2026-11-07): `RuntimeStore` owns the producer
  fence and work state, including SQLite's existing single `EventCache`. Native
  resolution and the Catalog union generation read dispatch through the handle.
  PostgreSQL native aliases remain explicitly provisional. Focused Query derives
  from qualified PostgreSQL indexes with request-owned source readers, a bounded
  relationship closure and a publication/runtime recheck after rendering. Catalog
  Query listing and stalls use published exact summaries and source-bound
  relationship context under forced RLS (PostgreSQL schema 8). The activity Query
  tool remains explicitly unavailable on PostgreSQL.
- Follow-up discovery: bounded traversal persists membership under RLS and keeps
  only the newest 64 cold candidates in memory before first publication. Removed
  inputs hand their published observation to retained custody; an observed empty
  machine publishes an empty page. Published native parents/aliases can expand a
  cohort within the existing 64-source bound; Codex header candidates are bounded.
  Refused cohorts leave the hot queue and are re-admitted by a later audit.
  Complete global lineage, empty replacement History handoff, composite History
  and atomic discovery checkpoints still require qualification. The Catalog
  continues to report partial completeness.
- Host custody adoption (2026-11-07): `SessionSourceWrite::begin_in` and
  `SessionFactsWrite::begin_in` borrow an admitted `SessionSources` runtime owner;
  invalid, foreign and retired source keys refuse before custody I/O. PostgreSQL
  JSONL intents commit before mutation, survive interruption/restart and complete
  with an independently hashed, physically fenced generation after the host fsync.
  Completion requeues discovery in the same transaction. Facts retain the last
  completed report separately from a pending wanted report; exact revision/content
  completion publishes it and requeues discovery atomically. Pending Facts refuse
  Current reads and production. Schema 9 replaces schema 8 directly; there is no
  migration. PostgreSQL sidecar/process-record custody remains explicitly refused.
- Host collection adoption: `collect_in`, `FactsSource::new_in` and
  `local_facts_in` use the admitted PostgreSQL indexes with an unattached in-memory
  derivation cache, retaining reported usage in PostgreSQL across collections.
  Local Facts collection refuses recorded-machine options, so local observations
  cannot overwrite reported machine custody. Options-only APIs still explicitly
  construct the default SQLite backend; existing CLI/receiver callers have not
  yet adopted a selectable backend. PostgreSQL readers/producers try shared Raw
  and Facts fences without waiting under the reactor mutex, so host completion
  cannot deadlock behind a reader waiting for its exclusive custody fence.
- Discovery success now clears pending work in its Catalog publication transaction,
  including a qualified new empty source. A later Raw/Facts completion cannot be
  acknowledged by an earlier publication's separate dequeue. Empty replacement
  handoff and refused-work dequeue ordering remain incomplete.
- Runtime custody follow-up (2026-11-07): admitted-owner APIs now dispatch source
  proofs, projection readiness and explicit index retention to PostgreSQL. Schema
  10 adds forced-RLS retention markers bound to the exact ledger, normalized
  index/proof digest and recipe generation, plus complete-discovery/refused-work
  state. Readiness refuses partial discovery, pending or refused cohorts and
  ambiguous owners. Retention captures outgoing History before Raw withdrawal;
  discovery removal preserves explicitly retained normalized indexes. Changed
  ledgers clear the marker. No Raw is deleted by these operations.
- Sidecar and process-record writes now use the same durable PostgreSQL intent,
  fsync/physical-fence and completion contract as JSONL. Pending runtime inputs
  fence Current and collection; completion conservatively requeues known owners.
  `SessionSourceRelocation::begin_in` fences both paths, verifies the complete
  native prefix without a SQLite lookup, journals the destination and independently
  verifies the copied bytes before completion. The CLI collection loop and received
  Facts publication now adopt admitted owners. Options-only APIs remain explicitly
  SQLite; the receiver transport itself still does not journal each Raw mutation.
- Selected transcript windows now resolve exact retained-prefix proof generations
  and fetch only covering block witnesses through the same repeatable-read
  PostgreSQL snapshot as identity/recipes. A request-local 1 MiB evidence budget,
  physical carrier pin and final recheck fence local bytes; no complete normalized
  index decode or SQLite proof connection is used for this path. Append between
  Index and Catalog publication is covered independently from altered-prefix
  refusal. Provider-backed reads keep their independently authorized provider.
- Still required: explicit startup backend configuration; complete receiver Raw
  custody adoption and relocation Index reuse; remaining direct selected readers
  and full discovery reconciliation. No PostgreSQL startup option is exposed while
  these paths require SQLite or refuse. Selected activity, complete search coverage,
  composite Current/History merging, empty-replacement History handoff, Catalog
  Query composite-History, selected details/causal context/fields/annotations,
  proof-generation collection and bounded delta preparation remain pending.
  Full shared lifecycle conformance and full-pipeline startup RLS/no-hidden-SQLite
  qualification remain pending; private-injection timings are not real startup.
- Follow-up qualification extends the private-injection runtime probe with host
  Raw/Facts interruption, restart and completion; pending-writer production/
  collection refusal; admitted collection/local Facts and foreign-source refusal;
  and empty-source checkpoint completion, under the existing inotify sentinel.
  SQLite conformance adds the admitted local-key writer and retired-owner refusal.
  No existing assertions or thresholds are loosened. CI evidence is reported
  against its pushed SHA; private-injection timings do not qualify real startup.

Acceptance checks: the slice 1 and 2 conformance suites run against both
engines; a real restricted-role PostgreSQL proves RLS; no SQLite file is
created in PostgreSQL mode. The transaction subset does not complete these
full-backend acceptance checks.

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
