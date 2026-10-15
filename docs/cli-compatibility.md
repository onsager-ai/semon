# CLI evidence compatibility

Tracking: #235, #236–242. Baseline audited at main
`24e1932ef78238fdb13b5c1d11b61636695baf34` on 2026-10-03.
Copilot CLI persisted-state implementation follows #285 and the supported-version
contract in [copilot-cli.md](copilot-cli.md); OpenCode and Kiro remain deferred.
The delivery in #297/#298 was reconciled through main #294 and the landed #297
on 2026-10-04. The bounded supported contract is documented below; issue
acceptance and final check evidence are tracked in #235.

Native session, control, mirror, namespace, transcript and usage contracts remain
supported. Retired Canonical Trace Store capture and replay are outside this
contract; see the [first-release product boundary](design/session-foundation-retirement.md).

## Evidence contract

A source identity includes machine, harness, native session/thread identity,
source generation and record identity. Call identities are scoped to their
session and generation. Only an explicit native call ID joins a result to a
call. Identical tool names, arguments, working directories, timestamps and text
are not relationship evidence. A missing result stays unmatched; a missing
approval, usage field, lifecycle transition or parent remains unknown.

Record-chain parents, logical fork parents, physical inherited history bases
and subagent relationships are distinct facts. Copied prefixes are context,
not new work or new usage. Usage needs field presence, source provenance and
model attribution; zero is different from absent. Reported monetary cost and
Semon price estimates must remain distinguishable.

Retain complete native records privately, including unknown and malformed
records. Commit a cursor only after the corresponding raw evidence is durable.
Partial framing cannot advance the durable cursor. A derived structural index
must be rebuildable without storing transcript text. Native homes remain
read-only; tests use explicit temporary roots and never discover personal
sessions. Remote evidence is a deliberately redacted viewing copy, not a
recovery source.

## Baseline and oracle

Native fixtures preserve Claude multiblock records, explicit parentUuid ancestry,
principal/subagent identity, Codex item-stream dispatch and exact native call/result
relationships. Existing linking and mirror behavior form the parser baseline.

`tests/fixtures/compatibility/v1/manifest.json` describes the checked-in synthetic
source fixtures and their hashes. Harness build/version are explicitly unknown;
these fixtures establish parser behavior and contain no real sessions. Current
native parser tests consume these inputs directly. Retired Store capture and raw
carrier replay do not qualify the native session product.

The native baseline is supplemented by the native/mock and source-shaped corpora
described below: archived discovery, continuation/fork/copied prefixes,
compaction, denial/cancellation, concurrent calls and machine namespaces. The
replacement and source-custody tests verify current-revision replay while retaining
original native generations; deletion does not authorize pruning retained evidence.
Parser fixtures alone do not establish Viewer or release acceptance.
The final copied-usage repair in #297 and persisted Copilot delivery in #298 add
full-model, transcript, restart/rebuild and redacted mirror acceptance coverage.

## Initial Copilot observation

An isolated Linux x64 runtime probe of `@github/copilot` 1.0.91 (package build
`216810c5`) using an offline localhost mock provider produced
`COPILOT_HOME/session-state/<uuid>/events.jsonl`. `session.start` declares
`version: 1`, producer and Copilot version. Persisted events have explicit event
IDs and parent IDs; user and assistant messages additionally have message IDs.
The home also contains workspace YAML and a SQLite session store with WAL/SHM.
Do not copy live database/WAL files into mirrors.

The headless stdout stream contains ephemeral model/delta/idle events absent
from saved events. Pinned 1.0.90 and 1.0.91 probes now cover concurrent tool
completion, same-session resume and explicit shell denial. Saved shutdown
metrics accumulate across resume under synthetic mock usage; their native billing
units remain unknown. See [the detailed probe](copilot-probe.md) for fixture
provenance, interactive persistence, cancellation, manual compaction and the
bounded go/no-go capability contract. Logical forks, persisted approval IDs,
native mutation/retention and billing remain unknown. The bounded production
adapter in #298 follows this probe contract; see [copilot-cli.md](copilot-cli.md)
for collection, browsing, mirror behavior and the supported matrix.

## Versioned Claude and Codex native baselines

The `claude-2.1.288` and `codex-0.159.0-alpha.3` fixture directories contain
transformed recordings from isolated Linux headless executions against local
synthetic model providers. Each manifest identifies the executable hash, native
version, storage shape, original and transformed hashes, transformations and
unknown capabilities. These are runtime persistence observations; synthetic
usage counts do not establish billing correctness or stable-version support.

Claude's baseline passes the full cold/restarted/retained-byte occurrence oracle.
Its attachment and prompt-state content is removed while retaining record types
and structural relationships. Codex's baseline establishes native paginated
history and mixed legacy/item records. It now passes the same full occurrence
and retained-byte oracle, including early mirror ingestion before item arrival.
Neither recording establishes tools, approvals, cancellation, compaction or
fork semantics. The offline Python validator checks provenance and file hashes;
CI does not launch either harness or access personal native homes.

