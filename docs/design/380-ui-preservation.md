# #380 UI preservation evidence

The selected visual reference is the actual complete `compat=1` application at
Source `263a02da278716ab2309b8e34a463419e5c781e5`, captured with Hub
`503fbe090749ea1d58b9ca9fa4ccae7289ee426e`. Hub main's design-assets merge
`e15596cdeff9a2c5c9f9431cd52932603c41e22e` records those browser captures and
source-derived DOM. Pen reconstruction exports are not pixel authority.
The approved feedback-20261015-v6 redesign remains separate work after #380.

## Acceptance contract repair — 2026-10-17

This section records the earlier adapter repair. The continuation policy below
supersedes keeping that renderer: obsolete executable mockups are now removed,
their data is preserved as verified JSON, and their complete frame/region set is
compared directly with the pinned application at unchanged strict thresholds.

The user authorized fixing the acceptance baseline before further product UI
alignment. This section supersedes the diagnostic-only and unchanged-adapter
descriptions below; their runs remain historical evidence.

The old adapter added a Composer action, an extra Sessions breadcrumb, mobile
lineage/state-lead changes and a leading Cached history row. Those additions
conflicted with the selected complete application's source and actual captures.
Remove those additions while preserving the independent renderer, fixture-unit
adaptation, original reference HTML, screen/region coverage, ratchets and
thresholds. No candidate screenshot replaces a golden. No product UI changes.

`preservation-contract.mjs` records the fixed Source/Hub reference pair and 62
required viewport/theme/state keys. The actual-application comparison is now
required on all PR, main and manual visual runs, not only PR #438. Its candidate
fails for pixel/size differences, missing/duplicate/unknown captures, invalid
measurements, altered provenance/thresholds or execution errors. It includes the
previous supplemental Thinking/tool-group/tool-output and completed-child cases.
Only the exact pinned `eb535ff` before-run remains historical diagnostics; it
still fails for invalid coverage/execution and reports all visual differences.

There are no new pixel masks or implicit exceptions for paused controls, native
scope/counts or cached/incomplete notices. Candidate differences need an actual
fix or a separately evidenced, explicit presentation decision. The new gate is
not a claim that the current product matches the reference: recomputing all 62
PNG pairs from run `39012188899` retains 14 exact states and 48 failed states.
The reference compared with itself passes all 62 cases at unchanged thresholds.
Hub's existing 48-state goldens and its 16 failures remain untouched.

Validation for this repair is recorded on its follow-up PR. Cloud review inspected
the actual reference/candidate screenshots. This does not waive the prior local
thermal restriction, qualify a new rendered candidate or satisfy release gates.

## Preserved presentation

- The canonical Sessions reader reuses the complete application's two-line rows,
  harness/delegation marks, duration, loaded child counts, four grouping controls
  and existing Filter/Select sheet. Grouping reads no additional data. The earlier
  native-only collapsed child list and inline filter form are not the selected
  complete presentation. Every loaded child now remains individually visible.
- The shared filter dismissal defers its URL commit until the existing overlay
  history transaction completes. The native search form retains tablet touch
  sizing, and native transcripts use the selected phone 2px / desktop 8px page inset. Routine cached
  qualification follows the records; stale/incomplete/loading/error notices stay
  ahead of them. Qualification stays visible without shifting the normal opening.
- Recent restores duration and machine/repository metadata, recorded state labels,
  failed-descendant flags and the selected ancestry in rail mode. Child activity
  dots are reserved for the collapsed rail; expanded rows retain their descendant
  flag without shifting the age text. The All action counts nested descendants. Recorded status
  does not grant control authority. Home retains the original compact ages,
  plain-text previews and source-qualified parent names.
