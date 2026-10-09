# Catalog recovery and linked-session context

Starting snapshot: `onsager-ai/semon` main `282007f96c60061a9e8539b5c7570909ebeb61fe`.
Refreshed baseline: `e9dba7cac8a7b6f36f936ba77361a9fc134737f8` (native Codex parsing,
subagent titles and filesystem stamp correction, #375). Its instructions and design
artifacts are unchanged. The source candidate includes this update alongside the UI
fixes; compiled before/after evidence uses this latest baseline.
Branch: `codex/ui-continuity-refinement`. The approved authority is
[DESIGN sections 9–10](../../../../.stitch/DESIGN.md), the
[design contract](../../design-contract.md), [viewer ownership](../../viewer-ownership.md),
and [session-inspection.pen](../session-inspection.pen). The annotation-refinement
directory is prior evidence; exploratory Stitch/ImageGen iterations are not targets.

| Reproduced gap | User impact / owner | Implemented correction |
| --- | --- | --- |
| Empty source said no sessions match filters, with no filters active | Implies a filtering problem / Semon catalog | Source-specific empty copy and existing Refresh history action; filtered no-results retains Clear filters |
| Expanded linked card still clipped its name and metadata | Inspection requires navigation away / Semon transcript | Complete wrapped identity in the expanded region, using existing text tokens; summary remains compact |

These follow the approved recovery and on-demand reading intent. No shared palette,
font, control density, relationship capability, navigation ownership, or reader
scale is changed. Failed states and full task/result text remain visible. Expansion
and navigation remain separate actions with keyed state and existing lifetimes.

## Evidence

Each `before/`, `after/` and `diff/` directory uses identical synthetic fixture
content at 390, 820 and 1280 pixels, light/dark, 860 pixel viewport height.

- `compiled-reader-*` and `compiled-expanded-*`: actual compiled CLI applications,
  production viewer bundle, frozen server/browser clocks, compatibility reader
  relationship fixture. No frontend substitution. `capture.mjs` records keyboard
  disclosure and direct child navigation/Back in `compiled-observations.json`.
- `empty-source-*`, `retained-error-*`, `linked-collapsed-*`, `linked-expanded-*`:
  real Preact catalog/transcript consumer with canonical CSS/fonts and deterministic
  host fixtures. These isolated component captures are not compiled server evidence.
  The long failed child fixture includes complete task/result and escaped markup.
- `pixel-review.json`: all matched differences at pixelmatch color threshold 0.1;
  `compare.mjs` creates review diffs without modifying approved baselines or tolerances.

Phone examples: [empty before](before/empty-source-390-light.png),
[empty after](after/empty-source-390-light.png),
[long expanded before](before/linked-expanded-390-light.png),
[long expanded after](after/linked-expanded-390-light.png),
[compiled expanded before](before/compiled-expanded-390-light.png),
[compiled expanded after](after/compiled-expanded-390-light.png).

The reviewed differences are empty-state copy/action and extra wrapped identity
only after expansion. Reader screenshots otherwise preserve the approved scale
and chrome. Component captures intentionally show only component-owned content.

## Validation

Passed: authored formatting, architecture/design checks, TypeScript, UI unit tests,
bundle freshness/sizes, Rust fmt and locked all-target Clippy; 18 catalog/refinement
browser cases; six compact-gallery cases; compiled bounded-catalog browser test;
12 matched compiled reader cases. Existing catalog browser coverage verifies query
submission, Back restoring filters/focus/reading position at phone and desktop.

The six new regression cases cover empty vs filtered vs discovering/indexing/
partial results, retained records during observation failure, keyboard refresh,
44px touch disclosure, wrapped long context, failed/task/result visibility,
distinct child/run navigation, keyed expansion updates and detached controls.
The new suite is included in the existing UI interaction job.

The starting snapshot's `cargo test --locked` was limited by the pre-existing
`viewer::tests::a_refused_archive_root_replaced_by_a_directory_invalidates_the_snapshot`:
528 passed, one failed, four ignored in semon-sessions. It failed twice on this tree
and again on the exact unchanged baseline; isolated rerun passed. Upstream #375
corrects the filesystem stamp and is included by merging current main; the refreshed
full locked Rust suite passes. No assertion or tolerance was weakened in this task.
Remote checks remain tied to the exact published head.

No physical mobile keyboard, real provider/SSH or full released-image qualification
is claimed. Existing approved production baselines were not regenerated.
