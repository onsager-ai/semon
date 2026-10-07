# The derived store: persisting what Semon derives from the logs

Status: historical design, reconciled for #320. The event SQLite store, incremental ledgers and in-memory session reuse are implemented; the first persisted description slice is documented in [incremental-model.md](incremental-model.md). The remaining Phase B/shadow-serving plan below is proposed, not a current implementation inventory. The Product Foundation task authorizes the scoped implementation work; this design grants no additional scope.

Read at `c4ec3f5`, with #104 (refresh pool) and #105 (server-side analytics) read from their open branches. Claims marked *measured* were measured on 2026-09-29; claims marked *inference* were not.

## The recommendation in brief

- **The raw logs stay the source of truth.** The derived store is a cache: rebuildable from the logs at any time, deletable at any time, and never holding message text. Recovery from anything is "rebuild from raw".
- **One SQLite file per machine view,** `sessions-index.sqlite3` beside today's `sessions-index.json` (so under `$XDG_STATE_HOME/semon/`). It replaces `sessions-index.events.json` and later most of what `build()` recomputes on every change.
- **It stays separate from `traces.sqlite3`.** That store keeps raw carrier bytes forever under the forensic policy; this one keeps metadata and offsets and is dropped and rebuilt on every version bump. Sharing a file would couple those lifecycles and put the forensic boundary at risk.
- **Two derivation phases.** Phase A is per file and strictly incremental per line (the event index, as today). Phase B is per lineage: sessions, handoffs, turns and analytics rows are recomputed only for the lineages a change touches, found through a persisted link-key table.
- **Crash consistency is one rule:** a file's offset never commits without the rows derived from the lines before it. Phase B work is queued in the same transaction that advances the offset.
- **Nothing that depends on the clock or on liveness is persisted.** "Live", "working", ages and offline state are computed when serving, from invariant parts the store keeps.
- **Serving:** `/api/model` keeps its shape and window, but its snapshot is loaded from the store (the window plus live sessions) instead of built from the whole history. `/api/tx` is computed per session on demand. `/api/analytics` and the query tools read SQL over any range.
- **Versions:** the schema (`user_version`) and each derivation layer (parser, model, action rules, detectors) carry versions. A bump rebuilds the affected layer, from stored events where it can and from the logs where it must, into a shadow file while the old one keeps serving.
- **Seven PRs** (a–g). Step (a), the event index moved from JSON to SQLite, ships alone and removes the 84 MB whole-file rewrite.

## What exists today

*Measured from the code.*

- **Inputs.** `inputs.rs` lists the files the builder reads: Claude `projects/**/*.jsonl`, subagent `*.meta.json`, `sessions/<pid>.json`, and Codex `sessions/**/*.jsonl`, plus `~/.claude.json`'s allowlisted last-run fields and `/proc`.
- **The event index** (`events.rs`). `scan_file` keeps a per-file `FileIndex`: events and extras (kind, line offset, block, time, ids, tool reply flags), fold state (`pending`, `tool_ids`, `yields`), busy intervals, lineage links (`session_ids`, `continued_in`, `bridges`), usage by message id, Codex usage deltas, rate limits, title and cwd. It resumes a file from its byte offset when the file only grew and its `(dev, ino)` is unchanged, and rescans it from 0 otherwise. It never holds text (risk:secret).
- **Its cache** is `EventCache`, one JSON file (`sessions-index.events.json`, `CACHE_VERSION` 13). It is read whole at startup and rewritten whole, at most every 30 s while dirty. It also accumulates `reported_runs` from `~/.claude.json`, which holds only each project's *last* run, so those snapshots **cannot be rebuilt from the logs**.
- **The model** (`model.rs`). `build()` rebuilds the whole `Built` on every change: sessions, handoffs, turns, busy intervals, each session's transcript slot index (`tx`), `SessionFacts`, and (with #105) the 60-day `analytics::Activity` rows. Several passes compare every session with every handoff. Text (briefs, results, answers, prompts) is read back from the source line by offset into a 32 MB in-memory cache.
- **Serving** (`viewer.rs`, `union.rs`). A `MachineView` answers from an `Arc` snapshot; a rebuild holds `work`, stats every input file (`Snapshot::changed`), rebuilds, and swaps the snapshot. `ViewerCore` unions several machines. #104 moves background checks onto a bounded pool and adds `invalidate(key)` for an embedding server that knows when a machine's files changed.
- **`traces.sqlite3`** (`semon-store`, schema v5, `$XDG_DATA_HOME/semon/`). A content-addressed trace store written by the `semon-claude` and `semon-codex` adapters. Its forensic region holds every source line verbatim and is retained indefinitely (`forensic-retention-and-exposure.md`). The viewer never reads it.

