# #380 UI preservation evidence

The selected visual reference is the actual complete `compat=1` application at
Source `263a02da278716ab2309b8e34a463419e5c781e5`, captured with Hub
`503fbe090749ea1d58b9ca9fa4ccae7289ee426e`. Hub main's design-assets merge
`e15596cdeff9a2c5c9f9431cd52932603c41e22e` records those browser captures and
source-derived DOM. Pen reconstruction exports are not pixel authority.
The approved feedback-20261015-v6 redesign remains separate work after #380.

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
- Repeated Find publication keeps the focused account widget in its host's Find
  row. Shared chrome does not reparent that widget into the ordinary toolbar.

Fonts, theme tokens, reading size, shell widths, buttons, Thinking disclosure,
tool grouping/expansion, Trace and native Hub layouts retain their owners. No v6
button, drawer, Trace-removal or transcript redesign is implemented.

## Data and pause boundaries

List pages remain 60 rows, retaining at most 300 per source. Grouping and filter
choices use loaded metadata; the source-qualified Machine field cannot imply a
workspace union. Header counts say "sessions loaded". Native discovery,
incomplete/stale generations, retry, cursors, selected transcripts, field expansion,
custody and request cancellation retain their existing implementations.

The user-approved temporary collection pause still removes Composer and New
Session actions, active runtime/control/SSH setup and broad full-text retrieval.
Saved credentials, metadata, operations, ordinary collection and selected reading
remain intact. Loaded-text navigation is independent of broad full-text search.
No paused action is restored to satisfy a visual comparison.

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
