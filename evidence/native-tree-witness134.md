# Worker134 current witness WIP

Base b0380be7c5edba4107cf7f29b22a4ec89d5db9f0 was clean and matched
origin/task/479-pg-foundation-20261025 before edits. Worker132 failed on
provider capacity after real WIP; it is not accepted or replayed here.

The current witness uses domain-separated fixed-width SHA-256 key addresses
and a length-framed SHA-256 stream, tagged tree4. SQLite stores generated
addresses and indexes them; PG uses matching generated addresses behind its
namespace-digest leading key. A source's namespace is authenticated by the
captured header and source owner; it does not permute the transferable witness
order. Full original keys and values remain in the heap and witness. Public
maps keep logical ordering. This is neither an XOR nor an additive checksum.

Fresh-only schema versions: SQLite 34, PG 9. No upgrade/reset or old reader.
Current SQLite and PG producers/readers now stream the same model/tool address
order. The witness covers every tool row, including retained closed names,
NULL names and closed state. PG runtime keyset reads recheck full key/address
equality; conflict paths check full namespace and original key. The tool-name
index no longer embeds unbounded names. SQLite generates matching stored
addresses using the deterministic, innocuous semon_tree_digest function.
Default/configured creation and current qualification register that function;
the authored reference schema uses the same definitions.

Authored, NOT executed locally:
- pg_native_keys134_current_witness_long_keys_real_producer_without_sqlite:
  actual SQLite parser/store/scalar/lazy oracle before PG; isolated child with
  process-wide SQLite-open rejection and a positive rejection self-check;
  nonrepeating long model/tool/name/path, NUL/non-BMP, namespace parity,
  restart and forced address collision with full transaction rollback.
- pg_native_keys134_tree_warm_generated_and_index_drift_refuses_before_effects.
- pg_native_keys134_tool_selected_physical_rows_and_bytes_ignore_unrelated_history:
  EXPLAIN ANALYZE on the exact runtime length query at 0/256/2048 unrelated
  generations; actual leaf/filter visits and selected heap-name byte length.
- tree_witness_current_addresses_authenticate_closed_null_and_missing_rows:
  changed closed/NULL names, closed state, missing models and unversioned
  witness refusal, with retained stored state.
- Existing independent SQLite JSON framing, actual row/byte admission and
  cancellation oracles were updated for this one current format.

API notes for Root/Native133: TreeCheckpoint is unchanged. Writer::tool now
takes (id, Option<name>, closed); finish returns tree4:<hex>. Tree model/tool
conflict targets end in model_digest/tool_id_digest. PG schema VERSION and
initializer are 9; SQLite SCHEMA_VERSION is 34. The PG generated definitions,
PKs and index manifest participate in exact warm qualification.

This does NOT finish Tree or scope125. Whole-dimension witness reauthentication
still visits all selected generation rows, and restore materializes selected
maps up to the existing admission limits. A real deferred/Merkle capture path,
complete hash/generation-write/dirty-tail measures and the all-boundary fault
matrix remain required. These were not replaced by relaxed bounds. Other
core Usage/copied/foundation/mutation/relocation/recovery long keys remain open.

No local formatter/build/check/test/benchmark/browser ran: thermal telemetry
was absent both before and after. CI 40260160872 covers 06dd3097; CI
40260799072 and 40260804151 cover 9d023697 and were in progress when observed.
They are not final-revision evidence or passes. Root collects exact final CI
and formatting artifacts; a fresh independent review is required. Full backend
qualification remains zero. Full477/478/479/Hub313, migration/catch-up/fenced
cutover/qualified rollback and #380-before-MAIN remain open. No merge or live
operations were performed. Worker134 is a partial WIP delivery, outcome fail
against its complete mandatory scope, not an accepted or complete increment.
