# Session activity analytics derivation

The internal pure derivation in `events/session_activity.rs` consumes normalized
native usage and compaction signals for one canonical session. The session model
publishes its typed results through `events/store/session_activity.rs`; it adds
no Query, MCP, Catalog JSON or transcript fields.

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

Schema 35 stores a session activity header, ordered step rows keyed by
`(session_key, generation, ordinal)`, and separate ordered compaction rows.
Parser 30 retains the exact last Claude usage observation offset alongside the
existing message ID and assistant UUIDs. The producer uses its established file
merge order and copied-record ownership map; Codex reads only model-accepted
facts whose normalized source receipt matches the selected consumed ledger.
Header and detail publication shares the Catalog transaction and its complete
source membership, ledger, claim and generation checks. Metadata-only updates
cannot retain activity for a different source cohort.

The header binds the source-vector generation, activity rules version, pricing
fingerprint and row counts. The internal reader returns at most 256 ordered
steps, checks the selected source cohort and rejects stale continuations, rules
or prices. It belongs inside the caller's selection transaction. Activity rules
changes enqueue current source owners through the existing Catalog invalidation
path. Changed source generations derive again during normal model publication.
Recognized obsolete indexes rebuild from supported sources under the existing
refusal/rebuild policy; there is no migration or old-store reader.

Each step saves typed pricing inputs without native bodies. The existing pricing
producer reprices at most 128 activity rows per transaction across restart,
including retained observations with unavailable native bytes. Step stamps prevent
mixed-price pages; the final batch advances the header fingerprint and activity
generation. Unknown prices remain unknown. No activity arrays or aggregates are
added to the cached description or served session model.

Family roll-ups, waste rules, change facts, public reads and selecting distinct
retained-history cohorts remain separate work. The retained latest observation
can only qualify against its own exact source vector; a different history cohort
is unavailable. Preserve the source and custody contracts in
[retained native projections](retained-native-projections.md) and
[native session source readers](session-source-reader.md).
