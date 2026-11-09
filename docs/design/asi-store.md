# ASI store

Status: proposed spec for #544 (revised 2026-11-09 for central derivation and
DuckDB gold). Direction: [ASI direction](asi-direction.md#architecture).
Method: [ASI methodology §1.1](asi-methodology.md#11-storage-schema). The
query surface is specified in [ASI query](asi-query.md) (#545).

## Shape

```
machine A ─┐  redacted Raw (existing semon push mirror path)
machine B ─┼──────────────▶ receiver (or a single machine, standalone)
           │                 bronze   immutable Raw chains + redaction provenance
           │                 silver   ingest index per machine (exists today) + activity + StepFacts
           │                 gold     Parquet partitions (facts + dimensions) ── DuckDB ── sql
           │                 durable  asi-durable.sqlite3 (labels, links, work items)
           │                 FTS5     body index in silver ─────────────────────────────── search / view
           │
           └─ Hub (per tenant) ◀── gold partitions only: no bodies, paths as HMAC tokens
```

Each source has one **authoritative store**: the receiver it pushes to, or the
machine itself if it pushes nowhere. Only the authoritative store exports to
Hub. A machine that pushes may also keep local gold for offline queries; that
copy is never merged or exported.

| Data | Location | Rebuildable from | Engine |
| --- | --- | --- | --- |
| Bronze | Native logs plus `archive/`; on a receiver, the mirror store | — (custody) | Files |
| Silver | `sessions-index.sqlite3`, one per namespace | Bronze | SQLite |
| Gold | `gold/` Parquet partitions plus `gold/manifest.json` | Silver | DuckDB (read) |
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

## Bronze

- **Receiver.** The mirror store is bronze. A `replace` append first moves
  the replaced bytes into an immutable chain
  (`archive/{archive_source_id}/{chain_id}.zst`, with random IDs and a
  manifest written by atomic rename), so earlier content stays re-derivable.
  Each chain records `redactor_version` and the digests of the redacted bytes
  it holds.
- **Standalone machine.** The archiver keeps an append-only zstd copy of each
  native log under `archive/`, admitting only the prefix silver has consumed.
  A native rewrite starts a new chain and keeps the old one. Silver reads a
  source from the archive when the native log is gone.
- **Retention.** Bronze is kept indefinitely by default (decided 2026-11-08),
  with an optional `archive.retention_days`. Nothing deletes native logs.

## Silver

Silver is today's ingest index and activity projection, run per namespace
(receivers already do this for mirrored machines, in `received.rs`). It gains:

- **`StepFacts`** (#556, owned by the session activity lane): turn ID, tool
  calls (tool, target path, command tag, ok, exit code, tool time), model time
  and the full `BillingUsage` inputs. These are facts only; the L0 rules stay
  with that lane.
- **Change sequence.** A monotonic per-namespace `ingest_seq`, stamped on each
  committed source advance or rewrite, lets gold transforms find touched
  sessions without scanning by event time. An offline machine can deliver
  months-old events today.
- **Bodies for FTS5** (phase 2): see [ASI query](asi-query.md#search).

## Gold model

Kimball facts at explicit grains, plus conformed dimensions. Columns are
typed, timestamps are UTC milliseconds, and enums are closed sets. Every
column declares a class: `key`, `enum`, `number`, `time`, `name`, `path` or
`body`. The class drives the Hub export and the query reference. Gold has no
`body` columns.

| Fact | Grain | Main columns |
| --- | --- | --- |
| `fact_step` | One model invocation | step, turn and session keys; `model_key`, `repo_key`, `date_key`; billing inputs; `context_tokens`, `context_band`; `model_ms`, `tool_ms`; `usd_est`, `price_status`; `activity`, `waste_kind`, `trigger`, `ok` |
| `fact_tool_call` | One tool call | `step_uid`, `ord`, `tool_key`, `target_path` (class `path`), `command_tag`, `ok`, `exit_code`, `tool_ms` |
| `fact_turn` | One turn | `initiator`, `started_at`, `ended_at`, step count |
| `fact_session` | One session (aggregate grain) | Totals by token class, `usd_est`, `reported_usd`, wall and model time, step and failure counts |
| `fact_compaction` | One compaction | `step_uid` (nullable), `ts` |
| `fact_label` | One label assertion | Target UID, field, value, labeller, taxonomy version, confidence, `created_at` (exported from the durable file; no free text) |

| Dimension | Notes |
| --- | --- |
| `dim_machine`, `dim_harness`, `dim_tool`, `dim_date` | Conformed and small |
| `dim_session` | Role, parent, parent method, lane root, repo, branch, status, start and end |
| `dim_repo` | `repo_id` (the normalized `origin` remote; non-default ports kept) and a display name |
| `dim_model` | SCD2: `model_key`, model, provider, `valid_from`, `valid_to` |
| `dim_price` | SCD2 by price version: provider, model, `rate_kind`, `tier`, `threshold_tokens`, `usd_per_unit`, `valid_from`, `valid_to`, `learned_at` |

`usd_est` is an estimate under a named price version, never presented as
billed spend. Missing prices are `price_status = unpriced` and never summed as
zero. Parallel tool calls live in `fact_tool_call`, so joining them never
multiplies step tokens. The semantic layer is a set of versioned DuckDB views:
`cost_by_day`, `step_failure`, `lane_session` and `current_label` (the latest
assertion per target and field, with human adjudication taking precedence).

## Partitions, publication and versions

- **Layout.** Each fact is stored as
  `gold/{table}/ns={namespace}/day={session start day}/part-{id}.parquet`.
  Dimensions are one file per table. `gold/manifest.json` lists every current
  file with its digest, row count and versions (`transform_version`,
  `rules_version`, `price_version`), plus the `ingest_seq` checkpoint of each
  namespace.
- **Single writer.** One gold writer per store, inside the long-running Semon
  process (the receiver, or the local push/viewer service), with a
  `gold.lock` file. Readers never take locks.
- **Incremental.** Each cycle (at most every 2 s) reads the sessions touched
  since the checkpoint, recomputes only their new or changed steps from
  silver (the tail), and rewrites the affected day partitions as new files.
  It then publishes the files, and the new checkpoint, with one atomic manifest
  rename. A crash leaves the previous manifest. Files no manifest names are
  removed after a 10-minute grace period, so in-flight queries finish.
- **Full refresh.** The same transform can rebuild all of gold from silver. CI
  asserts that incremental and full-refresh gold are identical.
- **Version bumps.** The writer compares each partition's versions in the
  manifest with the current ones and rewrites stale partitions in bounded
  background batches. A price bump recomputes only `usd_est` from the stored
  billing inputs.
- **Missing bronze.** A partition whose sources are gone keeps its old
  versions and is marked `source_missing` in the manifest. A gold schema change
  (`asi_schema_version`) rebuilds gold, except that a store holding
  `source_missing` partitions refuses (decided 2026-11-08): the operator runs
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
  gold or Hub, and are readable only through `view` ([ASI query](asi-query.md)).
- **Export.** Categorical columns are exported to `fact_label` on change.
  `semon asi backup PATH` copies the file with the SQLite online backup API.

## Hub export (phase 3)

A store pushes its gold partitions to its Hub tenant:
`PUT /v1/asi/gold/{table}/{namespace}/{day}`, with the Parquet file as the
body and `X-Semon-Digest` and `X-Semon-Versions` headers. The bearer token is
bound to the tenant workspace and to the namespaces that token may publish.

- **Paths.** Before upload, every `path` column is replaced by
  `pt1_` followed by the first 22 characters of
  base64url(HMAC-SHA256(key, canonical path)). The canonical path is the
  `repo_id` plus the repo-relative path, or else the absolute path with `~` for
  home. The key is 32 bytes in `~/.config/semon/asi-path-key` (mode 0600) and
  is shared across the operator's stores with `semon asi key export` and
  `semon asi key import`. Hub never holds it.
- **Validation.** Hub checks the file against the gold schema for its
  `asi_schema_version`: column set, types, enums, and token patterns on `path`
  columns. It refuses any column outside the schema, then replaces the
  partition atomically.
- **Errors.** `invalid_partition` (400), `schema_mismatch` (409),
  `namespace_not_allowed` (403), plus size and quota errors as for mirror.

## Crate boundary

`semon-asi` is a library with no dependency on `semon-sessions`. It holds:

- gold schema definitions (columns, classes and docs, from which the
  generated reference is built);
- transforms over a silver input trait;
- the partition writer, manifest and garbage collection;
- the durable store;
- Hub export and import validation;
- the DuckDB query engine ([ASI query](asi-query.md)).

`semon-sessions` implements the silver input trait and keeps parsing, the
ingest index, the activity projection, `StepFacts` and bronze. Hub depends on
`semon-asi` only.

DuckDB comes from the `duckdb` crate with its `bundled` feature. Its C++ build
adds CI time and binary size: the first slice measures both and caches the
build in CI.

## Performance from day 1

Budgets on a pinned 4-vCPU, 16 GB CI runner, using deterministic synthetic
data (1M rows per PR, 10M nightly, 100M weekly) plus the sanitized fixtures:

| Path | Budget |
| --- | --- |
| Raw → silver ingest | ≥ 10 MB/s |
| Freshness (append to gold visible) | p95 ≤ 5 s without backlog; per-cycle work independent of session length |
| Dashboard views | p95 ≤ 500 ms |
| Ad-hoc group-by over `fact_step` | p95 ≤ 2 s at 10M rows, ≤ 5 s at 100M |
| FTS top 20 | p95 ≤ 500 ms |
| Gold size | Recorded per 1M steps; Parquet with ZSTD |

Runs record throughput, CPU, RSS, disk and spill. A run that misses a budget
fails the scheduled job. For context, the measured workload of one heavy user
today is about 466 MB of logs in 2.5 months.

## Phases

1. **Local and receiver gold plus SQL.** `semon-asi`, the gold model,
   partitions, transforms and the DuckDB engine with `sql` and `schema`, plus
   the CI performance harness. This works standalone and on receivers, which
   already hold silver per machine.
2. **Labels, search and bronze hardening.** The durable store and
   `fact_label`, FTS5 with `search` and `view`, the bronze archive and
   receiver replace preservation, and Viewer routes.
3. **Hub.** Path keys, gold export and Hub import validation.

## Tests (CI only)

- Golden gold rows for the sanitized fixtures, and incremental output equal
  to full refresh.
- Identity: UIDs equal across rebuilds; the encoding round-trips; record
  anchors are stable under interleaved appends.
- Publication: a crash mid-cycle leaves the old manifest; garbage collection
  respects the grace period; a version bump rewrites only stale partitions.
- Bronze: a receiver `replace` preserves bytes; a standalone archive survives
  pruning and rewrites; `source_missing` refusal, then export and reset.
- Durable: bytes unchanged across rebuilds; label updates raise.
- Hub export: no `body` column ever exported; `path` columns are tokens;
  every refusal leaves the partition unchanged.
- The performance budgets above.
