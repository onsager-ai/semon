# Worker77 — publication custody and ordinary reference driver

SOL_USAGE_UNKNOWN_BOUNDED_EXCEPTION and LADDER_SKIP_JUSTIFIED are the Manager's
explicit single-attempt decisions, global77/model17/successor-own1. No retry or
counter reset. ACK every intake message ID, especially
post73-publication-ordinary-driver-010 and successor-after-prompt-bound-011.

Base: 30b54c69451aed9152fdc4cc9aadbbbd386a625f, clean at intake. Existing origin:
https://github.com/onsager-ai/semon.git; authorized branch:
task/477-raw-sql-backends-20261024. No other worktree edited.

## Step 0 audit and failure classification

Read the named base/driver evidence, Worker73 handoff, actual ordinary driver,
record/publish/replay/model/stub paths, repository AGENTS, semon-evolution skill,
source-reader and retained-native-projection designs, and all four issue specs.

The copied-endpoint failure is a fixture-driver defect. Actual production
native_evidence.rs:613-616 includes parent_paths AND companions in Relations;
:641-662 invokes ordinary index/identity/staged preparation. The old claude_ready
used only batch.paths and the admitted strict-membership wrapper on both sides.
The new ordinary_claude_ready uses those existing ordinary entrypoints and source
extensions. The two ordinary selector references use it; admitted preparation,
selector-value comparison and every existing oracle assertion are unchanged.
No normalization of native_ids, sources, metadata or relationships occurred.

The four publication failures share a producer custody mismatch. Shared model
model.rs:5423-5432 and :5557-5577 creates virtual stub transcripts, and
model/relation_stage.rs:845-861 captures explicit typed stubs on relations.
summary::catalog (model/summary.rs:212-217) excludes Stub sessions entirely.
Ordinary storage/catalog.rs:350-364 skips a transcript key absent from the real
Catalog rows. Thus stubs remain in the model witness but NEVER get Catalog,
Raw-source membership or persisted slot rows. Ordinary trace publication carries
the corresponding relation edges inside real owners' traces. Owners::replay
(semantic_owners.rs:294-312) independently recognizes explicit virtual stubs.

Admitted publish previously required all transcript keys to equal real Catalog
keys before opening the transaction. This rejected legitimate stub projections
before semantic_publish_owners fault injection (three tests) and directly refused
the foreign-isolation publication (one test).

The repair distinguishes real and virtual transcript keys. A virtual projection
must be an explicit typed stub in a relation which names that key and has a real
endpoint in BOTH this publication's rows and prepared endpoints. Its slots must
reference zero Raw paths. All projections still charge duplicate/slot/byte
and byte limits. The exact real projection key set must still cover all real rows.
Namespace-scoped staged identity/ledger/claim checks validate those real anchors
inside the transaction. The existing exact recorded staged_projection_witness
check covers every stub field, relation and transcript before the first write.
No witness/proof field or typed stub was removed. The unchanged shared slot
publisher skips virtual projections; repeat comparison now does the same.

Worker73's Relations record guard remains unchanged: only explicit relation.stubs
may substitute for a missing end, every relation needs a real prepared endpoint,
and every prepared endpoint has already been checked against the namespaced
staging table in that transaction. Publication additionally binds the complete
prepared relation/stub/projection payload to its recorded epoch/claims witness.
There is no arbitrary missing-owner exception or new Raw authority.

## Verification and test changes

No existing assertion was weakened or removed. Existing test count remains 83.
The foreign-isolation test gains checks using newly prepared REAL shared-model
output (not fabricated rows/endpoints): changing a virtual projection key refuses
with semantic_publication_projection_owner and whole-DB equality; changing a
typed anchored stub name without re-recording returns Conflict with whole-DB
equality. After legitimate publication the stub has zero rows in Catalog,
Catalog sources, slot headers, slots and semantic owner membership.

