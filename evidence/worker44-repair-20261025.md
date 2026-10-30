# Worker44 bounded repair — global48 / model6 / domain own3

Implementation complete as a bounded WIP. NOT #477 delivery acceptance. Exact-head CI and a fresh Manager verdict are required. No CI has been run or polled by this Worker.

## Revision and authorization

- Base: `9b76ac80576ee9f021d7d9c3219bb5ddfb5d8a9f`.
- Base parent: `d0c8ed6593b6251cc732d5d9ae8dbf327a672968`.
- Before editing: clean worktree, actual HEAD and full `ls-remote` SHA equal to base; no mismatch.
- Implementation head: `6c18b04cef44664e9736f34e4655c02a90a149cd`. The final delivery commit adds this handoff only; its exact SHA is recorded in the external ROOT/evidence copy and Worker result.
- Origin: `https://github.com/onsager-ai/semon.git`.
- Branch: `task/477-raw-sql-backends-20261024`.
- P0 checkpoint pushed: `ce2a46ad0f3b11026d0ecd208f1aa261d9dcac38`; P1 checkpoint pushed: `cb4f8126d86d4933bae815a93cbdce7d5e45cfc0`; claim-order refinement pushed: `099274ef`.
- Author and committer: Marvin Zhang <3393101+tikazyq@users.noreply.github.com>.

## Five failure causes and changes

F1/F2: The actual failure lines 2510/2590 call `semantic_batch`, BEFORE `add_primitive_endpoint` and its record. Worker43 added a native-frontier UNION that unconditionally used `catalog_native_dependencies.namespace`. The valid CURRENT primitive fixture has the authored singleton dependency profile. `frontier_page` now selects the exact validated support profile using the existing `support_sql`. Singleton dependency rows are read through their admitted `storage_event_tables` physical owner and qualification; composite rows use explicit namespace. No missing-column probe, empty-table assumption or unscoped fallback. Both prior regression tests are unchanged. This corrects the Manager hypothesis about endpoint record being the failure site.

F3/F4: `prepare_semantic_batch` accepted equal path sets in caller order; the Source journal preserves supplied order in its claims, while `Stage::record` compares claims in ordered `Batch.paths` order. Both multi-input fixtures pass paths in a different order, so record returns `Ok(Conflict)` BEFORE the selected fault hook. The boundary now verifies distinct/exact input and claim membership, then orders only the existing typed claim witness to the batch. The shared parser retains its original input order and every native claim value. No claim or selector is dropped. The helper requires the fault to have been consumed before asserting Err and full shared_state rollback. A new order/duplicate test covers this boundary.

F5: The actual call graph is `prepare_semantic_index_sources -> scan -> retire_scoped_sources -> ConfiguredIndex::remove_files`, which refuses explicitly deselected Raw sources before the typed withdrawal can be emitted. `ScanSelection::Semantic` now uses the SAME scan implementation while deferring generic missing-source removal to the existing typed withdrawal effects. Ordinary scans retain their retirement behavior. Identity/matching/publication use the same semantic custody policy. Negative authority remains exact current facts inventory + facts identity + Raw stat + runtime + admission/stream/epoch/claim fences. SQL absence never authorizes withdrawal. The domain43 custody assertions (retained catalog, byte-identical slots, trace header, historical search, foreign Source and zero-write rebind refusal) remain unchanged. No refusal string was removed.

## Fault-hit table

All counts below require CI; no locally observed count is claimed. `domain43::ready` emits its actual per-invocation `domain44_fault_hits` map. Every nonempty phase batch must consume its injected point exactly once (FAIL_EFFECT changes to None), return Err, and restore the whole database. There are no conditional skips on an unhit point.

| Phase/effect | Point | Hit-count status |
| --- | --- | --- |
| Indexing | semantic_record_owners | Unobserved; asserted on each indexing batch, hook moved after actual source/owner indexing writes inside the transaction |
| Identity | semantic_record | Unobserved; asserted on each identity batch |
| Relations | semantic_record_relations | Unobserved; asserted on each relation batch |
| Publication model record | semantic_model_record | Unobserved; asserted on each publication batch |
| Publish owners | semantic_publish_owners | Unobserved; consumed and whole-DB rollback asserted on each publish, actual parent/PID/lineage producers retained |
| Publish retirement | semantic_publish_retirement | Unobserved; consumed and whole-DB rollback asserted on each publish, explicit-negative withdrawal fixture retains real retirement coverage |

The existing publish helper still covers history, slots, rows, trace, search, proof, acknowledgement and head hooks. Empty reducer phases produce no nonempty batch to record (e.g. explicit withdrawal has no identity endpoints); no test was changed to accept Ok on a fault.

