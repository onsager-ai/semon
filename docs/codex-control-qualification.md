# Codex local control qualification — #293

October 4, 2026. **Go for the opt-in, Semon-started Linux shared-server slice**;
**existing arbitrary-terminal writes remain blocked**. This record supersedes the
qualification-only conclusion in PR #295 while retaining its alpha fixtures and
original probe independently. See [the current design](design/two-way.md).

## Native evidence, not mocks

Install the complete, explicitly pinned **`@openai/codex@0.160.0`** npm release.
Use the native executable inside the platform package, together with its
`codex-package.json` and resources; copying a binary alone is not an installed
package. The source tag is `rust-v0.160.0`, commit
`a956835d020762cb2b570053af06f643a11c0ecc`. Binary hashes, captured native frames,
schemas and probe hashes are in
[`tests/fixtures/compatibility/codex-0.160.0/control`](../tests/fixtures/compatibility/codex-0.160.0/control/manifest.json).
The earlier alpha evidence and original probe remain under the 0.159.0-alpha.3
fixture directory and `codex-control-alpha.py`; its limitations are not rewritten
as successful qualifications.

All new fixtures use disposable native homes/workspaces and a synthetic localhost
Responses provider. Real native Codex generates requests, notifications, execution
outcomes and stored history. The product probe also uses the actual Semon binary,
viewer HTTP routes, browser and bubblewrap executor. No personal authentication,
paid inference, E2B resources or deployment is involved. Rust fake-server and
TypeScript transport tests are separate regression evidence. Current official
authentication documentation is [research](codex-hosted-auth.md), not native
eligibility/inference evidence.

The supplied alpha package has an escaping link:
`runtimes/codex-primary-runtime/dependencies/python/lib/python3.12/site-packages/certifi/cacert.pem`
resolves to `/etc/ssl/certs/ca-certificates.crt`. Native `prepare_install` rejects
canonical symlink targets outside the package root, apart from its legacy
`codex -> bin/codex` entrypoint exception. This explains the observed installer
failure. The complete pinned npm package passes daemon startup; the protocol is
not generally blocked by that alpha packaging failure.

## What is measured

| Case | Native evidence | Product behavior |
| --- | --- | --- |
| Default TUI and daemon | Daemon starts; a second client resumes the running TUI thread, receives its approval and observes terminal clearance | Existing-terminal observation; writes disabled without executor isolation |
| Foreground server and TUI | `--remote unix://PATH` shares the thread and approval; late terminal answer cannot override the recorded native decline | Writable only for the isolated Semon-started topology |
| Command approvals | Same native request broadcast/replayed; exact command/cwd/thread/turn/item; separate successful execution output | Allow once/deny, exact payload hash, first claim, expiry/journal |
| Questions | Native Plan mode emits `item/tool/requestUserInput`; ID-keyed arrays deliver an offered answer | Qualified non-secret subset; unsupported shapes read-only |
| Patch | Actual Responses `custom_tool_call` with raw `apply_patch` input emits a file-change request and native diff; denial preserves file | Native changes bound/displayed; browser denial measured |
| Input/steer/interrupt | Native expected-turn rejection, steering and targeted interrupted outcome | Stale generations/turns refused; native identities retained |
| Concurrent clients | Terminal clears before viewer answer; duplicate/stale viewer answers refused; native late answers do not change outcome | Resolved means answered or cleared; winning client/decision never inferred |
| Reconnect | Pending native request replay, new connection generation, old viewer target refusal | Subscribe afresh; no write replay; unknown claims keep matching replayed requests read-only |
| Delivery loss | HTTP response abandoned after dispatch; native outcome and saved receipt inspected | No resend; fake-server regression also consumes a write and loses its reply |
| Native/Semon restart | Server loss disconnects driver; old target/refreshed transport fail; reused state directory refused | No automatic replacement or reconstructed writes |
| Owner isolation | Working same-UID broker and tmux baseline, including sockets inside the workspace; namespace-only escape reproduced; seccomp-confined escalated native tool blocked from both, native rendezvous/physical sockets, viewer even with correct cookie, owner home/config/proc/symlink/env | Fail-closed namespace and syscall boundary, including native escalation |
| Untrusted project | Host MCP program does not start on initial load or later untrusted config | No host-side project escape; inherited managed/native config restricted |

