# Worker125 physical-key audit

Task: raw-pg-keys125-20261026. Base and initial remote:
883679787d25a56b37b3a8e2553ee8b64f0adda4.
Only pg477/semon, task/479-pg-foundation-20261025 was edited. Catalog124,
initializer123 and other worktrees stayed untouched. Current-schema-only
supersedes all historical schema importer obligations. This is an unfinished
scope125 increment, not full #477 acceptance or public backend qualification.

## Bounds grounded in current code

* source_observations.rs:291 permits nonempty paths through 4096 bytes; recovery
  cursors may also reach 4096 (line 243). Neither fits a full btree key safely.
* Source operation IDs are ASCII and at most 256 bytes (lines 206-209), with
  typed namespace/kind/digest checks. Foundation receipt/intent operation keys
  are bounded; operations uses checked 64-character IDs.
* events/native_rows.rs borrows full parser message and record identities;
  it imposes no 2704-byte physical-key bound. Do not infer a smaller domain from
  an index's accidental limit. Model/tool/native identity fields likewise need
  real long-input qualification before they can be considered safe.
* events/tree_rows.rs:80-99 emits only labels first/last/cwd/branch, plus the
  writer's fixed codex_turn label. Their values are variable but not indexed.
* native_pages.rs:143,151 generates page keys from numeric page/level indices.
  ProofGenerationId::derive supplies generation keys; page blobs and current
  paths already use bounded digest identities. These families were preserved.

## Full physical inventory (namespace digest prefixes omitted)

| Family / index | Existing logical key fields | Status |
| --- | --- | --- |
| files PK | file_id | Fixed integer, preserved |
| files_path unique | path | Generated SHA-256, ported |
| files_cwd | cwd,path | Generated SHA-256 pair, ported; no public order changes |
| meta PK | key | Generated SHA-256, ported |
| source_mutations PK | source_path | Unported variable key |
| relocation_candidates PK / relocation_new | old_path / new_path,old_path | Unported variable keys |
| events / signals / codex_usage PKs | integer owner/ordinal,boolean | Bounded, preserved |
| usage_identity PK / usage_peers | file_id,message_id,record_uuid / peer permutation | Unported variable identities |
| usage PK | file_id,message_id | Unported variable identity |
| native_dependencies PK / dependency_native | source_path,harness,native_id / reverse access order | Unported variable keys |
| usage_sources PK / usage_source_path | source_id / source_path,source_id | Integer PK safe; path index unported |
| usage_current PK / usage_current_source | source_path / source_id | Path unported; integer unique safe |
| usage_facts PK / usage_observations | source_id,native_offset / source_id,identity,native_offset | Fixed PK safe; identity index unported |
| usage_identities PK | source_id,scope,identity | Unported variable keys |
| usage_dimensions PK | source_id,kind,dimension | Unported variable dimension |
| tree_generations PK / tree_cleanup | projection_id / flags,projection_id | Bounded, preserved |
| tree_usage_work PK | projection_id,message_id | Unported variable identity |
| tree_codex_work PK | projection_id,identity | Unported variable identity |
| tree_headers PK | source_path | Unported variable path |
| tree_models PK | projection_id,model | Unported variable model |
| tree_labels PK | projection_id,label | Current producer emits finite labels |
| tree_tools PK / tree_open_tools | projection_id,tool_id / projection_id,closed,tool_id,name | Unported variable identities/names |
| tree_usage_models PK | projection_id,message_id | Unported variable identity |
| tree_copied_identities PK | file_id,message_id,record_uuid | Unported variable identities |
| tree_repairs PK / tree_repair_queue | source_path / state,last_attempt,source_path | Unported variable path; state bounds not independently proved |
| recovery PK | cursor | Unported, domain permits 4096 bytes |
| page_blobs / page_generations / page_refs / page_current | digest / derived generation / generation,kind,numeric key / path_digest | Existing bounded addressing preserved |
| reported_runs / reported_stamp | session_digest,start / singleton | Existing bounded addressing preserved |
| foundation operations | operation_id | Checked fixed 64 characters |
| foundation source_state | namespace_digest | Existing digest, preserved |
| foundation source_members / source_invalidations | source_path | Unported 4096-byte domain |
| foundation source_pending | revision,source_path | Unported path; observable order must stay unchanged |
| foundation source_receipts / source_intents | operation_id | Checked ASCII <=256 bytes |
| foundation source_intent_order | sequence | Fixed integer |

## Implemented closed plan

native_keys::generated is a closed files/meta mapping shared by installer and
warm verifier. Exact generated expressions, nullable flags, index keys and the
full-value comparison function are validated before effect callbacks. Metadata
puts guard full namespace AND key on conflicts; reads/deletes address a bounded
digest then reject full-value mismatch. All original bytes remain in heap.
Files use digest equality plus full-path collision checks for core CAS and
Tree writer path lookups; file_id remains the physical primary identity.
No Raw body, permission, receipt or public IndexStore was invented.

The private native fingerprint changes, so earlier physical contracts refuse
explicitly. No historical readers, upgrades, resets or compatibility tests were
added. The schema initializer and global version were not edited.

## Observable verification and limits

CI 40250829914 at 4047dbd55850bd8072525ae54bb4caeeb265d42f passed
the new real SQLite-producer long-path/core-field/metadata/restart/namespace/
collision-function focused test. This is not final-pin CI acceptance.
CI-generated formatting was applied only to task-owned files. Its artifact
also contained inherited union.rs formatting; that unrelated file was preserved.

Exact push CI 40251087196 at 077ab447d1162557d8fe11bf8a6fc3eec8de666c
passed the two pg_native_keys125 focused tests, including the resource and warm
corruption probes below. Full regression is still running; formatting and
strict Clippy failed, so this is not all-green CI. Its CI-generated task-only
format patch was applied subsequently. The PR run 40251090375 uses a distinct
synthetic merge revision and is not exact task-commit evidence.

This revision adds actual EXPLAIN ANALYZE selected metadata scan rows and
actual returned value byte lengths at 0/256/2048 unrelated metadata rows,
inserted through the real metadata writer. This does not measure all-family
hash/generation writes, dirty-tail cost, or prove full native resource bounds.
Warm corruption probes cover changed equality-function bodies and generated
expressions, refusing callbacks and verifying stored key/value preservation.

Known SQLite carrier removal is exercised, but complete process-wide SQLite
open/access rejection instrumentation remains missing. Forced wrong physical
stored-key collisions across all families, opaque long IDs, full ledger/proof/
Usage/Tree/launch parity, and each rollback/cancel boundary remain unqualified.

Tree's shared witness explicitly requires original UTF-8 byte order. Replacing
its cursor with SHA-256 order would alter the witness; retaining a full-key sort
would fail the bounded-read requirement. Neither workaround was introduced.

Thermal telemetry was unavailable before/after work. No local cargo command,
formatter, test, build, Node/typecheck, browser, SQL benchmark or corpus scan ran.
All checks use CI. Stop CPU work at >=85 C or unavailable telemetry. Reads and
edits stayed bounded to named files. Full #477/#478/#479/Hub313 remains open;
#380 completion remains mandatory before MAIN. No MAIN/deploy/publish/live-data
operation occurred. This ONE bounded Worker attempt does not reset counters.
