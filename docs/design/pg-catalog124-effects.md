# Private PG Catalog effects, Worker124

This increment is incomplete and grants no public backend qualification. It
implements immutable physical Current metadata, exact source vectors, derived
native dependencies, slot headers/recipes, and trace headers/turns/edges/bookmarks.
The shared `publish_current_sources`, `publish_slot_rows` and
`causal_trace::publish_with_store` algorithms serve SQLite and PG. No parser,
identity derivation, recipe derivation or search algorithm is copied.

`storage::postgres::catalog_effects::stage(&mut PgTxn, generation, &Publication)`
is private, borrows the caller's transaction and returns only `()`. It does not
write a Catalog head, acknowledge claims, append durable publication hints or
return a publication/ingest receipt. It must not be connected to readiness until
the complete owner transaction qualifies admission, runtime/epoch, exact source
claims, retained history and native proof ownership. A hash pin is not a lease.

All nine physical tables are in the existing fresh-only current schema manifest:

| Table suffix after `semon_pg_native_` | Contents |
| --- | --- |
| `catalog_rows` | Publication generation, key digest, full key, Catalog JSON bytes |
| `catalog_sources` | Exact source cohort and full source JSON bytes |
| `catalog_dependencies` | Shared native dependency effects |
| `catalog_recipe_headers` | Recipe/source generations, version, total |
| `catalog_recipe_rows` | Ordered immutable recipe bytes |
| `catalog_trace_headers` | Trace generation, source generation, metadata |
| `catalog_trace_turns` | Exact turn/start identities and metadata |
| `catalog_trace_edges` | Exact edge identity and metadata |
| `catalog_trace_bookmarks` | Target/session/edge identities |

The schema manifest hash changes; there is no upgrade path or old-profile reader.
Fresh stores acquire forced namespace RLS and exact column/key/index/policy
verification through the existing installer. All opaque identities are `bytea`;
btree keys contain bounded SHA-256 digests and generation/ordinal values. Conflict
handling compares all full stored identities and bytes and refuses changed
same-generation effects. Old generations are never deleted by this module.
Per-row metadata is bounded to 1 MiB, publication batches to 64 rows, and source
vectors to 64 entries. Deadlines/cancellation remain owned by the borrowed PgTxn.

CI tests use real sanitized Claude input, the ordinary parser/model, and actual
SQLite Current/slot/trace producers before the PG phase. Fixture databases are
closed and removed before PG work. This is scoped physical producer parity,
not effective instrumentation proving a complete no-SQLite backend.

Registered tests (PostgreSQL workflow filter `pg_catalog124`):

- `pg_catalog124_real_sqlite_producer_recipe_parity_and_immutable_generations`
- `pg_catalog124_all_measured_boundaries_rollback_and_changed_generation_refuses`
- `pg_catalog124_long_opaque_keys_namespaces_and_precancel`
- `pg_catalog124_injected_source_role_rls_and_namespace_reuse`

The Storage workflow checks the exact direct-SQL test inventory and default
adapter conformance; public qualification remains zero. Exact-head CI and fresh
independent acceptance are still required. No local build/test/fmt was run:
thermal telemetry is absent. No claim is made for the contract's 0/256/2048 work,
two-range keysets, cold/append/replacement/relocation journey, selected rendering,
proof tampering, concurrent owner CAS or lost-ACK semantics.

Remaining Catalog work includes OriginalHistoryStore/CompositeHistoryStore,
SearchPublicationStore, overview, native proof generation references, writer-owned
Current/history ref changes, complete CatalogPublicationHead admission/CAS/change
hints, CatalogSnapshot, NativeNavigationStore, selected presentation, bounded SQL
filters/keysets and full family tests. Full #477 also retains complete native
ingest/read, semantic staging, ReaderLease/GC, recovery, current-format cross-engine
migration/catch-up/fenced cutover/qualified rollback and Hub #313. #380 must finish
before #477 MAIN merge. Merge, deployment, publication and live-data migration
remain unauthorized.
