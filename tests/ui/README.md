# Viewer UI checks

The required `viewer browser checks` status aggregates every Viewer UI suite. CI
builds the test-clock binary and account fixture once, transfers those binaries
to isolated runners, and runs twelve suites in parallel. Each runner owns its
servers, fixture directories, and report artifact (`viewer-ui-<suite>`). Failures
in one suite do not cancel the others; a skipped or failed suite fails the aggregate.
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

- The Sessions-default decision in `docs/design/ui-design-brief.md` supersedes
  Home navigation and inbox assertions. Root/title waits now require Sessions;
  ordering leaves the list through Analytics and returns to Sessions; pending
  overview retries use machine details. Native turn URLs retain anchor, flash,
  stale-tail-pin and Back coverage. Font, harness-mark, accessibility, storage,
  transcript answers, Recent and embedding checks stay on retained surfaces.
  Only removed Home inbox affordances (badges, working/inbox marks, question
  summary/Trace/Enter/Space controls and inbox grouping) lose their assertions.
  Negative checks ensure the inbox does not reappear in Sessions. Historical
  visual case identities remain required: candidate `home` means the root
  Sessions destination, while the independently pinned reference keeps Home.
  Pushed self-parity uses root Sessions on both sides. Actual visual differences
  still require exact artifact review; no baseline acceptance is implied here.
- Every retained-surface functional assertion stays required, including live updates, session order,
  embedding, approval reviews, transcript paging, and error navigation.
- The touch audit checks phone, landscape, and tablet viewports; top-level screens,
  account menus, and a named set of distinct transcript/control shapes across the
  sample and extras fixtures. `TAP_SESSIONS` records those examples. Missing examples
  fail. Add a representative session whenever a new control shape is introduced.
  `SEMON_UI_COVERAGE=exhaustive` restores every session and trace.
- Visual authority is the selected complete application, independently built
  from Source `263a02da278716ab2309b8e34a463419e5c781e5` and selected with Hub
  `503fbe0`. The required preservation gate now compares 68 states: the original
  62 plus an open Sessions Filter at 390/820/1280px in both themes. The historical
  actual-frame manifest remains unchanged with all 113 full screenshots and
  162 strict region captures, plus 12 full frames/20 regions for the two-machine
  viewer and 15 full frames/24 regions for pushed-data parity. The actual UI
  reference runs through its original `compat=1` route and history states; the
  pushed-data comparison is served against the machine's own viewer.
- The #380 task-branch visual/transport CI additionally runs
  `visual-pathname-audit.mjs` against the existing accepted registry records.
  `SEMON_UI_OUT` supplies current captures; no earlier-run artifact download,
  token or network access is used. Each raw state must have exactly one accepted
  record with the same reference pins. The five non-candidate-evidence digests
  must match that record, and current candidate evidence bytes must match their
  reported SHA-256. Identical accepted candidate evidence is fingerprint-only.
  Otherwise, the audit permits only a canonical top-level pathname replacement
  from `/s/claude/<id>` or `/s/codex/<id>` to `/session/local/<same encoded id>`:
  restoring either exact historical fixture namespace must produce exactly one
  byte sequence with the accepted candidate-evidence SHA-256. This digest-bound
  mapping is local to the audit; other namespaces, identities, escaping, or any
  text, geometry, style or history change are refused. `visual-pathname-audit.json`
  lists qualified and refused exact states; CI prints bounded state summaries.
  This diagnostic never edits the registry, establishes appearance approval or
  excuses missing coverage/execution errors. A follow-up may refresh only
  justified exact bindings while preserving original review/decision/provenance/
  history; final CI and independent review still apply.
- Every screenshot comparison keeps threshold 0.1 and maximum ratio 0.0001.
  Missing, duplicate, unknown or unmapped frames, selector failures, geometry
  changes, retired API requests, and execution errors fail independently. Raw
  pixel findings remain in the reports. `visual-acceptance.json` records only exact reviewed states;
  a reviewed entry can accept only one exact comparator, scope, state, candidate
  runtime-binary fingerprint, reference pins, screenshot/diff hashes and bounded
  text/geometry evidence hashes. The reviewed candidate commit is recorded as
  provenance; CI matches the runtime binary and exact captures so adding the
  registry does not create a self-referential commit hash or imply ancestry.
  Stale, unreviewed or incomplete records fail closed. No broad waiver, mask,
  auto-approval or threshold change can turn a mismatch green.
