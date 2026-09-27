# The mirror protocol

`semon push` keeps a copy of this machine's session logs on another server, a *receiver*, and sends it the machine's facts, so the receiver can build the same session model with `semon-sessions` (`Options::facts`) as `semon sessions --model-json` builds here. This document is the whole contract: any server that implements it can receive a push.

## What is sent

- **The input set:** exactly the files the model builder reads, listed by `semon_sessions::inputs` and checked by `semon_sessions::is_input_path`. Nothing else under the agent homes is read. `*.key` files never are.
  - `claude` root: `projects/**/*.jsonl`, `projects/**/subagents/agent-<id>.meta.json`, `sessions/<pid>.json`.
  - `codex` root: `sessions/**/*.jsonl`.
- **The facts** (`semon_sessions::Facts`): what the model takes from the machine rather than from the logs. That is the hostname, `$HOME`, the start time of each process a Claude pid file names, which Codex runs hold their writer lock (and the holder's pid), and the repository of each working directory the logs name. A receiver builds with these in place of its own `/proc`, hostname and disk.
- **Redaction.** Every byte is redacted on the sending machine before it leaves (`semon_push::redact`). Known secret shapes become `*`, byte for byte, so lengths and offsets are unchanged and JSON lines stay valid. It is best effort: a secret of an unknown shape is sent as it is.

## Requests

All requests are `POST` with a JSON body and `Authorization: Bearer <token>`. `<url>` is the base URL the user gives `--to`. It must be `https`, except plain `http` to a loopback address (`localhost`, `[::1]`, 127.0.0.0/8).

### `POST <url>/v1/mirror/append`

```json
{"root": "claude", "path": "projects/-work/abc.jsonl", "offset": 0, "head_sha256": "…", "bytes": "…", "replace": false}
```

- `root` is `claude` or `codex`. `path` is relative to that root, with `/` separators. The receiver checks both with `is_input_path` and answers 400 when they fail.
- `bytes` holds the file bytes, as standard base64 with padding. One request carries at most 4 MiB of file bytes.
- `offset` is where `bytes` go. Without `replace`, the receiver appends only when `offset` equals its copy's current length; a missing file has length 0.
- `replace: true` means: empty the copy, then write `bytes` from offset 0. `offset` is then 0.
- `head_sha256` is the lowercase hex SHA-256 of the first `min(4096, offset + len(bytes))` bytes of the file as it will be once this request is applied. The receiver checks it against its own copy plus `bytes`. A mismatch means the two copies have diverged.

Answers:

- **200** `{"length": N}`: applied. `N` is the copy's new length.
- **409** `{"length": N, "head_sha256": "…"}`: not applied, because `offset` isn't the copy's length or the head doesn't match. `N` is the copy's current length, and `head_sha256` is the hash of its first `min(4096, N)` bytes.
- **400**: the body or the path is invalid. **401**: the token is unknown or revoked. **413**: the body is too large.

### `POST <url>/v1/mirror/facts`

The body is a `Facts` object:

```json
{"version": 1, "hostname": "laptop", "home": "/home/me", "proc_starts": {"4242": 7788}, "codex_locks": {"<thread id>": 4250}, "repos": {"/home/me/work/app": "app", "/tmp": null}}
```

The answer is 200 when it is stored. A receiver that hasn't had facts for a while may treat the machine as offline: it keeps the same hostname and repositories, but no process as live and no lock as held (`Facts::offline`).

## How the client uses it

- **Complete lines only.** A `.jsonl` file is sent up to the end of its last complete line, so a line being written is held back until its newline arrives. No redaction pattern spans a newline, so redacting line by line and redacting the whole file give the same bytes.
  - A line longer than 4 MiB is redacted whole and then sent in 4 MiB pieces.
  - A line longer than 64 MiB is cut there.
- **Other inputs** (`*.json`) are small. Each is sent whole, with `replace`, whenever its content changes.
- **Resuming.** The client keeps, per file, how many bytes the receiver has acknowledged and the SHA-256 of the original's first 4 KiB. The state file is `$XDG_STATE_HOME/semon/push/<url hash>.json`, mode 0600, in a 0700 directory. A new pass appends from there.
- **Rewrites.** If the file is now shorter than what was sent, or its first 4 KiB changed, the client sends it again from 0 with `replace`.
- **409.** When the receiver's copy is the client's redacted file up to a line boundary (same length prefix, same head hash), the client resumes from the receiver's length. Otherwise it sends the file again with `replace`.
- **Deleted files** are not deleted on the receiver.
- **Cadence.** One pass and one facts post, then exit. With `--watch`, a pass every 2 s and facts every 10 s.
- **Token.** It is read from `--token-file`, which must be mode 0600; the client refuses anything else. The token is sent only in the `Authorization` header.
