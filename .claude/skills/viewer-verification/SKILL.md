---
name: viewer-verification
description: Verify Semon viewer changes with its typed UI sources, generated bundles, synthetic fixture servers, functional/touch/transport suites and reviewed screenshots.
---

# Semon viewer verification

## Scope

Owns Semon's viewer fixtures and coverage selection. ui/README.md owns source,
chrome/host and generation boundaries; tests/ui/README.md and
.github/workflows/ui.yml own exact setup and aggregate semantics. Native coding
agent tools come from harness-operations, not this product verification workflow.

## Prerequisites

Read those references, docs/design/viewer-ownership.md,
docs/design/design-contract.md and tests/ui/lib.mjs.
For refactors, follow the ownership contract and complete its review checklist.
Use the workflow's Node version,
locked dependencies, Chromium/fontconfig and test-clock binaries. Serve synthetic
fixtures, never personal logs. Do not change host fonts to compensate for results.

## Procedure

1. Reconcile the current decisions, affected states and reference disposition in
   the existing issue/PR using the [baseline scope](../../../docs/design/ai-design-workflow.md#baseline-scope-and-review).
   Identify affected screens, interactions and shared components. Select suites
   from the owning workflow and record the fixture cases/viewport/theme coverage.
2. Edit canonical ui/src/ sources, not generated bundles. Run the architecture
   check and verify ownership/dependency changes against the canonical contract.
   Follow ui/README.md's
   build/regeneration and UI foundation commands. Verify type/security checks,
   deterministic bundle freshness and sizes before browser evidence.
3. Prepare the tests/ui fixture servers and tooling using the workflow's exact
   environment/setup. Inspect representative affected states early; then run
   the required final coverage. Run affected functional, touch, transport, accessibility,
   and lifecycle checks; identify unselected suites explicitly.
4. View the uploaded screenshots for each change family (CI makes no pixel
   comparison); classify implementation and fixture defects separately from
   unresolved product choices. Verify interaction/source-scope claims against
   behavior and fixture setup. Preserve thresholds and required assertions.
5. Read the aggregate job's actual result. A single suite pass does not establish
   aggregate success; unavailable infrastructure is blocked execution, not a pass.

## Completion

For refactors, record ownership/dependency changes, checklist evidence and any
justified contract exceptions in the PR. Report selected coverage, setup,
commands/results, inspected image families, classified differences,
source/bundle freshness and unrun/blocked suites. Keep generation commands and
exact CI flags in their owning references rather than copying another toolchain.
