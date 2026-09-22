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
indefinitely — every complete source line, with no pruning, opt-out, or
sampling — so the store file is created `0600` and its parent directory
`0700`, and both are re-tightened on every open rather than trusted from a
prior run. That access control is the only remaining protection; keep the
database local, and a private repository is not a safe destination for
captured session data. No real session fixture should be committed.

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
files. It does not read the Semon store or use `semon ship`. Encryption and
signed requests are the default. The receiver stores age-encrypted session data
keys and XChaCha20-Poly1305 frame ciphertext; it never receives a private key or
plaintext transcript line.

Create this machine's age/X25519 and Ed25519 identities. Existing files are
never overwritten:

```sh
cargo run --locked -p semon-relay -- keys init
cargo run --locked -p semon-relay -- keys show
```

They live at `$XDG_CONFIG_HOME/semon/identity.age` and
`$XDG_CONFIG_HOME/semon/signing.key`, falling back to `~/.config/semon`, with
private file and directory permissions. `keys show` prints the age recipient,
signing public key, and signing-key fingerprint. The fingerprint is the machine
id in encrypted mode.

Create an age identity on each machine, plus a separate offline recovery age
identity. Keep the recovery secret offline. On every sending machine, explicitly
enroll every machine recipient and the recovery recipient:

```sh
cargo run --locked -p semon-relay -- \
  keys enroll age1... --name laptop
cargo run --locked -p semon-relay -- \
  keys enroll age1... --name workstation
cargo run --locked -p semon-relay -- \
  keys enroll age1... --name offline-recovery
```

These commands append to `$XDG_CONFIG_HOME/semon/recipients.txt`. Semon never
adds, removes, or changes a recipient unless `keys enroll` is run explicitly.
Each session gets a random 32-byte data key, wrapped with age to all listed
recipients. Enrolling credentials is a user action.

Enroll each machine's Ed25519 public key at the receiver. This is also an
explicit credentials action, and the receiver must be restarted after an
allowlist change:

```sh
cargo run --locked -p semon-relay -- \
  receive enroll SIGNING_PUBLIC_KEY --name laptop \
  --dir /path/to/private/relay-receiver
```

Only `receive enroll` edits `<receiver-dir>/machines.txt`. Signed requests bind
the method, path, body hash, timestamp, and random nonce. The receiver rejects
unknown keys, invalid signatures, timestamps outside five minutes, reused
nonces, and a signed fingerprint that differs from the request body's machine.
It checks enrollment, timestamp, and nonce and signature encoding before reading
the body. Request bodies are capped at 64 MiB, which accommodates multi-megabyte
Claude Code tool-output lines while bounding each worker's allocation.

Start an encrypted receiver on loopback with its private storage directory:

```sh
cargo run --locked -p semon-relay -- \
  receive --listen 127.0.0.1:8734 --dir /path/to/private/relay-receiver
```

The default listen address is `127.0.0.1:8734`. The directory contains
ciphertext, lease state, session modes, and allowlists. It still should not be
placed in a repository.

For a remote receiver, supply a certificate and key. Semon does not create or
renew certificates. This example creates a self-signed certificate with both
loopback SANs; replace the names and addresses with the receiver's real ones:

```sh
openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 365 \
  -keyout relay-key.pem -out relay-cert.pem \
  -subj '/CN=relay.example.test' \
  -addext 'subjectAltName=DNS:relay.example.test,IP:127.0.0.1' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=critical,digitalSignature,keyEncipherment' \
  -addext 'extendedKeyUsage=serverAuth'

cargo run --locked -p semon-relay -- receive \
  --listen 0.0.0.0:8734 --dir /path/to/private/relay-receiver \
  --tls-cert relay-cert.pem --tls-key relay-key.pem
```

A non-loopback bind is refused unless TLS is configured, `machines.txt` is
non-empty, and signed-request enforcement is active. Plaintext mode is always
refused there. Choosing a reachable host, opening ports, DNS, certificate
custody, and hosting remain manual operator actions; Semon never changes network
reachability and never binds publicly by default.

Point a sender at HTTPS and pin that exact supplied certificate as its sole
trust root. Built-in roots, proxies, and redirects are disabled:

```sh
cargo run --locked -p semon-relay -- send --follow \
  --endpoint https://relay.example.test:8734/v1/frames \
  --tls-ca relay-cert.pem
```

After enrolling a new age recipient, rewrap existing session data keys from a
machine that can decrypt them. Omit `--session` to process every envelope. The
receiver logs each replacement with old and new envelope hashes. Rewrap requires
the current lease holder unless the explicit `--force` option is used:

