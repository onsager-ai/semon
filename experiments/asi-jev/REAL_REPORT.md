# Real-session ASI/Jev evaluation

Completed real-session replay, 2026-10-30: **do not integrate this Jev
classifier into production ASI.** Across 436 events from 20 complete captured
sessions, Jev did not demonstrate an activity macro-F1 improvement over the
experimental rules baseline, generated too many anomaly alerts, and missed
the declared HTTP latency target. Haiku classified activity substantially
better, but the shared trigger lost failure recall; neither provider qualified
under the predeclared criteria. An optional asynchronous shadow experiment is
the appropriate next step, using separate sessions and human-reviewed labels.

This is actual inference: nine conditions and 7,254 remote requests, including
three repetitions per remote condition. Results from the separate fixture
[feasibility experiment](REPORT.md) are excluded. No production parser,
runtime, storage contract, or mandatory model dependency is changed.

Full disclosure-safe outputs: [quality and confidence intervals](REAL_RESULTS.json),
[latency, throughput and cost accounting](REAL_OPERATIONS.json), and
[Sol usage scope](REAL_SOL_USAGE.json). Reproduction is documented in
[README.md](README.md); exact inputs and individual outputs remain local.

## Cohort and custody

The frozen cohort contains 436 eligible events from 20 complete captured
physical sessions and 20 distinct original tasks: 12 Claude Manager captures
and eight Codex Worker captures. These are this user's real local native CLI
logs, predominantly older standalone orchestrated runs. Harness and role are
confounded; this purposive, small-session, error-enriched cohort does not
represent all interactive work or deployment prevalence. A complete captured
file does not prove that its task finished or that a native terminal event
exists.

Exact local raw copies retain 1,788 native records and 7,297,385 bytes.
Projection accounting records 33 text-field truncations, 20 orchestration
instruction envelopes redacted, 157 internal-reasoning omissions, 924 metadata
omissions, 169 unrecognized-envelope omissions, and 46 exact duplicate native
lines omitted from projection. Original bytes remain intact. An independent
AI privacy reviewer removed seven whole text fields across six packets, then
rechecked all 436 rows. No whole-session privacy exclusion was needed in the
final cohort.
Completeness refers to the retained native capture, not classification of every
native record. Accuracy and recall apply to eligible projected events only;
omitted or unsupported records can contain evidence this benchmark cannot
evaluate, especially for anomaly detection.

Raw copies, projections, annotations, predictions, audit material, examples,
and user-review material remain under gitignored `private/`, with directories
0700 and files 0600. Only reviewed sanitized projections may reach the
configured inference providers. No real session material goes to GitHub or CI.

## Frozen method and execution notes

[REAL_PROTOCOL.md](REAL_PROTOCOL.md) freezes the taxonomy, projection,
selection constraints, success criteria, and statistics before predictions.
It includes Codex error reason/message fields and Claude stop reasons. Native
parsing remains authoritative. The bounded semantic projection omits hidden
reasoning, system prompts, credentials, environment values, private paths,
and unsupported payloads.

Two fresh blind labellers use only the controlled annotation interface and
[taxonomy guide](LABEL_GUIDE.md), followed by a separate blind adjudicator.
Every evidence response must parse as complete JSON before labelling, with
40,000-token command and enclosing tool output budgets. The interface verifies
the frozen projection's counts and hashes on each request. Full-session review
opens only after both causal passes finish. No model predictions are visible
to reference labellers or adjudicator. These are AI reference judgements,
not human gold labels; the 50-event user audit is reported separately.
The two labellers and adjudicator use the same model family, so correlated
annotation errors remain possible. Bootstrap intervals condition on this
reference and selected cohort; they do not cover annotation error or sampling
bias.
A separate envelope audit reconstructed all 436 unique causal packets
against the frozen hashes: the maximum serialized response is 9,081 bytes,
reported as a size observation, not a token-budget measurement.
Expected packet hashes stay private;
actual receipt completeness is evidenced by the strict JSON-parse tool traces.

Two preliminary annotation attempts are excluded before classifier outputs:
the first B pass encountered tool-output clipping of unknown extent; the first
A pass lacked positive evidence of complete packet receipt. Original answers
remain archived unchanged locally. Both replacement labellers use fresh
Sol/high contexts. The replacements do not reuse either excluded pass.

