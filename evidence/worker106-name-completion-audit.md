# Source106 Name/member completion increment

Base: bcf3769c8a903b38c2c7b2c44e73ee9ef2fdf2b6. This is the single
approved Source106/model24/controller-own5 attempt, not a retry reset.
Full intake, #477/#478/#479/Hub313 and the #380 main gate remain binding.
This report describes implementation; fresh Manager delivery review is required.

The failing late-member helper called selection before handling an empty reset
continuation. `Stage::batch` commits bounded reset pages while `resetting=1`;
`Stage::select` correctly refuses that epoch until reset completes. The helper
now reacquires a batch after each empty continuation before probing selection.
All old-epoch selection/model/publication refusal assertions remain. Final
unique selection still must match ordinary native preparation and publish.

Member discovery now yields after 32 observations in the production frontier
pager, within the unchanged maximum of 64. Before scheduling any name replay,
the same fenced owner transaction checks the complete requested component's
distinct source count, stopping at 65. The metadata query starts with staged
sources and requested Current owner memberships in the admitted namespace;
no Raw is accessed. Over 64 refuses before a cursor or reset can commit.
The original 65-source unavailable-Raw rollback test remains unchanged.

The new registered domain94 test uses 34 real sanitized native sources: one
initiator and 33 independently published named peers. Name lookup commits a
real unfinished member keyset, with both a nonempty nonterminal cursor and an
unvisited suffix. After dropping/reopening the producer, reset continuations
must preserve those exact cursors. Actual producer batches then complete the
saved keyset and enroll the final member. Completed prefixes remain unchanged;
native event rows remain equal, and the final bounded ambiguity result and
publication match the ordinary producer oracle. Foreign namespace rows remain
byte-identical. No normalized Catalog/member seed or empty test checkpoint is
used. The production query resumes with `source_path > after_key`.

The existing actual 0/256/2048 same/foreign noise VM probe now also measures the
production component-capacity query. The unchanged per-query 4096 VM, growth64,
setup32768, 1s, source64 and 1MiB bounds remain. The added expected capacity row
count is two; the existing hints/candidates/member cardinalities remain intact.

Exact-head Storage, NativeUsage, Rust/strict Clippy, formatting and boundary
results are recorded in the external worker106 delivery JSON. Pending checks
are not passes. The base carries the user's cold FIRSTPAGE 5s amendment only;
foreground Catalog/Query 2s and background 60s are unchanged. Current authored
singleton/composite profiles remain; no historical schema compatibility is added.

Thermal telemetry is absent. No local cargo/build/check/test/clippy/fmt/lockgen,
node/typecheck, browser/screenshot/image, SQL bench or corpus analysis ran.
Checks are CI-only; CPU work stops at >=85 C or unavailable readings. Reads
remain one named file under 10MB and at most 150 lines/4000 tokens per output.
No nested harness/agent, main merge, deployment, publication, live migration,
credential, remote or permission change ran. Public qualification stays zero;
full PG/no-SQLite, all native families, supported-store migration/catch-up,
fenced cutover, qualified rollback and Hub313 remain incomplete.
