# Semon UI design brief

This source-derived brief retains applicable visual foundations and approved
reader/composer refinements. Current task scope and settled product decisions
supersede obsolete capability invitations and copy. The shared design contract,
canonical CSS and actual reviewed application references retain their ownership.

Snapshot provenance: initial viewer `274f0b9bb43f65982863a22c177413c04d805740`;
section-specific source revisions remain below. These are historical provenance,
not the current checkout or a live product pin. Read [the design contract](design-contract.md)
and [baseline scope and review](ai-design-workflow.md#baseline-scope-and-review)
before applying this brief. Dormant composer variants do not authorize enabling
paused session control, SSH or managed runtime delivery.

## Foundation

Sessions is the default application destination. The shared sidebar contains
Sessions, Analytics and Machines; Home is no longer a page or navigation entry.
The root URL opens Sessions while retaining its query and fragment. Keep the
existing Sessions list, filters, grouping and ordering; do not move Home's inbox,
Needs you or Working sections into Sessions. Older Home screenshots are historical
references for the removed surface. Shared workspace metadata still supplies
Recent and machine/analytics views through its existing bounded owner.

Preserve the compact actual viewer, shared semantic CSS roles, Instrument Sans
and JetBrains Mono, existing controls, flat rows, reading width and responsive
navigation. The source in `crates/semon-sessions/src/viewer.css` owns tokens;
`docs/design/design-contract.md` owns type, contrast and touch constraints.
Use real screenshots and the editable [session canvas](session-inspection/session-inspection.pen)
for review. A canvas is a derived intent model, not a replacement pixel oracle.
Keep native forms, keyboard/focus, scroll and lifecycle owners intact.

Routine source/cache/search architecture explanations are not required interface
copy. Preserve correct source-scoped metadata filtering and meaningful exceptional
states without adding explanatory paragraphs to compact controls.

The numbered refinements below retain their original section identities so
existing links remain precise. Earlier generation proposals and raw artifacts
are archived in [the historical record](../history/stitch/README.md).

## 9. Restrained surface refinement

Incremental baseline: `bcc0a7624e753a818665518837d050af13395bef`.
The user rejected the first whole-page Image Gen direction because it changed
too much. Keep the existing shell, message alignment, fonts, widths, reading
scale, spacing and interaction owners. Flatten repeated informational enclosures
without rearranging the workflow. Preserve grouped tools, thinking, child and
handoff sessions, agent communication, metadata/configuration and composer.

- Linked child sessions use the compact disclosure pattern in section 10.
  Message bubbles, tool output and modal boundaries keep their existing
  functional treatments.
- A shared composer containing supporting notes has open supporting regions.
  Its textarea keeps a 1px inset boundary and the existing 6px corner role;
  the enclosing border becomes transparent while retaining its geometry.
  Input growth, insets, toolbar, recovery, readiness and billing order do not move.
  Touch value chips use 6px horizontal padding and a 4px internal gap so the
  Codex/Copilot, Managed and E2B labels plus launch action fit at 390px within
  the existing 17px visual insets. All toolbar targets remain at least 44px.
- Composer enhanced selects use 13px text, 36px desktop triggers and 32px
  desktop option rows. Narrow/coarse-pointer triggers and rows use 40px, an
  explicit user-requested exception scoped to this composer dropdown. Generic
  selects, toolbar buttons, native fallback fields and other touch controls
  retain their existing 44px target contract. Long option labels wrap and grow.
- Selection is a trailing 14px check and 500 weight. Keyboard-active options
  use the existing hover fill without a left rule; moving active
  focus must not silently change selection. Preserve arrows, Home/End,
  typeahead, Enter, nested Escape, mobile Back and native form value ownership.
  The mobile sheet heading is 13px and its close control is 40px with a 16px icon.

Image Gen originals and rejection provenance are in
`session-inspection/surface-refinement/`. The retained-layout
image is an exploration, not a pixel specification: generated text and state
details can be inaccurate. The implementation deliberately keeps the original
message bubble and navigation shapes rather than adopting those redraw changes.
The native session canvas records the linked-session surface component; actual
before/after images verify unchanged reader geometry at 390/1280px in both themes.
Hub consumes these shared styles only after reviewed source adoption.

## 10. Annotation refinement: compact related sessions

Baseline: merged `defe9d791a197902c5bebcba2e2b7fe87dd9a075`.
The October 2026 review requested less prominent subagent/handoff entries and
removed the composer dropdown's active left border. These are deliberate
incremental changes to section 9, not a new type scale or shell layout.

In relationship-capable transcripts, show a quiet row containing the session
name, existing harness/run metadata, labeled status and an expansion control.
Use the 13px secondary role for the name at 500 weight and the existing 12px
metadata/state role. Keep the original harness artwork and working/failed state
indicators. No card perimeter, left rail or default task/result paragraph is
needed. Hover applies to the summary; keyboard focus uses the shared outline.

The name opens the session. A separately labeled button expands complete task,
current activity/result and the existing Run view action in place, without
changing route or silently navigating. Enter/Space work through native buttons;
`aria-expanded` communicates disclosure state. Expanded text wraps without a
two-line clamp. Expanding/collapsing reuses the transcript owner's keyed state;
disposed or detached controls cannot mutate it. Errors remain visible as a
labeled state even while their full explanation is collapsed.

At narrow/coarse-pointer widths, stack name and metadata within the identity
column and retain 44px navigation/disclosure targets. Keep metadata ellipsis,
full text in the expanded context and direct session navigation. Preserve
messages, tool aggregation, thinking, relays, configuration and composer owners.
The bounded catalog does not expose complete relationship context: do not
fabricate subagent/handoff links there. This pattern applies where real resolved
relationships already exist.

The editable canvas remains `session-inspection/session-inspection.pen`.
Its current compact disclosure board supersedes the explicitly historical
linked-session board. Annotation screenshots/checks are in
`session-inspection/annotation-refinement/`; previous Image Gen
proposals are retained as historical exploration, not implementation references.

The later Codex/ChatGPT composer reference requests one continuous rounded input
surface around prompt and bottom toolbar. Use `sh-composer-surface` inside the
native form: existing `--sunken`, a quiet `--line` border and the composed
`--r3 + --s2` radius (22px). The textarea has no separate inset outline and the
toolbar values use `--muted`; text still meets AA contrast. A circular send face
keeps the existing compact control's target and visual insets. Supporting details
remain outside this input surface. Keep a visible textarea keyboard outline,
45px desktop/48px native touch initial prompt height, growing to 280px before internal scrolling, 14px desktop/16px native touch input, 13px values and 44px toolbar
targets. This explicit reference supersedes section 9's separate textarea outline
for this composed prompt variant; conversation and generic composer variants
retain their existing contracts.

The spacing follow-up intentionally reduces the unified variant to 12px prompt
insets and 8px toolbar bottom padding. Its visual toolbar inset is 13px including
the surface border; coarse-pointer targets stay 44px (6px toolbar padding plus
the 6px compact-control visual inset). Generic composers retain their existing
96px minimum and 17px toolbar insets. This exception follows the supplied
Codex/ChatGPT reference and does not change transcript density.

The linked-session state board now uses 16px phone-example edge padding and an
automatically sized outer frame. Narrow metadata has a deliberate ellipsis,
rather than cutting a glyph at the canvas edge; expanded tasks/results wrap
without a clamp. This corrects the native demonstration, not product typography
or row density. Hosts may use the shared finite `minHeight` geometry slot for
visible-viewport fitting; layout policy and listener lifetime remain host-owned.


## 11. Approved staging reader corrections

Keep the existing typography, shell, tool categories, icons and tool input/output
surfaces. Recorded readable Thinking starts as a compact keyboard disclosure;
pending and masked thinking retain their existing meaning. Expansion follows
stable record/slot identities through pagination, renaming and incremental updates.
Built-in MCP labels show provider and action for dotted and double-underscore
formats; unfamiliar identities retain the original name.

Tool category summaries occupy one line. Ellipsis applies to summary text only;
failure counts and disclosure controls stay visible. Expanded rows keep complete
recorded information. Names update from source-qualified native metadata without
replacing a transcript or its draft.

Jump to latest belongs at the lower center of the reading region, above the
conversation input or its unavailable row. The dock fits the visible viewport,
safe area and expanded composer. Its measured transcript paint boundary leaves
text above the dock while retaining the document/desktop scroll owners and
anchor calculations. It disappears at the latest output.

Available conversation input starts at one line and expands on focus. Unavailable
input becomes a quiet reason row, labeled with a saved draft when present. A
keyboard-accessible disclosure reveals the disabled input and retained draft.
Capability recovery restores input; readable history never grants send authority.
Drafts belong to source and session identity and clear only after confirmed
acceptance of that exact draft. Unknown delivery remains blocked from replay.

The written staging requirements are the implementation contract when the owner
authenticated Site cannot be opened. This section changes the conversation reader
only; new-task composer variants retain their existing design contracts.

## 12. Optional Machine filter for hosted Sessions

The current Sessions task supersedes the hosted machine-selection landing step.
`/sessions` reads all authorized machines in the current workspace, with the
existing compact Filter sheet defaulting to `All machines`. A concrete Machine
selection updates the URL and starts a new first page. Preserve flat two-line
rows, grouping, existing sort/filters and browser history. Add the machine label
to existing row metadata; source-qualified identity remains independent of labels.
Selecting a row records its exact machine in the detail URL. Returning to the
workspace list retains its filters. Standalone source reading is unchanged.

Preserve existing shared tokens and control geometry. Update only references of
the superseded hosted picker/default Machine state; generic filter and reader
baselines remain authoritative. Review desktop/mobile list, open Machine filter,
loading, empty and unavailable states in both themes using actual served data.
The session canvas's `Hosted Sessions · optional Machine` note records this change.