One frozen-implementation deviation was discovered during fresh annotation,
before classifier inference: causal windows retain the byte tail of older
evidence and can start mid-record, rather than retaining whole older rows as
the protocol prose says. The current target is reserved. Offline windows use
whole selected context rows. The formatter remains unchanged for labels and
classifiers; interpret context-sensitive results with this limitation.

The nine conditions are rules prefix/offline; Jev per-event prefix,
microbatch prefix, triggered prefix, and offline; and Haiku per-event prefix,
triggered prefix, and offline. Model conditions repeat three times with four
independent session workers. Repetitions measure variability rather than
increasing the independent sample size. The two triggered arms share the
same frozen rules gate. There is no automatic request retry or provider switch.

Both model arms use the remote Magpie gateway. The documented local usage
ledger does not cover this route. Billed costs are unknown; any Jev direct
list-price estimate is separate from a gateway bill. No credential/config
discovery or undocumented remote ledger access is attempted.

CPU processing uses current actual kernel temperature readings before and
after bounded work and stops at 85°C or missing telemetry. Each input read is
one file below 10 MB. Synthetic tests run in CI; no private corpus is uploaded.
No local cargo/node tests or project-wide typechecks are run.
The first unpaced bootstrap calculation reached 85°C and stopped before
writing results. It was restarted with 50 ms idle pauses between checkpointed
draw groups, using the same 2,000 draws and seed. No reference or model run
was repeated or changed for this resource-control correction.

## Integration boundary

The repository inspection and source revisions are recorded in
[pipeline.md](pipeline.md). Semon owns reusable native capture, parsing and
viewer behavior; Hub owns hosted tenancy and authorization. An ASI classifier
belongs in an optional asynchronous consumer after authorized native ingestion,
outside native parsing and publication transactions. It must consume a bounded,
reviewed projection and emit revisable annotations keyed to the exact source
generation, consumed range, parser, taxonomy, context policy and model version.
It must not retain a request-scoped source reader or infer authority from a
source reference. Changed source bytes invalidate dependent annotations.

Rules handle native facts and ordinary events; an optional model handles a
shared uncertainty/failure trigger through a bounded per-tenant queue. Keep
session state in caller code, record queue wait separately from HTTP RTT,
reject stale-generation answers, and make timeout/invalid output explicit as
unknown. Native capture and browsing continue when classification is disabled
or unavailable. A semantic phase is not liveness, permission, approval, or
proof that a task ended.

Jev is decision-only, with typed choice/probability questions and no text
generation, tool use, or durable session state. Microbatch questions must each
carry only their target's causal evidence; do not mix tenants in shared state.
Four-session replay concurrency measures this runner, not sustained provider
capacity or multi-tenant scalability. Account rate limits, outages, queue
backpressure and confidence calibration remain separate qualification work.

The [primary-source API and privacy research](jev-research.md) records TypeSafe
limits, direct pricing, and the absence of a public fixed retention/ZDR or
regional-processing guarantee for this account. Sanitization and this user's
authorized experiment do not establish product-wide consent or contractual
custody. Any later pilot needs its own tenant opt-in and provider/gateway terms.

## Results and recommendation

### Reference quality and uncertainty

Both replacement Sol/high labellers independently completed 436 causal and
436 offline labels over all 20 sessions. The blind adjudicator resolved 91
causal and 94 offline event disputes before any classifier predictions existed.
Offline agreement before adjudication was:

| Dimension | Agreement | Cohen's kappa |
| --- | ---: | ---: |
| Activity | 86.0% | 0.724 |
| Failure category | 99.1% | 0.949 |
| Retry/loop | 97.0% | 0.311 |
| Anomaly | 99.8% | 0.856 |
| Phase | 90.8% | 0.721 |

Causal agreement was similar (activity 86.7%, kappa 0.740; phase 91.5%,
kappa 0.746). High raw retry agreement hides low chance-adjusted agreement.
The 50-event local user spot-check is prepared but **pending**; no human
agreement is claimed. It contains proposed reference labels and is a review,
not a blind independent human test.

