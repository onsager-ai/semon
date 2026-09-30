# Viewer UI/UX overhaul

Status: approved 2026-09-29. Phases 1 (audit) and 2 (reworked mockup) are done; phase 3 ports it one screen per PR.

Scope: a UI/UX overhaul of the session viewer, approved and ported one screen per PR.

The reference is `tests/ui/reference/overhaul.html`; the previous mockup, `semon-sample.html`, stays beside it until every screen is ported. `tests/ui/reference-map.json` says which reference each screen of the pixel comparison uses, and a screen's port PR flips it to `overhaul.html`. Its screenshots come from the Mockup screenshots workflow (`tests/ui/shots.json`), at 390 and 1280 px, light and dark.

## Visual system

### Type

One sans for the interface, one serif for what agents write, one mono for code. Sizes are tokens in the mockup's `:root`.

| Token | Size / line | Used for |
|---|---|---|
| `--fs-cap` | 12 / 18 | Labels, meta lines, timestamps, captions, chart axes |
| `--fs-sm` | 13 / 20 | Secondary UI: step rows, event headers, filters, links in rows |
| `--fs-ui` | 14 / 20 desktop, 15 / 22 phone | Interface body: list rows, names, buttons |
| `--fs-read` | 16 / 1.5 serif | Messages from agents, briefs and questions on Home |
| `--fs-title` | 16 desktop, 17 phone, 600 | The top bar's title, the only page title |
| `--fs-fig` | 22 / 28, 600 | Figures: Analytics headlines, the session's cost |

Nothing is smaller than 12 px. Weight 600 marks names and titles only.

### Space and shape

- Spacing steps: 4, 8, 12, 16, 24, 32 (`--s1`…`--s6`). Page sections are 24 apart; rows pad 11–14.
- Side gutter: 32 desktop, 16 phone. Reading width 760; wide transcript 1200; Analytics 1040.
- Radius: 6 (small), 10 (controls, panels' content), 14 (cards, dialogs). Pills are fully round.
- Tap size: `--tap` is 36 desktop, 44 phone. Rows are at least `--row` (34 desktop, 44 phone).

### Colour roles

Colour is for content and state, not decoration. No coloured borders anywhere.

| Role | Light | Dark | Rule |
|---|---|---|---|
| `--ink` | #191d1b | #e6e9e7 | Content: messages, names, values |
| `--ink-2` | #3d4440 | #c5cbc7 | Secondary content: briefs, step text |
| `--muted` | #5d6561 | #9ba39f | All chrome text. At least 5.3:1 on every surface |
| `--faint` | #8a918d | #6f7773 | Rules, separators and icons only. Never text |
| `--line`, `--line-2` | #e5e7e3, #d3d7d2 | #2a302d, #3d4541 | Hairlines; control and card outlines |
| `--ground`, `--side`, `--sunken` | #fff, #f7f7f5, #f1f2ef | #141817, #181c1b, #222725 | Page, sidebar, fills (inputs, bubbles, code) |
| `--accent` | #3b4dbf | #98a4f3 | Links and the pressed state of controls. Nothing else |
| `--work` | #1d6db0 | #66b0ef | The working dot and spinner |
| `--wait` / `--wait-dot` | #8f5700 / #d08f1a | #e4a53f | "Needs you": its dot, word and badge |
| `--err` | #b53232 | #f27c77 | Failed: its dot and word |
| `--done` | #8a918d | #7f8783 | Done and idle dots |
| `--h-claude`, `--h-codex` | #c0643f, #2b8070 | #de8a63, #4fb5a0 | Analytics chart series only |

A harness is named in words: "Claude Code", "Codex", and for child sessions "Subagent" or "Codex run". No logos, glyphs or tinted chips.

### Icons and density

- Line icons, 1.8 stroke, 18 px (16 in rows). Icons stand for actions and navigation. Metadata is words, not icons.
- A list row is two lines: name and age, then one muted line. Child rows are one line.
- One state indicator per place: a dot and a word.

## Principles, as checks on a screenshot

| # | Rule | How to check |
|---|---|---|
| P1 | No sideways scroll | The shot report's `horizontalOverflow` is false at 390 and 1280; nothing is cut at the right edge |
| P2 | Phone targets are at least 44 × 44 | Every control in a 390 shot is at least 88 device px tall (2x) or has a 44 px hit area around it |
| P3 | Quiet chrome | The transcript or list is the largest, darkest text on screen; chrome is `--muted` 12–13 px; no coloured borders |
| P4 | One way to do each thing | Each fact appears once per screen; each action has one entry point per screen |
| P5 | Controls look like controls; information doesn't | Tappable: icon button, bordered button, pill, link text, or a full-width row or card with hover. Labels (meta line, badges) are never tappable |
| P6 | State is shown once | One dot and one word per session per place |
| P7 | Contrast AA | Text at least 4.5:1 (muted is 5.3:1 at worst); faint is never text |
| P8 | Details lead with what matters | A step opens on its command; a failed command shows the end of its output |
| P9 | Long lists fold | Child lists show the 5 newest, then "Show N more" |
| P10 | Loading keeps the layout | Skeletons match the loaded rows' height and position (see Loading) |

## Loading

Loading states use skeletons.

- **Geometry.** A skeleton is the loaded screen's own components with placeholders in place of text: the same rows, the same line boxes, the same heights and positions. Nothing moves when content lands. The mockup measures it: each `loading-*` shot renders the loaded screen offscreen, compares row tops and heights, and logs a console error if any differs by more than 1 px (the shots' diagnostics record console errors). Rows are compared by height, and by position for as long as the loaded layout has the same rows in the same order: a parent's runs arrive beneath it.
- **Delay.** Nothing shows for the first 150 ms (a CSS animation delay), so a fast load never flashes.
- **Motion.** A slow, low-contrast shimmer (1.6 s), and none at all under `prefers-reduced-motion`.
- **Where.** The Sessions list and the sidebar before the model arrives (`loading-sessions`, `loading-sidebar`). A transcript before its first page: the bar is real, the first turn's frame is placeholders (`loading-transcript`); prose lengths are unknown, so this one is not measured. Older pages at the top: a fixed block above the first turn; entries land above the reader, and scroll anchoring (`overflow-anchor`) keeps what they are reading still (`loading-older`). Analytics (`loading-analytics`). Trace (`loading-trace`).
- **Data flow.** These states mean "this data isn't here yet", nothing more. They don't depend on polling or on per-session transcript fetches, so the planned SSE stream and worker don't change them.

