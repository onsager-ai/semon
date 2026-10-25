# Worker50 bounded repair handoff

Base: `d1143356e4cc542cecbb7743251e050879f89ee8` (parent `404852f6e1eff71d342a2f00e6462d567edc93e0`).

The worktree was clean before this change and `origin/task/477-raw-sql-backends-20261024` resolved to the same base.

## Repairs

- R1/R2, fixture defect: `configured.rs:5247` used `EventCache::default()` as the reference producer. `model.rs:8028-8044` requires source observations and publication sources without a runtime binding require a scoped store. The existing green two-source oracle at `configured.rs:3046-3053` opens `EventCache::open_scoped(&options.cache)`. The domain oracle now uses that same independent, ordinary cache. It still derives from the same Raw inputs and retains every row, relation, and projection-witness parity assertion. The sole explicitly normalized field remains the independently computed runtime witness (`configured.rs:5257-5275`).

- R3, fixture defect: hostname is descriptive, not source authority. `facts/source_authority.rs:20-43` defines authority as the facts file identity; `relation_stage.rs:20-50` rejects a replacement identity. The adversary now retains the hostname and atomically replaces the facts record, changing its actual identity. The publish contract in `storage/sqlite/semantic.rs:1790-1798` returns `Ok(Outcome::Conflict)` for a non-current withdrawal, not `Err`; the assertion now checks that exact refusal. The existing whole-DB and foreign-source byte-equality checks, and the authorized retention/custody checks, are unchanged.

No reserved regions were contacted. No production code changed. No local CPU-bound check was run; CI has not been run.

## Remaining work

D3/D4 lifecycle work; incidents; cross-harness keys; selectors/frontier; recovery intents; S2 integration and S3/S4; full PG; migration/catch-up/cutover/rollback; Hub313 RLS; combined 36/38/40/43/48 review; initial_catalog cold timing; union 30s; Viewer visual/transport binding.
