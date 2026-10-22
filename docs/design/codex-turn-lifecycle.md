# Codex turn activity and outcomes

The native Event Index records an exactly owned turn independently of writer
liveness. A held thread writer is a live process observation; it does not prove
that a turn is active. A qualified terminal makes the session idle while its
writer remains held. A subsequent new owned start makes it working again.

`turn_lifecycle` carries `thread_id`, `turn_id`, `phase`, native record `offset`
and `source_generation` (SHA-256 of the consumed native prefix). Phases are
`active`, `completed`, `aborted` and `errored`. Missing evidence is absent,
and the displayed activity is unknown. Completed describes a durable turn
terminal; it does not assert successful tools, process exit or runtime authority.
Writer PID/alive and machine freshness retain their existing separate contracts.
An active historical observation only displays working with a current live
writer observation. A dead, missing or offline writer cannot prove a terminal.
There is no silence or age timeout that completes a turn.

The fact lives in the existing ledger-qualified Tree summary and normalized
Event Index labels. Source replacement resets it; append resumes the same
reducer. Catalog's existing source-vector and runtime fences qualify the
published Query, MCP and Viewer observations. CLI tree collection consumes the
same fact. Home receives the same state and lifecycle; existing Viewer
`work`/`idle`/`err`/`off` presentation needs no generated UI change. Retained
observations preserve last known lifecycle without inheriting live authority.

## Source contract and support matrix

Public source pin:
[`openai/codex@c22a06c18070c3e33bd2648c7210a00a2a9f6f80`](https://github.com/openai/codex/tree/c22a06c18070c3e33bd2648c7210a00a2a9f6f80).
The protocol defines `TurnStartedEvent`, `TurnCompleteEvent`, `TurnAbortedEvent`,
`SessionMeta` and `HistoryPosition` in `codex-rs/protocol/src/protocol.rs`.
`codex-rs/core/src/rollout/policy.rs` persists all three lifecycle markers in
both history modes; standalone `Error` is transient. The recorder's flattened
`RolloutLineRef` contains timestamp, optional ordinal and the rollout item.
`ordinal.rs` documents the paginated ordinal and inherited-subagent boundary.
These are source-qualified parser guarantees, not native runtime captures or
a claim about every released Codex binary. The prior a9b0815 tool-outcome oracle
remains unchanged and does not by itself qualify this lifecycle contract.

| Carrier | Supported evidence | Qualification / limitation |
| --- | --- | --- |
| Legacy `event_msg` | `task_started`, `task_complete`, `turn_aborted` | Complete source begins with exact `session_meta.id`; start and terminal have the same nonempty turn ID |
| Aliases | `turn_started`, `turn_complete` | Same reducer and ownership rules as legacy names |
| Paginated `event_msg` | Same lifecycle markers and aliases | Explicit paginated mode, numeric increasing record ordinals |
| Terminal error | Object `error` with string `message` on complete/abort | Errored outcome; malformed non-null errors never become completion |
| Abort | interrupted, replaced, review_ended, budget_limited | Exact active turn ID required; absent/unknown reason stays unqualified |
| Paginated copied fork/subagent | Explicit `forked_from_ordinal_exclusive` or `subagent_history_start_ordinal` | Ignore inherited ordinals; only starts at/after the explicit local boundary qualify |
| Legacy copied fork; ambiguous paginated fork | Unknown | No attribution from ancestor history or inferred boundary |
| Standalone error; missing IDs; unknown modes | Unsupported / unknown | No success inferred; standalone error cannot close the durable turn |

Lifecycle payloads do not require `thread_id`: their owner is the exact physical
source's first session metadata. Optional explicit carrier thread IDs must agree.
`root_turn_id` is causal attribution, never an owning thread. `history_base` names
a physical rollout, which can differ after revert, and is never treated as the
logical fork cutoff. A second session metadata record invalidates ownership.
Missing header identity cannot be repaired from filename, prompt or proximity.

Terminals require an observed owned start. Missing-start terminals cannot prove
completion. Foreign/late terminals and duplicate/replayed starts cannot replace
the current turn. Exact replay rejection retains at most 4096 distinct turn IDs
of at most 256 bytes in the existing source summary. At that limit the reducer
fails closed to unknown until an independently qualified source rebuild/new
generation; rebuilding the same over-limit source remains unknown. It never
evicts old IDs and then accepts an old start as new. This is an explicit metadata
availability limit, not truncated successful lifecycle analysis.

Parser cache 29 rebuilds preexisting current facts from native source. Storage
schema and retained slot recipe version 6 are unchanged. Retained projections
remain historical observations under their existing original/source custody
contract. Tests use sanitized source-shaped JSON records, with no personal logs.
CI owns builds, tests, formatting and all visual gates; no local test acceptance
is implied by the implementation.
