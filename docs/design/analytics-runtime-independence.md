# Analytics without live runtime observation

Chief PM approved the design on 2026-11-24. The owner approved D11 for P4.1.
The staged adapter scans committed warehouse inputs in bounded batches. Infra
runs the hp-code-2 measurement before P4.2 proceeds.

## Problem

`semon analytics materialize` refreshes the live Catalog before draining the
analytics writer, on the first pass and every `--watch` pass.
[#735](https://github.com/onsager-ai/semon/pull/735) made each refresh reuse
one runtime capture so process churn would stop invalidating work in flight.
An older capture can still publish a removed Codex name after a newer refresh
observed its absence ([#736](https://github.com/onsager-ai/semon/issues/736)).

Four review rounds across #735 and [#737](https://github.com/onsager-ai/semon/pull/737)
ran into the same problem: ordering live captures and writes across producers.
The last attempt started the Viewer's live Catalog producer from materialize.
Its final review found four failures. The aggregate digest missed changed
process ancestry, owner qualification missed unheld Codex writer lock files,
runtime requalification held the SQLite writer lock during filesystem scans,
and two live producers could still restore a withdrawn name.
[#737 closing review](https://github.com/onsager-ai/semon/pull/737#issuecomment-7106590448)

## Analytics inputs

Native evidence enters through the Index. Its admitted bytes support records,
usage, tools and compactions, with source incarnations, committed prefixes and
proofs for evidence links. The [current SQLite adapter](../../crates/semon-sessions/src/origin_projection.rs)
joins Catalog identity and native lineage to matching activity and pricing
projections in a read snapshot. Catalog and activity generations qualify
those joins. `_gen` marks the separate analytics publication.

| Output | Required inputs |
| --- | --- |
| `fact_session` | Origin namespace, committed native session identity and start, source qualification, step coverage and exclusive usage, recorded pricing and transform versions. |
| `fact_step` | Committed session, turn and step identities, native anchors, times, usage and activity, committed repo identity, recorded rates and price version. |
| `fact_turn` | Native step grouping, timestamp bounds and step count. Initiator is currently unknown. |
| `fact_tool_call` | Admitted native call and result metadata attached to the step. |
| `fact_compaction` | Admitted native compaction event and record anchor, with a qualified step association when available. |
| `fact_label` | Durable categorical label assertions with target, author and taxonomy provenance. |
| `dim_session` | Committed harness, native identity and parent lineage, role, start, branch and repo. |
| `dim_machine`, `dim_harness`, `dim_repo` | Origin namespace, native harness and committed repository identity, respectively. |
| `dim_tool`, `dim_model`, `dim_date`, `dim_price` | Classified tool and model metadata, UTC dates from admitted timestamps, and recorded rates and versions, respectively. |

No published column consumes processes, PIDs, harness writer locks, runtime
files, Codex display names, or reported runs from `~/.claude.json`. Observed
parents are excluded, status stays unknown, and reported cost and unproven end
times stay null. A ledger-qualified native sidecar that proves identity or
lineage is source evidence. Warehouse queries must preserve that distinction
because Viewer parent selection can also consult runtime observations. Body
text stays origin-local and out of analytics and the Hub.
[#737 field audit](https://github.com/onsager-ai/semon/pull/737) · [ASI direction](asi-direction.md)

## Proposal

Materialize reads source-qualified, committed warehouse facts and identity from
one SQLite snapshot per input batch. Use committed source prefixes and a
warehouse change boundary for inventory and incremental work, including
replacement, withdrawal and late links. Keep the existing distinction between
pending, missing and admitted evidence so a partial scan cannot erase prior
facts. The analytics writer keeps its own store lock, checkpoints and publication
writes. It takes no runtime capture, starts no Catalog producer and writes no
Catalog or runtime metadata.

The [phase 2 producer](star-catalog-phase2.md#write-transactions) already writes
warehouse facts and native proofs with Index advancement. It supplies the
storage and parser path, but a warehouse read alone cannot discover new logs or
advance their committed prefixes. To preserve standalone use, reuse the Index's
native discovery, parsing and admission through a runtime-free entry point with
no semantic stage. On cold start, that entry point establishes the qualified
Index and storage binding and drains the initial authorized inventory. One-shot
materialize drains analytics through the resulting committed boundary. In watch
mode, each pass advances native ingestion in bounded batches and consumes the
new commits. A later append belongs to a later pass. Source admission and its
integrity and custody checks belong to the Index, even if the CLI invokes it
before running the analytics drain.

## Concurrent admission

The Viewer and materialize can admit the same growing source into the same
Index. `Ledger::resumes` checks the file identity and hashes of the consumed
prefix and tail before reusing it, unless the scanner already has qualified
append evidence. `SqliteStore::write_one_dirty` compares the committed `files`
ledger with the scanner's expected ledger under `BEGIN IMMEDIATE`. It writes the
new ledger, proofs and warehouse facts in that transaction. `commit_initial_files`
requires an absent ledger and matching file stat. A losing scan reads the
newer ledger before trying again. While Catalog publication still uses source
claims, `publish_catalog_with_writer` compares each claimed revision against
`session_catalog_invalidations` and acknowledges only the matching revision, so
old work cannot clear a newer claim. Phase 4 retires those semantic claims with
their consumers and keeps the Index ledger checks.
[Scanner and ledger](../../crates/semon-sessions/src/events.rs) · [Index transactions and claims](../../crates/semon-sessions/src/events/store.rs)

A pending scan never erases already-admitted facts. A losing transaction leaves
the committed facts visible, and `SessionChange::Pending` leaves the published
analytics rows in place. Neither the Viewer's Index admission nor materialize's
admission takes a runtime capture in this path. Process churn cannot invalidate
these batches, and there are no captured runtime values to reconcile or publish
out of order. This removes the failure class seen in #735 and #737 from
materialize.
[Analytics publication](../../crates/semon-asi/src/publish.rs)

The implementation PR must include `concurrent_admission_matches_single_run`.
Race two admission runs over the same growing native source using separate
processes or database connections. Assert that admitted facts increase
monotonically, stale work never overwrites newer data, both runs complete
without lock timeouts, and the analytics published afterward match a single-run
baseline over the same evidence.

## Phase 4

Make this the analytics slice of the [phase 4 cutover](star-catalog-phase3.md#phase-4).
It needs the phase 2 writer and phase 3 identity and source-resolution contracts,
then a warehouse input adapter with committed change tracking and native-only
lineage. Verify stable UIDs and evidence links, causal turns, shared/exclusive
and family totals, costs, and label provenance. Keep source qualification and
Index compare-and-swap checks. Remove the old stage and its generation fences
when the remaining CLI, MCP and export callers have moved.

### D11 · P4.1 full namespace scans

P4.1 stages a runtime-free native admission entry point and a warehouse input
adapter. Each drain opens one SQLite read snapshot after admission and pages
whole sessions with the existing input budgets. The snapshot pins membership,
ledgers, prefixes and facts for the whole drain. Appends or replacements that
commit after it opens appear on the next drain. A restart takes a fresh read snapshot. It can resume an interrupted inventory
when the existing checkpoint still matches the Index incarnation, prefix
revision and inventory bound. The adapter has no
persisted change feed or checkpoint format of its own.

The scan revisits the namespace on every drain. Measure that cost against the
roughly 3,000 sessions on the six-hour hp-code-2 timer. A long read snapshot can
also delay SQLite WAL reclamation until the drain closes. The existing Index
producer can qualify and commit a withdrawal. A fresh warehouse inventory then
marks the absent published identity source-missing. The runtime-free admission
entry point does not reconcile disappeared files or re-admit archived history.
These cases can remain visible until the other native producer reconciles them.
The staged adapter derives activity from normalized Index rows bound to the
same warehouse ledgers and uses the warehouse's committed identities, native
parents and usage facts. Where no recorded repo identity is available it
publishes an unknown repo. The next slice owns CLI materialize, ASI SQL and
export cutover and the production timing gate.

## hp-code-2 measurement before P4.2

Infra runs this procedure with a CI-built candidate that exposes the P4.2
materialize path and ASI SQL. Use a private, freshly created scratch directory
on the host. Stage a fixed, authorized copy of the three native home trees there
and keep their paths and contents stable until the watch append. Stage the
matching recorder facts file if the timer uses one. Record the capture time and
file and byte totals per harness. Use an empty runtime fixture. Leave the live
state, timer, and native homes alone. Compare against the pre-cutover binary on
the same corpus and quota if a paired estimate is needed. Give each binary a
fresh scratch copy, including the sentinel below. Cold here means an empty
private Index and Analytics Store. Leave the OS page cache alone.

In a Bash shell, set `BIN` to the absolute path of the candidate executable and
`RUN` to the absolute path of the prepared scratch directory. The staged homes
are `$RUN/claude`, `$RUN/codex` and `$RUN/copilot`. If the timer uses a recorder
facts override, append `--facts "$RUN/facts.json"` to `opts` in the block below.
Run one case at a time and take a fresh thermal reading before each case.

```bash
umask 077
set -o pipefail
mkdir -m 700 "$RUN/state" "$RUN/proc"
mkdir -p "$RUN/claude/projects/-work-p4-probe" "$RUN/codex" "$RUN/copilot"
probe="$RUN/claude/projects/-work-p4-probe/p4-watch-probe.jsonl"
cat > "$probe" <<'JSONL'
{"type":"user","uuid":"p4-user","sessionId":"p4-watch-probe","timestamp":"2026-11-24T00:00:00Z","message":{"role":"user","content":"Synthetic request"}}
{"type":"assistant","uuid":"p4-first","sessionId":"p4-watch-probe","timestamp":"2026-11-24T00:01:00Z","message":{"id":"p4-first","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"Synthetic reply"}],"usage":{"input_tokens":10,"output_tokens":5}}}
JSONL
cache="$RUN/state/index.json"
opts=(--all --cache "$cache" --claude-home "$RUN/claude"
      --claude-json "$RUN/empty-claude.json" --codex-home "$RUN/codex"
      --copilot-home "$RUN/copilot" --proc-root "$RUN/proc")
sql="SELECT coalesce(sum(step_count),0) FROM fact_session
     WHERE session_uid LIKE '%/claude/p4-watch-probe'"
export BIN RUN cache probe sql SEMON_PRICING_REFRESH=off

measure() {
  local case_name="$1" rc=0
  shift
  agent-workbench ops thermal now || return
  date -u +start=%FT%TZ > "$RUN/$case_name.utc"
  agent-workbench ops thermal gate -- systemd-run --user --scope \
    --unit="semon-p4-$case_name" -p CPUQuota=25% \
    timeout --kill-after=10s 30m /usr/bin/time \
    -f 'elapsed_s=%e user_s=%U sys_s=%S max_rss_kib=%M exit=%x' \
    -o "$RUN/$case_name.time" "$@" > "$RUN/$case_name.log" 2>&1 || rc=$?
  date -u +stop=%FT%TZ >> "$RUN/$case_name.utc"
  printf 'scope_exit=%s\n' "$rc" >> "$RUN/$case_name.utc"
  return "$rc"
}

snapshot() {
  timeout 10s "$BIN" query sql --cache "$cache" --json "$sql" |
    jq -e 'select((.rows[0][0] | tonumber) == 1) |
           {asi_schema_version, snapshot, rows}' > "$RUN/$1.snapshot.json"
}

# Run each case separately with its own thermal monitor.
measure cold "$BIN" analytics materialize "${opts[@]}" && snapshot cold
measure warm "$BIN" analytics materialize "${opts[@]}" && snapshot warm
mv -T -- "$RUN/state/analytics-store" "$RUN/analytics-before-replay" &&
  measure replay "$BIN" analytics materialize "${opts[@]}" && snapshot replay
```

Stop at the first nonzero exit. The replay preserves the scratch Index and
namespace and starts a fresh scratch Analytics Store. In the watch case, append
one complete synthetic step after the initial `status=idle` line and poll SQL
once per second inside the same quota scope. Append-to-visible is measured from
the end of the append to the first successful query returning `step_count=2`.
The polling interval is part of that observed result.

```bash
measure watch bash -eu -c '
  "$BIN" analytics materialize --watch "$@" &
  writer=$!
  trap '\''kill -TERM "$writer" 2>/dev/null || true; wait "$writer" || true'\'' EXIT
  until grep -q "status=idle" "$RUN/watch.log"; do
    kill -0 "$writer"
    sleep 1
  done
  before=$(timeout 10s "$BIN" query sql --cache "$cache" --json "$sql")
  jq -e "(.rows[0][0] | tonumber) == 1" <<< "$before" > /dev/null
  printf "%s\n" "$before"
  cat >> "$probe"
  appended_ms=$(date +%s%3N)
  for attempt in {1..60}; do
    result=$(timeout 10s "$BIN" query sql --cache "$cache" --json "$sql")
    if jq -e "(.rows[0][0] | tonumber) == 2" <<< "$result" > /dev/null; then
      printf "append_utc_ms=%s append_to_visible_ms=%s\n" \
        "$appended_ms" "$(( $(date +%s%3N) - appended_ms ))"
      printf "%s\n" "$result"
      exit 0
    fi
    kill -0 "$writer"
    sleep 1
  done
  exit 1
' p4-watch "${opts[@]}" <<'JSONL'
{"type":"assistant","uuid":"p4-second","parentUuid":"p4-first","sessionId":"p4-watch-probe","timestamp":"2026-11-24T00:02:00Z","message":{"id":"p4-second","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"Synthetic follow-up"}],"usage":{"input_tokens":10,"output_tokens":5}}}
JSONL
```

Start a thermal monitor in a second terminal immediately before each case. Set
`CASE` to `cold`, `warm`, `replay`, or `watch`, and use the same `RUN`. Stop the
monitor with Ctrl-C when the case exits. If any reading reaches 85 °C or the
sensor becomes unreadable, the monitor stops the scope. End the measurement
at that point. Keep the thermal gate active as well.

```bash
while :; do
  date -u +%FT%TZ
  if ! agent-workbench ops thermal now; then
    systemctl --user stop "semon-p4-$CASE.scope"
    break
  fi
  sleep 1
done >> "$RUN/$CASE.thermal.log" 2>&1
```

Abort on the 30-minute scope timeout, a failed query, or a watch sentinel that
fails to appear within the 60 attempts. Preserve private scratch logs for
Infra. Share only the binary revision, host, quota, corpus counts and bytes,
UTC starts and stops, exit status, peak temperature, elapsed/CPU times, peak
RSS, schema version, snapshot and sentinel freshness. Record the one-second
polling interval and any abort reason. The production timer last materialized
successfully at **2026-11-24 22:21:39 UTC**, with `asi_schema_version=5` and
`snapshot=128`. This establishes the last known production state. A relative
timing requires a paired run with the same inputs and quota.

## Decisions

Chief PM approved both choices on 2026-11-24.

- **Index admission.** Materialize invokes shared runtime-free admission. This
  preserves one-command cold start and fresh watch without a Viewer. A separately
  managed Index producer would give materialize a read-only process boundary and
  add setup and explicit stale-coverage handling.
- **Cutover.** Switch in phase 4. An earlier adapter over native evidence and
  committed identity and lineage would still need runtime-free admission and
  fact parity, then be replaced in phase 4. Collect the cold and warm one-shot
  timings and watch freshness measurements on hp-code-2 before implementation.

Close [#736](https://github.com/onsager-ai/semon/issues/736) after verifying that
neither materialize mode can capture or publish Codex names. Any stale-name
ordering issue reproduced between other Catalog writers belongs to the Catalog.
