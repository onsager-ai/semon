# Two-way control: answering and steering agents from Semon

Status: design only. Marvin chose "Design doc, then step 1 (Recommended)" and, for how far a machine trusts the embedding server, "Paired-device signatures (Recommended)" (Semon session, 2026-09-30T01:00:34Z). After the doc's first review he chose "Refuse agent-sent answers (Recommended)" and, for where remote answers may be signed, "Native app only (Recommended)" (Semon session, 2026-09-30T01:23:07Z). Nothing here is implemented. Each step below is its own PR series and needs its own go; step 1 starts after Marvin approves this doc. Marvin approved it and step 1's PRs 1 to 4 (Semon session, 2026-09-30T01:37:18Z). Spikes S1, S2, S6 and S7 ran the same day; their results are in [Spike results](#spike-results) and are folded into the design below.

## Goal

Today Semon only reads. It shows every session of every harness in one tree, and it shows when a session waits on you, but you still have to find the right terminal to answer. Two-way control closes that loop: answer a permission prompt or a question from the viewer, then send a message or interrupt a run, and later do the same from a phone.

The hard part is not the UI. It is that each harness exposes a different control surface, with different stability, and that a page able to approve a shell command is exactly the kind of local server that has produced critical CVEs in this space. This doc fixes the architecture and the trust model first, then designs step 1 in enough detail to split into PRs.

## What exists today

Verified on main, 2026-09-30:

- **No write path anywhere.** `semon` is a set of subcommands. `semon sessions --serve` is the only long-running local server (`viewer.rs:621`). It binds 127.0.0.1 only (`listen_addr`, `viewer.rs:597`), makes a random token per run (`viewer.rs:614`), and answers GET only (`authorized`, `viewer.rs:744`; `ViewerCore::respond` returns 405 for anything else, `viewer.rs:1372`). The token works two ways: as `?t=` in the URL, reusable for the whole run, and as the `semon_session` cookie, which holds the same value (`viewer.rs:698`). Every request needs exactly one `Host` header equal to `127.0.0.1:<port>` or `localhost:<port>`. `semon mcp` is stdio and read-only.
- **The push to an embedding server is one way** (`docs/mirror-protocol.md`): the machine POSTs log bytes and facts. Nothing comes back down except a status and a length.
- **Live-process facts:** Claude pid files (`~/.claude/sessions/<pid>.json`) give pid, session id and busy or idle; Codex liveness is a held writer lock. Semon records no tty and no socket, and the harness CLI version is not modelled. `HarnessDefinition` (`harness.rs:8`) is only names and icons.
- **The "needs you" signals already exist.** A live session with an open question or decision to you has state `wait` ("Needs you", the amber dot; `lineage_states` in `model.rs`, `STATE` in `viewer.js`). Home has a "Needs you" section and a "waiting on you" count. Analytics has "Waited on you". The overhaul gives the rail attention dots (`overhaul.md`). `semon query` has `waiting-question` and reserves `waiting-permission` until an exact signal for a pending permission prompt exists (`query.rs:46`).
- **Agent messaging** (`docs/design/agent-messaging.md`) is design only. It covers agent-to-agent messages, not a human at the console.

## Decisions

Marvin made these; they are not reopened here.

### 1. One local process, one driver interface, one adapter per harness

One local Semon process per machine is the only thing that touches harnesses. It talks to them through one internal driver interface whose vocabulary is borrowed from the Codex app-server and ACP: sessions, turns, approvals, questions, interrupt, start. In all three harnesses a permission prompt is already a request from the agent that blocks until a client answers, so the interface models it that way.

| Harness | Adapter | Live attach to a terminal session | Stability |
|---|---|---|---|
| Codex | The shared app-server daemon's control socket, `~/.codex/app-server-control/app-server-control.sock` (WebSocket over a Unix socket). `turn/start`, `turn/steer`, `turn/interrupt`; the server's `item/*/requestApproval` and `item/tool/requestUserInput` requests; `serverRequest/resolved`. | Yes: since 0.157.0 the TUI reuses the daemon by default (`--no-daemon` opts out). | `codex app-server` is documented as experimental. The adapter is version-gated and degrades to read-only on a version it hasn't been tested against. The daemon auto-attach is verified in source only. |
| OpenCode | The TUI's own HTTP server: `POST /session/:id/prompt_async`, `GET /permission` and `POST /permission/:id/reply`, `GET /question` and `POST /question/:id/reply`, `POST /session/:id/abort`, `GET /event` (SSE). | Yes, once Semon knows the port. | GA, except the question routes, which are in source but not in the docs. The port is random unless the TUI is started with `--port`; there is no documented registry. Semon has to be told the port, by launching OpenCode through a wrapper or with `--port`. |
| Claude Code | A `PermissionRequest` hook that long-polls the local Semon process for an answer, and `PostToolUse` and `PostToolUseFailure` hooks that tell Semon a tool ran. On timeout, or when Semon isn't there, the permission hook returns no decision and the terminal dialog is the fallback; the terminal dialog stays usable while the hook waits (S1). | Approvals and single-choice questions. | Hooks are GA. |

Claude Code specifics, decided:

- **`AskUserQuestion` is answerable in step 1, single choice only** (S2). The tool fires `PermissionRequest` with `tool_input.questions[]` (each with `question`, `header`, `options[{label, description}]` and `multiSelect`). The hook answers with `behavior: allow` and `updatedInput` set to the tool's input unchanged plus `answers`, a map from each question's text to the chosen option's label. The terminal dialog closes and the model gets the same result a terminal answer gives. `allow` without `updatedInput` leaves the dialog up. Multi-select and free-text questions are untested, so they are shown read-only ("answer in the terminal") until spike S8 says how they are answered. A request whose questions repeat the same text is read-only too, since the map can't tell them apart.
- **Messages into a live Claude terminal session are not supported in v1.** The per-session inbox socket frames every message as "not typed by your user", so it cannot stand in for the human, and its frame format is undocumented. Channels need a launch flag and are a research preview. This matches agent-messaging's refusal to write to that socket.
- **Sessions Semon starts itself get full control:** `claude -p` with `--input-format stream-json --output-format stream-json`, or `claude --bg`, gives messages, approvals and questions (through the SDK's permission callback), and interrupt.

### 2. No new listening port

No listening port beyond today's 127.0.0.1 viewer. The local write routes go on the existing viewer server: POST only, a token cookie, exact `Host` and `Origin` checks, and no CORS. The one other endpoint this design adds is a Unix socket for the Claude hook, restricted to the OS user ([The control socket](#the-control-socket)); it is file-system IPC, not a TCP port. The failure mode being designed against is the one three tools in this space already shipped:

- **OpenCode, CVE-2026-22812:** its local server started with no auth and permissive CORS, so any website could reach its shell and pty routes.
- **claudecodeui, CVE-2026-31975 (CVSS 9.8):** a default JWT secret, WebSocket auth that skipped the user lookup, and a path interpolated into bash; unauthenticated remote code execution.
- **Claude Code IDE extensions, CVE-2025-52882:** an unauthenticated local WebSocket server.

A viewer that can approve a shell command is a remote-code-execution surface for anything that can make it do so. Every check in [Local write routes](#local-write-routes) exists because one of these three lacked it, or because loopback alone is not a boundary between OS users.

### 3. The human at the console is a new trust class

agent-messaging has one class of sender: a peer agent. Its rule 1 says a peer message is untrusted, carries no authority and can't approve a permission prompt. That rule still holds, unchanged.

This doc adds a second class: **the owner at the console.** It is the human who owns the machine, answering through the local viewer or, from step 3, through the native app on a device they paired at the machine. Only this class can answer a request.

**Agent-sent answers are refused** (Marvin, 01:23:07Z). While control is on, every request to the viewer (answers, polls, page loads, the first use of the URL token) is traced to the process that sent it, and it is refused when that process descends from a harness process. [Who is on the other end](#who-is-on-the-other-end) says how, and what the check cannot stop.

How the two docs relate:

| | Peer message (agent-messaging) | Owner at the console (this doc) |
|---|---|---|
| Who | Another agent session | The machine's owner |
| Authenticated by | Process ancestry of the MCP caller (#20) | The viewer's cookie, the peer's uid, and a peer process outside every harness's tree; later a paired device's signature |
| Surface | `semon mcp` write tools: `send_message`, `inbox` | The viewer's POST routes; later signed commands down the relay |
| Can approve a prompt or answer a question | Never (rule 1) | Yes, one request at a time |
| How the harness sees a message | Framed with its sender and "no authority" | As the user's own input (step 2) |

The two never cross. `semon mcp` gets no tool that lists or answers requests, and the driver's answer operation is reachable only from the viewer routes. An agent can't approve itself by messaging, and a peer can't borrow the console's authority. Both classes use the same ancestry method (#20), in opposite directions: agent-messaging uses it to name the sending agent, and this doc uses it to refuse an answer that comes from one. Where both need the same harness mechanism (for example Codex `turn/start` for step 2 and for agent-messaging's delivery), they share the adapter but not the entry point, and the framing is decided by the entry point.

### 4. Remote relay: paired-device signatures, signed only in the native app

For step 3, the machine opens an outbound connection to the embedding server and receives commands down it. The embedding server can relay commands but cannot forge them:

- **Pairing happens at the machine.** A pairing code or QR code is shown locally. A device paired this way has its public key in the machine's local list of paired devices. The machine verifies every command's signature against that list.
- **Signing happens only in the native app** (Marvin, 01:23:07Z). The guarantee holds only because the signing code is not served by the embedding server. A web page the embedding server serves could be changed by it to sign anything, so the web console can show requests but cannot answer them remotely.
- **Control is off by default** and is enabled at the machine, never remotely.
- **Each answer is bound** to one request id and one session, carries an expiry, and has replay protection (a per-device counter).
- **No "always allow" and no permission bypass** from a remote device in v1.
- **A local, append-only audit log** records every remote input.
- **Devices can be listed and revoked** at the machine.

### 5. Credentials stay where they are

Model credentials stay on the user's machine, inside the unmodified harness binaries. Semon never reads, stores, forwards or uses them. Both vendors draw this line:

- Anthropic does not permit third-party developers to route requests through Free, Pro or Max plan credentials on behalf of their users, and does not prevent an end user from signing in to the unmodified Claude Code binary with their own subscription (code.claude.com, legal and compliance).
- OpenAI states that app-server authentication has never been permitted for commercial or hosted services. Semon attaches to the daemon the user's own Codex started, under the user's own login, and never calls its login methods.

Semon feeds the user's own running session, or starts the user's installed binary as the user on the user's machine. It never runs a harness on the embedding server's side and never holds a token for one.

### 6. Phases

Each step is its own PR series and needs Marvin's go.

1. **Local approvals and questions in the viewer**, with no remote access. The lane splits it into step 1 (Claude) and step 1b (Codex and OpenCode); see below.
2. **Messages and interrupt** for Codex and OpenCode live sessions, plus full control of sessions Semon starts itself (all three harnesses).
3. **Remote relay with paired-device signatures.** This step makes shell-capable agents reachable from outside the machine. It is a reserved action: it needs Marvin's explicit go in his own words, naming it, not a relayed one.
4. **Messages into live Claude terminal sessions**, when channels are GA (or another documented entry point exists).

## Lane decisions

These are the Semon lane's defaults for step 1, not Marvin's decisions. He can change them when he approves the doc.

- **Step 1 is Linux-only.** Semon already reads `/proc`, and the checks below need `SO_PEERCRED`, `sock_diag` or `/proc/net/tcp`, and `/proc/<pid>/stat`. macOS (`getpeereid`, `$TMPDIR`, `libproc`) and Windows come later. On other platforms `--control` refuses to start.
- **Step 1 is Claude-first.** PRs 1 to 4 cover the request model, the routes, the UI and the Claude hook. Codex and OpenCode move to **step 1b**, gated on spikes S3, S4 and S5. PRs 1 to 3 show users nothing: there is no adapter until PR 4, so `--control` has no requests to show.

## Spike results

Run on 2026-09-30 on one Linux desktop (Ubuntu 24.04, AppArmor on), with Claude Code 2.1.285 and Codex CLI 0.159.2, in throwaway directories. Each result is a single run unless it says otherwise.

**S1: the terminal dialog while a `PermissionRequest` hook waits. Outcome A, with one gap.** The dialog is on screen and usable the moment the hook starts, and both sides can answer; the first answer wins.

| First answer | What happens to the hook | What Claude does |
|---|---|---|
| The hook prints `allow` | Exits normally. | The dialog closes, the terminal shows "Allowed by PermissionRequest hook", the tool runs. |
| The terminal: No, or Esc | SIGTERM within about 30 to 100 ms. | The turn is interrupted. |
| The terminal: Yes | **Not signalled.** It runs to its own wait budget with its sockets to Claude still open, and nothing on them shows the answer. A decision it prints afterwards is ignored. | The tool runs. |

So a terminal No or Esc is visible to Semon (the hook dies and its connection closes), but a terminal Yes is not, from the hook's side. Semon learns of it only when the tool has run: from a `PostToolUse` hook (after success) or a `PostToolUseFailure` hook (after a failure), or from the transcript's tool result for that input. The hook's input carries `session_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name`, `tool_name`, `tool_input` (the command verbatim, plus the model's `description`), `permission_suggestions` and `prompt_id`. It carries **no `tool_use_id`** (`PreToolUse` has one), so Semon matches a tool-run report to a request by session id, tool name and the hash of the tool input. Hook stdio is a Unix socket pair, and the hook's parent is `/bin/sh -c` under the Claude process. The documented default hook `timeout` is 600 seconds; no maximum is documented or was tested.

**S2: `AskUserQuestion` in a live terminal session. Yes, through `PermissionRequest`.** `PreToolUse` fires first, then `PermissionRequest`, both with `questions[]`. Returning `allow` with `updatedInput` = the input plus `answers: {"<question text>": "<label>"}` answered it in the live terminal; the model got the same result text a terminal answer gives. Tested once, with one single-choice question. Multi-select and free text are untested (S8).

**S6: the browser as the TCP peer.** Microsoft Edge (Chromium) is identifiable: all its TCP sockets are held by its network-service process, which runs in the host network and pid namespaces with a `/proc/<pid>/fd` the same user can read. Its renderers and zygote run in their own pid namespace with an unreadable fd directory, but hold no TCP sockets. Google Chrome is probably identifiable by the same design, unverified. Firefox (snap) and Flatpak browsers are unknown; they were not running or not installed. Two other strict-confinement snaps shared the host network namespace and had readable fd directories, which suggests a snap Firefox is visible, unverified. A browser Semon can't resolve is refused, as designed. "Every holder" in [Who is on the other end](#who-is-on-the-other-end) can only mean every holder Semon can see, since unreadable same-user processes exist on every Chromium desktop.

**S7: leaving the harness's tree from a sandbox.**

- **Claude's Linux sandbox (bubblewrap): the ancestry check holds.** A sandboxed command runs in its own network namespace, so a direct connection to 127.0.0.1 is refused. Through the sandbox's proxy the viewer's peer is the Claude process itself, which is a harness process and refused, or, once, no holder at all, which fails closed. `setsid` and a double fork stay under the sandbox and die with it; `systemd-run --user` and connecting to a tmux server's socket were denied. Claude's sandbox needs `socat` installed; without it the sandbox doesn't start (with `failIfUnavailable`, Claude refuses to run), and an unsandboxed agent is the out-of-scope case below.
- **Codex's Linux sandbox with network on: the ancestry check is bypassed.** The command shares the host network, and it can write to `/tmp`, where a tmux server's socket lives. It asked a tmux server to run `curl`, and the viewer's peer chain was `curl ← sh ← tmux server`, with no harness ancestor. Any same-user process that spawns commands on request over a Unix socket would do. `setsid`, a double fork and `systemd-run --user` did not escape.
- **Codex with network off:** nothing reached loopback.

## Threat model

In scope:

- **Web pages in the user's browser:** cross-site requests (CSRF), DNS rebinding, and CORS misconfiguration against the 127.0.0.1 viewer. A page on another 127.0.0.1 port is same-site but not same-origin.
- **Other OS users** on the same machine. Loopback is shared by every user, and cookies are not scoped by port: the browser sends the viewer's cookie to any `127.0.0.1:<port>`. Another user who binds a loopback port and gets the browser to visit it (a link, an image) receives the cookie. So the cookie alone can't authorize a write, and the server checks who is on the other end of the TCP connection.
- **Agent output rendered in the viewer:** a transcript is attacker-reachable text (prompt injection upstream of it). If it could become HTML in the viewer, it could press the approve button.
- **The agent being approved** trying to approve itself through the viewer, including a sandboxed command that is allowed to reach loopback (through Claude's sandbox proxy, for example). Marvin decided these answers are refused.
- **Misleading requests:** a request whose shown text differs from what will run (a summary instead of the command, hidden control, bidirectional or zero-width characters), or a fake request registered by some process to phish an approval.
- **Stale and replayed answers:** an answer meant for one request applied to another, or after the request changed.
- **From step 3, the embedding server and the network between it and the machine.**

Out of scope: code running as the same OS user outside every harness's process tree, and an agent that drives the human's own input rather than the viewer. An unsandboxed agent runs commands as the user, so it can escape the ancestry check by other means: `tmux send-keys` into the terminal that shows the dialog, the browser's automation or debugging port, or starting a process that is re-parented out of its tree (a double fork, `systemd-run --user`). The ancestry check closes the plain case, and closes the sandbox-escape case for Claude's Linux sandbox, where S7 found a sandboxed command can reach the viewer only through a proxy that shows Claude itself as the peer, and can't leave the tree. It does not close it for a Codex sandbox with network on: S7 found such a command can have a tmux server run a process outside the tree. For step 1b, a Codex session with network on is therefore treated like an unsandboxed agent (its answers can't be told apart from the owner's after such a hop), unless a later spike finds a second marker. It is not a sandbox. Within that, Semon still makes self-approval harder: the answer operation is not on `semon mcp` or on the control socket, the cookie secret is never written to disk by Semon or put in a child process's environment, and the URL token is single-use.

## The driver interface

A proposal; names are for discussion and will settle in PR 1.

A **session** is Semon's existing session key (harness plus native id). The driver adds, per live session, what it can do and why not:

- `approve`: live (hook installed, daemon attached, port known), or none with a reason ("Claude hook not installed", "Codex started with --no-daemon", "OpenCode port unknown", "Codex 0.161 not tested; read-only").
- `question`: live or none, same shape.
- `message`, `interrupt`: none in step 1; step 2 fills them in.
- `start`: none in step 1.

Operations:

- `requests()`: the pending requests, as below.
- `answer(request_id, answer, payload_sha256)`: claim one request, deliver the answer to the harness, and return the state that delivery ends in, which is as much as that harness lets Semon see (the per-harness rules below).
- Step 2 adds `send(session, text)`, `interrupt(session)` and `start(harness, cwd, prompt)`.

Adapters push events into the driver: a request appeared, a request was resolved elsewhere (in the terminal, by another client, by the harness's own timeout), a tool ran (Claude, matched by `match_key`), a session ended.

### The pending-request model

One record per question or permission prompt the driver has seen:

| Field | What it holds |
|---|---|
| `id` | Semon's own id: 128 random bits, hex. Never the harness's id, which is short (Claude's channel ids are five letters) and guessable. |
| `session` | The Semon session key the request belongs to. |
| `harness`, `harness_ref` | The harness and its own request id (Codex JSON-RPC id, OpenCode request id, the Claude hook's connection), kept inside the driver and never shown. |
| `match_key` | Claude only: the tool name and the SHA-256 of the tool input in the canonical encoding (for `AskUserQuestion`, the input without an `answers` field). Claude's `PermissionRequest` carries no tool-use id, so this is how a later tool-run report finds its request. |
| `kind` | `permission` or `question`. |
| `payload` | What the human is shown, taken verbatim from the harness. For a permission: the tool, the full command (argv or the exact shell string), the working directory, file paths and the diff for a file change, and any network or permission scope asked for. The model's own description is a separate field, shown second and labelled as the agent's words, never in place of the command. For a question: each question, its options, whether it is multi-select, and whether free text is allowed. |
| `answerable` | Yes, or no with a reason. See [Verbatim or read-only](#verbatim-or-read-only). |
| `payload_sha256` | The SHA-256 of the payload in its canonical encoding, RFC 8785 (JSON Canonicalization Scheme). The encoding is pinned in PR 1 because step 3 devices recompute the hash from what they display. An answer must carry it, so the human approves exactly what they saw. |
| `created` | When the driver first saw it. |
| `expires` | The earliest of the harness's own deadline (Codex `autoResolutionMs`, the Claude hook's wait budget) and Semon's. After it, Semon refuses answers and the harness's own prompt applies. |
| `state` | `open`, then `claimed` while an answer is being delivered, then exactly one final state (below). |

Final states:

- `answered`: Semon delivered the answer, as far as it can see (below). Records the answer and which window sent it.
- `resolved`: it ended elsewhere, with the reason: answered in another client (as the harness reported it), or, for Claude, one of two: "allowed in the terminal" (a tool-run report for the same `match_key` arrived while the request was open), or "denied or interrupted in the terminal" (Claude ended the hook, so its connection closed from Claude's side while the request was open; S1 saw it for No and for Esc, and never for Yes).
- `rejected`: OpenCode only. Semon delivered an answer and OpenCode refused it with an error status, for example a late reply to a prompt it had already closed. Claude and Codex give Semon no refusal to see.
- `left`: handed back to the harness's own prompt, with the reason: timed out (the request expired) or delivery unknown (below). For Claude this means the terminal is asking now. It never means the terminal answered.
- `gone`: the session or the adapter's connection to the harness ended.

Rules:

- **Claim, then deliver.** `answer` checks, in this order and under the store's lock: an unknown id is 404; a request that is not `open` answers 409 with its current state, which takes precedence over a hash mismatch; a read-only request answers 409 with its reason; a hash mismatch answers 412 with the current payload; an answer that doesn't fit the payload answers 400 (an allow or deny for a question, or `answers` whose keys aren't exactly the request's question texts or whose labels aren't among each question's options). The shape is checked before the claim, since once a request is claimed an adapter can no longer refuse the answer. Then a compare-and-set from `open` to `claimed` under the store's lock, so the first answer wins. Other windows show a claimed request as "Being answered in another window".
- **What ends `claimed` differs per harness,** because the harnesses confirm different things:
  - **Claude:** the hook gives no acknowledgement. Semon writes the decision to the hook's connection; when the write completes, the state is `answered` ("Sent to Claude"). If the connection is already closed, the state is `resolved`, "denied or interrupted in the terminal". Whether Claude acted on the answer shows only later, in the log.
  - **Codex:** a client's response to a server request gets no reply. Semon sends it and waits for `serverRequest/resolved` for that request; it then records `answered`. Whether the notification says which client resolved it is part of S3; if it does, a request the TUI resolved first becomes `resolved` instead.
  - **OpenCode:** the reply route returns an HTTP status. 2xx is `answered`; an error status is `rejected`, with the status shown.
- **A delivery deadline.** If `claimed` hasn't ended 10 seconds after the claim, the state becomes `left` with the reason "delivery unknown", and the card says to check the terminal.
- **Allow once, deny, or answer. Nothing broader.** Step 1 offers no "always", no "for this session", no rule or policy amendment (Codex `acceptForSession` and execpolicy amendments, OpenCode `always`, Claude `updatedPermissions`). Those change policy, and stay in the terminal. This also keeps step 1 identical to what step 3 allows a remote device.
- **Answers map per harness:** allow once is Claude `behavior: allow` with nothing else set, Codex `accept`, OpenCode `once`. Deny is Claude `behavior: deny` with a message, Codex `decline` (not `cancel`, which ends the turn), OpenCode `reject` with a message. The message says it was denied from Semon's viewer, plus the human's own text if they typed any.
- **A terminal Yes, for Claude.** S1 found that a Yes in the terminal doesn't end the hook, so Semon learns of it only when the tool has run. When a tool-run report (`PostToolUse` or `PostToolUseFailure`, or the transcript's tool result) arrives for a session and `match_key`, it is matched to the oldest request in that session with that key that no earlier tool run has matched, that wasn't denied or interrupted in the terminal, and that no later request has superseded. A request is superseded when a new one with the same session and key registers after it has ended unmatched: the session has moved on, so the next run belongs to the new request. Without this, a retry after a deny the agent honoured, after an allowed run that failed unreported, or after a timeout would take the retry's run, mark the old request falsely, and leave the retry open. Its cases:
  - `open`: it becomes `resolved`, "allowed in the terminal", and Semon tells the hook to exit with no decision, which frees its slot. A viewer answer after that gets 409 with that state, and the journal records the refusal, never an `answered`.
  - `answered` with a deny from the viewer: the tool ran anyway, because the terminal's Yes came first and Claude ignored the later deny. The state stays `answered`, but the record gains "the tool ran: the terminal allowed it first", the card says so, and the journal gets a line for it.
  - `claimed`: it is marked as matched, and if the claim then ends in a delivered deny, the record gains the same note.
  - `answered` with an allow or an answer, or `left`: it is only marked as matched.

  This leaves a window: from a terminal Yes until the tool finishes, which for a long command can be most of the wait budget, the request looks open, and a viewer answer sent in it is written to a hook Claude no longer listens to. The rule above makes that visible after the fact; no hook closes it sooner, since a tool call's hooks run `PreToolUse`, then `PermissionRequest`, then the post-tool hooks, with nothing between a terminal Yes and the run. Spike S9 tries to narrow it for Bash from outside Claude.

  Matching on the oldest unmatched request, and superseding, assume Claude asks for one session's tool calls in order. That is a known limit: subagents running in parallel share their parent's session id, so two identical calls from two subagents can be matched to each other's requests.
- **Every answer is journaled** locally ([The answer journal](#the-answer-journal)).
- **Held in memory.** The store is rebuilt on restart from what the harnesses can list (below); the journal is a record, not a source of truth.
- **Bounded.** At most 256 open requests and 64 waiting Claude hook connections at a time, and at most 4 waiting hook connections per session within those 64. A payload over 1 MiB in the canonical encoding (refused from a cheap lower bound on its size before it is encoded, then checked exactly), or a session id, tool name or harness reference over 256 bytes, is not registered; PR 4 also caps each control-socket frame at about 1 MiB, so an oversized hook input is refused before it is parsed. Beyond any of these, a new Claude hook gets no decision at once (the terminal asks) and the viewer says requests were dropped. Final requests are kept for 10 minutes, and at most 1,024 of them; the oldest go first.

### Verbatim or read-only

A request is answerable only when Semon holds the verbatim command, diff and working directory the harness will act on. Otherwise it is shown read-only, with the reason and where to answer it. Never a summary in place of the thing itself.

- **Claude:** the hook receives the tool's full input on stdin (the command for Bash, the old and new text or content for Edit and Write) and the cwd. Answerable. An `AskUserQuestion` is answerable when every question is single choice with distinct text; otherwise read-only until S8.
- **Codex (step 1b):** a command approval may carry the command, or only an item id that has to be joined with the `item/started` notification. A file-change approval may carry only an item id, with the diff elsewhere. Unverified; part of S3. Each approval kind becomes answerable only once S3 shows where its verbatim content comes from.
- **OpenCode (step 1b):** a permission carries patterns and metadata. Whether that is the verbatim command is unverified; part of S4.

## Step 1 in detail

### Turning control on

Control is off unless the viewer is started with `semon sessions --serve --control`. Only one process per machine may hold control: it takes an exclusive lock on `$XDG_RUNTIME_DIR/semon/control.lock`. A second `--control` fails with a message naming the pid that holds it. If `XDG_RUNTIME_DIR` is not set, `--control` refuses to start; there is no fallback to a shared path. Without `--control`, the server is exactly today's read-only viewer, including today's reusable `?t=`.

### The token and the cookie

With `--control`, the URL token and the cookie are two different secrets:

- The printed URL's `?t=` is **single-use**. The first GET that presents it gets a `semon_control` cookie (`HttpOnly; SameSite=Strict; Path=/`) holding a separate 128-bit secret, and the URL token is burned. A second use answers 403. A URL left in shell history, terminal scrollback or a browser's history is dead after the first load.
- **Another browser or window** gets a fresh single-use URL from `semon control url`, which asks the controlling process over the control socket. The same peer checks apply as for the viewer: the same uid, and not from a harness's process tree.
- **The cookie never authorizes on its own.** Since cookies are not scoped by port, the server also checks the TCP peer, its uid and its ancestry, on every request while control is on (below). That includes the GET that spends `?t=`: a process inside an agent's session that sees the printed URL and spends it with `curl` is refused before the token is burned, so it gets no cookie and can't read `/api/requests`. `Sec-Fetch-Site` only binds browsers, so it is not what stops such a client; the peer checks are.

### Who is on the other end

While control is on, the server resolves the other end of every loopback TCP connection, for every request: page loads, the GET that spends `?t=`, polls, and POSTs. The accepted connection gives the client's port. The client's socket, `127.0.0.1:<client port>` connected to the viewer's port, is looked up through `sock_diag` (or `/proc/net/tcp` as the fallback), which gives its owner uid and its inode. The inode is then looked up among every process's `/proc/<pid>/fd`.

- **Uid.** A request whose peer uid is not the controlling process's uid is refused (403). This is what stops another OS user holding a stolen cookie.
- **Every holder.** A socket can be held by several processes: a child inherits it across a fork, and a process can pass it to another over a Unix socket. So every process holding the inode is checked, and the request is refused if any of them fails the ancestry check. If no holder is found, it is refused. A socket already in TIME_WAIT reports inode 0 and has no holder, so it fails closed too. "Every" means every holder Semon can see: same-user processes whose fd directory is unreadable (Chromium's renderers and zygote, S6) can't be searched. That matters only if one of them also holds the socket while a visible holder outside every harness's tree does too, which already takes an escape from the tree (S7).
- **Ancestry.** Each holder's ancestors are walked through `/proc/<pid>/stat`. The request is refused when any ancestor is a harness process: a Claude process a pid file names, a Codex process (the daemon, or a writer-lock holder), an OpenCode process Semon knows, or a process whose executable is one of the harness binaries. A harness process is identified by its pid and its start time (`/proc/<pid>/stat` field 22), the same way Semon already checks Claude pid files, so a reused pid doesn't match. This is the #20 ancestry method. The refusal is journaled and the viewer says why ("This came from inside an agent's session"). A browser that an agent launched as its own child is refused too, which is correct: it is driven by the agent.
- **Fail closed.** If the uid or the holders can't be resolved, the request is refused with that reason. S6 found Edge resolvable (its network-service process holds its TCP sockets and its fd directory is readable), and Chrome probably so by the same design, unverified. Firefox, as a snap or otherwise, and Flatpak browsers are unknown; if Semon can't resolve one, every request from it is refused, and the refusal says the browser couldn't be identified.

### The control socket

The Claude hook has to reach the local process without a new port. The controlling process binds a Unix socket at `$XDG_RUNTIME_DIR/semon/control.sock`, in a 0700 directory it creates and then checks with `lstat` (a directory, not a symlink, owned by the user, mode 0700). It unlinks a stale socket only while it holds the lock, so a second process can never remove a live one. Each connection's peer is read with `SO_PEERCRED` and must have the same uid.

The socket has three operations, and no others: the permission hook registers a request and waits for its outcome, the tool hook reports that a tool ran (session id, tool name, tool input) and gets no reply beyond an acknowledgement, and `semon control url` asks for a single-use URL. There is no answer operation and no list operation. Frames are newline-delimited JSON with a version field.

**A registration or a tool report must come from the session it names.** The hook's input names a Claude session id. Semon finds the Claude process whose pid file names that session id (matched by pid and start time) and checks that the peer's pid descends from it. Anything else is refused: a process that isn't inside that Claude session can't put a card in front of the human (a phishing approval), fill the request slots, or close another session's card.

This can't exclude the session's own tool processes: a Bash command the agent runs also descends from its Claude process, so it could register a fake card for its own session, or report a tool run to close one of its own session's cards. Both are harmless for approval. A fake card's answer goes back only on the connection that registered it, to the fake registrant, and never reaches Claude's real prompt. A closed card sends its hook away with no decision, so the terminal still asks. The per-session cap (4 waiting registrations) keeps such a process from crowding out the other sessions.

### The Claude hook

- **The permission hook** is `semon hook claude-permission --wait <seconds>`. The wait budget is an argument, so it always sits a few seconds under the `timeout` the hook is installed with. Claude Code runs the hook for `PermissionRequest` and passes the request on stdin: session id, tool name, the tool's full input, cwd.
- **It returns at once, with no decision,** when Semon isn't running with `--control`, when the socket refuses it, or when the request is not registered (a cap, a payload too large). The terminal dialog then appears as if Semon weren't there.
- **Otherwise it waits** for its budget, whether or not a viewer is open, since the terminal dialog stays usable meanwhile (S1). For a permission it prints `hookSpecificOutput` with `hookEventName: "PermissionRequest"` and `decision.behavior` `allow` or `deny` (with a message) and exits. For a question it prints `behavior: allow` with `updatedInput`: the tool input exactly as received, plus `answers`.
- **It gives up to the terminal** (no decision, state `left`, "timed out") when its wait ends. If Semon's side of the socket drops (a restart), the hook returns no decision too; the new Semon process doesn't know the request. When Semon resolves the request as "allowed in the terminal", it tells the hook to exit with no decision; Claude ignores that output anyway.
- **If Claude ends the hook first** (the human chose No or pressed Esc in the terminal), Claude sends the hook SIGTERM (S1); an interrupt of the session is expected to do the same, unverified. The hook doesn't trap it, Semon sees the connection close from Claude's side, and records `resolved`, "denied or interrupted in the terminal".
- **A terminal Yes doesn't end the hook** (S1). The tool hook, `semon hook claude-tool-done`, runs for both `PostToolUse` and `PostToolUseFailure`: Claude documents `PostToolUse` as firing only after a tool succeeds, so without the failure hook a terminal Yes on a command that fails would never be reported. It sends Semon the session id, tool name and tool input, and exits at once with no output, whatever Semon answers or if Semon isn't there. It never blocks and never returns a decision. Semon matches the report as in [the pending-request model](#the-pending-request-model). Where the tool hook isn't installed, Semon can read the same fact from the transcript (`transcript_path` is in the permission hook's input): a `tool_use` with the same input, followed by its `tool_result`, counting results with `is_error` set as runs too. PR 4 uses the hooks; the transcript is the fallback if they prove unreliable.
- **Installing is opt-in and visible:** `semon control install claude` prints the exact entries it would add (`hooks.PermissionRequest`, `hooks.PostToolUse` and `hooks.PostToolUseFailure`, matcher `*`, the commands, the permission hook's `--wait` and an explicit `timeout` for each) and the settings file it would change, shows the diff, and writes only after the human confirms. `--print` only prints. `--scope user|project` picks the file. `semon control uninstall claude` removes those three entries and nothing else. Semon never edits settings silently, never on `--control`, and never from the viewer.

**The wait budget.** S1 found outcome A: the terminal shows its dialog while the hook waits, and the first answer wins. So there is no watching rule: the hook waits its budget whether or not a viewer is open, and the terminal is never held up. The budget is still short, 120 seconds by default (installed with `timeout: 130`, since Claude's default of 600 seconds would apply otherwise), for three reasons. A terminal Yes leaves the hook running until the tool finishes or the budget ends, holding one of the 64 waiting slots and one of its session's 4. A viewer answer sent after an unseen terminal Yes is ignored by Claude, and the budget bounds that window. And a card that outlives the prompt by minutes is more likely to be answered by someone who no longer knows the context. `--wait` changes it.

### Codex and OpenCode adapters (step 1b)

Both wait for their spikes. Each is its own PR.

- **Codex ships only if S3 shows** that the daemon sends each server request to every attached client and sends `serverRequest/resolved` to the others. If only one client receives it, attaching could take the prompt away from the TUI, and the adapter doesn't ship. When it ships: with `--control`, Semon connects to the daemon's control socket, calls `initialize`, checks the version against the tested list, and subscribes to the loaded threads. An approval or input request becomes a pending request, answerable only when its verbatim content is in hand. `serverRequest/resolved` for an open request makes it `resolved`, and for a claimed one ends the claim (see the per-harness rules above). A version outside the tested list keeps the connection read-only: requests are shown, with "This Codex version isn't tested; answer in Codex". Sessions run with `--no-daemon` show approvals as none, with that reason. The ancestry check doesn't hold for a Codex sandbox with network on (S7), so such a session is treated like an unsandboxed agent: the check still runs, but the doc claims nothing for it beyond the plain case.
- **OpenCode ships after S4 and S5.** Semon connects to the TUI's server at a known port. The port comes from `semon opencode`, a thin wrapper that launches `opencode --port <free port>` and records the port and pid in `$XDG_RUNTIME_DIR/semon/opencode/<pid>.json`, or from the human running `opencode --port` and naming it once. Semon reads `/event` for `permission` and `question` events and lists `GET /permission` and `GET /question` on connect. It talks only to the server that owns the session and never starts a second `opencode serve` on the same store.

### Local write routes

The local `Viewer` wrapper in `viewer.rs` gains two routes, answered before it hands a request to `ViewerCore`. `ViewerCore` itself stays GET-only, so a server embedding it gets no write surface by construction; the routes exist only in the local process.

- `GET /api/requests`: the open and claimed requests and those resolved in the last 10 minutes, plus each live session's capabilities. It is an ETag'd JSON answer like `/api/model`. Like every request, it has already passed the peer uid and ancestry checks.
- `POST /api/requests/<id>/answer`: body `{"payload_sha256": "…", "decision": "allow" | "deny", "message": "…"}` for a permission, or `{"payload_sha256": "…", "answers": {"<question text>": "<option label>"}}` for a question, with one entry per question and each label one of that question's options. It answers 200 with the final state; 404 for an unknown id; 409 with the current state when the request is not `open` (someone else answered or is answering, it was left or resolved elsewhere, including "allowed in the terminal"); 409 with the reason when the request is read-only; 412 with the current payload when the hash doesn't match (the 409s are checked first); and 400 when the answer doesn't fit the request (the wrong kind, or `answers` that aren't exactly its questions with labels among their options). When delivery takes more than 5 seconds it answers 202 with `claimed`; the card then says "Delivering…" until its poll shows the final state, which the 10-second delivery deadline bounds.

Checks on every request while control is on, in this order, each failing closed with 403 and no body beyond "Forbidden":

1. Exactly one `Host` header, equal to `127.0.0.1:<port>` or `localhost:<port>` (as today). This defeats DNS rebinding.
2. The TCP peer's uid is the controlling process's uid, and no process holding the peer's socket descends from a harness process ([Who is on the other end](#who-is-on-the-other-end)).
3. The `semon_control` cookie holds the cookie secret, or this is the one GET that spends the single-use `?t=`. The URL token never authorizes a write.

And on every POST, in addition:

4. Control is on (`--control`); otherwise 405 as today.
5. An `Origin` header is present and equals `http://` plus that same `Host`, and `Sec-Fetch-Site`, when present, is `same-origin`. A missing `Origin` is refused.
6. `Content-Type` is exactly `application/json` (so a cross-site form can't send it without a preflight, which is never answered), and the body is at most 64 KiB of valid JSON with no unknown fields.

No response ever carries an `Access-Control-*` header. `OPTIONS` gets 405. The CSP and the other `SECURITY_HEADERS` apply as today. The viewer's existing banned-string test (no `innerHTML` and the like in the served script) becomes a control-security property: payloads and transcripts are text nodes only.

### How the viewer shows a waiting request

Reuse the signals that exist; add no new vocabulary.

- **State.** A session with an open request reads `wait`, "Needs you", with the amber dot, exactly like an open question today. It counts in Home's "waiting on you", appears in Home's "Needs you" list, and lights the rail's attention dot.
- **Home's "Needs you" item** for a request says what is asked, in one line ("Claude wants to run `cargo test -p semon-push`", "Claude asks: Which branch?"), with the session and age, and a "Review" action.
- **On the session page** the request is a card at the transcript's foot, where the waiting question card sits today. When the question already appears in the transcript as a "to you" card, that card gains the answer form instead of a second card appearing. When the card is scrolled away, the jump control says "Needs you" and scrolls to it.
- **The request card is chrome, not transcript.** It is drawn from `/api/requests` only, outside the transcript's column and style, with the harness icon and the words "Semon: waiting on you". Text in a transcript can look like anything, but it can't produce this card.
- **While claimed**, the card that sent the answer says "Delivering…" (after a 202 too), and other windows say "Being answered in another window".
- **Final states** stay on the card for 10 minutes, with the outcome and where it came from: "Allowed here at 14:02, sent to Claude", "Answered in another window", "Answered in Codex" (when S3 shows the notification names the client), "Allowed in the terminal", "Denied or interrupted in the terminal", "Left to the terminal: timed out", "Delivery unknown: check the terminal", "OpenCode refused the answer (409)". A viewer deny that the terminal's Yes overtook reads "Denied here at 14:02, but the terminal allowed it first; the tool ran". A card never says the terminal answered unless the harness or the log says so.
- **Read-only requests** show the same card without the buttons, with the reason and where to answer.

The answer form:

- **Exact text.** The command is shown whole, in a monospace block that wraps (no sideways scroll). A command is never folded or cut, however long. Only a diff folds, behind "Show all N lines". Control characters (C0, C1, ANSI escapes), bidirectional controls (U+202A to U+202E, U+2066 to U+2069) and zero-width characters (U+200B to U+200F, U+2060, U+FEFF) are drawn visibly, as a marked code such as `⟨U+202E⟩`, and the card says the command contains them. Newlines and tabs show as themselves.
- **Desktop (1280 px):** the card shows the tool, the command, the working directory, and for a file change the diff. Below it, "The agent says" and the model's description, in muted text. Then "Deny" on the left and "Allow once" on the right, with room between them, and a message field that opens under "Deny". Neither button has focus on arrival, there is no keyboard shortcut for allow, and both buttons are disabled while a POST is in flight.
- **Phone (390 px):** "Review" opens a bottom sheet the height of the content, up to the screen: the same blocks, scrolling vertically, with the two buttons pinned at the bottom above the safe area, each at least 44 px tall and half the width. No swipe-to-approve.
- **Questions:** the options as radio buttons and one "Send answer" button. Multi-select and free-text questions are read-only in step 1 (S8), with "Answer this in the terminal".

### Timeout, restart, two viewers

- **Timeout.** When `expires` passes, the request becomes `left` and further answers get 409. For Claude the hook has already returned no decision, so the terminal is asking. For Codex, the harness's own `autoResolutionMs` behaviour applies. OpenCode has no deadline; Semon's expiry for it only closes the viewer's form, and the TUI keeps asking.
- **Restart of Semon.** The run's secrets change, so open viewers must be reopened with a new URL. Claude hooks lose their socket and return no decision, so the terminal asks; the new process doesn't know those requests. Codex and OpenCode requests are rebuilt from the harness on reconnect (OpenCode lists them; Codex is S3). An answer POST in flight during a restart fails; the viewer shows "Semon restarted" and, once reopened, the true state.
- **Two viewers answering at once.** The first POST claims the request. The second gets 409 with `claimed` or the final state, and its card shows "Being answered in another window", then the outcome. When the terminal or another client ends the request first, the card shows the `resolved` reason the harness allows Semon to see.
- **The session ends** while a request is open: the request becomes `gone`.

### The answer journal

Every answer, every refused answer, every refused registration and every tool run found after a viewer deny is recorded in `$XDG_STATE_HOME/semon/control/journal.jsonl` (0600, in a 0700 directory, opened for append only), one JSON object per line: time, source (the window, the peer pid and uid; from step 3 the device id), request id, session, harness, kind, the answer, the payload hash, and the outcome (the final state, or refused and why). An answer's line is written when its claim ends, so it carries the final state. Refused registrations are coalesced so a flood can't grow the file without bound: at most one line per session per minute, carrying the count suppressed in that session's previous minute, with the session key cut to 64 bytes; the viewer's dropped count stays exact. The file is opened with `O_NOFOLLOW` and must belong to the user; after a failed write, the next line starts on a fresh line. In step 1 it is a plain append-only record. If a line can't be written (a full disk), the answer still stands, the error goes to stderr, and the viewer shows that the journal is failing. Step 3 reverses this for remote answers: it fails closed, writing the journal line before delivery and refusing the answer when that write fails. It is not tamper-evident: the user, and code running as the user, can edit it. Step 3 adds a hash chain with an anchor outside the machine's writable state, and only then claims tamper evidence.

## Step 1 PRs

One PR per point (R-20260929-10). PR 1 and PR 2 go first; PR 3 and PR 4 can then run in parallel. Users see nothing until PR 4 lands. Spikes S1, S2, S6 and S7 have run; S8 (multi-select and free-text questions) doesn't gate step 1, which shows those read-only.

| PR | What | Acceptance tests |
|---|---|---|
| 1 | The driver interface and the request store, with a fake adapter, in a new workspace crate, `crates/semon-control`. It depends on no other Semon crate, so the viewer (PR 2) and the hook commands (PR 4) can both use it, and it reuses the workspace's RFC 8785 encoder. No routes, no UI, no hooks. | Unit: the RFC 8785 test vectors give the pinned encoding, and a payload's hash is the same across key orders and number spellings; a number outside the I-JSON safe range makes the request read-only instead of hashing ambiguously; first claim wins under concurrent answers from several threads; 409 comes before 412 when both apply; a wrong hash on an open request is 412 with the current payload; the fake adapter's delivery modes end `claimed` as specified (a completed write, a closed connection, a later resolved notification, a status code), and `rejected` is reachable only through the status-code mode; a claim with no delivery outcome becomes `left`, "delivery unknown", at the deadline; answers after `expires` are refused and the request becomes `left`, "timed out"; every final state is final; `gone` on session end; a tool run matched by `match_key` resolves the oldest unmatched open request as "allowed in the terminal", after which an answer gets 409 and the journal records a refusal, not an answer; a tool run after a viewer deny marks the record and adds a journal line; two identical requests in one session are matched in order; a request registering after an earlier one with the same session and key ended unmatched supersedes it, so the next tool run resolves the new one: after a honoured viewer deny, after an allowed run that failed unreported, and after a timeout; question answers are checked against the payload before the claim (400 for a wrong question text, an unknown label or an extra key), and multi-select, repeated-text and option-less questions register read-only; a delivery that outlasts the deadline ends `left`, "delivery unknown"; the caps (256 open, 64 hook waits, 4 per session, 1 MiB payload refused before encoding, 256-byte session id, tool name and harness reference, 1,024 retained final requests) hold as invariants, with the overflow reported; the journal gets one line per answer and per refused answer, coalesces refused registrations per session per minute with the suppressed count, is created 0600 in a 0700 directory, refuses a symlinked directory or file, and starts a fresh line after a failed write. |
| 2 | `--control` (Linux only, refuses without `XDG_RUNTIME_DIR`), the single-holder lock, the single-use URL and the cookie secret, the peer uid and ancestry checks, and the two routes on the local `Viewer`. | HTTP, in the style of today's viewer tests. The URL token works once, then 403, and the cookie's value is not the URL token. Refused: a wrong or duplicated `Host`; a missing or foreign `Origin`; `Sec-Fetch-Site: cross-site` or `same-site`; the `?t=` token on a POST; a wrong cookie; a `text/plain` or form body; an oversized body; unknown fields. A peer with another uid is refused (through an injected resolver; a real-socket test checks that the resolver finds the test's own uid and pid). A request from a process whose ancestor is a registered harness process is refused and journaled for every route: the GET that spends `?t=` (which then stays unspent), a page load, a poll of `/api/requests` carrying a forged `Sec-Fetch-Site: same-origin`, and each POST; the same requests from a sibling process are accepted. A socket held by two processes, one inside a harness's tree, is refused. A registered harness pid reused by an unrelated process (a different start time) doesn't count as a harness. A peer with no holder found, or a TIME_WAIT socket, is refused. `OPTIONS` is 405; no response carries `Access-Control-*`. 200 then 409 for two answers; 412 on a stale hash; 409 before 412. `ViewerCore::respond` still returns 405 for POST. A second `--control` fails naming the holder. Without `--control`, today's tests pass unchanged. |
| 3 | The viewer UI: state, Home, the rail dot, the request card, the phone sheet, the final and read-only states, the question form. | UI checks at 390 and 1280 px, light and dark: a fixture request shows on Home and on the session page; allow and deny send the right body; the second window shows "Being answered in another window"; a 2,000-character command is shown whole with no sideways scroll at 390; a command containing `<img src=x onerror=…>` renders as text; U+202E, U+200B and an ANSI escape in a command render as visible marks; a diff folds and a command never does; the banned-string test still passes; no button is focused on arrival; a single-choice question sends `answers` keyed by question text; a multi-select or free-text question shows read-only; a deny the terminal overtook shows "the terminal allowed it first". |
| 4 | Claude: the control socket, `semon hook claude-permission` (`PermissionRequest`), `semon hook claude-tool-done` (`PostToolUse` and `PostToolUseFailure`), and `semon control install` / `uninstall claude`, which install all three entries. | The permission hook returns no decision within 100 ms when Semon is down, when control is off, and when a cap refuses the registration; with Semon up and no viewer open it waits its budget. It prints exactly the allow and the deny output, each with `hookEventName`, and for a question `updatedInput` equal to the received input plus `answers`; it honours `--wait`; it returns no decision on timeout, and the request becomes `left`, "timed out"; it returns no decision when Semon's side of the socket drops. Terminal deny: when Claude's side closes first (the hook SIGTERMed), the request becomes `resolved`, "denied or interrupted in the terminal", never `left`. Terminal allow: the hook's connection stays open, a tool-done report with the same session, tool name and input makes the request `resolved`, "allowed in the terminal", the permission hook then exits with no decision, and a later viewer answer gets 409 with no `answered` journal line; the same holds when the allowed command fails and the report comes from `PostToolUseFailure`. On the pinned Claude version, the `tool_input` that `PostToolUse` and `PostToolUseFailure` carry hashes the same as the `PermissionRequest`'s, for Bash, Edit and `AskUserQuestion` (without `answers`). The tool-done hook exits at once with no output, whether Semon is up, down or refuses it. A control-socket frame over about 1 MiB is refused before it is parsed. A completed write of the decision records `answered`, and no `rejected` state is produced for Claude. A registration or tool report from a process that doesn't descend from the Claude process named for its session id (pid and start time) is refused. A fifth waiting registration for one session gets no decision at once. A peer with another uid is refused. A second instance never unlinks the live socket; a stale one is replaced only under the lock. Install writes nothing without confirmation, prints the diff, is idempotent, and uninstall removes only its own three entries. |

Step 1b, each after its spikes and its own go:

| PR | What | Acceptance tests |
|---|---|---|
| 5 | Codex: the daemon adapter and the version gate. Ships only if S3 shows server requests reach every attached client. | Against a fake daemon replaying frames recorded from the pinned Codex version: an approval and a user-input request become pending requests; each approval kind is answerable only with its verbatim content, joined from `item/started` where S3 says so; allow once sends `accept` and deny sends `decline`, never `acceptForSession` or `cancel`; `serverRequest/resolved` makes an open request `resolved` and ends a claimed one as `answered`; a claim with no notification becomes `left`, "delivery unknown", at the deadline; an untested version is read-only with its reason; a dropped socket makes open requests `gone`. |
| 6 | OpenCode: the `semon opencode` wrapper, the port registry and the adapter. | Against a fake server: permission and question events become pending requests, read-only unless S4 found the verbatim command; allow once sends `once`, deny sends `reject` with the message, never `always`; a 2xx reply records `answered` and an error status `rejected`; pending requests are rebuilt from `GET /permission` and `GET /question` after a reconnect; an unknown port shows approvals as none with its reason. |

Follow-ups, not in step 1: `waiting-permission` in `semon query` (the driver is the exact signal it was waiting for), and exact permission-wait spans in "Waited on you" from the journal.

## Steps 2 to 4, in outline

- **Step 2.** `send` maps to Codex `turn/start`, or `turn/steer` with `expectedTurnId` while a turn runs, and to OpenCode `prompt_async`. `interrupt` maps to Codex `turn/interrupt` and OpenCode `abort`. The message box sits under the transcript, and a sent message shows as the user's own turn once the log carries it. Sends and interrupts get the same peer checks as answers. Sessions Semon starts run the user's installed binary as the user: `claude -p` with stream-json (approvals and questions through the permission callback, interrupt, messages), `thread/start` on the Codex daemon, `POST /session` on an OpenCode server Semon launched. Claude terminal sessions stay approvals-only.
- **Step 3.** The machine keeps one outbound connection to the embedding server, authenticated by the machine's existing push token for transport, and receives signed commands down it. Only the native app signs. The web console the embedding server serves shows requests and says to answer them in the app or at the machine. Pairing: `semon control pair` shows a QR code (and a code to type) holding the machine's public key and a one-time 128-bit secret that expires in five minutes. The app makes its own key pair and proves it knows the secret, and the machine shows the app's key fingerprint as words, which the human compares with the phone's before confirming at the machine. The machine signs each request it announces; the app checks that signature and hashes, in the pinned encoding, the payload it displays, so a server that alters the text shown gets an answer the machine refuses. Each command carries the device id, request id, session, answer, payload hash, expiry and a per-device counter that must increase. The machine checks the device is paired and not revoked, the signature, that the request is open and in that session, the hash, the expiry and the counter, then journals the outcome. The journal gains its hash chain and an external anchor here. `semon control devices` lists and revokes. Remote control is off until `semon control remote on` at the machine.
- **Step 4.** Messages into live Claude terminal sessions, through channels once they are GA and need no development flag, or another documented entry point.

## Open questions and the spike that answers each

| # | Question | Spike |
|---|---|---|
| S1 | While a Claude `PermissionRequest` hook waits, does the terminal still show its dialog, and if the human answers there first, is the hook killed and the answer kept? | Answered: outcome A. The dialog stays usable; No and Esc SIGTERM the hook; Yes does not signal it, so `PostToolUse` or `PostToolUseFailure` reports it ([Spike results](#spike-results)). |
| S2 | Can Semon answer `AskUserQuestion` in a live Claude terminal session? | Answered: yes, single choice, through `PermissionRequest` with `updatedInput.answers` ([Spike results](#spike-results)). |
| S3 | With the Codex TUI and Semon both attached to the daemon, does every client get `requestApproval` and `requestUserInput`, and do the others get `serverRequest/resolved`? Are pending requests re-sent after a reconnect? Does each approval kind carry its verbatim command or diff, or only an item id to join with `item/started`? | Attach a small read-only client to the daemon beside a TUI at the pinned version, trigger a command and a file-change approval, record the frames, answer in the TUI, then reconnect mid-request. |
| S4 | How does a client authenticate to an OpenCode TUI's server since the CVE-2026-22812 fix (per-process token, `OPENCODE_SERVER_PASSWORD`, or none on loopback), and can Semon learn it without reading OpenCode's files? Is a permission's patterns and metadata the verbatim command? | Start the TUI with `--port`, inspect what a request without and with credentials gets, trigger a shell permission and compare its payload with the command run, and read the fix's change. |
| S5 | Are OpenCode's question routes, which are in source but not in the docs, stable enough to depend on? | Diff the routes across the last several releases; if they moved, gate questions on the version like Codex. |
| S6 | Does the peer lookup (socket to uid, inode to every holding process) work for the browsers people use on Linux? | Partly answered: Edge yes, Chrome probably (unverified). Still open: Firefox, snap and Flatpak builds. Open the viewer from each with `--control` once PR 2 lands and log what the resolver finds; a browser it can't resolve is refused. |
| S7 | Can a command inside Claude's or Codex's Linux sandbox leave the harness's process tree? | Answered: not from Claude's sandbox; yes from Codex's with network on, through a tmux server ([Spike results](#spike-results)). Still open: a second marker that would let step 1b tell such a hop from the owner. |
| S8 | How are multi-select and free-text `AskUserQuestion` answers given through `updatedInput.answers` (a joined label string, a list, the typed text)? | On a throwaway session, answer a multi-select and an "Other" question from the terminal, read the `answers` the tool result reports, then return the same shape from a hook and compare. |
| S9 | Can Semon see a terminal Yes on a Bash request before the command ends? No hook runs between the Yes and the run, but the command starts as a new child of the Claude process. Before writing a viewer deny for a Bash request, Semon would look in `/proc` for a new child of that session's Claude process whose command line contains the command, and answer 409, "the terminal allowed it first", instead. Unverified; Edit and Write finish in milliseconds, so only long Bash commands matter. | On a throwaway session, allow a long command in the terminal while a viewer deny is pending, and check whether the child is found by pid ancestry and command line before the deny is written, and how often the check would misfire on an unrelated child. |
| Q6 | The Claude hook's default budget (120 seconds proposed) and whether Claude caps the hook `timeout`. | Settle in PR 4. |
| Q7 | How much of a very large diff to show before "Show all", and whether a payload over a size limit can be answered at all. | Settle in PR 3 with real file-change requests. |
| Q8 | Step 3: should payloads also be encrypted end to end so the embedding server can't read them, not only signed so it can't forge them? Paseo does this; Omnara chose not to. | Decide with step 3's own doc. |
| Q9 | Step 3: may a paired device start a session remotely, or only answer, message and interrupt? It runs the user's own binary under the user's login, but it moves closer to a hosted product driving a subscription login. | Decide with step 3's own doc, after rereading both vendors' terms at the time. |

## Risks

- **The ancestry check is not a sandbox.** It refuses requests from inside an agent's process tree. An unsandboxed agent can still reach the human's own input (tmux, the browser's automation port) or re-parent a process out of its tree. S7 found the check holds for Claude's sandbox but not for Codex's with network on, where a tmux server runs a command outside the tree; such sessions count as unsandboxed. Claude's sandbox also needs `socat`, and without it Claude runs unsandboxed.
- **Delivery is only as visible as each harness makes it.** Claude and Codex never refuse an answer in a way Semon can see, so `answered` means "delivered" for them, not "acted on"; the log shows the rest.
- **Codex's control surface is experimental**, and the daemon auto-attach is documented only in source. The version gate limits the damage to "read-only until tested", but every Codex release may need a test run and a new entry in the tested list.
- **A terminal Yes is invisible until the tool has run.** S1 showed Claude neither ends nor tells the hook, and no hook runs between the Yes and the run. Until the tool-run report arrives, the card looks open, and a deny sent from the viewer is ignored; the record then says the terminal allowed it first. A long command makes this window long, up to the hook's budget. S9 may narrow it for Bash.
- **Tool-run matching assumes in-order calls per session.** Parallel subagents share a session id, so identical calls from two of them can be matched to each other's requests. The effect is a wrong "allowed in the terminal" or "the terminal allowed it first" on a card, never an answer delivered to the wrong prompt, since answers go only to the hook connection that registered the request.
- **OpenCode needs the port.** Sessions started without the wrapper or `--port` stay read-only.
- **Peer resolution fails closed.** A browser whose process Semon can't see can't use the viewer while control is on. S6 found Edge resolvable and Chrome probably so; Firefox and snap or Flatpak builds are unknown until they are tried against PR 2.
- **The viewer becomes security-critical.** Any HTML injection in the viewer would now be an approval. The CSP without `unsafe-inline` and the banned-string test are what hold that line; both stay, and the request card never renders a string as HTML.
- **Step 3 makes shell-capable agents reachable from outside the machine.** That is why it is a reserved action, with its own doc and Marvin's explicit go.
