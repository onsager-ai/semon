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
Parser 35 retains the exact last Claude usage observation offset alongside the
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
relationships rather than entering the stable description cache. Catalog version 8
and the semantic recipe bind this output.

Rules version 6 classifies the first waste per step in this order: orchestration
wait or checks pending, explicit tool failure, refill after compaction, redundant
read. Failures retain path-not-found, malformed-command, timeout and rejected
tool input (Tooling), rate/auth/network/server failure (External), checks pending
(Orchestration), merge conflict and other failure (Unmapped). Empty search,
verify failure and silent exit are independent non-waste signals; other sleeps
are Unmapped. Wait intervals retain their exact recorded endpoints independently
of the capped step duration. Session wait time
is the union of these intervals, so overlap and duplicate observations count once;
adjacent intervals join without changing the total. The summary exposes observed,
exclusive and shared interval unions, plus observed/exclusive/shared counts of
orchestration-wait steps without any exact interval. An orchestration call whose
result record is not decoded by the classifier (for example a Codex `wait_agent`
completed through another native record) takes the exact call and reply timestamps
the Event Index recorded for that call, as the session wait edges do. Missing
endpoints stay unknown.
Family wait values sum each physical session's union and can exceed family wall-clock
time. Waste time and list cost sum capped step `elapsed_ms` and priced
`list_cost_usd` for exclusive waste steps, grouped by waste kind and category;
unpriced or untimed steps increase `unknown_steps`. Shared waste remains visible
in the existing observed waste counts and general ownership totals, but is excluded
from waste cost/time maps to avoid assigning duplicated observations. Compound
failures need an identified diagnostic component; ambiguous failures remain Unmapped.

Failure evidence is typed and separate from read fingerprints. A Claude `Bash`
result carries its exit only as a first text line `Exit code N`; only that header
sets the exit, never a later body line, and Codex keeps its structured exit
field. Exit 124 or 143 is a timeout. `gh pr checks` (without `--watch`) exiting 8
is checks pending. Claude native Edit, Write, MultiEdit, NotebookEdit and Read
errors carrying the `<tool_use_error>` wrapper are rejected tool input; a native
Read error saying the file does not exist is path-not-found instead. A failing
`git merge`, `rebase`, `cherry-pick` or `pull` whose output has a `CONFLICT (`
line is a merge conflict; a successful result that prints the text is not.
A non-zero exit from a recognized verification command is no waste: it records
the per-step `verify_failure` signal. The allowlist is `cargo test|check|build|
clippy`, `cargo fmt --check`, `npm test`, `npm run test|typecheck|lint` (with an
optional leading `--prefix <dir>`), `pytest`, `go test`, `gh pr checks` exiting 1
and `gh run view --log-failed`. Exit 124, 126, 127 and 143, and a failed `cd`
diagnostic, stay failures. The status owner is the one non-`cd` pipeline of the
command, and a pipeline's status belongs to its last stage (no `pipefail`
assumption). Within a Codex exec batch, a part that exited 1 with an empty body,
with no individually attributed command, records the per-step `silent_exit`
signal instead of waste.

The failure grammar accepts general pipelines and the harmless redirects `2>&1`,
`1>&2`, `>/dev/null`, `2>/dev/null` and `&>/dev/null`. A redirect to any other
file, expansions, subshells, braces, heredocs and background jobs leave the
command unattributed. Such pipelines and redirects supply failure evidence only:
they never prove a read, so they break read history like any unknown command.
A compound or pipeline failure takes a subtype only when its diagnostic lines name
exactly one program of the allowlist `cat`, `rg`, `grep`, `ls`, `sed`, `head`,
`tail`, `cd`, `git`, `find` and that program appears once among the components;
otherwise it is other failure.

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
`waste_by_kind`, `waste_by_category`, `waste_cost_by_kind`,
`waste_cost_by_category`, `wait_time_ms`, `waits_without_interval`,
`levers_not_computed: ["TaskSpecification"]`, `empty_search_steps`,
`verify_failure_steps`, `silent_exit_steps`, `change_facts` and
`exclusive_cost_complete`. Each step total has `steps`, `list_cost_usd`,
`carry_cost_usd` and `elapsed_ms`; measurements preserve `{known, unknown_steps}`.
Each waste cost total has `steps`, `elapsed_ms` and `list_cost_usd` measurements;
only exclusive steps contribute. Wait time and missing-interval counts contain
`observed`, `exclusive` and `shared` values, following `OwnershipTotals`; intervals
are unioned separately for each ownership view. Absent native usage/projections
remain null. No thrash-window/step arrays enter these summaries. TaskSpecification
is a known uncomputed lever in rules version 6. `get_session_activity` steps carry
the matching booleans `empty_search`, `verify_failure` and `silent_exit`.

A nullable `family` summarizes only the exact existing parent chain, using
producer-side leaf reduction and bounded stored-summary refresh for independently
published children. It deduplicates exact shared keys in ownership/context
totals; exclusive ownership remains unresolved. Compactions, waste signals and
change counts sum physical-session observations. `member_sessions` and
`thrashed_sessions` report session counts. Family wait values and missing-interval
counts sum per-session ownership values; intervals are not unioned across physical
sessions. `total_step_time_ms` explicitly sums all capped physical-session step
durations, including copied observations; it can exceed wall-clock time. No
interleaved cross-session thrash is inferred.
The stored fold caps at 256 members, 4096 shared observations and 4 MiB metadata;
incomplete, cyclic or oversized families stay null. Children retain their own
steps and parents acquire no child rows.

Selecting distinct retained-history cohorts remains separate work. The retained latest observation
can only qualify against its own exact source vector; a different history cohort
is unavailable. Preserve the source and custody contracts in
[retained native projections](retained-native-projections.md) and
[native session source readers](session-source-reader.md).
