# Copilot CLI persistence capability contract

Tracking: #238 under #235. Probe result: **go for the bounded version-1 adapter
contract below**. The persisted-state adapter in #298 implements this bounded
contract after the compatibility work in #297. This document records probe
evidence; [copilot-cli.md](copilot-cli.md) owns the implemented supported matrix
and collection, browsing and redacted viewing behavior.

Pinned runtime: npm `@github/copilot` / `@github/copilot-linux-x64` 1.0.91,
package build `216810c5`, Linux x86_64. The same offline probe also passes
on prior 1.0.90, using the same version-1 saved representation. Manifested binary hash and transformed
recording are in `tests/fixtures/compatibility/copilot-1.0.91/`.

To reproduce without installing into a native home, install the pinned package
in a disposable directory, then run:

```sh
python tests/spikes/copilot-offline.py \
  --copilot /path/to/disposable/node_modules/@github/copilot-linux-x64/copilot \
  --output /path/to/new/disposable-probe
```

The script requires a new output directory, sets `COPILOT_HOME` and offline
mode, constructs a minimal allowlist of PATH/locale/timezone/terminal settings, uses
private temporary paths, and disables custom
instructions, builtin MCP, remote export/control and updates, and uses only a
localhost mock provider. The two shell commands print fixture strings; one writes a marker inside the
disposable cwd and returns exit status 1. The other waits for that marker before
finishing. The probe asserts reversed completion from saved event order and
exact IDs, rather than inferring any relationship from timing. No parent HOME or credential variables are forwarded; COPILOT_HOME selects
the disposable native state root. The parent runtime environment stays intact.
No personal session or credential file is mounted.
The script checks pinned version, exact persisted tool identities and exit
codes, and exclusion of ephemeral events from the saved log.

## Observed saved representation

| Evidence | Observed location/fields | Confidence and limitation |
| --- | --- | --- |
| Home override | `COPILOT_HOME`, default `~/.copilot` in CLI help | Help plus isolated runtime |
| Session log | `session-state/<sessionId>/events.jsonl` | Native headless persistence with mock model |
| Format | `session.start.data.version = 1`, `copilotVersion = 1.0.91` | Pinned 1.0.90 and 1.0.91 only |
| Record identity | `id`, `parentId`, `timestamp` | Event chain; does not establish logical session lineage |
| Messages | `data.messageId`, assistant `originatingMessageId` | Exact IDs; no text-similarity linking |
| Tools | start `toolCallId`, `toolName`, `arguments`; complete same `toolCallId`, `result.content`, `shellExecution.exitCode` | Two overlapping same-name calls, completion order reversed in recording |
| Error semantics | `success = true` alongside `shellExecution.exitCode = 1` | Tool protocol success differs from command success |
| Shutdown | `session.shutdown`, per-model `modelMetrics` | Later lifecycle/task probes establish cumulative mock counts; no native billing claim |
| Other storage | workspace YAML, SQLite session store plus WAL/SHM, logs, rewind snapshots | Not approved mirror inputs; no DB/WAL copying |
| Ephemeral stream | stdout model calls, deltas, idle and background changes absent in saved log | Live observations cannot be prerequisites for historical browsing |

The fixture removes the native system prompt and replaces the disposable cwd
with `/fixture/probe`. Its manifest distinguishes the original raw hash from the
transformed fixture hash. It is a transformed runtime recording, not an exact
copy of retained native evidence. It contains no real user session.

## Supported boundary and unknown capabilities

Releases before 1.0.90, logical forks, unprobed subagent execution modes,
native billing, explicit approval identities, native truncation, mutable records,
retention/deletion and non-Linux platforms remain unverified.
The parent event chain and `parentAgentTaskId` must not be interpreted as a
logical parent session without additional source evidence. The supported subset is pinned below. A missing tool completion remains
unmatched: cancellation does not justify a complete-trajectory promise.

Repeat with the prior binary and `--expected-version 1.0.90` to reproduce the
prior release; its manifest is in `copilot-1.0.90/`. No compatibility claim is
made for older releases or future schemas.

## Resume, denial and cumulative usage observations

The second isolated probe passes on both pinned releases:

```sh
python tests/spikes/copilot-lifecycle.py \
  --copilot /path/to/disposable/node_modules/@github/copilot-linux-x64/copilot \
  --expected-version 1.0.91 --output /path/to/new/private-probe
```

Repeat with the prior executable and `--expected-version 1.0.90`. The script
uses the original probe's minimal environment allowlist. It verifies the binary
version before serving synthetic model responses, uses a new mode-700 native
root, and disables updates, custom instructions, builtin MCP and remote controls.
No native CLI is launched by the offline CI validator.

