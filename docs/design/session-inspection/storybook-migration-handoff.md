# Storybook migration corrections, 2026-10-19

This is implementation and CI preparation, not visual acceptance. No new reference
has been approved, copied into baselines or promoted. No production UI source,
native comparator, threshold, mask or pending ratchet changes are included.
No merge, deployment, Hub pin update, commit or push was performed by this
implementer. Root retains checkpoint/publication ownership.

The takeover began at `13df84ddba2f9dc26eb6b4366a271e67e6fc4f38`.
Root advanced the shared checkout through `fb991b42` to `f4995b82` during this
work; the separate static-server/main.mjs adapter edits are preserved. This
implementer did not edit those protected adapter files.

The separate adapter owner has one prepared follow-up:
`/tmp/semon-storybook-harness-assets-20261019.patch` adds the two exact official
Codex SVG files to Vite's development allowlist without exposing a directory.
The static build imports/emits these assets directly; `no-inline` and same-origin
path normalization preserve the production Recent renderer's URL validation.
Coordinate this development-workbench allowlist patch with root before applying
it to the protected `ui/.storybook/main.mjs` owner. No such edit was made here.

## Reviewable checkpoints

1. Authored stories and isolated type configuration: plural interaction actions,
   action-owned tooltip wait, production `.step.err`, 41-line failed Bash output
   with visible Copy/View all, a real collapsed tool, and production shell/Recent
   renderers with six populated rows. Harness fixtures import the official
   Codex light/dark assets, matching the native harness registry instead of
   substituting the Semon brand mark. The gallery mounts on its actual `.app`
   owner. Phone account actions tap the lead button then `#account-drawer`'s real
   trigger. Desktop/tablet sequences scope the desktop trigger to `#topbar`.
   The lifecycle story owns a scoped DOM event, removes its listener on teardown,
   unmounts while a native modal is open, checks cleanup and reopens/closes a sheet
   after remount. No production global hook is added.
2. Inventory and capture: every exported story has an expected state/assertions;
   every active pen frame has an explicit capture binding and visible ownership.
   Containers share only explicitly named captures with an ownership explanation.
   No descendant inherits coverage. The exact served inventory includes every
   frame/region/scope, strict-preservation key and gallery state, preserving selected
   Semon/Hub provenance. Missing/unknown/duplicate frames, routes, states, targets,
   actions and capture results fail. Actual 334/696px component widths are asserted
   alongside the 390/820/1280 viewport/theme matrix. Existing real-product/native
   suites retain their independent behavioral authority.
3. CI and references: the existing branch bootstrap runs server and plan regression
   tests, authored typecheck, build, contract, capture and separate render/approval
   reports. Build remains independent of the typecheck outcome for evidence.
   The v2 reference map remains empty. Candidate PNGs therefore fail the approval
   gate even when the render-only gate succeeds. Source/capture/image digests and
   exact state/geometry/review provenance are required for a future accepted entry.

## CI-only commands for root

First Sol correction round: historical approved source digests/revisions are now
validated independently of the current candidate source. Exact capture identity,
review metadata, image integrity and strict pixel policy remain required. Capture
reports keep historical `referenceProvenance` separately from current provenance.
The positive synthetic reviewed-record path compares across different source
revisions/digests without mutating the historical record; missing/invalid review
fields, changed state/viewport/theme/policy and altered reference bytes fail.

`storybook-assertions.mjs` now owns cardinality validation and execution, shared
by contract planning and capture. Singular text-filtered assertions require one
match, absence requires zero, and explicit plural counts require distinct expected
row identities/text in DOM order. Every row is checked. The current catalog story
contains one `session-1` row and binds it explicitly; the SessionRow fixture binds
both its real rows. Synthetic six-row tests cover missing, extra, hidden and wrong
rows/text, while a deliberately strict fake locator rejects collection `waitFor`.
Other potentially plural status selectors keep unique text filters; account menu
assertions now scope `.account-popover`. No production code/style, approval map,
baseline or protected adapter file was edited in this correction round. The
existing plan test command below runs these new regressions in CI.

