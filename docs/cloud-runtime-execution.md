# Explicit E2B execution transport (candidate)

`e2b::execution` is a bounded private stdio transport. `scripts/e2b-runtime.py`
uses E2B SDK 2.52.0 and an explicit embedding-supplied profile to create compute, install the embedding's
pinned Codex and Semon binaries, bootstrap Push, dispatch one initial task, and
inspect a retained receipt. The embedding commits the call stage before invoking
it, supplies owner-resolved credentials, checks current authority and generations,
and reconciles using `e2b::reconcile_launch` and complete inventory.

Provider keys never join guest payloads. Model authentication is private data for
Codex's file-backed credential store; it must remain excluded from mirror and
independent recovery artifacts. The guest runs `codex exec --json` with a retained
exclusive supervisor lock, protected local receipt before spawn, prompt on stdin,
and public OpenAI provider selection. It does not enable automatic approval.
A crash between receipt and spawn leaves delivery unknown. Neither this worker
nor the supervisor replays that task. A lost create reply never authorizes create
again. Guest attachment reads the pinned SDK’s raw detail GET (the public info object omits the private guest token), verifies ownership and running state on that response, and supplies the complete keyword-only SDK constructor options. It never uses `connect()` to resume as an inspection side effect. Both execution transports share this path. All execution requests disable SDK retries; connection retries are disabled in the private worker too.

The embedding supplies template, timeout and lifecycle policy; the managed native transport also requires this explicit profile rather than selecting hosted defaults. the
transport validates their shape and uses secure provider access. Hub’s candidate
policy uses base and a one-hour timeout with retained-memory pause. This is a provider deadline, not a qualified application
checkpoint or idle policy. No manual pause/resume, reconstruction, automatic
lifecycle or multi-input inbox is implemented here. These remain the separate
#253–#256 gates. Receipts distinguish a nonzero agent exit from successful completion.
Thread receipts establish launch identity, not arbitrary
exactly-once tool effects or current model health.

`qualify_profile` is an explicit operator test action. It uses the embedding-supplied short timeout in a
throwaway sandbox, verifies pinned binary installation, filesystem-only pause,
explicit reboot and retained bytes, and cleans up only that owned test resource
with no user task. It does not test model authentication or a production mirror
receiver. Hosts must retain its operation identity before retrying a lost result.

Offline evidence: lifecycle/auth/inventory Rust suites plus Python provider
refusal tests. Real provider and both Codex auth methods remain independent
qualification gates. No hosted URL or dependency belongs in this package.
