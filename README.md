<img src="crates/semon-sessions/src/favicon.svg" width="48" height="48" alt="Semon">

# Semon

Semon collects, synchronizes, browses and queries harness-native agent sessions.
Native source logs, the incremental Session Event Index, and Session Catalog form
the foundation for Viewer, CLI, MCP, future Session Intelligence.

The core CLI lives in `semon-cli`; session discovery/parsing and focused reads live
in `semon-sessions`, generic mirror synchronization in `semon-push`, retained SSH transport
in `semon-ssh`. Retained optional runtime/control engines live in `semon-runtime`/`semon-control`.

Encrypted Relay and Canonical Trace Store are retired pre-release experiments.
The product does not ship their protocols, capture services or migration tools.
Native source logs, the Session Event Index, generic Push, SSH and runtime
observation remain supported. This code change does not remove user data,
keys or installed services. See the [session foundation contract](docs/design/session-foundation-retirement.md).

The collection configuration temporarily pauses two-way harness control, managed runtime delivery, broad full-text retrieval and all SSH capabilities. Collection, synchronization, monitoring, metadata navigation and readable native history remain active without model, compute or SSH credentials or SSH connectivity. Direct `find`, control and SSH calls return `capability_unavailable` before native discovery, controller/provider initialization or delivery. Passive questions, approvals, outcomes, lineage and handoffs remain visible. Native archive/unarchive, enrollment/revocation, collection configuration, authorization, export and explicit data administration remain in scope. Existing credentials, history, operation receipts and uncertain outcomes are preserved; this pause stops no installed harness and replays no unknown write. See the [temporary collection scope](docs/design/session-foundation-retirement.md#temporary-collection-scope).

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

Native reads authorize a configured source key before resolving a session. `GET /api/session-catalog?machine=SOURCE` returns bounded metadata pages; its continuation cursor binds the published Catalog generation. `GET /api/session-transcript?machine=SOURCE&sid=KEY` reads a bounded selected transcript range, with `after` or `before` slot anchors and an optional expected `generation`. `GET /api/session-entry` expands a selected entry or recorded text field under the same source, session and generation checks. Current and retained history are explicit read scopes; neither grants runtime control authority. The reader contracts are documented in [docs/design/session-source-reader.md](docs/design/session-source-reader.md) and [docs/design/retained-native-projections.md](docs/design/retained-native-projections.md). Transcript data is placed in the page using text nodes so source HTML remains inert. The CLI `--json` tree schema is described below.

The Viewer is also a library API: `semon_sessions::ViewerCore` serves native pages, assets, Catalog, Transcript, Entry and focused relationship routes. The embedding server authenticates requests, supplies stable source keys, calls `respond` or `respond_with`, and sends the reply with `semon_sessions::SECURITY_HEADERS`. Run blocking native reads off an async runtime. The core supports concurrent callers through `Arc`, without a caller lock. Request-specific account menus belong in `Extras`. Native Catalog demand uses a shared `RefreshPool` in every mode. `Refresh::Background` observes source changes; a receiver can select `Refresh::OnInvalidate` and call `invalidate(key)` after a push. Catalog production does not construct a workspace Viewer model. `ViewerCore::with_machines` admits several native homes; `with_received` follows a receiver directory. Call `close` and drain active reads before changing source custody or removing files.

Server-rendered pages that share the viewer's visual language can use the documented shell contract in [docs/shell.md](docs/shell.md).

`semon push --to URL --token-file PATH [--watch]` keeps a copy of these logs on another server that implements the [mirror protocol](docs/mirror-protocol.md). It sends only supported native source files (`semon_sessions::inputs`), redacted on this machine before they leave (known secret shapes become `*`, byte for byte). It also sends this machine's facts: hostname, live processes, held Codex locks and repositories (`semon_sessions::local_facts`). A copy built with `--facts FILE` (or `Options::facts`) preserves the source machine’s recorded identity and runtime observations. The token file must be mode 0600, and the client's cursor state is kept under `$XDG_STATE_HOME/semon/push/`, 0600. One push at a time runs with a state file: it holds an OS lock on `<state>.lock` beside it, and a second push with the same state (by default, to the same receiver URL, however its scheme, host case or trailing slash is spelled) fails at once until the first ends. Ctrl-C or SIGTERM ends a push, `--watch` included, within a moment, from wherever it was, with the state file recording what the receiver acknowledged. An app that embeds the push calls `semon_push::push_until` with a `Stop`, and can pass the token in memory (`Credential::Memory`) instead of a file; a request or facts collection that such a stop leaves running keeps the state locked until it ends (a request within two minutes, its timeout; a facts collection has none), and a push started meanwhile says it is still finishing.

Live Claude statuses observed so far are `busy`, `idle`, `shell` (a finished turn with background shells running, not counted as working) and `waiting` (stopped on a question or permission prompt, shown as needs you); any other status on a verified live process is shown verbatim. An unreadable transcript or rollout shows `unknown` state with whatever metadata is available. The default view contains roots active in the last 24 hours plus live roots. `--all` includes older sessions; `--since 2h` or `--since 3d` changes the window; `--session ID` selects one subtree. `--claude-home`, `--codex-home`, `--proc-root`, and `--cache` override every source and the cache for fixtures or alternate installations. The configured cache stem defaults to `$XDG_STATE_HOME/semon/sessions-index.json`, or `~/.local/state/semon/sessions-index.json`. Tree and focused reads share the Session Event Index at the same stem with extension `.sqlite3`; existing tree JSON files are left untouched. The index stores metadata and evidence locations, not session bodies. Back up existing index/history data before an explicit reset.

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

On the machine that receives, make a token per machine. It is printed once, and `DIR/tokens` (0600, in a 0700 directory) keeps only its SHA-256 and the name. A name is 1 to 63 of `a-z`, `0-9` and `-`. A missing `DIR` is created 0700; an existing `DIR`, or `DIR/tokens`, that its group or others can use is refused with the `chmod` that fixes it, and never changed.

```sh
(umask 077; semon receive token add laptop --dir ~/semon-mirror > laptop.token)
semon receive token list --dir ~/semon-mirror
semon receive --dir ~/semon-mirror            # listens on 127.0.0.1:8735
```

Put the token on that machine in a 0600 file, and point `semon push` at the receiver. For a receiver on this machine, plain `http` to loopback is allowed. Across machines, use an operator-provisioned TLS receiver with a publicly trusted certificate; SSH tunnels and SSH-mediated collection are paused:

```sh
semon push --to http://127.0.0.1:8735 --token-file ~/.config/semon/laptop.token --watch
```

`semon receive token revoke laptop --dir ~/semon-mirror` removes a token. A running receiver re-reads `DIR/tokens` when the file changes, so the next request with that token gets 401 without a restart. If the file stops parsing, or its mode is loosened, no token is accepted until it is fixed.

The receiver listens on `127.0.0.1:8735` unless `--listen ADDR` says otherwise. A non-loopback address is refused unless `--tls-cert PEM --tls-key PEM` are given and `DIR/tokens` holds at least one token; plain HTTP off loopback is always refused. These are the generic `semon receive` rules, implemented in `semon-push`. Semon never opens a port or obtains a certificate: the address, the firewall, DNS and the certificate stay your choices. `semon push` checks a certificate against the standard web roots only, so a TLS receiver needs a certificate that chains to them; configure a trusted certificate before using a remote receiver.

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

`--machines` changes nothing about who can reach the viewer: it listens only on `127.0.0.1`, and requires the token URL (then its cookie) and an exact local Host header. Open the printed URL on that machine. SSH tunneling is paused in the collection configuration; it is not an active remote-viewing setup route.

## For agents: `semon query` and `semon mcp`

Native session evidence shaped for agents and scripts. `semon query TOOL` prints one tool's answer as JSON (one line with `--json`, pretty without); `semon mcp` serves the same tools as a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio. Local tools construct native evidence owners directly and reuse the Session Event Index without starting Viewer refresh workers. Both take the home options of `semon sessions` (`--claude-home`, `--codex-home`, `--proc-root`, `--cache`, `--facts`, and `--machine DIR` repeated for several machines' homes), and are read-only: no listener, no network, and nothing written but the metadata cache.

