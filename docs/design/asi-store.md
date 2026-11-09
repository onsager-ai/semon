# ASI store

Status: proposed spec for #544 (revised 2026-11-09 for central derivation and
DuckDB analytics). Direction: [ASI direction](asi-direction.md#architecture).
Method: [ASI methodology §1.1](asi-methodology.md#11-storage-schema). The
query surface is specified in [ASI query](asi-query.md) (#545).

## Shape

```
machine A ─┐  redacted Raw (existing semon push mirror path)
machine B ─┼──────────────▶ receiver (or a single machine, standalone)
           │                 raw   immutable Raw chains + redaction provenance
           │                 index   ingest index per machine (exists today) + activity + StepFacts
           │                 analytics     Parquet generations (facts + dimensions) ─ DuckDB ── sql
           │                 durable  asi-durable.sqlite3 (labels, links, work items)
           │                 FTS5     body index in index ─────────────────────────────── search / view
           │
           └─ Hub (per tenant) ◀── analytics generations only: no bodies, paths as HMAC tokens
```

Each source has one **authoritative store**: the receiver it pushes to, or the
machine itself if it pushes nowhere. Only the authoritative store exports to
Hub. A machine that pushes may also keep local analytics for offline queries; that
copy is never merged or exported.

| Data | Location | Rebuildable from | Engine |
| --- | --- | --- | --- |
| Raw | Native logs plus `archive/`; on a receiver, the mirror store | — (custody) | Files |
| Index | `sessions-index.sqlite3`, one per namespace | Raw | SQLite |
| Analytics | `analytics/` immutable Parquet generation files plus `analytics/manifest.json` | Index | DuckDB (read) |
| Durable | `asi-durable.sqlite3` | — (custody) | SQLite |

## Identity

Identity stays native. Rows are never keyed by ordinals, generations or row
IDs, so labels survive every rebuild.

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
  | `turn_uid` | `{session_uid}/t/{id}`, where `id` is the Codex `turn_id`, the Claude initiating record `uuid`, the handoff ID, or else a record anchor |
  | `step_uid` | `{session_uid}/s/claude/{message_id}` or `{session_uid}/s/codex/{thread}/{session}/{turn}/{response}` (`ActivityStepId`) |
  | `compaction_uid` | `{session_uid}/c/{id}`, where `id` is the native record ID or else a record anchor |

- **Record anchor.** `@{source}:{n}`, where `source` is the native log's file
  stem (which matches `^[A-Za-z0-9._\-]{1,128}$`) and `n` is the record's
  ordinal within that log. It is independent of paths, keys and how logs
  interleave, and stable while the log's prefix is unchanged.

## Raw

Raw custody is in phase 1: analytics is only as durable as the raw logs it can be
rebuilt from.

- **Receiver.** The mirror store is raw. Before acknowledging a `replace`
  append, the receiver moves the replaced logical bytes (read through `sealed`,
  whether plain or sealed) into an immutable chain:
  `archive/{archive_source_id}/{chain_id}.zst`, with random IDs and a manifest
  published by atomic rename. Retiring a seal never deletes bytes that no chain
  holds.
- **Redaction provenance.** The mirror `Append` gains a required
  `redactor_version`, a contract change made directly under the pre-release
  policy. Each chain records it, together with the digest of the redacted bytes
  it holds.
- **Standalone machine.** The archiver keeps an append-only zstd copy of each
  native log under `archive/`, admitting only the prefix index has consumed.
  A native rewrite starts a new chain and keeps the old one. Index reads a
  source from the archive when the native log is gone.
- **Retention.** Raw is kept indefinitely by default (decided 2026-11-08),
  with an optional `archive.retention_days`. Nothing deletes native logs.

## Index

Index is today's ingest index and activity projection, run per namespace
(receivers already do this for mirrored machines, in `received.rs`). It gains:

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
  changes the mirror facts contract directly. Index persists the typed identity
  in `silver_session_state.repo_id` alongside its analytics digest, in the same
  transaction as publication and `silver_change`. It remains available when
  sources disappear; local remotes remain class `path` and are not display
  metadata. Repository dependencies use the effective native working directory,
  including a subagent sidecar's `worktreePath`.
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
  becomes the store's single writer: `semon receive`, `semon push`, the Viewer
  server or `semon mcp` (so MCP-only users get fresh analytics). Other processes
  only read.
- **Cycle.** At most every 2 s, the writer reads the index change log and
  recomputes every touched session **whole**. It doesn't patch tails: a
  following step can change an earlier step's duration, and a late record can
  change older rows. Per-cycle work is proportional to the touched sessions'
  size, which is milliseconds even for sessions of thousands of steps.
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
- **Full refresh.** The same transform rebuilds all of analytics from index. CI
  asserts that incremental and full-refresh live rows are identical.
- **Version bumps.** Sessions whose rows carry a stale `transform_version`,
  `rules_version` or `price_version` are queued in bounded batches, like
  touched sessions.
- **Missing raw.** Sessions whose sources are gone keep their rows and
  versions and are listed as `source_missing` in the manifest. An analytics schema
  change (`asi_schema_version`) rebuilds analytics, except that a store with
  `source_missing` sessions refuses (decided 2026-11-08): the operator runs
  `semon asi export` and then `semon asi reset`.

## Durable state

`asi-durable.sqlite3` holds `label`, `adjudication`, `link`, `lane`,
`work_item`, `change`, `outcome`, `intervention` and `assignment`
(methodology §1.1), keyed by UIDs.

- **Writes.** WAL, `synchronous=FULL`, and a single writer through
  `semon-asi`. `label` and `adjudication` are insert-only, enforced by triggers.
- **Rebuilds** never open it for writing. It has its own version: on a
  mismatch it refuses, and the operator exports and then resets.
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

Hub holds the same layout per tenant, with one independent head per
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
- transforms over a index input trait;
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

Budgets on a pinned 4-vCPU, 16 GB CI runner, using deterministic synthetic
data (1M rows per PR, 10M nightly, 100M weekly) plus the sanitized fixtures:

| Path | Budget |
| --- | --- |
| Raw → index ingest | ≥ 10 MB/s |
| Freshness (append to analytics visible) | p95 ≤ 5 s without backlog, including sustained appends to a 10,000-step session while queries run |
| Dashboard views | p95 ≤ 500 ms |
| Ad-hoc group-by over `fact_step` | p95 ≤ 2 s at 10M rows, ≤ 5 s at 100M |
| FTS top 20 | p95 ≤ 500 ms |
| Analytics size and files | Recorded per 1M steps (Parquet, ZSTD); file count stays bounded under 100M-row sustained ingest |
| Query setup (snapshot, live map, views) | ≤ 300 ms cold with 1M sessions |

Runs record throughput, CPU, RSS, disk and spill. A run that misses a budget
fails the scheduled job. For context, the measured workload of one heavy user
today is about 466 MB of logs in 2.5 months.

## Phases

1. **Raw custody, analytics and SQL.**
   - Receiver replace preservation, `redactor_version`, and the standalone
     archive.
   - The index change log and `repo_id` facts.
   - `semon-asi`: the analytics model, generations, transforms, and the DuckDB
     engine with `sql` and `schema`.
   - The CI performance harness.

   This works standalone and on receivers.
2. **Labels and search.** The durable store and `fact_label`, FTS5 with
   `search` and `view`, and Viewer routes.
3. **Hub.** Path keys, analytics export and Hub import validation.

## Tests (CI only)

- Golden analytics rows for the sanitized fixtures, and incremental output equal
  to full refresh.
- Identity: UIDs equal across rebuilds; the encoding round-trips; record
  anchors are stable under interleaved appends.
- Incremental equals full refresh after: delayed tool results, a late earlier
  record, a rewrite, a session crossing midnight, a removed session, and an
  index rebuild (new incarnation).
- Publication: a crash mid-cycle leaves the old generation; a crash after a
  label commit still exports that label; repeating a compaction is a no-op; a file is retired
  only 10 minutes after it leaves the live generation; compaction keeps the
  live rows identical; a version bump queues only stale sessions.
- Spend: copied usage is not double-counted; the step-to-tool-call view
  doesn't multiply tokens.
- Raw: a receiver `replace` preserves bytes; a standalone archive survives
  pruning and rewrites; `source_missing` refusal, then export and reset.
- Durable: bytes unchanged across rebuilds; label updates raise.
- Hub export: no `body` column ever exported; `path` columns and local
  remotes are tokens; credentials are stripped from `repo_id`; foreign Parquet
  metadata is dropped; a stale publisher, a wrong parent and a retried upload
  each behave as specified; two publishers in one tenant compose, and neither
  can remove the other's objects; a namespace transfer shows exactly one copy
  of its rows before and after activation; an interrupted export leaves Hub on the old
  generation.
- The performance budgets above.
