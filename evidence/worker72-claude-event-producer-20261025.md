# Worker72 — bounded Claude event producer, CI pending

SOL_USAGE_UNKNOWN_BOUNDED_EXCEPTION: the single Manager-approved Sol attempt,
global72 / model15 / lifecycle-own7. No retry, escalation or counter reset.
This is a pushed partial implementation, not acceptance of Worker68, Worker72,
the full selector family, lifecycle, performance or #477. Manager adjudication
is required for the remaining native-peer prerequisite and test coverage.

## Prerequisite ACK and instructions

ACK all eleven supplied message IDs, including
`worker67-green-prerequisite-correction-006`, `post68-producer-plan-007` and
`source72-base-phase-order-008`. Read the mandatory named files BEFORE editing:
`ROOT/evidence/worker72-base-current.json` and
`ROOT/evidence/worker71-producer-phase-order.txt`; also read
`ROOT/evidence/approved-native-producer71-plan.txt`.

Base 23500b19734378149ab7d95515c508199906fe77 was clean and independently matched
origin refs/heads/task/477-raw-sql-backends-20261024. The named evidence records
parent 608d231edcd44c42d9cf763961df6fcd7180b553 conformance job 121206304386 PASS
32 SQLite + 3 journal + 77 configured + 90 catalog/storage + Query 3+3 + facts.
Base format 121208383012 and boundary 121208383161 PASS; base conformance
121208382899 was pending in that evidence. The Manager explicitly relaxed the
exact-target prerequisite for dispatch only. Worker did NOT poll CI; Gateway
must record the actual base result and classify any failure before review.

Read repository AGENTS.md and the semon-evolution skill. No older Semon schema
compatibility layer, DDL, dependency or native parser change was introduced.
Named ancestor/module instruction paths were checked; no deeper AGENTS.md was
present in the edited source ancestry. Native storage/retained design references
were read. Original user/contract authority and current-schema-only instruction
remain unchanged.

## Step 0 source enumeration (pre-edit base lines)

- ordinary semantic_stage.rs:1024-1135: up to 64 EVENT-or-skipped-source units,
  aggregate 1 MiB encoded metadata, exact xsm/agm/tn or
  tool SendMessage/Task/Agent candidates. Reads metadata with a 1 MiB guard,
  preserves native seq and offset/block/kind occurrence, stores copy identity;
  enqueues receipt+sender for message IDs, native_frontier for peer.sock.
- ordinary :987-1007 and :2188: qualification is SEPARATE, AFTER reduction.
  Takes 64 qualified=0 rows, derives owner from owner_sources, checks actual
  predecessor files.links.continued_in and physical event identity, then writes
  owner/copied/qualified. It is not legitimate to qualify in event_page.
- ordinary frontier_page :769-926: persisted keyset cursor, 64 observations per
  call; receipt reads events xsm.id joined to files; sender reads
  SendMessage.reply_m joined to files. Both enqueue physical sources. Members
  reads Current session_catalog_sources and has a different authority surface.
- ordinary native_frontier :683-728: native Current membership, five native
  dependency families, nativeall, filename/identity hints and cached headers.
  This is MORE than enqueuing one native key. No shortened parallel algorithm
  was introduced for it.
- ordinary event_page does not itself refuse sidechains; qualification tests
  target.is_top and model identity provides native ownership. Admitted base
  deliberately refuses is_top=0 and non-codex/claude. Those refusals remain.
- admitted semantic.rs base :1686-1703 went event/frontier/native keys ->
  Identity -> reduce_with -> Relations without qualification. Manager's
  additional phase hunk was necessary and was used.
- ordinary reset :1136 onward advances epoch, resets phase/cursors and key
  epoch. Existing admitted epoch/claim/journal reset and transaction fencing
  were retained. No new independent epoch or replay mechanism was added.

## Implementation and exact contacts (current code lines)

All source paths below are under crates/semon-sessions/src/.

- events/store/semantic_stage.rs:988: QualificationPort and qualify_with,
  with OrdinaryQualification's original SQL. Ordinary qualify_events now calls
  this recipe at the SAME post-reduce position (:2238).
