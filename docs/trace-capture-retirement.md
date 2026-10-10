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
commands, the legacy `semon ship` sender and regression tests. Preserve an exact source/binary copy and checksum
for any installation that still requires those tools. A disposable checkout of
that revision can build them without launching capture:

```sh
git clone https://github.com/onsager-ai/semon.git /path/to/legacy-semon
git -C /path/to/legacy-semon checkout --detach 0648a99f977971fb177a5dda7a445c4941d64b57
cargo build --release --locked --manifest-path /path/to/legacy-semon/Cargo.toml \
  -p semon-codex -p semon-claude -p semon-copilot -p semon-cli
```

This revision also preserves historical `semon log`, selected `semon forensic`,
the complete `--export-store` alias and explicit `semon forget --forensic`,
including durable queued Relay deletion. They are retired from the current CLI,
along with the `semon-store` package. Use the exact preserved binary by its
private absolute path, with an explicit `--store`; follow the
[historical inspection/deletion procedure](trace-store-export.md#legacy-inspection-and-deliberate-deletion).
Current session/query/MCP commands use native sources and the Session Event
Index/Catalog, with no historical Store dependency.

The Linux x86_64 `semon-linux-x86_64` artifact from
[release run 38050996502](https://github.com/onsager-ai/semon/actions/runs/38050996502)
was qualified against 12 synthetic inspection/export/deletion cases, including
shared traces and source lines, unprojected/unlinked rows, private excerpts,
confirmation refusal, unchanged occurrence logs after deletion and a durable
offline Relay deletion request. Artifact ID `11669627595` has ZIP SHA-256
`bb733325e53587478517011fad9e9165100b18edf9f4064d67d6a7f02ccabc7e`;
its extracted `semon` has SHA-256
`94958ab5b3ec22578cc1f509b003bf5cafe0b174288cb713ca78d9b32e19725f`.
Actions retention expires November 9, 2026; preserve a checksummed source/binary
copy rather than treating a temporary CI artifact as historical-data custody.
Other platforms can build the locked pinned source above. Qualification used
synthetic private stores only and does not establish that an existing deployment
has been migrated.

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
| Canonical replication | Configured `semon ship --endpoint` or `SEMON_REPLICATION_ENDPOINT`; sender/command are retired from the current workspace |

Preserve the database and present WAL/SHM/journal files, cursor/configuration
files, units and exact legacy tools. Historical raw rows may be unique after
native source rotation. Retained source custody records, canonical traces and
ordered occurrences must survive independently of the current native files.

With explicit operator control, quiesce all writers before using the standalone
[private forensic exporter](trace-store-export.md). Verify its versioned complete
artifact using the documented verifier and preserve the original files. Export
does not require a collector, does not open SQLite on the original, and refuses
changed sources or newer schemas. Historical inspection/deletion is supplied by
the qualified pinned CLI above; the complete exporter remains an independent
current tool. Preserve both exact tools beyond temporary CI artifact retention.

The current CLI no longer accepts `semon ship` or reads its endpoint environment
variable. Inventory scheduled invocations and downstream canonical-only receivers;
a current binary fails explicitly rather than silently reporting a successful sync.
Removing the sender does not modify a downstream service or its data. Generic Push
is the supported native-source mirror protocol, not a replacement endpoint for the
old trace documents or a complete forensic export.

Only after inventory and export verification should an operator decide whether
to stop/uninstall a collector. Deleting a database or sensitive forensic record
is a separate, explicit operator action under the existing
[retention/exposure policy](design/forensic-retention-and-exposure.md).