Files changed in this correction round: `tests/ui/storybook-capture.mjs`,
`storybook-references.mjs`, `storybook-reference-map.schema.json`, new
`storybook-assertions.mjs`, `storybook-plan.mjs`, `storybook-plan.test.mjs`,
`storybook-state-bindings.json`, `storybook-contract.json`, `tests/ui/README.md`
and this handoff. Bare filenames in that list are under `tests/ui/`.

The thermal log still ends at `2026-10-19T07:30:38Z`. No local test, parser/audit
script, build, install, browser, screenshot or image processing was run. These
commands are for the existing remote branch workflow or a CI runner with Node 22
and the repository's pinned Chromium/font setup:

```sh
npm --prefix ui ci --no-audit --no-fund
npm --prefix tests/ui ci --no-audit --no-fund
node --test tests/ui/storybook-server.test.mjs tests/ui/storybook-plan.test.mjs
npm --prefix ui run storybook:typecheck
npm --prefix ui run storybook:build
npm --prefix tests/ui run storybook:contract
npm --prefix tests/ui run storybook:capture
node tests/ui/storybook-report.mjs --render-only
node tests/ui/storybook-report.mjs
```

Capture intentionally returns nonzero for pending approvals; CI must still run
the render-only report and always upload `tests/ui/out/storybook` and the static
build. The bootstrap also uploads its authored-formatting patch. Apply reviewed
formatting feedback through root's normal checkpoint process and rerun the exact
head; formatting generated after capture does not change that capture's source
provenance. Authored-source errors remain real failures. The preview-only
`skipLibCheck` exception addresses the upstream optional renderer peer declaration
errors reported by run 39260976777; production strict/declaration checking remains
unchanged, and no React package/stub is added.

Required evidence paths are `storybook/contract-report.json` (all exact state
bindings), `storybook/candidates/capture-report.json` (expected keys, assertions,
source/capture hashes and individual failures/pending approvals), candidate PNGs
and any reference diffs. Source assertions are implementation reasoning, not a
claim that browser execution or image inspection succeeded.

## Required workflow integration after functional capture

Per the takeover instruction, do not remove the temporary bootstrap or activate
the final aggregate integration before functional capture is confirmed by CI.
A complete focused job and required-aggregate update are prepared for root at
`/tmp/semon-storybook-ui-integration-20261019.patch`; the full candidate workflow
is `/tmp/semon-ui-workflow-20261019.yml`. The patch adds the Storybook job to the
existing `.github/workflows/ui.yml` and includes its result in the existing
`viewer browser checks` aggregate. It adds no push workflow and removes no native,
served, foundation or product suite. The job always uploads evidence and keeps
pending-reference failures required. It is intentionally not applied before the
requested functional CI checkpoint.

After the render-only gate passes, root can apply/reconcile that patch, retain
any bootstrap artifact feedback still needed, then remove the temporary workflow
in the coordinated final checkpoint. Run the entire existing Viewer UI aggregate
on the resulting exact head. A passing Storybook render gate cannot substitute
for native checks, actual matched-image inspection or independent Hub acceptance.

No reference/candidate/diff images have been inspected in this takeover because
local browsers, screenshots and image processing are forbidden. Full migration
acceptance remains pending CI execution, actual image review and applicable Git
approval. The four historical Sources navigation frame IDs have explicit
superseded dispositions under the current Sessions source-context ownership in
`ui/README.md`; they are not asserted visible in successor captures. Root descendant
groups inventory all IDs; these individual dispositions are checked separately.
Superseded linked-session boards cite
the governing design brief. No paused capability is restored to satisfy an old
canvas. Hub's separate source-pair/product-page acceptance and reviewed-pin
adoption remain with its consumer delivery; this checkout's native visual
comparison keeps the selected Hub provenance.