```sh
cargo run --locked -p semon-relay -- keys rewrap --session SESSION_ID \
  --endpoint https://relay.example.test:8734/v1/frames --tls-ca relay-cert.pem
```

To test offline recovery custody without restoring a session, use a temporary
mounted path to the recovery identity. The command reports only success and
never prints the data key:

```sh
cargo run --locked -p semon-relay -- keys decrypt-envelope \
  --session SESSION_ID --identity /media/offline/recovery.age \
  --endpoint https://relay.example.test:8734/v1/frames --tls-ca relay-cert.pem
```

Live-session data-key rotation is not implemented.

Remote HTTP is refused, and HTTPS without `--tls-ca` is refused. Loopback HTTP
remains available for encrypted, signed traffic during local development.

### Relay threat model

| Attacker | Receiver behavior and remaining risk |
|---|---|
| Unenrolled client | Header prechecks reject the request before its body is read. A declared body over 64 MiB is also rejected before reading, and chunked or undeclared bodies are read through a 64 MiB plus one-byte bound. |
| Replayer | A nonce is retained for the full five-minute timestamp window. The cache holds 262,144 verified requests; if all entries are still live, the receiver fails closed with retryable HTTP 503 `replay_cache_full` until an entry expires. |
| Slow client | Four fixed application workers prevent one slow body from serializing all requests. `tiny_http` exposes no per-request header or body read deadline, so slow header connections can consume its internal connection threads and four simultaneous slow bodies can occupy the whole application worker pool; an Internet-facing deployment still needs connection and read timeouts at its network boundary. |

The receiver operator can still delete, withhold, reorder, or corrupt ciphertext
and cause denial of service. Envelope decryption, frame AEAD, and client-side
chain verification make stored-byte changes detectable, but do not provide
availability against the storage operator.

The receiver also owns one lease row per session: the current epoch, holder
machine, and expiry. It uses only its own clock for the three-minute lease;
sender wall time never decides expiry. A holder renews about once per minute in
resident follow mode. Every encrypted frame, key, orphan, and lease request uses
the signing-key fingerprint and carries a matching signature.

Run one sender pass, or poll continuously:

```sh
cargo run --locked -p semon-relay -- send --once
cargo run --locked -p semon-relay -- send --follow
```

Without an identity and at least one recipient, encrypted sending refuses to
start. The original plaintext M1 path is retained only for explicit loopback
use on both sides:

```sh
cargo run --locked -p semon-relay -- receive \
  --dir /path/to/private/plaintext-receiver --insecure-plaintext
cargo run --locked -p semon-relay -- send --once --insecure-plaintext
```

Plaintext mode uses `/etc/machine-id`; `--machine` exists for synthetic tests.
The receiver records a session's mode on its first frame and refuses to mix
plaintext and encrypted frames in that session.

Inspect the register and its append-only takeover log, or take over an expired
lease with a compare-and-swap:

```sh
cargo run --locked -p semon-relay -- lease status
cargo run --locked -p semon-relay -- lease status --session SESSION_ID
cargo run --locked -p semon-relay -- \
  lease takeover --session SESSION_ID
```

Takeover before expiry is refused unless `--force` is supplied. Before changing
the register, the encrypted command downloads the session envelope and
ciphertext frames, decrypts them locally, verifies every chain, and checks that
the local carrier files contain each acknowledged prefix. A missing, behind, or
mismatched stream refuses the takeover without incrementing the epoch. A
successful compare-and-swap records the previous holder and whether it was
forced, then initializes local sender watermarks from the verified tips.

There is one unavoidable race between that read-only check and the compare-and-
swap: the old holder can append another frame before it is fenced. A bare
`lease takeover` reports that post-CAS condition without writing sender state.
The restore workflow below closes it automatically: after fencing the old
holder, it fetches and verifies the now-stable suffix, appends it, and only then
initializes sender state at the new epoch.

### Taking over a session

When a node dies, first stop or isolate its Claude Code process if that is
possible, then make sure the receiver is reachable from the replacement node.
On the replacement node, use the intended checkout as `--cwd`:

```sh
cargo run --locked -p semon-relay -- restore \
  --session SESSION_ID --cwd /path/to/target-worktree \
  --projects /path/to/claude/projects --state /path/to/relay.json \
  --endpoint https://relay.example.test:8734/v1/frames \
  --tls-ca relay-cert.pem
```

`--projects` defaults to `~/.claude/projects`; `--state` uses the same
`$XDG_STATE_HOME/semon/relay.json` (or `~/.local/state/semon/relay.json`)
default as `send` and `lease takeover`.