Diagnostic/repair WIP df4b04c3d537d0da2802954c62420ad9b9117f35 (parent base)
was normally pushed early. Exact Storage run40211881972, conformance121224655496;
boundary121224655695 PASS. Formatting121224655572 failed; exact own-SHA artifact
12606062700, 1438 bytes, was downloaded/applied without a local formatter.
Final exact revision/check results are recorded below and in the delivery JSON.

First WIP exact-head conformance121224655496 actually executed 32 SQLite + 3
journal PASS and 82/83 configured PASS. All four publication failures and the
complete copied selector-value comparison passed. The copied test then reached
the unchanged fault-consumption assertion and failed before the owners hook.
The old 64-slot preflight cap conflated source/owner admission with transcript
length: the unchanged fixture has 65 tool calls. Shared catalog.rs:350-380 and
sqlite/publication.rs:188-216 publish complete slot vectors without that cap.
The repair removes ONLY that unsupported slot-count cap; 64 sources, 64 owners,
64 prepared projections at record, 1MiB aggregate serialized slot bytes, metadata
and producer-witness bounds remain. Full native transcripts remain byte-bounded.

Intermediate16b87ca5 CI40212312250/job121225983824 failed compilation in the
new diagnostic: EventCache returns io::Error rather than StoreError. No tests
executed there; no green claim. Corrected the diagnostic to print only allowlisted
static semantic refusal names and numeric max_slots. The existing fault assert
and rollback checks still run unchanged. Exact16b formatting artifact12605908845
(661 bytes) was applied. Implementation07169a41 follows16b with the slot-count
correction and diagnostic type fix; exact CI40212508965 is the next check.

The fault helper assertion changed syntactically from inline `publish(...).is_err()`
to `let result=publish(...); assert!(result.is_err())`, with an allowlisted diagnostic
added to the SAME fault-consumption equality. This preserves all original truth
conditions and does not clear the TLS hook. All parity/History/foreign/epoch/proof
and previous budget assertions retain their original expressions.

Final formatting exactly matches07169a41's own CI artifact12605394639 (560 bytes;
reverse-apply check succeeds against the local formatting). Boundary121226592973
passed on07169a41. The final commit includes this formatting plus this evidence
update, with no further semantic changes. Its exact-head CI results and verified
full remote SHA are supplied in the Worker JSON; checks pending at handoff-file
write time must not be read as acceptance. Named full code diff:
evidence/worker77-publication-driver.diff.patch (base30b54c69 to final code).

Hunk contacts: semantic.rs publication projection preflight and repeat comparison;
configured.rs ordinary Claude driver/two call sites/foreign-isolation additions.
model.rs and relation_stage.rs were read-only. No SQL joins/DDL changed; no PG,
S2 selected/presentation/proof, perf/cold, UI, Cargo, Hub, #380 or skills edits.
The 64-source/64-owner/1MiB limits and all old fault/rollback/History/proof/stale
epoch/foreign assertions remain intact.

Thermal telemetry was absent before work. No local cargo/build/check/test/clippy/
fmt/update/lockgen, Node/typecheck/browser/image/corpus work ran. CI only. Stop
at >=85 C or unavailable readings; one bounded named file <10MB per read/search.
No nested agents/harnesses/controllers, merge/deploy/publication/live PG, user
migration/deletion or credential/remote/permission changes occurred.

## Remaining full scope

This increment is not full #477/478/479/Hub313 acceptance. NativePeer/Name/members/
sidechains/cross-harness keys/local Factsless authority, recovery, S3/S4, full PG
EventIndex/semantic/catalog/selected/query with no SQLite, current-store migration
catch-up, exclusive fenced cutover, qualified rollback, Hub313, cold timing and
full UI remain. Preserve #484/Hub318, Hub321, Sessions-default279 and Source483
within #380. #380 is not complete; #477 main merge remains held. Current Semon
schema only; no historical Semon upgrade/compatibility ladder was added.
