<img src="crates/semon-sessions/src/favicon.svg" width="48" height="48" alt="Semon">

# Semon

Semon collects, synchronizes, browses and queries harness-native agent sessions.
Native source logs, the incremental Session Event Index, and Session Catalog form
the foundation for Viewer, CLI, MCP, runtime integration and future Session
Intelligence.

The core CLI lives in `semon-cli`; session discovery/parsing and focused reads live
in `semon-sessions`, generic mirror synchronization in `semon-push`, SSH onboarding
in `semon-ssh`, and runtime/control integration in `semon-runtime`/`semon-control`.

The experimental Canonical Trace Store (`semon-store`), Encrypted Relay
(`semon-relay`), and full-workspace Viewer compatibility are being retired in
dependency-safe stages. The three Trace Store capture packages are retired.
Existing trace, forensic and encrypted-data commands remain available during
migration. The CLI
temporarily retains those dependencies; extraction alone does not remove them.
See the [verified assessment, component decisions and migration path](docs/design/session-foundation-retirement.md).
The Session Event Index and source-backed reads are retained.

The former OTLP, OpenTelemetry Collector, ClickHouse, and analytics pipeline is
intentionally gone. Semon is local-first and does not configure or assume a
central endpoint.

Historical raw forensic records can contain prompts, responses, source code,
credentials, commercial data, and machine paths. They are retained in full and
indefinitely, with no automatic pruning. The legacy collectors retained every
complete consumed source line. The store file is created `0600` and its parent directory
`0700`, and both are re-tightened on every open rather than trusted from a
prior run. That access control is the only remaining protection; keep the
database local, and a private repository is not a safe destination for
captured session data. No real session fixture should be committed.

## Seeing your agent sessions

Run `semon sessions` to see a local read-only tree of Claude Code sessions and subagents alongside Codex runs and subagents. Live Claude sessions are checked against process start times, and Codex runs are checked against `/proc/locks`; the command never acquires their locks or writes to either agent home. A Codex run joins a Claude session only when its first user message carries a `Semon-Parent` marker. `Semon-Handoff` links it to the launching Bash or Skill call when that call can be found.

```sh
cargo run --locked -p semon-cli --bin semon -- sessions
cargo run --locked -p semon-cli --bin semon -- sessions --since 3d --json
cargo run --locked -p semon-cli --bin semon -- sessions --session THREAD_ID
cargo run --locked -p semon-cli --bin semon -- sessions --watch
cargo run --locked -p semon-cli --bin semon -- sessions --serve
cargo run --locked -p semon-cli --bin semon -- sessions --model-json --all
```

`--model-json` writes the viewer's session model: sessions, the handoffs between them (your messages, subagent and Codex spawns, relays, and questions and results to you), each session's turns, and busy intervals. The server returns the same JSON at `/api/model`, with an `ETag`; `?since=<version>` or `If-None-Match` answers 304 while nothing changed. Links come only from ids the logs record (`toolUseId`, `msg_id`, `session_id`, `continued-in`, `bridgeSessionId`, the Codex marker or `parent_thread_id`); when the other end isn't in the logs, a stub stands in and the handoff is marked `unmatched`. It indexes every log so links resolve; `--all` or `--since` only trim what it returns, and it doesn't take `--session`. Its event index is kept beside the tree's cache in a SQLite file (`sessions-index.sqlite3`, owner-only, WAL), so the tree, `--watch` and `--json` never load it; it holds kinds, byte offsets and ids only, and each file's change commits in one transaction as it is read, so a viewer and `semon query` can share it. If the file can't be opened, the index is kept in memory for that run; a damaged file is set aside as `sessions-index.sqlite3.corrupt` and rebuilt. The JSON event cache older versions kept (`sessions-index.events.json`) is imported whenever it is found (its reported runs, which `~/.claude.json` no longer holds, without replacing any the index has) and then removed; one that can't be read is kept as `sessions-index.events.json.corrupt`. Briefs, answers and results are read back from the source line when the model is built and kept in memory, in a bounded cache.