- same :1050-1185: EventProjectionPort, ProjectedEvent and event_page_with;
  the previous loop was MOVED to this shared recipe. It owns metadata decoding,
  event classification, location identity, aggregate bytes, 64-unit budget and
  receipt/sender/native enqueue decisions. projection_predicate (:1133) and
  projection_candidate share the exact candidate set and native row decoder.
  OrdinaryProjection retains the existing physical SQL and streaming rows.
- storage/sqlite/semantic.rs:432-515: ScopedProjection and ScopedQualification.
  Admitted rows have explicit namespaces; every side of cursor, copied-event,
  event/file/input joins is bound to the same namespace. Inserts/updates and
  all absence checks are namespace-bound. Ordinary decoding is called directly;
  there is no second native parser or event classifier.
- same :558-565: receipt and sender frontier arms, with identical path keyset
  ordering, 64 shared frontier budget, claim enrollment and done handling in
  the existing transaction. Both sides of the events/files joins are scoped.
- same :1780: after reduce_with reports done, qualify_with runs up to 64 rows;
  the stage remains Identity until qualification completes. Relations is never
  set early. No row is qualified during indexing.
- same :1334-1356: ClaudeCall/ClaudeReceipt remain typed pending until all
  staged sources are indexed, events_done and identity_done, all projected rows
  are qualified, all frontier tasks are done, and phase is Relations/Publication.
  Supported top-level events can then use the existing shared select_with.
  ClaudeName is explicitly pending even for an otherwise empty match.
- events/store/configured.rs:7657 onward: one permitted closed-producer test
  rewritten, six tests appended plus private real-native fixture helpers.
- model.rs and model/relation_stage.rs have NO changes. Existing select_with
  remains the single selector algorithm. Recipes were extracted next to the
  existing reduce_with in semantic_stage.rs rather than creating another module.

No contacts with PG Recovery/SourceObservationStore/source_journal/owner TX,
S2 presentation/clock/proof hooks, semantic_owners.rs, history.rs, catalog.rs,
perf/cold, Viewer/UI, Cargo, Hub, #380, AGENTS or skills. Preserve their other
worktrees and resolve these exact semantic-stage contacts on later task merges.

## Ported and refused families

Ported: top-level Claude candidate projection; physical receipt and sender
frontiers; post-reduction copied-event qualification; guarded Receipt/Call
selection for fully completed supported cohorts.

Still refused: peer.sock native discovery has
`semantic_claude_native_frontier_pending` in a preflight before any event write.
Its ordinary algorithm requires identity/filename/header hints and multiple
native dependency families; replacing it with one native query would invent
negative evidence. ScopedProjection::native is defensive InvalidQuery behind
that preflight, not a successful stub. Full native-peer relationships are NOT
qualified by this delivery. Manager should decide the next connected port.

Members and ClaudeName remain pending (`semantic_claude_name_members_pending`);
other unported frontier kinds retain `semantic_nonprimitive_frontier_pending`.
Sidechain is_top=0 / other harnesses retain
`semantic_nonprimitive_event_harness`. Cross-harness key collisions retain
`semantic_cross_harness_native_key_pending`. Limits 64 sources / 64 owners /
1 MiB unchanged. No public factory or qualification flag opened.

## Tests authored — NOT executed

Expected configured suite count: 83 (previous 77, one designated rewrite,
six additions). This is an expected count, not CI evidence.

1. Existing domain67 closed-producer test ->
   `configured_semantic_domain67_claude_event_producer_projects_real_call`:
   BEFORE required event-harness refusal, pending selection and zero Catalog.
   AFTER real Raw Agent+receipt must reach publication, call selection count 1,
   full ordinary model oracle and existing transactional publication assertions.
   The former blanket producer refusal is intentionally no longer the contract.
2. domain72_claude_receipt_call_real_producer_oracle: one/two actual native
   sources, positive Receipt/Call, call ambiguity 2, ordinary selector equality,
   exact full projected rows including metadata/copy fields/owner, ordinary full
   model oracle, real publication and existing rollback/ACK oracle.
