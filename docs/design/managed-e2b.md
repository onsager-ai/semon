# Managed E2B guest (proposed, disabled)

The companion Hub design owns configuration, product behavior, validation and
release gates: [Managed E2B Codex slice](https://github.com/onsager-ai/semon-hub/blob/codex/e2b-managed-slice/docs/managed-e2b.md).
This core change stacks on **open** PR #301 (`32ab108`), whose app-server driver
is the only interactive transport. It does not claim that #301 has landed or
close the broader #248/#250/#251/#252 lifecycle roadmap.

`scripts/build-managed-artifact.sh OUTPUT_DIRECTORY` builds Semon controller and
push executables plus the complete integrity-locked official Codex 0.160.0 Linux
package. It never publishes the artifact or deploys code. The private SDK worker
requires exact 2.52.0 and carries credentials only over protected stdin/files.
The provider key stays outside the guest. Fixed guest paths are `/workspace/project`
and `/var/lib/semon/session/home`; create uses base/provider-default resources
and a bounded 900-second VM deadline acknowledged by Hub before creation.

The guest uses #301's namespace/syscall-confined native executor and root-only
Unix controller socket. It starts one native thread and retains native identities
through reconnect and explicit idle credential renewal. Renewal restarts the same
app-server/provider/home and resumes the same thread; no exec/resume alternative
or uncertain input replay is introduced. Shutdown proof and renewal receipts allow
inspection after lost replies. End stops the native process but preserves compute,
workspace and runtime identity; provider deadline expiration still deletes the VM.

API keys and third-party SIWC bearer tokens are mutually selected billing sources.
The latter use the documented ACCESS_TOKEN model provider, never first-party IDs
or internal token login interfaces. The Hub owns registered-client OAuth custody,
rotating-refresh serialization, authority and generation checks. Identity consent,
commercial eligibility and completed inference are separate gates.

Approval, file approval, question-answer and interrupt writes remain disabled for
managed guests with an explicit reason, even after local namespace preflight.
Real E2B process-broker, credential/socket and control-route boundary qualification
is required before changing that policy. Namespace failure refuses launch.
Existing terminal attachment, checkpoints, reconstruction, queues, automatic
lifecycle, additional providers and retained guest-file recovery remain separate.

Offline worker tests use synthetic SDK replies plus a real local Unix bridge;
they are not native/provider/inference evidence. No authorized provider/model
bindings or approved third-party client registration were available. Real bounded
VM provisioning, completed inference for each method, reconnect/controller restart,
stale authority, renewal/revocation and explicit provider cleanup remain release
gates. No production flag or public remote control was activated.

## Opaque personal ChatGPT custody

The private managed transport accepts `chatgpt_device_code` as an explicit method.
The coordinator owns durable writer/generation claims before delivering the
bounded official `auth.json` string; the native launcher installs it in a fresh
0600 model file outside executor mounts and pins file credential storage. API-key
and personal-device billing cannot be mixed. Native refresh remains in the guest.
Confirmed shutdown stops the native writer before returning its updated opaque
artifact in a dedicated private response field, separate from snapshots and
transcripts. Retry reads the protected shutdown evidence and does not launch or
refresh again. The coordinator must authenticate and encrypt the return with
owner/generation fencing; lost compute never permits stale token replay.

Offline synthetic fixtures exercise shape refusal, private delivery, permissions,
links and SDK transport. Actual account entitlement and live provider turns remain
owner-authorized qualification, separate from these tests.
