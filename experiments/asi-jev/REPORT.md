# Jev as a semantic preprocessor for Semon ASI

Experiment date: 2026-10-29. Source base: `893dd56fdc63fd53b7febd597c1381570c01f64b`. This is an isolated experiment; it changes no production parser, API, storage, runtime control, or mandatory dependency.

## Decision and measured evidence

Pending the frozen-corpus benchmark. The results and limits below will be completed from retained prediction and request records.

## Integration architecture

If the benchmark justifies a further pilot, put classification in an optional asynchronous ASI consumer after authorized native ingestion and parsing. Keep native event kinds, identity, lineage, source generation, byte boundaries, and parser-derived outcomes authoritative. A label is a revisable annotation, never a replacement for a native event or evidence of process liveness, user approval, completion, or permission to act.

The input should be a bounded projection: supported native event kind, selected visible text, sanitized tool name/command identity, result status, and a short causal session window. Keep credentials, environment variables, system prompts, unrestricted tool arguments, private paths, attachments, and raw log bytes outside the provider request. The fixture-only allowlist in this experiment is not a production sanitizer.

An annotation key should bind the tenant and authorized source identity, generation, consumed record/range hash, parser version, classifier model version, taxonomy/prompt version, and context policy. Store input evidence references and confidence beside the label. Changed source bytes invalidate dependent annotations. Native collection and rendering must work when the classifier is disabled or unavailable; classification must not sit inside a source/Catalog publication transaction or extend an authorized source-reader lifetime.

For a pilot, prefer deterministic native parsing and cheap rules first, with Jev only on selected uncertain semantic events. Keep retry detection and phase state per session in caller code; Jev has no durable session memory. Record distinctions between a tool failure, a recovered failure, a model failure, and unavailable evidence. Low confidence and invalid responses become unknown annotations; the reported probabilities have not been calibrated for ASI.

## Scale and reliability

Use bounded queues and per-tenant request/token budgets. A batch must keep event targets and causal context separate; it must not mix tenant data in shared state. Jev questions are independent, so dependent judgments such as failure → retry → phase must be composed in application state rather than relying on answer order. Record queue delay separately from HTTP latency and reject stale-generation results before publication.

Native capture must continue during timeouts, malformed model responses, provider outages, or rate limiting. Keep unavailable classification explicit and observable, with no automatic provider switch or replay of native operations. A future background classifier may retry an idempotent annotation request under a separately chosen bounded policy; this experiment performs no automatic request retries. Measured sequential replay throughput is not a provider capacity or multi-tenant scaling qualification. Account-specific RPS/TPS limits, burst behavior, cancellation, and outage recovery require a later load test.

## Privacy

Only checked-in sanitized native fixtures and explicitly synthetic challenge records are eligible for this experiment. No private live session logs are sent. Original repository fixture files remain unchanged. Extracted text is still content sent to the remote service; omitting the raw JSON envelope does not make a request content-free.

The measured path uses an existing Magpie gateway and the configured TypeSafe or LLM provider. Gateway processing and retention are an additional boundary. TypeSafe says it does not train on customer content; public terms do not establish a fixed request retention period, default ZDR, or regional processing selection. The privacy policy describes U.S. hosting. Sensitive session content needs explicit product consent, a production redaction policy, tenant custody controls, and verified provider/gateway contractual terms before an integration pilot. See [the primary-source research](jev-research.md).

## Reproduction and artifacts

See [README.md](README.md) for commands, [pipeline.md](pipeline.md) for source/pipeline provenance, [data](data/) for the corpus, and [results](results/) for individual predictions, probability distributions, request timing, usage, and errors. CPU-bound checks and deterministic throughput measurements run in GitHub Actions because host thermal telemetry is unavailable. Remote inference replay is network-bound and performs only bounded processing of this small sanitized corpus; it does not run a model locally.
