# Session activity analytics derivation

The first implementation slice is an internal pure derivation in
`events/session_activity.rs`. It consumes normalized native usage and compaction
signals for one canonical session. It does not publish storage rows or expose
Query, MCP, Catalog or transcript fields.

The producer supplies the existing Claude usage merge order and exact
`claude_usage::shared` ownership map. One message ID is one observed step within
the canonical session. Shared records retain their exact shared key and remain
outside exclusive cost and time totals; no original owner is inferred. Codex
steps require the full native thread/session/turn/response tuple, the existing
usage decoder and an `accepted` model outcome. Duplicate, conflict and
unavailable outcomes do not contribute steps. A source receipt must already be
qualified by the caller; the pure function does not grant source authority.

Context is input plus cache read plus cache write. Bands are below 50,000,
50,000 through 150,000 inclusive, and above 150,000. One immutable pricing
snapshot supplies both list cost and the priced cache-read component (carry
cost). The rules version and pricing fingerprint remain independent. Missing
rates stay unknown, including when another component of that step is priced.
Zero cache-read tokens have a known zero carry cost.

Steps sort by timestamp, then stable source identity and offset, then native
step identity. Missing timestamps sort last in source order. The elapsed value
is the nonnegative interval to the next step, capped at one hour; either missing
timestamp or no successor leaves it unknown. Aggregate measured sums carry an
explicit unknown-step count, rather than treating missing values as zero.
Observed, exclusive and shared aggregates remain separate, including by band.

Compactions come only from normalized compact signals. Repeated observations
of an exact source offset count once. A timestamped signal belongs to the first
following step in the same ordering, or the last timestamped step if trailing.
Without a signal timestamp, same-source offsets establish placement; absent
same-source steps leave it unplaced. Counts and timestamps survive even without
steps. At least three compactions attached to any three consecutive steps flag
thrash and retain those three step identities. Fewer than three steps cannot
form a thrash window. Derive each physical session independently; interleaving
parent and child steps must never create a family thrash window.

Follow-up work must wire the exact selected source cohort into a versioned
SQLite activity projection, publish it atomically with the Catalog generation,
and qualify current and retained-history reads against their respective source
generations. That work owns the schema bump, rules-version rebuild and separate
pricing invalidation, append/restart tests, and bounded storage reads. The pure
slice changes neither parser nor schema versions. Family roll-ups, waste rules,
change facts and public reads remain separate work. Preserve the source and
custody contracts in [retained native projections](retained-native-projections.md)
and [native session source readers](session-source-reader.md).
