# AI-assisted design in Semon

Use the repository-selected ui-design workflow from `.agents/skills/ui-design/SKILL.md` (Claude's discovery projection is generated). The normal workflow is source-first and requires no pen.dev account, key, application, integration or tool call. Do not add repository MCP files, duplicate vendor skills or personal credentials here.

Current user setup selects `--harness codex`; Claude Code installation, user
configuration and live validation are deferred. The generated Claude discovery
projection remains for future compatibility.

## Source and design ownership

Read the [UI design brief](ui-design-brief.md), current product contracts and settled decisions before UI edits. Implement behavior and layout in the owning production Preact/CSS sources, reusing semantic tokens, components and controllers. The [session inspection canvas](session-inspection/session-inspection.pen) and [delivery evidence](session-inspection/README.md) preserve historical intent and review records; editing or opening a canvas is optional and does not establish acceptance.

Preserve applicable parts of the existing rendered viewer and its approved references unless current requirements justify a change. The product CSS and contracts own fonts, type scale, controls and compact spacing. Design canvases are optional exploration or historical intent and never replace source tokens, production behavior or the active visual acceptance references.

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

For complete migration of the original pen.dev acceptance baselines, use the product-owned [Storybook contract inventory](../../tests/ui/storybook-contract.json). It must account for every original root frame and active descendant, plus the current visual acceptance, visual coverage, gallery and native/product surface contracts. Each active state maps to production UI, a deterministic Storybook story and its required Playwright interactions/captures, or to an explicitly identified served/native test when Storybook is not the correct renderer. Every retired or inactive state needs a governing product decision and reason. Do not call a pilot or partial screen set a complete migration. The brief and settled product contracts determine intent; canvas files and old exports remain historical evidence.

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

## Source, story and capture workflow

Start from current product requirements, contracts and semantic roles in the production CSS/components. Make the smallest deterministic Storybook state using the actual production components and sanitized fixtures. Use the product's locked renderer and Playwright setup to exercise each mapped interaction and capture every required viewport/theme. Inspect the actual reference, candidate and diff images for each distinct change family. Refine production sources when the evidence reveals a defect; do not repair only the fixture or capture input.

Present justified reference changes as clearly unapproved candidates in a reviewable Git change, with original evidence, provenance, reason and thresholds preserved. CI compares and uploads evidence but cannot rewrite, approve or commit references. A human review through the existing product process records acceptance. Story fixtures establish presentation states only; existing native, backend, authorization and runtime gates establish their own behavior. Stitch is retired; its archived outputs remain historical evidence and are not active design or pixel authority.

## Sharing and acceptance

Semon remains the shared visual foundation; it never depends on Hub. Share
semantic roles, component contracts and derived design references through this
existing source boundary. Hub consumes an explicit reviewed Semon pin and owns
domain layout. Do not add a second token package, cross-repo symlinks, auto-sync
design projects or duplicate CSS/token authorities. Independent product pages
can differ while shared controls retain their contract.

Frontend changes use the existing static design policy, typed/security/bundle checks, gallery interaction coverage, text/touch audits and pixel ratchets from [tests/ui/README.md](../../tests/ui/README.md) and `.github/workflows/ui.yml`. The crosswalk lists all required migration states. Capture the declared viewports, themes, focus, keyboard, reduced-motion and lifecycle variants; retain every existing threshold and required suite. Keep generated bundles owned by their build tools. A story/visual pass does not qualify native runtime behavior.
