# Worker128 remaining-key increment (not scope completion)

Task raw-pg-keys125-20261026, Worker128 / PG45 / native-core component7 /
controller own2. This bounded continuation does not reset counters or authorize
an automatic retry. The controlling amendment is current-schema-only-20261026.
Base fb14ad343e3ba1d9ed0d2259cef26732bd1b2ee0 and origin task branch were equal
and clean at intake. Other worktrees were not edited. No nested agent ran.

Implemented: domain-separated SHA-256 physical addresses over fixed-width
namespace/domain hashes and complete original bytes for usage_current,
usage_sources path index, usage_facts identity index, usage_identities,
usage_dimensions, and native_dependencies. Heap witnesses remain complete.
The fresh current contract is version 7, with no old reader or upgrade.
The closed generated-column plan and warm verifier bind exact function bodies,
expression/column/index manifests, RLS, namespace and schema before callbacks.

Usage current reads qualify both current and source heap paths. Dimension and
current-pointer conflicts check complete namespace and logical key witnesses.
Dependency replacement/deletion qualifies full path; conflict writes qualify
path, harness and native ID. Unique insert conflicts refuse rather than being
interpreted as identity equality. Integer-key Usage reads/updates and max+1
allocation now address the namespace-digest prefix. Tree writer/checkpoint
current-Usage joins use the same digest plus full heap witness.

Current Usage receipt reads query stored byte length first, refuse over 1 MiB,
charge both SQL rows and receipt bytes plus integer metadata before decoding,
and retain the caller's cancellation/deadline. Other variable payload read
budgets and complete resource qualification are still unfinished.

Authored CI-only tests, registered under pg_native_keys128 in postgres.yml and
the writer inventory: real SQLite current producer first, then real PG Usage
and dependency effects; high-entropy 3584-byte identities/model/path inputs,
NUL/non-BMP, same keys in foreign namespaces, reopen, forced wrong heap witnesses
at actual physical addresses, rollback preservation, and selected byte-budget
refusal. A real EXPLAIN ANALYZE test uses the production CURRENT_SQL at
0/256/2048 unrelated Usage generations and measures visited SQL rows and actual
receipt bytes. These are authored tests, NOT claimed passing qualifications.
There is no process-wide SQLite access-rejection instrumentation in this work.

Observable CI:
- 93eee00a530cd9be549c3c68dd7919ba1cff74e4 push run 40253581030:
  existing keys125 step PASS; whole job still running at observation.
- 24f66732b0102aad6c838b9ae1ca1a3ef9fc60a6 push run 40253847131:
  Catalog and keys125 FAIL at fixture initialization with ScopeChanged;
  keys128 was skipped by the preceding failing command. The version constant
  had become 7 while initial DDL still stamped 6. This was our error.
- cde49623b117cdb5e623afbf1443021f0bfd0694 push run 40254191089:
  includes selected-read/wrong-witness tests; same stamp mismatch still present.
- a64f486bd5592bbefdfd3239a415229cb7710d03 push run 40254301171:
  fixes fresh stamp to 7; queued at last observation, no pass claim.
CI formatting patches from exact 93eee00a, 24f66732 and cde49623 artifacts were
applied only to task-owned paths. No local formatter or test ran. Historical
125 Clippy diagnostics identified reported_facts.rs cloned-ref assertions;
the coherent intake base already differs there. No lint was suppressed or
unrelated Facts code changed. Final exact CI and independent review are pending.

Remaining blocking scope: core usage/usage_identity and copied identities;
Tree maps/tools/cursors/headers/repairs and coherent shared witness traversal;
source_mutations, relocation_candidates, recovery; foundation SourceMembers,
invalidations and ordered pending paths. The Tree witness was not changed.
All-family SQL rows/bytes/hash/generation-write/dirty-tail measurements,
process-wide no-SQLite instrumentation, full proof/launch/ledger parity and
all rollback/stale-authority/cancel/pool-retention boundaries remain required.
No all-history sort, weakened oracle or changed security/timing bound was added.

No public PG readiness, full IndexStore or ingest/Raw receipt is asserted.
Native126 and Catalog127 remain separate owners. GC closed pages, retirement
refusal, Raw, parser, SourceRoot and UI are untouched. Full #477/#478/#479/Hub313
migration/catch-up/fenced cutover/rollback/lease/public wiring remains open;
#380 must finish before MAIN. No merge, deploy, publication or live data action.

Thermal telemetry /home/marvin/.cache/thermal-watch/temps.log remained absent.
No local cargo/build/check/test/clippy/fmt/update/lockgen, Node/typecheck,
browser/image, corpus or SQL benchmark ran. CI only; stop CPU work at 85 C or
unavailable readings. Resume with one named file <10 MB, <=150 lines/~4000
 tokens per read; no recursive content scans or raw logs/credentials. This is
partial pushed WIP, not completion. See delivery JSON for the final remote pin.
