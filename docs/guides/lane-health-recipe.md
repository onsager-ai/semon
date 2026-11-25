# Lane health pilot

Use this recipe on `hp-code-2` to report on one Manager root session and the
Workers whose sessions you can identify. The SQL reads one origin's published
analytics snapshot. It uses metadata and counters only. Message and tool body
text stays out of analytics and the Hub. Keep PR, branch head, and CI state in
Agent Workbench.

## Prepare and check the snapshot

Use a `semon` binary with `asi-query`. Linux release binaries have included it
since [#733](https://github.com/onsager-ai/semon/pull/733). Check the installed
binary with `semon query --help` and look for `sql`, `schema`,
`context_growth`, and `lane_health` in the usage output. The shell examples use `jq` to build
JSON parameters and select fields from responses.

On `hp-code-2`, every materialization must run under a systemd `CPUQuota` of
25–30%, including watch mode if used. An uncapped run took the host from 33 to
85 °C in 41 seconds. Run as the account that owns the native logs and cache.
The example uses the default cache. If you use `--cache PATH`, pass the same
value to the materializer, coverage, and queries. Configure the same cache and
native homes for CLI and MCP reads.

```sh
systemd-run --user --wait --pipe --collect --property=CPUQuota=25% \
  "$(command -v semon)" analytics materialize && date -u +%FT%TZ
semon analytics coverage
```

If the capped unit fails, resolve the failure before refreshing. Keep the UTC
completion time from a successful run and the `manifest generation` from
`coverage`. Compare that generation with the `snapshot` in the JSON query
results. If the generation changes during the report, rerun the affected reads
against the same publication. Generation identifies a snapshot. The successful
materialization time tells you when the last drain finished, even if there was
nothing new to publish. New native activity may arrive after the drained
endpoint. Coverage reports published cohorts and source-missing UIDs. Check it
for gaps and use the native read below for current activity.

## Run the root-scoped report

After materialization, pass the Manager's full, machine-qualified ASI UID.
Use step 2 below to find it from a native session ID. The report walks the
published family and reads its analytics from one snapshot generation.

```sh
manager_uid='REPLACE_WITH_FULL_MANAGER_UID'
semon query lane_health "$manager_uid"
```

For MCP over local stdio, call `lane_health` with
`{"root_session_uid":"FULL_MANAGER_UID"}`. Configure the same native homes
and cache for CLI and MCP. Native `--since` and `--all` options affect the live
read window. Compare `snapshot` with the generation from `analytics coverage`.

- `members` lists the root and its published descendants with parent UIDs,
  compaction counts and events. Each event carries the trigger, pre-tokens,
  first measured refill, and the same `early` and `storm` flags as context growth.
- `manager_tool_mix` counts the root's tools and command tags.
  `observed_children` lists the root's direct published children. A recorded
  call alone cannot establish that a Worker launched.
- `usage` reports exclusive and copied observations in separate buckets with
  token counters, priced USD estimates and priced/unpriced step coverage. Use
  exclusive usage for spend. Copied observations can repeat and must stay
  separate from spend totals. Deduplicated shared usage remains a follow-up.
- `runtime` gives the answer time, native observation time and native window
  start in UTC milliseconds. Each member's `live` object carries state,
  liveness and last activity. A null `live` means the native read found no
  matching observation in its window or was unavailable. Check
  `runtime.error_code` for a native read failure. A quiet or missing row leaves
  Worker disposition unknown.

Descendants linked only by marker or process evidence appear once ASI
publishes those edges. Review `analytics coverage` for gaps and use the native
reads below to investigate other observed sessions. Per-task attribution and
Worker dispositions remain follow-up work. Use Agent Workbench for PR, head
and check status.

## Find the Manager and Worker UIDs

1. Start with the Manager's native session ID from its harness or Workbench.
   Find the matching row in `semon query list_sessions --json` and save its
   native `id` as `manager_id`. Use `--parent "$manager_id"` to look for
   children, and repeat with a child's ID to look for descendants. Check
   `truncated` and widen the native window with `--since` if relevant logs are
   older. Use the Workbench launch record to identify any Workers you can
   verify independently.

   ```sh
   manager_id='REPLACE_WITH_NATIVE_MANAGER_ID'
   semon query list_sessions --json |
     jq --arg id "$manager_id" '{truncated, sessions: [.sessions[] |
       select(.id == $id) | {id, harness, parent, children}]}'
   ```
2. Resolve each native ID against the snapshot. For an ordinary native ID such
   as a UUID, match the final UID component. If an ID has escaped characters,
   supply its encoded component. Use the full `session_uid` returned by SQL,
   checking the origin and harness if there is more than one match. An empty
   result means the lookup found no published match. Check the ID and snapshot
   freshness and record any Worker you still cannot resolve.

   ```sh
   native_id='REPLACE_WITH_NATIVE_ID'
   semon query sql '
   SELECT session_uid, harness_key, started_at
   FROM dim_session
   WHERE ends_with(session_uid, ?)
   ORDER BY session_uid
   ' --params "$(jq -cn --arg suffix "/$native_id" '[$suffix]')"
   ```

3. Look for published children using the Manager's full UID. Repeat for each
   confirmed child to follow published edges further. `lane_root_uid` is
   currently null. `parent_session_uid` covers only admitted native parent
   evidence, so the published family can be partial. Keep an explicit list of
   UIDs and record missing or unconfirmed members. Identify the Manager from
   your lane records because the published `role` does not identify it. Worker
   outcome is `unknown` in this pilot. Semon does not yet publish the Worker
   disposition from the final report.

   ```sh
   manager_uid='REPLACE_WITH_FULL_MANAGER_UID'
   manager_params=$(jq -cn --arg uid "$manager_uid" '[$uid]')
   semon query sql '
   SELECT session_uid, parent_session_uid, started_at
   FROM lane_session
   WHERE parent_session_uid = ?
   ORDER BY started_at NULLS LAST, session_uid
   ' --params "$manager_params"
   ```

## Run the SQL recipes

Set the Worker UID, then use the same bound roster for compaction and spend.
The examples bind two UIDs. For more or fewer sessions, change the number of
`?` placeholders and the JSON array together. Use one entry per confirmed
session. Check `truncated` in each result and raise `--limit` if needed.

```sh
worker_uid='REPLACE_WITH_FULL_WORKER_UID'
lane_params=$(jq -cn --arg manager "$manager_uid" --arg worker "$worker_uid" \
  '[$manager, $worker]')
```

### Compactions per session

Each compaction row shows its timestamp in UTC milliseconds, recorded trigger,
and pre-compaction token count. The session total repeats on each row. A
published session with no compactions gets one row with a zero count and null
compaction fields. Null `trigger` or `pre_tokens` on a compaction means the
harness did not record that value. A UID absent from `lane_session` has no row.

```sh
semon query sql '
SELECT s.session_uid,
       count(c.compaction_uid) OVER (PARTITION BY s.session_uid) AS compactions,
       c.compaction_uid, epoch_ms(c.ts) AS ts_ms, c.trigger, c.pre_tokens
FROM lane_session s
LEFT JOIN fact_compaction c ON c.session_uid = s.session_uid
WHERE s.session_uid IN (?, ?)
ORDER BY s.session_uid, ts_ms NULLS LAST, c.compaction_uid
' --params "$lane_params"
```

### Context growth and compaction flags

Run the per-session command for a closer look at the Manager or a Worker.
It also accepts `--since` and `--until` as inclusive UTC milliseconds.

```sh
semon query context_growth "$worker_uid"
```

Step points show `context_tokens`, which includes uncached input, cache reads,
and cache creation. Compaction points show `pre_tokens` and the first measured
refill (`post_refill_tokens`) when available. `early` marks an automatic
compaction below half that session's measured peak context. `storm` marks each
compaction in any inclusive 15-minute window containing at least three
compactions. Both flags and refill use the full published session. Time bounds
filter the points returned. Missing evidence can leave a flag false, so read
flags alongside timestamps, trigger, and token coverage.

### Manager tool mix

This counts the Manager's recorded tool calls by tool and normalized command
family. A null tag means no command family was recorded. Use the mix to see
which calls dominate. Counts measure calls, so they cannot establish elapsed
time or how many Workers actually launched.

```sh
semon query sql '
SELECT tool_key, command_tag, count(*) AS calls
FROM fact_tool_call
WHERE session_uid = ?
GROUP BY tool_key, command_tag
ORDER BY calls DESC, tool_key, command_tag
' --params "$manager_params"
```

### Exclusive token usage and estimated spend

Aggregate at model-step grain and keep only `exclusive` usage. This excludes
copied usage and avoids multiplying it by tool-call rows. This query combines
all supplied sessions. USD sums the recorded estimates for priced steps and
stays null if there are none. Report both coverage counts alongside it. Token
sums include the recorded counters from both priced and unpriced exclusive
steps. Cache-write buckets can be incomplete when `unknown_cache_split_steps`
is nonzero. For reasoning, use the subtotal alongside `steps_with_reasoning`.

```sh
semon query sql "
SELECT count(*) AS exclusive_steps,
       count(*) FILTER (WHERE price_status = 'priced') AS priced_steps,
       count(*) FILTER (WHERE price_status = 'unpriced') AS unpriced_steps,
       sum(usd_est) FILTER (WHERE price_status = 'priced') AS priced_usd_est,
       sum(input_tokens) AS input_tokens,
       sum(output_tokens) AS output_tokens,
       sum(cache_read_tokens) AS cache_read_tokens,
       sum(cache_write_5m_tokens) AS cache_write_5m_tokens,
       sum(cache_write_1h_tokens) AS cache_write_1h_tokens,
       count(*) FILTER (WHERE split_unknown) AS unknown_cache_split_steps,
       sum(reasoning_tokens) AS known_reasoning_tokens,
       count(reasoning_tokens) AS steps_with_reasoning
FROM fact_step
WHERE usage_owner = 'exclusive' AND session_uid IN (?, ?)
" --params "$lane_params"
```

## Check live activity and stalls

Run native reads near the time you report. The examples show children of one
parent and stalls across the local window. Match stall rows to your roster by
native `id`. For the Manager's own state, omit `--parent` from `list_sessions`
and select its `id`. If a list is truncated, narrow the time window inside the
scan or raise `--limit` before interpreting an absent row. The filters below
keep the report free of native question previews.

```sh
semon query list_sessions --parent "$manager_id" --json |
  jq '{now, window_start, total, truncated,
       sessions: [.sessions[] | {id, parent, state, alive, last_activity}]}'
semon query stalls --idle-minutes 30 --json |
  jq '{now, window_start, idle_minutes,
       stalls: [.stalls[] | {id, state, alive, last_activity, idle_ms, stall_reason}]}'
```

For MCP, connect to local `semon mcp` over stdio and call `list_sessions` with
`{"parent":"NATIVE_MANAGER_ID"}` and `stalls` with `{"idle_minutes":30}`.
Both native tools accept a `since` filter (CLI `--newer-than`) inside the scan
window. `window_start` is the underlying window boundary, `now` is the answer
time, and `last_activity` is the last recorded activity, all in epoch
milliseconds when present. A long-running MCP server can retain an older scan
window until sources change. Check these fields before calling a state current.

`alive: null` means liveness is unknown. Native process evidence depends on
qualified Claude PID records or Codex writer locks. A quiet subagent that
would otherwise show `no-output` is `subagent-unconfirmed`. That leaves its
completion unknown. A quiet or missing row is insufficient to infer a stop,
blocker, or outcome.

## Manager status to paste

```text
Lane <name> | Manager <UID> | Workers <confirmed UIDs> | missing <IDs or unknown>
Analytics snapshot <generation> | materialization finished <UTC> | coverage gaps <notes>
Compactions <counts by session, trigger and pre-tokens> | early <events> | storms <events>
Manager calls <top tool_key/command_tag counts>
Exclusive usage <input/output/cache counters> | priced estimate <USD or unknown> | <priced>/<exclusive> priced, <unpriced> unpriced
Native state as of <UTC> | stalls <IDs, idle time, reasons> | Worker outcome unknown
PR/head/checks <Agent Workbench status>
```
