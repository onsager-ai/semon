# Durable catalog change delivery

This is the source-side foundation for a persistent multi-source catalog. It does
not activate a global Viewer list or change source authorization.

Schema 19 records changed catalog keys in the same SQLite transaction as the
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

Migration seeds existing Current and History keys once, including Current keys
that are readable through History before a separate History row exists.
Upgrading schema 18 preserves its stream epoch and existing hints, adds scoped
compaction floors, and appends recovery hints for the Current-to-History fallback
omitted by that schema's bootstrap. Restart does not repeat either migration.
Query continuation uses the covering `(read_scope, revision)` index without OFFSET.

`compact_session_catalog_changes` removes at most 256 acknowledged hints per
explicit call. The host must persist an aggregate batch and its continuation
atomically before acknowledging it, and supply the least acknowledgement of
every consumer of that source/read scope. This library does not register
consumers or schedule compaction. It does not delete session metadata, source
bytes or retained history. Hosts must budget admission and compaction themselves;
the existence of this API alone does not bound an unused append-only log.

Compaction checks the stream epoch under the same write transaction as deletion
and advancement of the durable scoped floor. A replacement epoch or
acknowledgement beyond the head is rejected. The newest hint is retained to keep
the scoped head durable, including an otherwise fully acknowledged stream. Lost
replies and duplicate acknowledgements are safe to repeat. Current compaction
does not remove History hints. Reads below `compacted_through` return
`ScopeChanged`, including a fresh zero cursor after compaction. Consumers must
rebuild from a coherently fenced metadata snapshot rather than treating a gap as
an empty/caught-up stream. Source generation changes preserve the stream epoch;
the existing generation-fenced hydration rule still applies.

## Verification

At the source checkpoint, the two focused tests pass: transaction rollback does
not emit changes; scope, updates, deletes, and replaced stream epochs behave as
specified; and the fixed first-key query performs identical SQLite VM work with
20,000 unrelated catalog rows. EXPLAIN confirms the covering index. The exact
test executable contains 524 tests. Formatting and package all-target locked
Clippy pass. The full suite, production notifications, persistent aggregate,
host admission, and browser global-list journey remain unverified for this
checkpoint. This is not evidence of complete #319 acceptance.

The compaction follow-up adds rollback, bounded batch, scope isolation, duplicate
acknowledgement, stale/replaced epoch, restart and source-generation regressions.
It also refreshes the branch against merged main, preserving qualified partial
Current indexes and Viewer/native identity fixes. Final combined qualification
is recorded on PR #366; earlier counts above describe the original checkpoint.
Aggregate activation, durable host consumer acknowledgements, notification
delivery/recovery and complete discovery journey improvement remain open.