They read a window of recent logs: only the log files modified in the last 30 days, so a first run on a large home doesn't read years of history. `--since DURATION` sets another window, and `--all` reads every file. A file that is read is answered whole: a session that began before the window keeps every turn, handoff and busy interval its files hold. A link whose other end is in a file outside the window isn't made; that end shows as a stub or unlinked, as it would if its file were gone. A session whose earlier file (after a `/clear`) a log line names by `session_id` but the window didn't read says `turns_truncated: true`; it is then listed under its first read file's id. Every answer carries `window_start` (epoch ms, `null` with `--all`): where the window of the model that answered starts, the same instant the scan used. A long-running `semon mcp` rebuilds only when the logs change, so its `window_start` can be older than 30 days back, never newer. Over several machines each keeps its own window, and `window_start` is the latest of their starts. A tool's own `since` argument filters inside the window; on the command line it is spelled `--newer-than`, because `--since` is the window. A `since` that reaches before `window_start` is the error `outside_window`, with the `window_start`; the answer is never silently cut short.

| Tool | Arguments | Returns |
|---|---|---|
| `list_sessions` | `state`, `repo`, `since`, `parent`, `harness`, `limit` | session summaries, most recently active first |
| `get_session` | `id` | the summary, busy intervals, turns (each incoming message and how the turn ended) and every handoff to or from it |
| `read_transcript` | `id`, one of `before` / `after` / `turn`, `limit` | a source-backed native transcript page, sharing Viewer slot ordering and rendering |
| `stalls` | `idle_minutes`, `since` | sessions quiet for that long that are working, wait on a question, or whose process died while its pid file said busy, each with a `stall_reason` |