- The 2026-10-19 unified Hub application review classifies the historical passive
  reader notice/disclosure as an obsolete reference under design brief section 11.
  The task owner explicitly accepted its removal after reviewing the 820px light
  Thinking comparison. Actual-image inspection also covers phone collapsed/tools,
  desktop expanded output, failed children and long-transcript clipping. The
  smaller dock exposes existing transcript text and moves Jump to Bottom. In the
  eight long mobile full-page frames it also shortens the document and therefore
  moves the fixed header captured at the latest scroll position; shared
  chrome, visible recorded text and controls retain their existing presentation.
  The original CI evidence remains at [run 39213975213](https://github.com/onsager-ai/semon/actions/runs/39213975213/artifacts/12168508595):
  42 preservation and 69 main-frame raw differences, no failed strict regions and
  no coverage errors. The three closed Atlas tool-group frames also contain a
  hidden `textContent` wording change (`no result` → `Result not recorded`); their
  visible screenshot difference is only the removed notice. Their exact bindings
  do not approve different expanded output. The registry retains separate records
  for every reviewed size/theme/state and its original pinned references; raw
  findings and the strict 0.0001 maximum ratio remain unchanged. Exact bindings
  use the reviewed [CI captures from run 39219258365](https://github.com/onsager-ai/semon/actions/runs/39219258365/artifacts/12171068756);
  local captures support review but are not treated as portable byte-identical
  approvals across font/rasterization environments. The latest captured runtime
  passed Rust and all nonvisual browser CI. Its raw visual result retains 18
  preservation and five main-frame stale byte bindings from the preceding run;
  reinspection and the final registry assess those exact current artifacts with
  zero pending/unreviewed findings. A fresh rerun can require fresh exact review;
  this record does not claim an automatically green visual CI aggregate.
- The account-menu SPA follow-up (PR #449) re-inspects the previously approved
  notice-removal differences for the changed runtime. Initial [CI images from
  run 39223065023](https://github.com/onsager-ai/semon/actions/runs/39223065023/artifacts/12172548963)
  cover phone tools, tablet Thinking, desktop expanded output, failed/returned
  children, closed Atlas, long mobile full-page clipping, and changed desktop
  parser/Principal/quill captures. The Atlas hidden-text distinction above stays
  limited to closed output. No new visible behavior is approved by these records.
- Exact PNG bindings require repeatable rasterization as well as stable DOM.
  Earlier reruns retained identical bounded text/geometry and strict mismatch
  ratios but changed rounded-edge/text pixels by only a few channel levels.
  Preservation/main comparators now use Chromium's baseline Skia CPU path,
  [deterministic rendering stages](https://github.com/GoogleChrome/chrome-launcher/blob/main/docs/chrome-flags-for-tools.md#rendering--gpu)
  and full rasterization on both independent applications. Playwright keeps normal
  frame scheduling. Functional/performance and pushed-parity browsers
  retain their existing launch settings. The capture regression checks independent
  browser processes, clipped sticky geometry, repeated scroll/repaint equality,
  and changed visible text.
- Two-machine comparisons also use that deterministic Skia launch path on both
  independent readers. The desktop Machines reference alternated only six SVG
  edge pixels in three 10.5px Claude marks; text, geometry and the other five
  image/evidence bindings stayed identical. The cross-process capture regression
  includes those fractional-size marks. This changes no screenshot content,
  animation policy, state, threshold or exact digest requirement; fresh CI images
  and actual review remain required before rebinding acceptance data.
  The d71b292 capture review uses CI run 40070762417, artifact 12550918798.
  Actual light/dark desktop Machines candidate/reference/diff images were viewed;
  only three PNG digests change, with identical bounded text/geometry evidence.
  Two exact frame bindings retain their prior decisions and all required coverage.
- Main desktop captures hold a real pointer in inert sidebar padding before
  navigation/capture and require matching visible row names and selection. This
  prevents the independent readers crossing their existing Recent idle-reorder
  boundary at different times. The separately observed survey sidebar-order
  mismatch is not accepted; its required state is restored and checked. Product
  timers/order, DOM rows and pixel comparisons remain intact; other suites still
  exercise actual idle, hover and focus behavior.
- The [reviewed deterministic captures from run 39246032745](https://github.com/onsager-ai/semon/actions/runs/39246032745/artifacts/12182933579)
  match [independent run 39244684719](https://github.com/onsager-ai/semon/actions/runs/39244684719/artifacts/12183397568)
  byte-for-byte for all 68 preservation and 113 main full-frame candidate,
  reference and diff PNGs, plus their bounded DOM evidence. All 162 strict-region
  PNGs also match between runs. Actual tablet Thinking and closed desktop Atlas
  images/diffs were re-inspected on this path. The registry refreshes 31 remaining
  byte bindings with exactly unchanged bounded text/geometry evidence, retaining
  all original reference pins, screenshot/evidence hash requirements, thresholds,
  coverage and raw findings. Final required CI must validate the delivered tree.
- After Source #450 merged, runtime `b8a0453a` in
  [run 39253904406](https://github.com/onsager-ai/semon/actions/runs/39253904406/artifacts/12187521876)
  retained all six candidate/reference/diff PNG and bounded-evidence digests for
  each of the 111 already reviewed states (42 preservation, 69 main). The only
  stale binding was the runtime fingerprint. Its registry update preserves every
  state, reference, artifact digest and original image-review provenance; the 68
  preservation cases, 113 main frames and 162 strict regions remain required.
- The Sessions-default review in Source #448 uses the actual
  [preservation/main captures from run 39943014615](https://github.com/onsager-ai/semon/actions/runs/39943014615/artifacts/12497546002)
  and its [two-machine/pushed captures](https://github.com/onsager-ai/semon/actions/runs/39943014615/artifacts/12496927958).
  The settled Foundation decision replaces the root Home inbox with the existing
  Sessions list and removes Home from navigation; it does not migrate inbox or
  status sections. Actual root/title, drawer, desktop Analytics, tablet Thinking,
  phone transcript and multi-machine comparisons retain the shared foundation.
  The resulting navigation/Recent row shift and the previously approved passive
  notice/dock changes account for 62 preservation, 90 main and six two-machine
  raw findings. The 111 earlier records retain their rationale and approval
  provenance; 47 new records bind only the settled root/title/navigation states.
  Every record retains the original reference pins and all six exact image/DOM
  digests. All 68 preservation cases, 113 main frames/162 regions, 12 two-machine
  frames/20 regions and 15 pushed frames/24 regions remain required. Pushed parity
  has no raw difference. No threshold, capture input or semantic assertion changes;
  full required CI must validate the delivered acceptance-data revision.
- PR474 fingerprint-only maintenance uses [run 39763385836, artifact 12418164659](https://github.com/onsager-ai/semon/actions/runs/39763385836/artifacts/12418164659)
  (archive SHA-256 `f6e073b085e4e665f2f5fcfae58540c5eedda61cef69505dbf32c065c4e3357e`,
  candidate `9dc88082ea630e542850d94e043ed9dc9ef52f21`): 111 identities and all
  666 same-run artifact digests match the existing bindings. Only their runtime
  fingerprint changes; every original per-state review field remains intact.
  The three exact Last-turn menu-region bindings record independent Claude
  `acceptance-image-table-ea77` image inspection and the Manager decision, with
  no human appearance approval claimed. The one-off audit remains in Git history
  and its CI artifact; final-head CI and acceptance remain required.
- The combined Source #448/main `c589a9d8` review uses
  [run 39948275965 visual captures](https://github.com/onsager-ai/semon/actions/runs/39948275965/artifacts/12500060720)
  and its [transport captures](https://github.com/onsager-ai/semon/actions/runs/39948275965/artifacts/12499047700).
  The 161 exact states retain 62 preservation, 93 main and six two-machine raw
  differences; pushed parity has none. Of 966 image/evidence digests, 965 match
  the prior Source 158-state and main three-menu reviews. The sole changed
  two-machine reference PNG and current family triplets, including all three
  Last-turn menus, were inspected. The same settled root/navigation and prior
  notice/menu decisions apply; all original approval history, references,
  thresholds and required coverage remain. The first interaction attempt had
  one pending-observation retry timeout (143/144); one unchanged-SHA diagnostic
  rerun passed 144/144 and the complete interaction lane. Its cause remains
  unproven. Final delivered-head CI must pass 144/144 on its first attempt, with
  no rerun allowance, followed by fresh independent acceptance. This capture
  record does not claim that the delivered revision has already passed CI.
- Preservation and main-scope full-frame PNGs and their bounded DOM/paint evidence share one animation
  transaction. As with the existing screenshot suppression, finite effects finish;
  infinite effects pause at time zero through both reads, then resume. The former
  screenshot-only suppression resumed effects before diagnostics and made exact
  hashes depend on wall-clock timing. Both independent applications use the same
  transaction; no content, selectors, regions, styles or pixel tolerances are
  removed. A bounded consecutive-PNG equality check also rejects unstable mobile
  full-page compositor tiles after scrolling fixed/clipped content. It does not
  select a favorable comparison: unchanged consecutive paints are required before
  any reference comparison. Two-machine and pushed parity retain their existing
  capture path. `capture-paint.test.mjs` checks the timing-dependent negative control,
  repeated PNG/evidence equality, visible-content changes and failure cleanup.
- Captures require bounded API/DOM quiescence and the requested rendered title,
  session heading or exact Trace root turn. Reference menu cleanup uses its real
  Close button and waits for the overlay history entry to finish. The original
  Escape cleanup could leave a stale menu owner and consume the next navigation;
  history state alone was not evidence that the requested page was rendered.
  Raw HTTP/network events remain retained. Only a reference model poll's exact
  recorded bodyless 304 can explain its matching `ERR_ABORTED`; candidate read
  failures and aborts after 200 remain fatal.
- The `overhaul.html` and `semon-sample.html` HTML renderers and their screenshot
  workflow are retired from the active tree. The historical fixture values stay
  in `fixture-data/*.json`; CI compares them deeply with extraction from pinned
  source commit `bf9190ef2b80fcd56726f61dc480346b698039a2`. The HTML source and
  prior adapter remain in Git history; `historical-reference-map.json` retains
  the old mapping, and `historical-pixel-ratchet.json` retains its per-case
  ratios and dimensions. These preserve the original data but never act as an
  active renderer or acceptance baseline.
- `preservation-contract.mjs` pins the selected actual reference and all 68 cases
  at 390/820/1280px, both themes, including open Filter, Thinking/tools/output, a
  completed child and phone drawers. Only the exact pinned historical before
  candidate may use `--historical-before`, which remains limited to the original
  62 cases; its raw differences remain reported.
- Request-budget measurements remain informational above their ceilings. They run
  alongside required performance/error checks; measurement infrastructure failures
  still fail. Turning them into hard limits requires calibrating the budgets.

Workflow dispatch exposes `exhaustive` for deeper touch audits.
`refresh_diagnostics` also runs the candidate harness against pinned recovery baseline `c402e481dbe23bcf03841e47b5f7eccd14c0e426`. This comparison runs only on explicit manual dispatch. Both binaries use the same runner, locked browser, long fixture, viewports and 4× CPU throttle. The original three append samples and assertions remain; eight additional appends sweep 0/750/1500/2250 ms after naturally occurring selected-metadata requests twice. Each still appends ten records. `perf.json` and `refresh-baseline/perf.json` retain per-endpoint CDP admission/response/completion/failure/cancellation, observed JSON generation/refresh state, and marker DOM-commit/two-frame-visible times. These are browser observations, not server worker-admission or index-publication timestamps; returned generations bound when changed evidence first becomes observable. No extra product polling or timer changes are introduced. Timing remains report-only; missing markers, instrumentation and page errors fail as before.
The infrastructure tests (capture network lifecycle, `suite-plan`, pixel diff, tap hit-testing, catalog cache,
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
stays complete. Candidate fixture metadata and capture traffic use bounded
Source and selected-session APIs. The actual visual comparator makes one pinned
reference-only `/api/model?delta=1&content=1` read to resolve old trace turn IDs;
its digest and per-state handoff/input mapping are reported. Candidate browser
traffic never calls that retired endpoint. Compatibility query bookmarks are
normalized to the same reader. Existing mismatch ratchets and approved baselines
remain required.

The interaction lane also runs `ui/tests/catalog-served.mjs` and
`ui/tests/overview-served.mjs` against the compiled producer and embedded bundle
at 390px, 820px and 1280px in both themes. They check bounded ranges, native bytes,
keyboard paging, focus, root Sessions/Analytics, causal navigation and paused-control
refusal, and save actual served screenshots for review. New screenshots do not
replace approved visual references.
