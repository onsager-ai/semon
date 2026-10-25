# Worker44 bounded repair (global48 / model6 / domain own3)

Base: 9b76ac80576ee9f021d7d9c3219bb5ddfb5d8a9f. Base parent: d0c8ed6593b6251cc732d5d9ae8dbf327a672968. Clean worktree and full origin branch SHA equality verified before editing.

P0 checkpoint: implementation is pending exact-head CI, not acceptance.

- F1/F2: actual failure lines are semantic_batch discovery, BEFORE add_primitive_endpoint. Worker43's native frontier UNION used namespace on singleton catalog_native_dependencies. Select the explicitly validated authored support profile; singleton rows are read through storage_event_tables physical ownership, composite rows by namespace. No prior test changes.
- F3/F4: input set equality accepted caller order, but claims preserved that order while Stage::record compared against ordered Batch.paths. Both two-source fixtures passed parent/child or old/new input order differing from SQL lexical order. Prepare the SAME model using batch path order; preserve all claims. Fault assertions now require the selected point to have been consumed (None) and Err, with phase/result diagnostics. Index owner hook moved after actual indexing effects within the transaction.
- F5: scan's generic retire_scoped_sources called unported ConfiguredIndex::remove_files for the explicitly deselected Raw source. Semantic scans now use the same parser with retirement deferred to typed withdrawal publication. Ordinary scan retirement remains intact. Withdrawals still require current explicit facts authority, Raw stat, runtime and epoch/claim witnesses; SQL absence does not authorize retirement.

Fault-hit table: Indexing semantic_record_owners, Identity semantic_record, Relations semantic_record_relations, Publication semantic_model_record; each executed nonempty batch must consume its point and roll back shared_state. Publication additionally consumes each existing point, including semantic_publish_owners and semantic_publish_retirement. Actual hit counts are NOT observed locally: CI has not been run. No zero-effect skips were introduced.

P1 D1/D2/D5/D6 analysis and tests pending at this checkpoint. D3/D4 lifecycle is not started.

Thermal log absent; no CPU-bound local checks, build/test/fmt/typecheck/browser/image work. CI belongs to Root; none run or polled. No reserved S2/PG/perf/cold regions edited; no other worktree writes. No main merge/deploy/publication/live PG/migration/credentials/remotes/permissions changes. Public qualifications remain closed; #380 holds #477 main merge.

Remaining: D3/D4 affected-owner lifecycle; incidents; cross-harness keys; selectors/frontier; recovery intents; S2 integration and S3/S4; complete PG; migration/catch-up/fenced cutover/rollback; Hub313 RLS; combined 36/38/40/43 review; all exact-head CI and fresh Manager verdict. Historical timing/union/Viewer visual-binding failures remain open.
