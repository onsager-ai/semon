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

The user-requested composer dropdown refinement in `.stitch/DESIGN.md` section 9
is a scoped exception: enhanced composer select triggers/options and their
sheet close control use 40px narrow/coarse-pointer targets with 13px text.
Generic controls, composer toolbar actions and native fallback keep 44px targets.
The compact gallery verifies that density alongside selection, focus and Back;
the generic gallery's touch assertions and visual thresholds remain unchanged.

Native application actions use `btn sh-compact` and `btn-row sh-actions` to match
the viewer's compact density. Primary, danger and quiet states retain their
existing meaning. The visible single-line surface is 32px; touch targets remain
at least 44px, and long labels wrap within the viewport. Compact control links
keep control styling in unvisited and visited states. Full-size controls remain
available for deliberately distinct layouts, rather than being a native-page
default. Hub enforces its compact choice in its authored-markup policy and
rendered native-fallback audit; visual baseline matches alone do not prove parity.

Composer task text uses the 14px UI role on a desktop, rather than the 16px
transcript reading role. Compact buttons, setting labels and enhanced select
values use the 13px secondary role. Native editable fields (textarea and select)
retain 16px on narrow/coarse-pointer devices to avoid browser focus zoom. Text
size does not determine target size: generic composers preserve the 96–280px task area, existing
32px visible toolbar surfaces and at least 44px touch targets. Keep transcript
reading text at 16px. These shared rules belong in `shell.css`, including the
native fallback; consumers must not introduce typography overrides.

Configuration entrances use quiet chips within the existing composer toolbar:
neutral at rest, with a subtle surface/border on hover or expansion and the
existing visible keyboard outline. A small disclosure chevron accompanies a
visible value. Settings use aligned label/value rows and a quiet sunken select
surface; retain native named selects, shared Select/Panel ownership and existing
keyboard behavior. Do not substitute a stack of full-width outlined fields or
invent model/effort choices to make the panel look more sophisticated.

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

Update Hub's `semon` submodule gitlink, its sole Semon pin, to the reviewed source
commit. Check out that exact source and verify it with Hub's
`scripts/verify-source-pair.sh`; Docker consumes the verified checkout. Hub no
longer uses `semon.rev` or fetches Semon inside its Dockerfile. For coordinated
PRs, test the exact Hub/Semon source pair. If the upstream merge changes the
Semon SHA, repin Hub and rerun affected validation. Regenerate its shell consumer
with that commit's locked toolchain. Run static design checks,
consumer freshness, Rust/PostgreSQL checks, page text/touch audits, navigation and
lifecycle checks, fallback parity and production-page visual comparisons. Review
both apps' visual evidence whenever a shared token/control changes. A green source
suite does not replace Hub acceptance. Preserve same-origin forms, no-JS content,
host/chrome ownership, focus and scroll behavior.

The explicitly requested Codex/ChatGPT unified composer variant is documented in
`.stitch/DESIGN.md` section 10. It uses 12px prompt padding, a 45px desktop/48px
native-touch initial height, a 280px scroll cap, an 8px toolbar bottom inset and
13px visual edge insets. Other composer variants retain their existing geometry;
44px touch toolbar targets and native 16px mobile input text remain required.


The approved staging conversation-reader changes are recorded in `.stitch/DESIGN.md`
section 11. They retain category/tool styling and shared tokens while adding
collapsed Thinking, one-line summaries, a lower reading dock and an availability
disclosure for retained drafts. `ui/tests/staging-viewer-browser.mjs` covers
390/820/1280px in both themes, keyboard disclosures, text contrast, viewport
occlusion and composer expansion; the existing galleries remain required.
