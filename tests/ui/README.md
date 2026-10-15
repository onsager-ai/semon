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
SEMON_ORDER_SCHEMES=desktop-light,desktop-dark node checks/order.mjs
```

These commands require the fixture servers and environment variables documented
in `lib.mjs`. Live, order, error-navigation, and performance checks create their
own scratch fixtures. Timing-sensitive checks retain real polling/backoff windows;
parallelism does not weaken their assertions or alter product timers.

## Required coverage and historical reports

- Every functional assertion stays required, including live updates, session order,
  embedding, approval reviews, transcript paging, and error navigation.
- The touch audit checks phone, landscape, and tablet viewports; top-level screens,
  account menus, and a named set of distinct transcript/control shapes across the
  sample and extras fixtures. `TAP_SESSIONS` records those examples. Missing examples
  fail. Add a representative session whenever a new control shape is introduced.
  `SEMON_UI_COVERAGE=exhaustive` restores every session and trace.
- Visual checks retain all existing full-screen mismatch ratchets, strict region
  comparisons, and pushed-data pixel parity. A `pending` reference still enforces
  its ratchet; it is not a reason to delete the check or loosen the baseline.
- Comparison against the older sample mockup is historical reporting and has no
  pass/fail assertion. It is off by default. `SEMON_PIXEL_SAMPLE_REPORT=1` restores
  those images and report columns without changing the required comparisons.
- Request-budget measurements remain informational above their ceilings. They run
  alongside required performance/error checks; measurement infrastructure failures
  still fail. Turning them into hard limits requires calibrating the budgets.

Workflow dispatch exposes `exhaustive` and `sample_report` for deeper audits.
`refresh_diagnostics` also runs the candidate harness against pinned recovery baseline `c402e481dbe23bcf03841e47b5f7eccd14c0e426`; recovery PR #438 enables this comparison automatically. Both binaries use the same runner, locked browser, long fixture, viewports and 4× CPU throttle. The original three append samples and assertions remain; eight additional appends sweep 0/750/1500/2250 ms after naturally occurring selected-metadata requests twice. Each still appends ten records. `perf.json` and `refresh-baseline/perf.json` retain per-endpoint CDP admission/response/completion/failure/cancellation, observed JSON generation/refresh state, and marker DOM-commit/two-frame-visible times. These are browser observations, not server worker-admission or index-publication timestamps; returned generations bound when changed evidence first becomes observable. No extra product polling or timer changes are introduced. Timing remains report-only; missing markers, instrumentation and page errors fail as before.
The infrastructure tests (`suite-plan`, pixel diff, tap hit-testing, catalog cache,
and catalog wire validation) guard coverage selection and the comparison machinery.

The diagnostic comparison also enables the existing `SEMON_BUILD_TIMING` observer
on both fixture servers. Each screen retains at most 20,000 sanitized phase,
queue and committed-publication records / 4 MiB, timestamped on stderr receipt;
`truncated` explicitly marks capture overflow. These timestamps approximate worker
phase completion and include pipe scheduling; native microsecond durations are
recorded separately. Diagnostic overhead applies to both binaries. This capture
does not add product requests or workers. The comparison alone has a 30-minute
job allowance because it runs two full journeys and phase sweeps; product timers,
the 30-second complete-indexing gate and other suite allowances are unchanged.

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
