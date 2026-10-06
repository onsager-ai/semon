# Shared UI design contract

Semon owns the visual tokens, shared component styles and interaction controllers.
Semon Hub consumes them from its pinned Semon submodule. Hub owns authenticated
domain content, authorization, server forms and domain-specific layout. Changes
to a shared control belong in Semon, followed by a reviewed Hub pin update.

`crates/semon-sessions/src/viewer.css` is the canonical token definition;
`shell.css` and `select.css` extend its components. `overhaul.md` explains the
visual principles. Preserve one sans family for content/UI and one mono for code,
12px minimum text, AA text contrast, ink accent, neutral borders, and one state
indicator per place. `--fs-display` is the 26px first-run/public-document heading;
it complements the existing title and figure roles. Phone and coarse-pointer
controls use 44px targets. Product-specific layout does not require pixel-identical
pages, but shared controls keep their appearance and interaction contract.

Native application actions use `btn sh-compact` and `btn-row sh-actions` to match
the viewer's compact density. Primary, danger and quiet states retain their
existing meaning. The visible single-line surface is 32px; touch targets remain
at least 44px, and long labels wrap within the viewport. Compact control links
keep control styling in unvisited and visited states. Full-size controls remain
available for deliberately distinct layouts, rather than being a native-page
default. Hub enforces its compact choice in its authored-markup policy and
rendered native-fallback audit; visual baseline matches alone do not prove parity.

## Enforced checks

- `npm --prefix ui run check:design` parses production CSS. Token references must
  resolve to a stylesheet definition or an explicitly documented runtime geometry
  token. Color/type literals must use existing approved values or role tokens.
  `--faint` text and literal type below 12px fail. Calculated/relative sizes are
  verified by the rendered audit rather than guessed statically.
- `ui/design-policy.json` records calibrated existing literals and exact
  icon/punctuation exceptions. Consumer policies may add exact declarations with
  reasons, not blanket exemptions. Stale exemptions fail. Changes to these files
  are design-contract changes and need a reason in review.
- `tests/ui/text-audit.mjs` is independent of viewer routes and is shared with Hub.
  It audits rendered text, including open menus/dialogs. It skips hidden/disabled
  text and punctuation-only separators. Small text needs 4.5:1; large text needs
  3:1. It does not replace broader accessibility or interaction testing.
- `node tests/ui/gallery.mjs` builds an independent consumer using the pinned
  production toolchain. Production CSS, fonts and controllers cover controls,
  fields/errors, notices, status, long rows, loading, selects, shell/account menus,
  sheets and panels. Thirty visual states cover 390/820/1280px, light/dark;
  keyboard selection, focus return, touch targets and reduced motion are checked.
  The interaction CI lane requires this suite in addition to existing tests.
- Existing viewer pixel ratchets, functional suites and the required aggregate
  remain unchanged. Hub adds its own production-page visual contract and uses
  this same text audit on its page/sheet screenshots.

## Intentional visual changes

Run the normal checks first. Inspect the actual images and diffs before accepting
a changed baseline. `node tests/ui/gallery.mjs --update-baselines` is an explicit
local operation and is forbidden in CI. Review the resulting PNG changes and
state the design reason in the PR. Never increase mismatch tolerances to make an
unexplained change pass. A removed state must be justified, not silently skipped;
missing cases and missing baseline images fail.

## Consumer pin updates

Update Hub's `semon` gitlink and `semon.rev` to the same source commit. Regenerate
its shell consumer with that commit's locked toolchain. Run static design checks,
consumer freshness, Rust/PostgreSQL checks, page text/touch audits, navigation and
lifecycle checks, fallback parity and production-page visual comparisons. Review
both apps' visual evidence whenever a shared token/control changes. A green source
suite does not replace Hub acceptance. Preserve same-origin forms, no-JS content,
host/chrome ownership, focus and scroll behavior.
