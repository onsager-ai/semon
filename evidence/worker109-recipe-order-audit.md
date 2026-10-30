# Source109 native receipt ordering

Base: 30eec97f22b08e1ada140d70434079b8ec99a63f. One bounded approved
Source109/model25/controller-own6 implementation; fresh Manager verdict required.

Ordinary `Builder::relays` sorts unmatched Claude receipts by their unhashed
native source key (`relay:<message id>` or `relay:recv:<native id>:<offset>`).
`Context::capture` previously used a missing outgoing sender endpoint for these
relations: empty path and zero offset, so install/replay sorted by hashed handoff
id. Equal timestamps made that wrong order visible in the virtual peer recipe.

The shared `Builder::orphan_relay_source` now supplies both the ordinary receipt
assembly and staged relation order. Staged receipts have their own phase after
Claude sends and before Codex relays, matching ordinary assembly. The native key
is retained in content-free relation metadata; physical row ids, receipts hashes,
keyset order and insertion order do not define semantic order. Existing paged
replay and relation derivation are reused. The current recipe marker is v5; old
recipes remain unsupported, with no historical-version upgrade path.

The unchanged 34-source durable member-resume oracle remains the primary
regression. A smaller real native-producer test adds three equal-time sources,
keyed/unkeyed receipts, append and reopen, checking full ordered ordinary recipes
before and after append. All old cursor, epoch, no-Raw, capacity and foreign-byte
assertions remain. No SQL/index/namespace/budget bypass was added.

CI results and verified remote revision are in the external delivery JSON.
Pending checks are not passes. Public qualification remains zero; this increment
does not complete #477/#478/#479/Hub313, PG/no-SQLite or migration/catch-up,
fenced cutover and qualified rollback. #380 gates main; no main merge is allowed.
The user first-page amendment is 8s only; foreground 2s/background 60s remain.
Both authored current singleton/composite profiles remain supported as before.

Thermal telemetry is absent. No local cargo/build/check/test/clippy/fmt/lockgen,
node/typecheck, browser/screenshot/image, corpus analysis or SQL benchmark ran.
CI only; stop CPU work at >=85 C or unavailable readings. Reads are one named
file under 10MB, <=150 lines/4000 tokens and <=500 chars per line. No nested
harness/agent, other-worktree edit, credentials/permissions/remote change,
deployment, publication, live migration or cleanup ran.
