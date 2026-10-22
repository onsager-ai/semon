# Retired Model oracle and native ownership

Tracking: #469, follow-up to #385 under #380. Audited base:
`5f34607070ff2e7cd5ef4f1e41a5ca9c2935f9f8` (2026-10-22).
This records code retirement, not merge, release or live-data cleanup.

## Inventory and disposition

| Boundary at the audited base | Disposition |
| --- | --- |
| `model::reference_json`, `build`, `build_sources`, `Built`, `Machine`, `Rest`, `MODEL_API`, `LIVE_MS`, `BuildTarget::Model`, `BuildOutput::Model` | Deleted, including test compilation. No executable full-workspace Model oracle remains. |
| Model-only human-ask eager reads, analytics snapshot, JSON envelope, serialized machine, FNV workspace version, transcript byte/running growth marks, `texts_by_turn`, `AFTER_MODEL_DERIVATION`, `COMPAT_BUILDS` | Deleted. Native asks remain source offsets; active Analytics publication and native Query facts remain. The removed counter cannot prove retirement after its target disappears; CI checks the source graph instead. |
| Model callers in `model/tests.rs`, `tx.rs`, Catalog, Query and Viewer tests | Replaced by the supported native Query producer, selected APIs, direct shared semantic assertions and independently preserved reference data. |
| `tx::page`, `page_limited`, `page_from`, `signals`, `errors`, `errors_limited`, `ERRORS_MAX`, `full_slot`, `full_yielded`, `full`, `text_json`, `read_record` | Retired Model/transport adapters deleted. `tx.rs` retains shared native rendering. `tx/tests.rs` owns focused, typed parser assertions and a generation-bound native page adapter; it builds no Model/version/transport envelope. |
| `model.rs`, `model/summary.rs`, `model/relation_stage.rs`, `QueryData`, `build_query`, `prepare_*`, `query_history`, native `Session`/`Turn`/`Handoff`/`Transcript` types, text caches and active Analytics derivation | Active shared native semantics. Kept under their existing owner; renaming the module would obscure a focused deletion with a repository-wide import change. The only build targets are Query, Catalog and Sources. |
| Native discovery/parsing in `events.rs`; older Claude tags, Codex tool formats, Copilot records, native ancestry, source proofs, retained recipes and archive generations | Supported native behavior and persisted history. Kept. A symbol containing `legacy` does not establish dead code. |
| `CatalogWriter::Legacy`, old runtime/proof/recipe versions and migration tests | Existing complete-producer transaction and persisted-data compatibility, independent of Model transport. Kept with their generation, custody and refusal assertions. |
| Retired `/api/model`, `/api/tx`, `/api/transcript`, `/api/entry`, `/api/analytics`, deltas/SSE and `compat=1` | Already removed from production at the audited base. Existing Viewer 404 and consumer transport regressions remain. No replacement routes or fallback are added. |
| Query `contract-reference.json`, five `model/golden/*.json` files, historical browser/reference evidence | Independent regression evidence. Kept byte-for-byte; never loaded by production. Historical design documents are explicitly marked and linked to current ownership. |
| Old JSON event-cache import | Required persisted-fact migration, isolated in `events/store/json_migration.rs`; see below. |

## Independent expectations and assertion migration

The 134-response Query reference remains from retired production
`7ce1ac4d4f34af9143aa292b631a07fd797890b1`. Its fixture families,
clock and capture method remain in [the Query contract](../../crates/semon-sessions/src/query/README.md).
All five original semantic golden files retain their exact bytes. Tests compare
native sessions, source-backed handoffs, turns, merged busy intervals, machine
identity and the shared pricing table against those preserved fields. Additions
that predate this retirement but postdate the goldens retain their explicit
usage/cost/call-count assertions. There is no golden-update switch.

`crates/semon-sessions/src/model/retirement-reference.json` additionally preserves:

- The complete selected session metadata and transcript page from the original
  `catalog::tests::selected_native_index_and_scoped_builder_preserve_membership_and_oracle`.
- The answered question event from the original
  `catalog::tests::home_native_previews_preserve_question_answers_and_archive_generation_without_caching_text`.

