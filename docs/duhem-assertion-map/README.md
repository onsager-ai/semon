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
| [browser-interaction.md](browser-interaction.md) | Browser interaction group: panel/account lifecycle, tokens, scrollbars, switch, embed, tooltip, details, select, filters, breakdown, tracebrief | 14 | 14 |
| [browser-live-performance-catalog.md](browser-live-performance-catalog.md) | Live, Catalog, order, errnav, performance (perf, budget, perf-refresh), pushed-parity, native helpers, capture infrastructure; canonical for catalog-paging-check, catalog-filter-check and long.mjs/perf | 20 | 20 |
| [browser-visual-and-suite.md](browser-visual-and-suite.md) | Visual acceptance, preservation, registry and suite infrastructure under tests/ui | 23 | 23 |
| [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md) | ui/tests/catalog-viewer-browser.mjs (mocked catalog routes, whole file) | 24 | 24 |
| [ui-served-scripts.md](ui-served-scripts.md) | ui/tests/*.mjs served/browser scripts (overview-served, catalog-served, catalog-journey, lifecycle and component browser scripts); canonical for these | 32 | 32 |
| [ui-unit-and-checks.md](ui-unit-and-checks.md) | ui/tests/*.test.mjs unit tests and ui/*.mjs check scripts; plus kept conflict/uncovered rows for browser scripts | 66 | 80 |
| **Total** | | **360** | **376** |

Deduplication removed 16 rows (2 from browser-transcript.md, 14 from ui-unit-and-checks.md); see Duplicates below.

## Totals

Counted with a script over the 360 data rows of the assertion tables, reading the cell under the "Source status" and "Disposition" headers and classifying by the first word of the cell (disposition: `defer*`, `native`/`retain*`, `VD`/`partial VD`/`M2 VD candidate`, `n/a`; status: the five documented values). Approximate where cells are compound: 63 disposition cells mention two or more of VD/native/deferred (for example "VD for DOM facts; native for keyboard"), and a first-word count assigns each to its leading category only. The "other" cells are free-text (for example "Keep as native/VD consumer unit coverage", "NEW GAP: ...", and one source status `docs`).

| Disposition (first word of the cell) | Rows |
|---|---:|
| VD | 80 |
| native | 203 |
| deferred | 69 |
| n/a | 4 |
| other | 4 |
| **Total** | **360** |

| Source status (first word of the cell) | Rows |
|---|---:|
| active | 230 |
| internal | 90 |
| deferred-paused | 19 |
| disabled-boundary | 10 |
| historical | 10 |
| other | 1 |
| **Total** | **360** |

Per file (same method):

| File | VD | native | deferred | n/a | other | active | internal | deferred-paused | disabled-boundary | historical | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rust-and-cli.md | 16 | 19 | 3 | 0 | 1 | 20 | 13 | 3 | 2 | 1 | 0 |
| browser-navigation.md | 0 | 35 | 11 | 0 | 0 | 42 | 0 | 0 | 4 | 0 | 0 |
| browser-transcript.md | 39 | 31 | 3 | 3 | 1 | 73 | 1 | 0 | 0 | 3 | 0 |
| browser-shell-accessibility.md | 0 | 4 | 15 | 0 | 0 | 15 | 4 | 0 | 0 | 0 | 0 |
| browser-interaction.md | 1 | 1 | 12 | 0 | 0 | 13 | 1 | 0 | 0 | 0 | 0 |
| browser-live-performance-catalog.md | 9 | 9 | 0 | 0 | 2 | 20 | 0 | 0 | 0 | 0 | 0 |
| browser-visual-and-suite.md | 0 | 16 | 6 | 1 | 0 | 11 | 9 | 0 | 0 | 2 | 1 |
| ui-catalog-viewer-browser.md | 0 | 24 | 0 | 0 | 0 | 22 | 2 | 0 | 0 | 0 | 0 |
| ui-served-scripts.md | 15 | 14 | 3 | 0 | 0 | 14 | 13 | 3 | 2 | 0 | 0 |
| ui-unit-and-checks.md | 0 | 50 | 16 | 0 | 0 | 0 | 47 | 13 | 2 | 4 | 0 |

## Duplicates resolved

The canonical row stays in one file; the other file points to it.

- `ui/tests/*.mjs` browser scripts: canonical in [ui-served-scripts.md](ui-served-scripts.md) and [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md). 14 non-conflicting duplicate rows (browser-lifecycle, application-lifecycle x3, native-shell-lifecycle, select-lifecycle x2, control-observation-browser x2, runtime-leaf deferred row, catalog-served x2, overview-served x2) were moved out of [ui-unit-and-checks.md](ui-unit-and-checks.md) into its "Duplicate rows moved" pointer table (not counted). Rows with a classification conflict were kept in both places.
- `tests/ui/catalog-paging-check.mjs`: canonical in [browser-live-performance-catalog.md](browser-live-performance-catalog.md). The browser-transcript.md `extras / catalogPagingCheck` row was removed (both views say VD-partial; no conflict).
- `tests/ui/long.mjs`: performance (live-perf) is canonical. The browser-transcript.md `long.mjs` out-of-slice row was removed; `long.mjs` is imported only by `budget.mjs:32` and `perf.mjs:15`, covered by the live-perf rows.
- `tests/ui/catalog-filter-check.mjs`: canonical in browser-live-performance-catalog.md; the browser-interaction.md row is kept because of a classification conflict (below).

### Classification conflicts (both views kept)

- `catalog-filter-check.mjs`: [browser-interaction.md](browser-interaction.md) deferred (interaction, request observation, geometry gaps) versus browser-live-performance-catalog.md "VD for visible list/URL/status scalars; retain native fixture and interaction".
- `catalog-journey.mjs`: ui-served-scripts.md row "active real-binary qualification harness; VD candidate" versus ui-unit-and-checks.md two rows "historical" (native read correctness; deferred metric sampler). Related to finding 2 below.
- `overview-served.mjs` screenshots: ui-served-scripts.md "active diagnostic evidence; native diagnostic artifact" versus ui-unit-and-checks.md "historical; deferred visual comparator".
- `code-scroll.mjs`, `jump-toolbar.mjs`, `continuity-refinement-browser.mjs`, `staging-viewer-browser.mjs`: ui-unit-and-checks.md "internal / native" versus ui-served-scripts.md "active synthetic component contract; VD for DOM/geometry facts, native for keyboard/touch".
- `passive-reading-browser.mjs`: ui-unit-and-checks.md splits disabled-boundary/native and internal/native; ui-served-scripts.md has one row "disabled-boundary plus active retained Jump; VD for DOM absence/text/geometry, native for keyboard/touch".
- `runtime-leaf-browser.mjs` unlinked mirrored history: ui-unit-and-checks.md "disabled-boundary / native" versus ui-served-scripts.md "internal / native" (same disposition, different status).
- `catalog-viewer-browser.mjs`: ui-unit-and-checks.md rows "internal / native", "deferred-paused" (runtime stream, line 2141) and "historical / deferred visual" (line 484) versus the 24 canonical rows (22 active, 2 internal; all native). The canonical rows cite no range for lines 1501-2336, so the line 2141 row has no canonical counterpart.

## Candidate M2 real-binary pilot checks

Journey (Duhem #501 pilot): native homes, Push, Receive, Catalog/selected read, transcript paging, append/restart/recovery/revocation. The Rust `M2.*` ids are the backbone (full table in [rust-and-cli.md](rust-and-cli.md#candidate-real-binary-m2-pilot-checks)); browser/UI rows below are counterparts, none of which covers the complete journey. All are proposals on synthetic inputs and the real Semon binary.

| Stage | Rust backbone (proposed) | Browser / UI counterparts (existing rows) |
|---|---|---|
| Native homes | `M2.native_homes.configured_roots`, `M2.native_homes.archived_codex` | `checks/live-catalog.mjs:6-120` (harbor home) and `native-fixture.mjs:10-77` (browser-live-performance-catalog.md); `pushed-parity.mjs:208-300` fixture home; copilot and copied-usage sanitized homes (browser-navigation.md); `catalog-served.mjs` native-format fixtures (ui-served-scripts.md) |
| Push | `M2.push.redacted_selection` | `pushed-parity.mjs` default mode, whose `semon push --watch` and receiver are started by workflow shell at `ui.yml:392-418` (a pilot must reproduce that setup); transport job `ui.yml:389-457` |
| Receive | `M2.receive.authenticated_append` | `pushed-parity.mjs:300-418` received-copy parity and `--native-pages` `:37-206` (the strongest browser pilot candidates); pushed visual and two-machines rows (browser-visual-and-suite.md, `ui.yml:420-460`) |
| Catalog / selected read | `M2.catalog.selected_read` | `live-catalog.mjs`, `catalog-served.mjs`, `overview-served.mjs`; attach api-references/entry-headers/entry-refusals, extras code-mode/yielded-ui and deeplink anchor-landing (browser-transcript.md); approval-reviews, home, turns, copied-usage, runview (browser-navigation.md); mocked `catalog-v1` contract rows (ui-catalog-viewer-browser.md, native-retained, no real binary) |
| Paging | `M2.paging.generation_bound_transcript` | `catalog-paging-check.mjs:5-75` (canonical in live-perf file), `pushed-parity.mjs:145-166, 217-263`, `budget.mjs` older-page walk, `errnav.mjs`, `catalog-served.mjs`, `catalog-journey.mjs` history counts 0/256/2048, mocked cursor/Load-more rows |
| Append / restart / recovery | `M2.recovery.append_restart_catchup` | `catalog-journey.mjs` first-import and persisted-restart (classification conflict above); `live-catalog.mjs`, `order.mjs`, `errnav.mjs` local appends and route-away/back recovery; `native-fixture.mjs`/`native-order-logs.mjs` append helpers; mocked 409 `stale_projection` recovery rows. Browser-transcript `selectedFixture` rows are client-faked stand-ins and the copilot append/restart row is a local parser check; neither is M2 recovery evidence |
| Revocation | `M2.revocation.refuse_subsequent_write` | None in the browser slices. The mocked capability-loss row (ui-catalog-viewer-browser.md) is only a stand-in |

Slices with no pilot relevance: [browser-shell-accessibility.md](browser-shell-accessibility.md), [browser-interaction.md](browser-interaction.md) (the `switch` and `tracebrief` rows touch retained-history reads on synthetic fixtures only) and [ui-unit-and-checks.md](ui-unit-and-checks.md) (native synthetic contract tests).

## Capability gaps

Counts are approximate: a row can need several capabilities, the slices counted by their own tagging, and categories overlap. "Rows tagged NEW GAP" by file (mechanical grep of table rows including candidate rows): rust-and-cli ~10, browser-navigation ~23, browser-transcript ~39, browser-shell-accessibility ~11, browser-interaction ~12, browser-live-performance-catalog ~19, browser-visual-and-suite ~6, ui-catalog-viewer-browser ~22, ui-served-scripts ~11, ui-unit-and-checks ~5.

### Existing gaps in the Duhem inventory

| Gap | Approx. rows | Files |
|---|---:|---|
| Owned background process lifecycle (identity, readiness, logs, stop) | ~20 | rust-and-cli (Receive, Push `--watch`, MCP, about 5), browser-navigation (3 files), browser-live-performance-catalog (6), browser-visual-and-suite (3), ui-served-scripts (1), ui-unit-and-checks (gap table: 4 owners, counted before moves), ui-catalog-viewer-browser (1) |
| Interaction: keyboard, pointer/touch, scroll, focus, computed geometry and hit testing | ~100+ | browser-navigation (~17 plus 3), browser-transcript (~16 geometry, ~6 keyboard, ~5 pointer/scroll), browser-shell-accessibility (~11), browser-interaction (8 interaction, 9 computed style/geometry), browser-live-performance-catalog (7), ui-served-scripts (11), ui-catalog-viewer-browser (~10), browser-visual-and-suite (3), ui-unit-and-checks (2) |
| Visual pixel comparison with bound reference, threshold, provenance and diff | ~20 | browser-visual-and-suite (7 plus registry governance), browser-interaction (8 diagnostic-screenshot rows), ui-unit-and-checks (3), ui-served-scripts (1), browser-shell-accessibility (~5 rendered-image measurements) |
| Metric sampler, window and comparator | ~9 | browser-live-performance-catalog (4: budget, perf, perf-refresh, perf-budget), browser-navigation (~3 analytics), ui-served-scripts (1), ui-unit-and-checks (1) |
| Resource leases | ~0 rows | rust-and-cli (keep current CI shards until a mapped parallel claim needs a lease) |
| Network/request observation (ordered request/event contract with product-defined count, order, provenance) | ~40 | browser-navigation (7), browser-interaction (7), ui-catalog-viewer-browser (~15), browser-live-performance-catalog (~9), browser-shell-accessibility (~3), rust-and-cli (2) |

### NEW GAP candidates

De-duplicated across files; none is in the c78b38d inventory.

| Candidate | Approx. rows | Files |
|---|---:|---|
| Filesystem read/hash/list/compare (source-byte integrity, mirror comparison, unchanged-source, image/font provenance) | ~15 | rust-and-cli (~11), browser-navigation (~3), browser-shell-accessibility (3) |
| Interactive stdio / JSON-RPC session (long-lived MCP process) | ~2 | rust-and-cli |
| Controllable clock / page-visibility override | ~17 | ui-catalog-viewer-browser (14), browser-transcript (2), browser-navigation (1) |
| Held/controlled responses and fault injection (route hold/fulfill/abort/patch, 409/503) | ~30 | browser-transcript (~12), ui-catalog-viewer-browser (~9 held, ~6 faults), browser-navigation (~7), browser-interaction (7), rust-and-cli (~7 native fault-injection rows stay native) |
| Browser history and `history.state` (Back/Forward, pushState/popstate) | ~15 | browser-transcript (~6), ui-catalog-viewer-browser (~6), browser-shell-accessibility and browser-interaction (a few) |
| Browser clipboard read/write | ~3 | browser-navigation (copilot resume command), ui-served-scripts (native shell confirmation) |
| Page-error / console observation | ~9+ | browser-transcript (shared `page.errors === []`) |
| Init-script / pre-load instrumentation (innerHTML hooks, Storage faults, scrollTop hooks) | ~3 | browser-transcript |
| Response headers and byte-length / digest evidence | ~3 | browser-transcript (2), browser-live-performance-catalog (session-trace evidence, 1) |
| Allowlist-filtered element census with attribute rules and scripted exhaustive walks | ~8 | browser-transcript (~3 census, ~5 walks) |
| Independent fixture/model facts | ~1 | browser-navigation (turns model parity) |
| Application mount/destroy and host-owned DOM lifecycle contract | ~1 | browser-interaction (embed) |
| Visual/paint capture (animation pause, stable frame, same-phase DOM evidence) | ~2 | browser-live-performance-catalog (capture-paint) |

## Runtime hold

semon-runtime, E2B, SSH, managed or cloud runtime and control-delivery cases are classified `deferred-paused` or `disabled-boundary` only, under the hold from Hub #267 and Hub #270. No work is proposed for them, and classification does not imply that active-feature tests passed or should be run as acceptance. Disabled-boundary refusal checks stay required native. The rows are in rust-and-cli.md (runtime/E2B/SSH/control), ui-unit-and-checks.md (runtime observation, control stream), ui-served-scripts.md and the moved/kept browser-script rows.

## Findings to act on

1. [`tests/ui/README.md:60`](../../tests/ui/README.md#L60) says the #380 visual/transport CI "additionally runs `visual-pathname-audit.mjs`", but `.github/workflows/ui.yml` has no step that runs it; only `visual-pathname-audit.test.mjs` runs, in `foundation` ([`.github/workflows/ui.yml:66`](../../.github/workflows/ui.yml#L66)). See [browser-visual-and-suite.md](browser-visual-and-suite.md).
2. `ui/tests/catalog-journey.mjs` is invoked only through `/scripts/catalog-journey.py`; neither `.github/workflows/ui.yml` nor [`ui/package.json:8-19`](../../ui/package.json#L8-L19) runs it, so its active CI profile is unresolved. See [ui-served-scripts.md](ui-served-scripts.md) and the conflict list above.
3. CI pins Playwright 1.63.0 ([`tests/ui/package.json:17`](../../tests/ui/package.json#L17)) and installs Chromium with `npx playwright install --with-deps chromium` ([`.github/workflows/ui.yml:209-213`](../../.github/workflows/ui.yml#L209-L213)), but pins no explicit Chromium revision, OS image or font digest for the measurement claims.
4. The visual acceptance registry (`tests/ui/visual-acceptance.json`) has 62 preservation records (`preservation/required/frame 62`) against 68 required states; the 6 states without a record are unwaived and unverified (the slice presumed them within threshold but did not check each). See [browser-visual-and-suite.md](browser-visual-and-suite.md).
5. The pushed visual scope has no acceptance records ([`tests/ui/visual-coverage.json:14-18`](../../tests/ui/visual-coverage.json#L14-L18), [`.github/workflows/ui.yml:420-460`](../../.github/workflows/ui.yml#L420-L460); registry scope counts main 93, required 62, two-machines 6, pushed 0), and it compares the served pushed copy against the same machine's own viewer rather than a reviewed reference.

## Open questions

Each file keeps its own "Uncertain classifications" section; the main items are collected here.

- Classification conflicts listed under Duplicates resolved need a Manager/owner decision on the canonical status.
- Required-ness: whether the `viewer / <suite>` matrix legs and the `transport` job are branch-protection required status checks is not visible from the repository (browser-transcript.md; ui.yml aggregate job `browser` requires all suites).
- Source-range coverage: [`ui/tests/catalog-viewer-browser.mjs:1501-2336`](../../ui/tests/catalog-viewer-browser.mjs#L1501-L2336) has no canonical row (ui-catalog-viewer-browser.md).
- One row in browser-visual-and-suite.md (the README documentation row) uses source status `docs`, outside the five documented values.
- [rust-and-cli.md](rust-and-cli.md): confirm the `semon-control` CI owner; spike scripts under `tests/spikes` are labelled historical unless a workflow calls them; `real-ssh-tests` is deferred-paused; grouped rows are module summaries, not one row per `#[test]`.
- [browser-navigation.md](browser-navigation.md): the Copilot append/restart row is a local parser fixture, not M2 recovery evidence; local fixture process behavior is not a managed/cloud runtime claim.
- [browser-transcript.md](browser-transcript.md): `selectedFixture` rows are HYBRID (real source identity and routes, synthetic rows and recomputed generation), so whether a VD may call them "real" is a scope decision; the md waiting-card and attach embedding rows are conditional; headers in viewer/full/extras describe assertions no code makes (classified historical); viewer-backlog analytics/model rows may duplicate navigation-group checks (not verified).
- [browser-shell-accessibility.md](browser-shell-accessibility.md): `tap-hit.test.mjs` and `text-audit.test.mjs` are internal measurement tests classified native.
- [browser-interaction.md](browser-interaction.md): `embed` is classified internal (owner layer) although it runs in CI; screenshots there are diagnostic only.
- [browser-live-performance-catalog.md](browser-live-performance-catalog.md): `live.mjs` is a wrapper, not a second assertion set; budget thresholds are report-only so "active" does not mean enforced; the first ten rows were verified only by spot checks; `perf-refresh.mjs` is inactive in the default run; `pushed-parity` default mode is VD only if Duhem owns the receiver and push processes.
- [browser-visual-and-suite.md](browser-visual-and-suite.md): whether the pathname-audit and binding-proposal tools run in CI (finding 1); `gaps.mjs` fits a VD only if a model-diff action exists; region counts (162/20/24) and frame counts (113/12/15) were recomputed with jq, the acceptance registry only through bounded summaries.
- [ui-catalog-viewer-browser.md](ui-catalog-viewer-browser.md), [ui-served-scripts.md](ui-served-scripts.md), [ui-unit-and-checks.md](ui-unit-and-checks.md): all `catalog.*` ids and `native` dispositions are proposals; `catalog-viewer-browser.mjs` rows 3410/3560 may be better retained-native diagnostics; `continuity-refinement-browser.mjs` `SEMON_REFINEMENT_BEFORE=1` has no visible CI configuration; `runtime-leaf-browser.mjs` is deferred-paused by the hold, not evidence of a real managed-runtime integration.
