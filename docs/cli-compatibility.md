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

The remaining acceptance corpus must cover item-stream arrival after early
legacy mirror ingestion, replacement/interior rewrites, archived discovery,
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
from the saved events. The saved file includes `session.shutdown` and per-model
metrics, but this single no-token mock response cannot establish native usage
accounting. This is native CLI persistence with a mock model, not a paid/native
model validation. Interactive, prior releases, concurrent tools, resume/fork,
approvals, compaction and mutation behavior remain unverified. #238 is not yet
accepted and the adapter implementation gate remains open.