`--serve` prints a token-bearing URL and starts a local viewer on a random `127.0.0.1` port. Open that URL on this machine; `--listen 127.0.0.1:PORT` chooses a port. The first request sets an `HttpOnly; SameSite=Strict` cookie, and subsequent pages use it without keeping the token in the address bar. The tree at `/` polls `/api/tree` every five seconds, and a transcript page polls for new entries the same way; both pause while the tab is hidden and refresh once when it becomes visible again. `/s/<harness>/<id>` shows a transcript with collapsed thinking, reasoning, tool input, and tool output; linked child sessions open from their parent call or the child links. A live transcript follows new entries. Each request is answered at once from the model last built, kept in memory, and never waits for a rebuild: while pages keep asking, a small pool of refresh threads (at most four) takes a stat pass over each machine's watched directories and files (plus process and lock liveness) every 250 ms and rebuilds only when it finds a change, at most once a second, so a page is at most about a second plus one rebuild behind the logs. Idle, the server does no work at all: a machine is no longer checked 30 s after the last request, the pool's threads stop 30 s after that, and the server blocks in the HTTP accept loop. The viewer serves only on IPv4 loopback, requires the token or cookie and an exact local Host header, accepts GET only, and sends no-store, CSP, frame, and referrer headers. It never writes agent homes.

The JSON tree route is `GET /api/tree` and has the same schema as `--json`. Transcript pages use `GET /api/transcript?harness=claude|codex&id=ID&before=N` to page backward from `before` (an opaque byte cursor returned by the previous page; omit it to open at the end), or `&after=N` to read only entries appended past a previously served page's `end`; `before` and `after` are exclusive. Paging always reads backward from the end or forward from a cursor in fixed-size chunks, never the whole file. A page contains `entries`, `before`, `end`, `live`, and `children`, with at most 200 entries or about 2 MB of preview data. Each entry has `offset`, `block`, `kind`, `text`, `name`, `tool_id`, `link`, `collapsed`, and `truncated`; `offset` is also the paging cursor. Tool input and results over 64 KB have a preview and `truncated: true`; `GET /api/entry?harness=...&id=...&offset=...&block=...` expands one entry, capped at 8 MB. Transcript data is sent as JSON and placed in the page using text nodes, so source HTML remains inert.

The viewer is also a library API, so another server can embed it: `semon_sessions::ViewerCore` answers the same pages, assets and API routes without a listener, token or Host check of its own. The embedding server authenticates each request itself, calls `ViewerCore::respond` with the method, path, query and `If-None-Match`, and sends the returned status, body and `ETag` with `semon_sessions::SECURITY_HEADERS`. A call does blocking file I/O, so an async server makes it off its runtime. `ViewerCore::respond` takes `&self`: share the core in an `Arc` and call it from as many threads at once as there are requests, with no lock around it. Values that differ per request (an account menu, say) go with the call, as `Extras` to `ViewerCore::respond_with`. With `ViewerCore::set_refresh(Refresh::Background)`, as `--serve` uses, a read answers from the last model built and a background check every 250 ms rebuilds when the logs change. A server that receives the logs itself uses `Refresh::OnInvalidate` instead and calls `ViewerCore::invalidate(key)` once a machine's new data is written: no stat pass every 250 ms, one rebuild for however many calls come in a second, and a check every 30 s as a safety net. Either way the checks run on a `semon_sessions::RefreshPool`, shared by every core in the process unless `ViewerCore::set_refresh_pool` gives one its own, so the refresh threads stay at the pool's size however many machines are served; when the pool falls behind, a read whose machine's check is over a second late refreshes that machine itself. The default, `Refresh::OnRead`, rebuilds before the read that finds a change answers. In either background mode a page never waits for a build: only the API routes refresh. `ViewerCore::warm` builds every machine's model now, on the calling thread and without counting as a read, for an embedding server that wants models ready before anyone reads them; `ViewerCore::warm_machine(key)` does it for the one machine it just wrote. `ViewerCore::close` stops a core, waiting out its background rebuilds, before the files it reads (its caches included) are removed. `ViewerCore::with_machines` serves several machines' homes (each with its own `Options`, facts included) as one model: every session keeps its machine, each machine answers for its own transcripts, and nothing is linked across machines. `semon sessions --model-json --machine DIR --machine DIR …` prints that model. `ViewerCore::with_received` also follows the machines a receiver keeps under one directory (`semon_sessions::ReceivedMachines`), as `semon sessions --machines` does below.

