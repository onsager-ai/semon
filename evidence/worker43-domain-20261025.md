# Worker43 / model5 — domain ownership WIP

This is an implementation checkpoint, not acceptance. CI has not been run and
nothing is qualified. One bounded attempt; no nested agents or controllers.

ACK: original; user-resume-001; user-schema-current-only-005;
domain42-current-001; domain42-pin-correction-002; domain43-reconciled-003.

Base HEAD and actual origin ls-remote both:
`6841d5bd1bd9ab02733d89bbcc162d19484201a5`.
Parents: `4aaed3b58ba257fadda5dcca569b162272e21f6e` and
`10276dab5185055855fee66fc573175a1e564bb1`.
Branch: `task/477-raw-sql-backends-20261024`. Initial status and stash list empty;
workspace evidence directory had no worker42 files. Corrected parent acknowledged.

| family | namespaced now? | current writer | consumer read | non-rebuildable? |
| --- | --- | --- | --- | --- |
| PID dependencies | yes, catalog_native_dependencies | CurrentPublicationStore and native identity stage | runtime dirty-source lookup | observed runtime facts require their original authority |
| lineage and native parent | yes, dependency/input/endpoint/native family | native index links, identity stage and CurrentPublicationStore | frontier and shared parent/root reducer | provenance must be preserved when Raw is unavailable |
| relations | yes, catalog_semantic_relations | existing native relation matcher/stage | final incident replay, trace/slots/Query | recorded placements and custody cannot be invented |
| withdrawal | yes, stage source withdrawal and stage authority | native producer using CurrentSources facts and Raw stat | stage/replay/publication validation | explicit negative admission authority, never inferred from absence |

Planned sites: typed namespace-owned stage effects; exact existing current
physical-profile validation before enrolment; same model identity/matching/replay;
durable owner witness in model/proof; transaction fault and colliding-source tests.
No new parser, Options.cache fallback, public qualification or schema upgrade.

S2 reservations remain untouched, including its proof publish hook and appended
tests. No PG/perf/Hub/#380 worktree writes. #380 incomplete; #477 main merge HOLD.

Remaining: event incident candidate replay, cross-harness native keys, complete
frontier/selectors, recovery intents, complete owner lifecycle and retirement,
S2 integration, S3 remaining owners, S4 qualification, injectable PG producer/full
PG backend, migration/catch-up/cutover/rollback and Hub #313 RLS. This list will
be reconciled against the actual final diff.

Thermal log absent. No local CPU-bound work, cargo build/check/test/clippy/fmt/
update/lockgen, Node checks/formatters, browsers, images or corpus analysis.
CI only, owned by Root. Stop CPU work at >=85 C if readings return. Every read
or search is bounded to one named file under 10 MB. Preserve unrelated out/.
No merge/deploy/publication/live PG/user migration/deletion or auth/remote changes.

## Implemented checkpoint and final handoff

Early evidence commit: d0a897e7d54dfa84b27826675141c0be9c0680a3, pushed and
actual ls-remote verified. First code WIP:
b40817e3c22c75c4c27ec3fdbad65c547196cd86, pushed and actual ls-remote verified.
The final follow-up commit contains this handoff and the bounded review fixes;
its full HEAD/remote SHA is recorded in the Worker JSON and workspace handoff.
Origin remains https://github.com/onsager-ai/semon.git; same task branch only.
No force/rebase, main merge, deployment or publication.

### Pending sites replaced and evidence

- Removed the broad publication nonprimitive-owner refusal only after adding real
  typed owner effects and their durable exact witness. `OwnerStore` carries
  endpoint, relation, dependency and withdrawal values; SQL stays in SQLite
  adapter files. New `semantic_owners.rs` is a child module of semantic.rs, so no
  storage.rs/sqlite.rs declaration or S2 reservation was edited.
- The real producer now dispatches Relations to existing matching mode and
  Publication to existing replay mode. No parser or PreparedSources fixture was
  invented. The shared parent/root/family-cost reducer is unchanged.
- Native parent and PID identity restrictions are replaced by endpoint columns,
  Source-owned dependencies and normalized parent/root descriptors. Claude
  continuation source IDs may share their model-derived canonical key; every
  physical source still matches its exact staged metadata/native ID, and the
  canonical key must exist in this Source's staged keys.
- Relation rows persist every typed field. The original publisher and configured
  adapter share `Relation::merge_prior`, preserving strict semantic equality and
  physical-sender origin-turn witness merging. Final fenced publication applies
  the exact model relations again. Indexed namespace/from/to pages of <=64 rows
  and <=1 MiB feed the existing model replay, including continuation cursors.
- Lineage/bridge observations retain ingest-owned per-path durable dependencies;
  native/bridge frontier reads are scoped. Remaining selectors refuse. No empty
  foreign-table assumption or missing-column fallback was added.
- Explicit withdrawals use the existing native CurrentSources/Raw-stat producer.
  The same collection helper is used at indexing and final re-observation.
  Source/admission-qualified batch runtime is compared to the native observation
  through its exact binding tuple, rather than comparing unlike fingerprints.
  Authority changes reset the existing stage on begin; stale publication refuses.
  Positive/negative source overlap refuses. Authority, stat, claims and ledgers
  recheck before effects and ACK. Raw or accepted ledgers are never deleted.
