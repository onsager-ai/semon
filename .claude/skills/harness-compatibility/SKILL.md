---
name: harness-compatibility
description: Verify Semon's Claude/Codex capture and control compatibility against sanitized real-format fixtures and carrier-neutral storage invariants. This tests product integrations, not the coding agent's tool adapter.
---

# Semon harness compatibility

## Scope

Owns Semon's supported capture/control/session/relay formats. AGENTS.md owns
trace/raw separation, identity, retention and Rust gates. This skill does not
configure the current coding harness; use harness-operations for tool mapping.

## Prerequisites

Read the affected adapter/module fixtures and applicable storage design. Identify
the supported carrier/version and accepted/refused input behavior. Use sanitized
fixtures rather than personal session logs; preserve useful provenance without
retaining credentials or private raw records.

## Procedure

1. Select the affected capture, control, session or relay path and its real input
   format. Reproduce the defect or establish an unchanged control before edits.
2. Add the smallest sanitized real-format fixture that distinguishes intended
   compatibility from malformed/refused input. Preserve parent/session and
   occurrence relationships; cover the refusal path as well as the accepted one.
3. Verify unchanged semantics retain canonical identity. Ordinary trace reads must
   still exclude raw bytes/carrier labels; consult the owning design for boundary
   changes rather than making parser fixtures the whole contract.
4. Run affected crate tests and the root Rust gates with exact flags/prerequisites
   from .github/workflows/rust.yml. Include any supported-feature lane affected by
   the change. A parser unit test alone does not establish end-to-end control
   behavior or a new supported harness/version promise.

## Completion

Report carrier/version, sanitized fixture origin, accepted/refused cases, identity
and exposure evidence, actual gate results and blocked coverage. Product capture
compatibility is distinct from native agent instruction/skill discovery.
