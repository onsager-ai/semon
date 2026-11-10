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
  becomes `%XX`.
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
- **Bodies for FTS5** (phase 2): see [ASI query](asi-query.md#search).

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
| `dim_repo` | `repo_id` and a display name. `repo_id` is the `origin` remote with credentials, scheme and `.git` removed and the host lowercased, keeping non-default ports (`github.com/onsager-ai/semon`). A local-path or `file:` remote is classed `path`, and no remote gives `unknown`. |
| `dim_model` | Immutable model identity: model, provider. Historical attributes can become SCD2 when they are needed. |
| `dim_price` | SCD2 by price version: provider, model, `rate_kind`, `tier`, `threshold_tokens`, `usd_per_unit`, `valid_from`, `valid_to`, `learned_at` |

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

- **Writer.** The first Semon process that needs analytics takes `analytics.lock` and
  becomes the authoritative producer store's single writer: push, Viewer, MCP,
  or receive for an explicitly transferred replica namespace. Other processes only
  read. Imported receiver heads are admitted publications, not locally rebuilt indexes.
- **Cycle.** At most every 2 s, the writer reads the index change log and
  recomputes every touched session **whole**. It doesn't patch tails: a
  following step can change an earlier step's duration, and a late record can
  change older rows. Per-cycle work is proportional to the touched sessions'
  size; CI measures the cost without assuming a latency for large sessions.
  - It writes one new file per table, `analytics/{table}/g{generation}.parquet`,
    holding the full rows of those sessions with a `_gen` column. Dimensions
    are rewritten whole, as `analytics/dim_x/g{generation}.parquet`.
  - It writes the live map, `session_uid → _gen` with removed sessions
    dropped, as a typed Parquet file `analytics/live/g{generation}.parquet`, so the
    manifest stays small at any number of sessions.
  - It fsyncs those files, then writes `manifest.json`: the generation, file
    lists with their compaction level, versions, each namespace's incarnation
    and `seq` checkpoint, the durable revision checkpoint, and the live-map
    file. It fsyncs the manifest, renames it into place, and fsyncs the
    directory. A crash leaves the previous generation.
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
- **Retirement.** A file leaves the store 10 minutes after the first
  published generation that no longer names it. Queries hold a snapshot for at
  most their deadline plus fetch time, well under that.
- **Full refresh.** The same pure projection rebuilds origin analytics from an
  available admitted index. CI compares incremental and full-refresh live rows.
  Imported frozen heads may be the only surviving facts and are not assumed
  rebuildable from a receiver index.
- **Versions and missing sources.** Queue stale rules, transform and pricing
  versions in bounded producer batches; a transform-only change reprojects admitted
  index facts without deriving labels/prices again. Origin rules/pricing changes
  publish the resulting admitted activity generation; receivers retain each origin's
  recorded versions and report coverage. A missing source retains its complete
  facts and is marked `source_missing`. In particular, the multi-source gap #582
  must not replace a complete generation with a fragment. Re-derive at the origin
  when raw returns. A format mismatch refuses exactly; preserve or export frozen
  facts and custody, with no automatic reset or multi-version exception.
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
  `semon asi backup PATH` copies the file with the SQLite online backup API.

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
