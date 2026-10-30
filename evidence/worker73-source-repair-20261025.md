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

## Established diagnosis and repair

Diagnostic exact-head CI 40209326196/job121216983086 executed 32 SQLite +
3 journal PASS and 83 configured (77 PASS, 6 FAIL). Actual checkout log ref and
post-checkout SHA both b867bb96e4435470cbcb4eb32ef6c100310865eb. F1-F5 EACH
reported `semantic_record_conflict line=2169 phase=Relations` in that WIP:
base :2130-2139 relation endpoint coverage, NOT epoch/claim/revision/qualification.
F6 reported committed enrollment 1 -> 59 -> 65, then the budget refusal. This
preserves the historical failures without relabelling them green.

F1-F5 classification: producer contract defect. Shared Context::capture
model/relation_stage.rs:845-861 explicitly produces Relation.stubs for native
unmatched calls/receipts. Ordinary semantic_stage.rs:2702-2712 accepts relations
anchored to a scope owner without requiring every trace endpoint to own Raw.
Existing admitted Owners::replay semantic_owners.rs:294-305 likewise treats
ONLY explicit relation.stubs as virtual endpoints; :187-188 records they carry
no Raw membership. The admitted record guard incorrectly required both ends
in the real endpoint map, making this valid producer output unrecordable.

Repair semantic.rs:2126-2143 now requires:
- at least one real prepared endpoint (already checked for namespaced staged
  ownership earlier in the same transaction);
- from and every present to are either real endpoints or an explicit typed
  stub from the shared native model.
There is no fabricated owner/source row and no arbitrary missing endpoint
exception. Native model, qualification, claim/epoch/ledger/publication guards
and source/owner/byte budgets are unchanged. Genuine stale tests remain
unchanged. A test-only static `relation_endpoint_evidence` diagnostic remains;
claude_ready's Written assertion now also prints phase/admission/rejection
reason without native content. All temporary per-line diagnostics were removed.

F6 classification: invalid paging oracle; fixture now verifies ordinary
producer behavior in the SAME test through scan_file -> begin(one real path)
-> batch -> actual model/record, then checks a persisted unfinished receipt
cursor and >1/<=64 enrolled sources. Admitted path requires the same observable.
BEFORE: `paged |= enrolled == 64`, then `assert!(paged)`.
AFTER: `paged |= enrolled > 1 && enrolled <= 64 && pending_receipt`, then the
same mandatory paged assertion with a precise message. New ordinary_paged
assertion additionally qualifies that observable. No budget error, full
failed-TX equality, prior positive/foreign/epoch/history/proof assertion, or
zero published Catalog check changed. Test count stays 83.

Hunk contacts: only semantic.rs Relations record coverage and configured.rs
F6/claude_ready diagnostics; no edits to ordinary recipes, qualification phase,
SQL joins, model/relation_stage.rs, PG, S2, history, proof, perf or UI. No DDL.

Implementation revision aca9a668d43716fc491da95463b37b9bb1e176a0, parent
b867bb96e4435470cbcb4eb32ef6c100310865eb, normally pushed to origin
https://github.com/onsager-ai/semon.git task/477-raw-sql-backends-20261024.
Storage 40209806484 conformance121218417883 pending as of this report;
boundary121218417786 PASS, format121218417640 FAIL. Exact own-SHA artifact
12605245657, metadata size1126 bytes (<2 MiB), downloaded and applied only its
configured.rs formatting hunks; no local formatter ran. Final delivery JSON
records the actual final remote SHA and latest CI status separately.

## Final bounded-attempt result — BLOCKED / WIP

Exact aca9a668 CI 40209806484/job121218417883 completed: 32 SQLite + 3
journal PASS; configured 78 PASS / 5 FAIL (83 executed); the 90/Query/facts
steps were NOT REACHED. F6 now PASS, including the added ordinary paging
oracle. Prior 77 remain PASS. F1-F5 no longer fail at record; none emitted
relation_endpoint_evidence. The five still fail at NEW downstream assertions:

- domain67 real call, domain72 receipt/call and unsupported-name/native-peer
  tests fail the UNCHANGED publication fault oracle configured.rs:6481:
  `unconsumed fault semantic_publish_owners`. Publication refuses before the
  injection; the hook was deliberately not cleared or skipped to obtain green.
- domain72 foreign isolation directly receives
  `semantic_publication_projection_owner` from publish. Publication preflight
  semantic.rs:806-834 requires every transcript key to have a real Catalog row
  and equal projection/row key sets. Shared native stubs have trace/transcript
  projections without real Raw owner rows. A correct repair needs actual shared
  projection/stub custody audit; accepting arbitrary missing owners or dropping
  projections would weaken custody. This region is outside the named Worker72
  record/event/frontier extraction hunk, so it was left unchanged.
- domain72 copied-event test passes counts (132,132,66), full projected-event
  parity and count=1, then FAILS the unchanged complete selector value oracle.
  A bounded parser of the one sanitized assertion identified ONLY these
  differences: /endpoint/row/native_ids lengths admitted2/ordinary1 and
  /endpoint/row/sources lengths admitted2/ordinary1. Location and every other
  endpoint field agree. Ordinary SelectorLookup::endpoint_row
  semantic_stage.rs:3030-3035 reads stored metadata directly; ordinary relation
  recording :2690-2698 overwrites endpoint metadata from each prepared batch.
  The cause of incomplete ordinary metadata needs audit of its paged matching
  context. No selector value field was stripped or assertion relaxed.

This is NOT a completed six-failure repair. No new frontier, public qualifier,
source budget, assertion suppression or automatic attempt was introduced.
WIP retains one proven fixture repair plus the minimal real/stub record guard
and phase diagnostics. The next Manager must explicitly adjudicate publication
stub projection custody and ordinary paged endpoint completeness, reserve the
needed hunks, and inspect fresh final-SHA CI. Current final formatting-only
successor CI may still be pending; no acceptance is inferred from source parity.

Full code diff: evidence/worker73-source-repair.diff.patch (against b256, source
files only). Final commit is labelled WIP; actual full remote SHA/parent and
current CI run/job state are supplied in the Worker JSON result. Thermal
telemetry was checked again and remains absent; no local CPU validation ran.
