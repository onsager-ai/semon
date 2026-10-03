---
name: viewer-verification
description: Verify Semon viewer changes with its typed UI sources, generated bundles, synthetic fixture servers, functional/touch/transport suites and reviewed pixel baselines.
---

# Semon viewer verification

## Scope

Owns Semon's viewer fixtures and coverage selection. ui/README.md owns source,
chrome/host and generation boundaries; tests/ui/README.md and
.github/workflows/ui.yml own exact setup and aggregate semantics. Native coding
agent tools come from harness-operations, not this product verification workflow.

## Prerequisites

Read those references, docs/design/viewer-ownership.md and tests/ui/lib.mjs.
For refactors, follow the ownership contract and complete its review checklist.
Use the workflow's Node version,
locked dependencies, Chromium/fontconfig and test-clock binaries. Serve synthetic
fixtures, never personal logs. Do not change host fonts to compensate for results.

## Procedure

1. Identify affected screens, interactions and shared components. Select suites
   from the owning workflow and record the fixture cases/viewport/theme coverage.
2. Edit canonical ui/src/ sources, not generated bundles. Run the architecture
   check and verify ownership/dependency changes against the canonical contract.
   Follow ui/README.md's
   build/regeneration and UI foundation commands. Verify type/security checks,
   deterministic bundle freshness and sizes before browser evidence.
3. Prepare the tests/ui fixture servers and tooling using the workflow's exact
   environment/setup. Run affected functional, touch, transport, accessibility,
   lifecycle and visual checks; identify unselected suites explicitly.
4. Review screenshots/diffs before updating only justified baseline entries.
   Preserve pixel thresholds and required assertions. A historical sample-mockup
   report is separate from the required served-fixture comparisons.
5. Read the aggregate job's actual result. A single suite pass does not establish
   aggregate success; unavailable infrastructure is blocked execution, not a pass.

## Completion

For refactors, record ownership/dependency changes, checklist evidence and any
justified contract exceptions in the PR. Report selected coverage, setup,
commands/results, reviewed baseline changes,
source/bundle freshness and unrun/blocked suites. Keep generation commands and
exact CI flags in their owning references rather than copying another toolchain.
