# Two-way control: one local Codex session

Updated October 4, 2026. [#293](https://github.com/onsager-ai/semon/issues/293)
is Codex-first. This replaces the earlier Claude-first delivery order and
ancestry-based owner check. The landed `semon-control` foundations remain the
request store; no second control framework or transcript collector is introduced.

## Delivered local slice

`semon control codex` explicitly starts one pinned **Codex 0.160.0** app-server,
one isolated Linux executor and the existing loopback viewer. The viewer supports
user input, steering the expected active turn, targeted interrupt, allow-once or
deny for qualified command/file approvals, qualified native questions and explicit
reconnect to that running server. A Codex terminal can share it using the documented
`--remote unix://PATH resume THREAD` mechanism.

This is a **Semon-started shared-server session**. It is not attachment to an
arbitrary existing terminal. The corrected native probe demonstrates default TUI
attachment to its daemon and foreground Unix-socket terminal attachment separately.
`semon control observe` can watch an existing daemon thread, but its capabilities
are read-only: its agent executor has no qualified owner boundary. Resolving this
remaining #293 requirement means isolating that executor before permitting writes,
including direct access to Codex's own socket. A different launch topology does not
satisfy the existing-terminal requirement.

E2B provisioning, absent-runtime queues, checkpoint/pause, reconstruction,
automatic lifecycle, remote control and additional harnesses are separate work.
Normal `semon sessions --serve` and stdio MCP remain read-only.

## Driver and identities

`semon-control::codex` is one native app-server connection. Native thread IDs,
turn IDs, item IDs and JSON-RPC request IDs remain authoritative. Its connection
generation includes the physical socket identity and a random generation; a
reconnect invalidates previous viewer targets. Each browser operation carries a
unique ID, exact thread/generation/active turn and a short expiry. Duplicate IDs
only read saved receipts; changing their payload is refused.

Input uses `turn/start` when idle and `turn/steer` with `expectedTurnId` when active.
An event-state check is repeated just before dispatch. Native `turn/start` itself
is start-or-steer: a terminal starting a turn after that check can receive the idle
input as a steer. There is no native idle compare-and-swap field in this release;
the returned native turn ID, not an assumed new turn, is the receipt. The product
binds idle input to the session; active input uses the exact expected turn. This
remaining native concurrency limit is recorded, not concealed as exactly-once input.
Interrupt always supplies the exact `threadId` and `turnId`. Native turn/item/request
notifications own live state. Existing session collection, transcript paging and
mirroring own historical viewing. There is no transcript-to-live-state inference.

The request store supplies canonical payload binding, first claims, monotonic
expiry, bounded retention and the private journal. The bound native payload includes
method, native request ID, connection generation, thread/turn/item parameters and
available item evidence. Native question answers use question IDs and arrays,
not Claude's question-text map. The qualified subset is one answer per distinct
question ID, offered labels or explicitly permitted free text, at most three
non-secret questions. Secret/malformed questions and unqualified tool requests
remain read-only with a reason. File approval requires the actual native file-change
item and its changes; missing diff evidence is read-only. No persistent approval
or rule amendment is offered.

`serverRequest/resolved` means **answered or cleared**. It supplies neither the
winning client nor its decision. Response writes have no native acknowledgement;
they remain claimed until resolution, deadline or loss, and resolution never
becomes proof that the viewer's answer won. Native tool outcome/history is separate
evidence. Transport loss closes open requests and leaves claims as delivery unknown.
An uncertain replayed native request is read-only. Reconnect subscribes afresh and
never replays a write. A writable reconnect also checks that the originally isolated native server process
and physical socket are unchanged. This is a continuity check, not proof of human
authority. Server/process restart does not reconstruct control actions
or launch replacement compute. A fresh launch requires a new state directory.

This demonstrated interactive requirement supersedes #252's `exec --json` / `exec
resume` plan for interactive Codex. Use this app-server driver for live input and
requests. Existing batch exec probes and transcript parsing remain useful evidence;
do not add a parallel interactive exec path. Native thread identity stays distinct
from a durable logical session or provider sandbox identity in #248.

## Owner authority

Agents must not approve themselves. Process ancestry, same UID, cookies and an
agent-readable bearer token are insufficient: a host tmux/process broker can
execute a request outside the agent's ancestry, and an accessible native socket
can bypass Semon entirely.

The writable slice therefore requires Linux x86_64 **bubblewrap namespaces and
seccomp**, verified
before launch. Only the workspace is writable in the outer executor. Read-only
OS/runtime binaries, private procfs/dev/tmp, an empty private agent home, cleared
environment, separate PID/mount/user/network namespaces and no host sockets are
provided. Pathname Unix sockets inside the writable workspace remain visible across
network namespaces: native evidence demonstrates this escape without seccomp.
The outer filter therefore denies socket creation, connect and io_uring setup,
rejects alternate syscall ABIs and permits only already-connected anonymous Unix
stream pairs required by the native executor. Datagram pairs are denied. Agent
commands cannot use even a host tmux/broker socket placed inside the workspace.
Even native escalated commands run inside this outer executor. Model
networking belongs to the trusted app-server outside it; **tool networking is off**.
Host home, Semon secrets, runtime directories, the physical Codex control socket,
tmux/process-broker sockets and owner procfs are absent. The complete pinned
package is mounted read-only.

Fresh native state accepts only model/provider configuration. Native hooks,
plugins/apps, host code mode, JS REPL and multi-agent features are disabled; project
configuration remains untrusted. System config is refused unless it contains only
plugin/marketplace settings that these disabled features cannot execute. Legacy
managed configuration/requirements are refused pending separate qualification.
No inherited executor selector or arbitrary environment is accepted. Only model
credential variables and explicit proxy/CA settings reach the trusted server; none
reach agent commands. Kernel isolation failure refuses startup, without a weaker
fallback. This boundary assumes the owner and installed kernel/runtime binaries
are trusted; it does not defend against an owner deliberately authorizing the agent
through another unconfined channel.

Real native attack evidence includes a working same-UID process-broker baseline,
and an escalated agent command unable to access that broker, native rendezvous and
physical sockets, authenticated viewer route even with the correct cookie supplied,
owner files/config, a workspace symlink to owner data and owner `/proc/.../root`.
Untrusted project MCP configuration does not start its host program. See the
[qualification record](../codex-control-qualification.md). These are tested concrete
boundaries, not a claim that generic same-UID terminals are safe.

Control is opt-in. Existing loopback binding and random per-run authentication are
retained. POST requires exactly one matching Host, exact same Origin, the session
cookie, JSON content type and bounded body. No CORS or URL-token write authorization
is allowed. The bootstrap URL is single-use for control and exchanges a separate nonce for the cookie secret;
a spent URL never authorizes a write. Authenticated cookie
refresh remains usable. Exact target/payload hashes and expiry are rechecked at
mutation time, and the journal precedes dispatch. HTTP has no peer PID/UID evidence;
journal zero peer fields mean unknown, never root authorization.

## Typed viewer ownership

The existing model transport prepares control data before its coherent model commit.
One local control owner uses the application effect scope, current page commits and
existing model poller. It owns write intent/busy/uncertain state; shared screens
receive a narrow typed view. No extra router, poller, document handler or transcript
cache is installed. Each capability is exposed individually. Unsupported requests
show their reason. After uncertain browser delivery, writes stay blocked until an
explicit reconnect; inspection of the native session is still required, and no
original operation is resent.

Shared consumers without a local control projection keep their existing behavior.
This local implementation does not supply authentication or enable control for a
remote service.

## Trust and authentication remain separate

Remote control of a **user-owned machine** still requires the agreed paired native
app, local opt-in and authenticated signed exact-target commands; a public hosted
browser must not replace the paired owner authority. Managed guests use the hosting
coordinator's tenant/owner isolation and scoped guest credentials. A guest Push token
is ingestion authority, never approval authority. Neither trust model implies the
other, and no remote route is activated here.

The agreed API-key and personal ChatGPT targets remain. Local native device login
is evidence of a protocol, not permission for commercial hosted use. The current
official third-party route is **Sign in with ChatGPT**, with OSS/local documentation
and a separate limited commercial-partner trial. Eligibility, token acquisition,
refresh and actual inference require independent hosted qualification. See
[authentication prerequisites](../codex-hosted-auth.md). These unresolved gates do
not erase the usable isolated local control slice.
