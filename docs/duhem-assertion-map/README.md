# Semon #501 assertion/state to owner migration map

## Purpose and scope

This directory is Plan item 1 of [onsager-ai/semon#501](https://github.com/onsager-ai/semon/issues/501): the consumer-side map of Semon assertions and states to their proposed owner (Duhem VD, retained native, or deferred), as required by Duhem's migration contract.

- Semon source ref: `893dd56` (`origin/main` at `893dd56fdc63fd53b7febd597c1381570c01f64b`).
- Duhem baseline: Duhem v0.7.0 (506774a), [release v0.7.0](https://github.com/onsager-ai/duhem/releases/tag/v0.7.0). Capability inventory and the consumer-map field list: [docs/product-regression-migration.md at c78b38d](https://github.com/onsager-ai/duhem/blob/c78b38d/docs/product-regression-migration.md).
- The map is inventory and proposed disposition only. Nothing has migrated or qualified, no Duhem VD has run against these assertions, and the proposed `M2.*` ids are map ids, not new tests or thresholds.
- Capability facts used: `ui/assert-state` is removed in v0.7.0 (assert state markers with `ui/assert-element`); `api/poll`, `db/observe` and `ui/select` are kept; `duhem coverage` and `claim_scope` ship in v0.7.0. Duhem #531, #544, #546, #535, #567 and #574 are satisfied by M1 and v0.7.0 and are not blockers. Frame/iframe locators (Duhem #568) merged at e26c210 after v0.7.0: they are post-v0.7.0 (#568) and deferred until the next Duhem release, not a missing capability.
- Citations: absolute draft paths were converted to repo-relative links to the Semon files at the source ref. Citations given without a directory (for example `checks/live.mjs:1-16`, relative to `tests/ui` per each file's intro) or inside compound spans stay plain text.
- Nothing was run to produce the map (no cargo, node, Playwright, build or typecheck); it comes from reading the source tree and CI workflows.

## Columns

Every assertion row records the eight fields required by the Duhem "Consumer assertion map":

| Field | What it records |
|---|---|
| Legacy owner | Old case/assertion/state identifier, source path, and responsible team or owner |
| Source status | Active, deferred/paused, disabled-boundary, internal, or historical; if an old case is retired, the owner and reason |
| VD mapping | Stable criterion, check ID, parameter instance, input/fixture, and required starting state |
| Disposition | `VD` (migrated), `native` (retained at its current layer), or `deferred` (with the missing capability or scope decision) |
| Claim scope | What the result claims, including real versus synthetic inputs, services, authentication, browser, and any deliberate boundary |
| Evidence | Expected and actual values, trace/artifact needed, secret handling, and how a reviewer inspects the result |
| Version and pin | Duhem commit, package/schema version, action contract, and any browser/font/measurement pins relevant to the claim |
| CI owner | Product repository workflow/profile, owning team, and whether the row is required, diagnostic, or retained-native |

The two catalog-viewer files use one extra leading "Behavior domain" column (nine columns). "Disposition" is a proposal: `VD` here means "VD candidate", not migrated. Disposition and status cells are free text and often compound.

## Index

Row counts are real data rows of the assertion tables (header and separator rows excluded; candidate-check, pilot-stage, gap and moved-row tables excluded), counted mechanically after deduplication. The last column is the draft count before deduplication.

| File | Scope | Rows (after dedup) | Rows in draft |
|---|---|---:|---:|
| [rust-and-cli.md](rust-and-cli.md) | Rust, CLI, Push/Receive, Catalog/selected read, paging, recovery/revocation, MCP and remaining non-browser domains (plus 7 proposed M2 candidate checks, not counted as rows) | 39 | 39 |
| [browser-navigation.md](browser-navigation.md) | Browser navigation group: approval-reviews, home, bar, turns, analytics, copied-usage, copilot, runview, signals, models, stepnames, sidebarfocus | 46 | 46 |
| [browser-transcript.md](browser-transcript.md) | Browser transcript group: viewer, md, full, check-real, extras, attach, viewer-backlog, deeplink-live, codex-thinking, selected-fixture | 77 | 79 |
| [browser-shell-accessibility.md](browser-shell-accessibility.md) | Browser shell and accessibility suites (shell, names, hicons, sidebar, embedsidebar, agentfont, tap/text audit support) | 19 | 19 |
| [browser-interaction.md](browser-interaction.md) | Browser interaction group: panel/account lifecycle, tokens, scrollbars, switch, embed, tooltip, details, select, filters, breakdown, tracebrief (the shared `catalog-filter-check.mjs` row is a pointer to the live/performance/catalog file) | 13 | 14 |
| [browser-live-performance-catalog.md](browser-live-performance-catalog.md) | Live, Catalog, order, errnav, performance (perf, budget, perf-refresh), pushed-parity, native helpers, capture infrastructure; canonical for catalog-paging-check, catalog-filter-check and long.mjs/perf | 20 | 20 |
| [browser-visual-and-suite.md](browser-visual-and-suite.md) | Visual acceptance, preservation, registry and suite infrastructure under tests/ui | 23 | 23 |
| [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md) | ui/tests/catalog-viewer-browser.mjs (mocked catalog routes, whole file) | 24 | 24 |
| [ui-served-scripts.md](ui-served-scripts.md) | ui/tests/*.mjs served/browser scripts (overview-served, catalog-served, catalog-journey, lifecycle and component browser scripts); canonical for these | 32 | 32 |
| [ui-unit-and-checks.md](ui-unit-and-checks.md) | ui/tests/*.test.mjs unit tests and ui/*.mjs check scripts; plus the three canonical `catalog-viewer-browser.mjs` rows for lines 1501-2140, 2141-2336 and the screenshot artifacts | 56 | 80 |
| **Total** | | **349** | **376** |

Deduplication removed 27 rows (2 from browser-transcript.md, 1 from browser-interaction.md, 24 from ui-unit-and-checks.md); see Duplicates resolved below.

## Totals

Counted with a script over the 349 data rows of the assertion tables, reading the cell under the "Source status" and "Disposition" headers and classifying by the first word of the cell (disposition: `defer*`, `native`/`retain*`, `VD`/`partial VD`/`M2 VD candidate`, `n/a`; status: the five documented values). The script was re-run on the committed 360-row version and reproduced the previous totals exactly before it was re-run on the updated rows. Approximate where cells are compound: 64 disposition cells mention two or more of VD/native/deferred (for example "VD for DOM facts; native for keyboard"), and a first-word count assigns each to its leading category only. The "other" disposition cells are free text (for example "Keep as native/VD consumer unit coverage"). The gap-wording cleanup after the final Duhem rulings changed no row's first disposition word or status, so these totals are unchanged. The previous stray status `docs` is fixed, so the status "other" count is now 0. "not an assertion" is counted under n/a.

| Disposition (first word of the cell) | Rows |
|---|---:|
| VD | 75 |
| native | 204 |
| deferred | 62 |
| n/a | 4 |
| other | 4 |
| **Total** | **349** |

| Source status (first word of the cell) | Rows |
|---|---:|
| active | 225 |
| internal | 89 |
| deferred-paused | 19 |
| disabled-boundary | 8 |
| historical | 8 |
| other | 0 |
| **Total** | **349** |

Per file (same method):

| File | VD | native | deferred | n/a | other | active | internal | deferred-paused | disabled-boundary | historical | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rust-and-cli.md | 15 | 20 | 3 | 0 | 1 | 20 | 13 | 3 | 2 | 1 | 0 |
| browser-navigation.md | 1 | 37 | 8 | 0 | 0 | 42 | 0 | 0 | 4 | 0 | 0 |
| browser-transcript.md | 39 | 32 | 2 | 3 | 1 | 73 | 1 | 0 | 0 | 3 | 0 |
| browser-shell-accessibility.md | 0 | 4 | 15 | 0 | 0 | 15 | 4 | 0 | 0 | 0 | 0 |
| browser-interaction.md | 1 | 1 | 11 | 0 | 0 | 12 | 1 | 0 | 0 | 0 | 0 |
| browser-live-performance-catalog.md | 9 | 9 | 0 | 0 | 2 | 20 | 0 | 0 | 0 | 0 | 0 |
| browser-visual-and-suite.md | 0 | 16 | 6 | 1 | 0 | 11 | 10 | 0 | 0 | 2 | 0 |
| ui-catalog-viewer-browser.md | 0 | 24 | 0 | 0 | 0 | 22 | 2 | 0 | 0 | 0 | 0 |
| ui-served-scripts.md | 10 | 19 | 3 | 0 | 0 | 9 | 17 | 3 | 2 | 1 | 0 |
| ui-unit-and-checks.md | 0 | 42 | 14 | 0 | 0 | 1 | 41 | 13 | 0 | 1 | 0 |

Changes against the previous totals come from the Duhem review (rust-and-cli.md MCP row to native, browser-navigation.md copied-usage row to VD via `cli/invoke` facts, ui-served-scripts.md component rows to native), from 11 duplicate rows turned into pointers (see Resolved classification conflicts) and from the fixed `docs` status.

## Duplicates resolved

The canonical row stays in one file; the other file points to it.

- `ui/tests/*.mjs` browser scripts: canonical in [ui-served-scripts.md](ui-served-scripts.md) and [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md). 14 non-conflicting duplicate rows (browser-lifecycle, application-lifecycle x3, native-shell-lifecycle, select-lifecycle x2, control-observation-browser x2, runtime-leaf deferred row, catalog-served x2, overview-served x2) were moved out of [ui-unit-and-checks.md](ui-unit-and-checks.md) into its "Duplicate rows moved" pointer table (not counted). The 10 rows that had a classification conflict were resolved to one canonical row each and moved to the same pointer table (see Resolved classification conflicts).
- `tests/ui/catalog-paging-check.mjs`: canonical in [browser-live-performance-catalog.md](browser-live-performance-catalog.md). The browser-transcript.md `extras / catalogPagingCheck` row was removed (both views say VD-partial; no conflict).
- `tests/ui/long.mjs`: performance (live-perf) is canonical. The browser-transcript.md `long.mjs` out-of-slice row was removed; `long.mjs` is imported only by `budget.mjs:32` and `perf.mjs:15`, covered by the live-perf rows.
- `tests/ui/catalog-filter-check.mjs`: canonical in browser-live-performance-catalog.md; the browser-interaction.md row is now a "See browser-live-performance-catalog.md" pointer (Resolved classification conflicts below).

### Resolved classification conflicts

Each decision follows the code and workflow at this ref: whether CI executes the file (`.github/workflows/ui.yml` or `ui/package.json`), and whether the assertions judge served-product behavior or an internal/diagnostic contract. The canonical row stays; the duplicate is a "See <file>" pointer.

- `catalog-filter-check.mjs`: canonical is the row in [browser-live-performance-catalog.md](browser-live-performance-catalog.md) ("VD for visible list/URL/status scalars; retain native fixture and interaction", tagged M3 #580 interaction and observation outputs). The [browser-interaction.md](browser-interaction.md) row is now a pointer. The two views agree once read together (scalars VD, interaction and request observation not yet). Evidence: it is imported by `tests/ui/checks/filters.mjs:1` and `tests/ui/checks/select.mjs:1`, both in the `interaction` group (`tests/ui/suite-plan.mjs:38-39`), run in CI by `.github/workflows/ui.yml:290-293` (`node checks/run.mjs --group "$SUITE"`).
- `catalog-journey.mjs`: resolved to `historical` (manual real-binary measurement harness, not CI coverage). Canonical is the [ui-served-scripts.md](ui-served-scripts.md) row (status corrected from active; disposition text kept), the two [ui-unit-and-checks.md](ui-unit-and-checks.md) rows are pointers. Evidence: the only invocation is `scripts/catalog-journey.py:160` (`ui/tests/catalog-journey.mjs` as default driver); `.github/workflows/ui.yml` has no match and `ui/package.json:8-19` lists no script for it. Its metrics are tagged M4 #582.
- `overview-served.mjs` screenshots: resolved to `active` diagnostic evidence, native diagnostic artifact. Canonical is the [ui-served-scripts.md](ui-served-scripts.md) row; the unit row "historical; deferred visual comparator" is a pointer (a pixel comparator would be M3 #581 visual, and is not proposed). Evidence: the file runs in CI at `.github/workflows/ui.yml:240` (`node --test ui/tests/catalog-served.mjs ui/tests/overview-served.mjs`, with `SEMON_OVERVIEW_VISUAL_OUT` at `:238`); the screenshots carry no assertion, so only the artifact is diagnostic.
- `code-scroll.mjs`, `jump-toolbar.mjs`, `continuity-refinement-browser.mjs`, `staging-viewer-browser.mjs`: resolved to `internal`, disposition native (retained; Duhem does not mock the web); canonical is the [ui-served-scripts.md](ui-served-scripts.md) row (status and disposition corrected from "active synthetic ... VD for DOM/geometry"), the unit rows are pointers. Evidence: CI-run at `.github/workflows/ui.yml:230`, but each test builds a component bundle with esbuild and serves it from a `page.route`-mocked `*.test` origin (`ui/tests/code-scroll.mjs:28`, `jump-toolbar.mjs:31`, `continuity-refinement-browser.mjs:39`, `staging-viewer-browser.mjs:78`), so they judge a UI-package component contract, not the served product.
- `passive-reading-browser.mjs`: one canonical row in [ui-served-scripts.md](ui-served-scripts.md): `disabled-boundary` for the no-active-controls assertions (`:26-166`) plus `internal` for the Jump interaction (`:132-166`), native (retained; Duhem does not mock the web). The two unit rows are pointers. Evidence: CI-run at `.github/workflows/ui.yml:230`; origin is `page.route`-mocked (`passive-reading-browser.mjs:38`).
- `runtime-leaf-browser.mjs` unlinked mirrored history (`:189`): resolved to `internal`, native; canonical is the [ui-served-scripts.md](ui-served-scripts.md) row, the unit "disabled-boundary" row is a pointer. Evidence: the test (`runtime-leaf-browser.mjs:189`, "unlinked mirrored history has no runtime warning block") asserts a saved-reading display contract on a `page.route`-mocked origin (`:23`), not the refusal of a held runtime feature; the first test in the file (`:19`) stays `deferred-paused` under the hold.
- `catalog-viewer-browser.mjs`: ui-unit-and-checks.md stays canonical for three ranges the 24 rows in [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md) do not cover. Row 60 is canonical for lines 1501-2140 (tests at `:1533` "Codex last-turn menu and tooltip" and `:1666` "catalog boot, selection and retained native ranges", umbrella level only, status corrected from `internal` to `active` to match the canonical rows; same full UI bundle on `http://catalog.test` routes, `:123`). Row 61 is canonical for `:2141-2336` ("temporary boot failure and pending projection recover", deferred-paused: it drives a synthetic runtime EventSource under the Hub #267/#270 hold). Row 62 is canonical for the screenshot/geometry artifacts (historical diagnostic, now tagged M3 #581 visual): `SEMON_UI_OUT` is set at `.github/workflows/ui.yml:172` and screenshots are written at `catalog-viewer-browser.mjs:484-490`, but no assertion judges them. The file runs at `.github/workflows/ui.yml:230`.
- Source status `docs` on the README documentation row in [browser-visual-and-suite.md](browser-visual-and-suite.md): changed to `internal` (documentation row; not an assertion). It is not a fixture assertion, so no other status fits; the original value is recorded in the cell.

## Candidate M2 real-binary pilot checks

Journey (Duhem #501 pilot): native homes, Push, Receive, Catalog/selected read, transcript paging, append/restart/recovery/revocation. The pilot table stays as is. The Rust `M2.*` ids are the backbone (full table in [rust-and-cli.md](rust-and-cli.md#candidate-real-binary-m2-pilot-checks)); browser/UI rows below are counterparts, none of which covers the complete journey. `pushed-parity.mjs` is the strongest browser counterpart once #579 (merged at Duhem main aff6eaf, unreleased) lands; Push, Receive and the pushed-parity receiver all need that owned background process. Source-byte integrity checks and the controlled append of a real source range (a product fixture helper) in the pilot are authored as `cli/invoke` facts plus assertions. All are proposals on synthetic inputs and the real Semon binary.

| Stage | Rust backbone (proposed) | Browser / UI counterparts (existing rows) |
|---|---|---|
| Native homes | `M2.native_homes.configured_roots`, `M2.native_homes.archived_codex` | `checks/live-catalog.mjs:6-120` (harbor home) and `native-fixture.mjs:10-77` (browser-live-performance-catalog.md); `pushed-parity.mjs:208-300` fixture home; copilot and copied-usage sanitized homes (browser-navigation.md); `catalog-served.mjs` native-format fixtures (ui-served-scripts.md) |
| Push | `M2.push.redacted_selection` | `pushed-parity.mjs` default mode, whose `semon push --watch` and receiver are started by workflow shell at `ui.yml:392-418`; a Duhem pilot needs an owned background process, #579 (merged at Duhem main aff6eaf, unreleased); transport job `ui.yml:389-457` |
| Receive | `M2.receive.authenticated_append` | `pushed-parity.mjs:300-418` received-copy parity and `--native-pages` `:37-206` (the strongest browser pilot candidates); pushed visual and two-machines rows (browser-visual-and-suite.md, `ui.yml:420-460`) |
| Catalog / selected read | `M2.catalog.selected_read` | `live-catalog.mjs`, `catalog-served.mjs`, `overview-served.mjs`; attach api-references/entry-headers/entry-refusals, extras code-mode/yielded-ui and deeplink anchor-landing (browser-transcript.md); approval-reviews, home, turns, copied-usage, runview (browser-navigation.md); mocked `catalog-v1` contract rows (ui-catalog-viewer-browser.md, native-retained, no real binary) |
| Paging | `M2.paging.generation_bound_transcript` | `catalog-paging-check.mjs:5-75` (canonical in live-perf file), `pushed-parity.mjs:145-166, 217-263`, `budget.mjs` older-page walk, `errnav.mjs`, `catalog-served.mjs`, `catalog-journey.mjs` history counts 0/256/2048 (historical manual harness, not CI), mocked cursor/Load-more rows |
| Append / restart / recovery | `M2.recovery.append_restart_catchup` | `catalog-journey.mjs` first-import and persisted-restart (historical manual harness, not CI; see Resolved classification conflicts); `live-catalog.mjs`, `order.mjs`, `errnav.mjs` local appends and route-away/back recovery; `native-fixture.mjs`/`native-order-logs.mjs` append helpers; mocked 409 `stale_projection` recovery rows; the controlled append of a real source range is pilot via cli/invoke facts (product fixture helper), part of the M2 recovery checks. Browser-transcript `selectedFixture` rows are client-faked stand-ins and the copilot append/restart row is a local parser check; neither is M2 recovery evidence |
| Revocation | `M2.revocation.refuse_subsequent_write` | None in the browser slices. The mocked capability-loss row (ui-catalog-viewer-browser.md) is only a stand-in |

Slices with no pilot relevance: [browser-shell-accessibility.md](browser-shell-accessibility.md), [browser-interaction.md](browser-interaction.md) (the `switch` and `tracebrief` rows touch retained-history reads on synthetic fixtures only) and [ui-unit-and-checks.md](ui-unit-and-checks.md) (native synthetic contract tests).

## Capability gaps

Rulings from the Duhem Manager's capability-gap review, relayed by the Semon Manager; they supersede the earlier "NEW GAP" tagging. History: earlier `NEW GAP` tags were resolved by these rulings, and the former `NEW GAP` wording in the area files is now rewritten to the bucket names below. Row counts are approximate and counted mechanically from the updated rows by literal tags in the Disposition cells (a row can carry several tags, so buckets overlap): `#579 (merged at Duhem main aff6eaf, unreleased)`, `cli/invoke facts`, `Duhem does not mock the web`, `M3 #580 interaction`, `M3 observation outputs`, `M3 #581 visual`, `M4 #582 metrics`, `kept native for now`. Rows that are already native with no gap wording (for example Rust fault-injection rows) were not retagged, so identity-boundary counts are a floor.

Duhem Manager review (2026-10-30): map is complete enough for M2 from Duhem's side.

### Owned processes (#579, merged; unreleased)

Owned background process lifecycle (identity, readiness, logs, stop): Duhem #579 was the M2 blocker and is now merged at Duhem main `aff6eaf` (#604), adding `process/start`, `process/inspect`, `process/stop` and `process/restart` (Linux/macOS only; mutation limited to the check/fixture; cleanup always required; see [docs/owned-processes.md](https://github.com/onsager-ai/duhem/blob/aff6eaf/docs/owned-processes.md)). It is not in an npm release yet, so the M2 Receive/restart rows pin Duhem main `aff6eaf` until the next release; other rows keep the v0.7.0 baseline. Fixed loopback ports stay product-owned until #410. Receive, Push `--watch` and the pushed-parity receiver need it. Process-level faults (kill or restart the receiver) also go through #579, not mocking; those rows read "deferred → real-process fault via #579". About 17 rows are tagged `#579 (merged at Duhem main aff6eaf, unreleased)`: rust-and-cli 7, browser-live-performance-catalog 6, browser-visual-and-suite 2, browser-navigation 1 (Copilot append/restart), ui-served-scripts 1 (historical catalog journey); 3 of them are the real-process-fault rows (rust-and-cli 2, browser-navigation 1). `semon mcp` is not a #579 case (see the kept-native bucket).

### Pilot workarounds (cli/invoke facts)

Filesystem read/hash/list/compare (source-byte integrity, mirror comparison, unchanged-source, image/font provenance) is not an M2 blocker. For the pilot, author it as `cli/invoke` returning explicit facts (for example a hash) plus assertions; description: pilot via cli/invoke facts; possible future fs/* action (Duhem may spec a small `fs/*` fact action if the pattern proves repetitive). The earlier "NEW GAP" for this item is removed. Two further items are covered by the same pilot: independent fixture/model facts (a product helper returns structured facts and the VD asserts on them; browser-navigation turns) and the controlled append of a real source range (product fixture helper, part of the M2 recovery checks; browser-transcript codex-thinking and `selectedFixture`). About 13 rows are tagged: rust-and-cli 7 (plus 2 rows in the candidate-check table), browser-navigation 3 (copied-usage, Copilot, turns), browser-transcript 2 (codex-thinking append, `selectedFixture`), browser-shell-accessibility 1 (`hicons` asset hash). localStorage reads are not a gap: `ui/capture-session` already returns storage state, so the VD asserts on its outputs (browser-transcript `extras` result-seen-state row is a partial VD; its other obstacle is the retained init-script storage fault). The review estimated about 11 Rust rows; the other Rust rows with filesystem wording are native fault-injection or parser rows that carried no gap wording and were not changed.

### Retained native by Duhem identity boundary

Duhem does not mock the web at verification time (an existing commitment). Route hold/fulfill/abort/patch, synthetic 409/503 responses, a controllable clock or page-visibility override, and init-script/pre-load hooks are therefore not Duhem gaps; the rows read "native (retained; Duhem does not mock the web)". Two retained-native rulings use a different phrase: the embed mount/destroy row (browser-interaction) is "native (retained; component-level host contract, not a system claim)" and the `gaps.mjs` set comparison (browser-visual-and-suite) is "native (retained for now; Duhem adds a small `$runtime` set-equality helper if a VD needs it)". About 40 rows carry the mocked-web phrase: browser-transcript 13, ui-catalog-viewer-browser 14, ui-served-scripts 6, browser-navigation 2, browser-interaction 2, browser-live-performance-catalog 2, browser-shell-accessibility 1. A row that also has a real-process fault says "deferred → real-process fault via #579" instead. These items are removed from the gap lists.

### M3 candidates (#580, observation outputs, #581)

In priority order:

1. #580 interaction (keyboard, pointer/touch, scroll, focus, history Back/Forward, geometry/computed-style extraction): about 95 rows tagged `M3 #580 interaction`, which now includes browser-context emulation (viewport, prefers-color-scheme), a client environment setting rather than web mocking (browser-transcript 28, browser-navigation 15, browser-shell-accessibility 12, ui-served-scripts 11, browser-interaction 10, ui-catalog-viewer-browser 10, browser-live-performance-catalog 6, browser-visual-and-suite 3; total 95 counted mechanically).
2. M3 observation outputs (no Duhem issue number yet): request count/order on `api/observe`, page-error/console capture, response headers and byte length on `api/call`. About 47 rows tagged (ui-catalog-viewer-browser 15, browser-transcript 11, browser-live-performance-catalog 9, browser-navigation 4, browser-shell-accessibility 4, browser-interaction 4); the earlier "network/request observation" bucket (about 40) and page-error observation (about 9) fall here.
3. #581 visual (bound-reference pixel comparison, threshold, provenance, diff): about 12 rows tagged `M3 #581 visual` (browser-visual-and-suite 6, ui-served-scripts 2, browser-live-performance-catalog 2 capture-paint rows, browser-interaction 1, ui-unit-and-checks 1). The capture-paint rows (animation pause, stable frame) are the stable-capture prerequisite for visual comparison.

### M4 (#582; #410 low priority)

#582 metrics (sampler, window, comparator) stays deferred: about 6 rows tagged `M4 #582 metrics` (browser-live-performance-catalog 4, browser-navigation 1, ui-served-scripts 1 for the catalog journey). #410 resource leases are low priority, with 0 rows needing them (rust-and-cli keeps the current CI shards).

### Kept native for now (MCP stdio, clipboard, element census)

- Interactive stdio/JSON-RPC (MCP): disposition native, kept for now; `cli/invoke` is a one-command invocation. 2 rows (rust-and-cli: the MCP protocol row and the MCP framing row).
- Clipboard read/write: native, kept for now. 3 rows (browser-navigation Copilot resume command, browser-shell-accessibility shell sheet, ui-served-scripts native shell confirmation).
- Allowlist-filtered element census and scripted exhaustive walks: native, kept for now. 3 rows (browser-transcript md census, viewer phone census, extras injection walk).

## Runtime hold

semon-runtime, E2B, SSH, managed or cloud runtime and control-delivery cases are classified `deferred-paused` or `disabled-boundary` only, under the hold from Hub #267 and Hub #270. No work is proposed for them, and classification does not imply that active-feature tests passed or should be run as acceptance. Disabled-boundary refusal checks stay required native. The rows are in rust-and-cli.md (runtime/E2B/SSH/control), ui-unit-and-checks.md (runtime observation, control stream), ui-served-scripts.md and the moved/kept browser-script rows.

## Findings to act on

1. [`tests/ui/README.md:60`](../../tests/ui/README.md#L60) says the #380 visual/transport CI "additionally runs `visual-pathname-audit.mjs`", but `.github/workflows/ui.yml` has no step that runs it; only `visual-pathname-audit.test.mjs` runs, in `foundation` ([`.github/workflows/ui.yml:66`](../../.github/workflows/ui.yml#L66)). See [browser-visual-and-suite.md](browser-visual-and-suite.md).
2. `ui/tests/catalog-journey.mjs` is invoked only through `/scripts/catalog-journey.py`; neither `.github/workflows/ui.yml` nor [`ui/package.json:8-19`](../../ui/package.json#L8-L19) runs it, so it is not CI coverage. Resolved to historical (see Resolved classification conflicts); its canonical row is in [ui-served-scripts.md](ui-served-scripts.md).
3. CI pins Playwright 1.63.0 ([`tests/ui/package.json:17`](../../tests/ui/package.json#L17)) and installs Chromium with `npx playwright install --with-deps chromium` ([`.github/workflows/ui.yml:209-213`](../../.github/workflows/ui.yml#L209-L213)), but pins no explicit Chromium revision, OS image or font digest for the measurement claims.
4. The visual acceptance registry (`tests/ui/visual-acceptance.json`) has 62 preservation records (`preservation/required/frame 62`) against 68 required states; the 6 states without a record are unwaived and unverified (the slice presumed them within threshold but did not check each). See [browser-visual-and-suite.md](browser-visual-and-suite.md).
5. The pushed visual scope has no acceptance records ([`tests/ui/visual-coverage.json:14-18`](../../tests/ui/visual-coverage.json#L14-L18), [`.github/workflows/ui.yml:420-460`](../../.github/workflows/ui.yml#L420-L460); registry scope counts main 93, required 62, two-machines 6, pushed 0), and it compares the served pushed copy against the same machine's own viewer rather than a reviewed reference.

## Open questions

Each file keeps its own "Uncertain classifications" section; the main items are collected here.

- Classification conflicts: all resolved above. Not decided: `catalog-viewer-browser.mjs` lines 1501-2140 are covered only by the umbrella row 60 in ui-unit-and-checks.md (tests at `:1533` and `:1666`); per-assertion rows were not authored, which needs a source read of about 600 lines.
- Identity-boundary count: the review estimated about 50 rows; 40 rows now carry the retained-native phrase. The remaining rows are already native without gap wording or mix the item into other tags (for example mocked-route rows in ui-catalog-viewer-browser.md without gap text); say if they should be retagged.
- Required-ness: whether the `viewer / <suite>` matrix legs and the `transport` job are branch-protection required status checks is not visible from the repository (browser-transcript.md; ui.yml aggregate job `browser` requires all suites).
- [rust-and-cli.md](rust-and-cli.md): confirm the `semon-control` CI owner; spike scripts under `tests/spikes` are labelled historical unless a workflow calls them; `real-ssh-tests` is deferred-paused; grouped rows are module summaries, not one row per `#[test]`.
- [browser-navigation.md](browser-navigation.md): the Copilot append/restart row is a local parser fixture, not M2 recovery evidence; local fixture process behavior is not a managed/cloud runtime claim.
- [browser-transcript.md](browser-transcript.md): `selectedFixture` rows are HYBRID (real source identity and routes, synthetic rows and recomputed generation), so whether a VD may call them "real" is a scope decision; the md waiting-card and attach embedding rows are conditional; headers in viewer/full/extras describe assertions no code makes (classified historical); viewer-backlog analytics/model rows may duplicate navigation-group checks (not verified).
- [browser-shell-accessibility.md](browser-shell-accessibility.md): `tap-hit.test.mjs` and `text-audit.test.mjs` are internal measurement tests classified native.
- [browser-interaction.md](browser-interaction.md): `embed` is classified internal (owner layer) although it runs in CI; screenshots there are diagnostic only.
- [browser-live-performance-catalog.md](browser-live-performance-catalog.md): `live.mjs` is a wrapper, not a second assertion set; budget thresholds are report-only so "active" does not mean enforced; the first ten rows were verified only by spot checks; `perf-refresh.mjs` is inactive in the default run; `pushed-parity` default mode is VD only if Duhem owns the receiver and push processes.
- [browser-visual-and-suite.md](browser-visual-and-suite.md): whether the pathname-audit and binding-proposal tools run in CI (finding 1); `gaps.mjs` is native (retained for now; Duhem adds a small `$runtime` set-equality helper if a VD needs it); region counts (162/20/24) and frame counts (113/12/15) were recomputed with jq, the acceptance registry only through bounded summaries.
- [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md), [ui-served-scripts.md](ui-served-scripts.md), [ui-unit-and-checks.md](ui-unit-and-checks.md): all `catalog.*` ids and `native` dispositions are proposals; `catalog-viewer-browser.mjs` rows 3410/3560 may be better retained-native diagnostics; `continuity-refinement-browser.mjs` `SEMON_REFINEMENT_BEFORE=1` has no visible CI configuration; `runtime-leaf-browser.mjs` is deferred-paused by the hold, not evidence of a real managed-runtime integration.
