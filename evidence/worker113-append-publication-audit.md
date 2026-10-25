# Source113 append publication

Base: 9c954b54dfa9653471c37e3019a571191d27726f. One approved bounded
Source113/model26/controller-own1 Worker increment; fresh Manager review required.

The ninth domain94 test reused the single-attempt publication fault helper for
a three-owner replacement. The shared `preserve_replacement_rows` deliberately
advances at most one original owner per attempt. `Stage::publish` commits only
private custody progress and returns `(Deferred, None)` until that completes,
before the owner fault is reached. The helper's first assertion requires an
error, so the old panic label did not establish a production owner error.
Diagnostic CI 40240264153 reproduced 8 passed / 1 failed without a relation-field
conflict. No production relation comparison or publication fence was relaxed.

The appended case now drives only that explicit Deferred result in a bounded
64-attempt loop. Each checkpoint must leave the old Current metadata and recipe
unchanged. The loop terminates only when the named owner fault is consumed and
the expected injected error is returned; stale/refused/unavailable errors with
an unconsumed fault fail. It then runs the unchanged full-database rollback,
publication and idempotent ACK helper. After finish/reopen the complete ordered
ordinary oracle runs again. Retained original metadata, recipe headers and
ordered slots are compared to their L1 snapshots after L2 publication/restart.
The virtual peer and all eight prior cases and their assertions are retained.

Only the test/admission path and a bounded unexpected-outcome diagnostic change.
Shared recipe V5, parser, source authority, old history/proofs, Facts, transaction
fences, namespaces and resource limits remain intact. FIRSTPAGE remains 8s only;
foreground 2s/background 60s and the unrelated 30s timeout are unchanged.

Exact revisions, CI states and remote verification are recorded externally in
evidence/worker113-append-publication-delivery.json. Pending checks are not passes.
Full #477/#478/#479/PG/no-SQLite/migration/catch-up/fenced cutover/qualified
rollback/Hub313 remain outstanding, public factory qualification remains zero,
and #380 must complete before any #477 MAIN merge. No merge is authorized here.

Thermal telemetry is unavailable. No local cargo/build/check/test/clippy/fmt,
lock generation, node/typecheck, browser/screenshots/image, corpus analysis or
SQL benchmark ran. CI only; stop CPU work at >=85 C or absent readings. Reads
are one named file under 10MB, <=150 lines/4000 tokens; CI output is allowlisted
before cleanup and bounded to <=500 characters per line. No nested harness or
agents, other-worktree edits, credentials/permissions/remotes changes, live data
operations, deployment or publication occurred.