A summary has `id`, `harness`, `kind` (`session`, `subagent`, `codex-run`, `stub`), `name`, `model`, `repo`, `branch`, `machine`, `parent`, `children`, `state`, `start`, `last_activity`, `pid`, `alive`, `exit`, `open_question`, `tokens` (`input` uncached, `cached`, `output`), `run` and `turns_truncated`. Times are epoch milliseconds. `state` is one of `working`, `waiting-question`, `idle`, `ended`, `error` and `unknown`; `stall_reason` is one of `waiting-question`, `no-output` and `process-gone`, and every `stalls` answer states its rule. A field Semon can't know exactly is `null`: `pid` and `alive` come only from a Claude pid file checked against its process's start time, or from a held Codex writer lock. `run` holds a live session process's run ids, read from `/proc/<pid>/environ` by exact name: `OSTROM_RUN_ID` and `OSTROM_WORK_ORDER_ID` ([Ostrom's run environment contract](https://github.com/onsager-ai/ostrom/blob/main/docs/loops.md)), and nothing else. Every other entry of the environment is dropped as it is parsed, never kept or logged. `run` is `{}` when the process has neither variable, and `null` when there is no live process (a subagent has none of its own) or its environment can't be read whole: unreadable, over 256 KB, or a run id that isn't UTF-8. The facts `semon push` sends carry each live process's run ids, so a mirror answers `run` without reading `/proc`. `exit` is `null`: no log records a run's exit status (a Codex `task_complete` error is a turn's outcome, shown as the state `error`), and none is inferred. Turns and handoffs use the same native identities across Query, Transcript and Viewer reads. `since` is a duration back from now (`90m`, `2h`, `7d`) or an RFC 3339 time. A failure has a closed `code` (`unknown_session`, `unknown_turn`, `invalid_arguments`, `outside_window`, …): `semon query` exits nonzero, and `semon mcp` returns a tool result with `isError`.

```sh
semon query list_sessions --state working --json
semon query get_session SESSION_ID
semon query read_transcript SESSION_ID --before 200 --limit 50
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

SSH onboarding, testing, installation/bootstrap/update, tunneling and SSH-mediated collection/execution are paused. Public transport calls refuse before credentials or network admission. [SSH bootstrap](docs/ssh-bootstrap.md) records the retained private transport and deferred qualification; existing credentials, installations and history are preserved.

Model reference prices refresh automatically in long-running processes; see
[runtime pricing](docs/runtime-pricing.md) for offline fallback and configuration.
