# Native session navigation resolution

`session_catalog_resolve_native(options, source_key, harness, native_id, scope)`
resolves native links against the selected configured source's persistent catalog.
The host authorizes that source before calling it. No event cache, native body,
global model, provider status or credential health check is required.

`GET /api/session-resolve?machine=<source>&harness=codex&native_id=<native>`
returns `{api:1,resolution:{state,source_key,harness,native_id,read_scope,catalog_key,generation}}`.
Scope defaults to `current`; `retained_history` is an explicit read intent.
`native_resolution` in session capabilities advertises an actually wired endpoint.

States are `resolved`, `provisional`, `pending`, `ambiguous`, and `unavailable`.
Only a coherent complete catalog and exactly one owner yields `resolved`.
A partially discovered catalog yields `provisional`, even with one owner.
A missing binding yields `pending`; multiple owners yield `ambiguous`.
An absent/unsupported projection or invalid selected metadata yields `unavailable`.
No state grants native control authority. Facts/native selection, runtime readiness,
and scoped current control identity remain independently checked.

The SQL native mapping lookup uses a covering index and at most two distinct
canonical keys. It then loads at most one metadata row, capped at 1 MiB and
64 source references, in the same SQLite snapshot as catalog version/generation.
It verifies parsed native membership and the configured logical input root.
Current uses current bindings; explicit history also considers retained bindings.
No native identifier is assumed to equal a canonical key or inferred from a path.

Verification: `native_alias_is_exact_bounded_and_preserves_ambiguity_and_provisional_reads`
passes, including absent local bytes/facts, provisional discovery, exact harness
scope, ambiguity, retained bindings and a covering query plan. The copied exact
executable contains 519 tests. Formatting and package all-target locked Clippy
pass. Full combined CI and actual managed-session UI handoff verification remain
integration gates; this test does not claim browser latency or complete #319.
