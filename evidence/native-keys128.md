# Current native key contract increment

Fresh PG schema 7 uses semon_pg_native_digest(namespace, value, domain) for the
private native Usage generation/identity/dimension and dependency families.
Its preimage is SHA256(namespace) || SHA256(UTF8(domain)) || complete value.
Digests are physical addresses; full namespace and heap bytes establish equality.
Warm transaction admission validates the exact generated-column/index/function
contract. Unsupported prior contracts refuse without upgrade or data reset.

Usage current receipt reads preflight stored length and charge selected SQL
rows/bytes before returning decoded metadata. pg_native_keys128 registers real
SQLite producer / PG long-key parity, namespace, restart, wrong-witness rollback,
byte budget and selected physical EXPLAIN noise tests. Their results remain
pending; authored tests alone are not qualification. Existing keys125 is retained.

This is partial: core copied usage, Tree witness traversal, remaining path and
foundation families, full resource/cancellation/no-SQLite evidence and the public
backend are unfinished. See worker128-remaining-keys-audit.md for exact revisions
and historical failures. Full #477 and the #380-before-MAIN gate remain open.