Both were captured before deletion from an isolated checkout of audited base
`5f34607070ff2e7cd5ef4f1e41a5ca9c2935f9f8`. Only writes of the existing old results
were inserted immediately before their assertions. The original Model, Query,
Viewer and parser bytes were unchanged, and both original tests passed. Inputs
are the existing synthetic `synthetic-catalog` fixtures, October 1 timestamps,
and `NATIVE_PRIVATE_*` sentinel text, not personal logs. The selected native Query
response has its existing `window_start: null` in addition to the old page.
The preserved file contains no filesystem paths, credentials or generator.

The new native tests request briefs through generation-bound recipes instead of
assuming that metadata preparation materializes every prompt. Rewrite coverage
now asserts refusal either during native derivation or on the selected body read,
with the original Catalog generation preserved. Sidecar races fail during native
preparation. Append/rewrite/restart seeded equivalence still compares native
sessions, relationships, facts, transcript slots and Analytics; every assistant
text is checked at its actual slot/turn/byte offset. Mirrored fixtures compare
semantic values and rendered pages, not machine-local path strings.

The retired envelope's build-timing list, API/version/growth marks and serve-time
30-minute activity serializer have no current consumer. Their exclusive output
assertions are removed. Native activity tests retain the recorded tool/start,
last-turn and qualified process-liveness assertions across clock changes. Native
timing measurements retain their separate producer owner/tests. Non-UTF-8 local
tree discovery remains covered; portable Query refuses unrepresentable source
paths and never publishes a lossy Catalog alias.

No source-backed output, ordering, paging, retained-history, relationship, usage,
security, generation or integrity expectation is regenerated from the candidate.
`test_model_retirement.py` checks the removed entry points/build targets and
hashes the independently preserved references; existing runtime tests retain
actual native build/parse/read accounting and retired-route refusals.

## Supported JSON cache/history disposition

`sessions-index.events.json` is not the retired Model JSON envelope. Its event
rows and file indexes are rebuildable and are skipped by the streaming importer.
Two fields remain supported: historical `reported_runs` and `claude_json_stamp`.
`~/.claude.json` overwrites its last reported run, so older captured costs,
durations and model usage can exist only in this cache or the SQLite store.
Native logs cannot reconstruct those reported facts. There is no evidence that
all supported installations have already transferred them. Removing the importer
would silently lose that history.

The isolated migration retains the existing behavior without a schema change:

- Import reported runs with `INSERT OR IGNORE`, preserving an already accepted
  snapshot. Adopt the stamp only when absent. Native parsing owns rebuildable rows.
- Commit with `synchronous=FULL` before removing a claimed JSON file. Repeated
  opens and concurrent importers remain idempotent.
- Claim by rename and verify the open-file identity before removal. Replaced
  files, old writers and crash-left claims are imported before disposition.
- Preserve unreadable files for retry; set malformed JSON aside without replacing
  another saved copy. Refuse symlinks/FIFOs and retain the collision/race tests.
- Read-only selected consumers perform no import. Writable Event Index opening
  owns migration. Shared regular-file admission remains with the storage owner.

Removal requires an explicit supported installation/cache policy and a proven
custody/export disposition for every non-rebuildable reported fact, including
old-writer and interrupted-import cases. #469 establishes neither global
migration completion nor authority to delete user caches, SQLite databases,
native logs, archives, credentials or deployed services.

## Reconciliation with #385 and #380

At the audited main, native consumer migration and retirement of the production
Model owner, old routes, compatibility warming, deltas and full-model fallback
were already implemented. #385's statements that those Rust routes/owners still
needed implementation were historical. This follow-up deletes their remaining
executable test oracle and exclusive preparation; it does not redo consumer
migration or assert that all broader foundation work is finished.

The supported chain remains native sources → Event Index/Catalog → focused
Query/Transcript/Entry → Viewer/CLI/MCP. Complete native producer reconciliation
and bounded selected-history preparation remain supported where documented;
these are not full Model fallbacks. Existing selected-source work limits and
refusals are unchanged. The separate live transcript/proof repair and independent
Codex/Claude conformance roadmaps remain outside this cleanup.

#385/#380 still own final integration/adoption and their applicable consumer,
visual, exact-pair and release qualification. This branch adds no shared public
interface, UI behavior, bundle or Hub pin change. Local/source CI evidence must
name the actual candidate; prior Source/Hub receipts do not qualify a new pair.
Control delivery, broad full-text retrieval and SSH remain paused. No merge,
image publication, staging/production deployment or live cleanup is performed.