3. domain72_claude_foreign_namespace_projection_isolated: identical paths/native
   IDs/message IDs in source-b and source-a; exact whole foreign namespace
   preservation through a's real producer and publication.
4. domain72_claude_projection_epoch_reopen_has_zero_effects: reopen Source while
   a real unqualified event page exists; stale selector/batch zero whole-DB
   effects. Existing domain67 old prepared publication Conflict test unchanged.
5. domain72_claude_copied_events_qualify_after_reduce_in_pages: two continued-in
   native sources, 132 events crossing multiple 64-event/qualification pages;
   132 qualified / 66 copied expected from genuine repeated UUIDs and continued
   custody. Before qualification helper requires exact pending selector. After
   reduction/qualification exact ordinary projected rows and selected owner,
   count 1 instead of duplicate-call ambiguity, full model oracle/publication.
6. domain72_receipt_frontier_pages_to_source_capacity_refusal: ingest 65 real
   receipt sources, stage ONE legal source, require a persisted 64-source
   enrollment page and exact semantic_component_source_budget after discovery,
   whole failed-TX equality and zero Catalog publication. Does NOT raise budget
   or stage 65 into a capped admission API.
7. domain72_claude_unsupported_name_and_native_peer_refuse: exact Name pending
   with zero effects; real peer.sock native input must hit the named native
   frontier refusal with whole failed-TX equality. No negative is certified.

The requested new sidechain/non-Claude explicit refusal tests were NOT added;
existing production guards are unchanged. These test-coverage gaps, the native
peer family and all runtime assertions require Manager adjudication. No test,
performance threshold, time limit, native parser, existing positive oracle or
prior foreign/epoch/history proof assertion was weakened. The six new tests and
rewritten test are uncompiled/unexecuted, including their metadata/copy-count
oracles; CI may expose fixture or implementation defects. No green is claimed.

## Revision and verification

- Base: 23500b19734378149ab7d95515c508199906fe77.
- WIP commit: 86f9b58c69edf01b6f5fcdd5f155edc793df5e06, parent base;
  pushed at about 15:36:52 UTC, within 15 minutes of this attempt's start.
- Source commit: 7d60bdd26ab49ba5700a2b5029ce1961b3e06d5e, parent WIP.
- Normal push to https://github.com/onsager-ai/semon.git branch
  task/477-raw-sql-backends-20261024; independent ls-remote matched full source
  SHA. The evidence-only successor containing this handoff is reported with its
  full final SHA and remote verification in the Worker JSON result.
- Full SOURCE diff from base: evidence/worker72-claude-event-producer.diff.patch.
- CI PENDING: Gateway must run exact final-SHA 83 configured (expected), 90 group,
  32 SQLite, 3 journal, Query, facts, format and boundary; record base conformance
  result separately. Worker did not poll CI or use historical green as delivery
  acceptance. Formatting has NOT been run locally and remains a CI gate.
- Thermal log /home/marvin/.cache/thermal-watch/temps.log absent at first check.
  No local cargo/build/check/test/clippy/fmt/update/lockgen, node/typecheck,
  browser/image, corpus or CPU-bound validation. Only bounded named-file reads,
  edits and ordinary git delivery. Stop CPU work >=85C or without readings.

## Full remaining objective

Native peer/incident and sidechain discovery, ClaudeName/members, cross-harness
native keys, local Factsless authority, further lifecycle/recovery qualification,
S3/S4, complete PG native EventIndex/semantic/catalog/selected/query and no hidden
SQLite, supported-current-store migration/custody/catch-up/exclusive fenced
cutover/qualified rollback, Hub #313, cold timing, full UI/visual binding, fresh
independent acceptance and exact final green pins all remain. #380 main approval
is pending; #477 main merge remains HELD. No main merge, deployment, publication,
live PG, real user migration/deletion, credentials/remotes/permission changes,
other harnesses or nested agents were performed. out/ and other worktrees were
preserved. No automatic additional attempt is authorized.