Initial execution followed by `--resume=<sessionId>` preserves the entire saved
prefix byte for byte and appends `session.resume`, a second user/assistant turn
and another shutdown. Both shutdown records remain in the same events file.
The supplied provider counts produce `modelMetrics.gpt-4.usage` snapshots of
11/3/2 then 22/6/4 input/output/cache-read tokens, with request counts 1 then 2.
These are cumulative session snapshots, so adding both would double-count the
initial turn. Presence and model attribution are established for these mock
records; real billing and the meaning of every category remain unverified.

A fresh run with `--deny-tool=shell` still persists the assistant tool request,
`tool.execution_start` and `tool.execution_complete`, joined by the exact
`toolCallId`. Completion has `success: false`, `error.code: "denied"` and a
permission-rule message. The forbidden fixture marker is absent. An execution
start therefore does not establish that the shell operation actually ran.
There is no shell exit status or separate approval event in this recording;
`interactionId` is not asserted to be an approval identity.

The `lifecycle/` subdirectories under each version contain transformed initial,
resumed and denied recordings. Manifests retain original/transformed hashes and
binary provenance, and declare removed system-prompt payloads, replaced private
paths and stable session UUIDs. Offline tests check exact prefix preservation,
usage snapshots, explicit denied results and requested/start/result identity.
The following task and boundary probes add subagent, interactive, cancellation
and compaction evidence. Logical forks and native retention remain unknown;
#238 remains open.

## Native task and subagent persistence

`tests/spikes/copilot-subagent.py` reproduces a synthetic `task` invocation with
pinned 1.0.90 or 1.0.91 using the same private-root, offline environment policy.
All root and child model responses come from the localhost mock. Each version's
`subagent/task.events.jsonl` records one physical session containing
`subagent.started`, `subagent.completed` and child messages whose explicit
`parentToolCallId` joins the task's request and result. Child assistant
`originatingMessageId` also joins its distinct child user message and explicit
interaction ID. Lifecycle and child message records have no `agentId` field;
that identity remains unknown. Manifests retain executable
and source hashes and declare every transformation.

This is a native tool relationship. No explicit logical parent session was
observed, and `parentAgentTaskId` also appears on root user messages, so it does
not establish session lineage. No separate child session file was observed.

The mock child reports 14 total tokens. Session shutdown reports three requests
with 33 input, 9 output and 6 cache-read tokens, already including that child.
Adding the child total again would double count. These synthetic measurements
establish persisted accounting shape; native billing units remain unknown.

## Interactive, cancellation and compaction boundaries

`tests/spikes/copilot-boundaries.py` runs each mode in a new private root with a
pinned executable, offline settings and a localhost mock. Interactive mode uses
a Linux pseudo-terminal and accepts only the temporary folder's live trust
prompt. It persists the synthetic assistant reply on both 1.0.90 and 1.0.91.
Folder trust is not a persisted approval identity.

Cancellation waits for a disposable shell marker and the exact saved tool-start
record before sending SIGINT to its own process group. Saved `abort` reasons
include `user_initiated` and `user_abort`. Neither carries a `toolCallId`; a
missing tool result stays unknown even when a turn/session abort is observed.
The probe preserves those reasons rather than converting them into call results.

Manual `/compact` appends start/completion, model events and a usage checkpoint;
the consumed event prefix remains byte-identical. A checkpoint Markdown file is
written under the private native home, and the probe verifies its path, existence
and hash. Checkpoint bytes and prompt-rich model payloads remain private. Fixture
transformations remove those payloads, including nested `requestMessages` in
model-call success records, while preserving structural event IDs and usage.

Compaction records its own mock 11 input, 3 output and 2 cache-read tokens. The
shutdown metrics still report only the initial request with 11 input tokens,
even though the provider saw two requests. Those snapshots describe different
accounting categories. They cannot be summed indiscriminately or treated as a
native bill. The synthetic summary increases the measured post-compaction token
count in this short probe; no compression-quality promise follows.

Native file truncation, automatic deletion/retention, logical forks and other
platforms remain unknown. These observations support the bounded go decision below. Issue acceptance
still requires the baseline and contract review; no production adapter is implied.


## Go decision and implementation boundary

Exact persisted call IDs join both overlapping same-name tool requests to their
starts and reversed results on releases 1.0.90 (`ccf052b4`) and 1.0.91
(`216810c5`). This meets the tool-trajectory viability gate. Proceed with a
focused adapter for these pinned Linux x86_64 version-1 records after the baseline
repairs are accepted. Do not require stdout events, hooks, a running CLI, the SDK
or private SQLite files to browse historical sessions.

