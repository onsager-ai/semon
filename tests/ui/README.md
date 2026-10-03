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
The infrastructure tests (`suite-plan`, pixel diff, tap hit-testing, viewer cache,
and model delta) guard coverage selection and the comparison machinery.

The October 1 baseline ran serially in 27–43 minutes. The largest avoidable costs
were repeated touch auditing across every session and running independent suites
one after another. This layout targets the longest suite's runtime plus binary
build/setup, rather than their sum. Measure GitHub runner timings before treating
that estimate as an achieved CI duration.

Shared design rules, consumer auditing and deliberate gallery baseline updates are
documented in [the design contract](../../docs/design/design-contract.md).
