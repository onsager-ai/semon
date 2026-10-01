# Codex rollout resume mechanics (#10, M5a)

Measured 2026-10-01 with `codex-cli 0.159.0-alpha.3` on Linux. This is a synthetic persistence experiment, using an isolated `CODEX_HOME`, an empty temporary working directory, and a localhost Responses API server. The server emits a fixed assistant message and records the request input; it does not invoke a model. No real transcripts, authentication files, or user credentials are read.

Reproduce with Python 3 and the installed Codex CLI:

```sh
python tests/spikes/codex-resume.py --output /tmp/codex-resume-spike.json
```

The script starts one synthetic `codex exec` thread, then sequentially runs `codex exec resume <id>` in three new homes. The donor rollout is temporarily hidden during each resume, so a copied SQLite absolute path cannot accidentally read the original. Databases are copied only through SQLite's backup API. All temporary homes are removed afterward; the report contains exit codes, replay checks, database table counts, and diagnostic messages.

| Restored material | Result | Earlier input sent to provider | Persistence |
|---|---|---|---|
| Only the rollout, in its original date-relative path | Resume exits 0 | Both the earlier user prompt and assistant reply are replayed | Restored rollout changes; Codex rebuilds its state and history databases |
| Rollout plus verbatim SQLite backups | Resume exits 1: `no rollout found for thread id` | No provider request | Backed-up state retains the donor's absolute rollout path; the donor is deliberately unavailable |
| Rollout plus a held exclusive `flock` on `thread-writer-locks/<id>.lock` | Resume exits 1: `already has an active writer` | No provider request | Rollout remains unchanged; state initialization creates databases before the writer conflict |

The created rollout advertises `history_mode: "paginated"`. Nevertheless, rollout-only resume reconstructs one `state_5.sqlite.threads` row, two `thread_history_1.sqlite.thread_turns` rows and four `thread_items` rows after the resumed turn. It also creates goals, queue, logs, and memories databases. For this minimal thread, the history database is reconstructible from the rollout and need not accompany it.

The restored thread has the same ID. Existing SQLite state can make an otherwise valid restored rollout harder to discover when its recorded absolute location is stale. Restore should not copy whole state databases over a target's existing databases or claim that verbatim backups are portable. The tested writer lock is an advisory OS lock held by another process, not merely the existence of a `.lock` file.

This supports designing the rollout-only relay path for the tested CLI version, with an explicit writer-lock check and a discovery plan for an existing target state index. It does not establish sufficiency for arbitrary Codex threads: compaction, long paginated history, attachments, pending tools, subagents, rewritten rollouts, non-Linux lock implementations, and older CLI versions remain untested. In particular, #10 describes version 0.156.1; these results apply to 0.159.0-alpha.3. Replaying earlier turns into a synthetic provider proves the CLI supplied those turns, not that a production model/provider will recover every kind of session. The M5b relay implementation and its rollout/state discovery policy remain a separate checkpoint.

## M5b rollout relay

Codex rollouts can now be selected explicitly:

```sh
semon-relay send --once --codex-sessions /chosen/.codex/sessions
semon-relay send --follow --projects /chosen/.claude/projects --codex-sessions /chosen/.codex/sessions
```

`--all` selects the default Claude projects root and, when present, `CODEX_HOME/sessions` (or `~/.codex/sessions`). Sending without any root selector is still rejected. Session filters apply across both roots. Discovery reads only regular rollout JSONL files in valid `YYYY/MM/DD` directories; the first record must be `session_meta` and its ID must match the filename suffix. Symlinks, SQLite databases and other sidecars are excluded. Duplicate stream identities are rejected. A rollout is keyed by its ID and `codex/YYYY/MM/DD/rollout-…jsonl`; the existing generation mechanism handles same-path rewrites.

Restore to a chosen home, preferably a fresh one:

```sh
semon-relay restore --harness codex --session THREAD_ID --cwd /chosen/worktree --codex-sessions /chosen/recovery-home/sessions
```

The target must be `CODEX_HOME/sessions`. The report prints a shell-quoted next step with that exact `CODEX_HOME`, recorded working directory/git branch, and unresolved function/custom tool calls. Restore preserves the original date-relative filename and rollout bytes, uses the existing verified-prefix/non-overwrite rule, and applies the same encryption, lease takeover and fencing protocol as Claude restore. Mixed harness streams, multiple rollout paths for one restored session, invalid metadata IDs, unsafe paths and symlink destinations are refused.

Restore acquires the target home's `thread-writer-locks/<thread>.lock` with a nonblocking exclusive OS lock before fetching/installing content and holds it until takeover completes. It refuses an active writer even with `--force`; force concerns the relay lease, not local file ownership. Linux `flock` interoperability is covered by the synthetic Rust round-trip test. SQLite databases in the target are neither copied nor modified. An existing Codex thread index can still point to an older rollout location; this implementation does not repair that index, which is why a fresh target home is recommended.

The compatibility claim remains the M5a measurement above: synthetic minimal rollout-only resume with CLI 0.159.0-alpha.3. The relay tests additionally cover plaintext/encrypted transport, writer exclusion, rewrite generations, metadata reporting, date-path recovery and live SQLite byte preservation; they do not broaden the set of proven resumable Codex histories.