Server-rendered pages that share the viewer's visual language can use the documented shell contract in [docs/shell.md](docs/shell.md).

`semon push --to URL --token-file PATH [--watch]` keeps a copy of these logs on another server that implements the [mirror protocol](docs/mirror-protocol.md). It sends only the files the model builder reads (`semon_sessions::inputs`), redacted on this machine before they leave (known secret shapes become `*`, byte for byte). It also sends this machine's facts: hostname, live processes, held Codex locks and repositories (`semon_sessions::local_facts`). A copy built with `--facts FILE` (or `Options::facts`) gives the same model as the original. The token file must be mode 0600, and the client's cursor state is kept under `$XDG_STATE_HOME/semon/push/`, 0600. One push at a time runs with a state file: it holds an OS lock on `<state>.lock` beside it, and a second push with the same state (by default, to the same receiver URL, however its scheme, host case or trailing slash is spelled) fails at once until the first ends. Ctrl-C or SIGTERM ends a push, `--watch` included, within a moment, from wherever it was, with the state file recording what the receiver acknowledged. An app that embeds the push calls `semon_push::push_until` with a `Stop`, and can pass the token in memory (`Credential::Memory`) instead of a file; a request or facts collection that such a stop leaves running keeps the state locked until it ends (a request within two minutes, its timeout; a facts collection has none), and a push started meanwhile says it is still finishing.

Live Claude statuses observed so far are `busy`, `idle`, `shell` (a finished turn with background shells running, not counted as working) and `waiting` (stopped on a question or permission prompt, shown as needs you); any other status on a verified live process is shown verbatim. An unreadable transcript or rollout shows `unknown` state with whatever metadata is available. The default view contains roots active in the last 24 hours plus live roots. `--all` includes older sessions; `--since 2h` or `--since 3d` changes the window; `--session ID` selects one subtree. `--claude-home`, `--codex-home`, `--proc-root`, and `--cache` override every source and the cache for fixtures or alternate installations. The cache defaults to `$XDG_STATE_HOME/semon/sessions-index.json`, or `~/.local/state/semon/sessions-index.json`; it contains metadata summaries only and can be deleted safely.

JSON output has `schema_version: 1` and a `roots` array of nested nodes. Every node has `id`, `harness`, `kind`, `label`, `state`, `pid`, `models`, `cwd`, `branch`, `first_activity`, `last_activity`, `last_activity_age_seconds`, `tokens`, `malformed_lines`, `open_tools`, `claude_link`, `via_tool`, `unlinked`, and `children`. Nullable fields use `null`, `tokens` contains `input`, `cached_input`, `output`, `reasoning_output`, and `total`, and each tool has `id` and `name`. Timestamps are RFC 3339 strings from source records. `malformed_lines` counts complete lines that are not JSON objects; they are skipped, and text output shows `malformed:N` when the count is nonzero. The view includes metadata only; it never prints prompts, transcript text, tool inputs, or tool outputs.

## Receive pushes from your other machines

`semon receive` is a receiver for `semon push`, for one person with several machines, and the [mirror protocol](docs/mirror-protocol.md)'s reference implementation (`semon_push::mirror::Receiver`). Each machine pushes with its own token, and its copy lands in `DIR/machines/NAME/`: `claude/` and `codex/` hold the input files at their paths under each home, and `facts.json` holds the machine's facts. A token decides the machine, so one machine's token can't write another's files.

