---
name: semon-evolution
description: Apply Semon's pre-release compatibility policy when evolving its own APIs, protocols, configuration or persisted formats, or reviewing legacy support. Upstream harness format support belongs to harness-compatibility.
---

# Evolve Semon before release

The pre-release compatibility policy in [AGENTS.md](../../../AGENTS.md) owns
this decision: Semon does not maintain compatibility with older Semon versions.
Use one current contract. Do not infer a compatibility requirement from an old
schema, fixture, design document or an existing migration path.

When changing a contract, update the affected Semon-owned producers, consumers,
fixtures and documentation together. Remove superseded implementations within
the task's scope. Do not retain old readers/writers, migration chains, deprecated
aliases or parallel implementations solely for older Semon installations.
If broader cleanup is outside the task, track the remaining work in an issue.
An explicit user requirement for a specific compatibility boundary is an exception;
record its scope rather than extending it to the rest of Semon.

Distinguish old-version support from current correctness. Keep current-format
validation, generation and integrity checks, bounded recovery and refusal of
unsupported or corrupt state. Rebuild derived caches only from available,
authorized source evidence; never treat non-rebuildable facts or retained native
history as disposable caches. Removing a reader is not permission to delete,
overwrite or reset live data. Report unsupported state and the applicable recovery
path without silently converting it into a valid empty history.

Supported Claude, Codex and other upstream harness formats remain product input
contracts. Use [harness-compatibility](../harness-compatibility/SKILL.md) for those
changes. A current row-wise writer fallback or corruption-recovery path is not
obsolete merely because its name contains `legacy` or `migration`; establish
whether it serves a current contract before removing it.

Verify current-contract behavior and meaningful refusal/recovery cases through
the repository's applicable checks, respecting the current host's execution
limits. Historical Semon-version parity is not an acceptance requirement. Report
the changed contract and any unsupported old state; do not add an upgrade matrix
or compatibility-only fixtures. Revisit this policy explicitly for the first
public release rather than introducing a release promise during routine work.
