# GitHub Copilot CLI persisted sessions

Tracking: #235, #239–242. The implementation follows the native evidence and
limitations in [the pinned probe contract](copilot-probe.md). It reads saved
state; it does not execute, resume, recover or control Copilot.

| Supported source | Platform | Saved schema | Evidence |
|---|---|---|---|
| CLI 1.0.90, build `ccf052b4` | Linux x86_64 | `session.start.data.version = 1` | Hash-manifested native/mock headless, interactive, lifecycle, boundaries and child-task recordings |
| CLI 1.0.91, build `216810c5` | Linux x86_64 | Same | Same fixture families and ingestion/transport acceptance tests |

Other versions and schemas produce an unsupported-format diagnostic. An SDK
schema alone is insufficient to add support. Windows/macOS, IDE/cloud sessions,
execution/recovery and other adapters are outside this release slice.

## Read and collect

`COPILOT_HOME` overrides `~/.copilot`; `--copilot-home` overrides that default.
Only `session-state/<sessionId>/events.jsonl` is authoritative. The first complete
record must be `session.start`, with a supported version/schema and a session ID
matching its directory. Links and non-regular files are rejected. SQLite/WAL,
workspace YAML, checkpoint Markdown, rewind snapshots and debug stores are never
mirrored or used to infer relationships.

Build both tools from the same checkout:

```sh
cargo build --release --locked -p semon-store --bin semon
cargo build --release --locked -p semon-copilot
semon-copilot --copilot-home /path/to/copilot
semon sessions --copilot-home /path/to/copilot --all
semon sessions --copilot-home /path/to/copilot --serve
```

A one-shot collection drains complete records. Continuous collection uses
`semon-copilot --watch`, polling every two seconds. The default store is the
shared `$XDG_DATA_HOME/semon/traces.sqlite3` (or
`~/.local/share/semon/traces.sqlite3`), and the cursor lives at
`$XDG_STATE_HOME/semon/copilot-capture.json` (or the corresponding
`~/.local/state` path). Explicit `--store` and `--state` paths are supported;
outputs inside the native home are refused. An exclusive cursor lock prevents
concurrent collectors. No native files are created, changed or removed.

Full complete frames, including unknown and malformed records after a validated
header, enter private forensic custody before the cursor advances. Partial frames
remain unconsumed. Cursor writes are atomic, owner-only and synced. Prefix
checksums, inode/device and change metadata detect ordinary truncation,
replacement, equal-size interior mutation and longer-prefix replacement.
Source-owned projections are retired on replacement; older raw generations remain
retained. Missing sources do not prune retained data. Lost cursors replay safely.
An unchanged source uses metadata checks without prefix hashing or JSON replay;
restart and changed-source verification intentionally read the consumed prefix.
Polling is a latency target, not proof that a process is alive or that a source
which has not yet flushed a record has completed work.

`semon sessions --model-json`, the query/MCP session tools, modern paged
transcripts, legacy transcript/entry routes and multi-machine views share the
same native source identities. Existing Claude/Codex JSON field types and
interfaces are preserved. Optional `copilot` evidence carries schema/release,
the latest usage snapshot and its exact event ID/byte offset/timestamp. Modern
transcript entries carry optional native event/message/interaction/turn and
parent-tool IDs, plus matched result event ID, protocol success and shell exit
code. Text and arguments are read back by bounded source offsets; the derived
SQLite index retains structural facts rather than transcript content.

## Facts and unknowns

Assistant `toolRequests[].toolCallId` joins only an unambiguous completion with
that exact persisted ID in the same session/source generation. Same names,
arguments, order, time and working directory never join calls. Replayed event IDs
count once. Repeated distinct requests with the same call ID stay unresolved.
Orphan starts/results are retained raw; they do not manufacture a trajectory.
A missing result is shown with an unknown outcome, not as failure or live work.
For shell calls, native `data.shellExecution.exitCode` determines command outcome;
protocol `success: true` with exit code 1 is a failed command. Denials remain
observed denials. Successful approvals remain unknown.

`abort` lacks a tool identity and cannot complete pending tools. Compaction
annotates the transcript and does not discard historical evidence. Child-task
records and child messages retain native `parentToolCallId`/
`originatingMessageId` in the physical session. They do not invent another agent
session, a spawn edge or a logical fork parent. Prompt authorship remains unknown
unless the native record supplies positive evidence.

Shutdown `modelMetrics` replaces the previous cumulative per-model usage snapshot;
11/3 followed by 22/6 means 22/6, not 33/9. Cache and reasoning categories remain
separate observations. Child `agentMetrics` and compaction usage are not added
again. Missing or incomplete metrics yield a null snapshot. Snapshot totals do
not establish fresh branch work, USD billing or original copied-history ownership.
Complete Copilot costs remain unknown, including in analytics; native request
cost units are not converted to dollars. Consumers needing usage presence should
read `copilot.usage`, rather than interpreting legacy numeric zero as an observed
zero. The snapshot's legacy `Tokens.total` is derived input plus output, not a
separately reported native billing field.

## Redacted viewing copies

Use the existing mirror transport, with the same explicit native home:

```sh
semon push --to https://receiver.example --token-file /private/mirror-token \
  --copilot-home /path/to/copilot --watch
semon sessions --machines /path/to/received --no-local --serve
```

Only the exact event-file allowlist crosses the wire. Existing best-effort,
length-preserving secret-shape redaction runs on source bytes before transport
encoding; local originals remain private and unchanged. Unknown secret shapes
are not guaranteed redacted. Received data is a viewing copy, not a recovery
source. Native identities are scoped by machine and harness; no cross-machine
ownership is inferred. Claude/Codex encrypted relay and restore behavior remains
covered by its existing regressions; this slice adds Copilot to the persisted-home
mirror, not an encrypted native-session execution adapter.

## Acceptance evidence

`semon-copilot` tests exercise both releases across seven native fixture families,
assert exact call/result IDs, protocol versus shell outcomes, latest shutdown
snapshot accounting, unknown cancellation/lineage/approvals, unchanged originals,
and all three ingestion paths: cold, incremental half-frame with restart, and
actual retained source-custody rebuild. Replacement, interior mutation, lost
cursor, missing source, unsupported version and strict allowlist/link tests use
isolated disposable homes. Source-shaped negative records remain synthetic.

The HTTP mirror regression commits an append and loses its acknowledgement,
restarts the sender, interrupts a frame, replaces its source and deliberately
redacts a secret. The local deliberately redacted view and received view agree
without normalizing away model/transcript fields. Existing fixture provenance,
Rust, UI, viewer security, browser, pixel, performance and transport gates remain
required. `copilot` is registered in the existing navigation browser lane with
phone/desktop, light/dark, command failure, missing result, child-task browsing,
unknown cost and live resume/restart coverage. No baseline or threshold was
relaxed. Passing probes alone is not release acceptance; PR/check evidence and
remaining limitations are recorded on #235 and the child issues.

The synthetic collection regression also measures cold capture, metadata-only idle
passes and append collection with four isolated sessions and 1,004 complete initial
records. Run `cargo test --locked -p semon-copilot
synthetic_collection_measures_idle_and_growing_sources -- --nocapture` (on one
line) to reproduce the measurements. One local Linux run on 2026-10-04 measured
639.47 ms cold capture, 0.051 ms mean idle pass over 20 passes, and 19.50 ms to
collect four appends. These measurements depend on the host and workload; they
are not performance gates or service guarantees. Watch adds a polling delay of
up to two seconds before collection begins. The regression verifies an unchanged
idle cursor and recovery when raw evidence is durable but the checkpoint still
contains the previous offset.