- Session headers use the selected complete application's responsive lineage:
  desktop ancestors, no extra Sessions breadcrumb, and no mobile ancestor text
  competing with the name. Sessions navigation retains the reader's existing
  filter/focus/scroll return transaction through native links and the shared drawer.
  Child cards retain Run view as a passive native Trace link when the recorded
  relationship identifies its sending turn; it never invokes runtime control.
  The selected session footer retains fenced recorded status, calls, duration and
  own-session cost. Completed child footers retain their recorded return status
  and native Open in parent action when loaded handoff context identifies the
  parent. No parent turn is invented when it is absent from that bounded context.
  Passive facts are not treated as paused control actions.
  Native turn anchors clear the measured fixed phone bar, including when the
  restored footer makes a short session scrollable.
- Repeated Find publication keeps the focused account widget in its host's Find
  row. Shared chrome does not reparent that widget into the ordinary toolbar.

Fonts, theme tokens, reading size, shell widths, buttons, Thinking disclosure,
tool grouping/expansion, Trace and native Hub layouts retain their owners. No v6
button, drawer, Trace-removal or transcript redesign is implemented.

## Data and pause boundaries

List pages remain 60 rows, retaining at most 300 per source. Grouping and filter
choices use loaded metadata; the source-qualified Machine field cannot imply a
workspace union. Header counts say "sessions loaded" unless one accepted,
complete, unpaged retained-history page proves the exact loaded key set for
the same source, generation and filters. Search also requires a complete native
search receipt. That bounded result may say "sessions"; it is never a workspace
census. Partial discovery, cursors, failed or ignored queries and retained extra
keys keep the loaded qualification. Native discovery,
incomplete/stale generations, retry, cursors, selected transcripts, field expansion,
custody and request cancellation retain their existing implementations.

The user-approved temporary collection pause still removes Composer and New
Session actions, active runtime/control/SSH setup and broad full-text retrieval.
Saved credentials, metadata, operations, ordinary collection and selected reading
remain intact. Loaded-text navigation is independent of broad full-text search.
No paused action is restored to satisfy a visual comparison.

The reference's unavailable-controls disclosure is preserved as a passive
saved-reading leaf. Its button has an explicit accessible name and opens only
an availability explanation, with no control owner, draft, form, delivery or
reconnect action. The original inert Claude external-menu row is retained as an
explicitly unavailable menu item with the shared tooltip; no URL or action is
invented. The Recent caption retains the original text while the existing
machine scope remains in its tooltip and accessible tree description. These
changes do not grant a workspace union or approve any remaining pixel mismatch.

Anchored navigation disables the initial follow-tail pin before rendering its
generation-bound range. Its ordinary Home check keeps the original timing and
geometry assertion without layout-forcing diagnostics; a separate controlled
range/frame test checks that queued bottom pins cannot override the turn anchor.
The earlier failed and instrumented captures remain evidence, not inherited
acceptance. Trace token presentation sums emitted million components as integer
tokens and rounds one-decimal million totals at the display boundary; native
accounting and the shared formatter's other consumers remain unchanged. Real
Trace-row tests cover joined sessions and passive peers. Final exact-head CI and
rendered comparison still determine acceptance.

## Fresh before result and classification

