# Session activity viewer

Status: **approved design direction; implementation and visual baselines not yet accepted**.

This text specifies the session activity direction; it does not claim that the viewer or the additional API support is implemented. The existing [activity derivation and query contract](session-activity-analytics.md), [shared design contract](design-contract.md), and [viewer ownership contract](viewer-ownership.md) remain authoritative. Semon owns the shared viewer, chrome, tokens and interaction controllers; implementation must respect the selected-session reader and transcript owners rather than creating a second session or scroll owner.

## Placement and overview

Place Activity between the session heading/metadata and transcript, preserving transcript access and existing session navigation. It opens **collapsed by default**. Its single-line disclosure contains the Activity label, a compact context sparkline, exclusive known cost, classified waste share and recorded change count. The sparkline communicates context growth and compaction drops, with an accessible text equivalent. Show a labeled unavailable preview when no qualified bounded preview exists; do not fetch an unbounded step history to draw it. At narrow widths, wrap the figures as needed without horizontal page scrolling.

On expansion, lead with a short evidence-based insight sentence and compact chips for exclusive cost, waste steps and recorded changes. Use the most relevant observed signal, such as the share of known cost above 150k or recorded compactions. Qualify incomplete pricing and classifier coverage. Exclusive totals lead throughout; observed and unresolved shared values remain separately labeled. Never infer savings, avoidability, causation, productivity or original ownership from these observations.

## Context and inspection

The main timeline plots ordered steps on the horizontal axis and context tokens on one vertical scale. Label the 50k and 150k boundaries and lightly wash the region above 150k. The bands are `<50,000`, `50,000–150,000` inclusive, and `>150,000`. Mark compactions at their producer-assigned step: the first following step, or the last timestamped preceding step for a trailing signal; unplaced signals stay unplaced. Do not infer a compaction from a visual drop.

Place waste glyphs on the timeline: diamond for orchestration, square for tooling or external failure, circle for context or unmapped signals. Display category in the persistent readout and reveal kind in a disclosure; shape and text supplement color. Align a separately labeled cost-per-step strip below the plot with the same step positions and its own maximum. Never use a shared cost/context vertical axis. Hatch unknown-price slots and label them unknown, not zero. Beneath it, a change-fact rail places timestamped commits, pushes and pull requests in time between timestamped steps; it must not imply an exact step join. List facts without usable timestamps separately. An exact step identity, if present, is distinct evidence.

Pointer selection snaps to the nearest step across the plot. One keyboard entry point supports left/right arrows for each step, Home/End for the endpoints, and Enter for the transcript action. A persistent inspector reports display ordinal, context, known or unknown cost and carry, elapsed time, compaction and waste category, with kind available on disclosure. Keep the native step identity separate from the display ordinal. A “Jump to transcript” action selects and focuses the corresponding entry only when a source- and generation-qualified transcript anchor is available. Until then, explain that navigation is unavailable; an ordinal, timestamp or adjacent record is not an anchor.

## Cost, interruptions and changes

“Where the money went” shows exclusive list cost with a thin stacked anatomy bar in stable order: fresh input, carry (cache read), cache write, output. Direct labels and values accompany the segments. Show a second ordered bar for known cost by context band, labeling percentages as shares of known cost when prices are incomplete. Represent unknown amounts with a labeled hatch or marker without assigning them a dollar width. Current summaries support carry versus other known cost; do not invent the four component dollar amounts from token counts or a partial step price.

“Where progress stalled” separates recorded waiting time from counts of classified waste steps. A measured wait-versus-other-measured-time bar can appear only when sufficient interval coverage exists; union overlapping wait intervals and do not call the remainder productive time. Show category rows and counts by default, with waste kind in each row's disclosure. Empty searches are a separate non-waste signal. Refill and compaction frequency are observations, not proof of thrash or avoidable work.

“What shipped” presents recorded commit, push and pull-request facts as a concise timestamped list with their supported typed details. Qualify “none detected” by classifier and source coverage; absence of a recorded fact is not proof that nothing shipped. Keep unplaced facts visible, and never expose raw command output.

## Family

Show family small multiples only when `family.member_sessions >= 2` and the family is available. Parent and child rows use independently qualified summaries and bounded step previews on a common context scale, with each member's exclusive figures first. Keep shared cost and unresolved ownership secondary and distinct. Family totals do not create child rows or attribute shared observations to an original payer. Family step time can exceed wall time; thrash remains a per-physical-session observation. If a member preview is unavailable, label that row rather than drawing an invented line.

## Partial, unavailable and stale states

- **No activity summary:** say activity is unavailable; do not display zeros as observations.
- **Missing prices:** retain known values, show the unknown-step count and an explicit incomplete indicator. Known shares use known cost as their denominator. Unknown cost is never zero.
- **Step pages unavailable or refused:** retain qualified summary, band, category and change counts. Replace the timeline and inspector with a clear reason when available; do not fabricate steps, a four-way cost split or exact wait duration. Offer retry only where a real retry is supported.
- **Partial coverage:** identify the affected source, pricing, timing, classification or fact coverage when known; distinguish a measured zero from unavailable evidence.
- **Stale generation:** stop using the stale continuation and any mismatched detail; refresh the qualified summary and restart paging under one generation. Explain when refresh cannot complete. Never combine pages from different generations.
- **Empty data:** show an explicit empty state only when complete coverage establishes zero observations. Shared and family unavailable states remain distinct from zero.

