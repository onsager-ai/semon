# ASI/Jev event-classification benchmark

The real-session follow-up uses [REAL_PROTOCOL.md](REAL_PROTOCOL.md) and the
`real_*.py` tools. Its corpus and individual outputs are strictly local under
gitignored `private/`; the fixture benchmark described below is separate.

## Private real-session reproduction

Use Python 3.10+ and the standard library. Restore the private frozen artifacts
locally to reproduce the exact cohort; a fresh inventory changes as local logs
age or new captures appear. Never copy private inputs to CI or a remote. The
guarded helpers refresh actual kernel CPU telemetry before/after bounded reads
and stop at 85°C or missing readings. Synthetic tests run in CI only.

Before freezing, prepare one native file under 10 MB at a time. The inventory
contains only this user's old local logs. Preparation preserves exact raw
bytes and provenance locally, rejects open/unstable/unsupported captures, and
redacts the reviewed allowlist. Candidate ranges are deliberately bounded:

```sh
python3 experiments/asi-jev/real_prepare.py --inventory
python3 experiments/asi-jev/real_prepare.py --harness claude-code --start 0 --end 160
python3 experiments/asi-jev/real_prepare.py --harness codex --start 0 --end 30
python3 experiments/asi-jev/real_prepare.py --task-metadata
python3 experiments/asi-jev/real_select.py
python3 experiments/asi-jev/real_finalize.py
```

Selection keeps whole Manager/Worker captures from distinct tasks, enriched for
error cues, with 7–60 eligible events per capture. This small-session cohort is
not representative of all interactive sessions. Finalization makes fresh
projections from stable selected snapshots. Do not rerun selection over a
finalized bundle. Manually review every projected field for privacy before
attesting to the freeze; exclude uncertain sessions before any provider call.
For fixable findings, create a local plan with a `redactions` array containing
`packet` basenames, `event_order`, and text `field`, then run
`python3 experiments/asi-jev/real_privacy.py --plan experiments/asi-jev/private/privacy-plan.json` from the
repository root. It preserves the original bundle and replaces entire fields.
Re-review the changed packets before freezing. The plan and audit stay local.

```sh
python3 experiments/asi-jev/real_run.py freeze-projection \
  experiments/asi-jev/private/review-bundle.jsonl --reviewed
python3 experiments/asi-jev/real_label.py next --annotator A
# Each blind labeller submits one five-dimension JSON label on stdin;
# submit saves it before returning the next causal prefix.
python3 experiments/asi-jev/real_label.py submit --annotator A
# Repeat independently for B. Offline access opens only after both causal
# passes finish. Each offline submission requires the returned session ID.
python3 experiments/asi-jev/real_label.py offline-next --annotator A
python3 experiments/asi-jev/real_label.py offline-submit --annotator A \
  --session-id OPAQUE_ID --reviewed
```

Labellers must not read rules, predictions, raw sources, future events during
the causal pass, or each other's labels. A third blind adjudicator resolves
disagreements before `freeze-reference`. Retain both original annotations;
user spot-check review remains separate. Freeze files and hashes live locally.
Follow [LABEL_GUIDE.md](LABEL_GUIDE.md), including its strict packet-completeness
check: parse the complete CLI JSON, use 40,000-token inner and outer output
budgets, and never label a clipped packet. Offline overrides use the exact
outer event IDs returned by the interface.
`real_adjudicate.py next` / `submit` serves one causal disagreement at a time.
After that pass freezes, `offline-next` / `offline-submit --session-id ID
--reviewed` serves complete-session review and requires an explicit full label
for every offline dispute. It writes `private/adjudicated-reference.jsonl`.
The reference input has `id` and `gold` containing `streaming` and `offline`
five-dimension objects. Then run each named condition with the same freeze:

```sh
python3 experiments/asi-jev/real_run.py freeze-reference \
  experiments/asi-jev/private/adjudicated-reference.jsonl
ASI_API_BASE=https://magpie.onsager.ai \
python3 experiments/asi-jev/real_run.py run --condition jev-triggered-prefix \
  --max-requests 4096
ASI_API_BASE=https://magpie.onsager.ai \
python3 experiments/asi-jev/real_run.py run --condition llm-triggered-prefix \
  --llm-model wokey/claude-haiku-5-5 --max-requests 4096
```