[Run 38989080847](https://github.com/onsager-ai/semon/actions/runs/38989080847)
reran the unchanged `eb535ff8920fe9aa54e78babc9649cf6c9400ad6` candidate:
**69/162 strict regions** and **8/113 whole-frame ratchets** fail, with zero page
errors. This replaces the historical 77/113 figure as a current measurement.
Every failing strict region is a session topbar.

[Run 38989176278](https://github.com/onsager-ai/semon/actions/runs/38989176278)
adds diagnostic DOM evidence without changing production bytes, references or
tolerances. In the representative harbor desktop header, the missing paused
Composer is 98.921875px wide in the mockup reference; the title grows from
765.078125 to 864px. The remaining action positions and metadata match. At 390px,
the reference Composer is 103.84375px wide, compressing the reference title and
breadcrumb. This repeated shared control removal explains the historical topbar
failure family; it is an approved pause difference, not a reason to reactivate it.

The mockup adapter also inserts a Sessions breadcrumb and phone lineage that the
selected complete application does not render (`sessionChrome.ts` at `263a02d`).
Restoring the selected application can therefore differ from that adapter even
when it agrees with the authoritative application. That reference conflict is
reported separately. No mockup HTML, adapter, golden, mismatch threshold or state
manifest is replaced, loosened or deleted.

## Reproducible actual-application comparison

On #438, the existing UI workflow builds exact binaries for the proposed source,
the before candidate `eb535ff`, and selected complete reference `263a02d`, in
addition to its existing refresh-performance baseline. `preservation.mjs` runs the
same native fixtures, fixed clock, locked browser/fonts, widths 390/820/1280 and
both themes against both applications. It retains 38 unmasked pairs, diffs and
source-readable geometry/text/icon snapshots for Home, Sessions, three transcript
shapes, Analytics and the phone drawer. The before binary runs the same protocol
in the same job. Both readers receive the same wheel/Home input to cancel initial
"follow latest" positioning, then must settle at the transcript head before capture.
Earlier diagnostic transcript frames with a renewed tail-follow transaction are
not evidence of matching scroll states. Execution errors and missing coverage fail; pixel differences
remain explicit diagnostics at the unchanged 0.1/0.0001 thresholds. The existing
strict regions, full-frame ratchets and every required suite remain independent
gates. Diagnostics do not turn a red gate green.

Remaining differences must be classified against that actual reference:

- Approved pause differences: omitted Composer/New Session and inactive runtime,
  SSH and control actions, including their occupied space and runtime-only detail.
- Native read qualification: visible cached/stale/incomplete history, bounded loaded
  counts, source-specific options and scope. These preserve the non-UI contract;
  their layout differences are reported separately from approved pause removals.
- Recorded data/presentation differences: activity wording, duration precision and
  review-aware visible child counts. These are not automatically approved pixel
  exceptions. Read the retained DOM/text evidence before attributing a mismatch.
- Mockup-only divergence: extra Sessions/mobile lineage in the adapter, contrasted
  with the authoritative complete application. Its injected leading cached-history
  row also shifts transcript crops after the normal notice is moved below records.
  The unchanged gate remains red.
- Capture or execution failures: for example a missing menu or unsettled scroll;
  these do not count as successful comparison and must be retained or rerun.

Shared keyboard, touch, focus, scroll, transcript, tools, Thinking, native routes,
generation and security suites remain required. Hub pins the exact Source commit,
regenerates its consumer with that source and independently checks its offline
candidate on the already-admitted PR image owner. This adds no publication,
deployment, provider credential or workflow admission authority.

The task's thermal rule prohibits local builds/tests/typechecks, browsers,
screenshots and image analysis. Bundles and all execution/capture evidence are
prepared in CI. Bounded source/DOM report inspection and single-file formatting
are local; screenshot visual inspection remains an explicit review limitation.
CI artifact preparation never bypasses the stale-asset failure: regenerated bytes
must be committed and the normal freshness gate must pass on a later exact head.

Final-head results, exact pair and remaining failures are linked from Source #438,
Hub #268 and the #380 acceptance discussion. Candidate UI evidence does not satisfy
trusted-main, published-image, staging or final release qualification. No merge,
deployment or issue closure is part of this work.

## Continuation and actual-reference policy — 2026-10-17

The user explicitly selected actual-application acceptance while retaining historical comparison evidence, then authorized removing the obsolete mockup renderer/adapter after preserving its fixture data and full capture coverage. The selected reference commits and thresholds above are unchanged. Legacy run artifacts remain history; their reconstruction is not another product implementation to preserve. Replacement checks must cover all retained frames and regions, reject missing captures and page errors, and require individual approval for remaining pause/native differences. This policy does not waive the current differences or grant visual acceptance.

The continuation fixes three reading defects: session navigation restores heading focus without moving scroll or stealing subsequent reader focus; the producer carries its unambiguous recorded sending turn into Open in parent; and anchor range reads retain the server's projection generation, with one re-resolution of that same turn on a proven generation conflict. An anchored range replaces accepted state only while its navigation epoch remains current. Superseded responses cannot scroll or overwrite a later selection.

Compact child activity reuses the existing bounded relationship preview owner. Metadata carries only tool identity/time; a selected generation-bound `session-home` request loads the capped argument through its native recipe. `session_fields` narrows that existing read to activity and avoids unrelated prompt reads. Current Working observations alone can show it. Retained-source publication drops a mismatched activity recipe instead of joining old arguments to newer runtime facts. No broad transcript scan, new polling owner, control action or SSH capability is added. Tests and exact-pair CI must validate these changes before they are accepted.

The first native replay check exposed a producer mismatch: a worker-only staged publication had no physical parent file and lost the recorded sending turn that a complete publication retained. The bounded relationship context now preserves that witness, distinguishes unavailable or ambiguous physical observations from an unobserved foreign parent, and rejects conflicting witnesses. Old active stages restart through the existing bounded reset recipe. Catalog version 5 starts existing discovery/drain recovery before resuming an old subset stage, retains published history, and prevents a small component from advertising completeness for untouched older rows. Version 5 also carries the recorded latest-prompt sender alongside its existing bounded prompt recipe; session ancestry alone is not sender proof. This upgrades derived metadata without resetting the event cache or inventing a holding turn. The failed Rust run and the initial browser-test selector failure remain recorded; corrected final-head CI is still required.

The pause boundaries authorize keeping those features inactive. They do not, by themselves, approve every resulting pixel difference. Any remaining actual-application visual difference still needs the explicit evidence-bound approval required by the policy above.

Expanded actual-application machine captures exposed a separate presentation regression: a Machine-only CSS variant changed phone padding/gaps and row geometry, suppressed the original detail while activity was present, moved ages into every row, and substituted harness/machine text for recorded ancestry. Machine pages now reuse the selected complete application's shared row and page styles, working-only harness/status placement, recorded parent names and bounded native prompt detail. The visually hidden machine heading is restored. The existing served tests verify those original phone/desktop dimensions and row semantics. These changes preserve native metadata qualification and do not adopt the deferred v6 layout.

### Recorded reading restoration — 2026-10-17 continuation

The active actual-application gate retains all 113 historical frame identities and
162 strict regions. Its additional complete reading protocol retains 62 states
at phone, tablet and desktop widths in both themes. Historical adapter results,
reference identities and thresholds remain evidence; the obsolete mockup renderer
and adapter are removed under the user's explicit bounded-removal decision. The
data-only fixtures retain their exact contents, verified against the original
extractor in CI. Empty approval registries do not approve today's differences.

The selected-session footer now follows the original recorded role and child
return rules. Cost menus retain known child subtotals, unknown incomplete totals,
last-activity ordering, nested run depth and the original reported-cost wording.
Family status uses the same Catalog source/runtime/PID/facts/native-selection
qualification as the Sessions list. Its private published inputs count within
the unchanged family byte limits and are stripped before serialization. Retained
Working/Needs-you observations cannot advertise current activity. Individual
source stamps are checked during the read; this does not claim an atomic
filesystem snapshot after those checks.

Trace restores root-reachable counts, original relationship wording, icons, kind
chips, event timestamps, outcome labels and chart money precision. Missing native
move endpoints remain unavailable rather than asserting a move between fabricated
machine names. Viewing a result item in an accepted bounded Trace uses the same
source-qualified, 2,000-entry local receipt owner as selected transcripts and
Home; it grants no delivery or runtime authority. Only reachable spawn/relay
turns contribute receipts. The cached-history qualification occupies the original
bottom dock instead of extending the transcript's content region. It remains
passive text; no controller or Composer is reinstated.

Candidate Trace response diagnostics hash the exact already-received response
and retain bounded graph/identity structure, excluding transcript/tool bodies.
They established that the atlas-ingest branch emits its Harbor relay and a
qualified participant identity, while its joined turn is unavailable. A separate
allowlisted participant summary now reuses that same generation-qualified Catalog
read for recorded model, token and cost totals. The navigable peer row explicitly
states that no joined turn is available. It cannot contribute invented activity,
duration, timeline segments, waits or critical edges, and stays outside joined-turn
traversal and activity calculations. As in the selected application, unique
qualified participants still contribute to the agent count and session-cost total;
those totals do not assert a receiving turn. Unknown or unpriced costs remain
unknown. The unchanged
128-participant and shared decoded-metadata input bounds remain; those input
bounds do not claim a serialized-response byte cap.

Trace relationship names return to the selected application's static prose.
Turning them into touch-sized buttons had enlarged phone line boxes and changed
wrapping despite identical recorded text. Agent rows and explicit Open in controls
retain navigation. Session menus also remove a duplicate machine chooser and a
new Directory detail absent from the selected application; the topbar chooser
remains. The original inert Open in claude.ai row is preserved with an explicit
unavailable description and no external URL or host action. That restoration
supersedes the earlier omit-row policy question; no exception is required.

A capture key can change in Node before browser navigation dispatches. An old
Quill Trace poll started in that handoff, was canceled by navigation, and was
misattributed to Sentinel. Request ownership now requires an exact completed
successful prior query during that narrow transition, and records its proof ID
alongside the raw failure. Expected-current, unmatched and post-transition errors
remain fatal. This does not introduce a general canceled-request allowance.

The first instrumented replay completed 113 captures without errors and retained
the scrolling transaction that can move a selected step out of view: bounded
child hydration redraws called scrollEnd while follow state still reflected an
earlier position. The diagnostics preserve native scroll writes and frame
positions without changing capture waits, thresholds or visibility assertions.
Final exact-pair checks still determine acceptance of the resulting owner fix.

CI failures for newly added test formatting, Trace union narrowing and missing
required fixture booleans remain retained and corrected; their failed runs are
not inherited as acceptance. The current evidence packet distinguishes prepared
changes, supporting branch runs, PR heads and tested merge-preview identities.

Recent child selection follows the selected application's three passes: retain up
to eight waiting/running children, include the selected or expanded ancestor path,
then fill an ordinary preview to three rows. A one-machine Home publication and
that same selected machine share the document's ordering scope, so a newly listed
selected child uses the existing required-path insertion. A fresh scope keeps its
original status/last-activity ordering. Multi-machine source changes retain their
scope boundary. Canonical navigation identity resets forced ancestor opening;
keyboard collapse stays effective until the next navigation. Expanding a complete
group retains its loaded descendants and ancestor path without new reads.

Trace navigation supplies its canonical selected session to Recent even when no
transcript is active. The original eight-root cap is applied after required-path
ordering; a selected root outside that cap does not replace another root. These
restore the selected application's document order, rather than creating a new
selection-priority policy.

Bounded captured DOM also identified two reading geometry regressions. The catalog
header added an empty Composer host slot despite its controller always being
unavailable; its flex gap hid the tablet branch label. Removing that empty slot
preserves the shared optional-controller API and all actual header actions. The
transcript retained its original clip CSS but had lost the sticky-dock clip
measurement. Restoring the original scroll/resize observation keeps content from
painting beneath the dock without changing scroll height, anchors or transcript
identity. Both corrections still require rendered exact-head comparison.

The served overview regression drains its owned asynchronous Playwright routes
before navigating Home. Individual route removal did not wait for in-flight
handlers and produced a double-handled response during navigation. The correction
uses the locked browser tool's waiting teardown without changing application
timers, assertions or the captured reference.
