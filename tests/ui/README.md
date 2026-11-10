# Viewer UI checks

The required `viewer browser checks` status aggregates the selected Viewer UI
tier. Pull requests run no browser suites when every changed path is docs-only
(except compiled `docs/shell.md`) or under `crates/semon-asi/`; other code changes
run the navigation, live-update and transport smoke suites; viewer inputs run all
eleven suites. Pushes to main and manual dispatch always run all eleven. The smoke
suites cover the catalog navigation/data path, updates arriving while logs are
written, and pushed/received data parity across transport. CI builds the test-clock
binary and account fixture once, transfers those binaries to isolated runners, and
gives each runner its own servers, fixture directories, and report artifact
(`viewer-ui-<suite>`). Failures in a selected suite fail the aggregate; intentional
skips pass when the selected tier succeeds.
New PR commits cancel superseded runs of that PR. Main and manual events have separate concurrency groups and do not cancel running validation. Infrastructure regressions run once in the required foundation job before binary builds and browser fan-out. The touch hit-testing regressions need Chromium and run once in the required interaction suite.

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

- CI makes no pixel or visual comparison. Screenshots written by the checks
  (`gallery`, `catalog-served`, `overview-served`, tokens, scrollbars, tooltips
  and others) are uploaded in each suite's `viewer-ui-<suite>` artifact for
  human review only. Text, contrast, geometry, touch-target, DOM, coverage,
  order and transport assertions stay blocking.
- The Sessions-default decision in `docs/design/ui-design-brief.md` supersedes
  Home navigation and inbox assertions. Root/title waits now require Sessions;
  ordering leaves the list through Analytics and returns to Sessions; pending
  overview retries use machine details. Native turn URLs retain anchor, flash,
  stale-tail-pin and Back coverage. Font, harness-mark, accessibility, storage,
  transcript answers, Recent and embedding checks stay on retained surfaces.
  Only removed Home inbox affordances (badges, working/inbox marks, question
  summary/Trace/Enter/Space controls and inbox grouping) lose their assertions.
  Negative checks ensure the inbox does not reappear in Sessions.
- Every retained-surface functional assertion stays required, including live updates, session order,
  embedding, approval reviews, transcript paging, and error navigation.
- The touch audit checks phone, landscape, and tablet viewports; top-level screens,
  account menus, and a named set of distinct transcript/control shapes across the
  sample and extras fixtures. `TAP_SESSIONS` records those examples. Missing examples
  fail. Add a representative session whenever a new control shape is introduced.
  `SEMON_UI_COVERAGE=exhaustive` restores every session and trace.
- The `overhaul.html` and `semon-sample.html` HTML renderers and their screenshot
  workflow are retired from the active tree. The historical fixture values stay
  in `fixture-data/*.json`; CI compares them deeply with extraction from pinned
  source commit `bf9190ef2b80fcd56726f61dc480346b698039a2`. The HTML source,
  prior adapter and historical pixel mappings remain in Git history.
- Request-budget measurements remain informational above their ceilings. They run
  alongside required performance/error checks; measurement infrastructure failures
  still fail. Turning them into hard limits requires calibrating the budgets.

Workflow dispatch exposes `exhaustive` for deeper touch audits.
`refresh_diagnostics` also runs the candidate harness against pinned recovery baseline `c402e481dbe23bcf03841e47b5f7eccd14c0e426`. This comparison runs only on explicit manual dispatch. Both binaries use the same runner, locked browser, long fixture, viewports and 4× CPU throttle. The original three append samples and assertions remain; eight additional appends sweep 0/750/1500/2250 ms after naturally occurring selected-metadata requests twice. Each still appends ten records. `perf.json` and `refresh-baseline/perf.json` retain per-endpoint CDP admission/response/completion/failure/cancellation, observed JSON generation/refresh state, and marker DOM-commit/two-frame-visible times. These are browser observations, not server worker-admission or index-publication timestamps; returned generations bound when changed evidence first becomes observable. No extra product polling or timer changes are introduced. Timing remains report-only; missing markers, instrumentation and page errors fail as before.
The infrastructure tests (`suite-plan`, tap hit-testing, catalog cache and catalog
wire validation) guard coverage selection and the shared browser helpers.

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

Shared design rules and consumer auditing are documented in [the design contract](../../docs/design/design-contract.md).

All served suites enter the canonical reader. The shared fixture oracle reads
Catalog, selected metadata and transcripts through source-qualified native pages
and pins their generation across every range. Prompt and answer expectations use
the same bounded native preview API as the browser. The reference census projects
recorded timing into the default 24-hour observation window; retained evidence
stays complete. Candidate fixture metadata and capture traffic use bounded
Source and selected-session APIs. Compatibility query bookmarks are normalized
to the same reader.

The interaction lane also runs `ui/tests/catalog-served.mjs` and
`ui/tests/overview-served.mjs` against the compiled producer and embedded bundle
at 390px, 820px and 1280px in both themes. They check bounded ranges, native bytes,
keyboard paging, focus, root Sessions/Analytics, causal navigation, and save actual served screenshots for review.
