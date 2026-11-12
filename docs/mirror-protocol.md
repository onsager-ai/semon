# The mirror protocol

`semon push` keeps a copy of this machine's session logs on another server, a *receiver*, and sends it the machine's facts, so `semon-sessions` can discover, index and query those native sources using the recorded facts (`Options::facts`). This document is the whole contract: any server that implements it can receive a push.

`semon receive` is the reference receiver (`semon_push::mirror::Receiver`, see the README's "Receive pushes from your other machines"). It keeps each machine's copy under `DIR/machines/<name>/`, and caps a request body at 6 MiB: one append's 4 MiB of file bytes as base64, plus the JSON around it. Beyond the answers below, it may answer 411 (no `Content-Length`) and 507 (the machine's copy would pass its size cap); a client treats either as a failure.

The reference receiver requires the current enrollment registry and a separate
`mirror_push` bearer credential scoped to one machine. `--token-file` remains the
Push client's protected input for that kind bearer; a bearer is machine transport
authority, never user identity or source authority. Legacy `DIR/tokens` state and
`receive token` commands are refused; preserve old tokens/history and explicitly
re-pair through the enrollment flow in a fresh receiver directory. No import, conversion or deletion occurs.
Local receiver enrollment starts with `receive enroll --machine NAME --advertise
HTTPS_URL --dir DIR --tls-cert PEM --tls-key PEM`; origin connect delivery is the
next integration slice. A kept-alive connection cannot switch enrollments.

The reference receiver takes an exclusive, non-blocking OS lock on its store directory for its lifetime. A second receiver on that directory is refused at startup, so preservation and replacement cannot interleave across receiver instances.

The generic mirror protocol is independent of the retired Encrypted Relay.
Mirrored content is readable by the receiver; sender redaction and HTTPS do not
provide end-to-end encryption. A source mirror does not contain the complete
workspace and native harness state required for independent recovery.

## What is sent

- **The input set:** supported native logs and sidecars listed by `semon_sessions::inputs`. Decoded mirror target paths are checked by `semon_sessions::is_input_path`. Nothing else under the agent homes is read. `*.key` files never are.
  - `claude` root: `projects/**/*.jsonl`, `projects/**/subagents/agent-<id>.meta.json`, `sessions/<pid>.json`.
  - `codex` root: `sessions/**/*.jsonl`, `archived_sessions/**/*.jsonl` and their native `.jsonl.zst` archive equivalents. Native databases are excluded.
  - Native `.jsonl.zst` equivalents of supported Claude and Copilot logs are also transport sources. The sender decodes and verifies each archive internally, then sends complete redacted logical records under the corresponding `.jsonl` path. Encoded byte positions never enter append cursors, status targets, quota accounting or source proofs. Full Codex inventory uses those same logical target paths. This changes neither the receiver allowlist nor the wire forms.
