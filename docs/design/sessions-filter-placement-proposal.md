# Sessions Filter placement proposal

Status: candidate for review only. The source changes on this branch do not
approve the placement or authorize a visual-baseline update.

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
the visual change. Whether moving the routine explanation out of the
always-visible footer is acceptable remains an open placement decision.

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
