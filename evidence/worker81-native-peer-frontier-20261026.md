# Worker81 native peer frontier increment

Task raw-sql-477-frontier-20261026, GLOBAL81 / model18 / successor-own1. Implementation worker; one bounded attempt, no nested harness or retry. All 16 supplied message IDs are acknowledged in the final structured result. Current Semon schema only. Starting revision: 63a4757e1362950e9c17b0b2fb2acee4b53cc84e on origin/task/477-raw-sql-backends-20261024. Base Storage 40212694493 was green (32 SQLite, 3 journal, 83 configured, 90 catalog/storage, Query 3+3, facts 1+1, formatting and boundary); that evidence is historical prerequisite, not this increment's acceptance.

## Actual phase and dependency audit

The ordinary native_frontier recipe first checks the requested harness/native Current, then the five dependency harness prefixes (empty, lineage:, bridge:, semantic-parent:, semantic-native:), then nativeall and identity_hint. Filename hints queue claudeid and copilotid; identity_hint additionally queues nativeall and headerid. Codex adds qualified headerid/headerparent. These are all kinds the extracted recipe can queue: native, dependency, nativeall, claudeid, copilotid, headerid, headerparent. Event projection independently queues receipt/sender. The admitted adapter already handled the latter and pathowner.

Event projection runs before frontier_page; discovered paths are enrolled with source_journal::claims and returned for native preparation. Identity advances only after events and frontiers finish, no pending indexed paths remain, and native key checks pass. Ownership reduction and copied-event qualification remain after indexing, before Relations. No phase integration or qualification ordering was changed.

## Production contacts

- events/store/semantic_stage.rs:595 onward: NativeFrontierPort, shared filename_hint_with / identity_hint_with / native_frontier_with, OrdinaryNativeFrontier physical SQL. Original ordinary SQL/decision order is retained, including header dependency exclusion. Existing native_keys_page calls the same shared identity recipe through its ordinary wrapper.
- storage/sqlite/semantic.rs:498 onward: ScopedProjection::native now invokes the shared recipe with harness claude; ScopedNativeFrontier implements namespaced physical checks/queueing. Every observed table, stage NOT EXISTS side and join namespace is guarded; singleton catalog_native_dependencies/meta use authored custody through support_sql / metadata_sql and storage_event_tables qualification=0. No arbitrary-SQL port exists.
- semantic.rs event_page:617 onward: removed only blanket peer_sock refusal. Unsupported event harness/non-top Claude refusal remains.
- semantic.rs frontier_page:647 onward: added claudeid/copilotid keyset pages, and dependency singleton/composite custody. Existing claims, frontier done cursor, 64-observation page budget, source/owner/1MiB bounds and transaction fences remain. No model.rs, proof/S2, PG, source journal, perf, UI, Cargo or writer inventory changes. Tests stay in the existing registered configured.rs file; public qualification remains 0.

## Real producer tests and assertion changes

Existing domain72 test is renamed to unsupported_name_refuses; its ClaudeName typed refusal and whole-DB equality are unchanged. Only the obsolete peer-refusal portion is replaced by supported-peer tests.

Five domain81 tests use real Raw files, native ingest/parser, stage/model recording and publication:

1. Known peer discovered from a one-path seed through filename authority; absent peer reaches publication only with no unfinished authorized frontier. Compares event rows against the ordinary canonical driver and complete prepared rows/relations/slot recipes against the existing ordinary oracle. Publication fault consumption, rollback and idempotent ACK assertions run through the unchanged publish helper.
2. Stale epoch holder: semantic batch refusal and publication Conflict with complete shared-DB equality.
3. Identical physical paths/native/message IDs in a populated foreign namespace: foreign Current/dependencies cannot seed local frontier tasks; all foreign table rows remain identical. Repeats through actually published Current/dependency authority.
4. 65 filename candidates from real ingested files: persisted unfinished claudeid cursor, real component source budget refusal, whole failed-TX equality and zero Current publication. Ordinary producer must independently exhibit an unfinished filename cursor. No invented exact observation count or raised limit.
5. Discovered target Raw changes after model preparation: publication Conflict and complete shared-DB equality, including History/proofs/ACK custody.

The existing 83 configured cases and publication oracles are retained apart from replacing the now-obsolete peer rejection assertion with stronger successful discovery/refusal/custody assertions. No fake PreparedSources, handfilled endpoint/catalog membership, masks or normalized comparison was added.

## Revision and CI ledger

