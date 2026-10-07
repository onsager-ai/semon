# Native Codex controller on SSH

This explicit opt-in controller builds on the private receipt broker. It does not
run during host discovery, connection tests or mirrored-history setup. The
embedding must persist a fresh native launch claim and separately authorize
execution against its current owner, workspace, SSH/model credential generations,
host pin and destination policy before calling `semon_ssh::controller::install`.
The SSH key authenticates transport; it is not application authorization.

The initial pairing is Linux x86_64, OpenSSH/Python3, working unprivileged
namespace isolation and the complete official Codex 0.160.0 musl package. Its
preinstalled package is checked against every file digest from the independently
verified upstream archive (SHA256
`4fcc47ab57f52ff75363951a8761146cd10c8288bd86fed45487dbb204a16b71`).
Package provisioning is a separate host prerequisite; this installer does not
fetch arbitrary native executables. The embedding selects and pins the reviewed
controller artifact/version. Its delivered bytes are integrity checked before
installation. This slice supports the OpenAI API-key method; other native
credential methods require separate evidence and never silently change billing.

Installation binds an persisted stable mirror enrollment to one immutable launch
identity and application binding. It creates a fresh private workspace/home,
refuses reused launch state and unsafe paths, and delegates native startup to
`LocalSession`. Its namespace and seccomp probes must both pass; there is no
same-UID permissions-only fallback. The agent executor cannot reach owner
sockets, credentials or receipts. Native model instructions remain from the
bundled catalog; the separately qualified standalone direct-tool profile remains
in use. Credentials travel through protected stdin and model transport, not
arguments, capture files, operation metadata or returned diagnostics.

A launch receipt is committed before native startup and the exact native thread
is committed before accepting input. Interrupted or duplicate installation reads
that same identity. It never silently creates another native thread or reuses a
home. `controller::observe` performs bounded read-only inspection without model
credentials: running requires a matching connected native snapshot, ended requires
an independent reaping receipt, and missing/unavailable proof remains unknown.
The application still rechecks current SSH/application authority on reads.
After installation, `bootstrap_native_receiver` watches only that enrollment's
fresh native home. It requires an immutable installation claim and does not
start or authorize a native process; ordinary mirror bootstrap keeps its existing
host-history behavior.

An independent Linux subreaper supervisor owns a maximum five-minute lifetime.
Both controller and supervisor use monotonic bounds; a stalled IPC peer cannot
extend the lease. The supervisor adopts the exclusively started native descendants
after controller loss and signals only processes matching its descendant/start
identity. Writer exclusion is published only after that process tree is reaped.
A native end acknowledgement begins shutdown; it does not skip that proof.
Expiry alone, a revoked grant, mirrored history or an unreachable host never proves
exclusion. Embeddings retain shutdown obligations until qualified evidence arrives.
No automatic lease extension is implemented.

The existing execution broker preserves permanent grant tombstones and fsynced
claims before dispatch/repair. Installed controllers additionally fsync bounded,
private per-command and renewal indexes. Recovery checks the exact thread,
application binding, input/generation fingerprint and native result. A missing
result remains unknown; neither controller duplicates nor broker reconciliation
replay it. Idle credential repair stops the previous native writer and reconnects
the same thread/home. It preserves model-generation provenance and refuses older
generations or another repair while an earlier renewal is uncertain.

Human native permission requests remain native. The bounded synthetic fixture
explicitly authorizes its one expected shell command through owner-only IPC.
Hosted approval/question/interrupt capabilities stay unadvertised; existence of
this controller does not grant permission to expose those operations.

Current capabilities: separately authorized native launch, initial input,
read-only observation/reconciliation, same-thread transport reconnect, idle
API-key repair and permanent execution revocation with shutdown proof. Restarting
a lost controller currently confirms shutdown and preserves receipt/history
inspection; it does not launch or resume a new process. A future explicit handoff
would create a new native session. Compute pause/snapshot/restore/replacement/
deletion, automatic native resume after expiry and environment recovery are absent.
Mirrored history remains viewable independently of those execution capabilities.

## Qualification

```sh
cargo build -p semon-control --bin semon-ssh-controller -p semon-ssh --example controller-install --locked
python3 tests/ssh/test_controller_supervisor.py
SEMON_CODEX_PACKAGE=/absolute/pinned/package \
SEMON_SSH_BINARY=/absolute/qualified/semon \
scripts/ssh-native-fixture.sh python3 tests/spikes/ssh-native-controller.py \
  --installer "$PWD/target/debug/examples/controller-install" \
  --controller "$PWD/target/debug/semon-ssh-controller" \
  --codex /absolute/pinned/package/bin/codex \
  --output /absolute/private/fixture-report.json
```

The fixture creates an isolated OpenSSH server, synthetic model HTTP server and
private keys; it never uses a user server or paid inference. Docker's fixture-only
seccomp/AppArmor/system-path restrictions are removed so the *unprivileged native
namespace boundary itself* can be tested. The native package and push binary are
mounted read-only and the invoking unprivileged UID matches the fixture user.
Docker containers/images, synthetic keys and receiver tokens are removed on exit.
The product remains fail-closed on hosts where those kernel probes fail.

Evidence includes actual native `exec_command`, preserved instructions, exactly
one tool execution, duplicate install/delivery, same-thread reconnect/idle repair,
permanent revocation, controller-loss/expiry shutdown with a stalled status client,
recovery after the controller stops, and missing-proof uncertainty without another
turn. The same fixture pushes the original native records with an actual Semon
binary and parses receiver history, checking native thread/tool output and absence
of credential material. Report the controller, push binary, package and checkout
identities separately. Hosted custody/consent/race/UI/API/readback adoption remains
an embedding qualification; this fixture does not enable an application feature.
