<img src="crates/semon-sessions/src/favicon.svg" width="48" height="48" alt="Semon">

# Semon

Semon is Agent Session Intelligence (ASI): durable storage, query and analytics
over harness-native agent sessions collected from your machines. Native source
logs remain the source of truth; the incremental Session Event Index and Session
Catalog are the foundation for the ASI store, CLI/MCP query and analytics. Session
history is browsable through Semon's standalone read-only Viewer. Two-way harness
control is not a product goal; see the
[ASI direction](docs/design/asi-direction.md).

The core CLI lives in `semon-cli`; native session discovery, parsing and focused
reads live in `semon-sessions`, and non-SSH Push/receive lives in `semon-push`.
The Session Event Index and Catalog support native history, query and analytics.

Encrypted Relay and Canonical Trace Store are retired pre-release experiments.
The product does not ship their protocols, capture services or migration tools.
Control delivery, managed runtime and SSH are also retired, with their code and
active entry points removed. Local collection, non-SSH Push/receive, native
identity and lineage, passive questions and approvals from logs, archive and
unarchive, enrollment, authorization, export, and passive runtime observation
remain supported. With the `asi-query` feature, ASI SQL, schema, origin-local
full-text search and focused body views are available through CLI and MCP after
analytics materialization. Remote body access remains follow-up work. Retirement
does not delete user data, credentials, history or installed services. See the [session foundation contract](docs/design/session-foundation-retirement.md#collection-retirement-scope).

## Start with local history

With `semon` installed and on your PATH, run:

```sh
semon sessions --serve
```

Open the printed local launch URL to browse your history. Semon works without
T3 Code; the Viewer is read-only and does not start or control a harness.

For ASI SQL, full-text search and focused views, use a binary built with the
`asi-query` feature. The current `release-binary.yml` workflow artifact does not
include that feature; downloading it alone does not enable these commands.
With a query-enabled binary, prepare the local analytics once, then query it:

```sh
semon analytics materialize
semon query sql 'SELECT COUNT(*) AS sessions FROM fact_session'
semon query search 'timeout'
```

Use `semon query view FULL_ASI_UID` with a `body_uid` returned by search to inspect
a hit. `semon query schema` describes the analytics tables. Materialization refreshes
the full local Catalog; initial preparation can take time. It publishes one
snapshot and exits. For ongoing publication, run `semon analytics materialize
--watch` in another process; Viewer and MCP do not start that writer automatically.
These commands keep data local. Remote ASI enrollment/viewing remains integration
work; existing `push` copies redacted native logs and is a separate custody choice.

| Goal | Current command family |
| --- | --- |
| Browse stored native history | `semon sessions --serve` |
| Read analytics or local evidence | `semon query sql`, `schema`, `search`, `view` (`asi-query` required) |
| Produce and inspect analytics snapshots | `semon analytics materialize`, `coverage` |
| Preserve a published analytics snapshot | `semon analytics export DESTINATION` |
| Author or back up durable labels/work records | `semon asi add`, `backup` |
| Explicitly mirror native logs to another machine | `semon push` and `semon receive` |

`analytics reset` is exceptional destructive maintenance, not a first-run or
routine error-recovery step. Preserve unsupported state and follow the custody
guidance below rather than deleting the state directory to start over.

## Seeing your agent sessions

Run `semon sessions` to see a local read-only tree of Claude Code sessions and subagents alongside Codex runs and subagents. Live Claude sessions are checked against process start times, and Codex runs are checked against `/proc/locks`; the command never acquires their locks or writes to either agent home. A Codex run joins a Claude session only when its first user message carries a `Semon-Parent` marker. `Semon-Handoff` links it to the launching Bash or Skill call when that call can be found.

```sh
cargo run --locked -p semon-cli --bin semon -- sessions
cargo run --locked -p semon-cli --bin semon -- sessions --since 3d --json
cargo run --locked -p semon-cli --bin semon -- sessions --session THREAD_ID
cargo run --locked -p semon-cli --bin semon -- sessions --watch
cargo run --locked -p semon-cli --bin semon -- sessions --serve
```

The CLI, Viewer and Query share the native Session Event Index (`sessions-index.sqlite3`, owner-only, WAL). It records source identity, generations, indexed byte proofs, metadata and relationships. Links come from ids and markers the logs record: `toolUseId`, `msg_id`, `session_id`, `continued-in`, `bridgeSessionId`, `Semon-Parent`, `Semon-Handoff` and Codex `parent_thread_id`. When a handoff's other end is absent, its native stub remains queryable. Warm Claude/Codex trees use indexed summary and lineage evidence without decoding event or signal rows; changed sources use the incremental parser. Copilot trees still require those rows. Transcript and entry reads verify and read selected native source bytes. Retained native history keeps its original source generation and integrity proofs. Session handoffs named relay remain supported.

`--serve` prints a token-bearing URL and starts a local viewer on a random `127.0.0.1` port. Open that URL on this machine; `--listen 127.0.0.1:PORT` chooses a port. The first request sets an `HttpOnly; SameSite=Strict` cookie, and subsequent pages use it without keeping the token in the address bar. Home, recent sessions and machine pages read native overview metadata. Session navigation uses the native Catalog; a selected session loads its own generation-fenced transcript and relationship context. Thinking and tool details start collapsed, live transcripts follow new entries, and native/no-JavaScript pages retain navigation and recorded history. Normal Viewer reads do not prepare a complete workspace model. Catalog discovery and complete reconciliation still depend on the configured history; a cold observation may be provisional while background preparation continues. The local viewer requires the token or cookie and an exact loopback Host header, and sends no-store, CSP, frame and referrer headers. It never writes agent homes.

Native reads authorize a configured source key before resolving a session. `GET /api/session-catalog?machine=SOURCE` returns bounded metadata pages; its continuation cursor binds the published Catalog generation. `GET /api/session-transcript?machine=SOURCE&sid=KEY` reads a bounded selected transcript range, with `after` or `before` slot anchors and an optional expected `generation`. `GET /api/session-entry` expands a selected entry or recorded text field under the same source, session and generation checks. Current and retained history are explicit read scopes; neither grants execution authority. The reader contracts are documented in [docs/design/session-source-reader.md](docs/design/session-source-reader.md) and [docs/design/retained-native-projections.md](docs/design/retained-native-projections.md). Transcript data is placed in the page using text nodes so source HTML remains inert. The CLI `--json` tree schema is described below.

The Viewer is also a library API: `semon_sessions::ViewerCore` serves native pages, assets, Catalog, Transcript, Entry and focused relationship routes. The embedding server authenticates requests, supplies stable source keys, calls `respond` or `respond_with`, and sends the reply with `semon_sessions::SECURITY_HEADERS`. Run blocking native reads off an async runtime. The core supports concurrent callers through `Arc`, without a caller lock. Request-specific account menus belong in `Extras`. Native Catalog demand uses a shared `RefreshPool` in every mode. `Refresh::Background` observes source changes; a receiver can select `Refresh::OnInvalidate` and call `invalidate(key)` after a push. Catalog production does not construct a workspace Viewer model. `ViewerCore::with_machines` admits several native homes; `with_received` follows a receiver directory. Call `close` and drain active reads before changing source custody or removing files.

Server-rendered pages that share the viewer's visual language can use the documented shell contract in [docs/shell.md](docs/shell.md).

`semon push [--receiver NAME] [--watch]` keeps a copy of these logs on another server that implements the [mirror protocol](docs/mirror-protocol.md). It sends only supported native source files (`semon_sessions::inputs`), redacted on this machine before they leave (known secret shapes become `*`, byte for byte). It also sends this machine's facts: hostname, live processes, held Codex locks and repositories (`semon_sessions::local_facts`). A copy built with `--facts FILE` (or `Options::facts`) preserves the source machine’s recorded identity and runtime observations. Run `semon connect --to https://receiver.example:8735 --machine laptop` first; it installs private origin custody after code confirmation and explicit source approval. The client's cursor state is kept under `$XDG_STATE_HOME/semon/push/`, 0600. One push at a time runs with a state file: it holds an OS lock on `<state>.lock` beside it, and a second push with the same state (by default, to the same receiver URL, however its scheme, host case or trailing slash is spelled) fails at once until the first ends. Ctrl-C or SIGTERM ends a push, `--watch` included, within a moment, from wherever it was, with the state file recording what the receiver acknowledged. An app that embeds the push calls `semon_push::push_until` with a `Stop`, and can pass the token in memory (`Credential::Memory`) instead of a file; a request or facts collection that such a stop leaves running keeps the state locked until it ends (a request within two minutes, its timeout; a facts collection has none), and a push started meanwhile says it is still finishing.

Live Claude statuses observed so far are `busy`, `idle`, `shell` (a finished turn with background shells running, not counted as working) and `waiting` (stopped on a question or permission prompt, shown as needs you); any other status on a verified live process is shown verbatim. An unreadable transcript or rollout shows `unknown` state with whatever metadata is available. The default view contains roots active in the last 24 hours plus live roots. `--all` includes older sessions; `--since 2h` or `--since 3d` changes the window; `--session ID` selects one subtree. `--claude-home`, `--codex-home`, `--proc-root`, and `--cache` override every source and the cache for fixtures or alternate installations. The configured cache stem defaults to `$XDG_STATE_HOME/semon/sessions-index.json`, or `~/.local/state/semon/sessions-index.json`. Tree and focused reads share the Session Event Index at the same stem with extension `.sqlite3`; existing tree JSON files are left untouched. The origin-local index stores metadata, evidence locations and supported message/tool body text for ASI search and focused view. Body rows and FTS have the same custody as native logs; they never enter analytics or the Hub. Back up existing index/history data before an explicit reset.

JSON output has `schema_version: 1` and a `roots` array of nested nodes. Every node has `id`, `harness`, `kind`, `label`, `state`, `pid`, `models`, `cwd`, `branch`, `first_activity`, `last_activity`, `last_activity_age_seconds`, `tokens`, `malformed_lines`, `open_tools`, `claude_link`, `via_tool`, `unlinked`, and `children`. Nullable fields use `null`, `tokens` contains `input`, `cached_input`, `output`, `reasoning_output`, and `total`, and each tool has `id` and `name`. Timestamps are RFC 3339 strings from source records. `malformed_lines` counts complete lines that are not JSON objects; they are skipped, and text output shows `malformed:N` when the count is nonzero. The view includes metadata only; it never prints prompts, transcript text, tool inputs, or tool outputs.

Native `.jsonl.zst` history is included in normal `push` and `push --watch` runs,
with no manual decompression. The sender verifies archives in bounded slices,
redacts the logical records, and keeps the existing `.jsonl` mirror paths and
cursors. It continues discovering new archives and following live logs after
catchup. A private transport cache beside the cursor avoids decompressing the
same archive after every batch or restart; see the
[archive transport contract](docs/native-archive-transport.md) for limits and measurements.

## Receive pushes from your other machines

`semon receive` is a receiver for `semon push`, for one person with several machines, and the [mirror protocol](docs/mirror-protocol.md)'s reference implementation (`semon_push::mirror::Receiver`). Each machine pushes with its own token, and its copy lands in `DIR/machines/NAME/`: `claude/` and `codex/` hold the input files at their paths under each home, and `facts.json` holds the machine's facts. A token decides the machine, so one machine's token can't write another's files.

The receiver uses the enrollment registry and separate kind-scoped credentials.
Legacy `DIR/tokens` and `receive token` commands are refused. Preserve those files
and retained history; explicitly re-pair machines without importing or deleting
old state. Choose a fresh receiver directory for enrollment so legacy custody stays intact. A machine name is 1 to 63 of `a-z`, `0-9` and `-`.

Receiver-side pairing starts in a trusted local terminal:

```sh
semon receive enroll --machine laptop --dir ~/semon-mirror \
  --advertise https://receiver.example:8735 --listen 0.0.0.0:8735 \
  --tls-cert /private/server.pem --tls-key /private/server-key.pem
```

The receiver terminal displays a code when the origin connects and accepts
`pending` or `cancel SESSION_ID`. On the origin, run:

```sh
semon connect --to https://receiver.example:8735 --machine laptop
semon push --watch
```

Enter the receiver's code in the non-echoing controlling-terminal prompt, then
review the authenticated receiver identity, mirror-only scope and current source
list and enter `yes` to approve. No code, bearer or private key belongs in argv,
environment variables or a piped input. `connect` requires public-CA TLS 1.3;
there is no trust-on-first-use or skipped certificate verification. ts.net
hostnames appear in public certificate-transparency logs.

Origin custody lives under `$XDG_CONFIG_HOME/semon/origin/` (default
`~/.config/semon/origin/`), with mode-0700 directories and separate mode-0600 key,
credential, signed configuration and source-consent files. `connect` prints a
stable receiver name; use `push --receiver NAME` when more than one is enrolled.
Only explicitly approved current files are eligible for later pushes; future
sources require a new consent update (that update command is not shipped yet).
Analytics/body enrollment, client certificates and issuer trust are not enabled.

A running receiver checks current enrollment, kind and credential validity and
revocation on each request, including kept-alive connections. Machine credentials
never establish user identity or source authority. Revocation CLI arrives in PR4.

The receiver listens on `127.0.0.1:8735` unless `--listen ADDR` says otherwise. A non-loopback address is refused unless `--tls-cert PEM --tls-key PEM` are given; plain HTTP off loopback is always refused. These are the generic `semon receive` rules, implemented in `semon-push`. Semon never opens a port or obtains a certificate: the address, the firewall, DNS and the certificate stay your choices. `semon push` checks a certificate against the standard web roots only, so a TLS receiver needs a certificate that chains to them; configure a trusted certificate before using a remote receiver.

Its limits:

- A request body is at most 6 MiB (one append's 4 MiB of file bytes, as base64, plus the JSON around it). A larger declared body gets 413 before any of it is read. A body needs a `Content-Length`: without one, or chunked, it gets 411.
- The token is checked (401) before the body is read.
- At most 32 connections at once. A connection that hasn't yet passed the token check also counts against its address (an IPv6 one by its /64), which may have 4 such; once a request on it is authenticated it counts only toward the 32, so machines behind one address (for example, behind a NAT) each keep their `--watch` connection. One more over either limit is closed as soon as it is accepted.
- Every read and write, the TLS handshake's included, runs against a deadline, so trickling bytes doesn't stretch it. The first request's head must be in within 10 s of the connection being accepted, a later one's within 10 s of its first byte, and a body within 120 s. A kept-alive connection closes after 30 s idle, and no single read or write waits more than 30 s.
- Each machine's copy holds at most 20 GiB (`--max-bytes SIZE`, as in `500G`), counted by a walk of `DIR/machines/` at start and then by each write. A push that would pass it gets 507, and `semon push` reports the failure and stops that pass. To recover, raise `--max-bytes` and restart the receiver. Deleting copies under `DIR` doesn't lower the count until a restart, and the machine sends deleted copies again anyway.
- Paths are checked with `semon_sessions::is_input_path`. The receiver creates its directories itself and refuses a request whose path meets a symbolic link.
- Each file has one writer at a time. A 200 is answered only once the bytes are synced to disk. An append that fails partway is cut back off before the 500, so the copy is as it was. A replace is a rename, so the copy is the old one or the new one; if only the directory sync after the rename fails, the answer is 500 with the new copy in place. A replace's temporary files left by a crash are removed when the receiver starts.

To see the received machines, serve `DIR` with `semon sessions --serve --machines DIR` ([See all your machines](#see-all-your-machines)).

## See all your machines

`semon push` sends a machine's session logs, and `semon receive` writes each machine it receives under one directory, `DIR/machines/<name>/`: that machine's `claude/` and `codex/` input files, and its `facts.json`. `semon sessions --serve --machines DIR` shows every one of them in one viewer, next to this machine's own sessions:

```sh
semon sessions --serve --machines ~/semon-machines              # this machine and every received one
semon sessions --serve --machines ~/semon-machines --no-local   # the received machines only
semon sessions --json --machines ~/semon-machines               # the tree, each node with its machine
```

Every session keeps its machine, each machine answers for its own transcripts, and nothing is linked across machines. The viewer follows DIR while it runs: the stat pass that already decides whether to rebuild also looks at `DIR/machines/`, so a machine received after the viewer started appears on the next poll, and one removed goes away. Only directories named with `a-z`, `0-9` and `-` (1 to 63 of them) are machines; any other entry is ignored, with a warning. A machine whose `facts.json` is missing or can't be read is still shown, offline and named after its directory, with a warning on stderr; it comes online when its facts arrive. `semon push --watch` rewrites a machine's facts every 10 seconds, so facts over two minutes old make it offline since they were written, with nothing running. A received hostname that can't name a machine (empty, over 253 bytes, or holding whitespace, a control character or `~`) gives way to the directory's name. A received machine is told apart from this one even when it has the same hostname: it is shown as `<hostname>~<name>`. A session an earlier machine already has (this machine's own push, or a copied home) is left out of the received machine, with a warning naming how many; use `--no-local` when this machine pushes to DIR. Received machines are read with the same rules as this one's homes: the model indexes every log, and the tree takes `--since` and `--all`.

DIR is only read. Nothing is written there, and no symbolic link is followed out of a machine's directory: a machine directory, a home (`claude/`, `codex/` or their `projects/` and `sessions/`), a `facts.json` or a log file that is a link is ignored. What the viewer keeps for each received machine (its metadata cache and the facts it serves) goes beside `--cache`, under `received/`.

`--machines` changes nothing about who can reach the viewer: it listens only on `127.0.0.1`, and requires the token URL (then its cookie) and an exact local Host header. Open the printed URL on that machine.

## For agents: `semon query` and `semon mcp`

Native session evidence is shaped for agents and scripts. The focused `semon query` tools print one answer as JSON (one line with `--json`, pretty without); `semon mcp` serves those tools as a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio. Focused local tools construct native evidence owners directly and reuse the Session Event Index without starting Viewer refresh workers. They take the home options of `semon sessions` (`--claude-home`, `--codex-home`, `--proc-root`, `--cache`, `--facts`, and `--machine DIR` repeated for several machines' homes), and are read-only: no listener, no network, and nothing written but the metadata cache. With `asi-query`, MCP additionally offers the explicit `asi_add` durable authoring tool described below.

The focused native tools read a window of recent logs: only the log files modified in the last 30 days, so a first run on a large home doesn't read years of history. `--since DURATION` sets another window, and `--all` reads every file. A file that is read is answered whole: a session that began before the window keeps every turn, handoff and busy interval its files hold. A link whose other end is in a file outside the window isn't made; that end shows as a stub or unlinked, as it would if its file were gone. A session whose earlier file (after a `/clear`) a log line names by `session_id` but the window didn't read says `turns_truncated: true`; it is then listed under its first read file's id. Every answer carries `window_start` (epoch ms, `null` with `--all`): where the window of the model that answered starts, the same instant the scan used. A long-running `semon mcp` rebuilds only when the logs change, so its `window_start` can be older than 30 days back, never newer. Over several machines each keeps its own window, and `window_start` is the latest of their starts. A tool's own `since` argument filters inside the window; on the command line it is spelled `--newer-than`, because `--since` is the window. A `since` that reaches before `window_start` is the error `outside_window`, with the `window_start`; the answer is never silently cut short.

| Tool | Arguments | Returns |
|---|---|---|
| `list_sessions` | `state`, `repo`, `since`, `parent`, `harness`, `limit` | session summaries, most recently active first |
| `get_session` | `id` | the summary, busy intervals, turns (each incoming message and how the turn ended) and every handoff to or from it |
| `get_session_activity` | `id`, `kind` (`steps` / `facts`), `generation`, `cursor`, `limit` | typed metadata pages, capped at 256 items; continuation requires the returned generation and cursor |
| `read_transcript` | `id`, one of `before` / `after` / `turn`, `limit` | a source-backed native transcript page, sharing Viewer slot ordering and rendering |
| `stalls` | `idle_minutes`, `since` | sessions quiet for that long that are working, wait on a question, or whose process died while its pid file said busy, each with a `stall_reason` |
| `sql` | `query`, `params`, `limit`, `timeout_ms` | read-only DuckDB SQL over the current local analytics snapshot (build with `--features asi-query`) |
| `search` | `query`, `repo`, `harness`, `part`, `since`, `until`, `limit` | origin-local FTS5 hits with untrusted snippets (build with `--features asi-query`) |
| `view` | `uid`, `part` | focused origin-local body, step or turn text by full ASI UID (build with `--features asi-query`) |
| `schema` | `table` | generated ASI analytics schema reference (build with `--features asi-query`) |

`sql` and `schema` read the current local analytics snapshot or generated schema rather than the native-log window; they return compact JSON by default (`--json` is accepted but redundant). `sql --format tsv` prints a tab-delimited header and rows; tabs, newlines and carriage returns in column names and values become spaces. Typed query failures print `{"error":{"code","message","detail"}}` on stdout and exit 1; successful commands exit 0. MCP exposes the generated JSON reference at `semon://asi/schema`. `search` and `view` also default to compact JSON and require materialization; their text is wrapped as untrusted data. Search time filters use inclusive UTC milliseconds (`--since` and `--until`). Remote body queries are refused.

Session summaries add nullable `activity_analytics`: ownership/context totals, compactions and per-session thrash, waste and empty-search signals, typed change counts and cost completeness. It measures wait time from exact interval unions and reports exclusive waste time and list cost by kind and category. Exact parent families receive aggregate totals only; wait measurements sum each physical session's union. Measurements retain known amounts and unknown-step coverage. See [session activity analytics](docs/design/session-activity-analytics.md) for fields and roll-up semantics. Activity pages contain no raw bytes, source paths, commands or output bodies.

A summary has `id`, `harness`, `kind` (`session`, `subagent`, `codex-run`, `stub`), `name`, `model`, `repo`, `branch`, `machine`, `parent`, `children`, `state`, `start`, `last_activity`, `pid`, `alive`, `exit`, `open_question`, `tokens` (`input` uncached, `cached`, `output`), `run` and `turns_truncated`. Times are epoch milliseconds. `state` is one of `working`, `waiting-question`, `idle`, `ended`, `error` and `unknown`; `stall_reason` is one of `waiting-question`, `no-output` and `process-gone`, and every `stalls` answer states its rule. A field Semon can't know exactly is `null`: `pid` and `alive` come only from a Claude pid file checked against its process's start time, or from a held Codex writer lock. `run` holds a live session process's run ids, read from `/proc/<pid>/environ` by exact name: `OSTROM_RUN_ID` and `OSTROM_WORK_ORDER_ID` ([Ostrom's run environment contract](https://github.com/onsager-ai/ostrom/blob/main/docs/loops.md)), and nothing else. Every other entry of the environment is dropped as it is parsed, never kept or logged. `run` is `{}` when the process has neither variable, and `null` when there is no live process (a subagent has none of its own) or its environment can't be read whole: unreadable, over 256 KB, or a run id that isn't UTF-8. The facts `semon push` sends carry each live process's run ids, so a mirror answers `run` without reading `/proc`. `exit` is `null`: no log records a run's exit status (a Codex `task_complete` error is a turn's outcome, shown as the state `error`), and none is inferred. Turns and handoffs use the same native identities across Query, Transcript and Viewer reads. `since` is a duration back from now (`90m`, `2h`, `7d`) or an RFC 3339 time. A failure has a closed `code` (`unknown_session`, `unknown_turn`, `invalid_arguments`, `outside_window`, …): `semon query` exits nonzero, and `semon mcp` returns a tool result with `isError`.

```sh
semon query list_sessions --state working --json
semon query get_session SESSION_ID
semon query get_session_activity SESSION_ID --kind steps --limit 100
semon query read_transcript SESSION_ID --before 200 --limit 50
semon query stalls --idle-minutes 30
semon query list_sessions --since 90d --newer-than 60d   # a wider window
semon query get_session SESSION_ID --all                 # every log file
```

## Local analytics materialization

`semon analytics materialize` is the explicit, local-only entry point for
analytics publication. It drains the Viewer's native Catalog producer in strict
mode, then writes Parquet under `<index parent>/analytics-store`; Viewer,
MCP, push and receive never enable it implicitly. The first run records a
random namespace in `<index parent>/analytics-namespace`, and later runs reuse
it. Deleting that state directory creates a new namespace and new session IDs;
there is no automatic reconciliation or rename/transfer workflow yet. Typed
local paths can be present in the Parquet, so governed export remains a
separate operation. Writer ownership is acquired before namespace creation or
Catalog refresh; contention returns `analytics writer busy` without changing the
index or namespace. Corrupt or unsupported indexes are refused unchanged.
Catalog refresh is complete; `--since`/`--all` do not bound analytics input.
`--batch-sessions N` limits sessions or journal entries per chunk (default 64),
and `--batch-steps N` limits admitted steps (default 8192). Both limits must be
positive integers no greater than `min(usize::MAX, i64::MAX)` for the build
(9223372036854775807 on a 64-bit build); a session exceeding the step budget
stays whole. One-shot runs drain chunks immediately through an endpoint captured
for that request. An inherited unfinished replay is completed first, then a
fresh drain runs before success; writes after that fresh endpoint belong to the
next request. Watch uses a minimum spacing of 2 s between chunk attempts.

A publication chain owns its request generation, progress and a budget of three
operational failures across opening the store, reload barriers, chunks and
compaction. Success within that chain never resets the budget; retries wait
1s/2s plus publication spacing. Periodic retirement has its own maintenance
chain and three-failure budget, suspending and restoring the entire publication
chain. Its retries can add failures to the overall wait; they neither spend nor
reset the publication budget. Terminal refusal, NoWriter, owning-waiter
cancellation and shutdown drop the chain. The next chain starts at zero.
Each request waits for all required chunks and final compaction. `--watch` exits
on refresh or publication failure and cancels its wait on SIGINT/SIGTERM before
joining the writer and re-raising the signal.

```sh
semon analytics materialize --all
semon analytics materialize --all --batch-sessions 16 --batch-steps 4096
semon analytics materialize --watch --since 7d
```

`semon analytics coverage` reads one already-published analytics snapshot without
starting the writer or taking its lock. It groups live `fact_session` rows by their
recorded transform, rules, price and taxonomy versions plus source/computation
status, and reports session and step coverage. The manifest versions are displayed
as writer **TARGETS**, not as a claim that every session refreshed uniformly.
Native pending, failed and frozen derivation state is not part of this report.
Like the writer, it refuses a symlinked state or store directory and a store without its
`analytics-namespace`. It holds the live map in memory, as the writer does, so its memory
grows with the number of live sessions.

```sh
semon analytics coverage
semon analytics coverage --json
```

`semon analytics export DESTINATION` makes a verified local bundle of the current
published snapshot. The destination must be empty and outside the store. The bundle
contains the namespace, manifest, live map, referenced immutable Parquet files and an
`export.json` receipt written last with paths, sizes and SHA-256 hashes. Export proves
that every live session UID belongs to the local namespace and only shares the existing
`analytics.lock`; if materialization holds it, export refuses rather than copying a torn
snapshot.

Author-owned labels and work records live separately in `asi-durable.sqlite3`.
`semon asi backup PATH [--cache PATH]` uses SQLite online backup and refuses an
existing destination. Analytics exports include categorical label assertions, never
durable free text; analytics rebuild/reset preserves the durable file. The Rust
writer API is available in `semon-asi`. Add a row with
`semon asi add lane --id my-lane --values row.json [--cache PATH]`, where
`row.json` contains a JSON object such as `{"title":"Investigate query latency"}`;
use `--values -` for stdin. The same shape supports all nine durable tables.
Success prints only `id` and `revision`. With `asi-query`, local stdio MCP offers
`asi_add` (`table`, `id`, `values`) against its configured store, using the same
Writer validation and lock. See [durable state](docs/design/asi-store.md#durable-state).

Choose backup operations by what you need to preserve:

| Operation | Preserves | Does not replace |
| --- | --- | --- |
| `semon analytics export DESTINATION` | Verified current published analytics snapshot and its namespace | Durable free-text backup, native history/archive custody, private enrollment state or the egress-handle mapping |
| `semon asi backup PATH` | Author-owned durable SQLite state, including its local body fields | Analytics snapshot export or native history/archive custody |

Neither command is a complete machine backup. Keep destinations private, and
preserve native evidence and continuity-critical state separately under the
[operator backup/recovery guidance](docs/design/asi-operator-journey.md#backup-and-recovery-beyond-the-two-shipped-export-commands).
Do not copy a running SQLite file as a substitute for its supported backup API.

`semon analytics reset` is explicit destructive maintenance for the analytics store
only. It requires `--confirm` with the exact `analytics-namespace`, plus exactly one of
`--verified-export DIR` or the clearly named `--without-export` waiver. A verified bundle
must be outside the store and still match the current manifest, live map, referenced files
and namespace bytes under the exclusive lock. The waiver can discard imported or frozen
facts that cannot be rebuilt. Reset does not touch native logs, archives or the SQLite
Event Index, and keeps the namespace by default so the next materialization retains session
UIDs. `--new-namespace` removes that namespace while the writer lock remains held; the next
materialization generates a fresh namespace and does not reconcile identities.

```sh
semon analytics export ./analytics-backup
semon analytics reset --confirm origin-... --verified-export ./analytics-backup
semon analytics reset --confirm origin-... --without-export --new-namespace
```

To give Claude Code's agents these tools, add the server once; user scope makes it available in every project. It reads the last 30 days; add `--since 7d` for a lighter first run or `--all` for everything:

```sh
claude mcp add --scope user semon -- semon mcp --since 30d
```

Or put it in a project's `.mcp.json`:

```json
{
  "mcpServers": {
    "semon": { "command": "semon", "args": ["mcp", "--since", "30d"] }
  }
}
```

## Copilot CLI saved-state support

Linux Copilot CLI 1.0.90/1.0.91 schema 1 sessions are browsed directly with
`semon sessions --copilot-home PATH`, without running `semon-copilot` capture.
`COPILOT_HOME` supplies the default override. Copilot event files also participate
in redacted `semon push`/receive and multi-machine viewing. Usage is a cumulative
saved snapshot; missing outcomes, approvals, fresh usage and logical lineage stay
unknown. Native homes are read-only. See [supported versions, commands and
acceptance limitations](docs/copilot-cli.md).

## Development checks

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Browser checks use the locked tooling in `tests/ui/` (`npm ci` and
`npx playwright install --with-deps chromium`); `.github/workflows/ui.yml`
contains the fixture and server setup. CI makes no pixel comparison; the
checks' screenshots are uploaded as artifacts for review. The Linux browser
launcher uses the checked-in `tests/ui/fontconfig.conf` to assign `rgba` to `rgb`,
matching Ubuntu CI without changing the host configuration or system fonts.

## Harness icons

Semon shows each harness's official icon to identify where a session came from. The unmodified files are in
`assets/harnesses/`, with their sources listed in `assets/harnesses/NOTICE.md`. They are not covered by this
repository's Apache-2.0 license.

Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.

Model reference prices refresh automatically in long-running processes; see
[runtime pricing](docs/runtime-pricing.md) for offline fallback and configuration.
