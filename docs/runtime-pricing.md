# Runtime model pricing

Long-running `semon sessions --serve`, `semon sessions --watch`, `semon mcp`,
and the Hub server refresh public model reference prices in a background thread
at startup and every **six hours**. A failed fetch retries after 30 minutes.
Foreground reads use an immutable in-memory snapshot and never wait for HTTP.
One-shot commands load local reference data without starting network requests.

`SEMON_PRICING_REFRESH=off` disables network refresh. Local cost recomputation
still runs in long-running processes. `SEMON_PRICING_REFRESH_SECS` accepts a
positive interval in seconds (default 21600). Neither setting enables paid
provider calls or changes provider authentication or application authority.

The public endpoint is <https://models.dev/api.json>; its
[API documentation](https://github.com/anomalyco/models.dev#api) describes a
community-maintained catalog. Prices are estimates, not provider invoices.
The local cache is `models-dev-pricing.json` in Semon's existing XDG state
directory, or Hub's configured data root. Startup prefers a validated cache,
then the bundled `pricing.json`. Corruption, network errors, invalid prices or
persistence failures retain the previous effective table. Fetches have a
10-second connection timeout, a 30-second total timeout, no redirects and a
16 MiB streaming response limit. A normalized cache is fsynced and atomically
renamed in its directory before it becomes active.

Only native `anthropic` and `openai` identities enter the table. Claude usage
selects Anthropic, and Codex selects OpenAI. Lookup uses the exact model ID first,
then the existing dated/bracketed native alias within that provider. Newly
listed IDs do not require a release. A model is never substituted with another
model family or provider. Both providers and at least two explicitly priced
models per provider must be present in an update. Nonnumeric, negative or
nonfinite rates and invalid context boundaries reject the entire update.

Explicit upstream categories override bundled categories for the same model,
including rows protected from the HTML synchronizer by `sync: false`. Missing
categories use that same model's bundled price, otherwise remain unknown.
Normalized rows retain per-category `models_dev`/`bundled` provenance and a
content fingerprint independent of fetch time. `scripts/pricing` and the
weekly price-sync workflow continue maintaining the bundled offline bootstrap;
they are no longer the only way runtime reference prices change.

Context tiers use the explicit `cost.tiers[].tier.size` (for example 272000),
with `context_over_200k` used only without an explicit tier. A compatibility
200k field cannot erase an authored exact hotfix boundary. The current native
calculator supports one context boundary; updates with multiple or non-context
tiers are rejected rather than flattened. Tier categories fall back to an
authored tier only at the same boundary. Fast mode uses explicit
`experimental.modes.fast.cost` categories, preserving nonuniform rates. Missing
fast categories use only the same model's authored fast fallback. A bundled
multiplier is retained when no explicit fast table is supplied. Missing fast
long-context categories remain unknown unless a same-boundary fallback exists.

The sampled API's generic `cache_write` has no TTL in its API contract. It is
therefore **not** mapped to native five-minute or one-hour writes, for either
provider. Anthropic's explicit `cache_write_5m`/`cache_write5m` and
`cache_write_1h` fields can map to those categories. OpenAI's generic and 30m/12h
writes never map to them. Bundled Anthropic five-minute/one-hour and web-search
rates remain distinct, with their recorded provenance. Actual usage of an
unavailable category makes the total unknown, never free.

## Retained derived costs

Every newly computed cost records its pricing fingerprint. A separate producer
revisits admitted SQLite indexes in the background. Its cursor and partial
accumulator live in the existing `meta` table and survive restart. Normalized
Codex `native_usage_facts` are replayed in batches of 128 accepted model facts;
`files.codex_native_usage` and `native_usage_dimensions` cost-model/cost-day rows
are replaced together. Native facts, identity outcomes, source receipts,
original references and parser version 27 remain unchanged. No native log is
opened or rescanned to refresh a price.

The producer then handles one Catalog owner per transaction, qualifying its
retained source generations and token totals. It updates the Catalog, matching
history, saved description, Home/Analytics and trace costs from retained usage.
A separate bounded family pass updates trace family totals. Previously captured
reported/computed comparisons become unknown until normal description derivation
recomputes them. Provider-reported historical invoice totals are never repriced.
In-memory description reuse also includes the pricing fingerprint.

Selected recomputation admits at most 64 sources and 4096 retained usage facts
or normalized dimensions per owner, with 1 MiB metadata values. Legacy,
over-budget, changed-source or incomplete observations retain their prior cost
and older/absent fingerprint. They are not zeroed or relabeled as current.
Appends onto a restored stale accumulator stay explicitly unpriced until its
retained facts are recomputed. A superseded table cannot publish further work
in the process; an older fetched table cannot roll back a newer durable job.
Completed passes repeat after 30 seconds to catch concurrent publications.

Offline unit fixtures cover the adapter, cache failures, retained session and
Home updates, and normalized Codex dimension updates across partial-job reloads.
All Rust compilation, Clippy and tests run in CI under the thermal policy.
