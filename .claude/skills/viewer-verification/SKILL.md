---
name: viewer-verification
description: Verify Semon viewer changes using existing fixture servers, functional suites and pixel comparisons without weakening required coverage.
---

# Viewer verification

1. Read ui/README.md, tests/ui/README.md, tests/ui/lib.mjs and
   .github/workflows/ui.yml for source ownership, generated assets, fixture servers,
   environment variables and the aggregate job. Select affected suites.
   With Node 22+, install locked ui tooling using `npm --prefix ui ci --no-audit
   --no-fund`; run typecheck, test, check:bundle and sizes as in the foundation lane.
   Edit ui/src/ sources and deliberately regenerate with `npm --prefix ui run build`;
   never hand-edit generated bundles or use regeneration to hide a failing check.
2. Install locked tests/ui tooling with npm ci and the workflow's Chromium setup.
   Use its test-clock binaries and synthetic fixtures; do not serve personal logs.
3. Run applicable functional, touch, transport and visual checks. Record selected
   coverage and fixture cases; infrastructure failure is a failed check.
4. Review changed screenshots before updating affected baseline entries. Preserve
   pixel thresholds and required assertions; historical sample reports are distinct.
5. Report command/setup, results, reviewed baselines and tests not run. Do not claim
   the full aggregate gate passed after running only one suite.

Pixel rendering depends on the documented fontconfig/browser setup. Use the
checked-in configuration rather than changing the host's fonts or system settings.
