# Viewer maintainability validation

This records the #277 implementation's local evidence. Required CI aggregates,
merge approval and a consumer pin to the merged source remain separate gates.
No visual/performance budgets or security checks were relaxed.

## Equivalent bundle measurements

`npm --prefix ui run sizes`, same pinned esbuild/Preact, production target and
raw/minified/gzip-9 definitions at all checkpoints:

| Checkpoint | Raw | Raw gzip-9 | Minified | Minified gzip-9 |
| --- | ---: | ---: | ---: | ---: |
| Before formatting/refactor | 522837 | 123975 | 306014 | 100677 |
| Formatting only | 580692 | 131044 | 306017 | 100679 |
| Architecture changes | 582434 | 130568 | 307701 | 99886 |

Formatting expands served raw whitespace/comments while equivalent minified
output stays effectively constant (+3 bytes). Architecture adds 1742 raw and
1684 minified bytes relative to formatting; gzip shrinks by 476/793 bytes.
The added page component boundary, revision notifications and explicit lifetime
owners account for the small minified increase. No extra runtime/store/router
is included. Consumer measurements use the consumer's own equivalent builds.

## Local checks

- Pinned formatting check/idempotence, strict types, source security,
  deterministic production builds, freshness and bundle sizes pass.
- 34 UI unit tests pass, including staged partial/renamed/deleted/space-containing
  paths, hook configuration preservation, raw/normalized atomic rejection,
  accepted update revisions, session subscriptions and stale/destroyed paging.
- 24 browser lifecycle tests pass across 390/1280 widths: standalone, sidebar,
  native and independent host mounts, teardown/replacement, late requests,
  shell/account/dialog/facet/Recent identity, native controls and cleanup.
- Rust fmt, clippy (all targets, locked, no-deps, warnings denied) and locked
  tests pass. Registry/nav source-contract assertions now support the formatter
  without changing the expected harness values/icons/navigation order.
- All five functional browser groups pass: navigation, transcript, shell,
  interaction and accessibility (including the phone/tablet touch audit).
- Visual checks pass: 113 screens, 159 regions, zero mismatches/page errors;
  pending ratchets remain enforced. No baseline was updated.

Long-session, live-update, ordering, transport and independent consumer gates are
reported with actual completion status in the source/consumer PRs. Local browser
fixtures are synthetic. The source workflow retains every required suite and
its authoritative `viewer browser checks` aggregate.
