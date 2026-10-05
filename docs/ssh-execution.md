# SSH execution receipt broker

Part of #303; builds on merged #304 and Hub #98. Inspected base: main
30588f6 (managed E2B #306 included). The generic Session/OperationId/CAS contracts
remain in semon-runtime; hosted custody, current owner/workspace authorization,
product controls and network policy remain in Hub. Local push and E2B are unchanged.

`semon_ssh::execution` transports separately authorized operations to an already
running, qualified private driver. It does **not** install or launch a native
harness. Mirror bootstrap neither calls nor creates execution authority. A driver
must expose the existing managed-guest JSON protocol on a private 0600 Unix socket
at `~/.local/state/semon-ssh/<stable-enrollment>/control.sock`. The embedding must
qualify the driver binary, version, controller protection, executor isolation,
workspace and harness-home binding before explicitly authorizing execution.
The socket and receipts must be outside the executor's mounts. Same-user file
permissions alone do not isolate an unconstrained agent. No fallback is permitted.

## Coordinator contract

Persist the Session, stable connection/enrollment identity and operation before
side effects. After the qualified driver supplies a native thread receipt, bind
that exact thread and mark the operation InFlight through the existing CAS store.
Construct Execution with the authenticated owner, enrollment, a separately issued
execution authority UUID and a short (at most 300-second) authority lease. The
application must recheck current workspace membership, ownership, credential
generation and outbound destination policy on **every** call, including inspection.
An SSH key is transport authentication, never product authorization.

Call authorize only after explicit execution consent. It pins owner, workspace,
connection, model connection, session, epoch, operation and thread to the remote
grant. Dispatch derives a stable 32-character native action identity from session,
epoch and OperationId; it requires the driver's observed generation. There is no
compute create, pause, snapshot, restore, replacement or deletion API. Execution
capabilities are explicitly separate from generic provider defaults.

The remote broker fsyncs an atomic private claim **before** submitting input.
Duplicate delivery with the same identity inspects the existing native action;
it never submits it again. Changed input/generation under an existing identity
is refused. Timeout, cancellation, interrupted SSH replies, missing controller
or lost native evidence retain Unknown. Reconcile reads receipts and qualified
native action evidence; a missing result is never evidence that dispatch failed.
Accepted evidence can be applied with record_dispatch and committed through the
same coordinator CAS. Acceptance proves dispatch, not completion of agent work.
No automatic replay, writer takeover or replacement follows Unknown.

One nonblocking enrollment writer lock covers all execution grants, dispatch,
repair, inspection and revocation. The native driver must additionally enforce
active-turn and exact thread/generation checks. Idle credential repair is required;
epoch changes, disconnects, lease expiry and revoked grants do not prove writer
exclusion. Host pins remain mandatory on every connection. Local futures kill
OpenSSH on cancellation; remote claims survive. Calls have a 40-second transport
bound, 25-second private IPC timeout, bounded request/response sizes and fixed,
actionable errors. Raw driver errors/output never become logs or receipts.

## Model credentials

ModelCredential and SSH Credential are distinct private types with no Debug or
Serialize implementation. Model material travels only through SSH stdin and the
private controller socket. It does not enter prompts, arguments, environment,
receipt records or returned diagnostics. The qualified controller must keep model
authentication outside its executor, as LocalSession does. Repair uses a distinct
stable operation identity and caller-persisted credential generation; lost replies
reconcile the native renewalId rather than repeating credential renewal. This
broker initially selects the OpenAI API-key driver method; other methods need
separate qualification and cannot be inferred from SSH authentication.

Revoke fsyncs a permanent authority tombstone before requesting native end. A
revoked authority cannot be reauthorized. Shutdown uncertainty remains visible:
writer_excluded is set only from a native ended response or private ended.json
receipt matching the bound thread. Reconciliation does not recreate credentials.
A caller must revoke its own authority/custody immediately and retain any pending
remote shutdown obligation when a server is unavailable or a writer lock is busy.
Remote fencing cannot promise instant shutdown of an unreachable server.

## Evidence and remaining gates

`scripts/ssh-fixture.sh cargo test -p semon-ssh --locked --features real-ssh-tests`
uses a real isolated OpenSSH server and synthetic private driver, keys and work.
It exercises separate grants, pinned transport, durable delivery receipts, lost
replies, canceled client futures, reconnect inspection, duplicate prevention,
writer exclusion, missing native evidence, private model repair and permanent
revocation. It checks that model material is absent from ordinary receipt files.
This is transport/protocol evidence, **not** native Codex launch/isolation or
hosted authority qualification. Hub's existing genuine binary push/readback and
native mobile/keyboard/no-JavaScript onboarding suite remains a separate gate.

Hub SSH execution actions must stay absent. Remaining native work is [#307](https://github.com/onsager-ai/semon/issues/307);
hosted integration is [Hub #102](https://github.com/onsager-ai/semon-hub/issues/102).
Follow-up work must qualify a native
SSH controller installer (with separate launch consent, no reused-home launch,
bounded lifetime and shutdown), owner/workspace revocation races and encrypted
model custody/generations, combined dispatch/session push/Hub readback, native
credential repair/revocation and mobile/keyboard/no-JavaScript execution forms.
Do not treat passing synthetic receipts as permission to launch on a user's server.
