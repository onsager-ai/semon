# Semon

Semon captures an agent's working memory in a carrier-neutral, content-addressed
form. The local SQLite store keeps transferable semantics structurally separate
from raw carrier records: ordinary trace reads cannot return the raw bytes or
their carrier labels.

The workspace currently contains:

- `semon-store`, the two-region trace store
- `semon-codex`, an incremental adapter for Codex session and history JSONL

The former OTLP, OpenTelemetry Collector, ClickHouse, and analytics pipeline is
intentionally gone. Semon is local-first and does not configure or assume a
central endpoint.

Raw forensic records can contain prompts, responses, source code, credentials,
commercial data, and machine paths. Keep the database local and access
controlled; a private repository is not a safe destination for captured
session data, and no real session fixture should be committed.

## Capture Codex sessions

Run one cursor-aware capture pass with:

```sh
cargo run --locked -p semon-codex -- --verbose
```

By default, the adapter reads `~/.codex/sessions/**/*.jsonl` and
`~/.codex/history.jsonl`, writes traces to
`~/.local/share/semon/traces.sqlite3`, and preserves the existing tailer's
cursor location at `~/.local/state/devlog/codex-tailer.json`. The corresponding
XDG base-directory variables override those roots.

Useful source and destination overrides are:

```sh
cargo run --locked -p semon-codex -- \
  --sessions /path/to/sessions \
  --history /path/to/history.jsonl \
  --state /path/to/cursor.json \
  --store /path/to/traces.sqlite3 \
  --repo repository-name \
  --verbose
```

Only complete JSONL records advance the cursor. Truncated files restart at
offset zero, and an incomplete final line waits for the next pass. The semantic
projection contains only authored work intent/outcome. Codex session IDs,
timestamps, repository inference, machine paths, token counts, tool framing,
and the full source object stay out of content identity. The exact original
line is retained only through the store's explicit forensic read surface.

## Replicate canonical traces

`semon ship` makes one replication pass from the local store to an explicitly
configured HTTP endpoint:

```sh
cargo run --locked -p semon-store --bin semon -- ship \
  --store /path/to/traces.sqlite3 \
  --endpoint http://127.0.0.1:8080/traces
```

The endpoint can instead be supplied through `SEMON_REPLICATION_ENDPOINT`.
When neither is configured, `ship` prints a skip message and exits successfully
without opening the store. This permits an unconditional local workflow while
keeping the SQLite file as the source of truth; there is no daemon and no
default remote.

Each canonical semantic document is sent as JSON with its existing trace ID in
the `X-Semon-Content-Hash` header. An endpoint can therefore upsert by content
hash, making repeated replication idempotent. The replication path never reads
or transmits raw forensic records or carrier labels.

## Periodic capture

The existing systemd user-timer installer now builds and installs the Rust
adapter instead of the removed Python/OTLP tailer:

```sh
./scripts/install-user-timer.sh
systemctl --user status devlog-codex-tailer.timer
```

The legacy unit filename is retained so existing user installations can be
updated in place; its service now runs `~/.local/bin/semon-codex`.

## Development checks

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
sh -n scripts/install-user-timer.sh
```
