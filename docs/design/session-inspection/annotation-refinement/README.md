# Compact linked-session annotation refinement

Baseline: `defe9d791a197902c5bebcba2e2b7fe87dd9a075`. The user requested linked
sessions that match the existing tool-call density and a Codex/ChatGPT-style
composer with tighter spacing. The authoritative intent is
[DESIGN.md section 10](../../../../.stitch/DESIGN.md#10-annotation-refinement-compact-related-sessions).

[session-inspection.pen](../session-inspection.pen) contains the native reusable
compact row and [collapsed/expanded state board](canvas/pkKQT.png), exported at
scale 2. The old linked-session board is explicitly historical. Harness assets,
neutral text roles, 13px names, 12px metadata, labeled dots and 36px desktop/44px
phone disclosure targets reuse the product foundation. Full task/result/activity
and Run view remain available through expansion; the name still opens the session.

[Before](before/) and [after](after/) compare the same sanitized fixture at
390/1280px, light/dark, DPR 2. Before uses the merged baseline CSS and bundle;
after uses the actual rebuilt test-clock binary, without frontend substitution.
The browser clock is pinned to the fixture. [Observations](observations.json)
record authored/bundle hashes, row heights, keyboard disclosure, direct navigation
and Back. These are compatibility-reader captures: the bounded catalog does not
expose the same complete relationships and gains no invented graph capabilities.

The shared `sh-composer-surface` is opt-in. It joins prompt and toolbar, keeps
native fields and keyboard focus, uses 12px prompt padding, 45px desktop/48px touch
initial height, a 280px scroll cap, 8px bottom padding and 13px visual toolbar
insets. Touch targets stay 44px. The generic/conversation variants keep their
existing geometry. Enhanced dropdown active options use fill without a left rule;
selection still uses a trailing check. Hub owns approval/agent/runtime arrangement.

## Verification and preview

[Validation](validation.json) records selected local checks. The reference change
is confined to linked-row CSS/markup and the equivalent activity projection.
Phone page heights intentionally shrink; only affected size records are refreshed,
with mismatch thresholds and all other baseline entries preserved. Strict
transcript/topbar/menu regions are compared separately. No aggregate CI result or
measured usability gain is inferred from these checks.

Build with `cargo build --locked -p semon-cli --bin semon --features
semon-sessions/test-clock`. Generate a synthetic fixture with
`node tests/ui/fixture.mjs /tmp/semon-ui-sample`, set `SEMON_TEST_NOW` to its printed
clock, and serve with the existing fixture paths documented in
[viewer verification](../../../../tests/ui/README.md). Open the token URL privately
with `compat=1`; never commit the authentication token. This task's running fixture
is on port 8810. `capture.mjs` reads its private log via `SEMON_PREVIEW_LOG` and the
fixture clock via `SEMON_FIXTURE_NOW`; use the existing CI server/environment flow
for a fresh checkout.

Future UI work continues through the pinned ui-design skill: read the authoritative
specification, reuse tokens/components, edit the native canvas when intent changes,
compare representative product states and explain deliberate deviations in the PR.
No parallel instruction platform or shared-skill projection changes are needed.

The full turns/navigation check also passes (134 light/dark screens and four
phone/desktop handoff navigation cases). Remote CI caught a stale content-click
coordinate that now targets the compact row's session-name button. The test now
clicks metadata to validate noninteractive content separately; name navigation
and route preservation assertions are retained. Final-head remote checks remain
separate from selected local validation.

The phone canvas example was subsequently corrected after the user identified
hard-cropped metadata. Its dark pane now has 16px edge padding, deliberate
metadata ellipses and full-width wrapping for disclosed task/result text. The
board grows to its content rather than clipping its lower edge. Native compact
rows/components remain editable; Source product row CSS is unchanged. The shared
finite CSSOM geometry API additionally exposes `minHeight` for consumer-owned
visible-viewport layout, with its existing cleanup and numeric validation.
