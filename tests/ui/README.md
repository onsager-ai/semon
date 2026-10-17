# Viewer UI checks

The required `viewer browser checks` status aggregates every Viewer UI suite. CI
builds the test-clock binary and account fixture once, transfers those binaries
to isolated runners, and runs twelve suites in parallel. Each runner owns its
servers, fixture directories, and report artifact (`viewer-ui-<suite>`). Failures
in one suite do not cancel the others; a skipped or failed suite fails the aggregate.
New commits cancel superseded runs on the same PR or branch.

Functional groups live in `suite-plan.mjs`. `checks/run.mjs` verifies that every
registered check belongs to exactly one group before running anything. Adding a
check requires assigning it to a group; missing coverage fails rather than silently
passing. The runner prints each check's elapsed milliseconds for balancing groups.

```sh
node checks/run.mjs                       # all functional checks
node checks/run.mjs --group transcript    # one CI group
node checks/live.mjs                     # native append/visibility/request ownership
SEMON_ORDER_SCHEMES=desktop-light,desktop-dark node checks/order.mjs
```

These commands require the fixture servers and environment variables documented
in `lib.mjs`. Live, order, error-navigation, and performance checks create their
own scratch fixtures. Timing-sensitive checks retain real polling/backoff windows;
parallelism does not weaken their assertions or alter product timers.
The live command executes `live-catalog.mjs` and requires its assertions to pass;
its completion witness is the native-publication success message in the CI log.

## Required coverage and historical reports

- Every functional assertion stays required, including live updates, session order,
  embedding, approval reviews, transcript paging, and error navigation.
- The touch audit checks phone, landscape, and tablet viewports; top-level screens,
  account menus, and a named set of distinct transcript/control shapes across the
  sample and extras fixtures. `TAP_SESSIONS` records those examples. Missing examples
  fail. Add a representative session whenever a new control shape is introduced.
  `SEMON_UI_COVERAGE=exhaustive` restores every session and trace.
- Visual authority is the selected complete application, independently built
  from Source `263a02da278716ab2309b8e34a463419e5c781e5` and selected with Hub
  `503fbe0`. The 62-state preservation gate remains required on every run. The
  historical actual-frame manifest also retains all 113 full screenshots and
  162 strict region captures, plus 12 full frames/20 regions for the two-machine
  viewer and 15 full frames/24 regions for pushed-data parity. The actual UI
  reference runs through its original `compat=1` route and history states; the
  pushed-data comparison is served against the machine's own viewer.
- Every screenshot comparison keeps threshold 0.1 and maximum ratio 0.0001.
  Missing, duplicate, unknown or unmapped frames, selector failures, geometry
  changes, retired API requests, and execution errors fail independently. Raw
  pixel findings remain in the reports. `visual-acceptance.json` starts empty;
  a reviewed entry can accept only one exact comparator, scope, state, candidate
  runtime-binary fingerprint, reference pins, screenshot/diff hashes and bounded
  text/geometry evidence hashes. The reviewed candidate commit is recorded as
  provenance; CI matches the runtime binary and exact captures so adding the
  registry does not create a self-referential commit hash or imply ancestry.
  Stale, unreviewed or incomplete records fail closed. No broad waiver, mask,
  auto-approval or threshold change can turn a mismatch green.
- The `overhaul.html` and `semon-sample.html` HTML renderers and their screenshot
  workflow are retired from the active tree. The historical fixture values stay
  in `fixture-data/*.json`; CI compares them deeply with extraction from pinned
  source commit `bf9190ef2b80fcd56726f61dc480346b698039a2`. The HTML source and
  prior adapter remain in Git history; `historical-reference-map.json` retains
  the old mapping, and `historical-pixel-ratchet.json` retains its per-case
  ratios and dimensions. These preserve the original data but never act as an
  active renderer or acceptance baseline.
- `preservation-contract.mjs` pins the selected actual reference and all 62 cases
  at 390/820/1280px, both themes, including open Thinking/tools/output, a
  completed child and phone drawers. Only the exact pinned historical before
  candidate may use `--historical-before`; its raw differences remain reported.
