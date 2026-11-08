# ASI store

Status: proposed spec for #544 (2026-11-08). Direction and settled
architecture: [ASI direction](asi-direction.md#architecture). Method and target
schema: [ASI methodology §1.1](asi-methodology.md#11-storage-schema). The query
surface over this store is specified in [ASI query](asi-query.md) (#545).

This document defines the public ASI database, its keys and versions, how
rows are derived and re-derived, the durable-state file, the metadata-push
wire contract, path tokens, retention and the `semon-asi` crate boundary.
Implementation slices are child issues of #544.

## Layers and files

```
native Raw ──▶ ingest index (per namespace, internal) ──▶ ASI projector ──▶ asi.sqlite3 (public, typed)
     │                                                                        ▲
     └──▶ sealed Raw archive                     metadata push (other origins) ┘
                                                 asi-durable.sqlite3 (labels, links, work, experiments)
                                                 asi-search.sqlite3 (body tables + FTS5; body custody only)
```

| File | Content | Rebuildable | Writer |
| --- | --- | --- | --- |
| ingest index (`sessions-index.sqlite3`, one per namespace) | Today's Session Event Index | Yes, from Raw (`SCHEMA_VERSION`) | `semon-sessions` |
| `asi.sqlite3` | Public §1.1 tables for every namespace the store holds | Origin rows: from Raw while archived. Received rows: by re-push. Frozen rows: no. | ASI projector (own namespace), push ingest (other namespaces) |
| `asi-durable.sqlite3` | Labels, adjudications, links, lanes, work items, changes, outcomes, interventions, assignments | No | Append-only APIs in `semon-asi` |
| `asi-search.sqlite3` | Body tables and their FTS5 index | Yes, from Raw | Body indexer, only where bodies are in custody |

All three ASI files sit beside the ingest index in Semon's state directory
(`$XDG_STATE_HOME/semon/`, else `~/.local/state/semon/`). A Hub tenant
workspace has its own `asi.sqlite3` and `asi-durable.sqlite3`, and no search
file. Queries open `asi.sqlite3` read-only and attach the durable file
read-only (see [ASI query](asi-query.md)). Ingest-index rebuilds never touch
any ASI file.

## Keys and identity

Identity stays native. A row is never keyed by an ordinal, generation or
SQLite row ID, so labels, links and evidence step IDs survive every rebuild.

| Key | Composition | Notes |
| --- | --- | --- |
| `namespace` | The ingest `Namespace` key of the machine that owns the source (`local` is rendered as the origin's configured machine name when pushed) | Receivers take it from the push token, never from the request body |
| `session_uid` | `{namespace}/{harness}/{native_session_id}` | Harness is the closed enum `claude`, `codex`, `copilot`, … |
| `turn_uid` | `{session_uid}/t/{native_turn_id}` | Codex turn ID; for Claude, the UUID of the turn's initiating record |
| `step_uid` | `{session_uid}/s/{encoded ActivityStepId}` | `claude:{message_id}` or `codex:{thread}:{session}:{turn}:{response}` |
| `compaction_uid` | `{session_uid}/c/{native record id}` | Falls back to `{step_uid}/c/{n}` only when the native record has no ID |
| `source_uid` | `{namespace}/{harness}` | One §1.1 source row per harness and machine |

Each table also stores the components (`namespace`, `harness`,
`native_session_id`, …) as separate indexed columns. `idx` columns are
ordinals for ordering only. Native IDs are percent-encoded where they contain
`/`. A rebuild from the same admitted Raw yields identical UIDs, and a
conformance test enforces this.

## Public schema (`asi_schema_version` 1)

Typed columns only: no JSON blobs, so the database exports to Parquet table by
table. Timestamps are integer Unix milliseconds (UTC). Enums are `TEXT` with a
`CHECK` constraint listing the closed set, and the same sets are used by the
push validator and the schema reference.

| Table | Columns beyond keys and versions | Origin of data |
| --- | --- | --- |
| `asi_meta` | `key`, `value`: `asi_schema_version`, `store_kind` (`origin`, `receiver`, `hub`), `path_key_id` | Store |
| `source` | `harness`, `machine`, `archive_coverage_from`, `archive_coverage_to` | Ingest catalog sources plus archive manifest |
| `session` | `harness`, `model`, `role` (`manager`, `worker`, `solo`, `unknown`), `parent_session_uid`, `parent_method` (`native`, `workspace`, `none`), `lane_id` (root `session_uid`), `repo`, `branch`, `started_at`, `ended_at`, `status` (`active`, `idle`, `ended`, `unknown`) | `session_catalog`, `model::Session`, lineage |
| `session_source` | `session_uid`, `source_key` (path token), `raw_offset`, `raw_prefix_sha256`, `ledger_epoch` | `ProjectionTag` for each contributing Raw ledger |
| `turn` | `session_uid`, `idx`, `started_at`, `ended_at`, `initiator` (`user`, `manager`, `peer`, `self`) | `model::Turn`, slots |
| `step` | `session_uid`, `turn_uid`, `idx`, `started_at`, `ended_at`, `model`, `tool` (primary call), `activity`, `ok`, `exit_code`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `context_tokens`, `context_band` (`lt50k`, `50k_150k`, `gt150k`), `model_ms`, `tool_ms`, `usd`, `carry_usd`, `waste_kind`, `trigger`, `compactions` | Activity projection plus step facts |
| `step_call` | `step_uid`, `ord`, `tool`, `target_token`, `command_tag`, `ok`, `exit_code`, `tool_ms` | Step facts: one row per tool call a model call requested |
| `compaction` | `session_uid`, `step_uid` (nullable), `ts` | `session_activity_compactions` |
| `cost` | `target_type` (`session`, `turn`, `step`), `target_uid`, the four token classes, `model_s`, `wall_s`, `usd`, `reported_usd` | Projector; `reported_usd` from `reported_runs` |
| `price` | `price_version`, `provider`, `model`, `input`, `cache_read`, `cache_write`, `output`, `observed_from` | Effective pricing snapshot per fingerprint |
| `event` | `turn_uid`, `ts`, `kind`, `tool`, `status`, token classes, `latency_ms`, `context_tokens`, `payload_ref` | Ingest `events` and `signals`; present only where Raw is in custody, never pushed |

Activity, waste-kind and trigger enums are the activity lane's closed sets
(methodology §3.1.1, `events/session_activity.rs`). This store mirrors them;
it does not define rules. `null` means "not derived", which is distinct from a
value. `lane` membership and metrics are views (methodology rule 5); the v1
views are `lane_session`, `cost_by_day` (repo, model, harness, day) and
`step_failure`. Segments are created by L1 (#546) and are not in v1.

Repo, branch, model and tool names are stored in plain text. No column holds a
file path, message, command line, tool output or other body text: paths appear
only as tokens (see [Path tokens](#path-tokens)) and commands only as the
classifier's allowlisted `command_tag`.

### Step facts input

`ActivityStep` today lacks turn ID, tool calls with target, `ok` and exit code,
and separate model and tool time. The projector reads them from an explicit
`StepFacts` input that `semon-sessions` provides next to `ActivityStep`, built
from the same admitted native facts. These are facts, not rules: activity,
waste and trigger stay owned by the session activity lane. Adding `StepFacts`
is coordinated with that lane (see Alignment in #544).

## Versions and re-derivation

Every derived row (`session`, `session_source`, `turn`, `step`, `step_call`,
`compaction`, `cost`) carries:

| Column | Meaning |
| --- | --- |
| `projector_version` | `semon-asi` projector version (new constant `ASI_PROJECTOR_VERSION`) |
| `rules_version` | `ACTIVITY_RULES_VERSION` the activity fields were computed under |
| `price_version` | Pricing fingerprint (effective-table SHA-256) used for `usd`; `null` when unpriced |
| `path_key_id` | Key ID of the path tokens in the row; `null` when the row has none |
| `generation` | Origin's monotonic per-session projection generation |
| `frozen` | `1` when the Raw the row came from is no longer available |

`asi_schema_version` is a single fence per file, as `SCHEMA_VERSION` is for the
ingest index. The unit of re-derivation is a session.

- **Append.** A source grows (`TagCheck::Advanced`): re-project that session,
  bump `generation`, queue it for push.
- **Rules or projector bump.** Indexed scans
  (`WHERE rules_version <> ?`, `WHERE projector_version <> ?`) select stale,
  non-frozen sessions in bounded batches, following the
  `session_activity_projections` repair pattern. Each batch re-projects whole
  sessions from the ingest index (which itself reads Raw or the archive).
- **Price bump.** Only `usd`, `carry_usd` and `price_version` are recomputed,
  from retained token columns, following the existing repricing worker. Rules
  are not re-run.
- **Key rotation.** A new `path_key_id` re-projects every non-frozen session.
- **Raw unavailable.** A session whose Raw is neither native nor archived is
  marked `frozen = 1` and keeps its versions. Frozen rows are never
  re-derived and never repaired in place.
- **Mismatch.** A row whose `session_source` no longer matches the committed
  ledger (`TagCheck::Mismatch`) is re-projected before it is served. If its Raw
  is gone it freezes.

Labels are never rewritten by any of this: they reference UIDs, which do not
change.

### Public schema changes

An `asi_schema_version` change recreates `asi.sqlite3`, with these
consequences:

- An origin re-projects its non-frozen sessions from Raw.
- A receiver or Hub store recreates its tables and reports an empty status, so
  origins re-push (see [Re-push](#staleness-and-re-push)).
- A store holding frozen rows refuses to open under the new version (decided
  2026-11-08). The operator runs `semon asi export --frozen` (JSON Lines, and
  Parquet once export lands), then `semon asi reset` to proceed. No migration
  code.

The durable file has its own `durable_schema_version` and follows the same
rule: refuse on mismatch, export, then explicit operator reset. It is never
recreated implicitly.

## Durable state (`asi-durable.sqlite3`)

Tables: `label`, `adjudication`, `link`, `lane` (title, `work_item_id`),
`work_item`, `change`, `outcome`, `intervention` and `assignment`, as in
methodology §1.1. Targets are UIDs.

- **Custody.** WAL, `synchronous=FULL`, a single writer, opened only through
  `semon-asi`'s durable store type. Derived-cache rebuilds, schema refusals on
  `asi.sqlite3` and ingest rebuilds never open it for writing.
- **Append-only.** `label` and `adjudication` are insert-only, enforced by the
  API and by `BEFORE UPDATE`/`BEFORE DELETE` triggers that raise. Corrections are
  new rows. `link`, `work_item`, `change` and `outcome` are upserts keyed by
  natural key, with `updated_at`. `outcome` stays `null` until its window closes.
- **Bodies.** Free-text fields (`label.mechanism_text`, segment goal text later)
  count as bodies: they never leave the machine without opt-in, and `sql`
  cannot read them (see [ASI query](asi-query.md)).
- **Backup.** `semon asi backup PATH` uses the SQLite online backup API.
  Deletion is only an explicit operator action.
- **Hub.** Hub receives categorical labels as metadata through the label-push
  extension specified by #546; it never receives free-text fields.

## Metadata-push wire contract

The push receiver (`semon receive`) and Hub serve two endpoints next to
`/v1/mirror/append`, with the same bearer tokens, TLS and authentication
rules ([mirror protocol](../mirror-protocol.md)). The token resolves to a
namespace (and, on Hub, a tenant workspace) before the body is read. The wire
format has no namespace field, so a request cannot name another namespace.

### `GET /v1/asi/status?cursor=C`

Returns `{"asi_schema_version":1,"path_key_id":"k1_…"|null,"sessions":[{"session":"{harness}/{native_session_id}","generation":N,"projector_version":N,"rules_version":N,"price_version":"…","repush":false}],"next":C|null}`
for the token's namespace, paged at 5,000 sessions. The sender diffs this
against its own store and pushes sessions whose generation or versions differ.
`repush: true` marks sessions the receiver wants again (failed staging, or an
operator's `semon receive asi repush --machine M`).

### `POST /v1/asi/rows`

One session snapshot, optionally split into pages:

```json
{
  "asi_schema_version": 1,
  "path_key_id": "k1_3f9a0c1d2e4b5a69",
  "session": {"harness": "codex", "native_session_id": "…", "generation": 7, "…": "§1.1 session columns and versions"},
  "page": {"index": 0, "count": 2},
  "session_sources": [], "turns": [], "steps": [], "step_calls": [], "compactions": [], "costs": [],
  "prices": []
}
```

- **Replace semantics.** A committed snapshot replaces every row of that
  session in the namespace in one transaction. Pages are staged under
  `(namespace, session, generation)` and committed when all `count` pages have
  arrived. A newer generation discards older staging; staging that stays
  incomplete for 24 hours is discarded and marked `repush`.
- **Validation** (all before any write; a refused request leaves no effect):
  - Strict schema: unknown fields, missing required fields and wrong types are
    refused.
  - Enums must be in their closed sets. Numbers must be non-negative and finite
    and within column range. Timestamps must be integer milliseconds.
  - Repo, branch, model and tool: at most 256 bytes, no control characters, and
    must not look like an absolute path (leading `/`, `~`, `\` or a drive
    letter). Tool names match `^[A-Za-z0-9_.:\-]{1,128}$`.
  - Path tokens match `^pt1_[A-Za-z0-9_-]{22}$`, and the batch `path_key_id` is
    required whenever tokens are present.
  - Every row belongs to the snapshot's session, and UIDs are rebuilt by the
    receiver from the token's namespace plus the native IDs.
  - Every `price_version` must be either in `prices` or already stored.
  - The body is at most 6 MiB and a page at most 5,000 steps. Bytes count
    toward the machine's receiver quota.
- **Responses.** `200 {"committed":true|false,"generation":N}`. Errors are
  `{"error":code,…}`:

| Status | `error` | Meaning |
| --- | --- | --- |
| 400 | `invalid_rows` (with `path` to the first offending field) | Validation failed |
| 401 | — | Token unknown |
| 409 | `schema_mismatch` (with `asi_schema_version`) | Sender and receiver disagree; upgrade one side. No negotiation (pre-release). |
| 409 | `stale_generation` (with stored `generation`) | Generation is not newer than the committed one |
| 409 | `unknown_price_version` | Resend with `prices` |
| 413 / 507 | — | Size or quota, as for mirror |

### Staleness and re-push

The receiver never derives or repairs ASI rows. It stores what the origin
sends, under the origin's versions. When the origin re-derives (a version
bump, an append or key rotation), it pushes a new generation. When the
receiver's own schema moves, its status comes back empty and origins push
everything again, including their frozen rows. A receiver is a replica of its
origins. If an origin machine is decommissioned, the receiver's rows are the
only copy, so its operator should run `semon asi export` before upgrading the
receiver across a schema change.

### Sender behavior and body opt-in

`semon push` pushes ASI rows for every local session by default. Raw is
mirrored through `/v1/mirror/append` only for sources the operator opts in,
per harness (`semon push --bodies claude --bodies codex`, or the push config).
Opted-in Raw is redacted by the existing push-side redaction. On the receiver
it feeds only `asi-search.sqlite3` and L1 evidence packets; it never produces
`asi.sqlite3` rows. Senders without the bodies opt-in send no Raw at all.
This replaces the current Raw-only push default (pre-release, no
compatibility mode).

## Path tokens

File paths cross machines only as keyed HMAC tokens, so redundant-read and
rework overlap still join across sessions and machines.

- **Key.** 32 random bytes in `$XDG_CONFIG_HOME/semon/asi-path-key`
  (`~/.config/semon/asi-path-key`), mode 0600, created on first projection if
  absent. One key per operator workspace: `semon asi key export` and
  `semon asi key import FILE` copy it between the operator's machines.
  Receivers and Hub never hold it, and no wire field carries it.
- **Key ID.** `k1_` followed by the first 16 hex digits of
  `SHA-256("semon-asi-path-key-id\0" || key)`. Rows and batches carry it, and
  tokens are only comparable within one key ID.
- **Canonical input.** Inside a repository with a known repo identity:
  `"r\0" || repo || "\0" || repo-relative path`. Otherwise:
  `"a\0" || absolute path`, with the home directory written as `~`. Paths are
  lexically normalized (`.` and `..` resolved, no trailing slash) and not
  resolved through symlinks.
- **Token.** `pt1_` followed by the first 22 characters of
  base64url(HMAC-SHA256(key, input)), which is 132 bits.
- **Rotation.** Importing a new key changes `path_key_id`, re-projects
  non-frozen sessions and re-pushes them. Frozen rows keep their old key ID.

Session sources (`session_source.source_key`) use the same tokenization.

## Retention

- `asi.sqlite3` and `asi-durable.sqlite3`: no automatic deletion. The 60-day
  `analytics::KEEP_MS` window bounds only the existing Analytics input
  projection until Analytics is rebuilt on this store (#547); it does not
  apply here.
- **Raw archive.** It is kept by whoever holds body custody: the origin for
  its own sessions, a receiver for opted-in sources. It is retained
  indefinitely by default (decided 2026-11-08) and the operator can set
  `archive.retention_days`. The origin copies each native log into the
  archive as it is observed, sealed with the existing zstd segment format
  ([`sealed`](../../crates/semon-sessions/src/sealed.rs)), so a harness pruning
  its own logs does not freeze rows. Archive withdrawal follows existing
  History custody rules: it never deletes native logs and only marks rows
  `frozen`.
- `source.archive_coverage_from/to` report what the archive covers.

## Crate boundary: `semon-asi`

`semon-asi` is a library crate with no dependency on `semon-sessions` or any
native parser:

- public schema definition (tables, columns, enums, documentation), which
  emits the DDL and the generated schema reference;
- row and wire types (serde, `deny_unknown_fields`), validation, and the
  push-ingest function `ingest(&NamespaceCapability, Snapshot) -> Result`;
- path tokenization and key-ID derivation (the key file itself is managed by
  the CLI);
- public store: open and version fence, snapshot replace, version scans,
  freeze, export and reset;
- durable store;
- the query engine ([ASI query](asi-query.md)).

`semon-sessions` depends on `semon-asi`. It keeps native parsing, the ingest
index, the activity projection, `StepFacts`, the Raw archive and the ASI
projector, which maps ingest state to `semon-asi` row types. `semon-push`
hosts the HTTP endpoints and calls `ingest`. Hub depends on `semon-asi` only,
for ingest, store and query per tenant workspace, mapping its own
authentication to a namespace capability.

## Budgets

Proposed defaults, to be measured under #510:

- incremental projection of an appended step: under 5 ms;
- ingest of a 5,000-step page on the CI runner: under 500 ms;
- `asi.sqlite3` growth: under 2 KiB per step;
- the existing 2 s cold first-page gate stays green.

## Tests (CI only)

- Golden public rows for the sanitized harness fixtures (`asi-golden.json`),
  covering every §1.1 column the fixtures exercise.
- Rebuild identity: dropping the ingest index and `asi.sqlite3` and
  re-projecting yields identical UIDs and rows; durable labels still resolve.
- Version scans: a rules bump re-derives only rows with a stale
  `rules_version`; a price bump updates only cost columns; counts are asserted.
- Freeze: withdrawing Raw marks rows frozen; a schema bump with frozen rows
  refuses to open; export then reset proceeds.
- Durable custody: the durable file's bytes are unchanged across ingest and
  public rebuilds; update or delete of a label raises.
- Wire: every refusal in the table above, with no partial effect; a token for
  machine A cannot write machine B; paged staging and commit; stale
  generation; re-push after the receiver's schema bump.
- Path tokens: known-answer vectors; same repo-relative path on two machines
  gives the same token; no path, message or command text appears in any
  pushed byte (fixture scan).
- Namespace conformance cases from `storage/conformance.rs` keep passing.
