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
states that no joined turn is available. An optional `observed_extent` has the
explicit `whole_session` scope, a bounded published `last`, and the same qualified
`status` shape as Catalog relatives. One additional shared runtime/facts/source
capture, beyond the identity lookup's earlier capture, applies the existing
custody, physical source stamp, PID, lifecycle and generation fences to at most
128 peers with at most 64 published source stamps each. The response checks the
runtime generation before and after this work and validates custody before
publication. These additional bounded metadata observations add no body reads or
poller. Only current Working observations extend to the
capture clock; retained or unavailable observations cannot imply live activity.
The row and chart tooltips distinguish this whole-session extent from a receiving
turn. Qualified harness marks, the recorded relay icon and inert chart gridlines
preserve the selected row layout without claiming activity. Peers never contribute
invented turns, idle continuity, waits or critical edges, and stay outside
joined-turn traversal and causal activity calculations.
As in the selected application, unique
qualified participants still contribute to the agent count and session-cost total;
those totals do not assert a receiving turn. Unknown or unpriced costs remain
unknown. The unchanged
128-participant and shared decoded-metadata input bounds remain; those input
bounds do not claim a serialized-response byte cap.

Optional `observed_extent.recorded_activity` separates whole-session recorded
timestamp clusters from a receiving turn: `state: available` carries
`kind: timestamp_clusters` and at most 256 ordered, nonoverlapping integer-ms
intervals (16 KiB); `state: unavailable` carries no intervals. The existing peer
header lookup admits at most 1 MiB and charges its entire content-free metadata
against the existing shared 4 MiB decoded-metadata budget before any source or
status rejection; unavailable peers cannot bypass the cap. Only its `busy` clusters
escape the allowlist, under the same generation, source, custody and status fences;
runtime drift clears current activity along with current status. No extra query,
Event Index/body read, source scan or poller is added. The producer already hashes
these clusters in the header generation. They cluster recorded timestamps within
five minutes; they do not prove continuous CPU activity.

The peer track clips only these recorded clusters to the relay/qualified extent,
with no idle fill, tail extrapolation, receiving turn or causal edge. Blank ranges
stay unknown. Missing, malformed or capped activity has a visible per-peer
unavailable qualification; it never becomes an accepted prefix. Activity availability
does not change causal `incomplete` or its receiving-turn coverage flag. Old
responses without the optional activity field remain compatible and gain no marks.

`incomplete` remains true for absent receiving turns. The optional
`incomplete_only_missing_receiving_turns` flag permits the UI to omit a redundant
global notice only when the producer found exclusively absent, unambiguous joins,
complete previews and no frontier or other missing metadata. The UI independently
requires every reachable unjoined spawn/relay to have a qualified participant and
the visible “Its turn isn't in these logs” hop note. Legacy responses, ambiguous
joins, unavailable observations, stale metadata, bounds and other missing context
retain the global warning. No request owner, body scan, authority or bound changes.

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
ordering; a selected root already present in the publication does not replace
another root. Qualified selected metadata can supply a root missing from the
bounded publication; only that off-page ancestor fallback may replace the eighth
row. This retains native approval ancestry outside the first 60 rows while
preserving the selected application's order for already-published roots.

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
The catalog-list mock now passes selected lookups through unchanged. On a direct
session link, selected metadata can precede the existing bounded catalog refresh;
the test waits for the third preview row before asserting the exact three-row
cap. Failure-only diagnostics retain bounded public metadata and preserve the
original exception. No application refresh cadence or wait timeout is changed.
Cold direct-link coverage asserts inclusion within that cap, matching the
original ranked ordering. A separate same-document route transition starts with
the selected child absent and verifies its required-path promotion to the front
of the retained ordering scope. This preserves both contracts without making a
fresh page depend on response arrival order.

Workspace Recent, machine labels and the Home waiting badge have a separate
publication from focused Machine content. A focused response had overwritten
that shared sidebar with one source's rows. The existing bounded Home POST read
now refreshes workspace metadata within the same overview epoch, navigation
cancellation and eight-second polling owner. Its 60 Recent rows and machine labels
replace the workspace snapshot together; focused rows, section pages and versions
remain source-specific. Sidebar refreshes do not hydrate transcript previews,
repair selected records or add a polling owner. The exact bare Home request joins
the existing request-abort set and retains its 15-second deadline. Already
delivered JSON must still pass the navigation epoch before publication.

