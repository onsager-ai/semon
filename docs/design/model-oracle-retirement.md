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
| `CatalogWriter::Legacy` | Active row-by-row SQL transaction used when a Catalog batch is unsupported or oversized. Kept as a current producer path, independent of the retired Model. |
| Pre-scoped Catalog runtime anchors and upgrade qualification | Deleted. Current runtime observations qualify their own scoped dependencies; unsupported old runtime observations are not promoted through an upgrade anchor. |
| Persisted native proofs, retained recipes and schema custody | Existing storage ownership and refusal assertions remain. These protect native history independently of the retired Model. |
| Retired `/api/model`, `/api/tx`, `/api/transcript`, `/api/entry`, `/api/analytics`, deltas/SSE and `compat=1` | Already removed from production at the audited base. Existing Viewer 404 and consumer transport regressions remain. No replacement routes or fallback are added. |
| Query `contract-reference.json`, five `model/golden/*.json` files, historical browser/reference evidence | Independent regression evidence. Kept byte-for-byte from the independently captured Query reference and current upstream goldens; see the pricing-only upstream refresh below. Never loaded by production. Historical design documents are explicitly marked and linked to current ownership. |
| Old JSON event-cache import | Removed on 2026-10-22 at the user's explicit no-compatibility decision. Historical reported facts that exist only in this cache are no longer imported; existing cache files are left untouched. |

## Independent expectations and assertion migration

The 134-response Query reference remains from retired production
`7ce1ac4d4f34af9143aa292b631a07fd797890b1`. Its fixture families,
clock and capture method remain in [the Query contract](../../crates/semon-sessions/src/query/README.md).
All five semantic golden files retain their exact bytes from upstream
`634b5312f9359dae1a9ab1c6979ae4878539e5bc` (#466), integrated after the original
retirement review. That independent runtime-pricing change updated only the
`pricing` object: its date and the `claude-haiku-5-5`/`gpt-6.1-sol` entries.
The sessions, handoffs, turns, machine and busy expectations are unchanged from
the original `5f346070` Model capture. This cleanup generates no replacement
expectations; its integrity guard pins the independently updated upstream bytes.
Tests compare
native sessions, source-backed handoffs, turns, merged busy intervals, machine
identity and the shared pricing table against those preserved fields. Additions
that predate this retirement but postdate the goldens retain their explicit
usage/cost/call-count assertions. There is no golden-update switch.

Catalog tests now state native expectations directly: selected session metadata,
transcript entries and turn identity, answered question timestamps and status,
and source-backed private question/answer text. They retain build/read counts,
unrelated membership, conflicting source rejection and archive generation checks.
No supplementary snapshot of the retired Model is loaded by these tests.

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

## Retired JSON event-cache import

On 2026-10-22, the user decided to drop compatibility and remove the old JSON
event-cache importer. Writable full and scoped SQLite store opens no longer look
for or read `sessions-index.events.json` or interrupted `.importing` claims.
They do not claim, rename, quarantine or delete those files.

The current SQLite reported-usage snapshots and live collection from
`~/.claude.json` remain supported. Historical reported runs or the file stamp
that exist only in the retired JSON cache are no longer imported and cannot be
reconstructed from native logs.

There is no retired JSON test schema or import simulation. The reported-run
regression collects two successive live snapshots, overwrites the live source,
and reopens SQLite to verify both runs and their recorded costs remain.

Any old JSON cache or interrupted import file already present is left
untouched because no code opens it. Removing the reader does not authorize data cleanup; deletion
remains an explicit operator action. This decision removes compatibility support
without changing current SQLite schema preparation, corruption recovery, or
native event parsing.

The pre-scoped Catalog runtime upgrade path is also removed: no compatibility
anchors are written, preserved or accepted by readers. Local process and naming
observations still supply the current aggregate generation. PID write intents
retain only the fingerprint of unaffected inputs so interrupted writes can
replay dependency invalidation without rebuilding unrelated native history.

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
