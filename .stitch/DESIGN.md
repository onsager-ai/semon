---
name: Semon Session Inspection
colors:
  ground: '#ffffff'
  side: '#f7f7f5'
  sunken: '#f1f2ef'
  ink: '#191d1b'
  secondary: '#3d4440'
  muted: '#5d6561'
  line: '#e5e7e3'
  lineStrong: '#d3d7d2'
  error: '#b53232'
  errorSurface: '#fbeaea'
  working: '#1d6db0'
  waiting: '#8f5700'
---

# Design System: Semon Session Inspection

Source baseline: `274f0b9bb43f65982863a22c177413c04d805740`.
Canonical CSS tokens remain in `crates/semon-sessions/src/viewer.css`.
This is the authoritative design specification for UI work in this repository,
derived using the pinned Stitch extract-design-md/design-md workflows and the
live Stitch design-system workflow. It describes baseline foundations
and intentional changes separately. Existing design-contract.md owns shared
implementation and validation constraints.

## 1. Visual Theme & Atmosphere

Baseline: a quiet, compact record reader, neutral surfaces, ink actions, flat
lists, a 296px application sidebar and a 56px toolbar. Content takes priority
over decoration. Source observation and runtime status are separate concepts.

Proposal: prioritize metadata search, keep labeled filters readable, give
results a compact summary, and make recovery actions discoverable. Preserve
the reader's surrounding context when opening tool output or returning to the
list. Do not introduce global conversation search, runtime totals, nested agent
relationships, attachments or remote execution where capabilities omit them.

Final visual direction: preserve the existing actual viewer, including its
Instrument Sans/JetBrains Mono families, type scale, compact controls, page
spacing and reader components. Stitch screens are workflow explorations;
their aesthetics do not replace the approved product UI. Canvas typography and
geometry derive from the rendered product, not an independent visual redesign.

## 2. Color Palette & Roles

Use the YAML light roles above. Dark roles: ground #141817; sidebar #181c1b;
sunken #222725; raised #1b201e; ink #e6e9e7; secondary #c5cbc7; muted #9ba39f;
line #2a302d; strong line #3d4541; error #f27c77; error surface #3a1f1d;
working #66b0ef; waiting #e4a53f. Accent is ink in both themes. Neutral borders
separate regions. State color appears only alongside its state text. Faint
#8a918d / #6f7773 is for rules and decorative icons, never readable text.

## 3. Typography Rules

Instrument Sans for interface and long messages; JetBrains Mono for commands,
code and identifiers. UI 14px/20px (15px/22px phone); reading 16px; secondary
13px; caption 12px/18px; titles 16px (17px phone). Use 500–600 weight for row
names and headings. No text below 12px. Wrap long prose; horizontally scroll
code inside its region without creating page overflow.

Intentional composer refinement from source baseline
`2c9a7dade9dc045ca1218f016346d88d054caefb`: desktop task input changes from the
16px reading role to the 14px UI role; expanded select values change from 14px
to the same 13px secondary role already used by compact setting buttons and
labels. Preserve Instrument Sans, existing insets/control geometry and the
16px transcript text. On narrow/coarse-pointer devices native textarea/select
fields remain 16px to avoid focus zoom, while enhanced button-based selects
stay 13px. Toolbar targets remain 44px; the scoped dropdown density is specified
in section 9. This is an incremental control adjustment,
not a change to the reading type scale or navigation.

Composer configuration is presented as quiet value chips, with a small
disclosure chevron and emphasis on hover, keyboard focus or expansion. Expanded
settings align labels and values in property rows; the shared Select uses a
quiet sunken surface rather than repeated full-width outlined fields. Preserve
native fallback, labels, value owners and focus return. Retain the established
state-dot-plus-word vocabulary; never replace actual runtime or connection
states with decorative or inferred readiness.

## 4. Component Stylings

Reuse shared shell, search, compact buttons, session rows, transcript messages,
tool-step disclosure and panel chrome. Use 4/8/12/16/24/32px spacing, 6/10/14px
corner roles. Panels alone use the existing shadow. List rows remain flat.
Search is the primary field; Harness and Repository are secondary labeled fields.
Reuse the existing `search` controls and wrapping `facet-filters` form with inline
labels. Explain search scope once.
Clear filters is a real action.
Show a loaded-result count without pretending it is a global total.
Tool summaries show name, command/title, recorded exit status and expansion.
Expanded output keeps input/output labels, copy actions and truncation notices.

## 5. Layout Principles

The desktop main reading column retains existing width and 32px gutters. Phone
gutters are 16px and the shell becomes its existing drawer at 760px. Keep the
existing 8px wrapping form gap and 260px field flex basis. At 1280px, Search and
Harness share the first row; Repository and Apply filters share the next. At
390px, fields wrap with 44px targets. Match 390/820/1280px evidence.
Focus is a visible 2px ink outline. Every control has an accessible name;
disclosures expose aria-expanded, filters have persistent labels, and Tab/Enter/
Space/Escape follow native semantics. Coarse-pointer targets are at least 44px.
Loading retains useful records; empty filtered states offer Clear filters;
errors retain history and offer Retry. Never erase retained data on stale reads.

## 6. Design System Notes for Stitch Generation

Create materially different workflow layouts using the same foundations.
Use synthetic harbor/atlas/ledger content, long commands, recorded failures and
source limitations. Preserve normal, loading, empty, error, focused and expanded
states. A design exploration is not proof of an implemented product capability.