- **The facts** (`semon_sessions::Facts`): what the model takes from the machine rather than from the logs. That is the hostname, `$HOME`, the start time of each process a Claude pid file names, which Codex runs hold their writer lock (and the holder's pid), the repository of each working directory the logs name, and its sanitized `origin` identity (`repo_id`). A receiver builds with these in place of its own `/proc`, hostname and disk.
- Optional v2 facts `codex_rollouts` lists the sender's current plain Codex rollout paths, relative to its home and validated by the Codex input allowlist. It is a content-free collector inventory. A present empty list selects no current sources; an absent field means selection is unknown for an older snapshot. The receiver retains previously uploaded files, but model, tree and transcript lookup use only the explicitly selected paths when this inventory is present. Thus an active-to-archive move, or its reverse, does not duplicate work. A completed push sends facts after its file pass; a partial pass is not a complete snapshot. No deletion or inferred move is performed.
- **Redaction.** Every byte is redacted on the sending machine before it leaves (`semon_push::redact`). Known secret shapes become `*`, byte for byte, so lengths and offsets are unchanged and JSON lines stay valid. It is best effort: a secret of an unknown shape is sent as it is.

## Requests

All requests are `POST` with a JSON body and `Authorization: Bearer <token>`. `<url>` is the base URL the user gives `--to`. It must be `https`, except plain `http` to a loopback address (`localhost`, `[::1]`, 127.0.0.0/8).

Request bodies currently use identity encoding. Response gzip/Brotli and sealed
storage are independent. See [native archive transport and codec measurements](native-archive-transport.md)
for the bounded archive reader, coexistence/restart rules and upload experiment.

### `POST <url>/v1/mirror/append`

```json
{"root": "claude", "path": "projects/-work/abc.jsonl", "redactor_version": 1, "offset": 0, "head_sha256": "…", "bytes": "…", "replace": false}
```

- `root` is `claude` or `codex`. `path` is relative to that root, with `/` separators. The receiver checks both with `is_input_path` and answers 400 when they fail.
- `redactor_version` is the required integer version of the sender's redaction rules. The current sender uses version 1.
- `bytes` holds the file bytes, as standard base64 with padding. One request carries at most 4 MiB of file bytes.
- `offset` is where `bytes` go. Without `replace`, the receiver appends only when `offset` equals its copy's current length; a missing file has length 0.
- `replace: true` means: empty the copy, then write `bytes` from offset 0. `offset` is then 0.
- Before a `replace` of a non-empty copy is acknowledged, the reference receiver copies the bytes being replaced, read through `sealed::LogFile` so plain and sealed copies alike, into an immutable zstd chain under `machines/<name>/archive/` (see [Raw custody](#raw-custody)). A failure to keep them refuses the replace and leaves the copy as it was.
- `head_sha256` is the lowercase hex SHA-256 of the first `min(4096, offset + len(bytes))` bytes of the file as it will be once this request is applied. The receiver checks it against its own copy plus `bytes`. A mismatch means the two copies have diverged.

Answers:

- **200** `{"length": N}`: applied. `N` is the copy's new length.
- **409** `{"length": N, "head_sha256": "…"}`: not applied, because `offset` isn't the copy's length or the head doesn't match. `N` is the copy's current length, and `head_sha256` is the hash of its first `min(4096, N)` bytes.
- **400**: the body or the path is invalid. **401**: the token is unknown or revoked. **413**: the body is too large.

### `POST <url>/v1/mirror/facts`

The body is a `Facts` object:

```json
{"version": 1, "hostname": "laptop", "home": "/home/me", "proc_starts": {"4242": 7788}, "codex_locks": {"<thread id>": 4250}, "repos": {"/home/me/work/app": "app", "/tmp": null}, "repo_ids": {"/home/me/work/app": {"kind": "remote", "value": "github.com/me/app"}}}
```

`repo_ids` holds, per working directory inside a repository, a sanitized identity of the repository's `origin` remote, read from the Git config without running git. A `remote` value has credentials, the scheme and a trailing `.git` removed, the host lowercased and the scheme's default port dropped (https 443, http 80, ssh 22, git 9418; every other port is kept); scp-style `git@host:owner/repo` becomes `host/owner/repo`. A `file:` URL or local path remote is `{"kind": "path", "value": "<remote as written>"}`, and a repository with no `origin` is `{"kind": "unknown"}`. A directory outside any repository has no entry.

The answer is 200 when it is stored. A receiver that hasn't had facts for a while may treat the machine as offline: it keeps the same hostname and repositories, but no process as live and no lock as held (`Facts::offline`).

## How the client uses it

- **Complete lines only.** A `.jsonl` file is sent up to the end of its last complete line, so a line being written is held back until its newline arrives. No redaction pattern spans a newline, so redacting line by line and redacting the whole file give the same bytes.
  - The sender defaults to 256 KiB per append, below the protocol's 4 MiB maximum, so slow links can finish requests within the timeout.
  - A line longer than the sender's chunk size is redacted whole and then sent in pieces of that size.
  - A line longer than 64 MiB is cut there.
- **Other inputs** (`*.json`) are small. Each is sent whole, with `replace`, whenever its content changes.
- **Resuming.** The client keeps, per file, how many bytes the receiver has acknowledged and the SHA-256 of the original's first 4 KiB. The state file is `$XDG_STATE_HOME/semon/push/<url hash>.json`, mode 0600, in a 0700 directory. A new pass appends from there.
- **Rewrites.** If the file is now shorter than what was sent, or its first 4 KiB changed, the client sends it again from 0 with `replace`.
- **409.** When the receiver's copy is the client's redacted file up to a line boundary (same length prefix, same head hash), the client resumes from the receiver's length. Otherwise it sends the file again with `replace`.
- **Deleted files** are not deleted on the receiver.
- **Cadence.** One complete pass and one facts post, then exit. With `--watch`, each sweep sends at most four complete redaction batches before waiting 2 s and discovering fresh targets. Recent/live batches and history retain the scheduler's three-to-one preference; a long line remains one indivisible redaction batch. Full facts are posted at most every 10 s, after a complete sweep only; a bounded partial sweep never commits a complete source inventory. Optional status heartbeats continue during backfill.
- **Token.** It is read from `--token-file`, which must be mode 0600; the client refuses anything else. The token is sent only in the `Authorization` header.

## Sealed copies on the receiver

A receiver may seal an idle copy to save space (`semon_sessions::sealed`). Its bytes up to its last complete line go into zstd segments in `<file>.seal/`, and that range is punched out of the copy as a hole. The copy keeps its path, inode, length and modified time, so every offset, length and `head_sha256` in this protocol stays what it was. Anything that reads a copy's bytes, the receiver's own 409 hash included, reads them through `sealed::LogFile`. Appends land after the sealed range as plain bytes, and a later seal adds a segment. A `replace` renames a fresh file over the copy, and the next seal retires the old segments, but only once a chain holds their bytes (see [Raw custody](#raw-custody)); until then they stay.

## Optional partial sync observations (status v1)

`POST <url>/v1/mirror/status` is an optional extension, authenticated, admitted, size limited and scoped to the same machine as append and facts. Before posting status, a sender performs a cancellable bearer-authenticated HEAD capability request to that exact path, without following redirects, and requires 405 with `Allow: POST`. A cookie-gated 401/403 fallback, unknown route or other non-advertisement disables only this optional extension; append and full-facts refusals remain fatal. Once advertised, status POST token refusal is fatal, including subsequent revocation. A sender still disables status after POST 404 or 405, because an old generic HEAD handler can advertise POST globally. Successful status never lifts an append pause, storage restriction or retry delay. It does not replace `/facts`, delete retained copies, establish a complete collector selection or imply that an agent is executing.

```json
{"version":1,"observation_id":"0123456789abcdef0123456789abcdef","sequence":1,"observed_at_ms":1791539000000,"phase":"syncing","runtime":{"version":2,"hostname":"laptop","home":"/home/me","proc_starts":{},"codex_locks":{},"repos":{},"repo_ids":{}},"inventory_complete":true,"targets":[{"root":"codex","path":"sessions/native.jsonl","generation":"abcdef0123456789abcdef0123456789","target_bytes":12000,"acked_bytes":4096,"head_sha256":"…"}]}
```

`phase` is `syncing`, `up_to_date`, `waiting_to_retry`, `paused` or `storage_full`. `runtime` is newly collected allowlisted process, lock, run and ancestor evidence; it omits both source selections, repository scans and reported-run snapshots. The heartbeat worker collects it independently of uploads and the full event-cache facts worker, normally every ten seconds. Contact freshness, confirmed sync coverage and agent execution remain separate observations. Failed collection cannot repost a previous runtime snapshot with a new timestamp.

`observation_id` and transfer `generation` are content-free 32-character lowercase hex random identifiers. A shared allocator assigns sequences to both fresh status submissions and prepared full-facts commits. Status snapshots and allocation happen together under the progress lock; the first status may have a sequence greater than one if a full facts commit consumed an earlier sequence. Accepted observations increase sequence and observation time. A receiver rejects stale, reordered and old-run updates and must not refresh contact time from a rejected update. The reference receiver permits a one-minute clock window; sequence advances for each fresh submission, so an applied response with a lost acknowledgement cannot trap the sender on an old sequence. A receiver retains a bounded set of recently retired observation IDs for its stale-observation window, so retries can advance sequence without restoring a retired run. Status is stored separately as `status.json`; reference receiver confirmed indices refer to the stored observation's target array and must be revalidated against current generation metadata before later use.

The manifest contains at most 4096 unique allowlisted paths and the serialized request remains within the existing 6 MiB limit. A bounded indexed window favors live sources, recent/changed/runtime-sidecar foreground sources, newer source modification times and small-file ties, with deterministic path ties. Fresh foreground proof replaces the lowest cold descriptor; later cold ACKs retain their sweep priority and cannot promote themselves above it. Descriptors serialize in that priority order, so byte trimming also retains latest/live prefixes. A previously provisional history source can leave the window while its stored bytes remain retained and committed full-facts selection stays authoritative. Overflow makes `inventory_complete` false. That field describes a bounded target manifest, not authoritative collector inventory. Unknown totals mean no percentage. Target lengths freeze per sweep; later growth belongs to the next sweep. Incomplete tails are excluded from the transferable target when discovered. Confirmed totals sum unique paths in their current generation, never attempts or retransmissions; rewrites rotate generation and reset coverage. Before an input has been transferred, its generation may be empty only when `acked_bytes` is zero.

Append requests and 200/409 `Length` responses add optional `generation` fields. Old requests and receivers remain compatible. A generation-aware receiver durably binds its generation to accepted bytes, length, head and file identity before echoing it. The reference receiver invalidates its previous proof durably before writing data; interrupted data/metadata commits leave proof unavailable. An unchanged legacy cursor negotiates a generation with an empty append using the existing offset/head checks, so history need not be uploaded again. A sender receiving no echoed generation preserves its normal cursor but cannot claim generation-confirmed coverage. A per-client capability cache learns only from actual append ACKs: an omitted echo suppresses repeated unchanged-file probes for that client, while a later matching data ACK enables qualification again. Restart resets that cache, so unchanged whole JSON and log cursors saved against a legacy receiver can bind after an upgrade with empty offset/head appends rather than retransferring history. A physically absent legacy marker may bind an existing valid offset/head. A present malformed or descriptor-invalid marker never rebinds an unverified copy: its 409 omits `head_sha256` and generation, which forces rewind with a new generation and full replacement, including at length zero. Head-only equality cannot establish that modified bytes beyond the first 4 KiB were previously acknowledged.

Status `acked_bytes` is an acknowledged complete redaction-batch boundary, never an intermediate piece of a long line. A receiver independently verifies matching generation, stored length at least that boundary, stored prefix head hash at `min(4096, acked_bytes)`, current file identity and a complete JSONL line boundary. A malformed, ambiguous, stale or missing proof is unavailable. The reference receiver returns 200 with `{"confirmed":[<verified targets>]}`; 409 rejects stale observations. Even a receiver-confirmed prefix must be rechecked under the same write/generation lock when used later. A receiver may use `semon_sessions::codex_native_id_prefix(path, accepted_bytes)` to read the exact first complete `session_meta` record through sealed copies within 64 KiB; duplicate fields, malformed records and incomplete prefixes provide no identity. Filename, cwd and timing never prove an active/archive move.

A receiver may overlay verified, deduplicated provisional Codex paths in the optional `Facts.codex_provisional_rollouts` field **only in its served view**. Stored authoritative full facts retain the original `codex_rollouts`, including unknown (`None`) versus explicitly empty. A provisional overlay never makes full inventory known. Model source reads select the union of committed and provisional paths; bootstrap without a committed inventory selects only the explicit provisional subset. A native ID already selected by a committed source keeps that source until the next complete inventory commit. A sender's local/full facts never populate the provisional field, and the reference receiver refuses that field on `/facts`.

## Optional native Codex relocation

An append may include `relocation` with `from_path`, `native_id` and
`prefix_sha256`. It applies only between allowlisted `sessions/` and
`archived_sessions/` paths in the same Codex home, with a previously confirmed
32-character custody generation and a complete acknowledged prefix at `offset`.
The SHA-256 covers every byte of that **redacted** prefix, not only the head.
The sender proposes a move only for a unique previously acknowledged local inode
whose former path disappeared and whose explicit first native header still matches.

A supporting receiver verifies the old custody generation, native header,
confirmed prefix digest and destination head under its existing Raw write fence.
It copies exactly that logical prefix into a new independent Raw file, then
applies any intervening append. Sealed source bytes use the existing logical
reader. Local copy bytes count against quota; stale copies remain independent and
unchanged. An old Raw path longer than the sender's confirmed boundary can supply
only that verified prefix, including recovery from a lost acknowledgement.

SQLite relocation intents precede Raw writes; Index reuse follows validated
native identity and content evidence. Facts retain sole authoritative rollout
selection. A partially synced candidate sharing an already selected native ID
cannot become a duplicate provisional current source. Index discovery hints
identify candidate paths but grant no authority; the receiver revalidates the
selected Raw native header and current inventory.

An existing destination uses ordinary offset/head/generation admission, including
idempotent retries and Unarchive. Unsupported or rejected relocation falls back
to the existing 409 reconciliation and normal transfer. Failed or interrupted
proof commits permit conservative repair, never trusted reuse of ambiguous bytes.
Native Codex Archive is unrelated to Semon retained-history or Hub S3/R2 archival.

## Bounded sender scheduling and checkpoints

Each scheduling turn transfers one complete-line/redaction batch or one whole JSON input. Recent, changed, live Codex and runtime-sidecar inputs enter the foreground queue; freshly discovered foreground inputs precede pending foreground work; deterministic time/size/path ordering and queue rotation let small sessions pass a large newest file. No more than three foreground turns precede a pending history turn. Discovery refreshes about every two seconds between slices, while lengths of existing sweep targets stay frozen. Large lines retain the existing whole-redaction and 64 MiB cap semantics.

File identity, size and raw head are revalidated between slices; equal-length modifications trigger replacement. Every completed redaction batch and whole-file acknowledgement saves the private cursor atomically. Split-line pieces cannot persist intermediate offsets. A failed or cancelled slice leaves the preceding checkpoint and the receiver's retained bytes available for safe 409 reconciliation. Upload and facts/status workers share the existing stop and lock lifetime; a stopped worker may retain the lock until its outstanding operation ends. Full facts commits only after a completed frozen pass, using its exact selected Codex paths; rediscovery removes paths that disappeared or moved before committing the inventory. Local failures leave the previous authoritative inventory intact.

Full `/facts` requests optionally carry paired `mirror_observation_id` and `mirror_sequence` fields from that same allocator. Orphan markers are invalid; local/runtime facts omit both. The sender prepares the fence only after synchronizing targets to the exact completed sweep. A receiver serializes full facts and status observations together, records the full-facts fence, clears provisional proof on full inventory commit and rejects any later-arriving status at or below that sequence in the same observation. A higher-sequence fresh status may prove subsequent source changes without replacing the committed inventory. Legacy full facts remain accepted and clear provisional proof conservatively. Receiver status and generation metadata count against the machine quota and are included on restart; admitted growth reserves bounded metadata space before invalidating an old generation proof. Refused growth retains the previous accepted proof.

## Raw custody

`machines/<name>/archive/` is the receiver's raw archive (`semon_sessions::archive`):

- `sources.log` maps each mirrored file (`<root>/<path>`) to a random `archive_source_id`: one JSON record per line, appended and synced under the archive lock before anything is written under the new id, so another instance reuses the id and a crash cannot orphan a directory. A torn final line is ignored and cut off by the next append. Every journal read holds the archive's shared flock, and every write (including torn-tail repair) holds its exclusive flock; the flock is always acquired before the in-memory cache mutex.
- `<archive_source_id>/manifest.json` names its source (so a directory can always be attributed) and lists that file's chains in order. Each entry has a random `chain_id`, the plain `length`, the `sha256` of the plain bytes, the stored size and the `redactor_version`. A manifest is published by writing a temporary file, syncing it, renaming it over the old one and syncing the directory, so a failure before the rename leaves the last manifest intact.
- `<archive_source_id>/<chain_id>.zst` is one immutable chain: the logical bytes of the file as they were just before a `replace`.
- Before a replace, the receiver records in the copy's `.seal/` directory that a chain holds its bytes, but only for the seal that describes that file (bound to it, or agreeing with its bytes). A seal left by other content is not certified. The custody record binds the seal generation to an archive directory, source and exact chain record (including its length and digest). Immediately before retirement (`seal`, `tidy`, `forget`), the archive verification cache rechecks that chain's file identity and manifest, and the chain's bytes are compared directly with every named segment, under the shared archive flock through deletion. Missing, damaged or mismatched custody keeps the segments. Only that verified generation's named files are removed. Unnamed segments survive even if a new manifest is created after a lost one; `forget` never recursively deletes a directory and refuses while unverified files remain.

The `redactor_version` of a chain is the version recorded when its bytes were appended (`redactor-versions/` beside the copy), not the version of the replacement. A file appended to under different versions is `mixed`; bytes stored before the version was recorded are `unknown`. The archive and the version records are not counted toward the receiver's cap. A whole-file input (`*.json`) is replaced on every change, so each change keeps a chain; chains of identical bytes under the same version are stored once, and only if the earlier chain reads back intact; a damaged or missing chain stays listed and a fresh one is published beside it. Every manifest is validated when it is loaded (chain ids, digests, continuation state) before any path is built from it.

Before a native chain is trusted or extended, its verification cache is checked against the observed file (device, inode, length, modification time and change time) and the manifest's plain length, digest and stored size. A changed file or manifest requires a whole-chain read; a missing or damaged chain stays listed as evidence and a new chain is written from the log's consumed prefix. The ingestion cache owns one archive across scans, so unchanged prefixes are not reread on each bounded slice; its own completed, hashed writes update that verification cache. When the archive is ahead of a rebuilding index, the same cache retains a forward prefix decoder, hash state and compared-offset digests: increasing slices decompress only their new range, and repeated shorter prefixes reuse their digest. An unseen offset behind that cursor starts a new chain from native bytes instead of rereading the archive. File or manifest changes invalidate this progress. Each archive instance keeps at most 16 active prefix decoders with at most 256 compared-offset digests each. Catch-up and supersession release their decoder and open file. At either capacity limit, admission preserves the old chain and starts a new chain from native bytes rather than evicting an active cursor and repeatedly decompressing its prefix; this can duplicate compressed storage. Archive and seal temporary writes use unique names, exclusive creation and no-follow opens; planted symlinks are never truncated. A standalone machine keeps the same archive for its own native logs under its state directory (`archive/`, beside the index cache). After each scan of this machine's own logs, including the bounded scans of the Catalog producer, each log's consumed prefix (offset and SHA-256) is admitted again, including unchanged prefixes: intact cached chains need only a metadata check, while lost custody is repaired from the native bytes. Growth is appended only when the log still hashes to the consumed digest. A log whose earlier bytes changed starts a new chain and the old chain stays. Reading a source back from the archive when its native log is gone, and an optional `archive.retention_days`, are tracked in #575.
