# ASI/Jev event-classification benchmark

This experiment compares a small deterministic rules baseline with TypeSafe Jev and, optionally, an OpenAI-compatible lightweight LLM. It reads only the reviewed sanitized fixture corpus at `data/events.jsonl`; it is not a reader for private native session files and does not change the production parser.

## Input and context

Each JSONL row has an `id`, `split`, `origin`, `source`, `context`, `native`, and `gold`. `native.raw` is the exact native JSONL event string and `native.raw_sha256` must match it. `context` supplies `session_id`, `event_order`, and an optional timestamp. `gold.streaming` and `gold.offline` each contain the five classification dimensions. `source`, gold labels, split, annotation notes, session identifiers, timestamps, native headers, and raw native lines stay out of provider requests.

The runner extracts bounded user and assistant text, tool names, sanitized command strings, and tool result text/status from recognized Codex and Claude event envelopes. It excludes system and developer messages, arbitrary tool arguments, and unselected native fields. It hashes each exact raw event for local result provenance. Unknown harnesses and malformed events remain explicit evidence gaps.

`--context prefix` gives a target its same-session history through that event and scores it against `gold.streaming`. `--context offline` gives it same-session history from the complete captured session and scores it against `gold.offline`. Context is capped at 8,192 UTF-8 bytes per target by default; truncation keeps the target event and the newest available history. Jev uses its shared `state` for a single event and attaches distinct prefixes to questions in a microbatch. The TypeSafe API documents that each question is evaluated in isolation with shared state and that question's context.

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
MAGPIE_BASE_URL=https://magpie.onsager.ai \
MAGPIE_API_KEY=… \
python3 experiments/asi-jev/benchmark.py \
  --strategy jev --mode microbatch --context prefix \
  --repeats 3 --max-requests 127
```

The same runner accepts `ASI_API_BASE` and `ASI_API_KEY`. The default Jev request model is `typesafe/jev-latest`; every response's resolved model version is recorded. To compare the lightweight LLM, supply its exact model name:

```sh
MAGPIE_BASE_URL=https://magpie.onsager.ai \
MAGPIE_API_KEY=… \
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
