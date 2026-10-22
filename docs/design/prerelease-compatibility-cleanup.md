# Prerelease compatibility cleanup (#475)

Implementation baseline: main `4ef9ac9a077861425c632e3d15863dc849760d7a`.
The user released the implementation hold and waived historical Semon
compatibility. This change does not merge policy PR #476 or repeat #470.
Current schema is 32; older stores, including schema 31, are explicitly
unsupported. No operator data is deleted, reset or upgraded by this change.

| Branch / historical Semon contract | Live callers | Data category | Disposition |
| --- | --- | --- | --- |
| Schema 1–31 ALTER/additive upgrades, usage-identity bootstrap, schema-10 source hashes, schema-17/18 outbox bootstrap, overview/search promotion | ordinary/scoped store opens; focused reader | mixed derived rows, reported facts, retained recipes and proofs | Removed; one coherent fresh DDL; older/newer/nonempty-zero refused |
| Parser/cache mismatch DERIVED reset | store init | derived rows mixed with retained provenance | Removed; parser mismatch refused before writes |
| Generic cache open/reopen fallback for unsupported stores | NativeEvidence preparation; complete scan; FactsSource | observable history | Unsupported errors propagated; operational in-memory fallback and current corruption set-aside remain |
| Archived marker versions 1–5 | archive qualification, retention and publication membership | archive custody/source proofs | Refused; only marker 6 accepted; no archive deletion |
| Tree summary versions 1–2 and old inline native-usage accumulator reader/writer | selected/native tree and FileIndex reads | derived native projections | Removed; current tree 3 and normalized usage root 1 only |
| Native-usage discovery/conversion queue and old representation worker | background NativeEvidence scheduler | derived usage/tree with retained evidence | Removed; explicit current tree repair retains source locks, receipt/source/stat/generation and proof checks |
| native_tree_legacy_max_file and copied-identity backfill queue/reader | tree write and shared-usage reads | derived copied usage | Removed; current usage_identity insert/delete triggers maintain the sole indexed representation |
| Search format-1 postings plus lazy per-key conversion and UNION readers | Catalog search and publication | rebuildable allowlisted metadata | Removed; current memberships only, with per-key bounds, exact counts and atomic rollback |
| Old semantic recipe reset | staged Catalog producer | derived relationship work | Refused; current runtime/source/generation reset guards retained |
| Unscoped Home seen-result aliases | Home endpoint and current Viewer | browser seen-state metadata | Removed; current source-qualified IDs only; existing localStorage left in place |
| Optional missing history-table fallback | focused retained lookup and Catalog list | retained identity/recipes | Removed; current schema always owns history tables |
| Current projection/recipe/outbox versions and unsupported-header custody | Query/Catalog/Transcript, cross-harness replacement | provenance, immutable recipes, private staged custody | Kept-current: version/integrity/source/generation guards and opaque unsupported-recipe custody remain necessary |
| Catalog row-wise writer outside 2–64 rows / 1 MiB / unique-key batch eligibility | current Catalog publisher | current metadata | Kept-current; test selector renamed RowWise; fallback/rollback coverage retained |
| Upstream Claude/Codex/Copilot historical log formats | native adapters/parsers | supported upstream input | Kept-current; these are not old Semon contracts |

Focused Catalog, Query, Transcript, overview and retention readers qualify the
current database contract in their read paths. Outbox compaction and selected
publication recheck versions under their write locks. Focused reads do not scan
unrelated archived markers: marker consumers validate the selected record, while
store initialization qualifies markers before mutation.

Historical upgrade/parity fixtures are removed. Current cold-start, incremental,
restart, refusal/no-mutation and corruption/availability/custody checks are the
acceptance evidence. Required runtime and formatting checks run in CI only:
local thermal readings are unavailable, and host policy forbids local Cargo and
Node test/typecheck execution. CI success and independent final-diff acceptance
must be reported separately and bound to the final pushed SHA.
