# CLI evidence compatibility

Tracking: #235, #236–242. Baseline audited at main
`24e1932ef78238fdb13b5c1d11b61636695baf34` on 2026-10-03.
Copilot CLI is the next adapter; OpenCode and Kiro remain deferred.

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

Existing implementations already preserve Claude multiblock records, explicit
parentUuid ancestry, principal/subagent identity, Codex item-stream dispatch,
raw-only records and content-addressed trace identity. Existing exact linking
and mirror functionality are baseline work, not new adapter requirements.

`tests/fixtures/compatibility/v1/manifest.json` describes the checked-in
source-shaped fixtures and their hashes. Harness build/version are explicitly
unknown: these fixtures establish parser behavior, not native-runtime support.
They contain no real sessions. The Rust `compatibility_*` tests compare cold
capture, append capture with cursor and SQLite restart at every complete and
half-frame boundary, and replay of the actual retained raw bytes. They compare
full occurrence records and verify byte-for-byte retention of malformed and
unknown complete lines. The fixtures cover reversed results for overlapping
same-name calls; this structural-store oracle does not establish full viewer
transcript or usage correctness.

The remaining acceptance corpus must cover archived discovery,
continuation/fork/copied prefixes, compaction, denial/cancellation, concurrent
sessions and machine namespaces. Existing truncation tests compare surviving
occurrences only; they do not establish final-generation deletion semantics.
Passing these initial tests does not close #236 or #237.

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
provenance and observed boundaries. Interactive execution, logical forks,
approval identities, compaction and mutation behavior remain unverified. #238
is not yet accepted and the adapter implementation gate remains open.

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

## Capture replacement detection

Claude and Codex capture cursors store a constant-size SHA-256 digest of the
complete consumed prefix. Before resuming, capture verifies that prefix;
replacement, interior mutation or truncation resets parser context and line
ordinals, then replays the current source. Older cursors without a digest replay
once. Batch commits extend the digest only with complete records after their raw
capture succeeds. No transcript text is added to cursor state and no old raw
record is deleted.

This deliberately reads the consumed prefix on each capture invocation, including
idle invocations. It prioritizes verified replacement handling over the former
size-only shortcut; an idle-read optimization needs equally strong source-change
evidence. It does not establish rebuild selection across retained generations.
Those remain #237 work, so this bounded repair
does not establish the complete historical/incremental/retained-evidence oracle.

Capture records local source ownership of projected occurrence rows alongside
its raw/semantic transaction. When verified source bytes change, it retires that
source's occurrence projection before replay; another recorded owner keeps its
shared rows. This reconciles metadata/malformed replacement, fewer Claude blocks,
truncation tails and changed Codex session identity without deleting canonical
traces, raw records or raw-to-trace links. Source keys are resolved local paths;
they are excluded from transferable identity and ordinary trace/log results.
The version-6 store migration adds only this structural ownership index.

Pre-upgrade occurrence rows have no reliable file ownership. They remain observed
memory until independently attributable; the migration never guesses ownership
from session name, timing or text. A replay claims only rows actually projected
by that source. Full parity for arbitrary pre-upgrade stale rows and generation
selection when rebuilding from all retained raw bytes remain unresolved release
gates, not a native support claim.

When the first complete Codex `item_completed` record arrives after an earlier
legacy `response_item` message was captured, dispatch now resets that source's
owned projection and replays with the item stream selected for the whole file.
An incomplete item frame does not switch dispatch. The source-shaped early-item
fixture and native paginated alpha fixture both pass full cold/restarted/rebuilt
occurrence comparisons and exact retention at every half-frame and complete-line
boundary. This does not retire unowned pre-upgrade observations or establish
native billing, tools, approval or fork support for the alpha release.

The sessions viewer ledger also verifies the complete consumed prefix when file
stat changes, replacing its former two-window shortcut. Its versioned, text-free
cache rebuilds once on upgrade; idle scans with unchanged stat retain their
existing shortcut. Changed-file scans hash the old prefix before resuming and
the consumed prefix after parsing. This adds reads on growing or touched files,
but does not reparse unchanged history. Explicit native tool IDs and byte-offset
metadata are rebuilt from the source after an interior mutation. A writer that
preserves the entire file stat while mutating bytes is outside this shortcut's
change-detection contract.