## Native home overrides

Capture CLI defaults and session discovery honor nonempty `CODEX_HOME` and
`CLAUDE_CONFIG_DIR`. Explicit input flags still take precedence. Codex sessions
and history live beneath its configured root. Claude projects live beneath its
configured root, and custom-root settings use `<CLAUDE_CONFIG_DIR>/.claude.json`,
as observed in the isolated 2.1.288 probe; default settings remain
`~/.claude.json`. Empty overrides retain the ordinary home defaults.

Fixture-backed discovery tests resolve only disposable roots and verify native
transcripts and settings remain byte-identical after collection. These native
roots do not relocate Semon's own state or derived cache.

## Claude resume and copied fork context

The pinned 2.1.288 lifecycle probe retains initial, resumed and forked native
transcript snapshots under a disposable `CLAUDE_CONFIG_DIR`. Parent resume
preserves its consumed bytes. The fork rewrites `sessionId` while copying the
four existing user/assistant UUIDs, parent UUIDs and API message identities.
These exact identities establish inherited context; no explicit logical
parent-session field was observed, so they do not establish that lineage.

The localhost mock supplies five input and three output tokens per request.
Cost-state model usage accumulates 5/3, 10/6 and 15/9; the fork includes inherited
context. These are native snapshots of synthetic accounting, not invoices or
newly attributable child consumption. Native records preserve the physical
session namespace. Current source identity and generation checks govern viewing
and query interpretation; the snapshots do not establish custody for retired
Store records.

### Native Claude tools and model switch

`claude-2.1.288/tools` exercises Read, Edit, successful Bash and Bash exit 1
against disposable files with localhost synthetic model replies. Tool request
IDs join exact persisted result IDs. Three streamed assistant blocks share one
API message ID but retain distinct record UUIDs. They must all remain visible;
an API message ID alone is not a block identity.

The batch executes serially in this native run. Overlapping/reversed same-name
call decoding remains covered by the separately labeled source-shaped fixture.
The initial run uses `permission-mode default` with an explicit fixture tool
allowlist; resume and fork use native default `auto` and perform no new tools.
The allowlist does not establish a persisted approval ID.
Resume appends under the same physical session and switches Sonnet to Haiku;
fork copies these records into a distinct physical session without an explicit
logical parent edge. The cold/restart/raw oracle compares six, eight and ten
semantic occurrences respectively. Tool results remain in the forensic region
under the existing Claude projection contract. Usage is mock evidence, and
shared streamed API message IDs must not be summed as independent requests.

Reproduce with `tests/spikes/claude-tools.py` using the pinned binary and a new
private output directory. The manifest pins script content, binary and original
source hashes, transformations, and unsupported scenarios.

## Source-backed Codex archive discovery

Codex `0.159.0-alpha.3` source at commit
`3b01b36fa5eb96ba82a776bd3c2fc57f8969181f` defines `archived_sessions` and
archives owned rollout files by moving them there. The source hashes and the
simulated-move fixture limitations are recorded in
`codex-0.159.0-alpha.3/archive-manifest.json`. This is source-backed evidence;
the native archive command has not been executed in the test.

Semon discovers plain JSONL in both `CODEX_HOME/sessions` and
`CODEX_HOME/archived_sessions` for session lists, model building, transcript
lookup and refresh. These archived files are part of the existing explicit
viewing-input allowlist. Input copies with recorded machine facts preserve
model and transcript parity. Optional recorded facts carry a content-free
current Codex source inventory, so active/archive transitions select the
current paths while retaining older mirrored bytes. End-to-end push tests cover
both directions with deliberate redaction. An absent inventory in older facts
remains unknown; an explicit empty inventory selects no current sources.
Received archive roots behind symbolic links are refused, and native source
bytes remain read-only.

Compressed native rollouts are outside this supported subset. Current session
discovery includes plain archived files under the configured Codex home; this
fixture does not establish native archive lifecycle or logical lineage. Native source selection is recorded by the shared Event Index and Catalog.

## Codex native fork boundaries and child usage

The pinned `0.159.0-alpha.3` probe executes a parent against a synthetic localhost
provider, forks through native app-server `thread/fork` with `excludeTurns=true`,
and completes a child-owned turn. Its three hash-addressed snapshots preserve
native logical `forked_from_id` and exclusive ordinal separately from physical
`history_base`. The fresh fork references the parent prefix without copying its
messages into the child JSONL. Pinned source comments allow logical and physical
bases to diverge after revert; this probe does not execute that scenario.

The fork itself makes no model request and has no child usage observation.
After its own turn, `token_usage_record.usage` reports fresh mock usage of 5 input
and 3 output tokens, while `thread_token_usage` and the cumulative `token_count`
report 10/6 including inherited parent context. These are distinct observations,
not billing evidence. Live mock-provider checks that inherited context was sent
are recorded separately from persisted facts. Native reader fixtures preserve
the parent, empty child and completed child observations across index restart.
The corpus supplies the native evidence for the viewer interpretation described
below; the probe itself does not claim decoder support.

