# Real-session ASI evaluation protocol v1

Frozen before selecting labels or observing classifier predictions, 2026-10-30.
This protocol supersedes neither the fixture experiment nor native parsing.
Changes after scoring starts require a new named protocol and new evaluation.

## Custody

Use only this user's local Claude Code and Codex physical session logs. All raw
copies, inventory paths, projections, annotation files, predictions, response
details, examples, failure cases, and spot-check material stay under gitignored
`private/`, mode 0700 (files 0600). Never upload them as CI artifacts or commit
them. Public output is code, this spec, and aggregate numeric results only.
Original native sources are read-only. Raw bytes are preserved locally.

Only sanitized, explicitly reviewed projections may leave this machine for
inference. Exclude system/developer prompts, attachments, hook context,
credentials, authorization headers, tokens, private paths, environment values,
and unknown native payloads. Redaction is a prerequisite, not a guarantee from
regex matching. A session with unresolved sensitive content is excluded. If
the Manager cannot confidently decide that the remainder is appropriate for
Magpie and its provider, ask the user before egress. There is no remote raw-log
labelling, private-data CI, automatic external fallback, or automatic retry.

## Population and sampling

Aim for 20 complete captured physical sessions (minimum 15), at least five from
each harness where eligible, and 300–600 labelled target events. Require logs
below 10,000,000 bytes, older than 24 hours, stable size/mtime across capture,
and a native session start. Exclude subagent/fork duplicates, harness fixtures,
this evaluation's sessions, and logs that cannot be safely sanitized. A stable
closed file is a complete captured log, not proof that the underlying task
finished; do not invent a session-end event. Record exclusions as counts/codes.

Pre-label availability clarification: the older eligible native CLI logs are
predominantly standalone orchestrated runs. Select `manager` and `worker`
roles, omit preflight/acceptance runs, and keep one physical capture per
distinct original objective and project. Report role counts and the resulting
harness/role confounding; this cohort is separate from interactive sessions.
Do not generalize across them. Entire serialized orchestration
envelopes are replaced with a task-instructions-redacted marker before egress.
This clarification precedes all blind labels and model predictions.

Before the projection freeze, an independent privacy reviewer inspects every
remaining projected string. Fixable residual paths, artifact references, or
instruction fragments cause replacement of the entire affected text field
with `[privacy review redacted]`, never selective rewriting. The private plan,
original bundle, transform hashes, and review audit remain local. Re-review
every changed field; exclude a whole session if any uncertainty remains.
Publish only field/session counts. This privacy pass precedes all labels.

Before predictions, stratify candidates using native errors, repeated tool
invocations, unusual record kinds, and ordinary work. Choose varied local work
without reading classifier output. Preserve full selected session order and
history. Include all events eligible under the frozen projection as context;
target-sample using seed 20261030 within harness and selection strata. Include
rare-cue targets and random ordinary targets. Report this enrichment explicitly;
precision/false-alert estimates apply to this sample, not deployment prevalence.
If full-session scoring is needed for false-alert rates, label all eligible
events in the selected sessions; do not estimate per-session budgets from a
subset of targets. Do not drop events based on label/prediction difficulty.

## Frozen projection

Allowed envelope fields: harness, native record kind, native role, tool name,
normalized result success/error/exit status, native stop reason, relative event
order, and redacted bounded visible user/assistant/tool text. Include Codex
`payload.reason`, `payload.error.message`, and Claude `message.stop_reason`.
Keep error reason distinct from visible text. Never include source filename,
physical session ID, private cwd, user identity, raw native JSON, labels,
selection stratum, timestamps, or annotation notes in the model input.

The private review bundle separately carries `arrival_ms`, relative to the
first usable native timestamp, solely for local batching/queue accounting. It
is never part of the projection text or provider input. Missing timing remains
unknown, and cannot establish arrival-to-label latency acceptance.

Use a maximum 1,200 characters per text field, 8,192 UTF-8 bytes per target
window, newest whole context rows with the target reserved, and original
chronological target position. Prefix sees only current/earlier rows. Offline
sees the selected complete captured session under the same cap. Truncation and
redaction counts are reported. Log text is data, not model instructions.

