# Source witness for the synthetic Antigravity fixture

## Managed ACP 1.5.18

A bounded metadata inspection on hp-code-2 opened one managed conversation
database with SQLite URI `mode=ro` on its origin. `PRAGMA journal_mode`
returned `wal`. The table below is the allowlisted projection of
`SELECT name, type, "notnull", pk FROM pragma_table_info(?)`.

| Table | Column | Type | NOT NULL | PK |
| --- | --- | --- | --- | --- |
| trajectory_meta | trajectory_id | TEXT | 0 | 1 |
| trajectory_meta | cascade_id | TEXT | 0 | 0 |
| trajectory_meta | trajectory_type | INTEGER | 0 | 0 |
| trajectory_meta | source | INTEGER | 0 | 0 |
| steps | idx | INTEGER | 0 | 1 |
| steps | step_type | INTEGER | 1 | 0 |
| steps | status | INTEGER | 1 | 0 |
| steps | has_subtrajectory | numeric | 1 | 0 |

A query over at most two `trajectory_meta` rows returned only three
booleans. All were true for the sampled database.

```sql
SELECT count(*)=1, min(cascade_id=?1)=1, min(trajectory_id<>cascade_id)=1
FROM (SELECT trajectory_id, cascade_id FROM trajectory_meta LIMIT 2);
```

The bind parameter was the filename stem. The following query plan used
`SEARCH steps USING INTEGER PRIMARY KEY (rowid>?)`. Only the plan was taken.

```sql
EXPLAIN QUERY PLAN
SELECT idx, step_type, status, has_subtrajectory
FROM steps WHERE idx>=?1 ORDER BY idx LIMIT ?2;
```

## Deployed T3 and later upstream model evidence

The local runtime reported active version `0.1.13`. The installed Linux
executable's GNU build ID is `a1f01912317a13388dfea0a2dd138539d0eee970`.
The release tag resolves to `bc0f04da94f64b5e487c598761a43b4cf06b2ac9`.
The live database's `effect_sql_migrations` head was 119. The writer
version of individual historical rows remains unknown.
[Release](https://github.com/pingdotgg/t3code/releases/tag/v0.1.13)

On the deployed release, a read-only WAL query sampled eight completed
Antigravity turns. The plan used indexed lookups on four
`orchestration_v2_projection_*` tables in order: provider threads by
`provider_instance_id`, runs by `provider_thread_id`, attempts by `run_id`,
and provider turns by `run_attempt_id`. The output retained only booleans,
JSON types and usage enums. Every sampled attempt had an ordinal of at least 1,
a string `nativeThreadId` and a SQL-null forward turn pointer. Every sampled
turn reported `main_agent` / `complete` and omitted both `payload_json.model`
and `payload_json.turnTokenUsage.models`. This was a field-shape sample.
[Deployed attempt writer](https://github.com/pingdotgg/t3code/blob/bc0f04da94f64b5e487c598761a43b4cf06b2ac9/apps/server/src/orchestration-v2/ProviderTurnStartService.ts#L2138-L2145) ·
[Deployed ACP usage writer](https://github.com/pingdotgg/t3code/blob/bc0f04da94f64b5e487c598761a43b4cf06b2ac9/packages/provider-acp/src/server/adapter.ts#L397-L433)

A later revision, `5d94090ed031fd37d27cca847f571ab5ce2da26c`, populates the
provider turn's `model` from the latest native planner response after the
last user prompt. This is a main-agent execution observation, with no
per-model token allocation implied. The shared usage schema also allows
`turnTokenUsage.models` as an optional array of provider-reported model IDs.
The pinned ACP writers do not populate that array. The fixture's
`upstream_executed_model` marker identifies simulated evidence from the
later writer on `fixture-next`. The writer accepts only IDs matching
`^[A-Za-z0-9-]{1,64}$` before reading execution metadata. The encoding case
`fixture-cascade+` therefore has unknown execution. The generic array on
the other origin demonstrates a field whose Antigravity provenance is unqualified.
[Executed-model change](https://github.com/pingdotgg/t3code/commit/5d94090ed031fd37d27cca847f571ab5ce2da26c) ·
[Native ID check](https://github.com/pingdotgg/t3code/blob/5d94090ed031fd37d27cca847f571ab5ce2da26c/apps/server/src/provider/acp/AntigravityChildWork.ts#L1510-L1514) ·
[Shared usage schema](https://github.com/pingdotgg/t3code/blob/5d94090ed031fd37d27cca847f571ab5ce2da26c/packages/contracts/src/providerRuntime.ts#L56-L85)