| Capability | Supported evidence and adapter rule | Unsupported or unknown |
| --- | --- | --- |
| Discovery and versions | Read `COPILOT_HOME/session-state/<sessionId>/events.jsonl`; default home is `~/.copilot`. Check saved `session.start.data.version` and `copilotVersion`. Explicit configured input roots take precedence. | Older/newer releases and future schemas have no compatibility claim; diagnose unsupported formats without migrating native storage or launching the CLI. |
| Session, event and message identity | Scope native IDs by machine, harness, session and source generation. Preserve event `id`/`parentId`, message IDs and explicit originating-message joins. | An event-chain parent, `parentAgentTaskId`, cwd or timestamp does not establish logical session lineage. |
| Tools and outcomes | Exact `toolCallId` request/start/result joins; retain arguments, outputs, explicit protocol errors and shell exit codes. Interpret protocol success separately from shell exit status. | Never join by tool name/order. Absent completion, exit code or error stays unknown. |
| Resume and mutation | Same-session resume appends to an unchanged prefix. Manual compaction also appends; retain prior events and structural checkpoint references. Verify complete consumed prefixes on every changed-file resume and reconcile explicitly source-owned projections. | Native truncation, in-place mutation, automatic rotation and retention policy were not observed. Collector replacement handling is a Semon requirement verified with source-shaped inputs, not a native-runtime claim. |
| Cancellation and denial | Retain observed abort reason tags and denied tool results. A denial start is not proof of shell execution; an abort without a call ID cannot complete a pending call. | No persisted folder-trust or standalone approval identity is established; no per-call cancellation result may be invented. |
| Task and child messages | Preserve `subagent.started/completed` and message `parentToolCallId`, `originatingMessageId`, interaction and turn IDs as explicit task relationships within the physical session. | No `agentId`, separate child-session identity or logical fork parent was observed. Other subagent modes remain unverified. |
| Usage and cost | Present per-model shutdown metrics are cumulative snapshots. Resume selects the latest snapshot rather than summing snapshots. Child totals are already included. Keep compaction usage in its separately reported category. Preserve missing fields as absent. | Per-message usage, complete billing units, prices and native invoice totals are not established. A price-table estimate must be labelled separately from reported cost. |
| Interactive versus headless | Both modes persist their tested user/assistant turns. Historical behavior uses only saved records. | PTY trust acceptance and mock-provider input checks are live observations; ephemeral stdout events cannot supply historical facts. |
| Platform and source lifetime | Pinned Linux x86_64 native executions use private, fresh roots. Semon must read native homes without writes and preserve retained native/raw evidence privately. | Other platforms and native retention/deletion defaults are unknown. No automatic pruning or live DB/WAL mirroring. |

The authoritative historical input is the saved JSONL. Workspace YAML, SQLite
and WAL/SHM, debug logs, rewind snapshots and checkpoint Markdown are outside the
initial discovery/mirror allowlist. Public fixtures remove prompt-rich harness
payloads and private paths; native records captured by Semon remain private raw
evidence. A remote viewing copy follows the existing redaction contract and
cannot be used as a recovery source. Unknown or malformed complete records must
remain retained without inventing transferable semantics.

## Provenance, confidence and planning record

All four probe families (`copilot-offline.py`, `copilot-lifecycle.py`,
`copilot-subagent.py`, `copilot-boundaries.py`) ran against both pinned binaries
with localhost synthetic model responses and minimal child environments. Their
versioned manifests bind executable hashes, probe/source hashes, native source
hashes, transformed fixture hashes and declared transformations. Runtime/mock
confidence establishes persistence shape and exact joins; it does not establish
real model quality or billing. Help establishes the default home; non-observed
retention and platforms remain unknown. The offline fixture validator checks
these saved artifacts and never launches a native harness.

The pre-implementation re-estimate was: historical discovery/indexing/paging and CLI/viewer
integration (#239) 4–6 engineer-days; durable continuous collection and
replacement parity (#240) 2–3; redacted mirror/allowlist/ACK parity (#241) 2–3;
release matrix, security checks and documentation (#242) 1–2. Combined adapter
work is 9–14 engineer-days, excluding remaining Claude/Codex baseline repairs.
Reuse existing source custody, marker/link support and mirror transport; do not
introduce a universal adapter framework or rewrite trace storage. These are
planning estimates, not elapsed execution promises or remaining work. The
implemented adapter and its acceptance coverage are documented in
[copilot-cli.md](copilot-cli.md), with final landing/check evidence tracked in #235.

No-go applies to claims of arbitrary-version/platform support, inferred logical
forks or approvals, automatic native retention semantics, complete trajectories
with missing results, or billing accuracy. Those claims require additional native
evidence and are not prerequisites for the explicitly limited adapter above.
