# AI-assisted design in Semon

Use the pinned ui-design workflow from `.agents/skills/ui-design/SKILL.md`
(Claude's discovery projection is generated). Optional Stitch/pen.dev tooling is
configured once at user level by its integration setup reference. Do not add
repository MCP files, duplicate vendor skills or personal credentials here.

Current user setup selects `--harness codex`; Claude Code installation, user
configuration and live validation are deferred. The generated Claude discovery
projection remains for future compatibility.

## Source and design ownership

Read the authoritative [DESIGN.md](../../.stitch/DESIGN.md) before UI edits.
The editable [session inspection canvas](session-inspection/session-inspection.pen)
records layout and interaction intent; [delivery evidence](session-inspection/README.md)
links the actual Stitch screens, before/after captures and checks.
Reuse semantic tokens and established components. Update the canvas when layout
or interaction intent changes, compare representative implementation screenshots
at matching widths/themes/states, and explain intentional deviations in the PR.
Review visual diffs before updating baselines. These controls extend this existing
brief and the pinned skill; they do not create another instruction system.

Preserve the existing rendered viewer as the default visual baseline. Stitch
explorations and pen.dev canvases describe workflow intent within that foundation;
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

## Tool workflow

Start with source extraction, not a new visual identity. Stitch's
extract-design-md/design-md and pen.dev variables/components can describe the
current tokens. Treat `.stitch/DESIGN.md` or a .pen library as a derived snapshot:
record the Semon source commit, exact token roles/values and both themes, and
reconcile it with the contract before reuse. Keep artifacts in an explicit design
work directory. Synthetic fixtures may be exported to design tools under the
task's scope; real logs and credential-bearing forensic content must stay out.

Use Stitch for alternative layouts/screens and pen.dev for canvas refinement,
variables, reusable components and screenshots. Each harness can use the same
user tooling independently, with separate scratch documents/output paths.
Desktop MCP requires the app on that machine; cloud sessions use pen's Headless
CLI. Confirm the active document and inspect exported images. Implement only
authorized design changes in authored Preact/CSS, preserving existing component
APIs, native links/forms and host/chrome lifecycles. Framework-specific vendor
build examples do not change the production stack.

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