Taxonomy uses `benchmark.py` with the following real-only description
clarifications in `real_run.py`, shared by labellers and providers: completion
means the assistant reports the current requested work/turn complete (a stop
marker alone is insufficient); ending requires session-terminal evidence;
ordinary tool failure alone is not an anomaly. Archived experiment prompts
remain unchanged. The labels are: activity
`task_request/progress/tool_action/wait/failure/recovery/completion/unknown`;
failure `none/tool/permission/environment/model/transport/malformed/unknown`;
retry `none/retry/loop`; anomaly boolean; phase
`start/active/blocked/recovery/ending/unknown`. Completion is a turn-level
outcome; `ending` needs session-terminal evidence, not merely the last captured
row. Repeated commands alone do not prove a retry or loop. An anomaly is an
unexpected record/behavior requiring investigation, not every normal failure.

## Independent reference

Freeze the corpus and sanitized projection hashes before labelling. Two blind
labellers receive sanitized causal prefixes, the taxonomy, and offline views
in separate passes; neither receives rules, model predictions, or authoring
history. Labellers must not have designed the rules. Retain their independent
answers unchanged; a separate blind adjudicator resolves disagreements using
the same evidence, and may retain `unknown`. Record raw agreement and Cohen's
kappa per dimension before adjudication. If an allowed field is redacted or
unavailable, label only supported evidence and mark the ambiguity locally.

Select approximately 50 events using the frozen seed, stratified across both
harnesses and rare labels, for a local user spot-check. Record user corrections
and agreement when returned. Pending user review is reported as pending and
never counted as agreement or independent validation. No predictions run until
blind annotations and adjudication are frozen. No threshold/rule tuning on the
evaluation reference. Any further development uses separate sessions.

## Conditions

Use identical frozen inputs, taxonomy, causal history, trigger gate, and trial
count for rules, Jev per-event, Jev microbatch (four targets/500 ms), rules plus
triggered Jev, Haiku per-event, and rules plus triggered Haiku. Compare rules,
Jev, and Haiku prefix versus offline per-event using the same offline reference;
report causal-label scores separately. Three remote repetitions quantify
stability and latency; they do not increase the sample size. Keep prefix
microbatch LLM disabled because sibling targets can leak future context.
Measure RTT separately from queue delay and caller compute. Model IDs and
resolved versions are recorded locally; publish only provider/version labels.

## Predeclared acceptance criteria

A candidate semantic sidecar must meet all primary quality conditions, without
weakening raw/native integrity or privacy:

- Anomaly precision: lower 95% event-bootstrap confidence bound >= 0.80.
- False anomaly alert budget: upper 95% session-bootstrap bound on mean false
  positives <= 1 per complete scored session. Also report sessions exceeding 1.
- Any-failure recall: lower 95% bound >= 0.85.
- Retry recall and loop recall separately: lower 95% bound >= 0.80.
- Activity macro F1: lower 95% bound on paired improvement versus rules > 0.
- Fewer than 10 unique positive examples in any primary class means evidence
  insufficient for acceptance, even if the point estimate is perfect.
- Model request failure fraction <= 1%; Jev HTTP P95 <= 1,500 ms; Haiku HTTP
  P95 <= 10,000 ms; projected asynchronous arrival-to-label P95 <= 15,000 ms.
  Logical batch wait is explicitly an estimate rather than wall-clock replay.

Report phase per-class and boundary P/R/F1; phase alone cannot establish task
completion or process liveness. A failed or insufficient primary criterion
means no production recommendation, even if another aggregate improves.
Evaluate triggered Haiku by the same quality criteria; asynchronous operation
makes its latency tolerance intentionally looser than Jev's.

## Statistics, cost, and execution

Report accuracy and per-class P/R/F1 with 95% percentile bootstrap intervals,
2,000 draws, seed 20261030. Resample unique events; average repetitions within
each sampled event rather than treating trials as independent. Use paired
draws for model-minus-rules comparisons. Report a session-cluster sensitivity
analysis and session resampling for alert budgets because within-session events
are correlated. State unique event/session/positive counts. No significance
claim is based on repetition count.

Attempt provider/gateway usage-ledger attribution and explicit Haiku billing
units. Report actual billed cost only with matched documented ledger evidence;
otherwise report tokens plus unknown billed cost. Jev direct list estimates
remain labelled estimates and are never substituted for gateway billing.

CPU-bound execution requires a current measured temperature in
`/home/marvin/.cache/thermal-watch/temps.log` before and after work, stopping at
85 °C or on unavailable telemetry. Refresh from actual kernel CPU readings;
never synthesize readings. Parse one file below 10 MB at a time; larger logs
are excluded, not read wholesale. No local cargo/node tests or project-wide
typechecks. CI receives only code and synthetic unit inputs, never private
corpus/projections/labels/predictions/cases. Public aggregate export uses an
explicit numeric schema, with no text examples, session identifiers, hashes,
source paths, timestamps, or private free-form strings.
