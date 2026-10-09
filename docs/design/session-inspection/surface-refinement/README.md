# Restrained surface refinement

Baseline: `bcc0a7624e753a818665518837d050af13395bef` on
`codex/compact-composer-typography`. The authoritative intent is
[DESIGN.md section 9](../../../../.stitch/DESIGN.md#9-restrained-surface-refinement).

The user rejected the first Image Gen whole-page reflow. The retained-layout
proposal keeps the existing viewer and reduces repeated informational outlines.
[Image provenance](imagegen.json) labels both original, unscaled tool outputs.
Generated text/state details are illustrative. The implementation keeps the
original navigation and message bubbles and only changes linked-session paint.
Tool aggregation, thinking, agent messages, subagent/handoff actions, configuration
and composer retain their existing owners. No new frontend dependency is added.

[session-inspection.pen](../session-inspection.pen) now contains an editable
linked-session component and [light/dark constrained-width state board](canvas/linked-session-states.png).
Its native document has 10 top-level nodes, 252 named layers, 33 instances,
4 reusable components and 12 variables. [Frames](canvas/frames.json) record scale-2
exports. The state board describes the changed component, not a replacement
whole-viewer layout; exact code spans, state animation and chrome remain source-owned.

[Observations](observations.json) and the 24 actual screenshots compare 390/1280px,
both themes, before/after, the linked-session action focus and expanded failures.
DOM counts and all measured reader block bounds/type sizes are equal across the
two phases. Harbor contains grouped calls, thinking and three linked sessions;
its communication count is zero, so this fixture alone does not validate agent
communication. Existing communication/fan-out visual cases remain required.
All images use device scale 2; no enlargement or resampling is applied.

The capture uses a synthetic compatibility reader and routes only authored CSS.
It does not claim new API relationship capabilities or an adopted Hub source pin.
In this workspace, source [capture-viewer.mjs](capture-viewer.mjs) reproduces the
comparison against the disposable viewer. The operation record
`canvas/refine.pen.js` applies to the baseline canvas; do not replay its insertions
against the final canvas. Future changes edit the existing native component.

The reviewed reference change is limited to the same child-session perimeter and
hover paint in `tests/ui/reference/overhaul.html`. Message layout, visual mismatch
thresholds, required cases and generic touch checks remain unchanged. Composer
dropdown density is a separate user-requested 40px touch exception, documented
in the shared contract and covered by the compact gallery. See validation.json
for actual checks and limits; passing local checks is not aggregate CI approval.