On the machine that receives, make a token per machine. It is printed once, and `DIR/tokens` (0600, in a 0700 directory) keeps only its SHA-256 and the name. A name is 1 to 63 of `a-z`, `0-9` and `-`. A missing `DIR` is created 0700; an existing `DIR`, or `DIR/tokens`, that its group or others can use is refused with the `chmod` that fixes it, and never changed.

```sh
(umask 077; semon receive token add laptop --dir ~/semon-mirror > laptop.token)
semon receive token list --dir ~/semon-mirror
semon receive --dir ~/semon-mirror            # listens on 127.0.0.1:8735
```

Put the token on that machine in a 0600 file, and point `semon push` at the receiver. The simplest route keeps the receiver on loopback and forwards a port over SSH, since plain `http` to a loopback address is allowed:

```sh
ssh -N -L 8735:127.0.0.1:8735 receiving-host &
semon push --to http://127.0.0.1:8735 --token-file ~/.config/semon/laptop.token --watch
```

`semon receive token revoke laptop --dir ~/semon-mirror` removes a token. A running receiver re-reads `DIR/tokens` when the file changes, so the next request with that token gets 401 without a restart. If the file stops parsing, or its mode is loosened, no token is accepted until it is fixed.

The receiver listens on `127.0.0.1:8735` unless `--listen ADDR` says otherwise. A non-loopback address is refused unless `--tls-cert PEM --tls-key PEM` are given and `DIR/tokens` holds at least one token; plain HTTP off loopback is always refused. These are the generic `semon receive` rules, implemented in `semon-push`. Semon never opens a port or obtains a certificate: the address, the firewall, DNS and the certificate stay your choices. `semon push` checks a certificate against the standard web roots only, so a TLS receiver needs a certificate that chains to them; otherwise use the SSH route above.

Its limits:

- A request body is at most 6 MiB (one append's 4 MiB of file bytes, as base64, plus the JSON around it). A larger declared body gets 413 before any of it is read. A body needs a `Content-Length`: without one, or chunked, it gets 411.
- The token is checked (401) before the body is read.
- At most 32 connections at once. A connection that hasn't yet passed the token check also counts against its address (an IPv6 one by its /64), which may have 4 such; once a request on it is authenticated it counts only toward the 32, so machines behind one address (an SSH tunnel, a NAT) each keep their `--watch` connection. One more over either limit is closed as soon as it is accepted.
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
semon sessions --model-json --machines ~/semon-machines         # the same model, printed
semon sessions --json --machines ~/semon-machines               # the tree, each node with its machine
```

Every session keeps its machine, each machine answers for its own transcripts, and nothing is linked across machines. The viewer follows DIR while it runs: the stat pass that already decides whether to rebuild also looks at `DIR/machines/`, so a machine received after the viewer started appears on the next poll, and one removed goes away. Only directories named with `a-z`, `0-9` and `-` (1 to 63 of them) are machines; any other entry is ignored, with a warning. A machine whose `facts.json` is missing or can't be read is still shown, offline and named after its directory, with a warning on stderr; it comes online when its facts arrive. `semon push --watch` rewrites a machine's facts every 10 seconds, so facts over two minutes old make it offline since they were written, with nothing running. A received hostname that can't name a machine (empty, over 253 bytes, or holding whitespace, a control character or `~`) gives way to the directory's name. A received machine is told apart from this one even when it has the same hostname: it is shown as `<hostname>~<name>`. A session an earlier machine already has (this machine's own push, or a copied home) is left out of the received machine, with a warning naming how many; use `--no-local` when this machine pushes to DIR. Received machines are read with the same rules as this one's homes: the model indexes every log, and the tree takes `--since` and `--all`.

DIR is only read. Nothing is written there, and no symbolic link is followed out of a machine's directory: a machine directory, a home (`claude/`, `codex/` or their `projects/` and `sessions/`), a `facts.json` or a log file that is a link is ignored. What the viewer keeps for each received machine (its metadata cache and the facts it serves) goes beside `--cache`, under `received/`.

`--machines` changes nothing about who can reach the viewer: it listens only on `127.0.0.1`, and wants the token URL (then its cookie) and an exact local Host header. To see it from another computer, forward the port over SSH. The Host check includes the port, so use the same port number on both ends:

1. On the machine that holds DIR, serve on a fixed port:

   ```sh
   semon sessions --serve --machines ~/semon-machines --listen 127.0.0.1:8765
   ```

   It prints `http://127.0.0.1:8765/?t=TOKEN`.

2. On the computer you view from, open the tunnel and leave it running:

   ```sh
   ssh -N -L 8765:127.0.0.1:8765 you@that-machine
   ```

3. Open the printed URL, unchanged, in a browser on the computer you view from.

If port 8765 is taken on either side, pick another and use it in both `--listen` and `-L`. A different local port (`-L 8080:127.0.0.1:8765`, then `http://127.0.0.1:8080/…`) is refused by the Host check with 403.

## For agents: `semon query` and `semon mcp`

The same session model, shaped for agents and scripts rather than people. `semon query TOOL` prints one tool's answer as JSON (one line with `--json`, pretty without); `semon mcp` serves the same tools as a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio. Both take the home options of `semon sessions` (`--claude-home`, `--codex-home`, `--proc-root`, `--cache`, `--facts`, and `--machine DIR` repeated for several machines' homes), and are read-only: no listener, no network, and nothing written but the metadata cache.

