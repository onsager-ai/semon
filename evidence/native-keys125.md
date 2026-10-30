# Native physical keys, Worker125 increment

The current native contract now addresses `files.path`, `files.cwd` and
`meta.key` with generated SHA-256 columns. Complete original bytea values stay
in the heap. Files retain their integer primary identity. Metadata conflicts
compare both full namespace and key; candidate reads and deletes explicitly
reject a digest/full-value mismatch. Native key addresses grant no Raw access.

The installer and warm verifier share the generated-column plan and validate
the exact expressions, column nullability, indexes and equality function. The
native contract fingerprint changes; unsupported earlier contracts are refused,
without any upgrade, reset or data deletion. The foundation initializer and
new Catalog families are outside this increment.

CI filter `pg_native_keys125` runs the real current SQLite scanner first, using
a filesystem path and parsed cwd over 2704 bytes with independently hashed
components. It compares all existing core fields, transfers actual producer
metadata, reopens PG, checks foreign namespace isolation, and exercises the
full-value collision function. It removes the known SQLite carrier before PG;
this is not complete process-wide SQLite open/access instrumentation.

This is a WIP increment, not acceptance of scope125 or #477. Existing Usage,
Tree, dependency, relocation and foundation variable-key indexes still need
their coherent port. Tree witness folding currently depends on original-key
order; replacing it with digest order is not authorized by this increment.
Full 0/256/2048 physical work measurements and whole-family qualification remain
required. No timing, resource, security, parser or public-factory gate changes.
