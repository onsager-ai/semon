# Cold Index transaction statement reuse

Worker39 / cold component attempt3, base
`114434c9b986d407ef7e488f11888b92b0ac5bc1`.
This is an implementation increment pending CI, not a passing cold-performance
gate or completion of #477, #478, #479 or Hub #313.

## Production change

`events/store.rs` keeps the two copied-usage DELETE statements in a private
`UsageDeletes` value. The ordinary writer owns this value for its write call;
`commit_initial_files` owns one value across its existing maximum 64-file
transaction. Its statements borrow that transaction and are explicitly dropped
before commit. There is no connection/pool/global or cross-transaction cache.
The INSERT/UPDATE statements, SQL text, parameters, per-message iteration order,
trigger order, claim/revision handling and publication guards are unchanged.

Every original `copied_usage_sql` qualification still executes, including on
reuse. A changed selected SQL family refuses rather than using the old prepared
statement. `configured::sql` and `metadata_sql` are untouched. Shape-per-transaction
did **not** land: there is no new evidence establishing its cost and no reason
to weaken the complete current-layout checks. Singleton, composite, transitional
and malformed-current-store tests remain. Historical Semon upgrade compatibility
is not added.

## Exact authored counts

Let N be changed usage message IDs across one initial-index transaction and M
be those absent from the new usage map. Previously, the two DELETE sites did
N+M explicit preparations and N+M executions. They now do
`(N>0 ? 1 : 0) + (M>0 ? 1 : 0)` explicit preparations and the same N+M executions.
Deletion qualifications remain N+M. These are source-derived predictions and
authored CI assertions, not observed CI results or SQLite internal reprepare
counts. For 64 cold files with one new usage message each: 64 -> 1 deletion
prepares, 64 executions unchanged. If N=0, no delete is prepared or executed;
the two pre-existing put-statement preparations/qualifications remain.

`index_rows` now includes fixed-label nested observations:

- `usage_identity_delete_qualification`, `usage_delete_qualification`:
  elapsed microseconds and qualification calls;
- `usage_identity_delete_prepare`, `usage_delete_prepare`: elapsed microseconds
  and successful explicit prepares;
- `usage_identity_delete_execute`, `usage_delete_execute`: elapsed microseconds
  and successful executions, including zero-row deletes;
- `usage_delete_reference_prepares`: the old authored count N+M;
- `usage_delete_authored_prepares`: the new explicit count.

These overlap the inclusive `usage` leaf. Failed invocations can contain partial
timing records; returned errors still roll back in the owning transaction.
No paths, native IDs, body bytes, claims or namespace values are logged. Existing
writer_shape and eight leaf written-row observations remain available.

## Tests and limitations

Three new tests cover both current physical families with foreign keys enabled:

- `usage_delete_reuse_matches_rowwise_order_and_preserves_foreign_rows`: full
  usage/identity/Codex values and trigger-event order match the frozen rowwise
  reference; a second file and foreign namespace survive; empty changes write
  zero rows and prepare no deletes. Four changed IDs/three removals assert
  7 -> 2 prepares and unchanged 7 executions.
- `usage_delete_statements_are_transaction_owned_and_recheck_on_reuse`: repeated
  ABORT/savepoint rollback, malformed DDL and DDL rollback with live statements,
  statement reuse across alternating file IDs, 64 calls/512 executions with
  two explicit preparations, and fresh preparations after outer rollback.
- `usage_delete_reuse_rolls_back_and_requalifies_ddl_and_aba`: complete rollback
  after a late trigger failure, zero writes for malformed index shape,
  DDL rollback/committed-cookie ABA refusal, and repaired-family admission.

All existing tests are preserved, including stale initial-batch admission,
configured mixed-layout/namespace guards and semantic/publication suites.
Tests, formatting and compilation have not run locally: thermal readings are
absent and the task requires CI only. Gateway dispatches and reviews CI.

No quantitative latency saving is established. The old 8c 3.604s observation
and PG run40175549601's 2.445s observation use different contexts/runners/features;
they are not a controlled before/after. The old usage leaf's 140,790us includes
qualification, writes and triggers and is not an estimate of prepare cost. The
new diagnostic must determine how much of this leaf is preparation and whether
this increment is sufficient. No claim is made that it closes the observed
0.445s distance to the 2s gate. If remaining cost lies in qualification or other
leaves, further optimization remains unimplemented, subject to task control.

## CI handover

An early Rust CI step runs the **unchanged** cold test with timing enabled and
`--nocapture`. It is explicitly diagnostic and continues on failure so existing
gates still run. Existing TIMING=0 isolated tests and TIMING=1 default-parallel
full suite are unchanged. Qualification requires the normal 0/256/2048 first
page <=2s and no original 60s timeout. An isolated pass is not qualification.

Gateway commands, after verifying the pushed task SHA:

```sh
gh workflow run rust.yml --repo onsager-ai/semon --ref task/477-perf-warm-20261025
gh workflow run storage-contract.yml --repo onsager-ai/semon --ref task/477-perf-warm-20261025
```

Focused runner command (not a replacement for the full suite):

```sh
cargo test -p semon-sessions --locked usage_delete_ -- --nocapture
```

Root owns dispatch/polling/artifact adoption and ordinary integration. No CI was
dispatched by this Worker. Fresh independent acceptance and all applicable CI
remain pending. Parent Raw/PG failures and full migration/Hub integration stay
parent-owned. #380 must complete before #477 main merge; no merge is authorized.

Thermal log absent. No local CPU cargo/build/check/test/clippy/fmt/update/lockgen,
Node checks/formatters, browser/screenshots/images or corpus analysis. CI only.
Check before/after CPU work; stop at >=85 C and do not start without readings.
One bounded named file under10MB per content read/search. No nested agents,
harnesses or controllers; no live migration/deletion/deploy/publication,
credential/remote/permission changes or force push.