Restore downloads the highest live generation of every main and subagent
stream, unwraps the session key with this machine's identity, decrypts and
checks every frame and chain, and writes through to private staging files
beneath the target cwd's Claude project slug. Frame retrieval is paginated per
stream and generation: requests carry `after_seq`, `limit_bytes`, and
`limit_frames`, and responses carry a `next` sequence cursor. The client
verifies across page boundaries without buffering a complete stream. It then
takes over the lease and initializes the local sender watermark. If the old
holder appended between the fetch and the lease compare-and-swap, restore
fetches and appends that suffix after fencing it.

Restore never overwrites existing content. An existing target must be an exact
byte prefix, in which case only its missing suffix is appended. Any differing
file is refused, as is the same session id under another project slug, because
Claude's non-interactive resume lookup is global by session id. New files are
created atomically with mode `0600`; restore directories use mode `0700`.

Takeover normally waits for the three-minute lease to expire. Use `--force`
only when an earlier manual takeover is intended. A gap or chain break refuses
the restore and names the stream and sequence. `--allow-gaps` salvages only the
verified prefix before each first gap, but deliberately does not take over the
lease or print resume guidance; nothing after a hole is written. JSON reporting
is available with `--json`. The explicit loopback-only plaintext deployment can
restore with `--insecure-plaintext` (and the same synthetic `--machine` override
as the other plaintext commands).

Review the report before continuing. It includes restored stream and line
counts, the new epoch, the transcript's last recorded `cwd` and `gitBranch`, a
read-only branch warning from `git rev-parse`, every `tool_use` id and name that
has no matching `tool_result`, and retained orphan groups by fenced epoch. It
never prints prompts, responses, tool inputs, or tool results. Check real state
before retrying any unfinished tool call. The final report line is the command
to run manually:

```sh
cd /path/to/target-worktree && claude --resume SESSION_ID
```

Semon never starts Claude or switches branches. Uncommitted worktree changes
are not restored; the old agent may still be running; and its git side effects
are not fenced. Memory and sidecar snapshots remain out of scope, so
`*.meta.json` and `custom-title.json` are not restored.

The receiver cannot verify encrypted chain values because each chain value is
inside its frame ciphertext. It still enforces session mode, holder and epoch,
sequence contiguity, gaps, duplicate tags, collisions, and orphan separation.
Run a full client-side decryption and chain check at any time with:

```sh
cargo run --locked -p semon-relay -- \
  verify --session SESSION_ID --endpoint https://relay.example.test:8734/v1/frames \
  --tls-ca relay-cert.pem
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

Each encrypted frame uses XChaCha20-Poly1305 with a fresh random nonce. Its
plaintext is the complete source line plus the new chain value. The frame key
`(session, stream, generation, epoch, seq)` and machine fingerprint are AEAD
associated data, so moving ciphertext to another position fails authentication.
A visible HMAC-SHA256 content tag, under a key derived from the session data key,
lets the receiver distinguish a retry from a collision without learning the
line. A retry may have different ciphertext because it uses a fresh nonce.

The sender checks the register before its first frame for every known session.
Frames below the current epoch are rejected with a distinct fencing response,
and frames at the current epoch are accepted only from its holder. Sequence and
chain continuity continue across an epoch boundary: the new holder's first
frame follows the previous live tip rather than starting again at sequence zero.

When a sender learns it was fenced, it permanently stops advancing that stream
in live history and logs the event. Complete records above its live watermark
are uploaded under the receiver's separate orphan namespace, without being
merged into or gap-checked against live history. List retained orphan groups
with:

```sh
cargo run --locked -p semon-relay -- orphans list
```

Fencing controls Semon frames only. It does not stop the old Claude Code agent,
and it cannot prevent that agent from editing files, pushing branches, or
opening pull requests after takeover.

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
observed until the next pass begins. A known relay limit is that a same-inode,
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

`--session` and `--day` select each raw row's own session and timestamp, so
they include complete lines that produced no semantic trace or occurrence.
When a line has no source timestamp, capture time is used. `--trace` selects
by projected content and therefore cannot select unprojected raw rows, which
have no trace id; the command states that limit when the selector is used.

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

`--session` and `--before` select each raw record's own provenance, not by
content: `--session ID` deletes exactly that session's own raw records, and
`--before YYYY-MM-DD` deletes exactly the raw rows whose own timestamp is
strictly before that UTC day's start, including unprojected lines. A trace
that recurs after the cutoff keeps only its later capture. `--trace ID` is
the exception: it matches projected content, so it removes that trace's raw
records from *every* session it was captured in. It cannot remove
unprojected rows because they have no trace id. A raw record written before
the schema-v3 session/sequence link existed cannot have its timestamp
backfilled during the schema-v4 migration and cannot be reached by
`--session` or `--before`; the command reports how many such records exist
and that only `--trace` can remove them individually.

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
