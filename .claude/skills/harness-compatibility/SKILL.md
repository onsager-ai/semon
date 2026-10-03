---
name: harness-compatibility
description: Verify Semon capture, normalization or control changes for Claude or Codex against real-format fixtures and the carrier-neutral storage contract.
---

# Harness compatibility

1. Identify the affected capture, control, session or relay module and the input
   formats it supports. Read its existing fixture tests before changing behavior.
2. Establish a failing case with a minimal sanitized real-format fixture. Preserve
   session/provenance relationships and test both accepted and refused inputs.
3. Verify unchanged semantics retain identity and ordinary trace reads still exclude
   raw carrier bytes/labels. Read the relevant storage design when that boundary changes.
4. Run the affected crate tests, then the root Rust gates. Use the owning workflow
   for additional supported-feature coverage; do not substitute invented commands.
5. Report fixture origin, changed compatibility assumptions, checks and blocked cases.

Do not upload raw personal session logs as fixtures. No new harness/control promise
is established merely by a successful parser unit test.
