# Session activity analytics derivation

The internal pure derivation in `events/session_activity.rs` consumes normalized
native usage and compaction signals for one canonical session. The session model
publishes its typed results through `events/store/session_activity.rs`. Query
JSON and MCP expose compact Catalog summaries and bounded detail pages; ordinary
transcript and viewer payloads retain their existing contract.

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

Schema 41 stores a session activity header, ordered step rows keyed by
`(session_key, generation, ordinal)`, separate ordered compaction rows, compact summary metadata, and typed
change facts keyed by session, generation and stable identity.
Parser 34 retains the exact last Claude usage observation offset alongside the
existing message ID and assistant UUIDs. The producer uses its established file
merge order and copied-record ownership map; Codex reads only model-accepted
facts whose normalized source receipt matches the selected consumed ledger.
Ordinary header and detail publication shares the Catalog transaction and its
complete source membership, ledger, claim and generation checks. Focused activity
publication checks the selected physical sources' exact committed ledgers and
roles in its own transaction and saves the source recipe atomically with the
activity generation. It does not publish Catalog ownership or family summaries.
Metadata-only updates cannot retain activity for a different source cohort.

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
generation. Unknown prices remain unknown. Step arrays stay out of cached descriptions and session summaries. Compact
`activity_analytics` is published with Catalog/query summaries; it rebuilds after
relationships rather than entering the stable description cache. Catalog version 7
and the semantic recipe bind this output.

Rules version 5 classifies the first waste per step in this order: orchestration
wait, explicit tool failure, refill after compaction, redundant read. Failures
retain path-not-found and malformed-command (Tooling), rate/auth/network/server
failure (External), or other failure (Unmapped). Empty search is an independent
non-waste signal; other sleeps are Unmapped. Wait intervals retain their exact
recorded endpoints independently of the capped step duration. Missing endpoints
stay unknown. Compound failures need an identified diagnostic component; ambiguous
failures remain Unmapped.

The bounded classifier runs during the existing complete-line scan. Tool events
retain only allowlisted command tags, read-selection hashes, call/result record
digests, typed outcomes and sanitized change values. The file ledger binds these
observations to the consumed source receipt. A source-local read repeat requires
a completed prior read of the same selection and no intervening or overlapping
unknown mutation; compaction changes a repeated read to Context refill. Unknown
commands and mutating/agent calls break that proof. A terminal failed call
invalidates earlier reads but ends its overlap barrier at the recorded result;
running, background and undecodable results keep that barrier open.
Native Read selections,
`cat`, bounded `sed -n`/`head`/`tail`, and successful `rg`/`grep` selections
participate; working directories and literal absolute `cd` prefixes enter only their
selection hashes. Directory changes discard unrelated relative selections;
relative `cd` remains unknown. Literal `bash -c`/`bash -lc` and `sh` wrappers use the same
bounded grammar. A single read/search piped only through bounded `head`, `tail`,
`nl -ba`, or numeric `sed -n` selectors retains the complete pipeline fingerprint.
Other pipelines, expansions, redirects and arbitrary scripts stay unknown.
Native Grep/Glob no-match replies and exit-1 empty shell searches are independent
empty-search signals. Claude request-blob copies do not create tool
events: only assistant calls and user results own those observations.
Decoding accepts recognized
Codex result keys, text blocks and nested JSON strings, with a 256 KiB cumulative
string budget, eight decoding levels and bounded block counts. Code-mode envelope
ownership requires a literal call whose complete result is printed, or an exactly
printed native wait result. One literal exec call followed by `text(result.output)`
can supply activity output but cannot supply native process/session ownership.
Arbitrary scripts' printed JSON remains opaque and a terminal unknown script
ends its overlap barrier. A straight-line sequence of exactly printed literal
exec results also supplies failure-only diagnostics; it grants no read, change or
process ownership. Dynamic batches (`Promise.all`, `Promise.allSettled`) and
transformed outputs remain unclassified. Exit text in a successful result is not
failure evidence.

Operator qualification of the same seven-day window observed failure counts
before opaque-output handling, after that handling, and after literal-sequence
recovery respectively: OtherFailure 482 / 245 / 321, PathNotFound 196 / 53 / 81,
MalformedCommand 2 / 0 / 1, and ExternalFailure 4 / 0 / 0. These aggregate counts
show the conservative dynamic-batch coverage limit; they are not a false-positive
or recall estimate. The final recovery run covered 782 sessions and 56,069 steps;
all 39 sampled physical sessions paged, including child continuations. RedundantRead
and Refill each counted five, and empty search counted seven.

