# Worker73 — six-failure repair (in progress)

SOL_USAGE_UNKNOWN_BOUNDED_EXCEPTION: ONE Manager-approved 1200 s attempt,
global73/model16/lifecycle-own8. No retry or counter reset. ACK all twelve
intake message IDs, particularly post72-six-fail-repair-009.

Base b256333752659205d7d92a5dd5af290e89eec2bb was clean. Read named base JSON,
Worker72 report and bounded diff hunks before changes. Repository AGENTS,
semon-evolution skill and source-reader/retained-projection design read.
Thermal log absent: no local cargo/build/check/test/clippy/fmt/update/lockgen,
Node/typecheck/browser/image/corpus work. CI only; own-SHA CI delegation applies
for this dispatch. No nested agents/harnesses. Current Semon schema only.

## Step 0 source diagnosis (base line numbers)

claude_ingest configured.rs:7851-7875 follows selector_ready: admitted ingest,
ordinary scan_file. claude_ready:7877-7943 follows begin -> semantic_batch ->
shared model prepare -> record, excluding ordinary Publication record. Empty
paths are skipped. Its phases/epochs come directly from batch. Unlike
model_ready's publication fault hooks, no fault point is intentionally enabled.

Admitted record semantic.rs:1895-2155 has these Conflict conditions:
- :1926-1933 stage epoch/phase/runtime/generation/resetting, handle epoch,
  prepared/base/head/admission generation mismatch;
- :1935-1937 exact ordered journal claims mismatch;
- Publication :1940-1987 stale lifecycle/ledger witness, duplicate/empty row,
  missing staged endpoint, exact identity mismatch, covered-path mismatch;
- non-indexing :2009-2022 ledger witness or per-path phase pending mismatch;
- Identity :2053-2086 native-ID set, source membership, observed CatalogSource,
  canonical native key, complete source coverage;
- Relations :2113-2136 missing owned endpoint or relation endpoint coverage;
- Indexing :2157-2199 source count/duplicate/path, missing ledger, source/stat,
  native role, captured claim, withdrawal membership/runtime/current proof.
All budget/refusal errors are separate from Conflict. Publication retirement
and provenance checks remain. No cursor is supplied to admitted record;
ordinary batch_rejection semantic_stage.rs:1377-1438 independently checks missing
stage/reset/epoch/phase/revision/generation/runtime/head and witness rejection.
Ordinary record :2409-2498 additionally checks prepared generation/claims, row
roles and withdrawal authority/membership. No guard has been removed.

Batch semantic.rs:1800-1830 projects/discovers first, hands out Identity paths,
then (when those are exhausted) reduce_with followed by qualify_with. It stays
Identity until qualification completes. Shared ordinary qualification remains
post-reduce. At this point the five concrete guards are not yet established;
the diagnostic WIP prints only source line and phase, never native data.

F6 source evidence: ordinary frontier_page semantic_stage.rs:769-930 shares ONE
64-observation budget across tasks; :879-929 increments for every returned row
(including already-enrolled sources) and charges at least one for empty tasks.
It persists after_key and done, not an invariant of 64 unique newly enrolled
sources. Admitted semantic.rs:566-646 follows that budget schedule. Thus the
old `enrolled == 64` oracle was invented. The replacement requires >1 and <=64
enrolled sources plus a nonempty unfinished receipt keyset cursor, AND runs the
ordinary real producer to prove the same observable. Exact budget error,
failed-transaction whole-DB equality and zero published Catalog remain.

## Delivery/checks

Diagnostic WIP b867bb96e4435470cbcb4eb32ef6c100310865eb pushed normally.
Exact-head Storage 40209326196 / conformance 121216983086 pending;
boundary passed, formatting expected failed for temporary diagnostics.
Accidental redundant manual base/head runs 40209302513/40209323966 cancelled;
PR-head run 40209326196 has explicit head checkout in workflow.

## Remaining scope

Native peer/incidents, Name/members, sidechain, cross-harness keys, Factsless
authority, recovery, S3/S4, full PG/no-SQLite, current-store migration catch-up,
fenced cutover/qualified rollback, Hub #313, cold timing (OPEN first page
3.480 s > 2 s), UI and #380 remain. #380 main approval pending; #477 main held.
No full #477, selector or producer acceptance is claimed.
