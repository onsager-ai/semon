# Worker68: native selector reissue — unfinished WIP

SOL_USAGE_UNKNOWN_BOUNDED_EXCEPTION: one Manager-approved 1200 s reissue of plan-native-selector67-005; no retry, fallback, tier change or counter reset. Actual Worker68 / model14 / lifecycle-own6. This is the clerical evidence-delivery correction to Worker67, which made no edits. This report does not accept this slice or #477.

## Prerequisite and authorization

ACK all nine intake IDs, particularly `worker67-green-prerequisite-correction-006`. Read the actual named ROOT/evidence/worker67-base-current.txt before implementation and the full ROOT/evidence/approved-native-selector67-plan.txt. Local HEAD and actual origin task branch both equalled c7cd6daac1a21b435f2e65ed994df5f41cacef3c, with a clean tree. The named Gateway evidence records exact Storage 40201535363 conformance 121193742369 (32 SQLite + 3 journal + 73 configured + 90 catalog/storage + Query/facts), format 121193742534 and boundary 121193742611 all green. Worker did not poll CI or treat the historical pending note as current evidence. This prerequisite was satisfied.

Only current Semon schema is supported. Repository AGENTS and semon-evolution were read. No compatibility layer or DDL migration was introduced. Commit/push authority is the supplied user contract and shared remote-delivery instruction; no merge/deploy/publish authority.

## Actual production gap found while implementing

The plan correctly found selector and frontier refusals but missed an earlier producer gate: `storage/sqlite/semantic.rs:432` (`event_page`) refuses Claude non-top sources and actual Raw-backed xsm/agm/tn or SendMessage/Task/Agent events with `semantic_nonprimitive_event_harness`. It then only acknowledges sources without relationship-event candidates; it does not populate catalog_semantic_events. Therefore real Claude Receipt/Call positive and ambiguity fixtures cannot reach the new lookup adapter under this producer. The ordinary producer in events/store/semantic_stage.rs:event_page does populate native events, discovers receipt/sender/native frontiers and later qualifies copied-event ownership. Porting that producer entails the native incident/frontier work explicitly reserved out of this plan. Merely removing the refusal, inserting synthetic endpoints/events, or declaring empty admitted event rows to be negative evidence would be wrong.

This is a code-grounded scope/plan blocker, not missing CI or authority. No incident producer/frontier guard was removed. The prescribed six test families and five additional working variants have NOT been delivered. Manager must adjudicate the real producer/incident slice rather than automatically retrying this unchanged premise. Full plan outcome is BLOCKED with useful, pushed WIP for review.

## Implemented WIP

- `model/relation_stage.rs:236` adds SelectorLookup; `:256` owns the single 1 MiB JSON budget; `:269` resolves a decoded event's physical owner; `:280` assembles parent/root/root_alive; `:299` is select_with. It owns variant dispatch, ambiguity cardinality, shared call+endpoint byte accounting, and hint-before-match ordering. Primitive SQL and existing NativeKey derivation remain in adapters. No matching algorithm was copied.
- `events/store/semantic_stage.rs:2800` retains the ordinary epoch check and now calls select_with using OrdinaryLookup (`:2830`). Original selection SQL text is moved into primitive methods. Ordinary name replay retains its IMMEDIATE transaction, epoch-qualified runtime lookup, members queue, bounded frontier page, reset and commit; it still returns the same replay Data error. Existing normalized_endpoint (`:495`), decode_event (`:1002`) and bounded (`:1279`) delegate to the moved common helpers instead of retaining separate decoding/assembly algorithms.
- `storage/sqlite/semantic.rs:538` adds AdmittedLookup with namespace=?1 on every physical table/each admitted join side, including names, receipt first subquery, sender/receipt/endpoints/root joins, path EXISTS, descriptions/catalog hints and event ownership. NativeKey keeps its existing count-0/1 versus ordinary derived-key count-1 asymmetry.
- `storage/sqlite/semantic.rs:1232` retains source validate, transaction and exact epoch/resetting check. Admitted Codex Sibling/Relay/Wait can use the shared recipe. Admitted Claude Receipt/Call remain explicitly refused as `semantic_claude_event_projection_pending` (`:1251`), including before event projection: the empty stage cannot certify negatives. Claude name hints return `semantic_claude_name_members_pending` (`:1258`), never count 0. Full Claude name positive/duplicate/replay qualification is pending.
- The members frontier did NOT land; the optional typed pending refusal was used. Existing nonprimitive frontier guard remains at `:510`. No source, owner, row or byte budget changed; 64/64/1 MiB remain.

## Schema finding

No DDL/producer extension was necessary for physical shape: the current authored admitted DDL already carries namespace-leading events, endpoints, names, paths and owner_sources (events/store/semantic_stage.rs:134-170). The missing part is real production/qualification/discovery of Claude event content, not a missing table. No schema or public qualification version changed.

## New tests (written, NOT run)

Appended only in configured.rs's existing domain52 module; all prior 73 tests are untouched. Four new tests, so expected configured count is 77 if they compile:

