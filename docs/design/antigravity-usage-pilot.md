# Managed Antigravity usage pilot

The November 30 fixture and field contract cover managed Antigravity ACP
1.5.18. The December 2 pilot targets Semon-backed session identity, completed
runs and completed provider turns for Peirce, with timing, selected model and
token usage where qualified. Semon owns session usage, reference prices and costs
across harnesses.
Semon ingests the origin-local T3 metadata into its session model. Peirce reads the
result from Semon, with sources named in the scorecard. `peers.md` remains lane
configuration. Parent relationships, rework, review outcomes and Agent Workbench
denials are outside this pilot.

## Source identity and custody

| Source | Qualified evidence |
| --- | --- |
| Antigravity `conversations/<cascade-id>.db` | The filename stem must equal a unique `trajectory_meta.cascade_id`. The Semon native key is `(origin, harness_key = antigravity, cascade_id)`. `trajectory_id` identifies the trajectory and is a distinct field. |
| Antigravity `steps` | Read `idx`, `step_type`, `status` and `has_subtrajectory` as opaque native metadata. Step count and subtrajectory presence do not establish turns, model calls, token usage or cost. |
| T3 provider thread | Require projection `driver = antigravity` and a matching `nativeThreadRef.driver` with `strength = strong`. Match its `nativeId` exactly to the origin's native cascade ID. An absent, weak, conflicting or wrong-driver link supplies no turn or usage attribution. A selected model label or `provider_session_id` alone cannot qualify the link. |

The Semon producer reads the already-present stores on their origin using
indexed SQLite reads with WAL-aware read-only access. It projects only the
qualified metadata and provenance into Semon. Native databases, bodies,
binary step payloads, paths and raw T3 payloads stay under their existing
custody. The T3 runtime service is not a dependency. This evidence grants no
new body access, archive access or control authority.
[Source custody](session-source-reader.md)

## Pilot fields

ASI v5 already defines the identity and timestamp columns in `dim_session` and
`fact_turn`. Antigravity population of those columns and the supplemental run,
thread, model and usage fields below are pilot proposals. The generated
[v5 schema](../reference/asi-schema.md) describes the current interface.

Use the T3 provider-thread ID to select completed runs. Join run ID to its
attempt's `run_id`, then completed attempt ID to the completed provider turn's
`run_attempt_id`. Link rows must agree on their T3 thread and provider thread.
The attempt-side `provider_turn_id` may be null. If present it must agree with
the provider turn's ID. An unresolved or ambiguous link supplies no attributed
turn for that run. Report it as a coverage gap.

| Semon field or measure | Pilot meaning |
| --- | --- |
| `dim_session.harness_key`, `dim_session.session_uid` | Use `antigravity` and the full machine-qualified UID `{machine}/antigravity/{cascade_id}` with each component encoded by the existing ASI UID rules. Resolve by exact qualified identity because the final component can recur on another machine. Keep the native session even when its supplemental link is unavailable. |
| T3 thread and provider instance | Resolve `provider_threads.thread_id` against `threads.thread_id` on the same origin. Attribute the provider instance from the qualified provider thread and require its ID to agree with the run and attempt. Driver qualifies the harness. Instance identifies the specific T3 provider configuration. |
| Session timestamps and selected model | The joined T3 app thread's `created_at` and `updated_at` provide UTC thread creation and last update. Label them as T3 thread observations. The thread's `modelSelection.model` is its current selection. Native session start/end timestamps are unknown from this metadata projection. |
| Completed run ID and count | Count distinct origin-local T3 `run_id` values in the qualified session whose run status is completed. Count a completed run even when its timing or provider-turn link is unknown. |
| Run start, completion and latency | Use the run's `startedAt` and `completedAt` as UTC instants. Latency is completion minus start. Publish unknown when a timestamp is missing, invalid or reversed. Include only valid pairs in the timing denominator and report that denominator alongside completed runs. An observed zero duration is valid. |
| `fact_turn.turn_uid`, `fact_turn.session_uid` and completed turn count | Use the qualified session UID and the origin-local T3 `provider_turn_id` as the final component of `{session_uid}/t/{id}`. Identical observations or replays of the same key count once. A weak `nativeTurnRef` supplies no extra identity. Failed attempts contribute no completed turn. A missing link leaves coverage incomplete. An attributed completed turn counts even if it has no usable interval. |
| `fact_turn.started_at`, `fact_turn.ended_at` | UTC timestamps from T3 provider turn `started_at` and `completed_at`. Calculate duration only from a valid, ordered pair. For p50 and p90 use only timed turns and report their count as the denominator. Run timing comes from the linked run and cannot fill missing turn timing. |
| Selected model | Per-run `modelSelection.model` with T3 selected provenance. Carry only the exact linked run's selection onto its turn. A missing or ambiguous run-level selection stays unknown. The current session selection cannot fill an earlier turn. |
| Executed model | Publish only from explicit executed-model evidence attributable to the specific run or turn. Both are unknown from the verified cohort's current inputs. The selected label supplies neither an executed ID nor price provenance. |
| Token usage | T3 provider turn `turnTokenUsage` with `usageScope`, `usageStatus`, `inputTokens`, `outputTokens`, `cachedInputTokens`, `cacheCreationTokens`, `reasoningTokens` and `hasSubagents`. Semon preserves the observed scope and status. `main_agent` with `complete` describes the main agent's reported turn usage. `hasSubagents` is an independent observation and supplies no child counters. |
| Usage availability | A missing category is null, including when other categories are observed. Preserve an explicit zero and an explicit false. Missing or conflicting usage for a uniquely identified turn leaves usage unknown while the turn still counts once. Keep scope and status visible for incomplete reports and leave unsupported totals unknown. |
| Reference price and estimated cost | Semon resolves only exact models.dev provider/model rates for the verified executed model and applicable usage categories. Estimates carry models.dev provenance and remain separate from billed amounts. Missing execution evidence, usage or applicable rates leaves cost unknown. Any billed amount, including a gateway charge, is evidence for reconciliation inside Semon only. |

Peirce can group qualified turns by selected model if it labels that dimension
as selected. Unknown values remain unknown in Semon and in consumers. A sum over
observed usage must disclose its coverage. Native steps, runs, provider turns and
model calls have different grains. The pilot makes no model-call count claim.
The existing per-response accounting path cannot treat these turn counters as
response counters. [UID rules](asi-store.md#identity) ·
[Existing usage model](native-usage-storage.md)

## Acceptance evidence

The November 28 verified cohort matched 7 of 7 native session identities. Of
402 runs with Neon selected, 27 used other drivers. The remaining 375 matched
375 completed Antigravity provider turns with counters. These are observations
for the qualified cohort at the time of the check. Production pilot acceptance
requires Semon to reproduce the joins and counts for that cohort, expose gaps,
and keep spend unknown until executed identity, counters and exact rates qualify.

The [synthetic fixture](../../tests/fixtures/compatibility/antigravity-acp-1.5.18/manifest.json)
exercises the source keys, turn join, replay handling and usage gaps with
fabricated values. The pilot producer and Semon read surface are follow-up work.
