# Prerelease compatibility cleanup (#475)

Implementation baseline: main `4ef9ac9a077861425c632e3d15863dc849760d7a`.
The user released the implementation hold and waived historical Semon
compatibility. This change does not merge policy PR #476 or repeat #470.
Current schema is 33. The October 23 user correction supersedes permanent
refusal for recognized obsolete derived indexes: writable producers discard and
reindex them under the [storage contract](session-source-reader.md#current-semon-storage-contract).
There is no historical conversion, and no original native/archive data is deleted.

Main `644885090fc5e3a84a85dab47cca48c3824e3333` is integrated with its current
proof snapshots, selected field recovery and Viewer progress behavior. Selected
repair opens qualify an existing current WAL database without initialization or
unrelated archive traversal. Custody writers validate the selected marker;
ordinary producer opens qualify all retained markers. Both paths use the same
schema and parser contract.

| Branch / historical Semon contract | Live callers | Data category | Disposition |
| --- | --- | --- | --- |
| Schema 1–31 ALTER/additive upgrades, usage-identity bootstrap, schema-10 source hashes, schema-17/18 outbox bootstrap, overview/search promotion | ordinary/scoped store opens; focused reader | mixed derived rows, reported facts, retained recipes and proofs | Removed; one coherent fresh DDL; recognized obsolete index discarded by producers; future/foreign/nonempty-zero refused |
| Parser/cache mismatch DERIVED reset | store init | derived rows mixed with retained provenance | Removed; recognized older parser invalidates the whole index, newer/malformed refused |
| Proof pages without `sha256`, headers without `pages_root`, whole-vector qualification, resumable `native_proof_repair` preparation and diagnostic-field suppression for old readers | focused proof reads, complete chunk reads, proof readiness and published-generation retention | rebuildable content-free byte proofs | Removed; current page witness and Merkle root required, missing/null fields and unsupported versions refused without read-path writes; writers emit `consumed_offset`; current pinned snapshots, retained publications, reference-counted GC and WAL lifetime guards retained |
| Generic cache open/reopen fallback for unsupported stores | NativeEvidence preparation; complete scan; FactsSource | observable history | Unsupported errors propagated; missing tables/columns required by the complete loader (including reported facts and conditional native projections) refused before writes; operational in-memory fallback and current physical-corruption set-aside remain |
| Archived marker versions 1–5 | archive qualification, retention and publication membership | archive custody/source proofs | Read paths refuse; producer invalidates recognized obsolete index; only marker 6 written, no archive deletion |
| Tree summary versions 1–2 and old inline native-usage accumulator reader/writer | selected/native tree and FileIndex reads | derived native projections | Removed; current tree 3 and normalized usage root 1 only |
| Native-usage discovery/conversion queue and old representation worker | background NativeEvidence scheduler | derived usage/tree with retained evidence | Removed; explicit current tree repair retains source locks, receipt/source/stat/generation and proof checks |
| native_tree_legacy_max_file and copied-identity backfill queue/reader | tree write and shared-usage reads | derived copied usage | Removed; current usage_identity insert/delete triggers maintain the sole indexed representation |
| Search format-1 postings plus lazy per-key conversion and UNION readers | Catalog search and publication | rebuildable allowlisted metadata | Removed; current memberships only, with per-key bounds, exact counts and atomic rollback |
| Old semantic recipe reset | staged Catalog producer | derived relationship work | Recognized older recipe invalidates whole index; current runtime/source/generation reset guards retained |
| Unscoped Home seen-result aliases | Home endpoint and current Viewer | browser seen-state metadata | Removed; current source-qualified IDs only; existing localStorage left in place |
| Optional missing history-table fallback | focused retained lookup and Catalog list | retained identity/recipes | Removed; current schema always owns history tables |
| Current projection/recipe/outbox versions and unsupported-header custody | Query/Catalog/Transcript, cross-harness replacement | provenance, immutable recipes, private staged custody | Kept-current: version/integrity/source/generation guards and opaque unsupported-recipe custody remain necessary |
| Catalog row-wise writer outside 2–64 rows / 1 MiB / unique-key batch eligibility | current Catalog publisher | current metadata | Kept-current; test selector renamed RowWise; fallback/rollback coverage retained |
| Upstream Claude/Codex/Copilot historical log formats | native adapters/parsers | supported upstream input | Kept-current; these are not old Semon contracts |

Focused Catalog, Query, Transcript, overview and retention readers qualify the
current database contract in their read paths. Outbox compaction and selected
publication recheck versions under their write locks. Focused reads do not scan
unrelated archived markers: marker consumers validate the selected record, while
store initialization classifies markers before whole-index invalidation or current writes.

Schema qualification prepares the complete loader's SQL dependencies without
reading history rows, including the reported-run columns and conditional native
usage/tree readers. Missing or malformed SQL objects are unsupported state, not
an empty in-memory history. Persistent SQL/value decoding errors also propagate
as refusals. Lock, permission, I/O, capacity and interruption failures retain the
existing operational fallback; physically damaged/non-SQLite files retain the
existing set-aside recovery. Refusal tests check future/foreign/malformed state
through EventCache and Facts, unchanged database/WAL bytes and retained custody.
Rebuild tests cover complete/scoped producer opens, restart, concurrent opens,
WAL snapshots, native Catalog/history/selected reads and original source integrity.

Historical upgrade/parity fixtures are removed. Current cold-start, incremental,
restart, refusal/no-mutation and corruption/availability/custody checks are the
acceptance evidence. Required runtime and formatting checks run in CI only:
local thermal readings are unavailable, and host policy forbids local Cargo and
Node test/typecheck execution. CI success and independent final-diff acceptance
must be reported separately and bound to the final pushed SHA.
