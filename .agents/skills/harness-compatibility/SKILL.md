---
name: harness-compatibility
description: Verify Semon's native session discovery, synchronization and control against sanitized harness fixtures, identity, provenance and source-backed read contracts. This tests product integrations, not the coding agent's tool adapter.
---

# Semon harness compatibility

## Scope

Owns Semon's supported native source/control/session formats and source
evidence. AGENTS.md owns native identity, provenance, custody and Rust gates. This skill does not
configure the current coding harness; use harness-operations for tool mapping.

## Prerequisites

Read the affected adapter/module fixtures and applicable storage design. Identify
the supported carrier/version and accepted/refused input behavior. Use sanitized
fixtures rather than personal session logs; preserve useful provenance without
retaining credentials or private raw records.

## Procedure

1. Select the affected native discovery, synchronization, control or session path
   and its real input format. Use sanitized synthetic working copies. Reproduce
   the defect or establish an unchanged control before edits.
2. Add the smallest sanitized real-format fixture that distinguishes intended
   compatibility from malformed/refused input. Preserve parent/session and
   provenance relationships; cover the refusal path as well as the accepted one.
3. Verify native session identity, lineage and source-generation validation.
   Preserve the core Session Event Index/Catalog and source-backed reads. Session reads must preserve authorization and archive custody. Consult the owning design for boundary
   changes rather than making parser fixtures the whole contract.
4. Run affected crate tests and the root Rust gates with exact flags/prerequisites
   from .github/workflows/rust.yml. Include any supported-feature lane affected by
   the change. A parser unit test alone does not establish end-to-end control
   behavior or a new supported harness/version promise.

## Completion

Report carrier/version, sanitized fixture origin, accepted/refused cases, identity
and exposure evidence, actual gate results and blocked coverage. Product harness
compatibility is distinct from native agent instruction/skill discovery.
