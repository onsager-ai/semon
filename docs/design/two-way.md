# Two-way control: answering and steering agents from Semon

Status: design only. Marvin chose "Design doc, then step 1 (Recommended)" and, for how far a machine trusts the embedding server, "Paired-device signatures (Recommended)" (Semon session, 2026-09-30T01:00:34Z). After the doc's first review he chose "Refuse agent-sent answers (Recommended)" and, for where remote answers may be signed, "Native app only (Recommended)" (Semon session, 2026-09-30T01:23:07Z). Nothing here is implemented. Each step below is its own PR series and needs its own go; step 1 starts after Marvin approves this doc.

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
| Claude Code | A `PermissionRequest` hook that long-polls the local Semon process for an answer. On timeout, or when Semon isn't there, the hook returns no decision and the terminal dialog is the fallback. | Approvals only. | Hooks are GA. |

Claude Code specifics, decided:

- **Answering an `AskUserQuestion` in a live terminal session is unverified.** It is an open question with a spike (S2 below). Until the spike says yes, the viewer shows the question and says to answer it in the terminal.
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

**Agent-sent answers are refused** (Marvin, 01:23:07Z). Every answer that reaches the viewer is traced to the process that sent it, and it is refused when that process descends from a harness process. [Who sent this answer](#who-sent-this-answer) says how, and what the check cannot stop.

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

## Threat model

In scope:

- **Web pages in the user's browser:** cross-site requests (CSRF), DNS rebinding, and CORS misconfiguration against the 127.0.0.1 viewer. A page on another 127.0.0.1 port is same-site but not same-origin.
- **Other OS users** on the same machine. Loopback is shared by every user, and cookies are not scoped by port: the browser sends the viewer's cookie to any `127.0.0.1:<port>`. Another user who binds a loopback port and gets the browser to visit it (a link, an image) receives the cookie. So the cookie alone can't authorize a write, and the server checks who is on the other end of the TCP connection.
- **Agent output rendered in the viewer:** a transcript is attacker-reachable text (prompt injection upstream of it). If it could become HTML in the viewer, it could press the approve button.
- **The agent being approved** trying to approve itself through the viewer, including a sandboxed command that is allowed to reach loopback (a Codex sandbox with network on, for example). Marvin decided these answers are refused.
- **Misleading requests:** a request whose shown text differs from what will run (a summary instead of the command, hidden control, bidirectional or zero-width characters), or a fake request registered by some process to phish an approval.
- **Stale and replayed answers:** an answer meant for one request applied to another, or after the request changed.
- **From step 3, the embedding server and the network between it and the machine.**

Out of scope: code running as the same OS user outside every harness's process tree, and an agent that drives the human's own input rather than the viewer. An unsandboxed agent runs commands as the user, so it can escape the ancestry check by other means: `tmux send-keys` into the terminal that shows the dialog, the browser's automation or debugging port, or starting a process that is re-parented out of its tree (a double fork, `systemd-run --user`). The ancestry check closes the plain case and the sandbox-escape case, where a sandboxed command can reach loopback but can't leave the tree (S7 checks this for Codex's Linux sandbox). It is not a sandbox. Within that, Semon still makes self-approval harder: the answer operation is not on `semon mcp` or on the control socket, the cookie secret is never written to disk by Semon or put in a child process's environment, and the URL token is single-use.

## The driver interface

A proposal; names are for discussion and will settle in PR 1.

A **session** is Semon's existing session key (harness plus native id). The driver adds, per live session, what it can do and why not:

- `approve`: live (hook installed, daemon attached, port known), or none with a reason ("Claude hook not installed", "Codex started with --no-daemon", "OpenCode port unknown", "Codex 0.161 not tested; read-only").
- `question`: live or none, same shape.
- `message`, `interrupt`: none in step 1; step 2 fills them in.
- `start`: none in step 1.

Operations:

- `requests()`: the pending requests, as below.
- `answer(request_id, answer, payload_sha256)`: claim one request, deliver the answer to the harness, and return the final state the harness reports.
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
| `payload` | What the human is shown, taken verbatim from the harness. For a permission: the tool, the full command (argv or the exact shell string), the working directory, file paths and the diff for a file change, and any network or permission scope asked for. The model's own description is a separate field, shown second and labelled as the agent's words, never in place of the command. For a question: each question, its options, whether it is multi-select, and whether free text is allowed. |
| `answerable` | Yes, or no with a reason. See [Verbatim or read-only](#verbatim-or-read-only). |
| `payload_sha256` | The SHA-256 of the payload in its canonical encoding, RFC 8785 (JSON Canonicalization Scheme). The encoding is pinned in PR 1 because step 3 devices recompute the hash from what they display. An answer must carry it, so the human approves exactly what they saw. |
| `created` | When the driver first saw it. |
| `expires` | The earliest of the harness's own deadline (Codex `autoResolutionMs`, the Claude hook's wait budget) and Semon's. After it, Semon refuses answers and the harness's own prompt applies. |
| `state` | `open`, then `claimed` while an answer is being delivered, then exactly one final state (below). |

Final states:

- `answered`: Semon delivered the answer and the harness accepted it. Records the answer and which window sent it.
- `resolved`: answered elsewhere (the terminal, another client), as the harness reported it.
- `rejected`: Semon delivered an answer and the harness refused it, for example a late reply to a prompt it had already closed.
- `left`: handed back to the harness's own prompt, with the reason: timed out, released by the human ("Answer in terminal"), no viewer watching, or Semon's connection to the hook dropped (a restart). For Claude this means the terminal is asking now. It never means the terminal answered: Semon doesn't know that until the log shows it.
- `gone`: the session or the adapter's connection to the harness ended.

Rules:

- **Claim, deliver, then the harness decides.** `answer` checks the state first: a request that is not `open` answers 409 with its current state, and this takes precedence over a hash mismatch. Then the hash: a mismatch answers 412 with the current payload. Then a compare-and-set from `open` to `claimed` under the store's lock, so the first answer wins. The adapter delivers it, and the final state comes from the harness's reply, not from Semon's hopes. Other windows show a claimed request as "Being answered in another window".
- **Allow once, deny, or answer. Nothing broader.** Step 1 offers no "always", no "for this session", no rule or policy amendment (Codex `acceptForSession` and execpolicy amendments, OpenCode `always`, Claude `updatedPermissions`). Those change policy, and stay in the terminal. This also keeps step 1 identical to what step 3 allows a remote device.
- **Answers map per harness:** allow once is Claude `behavior: allow` with nothing else set, Codex `accept`, OpenCode `once`. Deny is Claude `behavior: deny` with a message, Codex `decline` (not `cancel`, which ends the turn), OpenCode `reject` with a message. The message says it was denied from Semon's viewer, plus the human's own text if they typed any.
- **Every answer is journaled** locally ([The answer journal](#the-answer-journal)).
- **Held in memory.** The store is rebuilt on restart from what the harnesses can list (below); the journal is a record, not a source of truth.
- **Bounded.** At most 256 open requests and 64 waiting Claude hook connections at a time. Beyond that, a new Claude hook gets no decision at once (the terminal asks) and the viewer says requests were dropped.

### Verbatim or read-only

A request is answerable only when Semon holds the verbatim command, diff and working directory the harness will act on. Otherwise it is shown read-only, with the reason and where to answer it. Never a summary in place of the thing itself.

- **Claude:** the hook receives the tool's full input on stdin (the command for Bash, the old and new text or content for Edit and Write) and the cwd. Answerable.
- **Codex (step 1b):** a command approval may carry the command, or only an item id that has to be joined with the `item/started` notification. A file-change approval may carry only an item id, with the diff elsewhere. Unverified; part of S3. Each approval kind becomes answerable only once S3 shows where its verbatim content comes from.
- **OpenCode (step 1b):** a permission carries patterns and metadata. Whether that is the verbatim command is unverified; part of S4.

## Step 1 in detail

### Turning control on

Control is off unless the viewer is started with `semon sessions --serve --control`. Only one process per machine may hold control: it takes an exclusive lock on `$XDG_RUNTIME_DIR/semon/control.lock`. A second `--control` fails with a message naming the pid that holds it. If `XDG_RUNTIME_DIR` is not set, `--control` refuses to start; there is no fallback to a shared path. Without `--control`, the server is exactly today's read-only viewer, including today's reusable `?t=`.

### The token and the cookie

With `--control`, the URL token and the cookie are two different secrets:

- The printed URL's `?t=` is **single-use**. The first GET that presents it gets a `semon_control` cookie (`HttpOnly; SameSite=Strict; Path=/`) holding a separate 128-bit secret, and the URL token is burned. A second use answers 403. A URL left in shell history, terminal scrollback or a browser's history is dead after the first load.
- **Another browser or window** gets a fresh single-use URL from `semon control url`, which asks the controlling process over the control socket. The same peer checks apply as for a POST: the same uid, and not from a harness's process tree.
- **The cookie never authorizes on its own.** Since cookies are not scoped by port, the server also checks the TCP peer on every request while control is on (below).

### Who sent this answer

On Linux the server resolves the other end of each loopback TCP connection. The accepted connection gives the client's port. The client's socket, `127.0.0.1:<client port>` connected to the viewer's port, is looked up through `sock_diag` (or `/proc/net/tcp` as the fallback), which gives its owner uid and its inode. The inode is found among `/proc/<pid>/fd` to get the process.

- **Uid.** A request whose peer uid is not the controlling process's uid is refused (403). This is what stops another OS user holding a stolen cookie.
- **Ancestry.** For every POST, and for `semon control url`, the peer process's ancestors are walked through `/proc/<pid>/stat`. The request is refused when any ancestor is a harness process: a pid a Claude pid file names, a Codex process (the daemon, or a writer-lock holder), an OpenCode process Semon knows, or a process whose executable is one of the harness binaries. This is the #20 ancestry method. The refusal is journaled and the viewer says why ("This answer came from inside an agent's session"). A browser that an agent launched as its own child is refused too, which is correct: it is driven by the agent.
- **Fail closed.** If the uid or the process can't be resolved (a browser in a sandbox that hides its process, for example), the answer is refused with that reason. S6 checks the common sandboxed browsers.

### The control socket

The Claude hook has to reach the local process without a new port. The controlling process binds a Unix socket at `$XDG_RUNTIME_DIR/semon/control.sock`, in a 0700 directory it creates and then checks with `lstat` (a directory, not a symlink, owned by the user, mode 0700). It unlinks a stale socket only while it holds the lock, so a second process can never remove a live one. Each connection's peer is read with `SO_PEERCRED` and must have the same uid.

The socket has two operations, and no others: a hook registers a request and waits for its outcome, and `semon control url` asks for a single-use URL. There is no answer operation and no list operation. Frames are newline-delimited JSON with a version field.

**A registration must come from the session it names.** The hook's input names a Claude session id. Semon finds the Claude pid whose pid file names that session id and checks that the registering peer's pid descends from it. Anything else is refused: a process that isn't inside that Claude session can't put a card in front of the human (a phishing approval) or fill the request slots.

### The Claude hook

- **The hook command** is `semon hook claude-permission --wait <seconds>`. The wait budget is an argument, so it always sits a few seconds under the `timeout` the hook is installed with. Claude Code runs the hook for `PermissionRequest` and passes the request on stdin: session id, tool name, the tool's full input, cwd.
- **It returns at once, with no decision,** when Semon isn't running with `--control`, when the socket refuses it, or when no viewer is watching. The terminal dialog then appears as if Semon weren't there.
- **Otherwise it waits,** by the rules below. It prints `hookSpecificOutput` with `hookEventName: "PermissionRequest"` and `decision.behavior` `allow` or `deny` (with a message) and exits.
- **It gives up to the terminal** (no decision, state `left`) when its wait ends, when the last viewer stops watching, when the human clicks "Answer in terminal", or when the socket drops.
- **Installing it is opt-in and visible:** `semon control install claude` prints the exact entry it would add (`hooks.PermissionRequest`, matcher `*`, the command with its `--wait`, an explicit `timeout`) and the settings file it would change, shows the diff, and writes only after the human confirms. `--print` only prints. `--scope user|project` picks the file. `semon control uninstall claude` removes that entry and nothing else. Semon never edits settings silently, never on `--control`, and never from the viewer.

**Watching.** A viewer counts as watching only when a page reports that it is visible and focused. The page's poll of `/api/requests` carries `watching=1` only when the Page Visibility API says visible and the document has focus. The server counts it only when the poll carries `Sec-Fetch-Site: same-origin`, so a page on another port can't keep the hook waiting by loading the URL as an image. Watching lapses 15 seconds after the last such poll.

**The wait, for both outcomes of S1.** S1 finds out whether the terminal still shows its dialog while the hook waits. Both answers are designed now:

| | A: the terminal shows its dialog while the hook waits, and an answer there ends the hook | B: the terminal shows nothing until the hook returns |
|---|---|---|
| Initial wait | The full budget, while a viewer is watching. The terminal and the viewer race; the first answer wins. | A short initial wait, 10 seconds, while a viewer is watching. The human sees the card appear. |
| Extended | Not needed. | To the full budget when the human opens the card (a `POST /api/requests/<id>/open`, with every POST check). |
| Released early | When watching lapses. | When watching lapses, the page is hidden or loses focus, or the human closes the card without answering. |
| The card says | Nothing extra. | "The terminal is waiting for this. Answer here or release it." |

### Codex and OpenCode adapters (step 1b)

Both wait for their spikes. Each is its own PR.

- **Codex ships only if S3 shows** that the daemon sends each server request to every attached client and sends `serverRequest/resolved` to the others. If only one client receives it, attaching could take the prompt away from the TUI, and the adapter doesn't ship. When it ships: with `--control`, Semon connects to the daemon's control socket, calls `initialize`, checks the version against the tested list, and subscribes to the loaded threads. An approval or input request becomes a pending request, answerable only when its verbatim content is in hand. `serverRequest/resolved` makes it `resolved`. A version outside the tested list keeps the connection read-only: requests are shown, with "This Codex version isn't tested; answer in Codex". Sessions run with `--no-daemon` show approvals as none, with that reason.
- **OpenCode ships after S4 and S5.** Semon connects to the TUI's server at a known port. The port comes from `semon opencode`, a thin wrapper that launches `opencode --port <free port>` and records the port and pid in `$XDG_RUNTIME_DIR/semon/opencode/<pid>.json`, or from the human running `opencode --port` and naming it once. Semon reads `/event` for `permission` and `question` events and lists `GET /permission` and `GET /question` on connect. It talks only to the server that owns the session and never starts a second `opencode serve` on the same store.

### Local write routes

The local `Viewer` wrapper in `viewer.rs` gains three routes, answered before it hands a request to `ViewerCore`. `ViewerCore` itself stays GET-only, so a server embedding it gets no write surface by construction; the routes exist only in the local process.

- `GET /api/requests`: the open and claimed requests and those resolved in the last 10 minutes, plus each live session's capabilities. It is an ETag'd JSON answer like `/api/model`. With `watching=1` and same-origin fetch metadata, it also counts as watching.
- `POST /api/requests/<id>/answer`: body `{"payload_sha256": "…", "decision": "allow" | "deny", "message": "…"}` for a permission, or `{"payload_sha256": "…", "answers": [[…]]}` for a question. It answers 200 with the final state, 409 with the current state when the request is not `open` (someone else answered or is answering, it was left or resolved elsewhere), 412 with the current payload when the hash doesn't match (409 is checked first), and 404 for an unknown id. When delivery takes more than 5 seconds it answers 202 with `claimed`, and the page learns the final state from its poll.
- `POST /api/requests/<id>/open`: the human opened the card (S1 outcome B). No body.

Checks on every request while control is on, in this order, each failing closed with 403 and no body beyond "Forbidden":

1. Exactly one `Host` header, equal to `127.0.0.1:<port>` or `localhost:<port>` (as today). This defeats DNS rebinding.
2. The TCP peer's uid is the controlling process's uid ([Who sent this answer](#who-sent-this-answer)).
3. The `semon_control` cookie holds the cookie secret, or this is the one GET that spends the single-use `?t=`. The URL token never authorizes a write.

And on every POST, in addition:

4. Control is on (`--control`); otherwise 405 as today.
5. An `Origin` header is present and equals `http://` plus that same `Host`, and `Sec-Fetch-Site`, when present, is `same-origin`. A missing `Origin` is refused.
6. The peer process does not descend from a harness process.
7. `Content-Type` is exactly `application/json` (so a cross-site form can't send it without a preflight, which is never answered), and the body is at most 64 KiB of valid JSON with no unknown fields.

No response ever carries an `Access-Control-*` header. `OPTIONS` gets 405. The CSP and the other `SECURITY_HEADERS` apply as today. The viewer's existing banned-string test (no `innerHTML` and the like in the served script) becomes a control-security property: payloads and transcripts are text nodes only.

### How the viewer shows a waiting request

Reuse the signals that exist; add no new vocabulary.

- **State.** A session with an open request reads `wait`, "Needs you", with the amber dot, exactly like an open question today. It counts in Home's "waiting on you", appears in Home's "Needs you" list, and lights the rail's attention dot.
- **Home's "Needs you" item** for a request says what is asked, in one line ("Claude wants to run `cargo test -p semon-push`", "Claude asks: Which branch?"), with the session and age, and a "Review" action.
- **On the session page** the request is a card at the transcript's foot, where the waiting question card sits today. When the question already appears in the transcript as a "to you" card, that card gains the answer form instead of a second card appearing. When the card is scrolled away, the jump control says "Needs you" and scrolls to it.
- **The request card is chrome, not transcript.** It is drawn from `/api/requests` only, outside the transcript's column and style, with the harness icon and the words "Semon: waiting on you". Text in a transcript can look like anything, but it can't produce this card.
- **Final states** stay on the card for 10 minutes, with the outcome and where it came from: "Allowed here at 14:02", "Being answered in another window", "Answered in another window", "Answered in the terminal" (only when the harness or the log says so), "Left to the terminal: timed out", "Left to the terminal: Semon restarted", "Released to the terminal", "Claude refused the answer: the prompt had closed".
- **Read-only requests** show the same card without the buttons, with the reason and where to answer.

The answer form:

- **Exact text.** The command is shown whole, in a monospace block that wraps (no sideways scroll). A command is never folded or cut, however long. Only a diff folds, behind "Show all N lines". Control characters (C0, C1, ANSI escapes), bidirectional controls (U+202A to U+202E, U+2066 to U+2069) and zero-width characters (U+200B to U+200F, U+2060, U+FEFF) are drawn visibly, as a marked code such as `⟨U+202E⟩`, and the card says the command contains them. Newlines and tabs show as themselves.
- **Desktop (1280 px):** the card shows the tool, the command, the working directory, and for a file change the diff. Below it, "The agent says" and the model's description, in muted text. Then "Deny" on the left and "Allow once" on the right, with room between them, and a message field that opens under "Deny". Neither button has focus on arrival, there is no keyboard shortcut for allow, and both buttons are disabled while a POST is in flight.
- **Phone (390 px):** "Review" opens a bottom sheet the height of the content, up to the screen: the same blocks, scrolling vertically, with the two buttons pinned at the bottom above the safe area, each at least 44 px tall and half the width. No swipe-to-approve.
- **Questions:** the options as radio buttons, or checkboxes for multi-select, and a text field when free text is allowed; one "Send answer" button.
- **For Claude,** an extra quiet action, "Answer in terminal", releases the hook at once.

### Timeout, restart, two viewers

- **Timeout.** When `expires` passes, the request becomes `left` and further answers get 409. For Claude the hook has already returned no decision, so the terminal is asking. For Codex, the harness's own `autoResolutionMs` behaviour applies. OpenCode has no deadline; Semon's expiry for it only closes the viewer's form, and the TUI keeps asking.
- **Restart of Semon.** The run's secrets change, so open viewers must be reopened with a new URL. Claude hooks lose their socket and return no decision: their requests were `left`, and the terminal is asking. Codex and OpenCode requests are rebuilt from the harness on reconnect (OpenCode lists them; Codex is S3). An answer POST in flight during a restart fails; the viewer shows "Semon restarted" and, once reopened, the true state.
- **Two viewers answering at once.** The first POST claims the request. The second gets 409 with `claimed` or the final state, and its card shows "Being answered in another window", then the outcome. The same applies when the terminal or another client answers first.
- **The session ends** while a request is open: the request becomes `gone`.

### The answer journal

Every answer, every refused answer and every refused registration is appended to `$XDG_STATE_HOME/semon/control/journal.jsonl` (0600, in a 0700 directory, opened for append only): time, source (the window, the peer pid and uid; from step 3 the device id), request id, session, harness, kind, the answer, the payload hash, and the outcome (the final state, or refused and why). In step 1 it is a plain append-only record. It is not tamper-evident: the user, and code running as the user, can edit it. Step 3 adds a hash chain with an anchor outside the machine's writable state, and only then claims tamper evidence.

## Step 1 PRs

One PR per point (R-20260929-10). PR 1 and PR 2 go first; PR 3 and PR 4 can then run in parallel. Users see nothing until PR 4 lands. Spike S1 runs before PR 4, S6 and S7 before PR 2 merges.

| PR | What | Acceptance tests |
|---|---|---|
| 1 | The driver interface and the request store, with a fake adapter. | Unit: the RFC 8785 test vectors give the pinned encoding, and a payload's hash is the same across key orders and number spellings; first claim wins under concurrent answers from several threads; 409 comes before 412 when both apply; a wrong hash on an open request is 412 with the current payload; the harness's reply decides the final state (accepted, refused, resolved elsewhere); answers after `expires` are refused; every final state is final; `gone` on session end; the caps (256 open, 64 hook waits) hold, with the overflow reported; the journal gets one line per answer and per refusal. |
| 2 | `--control` (Linux only, refuses without `XDG_RUNTIME_DIR`), the single-holder lock, the single-use URL and the cookie secret, the peer uid and ancestry checks, and the three routes on the local `Viewer`. | HTTP, in the style of today's viewer tests. The URL token works once, then 403, and the cookie's value is not the URL token. Refused: a wrong or duplicated `Host`; a missing or foreign `Origin`; `Sec-Fetch-Site: cross-site` or `same-site`; the `?t=` token on a POST; a wrong cookie; a `text/plain` or form body; an oversized body; unknown fields. A peer with another uid is refused (through an injected resolver; a real-socket test checks that the resolver finds the test's own uid and pid). A POST from a process whose ancestor is a registered harness pid is refused and journaled, and one from a sibling process is accepted. A peer that can't be resolved is refused. `OPTIONS` is 405; no response carries `Access-Control-*`. 200 then 409 for two answers; 412 on a stale hash; 409 before 412. `watching=1` counts only with `Sec-Fetch-Site: same-origin`. `ViewerCore::respond` still returns 405 for POST. A second `--control` fails naming the holder. Without `--control`, today's tests pass unchanged. |
| 3 | The viewer UI: state, Home, the rail dot, the request card, the phone sheet, the final and read-only states, the watching report. | UI checks at 390 and 1280 px, light and dark: a fixture request shows on Home and on the session page; allow and deny send the right body; the second window shows "Being answered in another window"; a 2,000-character command is shown whole with no sideways scroll at 390; a command containing `<img src=x onerror=…>` renders as text; U+202E, U+200B and an ANSI escape in a command render as visible marks; a diff folds and a command never does; the banned-string test still passes; no button is focused on arrival; a hidden or unfocused page stops sending `watching=1`. |
| 4 | Claude: the control socket, `semon hook claude-permission`, and `semon control install` / `uninstall claude`. | The hook returns no decision within 100 ms when Semon is down, when control is off and when no viewer is watching; it prints exactly the allow and the deny output, each with `hookEventName`; it honours `--wait`; it returns no decision on timeout, on release and when the socket drops, and the request becomes `left` with that reason. A registration from a process that doesn't descend from the Claude pid named for its session id is refused. A peer with another uid is refused. A second instance never unlinks the live socket; a stale one is replaced only under the lock. Install writes nothing without confirmation, prints the diff, is idempotent, and uninstall removes only its own entry. The S1 outcome that was found is the one tested for the wait. |

Step 1b, each after its spikes and its own go:

| PR | What | Acceptance tests |
|---|---|---|
| 5 | Codex: the daemon adapter and the version gate. Ships only if S3 shows server requests reach every attached client. | Against a fake daemon replaying frames recorded from the pinned Codex version: an approval and a user-input request become pending requests; each approval kind is answerable only with its verbatim content, joined from `item/started` where S3 says so; allow once sends `accept` and deny sends `decline`, never `acceptForSession` or `cancel`; `serverRequest/resolved` makes the request `resolved`; an untested version is read-only with its reason; a dropped socket makes open requests `gone`. |
| 6 | OpenCode: the `semon opencode` wrapper, the port registry and the adapter. | Against a fake server: permission and question events become pending requests, read-only unless S4 found the verbatim command; allow once sends `once`, deny sends `reject` with the message, never `always`; pending requests are rebuilt from `GET /permission` and `GET /question` after a reconnect; an unknown port shows approvals as none with its reason. |

Follow-ups, not in step 1: `waiting-permission` in `semon query` (the driver is the exact signal it was waiting for), and exact permission-wait spans in "Waited on you" from the journal.

## Steps 2 to 4, in outline

- **Step 2.** `send` maps to Codex `turn/start`, or `turn/steer` with `expectedTurnId` while a turn runs, and to OpenCode `prompt_async`. `interrupt` maps to Codex `turn/interrupt` and OpenCode `abort`. The message box sits under the transcript, and a sent message shows as the user's own turn once the log carries it. Sends and interrupts get the same peer checks as answers. Sessions Semon starts run the user's installed binary as the user: `claude -p` with stream-json (approvals and questions through the permission callback, interrupt, messages), `thread/start` on the Codex daemon, `POST /session` on an OpenCode server Semon launched. Claude terminal sessions stay approvals-only.
- **Step 3.** The machine keeps one outbound connection to the embedding server, authenticated by the machine's existing push token for transport, and receives signed commands down it. Only the native app signs. The web console the embedding server serves shows requests and says to answer them in the app or at the machine. Pairing: `semon control pair` shows a QR code (and a code to type) holding the machine's public key and a one-time 128-bit secret that expires in five minutes. The app makes its own key pair and proves it knows the secret, and the machine shows the app's key fingerprint as words, which the human compares with the phone's before confirming at the machine. The machine signs each request it announces; the app checks that signature and hashes, in the pinned encoding, the payload it displays, so a server that alters the text shown gets an answer the machine refuses. Each command carries the device id, request id, session, answer, payload hash, expiry and a per-device counter that must increase. The machine checks the device is paired and not revoked, the signature, that the request is open and in that session, the hash, the expiry and the counter, then journals the outcome. The journal gains its hash chain and an external anchor here. `semon control devices` lists and revokes. Remote control is off until `semon control remote on` at the machine.
- **Step 4.** Messages into live Claude terminal sessions, through channels once they are GA and need no development flag, or another documented entry point.

## Open questions and the spike that answers each

| # | Question | Spike |
|---|---|---|
| S1 | While a Claude `PermissionRequest` hook waits, does the terminal still show its dialog, and if the human answers there first, is the hook killed and the answer kept? Decides between outcomes A and B above. | A hook that sleeps 60 s on a throwaway session; watch the terminal, answer there, check the hook's exit and the tool result. Record the Claude Code version. |
| S2 | Can Semon answer `AskUserQuestion` in a live Claude terminal session? Candidates: a `PreToolUse` hook returning `updatedInput` with the answers, or `PermissionRequest` firing for that tool. | Try both on a throwaway session; check what the model receives and whether the terminal still asks. |
| S3 | With the Codex TUI and Semon both attached to the daemon, does every client get `requestApproval` and `requestUserInput`, and do the others get `serverRequest/resolved`? Are pending requests re-sent after a reconnect? Does each approval kind carry its verbatim command or diff, or only an item id to join with `item/started`? | Attach a small read-only client to the daemon beside a TUI at the pinned version, trigger a command and a file-change approval, record the frames, answer in the TUI, then reconnect mid-request. |
| S4 | How does a client authenticate to an OpenCode TUI's server since the CVE-2026-22812 fix (per-process token, `OPENCODE_SERVER_PASSWORD`, or none on loopback), and can Semon learn it without reading OpenCode's files? Is a permission's patterns and metadata the verbatim command? | Start the TUI with `--port`, inspect what a request without and with credentials gets, trigger a shell permission and compare its payload with the command run, and read the fix's change. |
| S5 | Are OpenCode's question routes, which are in source but not in the docs, stable enough to depend on? | Diff the routes across the last several releases; if they moved, gate questions on the version like Codex. |
| S6 | Does the peer lookup (socket to uid, inode to pid) work for the browsers people use on Linux, including snap and Flatpak builds? | Open the viewer from each and log what the resolver finds. A browser it can't resolve is refused; if that is common, the doc comes back. |
| S7 | Can a command inside Codex's Linux sandbox, with loopback allowed, leave the Codex process tree (a double fork, `setsid`, `systemd-run --user`)? If it can, the ancestry check doesn't close the sandbox case, and a second marker is needed. | Run each escape as a sandboxed command and check its parent chain from outside. |
| Q6 | The Claude hook's budget, the 10-second initial wait and the 15-second watching window. | Settle in PR 4 from S1's result. |
| Q7 | How much of a very large diff to show before "Show all", and whether a payload over a size limit can be answered at all. | Settle in PR 3 with real file-change requests. |
| Q8 | Step 3: should payloads also be encrypted end to end so the embedding server can't read them, not only signed so it can't forge them? Paseo does this; Omnara chose not to. | Decide with step 3's own doc. |
| Q9 | Step 3: may a paired device start a session remotely, or only answer, message and interrupt? It runs the user's own binary under the user's login, but it moves closer to a hosted product driving a subscription login. | Decide with step 3's own doc, after rereading both vendors' terms at the time. |

## Risks

- **The ancestry check is not a sandbox.** It refuses answers from inside an agent's process tree. An unsandboxed agent can still reach the human's own input (tmux, the browser's automation port) or re-parent a process out of its tree. The doc says so in the threat model, and S7 checks the sandboxed case.
- **Codex's control surface is experimental**, and the daemon auto-attach is documented only in source. The version gate limits the damage to "read-only until tested", but every Codex release may need a test run and a new entry in the tested list.
- **The Claude hook can hold up the terminal** if S1 finds outcome B. The short initial wait, the watching rule and "Answer in terminal" bound this, and the hook is opt-in.
- **OpenCode needs the port.** Sessions started without the wrapper or `--port` stay read-only.
- **Peer resolution fails closed.** A browser whose process Semon can't see can't answer. S6 measures how common that is.
- **The viewer becomes security-critical.** Any HTML injection in the viewer would now be an approval. The CSP without `unsafe-inline` and the banned-string test are what hold that line; both stay, and the request card never renders a string as HTML.
- **Step 3 makes shell-capable agents reachable from outside the machine.** That is why it is a reserved action, with its own doc and Marvin's explicit go.