1. `:7442` configured_semantic_domain67_codex_sibling_relay_wait_real_producer_oracle: real parent/two children, native collaboration.send_message, native path and nickname selectors, ordinary staged selector comparison, duplicate count 2, excluded Wait cardinality, full ordinary model oracle and atomic publish/rollback helper. Direct endpoint comparisons remove only the already distinct ordinary/Source query runtime address; native provenance and all other fields agree. Full model comparison uses the existing ordinary oracle.
2. `:7474` configured_semantic_domain67_foreign_native_ids_and_paths_are_isolated: both admitted Sources ingest the same real paths/native IDs and build/publish; source-a selection counts remain one, selection has whole-DB zero effects and foreign namespace remains identical after source-a publication.
3. `:7499` configured_semantic_domain67_stale_epoch_source_collision_has_zero_effects: same physical Source is reopened with replaced Facts authority and a real runtime/reset; stale selector refuses, old prepared publication conflicts, whole DB unchanged.
4. `:7521` configured_semantic_domain67_claude_event_producer_remains_explicitly_closed: real native Agent tool input; intended to reach the existing precise event producer guard and whole-transaction rollback, with no Catalog publication. It also checks explicit Call/Receipt pending refusal and zero effects during indexing. This proves nothing until CI actually executes it.

Missing prescribed tests remain explicit: positive/ambiguous Claude Receipt/Call, unique/duplicate/hint-replay ClaudeName, same tool/name foreign isolation, and real-producer oversize metadata selector rollback. No fabricated PreparedSources, Catalog, endpoint or event rows were used in these new tests. Existing Raw admission/ingest/model/record/publication paths are used. Ordinary full-model/native parser, History, retirement, trace and selected assertions were not weakened.

## Exact contacts / reservations

Only these four source files changed:

- model/relation_stage.rs: inserted port/recipe/helpers between Selection and Endpoint, around 233-341. Existing lifecycle/OwnerStore code and Context are unchanged.
- events/store/semantic_stage.rs: normalized_endpoint 495; decode_event 1002; bounded 1279; semantic_selection 2800 plus OrdinaryLookup after its impl. No ordinary producer, event qualification, frontier, source witness, retirement or publication logic edited.
- storage/sqlite/semantic.rs: inserted AdmittedLookup immediately before ScopedReduction at 538; select at 1232 only. No event_page, frontier_page, reset, lifecycle reduction, owner record/publish, PG transaction/receipt, source journal or S2 witness/proof contacts edited.
- events/store/configured.rs: append-only domain67 helpers/tests from 7388. No prior test changes.

model.rs: NO contact. Proof generation / S2 hooks: NO contact. catalog.rs/history.rs/semantic_owners.rs: unchanged. PG66, S265 and other worktrees remain read-only; out/ preserved. No Cargo, UI, performance/cold, Hub, #380, AGENTS or skill changes.

## Revisions, diff and verification

- Authorized remote: origin, https://github.com/onsager-ai/semon.git.
- Branch: task/477-raw-sql-backends-20261024.
- Base: c7cd6daac1a21b435f2e65ed994df5f41cacef3c.
- Initial WIP: c02f9715f37387dafb57185d115ba192ef29b2e6, parent c7cd6daa; pushed and actual ls-remote verified before 15 min.
- Code revision: 35591d6157cd8a2725812cc2feb527bf67cf10c9, parent c02f9715f37387dafb57185d115ba192ef29b2e6; adds explicit nonnegative Claude event fence and corresponding fixture assertions.
- Full code patch: evidence/worker68-native-selector.diff.patch, base c7cd6daa to code 35591d61. Final delivery adds this report/patch only; its exact final full SHA and actual remote verification are in the Worker result. Author/committer Marvin Zhang <3393101+tikazyq@users.noreply.github.com>.
- Bounded git diff --check produced no whitespace errors. This is not compilation, formatting or acceptance evidence.
- Final revision: CI PENDING. No cargo build/check/test/clippy/fmt/update/lockgen, Node/typecheck, browser/image/corpus work, formatter or CI polling was run. In particular no claim that Rust compiles or the new tests pass. Root must run actual final-SHA 77 configured (if compiled), 90-group, 32 SQLite, 3 journal, format/boundary and applicable full gates, then Manager audits both this code and the earlier c7 History oracle freshly.
- Thermal telemetry /home/marvin/.cache/thermal-watch/temps.log was absent; no CPU-bound work was started. Stop at >=85 C or unavailable readings. All git commands disabled gc.auto and maintenance.auto.

## Remaining objective

Claude event producer/copy qualification and receipt/sender/native discovery; Claude name replay/members; cross-harness keys/collision-stable identity; native incidents; local authority without Facts; recovery; full S3/S4; complete PostgreSQL and no-SQLite instrumentation/parity/RLS; supported-current-store migration/catch-up/exclusive fenced cutover/qualified rollback; Hub #313; cold timing/performance; UI binding/full visual/browser acceptance; independent final review. #380 remains incomplete pending user Main+GHCR/staging authorization, and #477 main merge remains held. No user data migration/deletion, live PG, credentials/remotes/permissions changes, merge/deploy/publication or nested agents/harnesses/controllers occurred.
