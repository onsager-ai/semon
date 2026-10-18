# Sessions Filter presentation decision

## Authoritative user review — 2026-10-18

The user reviewed the [22-state comparison Site](https://semon-380-visual-review.tikazyq.chatgpt.site/)
for Source `903a853dee60deb809b3af992ae5302c1162446f` and Hub
`dd969648ce6c780adacc9ec450fe07496cc8e02b`.
They rejected the routine paragraph beginning “Selected source: laptop. Cached
history.” and directed its removal from Filter without a replacement visible
explanation. The catalog browser now omits the shared sheet's `description`
option. Existing tooltip/accessibility attributes are unchanged; the paragraph
is not moved or replaced. The selected-source Machine value, source-only metadata
search, request boundaries, controls, focus, keyboard, clearing and paging remain
owned by the same implementations. Exceptional freshness/error/discovery and
bounded-search notices remain visible.

All six open Filter states (390/820/1280, light/dark) require fresh capture under
the original protocol. None is approved by this removal decision. No Filter
reference, threshold, mask, capture input or acceptance policy changes. Residual
mismatches must distinguish defects from correct source-scoped values; do not
replace the reference with the candidate. The browser regression now requires
absence of the paragraph and retains its sheet-scroll, touch, focus, clear,
source-option and request assertions. Run39101911725 identified the old phone
assertion's dependence on the paragraph: the compact sheet naturally fits in the
unchanged 480px interaction viewport. The regression now requires that fit,
briefly exercises the same scroll owner at 320px, and restores 480px before its
diagnostic capture. Required preservation/pixel capture inputs are untouched.
The two obsolete-overflow failures remain recorded, not counted as passes.

The same review approves only Hub's omitted New Session and resulting navigation
positioning in the twelve sidebar-only states and desktop first-run sidebars,
and the omitted first-run “or prepare your connections for a cloud runtime”
invitation and its wrapping/layout in four states. Hub records those exact
baseline corrections. No other cards, onboarding or PNG-equivalence policy is
approved. Original reference identity and prior raw failures remain traceable.

This decision supersedes the placement proposal below. Final commits and check
results belong to the #380 acceptance record and PR updates; preparation is not
acceptance or release.

## Superseded proposal and preparation evidence


Status: preservation implementation candidate for the draft #438 review. Its
visual acceptance remains open; no baseline or intentional difference is approved.

The candidate removes the browser-native Sessions result footer, which repeats
the loaded count already shown in the page summary and adds a count/sort/scope
row below the list. The Filter sheet becomes the home for routine source, cache,
ordering and search-scope qualification. Its note names the selected source and
current history observation, explains metadata-only search and its limits, and
states the latest-recorded-activity ordering. The search field and Filter trigger
carry the search qualification through the shared tooltip and `aria-description`.

The selected-source Machine field keeps its exact source label and single-source
options; no workspace-wide machine union or total is implied. The page summary
continues to show the loaded or complete session count. `Clear filters` remains a
separate action from the Filter sheet's existing `Clear all` behavior, and keeps
its current URL, request, focus and scroll semantics.

Stale, incomplete, unavailable, retry/error, discovery and bounded-search notices
remain in the page flow outside Filter. Routine cached/source/search details move
into Filter, while a stale, incomplete or unavailable observation retains a
visible status even when no separate error note is present. The non-browser
fallback result header remains unchanged.

The catalog wire parser only accepts an `updating` freshness state with partial
completeness, which is already announced outside Filter by the discovery notice.
The renderer also retains a typed freshness fallback when neither a request nor
the discovery summary supplies that notice.

No native transport, authorization, caching, polling, retry timer, pagination or
query contract changes are proposed. The existing Filter, Select, dialog and
focus-return owners remain in use; no alternate drawer or host DOM mutation is
introduced.

Review should cover the existing phone and desktop browser scenarios in light and
dark themes, including source qualification, metadata-search qualification,
inline warning visibility, clearing, Back, retained focus and list scroll. At
390px the fixture uses a short touch viewport and verifies the Filter context
shares the existing `.vb` scroll owner. It also checks both the Filter action's
focus return and browser Back behavior, plus clearing the query and facets. The
browser screenshot artifacts are diagnostic review material. The six actual
open-Filter comparisons extend the existing 62 states to 68; the historical
before candidate remains limited to its original 62. The 68-state gate compares
candidate and pinned selected-reference captures with the same dialog open,
close button focused, and Filter scroll body at the top. The required 113 full
screenshots and 162 strict regions, all thresholds, references and approval rules
remain unchanged. Any resulting visual difference stays subject to explicit
review and approval. A passing fixture suite or request-budget run does not accept
the visual change. Final acceptance must review this qualified native placement
against the exact source pin and the six open-Filter capture pairs.

Asset preparation at `5ac406349f7a7a933e60fe097d92b8589d8db55a` ran in
[CI 39074023198](https://github.com/onsager-ai/semon/actions/runs/39074023198).
Fixture equivalence, formatting, architecture, design and types passed. Of 147
unit tests, 146 passed; the sole failure was the expected stale generated viewer.
The subsequent freshness/size and browser checks were unrun. The fallback build
produced [artifact 12115279867](https://github.com/onsager-ai/semon/actions/runs/39074023198/artifacts/12115279867)
(160262 bytes; ZIP SHA256
`4979b409a1e84788d934ab74620386f7d1135421ff46fa5f65a3a097a3d60245`).
The exact 371606-byte generated viewer is adopted for the next full check, SHA256
`d1ed3e6927d4a6f143d246a28f7047d9f69101f2e9809d3823ba0bd20f0a3818`.
Shared and shell bundles are unchanged. Asset preparation is not acceptance.