Intervals below are 95% percentile bootstraps from 2,000 draws, seed 20261030.
The independent unit is the unique event; its three repeated predictions stay
together and contribute fractional confusion counts. Repetitions do not turn
436 events into 1,308 independent observations. The JSON also reports session
cluster sensitivity intervals and every taxonomy class's precision/recall.
False-alert intervals resample the 20 whole sessions. All intervals condition
on the frozen AI reference and purposively selected cohort.

The reference has 44 failures (31 tool, nine permission, three environment,
one model), ten retries, four loops and four anomalies. Activity is dominated
by 323 tool actions; there are no waits, five recoveries, two completions and
eight unknowns. Rare-class estimates, including perfect recall and degenerate
intervals, cannot establish population reliability. The frozen minimum of ten
positives is unmet for anomalies, loops and several activity classes.

### Streaming quality

Primary comparisons use the same adjudicated **offline reference** for every
condition. Causal-reference scores are separately available in the JSON and
give similar activity F1. The rules are a small experimental classifier,
not Semon's native parser or a qualified production alert policy.

| Prefix condition | Activity accuracy | Activity macro F1 [95% CI] | Paired F1 improvement vs rules [95% CI] |
| --- | ---: | --- | --- |
| Rules | 0.784 | 0.420 [0.369, 0.461] | — |
| Jev per-event | 0.853 | 0.393 [0.340, 0.447] | -0.027 [-0.088, 0.043] |
| Jev microbatch | 0.855 | 0.387 [0.330, 0.443] | -0.033 [-0.098, 0.046] |
| Rules + triggered Jev | 0.856 | 0.422 [0.373, 0.469] | +0.002 [-0.052, 0.070] |
| Haiku per-event | 0.963 | 0.692 [0.595, 0.766] | +0.272 [0.152, 0.373] |
| Rules + triggered Haiku | 0.888 | 0.522 [0.431, 0.592] | +0.102 [0.010, 0.182] |

Macro F1 includes the fixed eight activity classes, including absent waits.
Accuracy alone hides errors on uncommon activities. Jev per-event recovered
no recovery activities and only 28.6% of task requests; Haiku per-event
recovered 53.3% and 98.8% respectively. Those five recovery events are weak
evidence, not an estimate for general deployment.

| Prefix condition | Any-failure P / R [recall CI] | Retry P / R [recall CI] | Loop P / R (n=4) |
| --- | --- | --- | --- |
| Rules | 0.679 / 0.432 [0.283, 0.578] | 0.235 / 0.400 [0.091, 0.750] | 0.028 / 1.000 |
| Jev per-event | 0.680 / 0.932 [0.846, 1.000] | 0.187 / 0.467 [0.143, 0.778] | 0.197 / 1.000 |
| Jev microbatch | 0.600 / 0.886 [0.794, 0.968] | 0.100 / 0.400 [0.100, 0.714] | 0.190 / 1.000 |
| Rules + triggered Jev | 0.593 / 0.530 [0.377, 0.674] | 0.387 / 0.400 [0.100, 0.727] | 0.308 / 1.000 |
| Haiku per-event | 1.000 / 0.871 [0.781, 0.952] | 0.460 / 0.967 [0.889, 1.000] | 0.600 / 1.000 |
| Rules + triggered Haiku | 1.000 / 0.500 [0.350, 0.644] | 0.619 / 0.867 [0.625, 1.000] | 0.571 / 1.000 |

Any-failure means a non-`none` failure category; it is distinct from activity
`failure`. For tool categorization specifically (31 positives), precision/recall
was 0.333/0.226 for rules, 0.723/0.925 for Jev per-event, 1.000/0.892 for Haiku
per-event, 0.544/0.398 for triggered Jev, and 1.000/0.387 for triggered Haiku.
All conditions missed all three environment categories. Other failure categories
have insufficient or zero support; full per-class intervals remain in the JSON.