The viewer now exposes machine-scoped native root, logical fork and physical
prefix fields separately as optional `codex_history` metadata. A physical base
alone does not invent a logical parent, and forks do not become spawned-agent
edges or merge their child-owned turns with the parent. Missing fields stay
absent, including exclusive ordinals; an observed zero remains zero.

For the pinned native per-response format, exact thread/session/turn/response
identities select fresh usage independently of the cumulative `token_count`
channel. Repeated exact reports count once, and the selected accounting survives
index restart. A fork's cumulative-only or incomplete report is insufficient
for fresh usage and produces no model-usage row. Legacy non-fork files retain
existing cumulative-delta behavior. Complete per-response observations take
precedence over legacy counters; mixed-format histories without complete native
reports are not claimed as an accepted usage baseline. This remains usage
telemetry, not an invoice. The native Event Index derives this accounting from
original source evidence and preserves separately retained reported-run snapshots.

The CLI session tree uses the same complete native per-response decoder. Private
structural summaries deduplicate exact request identities and keep cumulative fork
context out of fresh request counts. Physical inherited history does not create
logical spawn edges. Native logs remain intact.

### Colliding native identifiers

Native session identifiers are scoped to their harness and machine. The viewer
keeps existing bare keys for unambiguous Claude/Codex inputs. When both harnesses
use the same identifier, their viewer keys are qualified with `claude:` or
`codex:`; extra leading colons avoid conflicts with recorded native identifiers.
Source identifiers remain unchanged. Exact native parent and process lookups use
the harness namespace rather than the served key. Tool lookups are scoped to the
modeled session; a Claude child first looks for its exact call under its
recorded storage ancestor. Nested agents can share that ancestor's directory;
when it has no matching call, only a unique exact Claude call elsewhere is
accepted. Repeated ambiguous IDs remain unlinked.

The versioned `namespace-*` fixtures are source-shaped synthetic records, with
unknown native versions. They demonstrate separate sessions with the same native
ID and call ID, source-local results, and generation-bound native reads.
This is no claim about native UUID collision frequency or invented lineage.

## Copied Claude usage accounting

The pinned lifecycle fixture has two physical sessions. Summing their observed
usage produces 25 input / 15 output tokens; the distinct assistant request
records carry 15 / 9. The parent and fork share two exact assistant record UUIDs
and API message IDs (10 / 6), while only the fork contains the third request
(5 / 3). API IDs alone, similar content, timestamps and filenames do not
establish copying. Multiple streamed blocks of one API request still count once.

When exact records appear in different modeled Claude sessions on one machine,
Semon excludes their shared usage from the primary token and model-token fields.
Those fields describe **exclusive observations**, not a complete fresh-work total.
Optional `claude_usage` evidence preserves `observed`, `exclusive`, shared usage
under stable hashed request groups, and its recorded model attribution. `fresh`
and `shared_owner` are explicitly null: the recordings do not establish original
branch ownership. Do not add session observations to obtain fresh usage, assign
shared requests to the first session, or infer a logical fork parent.

For the lifecycle fixture the parent has no exclusive observation, the fork has
5 / 3, and the shared pool has 10 / 6 once. Zero exclusive observations is not
proof of zero fresh work. The original 10 / 6 and 15 / 9 remain inspectable.
Conflicting reports remain separate source observations; grouping does not resolve
their amount or ownership. A missing record UUID cannot establish copying.
Copies on different machines are not grouped into an invented owner.

The viewer, CLI tree, query/MCP and encrypted remote summaries carry the same
accounting. Estimated complete session costs remain unknown for copied history;
reported-run snapshots remain source evidence. Analytics omits shared usage from
priced bars and labels their incompleteness, preserving unknown aggregate costs
on affected days. Unaffected sessions and JSON field types are preserved. New
fields are optional. The text-free viewer cache is schema 3/parser 21; local tree
cache 5 and encrypted summary cache 3 rebuild older structural summaries without
changing raw bytes, canonical traces or retained reported-run snapshots.

Regression evidence uses the already hash-manifested native mock lifecycle
recordings, not new personal-session data. `native_copied_usage_view_agrees_cold_incremental_and_retained_custody_rebuild`
checks full session evidence over cold ingestion, half-frame interruption plus
cursor/SQLite restart, and actual current-source raw custody replay. The sessions
model, local tree and encrypted remote tests verify unknown ownership, distinct
physical sessions, exact identities and restart behavior. Identity-negative tests
reject an API ID without a matching UUID, a UUID without the same API ID, and
cross-machine attribution. Existing continuation grouping stays unchanged.

These are synthetic provider usage observations in native Linux Claude Code
2.1.288 persistence, not billing assertions. Native logical fork lineage and
unobserved ownership remain unknown. This repair supplies acceptance evidence
for #236/#237; production Copilot acceptance still requires #239–242.
