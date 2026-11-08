# ASI store

Status: proposed spec for #544 (2026-11-08). Direction and settled
architecture: [ASI direction](asi-direction.md#architecture). Method and target
schema: [ASI methodology §1.1](asi-methodology.md#11-storage-schema). The query
surface over this store is specified in [ASI query](asi-query.md) (#545).

This document defines the public ASI database, its keys and versions, how
rows are derived, published and re-derived, the Raw archive, the
durable-state file, the metadata-push wire contract, path tokens, retention
and the `semon-asi` crate boundary. Implementation slices are child issues of
#544.

## Layers and files

```
native Raw ─┬─▶ ingest index (per namespace, internal) ──▶ ASI projector ──▶ asi.sqlite3 (public, typed)
            │                       ▲                                             ▲
            └─▶ Raw archive ────────┘ (read when native Raw is gone)  metadata push from other origins
                                                     asi-durable.sqlite3 (labels, links, work, experiments)
                                                     asi-search.sqlite3 (body tables + FTS5; body custody only)
```

| File | Content | Rebuildable | Writer |
| --- | --- | --- | --- |
| ingest index (`sessions-index.sqlite3`, one per namespace) | Today's Session Event Index | Yes, from Raw (`SCHEMA_VERSION`) | `semon-sessions` |
| `asi.sqlite3` | Public §1.1 tables for every namespace the store holds | Own rows: from Raw or the archive. Received rows: by re-push. Frozen rows: no. | ASI projector (own namespace), push ingest (other namespaces) |
| `asi-durable.sqlite3` | Labels, adjudications, links, lanes, work items, changes, outcomes, interventions, assignments | No | Append-only APIs in `semon-asi` |
| `asi-search.sqlite3` | Body tables and their FTS5 index | Yes, from Raw | Body indexer, only where bodies are in custody |

All ASI files sit in Semon's state directory (`$XDG_STATE_HOME/semon/`, else
`~/.local/state/semon/`). A Hub tenant workspace has its own `asi.sqlite3`
and `asi-durable.sqlite3`, and no search file. Queries open `asi.sqlite3`
read-only and attach the durable file read-only (see
[ASI query](asi-query.md)). Ingest-index rebuilds never touch any ASI file.

## Keys and identity

Identity stays native. A row is never keyed by an ordinal, generation or
SQLite row ID, so labels, links and evidence step IDs survive every rebuild.

**Namespace.** Each store's own sessions use its configured machine name as
their namespace from the start, never `local`, so a UID is the same on the
origin and on every receiver. A receiver binds each push token to a
namespace equal to the token's name (`semon receive token add NAME`), refuses
a token whose name equals its own machine name, and reserves `local`. The
origin learns the receiver's name for it from `GET /v1/asi/status` and refuses
to push when that name differs from its own machine name, so the two can't
diverge silently.

**Encoding.** A UID is a sequence of components joined by `/`. Every
component is percent-encoded: RFC 3986 unreserved characters stay as they
are, and every other byte (including `/`, `:` and `%`) becomes `%XX`. Joining
is therefore unambiguous. On the wire, rows carry namespace-relative UIDs (the
part after `{namespace}/`), and each store prefixes the namespace.

| Key | Components | Native source, in order of preference |
| --- | --- | --- |
| `session_uid` | `{namespace}/{harness}/{native_session_id}` | Harness session ID |
| `turn_uid` | `{session_uid}/t/{turn_id}` | Codex `turn_id`; Claude initiating record `uuid`; the handoff ID; else the source anchor `{source_token}@{offset}` of the turn's first record |
| `step_uid` | `{session_uid}/s/claude/{message_id}` or `{session_uid}/s/codex/{thread}/{session}/{turn}/{response}` | `ActivityStepId` components, each encoded separately |
| `compaction_uid` | `{session_uid}/c/{id}` | Native record ID; else the source anchor of the compaction record |
| `source_uid` | `{namespace}/{harness}` | One §1.1 source row per harness and machine |

Source anchors are stable while the Raw prefix that holds them is unchanged.
A native rewrite (a new ledger epoch) can change them, and that publishes a
new generation. Each table also stores the components as separate indexed
columns. `idx` columns are ordinals for ordering only. A rebuild from the same
admitted Raw yields identical UIDs, and a conformance test enforces this.

## Public schema (`asi_schema_version` 1)

Typed columns only: no JSON blobs, so the database exports to Parquet table by
table. Timestamps are integer Unix milliseconds (UTC). Enums are `TEXT` with a
`CHECK` listing the closed set.

Every column in `semon-asi`'s schema definition declares a **class**: `key`,
`enum`, `number`, `time`, `name` (bounded plain identifiers: repo, branch,
model, tool), `token` (path tokens, digests) or `body` (free text). The class
drives the push validator, the egress rules and the query allowlist
([ASI query](asi-query.md)). A column without a class does not compile into
the schema. No `body` column exists in `asi.sqlite3`.

| Table | Columns beyond keys and versions | Origin of data |
| --- | --- | --- |
| `asi_meta` | `key`, `value`: `asi_schema_version`, `store_kind` (`origin`, `receiver`, `hub`), `machine`, `path_key_id` | Store |
| `source` | `harness`, `machine`, `archive_coverage_from`, `archive_coverage_to` | Ingest catalog sources plus archive manifest |
| `session` | `harness`, `model`, `role` (`manager`, `worker`, `solo`, `unknown`), `parent_session_uid`, `parent_method` (`native`, `workspace`, `none`), `lane_id` (root `session_uid`), `repo` (display name), `repo_id`, `branch`, `started_at`, `ended_at`, `status` (`active`, `idle`, `ended`, `unknown`), `stale` | `session_catalog`, `model::Session`, lineage, [repo identity](#repo-identity) |
| `session_source` | `session_uid`, `source_token`, `raw_offset`, `raw_prefix_sha256`, `ledger_epoch` | `ProjectionTag` of each contributing Raw ledger (the publication witness) |
| `turn` | `session_uid`, `idx`, `started_at`, `ended_at`, `initiator` (`user`, `manager`, `peer`, `self`) | `model::Turn`, slots |
| `step` | `session_uid`, `turn_uid`, `idx`, `started_at`, `ended_at`, `model`, `tool` (primary call), `activity`, `ok`, `exit_code`, billing inputs, `context_tokens`, `context_band` (`lt50k`, `50k_150k`, `gt150k`), `model_ms`, `tool_ms`, `usd`, `carry_usd`, `price_status`, `waste_kind`, `trigger`, `compactions` | Activity projection plus `StepFacts` |
| `step_call` | `step_uid`, `ord`, `tool`, `target_token`, `command_tag`, `ok`, `exit_code`, `tool_ms` | `StepFacts`: one row per tool call the model call requested |
| `compaction` | `session_uid`, `step_uid` (nullable), `ts` | `session_activity_compactions` |
| `cost` | `target_type` (`session`, `turn`), `target_uid`, billing inputs, `model_s`, `wall_s`, `usd`, `reported_usd`, `price_status` | Projector; `reported_usd` from `reported_runs` |
| `price` | `price_version`, `provider`, `model`, `rate_kind`, `tier`, `threshold_tokens`, `usd_per_unit` | The effective pricing snapshot identified by its fingerprint |
| `event` | `turn_uid`, `ts`, `kind`, `tool`, `status`, token classes, `latency_ms`, `context_tokens`, `payload_ref` | Ingest `events` and `signals`; present only where Raw is in custody, never pushed |

**Billing inputs** are the typed `BillingUsage` facts, kept so price bumps
are reproducible: `input_tokens`, `output_tokens`, `reasoning_tokens`,
`cache_read_tokens`, `cache_write_5m_tokens`, `cache_write_1h_tokens`,
`web_search_requests`, `speed`, `service_tier` and `prompt_tokens` (selects
the long-context tier). `price.rate_kind` covers each of those inputs, and
`tier` (`base`, `long_context`, `fast`, …) with `threshold_tokens` covers tiered
rates. `price_status` is `priced`, `partial` (some inputs had no rate) or
`unpriced` (unknown model). `usd` is `null` when unpriced. Step-level cost
lives on `step`; `cost` holds session and turn totals, including
provider-reported cost.

Activity, waste-kind and trigger enums are the activity lane's closed sets
(methodology §3.1.1, `events/session_activity.rs`). This store mirrors them;
it does not define rules. `null` means "not derived", which is distinct from a
value. Lane membership and metrics are views (methodology rule 5); the v1
views are `lane_session`, `cost_by_day` (repo, model, harness, day) and
`step_failure`. Segments are created by L1 (#546) and are not in v1.

### Step facts input

`ActivityStep` today lacks turn ID, tool calls with target, `ok` and exit code,
separate model and tool time, and the full billing inputs. The projector reads
them from an explicit `StepFacts` input that `semon-sessions` provides next to
`ActivityStep`, built from the same admitted native facts. These are facts,
not rules: activity, waste and trigger stay owned by the session activity
lane. Adding `StepFacts` is coordinated with that lane (see Alignment in
#544).

## Versions, publication and re-derivation

Every derived row (`session`, `session_source`, `turn`, `step`, `step_call`,
`compaction`, `cost`) carries:

| Column | Meaning |
| --- | --- |
| `projector_version` | `semon-asi` projector version (new constant `ASI_PROJECTOR_VERSION`) |
| `rules_version` | `ACTIVITY_RULES_VERSION` the activity fields were computed under |
| `price_version` | Pricing fingerprint (effective-table SHA-256) used for `usd`; `null` when unpriced |
| `path_key_id` | Key ID of the path tokens in the row; `null` when the row has none |
| `generation` | Session generation (see below) |
| `frozen` | `1` when the Raw the row came from is no longer available |

`asi_schema_version` is a single fence per file, as `SCHEMA_VERSION` is for the
ingest index. The unit of projection, publication and re-derivation is a
session.

**Generation.** A session's generation is `max(previous + 1, now_ms)`, where
`previous` is the larger of the generation stored locally and the one the
receiver reports in status. It is monotonic across local rebuilds without a
persisted counter. A receiver refuses a generation more than 24 hours in the
future.

**Single projector queue.** One projector task per store serializes all work
for its own namespace: appends, rules and projector bumps, repricing, key
rotation and freezing. It captures a **witness** inside the ingest read
transaction (the `ProjectionTag` of every contributing source, plus the
activity projection's source generation). It then publishes the session in one
`asi.sqlite3` transaction, only if the stored witness is not newer (same or
older ledger epoch and offset for every source). Otherwise it discards the
result and re-queues. A delayed or concurrent result therefore never replaces a
newer one.

**Triggers:**

- **Append.** A source grows (`TagCheck::Advanced`): re-project that session.
- **Invalidation.** A source mismatches (`TagCheck::Mismatch`): the session's
  rows get `stale = 1` and remain queryable until re-projection publishes them
  again or they freeze. SQL never blocks on the projector; staleness is visible
  as data.
- **Rules or projector bump.** Indexed scans (`WHERE rules_version <> ?`,
  `WHERE projector_version <> ?`) queue stale, non-frozen sessions in bounded
  batches, following the `session_activity_projections` repair pattern.
- **Price bump.** Non-frozen rows are repriced from their stored billing
  inputs and the new `price` rows (`usd`, `carry_usd`, `price_version` and
  `price_status` only), following the existing repricing worker. Rules are not
  re-run.
- **Key rotation.** A new `path_key_id` re-projects every non-frozen session.
- **Raw unavailable.** A session whose Raw is neither native nor archived gets
  `frozen = 1`. Frozen rows are never changed again, including their price:
  they keep the versions they were computed under, and views can re-price
  them on read from `price`.

Labels are never rewritten by any of this: they reference UIDs, which do not
change.

### Public schema changes

Each file has one rule, in this order:

1. **A store holding any frozen rows refuses to open** under a new
   `asi_schema_version` (decided 2026-11-08). This applies to origins,
   receivers and Hub alike: replicated frozen rows carry `frozen = 1` too. The
   operator runs `semon asi export` (JSON Lines of every table, Parquet once
   export lands) and then `semon asi reset`. No migration or import code: after
   the reset, the export is an external record only.
2. **Otherwise the store recreates `asi.sqlite3`.** An origin re-projects
   everything its native Raw and archive still hold. A receiver or Hub store
   reports an empty status and origins re-push.

A receiver is a replica of its origins. If an origin machine is
decommissioned, the receiver's rows are the only copy, and they follow rule 1
once they freeze. Until then, the operator should export before upgrading.

The durable file has its own `durable_schema_version`: it refuses on
mismatch, and the operator exports and then resets explicitly. It is never
recreated implicitly.

## Raw archive

Whoever holds body custody keeps the archive: the origin for its own
sessions, a receiver for opted-in sources. On a receiver, the existing mirror
store already retains deleted files and seals them, and serves as the
archive. On an origin, the archive is new:

- **Objects.** `archive/{source_token}/{epoch}/{start}-{end}.zst` holds
  immutable copies of the byte range `[start, end)` of one Raw ledger epoch, in
  the zstd seekable format that `sealed` already writes and reads. It does not
  punch holes in or otherwise modify native files.
- **Manifest.** `archive/{source_token}/manifest.json` lists each epoch's
  segments, contiguous from 0, each ending on a line boundary, with the
  SHA-256 of the plain bytes and the `prefix_sha256` at each segment end.
  Segments are written to a temporary name, fsynced and renamed, and then the
  manifest is replaced by fsync and rename. On startup, files not named by the
  manifest are removed. A crash leaves the last published manifest.
- **Admission.** The archiver copies only the prefix the ingest index has
  admitted (the same consumed offset and digest as the `ProjectionTag`), so
  the archive and the projection agree on bytes.
- **Rewrites.** A new ledger epoch starts a new segment chain. Earlier epochs
  are kept, so generations already projected stay reproducible.
- **Reads.** When native Raw is gone, the ingest index reads the source from
  the archive by verifying segment hashes against the manifest. A hash mismatch
  is `archive_corrupt`: the source is unavailable and rows stay as they are
  (not frozen) until the operator intervenes.
- **Retention.** Kept indefinitely by default (decided 2026-11-08); the
  operator may set `archive.retention_days`. Withdrawal follows existing
  History custody rules: it never deletes native logs, and it marks affected
  rows `frozen`.
- `source.archive_coverage_from` and `source.archive_coverage_to` report what
  the archive covers.

## Durable state (`asi-durable.sqlite3`)

Tables: `label`, `adjudication`, `link`, `lane`, `work_item`, `change`,
`outcome`, `intervention` and `assignment`, as in methodology §1.1. Targets are
UIDs.

- **Classes.** Every column declares a class, as in the public schema. These
  columns are `body`: `label.mechanism_text`, `adjudication.reason`,
  `lane.title`, `intervention.hypothesis` and `intervention.guardrails`, and any
  later free-text field (for example, segment goal text in #546). Body columns
  never leave the machine without opt-in, `sql` cannot read them, and `view`
  reads them ([ASI query](asi-query.md)).
- **Custody.** WAL, `synchronous=FULL`, a single writer, opened only through
  `semon-asi`'s durable store type. Derived-cache rebuilds, schema refusals on
  `asi.sqlite3` and ingest rebuilds never open it for writing.
- **Append-only.** `label` and `adjudication` are insert-only, enforced by the
  API and by `BEFORE UPDATE`/`BEFORE DELETE` triggers that raise. Corrections are
  new rows. `link`, `lane`, `work_item`, `change` and `outcome` are upserts keyed
  by natural key, with `updated_at`. `outcome` stays `null` until its window
  closes.
- **Backup.** `semon asi backup PATH` uses the SQLite online backup API.
  Deletion is only an explicit operator action.
- **Hub.** Hub receives categorical labels as metadata through the label-push
  extension specified by #546, never body columns.

## Metadata-push wire contract

The push receiver (`semon receive`) and Hub serve two endpoints next to
`/v1/mirror/append`, with the same bearer tokens, TLS and authentication
rules ([mirror protocol](../mirror-protocol.md)). The token resolves to a
namespace (and, on Hub, a tenant workspace) before the body is read. The
wire format has no namespace field, so a request cannot name another
namespace.

### `GET /v1/asi/status?cursor=C`

Returns `{"asi_schema_version":1,"namespace":"…","path_key_id":"k1_…"|null,"sessions":[{"session":"{harness}/{native_session_id}","generation":N,"digest":"…","repush":false}],"next":C|null}`
for the token's namespace, paged at 5,000 sessions. The sender diffs this
against its own store and pushes sessions whose generation or digest differs,
or that are marked `repush` (failed staging, or an operator's
`semon receive asi repush --machine M`).

### `POST /v1/asi/rows`

One session snapshot, optionally split into pages. Every page repeats the
snapshot descriptor:

```json
{
  "asi_schema_version": 1,
  "snapshot": {"session": "codex/…", "generation": 1762600000000, "digest": "sha256:…", "pages": 2, "rows": {"turns": 40, "steps": 6100, "step_calls": 9050, "compactions": 2, "costs": 41}},
  "page": 0,
  "path_key_id": "k1_3f9a0c1d2e4b5a69",
  "session": {"…": "§1.1 session columns and versions (page 0 only)"},
  "session_sources": [], "turns": [], "steps": [], "step_calls": [], "compactions": [], "costs": [],
  "prices": []
}
```

`digest` is the SHA-256 of the snapshot's canonical serialization (the rows
in UID order, each column in schema order).

- **Commit and replace.** Pages are staged under `(namespace, session,
  generation)`. When every page has arrived and the row counts and digest
  match the descriptor, the snapshot replaces every row of that session in one
  transaction.
- **Idempotency.** A page whose descriptor and content equal a staged or
  committed one is acknowledged without effect, so lost acknowledgements and
  `repush` resends converge. The same generation with a different descriptor
  or page content is `conflicting_snapshot`. A newer generation discards older
  staging. Staging still incomplete after 24 hours is discarded and marked
  `repush`.
- **Validation.** All of it happens before any write, and a refused request
  leaves no effect:
  - Strict schema: unknown fields, missing required fields and wrong types are
    refused.
  - Enums must be in their closed sets. Numbers must be non-negative, finite
    and within column range. Timestamps must be integer milliseconds.
  - `name` columns: at most 256 bytes, no control characters, and must not look
    like an absolute or home path (leading `/`, `~` or `\`, a drive letter, or
    a `..` segment). Tool names must match `^[A-Za-z0-9_.:\-]{1,128}$`, and
    `repo_id` must match `^[a-z0-9.\-]+(/[A-Za-z0-9._\-]+)+$`.
  - `token` columns match their pattern (`^pt1_[A-Za-z0-9_-]{22}$` for paths),
    and `path_key_id` is required whenever path tokens are present.
  - Every row belongs to the snapshot's session.
  - Every `price_version` must be in `prices` or already stored.
  - The body is at most 6 MiB and a page at most 5,000 steps. Bytes count
    toward the machine's receiver quota.
- **Responses.** `200 {"committed":true|false,"generation":N}`. Errors are
  `{"error":code,…}`:

| Status | `error` | Meaning |
| --- | --- | --- |
| 400 | `invalid_rows` (with `path` to the first offending field) | Validation failed |
| 401 | — | Token unknown |
| 409 | `schema_mismatch` (with `asi_schema_version`) | Sender and receiver disagree; upgrade one side. No negotiation (pre-release). |
| 409 | `stale_generation` (with stored `generation`) | Older than the committed generation |
| 409 | `conflicting_snapshot` | Same generation, different content |
| 409 | `unknown_price_version` | Resend with `prices` |
| 413 / 507 | — | Size or quota, as for mirror |

The receiver never derives or repairs ASI rows. It stores what the origin
sends, under the origin's versions. Origins push a new generation whenever
they re-derive, and push everything again when a receiver's status comes back
empty. Senders push at most 4 sessions concurrently and back off on 5xx or
507, so a re-push after a reset does not flood the receiver.

### Sender behavior and body opt-in

`semon push` pushes ASI rows for every local session by default, and nothing
else. Without a bodies opt-in it sends **no** `/v1/mirror/append`,
`/v1/mirror/facts` or status heartbeat: today these carry `$HOME`, repository,
rollout and target paths (`crates/semon-push/src/lib.rs`, push heartbeat and
facts worker). Machine facts that ASI needs travel as ASI rows.

Raw, facts and status run only for sources the operator opts in, per harness
(`semon push --bodies claude --bodies codex`, or the push config), and they are
scoped to those sources. Opted-in Raw is redacted by the existing push-side
redaction. On the receiver it feeds only `asi-search.sqlite3` and L1 evidence
packets; it never produces `asi.sqlite3` rows. This replaces the current
Raw-only push default (pre-release, no compatibility mode). A CI test captures
the complete outbound HTTP exchange of a metadata-only push over the fixtures
and fails on any path, message, command or output text.

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
- **Canonical input.** For a path inside a repository with a remote-derived
  `repo_id`: `"r\0" || repo_id || "\0" || path relative to the checkout or
  worktree root`. Otherwise: `"a\0" || absolute path`, with the home directory
  written as `~`. Paths are lexically normalized (`.` and `..` resolved, no
  trailing slash) and not resolved through symlinks.
- **Token.** `pt1_` followed by the first 22 characters of
  base64url(HMAC-SHA256(key, input)), which is 132 bits.
- **Rotation.** Importing a new key changes `path_key_id`, re-projects
  non-frozen sessions and re-pushes them. Frozen rows keep their old key ID.

`session_source.source_token` and archive directory names use the same
tokenization.

### Repo identity

`repo` stays the existing display name (`repo_of`: the checkout's basename).
`repo_id` is the stable identity: the `origin` remote URL with scheme,
credentials, port and a trailing `.git` removed, and the host lowercased:
`github.com/onsager-ai/semon`. Linked worktrees resolve to the main checkout's
remote. A repository without a remote has `repo_id` `null`, and its paths use
the absolute form, so unrelated repositories that share a basename never share
tokens.

## Retention

- `asi.sqlite3` and `asi-durable.sqlite3`: no automatic deletion. The 60-day
  `analytics::KEEP_MS` window bounds only the existing Analytics input
  projection until Analytics is rebuilt on this store (#547); it does not
  apply here.
- The Raw archive is kept indefinitely by default (see
  [Raw archive](#raw-archive)).

## Crate boundary: `semon-asi`

`semon-asi` is a library crate with no dependency on `semon-sessions` or any
native parser:

- schema definition (tables, columns, classes, enums, documentation), which
  emits the DDL, the validator rules, the query allowlist and the generated
  schema reference;
- row and wire types (serde, `deny_unknown_fields`), validation, snapshot
  digests, and the push-ingest function `ingest(&NamespaceCapability, Page) ->
  Result`;
- path tokenization, key IDs and `repo_id` normalization (the key file itself
  is managed by the CLI);
- public store: open and version fence, frozen refusal, conditional
  publication, version scans, repricing, freeze, export and reset;
- durable store;
- the query engine ([ASI query](asi-query.md)).

`semon-sessions` depends on `semon-asi`. It keeps native parsing, the ingest
index, the activity projection, `StepFacts`, the Raw archive, the body indexer
and the ASI projector, which maps ingest state to `semon-asi` row types.
`semon-push` hosts the HTTP endpoints and calls `ingest`. Hub depends on
`semon-asi` only, for ingest, store and query per tenant workspace, mapping its
own authentication to a namespace capability.

## Budgets

Proposed defaults, to be measured under #510:

- incremental projection of an appended step: under 5 ms;
- ingest of a 5,000-step page on the CI runner: under 500 ms;
- `asi.sqlite3` growth: under 2 KiB per step;
- the existing 2 s cold first-page gate stays green.

## Tests (CI only)

- Golden public rows for the sanitized harness fixtures (`asi-golden.json`),
  covering every §1.1 column the fixtures exercise.
- Identity: rebuilding gives identical UIDs and rows; encoding round-trips
  components containing `/`, `:` and `%`; turns and compactions without native
  IDs use source anchors; UIDs are equal on origin and receiver; a receiver
  refuses a token named after itself or `local`.
- Publication: a delayed projector result never replaces a newer witness;
  concurrent append, rules bump and repricing converge; invalidation sets
  `stale`.
- Version scans: a rules bump re-derives only rows with a stale
  `rules_version`; a price bump touches only cost columns and skips frozen
  rows; counts are asserted.
- Archive: pruning native Raw leaves rows re-derivable; a rewrite keeps the
  earlier epoch; a crash mid-copy leaves the last manifest; corruption is
  `archive_corrupt`, not frozen.
- Freeze and schema change: withdrawing the archive freezes rows; a schema
  bump with frozen rows refuses on origin and receiver; export then reset
  proceeds; without frozen rows, a receiver recreates and origins re-push.
- Durable custody: the durable file's bytes are unchanged across rebuilds;
  updating or deleting a label raises; every text column has a class.
- Wire: every refusal in the table, with no partial effect; a token for
  machine A cannot write machine B; paged staging; a lost acknowledgement and
  a `repush` resend converge; a conflicting snapshot is refused; stale
  generation; generation stays monotonic after an origin rebuild.
- Privacy: path-token known-answer vectors; same-name unrelated repositories
  never share tokens; worktrees and renamed clones of one remote do share
  them; a full outbound-exchange scan of a metadata-only push finds no path,
  message, command or output text.
- Namespace conformance cases from `storage/conformance.rs` keep passing.
