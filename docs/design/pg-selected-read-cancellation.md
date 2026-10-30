# PostgreSQL selected-read cancellation component

The private PostgreSQL TreeCheckpoint binds the current selected-read request
at capture/restore and on each deferred Summary tool-name or shared-usage read.
The selected-read accessor clones only the cancellation flag and deadline; row
and byte accounting stays in the existing outer Scope. No borrowed thread-local
state enters the driver. A retained checkpoint stores the independent host
cancellation token, so cancellation of its original request does not poison
later valid readers. Nested scopes retain the outer control.

The existing inline Driver cancellation path observes both host and request
flags. Acquire, connection establishment, BEGIN and statements use the earlier
request/pool deadline. Rollback and successful cleanup retain the request
deadline cap and their bounded cleanup grace. Cancellation transport and query
completion stay owned by the synchronous worker and pool lease; tainted
connections are stopped/retired rather than returned to the idle pool. No
detached request watcher, SQLite fallback, public SQL API or credential API is
introduced.

Real parser-based PG tests block the actual Summary tool, copied-usage and
capture queries with table locks. A test connector holds cancellation I/O open
after SQL completion to verify that the worker and pool slot remain accounted
for. Further cases cover a shorter request deadline while acquiring capacity,
pre-cancel, outer/nested scope precedence, later reuse of a deferred checkpoint,
independent host cancellation, retirement and successful replacement reads.
All execution, formatting and regression checks run in CI because local
temperature readings are unavailable. CI outcomes and exact pins are recorded
in the worker105 audit; tests being present does not imply they passed.

This is a private #479 component. Full #477 qualification still requires the
complete IndexStore and all native families, no-SQLite instrumentation, bounded
supported-current-store migration/catch-up, fenced cutover, qualified rollback,
Hub #313, fresh independent review and applicable regression gates. Both current
authored SQLite profiles remain supported; historical Semon schemas are outside
scope. The user's #380 predecessor work must complete before any #477 main merge.
