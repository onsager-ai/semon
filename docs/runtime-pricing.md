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
nonfinite values in mapped fields and invalid context boundaries reject the
entire v1 update with provider/model/field context. Unknown cost keys and
non-context tiers are ignored; they do not freeze otherwise valid updates.

For each category, precedence is valid explicit v1, valid exact-ID Anthropic v2
TTL supplement, same-model bundled value, then unknown. Explicit v1 categories
override bundled categories for the same model, including rows protected from
the HTML synchronizer by `sync: false`. The v2 supplement only maps
`anthropic.models[id].cost.text.input.cache.write.{5m,1h}` and only for exact
Anthropic IDs already present in the merged v1/bootstrap table. A mapped value
must be finite and nonnegative. Long-context TTL values are used only when a v2
context tier has the same explicit threshold as the model's v1 context tier.
Unknown v2 fields, aliases and non-Anthropic rows are ignored. OpenAI generic,
30m and 12h writes remain unmapped to Anthropic TTL categories.

The required endpoint is <https://models.dev/api.json>. Runtime then requests
the optional <https://models.dev/experimental/v2.0/api.json> with the same
10-second connect timeout, 30-second total timeout, no redirects and 16 MiB
streaming cap. A v2 request, parse or schema failure never blocks valid v1
prices. Exact IDs retain their last-known-good v2 TTLs, including their separate
`v2_fetched_at`, until a later valid v2 response replaces them. An invalid v2
entry fails closed for that entry and retains its previous supplement. v1 still
outranks a retained v2 field. The cache fingerprint covers effective prices and
provenance, but not fetch timestamps.

The TTL split follows models.dev's upstream implementation: `packages/core/src/translate.ts`
reduces cache-write TTLs to the first v1 price, while `packages/core/src/generate-v2.ts`
preserves them. The web build publishes `_experimental_v2.0_api.json` in
`packages/web/script/build.ts`, and `packages/web/src/server.ts` serves it at
`/experimental/v2.0/api.json`. Normalized rows retain per-category
`models_dev`, `models_dev_v2` or `bundled` provenance. `scripts/pricing` and the
weekly price-sync workflow continue maintaining the bundled offline bootstrap;
they are no longer the only way runtime reference prices change.

Context tiers use the explicit `cost.tiers[].tier.size` (for example 272000),
with `context_over_200k` used only without an explicit tier. A compatibility
200k field cannot erase an authored exact hotfix boundary. The current native
calculator supports one context boundary; updates with multiple context tiers
are rejected rather than flattened. Non-context tiers are ignored. Tier categories fall back to an
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

The additive SQLite schema-31 pricing projection stores `target_fingerprint`,
phase, cursor and `completed_fingerprint` separately in the existing `meta`
projection. Nullable `files.codex_pricing_fingerprint` and
`session_catalog.pricing_fingerprint` columns are stamped by writer triggers and
the bounded pass; indexes select rows whose stamp differs before payload JSON
is decoded. A completed fingerprint is idle: one read-only metadata query checks
it and the pending queue, without a write transaction or payload decode. The
worker parks on a wake channel while idle; price swaps, stale-owner enqueues and
new index registrations wake it. A five-minute fallback performs only the same
read-only check. The process-local
index registry is capped at 64 entries and retires paths not touched for 30
minutes or whose files have disappeared. An empty registry parks the worker.

Writers enqueue a stale source or Catalog owner in the same SQLite transaction
that saves its retained facts or derived cost. Repricing drains those owners in
bounded transactions and does not revisit unrelated owners. Reads and repeated
index registration do not enqueue work. Selected recomputation admits at most
64 sources and 4096 retained usage facts or normalized dimensions per owner,
with 1 MiB metadata values. Legacy, over-budget, changed-source or incomplete
observations retain their prior cost and older/absent fingerprint. They are not
zeroed or relabeled as current and are reconsidered only after a price change or
a new retained-fact write. Appends onto a restored stale accumulator stay
explicitly unpriced until its retained facts are recomputed. A superseded table
cannot publish further work in the process; an older fetched table cannot roll
back a newer durable job.

If `events.rs` finds a pricing-only in-memory Codex fingerprint mismatch, it
falls back to `SqliteStore::load_file`/`read_one`, restoring the saved index and
retained usage facts from SQLite. It does not reopen or parse the native log
body for a pricing-only change. The asynchronous repricer updates derived costs
from those retained facts.

Activity `cost_by_day` remains a numeric `(UTC day start, USD)` series. Unknown
pricing is represented separately by `unpriced_models` and incomplete-usage
fields; the per-day series may contain a numeric partial amount. Repricing only
replaces a retained day when new retained facts produced that day and otherwise
keeps the existing value, rather than inventing a zero.

Offline unit fixtures cover the adapter, cache failures, retained session and
Home updates, and normalized Codex dimension updates across partial-job reloads.
All Rust compilation, Clippy and tests run in CI under the thermal policy.
