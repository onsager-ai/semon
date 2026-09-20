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
commercial data, and machine paths. They are retained in full and
indefinitely — no pruning, no opt-out, no sampling — so the store file is
created `0600` and its parent directory `0700`, and both are re-tightened on
every open rather than trusted from a prior run. That access control is the
only remaining protection; keep the database local, and a private repository
is not a safe destination for captured session data. No real session fixture
should be committed.

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

## Read the log

`semon log` renders the occurrence log — what was captured, joined to its
semantics, in session/sequence order:

```sh
cargo run --locked -p semon-store --bin semon -- log \
  --store /path/to/traces.sqlite3 \
  --repo repository-name \
  --day 2026-09-20 \
  --limit 100
```

`--repo`, `--day`, and `--limit` are all optional filters; an unfiltered call
renders everything. This is an ordinary read: it queries only `occurrences`
joined to `canonical_traces`, and is structurally unable to reach
`raw_carrier_records` — enforced by a test that drops that table entirely and
checks the output is byte-for-byte unchanged.

## Read forensic records

`semon forensic` is the only command that reads `raw_carrier_records` — the
verbatim carrier bytes, including whatever prompts, responses, source code,
credentials, and machine paths passed through a captured session. Every
invocation writes a one-line warning to stderr before any output, so
redirecting stdout to a file still shows it.

Select exactly one of `--trace`, `--session`, or `--day`:

```sh
cargo run --locked -p semon-store --bin semon -- forensic \
  --store /path/to/traces.sqlite3 \
  --day 2026-09-20

cargo run --locked -p semon-store --bin semon -- forensic \
  --store /path/to/traces.sqlite3 \
  --session codex-session-id \
  --out /path/to/forensic-excerpt.txt
```

Output is one raw record per line, verbatim bytes as stored. `--out FILE`
writes to a file created (and re-tightened) `0600` instead of stdout; it is
an option, not a requirement — bulk selection to stdout works without it,
by design, so the command's honest bulk capability isn't fenced off behind a
narrower one nobody chose. See
`docs/design/forensic-retention-and-exposure.md` for the full reasoning.

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