The badge uses the accepted workspace waiting total only when that publication
has no pending or unavailable observations. Preview hydration cannot turn an
incomplete receipt into a qualified total. Unavailable refreshes retain saved
Recent metadata with an availability description and withhold the badge.
Source-qualified row identities prevent equal native keys on different machines
from sharing selection or ancestry. Browser fixtures cover phone/desktop and both
themes, transport cancellation independently from late JSON, focused content,
partial/unavailable workspace metadata, selected reading and Trace. These changes
require generated-asset freshness and exact-pair CI before acceptance.

The first restored-clip run exposed real View all/View script click failures.
Failure-only geometry in CI run `39057943250`, artifact `12110193218`, identifies
the final obstruction as the sticky topbar: the button lands near viewport y=0,
its center hit stack reaches `header#topbar`, and transcript bottom-clip clearance
exceeds 230px. Earlier dock interceptors remain in the raw call logs. The shared
output expansion buttons now have a scroll margin using the shell's measured
`--barh` plus the existing 8px spacing token. This changes scroll alignment without
changing layout, clip geometry or click assertions. Independent source review
covers desktop's main scroller and the phone document scroller; CI must still
prove successful real clicks and unchanged visual comparisons.

The same diagnostic's selected-status assertion had a fixture precondition
failure. Its query-only Home route missed the bare Home POST, so the selected row
remained in the accepted publication instead of exercising the intended native
metadata fallback. The matcher now covers the exact Home pathname with or without
a query. Each cold/warm attempt proves removal from all Home arrays and waits for
its own visible accepted-publication witness before checking the unchanged
selected inclusion, unavailable-status and three-row cap assertions. No product
status logic or timing changed. The earlier interpretation as a selected-status
application bug is superseded by this request/publication evidence.

### Workspace fixture receipts and exact-pair evidence — 2026-10-18

The full e361 UI run `39058724365` validates the original expansion clicks,
accessibility and selected-approval navigation. Its transport comparison is
27/27 frames and 44/44 regions pixel-exact; accessibility covers 473 screens
and 19,294 controls with no failures. Foundation, generated freshness and nine
of ten functional suites pass. The interaction job reaches its existing
15-minute job limit without reporting the new workspace cases; its exact
stalled stage is unproven. These partial results are not final acceptance.

The four new workspace fixtures pause the browser clock to preserve deliberate
eight-second polling advances. Their readiness checks now poll actual state
through the existing bounded Node deadline rather than depending on a paused
animation frame. Supporting run `39059912038` then identifies an independent
fixture wait: all four cases reach the unavailable publication and wait for the
completion of an unread 503 body that the application intentionally cancels.
The fixture now requires the exact 503 HTTP receipt and unchanged rendered
saved-row/qualification/badge assertions; successful 200 receipts still require
body completion. Supporting run `39060191323` passes all four phone/desktop
light/dark cases against unchanged e361 application code. No product deadline,
poll cadence, cancellation, HTTP assertion or fixture state is relaxed. The
failure and temporary diagnostic workflow remain historical evidence; the
workflow is not included in the PR. Full final interaction checks remain required.

The same e361 visual run is incomplete: two aborted Home POSTs prevent three
captures, so 60 exact expanded captures do not establish 62-state acceptance.
Failure-only request ownership diagnostics retain those failures without an
abort waiver. Strict regions are 162/162 exact; only 112/113 frames are captured,
with 108 threshold passes and four raw differences. The preceding d383 complete
comparison remains historical rather than inherited evidence. Performance has
52 report-only budget overages and no warnings/failures; this debt is not fixed.
The current pair, required checks, original artifacts and unresolved visual
decisions are maintained in the #380 evidence continuation linked from both PRs.

### Restoring native-page history isolation

Hub's existing approved application navigation contract requires an empty Recent
slot on unselected native pages and no workspace-history reads for chrome. The
selected Source263 was used for the twelve explicitly approved native references
in Hub commit a84eef99, which is already included in selected Hub503. The
canonical candidate had added a Home read at native-page entry and recheck,
populating workspace Recent. This is an application isolation regression; its
location in the sidebar does not make it an approved native-data difference.