## Audit

Evidence: the viewer on main (Viewer UI run 36523657501, `viewer-ui` artifact, main at 4512f9f) and the mockup it mirrors (Mockup screenshots run 36522889130, `mockup-shots`, #38's head 92e9ef6). Shot names below are from the mockup artifact unless marked "viewer". Severity: High breaks a principle on a primary screen or was reported by a user; Medium costs time or clarity; Low is polish.

### Patterns behind the defects

| Pattern | Where it shows | Fix in the system |
|---|---|---|
| Colour says who | Orange 3 px borders on child cards, briefs, relays and a child's whole transcript; ✳ and ⌘ glyphs; tinted "Codex run" chips; harness squares in Trace | Harness in words; no coloured borders; harness colour only in charts |
| State shown two or three times | Bar dot and a second dot by the runs count; "Working" in the bar and again at the transcript's foot; a machine's dot and its "up" word | One dot and one word per place (P6) |
| Information dressed as controls, controls dressed as information | The meta line is one big button; the cost row is a full-width grey button; "?" icons that only hover; a child card that is a link with two more links inside | Labels are plain text with tooltips; one control per card; the menu holds details and cost (P5) |
| Several ways to the same place | Details in the "…" menu and in the details sheet; runs in a popover, in cards, in the sidebar and on Sessions; the parent via crumb, header link and "Open in"; a trace from every Home item and every turn | One entry point each (P4) |
| Chrome louder than content | Four filter selects before the first session or figure; icon-and-number meta; "vs previous 7 d" eight times; faint 2.6:1 text everywhere | Filters in a sheet with chips; words not icons; one caption; AA text (P3, P7) |
| Details lead with metadata | A step opens on its output with no command; a failed test log opens at its first line | Command first; failures show the tail (P8) |
| The phone as a narrow desktop | Select grids, 11 px chart bars that are tap targets, 36–40 px buttons, a 44 px toggle gutter on every sidebar row | Phone-first layouts and 44 px targets (P2) |

### Shell and sidebar

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| The expand/collapse toggle takes a 28 px (desktop) or 44 px (phone) left gutter on every row, leaves included | `sidebar-tree-390-light`: names start at x≈95 of a 330 px drawer | High | P3 |
| Parents are highlighted when their child is open: harbor, "Review offline-sync diff" and "Check flush ordering" are all selected | `grandchild-session-1280-light` | High | P6 |
| Unofficial ✳ / ⌘ glyphs mark the harness on every row | every 1280 shot | High | Decision: words, not logos |
| Child lists are unbounded; long fan-outs push lanes off screen | `sidebar-tree-390-light` | Medium | P9 |
| Two-line rows repeat host and repo ("marvin-mbp · harbor") on every row; 7 lanes fill the phone drawer | `sidebar-tree-390-light` | Medium | P3 |
| "All sessions ›" repeats the Sessions nav item | `home-1280-light` | Medium | P4 |
| Nav counts that ask nothing of you ("Sessions 15") | `home-1280-light` | Low | P3 |
| Rail mode shows each session as an identical ✳ with a dot: sessions can't be told apart | `desktop-rail-1280-light` | Medium | P5 |
| On phone, the drawer's edge stays over the left 40 px of the page, over the state dots (cause unverified: a capture during the close transition, or a transform bug) | viewer `check-real-home.png` | High | P1 |
| The wide-mode toggle is on every page's bar, where it does little | `home-1280-light` | Low | P3 |

### Session: top bar and menu

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| The whole second line is one button that opens details, while its parts look like labels; the error count and runs count are buttons inside it | `session-jump-1280-light` | High | P5 |
| Icons with numbers and no words: wrench 6, stack 3•, "T 38.8M", "$14.0" | `session-jump-1280-light` | Medium | P3, P5 |
| "1 error" in red, bold, next to the state; a second state dot beside the runs count | `session-jump-390-light` | Medium | P3, P6 |
| The state dot is clipped on the left | a user report; viewer `check-real-home.png` | High | P1 |
| Four icon buttons on the desktop bar (find, filter, more, wide) | `session-jump-1280-light` | Low | P3 |
| The "…" menu lists Model, Machine, Started, Duration, Tokens and Session id, which the details sheet lists again | `session-details-cost-1280-dark`, mockup `toggleMenu` | High | P4 |
| Session details is a list of label/value rows, then a full-width grey button "$14.04 own · $15.16 incl. runs" | `session-details-cost-390-light` | High | P5 |
| In the viewer, a loose line "Claude Code reported $X for its last run", then an amber "Differs from Claude Code's figure by 67%" alarm for an expected difference | viewer `viewer.js` 836–842 | Medium | P3 |
| Dialogs open with a focus ring on the close button | `session-details-cost-390-light`, `analytics-slice-390-dark` | Low | P3 |
| "Runs · 3 · API-equivalent cost" lists four rows (a grandchild counts under the three) | `parent-runs-390-light` | Medium | P4 |

### Session: transcript

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| Child cards carry a 3 px harness border (orange even on Codex runs) and say the kind three times ("⌘ Codex run", "feat/offline-sync · Codex run") | `session-jump-390-light` | High | P3 |
| A child card is a link with "Open" and "What X did" inside it; the inline mini-transcript duplicates the child's page | `session-jump-390-light` | Medium | P4, P5 |
| Relays and messages to you use coloured left borders (orange, amber) | `thinking-masked-390-light` | Medium | P3 |
| Masked thoughts show as bare "Thought for 40s" rows | `thinking-masked-390-light` | High | Decision: one quiet "Thinking hidden by the harness" line per turn that never splits a run of steps (was: hide them; changed on Marvin's request that thinking be visible by default, semon #114) |
| Every turn ends in a status row ("● Replied") that repeats what the turn shows | `thinking-masked-390-light` | Low | P3, P6 |
| Group summaries are cut at 390: "Ran 2 commands, read 1 file, edited 1 fi… · 1 failed" | `session-jump-390-light` | Medium | P3 |
| The jump pill sits at the bottom right, 40 px tall, over the text it should reveal | `session-jump-390-light` | Medium | P2 |
| A step opens on its output; the command isn't shown; a failed test log opens at "running 128 tests", with the failure 40 lines down | viewer `v-sheet.png` | Medium | P8 |
| A grey 2 px rail beside every run of steps, even a single step | `thinking-masked-390-light` | Low | P3 |

### Child session

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| Three coloured 3 px rails: beside the brief, the whole transcript, and the return block | `child-session-390-light` | High | P3 |
| Three ways to the parent: the crumb, the "harbor" link in "Brief from harbor", and "Open in harbor" | `child-session-390-light` | Medium | P4 |
| A sibling pager ("‹ 1 of 3 ›") in the brief header: a fourth way between siblings | `child-session-390-light` | Low | P4 |
| "Show more" and "Open in harbor" touch with no gap | a user report | Low | P2 |
| "Working" in the bar and again in the foot block ("Working · 3 tool calls · 48m") | `child-session-390-light` | Medium | P6 |

### Sessions

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| Four filter selects fill the first phone screen; the first session starts 800 device px down | `sessions-390-light` | High | P3 |
| One flat list mixes subagents and Codex runs with the sessions you started | `sessions-390-light` | Medium | P4 |
| Search, group-by and filters are three bands of chrome | `sessions-1280-light` | Medium | P3 |

### Analytics

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| The range control sits 6 px from the right edge at 390 | `analytics-7d-390-light` (#54 point 5) | Medium | P1 |
| Four selects again before any figure | `analytics-7d-390-light` | Medium | P3 |
| "vs previous 7 d" eight times; "+19.6 h vs previous 7 d" when there was nothing before | `analytics-7d-390-light` | Medium | P3 |
| Chart bars are tap targets about 11 px wide at 390 | `analytics-7d-390-light` | Medium | P2 |
| "?" icons that look like buttons and only show a hover tooltip | `analytics-7d-1280-light` | Low | P5 |
| The slice sheet's first row is cut under its header | `analytics-slice-390-dark` | Low | P1 |

### Machines, Trace, everywhere

| Defect | Evidence | Sev | Breaks |
|---|---|---|---|
| Machines: a right-hand "up / idle / offline" word repeats the dot; "up" uses the working blue | `machines-390-light` | Low | P6 |
| Trace: every hop repeats "Claude Code · mac-studio" behind a coloured square, and "Open in X ›" on its own line | `trace-390-light` | Medium | P3, P4 |
| Trace (#51 draft): 9.5–10.5 px labels, 32–36 px rows, the critical path in the link colour, harness dots | `run-view-1280-light`, `run-view-390-light` (run 36510271962) | Medium | P2, P3, P7 |
| Faint text for times, ages, counts and "no repo": 2.6:1 light, 3.3:1 dark | every shot | High | P7 |
| Muted text on the sidebar is 4.3:1 in light; amber "waiting" text 3.8:1; Claude-orange text 4.1:1 | `home-1280-light`, `trace-390-light` | Medium | P7 |
| No loading, empty or error states | — | Medium | P10 |

## What the mockup changes, by screen

| Screen | Before | After |
|---|---|---|
| Sidebar | Gutter toggles, two-line rows, glyphs, ancestors highlighted, search, "All sessions" | One-line rows (dot, name, age); a count pill on the right folds a parent's runs; 5 newest then "Show N more"; only the open session highlighted and its parents expanded; no sidebar search; badges only for what needs you |
| Rail | Nav plus a glyph per session | Nav icons with attention dots |
| Top bar | Title, icon-and-number line that is a button, 4 actions | Crumb up one level, title, a line of labels with tooltips (state, kind, model, failed steps, machine, branch, cost) that drop from the right to fit; two actions: Find, "…" |
| Session menu | "…" dropdown with a detail list; a separate details dialog; a runs popover | One panel (anchored on desktop, bottom sheet on phone): actions, details, cost. Cost: one figure, this session and its runs, the harness's own figure, a quiet note if they differ, the runs with their cost, tokens by model folded |
| Find and filter | A search bar, a filter popover (three checkboxes), an errors link | One mode: the field, then one choice of All, Messages, Steps or Failed steps (with its count). Thoughts show under All only |
| Transcript | Coloured cards, "Replied" rows, masked thoughts, cut summaries | Outlined incoming bubble under "Relay from X" or "Brief from X"; event rows with an icon, a sentence and the text; child cards with a hairline border and one action; turn foot only for a failure or a Trace link; masked thoughts hidden; summaries wrap |
| Steps | Output first, head of the log | Command (or file, or input) first, then output; a failed command shows its last 12 lines; "View all N lines" |
| Jump | Bottom right, 40 px | Centred at the transcript's foot, sticky, 44 px on phone; "N new" when new entries arrived |
| Child session | Three rails, three links up, sibling pager | A normal session page: the brief is its first turn's incoming bubble; one link up (the crumb) plus the sender link in the header |
| Home | Summary line; five lines and a Trace button per item | Sections only; each item: who and when, the text (3 lines), one context line; the row is the link |
| Sessions | Four selects, search, group-by, flat list | Search with a filter button; active filters as chips; sessions you started, with their runs beneath (5, then more); empty state with a way out |
| Analytics | Selects, range in the bar, repeated captions, thin tap bars | Range and filter in one row under the bar; one caption, which also says when there is no earlier period to compare with (then the delta lines stay blank); phone buckets at least 44 px wide; allowance as meters |
| Machines | Dot plus a repeating word | Dot and one line; a machine that stopped responding says so in red |
| Trace | Rail of hops with chips and "Open in" links | Timeline from #51 (see below), then the handoffs; names in sentences are the links |
| States | None | Empty (no sessions, no match, no analytics), error (a machine's logs can't be read), not found, and loading skeletons |

### The #51 Agents panel

Folded into Trace as "Timeline", because it answers what the transcript can't (what ran in parallel, what the end waited on) and it holds up once fixed: labels at least 12 px; rows 44 px on desktop and 58 px on phone; the critical path drawn in ink and everything else in faint grey, so the link colour means only links; harness dots removed (the model name says it); the cost column kept, as a rollup. The handoff rail stays below it for the briefs and results, without "Open in X" lines, since each row of the timeline and each name in a sentence already opens that session.

## Choices where the reports left room

- **Sidebar toggle.** Agrees with #56 (on the right, quiet, 5 newest then "Show N more"). Goes further: the toggle is a pill with the descendant count, so it also says how many runs are folded, and it turns amber when something inside needs you.
- **Selection.** Only the open session's row is highlighted; its ancestors expand so it is visible.
- **Harness names.** Agrees with #64: plain words. Top-level sessions say "Claude Code" or "Codex"; children say "Subagent" or "Codex run". The sidebar shows no harness at all: it is navigation, and the name and state are enough.
- **Masked thoughts.** Was hidden (#60). Changed on Marvin's request that thinking be visible by default (#114): at most one quiet "Thinking hidden by the harness" line per turn, at the first masked thought's place or before the run of steps it falls in, never splitting a run. Readable thinking is shown inline in full, with no control to open.
- **Rails.** Agrees with #59 and goes further: no coloured borders anywhere.
- **Jump.** Agrees with #62 (centred), and makes it sticky inside the transcript column so it centres on the text, not the window.
- **Gap.** #63's missing gap goes away: the child page no longer has "Open in".
- **Meta line.** Supersedes the in-flight meta-line fix: labels, not a button. Failed steps are a label; finding them is the Find chip, not a link in the bar.
- **Wide mode.** Moved into the session menu (a switch, remembered per device). It only ever mattered on transcripts. It lifts the prose caps too (an assistant message's 68 ch, the bubbles' 600 and 640 px), so replies use the whole 1200 px column; the normal width keeps its reading measure.
- **Details and cost.** One menu, as suggested in review. The headline figure includes runs when there are runs, because that is what the work cost; "This session" and "Its runs" split it. The harness's own figure is a row, and a difference is a sentence, not an alarm.
- **Sessions page.** Shows the sessions you started, with their runs beneath them, so the page and the sidebar agree.
- **Analytics taps.** Phone buckets are wider (6–7 bars) so each bar is a 44 px target; desktop keeps the finer buckets.
- **Home.** No Trace button per item: the row opens the turn, and the Trace link is at the turn's foot.

## Open questions

1. **Rail.** Nav icons only (recommended: sessions can't be told apart by an icon) or a two-letter monogram per recent session?
2. **Wide mode.** In the session menu (recommended: one fewer button on every bar) or back on the top bar?
3. **Sessions page.** Runs beneath their parent, 5 then more (recommended), or top-level sessions only with a run count?
4. **Sidebar harness.** No harness in the sidebar (recommended), or a muted "Codex" after Codex runs only?
