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

Only native `anthropic` and `openai` identities enter the table. For plain IDs,
Claude usage selects Anthropic and Codex selects OpenAI. For a routed ID with
slash-separated segments, lookup uses the final segment in the provider tables;
exact IDs and the existing dated/bracketed native aliases are accepted. The
match must belong to exactly one provider. Lookup never substitutes another
model or family. Newly listed IDs do not require a release. Both providers and at least two explicitly priced
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

## Magpie execution evidence for Claude Code and Codex

Semon keeps recognized Magpie-only route costs unknown, even for zero-token
observations. The native parsers do not establish which upstream provider and
model executed a Magpie call. The routed-ID lookup described above is a model
name lookup. A matching suffix, configured model, requested model, or model
sent on the wire cannot establish execution.

Magpie v0.1.4690 records the model named by the upstream reply as `served` on
its own usage rows. For Responses the sniffer reads `response.model` in stream
events or `model` in a whole response. For Anthropic it reads `message.model`
in stream events or `model` in a whole response. The gateway copies that
value into the usage ledger with the client-facing `response_id` and
`native_session` when available. The upstream request ID (`rid`) is distinct
from the response ID. The same usage endpoint also returns native-log
projections and accepts client-reported `served` values. Only gateway-owned
rows from an authorized source can supply this evidence.
[Upstream response parser][magpie-sniffer] · [Gateway ledger write][magpie-write] ·
[Ledger source rules][magpie-source] · [Usage endpoint][magpie-api] ·
[Client reporting][magpie-ingest]

- **Claude Code.** Match an admitted native response's `sessionId` and
  `message.id` to the gateway row's `native_session` and client-facing
  `response_id`. Magpie recognizes `x-claude-code-session-id` and captures the
  Anthropic message ID sent to the client, including an ID chosen by its
  translation encoder. Semon deduplicates assistant usage by `message.id`.
  Some native records carry an outer `requestId` that Magpie's native reader
  parses. Semon's Claude usage observation does not retain it, and its
  equality with the upstream `rid` across gateway translations is unverified.
  Missing native session or matching response ID leaves the gateway cost
  unknown. [Session header handling][magpie-session] ·
  [Encoder ID capture][magpie-response-id] ·
  [Magpie's Claude reader][magpie-claude-reader] ·
  [Semon's Claude observation][semon-claude]
- **Codex.** Match the `session_id` and `response_id` of an accepted
  `token_usage_record` to the gateway row's `native_session` and client-facing
  `response_id`. Require a unique match to the native
  `(thread_id, session_id, turn_id, response_id)` observation. The gateway
  row does not carry that full native tuple. Magpie already uses unique
  response IDs for its own Codex pairing. Older cumulative `token_count`
  events lack a per-response ID, and `turn_context.model` is turn configuration.
  Session- or thread-level counters alone cannot establish per-call usage.
  [Pinned native probe](cli-compatibility.md#codex-native-fork-boundaries-and-child-usage) ·
  [Semon's Codex decoder][semon-codex] · [Magpie's Codex pairing][magpie-codex-pair]

The smallest next change is to admit a metadata-only, gateway-owned response
observation into Semon's existing usage projection. It needs a stable source
identity and call identity, the native session and client response IDs, and an
upstream-served model with a verified models.dev provider namespace. A
user-configurable Magpie provider label is insufficient to identify that
namespace. Accept only one-to-one matches with the same usage scope. A
response spanning several attempts or upstream calls needs independently
attributed usage for each call before it can be priced. Known native response
counters can supply rate dimensions when they cover the verified call.
Missing identities, ambiguous matches, or missing rate dimensions leave the
estimate unknown.

Look up the exact provider and model ID and use only rates with models.dev
provenance for the categories consumed, including applicable cache and tier
rates. Label the result **estimated, list price (models.dev)**. Any separately
recorded billed amount can be kept as reconciliation evidence in Semon. The
Magpie ledger's calculated cost is a list-price computation, and a reported
client cost is a client assertion. Neither supplies rated usage dimensions or
proves a billed charge. Semon has no admitted Magpie input or correlation path
today. [Magpie cost calculation][magpie-cost]

A synthetic producer-to-Index test should pair a gateway response with each
harness and give its requested alias a different price from the served model.
Assert the exact models.dev estimate and the original reconciliation amount.
Exercise missing or duplicate response IDs, a reported row, an alias-only
row, a cumulative Codex counter, and a category available only in the
bundled table. Each of those cases should leave the estimate unknown.

[magpie-sniffer]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/gateway/sniff.go#L207-L277
[magpie-write]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/gateway/gateway.go#L6098-L6117
[magpie-source]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/usage/ledger.go#L125-L155
[magpie-api]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/gateway/usage_api.go#L74-L95
[magpie-ingest]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/gateway/usage_ingest.go#L75-L81
[magpie-session]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/gateway/gateway.go#L10758-L10836
[magpie-response-id]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/gateway/response_id.go#L5-L46
[magpie-claude-reader]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/sessions/claude.go#L339-L354
[magpie-codex-pair]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/usage/codex_identity.go#L44-L69
[magpie-cost]: https://github.com/yetone/magpie/blob/62852a53637c43ee1dbc004fa42aae03365143cb/internal/usage/usage.go#L573-L603
[semon-claude]: https://github.com/onsager-ai/semon/blob/b21cde70e18df5e6d05bcc5fc84e3f00e1185c5e/crates/semon-sessions/src/events.rs#L6421-L6542
[semon-codex]: https://github.com/onsager-ai/semon/blob/b21cde70e18df5e6d05bcc5fc84e3f00e1185c5e/crates/semon-sessions/src/events/usage.rs#L130-L159

## Retained derived costs

Every newly computed cost records its pricing fingerprint, which includes an
explicit lookup-rules version as well as effective table contents. Increment that
version when alias or provider resolution changes so retained costs are repriced.
Coverage counts tokens with known rates and tokens without rates separately;
web-search requests are not tokens. A separate producer
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