They read a window of recent logs: only the log files modified in the last 30 days, so a first run on a large home doesn't read years of history. `--since DURATION` sets another window, and `--all` reads every file. A file that is read is answered whole: a session that began before the window keeps every turn, handoff and busy interval its files hold. A link whose other end is in a file outside the window isn't made; that end shows as a stub or unlinked, as it would if its file were gone. A session whose earlier file (after a `/clear`) a log line names by `session_id` but the window didn't read says `turns_truncated: true`; it is then listed under its first read file's id. Every answer carries `window_start` (epoch ms, `null` with `--all`): where the window of the model that answered starts, the same instant the scan used. A long-running `semon mcp` rebuilds only when the logs change, so its `window_start` can be older than 30 days back, never newer. Over several machines each keeps its own window, and `window_start` is the latest of their starts. A tool's own `since` argument filters inside the window; on the command line it is spelled `--newer-than`, because `--since` is the window. A `since` that reaches before `window_start` is the error `outside_window`, with the `window_start`; the answer is never silently cut short.

| Tool | Arguments | Returns |
|---|---|---|
| `list_sessions` | `state`, `repo`, `since`, `parent`, `harness`, `limit` | session summaries, most recently active first |
| `get_session` | `id` | the summary, busy intervals, turns (each incoming message and how the turn ended) and every handoff to or from it |
| `read_transcript` | `id`, one of `before` / `after` / `turn`, `limit` | a page of the transcript, exactly as the viewer's `/api/tx` serves it |
| `find` | `text`, `since`, `limit`, `max_bytes` | entries containing the text (ASCII case-insensitive), newest first, with their session, position and a snippet; bounded by a result limit and a byte budget |
| `stalls` | `idle_minutes`, `since` | sessions quiet for that long that are working, wait on a question, or whose process died while its pid file said busy, each with a `stall_reason` |

