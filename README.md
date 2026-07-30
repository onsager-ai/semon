# devlog

devlog automatically collects local Claude Code and OpenAI Codex activity for
offline analysis. Claude Code sends its native OpenTelemetry events and metrics
to an OpenTelemetry Collector. A small, resumable Python tailer translates
Codex's JSONL records to OTLP. ClickHouse stores both raw OTel signals and a
harness-neutral `devlog.events` table.

This store intentionally contains full prompts, responses, tool inputs, and
tool outputs. It can therefore contain credentials, source code, commercial
data, and data from multiple repositories. Every published port is bound to
`127.0.0.1`; do not change that or host the store remotely without making and
reviewing a new security decision.

## Start the local stack

Docker with Compose v2 is the only stack dependency:

```sh
docker compose up -d
docker compose ps
```

ClickHouse is available locally on HTTP port `8123` and native port `9000`.
OTLP/gRPC is on `4317`; OTLP/HTTP is on `4318`. The local ClickHouse login is
`devlog` / `devlog-local`.

The collector uses a disk-backed sending queue, so a collector restart retains
accepted-but-not-yet-exported telemetry. ClickHouse data is in a named Docker
volume. The exporter creates its raw `otel.*` tables, and the one-shot
`schema-init` service creates the normalized table and materialized view.

Check that initialization completed:

```sh
docker compose ps --all
docker compose exec clickhouse clickhouse-client \
  --user devlog --password devlog-local \
  --query "SHOW TABLES FROM devlog"
```

## Enable Claude Code

Add this block to your interactive shell configuration. The function keeps the
usual `claude` command name but derives `devlog.repo` from the current Git
worktree every time Claude starts. After this one-time setup, collection is a
side effect of normal work; there is no per-session logging step.

```sh
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_LOGS_EXPORTER=otlp
export OTEL_METRICS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4317
export OTEL_LOG_USER_PROMPTS=1
export OTEL_LOG_ASSISTANT_RESPONSES=1
export OTEL_LOG_TOOL_DETAILS=1
export OTEL_LOG_TOOL_CONTENT=1

claude() {
  local devlog_root devlog_repo
  devlog_root=$(command git -C "$PWD" rev-parse --show-toplevel 2>/dev/null \
    || printf '%s' "$PWD")
  devlog_repo=${devlog_root##*/}
  OTEL_RESOURCE_ATTRIBUTES="devlog.repo=${devlog_repo}" command claude "$@"
}
```

If other resource attributes are required, append them inside the function;
OpenTelemetry resource attributes are comma-separated and values must not
contain spaces or commas.

`OTEL_LOG_TOOL_CONTENT=1` also applies if Claude Code's optional beta tracing
is enabled later. The collector already has a traces pipeline, but tracing is
not required for the log and metric pipeline described here.

## Run the Codex tailer

The tailer requires Python 3.10 or newer and uses only the standard library.
One invocation reads all complete, not-yet-exported lines and exits, which
makes it safe to run from a timer:

```sh
./scripts/codex_tailer.py --verbose
```

It reads:

- `~/.codex/sessions/**/*.jsonl`
- `~/.codex/history.jsonl`

Offsets and the small amount of call-correlation context are stored atomically
in `~/.local/state/devlog/codex-tailer.json` (or under
`$XDG_STATE_HOME`). An offset advances only after the collector returns a
successful OTLP response. Truncated files restart at offset zero, and an
incomplete final JSONL line is left for the next run.

For automatic collection, install the included systemd user timer:

```sh
./scripts/install-user-timer.sh
systemctl --user status devlog-codex-tailer.timer
```

Systems without systemd can either run `codex_tailer.py` from their timer of
choice or keep it polling:

```sh
./scripts/codex_tailer.py --watch --interval 10
```

Useful overrides are `--endpoint`, `--state`, `--sessions`, `--history`, and
`--repo`. See `--help` for the complete list.

## Data model

`otel.otel_logs` and the `otel.otel_metrics_*` tables are the collector's raw
store. `devlog.events` is the analysis surface:

```text
harness, session_id, repo, ts, kind, tool, decision,
tokens_in, tokens_out, cost_usd, duration_ms,
extras, prompt, response, tool_input, tool_output
```

`extras` is ClickHouse's native JSON type. Harness-specific and evolving fields
stay there so the shared columns remain narrow. Claude's `api_request` log
events and Codex's per-request `token_count` records both normalize to
`kind = 'api_request'`; analysis does not branch on harness. Codex currently
does not expose cost, so its `cost_usd` is zero.

No TTL is configured. Content retention was explicitly decided to be
indefinite, and the metadata retention window remains an open human decision.
Adding a TTL before that decision would silently discard the content carried
by the same rows.

## Analysis v1

[`analysis/queries.sql`](analysis/queries.sql) contains ready-to-run queries
for:

- seven-day cost and token use by repository and harness
- tool failure rates
- permission-denial clusters
- the ostrom#6 approval pass-rate
- human-intervention clustering by workstream

Run all queries with:

```sh
docker compose exec -T clickhouse clickhouse-client \
  --user devlog --password devlog-local --multiquery \
  < analysis/queries.sql
```

## Development checks

```sh
python3 -m py_compile scripts/codex_tailer.py
python3 -m unittest discover -s tests -v
sh -n clickhouse/install.sh scripts/install-user-timer.sh
docker compose config --quiet
```