- Request-budget measurements remain informational above their ceilings. They run
  alongside required performance/error checks; measurement infrastructure failures
  still fail. Turning them into hard limits requires calibrating the budgets.

Workflow dispatch exposes `exhaustive` for deeper touch audits.
`refresh_diagnostics` also runs the candidate harness against pinned recovery baseline `c402e481dbe23bcf03841e47b5f7eccd14c0e426`; recovery PR #438 enables this comparison automatically. Both binaries use the same runner, locked browser, long fixture, viewports and 4× CPU throttle. The original three append samples and assertions remain; eight additional appends sweep 0/750/1500/2250 ms after naturally occurring selected-metadata requests twice. Each still appends ten records. `perf.json` and `refresh-baseline/perf.json` retain per-endpoint CDP admission/response/completion/failure/cancellation, observed JSON generation/refresh state, and marker DOM-commit/two-frame-visible times. These are browser observations, not server worker-admission or index-publication timestamps; returned generations bound when changed evidence first becomes observable. No extra product polling or timer changes are introduced. Timing remains report-only; missing markers, instrumentation and page errors fail as before.
The infrastructure tests (`suite-plan`, pixel diff, tap hit-testing, catalog cache,
catalog wire validation, bounded DOM evidence, and preservation contracts) guard
coverage selection, exact artifact approval bindings and the actual comparison
machinery. The visual artifacts include a cross-state selector summary of changed
text, boxes, and computed styles, with bounded examples linked to the per-state
screenshots and evidence JSON.

The diagnostic comparison also enables the existing `SEMON_BUILD_TIMING` observer
on both fixture servers. Each screen retains at most 20,000 sanitized phase,
queue and committed-publication records / 4 MiB, timestamped on stderr receipt;
`truncated` explicitly marks capture overflow. These timestamps approximate worker
phase completion and include pipe scheduling; native microsecond durations are
recorded separately. Diagnostic overhead applies to both binaries. This capture
does not add product requests or workers. The comparison alone has a 30-minute
job allowance because it runs two full journeys and phase sweeps; product timers,
the 30-second complete-indexing gate and other suite allowances are unchanged.

The separate request-budget live scenario retains its original six appends at
five-second intervals and original 30-second window metrics. Passive fetch/JSON
traces distinguish response status from transport cancellation. After that window
it observes up to 60 seconds of ordinary catch-up, recording the final generation,
expected slot count, all six markers and visible entry-key uniqueness separately;
that observation adds no requests or pollers and is not part of the budget row.

The October 1 baseline ran serially in 27–43 minutes. The largest avoidable costs
were repeated touch auditing across every session and running independent suites
one after another. This layout targets the longest suite's runtime plus binary
build/setup, rather than their sum. Measure GitHub runner timings before treating
that estimate as an achieved CI duration.

Shared design rules, consumer auditing and deliberate gallery baseline updates are
documented in [the design contract](../../docs/design/design-contract.md).

All served suites enter the canonical reader. The shared fixture oracle reads
Catalog, selected metadata and transcripts through source-qualified native pages
and pins their generation across every range. Prompt and answer expectations use
the same bounded native preview API as the browser. The reference census projects
recorded timing into the default 24-hour observation window; retained evidence
stays complete. Neither fixture metadata nor browser traffic reads `/api/model`.
Browser traffic must use bounded Source and selected-session APIs. Compatibility query bookmarks are
normalized to the same reader. Existing mismatch ratchets and approved baselines
remain required.

The interaction lane also runs `ui/tests/catalog-served.mjs` and
`ui/tests/overview-served.mjs` against the compiled producer and embedded bundle
at 390px, 820px and 1280px in both themes. They check bounded ranges, native bytes,
keyboard paging, focus, Home/Analytics, causal navigation and the contextual
Composer, and save actual served screenshots for review. New screenshots do not
replace approved visual references.