| Prefix condition | Anomaly precision [95% CI] | Anomaly recall (n=4) | False alerts/session [session CI] | Sessions averaging >1 false alert |
| --- | --- | ---: | --- | ---: |
| Rules | 0.008 [0.000, 0.027] | 0.250 | 5.950 [3.050, 8.950] | 45% |
| Jev per-event | 0.066 [0.015, 0.135] | 1.000 | 2.817 [1.933, 3.750] | 65% |
| Jev microbatch | 0.167 [0.037, 0.323] | 1.000 | 1.000 [0.517, 1.567] | 35% |
| Rules + triggered Jev | 0.119 [0.026, 0.236] | 1.000 | 1.483 [0.816, 2.167] | 55% |
| Haiku per-event | 0.571 [0.176, 0.882] | 1.000 | 0.150 [0.017, 0.317] | 5% |
| Rules + triggered Haiku | 0.571 [0.166, 0.900] | 1.000 | 0.150 [0.017, 0.333] | 5% |

These alert rates describe all eligible events in each selected capture,
averaged over repetitions; they are not alerts per hour or deployment
prevalence estimates. Jev's descriptive false-alert bounds exceed the budget
of one/session in every strategy. Haiku's bounds are below it, but its anomaly
precision remains below the required lower bound of 0.80. Formal anomaly
acceptance is **insufficient support**, not pass or an adequately powered
rejection, because only four anomalies exist.

Phase-change precision/recall/F1 on 416 adjacent projected-event pairs was
0.520/0.654/0.580 for rules, 0.645/0.543/0.589 for Jev per-event,
0.701/0.560/0.622 for microbatch, 0.495/0.385/0.433 for triggered Jev,
0.618/0.684/0.649 for Haiku per-event and 0.624/0.483/0.545 for triggered
Haiku. These are descriptive projected-label boundaries, not physical process
transitions, task completion or liveness measurements.

### Operations and cost

Versions resolved to `jev-1.14.0` and `claude-haiku-5-5`. Remote conditions ran
sequentially, each with four independent session workers and three repetitions.
The measurements include gateway/network time; caching, network load and
provider load were not controlled. Throughput is prediction rows divided by
runner wall time, not sustained provider capacity.

| Condition | Remote requests / failures | HTTP P50 / P95 (ms) | Prediction rows/s | Input / output tokens | Jev direct list estimate (USD) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Jev prefix | 1,308 / 0 | 1,139 / 3,239 | 2.156 | 3,718,245 / 366,999 | 0.15617 |
| Jev microbatch | 834 / 0 | 3,007 / 6,470 | 1.525 | 8,664,513 / 365,617 | 0.36391 |
| Triggered Jev | 594 / 1 | 2,557 / 4,937 | 2.853 | 1,839,543 / 166,344 (partial) | 0.07726 (partial) |
| Jev offline | 1,308 / 0 | 2,339 / 4,863 | 1.412 | 4,521,438 / 367,046 | 0.18990 |
| Haiku prefix | 1,308 / 0 | 4,639 / 7,079 | 0.686 | 3,816,303 / 291,775 | — |
| Triggered Haiku | 594 / 0 | 4,302 / 6,508 | 1.633 | 1,967,004 / 142,402 | — |
| Haiku offline | 1,308 / 0 | 4,122 / 6,509 | 0.757 | 4,977,834 / 387,582 | — |

Rules prefix compute P50/P95 was 0.196/0.632 ms, throughput 2,615 rows/s;
offline 0.142/0.455 ms and 3,413 rows/s. No remote request or token cost.
The one triggered-Jev timeout remains an abstention, with no retry or invented
usage; its token and estimate totals exclude any unreported timeout usage.
Both providers' **gateway billed costs are unknown**. Jev estimates use the
published direct price of $0.042/M input, free output, and are not invoices.
No Haiku bill or list-price estimate is asserted without a verified rate for
the served model. Reported token counts permit later pricing reconciliation.

Both triggered arms called the model for 198/436 unique events (45.4%).
Their mixed event-latency P95s were 4,442 ms (Jev) and 5,924 ms (Haiku);
mixed P50s were below 1 ms because rules handled the majority. Microbatching
reduced request count 36.2%, but repeated per-target contexts increased input
tokens/list estimate 2.33 times, with lower throughput and worse latency.
Timestamp-derived batch wait P50/P95 was 500/500 ms; projected event P95 was
7,153 ms. Queue waits are logical estimates, not a wall-clock arrival replay.

