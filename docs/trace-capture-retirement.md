# Historical Trace Store capture

The experimental `semon-codex`, `semon-claude` and `semon-copilot` packages are
retired from the current workspace. They wrote `traces.sqlite3`; session browsing,
queries, generic Push, SSH and runtime integration read native sources through
the separate Session Event Index/Catalog and do not require those collectors.
Native Claude, Codex and Copilot parsers, shared sanitized fixtures and session
identity/relationship tests remain supported.

Removing source packages does not stop or uninstall existing collectors. No
native logs, databases, raw forensic records, cursor files or installed binaries
are removed or migrated automatically. Deployment inventory remains an operator
obligation; checked-in source removal cannot establish that a writer is absent.

## Pinned legacy tools

Revision `0648a99f977971fb177a5dda7a445c4941d64b57` contains the last capture
implementation before this removal, its locked dependencies, repair/backfill
commands and regression tests. Preserve an exact source/binary copy and checksum
for any installation that still requires those tools. A disposable checkout of
that revision can build them without launching capture:

```sh
git clone https://github.com/onsager-ai/semon.git /path/to/legacy-semon
git -C /path/to/legacy-semon checkout --detach 0648a99f977971fb177a5dda7a445c4941d64b57
cargo build --release --locked --manifest-path /path/to/legacy-semon/Cargo.toml \
  -p semon-codex -p semon-claude -p semon-copilot
```

The pinned [Codex capture/repair/backfill guide](https://github.com/onsager-ai/semon/blob/0648a99f977971fb177a5dda7a445c4941d64b57/README.md#capture-codex-sessions)
and [Copilot capture contract](https://github.com/onsager-ai/semon/blob/0648a99f977971fb177a5dda7a445c4941d64b57/docs/copilot-cli.md#existing-experimental-capture)
document the historical behavior. `--repair-subagent-keys` and `--backfill-raw`
remain available only from that pinned source or preserved installed binaries.
Even a legacy `--dry-run` can open/migrate an older Store schema; perform repair
experiments on a verified private copy, after exporting the original. Do not
point a legacy collector at a current native home simply to inspect old data.

## Inventory and custody

Record configured paths and exact versions privately; do not publish raw records
or credentials. Defaults below identify historical locations, not an instruction
to scan or alter them. XDG variables and explicit flags may select other paths.

| Component | Historical default or configuration |
|---|---|
| Store | `$XDG_DATA_HOME/semon/traces.sqlite3`, otherwise `~/.local/share/semon/traces.sqlite3`, or `--store` |
| Codex cursor | `$XDG_STATE_HOME/devlog/codex-tailer.json`, otherwise `~/.local/state/devlog/codex-tailer.json`, or `--state` |
| Claude cursor | `$XDG_STATE_HOME/semon/claude-cursor.json`, otherwise `~/.local/state/semon/claude-cursor.json`, or `--state` |
| Copilot cursor | `$XDG_STATE_HOME/semon/copilot-capture.json`, otherwise `~/.local/state/semon/copilot-capture.json`, or `--state` |
| Periodic/manual writers | Installed `devlog-codex-tailer.timer`/service, custom Claude/Copilot services, shell jobs and collector `--watch` processes |
| Canonical replication | Configured `semon ship --endpoint` or `SEMON_REPLICATION_ENDPOINT`; Store replication remains for a later slice |

Preserve the database and present WAL/SHM/journal files, cursor/configuration
files, units and exact legacy tools. Historical raw rows may be unique after
native source rotation. Retained source custody records, canonical traces and
ordered occurrences must survive independently of the current native files.

With explicit operator control, quiesce all writers before using the standalone
[private forensic exporter](trace-store-export.md). Verify its versioned complete
artifact using the documented verifier and preserve the original files. Export
does not require a collector, does not open SQLite on the original, and refuses
changed sources or newer schemas. Ordinary `semon log`, explicitly named
`semon forensic`, and the existing export alias remain available in the main CLI
for this migration stage. Preserve the standalone export tool beyond temporary
CI artifact retention.

Only after inventory and export verification should an operator decide whether
to stop/uninstall a collector. Deleting a database or sensitive forensic record
is a separate, explicit operator action under the existing
[retention/exposure policy](design/forensic-retention-and-exposure.md).
