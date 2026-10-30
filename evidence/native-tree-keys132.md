# Worker132 Tree physical addressing WIP

Fresh private PG schema 8 adds namespace/domain-separated generated SHA-256
addresses for Tree usage work, Codex work, headers, usage-model messages,
copied message/record identities and repairs. Full heap bytes remain mandatory
identity witnesses. Schema VERSION, initial stamp and native manifest agree.
Warm qualification includes the generated columns, PKs, indexes and functions.
Header publication and lookup, repair mutations and usage-model conflict/read
paths use bounded addresses and full-value checks. Copied-identity triggers
qualify complete namespace and values on conflicts and deletion. Internal copied
identity traversal uses its physical tuple; output remains a BTreeMap.

This does NOT complete Worker132 or scope125. Models/tools (including the open
name index) retain variable keys. The current cryptographic witness remains
unchanged and UTF-8 ordered on both engines: replacing its order without a
coherent shared bounded design would invalidate existing integrity assertions.
A bounded order-preserving structure (or coherent current-format evolution on
both engines), whole-dimension capture/defer, all-family long-key/resource and
fault evidence remain required. There is no new witness version or old reader.
TreeCheckpoint callers require no API changes; Native130 must use the schema8
physical conflict keys when it adds work/repair writers. SQL is no Raw authority.

Tests run only in CI. No local formatter/build/test/benchmark or browser ran:
thermal telemetry is absent. Full #477/#478/#479/Hub313, source mutations,
relocation/recovery/foundation, migration/catch-up/fenced cutover/rollback and
#380-before-MAIN remain open. This is pushed WIP, not acceptance/readiness.