**What that costs** (*measured* 2026-09-29 on the maintainer's laptop, sizes only):

- `sessions-index.events.json` is **84 MB**. Every save rewrites all of it. At one save per 30 s while any session is active, that is up to about 240 GB of writes a day (*inference*: the save interval times the size).
- Parsing it is heavy: a `jq` pass over it, niced, was stopped by the thermal gate at 98 °C after half a second. The viewer parses it at every start, then keeps every file's `FileIndex` in memory for good.
- The laptop's logs are about 2 GB (*inference*, from an earlier estimate), so the event index is about 4% of the raw bytes.
- A warm rebuild of the model took 185–303 ms for 519 files when #94 measured it. It grows with every session ever logged, because `build()` covers all of them.

Nothing derived survives a restart except the event index. None of this reaches 100k sessions.

## Invariants

These hold for every step below. A PR that breaks one is wrong, whatever else it does.

1. **Derived = f(logs, facts, versions).** Every row in the store can be recomputed from the input set, the machine's facts and the derivation versions. Deleting the file loses nothing but time. The one exception, `reported_runs`, is marked as observed and carried across rebuilds (see [Versioning](#5-versioning)).
2. **No text.** The store holds kinds, offsets, times, ids, counts, flags, labels and hashes: what the JSON cache holds today. Briefs, results, answers, prompts, tool input and output are read back from the source line by `(file, offset, block)` and kept in memory only. This keeps the store as safe to copy, inspect and back up as today's cache, and keeps it structurally outside the forensic policy.
3. **Nothing time- or liveness-dependent.** A value that changes without a log line changing (a tool call being live, a session working or idle, a machine offline, an age in seconds) is never stored. The store keeps its invariant parts, and serving applies the rule.
4. **Incremental equals full.** For any sequence of appends, rewrites and deletions, the store after incremental updates equals the store built from scratch over the final logs. The full rebuild is the oracle, and it stays in the code.
5. **Offsets never run ahead of rows.** A file's resume offset commits in the same transaction as every row derived from the lines before it.
6. **The logs are only read.** The store never writes into an agent home.

## 1. What to persist

| Layer | Persisted | Why |
|---|---|---|
| **File ledger** | Path, root, harness, `(dev, ino)`, size, mtime, resume offset, SHA-256 of the complete consumed prefix and of the 4 KiB before the offset, line count, parser version, and the fold state (`pending`, `yields`, the open code-mode call) | Resuming a file needs exactly this; today it lives in `CachedFile` and `FileIndex` |
| **Events and extras** | Every `Event` field as columns, keyed `(file, extra, seq)`, updated in place when a tool result resolves an earlier call | The per-line product of phase A; everything else derives from it |
| **Per-file facts** | Links, entrypoint, title, agent name, first and last time, cwd, branch, last model, failed flag, usage by message id, Codex usage deltas, rate limits, busy intervals | Today in `FileIndex`; needed by phase B without reading the file |
| **Link keys** | Every logged id a file defines or refers to: `session_id`, `continued-in`, bridge, Codex marker and `parent_thread_id`, agent id, tool-use id, `msg_id`, spawn task name | Finds the lineages a change affects without scanning every file ([§4](#4-incrementality)) |
| **Lineages and sessions** | Lineage membership per file; per session its key, harness, kind, lane, stub, role, machine-independent fields, model, start and last, tokens and tokens by model, cost, calls, final errors, and pending calls split into "in the last turn" and "elsewhere" | Recomputed per lineage, not per build |
| **Handoffs and turns** | Every `Handoff` and `Turn` field except text; text fields become refs (`brief_ref`, `result_ref`, `answers_ref`, a `u` turn's `text_ref`) | Recomputed with the lineage that owns them |
| **Analytics rows** | Per session and UTC day: agent ms, turns started, calls, errors, cost by model, unpriced models; busy intervals per session | Replaces #105's 60-day in-memory rows, for any range |
| **Intelligence** (later) | An action label and rule-set version per event; per-session features, outcome rung and flags, each with its detector id, version and evidence refs | The Session Intelligence tiers 0 and 1 |
| **Observed, not derived** | `reported_runs` from `~/.claude.json` | The source keeps only the last run per project |

**Not persisted:**

- **Transcript slots (`tx`).** A slot index is about one row per event, it depends on liveness (`Shown::Live`), and a slot's position shifts as turns change. It is computed per session on demand from that session's stored events and kept in a bounded in-memory cache ([§6](#6-serving)).
- **Text** of any kind (invariant 2), and the text cache.
- **Liveness and time:** `/proc` starts, writer locks, pid records' status, facts' online state, `activity[2]` ages, session state (`work`, `wait`, `idle`), `Shown::Live` versus `Unfinished` (invariant 3). The store keeps what they are computed from: a session's pending calls by turn, its pid record's pid and recorded start, its last time.
- **The served JSON.** It stays an in-memory string built once per snapshot, as today.

**Does the snapshot become a cache over the store?** Yes. `Built` stops being "everything ever logged" and becomes the served window: sessions active in the window or live, the handoffs that name them and the sessions at their other ends, and their turns. It is loaded from the store after each committed change. Its JSON, version and `ETag` rules are unchanged.

**Evidence refs.** The Session Intelligence proposal writes evidence as `(machine, file, offset, slot)`. A slot's ordinal isn't stable (it shifts when a session's turns change) and slots aren't persisted, so the store's ref is `(file, offset, block)`, which never changes while the file only grows. The machine is implicit in which store the row is in. A slot is found from a ref at serve time.

## 2. Where

### Local: SQLite, one file per machine view, separate from `traces.sqlite3`

The path is derived from `Options::cache` as the event cache's is today: `sessions-index.json` becomes `sessions-index.sqlite3`. A machine read through `Options::facts` (received machines, an embedding server's mirrors) gets its own file next to its own cache, with no new option. The file is created `0600` in a `0700` directory and re-tightened on open, as `semon-store` does. It uses `journal_mode = WAL`, `synchronous = NORMAL` (a power loss may lose the last transaction, never consistency), and `STRICT` tables.

**Why not merge with `traces.sqlite3`:**

| | `traces.sqlite3` | Derived store |
|---|---|---|
| Content | Raw carrier bytes, text, canonical traces | Metadata, offsets, ids, counts; no text |
| Retention | Everything, indefinitely (forensic Decision 1) | A cache: dropped and rebuilt on a version bump |
| Boundary | Ordinary reads must be structurally unable to reach raw rows (Decision 4) | Nothing raw exists to reach |
| Writer | The capture adapters, on a timer | The viewer's machine view, on each change |
| Location | `$XDG_DATA_HOME` (data) | `$XDG_STATE_HOME` (state, deletable) |

A shared file would mean a version bump either can't drop tables freely or risks the forensic rows, and every derived table would widen what the forensic boundary test has to prove unreachable. Keep them apart; share conventions (bundled `rusqlite`, `user_version` checks that refuse a newer schema, owner-only permissions). Whether the capture pipeline itself should later read action labels from the derived store, or be retired, is a product question ([open questions](#open-questions)).

**Code location.** `semon-sessions` owns the builder and must own the store: `semon-store` depends on `semon-sessions`, so the reverse would be a cycle. `semon-sessions` gains the `rusqlite` (bundled) dependency the Cargo workspace already has and a `derived` module. No trait is needed: every consumer, an embedding server included, uses this SQLite store through `Options`.

## 3. Schema sketch

The historical SQL column name `head_sha256` now stores the complete consumed
prefix hash in the implemented event store (parser version 19 and later), not
just a 4 KiB head marker. `events::consumed_hashes` hashes bytes `[0, resume_at)`;
`tail_sha256` covers at most 4 KiB before that boundary. Neither establishes an
archive object's full generation when bytes remain after `resume_at`; hosted
archive access must bind its own immutable object generation separately.

Types and columns are indicative; step (a) and (b) PRs fix them. All tables are `STRICT`; wide keyed tables are `WITHOUT ROWID`.

```sql
-- Versions and the change counter.
meta(key TEXT PRIMARY KEY, value TEXT)          -- parser, model, rules, detectors, generation

-- Phase A: per file.
files(file_id INTEGER PRIMARY KEY, path TEXT UNIQUE, root TEXT, harness TEXT,
      dev INTEGER, ino INTEGER, size INTEGER, mtime_ns INTEGER, offset INTEGER,
      head_sha256 BLOB, tail_sha256 BLOB, lines INTEGER, parser INTEGER,
      fold BLOB,                                 -- pending, yields, open code-mode call
      lineage_id INTEGER, first_ms INTEGER, last_ms INTEGER, cwd TEXT, branch TEXT,
      entrypoint TEXT, title TEXT, agent_name TEXT, last_model TEXT, failed INTEGER)
events(file_id, extra, seq, kind, offset, block, t, id, name, flags, code_mode,
       parent_seq, poll_seq, script, reply_offset, reply_block, reply_t, reply_error,
       reply_flags, reply_msg_id, item_offset, peer_sock, peer_pid, peer_start, uuid,
       action, action_rules,                     -- step (e)
       PRIMARY KEY (file_id, extra, seq))
event_yields(file_id, seq, polls BLOB, cut, done_offset, done_block, done_t, done_flags,
             PRIMARY KEY (file_id, seq))
link_keys(kind, key, file_id, role,              -- role: defines | refers
          PRIMARY KEY (kind, key, file_id, role))
usage(file_id, message_id, model, input, output, cache_read, cache_write_5m,
      cache_write_1h, web_search, speed, tier, prompt_size, t, split_unknown,
      PRIMARY KEY (file_id, message_id))
codex_usage(file_id, seq, model, input, output, cache_read, cache_write, t)
rate_limits(file_id PRIMARY KEY, recorded_at, windows TEXT)
file_busy(file_id, start_ms, end_ms)

-- Phase B: per lineage.
lineages(lineage_id INTEGER PRIMARY KEY, model INTEGER, generation INTEGER)
dirty_lineages(lineage_id INTEGER PRIMARY KEY, since_generation INTEGER)
sessions(key TEXT PRIMARY KEY, lineage_id, harness, kind, lane, stub, role, parent,
         repo, branch, model, start_ms, last_ms, tokens_in, tokens_cached, tokens_out,
         cost_usd, calls, errors_final, pending_last_turn, pending_other,
         pid, pid_start, generation)
session_models(key, model, input, output, cache_write, cache_read, PRIMARY KEY (key, model))
handoffs(id TEXT PRIMARY KEY, lineage_id, kind, ask, from_key, to_key, target, at, done,
         status, declined, unmatched, ambiguous,
         brief_file, brief_offset, brief_block, result_file, result_offset, result_block,
         answers_file, answers_offset, answers_block)
turns(id TEXT PRIMARY KEY, sid, start_handoff, u, at, end_st, end_why, end_h,
      file_id, offset, last, text_file, text_offset, text_block)
turn_sent(turn_id, handoff_id, PRIMARY KEY (turn_id, handoff_id))
session_days(key, day, agent_ms, turns, calls, errors, cost_usd, unpriced,
             PRIMARY KEY (key, day))
session_busy(key, start_ms, end_ms)

-- Observed, carried across rebuilds.
reported_runs(session_id, start, cost_usd, duration_ms, api_ms, tool_ms, lines_added,
              lines_removed, by_model TEXT, capture_at, PRIMARY KEY (session_id, start))

-- Step (f).
session_features(key, detectors INTEGER, features TEXT, rung INTEGER, PRIMARY KEY (key))
flags(key, detector, detector_version, evidence TEXT, PRIMARY KEY (key, detector, evidence))

-- Indexes the serving and incremental paths need.
CREATE INDEX sessions_by_last ON sessions(last_ms);
CREATE INDEX handoffs_by_from ON handoffs(from_key, at);
CREATE INDEX handoffs_by_to ON handoffs(to_key, at);
CREATE INDEX turns_by_session ON turns(sid, at);
CREATE INDEX session_days_by_day ON session_days(day);
CREATE INDEX link_keys_by_file ON link_keys(file_id);
CREATE INDEX events_by_id ON events(id) WHERE id IS NOT NULL;
```

## 4. Incrementality

### The per-line append path (phase A)

For each file the change check names ([below](#finding-what-changed)):

1. Read its ledger row. If `(dev, ino)` differs, the file is shorter than the offset, or `head_sha256` or `tail_sha256` no longer match the file, it was **rewritten**: go to [invalidation](#invalidation).
2. Otherwise load its fold state and the events it can still change: the pending tool calls, the open yields, the open code-mode call. That is bounded (`YIELDS_MAX`, `POLLS_MAX` and the calls still waiting), not the file's history.
3. Parse the complete lines past the offset with today's `claude`/`codex` fold functions, unchanged. They see a `FileIndex` holding only what step 2 loaded plus the new events; new events get `seq` values after the file's last.
4. Write, in one `BEGIN IMMEDIATE` transaction: new and changed event rows, per-file facts, changed link keys, the new offset, size, mtime, hashes and fold state, and the file's lineage (plus the lineages at the other end of any link key it added or removed) into `dirty_lineages`. Bump `generation`.

The transaction re-reads the ledger's offset after `BEGIN IMMEDIATE` and applies only lines past it, so two writers never double-apply ([§7](#7-concurrency)).

The fold functions are the parser. They stay the only code that turns lines into events, so the equivalence of the persisted and in-memory paths is a property of the plumbing, not of a second parser.

### Cross-file links (phase B)

Every link Semon makes joins on a logged id ([model.rs's rule](../../crates/semon-sessions/src/model.rs)). That is what makes an affected set computable:

- **Lineage (union-find).** Top-level files are unioned by `session_id`, `continued-in` and bridge ids. An append can only add links, so lineages only merge. A rewrite can remove a link and split one. So phase B recomputes the *connected component* of each dirty lineage from `link_keys` (a small graph walk), never an incremental union-find with deletes, and assigns lineage ids.
- **Spawns, relays, asks and questions.** A handoff joins a file that refers to an id (a tool-use id, a `msg_id`, an agent id, a Codex marker or `parent_thread_id`, a task name) with the file that defines it. When either side changes, `link_keys` names the other, and both lineages are dirty.
- **Stubs.** A handoff whose far end isn't in the logs gets a stub session, owned by the lineage that refers to it. When the far end's file appears, its `defines` key finds the referring file, that lineage is recomputed, and the stub goes.
- **Ambiguity.** "Several calls or runs share this task name" is decided today by counting over all sessions. With `link_keys` it is a count over one key: all `refers` and `defines` rows with that task name. A change to any of them dirties all of them.
- **Copy-resume de-duplication** (`copied_events`) compares files within one lineage only, so it recomputes with the lineage.

Phase B then, per dirty component: deletes its lineages' sessions, handoffs, turns, analytics and feature rows; runs the builder's passes over exactly those files, with lookups outside the component answered from the store by key; writes the new rows; and clears the queue entries. This is the largest change in the plan. Today's passes loop over all sessions; they must become per-component passes with keyed lookups, not the old passes wrapped in a filter, or they stay O(sessions × handoffs).

### Invalidation

A file is **rewritten** when its `(dev, ino)` changed, it is shorter than its committed offset, or the hash of its first 4 KiB or of the 4 KiB before the offset changed. `head_sha256` matches the mirror protocol's `head_sha256` over the same 4 KiB, so a receiver's rewrite detection and the store's agree. A rewritten file loses all its rows (events, facts, link keys) in the transaction that rescans it from 0, and its old and new lineages and their link-key neighbours are dirty. A rewrite that keeps the length and both hashed windows is not detected; the harnesses never do that (*inference*), and the logs are appended, not edited.

A **deleted** file loses its rows and dirties its neighbours, as `EventCache::retain` drops it today. Keeping the metadata of deleted logs is an [open question](#open-questions).

The same-size, new-mtime case that today forces a full rescan becomes a hash check: when both windows still match, the file resumes from its offset.

### Crash consistency

- Phase A commits a file's offset together with its rows and its `dirty_lineages` entries. A crash before the commit re-reads those lines; a crash after it leaves the queue.
- Phase B commits per component. At startup, a non-empty `dirty_lineages` is drained before the first snapshot.
- The snapshot is loaded only from committed state, so it never mixes a half-applied change.

### Finding what changed

Today every check stats every input file (`Snapshot::changed`), every 250 ms in the background mode. That is O(files) and grows with history. With the ledger:

- **The hot set** is stat'ed on every check: files modified in the last hour, files with pending calls or open yields, and each input directory (a new file changes its directory's mtime; an append doesn't).
- **A full sweep** runs every 60 s and at startup.
- **An embedding server that knows which file changed** passes it with `invalidate`. That extends #104's `invalidate(key)` to name paths, so the check reads those files and skips the sweep.
- `/proc`, locks and facts stay as today: they are liveness, not log data.

## 5. Versioning

| Version | Where | A bump means |
|---|---|---|
| Schema | `PRAGMA user_version` | Table shapes changed. Additive changes migrate in place; anything else rebuilds the file. A binary that finds a newer schema doesn't touch the file and runs in memory only, with one warning, so two installed versions never rebuild each other's store in turn. |
| Parser | `meta.parser` (today's `CACHE_VERSION`, continuing at 14) | Events must be re-read from the logs: every file rescans. |
| Model | `meta.model` | Phase B rules changed: every lineage is recomputed from stored events. No log is read. |
| Action rules | `meta.rules`, and `events.action_rules` | Labels are recomputed. Labels need command text and paths, which aren't stored, so only tool-call lines are read back by offset. |
| Detectors | `meta.detectors`, and each flag's `detector_version` | Features and flags are recomputed from stored events and labels. |

**Shadow rebuilds.** A bump that needs the logs (schema or parser) builds `sessions-index.sqlite3.new` while the old store keeps serving with the old rules. The new file is renamed over the old one when complete. A bump that doesn't need the logs rebuilds tables in place, in batches, marking the lineages it has done. Either way the viewer never goes cold, where a `CACHE_VERSION` bump makes it go cold today.

**Rebuild from raw** is the recovery path for everything: `semon sessions --rebuild-index` (or deleting the file). Rows carry no version-dependent meaning after a rebuild, because everything is recomputed. The exception is `reported_runs`: a rebuild copies them across, since their source keeps only the latest run.

**Migrations** of derived tables are therefore rare by design. Prefer a model-version bump, which rebuilds from events in seconds, to an in-place data migration.

## 6. Serving

- **`/api/model`.** After each committed change, the view loads the served window from the store: sessions with `last_ms >= cutoff` or pending or live state, the handoffs naming them, the sessions at the other ends, and their turns. It then applies liveness from `/proc` and facts, reads text back by ref (through the existing text cache), serializes once, and swaps the `Arc`. The JSON, the `version` hash rule (content plus source lengths) and the `ETag` stay as they are. A client can't tell the difference.
- **`/api/tx` and `/api/entry`.** A session's transcript is computed on demand from its lineage's stored events, with liveness applied, and kept in a bounded LRU keyed by `(session, lineage generation)`. The served window's transcripts are precomputed, as today, so the hot path doesn't change. `calls` and `errors` in the model come from the session row: `errors = errors_final + pending_other + (pending_last_turn if the session isn't working)`.
- **`/api/analytics`.** SQL over `session_days` and `session_busy` for any range, not only the 60 days #105 keeps in memory. #105's answer cache per range and filters stays.
- **Query and MCP tools.** `list_sessions`, `get_session`, `find` and `stalls` take their window as a query predicate. `Options::scan_window` goes, and so does its limitation: a link whose other end is in an older file resolves, because that file is in the store.
- **A paged session list** (`/api/sessions`) is possible once this lands, for an "All sessions" view over the whole history. It isn't part of this plan.
- **Hot in memory:** the served snapshot and its JSON, the text cache, the transcript LRU, liveness. Nothing else stays resident; today every file's `FileIndex` does.
- **Union.** `ViewerCore` keeps unioning per-machine snapshots in memory. Analytics over several machines sums per-machine query results, with #105's first-wins rule for a shared session id.

## 7. Concurrency

- **One writer per store.** The writer connection lives in the view's `Work`, which is held for a whole refresh already. No new lock enters the order (work, files, shown, live, then #104's pool queue); the SQLite transaction is always opened and closed inside `work`, and never waits on another of these locks while it is open.
- **Several processes.** `semon sessions --serve`, `semon query` and `semon mcp` can run at once over one machine's store. Each write is `BEGIN IMMEDIATE` with a `busy_timeout`, and applies only the lines past the committed offset it re-reads inside the transaction, so a second writer finds the work done and writes nothing. A reader that finds the store behind the logs (a writer holds it for longer than its timeout) folds the missing tail in memory for that answer and doesn't persist it.
- **Readers.** Requests read the in-memory snapshot. Cold paths (a transcript outside the window, analytics, queries) use a small per-view pool of read-only connections (two by default). WAL gives each query one consistent view while the writer commits.
- **Refresh pool (#104).** A pool worker's check becomes: find what changed, phase A, phase B, reload the snapshot. The pool's thread bound is unchanged; the work per check shrinks to what changed.
- **Checkpointing.** SQLite's automatic checkpoint (1,000 pages) is kept; after a rebuild the writer runs `wal_checkpoint(TRUNCATE)` so the WAL doesn't stay at the rebuild's size.
- **Filesystem.** WAL needs shared memory, so the store must be on a local filesystem, not a network mount. `Options::cache` is already a local state path.

## 8. Cost

All figures are *inference* from the structs and the measured sizes above, until step (a) reports real ones (`--index-stats`).

**Disk.** An `Event` row costs about 30 B (text or prompt event) to 110 B (a tool call with its id, name and reply), plus about 40 B for each tool event in the `events_by_id` index. The JSON cache spends a similar amount per event with its one-letter keys, so the store should be about 1 to 1.5 times the JSON cache: **about 85–130 MB for the maintainer's laptop, or 4–6% of the raw logs.** Per session: a Codex rollout averages about 455 lines (*measured* 2026-09-20: 128,890 lines over 283 rollouts), which is roughly 150–300 events, so 10–35 KB of events, plus 3–5 KB of session, handoff, turn and analytics rows. That is **about 15–40 MB per 1,000 sessions, and 1.5–4 GB at 100,000.**

**Rebuild from raw.** It is the same parse as today's `CACHE_VERSION` bump: about 2 GB of JSON at roughly 150–300 MB/s on one core, plus about a million row inserts in batched transactions. That is **10–20 s**, on one thread and niced, because of the thermal limit.

**Startup.** It opens the file, drains `dirty_lineages` (normally empty), and loads the window: tens to hundreds of sessions and their files' events. That is **tens to low hundreds of milliseconds**. Today's startup instead parses 84 MB of JSON and builds the whole model.

**Per change.** A commit writes the changed pages: kilobytes, not 84 MB.

**Memory.** It is the window's snapshot plus the caches, not every file's index.

## 9. The plan in PR-sized steps

Each step ships alone, keeps `/api/model` byte-identical on the fixtures, and has the full rebuild as its test oracle. Routing follows the project's implementer rules: storage consistency, multi-process locking and reader pools are concurrency work, so they go to Opus. Local log parsing and aggregation, with no HTML sink or network input, are eligible for the Sonnet 5.5 pilot.

| Step | What | Test that decides it | Routing |
|---|---|---|---|
| **(a)** Event index in SQLite | The file ledger, events, per-file facts, fold state, `reported_runs` and link keys move from `sessions-index.events.json` to `sessions-index.sqlite3`, with per-file transactions, head and tail hashes, WAL and `0600`. At startup the whole index is still loaded into `FileIndex`es, so `build()` doesn't change. The old JSON file is deleted after a successful import of its `reported_runs`. `--index-stats` prints row counts and sizes. | Model JSON identical to the JSON-cache build on every fixture; appends, rewrites, truncation, same-size rewrites and deletion; a crash between parse and commit (a failpoint) re-reads and ends identical; two processes writing at once end identical. | **Opus** (crash consistency, multi-process writer) |
| **(b)** Phase B persisted | Lineages, sessions, handoffs, turns and `turn_sent` as rows with text refs; `dirty_lineages`; per-component recompute with keyed lookups. `build()` becomes phase B plus a load of every session, so serving doesn't change yet. | A property test: random interleavings of appends, rewrites and deletions over the fixture corpora, and incremental equals full after every step (invariant 4). Stubs appear and resolve. Ambiguity counts are right. | **Opus** (the subtle invariants) |
| **(c)** Serve from the store | The snapshot becomes the window loaded from the store; transcripts on demand with an LRU; the read-only connection pool; the hot-set change check; `invalidate` with paths; `scan_window` retired. | `/api/*` byte-identical on the fixtures; a session outside the window served by `/api/tx`; a bounded LRU; stat calls per check bounded by the hot set (a counting hook, as #104's). | **Opus** (reader pool, change detection) |
| **(d)** Analytics rows | `session_days` and `session_busy`; `/api/analytics` over SQL for any range; #105's in-memory `Activity` goes. | #105's tests pass unchanged; a 90-day range over a fixture with older sessions. | Sonnet 5.5 pilot (local aggregation; query parsing unchanged) |
| **(e)** Action labels (Tier 0) | `events.action` and `action_rules`; capture compaction, interrupt and permission-denied events (a parser bump). | Labels per fixture tool call; a rules bump reads back only tool-call lines (a counting hook). | Sonnet 5.5 pilot (local log parsing) |
| **(f)** Features, rungs and flags (Tier 1) | `session_features` and `flags` with evidence refs and detector versions; a `session_flags` query and MCP tool. | Each detector against hand-built fixtures; evidence refs resolve to the right step; a detector bump recomputes without reading logs. | Sonnet 5.5 pilot |
| **(g)** Shadow rebuilds | Builds `.new` while serving on a schema or parser bump, then swaps it in; `--rebuild-index`; the newer-schema refusal. | Reads keep answering during a rebuild; the swap is atomic; `reported_runs` survive it. | **Opus** (two stores, atomic swap under load) |

Order: (a) → (b) → (c) are the spine; (d) needs (b); (e) needs only (a) and can go in parallel with (b); (f) needs (e); (g) can follow (a) at any time.

## 10. Risks, and what not to do

**Risks:**

- **Incremental and full drift apart.** The builder has subtle global rules (copy-resume de-duplication, task-name ambiguity, stub spans, the parent rule). Mitigation: invariant 4 as a property test in (b), and a niced idle check that rebuilds a sample of lineages from scratch and logs any difference.
- **Harness retention.** Claude Code deletes transcripts after its cleanup period (30 days by default, *unverified here*). Because derived rows follow their source, long analytics ranges shrink to what the harness kept. See the open questions.
- **The first build after upgrading** is a full parse, as a `CACHE_VERSION` bump is today. It runs niced, on one thread, in the background (g), committing in batches.
- **Dependency weight.** Bundled SQLite adds C compilation to every build of `semon-sessions` and of anything embedding it. That is CI time only.
- **WAL on a network filesystem** would corrupt or deadlock. The store refuses to open in WAL mode where SQLite reports it can't, and falls back to in-memory with a warning.

**What not to do:**

- Don't store text, not even capped text. A store with text is a second copy of the transcripts, with its own retention and exposure questions.
- Don't store anything that changes with the clock or with liveness.
- Don't merge with `traces.sqlite3`.
- Don't keep user-authored or otherwise non-rebuildable state (a finding's triage status, an approval, a note) in the derived store: a rebuild would erase it. That state needs a store of its own when it arrives.
- Don't dual-write the JSON cache and the store. Each step switches over once, behind its equivalence test.
- Don't write in-place data migrations for derived tables; bump a version and rebuild.
- Don't wrap today's global builder passes in a filter and call them incremental; they must look up by key.
- Don't widen `/api/model` to carry history. Aggregates and history get their own endpoints.

## Open questions

These are Marvin's to answer; everything else above is a recommendation that can change in review.

1. **Deleted logs.** Should the store keep the metadata of a log file the harness has deleted (marked `source_gone`), so analytics and history outlive the harness's retention? That breaks invariant 1 for those rows (they can't be rebuilt), and turns the cache into an archive. The recommendation is no: follow the source.
2. **The capture pipeline.** `traces.sqlite3` and its adapters are a parallel pipeline the viewer never reads. Should they stay as they are (the recommendation here), later read action labels from the derived store, or be retired? This is a positioning decision more than a storage one.
3. **Order against the Session Intelligence MVP.** The proposal's MVP says "no new store": labels on the JSON cache, flags in `SessionFacts`. The recommendation is to land (a) first, so labels go into the store (e) rather than into one more version of the 84 MB JSON file, and keep Tier 1 in memory until (f).
