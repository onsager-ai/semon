# Copilot CLI persistence probe (in progress)

Tracking: #238 under #235. The capability contract is not yet accepted.

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
| Shutdown | `session.shutdown`, per-model `modelMetrics` | Mock sent no token counts; native token/billing accounting unverified |
| Other storage | workspace YAML, SQLite session store plus WAL/SHM, logs, rewind snapshots | Not approved mirror inputs; no DB/WAL copying |
| Ephemeral stream | stdout model calls, deltas, idle and background changes absent in saved log | Live observations cannot be prerequisites for historical browsing |

The fixture removes the native system prompt and replaces the disposable cwd
with `/fixture/probe`. Its manifest distinguishes the original raw hash from the
transformed fixture hash. It is a transformed runtime recording, not an exact
copy of retained native evidence. It contains no real user session.

## Remaining gate

Interactive behavior, releases before 1.0.90, fork/subagent metadata,
native model billing, explicit approval records, cancellation, compaction, truncation,
mutable records, retention/deletion and non-Linux platforms remain unverified.
The parent event chain and `parentAgentTaskId` must not be interpreted as a
logical parent session without additional source evidence. No supported-version
or complete-trajectory promise is made yet. Follow-up probes must establish
what is authoritative before discovery, cursor or mirror code is implemented.

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
This increment does not accept #238 or establish fork, cancellation, compaction,
subagent, interactive or native retention behavior.