## D1/D2/D5/D6 classification

- **D1 disputed as an existing silent-overwrite defect; explicit refusal strengthened.** The old `Option<String>` comes from `query_row::<String>(...).optional()`: SQL NULL from the CASE is a String conversion error, not row absence. The old code therefore already refused oversized priors. It now uses `Option<Option<String>>` and explicitly refuses inner None, making row absence versus oversize visible. The new test seeds an actual parent relation with >1 MiB valid JSON, attempts the real relation record after endpoint effects, verifies Err and whole-DB rollback, and verifies the exact oversized bytes remain.
- **D2 fixed.** Before endpoint upsert, within the existing immediate transaction, read the bounded prior and require physical harness, serialized harness and serialized session key to match the incoming owner. No mismatch is overwritten. One test exercises physical harness, metadata harness and metadata key corruptions with exact row preservation and whole-DB rollback.
- **D5 fixed.** Missing root endpoint now refuses instead of manufacturing `root_alive=false`. Remove dead `let _ = cost` and the duplicate family_cost read from endpoint normalization. A real parent-child publication test seeds a nonexistent root, verifies record Err, full rollback and exact retained root text.
- **D6 fixed.** Internal typed replay state distinguishes Staged, Unpublished, Virtual and Missing. Unpublished requires actual allocated native identity and a positively indexed source still awaiting identity reduction; both Unpublished and Missing refuse replay, rather than silently omitting required owners. A virtual endpoint requires the existing typed relation stub declaration. Tests delete a requested owner and separately a relation target after completed identity, then verify refusal, whole-DB preservation and exact continued absence. No endpoint/selector is silently dropped.
- **D3/D4 deferred as instructed.** General path/name/PID and affected-owner retirement lifecycle was not started.

## Tests and verification

Five new tests are in separate `configured::tests::domain44`:

1. configured_semantic_domain44_input_order_preserves_exact_claim_witness
2. configured_semantic_domain44_oversized_relation_prior_refuses_and_rolls_back
3. configured_semantic_domain44_endpoint_identity_mismatch_refuses_and_rolls_back
4. configured_semantic_domain44_missing_root_refuses_and_rolls_back
5. configured_semantic_domain44_missing_replay_owner_refuses_without_writes

They use normal EventCache ingestion, actual Raw-derived model batches, actual mutation claims, and real identical Source-b paths/native identities populated before every negative gate. Corrupt stored rows are test injections, never hand-filled PreparedSources. Every refusal compares shared_state over all database tables, including foreign Source rows.

`git diff --check` passes. No local cargo build/check/test/clippy/fmt/update/lockgen, Node checks/formatters, browser/screenshots/image processing or corpus analysis ran. Formatting, compilation and functional results remain UNVERIFIED. Root must run exact-head configured 56 existing + 5 new, 32 SQLite, 3 journal and the 90-test storage group, preserving t1–t6/replacement/runtime/trace and unbound every-field oracles. Report Rust/Usage/Viewer results accurately. Exact 9b failure is not relabeled; historical 4aa/6841 greens do not qualify this revision.

## Reservations and remaining scope

No S2 proof_lifetime/proof_generations, Stage::publish proof hook/error or existing S2 tests were edited. No PG SourceObservation/source_observations/SQLiteSourceJournal/PG family, perf Viewer/causal_trace page/diagnostics, Cargo/Hub/UI/baselines/PG pool or other worktree writes. No edits to cold write_file/index_write/source_journal/native_timing/native_evidence. The shared model scan selection boundary is touched only for F5; its ordinary retirement semantics stay intact. Preserve `/home/marvin/projects/semon/out/`.

Public qualifications remain 0/None, catalog completeness partial, selected factories closed. Current-schema-only policy remains; no historical compatibility/upgrade ladder. #380 must COMPLETE before #477 MAIN merge. No main merge/deploy/publication/live PG/user migration/deletion/credential/remote/permission changes.

Still remaining: D3/D4 affected-owner lifecycle, incident discovery, cross-harness native keys, selectors/frontier, recovery intents, S2 integration and S3/S4, complete native PG, supported-current-store migration/catch-up/exclusive fenced cutover/qualified rollback, Hub313 RLS, fresh combined 36/38/40/43 review and final independent acceptance. Initial-catalog cold timing, union 30s and Viewer visual/transport binding remain open historical gates. Predecessor #380 completion is not claimed.

Thermal log `/home/marvin/.cache/thermal-watch/temps.log` remained absent; no local CPU-bound work. Check before/after any future CPU work and stop at >=85 C. All checks remain CI-only. Every git command disabled automatic gc/maintenance. No nested harness/controller/agent was launched.
