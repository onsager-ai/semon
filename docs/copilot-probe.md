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
mode, strips GitHub/provider credential environment variables, disables custom
instructions, builtin MCP, remote export/control and updates, and uses only a
localhost mock provider. The two shell commands print fixture strings; one
returns exit status 1. No personal session or credential file is mounted.
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

Interactive behavior, releases before 1.0.90, resume/fork/subagent metadata,
model-attributed usage, approvals, denial/cancellation, compaction, truncation,
mutable records, retention/deletion and non-Linux platforms remain unverified.
The parent event chain and `parentAgentTaskId` must not be interpreted as a
logical parent session without additional source evidence. No supported-version
or complete-trajectory promise is made yet. Follow-up probes must establish
what is authoritative before discovery, cursor or mirror code is implemented.

Repeat with the prior binary and `--expected-version 1.0.90` to reproduce the
prior release; its manifest is in `copilot-1.0.90/`. No compatibility claim is
made for older releases or future schemas.