## Later Analytics page

A separate later Analytics view compares a qualified cohort; it is not an existing query promise. Lead with an insight and scope/coverage summary. Show ranked horizontal known-cost bars for the top sessions plus a separately aggregated tail row. A steps-versus-cost scatter represents every eligible session, including known zero-cost observations; overlapping points remain visible or are represented by declared density bins. Use a labeled ordered single-hue ramp for waste share, avoid overlapping labels with collision-aware placement or omission, and provide the complete observation table. Daily 100% stacked columns show the mix of known cost across the same three context bands, with per-day price coverage. Do not mix shared and exclusive accounting silently. On phones these views stack vertically.

## Accessibility, theme and layout

Use semantic headings, real disclosure controls, visible focus and keyboard-operable chart inspection. Every chart needs an accessible summary and a text or table equivalent containing values, units and coverage. Hover and focus expose equivalent detail; retain the selected-step readout without hover. Use glyphs, labels and optional texture in addition to color, support forced colors and reduced motion, and maintain at least 12px text and AA text contrast. Phone and coarse-pointer controls need 44px targets; the dense timeline uses the plot area as its pointer target.

Reuse the canonical viewer's Instrument Sans and JetBrains Mono, spacing, radii, surfaces and state tokens. The approved mock proposes restrained chart colors, not a new production token contract: light surfaces use white ground, dark ink and neutral borders; dark surfaces use near-black green ground, light ink and neutral borders. Context uses a blue line and a subtle high-context wash; band shares use an ordered blue ramp. Fresh input, carry, cache write and output use stable distinct hues; interruption status colors remain reserved for classified signals. Direct labels and texture preserve meaning where segment colors have insufficient contrast. Any final chart tokens must be reconciled with the shared contract in both themes.

At 360px, retain 16px side gutters and safe-area insets, stack analysis sections and family rows, allow labels and metrics to wrap, and avoid horizontal page overflow. Preserve legible chart axes, the persistent inspector, disclosure access and usable touch targets rather than shrinking desktop content.

## Data mapping and required API work

| UI | Existing source | Boundary |
| --- | --- | --- |
| Summary and chips | `activity_analytics.totals.exclusive`, `waste_by_category`, `empty_search_steps`, `change_facts`, `exclusive_cost_complete` | Measurements carry `known` and `unknown_steps`; `activity_analytics` can be null. |
| Anatomy bar | `activity_analytics.component_cost_usd` (exclusive steps; `fresh_input_usd`, `cache_read_usd`, `cache_write_usd`, `output_usd`) | Each is `{known, unknown_steps}`; a known component stays known beside an unknown one. Not available per band. |
| Context sparkline | `get_session_activity` with `kind: "preview"`: `generation`, `steps_total`, `binned_steps`, `bins` (at most 64, each `first_step`, `last_step`, `max_context_tokens`, `last_context_tokens`, `list_cost_usd`, `waste_by_category`, `compactions`) | Fetched on demand, not in the summary; same generation and typed refusals as step pages. A refusal is the labeled unavailable preview. |
| Bands and compactions | `by_band` exclusive totals, `compactions`, `thrash` | Respect producer band boundaries and physical-session thrash semantics. |
| Timeline and inspector | `get_session_activity` with `kind: "steps"`; step `id`, `timestamp`, `context_tokens`, `band`, `tokens`, `list_cost_usd`, `fresh_input_usd`, `cache_read_usd`, `cache_write_usd`, `output_usd`, `elapsed_ms`, `compactions`, `ownership`, `waste_category`, `waste`, `waits`, `empty_search` | Pages are bounded to 256 items; pass the returned opaque cursor and same generation. Detail may refuse or become stale. |
| Shipment | Summary `change_facts`; `get_session_activity` with `kind: "facts"`, including `identity`, `timestamp`, `step_id`, and typed `change` | Optional exact step IDs are not implied by timestamps; facts page independently. |
| Family | `family.member_sessions` and family totals; existing qualified relationships and separately fetched member summaries/pages | Family may be null; never reconstruct ownership or member detail from a roll-up. |

Implementation needs the following additions, with explicit source/cohort and generation qualification:

1. Per-step transcript anchors for the selected native session and source generation.
2. Delivered: nullable fresh-input, cache-read, cache-write and output **dollar** components per step, measured exclusive component totals in `component_cost_usd` (not per band) and the pricing fingerprint. Independently known components stay known. See [the analytics contract](session-activity-analytics.md).
3. Delivered in part: `get_session_activity` with `kind: "preview"` serves a bounded timeline of at most 64 bins (not in the summary), and `coverage` reports pricing, timing and classifier gaps. Bounded range/bucket reads for expanded heavy-tail sessions remain open.
4. Delivered: `coverage` plus the typed `error.reason` of a refused `get_session_activity` (`no_activity`, `stale_generation`, `source_unavailable`, `limit_exceeded`, `interrupted`, `unsupported_storage`). The measured unique wait duration is the existing `wait_time_ms`, which survives when step pages refuse.
5. Bounded cross-session aggregates for ranked top-N plus tail, scatter rows or declared density bins, and daily band mix, with cohort, shared ownership treatment, pricing and timing coverage.

All example values in future fixtures or explanatory copy must be synthetic and illustrative, such as `$2.40`; never incorporate private session records.