- 60fa77fc28596e1b6a993901f80c9c613a8e8ae7: early WIP pushed within 5–7 minutes, shared recipe and initial tests.
- 0108bf294e931c2fe1d6714ec54bd617b47814ea: namespace/paging tests. Storage 40214630755 checkout 0108bf2 passed 32 SQLite + 3 journal, then was automatically cancelled for successor pushes before configured completion; not accepted.
- c12c0094edcaf2d09412de0dd9e40e996057522d: Raw claim test and direct EXISTS filename lookup, plus exact 0108 formatting artifact 12607067720 (34,252-byte uncompressed patch). Storage 40214947086 is superseded; no green claim.
- 4ac6a8f1153dae5fe9f4d4300da23e73b3fee02f: test formatting from own exact c12c artifact 12606409955 (3,383-byte uncompressed patch). Its remaining semantic.rs formatting hunk is included in this handoff commit. No local formatter ran.
- Final checkout/remote SHA, Storage run/job IDs, executed counts and actual result are reported in the final structured result. CI is pending when this note is committed; this note alone makes no passing claim. All checks are CI-only; thermal telemetry was absent before work. No local cargo/build/check/test/clippy/fmt/update/lockgen, Node/typecheck, browser/image or corpus work ran.

Named review diff: evidence/worker81-native-peer-frontier.diff.patch (production and tests versus starting revision, written directly to file; excludes this note and itself). Normal commit/push only, author/committer Marvin Zhang <3393101+tikazyq@users.noreply.github.com>. Full remote verification is in the final result. No main merge, publication, deployment or real user migration.

## Remaining mandatory scope

Fresh Manager review of this revision is required. This is not full producer/backend/#477 acceptance. Native Name/members, sidechains, cross-harness key reservation and Factsless authority remain pending. Full S1–S4, complete PostgreSQL/no hidden SQLite, supported-current-store bounded migration/catch-up/exclusive fenced cutover/qualified rollback, Hub #313 and full performance/UI gates remain required. Existing cold first-page >2s failures are not waived. #380 main approval/completion is still pending; #477 main remains held until #380 is complete. No historical Semon compatibility was introduced.

## Actual final CI adjudication — BLOCKED, not accepted

Storage 40215221822, actual checkout 939c5abf5775e32126e1a25c82f2b146f5d328bc (checkout log short 939c5ab), completed FAILURE. Formatting job 121234979034 PASS; boundary job 121234979236 PASS; conformance job 121234979124 compiled and executed 32 SQLite PASS, 3 journal PASS, then 88 configured: 86 PASS / 2 FAIL. The later 90 catalog/storage, Query and facts groups were NOT reached. All prior 83 configured cases pass; three new domain81 cases pass (known/absent peer oracle, stale epoch, changed Raw claim).

Two actual failures remain unchanged for fresh Manager adjudication:

- domain81_peer_foreign_collision_current_and_dependency_custody, configured.rs:8405: the added assertion that no native/dependency/nativeall task exists before local publication fails. Source audit after CI establishes an invalid premise in that assertion: ordinary native ingest writes namespaced lineage:claude dependencies from index.links.session_ids/continued_in before Catalog publication (events/store.rs:6811–6827). Local dependencies therefore legitimately exist during this fixture; task existence alone cannot prove foreign leakage. Do not merely delete the assertion or normalize observations. Build a fixture/oracle that distinguishes actual locally authorized dependencies from foreign-only authority. Later foreign whole-state and repeat-Current assertions in this particular case were not reached; no namespace acceptance is claimed.
- domain81_filename_peer_pages_before_source_budget_refusal, configured.rs:8475: prepare_semantic_model_batch returned "semantic model stage changed" during the newly discovered candidate batch. events.rs:1990–2010 shows this is the wrapper's conversion of record_semantic_batch returning a non-Written outcome, not proof of the intended semantic_component_source_budget guard. The exact deeper conflict remains unproven. Existing expected capacity error, whole-DB rollback, zero Current and ordinary unfinished-cursor assertions are retained, not weakened. Further bounded diagnostics are required before a repair is chosen.

No speculative production correction or oracle relaxation was made after these results. The attempt stops BLOCKED at its time bound, with code and all tests preserved and pushed. The final commit after 939c5ab is documentation-only to record this failure accurately; its full remote SHA is in the structured result. Fresh Manager decision is required; no automatic additional Worker/retry. The named code diff still exactly represents the production/test changes against base. Final CI log was read only as one 150,793-byte named file with bounded checkout/test/panic summaries; no environment, tokens, signed URLs or full log output was exposed. Thermal telemetry remains absent and no local CPU checks ran.
