# AI-assisted design in Semon

Use the pinned ui-design workflow from `.agents/skills/ui-design/SKILL.md`
(Claude's discovery projection is generated). Optional pen.dev tooling is
configured once at user level by its integration setup reference. Do not add
repository MCP files, duplicate vendor skills or personal credentials here.

Current user setup selects `--harness codex`; Claude Code installation, user
configuration and live validation are deferred. The generated Claude discovery
projection remains for future compatibility.

## Source and design ownership

Read the [UI design brief](ui-design-brief.md) before UI edits, and
reconcile its applicable scope with the current task and settled product decisions.
The editable [session inspection canvas](session-inspection/session-inspection.pen)
records layout and interaction intent; [delivery evidence](session-inspection/README.md)
links historical explorations, actual before/after captures and checks.
Reuse semantic tokens and established components. Update the canvas when layout
or interaction intent changes, compare representative implementation screenshots
at matching widths/themes/states, and explain intentional deviations in the PR.
Review visual diffs before updating baselines. These controls extend this existing
brief and the pinned skill; they do not create another instruction system.

Preserve the applicable parts of the existing rendered viewer as the default
visual baseline. Pen.dev canvases describe workflow intent within that foundation;
they do not replace its fonts, type scale, controls or compact spacing. Calibrate
design artifacts from computed product styles and use actual UI screenshots for
visual review. Document an intentional foundation change separately when the
task calls for one.

Read [the design contract](design-contract.md), [viewer ownership](viewer-ownership.md),
[UI source/build guidance](../../ui/README.md) and
[viewer verification](../../.agents/skills/viewer-verification/SKILL.md).
The frontend is Preact/TSX with esbuild, shared CSS and Rust-embedded generated
bundles. It does not use React/Vite/Tailwind/shadcn. Existing components and
controllers are exposed by `ui/src/lib/index.ts`; UI state and host ownership
stay in their current modules.

`crates/semon-sessions/src/viewer.css` owns visual tokens; shell.css/select.css
extend the shared controls. Preserve Instrument Sans, JetBrains Mono, role-based
light/dark colors, 12px minimum text, AA text contrast, ink accents, neutral borders
and 44px coarse-pointer targets. Native compact controls retain their 32px visible
surface and 44px touch target. The gallery covers controls, fields, error/loading
states, status, menus, sheets and panels.

## Baseline scope and review

Apply the pinned ui-design skill's baseline consistency review in the existing
issue or PR. Record the governing decision/source, affected state, expected
visible behavior and whether its reference is preserved, updated or unresolved.
Historical application versions, PNGs and design canvases retain their recorded
provenance; current requirements determine which parts remain applicable. A
chosen historical version does not reinstate a capability paused by a later
decision. A design canvas records intent; it does not independently override
the repository's visual comparator or shared token authority.

For the agreed collection scope in [#380](https://github.com/onsager-ai/semon/issues/380),
these examples distinguish the required actions:

| Decision | Expected presentation | Review action |
| --- | --- | --- |
| Pause active session initiation | Omit the New Session navigation entry | Correct the affected old reference; preserve collection navigation. |
| Pause cloud runtime setup invitations | Omit the first-run cloud preparation invitation | Correct that copy/layout reference; inspect retained disclosures before making claims about active delivery. |
| Keep compact metadata filters | Do not add routine source/cache/search architecture paragraphs | Correct the implementation; preserve truthful machine scope and meaningful errors. |

These examples explain classification, not approval of particular captures or
delivery of the product changes in this workflow update. An old "All machines"
value is not evidence that a correctly source-scoped candidate should broaden
its reads; check the fixture and actual behavior before attributing the value
difference to a test defect.

View representative actual reference/candidate images and diffs for each cause,
including relevant interaction states, before a full regression cycle. Group
repeated sizes/themes by cause without reducing required coverage. Record the
classification and concrete action; escalate only an unresolved product choice.
Logs and hashes establish provenance, not that someone inspected the interface.
If image review is unavailable, report that limit and continue independent work
without claiming visual acceptance.

Apply settled changes through the owning comparator/baseline procedure, preserving
history, thresholds and required states. Review only justified entries instead of
bulk replacement or altered capture inputs. Run the declared final suites on the
delivered tree. Keep product behavior/availability checks separate from pixel
regression; neither can establish the other's result. No new approval service or
machine-enforced semantic-review gate is introduced here.

## Tool workflow

Start with source extraction, not a new visual identity. Read semantic roles
from the current production CSS and components. A derived design brief or .pen
library records its source commit and both themes, and must be reconciled with
the shared contract and current requirements before reuse.

Use pen.dev for editable canvas refinement, variables, reusable components and
exports. Desktop MCP requires the same-host app; cloud sessions use the supported
headless CLI. Confirm the document, save deliberately and inspect actual exported
images. Implement authorized changes in authored Preact/CSS using existing owners.
Stitch is retired; its archived outputs are historical evidence and cannot become
new design or pixel authority. Do not run its setup, generation or MCP workflows.

## Sharing and acceptance

Semon remains the shared visual foundation; it never depends on Hub. Share
semantic roles, component contracts and derived design references through this
existing source boundary. Hub consumes an explicit reviewed Semon pin and owns
domain layout. Do not add a second token package, cross-repo symlinks, auto-sync
design projects or duplicate CSS/token authorities. Independent product pages
can differ while shared controls retain their contract.

Future frontend changes use the existing static design policy, typed/security/
bundle checks, gallery interaction coverage, text/touch audits and pixel ratchets
from [tests/ui/README.md](../../tests/ui/README.md) and `.github/workflows/ui.yml`.
Compare 390/820/1280px and light/dark with approved references, including focus,
keyboard, reduced motion and relevant lifecycle states. Inspect screenshot diffs
before intentional baseline updates; preserve every threshold and required suite.
Setup itself changes no production UI, generated bundle or approved baseline.
