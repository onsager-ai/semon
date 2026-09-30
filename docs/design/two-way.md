# Two-way control: answering and steering agents from Semon

Status: design only. Marvin chose "Design doc, then step 1 (Recommended)" and, for how far a machine trusts the embedding server, "Paired-device signatures (Recommended)" (Semon session transcript `10c47490-e569-49f3-a26c-8cbd8399c134.jsonl`, line 33129, 2026-09-30T01:00:34Z). Nothing here is implemented. Each step below is its own PR series and needs its own go; step 1 starts after Marvin approves this doc.

## Goal

Today Semon only reads. It shows every session of every harness in one tree, and it shows when a session waits on you, but you still have to find the right terminal to answer. Two-way control closes that loop: answer a permission prompt or a question from the viewer, then send a message or interrupt a run, and later do the same from a phone.

The hard part is not the UI. It is that each harness exposes a different control surface, with different stability, and that a page able to approve a shell command is exactly the kind of local server that has produced critical CVEs in this space. This doc fixes the architecture and the trust model first, then designs step 1 in enough detail to split into PRs.

## What exists today

Verified on main, 2026-09-30:

- **No write path anywhere.** `semon` is a set of subcommands. `semon sessions --serve` is the only long-running local server (`viewer.rs:621`). It binds 127.0.0.1 only (`listen_addr`, `viewer.rs:597`), makes a random token per run (`viewer.rs:614`), turns it into an `HttpOnly; SameSite=Strict` cookie (`viewer.rs:698`), requires exactly one `Host` header equal to `127.0.0.1:<port>` or `localhost:<port>`, and answers GET only (`authorized`, `viewer.rs:744`; `ViewerCore::respond` returns 405 for anything else, `viewer.rs:1372`). `semon mcp` is stdio and read-only.
- **The push to an embedding server is one way** (`docs/mirror-protocol.md`): the machine POSTs log bytes and facts. Nothing comes back down except a status and a length.
- **Live-process facts:** Claude pid files (`~/.claude/sessions/<pid>.json`) give pid, session id and busy or idle; Codex liveness is a held writer lock. Semon records no tty and no socket, and the harness CLI version is not modelled. `HarnessDefinition` (`harness.rs:8`) is only names and icons.
- **The "needs you" signals already exist.** A live session with an open question or decision to you has state `wait` ("Needs you", the amber dot; `lineage_states` in `model.rs`, `STATE` in `viewer.js`). Home has a "Needs you" section and a "waiting on you" count. Analytics has "Waited on you". The overhaul gives the rail attention dots (`overhaul.md`). `semon query` has `waiting-question` and reserves `waiting-permission` until an exact signal for a pending permission prompt exists (`query.rs:46`).
- **Agent messaging** (`docs/design/agent-messaging.md`) is design only. It covers agent-to-agent messages, not a human at the console.

## Decisions

These were made with Marvin before this doc and are not reopened here.

### 1. One local process, one driver interface, one adapter per harness

One local Semon process per machine is the only thing that touches harnesses. It talks to them through one internal driver interface whose vocabulary is borrowed from the Codex app-server and ACP: sessions, turns, approvals, questions, interrupt, start. In all three harnesses a permission prompt is already a request from the agent that blocks until a client answers, so the interface models it that way.

| Harness | Adapter | Live attach to a terminal session | Stability |
|---|---|---|---|
| Codex | The shared app-server daemon's control socket, `~/.codex/app-server-control/app-server-control.sock` (WebSocket over a Unix socket). `turn/start`, `turn/steer`, `turn/interrupt`; the server's `item/*/requestApproval` and `item/tool/requestUserInput` requests; `serverRequest/resolved`. | Yes: since 0.157.0 the TUI reuses the daemon by default (`--no-daemon` opts out). | `codex app-server` is documented as experimental. The adapter is version-gated and degrades to read-only on a version it hasn't been tested against. The daemon auto-attach is verified in source only. |
| OpenCode | The TUI's own HTTP server: `POST /session/:id/prompt_async`, `GET /permission` and `POST /permission/:id/reply`, `GET /question` and `POST /question/:id/reply`, `POST /session/:id/abort`, `GET /event` (SSE). | Yes, once Semon knows the port. | GA, except the question routes, which are in source but not in the docs. The port is random unless the TUI is started with `--port`; there is no documented registry. Semon has to be told the port, by launching OpenCode through a wrapper or with `--port`. |
| Claude Code | A `PermissionRequest` hook that long-polls the local Semon process for an answer. On timeout, or when Semon isn't there, the hook returns no decision and the terminal dialog is the fallback. | Approvals only. | Hooks are GA. |