Use `run --help` for all nine conditions. Requests use existing environment
credentials, three remote repetitions, four independent session workers, and
no automatic retry. Individual outputs stay in `private/runs/`. Aggregate with
`real_metrics.py --help`: supply the private event reference, all prediction
and summary files, both independent annotations, `--adjudication-status frozen`,
2,000 draws and seed 20261030. Its public output schema permits numeric metrics
and fixed taxonomy labels only. Inspect that export before committing it.
Use `--checkpoint-pause-ms 50` to pace CPU work on the thermally constrained
local host. This sleeps between bounded draw groups without changing draws,
seeds, samples, or statistics; the 85°C stop remains active.
Private examples and the 50-event user review packet must remain local.
After both freezes, `ASI_API_BASE=https://magpie.onsager.ai python3
experiments/asi-jev/real_matrix.py --all` runs the nine conditions sequentially,
checks measured CPU temperature around every HTTP call, and prints counts only.
It stores progress and completed run paths privately. An interrupted matrix
requires inspection of those records; it never retries or resumes automatically.
After the reference freeze, `python3 experiments/asi-jev/real_sample.py`
creates the local 50-event packet, readable `spot-check/REVIEW.md`, and a blank
return template. It never treats that template as a submitted user review.
After the runs, `real_cases.py --run-dir PRIVATE_RUN` (repeat per condition)
creates a local packet of up to five unique events per condition with the most
disagreement against the common offline reference. This post-hoc packet is
for inspection, not an additional independent test set or proof of model error.
`real_operations.py --run-dir PRIVATE_RUN --output AGGREGATE_JSON` separately
exports pooled request P50/P95 and measured wall-time throughput; repeat
`--run-dir` for each condition. Both remote model arms use the same gateway.
The local Magpie ledger cannot establish remote gateway bills, so Haiku billed
cost remains unknown. Jev list-price estimates are explicitly separate.

For protocol-grade scoring, pass the disclosure-safe output of
`real_operations.py --run-dir ... --output AGGREGATE_JSON` to
`real_metrics.py --operations AGGREGATE_JSON` with the private prediction and
summary inputs. Metrics validates event, prediction-row, and request counts,
uses pooled HTTP and projected latency P95 values for acceptance, and retains
maximum session-summary P95s as descriptive values. Missing operations data
or unknown queue timing leaves latency acceptance unmeasured. Exploratory runs
may use smaller bootstrap draws or different repetition sets, but cannot pass
protocol acceptance; the frozen run uses 2,000 draws, seed 20261030, one Rules
repetition, and three repetitions for each remote condition.

This experiment compares a small deterministic rules baseline with TypeSafe Jev and, optionally, an OpenAI-compatible lightweight LLM. It reads only the reviewed sanitized fixture corpus at `data/events.jsonl`; it is not a reader for private native session files and does not change the production parser.

## Input and context

Each JSONL row has an `id`, `split`, `origin`, `source`, `context`, `native`, and `gold`. `native.raw` is the exact native JSONL event string and `native.raw_sha256` must match it. `context` supplies `session_id`, `event_order`, and an optional timestamp. `gold.streaming` and `gold.offline` each contain the five classification dimensions. `source`, gold labels, split, annotation notes, session identifiers, timestamps, native headers, and raw native lines stay out of provider requests.

The runner extracts bounded user and assistant text, tool names, sanitized command strings, and tool result text/status from recognized Codex and Claude event envelopes. It excludes system and developer messages, arbitrary tool arguments, and unselected native fields. It hashes each exact raw event for local result provenance. Unknown harnesses and malformed events remain explicit evidence gaps.

`--context prefix` gives a target its same-session history through that event and scores it against `gold.streaming`. `--context offline` gives it all selected same-session rows and scores it against `gold.offline`. Native groups are incomplete fixture excerpts; only three authored sessions (22 events) are complete. Context is capped at 8,192 UTF-8 bytes per target by default; truncation reserves the target, selects the newest whole context rows, and renders them chronologically with the target in its original position. Jev uses its shared `state` for a single event and attaches distinct prefixes to questions in a microbatch. The TypeSafe API documents that each question is evaluated in isolation with shared state and that question's context.

## Strategies

`--mode per-event` sends one request for each target. `--mode microbatch` groups up to four events from one session, flushing at the timestamp-derived 500 ms window or at the size limit. Queue wait is estimated from fixture timestamps and reported separately from provider HTTP round-trip time; the runner does not sleep to replay the delay. Each Jev question contains only the prefix for its target. Prefix microbatching is disabled for the LLM strategy because a shared chat prompt does not guarantee isolation between sibling targets.

`--mode event-triggered` is an online prefix run. The rules pass handles every target, and the model is called for events with unknown dimensions or failure, wait, retry, blocked, or recovery cues. Non-triggered events keep their rules predictions. The summary reports model request coverage separately. Rule gate compute time is measured once per event and reused for repeated model trials; projected latency adds gate compute to the HTTP round trip for triggered events. This mode requires `--context prefix`.

The rules are a transparent experiment baseline built from native event roles, allowlisted text, tool status, and short retry history. They are not Semon's production parser. Rule inference preserves chronological state in prefix order. The benchmark classifies only the current event for failure; surrounding events may inform retry, recovery, and phase.

## Running

From the repository root, run the rules baseline without API credentials:

```sh
python3 experiments/asi-jev/benchmark.py \
  --strategy rules --mode per-event --context prefix
```

Run Jev microbatches with the configured gateway credentials:

```sh
ASI_API_BASE=https://magpie.onsager.ai \
python3 experiments/asi-jev/benchmark.py \
  --strategy jev --mode microbatch --context prefix \
  --repeats 3 --max-requests 127
```

The same runner accepts `ASI_API_BASE` and `ASI_API_KEY`. The default Jev request model is `typesafe/jev-latest`; every response's resolved model version is recorded. To compare the lightweight LLM, supply its exact model name:

```sh
ASI_API_BASE=https://magpie.onsager.ai \
python3 experiments/asi-jev/benchmark.py \
  --strategy llm --llm-model wokey/claude-haiku-5-5 \
  --mode per-event --context prefix --repeats 3 --max-requests 256
```

Remote runs require at least three repetitions. The default hard request cap is 127; the runner counts the full plan and refuses to start if it exceeds the cap. It makes sequential requests with no automatic retries or cross-provider fallback. The throughput metric is end-to-end sequential throughput and does not measure concurrency scaling.

## Results and interpretation

Each run writes predictions JSONL, a summary, per-request telemetry, failures JSON, and provenance JSON under `experiments/asi-jev/results/`. Per-event output includes both gold reference views, the selected view, canonical predictions, validated answer details, abstention codes, local raw-event hash, repetition, context mode, and measured latency. No raw prompt, provider response body, API key, or API base URL is saved.

The summary reports accuracy, coverage, precision, recall, macro F1, per-class scores, and confusion matrices for every dimension. Abstentions count as incorrect in accuracy and as false negatives in class recall; confusion matrices have an explicit `<abstain>` column. Metrics are grouped overall and by split, origin, and harness. Phase also has a boundary precision/recall/F1 score over adjacent same-session events, with pair coverage reported.

Latency includes provider HTTP P50/P95, per-event compute or HTTP P50/P95, timestamp-derived queue wait, and projected online latency. Request failures retain their measured elapsed time and count as failures; individual malformed or missing answers become dimension-level abstentions. Input/output token counts come from provider usage when complete. Jev cost is a published list-price estimate using the token usage returned by the gateway, not a billed-cost claim; estimates are suppressed when request failures make total usage uncertain. For other models, pass both `--input-price-per-1m` and `--output-price-per-1m` to calculate a user-configured estimate.

The TypeSafe request and response schema is documented in the [official OpenAPI document](https://api.typesafe.ai/openapi.json). Jev published model pricing is listed in the [TypeSafe model documentation](https://docs.typesafe.ai/models).

## Reproduce the measured matrix

Python 3.10+ and its standard library are sufficient. Supply `MAGPIE_API_KEY` or `ASI_API_KEY` securely in the environment; commands below reuse it without copying a key into source or shell history. The recorded endpoint is a gateway, not the direct TypeSafe API. Model aliases may move: retained responses resolved to `jev-1.14.0` and `claude-haiku-5-5`. Each remote condition uses three trials over the same 58 unique events. Request caps below are ceilings, not target counts.

Run CPU-bound tests, corpus aggregation, and rules throughput in CI. On this host `/home/marvin/.cache/thermal-watch/temps.log` was unavailable, so no local tests, builds, typechecks, or computational corpus analysis were run. Stop CPU-bound work at 85 °C or without readings. Network clients processed this small corpus with negligible local CPU use.

```sh
# In CI, or on a host permitted to run CPU-bound checks:
python3 -m unittest discover -s experiments/asi-jev -p 'test_*.py' -v
python3 experiments/asi-jev/benchmark.py --strategy rules --context prefix --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/benchmark.py --strategy rules --context offline --run-id reproduce --out /tmp/asi-jev-replay

# Network replays: keep output separate from retained measurements.
export ASI_API_BASE=https://magpie.onsager.ai
python3 experiments/asi-jev/benchmark.py --strategy jev --mode per-event --context prefix --repeats 3 --max-requests 256 --timeout 20 --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/benchmark.py --strategy jev --mode microbatch --context prefix --repeats 3 --max-requests 127 --timeout 20 --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/benchmark.py --strategy jev --mode event-triggered --context prefix --repeats 3 --max-requests 127 --timeout 20 --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/benchmark.py --strategy jev --mode per-event --context offline --repeats 3 --max-requests 256 --timeout 20 --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/benchmark.py --strategy llm --llm-model wokey/claude-haiku-5-5 --mode per-event --context prefix --repeats 3 --max-requests 256 --timeout 20 --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/benchmark.py --strategy llm --llm-model wokey/claude-haiku-5-5 --mode per-event --context offline --repeats 3 --max-requests 256 --timeout 20 --run-id reproduce --out /tmp/asi-jev-replay
python3 experiments/asi-jev/load_probe.py --concurrency 1 2 4 --requests-per-level 12 --out /tmp/asi-jev-load.json
python3 experiments/asi-jev/projection_probe.py --out /tmp/asi-jev-projection.json

# CI aggregates either the retained matrix (default) or one new matrix:
python3 experiments/asi-jev/analyze.py --results /tmp/asi-jev-replay
```

See [REPORT.md](REPORT.md) for the recommendation and limitations, [results/RESULTS.md](results/RESULTS.md) for the matrix, and [results/comparison.json](results/comparison.json) for complete per-class/stratified metrics. `results/ci` preserves the original measured rules timings; workflow reproductions are uploaded separately. Aggregation re-scores saved predictions against the corrected reference after verifying native hashes. Original per-run gold/summary fields remain historical; use `comparison.json` for final scores. Correction snapshots and the superseded initial offline formatter experiment remain in `evidence/`.