All Jev streaming conditions missed HTTP P95 <=1,500 ms. Haiku met its
declared <=10,000 ms HTTP threshold. All five met the common projected async
P95 <=15,000 ms criterion on this cohort, so asynchronous architecture makes
Haiku's longer RTT manageable here. This does not fix either arm's quality
failures or establish queue behavior under multi-tenant load.

### Streaming versus offline

Offline models receive bounded windows selected from complete captured
sessions, under the same 8 KiB cap. Reference labellers reviewed the whole
sanitized session. This comparison does not test unlimited complete-context
model analysis, nor model learning across sessions.

| Classifier | Prefix activity F1 | Offline activity F1 | Prefix minus offline F1 [paired event CI] |
| --- | ---: | ---: | --- |
| Rules | 0.420 | 0.420 | 0.000 [0.000, 0.000] |
| Jev | 0.393 | 0.305 | +0.088 [0.032, 0.143] |
| Haiku | 0.692 | 0.562 | +0.130 [0.075, 0.191] |

Offline context worsened both model activity scores in this frozen formatter;
session-cluster sensitivity intervals also exclude zero (Jev [0.002, 0.156],
Haiku [0.061, 0.184]). Offline failure recall was 0.758 for Jev and 0.841 for
Haiku; anomaly precision was 0.111 and 0.333. More future context did not
automatically improve current-event attribution. Context displacement, target
attribution and the causal formatter deviation are possible explanations,
not experimentally isolated causes.

### Failure inspection and decision

The local failure packet retains 45 cases, five per condition, chosen after
scoring for high disagreement. It is diagnostic material, not a second test
set; disagreement with AI reference labels is not automatically model error.
Observed taxonomy patterns include recovery evidence classified as ongoing
failure/blocked state, ordinary repeated tool actions elevated to anomaly or
loop, and tool failures retained as ordinary active actions in the triggered
Haiku condition. Only taxonomy-level patterns and aggregate results are
published; the examples and all associated session text stay private.

All five candidate streaming conditions have acceptance status **fail**.
Population, frozen adjudication, independent annotation and protocol-conformance
gates pass. Every condition misses the any-failure recall lower bound of 0.85;
Jev per-event is close at 0.846 but does not pass. Only Haiku per-event passes
retry recall's lower bound of 0.80. Anomalies, loops and activity improvement
have insufficient rare-positive support for formal acceptance. Numerically,
Haiku improves activity F1, while Jev's paired intervals include zero.
No criteria were relaxed and no rule or trigger was tuned to these results.

Keep native parsing and storage unchanged. If ASI proceeds, use the optional
asynchronous sidecar boundary above and keep it in shadow mode: no automatic
alerts, routing, session control or completion decisions based on these labels.
Develop the trigger and atomic failure/recovery questions on separate sessions,
then evaluate against independent human-reviewed references with enough rare
positives and a representative prospective workload. Compare triggered Haiku
and Jev again under the same queue and cost accounting; qualify provider terms,
failure behavior and calibration before a product pilot. The current evidence
supports the architecture boundary, not a decision to select Jev.

### Resource accounting and delivery limits

Sol/high replacement labellers and adjudicator were dispatched at 10%, 11%
and 14% weekly usage, below the configured 60% brake. Latest account reading:
17%. The local ledger's annotation-window aggregate contains 1,953 Sol
request records: 2,955,548 reported input tokens, 456,630 output, 255,023,744
cache-read and 126,090 reasoning tokens (zero cache-write). These are separate
reported fields, not an additive billing formula. The ledger combines
concurrent host traffic; it cannot establish exact task/Worker usage or cost.
Worker-specific billed cost is unknown. See [the scoped ledger aggregate](REAL_SOL_USAGE.json).

The thermal guard stopped the first analysis at 85°C; the unchanged paced
analysis completed at 68.5°C. No local cargo/node test or project typecheck was
run. CI runs synthetic checks without private inputs; it does not independently
reproduce private-corpus results. The private corpus is required to reproduce
the exact cohort locally. **No real session content, labels, predictions,
identifiers, hashes, paths or failure examples are committed or pushed.**