A summary has `id`, `harness`, `kind` (`session`, `subagent`, `codex-run`, `stub`), `name`, `model`, `repo`, `branch`, `machine`, `parent`, `children`, `state`, `start`, `last_activity`, `pid`, `alive`, `exit`, `open_question`, `tokens` (`input` uncached, `cached`, `output`), `run` and `turns_truncated`. Times are epoch milliseconds. `state` is one of `working`, `waiting-question`, `idle`, `ended`, `error` and `unknown`; `stall_reason` is one of `waiting-question`, `no-output` and `process-gone`, and every `stalls` answer states its rule. A field Semon can't know exactly is `null`: `pid` and `alive` come only from a Claude pid file checked against its process's start time, or from a held Codex writer lock. `run` holds a live session process's run ids, read from `/proc/<pid>/environ` by exact name: `OSTROM_RUN_ID` and `OSTROM_WORK_ORDER_ID` ([Ostrom's run environment contract](https://github.com/onsager-ai/ostrom/blob/main/docs/loops.md)), and nothing else. Every other entry of the environment is dropped as it is parsed, never kept or logged. `run` is `{}` when the process has neither variable, and `null` when there is no live process (a subagent has none of its own) or its environment can't be read whole: unreadable, over 256 KB, or a run id that isn't UTF-8. The facts `semon push` sends carry each live process's run ids, so a mirror answers `run` without reading `/proc`. `exit` is `null`: no log records a run's exit status (a Codex `task_complete` error is a turn's outcome, shown as the state `error`), and none is inferred. Turns and handoffs are the model's own objects, as `--model-json` has them. `since` is a duration back from now (`90m`, `2h`, `7d`) or an RFC 3339 time. A failure has a closed `code` (`unknown_session`, `unknown_turn`, `invalid_arguments`, `outside_window`, …): `semon query` exits nonzero, and `semon mcp` returns a tool result with `isError`.

```sh
semon query list_sessions --state working --json
semon query get_session SESSION_ID
semon query read_transcript SESSION_ID --before 200 --limit 50
semon query find "cargo test" --newer-than 2d
semon query stalls --idle-minutes 30
semon query list_sessions --since 90d --newer-than 60d   # a wider window
semon query get_session SESSION_ID --all                 # every log file
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

## Historical Trace Store collectors

The experimental `semon-codex`, `semon-claude` and `semon-copilot` packages are
retired. Native session parsing, browsing, queries and synchronization remain
supported independently of `traces.sqlite3`. Existing collectors and data are
not changed automatically. Preserve a pinned legacy source/binary for historical
repair/backfill, inventory writers and export the Store privately before changing
an installation. See the [capture custody and pinned-tool reference](docs/trace-capture-retirement.md)
and [independent forensic exporter](docs/trace-store-export.md).

## Historical canonical replication

The experimental `semon ship` command and Trace Store HTTP sender are retired.
It previously sent canonical semantics only and never represented a complete
historical or forensic export. Preserve any configured endpoint/jobs in the
[historical inventory](docs/trace-capture-retirement.md) and use the
[private complete Store exporter](docs/trace-store-export.md) for custody.
Existing jobs are not stopped automatically; current `semon ship` invocations
fail as an unknown command before opening a Store or sending anything. The
pinned legacy source remains available if an operator needs to inspect its old
protocol. Generic `semon push`/receive and hosted native-source synchronization
remain supported independently.

## Existing Encrypted Relay installations

New Encrypted Relay service setup is retired. The source sender/follow/timer unit
templates and active deployment tutorial are removed; this does not stop or
uninstall any existing service. Legacy Relay commands and encrypted remote
readers remain available while deployment inventory and decryption/export are
qualified. Preserve identities, recipients, ciphertext, receipts, snapshots,
lease/epoch and deletion state. See the [retirement and custody contract](docs/encrypted-relay-retirement.md)
and [historical deployment reference](docs/history/encrypted-relay-deployment.md).
The independent [Relay custody exporter](docs/relay-custody-export.md) preserves
all explicitly selected receiver/configuration/sender trees and recovery keys;
its byte-integrity result is separate from decryption qualification.
The separate [offline history reader](docs/relay-history-export.md) provides
decrypted historical inspection/export from that artifact, including retained
generations, orphan branches, snapshot forks and pending outboxes. It builds
against the pinned historical source outside the product workspace and records
missing keys, integrity failures and deletion boundaries explicitly.

Use generic `semon push`/`semon receive` or hosted source synchronization for
session mirrors. Their readable redacted copies are not end-to-end encrypted
and are not complete workspace/harness recovery artifacts. Native agent handoff
and peer communication events remain supported independently of Relay transport.

## Historical Trace Store data

The experimental Canonical Trace Store, its collector packages and `semon
log`/`forensic`/`forget`/`ship` commands are retired from the current product.
Native session browsing, query/MCP, generic Push/receive, SSH and runtime
integration retain the separate Session Event Index and Catalog.

Export an existing Store with the independent offline tool:

```sh
semon-forensic-export --store /private/history/traces.sqlite3 \
  --out /private/history/trace-export-v1
```

Quiesce writers under explicit operator control, verify the complete private
artifact, and preserve original DB/WAL/SHM/journal files and cursors. The tool
copies every table, including unprojected/unlinked forensic records, without
opening SQLite on or modifying originals. It never stops services or deletes
data. See [export, verification, inspection and deliberate deletion](docs/trace-store-export.md).

Existing installations can inspect private working copies or explicitly delete
forensic records through the [qualified pinned legacy CLI/source](docs/trace-capture-retirement.md#pinned-legacy-tools).
Preserve its checksum and existing Relay deletion queues before replacing an
installed binary. Current commands fail explicitly before historical data
access; installed services, databases, records, keys and queues remain untouched.

## Copilot CLI saved-state support

Linux Copilot CLI 1.0.90/1.0.91 schema 1 sessions are browsed directly with
`semon sessions --copilot-home PATH`, without running `semon-copilot` capture.
`COPILOT_HOME` supplies the default override. Copilot event files also participate
in redacted `semon push`/receive and multi-machine viewing. Usage is a cumulative
saved snapshot; missing outcomes, approvals, fresh usage and logical lineage stay
unknown. Native homes are read-only. See [supported versions, commands and
acceptance limitations](docs/copilot-cli.md).

## Existing capture installations

Trace Store collector packages and new periodic capture installation are retired.
The repository no longer ships `install-user-timer.sh` or `devlog-codex-tailer`
unit templates.
Session browsing and generic Push read native logs directly and need no capture
adapter or `traces.sqlite3`.

This source change does not stop installed services or remove binaries, unit
files, cursors or databases. Existing `devlog-codex-tailer.timer` installations
can still write to the store; inventory them and any manually configured
Claude/Copilot collectors and ship endpoints. Preserve the installed unit/config
and cursor paths, then follow the [private export contract](docs/trace-store-export.md)
with explicit writer quiescence before deciding to uninstall capture. Existing
adapter repair/backfill commands require the [pinned legacy tools](docs/trace-capture-retirement.md).
Historical log/forensic/export commands remain in the qualified pinned CLI.
The current product CLI excludes the experimental Store; complete private
historical export remains available through `semon-forensic-export`.

## Development checks

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Browser checks use the locked tooling in `tests/ui/` (`npm ci` and
`npx playwright install --with-deps chromium`); `.github/workflows/ui.yml`
contains the fixture and server setup. Capture pixel baselines in Ubuntu 24.04
with that locked browser and its default fontconfig RGB subpixel rendering.
The same vendored fonts and Chromium version produce different glyph pixels on
Debian 13, whose fontconfig defaults to no subpixel rendering. The Linux browser
launcher uses the checked-in `tests/ui/fontconfig.conf` to assign `rgba` to `rgb`,
matching Ubuntu CI without changing the host configuration or system fonts.
Review changed screenshots before updating affected baseline entries, and keep
the pixel thresholds unchanged.

## Harness icons

Semon shows each harness's official icon to identify where a session came from. The unmodified files are in
`assets/harnesses/`, with their sources listed in `assets/harnesses/NOTICE.md`. They are not covered by this
repository's Apache-2.0 license.

Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.

Encrypted relay session trees are available locally through
`semon sessions --remote ENDPOINT [--remote-config PATH] [--remote-ca CERT]`.
They decrypt and verify bounded pages locally and persist metadata only, with
machine roots, exact cross-harness links, lease liveness and incremental reads.
See [encrypted remote session trees](docs/encrypted-remote-sessions.md) for
identity enrollment, snapshots and the content boundary.

Reusable SSH checks and mirror bootstrap for embedding applications are documented
in [SSH bootstrap](docs/ssh-bootstrap.md). SSH enrollment grants no agent execution.
