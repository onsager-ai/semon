# Harness-agnostic agent messaging

Status: design only (Marvin, 2026-09-28: "Design doc only"). Nothing here is implemented. Building any milestone needs a go that names it.

## Goal

Agents in different harnesses should be able to message each other the way Claude Code sessions already can: a Claude Code session and a Codex run, on one machine or several. Semon already puts every session of both harnesses into one tree with one id space (`list_sessions`, `get_session` over `semon mcp`). Messaging adds the write side of that tree.

## What exists today

### Claude Code (documented behaviour)

Sources: [cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging) and [agent teams](https://code.claude.com/docs/en/agent-teams).

- **Transport.** Each session binds a private Unix domain socket at startup, under `$XDG_RUNTIME_DIR/cc-socks/<pid>.sock`, restricted to the OS user. Its registration record is `~/.claude/sessions/<pid>.json`: pid, sessionId, cwd, name, status, `messagingSocketPath`, and `peerFeatures` such as `notify_idle`. Sessions on other machines and in the cloud are reached through Anthropic's servers.
- **Addressing.** By session name, with a suffix when names are ambiguous. The tools are `ListAgents` and `SendMessage`.
- **Delivery.** A message arrives between tool calls, or as a new turn when the receiver is idle; it never interrupts a running tool. It is delivered, held for the user's approval (by default when the two sessions' permission modes differ), or refused (size cap, rate limit). `notify_when_idle` gives a one-shot notice when a same-machine peer goes idle.
- **Trust.** A peer message is untrusted by construction. It can't approve a permission prompt or change settings, and slash commands inside it are inert text. The receiver's own permission rules still gate anything the message asks for.
- **Stability.** The tools and settings are documented. The socket's wire format and token handling are not published as an API for third parties, so a third process writing to another session's socket would rely on undocumented, version-coupled internals.

### Codex (codex-cli 0.156)

- **`codex queue --thread <id|name> --message <text>`** queues a message into an existing session. It is documented and needs no protocol work.
- **`codex agents`** browses the sessions on the shared local app-server daemon, roughly Codex's `ListAgents`.
- **`codex app-server`** is JSON-RPC over stdio, a socket or a WebSocket. It supports `turn/steer` (inject input into a running turn), `turn/start` and `thread/resume`. It is documented as experimental and not for production.
- **MCP.** Codex is an MCP client: it can call Semon's tools. It is not itself an MCP server.
- **`notify`.** Runs a program on `agent-turn-complete`. It is outbound only.

## Rules (decided)

1. **Every message is untrusted.** Semon frames every delivered message with who sent it (session, harness, machine) and states that it carries no authority: it can't grant permissions, approve prompts or change configuration. This is Claude Code's rule, applied to every harness through Semon's framing, because a harness that receives the text through a tool or a queued prompt doesn't know it came from a peer.
2. **Same owner only.** A session may message only sessions of the same OS user on the same machine. Across machines, it may message only sessions of the same owner on a relay server. There is no public addressing.
3. **Never guess.** A message is addressed by Semon's session id, or by a name that resolves to exactly one session. Delivery status is reported as observed (queued, delivered, read), never assumed.

## Design

### Addressing
Semon session ids, the same ids `list_sessions` returns, already cover both harnesses and carry their machine. A `list_agents` answer adds each session's delivery capability: push, or pull only.

### Mailbox
Semon keeps a durable per-session mailbox (append-only entries: id, from, to, text, sent_at, delivered_at, read_at). It is local by default. A relay server can mirror mailboxes between machines, as it mirrors logs today.

### Tools (`semon mcp`, a write surface kept apart from today's read-only tools)
- `send_message(to, text)`: checks rule 2, appends to the mailbox, and attempts native delivery (below). It returns the observed status.
- `inbox()`: the caller's unread messages, each framed per rule 1, marked read on return.
- `list_agents()`: the reachable peers and their delivery capability.

### Delivery per harness
| Harness | Delivery | How | When the receiver sees it |
|---|---|---|---|
| Codex | push | `codex queue --thread <id> --message <framed text>` | at its next turn |
| Claude Code | pull | the session calls `inbox()` (prompted by a hook or its instructions) | when it checks |
| Claude Code (later) | push | only through a documented third-party entry point, if Anthropic publishes one | between tool calls |

Semon doesn't write to Claude Code's socket or to its team mailbox files (`~/.claude/teams/*/inboxes/*.json`). Both are documented as behaviour, not as extension points, and would couple Semon to internals that can change without notice.

### Identity
The sender is the session that called the tool. Semon resolves it from the MCP server's parent process, using the ancestry method of #20, and never from the message text. If the caller can't be resolved exactly, sending is refused.

## Milestones (proposed; each needs a go)
1. **M1, local mailbox and tools.** `send_message`, `inbox` and `list_agents` on one machine. Codex gets push through `codex queue`; Claude Code gets pull. The framing and rules 1–2 are enforced and tested with synthetic sessions.
2. **M2, a nudge for pull receivers.** A Claude Code hook (for example on `Stop` or `UserPromptSubmit`) runs `semon inbox --count` and asks the session to read its inbox when it's non-empty. The session reads it itself; the hook never injects the text.
3. **M3, across machines.** A relay server mirrors mailboxes between the same owner's machines, with the same rules; delivery at the receiving machine is as in M1.
4. **M4, native push for Claude Code**, only if a documented entry point exists.

## Open questions
- **Size and rate limits** per sender (Claude Code's own are about 1 M characters, plus a burst limit).
- **Retention:** how long delivered messages are kept, and whether they appear in the transcript view as handoffs (they should, as the kind `message`).
- **Idle notifications:** whether `semon` should offer a harness-agnostic `notify_when_idle` built on its existing liveness facts.
