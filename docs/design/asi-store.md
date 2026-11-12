# ASI store

Status: accepted architecture/spec for #544 (revised 2026-11-09 for the user's
federation decisions D1–D8 and DuckDB analytics). Implementation and validation
remain delivery work. Direction: [ASI direction](asi-direction.md#architecture).
Governance, read channel, cache and authority: [ASI federation](asi-federation.md).
Method: [ASI methodology §1.1](asi-methodology.md#11-storage-schema). The
query surface is specified in [ASI query](asi-query.md) (#545).

## Shape

```text
origin (per namespace)                         operator receiver
native logs + archive                         admitted immutable analytics heads
  → SQLite activity, L0, StepFacts, prices    encrypted, bounded typed-page cache
  → body-free analytics projection ─────────▶ DuckDB / SQL; remote view on demand
  → authorized, audited typed view ─────────▶ Viewer
                                                        │
                                                        └─ Hub: analytics only,
                                                           paths HMAC-tokenized
```

The origin is the default producer and authority for its namespace. It retains
raw custody and computes the SQLite activity projection, L0 classifications,
StepFacts and prices before producing analytics. The receiver imports body-free
facts, including plain typed names and paths, and does not derive activity or
prices for origin-published namespaces. Only an explicitly transferred complete
replica namespace uses the receiver as its authoritative producer. Mixed rules
versions within the current format and permanent evidence loss when an origin and its
raw disappear are accepted. A receiver may hold an explicitly opted-in raw replica;
that is custody, not the default analytics path. The supported mirror and #572
custody contracts remain in force.

A namespace-wide authority transfer to a replica requires complete admitted source
cohorts and preserves unaffected facts and categorical labels. If a machine has
both replicated and pull-only sources, its origin retains authority for the whole
namespace. A cache is never an archive. Every multi-source derivative, including
labels and coverage, must satisfy the intersection of all contributor policies; a
permissive source cannot export another source's contribution or make a fragment
appear complete.

| Data | Location | Rebuildable from | Engine |
| --- | --- | --- | --- |
| Raw | Origin native logs plus `archive/`; optional approved replica | — (custody) | Files |
| Index | Origin `sessions-index.sqlite3`, one per namespace | Available raw | SQLite |
| Analytics | Origin projections and receiver admitted immutable Parquet heads | Origin index when available; imported frozen heads may be sole facts | DuckDB (read) |
| Durable | Author-owned `asi-durable.sqlite3` | — (custody) | SQLite |
| Remote view cache | Receiver encrypted typed pages, bounded and expiring | An authorized online origin | Encrypted cache |

## Identity

Identity stays native. Rows are never keyed by ordinals, generations or row
IDs, so labels survive every rebuild.

The origin adapter derives a session UID from configured machine namespace,
native harness and the exact native session ID. A Catalog key is only a
source-local disambiguator and must not replace that ID during a harness collision;
recorded native parent links use the same identity rule.

The origin adapter exports `parent_method = native` only for resolved native
Catalog parentage (`parent_source` absent). Catalog's observed `ancestry`,
`environment` (`SEMON_PARENT`) and `marker` (`Semon-Parent` in prompt text) links
set `parent_source` only when they establish or replace the parent. Agreement
with an existing native parent leaves native provenance intact; a marker-only
Codex spawn still records `marker`. These observed links are not native lineage.
They also do not establish the workspace-and-time inference described by
`workspace`. The current ASI methods cannot represent
these sources, so the adapter omits their parent UID and uses `none`; it treats
unknown source values the same way. An absent parent also uses `none`.
Only native parentage promotes a `codex-run` to worker; otherwise it remains
solo. A native `subagent` kind independently establishes worker status.

- **Namespace.** Each machine's sources use its configured machine name. A
  receiver binds each push token to the token's name, refuses a token named
  after itself, and reserves `local`.
- **Encoding.** A UID is a sequence of percent-encoded components joined by
  `/`. RFC 3986 unreserved characters stay as they are, and every other byte
  becomes uppercase `%XX`. The adapter and namespace reconciliation use the
  same component encoder owned by `semon-asi`.
- **Keys.**

  | Key | Components |
  | --- | --- |
  | `session_uid` | `{namespace}/{harness}/{native_session_id}` |
  | `turn_uid` | `{session_uid}/t/{id}`, where `id` is the Codex `turn_id`, the Claude initiating record `uuid` (or the step's native `message.id` when no initiating UUID is recorded), the handoff ID, or else a record anchor |
  | `step_uid` | `{session_uid}/s/claude/{message_id}` or `{session_uid}/s/codex/{thread}/{session}/{turn}/{response}` (`ActivityStepId`) |
  | `compaction_uid` | `{session_uid}/c/{id}`, where `id` is the native record ID or else a record anchor |

- **Record anchor.** `@{source}:{n}`, where `source` is the native log's file
  stem (which matches `^[A-Za-z0-9._\-]{1,128}$`) and `n` is the record's
  ordinal within that log. It is independent of paths, keys and how logs
  interleave, and stable while the log's prefix is unchanged. Native IDs are
  preferred. The existing persisted Claude `ActivityStepId` supplies the native
  message ID without changing the Event Index format. Same-stem contributors
  cannot use the fallback to distinguish their records; no physical path or
  byte offset may enter the anchor.

  If a step or compaction has neither a usable native ID nor an unambiguous
  record anchor, the origin input returns `SessionChange::Pending` for that
  whole session generation. Consumers retain prior facts; the input emits no
  `SourceRestored` for that session and still admits other eligible sessions in
  the batch. This is an admission limitation, not a source-missing observation
  or a removal. A later publication with representable evidence can be admitted.
  Invalid persisted data and database errors still fail the read.

- **Repeated compaction identity.** Copied contributors can carry the same
  native compaction ID. Compatible timestamps and associated step UIDs collapse
  to one fact, retaining first occurrence in persisted ordinal order. Missing
  values are unknown and merge with known values in either contributor order.
  Two different known timestamps or step UIDs make the whole session generation
  `Pending`, with the same retention and batch isolation as an unrepresentable
  anchor. Physical compaction rows must still match the header's declared count
  and contiguous ordinals before deduplication; missing rows fail integrity.

## Raw

Raw remains at the origin under native custody. The standalone archiver keeps
an append-only zstd copy of each native log under `archive/`, admitting only the
prefix index has consumed. A rewrite starts a new chain and preserves the old
one; index can read archived sources when native logs are gone. Existing origin
retention is unchanged: indefinite by default, with optional
`archive.retention_days`; native logs are never deleted by this design. This
contract is carried by explicit maintenance, `semon archive prune
--retention-days N [--cache PATH]`, with no configuration file or background
deleter. Omitting the flag deletes nothing and reports indefinite retention.
Age is immutable chain admission time, not file modification time; only chains
strictly older than the window and without an exact chain dependency may retire.
`archive/clock.json` records the highest admission or successful prune time.
Admission clamps its timestamp to that durable high-water mark. Retention fails
closed with an error and deletes nothing when the wall clock is behind the mark;
a successful prune advances the mark only after completing its work. A clock
already behind before the very first admission cannot be detected; that residual
risk is accepted.
The native source binding, index retention markers and seal custody certificates
protect their named chain regardless of age. Custody registers each dependency
before publishing its external marker; interrupted publication may over-retain
bytes. Retiring a marker does not automatically release that custody reference.

Standalone manifests bind the exact chain to the configured native root,
allowlisted relative path, native ID, consumed-prefix ledger and observed Claude
sidecar metadata. Missing-source discovery uses only bindings under the current
configured roots; present native files retain precedence. Rebuilds verify the
chain and consumed prefix, replay the same native parser and preserve the
original ledger instead of assigning a temporary file's physical witness.
Unconsumed suffixes are not archived or promoted to complete source custody.
The current manifest requires admission times and dependency metadata; unsupported
old manifests are refused without converting or deleting their bytes.

An opt-in replica follows supported mirror and #572 custody, including preserving
replaced bytes and redaction provenance. Before acknowledging `replace`, preserve
the logical bytes (plain or read through `sealed`) in an immutable verified chain
`archive/{archive_source_id}/{chain_id}.zst` with atomic manifest publication.
Seal retirement never deletes bytes without verified chain custody. Mirror `Append`
requires `redactor_version`; each chain retains it and its redacted-byte digest.
Replica enrollment records retention explicitly; neither federation nor cache expiry
deletes existing custody. Replica custody does not itself transfer producer authority.
Replica reads still require online origin authorization and audit; no offline grants.

## Index

Index is the authoritative producer's ingest index/activity projection, normally
at the origin, or at the receiver after an explicit complete-namespace replica
transfer. It runs per namespace and supplies:

- **`StepFacts`** (#556, owned by the session activity lane): turn ID, tool
  calls (tool, target path, command tag, ok, exit code, tool time), model time,
  the full `BillingUsage` inputs and usage ownership (exclusive or copied).
  These are facts only; the L0 rules stay with that lane.
- **Change log.** `silver_change(seq, session_key, kind)`, where `kind` is
  `changed` or `removed`. A row is written in the same transaction that makes a
  session's derived state query-ready: activity publication, a `StepFacts`
  change, or a rewrite or removal. Ingest commits that are not yet query-ready
  write nothing. `seq` is monotonic within an index **incarnation**, a random
  ID created whenever the index is created or rebuilt. The analytics writer reads the changes
  and the session rows in one SQLite read transaction. A new incarnation
  makes the analytics writer re-read that namespace in full.
- **Repository identity.** Sender facts gain, per repository root, a
  sanitized `repo_id`, or `unknown` (see [Analytics model](#analytics-model)). This
  changes the supported mirror facts contract directly. Index persists the typed
  identity in `silver_session_state.repo_id` alongside its analytics digest, in the
  same transaction as publication and `silver_change`. It remains available when
  sources disappear; local remotes remain class `path` and are not display
  metadata. D7 permits those typed paths in receiver analytics, not in arbitrary
  display fields. Repository dependencies use the effective native working
  directory, including a subagent sidecar's `worktreePath`.
- **Bodies for FTS5** (#567): `body` and its external-content `body_fts` live only
  in the origin-local index. These rows and FTS postings contain private native
  message/tool text and have the same custody as native logs; they never enter
  analytics or the Hub. Metadata tables remain free of message/tool content.
  Stored UID columns are namespace-relative (beginning
  with `{harness}/{native_session_id}`); search/view prefix the validated
  `analytics-namespace` at read time and accept full UIDs. Namespace creation stays
  exclusively in `analytics materialize`. `body_source` records source ownership
  for transactional append/rewrite cleanup. Event Index schema 48 refuses older
  indexes without mutation; explicit recovery must establish source availability
  and custody before rebuilding. See [ASI query](asi-query.md#search).

## Analytics model

Kimball facts at explicit grains, plus conformed dimensions. Columns are
typed, timestamps are UTC milliseconds, and enums are closed sets. Every
column declares a class: `key`, `enum`, `number`, `time`, `name`, `path` or
`body`. The class drives the Hub export and the query reference. Analytics has no
`body` columns.

| Fact | Grain | Main columns |
| --- | --- | --- |
| `fact_step` | One model invocation | step, turn and session keys; `model`, `repo_key`, `date_key`; billing inputs; `usage_owner` (`exclusive`, `copied`); `context_tokens`, `context_band`; `model_ms`, `tool_ms`; `usd_est`, `price_status`; `activity`, `waste_kind`, `trigger`, `ok` |
| `fact_tool_call` | One tool call | `step_uid`, `ord`, `tool_key`, `target_path` (class `path`), `command_tag`, `ok`, `exit_code`, `tool_ms` |
| `fact_turn` | One turn | `initiator`, `started_at`, `ended_at`, step count |
| `fact_session` | One session (aggregate grain) | Totals by token class over exclusive usage, `usd_est`, `reported_usd`, wall and model time, step and failure counts, `usage_complete` |
| `fact_compaction` | One compaction | `step_uid` (nullable), `ts` |
| `fact_label` | One label assertion | Target UID, field, value, labeller, taxonomy version, confidence, `created_at` (exported from the durable file; no free text) |

| Dimension | Notes |
| --- | --- |
| `dim_machine`, `dim_harness`, `dim_tool`, `dim_date` | Conformed and small |
| `dim_session` | Role, parent, parent method, lane root, repo, branch, status, start and end |
| `dim_repo` | `repo_id` and a display name. `repo_id` is the `origin` remote with credentials, scheme and `.git` removed and the host lowercased, keeping non-default ports (`github.com/onsager-ai/semon`). A local-path or `file:` remote is classed `path`, and no remote gives `unknown`. `repo_path` is the path identity its local key is bound to, identical across checkouts sharing that identity; remote and shared `unknown` keys carry no machine-local path. |
| `dim_model` | Immutable model identity: model, provider. Historical attributes can become SCD2 when they are needed. |
| `dim_price` | SCD2 by price version: provider, model, `rate_kind`, `tier`, `threshold_tokens`, `usd_per_unit`, `valid_from`, `valid_to`, `learned_at` |

A price row's logical key is `(price_version, provider, model, rate_kind, tier,
speed, service_tier, threshold_tokens, unit)`. Repeated observations of the same
rate retain one row: the minimum `learned_at`, minimum `valid_from`, and minimum
non-null `valid_to`, independently. The end remains null only if every observation
is open; an older open observation never reopens a closed interval. This merge is
commutative, associative and idempotent within batches and across publication
cycles. A different `usd_per_unit` for the same key refuses the entire
publication without advancing its checkpoint. Different price versions retain
their separate SCD2 rows.

The SQLite activity projection is the only derivation of steps, L0 labels,
context, StepFacts, cost components and prices. Analytics is a pure projection of
one admitted activity generation and catalog metadata in the same SQLite read
transaction; it does not recalculate them. `fact_step` and `dim_price` retain all
billing inputs and the pricing snapshot, including cache-write durations,
web-search requests, speed and service tier. See the [one-model agreement](https://github.com/onsager-ai/semon/issues/557#issuecomment-6733930340).
The origin may update pricing from retained billing inputs without raw bytes and
atomically refresh all price-dependent summaries; reclassification requires
available sources. Receivers use origin-recorded costs and expose their price
versions and coverage. A future central valuation requires a separately named
contract and must never be silently mixed with recorded costs.

`usd_est` is an estimate under a named price version, never presented as
billed spend. Spend views sum only `usage_owner = exclusive`, so copied Claude
usage is never counted twice. Missing prices are `price_status = unpriced` and
never summed as zero. Joining `fact_step` to `fact_tool_call` repeats step
measures once per call, so the views aggregate tool calls to step grain
before joining. The schema reference states this rule. The semantic layer is a set of versioned DuckDB views:
`cost_by_day`, `step_failure`, `lane_session` and `current_label` (the latest
assertion per target and field, with human adjudication taking precedence).

## Generations, publication and versions

Analytics is published as immutable files, plus a map of which file holds each
session's current rows. This is the merge-on-read pattern used by lakehouse
tables.

- **Writer.** An explicitly invoked local materializer takes `analytics.lock` and
  becomes the authoritative producer store's single writer. Other processes only
  read. Imported receiver heads are admitted publications, not locally rebuilt indexes.
  The local `semon-sessions` writer service is an explicit lifecycle primitive.
  `semon analytics materialize` is its only enablement surface: Viewer, MCP,
  push and receive do not start it. On the first materialize, it creates a
  random `origin-` namespace in `<index parent>/analytics-namespace`; later
  materializations reuse that value. If the state directory is deleted, the
  next materialize has new session UIDs and no automatic reconciliation.
  Rename or transfer is a later explicit operator workflow. The writer opens a
  separate read-only index connection per cycle, and its store-root helper uses
  `<index parent>/analytics-store`, never a path inside the SQLite index file.
  A present store with a missing namespace file refuses instead of silently
  forking those identities.
  The CLI checks namespace validity and missing-file/store ambiguity read-only
  before opening the store, so refusal preserves crash leftovers. It validates
  an existing index read-only, then acquires writer ownership before recovery,
  namespace creation or writable refresh, and drains stale rules through the
  native Catalog/activity producer's bounded repair rounds before publishing,
  with strict opening and no recovery fallback. Viewer only observes that
  qualification; it does not start origin refresh or replacement publication.
  Namespace loading refuses symlinks and non-regular files and reads at most the
  expected length plus one. The index path must be the user's own trusted state
  file. Analytics input opens refuse symlinks with SQLite NOFOLLOW; regular-file
  replacement by the same user is outside this boundary, since that user can
  already edit the state files. Each waiter owns a request generation and outcome;
  success requires that generation's complete input publication and final
  compaction, never a partial chunk, intermediate Idle or manifest reload.
  SIGINT/SIGTERM cancels a CLI watch wait, then joins the writer and is re-raised.
  Watch exits on refresh/publication failure.
  A busy lock is observable as a no-writer state; unsupported input latches a refusal
  without changing the index or a checkpoint, until a later notification retries.
  Store refusals also latch until a new notification; each refusal consumes only
  the initiating notification generation retained across its entire recovery
  chain, preserving notifications received during backoff or a later chunk.
  A new chain owns generations `(consumed, captured]`, capturing the earliest
  unsettled waiter above `consumed`; only notification-only work coalesces to
  the latest notification. Two waiters queued before selection therefore have
  separate chains, failure budgets and successful publications as well.
  Successful chunks advance the scheduling watermark `consumed` through the
  captured generation while deferring request settlement until final compaction.
  This watermark does not end the chain or reset its counters.
  Notification counters saturate at u64::MAX, an unreachable process-lifetime
  bound of more than 1.8e19 requests; settlement at exhaustion is unspecified.
- **Local custody.** The generated Parquet can contain typed local-path
  fields. It remains local-only. `semon analytics export DESTINATION` is an
  explicit, verified local custody bundle rather than materialization: it copies
  the namespace, manifest, live map and manifest-named immutable Parquet files,
  records SHA-256 size/digest evidence in an `export.json` receipt written last,
  and verifies the completed bundle. It takes the existing writer lock shared
  and non-blocking without creating it or running recovery, so a writer-held
  lock refuses rather than producing a torn snapshot. It refuses unsafe or
  symlinked destinations, a destination inside the source store, and all
  missing/foreign/unsupported/ambiguous source state. Export and reset prove
  local ownership by requiring every live UID to carry the confirmed namespace.
  `semon analytics reset` is never automatic. It requires exact namespace
  confirmation, exclusive non-blocking writer ownership and exactly one verified
  current export or an explicit `--without-export` waiver for possible loss of
  imported/frozen facts. A verified export must remain outside the store and
  match the current manifest, live map, namespace and referenced files by size
  and SHA-256 under that lock. Reset deletes the manifest before other
  analytics-store entries but retains the lock inode, never touches native logs,
  archive or Event Index, and retains the namespace unless `--new-namespace`
  removes it while the writer lock is held. A retained namespace makes the next
  materialization preserve UIDs; removing it deliberately creates fresh identities
  without reconciliation.
- **Cycle.** The caller selects an interval (2 s by default) and an input
  budget. `publish_cycle` publishes one bounded chunk as one `Update` and
  generation, then returns its generation and completion flag. The caller may
  drain further chunks immediately, yield between them, or stop and resume at
  the next tick; 2 s is neither a transaction deadline nor a promise to finish a
  cold inventory in one cycle. The existing local writer selects its budget via
  `AnalyticsWriterConfig.input_budget` and schedules each incomplete chunk at the
  next interval without requiring another external notification. The explicit CLI
  accepts positive `--batch-sessions` and `--batch-steps` limits up to
  `min(usize::MAX, i64::MAX)`, validated before opening or creating state. One-shot
  materialization uses zero spacing between chunks and exits after the first
  fresh drain and its final bounded compaction; watch uses a minimum spacing
  of 2 s between chunk attempts. If a new chain inherits a persisted unfinished
  replay endpoint, it first finishes that replay and compaction, then runs one
  fresh drain whose endpoint is captured after the request registered. Completing
  the inherited endpoint alone never settles the new request. The adapter owns
  checkpoint inspection; semon-asi still treats checkpoints as opaque.
  A continuously advancing source cannot extend that drain's captured replay
  endpoint. Later writes require another request. Cancellation stops the wait;
  joining stops future chunks, preserving the last published checkpoint.
  Recompute touched sessions **whole**: a following step can change an earlier
  step's duration, and a late record can change older rows. No session is split
  to meet a budget; CI measures the cost of the exceptional oversize session.
  - It writes one new file per table, `analytics/{table}/g{generation}.parquet`,
    holding the full rows of those sessions with a `_gen` column. Dimensions
    are rewritten whole, as `analytics/dim_x/g{generation}.parquet`.
    Their next state comes only from the currently named dimension files: replace
    touched or removed `dim_session` rows, then merge the newly projected
    dimensions. Fact files are never scanned to retain dimensions. Unreferenced
    dimension entries remain; bounded pruning is deferred maintenance.
  - It writes the live map, `session_uid → _gen` with removed sessions
    dropped, as a typed Parquet file `analytics/live/g{generation}.parquet`, so the
    manifest stays small at any number of sessions.
  - It fsyncs those files, then writes `manifest.json`: the generation, file
    lists with their compaction level, versions, each namespace's opaque
    resumable input checkpoint, the durable revision checkpoint, and the live-map
    file. It fsyncs the manifest, renames it into place, and fsyncs the
    directory. A crash leaves the previous generation.
  - The local service coalesces catalog-publication notifications. Operational
    classification covers direct store I/O, Parquet `External` I/O, and SQLite
    busy/locked/I/O/full/cannot-open errors. These retry with bounded exponential
    backoff, with at most three operational failures per active chain before
    visible refusal carrying the last error (1s/2s backoffs plus publication
    spacing). One publication chain includes any required Open, Reload and
    compaction recovery operations. Success of a barrier, chunk or intermediate
    compaction preserves its attempt and failure counts. The bound is cumulative
    over that chain, not reset by a successful operation within it.
    Periodic retirement uses an independent maintenance chain and budget. It
    suspends the publication Chain whole, then restores it whole on success;
    maintenance failures neither spend nor reset its publication counters. Thus
    retirement retries can add failures to the overall request wait. A terminal
    retirement refusal also terminates the suspended request explicitly.
    All identity, kind, attempt/failure counters, endpoint origin, operation and
    completion state live in a Chain record. Success after final fresh-drain
    compaction, terminal refusal, NoWriter, owning-waiter cancellation and shutdown
    drop the appropriate record; the next chain starts at 0/0. NoWriter retains
    only a 30 s scheduling cooldown. A refused/cancelled operation can retain a
    required recovery barrier, but no request identity or counters.
    Cancellation retains only a pending manifest reload after a store error;
    clean pending work leaves retirement enabled without another notification.
    Parquet page-read I/O can surface as an Arrow error after losing its typed
    I/O source; it latches Refused like structural errors. A store error requires
    manifest reload before its operation resumes, since a rename may already be
    visible. Reload success preserves the chain's counters. Every publication
    attempt advances the minimum-spacing deadline, including a rename-visible
    error. Cancellation during I/O lets that operation finish, then drops the
    owning chain before selecting another operation; shutdown schedules no work.
    It uses the activity rules target and current pricing-snapshot fingerprint
    as writer targets; these do not assert that untouched rows have uniformly
    refreshed versions.

The writer's pure transition table is the authority for chain ownership:

| Event | Active/suspended record | Outcome |
| --- | --- | --- |
| Capture | Create earliest-waiter chain at 0/0; an active chain keeps its capture | No settlement |
| Open or Reload success | Same publication record and counters, next operation | No settlement |
| Chunk success | Same record, record completion, select compaction | Advance `consumed`, defer settlement |
| Intermediate compaction success | Same record, next chunk | No settlement |
| Inherited drain compaction success | Same record, require a fresh drain | No settlement |
| Final fresh drain compaction success | Drop publication record | Settle its captured generation Idle |
| Operational failure | Increment active record only; retry at 1s/2s | Third failure drops record and refuses its generation |
| Structural refusal | Drop active and any suspended record | Refuse captured generation |
| NoWriter | Drop record; retain 30 s cooldown only | Settle captured generation NoWriter |
| Retirement interrupt | Suspend whole publication record; create maintenance record at 0/0 | No settlement |
| Retirement success | Drop maintenance record; restore whole suspended record | No settlement |
| Terminal retirement refusal | Drop maintenance and suspended publication records | Refuse their captured generation |
| Owning waiter cancellation | Drop active and suspended records; retain needed recovery barrier | Withdraw its demand |
| Shutdown | Drop active and suspended records | Stop without scheduling another operation |
| Later request arrival | Active record unchanged | Later generation remains pending |

- **Local publication provenance (#593 slice 1).** Each touched session supplies
  exactly one typed custody record in immutable `provenance/g{generation}.parquet`,
  keyed by `(session_uid, _gen)`. The manifest lists its size, row count, decoded
  size and generation range alongside facts. It is written and synced before the
  same atomic manifest rename and uses the same live map. Keeping the cohort once
  per session avoids duplicating source vectors in every fact row and lets the
  existing bounded compactor filter dead records while preserving live `_gen`.
  Compaction and delayed retirement use the ordinary immutable-file contract;
  recovery discards uncommitted provenance files just like fact files. Before
  opening or reloading a writer snapshot, validate every referenced provenance
  file's size and row count and require exactly one matching record for every
  live session generation. Validation streams only UID/generation columns, with
  one batch plus O(live sessions) keys in memory; work is O(referenced provenance
  rows), including dead rows awaiting compaction, without decoding source vectors.
  Corruption refuses with preserve/export guidance before recovery deletes files
  or the writer adopts the snapshot. Historical dead records remain legitimate
  immutable history until compaction; they never satisfy a live generation binding.
  The record retains the complete admitted source generation and ordered cohort
  (`native_id`, local path, consumed offset and prefix SHA-256), activity generation,
  producer package and parser versions, index incarnation when known, recorded
  transform/rules/price/taxonomy versions and coverage, and the injected-clock
  analytics projection time. SQLite does not record a computation timestamp, so
  that field stays unknown; event timestamps are never substituted. Index
  incarnation identifies input continuity, not an authenticated publisher.
  `Pending`, source loss and restoration never rewrite the record from today's
  Catalog. A complete admitted replacement changes facts and provenance together;
  explicit session removal makes both dead. Restart retains the same binding.
  This artifact contains local paths and is local-only: it is absent from
  `schema::TABLES`, generated SQL schema reference, SQL AST allowlist and the query
  worker's allowed file paths. It has no analytics egress serializer. Governed
  federation uses the distinct `semon-asi::egress` receiver projection (#593 slice
  2), described in [the governance API](asi-governance-api.md#pinned-receiver-projection-593-slice-2).
  It pins and decodes a local snapshot under the shared lock, resolves every retained
  contributor through explicitly loaded local policy configuration, and builds fresh
  policy-qualified rows with persistent random opaque source/evidence handles.
  Receiver plain name/path fields obey D7 and all contributor policies. Coverage
  permission is required for session policy-version vectors; denied sessions and
  unreferenced dimension values never enter the output. The handle database lives
  at `<state dir>/analytics-egress-handles.sqlite`, beside the namespace file and
  outside `analytics-store/`. It binds only to the namespace, not a filesystem
  path. Reset preserves it while keeping the namespace; `--new-namespace` removes
  both files under the writer lock, retiring the old mappings. Custody export
  bundles exclude this private operator state. No network, object protocol, claims/heads, Hub
  profile, L1 or automatic export is delivered by this slice.
  Explicit operator custody export includes and hashes it, verifies its one-to-one
  live binding, and compares it before guarded reset. Reset removes it with the
  analytics store; coverage continues to read only `fact_session`.
  ASI schema version 4 requires this contract. Older stores are refused with the
  existing export/reset guidance, without migration or dual readers. Export and
  reset also require the current schema: preserve an unsupported store intact and
  use its matching older binary for explicit custody export/reset before rebuilding
  from available admitted sources. Never silently discard frozen data.
- **Reading.** Each fact view keeps only live rows:
  `… semi join live on (session_uid, _gen)`.
- **Compaction** is tiered. Cycle files are level 0. When a table has more
  than 32 level-0 files, the writer merges them into level-1 files of at most
  128 MB, sorted by namespace and start time so that Parquet row-group
  statistics prune time ranges. A level-1 file is rewritten only when more
  than 30% of its rows are dead, together with its dead-row neighbours. Clean
  level-1 files are never rewritten, so work grows with changes and not with
  table size, and a second compaction run straight after the first is a no-op.
  Compaction publishes as just another generation.
  The local service makes at most one bounded compaction attempt after a
  successful publish.
- **Retirement.** A file leaves the store 10 minutes after the first
  published generation that no longer names it. Queries hold a snapshot for at
  most their deadline plus fetch time, well under that. The local service only
  considers retirement on a slow maintenance cadence and never during shutdown.
  Due retirement can precede a publication waiting for its interval, preserving
  publication demand and its retry count. Reload, refusal and retry deadlines
  remain barriers; waiting uses the earliest eligible deadline.
- **Full refresh.** The same pure projection rebuilds origin analytics from an
  available admitted index. CI compares incremental and full-refresh live rows.
  Imported frozen heads may be the only surviving facts and are not assumed
  rebuildable from a receiver index.
- **Versions and missing sources.** Transform changes reproject admitted index
  facts without deriving labels or prices again. Rules refresh of untouched
  sessions is origin derivation work: the native Catalog producer advances stale
  rules in bounded rounds (64 sources per qualification), durably as ordinary
  source invalidations. Price refresh is separate origin work in the reprice
  worker, bounded to 128 owners and 4,096 facts. Stale activity is `Pending`
  to the writer, so prior facts remain live until a re-admitted replacement is
  published. Manifest `Versions.rules` is the writer target, not a claim about
  retained rows, and there is no uniform-refresh claim. Origin rules/pricing
  changes publish the resulting admitted activity generation; receivers retain
  each origin's recorded versions and report coverage. A missing source retains
  its complete facts and is marked `source_missing`. In particular, the
  multi-source gap #582
  must not replace a complete generation with a fragment. Re-derive at the origin
  when raw returns. A format mismatch refuses exactly; preserve or export frozen
  facts and custody, with no automatic reset or multi-version exception.
  A missing activity row or a stored witness that no longer matches the current
  Catalog source cohort is not retried by rules repair; source change re-derives
  it, while unsupported incomplete state stays Pending with prior facts retained.
- **Namespace reconciliation.** Only the final inventory chunk carries
  `ReplaceNamespace { session_uids }`. This set contains the complete current
  namespace inventory, independently of the sessions admitted in that chunk.
  Every already-live session UID with that exact encoded namespace component
  followed by `/` absent from the set is marked `source_missing`, retaining its
  rows and recorded versions; other namespaces are untouched. Partial inventory
  chunks never infer absence. A later `SourceRestored` clears the marker.
- **Federated publication.** #555's immutable objects, `expected_parent` and
  namespace claim epochs publish complete admitted heads. Receiver imports carry
  per-origin heads and source, activity and transcript generations separately;
  they do not resurrect the old #561 rows wire. Publication and namespace transfer
  preserve the prior complete head until an eligible replacement is committed.
- **Partial-source retention.** If a current multi-source session loses any
  `(path, native_id)` identity from its last complete activity witness, the index
  retains that complete activity generation, source witness and versions. It
  exposes `source_missing` independently from current Catalog source membership
  and emits `changed` in the same publication transaction. It never publishes a
  fragment from the surviving sources. An exact return to the retained source
  generation reuses its complete activity; a changed generation clears the
  missing state only after a complete activity publication contains every
  previously witnessed identity and stores the full source vector.

### Resumable admitted input

`AnalyticsInput::read_admitted(checkpoint, budget)` returns one `AdmittedBatch`:
namespace, opaque next checkpoint, ordered whole-session changes, reconciliation,
and `complete`. `InputBudget` limits selected inventory sessions or incremental
journal entries, and admitted steps; defaults are 64 sessions/entries and 8,192
steps. Both limits must be positive. Multiple entries for one session in an
incremental journal page collapse to its last entry, ordered by that sequence. A first session exceeding
the step limit is returned whole, alone; otherwise the next session is deferred
without advancing past its entry. `Pending`, `SourceMissing`, `SourceRestored`
and `Removed` retain their existing admission and custody meanings.

For a cold read or changed incarnation the SQLite adapter captures the current
maximum journal sequence **S** and maximum current inventory key **K**. It pages
current Catalog union retained activity keys in binary key order, through K,
using an exclusive last-key cursor. Each page reads the current complete evidence
for its selected sessions in that page's transaction. Metadata-only Catalog rows
are visited as `Pending`, even when they have never had admitted activity and
therefore have no silver journal entry. No page allocates all Catalog keys or all
changes since a checkpoint. A disappearing key need not be visited: only the
final page queries the complete current UID set (current Catalog union retained
activity identities). That page emits `ReplaceNamespace` and changes the
checkpoint to replay after S, with endpoint **E** equal to the journal maximum
in that final inventory transaction. The UID set is independent of the key bound
K: it includes identities first added while inventory was running.

Chunks use separate read transactions. Every changed identity after S through E
is replayed as incremental whole-session replacement or an explicit `Removed`; this
includes new keys, late records, availability changes and removals during the
inventory. `complete` is true when the returned cursor reaches E. Thus the final
inventory chunk can carry reconciliation while still reporting incomplete,
requiring bounded replay from S through E. A new incremental drain captures E
in its first read; intermediate replay checkpoints retain both `after` and
`through` so separate transactions, retries and process restarts keep the same
endpoint. The completed checkpoint retains only `after=E`; the next request
captures a new endpoint. Writes after E belong to that next drain. Whole-session
readers still use current evidence and can observe a newer replacement early;
this is not a historical SQLite snapshot at E.

Correctness follows per identity: one unchanged since S is visited by inventory
and retains exactly its final admitted evidence; one changed after S is visited
again by replay, whose last observation supplies its complete current state or
removal. A pending generation retains the prior admitted rows in both paths.
Final UID reconciliation covers all current identities, including those first
added after S, so it cannot mark a retained current identity absent merely because
its replay is still pending. If source writes have quiesced by endpoint capture,
once replay reports complete, live facts and their referenced dimensions equal
a one-shot read at that final transaction, starting from the same retained facts and applying the same explicit removals. Physical
generation numbers and unreferenced retained dimensions can differ. The oracle
must use the same frozen evidence: an empty store cannot recreate an admitted
generation observed by an earlier chunk if that generation later becomes
unavailable or `Pending`. Retaining that evidence is the existing custody rule.
With writes continuing beyond E, sessions already passed can change again;
the request does not claim a one-shot snapshot of those later writes. The next
drain replays them. A permanently advancing journal therefore cannot keep a
request open by moving its endpoint. Each call remains budgeted, and finite
replay through E terminates without discarding the later journal suffix.

The adapter strictly validates the checkpoint format, namespace binding,
incarnation syntax, nonnegative sequences, ordered replay endpoints, inventory
key bounds and actual journal/key anchors before using a same-incarnation cursor. Inventory key anchors
may outlive current membership through retained Catalog or native identity rows;
a removal does not make a valid cursor skip or fail. Unknown fields,
invalid phase combinations, out-of-range or nonexistent anchors are
`InvalidQuery`, never a successful empty read. A valid checkpoint from a different
incarnation starts a new inventory from zero; it never completes the abandoned
incarnation's reconciliation. The checkpoint is opaque to `semon-asi` and is
committed only with the chunk's manifest. Error, stop or restart resumes from
that committed cursor; a failed publication re-reads the same range. No partial
inventory can infer source absence. Explicit source-availability observations
remain effective.

Checkpoints are writer-owned manifest state, not authenticated claims. Validation
rejects malformed or inconsistent cursors, including sequences ahead of the
current journal maximum; it cannot distinguish an edited, otherwise valid cursor
from one the writer produced. Same-user tampering with that manifest, or with the
configured namespace/index binding, is outside this trust boundary.

Input memory is bounded by the selected key/journal page and step budget plus one
whole oversize session (including its evidence/projection). The final UID set
requires O(namespace sessions) strings, not StepFacts. Existing store live maps
and complete dimension merging remain O(store sessions/dimensions); this change
does not bound those publication structures or the size of a single session.
Inventory traverses Catalog/activity keys, not journal history. The silver journal
is append-only within an incarnation: production never deletes or rewrites its
rows. Any pruning or resequencing MUST change the incarnation, restarting
inventory rather than silently skipping changes. Sequence numbers may have gaps;
replay does not require contiguity. Native identity/Catalog anchors
are retained on ordinary removal. Explicit operator purges may refuse an affected
checkpoint; this read-only adapter never recreates deleted data or resets custody.


## Receiver publication objects (#593 slice 3)

`semon_asi::objects` builds a fresh `PublicationObject` only from a governed
slice-2 `Projection`. Format 1 is canonical, uncompressed JSON: a closed manifest
and the strict `receiver_plain_v1` session payload. This avoids admitting source
Parquet bytes, metadata, codecs or row groups. Compression and Parquet are refused,
so stored/decompressed payload lengths must both equal the canonical payload size.
The manifest binds schema version, profile, namespace, encoding, session count and
per-table row counts. SHA-256 and exact length cover the completed whole envelope,
including the manifest. There is no self-referential digest field. This updates
slice 2's one current exported ID contract: `Projection` retains its namespace,
evidence IDs change from `opaque/receiver/...` to `<hex namespace>/receiver/...`,
and source handles become `<hex namespace>/source/<random hex>`. No old-shape reader
is retained; projection and admission tests use the same current shape. Struct field
order, sorted maps and projection iteration order make the same projection produce
identical bytes. Random handles must already exist; regenerated handle registries
represent different projections. Unsupported schema/format/profile refuses; no
compatibility reader is provided. Hub token profiles are explicitly unsupported.

The Linux store accepts a host-authorized structured scope `(tenant, destination,
publisher, namespace)`. Components are 1–128 ASCII letters, digits, `-` or `_`;
only destination `receiver` is supported. A SHA-256 of the serialized tuple names
the scope directory, preventing delimiter collisions and user-controlled paths.
The trusted host must authorize that tuple: a digest or a caller-created Scope
alone grants no permission. Every lookup, deduplication and token read is confined
to that scope. No global digest inventory exists.

The operator-provided root must already exist. Descriptor-relative `openat` with
`O_NOFOLLOW`, directory-only traversal, exclusive file creation and atomic
`renameat2(RENAME_NOREPLACE)` prevent traversal, symlink following and overwrites.
The store owns `asi-objects-v1/<scope digest>/`; an advisory exclusive lock permits
one store instance per scope, with one reserved incomplete `stage`. Upload verifies
exact digest/length, fsyncs bytes and directory, and remains invisible. Validation
success installs by atomic no-overwrite rename and directory fsync. Any refused
validation, dead worker or expired deadline discards that call's stage immediately;
an explicit `discard(StagedObject)` cancels an unconsumed upload. Each stage token keeps an open file descriptor, pinning its inode even after
restart unlinks the path; comparison with that descriptor prevents stale tokens
from deleting replacement stages, including when identical bytes are re-uploaded. Identical
re-upload compares exact bytes; differing bytes never replace an object. Restart
under the lock removes only the reserved regular incomplete stage. Unknown entries
and symlinks refuse and are preserved; objects and lock files are never deleted.
Same-user/root malicious modification is outside filesystem custody guarantees;
read-time digest checks still detect byte changes. Linux `/proc/self/fd` is required
for bounded scope inventory. Other platforms do not expose this store API.

Limits are 8 MiB per object, 20,000 rows, at most 1,024 policy contributors per
session, and the host-supplied per-scope byte quota (at least 8 MiB). The quota counts
installed and incomplete staged bytes, including the temporary duplicate during
re-upload. Scope inventory is capped at 10,002 entries (10,000 installed objects plus
stage and lock); aggregate tenant concurrency is a host admission responsibility. The store keeps its scope lock during validation.

The [internal object worker](#internal-worker-entries) consumes a bounded namespace line and exact
object bytes through stdin, emits one fixed success/failure byte and no diagnostic
values. It has a cleared environment, private process group, parent-death signal,
256 MiB address-space ceiling, two CPU seconds, 16 file descriptors, zero child
processes (`RLIMIT_NPROC=0`), `PR_SET_NO_NEW_PRIVS` and a three-second caller deadline.
Limits apply before protocol parsing; environment clearing and no-new-privileges
also apply at launch. `RLIMIT_NPROC` is a per-real-user limit and root/capability
exemptions remain an operator service-account concern.
The parent kills/reaps the worker group on completion or failure. The parser admits
only canonical JSON (including duplicate-key refusal), closed envelopes, known
schema columns/types/enums, receiver governance name/path/key checks and required
fields. Body fields, unknown metadata, `fact_label`, `dim_price`, foreign namespace
UIDs/source handles, unsupported encodings and inconsistent counts/sizes refuse.
Serde's recursion limit bounds nesting. No decompressor or Parquet parser runs.
Authentication and truth of publisher-supplied policy/version assertions are not
established by structural validation.

Only successful isolated validation returns the private-field `ValidatedObject`
token. Lookup after restart revalidates before returning a token. Failed or pending
stages cannot be read through the object API. The token is input for future head
commit work; it activates nothing itself. Claims, epochs, expected-parent commits
and complete heads (4b), enrollment/credentials (4a), transport/reconciliation (5),
Hub tokenization and L1 taxonomy are deferred. There is no operator-facing command,
configuration, listener or network route in this slice; the worker is internal.

### Internal worker entries

These hidden early-dispatch entries are re-exec protocols for trusted local
callers, not operator commands. They are absent from usage/help and README CLI
inventories. Do not combine the object validator with the query engine.

| Internal entry | Protocol / owner |
| --- | --- |
| `asi query-worker` | Bounded JSON request/result, SQL engine; [query protocol](asi-query.md#engine-and-worker-protocol). Requires `asi-query`. |
| `asi body-worker` | Bounded body request/result with query admission; [body surface](asi-query.md). Requires `asi-query`. |
| `asi object-worker` | Bounded namespace line plus canonical object, fixed one-byte result; object admission above. TTY stdin refuses before parsing with `internal worker; not a user command`. |

## Durable state

`asi-durable.sqlite3` holds `label`, `adjudication`, `link`, `lane`,
`work_item`, `change`, `outcome`, `intervention` and `assignment`
(methodology §1.1), keyed by UIDs.

- **Writes.** WAL, `synchronous=FULL`, and a single author-owned writer through
  `semon-asi`. Label assertions and adjudications are insert-only, enforced by
  triggers; categorical exports retain authorship and contributor policies.
- **Rebuilds** never open it for writing. A format mismatch refuses; preserve
  or export the durable state without automatic reset.
- **Body columns** (`label.mechanism_text`, `adjudication.reason`,
  `lane.title` and the intervention text) are classed `body`. They never reach
  analytics or Hub, and are readable only through `view` ([ASI query](asi-query.md)).
- **Export.** Every write increments a `revision` counter in the same
  transaction. The analytics manifest checkpoints the last exported revision, so a
  crash can't lose a label. Each cycle exports the label and adjudication rows
  above the checkpoint (categorical columns only) to `fact_label` generation
  files. Labels are append-only, so `fact_label` isn't filtered by the session
  live map, and labels on lanes or work items need no session at all.
  `semon asi backup PATH [--cache PATH]` copies the file with the SQLite online
  backup API and refuses an existing file or symlink destination. Its parent must
  exist. Backup writes inside a private sibling staging directory (0700), sets the
  finished file to 0600, fsyncs it, and publishes with an atomic no-overwrite hard
  link. Staging is removed on success or failure.
- **Authoring.** `semon asi add TABLE --id ID --values FILE|- [--cache PATH]`
  inserts one row in any of the nine durable tables. `--values` reads one JSON
  object from a file or stdin (`-`), with column names as keys and string, number
  or null values. Authoring values are limited to 256 KiB: CLI input bytes and
  the serialized MCP `values` object are checked before writing; oversized input
  returns a fixed error without supplied text. Arrays are JSON-encoded strings, integers are signed 64-bit,
  and timestamps are UTC milliseconds. The Writer validates columns, types,
  categorical shapes, UIDs and constraints; callers cannot supply `id` or
  `revision` inside values. Reusing an ID refuses rather than updating a row.
  Success prints only a JSON object containing `id` and `revision`; errors never
  echo supplied values. Body text is accepted through the same file/stdin object
  and remains author-local.
  With `asi-query`, the local stdio MCP extension offers `asi_add` with the same
  `table`, `id`, and `values` object. Its destination is fixed by the MCP process's
  configured cache; tool arguments cannot select paths. Both transports acquire
  the same analytics author lock and refuse immediately when it is held. MCP
  stdio runs as the local author; no Viewer write route or receiver authoring
  endpoint is provided. Analytics materialization remains an explicit next step.
- **Row identity.** Every durable table has an author-assigned `id TEXT PRIMARY KEY`;
  IDs are nonempty single UID components. Focused view UIDs are `{table}/{id}`.
  `link` additionally has a unique `(from_type, from_id, change_id, method)`,
  `outcome` a unique `change_id`, and `assignment` a unique
  `(intervention_id, unit_type, unit_id)`.
- **Adjudications.** Store required `target_type`, `target_uid`, `field`,
  `taxonomy_version` and UTC-millisecond `created_at` alongside the methodology
  fields. A nonempty `label_ids` list must reference existing labels that all
  agree with those target/field/taxonomy columns. Export one assertion with
  `label_id = adjudication/{id}`, `labeller = human:<adjudicator>`, `value =
  final_value`, null confidence and its own revision/time. Label assertion IDs
  export as `label/{id}`. The existing `current_label` human-first, revision-first
  ordering applies; analytics columns do not change.
- **Classes.** The single durable column table in `semon-asi::durable` declares
  every semantic column. Body columns are exactly `label.mechanism_text`,
  `adjudication.reason`, `lane.title`, `intervention.hypothesis` and
  `intervention.guardrails`. Intervention `lever` and `primary_metric` are
  categorical and are not exported in this slice. IDs, references, arrays and
  timestamps are internal; the remaining categorical values are declared in code.
  Every row also has an internal writing revision. Arrays use JSON text.
  Categorical text uses a shape guard: nonempty, at most 128 bytes, and only
  `[A-Za-z0-9._:/+@=-]`. This is not a vocabulary check; closed taxonomy membership
  belongs to the labelling layer. Prompts/sentences belong in body columns, which
  remain unrestricted. Labelers must have `rule:`, `model:` or `human:` plus a
  nonempty suffix. UID-shaped values instead allow canonical unreserved component
  bytes (`A-Za-z0-9-._~`), valid `%XX` escapes and record-anchor `@`/`:` forms.
  Each component is nonempty and at most 128 bytes; composite UIDs allow `/`
  separators and at most 1024 bytes total. Row IDs and referenced label IDs are
  single components; evidence step IDs and target UIDs may be composite.
- **Format/refusal.** The SQLite header carries `application_id = 0x53414455`
  (`SADU`) and `user_version = 1`. Before any SQLite connection, a read-only,
  no-follow regular-file header read checks the SQLite magic and both stamps.
  Existing empty, foreign and mismatched files refuse without changing bytes or
  creating SQLite sidecars. Once open, required tables and insert-only triggers
  are also checked. A valid WAL-mode file opens SQLite read-only for readers;
  SQLite may create `-wal`/`-shm` sidecars. A read-only filesystem without existing
  sidecars cannot be read; the single-owner state directory must be writable.
  Readers do not use `immutable=1`.
  First creation initializes schema, triggers and both stamps in a private sibling
  staging directory under the author lock. After commit, WAL checkpoint/close and
  file/directory fsync, an atomic no-overwrite hard link publishes the final file.
  A publication loser validates and opens the winner. Stale uniquely named staging
  directories are left alone and never block later creation. An existing unstamped
  final file still refuses; it is never treated as a new database.
- **Ownership/API.** The Rust writer takes an explicit table, author-assigned ID
  and typed values. It shares the analytics author lock. Export, rebuild, backup
  and focused view use a separate read-only reader; version 1 mismatches refuse
  before a writable SQLite open. Durable focused views require exactly one
  admitted origin namespace and return body fields framed as `untrusted_text`.

## Hub export (phase 3)

Hub holds analytics and categorical L1 labels only; it runs no L1 inference and
holds no bodies, body proxy or body cache. **PM interpretation of D7, open to user
correction:** plain typed names/paths apply origin→operator receiver; the settled
Hub boundary still HMAC-tokenizes all typed paths. This does not authorize arbitrary
free text in labels or other fields: free text remains body data. Hub holds the
same layout per tenant, with one independent head per
**publisher** (a store that exports namespaces). Hub keeps a namespace table
(`namespace → active publisher, epoch`), and the tenant's views read each
namespace's rows (facts, namespace-scoped dimensions and labels) only from
that namespace's active publisher head. Dimension tables use stable natural
keys (model name, `repo_id`, tool name, date, `session_uid`) and no surrogate
keys, so the union composes without renumbering, and duplicate rows across
publishers collapse in the views. Export is coherent and idempotent:

1. **Bind.** `semon hub claim --namespace M` records this store as M's
   pending publisher with a new epoch. Hub accepts generations for M only from
   that publisher and epoch, so a transfer is an explicit operator action and
   stale publishers are refused. The previous publisher's rows for M stay
   visible until the new publisher's first complete commit containing M. That
   commit switches M's active publisher in the same transaction, so exactly
   one copy of M is visible before and after the switch.
2. **Upload.** For each new generation, the store writes fresh export files:
   `path` columns become tokens, and only approved Parquet metadata is kept.
   This is the Hub export boundary, not origin-to-receiver tokenization.
   It uploads each one with `PUT /v1/asi/objects/{digest}`. Uploads are
   immutable and content-addressed, so retries are safe.
3. **Commit.** `POST /v1/asi/commit` sends the generation: added and removed
   files, the live-map delta, versions, and `expected_parent`. Hub validates
   each file (column set, types, enums, token patterns on `path` columns, every
   row's namespace in the claimed set) and commits atomically. A wrong parent
   is `parent_mismatch` (409); the store fetches Hub's head and re-sends the
   difference. A commit can add, remove or remap only objects and live-map
   entries belonging to its own publisher and claimed namespaces. Hub validates
   the publisher's complete resulting head before swapping it in.

- **Paths.** A token is `pt1_` followed by the first 22 characters of
  base64url(HMAC-SHA256(key, canonical path)). The canonical path is the
  `repo_id` plus the repo-relative path, or else the absolute path with `~` for
  home. The key is 32 bytes in `~/.config/semon/asi-path-key` (mode 0600),
  shared across the operator's stores with `semon asi key export` and
  `semon asi key import`. Hub never holds it.
- **Errors.** `invalid_object` (400), `schema_mismatch` (409),
  `not_publisher` (403), `parent_mismatch` (409), plus size and quota errors.

## Crate boundary

`semon-asi` is a library with no dependency on `semon-sessions`. It holds:

- analytics schema definitions (columns, classes and docs, from which the
  generated reference is built);
- pure projections over an index input trait;
- the partition writer, manifest and garbage collection;
- the durable store;
- Hub export and import validation;
- the DuckDB query engine ([ASI query](asi-query.md)).

`semon-sessions` implements the index input trait and keeps parsing, the
ingest index, the activity projection, `StepFacts` and raw. Hub depends on
`semon-asi` only.

DuckDB comes from the `duckdb` crate, at a pinned version, with the
`bundled`, `parquet` and `json` features. The engine initializes Parquet and
JSON explicitly; extensions are never autoloaded. The C++ build adds CI time
and binary size: the first slice measures both and caches the build.

## Performance from day 1

CI measures deterministic synthetic workloads (1M rows per PR, 10M nightly,
100M weekly) and sanitized fixtures on a pinned 4-vCPU, 16 GB runner:

| Local path | Budget |
| --- | --- |
| Raw → index ingest | ≥ 10 MB/s |
| Append → origin analytics, without backlog | p95 ≤ 5 s, including sustained appends to a 10,000-step session while queries run |
| Dashboard views | p95 ≤ 500 ms |
| Ad-hoc group-by over `fact_step` | p95 ≤ 2 s at 10M rows, ≤ 5 s at 100M |
| FTS top 20 | p95 ≤ 500 ms |
| Analytics size and files | Recorded per 1M steps (Parquet, ZSTD); file count stays bounded under 100M-row sustained ingest |
| Query setup (snapshot, live map, views) | ≤ 300 ms cold with 1M sessions |

Record throughput, CPU, RSS, disk and spill. Measure origin computation and
publication lag, connected push-hop freshness, bounded remote-view latency and
memory, cancellation and offline/partial coverage separately. Local FTS is not a
fleet latency promise; no fleet-wide 500 ms target is implied. Scheduled jobs fail
their applicable budgets.

## Phases

1. **Origin projection and SQL:** preserve raw custody, implement the index
   change log and repository identity, pure analytics projection, immutable
   publication, SQL and CI performance harness.
2. **Federated pull and viewing:** admit body-free heads at the receiver; deliver
   local search and authorized, audited bounded remote view. Receiver search over
   remote sources remains unavailable until federated coverage and audit exist.
   Add author-owned durable categories and Viewer routes.
3. **Hub:** path keys, analytics-only export and Hub import validation.

These are accepted design requirements, not claims that authorization, audit or
other safeguards have shipped.

## Tests (CI only)

- Golden sanitized fixture rows and incremental equals full refresh for admitted
  origin generations, including late results, rewrites, index incarnation changes
  and source loss; #582 never publishes a partial multi-source generation as complete.
- Identity and exact anchors survive supported rebuilds and source interleaving;
  UID encoding round-trips. Incremental/full refresh cases include delayed results,
  late earlier records, midnight crossings, rewrites, removals and new incarnation.
- Keep publication/compaction gates: mid-cycle crash preserves the previous head;
  a crash after durable label commit still exports it; repeat compaction is a no-op;
  live rows remain identical; files retire only 10 minutes after unpublication;
  only stale sessions queue on version bumps. Raw replace preserves logical bytes,
  and origin archives survive pruning/rewrites. Durable updates raise and rebuilds
  leave bytes unchanged. These gates are not replaced by federation tests.
- Publication survives crashes, lost acknowledgments, parent conflicts, restarts,
  compaction and interrupted refresh. Namespace transfer admits complete cohorts,
  retains unaffected facts and labels, rejects stale publishers and leaves one
  complete head; mixed replica/pull-only namespaces retain origin authority.
- Intersection of all contributor policies governs each multi-source derivative,
  label and coverage disclosure. Restricted contributions cannot leak through a
  permissive source or a purportedly complete partial aggregate.
- Mixed rules and price versions report coverage; copied usage is not double
  counted and tool joins do not multiply measures. Origin repricing from stored
  billing inputs updates all dependent summaries without receiver repricing.
- Missing raw retains frozen facts. Exact format mismatch refuses without deletion,
  automatic reset or a multi-version reader; export preserves custody.
- Durable categorical assertions retain authorship and append-only history; free
  text never enters analytics. Rebuilds leave durable bytes unchanged.
- Hub export tokenizes typed paths and strips credentials and foreign Parquet
  metadata; Hub rejects bodies and unsupported fields, scopes tenants and validates
  claims, parents and complete heads. Retry is idempotent.
- Exercise remote authorization, audit, cache, typed pages and limits as specified
  in [ASI query](asi-query.md), and measure the performance paths above.
