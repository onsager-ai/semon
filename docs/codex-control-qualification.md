# Codex local control qualification — #293

Decision: **no-go for enabling Semon local control**. This is the first qualification slice of [#293](https://github.com/onsager-ai/semon/issues/293), not delivery of the viewer write path. Codex-first supersedes the previous Claude-first order. No Codex version is currently supported for Semon answers, messages, steering or interrupts.

Main reconciled: `b0d4aaae483969aaad7bc92c1a79de5dab28dc48`, October 4, 2026. `semon-control` already owns payload hashing, requests, matching, claims, deadlines and journal. The typed viewer owners and shared Hub contracts are retained. This change adds qualification evidence only: no control flag, routes, driver, poller, store, UI, remote pairing, Claude adapter, hosted runtime management or Copilot collection.

## Pin and reproduction

- Installed native binary: `codex-cli 0.159.0-alpha.3`, Linux x86_64, Debian 13.
- Binary SHA-256: `981ade7b03926534c654fd718ced3a9f378b7b2841271e29156f939462d176e9`.
- Client dependency: `websockets==16.0`; qualification tooling only, not a product dependency.
- Model: synthetic localhost Responses SSE. The model emits controlled tool calls; the native binary generates requests, notifications and persisted history. This is real native protocol evidence with a mock model, not a live commercial model or a mocked daemon.

```sh
python3 -m venv /workspace/scratch/control-venv
/workspace/scratch/control-venv/bin/pip install -r tests/spikes/requirements-control.txt
/workspace/scratch/control-venv/bin/python tests/spikes/codex-control.py \
  --codex /path/to/pinned/codex --output /workspace/scratch/codex-control.json
python3 -m unittest discover -s tests/spikes -p test_control_fixtures.py -v
```

Use a scratch output directory outside native homes and outside `/tmp` (this binary refuses helper aliases under `/tmp`). The probe creates private disposable `CODEX_HOME` and working directories, strips model authentication selectors, retains proxy/CA configuration, copies no native config or credentials, and reads no personal transcript. It bounds waits, closes clients and terminates its foreground process group. Default daemon management targets only the disposable home. The fixture manifest pins the probe and evidence hashes; disposable paths are replaced with `<FIXTURE>`, while native IDs, ID types and payloads are preserved. Terminal evidence strips only screen-control sequences.

## Native results

| Case | Observed result | Scope/limit |
| --- | --- | --- |
| Default `app-server daemon start` | Exit 1: `package link escapes its root`; shared control socket absent | Environment/package startup blocker. Does not establish that every installation of this version fails. |
| Default TUI in a PTY | Exit 1 with the same package error; suggests `--no-daemon` | Cannot qualify an existing default TUI session, terminal approval races or terminal fallback behavior here. |
| Explicit `app-server --listen unix://<private-socket>` | Native initialize and persistent thread/turn APIs work | A separately started foreground server; a potential Semon-started-session alternative, **not** attachment to the user's running terminal. |
| Second client `thread/resume` | Works after a warm-up turn has persisted the thread | An exploratory resume before the first turn failed with `no rollout found`; do not assume thread/start immediately makes disk-backed resume possible. |
| Command approval | Owner and resumed observer receive identical JSON-RPC request and exact command/cwd | Synthetic `exec_command` escalation; other approval kinds remain unqualified. |
| Pending reconnect | New initialized client + `thread/resume` replays the same request ID and payload | Explicit foreground server only; not evidence of recovery across a daemon restart. |
| Resolution | Both original clients receive `serverRequest/resolved` | Contains thread ID and request ID only; no decision, winning client, turn or item. |
| Competing client answer | Decline first; late accept does not change native item status `declined` | Proxy for a two-viewer race. No TUI was answering this request. |
| Allow once | `accept`, followed by command completion, exit 0, exact fixture output | No `acceptForSession`, policy amendment or bypass. Delivery/resolution and execution are distinct observations. |
| Stale steering | Native error for an incorrect `expectedTurnId` | Exact active turn targeting is required. Successful steering is not qualified by this slice. |
| Interrupt | Native `{}` response for exact thread/turn, then `turn/completed` with `interrupted` | Acceptance and outcome recorded separately; history read succeeds. |
| Malformed JSON | A bad client frame does not prevent another client reading the thread | No Semon decoder or authenticated route exists yet. |
| Server termination with a pending approval | Client disconnect observed | Pending request is no longer safely answerable; no response is replayed. |
| File-change probe | Native rejects the attempted `apply_patch` call; no approval/diff request; file absent | Does **not** prove file-change control unsupported. Need a supported native tool invocation and exact diff join. |
| Question probe | Tool feedback: `request_user_input is unavailable in Default mode`; no question request | Need a supported native mode/tool invocation before qualifying questions. |

The command approval binds JSON-RPC request ID (integer `0` in the first case), native `threadId`, `turnId`, `itemId`, command `/bin/bash -lc 'printf SEMON_APPROVAL_PROBE'`, and `<FIXTURE>/work`. `item/started` carries the same command and exact identity tuple. The native response is `{"id": <request-id>, "result": {"decision": "accept"}}` or `decline`. No response acknowledgement identifies which client won. Native `availableDecisions` in this escalation case lists accept, a policy amendment and cancel; nevertheless the tested `decline` was honored. Do not infer all approval variants from this case.

The generated native schemas describe additional callback identities (`approvalId`, stdin approval `kind`) and question answers keyed by question **ID**, with `answers: {id: {answers: [...]}}`. Schemas establish shapes, not delivery support. The existing store validates Claude questions keyed by question text; a future Codex extension must validate native IDs and answer shapes without losing exact payload binding or duplicating the store. File-change schema has no diff; a future driver must qualify the authoritative item join rather than displaying a summary.

## Trust gate and outcome semantics

The September 30 S7 evidence in `docs/design/two-way.md` remains a blocking finding: a network-enabled Codex sandbox can ask a same-user tmux/process broker to connect to the viewer, leaving the harness ancestry tree. This run does not claim that escape has been closed or requalified on the pinned alpha. Ancestry, uid, cookie, Host/Origin and single-use URL checks cannot distinguish a broker-launched client from the owner. A token readable by an agent is not a new owner boundary. Relabeling the session unsandboxed, excluding tmux from tests, or accepting an outside-tree process would weaken #293.

Before enabling local writes, demonstrate an enforced boundary preventing broker access and owner credential/input theft while retaining the required owner flow. If that requires changing native sandbox policy or adding an owner authentication mechanism, record the concrete mechanism and adversarial tests first. No insecure ancestry-only route is shipped in this slice.

`serverRequest/resolved` is insufficient to say a Semon claim won a race. The future driver must retain a neutral resolution/uncertain-delivery outcome, join separate native item completion and persisted history, and never retry an uncertain answer/message/interrupt blindly. Request IDs must be scoped to server lifetime and native session/turn/item identities; reused item IDs across different turns in these fixtures cannot collapse. Disconnects invalidate pending delivery authority; reconnect may rebuild native open requests but must not revive old claims or replay writes. Existing store deadlines and terminal fallback remain mandatory.

## Remaining acceptance

1. Qualify default live TUI/daemon attachment on an installation where native startup succeeds, including actual terminal/viewer races and fallback, native version mismatch and daemon restart.
2. Close the same-user broker owner-authentication gap with an enforced and tested boundary. Cross-user isolation and process reuse checks are additional requirements, not substitutes.
3. Qualify supported native question mode, all permitted answer shapes and exact command/diff joins. Record question reconnect/resolution and file-change races.
4. Implement the version-gated driver, authenticated opt-in routes and shared request UI using existing owners. Extend the shared store only where native identities/outcome semantics require it. Unknown versions/capabilities stay read-only with an explicit reason.
5. Qualify successful new messages and steering, persisted outcome joins, ambiguous delivery and reconnect without write replay. Then implement exact-target local messages/interrupts.
6. Run full product acceptance for the final implementation. No remote or Claude adapters are enabled by this milestone.

## Validation for this qualification slice

Local checks passed: required Rust fmt, Clippy and locked tests (including shared store race/deadline/unknown-delivery/session-end regressions); CLI fixture provenance; harness icon hashes; UI format, architecture, design, typecheck, unit tests, bundle freshness and sizes; browser infrastructure and text-audit regressions; all 30 independent shared-consumer gallery visual states; 24 browser/application/native-shell lifecycle tests. No thresholds or baselines changed. The added evidence tests check native identities, command binding, reconnect, competing answers, allow-once outcome, interrupt, malformed-client isolation, termination, capability limits and provenance.

The complete viewer fixture/browser aggregate remains a required CI gate. The local gallery is independent consumer evidence, not a claim of Semon Hub production acceptance. Shared UI sources and consumer contracts were not changed, so this slice requires no Hub pin update. CI state is reported on the PR and #293; pending/unavailable checks are not acceptance passes.