Claude Code specifics, decided:

- **Answering an `AskUserQuestion` in a live terminal session is unverified.** It is an open question with a spike (S2 below). Until the spike says yes, the viewer shows the question and says to answer it in the terminal.
- **Messages into a live Claude terminal session are not supported in v1.** The per-session inbox socket frames every message as "not typed by your user", so it cannot stand in for the human, and its frame format is undocumented. Channels need a launch flag and are a research preview. This matches agent-messaging's refusal to write to that socket.
- **Sessions Semon starts itself get full control:** `claude -p` with `--input-format stream-json --output-format stream-json`, or `claude --bg`, gives messages, approvals and questions (through the SDK's permission callback), and interrupt.

### 2. No new listening port

No listening port beyond today's 127.0.0.1 viewer. The local write routes go on the existing viewer server: POST only, the existing per-run token cookie, exact `Host` and `Origin` checks, and no CORS. The failure mode being designed against is the one three tools in this space already shipped:

- **OpenCode, CVE-2026-22812:** its local server started with no auth and permissive CORS, so any website could reach its shell and pty routes.
- **claudecodeui, CVE-2026-31975 (CVSS 9.8):** a default JWT secret, WebSocket auth that skipped the user lookup, and a path interpolated into bash; unauthenticated remote code execution.
- **Claude Code IDE extensions, CVE-2025-52882:** an unauthenticated local WebSocket server.

A viewer that can approve a shell command is a remote-code-execution surface for anything that can make it do so. Every check in [Local write routes](#local-write-routes) exists because one of these three lacked it.

### 3. The human at the console is a new trust class

agent-messaging has one class of sender: a peer agent. Its rule 1 says a peer message is untrusted, carries no authority and can't approve a permission prompt. That rule still holds, unchanged.

This doc adds a second class: **the owner at the console.** It is the human who owns the machine, answering through the local viewer (authenticated by the per-run token cookie) or, from step 3, through a device they paired at the machine (authenticated by that device's signature). Only this class can answer a request.

How the two docs relate:

| | Peer message (agent-messaging) | Owner at the console (this doc) |
|---|---|---|
| Who | Another agent session | The machine's owner |
| Authenticated by | Process ancestry of the MCP caller (#20) | The viewer's token cookie; later a paired device's signature |
| Surface | `semon mcp` write tools: `send_message`, `inbox` | The viewer's POST routes; later signed commands down the relay |
| Can approve a prompt or answer a question | Never (rule 1) | Yes, one request at a time |
| How the harness sees a message | Framed with its sender and "no authority" | As the user's own input (step 2) |

The two never cross. `semon mcp` gets no tool that lists or answers requests, and the driver's answer operation is reachable only from the viewer routes. An agent can't approve itself by messaging, and a peer can't borrow the console's authority. Where both need the same harness mechanism (for example Codex `turn/start` for step 2 and for agent-messaging's delivery), they share the adapter but not the entry point, and the framing is decided by the entry point.

### 4. Remote relay: paired-device signatures

For step 3, the machine opens an outbound connection to the embedding server and receives commands down it. The embedding server can relay commands but cannot forge them:

- **Pairing happens at the machine.** A pairing code or QR code is shown locally. A device paired this way has its public key in the machine's local list of paired devices. The machine verifies every command's signature against that list.
- **Control is off by default** and is enabled at the machine, never remotely.
- **Each answer is bound** to one request id and one session, carries an expiry, and has replay protection (a nonce or a per-device counter).
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

1. **Local approvals and questions in the viewer**, for all three harnesses, with no remote access. Designed in detail below.
2. **Messages and interrupt** for Codex and OpenCode live sessions, plus full control of sessions Semon starts itself (all three harnesses).
3. **Remote relay with paired-device signatures.** This step makes shell-capable agents reachable from outside the machine. It is a reserved action: it needs Marvin's explicit go in his own words, naming it, not a relayed one.
4. **Messages into live Claude terminal sessions**, when channels are GA (or another documented entry point exists).

## Threat model

In scope:

- **Web pages in the user's browser:** cross-site requests (CSRF), DNS rebinding, and CORS misconfiguration against the 127.0.0.1 viewer.
- **Other OS users** on the same machine.
- **Agent output rendered in the viewer:** a transcript is attacker-reachable text (prompt injection upstream of it). If it could become HTML in the viewer, it could press the approve button.
- **The agent being approved** trying to approve itself through Semon's own surfaces.
- **Stale and replayed answers:** an answer meant for one request applied to another, or after the request changed.
- **From step 3, the embedding server and the network between it and the machine.**

Out of scope: arbitrary code already running as the same OS user. Such code can read the user's files and drive the harness directly, and no local design fixes that. Within that, Semon still does not make self-approval easy: the answer operation is not on `semon mcp` or on the control socket (below), the token is never written to disk by Semon and never put in a child process's environment, and the `?t=` token in the URL does not authorize a write.

## The driver interface

A proposal; names are for discussion and will settle in PR 1.

A **session** is Semon's existing session key (harness plus native id). The driver adds, per live session, what it can do and why not:

- `approve`: live (hook installed, daemon attached, port known), or none with a reason ("Claude hook not installed", "Codex started with --no-daemon", "OpenCode port unknown", "Codex 0.161 not tested; read-only").
- `question`: live or none, same shape.
- `message`, `interrupt`: none in step 1; step 2 fills them in.
- `start`: none in step 1.

Operations:

- `requests()`: the pending requests, as below.
- `answer(request_id, answer, payload_sha256)`: resolve one request. Returns the request's final state.
- Step 2 adds `send(session, text)`, `interrupt(session)` and `start(harness, cwd, prompt)`.

Adapters push events into the driver: a request appeared, a request was resolved elsewhere (in the terminal, by another client, by the harness's own timeout), a session ended.

### The pending-request model

One record per question or permission prompt the driver has seen:

| Field | What it holds |
|---|---|
| `id` | Semon's own id: 128 random bits, hex. Never the harness's id, which is short (Claude's channel ids are five letters) and guessable. |
| `session` | The Semon session key the request belongs to. |
| `harness`, `harness_ref` | The harness and its own request id (Codex JSON-RPC id, OpenCode request id, the Claude hook's connection), kept inside the driver. |
| `kind` | `permission` or `question`. |
| `payload` | What the human is shown, taken verbatim from the harness's request. For a permission: the tool, the full command (argv or the exact shell string), the working directory, file paths and the diff for a file change, and any network or permission scope asked for. The model's own description is a separate field shown second and labelled as the agent's words, never in place of the command (Claude's channel relay sends a sanitised summary; Semon doesn't use it). For a question: each question, its options, whether it is multi-select, and whether free text is allowed. |
| `payload_sha256` | The SHA-256 of the canonical payload. An answer must carry it, so the human approves exactly what they saw. |
| `created` | When the driver first saw it. |
| `expires` | The earliest of the harness's own deadline (Codex `autoResolutionMs`, the Claude hook's timeout) and Semon's. After it, Semon refuses answers and the harness's own fallback applies. |
| `state` | `open`, then exactly one final state: `answered` (by Semon, with the answer and which window), `resolved` (elsewhere: the terminal, another client, the harness), `expired`, `released` (the human chose "Answer in terminal"), or `gone` (the session or the adapter's connection ended). |

Rules:

- **First answer wins.** `answer` is a compare-and-set from `open` under the store's lock. Every later answer gets the final state back, not an error page.
- **Payload hash must match.** A mismatch is refused with the current payload.
- **Allow once, deny, or answer. Nothing broader.** Step 1 offers no "always", no "for this session", no rule or policy amendment (Codex `acceptForSession` and execpolicy amendments, OpenCode `always`, Claude `updatedPermissions`). Those change policy, and stay in the terminal. This also keeps step 1 identical to what step 3 allows a remote device.
- **Answers map per harness:** allow once is Claude `behavior: allow` with nothing else set, Codex `accept`, OpenCode `once`. Deny is Claude `behavior: deny` with a message, Codex `decline` (not `cancel`, which ends the turn), OpenCode `reject` with a message. The message says it was denied from Semon's viewer, plus the human's own text if they typed any.
- **Every answer is journaled** locally in the same format the step 3 audit log uses (below), from step 1 on.
- **Held in memory.** The store is rebuilt on restart from what the harnesses can list (below); the journal is a record, not a source of truth.
- **Bounded.** At most 256 open requests and 64 waiting Claude hook connections at a time; beyond that, a new Claude hook gets "no decision" at once (the terminal answers it) and the viewer says requests were dropped.

## Step 1 in detail

### Turning control on

Control is off unless the viewer is started with `semon sessions --serve --control`. Only one process per machine may hold control: it takes an exclusive lock on `$XDG_RUNTIME_DIR/semon/control.lock`. A second `--control` fails with a message naming the pid that holds it. Without `--control`, the server is exactly today's read-only viewer.

### The control socket (Claude hook to Semon)

The Claude hook needs to reach the local process without a new port. The controlling process binds a Unix socket at `$XDG_RUNTIME_DIR/semon/control.sock`, in a 0700 directory, and checks the peer's uid (`SO_PEERCRED`) on every connection. This is file-system IPC restricted to the OS user, like Claude's own session sockets, not a network listener.

The socket has one operation: a hook registers a request and waits for its outcome. It has no answer operation, no list operation and no way to reach the viewer's routes. Frames are newline-delimited JSON with a version field.

### The Claude hook

- **The hook command** is `semon hook claude-permission`. Claude Code runs it for `PermissionRequest` and passes the request on stdin: session id, tool name, the tool's full input, cwd.
- **It returns at once, with no decision,** when Semon isn't running with `--control`, when the socket refuses it, or when no viewer is watching. "Watching" means a viewer page with control polled `/api/requests` within the last 15 seconds. The terminal dialog then appears as if Semon weren't there.
- **Otherwise it waits** for the answer, for at most a few seconds less than the timeout the hook is installed with. It prints `hookSpecificOutput.decision.behavior` `allow` or `deny` (with a message) and exits.
- **It gives up to the terminal** (no decision) when the wait times out, when the last viewer stops watching, when the human clicks "Answer in terminal", or when the socket drops (a restart).
- **Installing it is opt-in and visible:** `semon control install claude` prints the exact entry it would add (`hooks.PermissionRequest`, matcher `*`, the command, an explicit `timeout`) and the settings file it would change, shows the diff, and writes only after the human confirms. `--print` only prints. `--scope user|project` picks the file. `semon control uninstall claude` removes that entry and nothing else. Semon never edits settings silently, never on `--control`, and never from the viewer.

Whether the terminal still shows its dialog, and can answer it, while the hook waits is not verified (S1). If it can't, the wait blocks the terminal, which is why the hook waits only while a viewer is watching and why "Answer in terminal" exists.

### Codex and OpenCode adapters

- **Codex:** with `--control`, Semon connects to the daemon's control socket, calls `initialize`, checks the version against the tested list, and subscribes to the loaded threads. An approval or input request becomes a pending request; `serverRequest/resolved` for it becomes `resolved`. A version outside the tested list keeps the connection read-only: requests are shown, with "This Codex version isn't tested; answer in Codex". Sessions run with `--no-daemon` show approvals as none, with that reason. Whether the daemon sends a server request to every attached client, and what happens to pending requests on reconnect, is S3.
- **OpenCode:** Semon connects to the TUI's server at a known port. The port comes from `semon opencode`, a thin wrapper that launches `opencode --port <free port>` and records the port and pid in `$XDG_RUNTIME_DIR/semon/opencode/<pid>.json`, or from the human running `opencode --port` and naming it once. Semon reads `/event` for `permission` and `question` events and lists `GET /permission` and `GET /question` on connect. It talks only to the server that owns the session and never starts a second `opencode serve` on the same store. How it authenticates to a TUI server started after the CVE fix is S4.

### Local write routes

The local `Viewer` wrapper in `viewer.rs` gains two routes, answered before it hands a request to `ViewerCore`. `ViewerCore` itself stays GET-only, so a server embedding it gets no write surface by construction; the routes exist only in the local process.

- `GET /api/requests`: the open requests and those resolved in the last 10 minutes, plus each live session's capabilities. It is an ETag'd JSON answer like `/api/model`, and each poll of it refreshes "a viewer is watching".
- `POST /api/requests/<id>/answer`: body `{"payload_sha256": "…", "decision": "allow" | "deny", "message": "…"}` for a permission, or `{"payload_sha256": "…", "answers": [[…]]}` for a question. It answers 200 with the final state, 409 with the final state when the request is no longer open (someone else answered, it expired, it was resolved elsewhere), 412 with the current payload when the hash doesn't match, and 404 for an unknown id.

Checks on every POST, in this order, each failing closed with 403 and no body beyond "Forbidden":

1. Control is on (`--control`).
2. Exactly one `Host` header, equal to `127.0.0.1:<port>` or `localhost:<port>` (as today). This defeats DNS rebinding.
3. An `Origin` header is present and equals `http://` plus that same `Host`. A missing `Origin` is refused. If `Sec-Fetch-Site` is present it must be `same-origin`.
4. The `semon_session` cookie equals the run's token. The `?t=` query token authorizes GET bootstrap only, never a write.
5. `Content-Type` is exactly `application/json` (so a cross-site form can't send it without a preflight, which is never answered), and the body is at most 64 KiB of valid JSON with no unknown fields.

No response ever carries an `Access-Control-*` header. `OPTIONS` gets 405. The CSP and the other `SECURITY_HEADERS` apply as today. The viewer's existing banned-string test (no `innerHTML` and the like in the served script) becomes a control-security property: payloads and transcripts are text nodes only.

### How the viewer shows a waiting request

Reuse the signals that exist; add no new vocabulary.

- **State.** A session with an open request reads `wait`, "Needs you", with the amber dot, exactly like an open question today. It counts in Home's "waiting on you", appears in Home's "Needs you" list, and lights the rail's attention dot.
- **Home's "Needs you" item** for a request says what is asked, in one line ("Codex wants to run `cargo test -p semon-push`", "Claude asks: Which branch?"), with the session and age, and a "Review" action.
- **On the session page** the request is a card at the transcript's foot, where the waiting question card sits today. When the question already appears in the transcript as a "to you" card (Codex `request_user_input`, Claude `AskUserQuestion`), that card gains the answer form instead of a second card appearing. When the card is scrolled away, the jump control says "Needs you" and scrolls to it.
- **The request card is chrome, not transcript.** It is drawn from `/api/requests` only, outside the transcript's column and style, with the harness icon and the words "Semon: waiting on you". Text in a transcript can look like anything, but it can't produce this card.
- **Resolved** requests stay on the card for 10 minutes, with the outcome and where it came from: "Allowed here at 14:02", "Answered in another window", "Answered in the terminal", "Expired: the terminal is asking now", "Released to the terminal".

The answer form:

- **Desktop (1280 px):** the card shows the tool, then the full command in a monospace block that wraps (no sideways scroll), the working directory, and for a file change the diff (long ones folded behind "Show all N lines", never cut silently). Below it, "The agent says" and the model's description, in muted text. Then "Deny" on the left and "Allow once" on the right, with room between them, and a message field that opens under "Deny". Neither button has focus on arrival, there is no keyboard shortcut for allow, and both buttons are disabled while a POST is in flight.
- **Phone (390 px):** "Review" opens a bottom sheet the height of the content, up to the screen: the same blocks, scrolling vertically, with the two buttons pinned at the bottom above the safe area, each at least 44 px tall and half the width. No swipe-to-approve.
- **Questions:** the options as radio buttons, or checkboxes for multi-select, and a text field when free text is allowed; one "Send answer" button.
- **For Claude,** an extra quiet action, "Answer in terminal", releases the hook at once.
- **Nothing is answerable when the capability is none;** the card then shows the request read-only with the reason and where to answer it.

### Timeout, restart, two viewers

- **Timeout.** When `expires` passes, the request becomes `expired` and further answers get 409. For Claude the hook has already returned no decision, so the terminal dialog is showing. For Codex, the harness's own `autoResolutionMs` behaviour applies. OpenCode has no deadline; Semon's expiry for it only closes the viewer's form, and the TUI keeps asking.
- **Restart of Semon.** The run's token changes, so open viewers must be reopened with the new URL (as today). Claude hooks lose their socket and return no decision: the terminal asks. Codex and OpenCode requests are rebuilt from the harness on reconnect (OpenCode lists them; Codex is S3). An answer POST in flight during a restart fails; the viewer shows "Semon restarted" and, once reopened, the true state.
- **Two viewers answering at once.** The first POST wins. The second gets 409 and the card shows "Answered in another window: Allowed at 14:02". The same applies when the terminal or another client answers first.
- **The session ends** while a request is open: the request becomes `gone`.

### The answer journal

Every answer and every refused answer is appended to `$XDG_STATE_HOME/semon/control/journal.jsonl` (0600, in a 0700 directory, opened for append only): time, source (`viewer`; from step 3 the device id), request id, session, harness, kind, the answer, the payload hash, and the outcome (applied, or refused and why). Each line carries the SHA-256 of the previous line, so an edit or a removed line is detectable. Step 3's audit log is this file.

## Step 1 PRs

One PR per point (R-20260929-10). PR 1 and PR 2 go first; PRs 3 to 6 can then run in parallel. Spikes S1 to S4 run before the adapter PR they inform.

| PR | What | Acceptance tests |
|---|---|---|
| 1 | The driver interface and the request store, with a fake adapter. | Unit: first answer wins under concurrent answers from several threads; a wrong payload hash is refused with the current payload; answers after `expires` are refused; every final state is final; `gone` on session end; the caps (256 open, 64 hook waits) hold, with the overflow reported; the journal gets one line per answer and per refusal, and its hash chain detects an edited and a removed line. |
| 2 | `--control`, the single-holder lock, and the two routes on the local `Viewer`. | HTTP, in the style of today's viewer tests: POST refused without `--control`; refused for a wrong or duplicated `Host`, a missing or foreign `Origin`, `Sec-Fetch-Site: cross-site`, the `?t=` token without the cookie, a wrong cookie, a `text/plain` or form body, an oversized body, unknown fields; `OPTIONS` is 405; no response carries `Access-Control-*`; 200 then 409 for two answers; 412 on a stale hash; `ViewerCore::respond` still returns 405 for POST (embedders get nothing); a second `--control` fails naming the holder. |
| 3 | The viewer UI: state, Home, the rail dot, the request card, the phone sheet, the resolved states. | UI checks at 390 and 1280 px, light and dark: a fixture request shows on Home and on the session page; allow and deny send the right body; the second window shows "Answered in another window"; no sideways scroll at 390 with a 2,000-character command; a command containing `<img src=x onerror=…>` renders as text; the banned-string test still passes; no button is focused on arrival. |
| 4 | Claude: the control socket, `semon hook claude-permission`, and `semon control install` / `uninstall claude`. | The hook returns no decision within 100 ms when Semon is down, when control is off and when no viewer is watching; it prints exactly the allow and the deny output; it returns no decision on timeout, on release and when the socket drops; a peer with another uid is refused; install writes nothing without confirmation, prints the diff, is idempotent, and uninstall removes only its own entry. |
| 5 | Codex: the daemon adapter and the version gate. | Against a fake daemon replaying frames recorded from the pinned Codex version: an approval and a user-input request become pending requests; allow once sends `accept` and deny sends `decline`, never `acceptForSession` or `cancel`; `serverRequest/resolved` makes the request `resolved`; an untested version is read-only with its reason; a dropped socket makes open requests `gone`. |
| 6 | OpenCode: the `semon opencode` wrapper, the port registry and the adapter. | Against a fake server: permission and question events become pending requests; allow once sends `once`, deny sends `reject` with the message, never `always`; pending requests are rebuilt from `GET /permission` and `GET /question` after a reconnect; an unknown port shows approvals as none with its reason. |

Follow-ups, not in step 1: `waiting-permission` in `semon query` (the driver is the exact signal it was waiting for), and exact permission-wait spans in "Waited on you" from the journal.

## Steps 2 to 4, in outline

- **Step 2.** `send` maps to Codex `turn/start`, or `turn/steer` with `expectedTurnId` while a turn runs, and to OpenCode `prompt_async`. `interrupt` maps to Codex `turn/interrupt` and OpenCode `abort`. The message box sits under the transcript, and a sent message shows as the user's own turn once the log carries it. Sessions Semon starts run the user's installed binary as the user: `claude -p` with stream-json (approvals and questions through the permission callback, interrupt, messages), `thread/start` on the Codex daemon, `POST /session` on an OpenCode server Semon launched. Claude terminal sessions stay approvals-only.
- **Step 3.** The machine keeps one outbound connection to the embedding server, authenticated by the machine's existing push token for transport, and receives signed commands down it. Pairing: `semon control pair` shows a QR code (and a code to type) holding the machine's public key and a one-time 128-bit secret that expires in five minutes. The device makes its own key pair, proves it knows the secret, and the machine shows the device's key fingerprint as words, which the human compares with the phone's before confirming at the machine. The machine signs each request it announces; the device checks that signature and hashes the payload it displays, so a server that alters the text shown gets an answer the machine refuses. Each command carries the device id, request id, session, answer, payload hash, expiry and a per-device counter that must increase. The machine checks the device is paired and not revoked, the signature, that the request is open and in that session, the hash, the expiry and the counter, then journals the outcome. `semon control devices` lists and revokes. Remote control is off until `semon control remote on` at the machine.
- **Step 4.** Messages into live Claude terminal sessions, through channels once they are GA and need no development flag, or another documented entry point.

## Open questions and the spike that answers each

| # | Question | Spike |
|---|---|---|
| S1 | While a Claude `PermissionRequest` hook waits, does the terminal still show its dialog, and if the human answers there first, is the hook killed and the answer kept? | A hook that sleeps 60 s on a throwaway session; watch the terminal, answer there, check the hook's exit and the tool result. Record the Claude Code version. |
| S2 | Can Semon answer `AskUserQuestion` in a live Claude terminal session? Candidates: a `PreToolUse` hook returning `updatedInput` with the answers, or `PermissionRequest` firing for that tool. | Try both on a throwaway session; check what the model receives and whether the terminal still asks. |
| S3 | With the Codex TUI and Semon both attached to the daemon, which client gets `requestApproval` and `requestUserInput`: all subscribers or one? Does the other get `serverRequest/resolved`? Are pending requests re-sent after a reconnect? | Attach a small read-only client to the daemon beside a TUI at the pinned version, trigger an approval, answer in the TUI, then reconnect mid-request. |
| S4 | How does a client authenticate to an OpenCode TUI's server since the CVE-2026-22812 fix (per-process token, `OPENCODE_SERVER_PASSWORD`, or none on loopback), and can Semon learn it without reading OpenCode's files? | Start the TUI with `--port`, inspect what a request without and with credentials gets, and read the fix's change. |
| S5 | Are OpenCode's question routes, which are in source but not in the docs, stable enough to depend on? | Diff the routes across the last several releases; if they moved, gate questions on the version like Codex. |
| Q6 | The Claude hook's installed timeout and the 15 s "watching" window. | Settle in PR 4 from S1's result. |
| Q7 | How much of a very large payload (a big diff) to show before "Show all", and whether a payload over a size limit can be answered at all. | Settle in PR 3 with real Codex file-change requests. |
| Q8 | Step 3: should payloads also be encrypted end to end so the embedding server can't read them, not only signed so it can't forge them? Paseo does this; Omnara chose not to. | Decide with step 3's own doc. |
| Q9 | Step 3: may a paired device start a session remotely, or only answer, message and interrupt? It runs the user's own binary under the user's login, but it moves closer to a hosted product driving a subscription login. | Decide with step 3's own doc, after rereading both vendors' terms at the time. |

## Risks

- **Codex's control surface is experimental**, and the daemon auto-attach is documented only in source. The version gate limits the damage to "read-only until tested", but every Codex release may need a test run and a new entry in the tested list.
- **The Claude hook can hold up the terminal** if S1 finds the dialog hidden while it waits. The watching rule and "Answer in terminal" bound this, and the hook is opt-in.
- **OpenCode needs the port.** Sessions started without the wrapper or `--port` stay read-only.
- **The viewer becomes security-critical.** Any HTML injection in the viewer would now be an approval. The CSP without `unsafe-inline` and the banned-string test are what hold that line; both stay, and the request card never renders a string as HTML.
- **Step 3 makes shell-capable agents reachable from outside the machine.** That is why it is a reserved action, with its own doc and Marvin's explicit go.
