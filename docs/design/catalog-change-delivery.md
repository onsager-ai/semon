# Durable catalog change delivery

This is the source-side foundation for a persistent multi-source catalog. It does
not activate a global Viewer list or change source authorization.

Schema 18 records changed catalog keys in the same SQLite transaction as the
metadata publication. Triggers cover complete and partial local publications,
immutable History publications, retirement updates, and deletion. Current row
changes also notify History consumers because History can use current metadata.
Repeated keys are expected and consumers must apply them idempotently.

`session_catalog_changes` is a metadata-only library API. It requires an exact
externally authorized source key and returns at most 256 keys, a durable stream
epoch, source generation, scoped head revision, and continuation revision. Its
read transaction captures all these values coherently. Requests neither decode
native events nor enumerate configured sources. A replaced store has a new
stream epoch and rejects an old continuation.

The keys are hydration hints, not snapshots or native authority. A background
consumer must hydrate each selected key against the returned source generation,
retry a changed generation before committing, and apply absence or retained
lifecycle as a Current-list tombstone. Host policy must authorize publication as
well as reads. The eventual aggregate must additionally bind its snapshots to
the host's durable source-registry revision; this log alone grants no access.

Migration seeds existing Current and History keys once. Query continuation uses
the covering `(read_scope, revision)` index without OFFSET. The current log is
append-only; consumer acknowledgements and safe compaction remain to be added
before a long-running aggregate is advertised as complete.

## Verification

At the source checkpoint, the two focused tests pass: transaction rollback does
not emit changes; scope, updates, deletes, and replaced stream epochs behave as
specified; and the fixed first-key query performs identical SQLite VM work with
20,000 unrelated catalog rows. EXPLAIN confirms the covering index. The exact
test executable contains 524 tests. Formatting and package all-target locked
Clippy pass. The full suite, production notifications, persistent aggregate,
host admission, and browser global-list journey remain unverified for this
checkpoint. This is not evidence of complete #319 acceptance.
