# Source94 native names and members (WIP)

Base: `6afc885b40a8fd171253047bb977276b474e646d`, task branch
`task/477-raw-sql-backends-20261024`. Full binding intake:
`post91-source94-name-members` in task `raw-vm87-20261026`.

The authored support schema has singleton `session_descriptions`; the former
scoped hint query incorrectly expected namespace and scanned description JSON.
The current recipe now records native endpoint names as `name:claude`
dependencies. The existing namespace/harness/native-ID/source-path B-tree
serves exact hint lookup; Current membership qualifies every hinted owner.
These dependencies schedule observation only; no Catalog row becomes a native
endpoint. Current publication clears runtime dependencies, so native hints are
repopulated from the qualified endpoint in that same fenced transaction.

The shared selector still owns unique/ambiguous/absent semantics. The shared
member replay enqueue recipe now serves ordinary and scoped adapters. Scoped
member discovery uses durable namespace/key/path cursors and captured claims,
at most 64 observations per page. The reset retains member cursors while
invalidating the epoch and model batch. Existing owner closure, source, owner,
projection and 1MiB limits remain. Metadata paging reads no Raw bodies. Name
selection requires complete discovery/reduction/copied-event qualification,
the current source revision and unchanged native source stats.

New configured tests use sanitized real Claude Raw through native production:

- `configured_semantic_domain94_claude_names_real_producer_oracle`
- `configured_semantic_domain94_name_hint_replays_native_members_and_fences_old_model`
- `configured_semantic_domain94_name_members_physical_work_is_indexed`
- `configured_semantic_domain94_name_source_change_refuses_without_effects`
- `configured_semantic_domain94_name_hints_capacity_refuses_without_truncation`

The physical-work probe separately measures setup (32768 VM maximum) and real
hint/candidate/member operations (4096 VM maximum, 64 growth, one second,
64 returned observations) for 0/256/2048 same and foreign namespace noise.
It verifies unchanged native event counts and whole-DB rollback. These are
registered before the unchanged complete configured suite in Storage CI.

Initial WIP `70a725f1` CI 40226071025 passed the old frontier physical-work test,
58 storage, 3 journal and 10 recovery tests. Configured reached 106 passes and
2 new failures: unchanged physical Raw was not checked by the selector, and
publication cleared hints. Both received production fixes. This initial run
does not validate the successor. Exact final CI and any remaining failures
are recorded in the external `evidence/worker94-delivery.json`.

No local CPU verification ran: thermal telemetry is absent. All compilation,
formatting and tests run in CI. Independent review and full exact-head checks
remain required; this document is not acceptance. Dedicated 65-member cursor,
late membership mutation and broader native-family qualification still require
explicit final evidence. Public qualification remains zero. Full #477/478/479,
PG without hidden SQLite, migration/catch-up/fenced cutover/rollback and Hub313
remain unfinished; #380 still gates any #477 merge to main.
