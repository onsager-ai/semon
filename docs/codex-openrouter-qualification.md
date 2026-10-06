# Standalone Codex/OpenRouter diagnosis — #307 / #303

2026-10-05. Base `52d177c26fdbc2a9cffa77a06276e09482800b67`.
Native source `rust-v0.160.0` at
`a956835d020762cb2b570053af06f643a11c0ecc`; complete pinned npm
`@openai/codex@0.160.0` Linux x86_64 package. Hashes and sanitized evidence:
[`standalone/manifest.json`](../tests/fixtures/compatibility/codex-0.160.0/standalone/manifest.json).

## Diagnosed cause

Native `models-manager/src/manager.rs::find_model_by_namespaced_suffix` maps
`openai/gpt-6-luna` to bundled `gpt-6-luna` metadata. That metadata selects
`tool_mode=code_mode_only`, `multi_agent_version=v2` and `use_responses_lite=true`.
`core/src/tools/mod.rs::requested_tool_mode` gives the model selector precedence
over disabled feature flags. Disabling `multi_agent` disables v1; model-selected
v2 still reaches `core/src/session/multi_agents.rs::usage_hint_text`, then
`prompts/src/multi_agent_instructions.rs` composes the `functions.exec` guidance.
These are native-generated instructions, not evidence that the outer coding
conversation was copied. The bundled model instruction template also contains
`functions.exec` guidance. No personal home/config was used.

The unmodified base was reproduced with a separate helper build and fresh
operation `30700000000000000000000000000010`. Actual outbound input included
`additional_tools` with custom code-mode `exec`; native rejected its call with
`code-mode host is disabled`. The synthetic provider's final message reports that
failure. Shell execution did not occur. The native process was stopped and reaped.

The isolated launcher exports the installed binary's bundled catalog offline
and adapts only tool/multi-agent capability selectors. It preserves all native
model instruction bundles. It pins this catalog through a CLI override in its fresh owner home, keeps agents disabled,
and retains all existing code-mode disabling flags. Known models keep their
instructions, limits/reasoning and shell/patch shapes. The private session profile
records native/effective catalog hashes and capability overrides. Live catalog
changes cannot override this profile. See the [catalog provenance](../crates/semon-control/src/native/README.md).
The native request uses standard Responses `tools`, rather than the bundled
Responses-lite `additional_tools` format. User-supplied config still accepts only
model/provider settings. Credential restart uses the same pinned command/home.

## What passed

The earlier fallback-profile evidence used fresh operation
`30700000000000000000000000000011`. The current capability-only profile used
fresh operation `31600000000000000000000000000023` and is verified by the native
probe, which checks that outbound requests preserve the installed model's exact base instructions. Both use actual
LocalSession, native Codex and its confined executor. A synthetic localhost
Responses provider emitted the advertised direct `exec_command` call. Native
executed a shell check, returned exit 0 and `SEMON_EXECUTED_307:42`, then persisted
the exact final assistant output. The fixture retains the actual function schema.

A private synthetic credential arrived through helper stdin, reached only the
model transport Authorization header, and was absent from outbound prompts,
shell environment, helper output and all generated fixture files. The shell also
checked that the owner config was not mounted. Duplicate delivery of the stable
operation read its saved receipt; exactly one native tool call occurred. Explicit
`LocalSession::end` waited for the native process; a subsequent /proc executable
identity check confirmed that neither the native controller nor executor remained. The reconciliation fixture
records all preceding fresh probe operations, their retained dispatch receipts,
terminal evidence and observed absence of their native listener processes. None
was replayed, including operations whose probe assertion failed after dispatch.

The command used the existing on-request approval path for one exact, bounded
synthetic command, inside the unchanged outer namespace/seccomp confinement.
Default inner native read-only sandbox execution separately failed with
`bwrap: loopback: Failed to look up lo: Operation not permitted`; its socket setup
conflicts with outer socket denial. That operation was retained and not replayed.
This does not qualify managed-guest approvals, which remain disabled. No sandbox
or network policy was relaxed to bypass the failure.

Local checks passed:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --locked -- --no-deps -D warnings`
- `cargo test --locked`
- Standalone native baseline/corrected probes and four offline provenance/evidence tests.
- Existing `codex-local.py` product probe: approvals, terminal competition, steering,
  questions, patch denial, lost HTTP result, reconnect/restart and escalated executor
  isolation. Host broker baseline escaped as expected; native executor blocked owner
  sockets, config/home/proc/symlinks/environment, io_uring and datagram-pair access.
  Browser and tmux cases were unavailable in this environment; no new UI claim.
- Real isolated OpenSSH receipt suite: 8 protocol tests and 1 transport integration.
  Synthetic private driver only; not native SSH launch or combined push/readback.
- Managed E2B SDK boundary tests and existing native control fixture tests.

## Reproduce

Use working Linux x86_64 bubblewrap namespaces/seccomp, a complete pinned npm
package, and the probe venv from [local qualification](codex-control-qualification.md).
Run on a fresh disposable home/workspace. Retain state after any interruption.
Choose and persist a fresh 32-hex operation identity before running; keep it stable
within the operation. Never run the same interrupted operation again.

```sh
cargo build --locked -p semon-control --example standalone-qualification
/path/to/probe/bin/python tests/spikes/codex-standalone.py \
  --codex "$NATIVE" --driver target/debug/examples/standalone-qualification \
  --operation "$FRESH_OPERATION" --output /path/to/sanitized-evidence.json
python3 -m unittest discover -s tests/spikes -p test_standalone_fixtures.py -v
```

For the before case, build the **same helper source** in an isolated checkout of
`52d177c` with a **different CARGO_TARGET_DIR**, then pass that helper to the
probe with `--baseline` and a distinct fresh operation identity. Do not share
compiled helper artifacts between base and corrected checkouts.

## Remaining adoption gates

**Live OpenRouter execution is unqualified.** The previous issue evidence proved
live model access only; this change proves native execution with a synthetic
provider. This environment has no configured authorized private model source.
Do not copy issue credentials or infer a model/provider pass from the synthetic
provider's forced tool output.

**Native SSH adoption is unqualified.** No authorized native SSH target, scoped
installation/launch authority or combined Hub fixture was supplied here. The
existing receipt broker transports to an already qualified private driver; it
still cannot install or launch Codex. Qualification must bind version/integrity,
workspace/home/thread, bounded controller lifetime, real receipts and native
reconnect/restart/lost-result/Unknown recovery without replay, duplicate and writer
exclusion, idle credential repair, permanent revocation and unavailable-server
shutdown obligations. It must combine dispatch with genuine session push and Hub
readback. Inner sandbox/approval mapping is tracked separately in
[#309](https://github.com/onsager-ai/semon/issues/309) and must pass before guest
command approvals can be enabled.

Hub main `d8da5c4c51e9ef8e27ddbcaecb0097d751a4d4f0` still contains
`ssh_enrollment_has_no_execution_actions_or_compute_rights`, and its SSH router
exposes enrollment/mirroring only. No Hub execution action was enabled or changed.
Keep [#303](https://github.com/onsager-ai/semon/issues/303),
[#307](https://github.com/onsager-ai/semon/issues/307), and
[Hub #102](https://github.com/onsager-ai/semon-hub/issues/102) open until their
native and hosted evidence passes. This work does not authorize a user's server,
claim live E2B qualification, deploy a service or change normal Codex configuration.
