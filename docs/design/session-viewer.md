# Session viewer: from the mockup to `semon sessions --serve`

Status: draft for review, 2026-09-26. Part of #40. It supersedes #47's UI and keeps #47's security model.

## Goal

`semon sessions --serve` shows exactly what the approved mockup shows, on this machine's live logs. The target is a 100% mirror: the same screens, layout, type, colours, interactions and wording. The mockup is not a picture to reimplement; its HTML, CSS and JavaScript become the viewer's frontend.

References:

- Sample mockup: https://claude.ai/artifact/64zPCUAf1ub9drk2yNACtL (v20)
- Real-data mockup: https://claude.ai/artifact/Xyg3W8wpP2jNYXSNMER3FU (v15)
- The design decisions behind them: the comments on #40 from 2026-09-25 and 2026-09-26.

## Principle: keep the frontend, replace the data

The mockup is a single page whose script starts from six in-memory globals: `SESS`, `H`, `TX`, `MACHINE`, `MACHINE_UP` and `NOW`. Every screen is computed in the browser from them. So is every derived structure: turns, traces, the Timeline map, Home's sections, search and the handoff sentences.

The implementation keeps that code and changes only where the globals come from:

1. **A Rust model builder** reads `~/.claude` and `~/.codex`, the same way V1 does. It produces those globals in exactly the mockup's shapes.
2. **The server** exposes them as JSON.
3. **The frontend** is the mockup's own script, split into files. A small loader replaces the embedded data.

A change to the frontend is allowed only for one of four reasons:
- loading data
- real URLs
- live updates
- transcripts too large to hold in memory