Change facts accept only recognized command/output pairs: commit branch/SHA,
GitHub push destination plus successful ref updates, and a GitHub pull-request
number from `gh pr create`. Literal arguments and setup compounds (`cd`,
`git add`, and at most one of each change command) are accepted. Literal quoted
PR-body heredocs, body files and `gh pr checks` without output-rewriting flags
also qualify; dynamic bodies and other output-producing commands remain ambiguous.
Root-commit headers are
recognized. Fetch blocks and updates into `origin/*` are ignored.
Facts retain a timestamp when recorded, source offset/digest, stable identity and
an optional exact native step ID; source ordering never invents that identity.
Claude assistant message IDs and explicit complete Codex
thread/session/turn/response tuples link only when that step survives qualified
usage derivation. Codex turn-only metadata, item IDs, ordinals and adjacent usage
records remain unlinked; multiple responses can belong to one turn.
No command text, stdout/stderr or result body is persisted. Facts publish and
advance pricing generations atomically with the step projection.

Cold local activity preparation discovers native identities in bounded directory
pages, then derives only the requested identity's sources and exact lineage
references. Claude identities come from native paths; Codex reads at most 64 KiB
of the first record. Parent and descendant event indexes are never decoded to
page a physical session. The focused activity transaction verifies exact source
ledgers and roles and publishes its source recipe with the existing activity
generation; Catalog relationships and family summaries retain their own producer. A direct cold call needs no prior listing. The request's unchanged
131,072-row/64 MiB accounting ceiling, deadline and cancellation cover the selected
derivation and page. Oversized selected components still refuse: a single
11,820-step source in operator qualification exceeds that unchanged budget, so
its step detail remains unavailable even though its summary is readable. A terminal
selected-read limit propagates without treating the durable Event Index as broken
or falling back to an in-memory scoped producer.

Query JSON and MCP expose the same `get_session_activity` tool. Select `kind`
`steps` (default) or `facts`, with `limit` 1–256 (default 100). Responses contain
`session: {id, harness, native_ids}`, `generation`, `kind`, `items`, and nullable
`next_cursor`. Pass that opaque string cursor and the same generation on the next
request. Steps sort by producer ordinal (known timestamp, source position and
exact identity); facts sort by stable identity with a keyset cursor. Selected
Catalog source membership, rules, prices and generation qualify in one SQLite
snapshot, with request-owned source witnesses. Mismatches return
`stale_projection`; unknown sessions and invalid arguments refuse explicitly.
No source paths, offsets, commands, raw bytes or output bodies leave these pages.

`get_session` and `list_sessions` add nullable `activity_analytics`. A supported
complete observation contains `rules_version`, `pricing_fingerprint`,
`totals: {observed, exclusive, shared}`, `by_band`, `compactions`, `thrash`,
`waste_by_kind`, `waste_by_category`, `empty_search_steps`, `change_facts` and
`exclusive_cost_complete`. Each step total has `steps`, `list_cost_usd`,
`carry_cost_usd` and `elapsed_ms`; measurements preserve `{known, unknown_steps}`.
Absent native usage/projections remain null. No thrash-window/step arrays enter
these summaries.

A nullable `family` summarizes only the exact existing parent chain, using
producer-side leaf reduction and bounded stored-summary refresh for independently
published children. It deduplicates exact shared keys in ownership/context
totals; exclusive ownership remains unresolved. Compactions, waste signals and
change counts sum physical-session observations. `member_sessions` and
`thrashed_sessions` report session counts. `total_step_time_ms` explicitly sums
all capped physical-session step durations, including copied observations; it
can exceed wall-clock time. No interleaved cross-session thrash is inferred.
The stored fold caps at 256 members, 4096 shared observations and 4 MiB metadata;
incomplete, cyclic or oversized families stay null. Children retain their own
steps and parents acquire no child rows.

Selecting distinct retained-history cohorts remains separate work. The retained latest observation
can only qualify against its own exact source vector; a different history cohort
is unavailable. Preserve the source and custody contracts in
[retained native projections](retained-native-projections.md) and
[native session source readers](session-source-reader.md).