- Fully withdrawn owners alone can retire, and only within their namespace and
  complete claimed source closure. Catalog/source lifecycle becomes retained;
  slots, trace and retained search positions survive. Current search counts and
  memberships retire through the existing typed search port. General relation /
  replacement ownership moves still refuse explicitly. Priority B was not begun.
- The durable model/proof witness now includes PID sets, native parents, lineage,
  endpoints, every relation field, withdrawal authority and message selectors,
  alongside exact trace/recipe witnesses. All effects remain in the existing
  epoch/admission/Raw/ledger/claim publication transaction, before proof/ACK/head.
- Admission validates exact current authored stage DDL/columns/PK/indexes through
  existing support validation; the owner dependency family also validates exact
  types/nullability/PKs, STRICT/WITHOUT ROWID and namespace-leading lookup index
  before any enrolment write. No old-schema support or conversion was added.

### Tests authored, not run

Four tests are in the separate `tests::domain43` block of configured.rs. Existing
storage-contract CI's configured::tests:: prefix includes this block.

1. `configured_semantic_domain_parent_relations_and_pid_collisions_are_source_owned`:
   actual Raw Codex parent/child, same absolute paths and native IDs in both
   Sources, exact stored parent/root/PID and relation SQL assertions, every-field
   unbound relation/row/recipe/trace parity, independently hashed runtime tuple,
   and old-epoch zero-write publication after same-runtime takeover.
2. `configured_semantic_domain_claude_pid_and_continuation_are_source_owned`:
   actual native Claude continuation records and sessions/123.json, two Sources,
   nonempty PID and lineage facts, exact stored dependency checks and the same
   unbound producer parity oracle.
3. `configured_semantic_domain_unsupported_owner_profile_refuses_before_enrollment`:
   nonempty current but unsupported dependency profile remains unchanged.
4. `configured_semantic_domain_withdrawal_retires_only_owned_current_and_keeps_recipes`:
   real prior publication in both Sources, authoritative native selection removal,
   stale authority zero writes and fresh-authority stage recovery, exact retained
   Catalog/source/slot custody and Current versus retained search assertions.

Each real stage test injects rollback at semantic_record_owners, semantic_record,
semantic_record_relations and semantic_model_record. Publication loops cover
history, slots, Current, owners, retirement, trace, search, proof, ACK and head.
No existing test or threshold was removed, ignored or weakened.
`git diff --check` passed; it is not a Rust/format/semantic check. CI has NOT been
run, no compiler or rustfmt result exists for this increment, and nothing is
qualified. Root owns exact-head CI and formatting artifact adoption. Preserve all
historical FAIL results and request fresh Manager review of Worker36/38/40/43.

### Paths, reservations and honest remaining work

Changed implementation files: events/store/configured.rs;
events/store/semantic_stage.rs; model.rs; model/relation_stage.rs;
storage/sqlite/semantic.rs; storage/sqlite/semantic_owners.rs;
storage/sqlite/writer-inventory.json; docs/design/session-storage-contract.md.
Also changed this tracked evidence file. Per-file base/head diffs are supplied
in workspace evidence/worker43-diff/ with an explicit mapping.

S2 contact: NONE in reserved hooks/declarations/files. configured.rs adds one
physical-profile validation call and a separately named test module; its
prepare_owned initialization was not edited. semantic.rs changes domain
record/replay/publication before the reserved proof insert/effect hook, leaving
that exact hook untouched. docs append a distinct Worker43 section, not S2.
No perf cold regions, PG/perf/Hub/#380 worktrees, or unrelated out/ were touched.

Remaining, explicitly:
- Event harness/incident candidate projection and replay discovery (Claude
  incident refusal remains; only already derived supported relations replay).
- Cross-harness native-key collisions and prefix occupancy allocation.
- Frontier selectors beyond the existing native/path/header plus added lineage /
  bridge dependency cases; complete dirty closure and high-degree matching.
- Recovery intents and full producer recovery integration.
- Nonprimitive selectors (ClaudeCall/Receipt/Name, CodexSibling/Relay/Wait).
- Native-parent/phase/identity for unsupported harnesses, unresolved companions
  and complete process/incident closure; supported typed ports are WIP pending CI.
- General affected-owner retirement on relation/replacement ownership moves.
  Full History recipe transfer/re-read, slot/trace/search/proof retained lifecycle
  remains unqualified; the new withdrawal test re-reads retained Catalog and exact
  stored slots, not a claim of complete History reader acceptance.
- S2 Worker41 integration and its CI/review; S3 remaining owners; S4 public
  qualification. Public flags remain 0/None, catalog completeness partial,
  selected factories closed.
- Injectable PG producer; full PostgreSQL backend and shared parity/no-SQLite
  instrumentation; migration/catch-up/exclusive cutover/qualified rollback;
  Hub #313 restricted-role forced RLS and full integration.
- Exact-head storage CI/new and all prior tests; CI rustfmt or exact artifact;
  full Rust/Usage/Viewer and final independent Manager verdict / exact pair.
  Supplied base CI facts remain historical: initial_catalog cold timing open,
  Viewer visual/transport binding pending, combined 6841 Rust result unverified
  by this Worker. No CI polling was performed.
- #380 is not complete under the supplied record; #477 main merge HOLD persists.

Thermal log remained absent. No local CPU-bound tests/builds/checks/formatters,
Node checks, browsers/images or corpus work ran. Every git command disabled
automatic GC/maintenance; author/committer is Marvin Zhang
<3393101+tikazyq@users.noreply.github.com>. No nested process/agent/controller.