Every such change is listed under [Deliberate differences](#deliberate-differences). A diff against the mockup's script shows everything else unchanged.

## What already exists

- **`crates/semon-sessions` (V1, #45)** reads the agent files directly and never opens the store. It already has most of the parsing:
  - **Claude:**
    - liveness from `sessions/<pid>.json`, checked against `/proc/<pid>/stat`
    - subagents via `subagents/agent-<id>.jsonl` and their `.meta.json` `toolUseId`
    - tokens de-duplicated by message id
  - **Codex:**
    - nodes keyed by `session_meta.id`
    - subagents via `parent_thread_id`
    - liveness from `/proc/locks`
    - runs linked to a Claude parent by the `Semon-Parent` / `Semon-Handoff` marker
  - **Cache:** a metadata-only index at `$XDG_STATE_HOME/semon/sessions-index.json`, resumed by byte offset per `(dev, ino)`.
- **`--serve` (#47)** is a `tiny_http` server on 127.0.0.1. Its security is kept as is:
  - a per-run token and an `HttpOnly; SameSite=Strict` cookie
  - an exact `Host` check
  - GET only
  - on every response: `no-store`, `CSP default-src 'self'; script-src 'self'; style-src 'self'`, `X-Frame-Options: DENY` and `no-referrer`
  - DOM built from text only, with a test that bans `innerHTML`
  - **Routes today:** `/api/tree`, `/api/transcript` (paged from the end by offset) and `/api/entry` (one entry in full, up to 8 MB).
- **The store** (`semon-store`, schema v5) is not used by the viewer, and this plan doesn't change that. The live store is still at v3 and its upgrade is a separate decision. The store only matters for the multi-machine phase (M5).
- **The prototype extractor** (`extract_real.py` in the mockup work) produced the real-data mockup. It is the reference implementation of the rules below, and its rules and edge cases are ported, not reinvented.

## Architecture

```
~/.claude, ~/.codex, /proc          (read-only; *.key never opened)
        │
  scan (V1, extended)  ── per-file event index, metadata + byte offsets only, cached
        │
  model builder         ── sessions, handoffs, turn index, busy, state   (in memory)
        │
  tiny_http API         ── /api/model  /api/tx  /api/entry  (token, Host, CSP as #47)
        │
  frontend = the mockup ── viewer.html + viewer.css + viewer.js + fonts, served from the binary
```

### Scan: a per-file event index

The scan extends V1's per-file summary. It records one index row per event, holding the kind, time, byte offset, ids and flags. It holds no text: the cache stays metadata only, as V1's planted-secret tests require. Text such as briefs, messages and tool inputs and outputs is read from the source file by offset when it's needed, and held in memory only.

Event kinds are the prototype's:

- **`ask`:** your message. This is a user record or `queued_command` whose `origin.kind` is `"human"`, that isn't `isMeta` or a compact summary, and that isn't empty once reminders are stripped. Human slash commands count as your messages. sdk-cli sessions have no "you": their first prompt is a program's brief. (M0 corrected the prototype, which counted typed `/compact` and sdk-cli prompts and skipped slash commands.)
- **`a`, `think`, `tool`:** assistant text, a thinking block, and a `tool_use` carrying id, name and input offset.
- **`result`:** a `tool_result`, keyed by `tool_use_id`, with its error flag and `async_launched` flag.
- **`xsm`:** a `<cross-session-message from=… from-name=…>` block (a relay received).
- **`agm`:** an `<agent-message from=…>` block (a subagent handing back).
- **`tn`:** a `<task-notification>`, carrying tool-use id, status and result.
- **Codex:** the same kinds from rollout items: user and agent messages, reasoning, function calls and outputs.

Every line's timestamp also feeds the session's busy intervals.

### Model builder: the rules

Each rule's precision is stated. #40's ruling holds: **Semon never guesses a link**. Where the logs don't say something, the model says "unknown", not the likeliest answer.

| Output | Source | Precision |
|---|---|---|
| Session (Claude) | The files of one lineage (see *Resume chains*), plus `sessions/<pid>.json` for `name`, status, `bridgeSessionId` and `messagingSocketPath` | Exact |
| Subagent spawn | `.meta.json` `toolUseId`, matched to the parent's Agent/Task `tool_use`; the brief is its `prompt` | Exact |
| Spawn result | In order: a `task-notification` with the same tool-use id; an `agent-message` from the agent id; the synchronous `tool_result` (not `async_launched`); the subagent's own `SubagentHandback` | Exact |
| Codex spawn | `Semon-Parent` / `Semon-Handoff` marker (V1), or `parent_thread_id` | Exact; no marker means `unlinked` |
| Relay | A `SendMessage` `tool_use`, joined on `msg_id` to a received message: the sender's `toolUseResult.msg_id` equals the receiver's `origin.msg_id`. The brief is the input's `message`, which equals the receiver's `origin.body`. A send with `success:false` or a denied send is a failed relay with no receiver. A received message whose sender file is gone keeps the sender as a stub, from `origin.name` and (pid, procStart) | Exact: 3,091 of 3,091 received join (M0) |
| To you: question | An `AskUserQuestion` `tool_use`. Answers come from the structured `toolUseResult.answers`, ordered by `input.questions`. A list answer is multi-select. A value that isn't an option label is free text. `annotations[q].preview` is attached. `is_error`, or a string result, means declined. The result text is used only when the structured field is missing, anchored on the known question strings | Exact (the text-only regex was wrong in 153 of 404 cases) |
| To you: result | The last assistant message of a live top-level session that is waiting and has no open question | Exact from state |
| Your message | `ask` events in a top-level session | Exact |
| Turn | Starts at each incoming entry: your message, a relay received, or the brief that starts a run. A gap marker also ends a turn. It ends as replied / asked you / failed / returned / still working / no reply in these logs | Computed from the above |
| Busy | Each line's timestamp. Consecutive stamps no more than 5 minutes apart form one interval | Exact |
| State | `work`: Claude status busy, a subagent's call still open, or a Codex writer lock. `wait`: an open question, or a waiting result. `idle`: alive and idle. `done`: ended or returned. `err`: the run failed | Exact |
| Machine | This machine's hostname. One row; `MACHINE_UP` is true | Exact |
| Move | Not produced until M5 (from the relay's `takeovers.jsonl`) | n/a |

**Relays across ended sessions.** Messaging sockets (`/run/user/<uid>/cc-socks/<pid>.sock`) belong to a process, not a session, so they are keyed by (pid, procStart). The `msg_id` join makes them unnecessary for linking. Names are shown from `origin.name` and the `SendMessage` result. D3's unique-name rule remains only as the fallback for the roughly 34 sends that have no `msg_id`.

**Resume chains.** One working session can span several transcript files; for example `/clear` starts a new file. M0 found two exact, logged links:
1. **`session_id`** (snake_case, on each line) names the lineage's first file. It covers 34 of 37 `/clear`s and exists only in Remote Control-bridged sessions.
2. **`continued-in`**, written into the old file by a copy-resume.

Files linked by these are one session, with their entries merged by timestamp. Files in one lineage can overlap in time: M1 found two processes writing to the same Remote Control session at once. So the merge never assumes the files run one after another, and never de-duplicates across them. Files are also joined by `bridgeSessionId`, meaning the same Remote Control session even across a process restart (D5). Everything else stays separate, per D4.

**Codex.** Relays exist only inside a Codex tree, in the `collaboration` namespace (`send_message`, `spawn_agent`, `agent_message` hand-backs), addressed by agent path. They map to relay and spawn handoffs in the same way. Codex's `request_user_input_async` has no answer record, so a Codex question shows as asked, with its answer unknown.

### API

The model endpoint returns the mockup's globals verbatim in shape; other shapes are added only where the size of the data forces it.

- **`GET /api/model`** returns:
  - `{version, now, machine, sessions (SESS), handoffs (H), turns, busy}`
  - `sessions` and `handoffs` use the mockup's exact field names: `name`, `harness`, `kind`, `lane`, `stub`, `state`, `model`, `tokens`, `repo`, `branch`, `start`, `last`, `activity`, `busy`; and `id`, `kind`, `ask`, `from`, `to`, `at`, `done`, `status`, `brief`, `result`, `answer`.
  - Each session also carries `calls` and `errors`, its transcript's tool-call and failed-or-unfinished totals, the same numbers `/api/tx` reports. A page counts from the model and fetches no transcript for it. They are absent for a session with no transcript, and from a server that predates them.
  - Briefs and results are capped at 4 KB, as in the mockup data.
  - `turns` is the turn index: id, session, start handoff, end state, sent handoff ids, and the transcript offset of its first entry. The browser can no longer compute it from a full `TX`, because it no longer loads every transcript.
  - It carries an `ETag` and supports `?since=<version>` for live polling.
- **`GET /api/tx?sid=&before=|after=|turn=`** returns one session's transcript entries in the mockup's `TX` entry shape (`{k:"a"|"tool"|"think"|"h"|"u"|"harness"|"end", …}`).
  - Tool entries carry `in` and `out` previews capped as in the mockup, plus an `offset` for the full text.
  - Pages are bounded like #47's (200 entries or 2 MB). `turn=` returns the page that contains a turn, for deep links.
- **`GET /api/entry`** (#47, kept) returns one tool call's full input and output for the View all sheet.
- **Pages:**
  - `/`, `/timeline`, `/sessions`, `/machines`, `/machines/<id>`, `/s/<harness>/<id>` (with `?turn=`) and `/trace/<harness>/<id>/<turn>` all serve the same HTML.
  - The frontend switches from state-only `history.pushState` to real URLs. `/s/...` stays compatible with #47's links.

### Frontend: the mockup, split into files

- **`viewer.html`:** the mockup's `<body>` shell (sidebar, scrim, main and top bar). There is no inline script or style.
- **`viewer.css`:** the mockup's `<style>` block, unchanged.
- **Fonts:** the mockup loads Instrument Sans, Source Serif 4 and JetBrains Mono from Google Fonts. The CSP and #47's no-CDN rule forbid that, so the fonts are vendored as woff2 (all three are OFL) and served from the binary with `@font-face`. That's decision D1.
- **`viewer.js`:** the mockup's script, with its data block replaced by a loader:
  - `boot()` fetches `/api/model`, assigns the globals and renders.
  - `transcript()` asks for `TX[sid]` on demand, with a `Load earlier` control at the top.
  - The data-derived builders (`TURNS`, `STARTS`, `HOLDS`) read the turn index instead of scanning every `TX`.
- **CSP:** the mockup only sets element styles through the CSSOM (`el.style.left = …`), which `style-src 'self'` allows. It never uses `style="…"` or `innerHTML`. Both audits become tests.

### Live updates

Home, Timeline, Sessions and Machines poll `/api/model?since=` every 2 s, paused while the tab is hidden. A session page follows its tail with `/api/tx?after=`.

On the server no poll waits for a rebuild (#29). `--serve` answers every request from each machine's last built model, a snapshot shared by every request in flight, and the `ETag` is that snapshot's version. While requests keep coming, a thread per machine takes a stat pass over the watched files every 250 ms and rebuilds only when something changed, at most once a second, so the changes of one second are one rebuild. An answer is then at most about 1 s plus one build behind the logs. The thread stops 30 s after the last request; the next request after that, like the first, refreshes before it answers (a stat pass, and a build only if something changed). An embedding server chooses this with `ViewerCore::set_refresh(Refresh::Background)`; the default, `Refresh::OnRead`, rebuilds before the read that finds a change, for one-shot and tool callers.

A new model re-renders the current screen with its view state kept:
- the scroll position
- expanded steps and child runs
- the Timeline's zoom and horizontal scroll
- an open View all sheet
- the find query
- the filters

The mockup's `render()` rebuilds a page from scratch. That's fine for navigation, but it would reset all of this every 2 s, so live re-render needs this state capture and restore. It is the largest new piece of frontend code.

### Security and privacy

- **#47's server rules are kept unchanged.** Its tests are kept, and new ones are added:
  - every model and transcript string goes through the XSS payload test
  - no inline script or style
  - no `innerHTML`
  - every link except http(s) stays text; this is already the mockup's markdown rule
- **No redaction.** The page shows your own logs, on loopback, to you. The mockups published to claude.ai were redacted because they left the machine; this viewer never does.
- **Nothing is written** under `~/.claude` or `~/.codex`. `*.key` files are never opened, and the cache holds no content.

## Deliberate differences

These are the only places the served viewer departs from the mockup:

1. **Real URLs** instead of state-only history entries.
2. **Transcripts load a page at a time,** with "Load earlier". The mockup's "Earlier entries not included in this copy" divider appears only when the logs themselves are truncated.
3. **Live updates** with view state kept, as above.
4. **One machine.** The Machines page lists this machine. Moves and offline machines appear only once M5 brings in other machines.
5. **Sample-only data is gone:** the made-up machines, the `T(h,m)` clock, and the unused `THREADS` object.
6. **Several machines from real data.** A viewer over several machines' homes (`ViewerCore::with_machines`) serves them as `machines`, each with its sessions. An offline machine without a move shows when it was last seen ("Not responding since …", "Last seen …") where the sample showed its move. Stubs and unsent sends are keyed per machine (`<id>@<machine>`), so nothing is linked across machines.
7. **An embedder's management link.** When the embedding server sets one (`ViewerCore::set_admin_link`), the Machines page ends its list with it, in the list's "more" style. The local viewer never sets one.
8. **An embedding server's account menu and navigation destination.** `AccountMenu::new` validates the identity, optional same-origin avatar path, up to 50 named workspaces, and up to 12 same-origin account links as one menu. `ViewerCore::set_account` serves it in `/api/model` as `account`: `{name, login, initials, avatar_href, workspaces: [{name, role, current, switch_href}], links: [{label, href, method, danger}]}`. Choosing a workspace submits a POST to its `switch_href`; account links default to `method: "get"` and may use `method: "post"`. POST actions render as same-origin forms with one submit button and no model-supplied fields, so the embedding server can enforce its same-origin check before applying a change. `ViewerCore::set_nav_override("machines", href)` serves `nav: {machines: href}` and sends the Machines navigation entry and its route entry points to that same-origin path. Neither value changes the model version or `ETag`; the local viewer does not set them. With an account present, desktop uses a top-bar avatar popover and phones expand the account row at the drawer's foot.

## Verification

- **Rules.** Rust tests run on synthetic fixture homes, one per rule in the table:
  - spawn and each of its four result sources
  - a Codex marker, and an unlinked run
  - a relay by socket, by a unique name, by an ambiguous name, and received-only
  - a question with one answer, two answers, and free text
  - turns split by each incoming kind and by a gap
  - busy clustering
  - each state
  - a resume chain, if M0 finds a link

  Each test asserts an exact `/api/model` golden file.
- **Mirror.** A fixture home is generated so that the sample mockup's data comes out of it: the same sessions, handoffs and transcripts, written as Claude and Codex log files. The served viewer on that home must render every screen pixel-identical to the sample mockup, within a small anti-aliasing tolerance, at 390×844 light and dark and at 1280×860. This is the 100% check.
- **Behaviour.** The mockup's check scripts become the UI acceptance suite and run against the served viewer: `full.js`, `viewer.js`, `turns.js`, `bar.js`, `md.js`, `home.js`, `timeline.js` and `check-real.js`. They cover overflow, tap targets, deep links, back, View all, markdown, answers, the Timeline geometry and the census.
- **Where it runs.** The thermal rule stands. Locally that means only `cargo check -p semon-sessions` and single named tests. The workspace tests and the browser suite run in CI, as a new job with Node, Playwright and Chromium (decision D2).
- **One manual look** by you on your phone before merge, as with #47.

## Milestones

Each milestone goes through the plan→implement checkpoint. Codex is out until 2026-09-28 10:33, so until then the implementers are Opus and Sonnet subagents; after that, Luna takes the concretely specified pieces.

| # | Work | Implementer | Done when |
|---|---|---|---|
| M0 | Read-only spikes. **Done 2026-09-26:** found the exact links (`msg_id`, `session_id`, `continued-in`), the structured answers, and the corrected ask rule. See the #40 comment | Opus subagent | Done |
| M1 | Event index and model builder in `semon-sessions`, plus `/api/model` | Opus subagent (the linking rules are subtle) | The golden fixtures pass. On this machine, the model matches the real-data mockup's counts (sessions, 132 handoffs by kind, turns, busy intervals), with each difference explained |
| M2 | Frontend port: the mockup split into files, vendored fonts, the loader, real URLs, `/api/tx` paging, the turn index, and View all through `/api/entry` | Sonnet subagent; Luna after 09-28 | The served sample fixture matches the mockup pixel for pixel on every screen. The ported check suite passes. The CSP and XSS tests pass |
| M3 | Live updates with view state kept | Opus subagent | A scripted fixture that appends lines while the page is open: the new work appears within 4 s, and scroll, expanded steps, zoom and find are unchanged |
| M4 | The CI browser job: fixture generator, pixel comparison and check suite | Sonnet subagent | Green on the M2/M3 PR, and required for merge |
| M5 | Several machines: through the relay or the store, per #40's rule that the server sees nothing. Moves from `takeovers.jsonl`; per-machine heartbeat and last-seen | Its own spec | Out of scope here |

M1 and M2 can overlap once the `/api/model` shape is frozen. That shape is the mockup's, so it is known today.

## Decisions (the user, 2026-09-26, Semon session)

- **D1, fonts:** bundle. The three OFL fonts are vendored as woff2 in the binary.
- **D2, CI browser job:** add it. Node, Playwright and Chromium run in CI only, for the pixel and behaviour suites.
- **D3, relays by name:** resolve a unique name only. Otherwise the other end is "not matched".
- **D4, resume chains:** if M0 finds no exact link, each file is its own session. There is no stitching by timing. M0 found `session_id` and `continued-in`, which are exact, so those chains are one session.
- **D5, Remote Control sessions:** files that share a `bridgeSessionId` are one session, even across a process restart.