## 7. Selected direction and interaction specification

Stitch project: `4096612056644866749`; design system:
`assets/4efd29b1bcee4cccbe2a54657836d9ce`.
Selected screen: `3e0ca02d8a604ba2932095cee5879661` (direction A).
Alternative: `ee33d0e2514041ef9eb45e91c917f401` (direction B).
See metadata.json for screen references and local rendered previews.

Choose A for the retained reading width, narrow-screen navigation, existing
host ownership and bounded implementation. B offers simultaneous list context
but narrows the reader and requires new pane/focus ownership. It is deferred.

- Preserve the existing compact viewer anatomy: one toolbar, contiguous session
  rows and one reading column. Avoid extra panes, summary cards or permanently
  expanded tool output. Use the compact reader frame as the default review view;
  the expanded failure frame demonstrates an explicitly opened tool.
- Keep labels inside the existing shared search controls, with the original
  UI type size, 10px horizontal padding, 8px gap and 10px corners. Search has a
  short visible label and an accessible name of `Search session details`.
  Harness and Repository retain visible inline labels. Form wrapping follows
  the existing facet filters; no bespoke stacked-label layout is introduced.
  The explicit submit
  button makes Enter reliable in a multi-field form. Commit on native change,
  Apply filters or Enter. Do not fetch on every keystroke.
- Keep the full search/scope explanation inside a native disclosure, labeled
  `Search scope`, beside the loaded count and Clear filters on one metadata row.
  Its expanded explanation wraps across the content width. Do not hide active
  failure/loading notices. State `N sessions loaded`, never a global total.
- Clear filters resets query/harness/repository atomically, loads one list and
  returns focus to the first available filter. Empty matching results retain
  the same recovery action; an empty source without filters does not show it.
- A selected session has a visible `Back to sessions` action. Return restores
  the source's filters, list scroll and selected row focus. Browser Back also
  follows that transaction. Do not create a second router or model store.
- Keep existing long-message clamps, tool expansion/copy and recorded-text
  paging. Failed tool summaries expose recorded exit status, not an inferred
  session failure. Partial history remains explicit. Expand in place without
  dropping the surrounding messages; compatibility view alone supplies full
  relationship context.

Preferred patterns: `label.search` contains the inline label and named input;
`facet-filters` owns wrapping; `nrow` contains one name line and one metadata line; native
`details/summary` contains background explanation; `link` buttons contain real
actions. Reuse shared transcript and tool components without copying their
controllers into a product host. Lists must remain contiguous, not card grids.

## 8. Handoff reconciliation and future changes

Editable canvas: `../docs/design/session-inspection/session-inspection.pen`.
Frames cover compact 1280px catalog and reader, 390px filtered catalog, expanded failed tool,
loading/empty/error/focus and dark theme. The shared navigation in the canvas is
a schematic reference; production chrome keeps its existing components.

Intentional changes after Stitch: removed invented daemon sockets, a 100-entry
ceiling, connection buttons and runtime/exit totals; restored exact repository
tokens instead of Stitch's generated Material palette; kept the existing 16px
title and neutral tool borders; separated list and reader states; preserved
native 44px targets, scoped capabilities and contiguous rows. Stitch's returned
system was inspected and reconciled, not accepted as a new CSS authority.
Browser validation added a real submit action and moved Back to sessions beside
the truncated session name within the existing single-line toolbar. These changes
are reflected in the native canvas. Reader controls use viewer.css primitives;
they must not rely on Hub-only shell.css loading.

User review preferred the previous actual compact viewer over the mockup style.
The final refinement restores its original inline search controls, automatic
form wrapping, fonts, sizing and page spacing, with one count/scope row. Labels,
atomic submission and restored navigation context remain. Stitch A/B PNGs are
unaltered exploration evidence; the editable canvas and implementation capture
the selected source-derived intent. The failed-tool component stays collapsed until
opened; its separate expanded frame is an interaction state, not the default.

Real native-record validation also exposed two baseline defects: retained metadata
search could fail before a history copy existed, and incomplete handoff prompts
or a derived end marker could invalidate a selected transcript. Current metadata
may be read as a fallback only while its catalog lifecycle remains current.
Unavailable original prompts are labeled explicitly; an unprovenanced end is
a session boundary and has no native field-read authority. Preserve all other
scope, identity and provenance validation. The final failure canvas uses the
sanitized harbor retry_backoff output and existing sunken tool component.

Before future UI edits, read this specification and the existing design and
ownership contracts. Reuse semantic CSS roles and shared components. Update the
native canvas when layout or interaction intent changes. Compare representative
implementation screenshots at matching viewport, theme, content and expansion
state. Record deliberate deviations and actual checks in the PR. Inspect visual
diffs before updating baselines; never relax their thresholds. Keep documentation
and canvas links in the existing ai-design-workflow brief and ui-design skill
architecture; no additional instruction platform is required.

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
`../docs/design/session-inspection/surface-refinement/`. The retained-layout
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

The editable canvas remains `../docs/design/session-inspection/session-inspection.pen`.
Its current compact disclosure board supersedes the explicitly historical
linked-session board. Annotation screenshots/checks are in
`../docs/design/session-inspection/annotation-refinement/`; previous Image Gen
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
