# Semon

Semon captures an agent's working memory in a carrier-neutral, content-addressed
form. The local SQLite store keeps transferable semantics structurally separate
from raw carrier records: ordinary trace reads cannot return the raw bytes or
their carrier labels.

The workspace currently contains:

- `semon-store`, the two-region trace store
- `semon-codex`, an incremental adapter for Codex session and history JSONL
- `semon-claude`, an incremental adapter for Claude Code session JSONL
- `semon-relay`, loopback failover replication for complete Claude Code streams

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

## Relay Claude Code session files

`semon-relay` is a separate failover path for the carrier's complete session
files. It does not read the Semon store or use `semon ship`. M1 is deliberately
**loopback-only**: frames contain unencrypted transcript bytes, so both the
receiver bind address and sender endpoint must be literal loopback IP addresses.
Nothing may be sent to another machine until client-side encryption exists.

Start a receiver with an explicit private storage directory:

```sh
cargo run --locked -p semon-relay -- \
  receive --listen 127.0.0.1:8734 --dir /path/to/private/relay-receiver
```

That directory contains plaintext transcript frames. The receiver creates its
root with private permissions, but it should still be treated as sensitive
local state and never placed in a repository.

Run one sender pass, or poll continuously:

```sh
cargo run --locked -p semon-relay -- send --once
cargo run --locked -p semon-relay -- send --follow
```

The sender reads `~/.claude/projects` by default and writes its atomic ack
watermarks to `$XDG_STATE_HOME/semon/relay.json`, falling back to
`~/.local/state/semon/relay.json`. Use `--projects`, `--state`, and `--endpoint`
to override those locations. Only complete newline-terminated records are
framed. State includes the byte offset after each stream's watermark, its file
identity, and the start and hash of its last acknowledged line. Every process
start re-hashes the complete acknowledged prefix. Later resident follow passes
check identity, length, and that last line, then seek directly to the saved
offset instead of re-reading retained history.

The source files remain the spool when the receiver is unavailable; the sender
holds one frame at a time and retains one observation range per failed pass,
not one entry per queued line, while retrying without advancing past a hole.
Lines complete at the start of a pass are observed at pass start, so a large
backfill or receiver outage includes the time each line waits behind earlier
frames. Each pass or follow interval reports acknowledged lines and bytes,
p50/p95/max observation-to-ack lag, remaining backlog, and pass wall duration.

The conservative loss bound for timer mode is roughly the timer interval plus
pass duration plus reported lag. For follow mode it is the configured sleep
interval plus pass duration plus lag: a line appended during sleep cannot be
observed until the next pass begins. A known M1 limit is that a same-inode,
in-place rewrite inside the already-acknowledged prefix which does not shrink
the file and leaves the last acknowledged line unchanged is detected only by
the full re-hash at the next process start.

The user units are provided for manual installation; the repository does not
install or enable them automatically:

```sh
cargo build --locked --release -p semon-relay
install -Dm755 target/release/semon-relay ~/.local/bin/semon-relay
install -Dm644 systemd/semon-relay.service \
  ~/.config/systemd/user/semon-relay.service
install -Dm644 systemd/semon-relay.timer \
  ~/.config/systemd/user/semon-relay.timer
install -Dm644 systemd/semon-relay-follow.service \
  ~/.config/systemd/user/semon-relay-follow.service
systemctl --user daemon-reload
```

The recommended installation is `semon-relay-follow.service`. After starting
the loopback receiver separately, enable the resident mode with
`systemctl --user enable --now semon-relay-follow.service`. Resident `--follow`
is the supported relay mode, targeting an RPO of about 2 seconds. Its measured
idle cost is about 6 ms of CPU per pass, while every `--once` process start
re-hashes acknowledged history and took 0.42 seconds on the measured corpus;
that one-shot cost grows with retained history.

The `semon-relay.service` and `semon-relay.timer` units remain available for
one-shot or manual use, but do not enable the timer alongside the follow
service. Both sender modes run at nice level 19 with idle I/O scheduling, a 5%
CPU quota, and a 128 MiB memory limit. The earlier no-daemon statement applies
only to `semon ship`; the relay is a resident process by decision.

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

## Forget forensic records

`semon forget --forensic` permanently deletes rows from `raw_carrier_records`
— the only supported way to remove forensic data. `canonical_traces` and
`occurrences` (the log) are never touched, so `semon log` renders
byte-for-byte identically before and after, including after the store is
reopened. This is what makes the occurrence region's whole point reachable:
see `docs/design/trace-identity-and-occurrences.md` on why the log is
designed to survive forensic deletion.

`--forensic` is required, and so is exactly one selector:

```sh
cargo run --locked -p semon-store --bin semon -- forget --forensic \
  --store /path/to/traces.sqlite3 \
  --before 2026-01-01

cargo run --locked -p semon-store --bin semon -- forget --forensic \
  --store /path/to/traces.sqlite3 \
  --session codex-session-id \
  --yes
```

A bare `semon forget --forensic` with no selector is refused rather than
deleting everything — this is the first destructive, irreversible command in
the tool, so it never defaults to the maximal action.

`--session` and `--before` select through each raw record's own
session/sequence link, not by content: `--session ID` deletes exactly that
session's own raw records, and `--before YYYY-MM-DD` deletes exactly the
captures whose own occurrence timestamp is strictly before that UTC day's
start, per capture rather than per trace — a trace that recurs after the
cutoff keeps only its later capture, not its earlier one too. `--trace ID`
is the exception: it matches by content, so it removes that trace's raw
records from *every* session it was ever captured in, not just one — naming
a trace means removing all of its captures. A raw record written before this
link existed (schema version 3) cannot be reached by `--session` or
`--before`; the command reports how many such records exist and that only
`--trace` can remove them.

Without `--yes`, the command prompts interactively, stating exactly how many
raw records will be deleted and that it cannot be undone. If stdin is not a
terminal and `--yes` is absent, it errors instead of proceeding
non-interactively.

The delete itself runs under `PRAGMA secure_delete = ON` (deleted content is
overwritten with zeroes before its page is freed) and is followed by
`VACUUM` (which rewrites the file without the freed pages at all), because a
plain SQLite `DELETE` leaves deleted bytes readable on disk until the page is
reused — theatre for a command whose purpose is removing credentials and
source text.

This does not change retention policy: nothing calls this automatically, and
`docs/design/forensic-retention-and-exposure.md` Decision 1 still retains
everything indefinitely by default. This adds a deliberate operator action,
not a schedule.

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