The overview refresh owner now refuses native host content before any reader
request or epoch work. Native location handling stays within its server-owned
content before interpreting source/reader paths, and capability rechecks do not
start reader requests. The existing eight-second effect lifetime and teardown are
preserved. Home/Analytics links still enter their reader document; selected-source
and selected-session Recent retain their existing bounded publications. Four
focused browser cases cover both widths/themes, cadence, visibility, manual
refresh, history notifications, input/focus/content identity and destroy without
APIs. Existing lifecycle cases retain replacement/effect checks with zero native
history reads. Hub retains its six-layout Recent refresh assertion on the selected
reader and adds native-page isolation assertions. Final embedded assets, consumer
freshness and exact-pair runtime/visual/security checks remain required.

The corrected served fixture also qualifies the authoritative selected sid lookup
as unavailable while preserving stale details.state=done, matching its ranged
metadata. It waits for distinct cold/warm accepted selected-name witnesses. The
previous fixture only changed the range response, allowing the valid later sid
lookup to replace it with Done. Supporting run39061220845 passes both served
cases and all87 lifecycle cases against the old exact runtime, then exposes two
functional tooltip selector failures: the first menu tooltip is now the restored
Claude availability explanation, while the assertion expects a run state word.
The test now targets the visible run state dot explicitly, preserving its hover
allowance, exact state-word and both Escape assertions. No production status or
tooltip change is justified by those fixture failures. Final checks remain pending.

### Matching actual-application capture readiness

Fresh embedded run39062263341 passes all91 lifecycle cases, both served cases
and all12 interaction checks. Its shell fixture still expected Recent rows on
an unselected native Devices document. The fixture now requires empty Recent
and no native API reads through document exit, preserving all six native
captures, drawer/focus checks and source-qualified Recent navigation in the
actual reader after the explicit Sessions link.

The expanded comparison stopped at Atlas Ingest's scroll-zero assertion. An
earlier capture recorded reference scrollTop418 and a44px jump-slot difference.
The reference fixture waited only for a transcript container before sending its
wheel/Home/reset, while the candidate already waited for a loaded entry and
stable reading state. In original263 destination.ts, done() ends loading and
synchronously starts the opening-end pin; viewport.ts keeps it until user input
or its existing two-second limit. Early fixture input could precede that pin.
Both sides now require the same loaded route, fonts and existing API/DOM
stability before the same input. The post-reset deadline/assertion, all62 states,
screenshots, reference commits and thresholds remain unchanged. Failure-only
diagnostics retain bounded side/scroll/layout flags with no query or body text.
CI must validate these fixture corrections; missing captures remain failures.

That run's162 strict regions are exact, but a Home POST cancellation leaves one
of113 frames missing. Temporary ownership diagnostics preserve raw failures and
report unknown attribution when network timing cannot establish an owner. No
abort waiver is adopted. Rust run39062263316 also fails an offline Codex process
case with expectedProtocol/actualUnavailable. Instrumented supporting39063659770
passes the original six cases once without emitting an unavailable stage/errno;
the historical cause remains unproven and no runtime fix is claimed. Temporary
instrumentation/workflows stay outside the PR. Final exact-pair checks and any
evidence-bound visual decisions remain required.

### Native fixture mount and unavailable-state receipts

Run39063938849 exposes a mistaken native fixture selector: catalog-title belongs
to the title container and is cleared for Machines, so the nested title span
never receives it. The independently built fixture now emits a receipt only
after both real mount calls return. All six native captures, empty Recent,
history-read refusal through document exit, drawer/focus and qualified reader
navigation assertions remain intact.

The same run misses the second temporary-unavailable note in the recovery
fixture. Its8100ms clock advance can cross both capability recheck and a selected
range's1000ms retry, depending on when the mocked503 arrives. The old failure
does not retain the response/DOM clock witnesses needed to establish that exact
interleaving. The fixture now holds only the first range503 until the advance
returns, verifies its exact response headers while the clock is paused, then
requires the original unavailable note and1100ms recovery. It does not wait for
the intentionally canceled unread error body. Failure-only diagnostics retain
bounded stage/counts and allowlisted status-note classifications. Product
timers, deadlines, error behavior and all original recovery/security assertions
are unchanged. These corrections require fresh CI; they are not runtime fixes.
