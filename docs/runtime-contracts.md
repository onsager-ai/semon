# Durable runtime records (#250)

`semon-runtime` is independent of capture, Push, relay, the CLI and provider
SDKs. It defines distinct logical session, workspace, harness thread, provider
runtime, operation, checkpoint and credential-reference identifiers. Credentials
are coordinator-resolved owner/name references; values never belong in records.

One owner/workspace/root-thread binding is immutable for a session. Stable guest
paths and mirror enrollment belong to that binding. Compute and harness states
are separate; desired paused state is intent and does not end the session.
Only explicit desired `ended` intent at an absent, excluded-writer boundary ends
it. A thread receipt must be committed immediately with `bind_thread` before
the coordinator continues. A different thread or runtime cannot be rebound to
the same attempt.

The embedding server owns `SessionStore`. Its aggregate contains the session,
attempts, lifecycle operations and checkpoint references; compare-and-swap
commits all of them atomically outside guest compute. Every checked mutation
increments revision once. Stores verify authenticated ownership, expected old
revision and old epoch, then use `validate_successor` while holding their
transaction lock. Even a deserialized candidate must match exactly one legal
mutation. Creating a record accepts only the initial constructor state.

The coordinator persists a session, then a prepared launch operation and manual
input, before provider work. Provider dispatch records `in_flight` before the
call. Unknown outcomes can become succeeded/failed only following inspection;
they cannot be put back in flight for blind retry. Prepared/in-flight/unknown
operations remain discoverable after server restart or a lost notification.
No manual input is dispatched by these contracts; the durable command inbox and
ambiguous harness delivery machinery remain #254/#252.

A new launch increments execution epoch. An old attempt must first be excluded
by externally observed shutdown or qualified storage exclusion. An expired
relay lease, stale facts or a higher epoch are not that proof. These contracts
record the coordinator's proof; they do not stop an old writer themselves.
The server must authenticate the actor and establish authorization before
choosing the owner context. There are no public launch/dispatch routes yet.

Provider capability checks explicitly reject unsupported memory retention,
filesystem retention, restore and export. Destruction is always unqualified;
there is no provider destroy interface. Checkpoint references have coverage
slots for workspace, originals, harness, thread, commands, path mapping, epoch,
input revision and intended lifecycle, but committing checkpoints or entering
suspension is refused until #253 implements actual verification. Independent
storage/deletion eligibility remains #255. A stored reference alone never
qualifies an action.

The first implementation stores the aggregate as one versioned, owner-scoped
PostgreSQL row in semon-hub. This gives an atomic boundary without a distributed
transaction across record kinds. JSON contains sensitive manual input, so it
must stay in the authorized coordinator storage, not normal trace/model APIs or
logs. Raw recovery file bytes belong in qualified recovery storage. No Push WAL,
replication redesign, or semon-control in-memory inbox is introduced.
