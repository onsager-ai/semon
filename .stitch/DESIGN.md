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

## 4. Component Stylings

Reuse shared shell, search, compact buttons, session rows, transcript messages,
tool-step disclosure and panel chrome. Use 4/8/12/16/24/32px spacing, 6/10/14px
corner roles. Panels alone use the existing shadow. List rows remain flat.
Search is the primary full-width field; Harness and Repository are secondary
labeled fields. Explain search scope once. Clear filters is a real action.
Show a loaded-result count without pretending it is a global total.
Tool summaries show name, command/title, recorded exit status and expansion.
Expanded output keeps input/output labels, copy actions and truncation notices.

## 5. Layout Principles

The desktop main reading column retains existing width and 32px gutters. Phone
gutters are 16px and the shell becomes its existing drawer at 760px. Search and
secondary filters stack without clipped labels. Match 390/820/1280px evidence.
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

- Search occupies a whole row. Keep its label above the shared sunken input.
  Harness and Repository use equal desktop columns beside the existing reader's
  `link` submit control, Apply filters; stack on phones. The explicit submit
  button makes Enter reliable in a multi-field form. Commit on native change,
  Apply filters or Enter. Do not fetch on every keystroke.
- Keep the full search/scope explanation inside a native disclosure, labeled
  `Search scope and source limitations`. Do not hide active failure/loading
  notices in that disclosure. State `N sessions loaded`, never a global total.
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

Preferred patterns: `catalog-field` wraps a persistent label and the shared
`search` surface; `nrow` contains one name line and one metadata line; native
`details/summary` contains background explanation; `link` buttons contain real
actions. Reuse shared transcript and tool components without copying their
controllers into a product host. Lists must remain contiguous, not card grids.

## 8. Handoff reconciliation and future changes

Editable canvas: `../docs/design/session-inspection/session-inspection.pen`.
Frames cover 1280px catalog, 390px filtered catalog, expanded failed tool,
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