The terminal test initially used a non-replayable `-c` trust override, causing
embedded fallback. The corrected test puts trust in its disposable config and
omits that override. A background native title request could consume a synthetic
tool fixture; the provider now excludes title requests. These were probe setup
errors, not protocol limitations.

Native `turn/start` is start-or-steer, without an idle compare-and-swap parameter.
A terminal start after Semon's pre-dispatch check can steer the submitted idle input
into that native turn. Active steering uses `expectedTurnId`; all accepted receipts
retain the returned native identity. No exactly-once/new-turn guarantee is claimed.
Native Unix listeners publish a guarded rendezvous symlink to a protected physical
socket; both paths are tested for isolation. A newly created empty thread has no
resumable rollout until its first user message, so send that message before
attaching the shared terminal.

## Reproduce and run locally

Prerequisites: Linux x86_64 with working unprivileged bubblewrap namespaces and seccomp, Python,
Node 22+, a complete pinned native npm package and a built Semon. Tool networking
is off in this first slice. Native homes and state must be outside the workspace;
state must be new and private. Managed native configuration is not qualified.

```sh
npm install --prefix /path/to/native @openai/codex@0.160.0
python3 -m venv /path/to/probe-venv
/path/to/probe-venv/bin/pip install -r tests/spikes/requirements-control.txt
cargo build --locked -p semon-store
npm --prefix tests/ui ci
npm --prefix tests/ui exec -- playwright install chromium

# NATIVE is the actual .../@openai/codex-linux-x64/vendor/.../bin/codex,
# rather than the npm JavaScript wrapper. OUT is a disposable output directory.
/path/to/probe-venv/bin/python tests/spikes/codex-control.py --codex "$NATIVE" --output "$OUT/qualification.json"
/path/to/probe-venv/bin/python tests/spikes/codex-terminal.py --codex "$NATIVE" --output "$OUT/terminal.json"
/path/to/probe-venv/bin/python tests/spikes/codex-local.py --codex "$NATIVE" \
  --semon target/debug/semon --browser tests/ui/local-control.mjs \
  --tmux /usr/bin/tmux --output "$OUT/local.json"
python3 -m unittest discover -s tests/spikes -p test_control_fixtures.py -v

semon control codex --codex "$NATIVE" --workspace /path/to/work \
  --state /path/to/new-private-state --config /path/to/model.toml
```

The model config accepts only native model/provider settings, for example a
Responses provider using `env_key="OPENAI_API_KEY"`. Credentials remain in the
trusted model transport; the tool executor receives none. Existing auth homes
are never copied or modified. This slice does not add personal sign-in onboarding
or qualify commercial hosted use. The CLI prints a single-use local viewer URL
and, after the first message, the quoted terminal attachment command. An existing
terminal can instead be observed with `semon control observe --socket PATH --thread
ID --codex-home HOME --journal PRIVATE_PATH`, with all write capabilities disabled.

## Release gates and validation

The usable local product slice is independent of E2B/coordinator lifecycle and
hosted-auth eligibility. The smallest next action for existing-terminal writes is
to qualify a launch/attachment arrangement that places its executor behind the
same concrete boundary, including brokers and native socket access, while
preserving the existing terminal. The commercial hosted gate needs an approved
Sign in with ChatGPT route and separate authenticated model/lifecycle tests.
Neither gate is waived by the passing shared-server tests.

Verification commands/results and selected browser coverage are recorded in the
implementation PR. Native fixtures are reproducible evidence, not the complete
viewer CI aggregate. No existing pixel threshold or baseline is changed; CI
aggregate status remains a separate merge gate. No deployment or remote control
activation is performed.
