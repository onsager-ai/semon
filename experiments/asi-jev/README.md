# ASI/Jev event-classification benchmark

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
