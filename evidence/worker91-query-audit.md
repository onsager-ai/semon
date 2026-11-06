# Worker91 header query access-path audit

Task raw-vm87-20261026; Source91/model21/controller-own2. One Manager-authorized
1200s attempt; no retry, counter reset, fallback or acceptance. Base verified
bc06f31cb329e0ae852eace7f2a86764c34067ed on
origin/task/477-raw-sql-backends-20261024. All eight supplied message IDs are
acknowledged; the full original contract and continuity records remain binding.

## Actual authored schema

`crates/semon-sessions/src/events/store/semantic_stage.rs:424` defines
`metadata_indexes`. The scoped native index is
`meta(namespace,json_extract(value,'$.header.id'),key)`; the parent index uses
`json_extract(value,'$.header.metadata.parent_thread')`. Both are partial on
`key GLOB 'native_codex_header:*' AND json_valid(value)`. The singleton family
uses the same expressions and predicate without the namespace column.
`events/store.rs` assembles these indexes through
`semantic_stage::metadata_indexes(p.metadata)` in its authored schema recipe.
There is no CASE expression in these header indexes. The Manager's proposed
CASE mismatch was a hypothesis, not the actual DDL.

The original configured `needed_header` and `frontier_page` already matched
these expressions and partial predicates. They also admitted an alternative
namespace/key primary-key range over all native headers. The supplied actual
CI 40221069278/job121252554453 measured header71/parent100/headerid569 at zero
noise and 2121/2150/2619 at256 same+foreign noise: exactly2050 additional VM
instructions each. This is evidence of growing production access work; it is
not a fixture failure. Which alternative the previous planner selected is an
inference from those counts, not a claimed EXPLAIN observation.

## Repair

`storage/sqlite/semantic.rs` now selects the existing named header-native or
header-parent index with `INDEXED BY` in actual `needed_header`, plus both
singleton/scoped `headerid` and `headerparent` frontier queries. This makes the
bounded access path a required part of preparing the production statement;
missing/ineligible indexes refuse instead of falling back to history scans.
No new DDL, CASE rewrite, index duplication, parser or identity recipe was added.
The valid-JSON predicate, native key prefix, namespace equality, singleton owner
join, dependency qualification and unclaimed-source exclusion remain intact.

Source87's page-local `FrontierLayout`, owner transaction, full custody checks,
epoch/schema/main+TEMP/ABA fences and shared `NativeFrontierPort` recipe are
unchanged. No global/thread/unchecked cache was introduced. All source/owner/
projection64,1MiB,4096queryVM,64growth,32768setup and2s/60s cold limits remain.
The existing physical-work regression still runs0/256/2048 unrelated rows in
both namespaces, three actual native identities and every registered operation.
Each operation now also prints real elapsed microseconds and asserts<1s, as
the separately measured layout setup already did. VM measurement remains on
the actual connection and production adapter, without diagnostic SQL substitutes.

## Verification and delivery

Exact-head CI evidence and remote SHA are recorded externally in
`../evidence/worker91-delivery.json`. Pending is not passing; full configured,
Storage/catalog/Query/facts, formatting/boundary and independent ownership/layout
review are still required. The known separate cold failure is not waived.

Thermal telemetry is absent: no local cargo/build/check/test/clippy/fmt/update/
lockgen, Node/typecheck, SQL/EXPLAIN/bench, browser/image or corpus work ran.
Checks run in CI only; stop CPU work at85C or unavailable telemetry. Reads are
bounded to one named file<10MiB,<=150lines/4000tokens. No nested agents,
Main/rebase/force, deployments, livePG, user migration/deletion, credentials,
remotes, permissions or unrelated worktrees were changed. PG90 and S2 are
reserved. Normal task-branch delivery uses the authorized attribution.

This is a narrow Source increment, never full#477 completion. Full#477/#478/
#479/Hub313, PostgreSQL without hiddenSQLite, native families, supported-current-
store migration/catch-up/fenced cutover/qualified rollback and fresh independent
acceptance remain mandatory. Current schema only;#380 must finish before#477
merges to main.
