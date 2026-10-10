# Historical mutable memory snapshots

This records snapshot behavior at source commit
`0648a99f977971fb177a5dda7a445c4941d64b57`. These commands are available only
in retained legacy tools. Current product builds remove the Relay snapshot
protocol. Follow the [retirement and custody contract](../encrypted-relay-retirement.md)
and [offline history export](../relay-history-export.md) before changing existing
artifacts. Future recovery follows explicit verified/versioned artifacts in
[#255](https://github.com/onsager-ai/semon/issues/255). The behavior below does
not establish complete workspace or native-harness recovery.

`semon-relay snapshot` keeps an encrypted history of explicitly selected file or
directory roots. It uses the relay's enrolled age recipients and signed TLS
transport. It does not enroll configuration files, skills or hooks implicitly.
Claude roots include `~/.claude/CLAUDE.md`, a project's `memory/` directory, and
individual `subagents/*.meta.json` or `custom-title.json` sidecars. Codex SQLite
memory remains outside this file-snapshot mechanism: it needs a consistent
SQLite backup and a harness-specific merge policy.

```
semon-relay snapshot follow --root /home/me/.claude/CLAUDE.md \
  --endpoint https://relay.example --config /home/me/.config/semon-relay
semon-relay snapshot list --root-id ROOT_SHA256 --endpoint https://relay.example
semon-relay snapshot restore --root-id ROOT_SHA256 --target /recovery/new-memory \
  --manifest MANIFEST_SHA256 --endpoint https://relay.example
```

The default root id is SHA-256 of the absolute lexical root path. Supply the
original `--root-id` on a different machine. `--parent` explicitly branches from
a known manifest; later captures on the same machine continue its own latest
history. Two machines sharing a parent keep both heads. Restore uses the chosen
manifest or the newest head and reports other heads and changed paths, including
mode-only differences. It never merges them. `--session SESSION` records an
associated session inside the encrypted manifest and requires that this machine
currently holds its live relay lease. The observed lease epoch is recorded;
`--epoch` must agree when supplied with `--session`. Without `--session`,
`--epoch` remains an operator-known observation. This is provenance, not a fence
on memory edits or a transaction with transcript capture.

Capture and follow preserve file modes and content-address files by plaintext
SHA-256. Only paths and bytes inside age ciphertext leave the machine; the
receiver sees root/blob/manifest ids, parent links, machine and observation wall
time. Those hashes expose equality. Public references allow collection without
decryption. Existing blobs are not uploaded again. Follow scans every two
seconds, omits a new manifest when paths, contents and modes are unchanged, and
retries a private ciphertext outbox scoped to the receiver URL. A process-local
cache holds file metadata, content hashes and ciphertext only. Each pass still
opens the root, directories and files through no-follow descriptors; unchanged
device/inode, size, mode, mtime and ctime skip file reads and file encryption.
Changed generations are reread and hashed, and before/after metadata must agree.
Recipient or root changes invalidate the cache. Restarting follow rereads files;
no plaintext cache or durable plaintext outbox is kept. Source reads bind all
ancestor directories and leaf files through
no-follow descriptors. Symlinks and special files fail the scan. A disappeared
root with an existing safe parent produces an empty manifest, recording deletion.

Restore authenticates root, selected id, public headers and every blob hash,
stages all content, then publishes a new directory with atomic no-replace rename
on Linux and macOS. Existing targets, including dangling symlinks, are refused.
The limits are 4,096 entries, 8 MiB per file, 24 MiB per root and 2 MiB for the
plaintext manifest. History listing uses bounded pages without ciphertext.
The macOS publication branch requires CI validation; Linux is tested locally.

Memory forgetting uses the durable relay deletion queue's `--memory` selector
(or explicit original root id). A fixed wall-time cutoff makes retries stable.
Only blobs unreferenced by surviving manifests are collected. Root cutoff and
body-free pruned-id tombstones reject late old manifests and replay, while
allowing descendants to name pruned parents. This does not remove copies already
restored elsewhere. Stop a root's follow process when retiring it: newly captured
edits after the cutoff form new history.

Recipients must already be enrolled when a snapshot is encrypted. Adding a new
recipient does not rewrite historical age ciphertext; restore with an existing
recipient identity. This also applies to unchanged blobs reused by later
manifests: a newly enrolled identity may decrypt their manifest while lacking
access to shared older blobs. Session envelope rewrapping does not rewrap these
snapshots.

Session recovery can restore one explicitly selected memory or sidecar root:

```sh
semon-relay restore --session SESSION --cwd /work/recovered \
  --projects /recovery/claude-projects --memory-root-id ROOT_SHA256 \
  --memory-target /recovery/new-memory --memory-manifest MANIFEST_SHA256 \
  --endpoint https://relay.example --json
```

The memory target must be a new directory, including when the snapshot root was
one file. Existing directories, files and dangling symlinks are refused. An
authenticated session association must match `--session`; historical snapshots
without an association remain usable by explicit root selection. A restored
root is staged separately; review it and place its contents in the intended
harness location yourself. Snapshot capture and transcript recovery are separate
operations, so neither an epoch nor a matching session proves the same instant
was captured. The selected snapshot's session and epoch are included in the
memory report.

`recovery_status.memory` reports `not_selected`, `restored` or `failed`. With no
memory selection, transcript recovery can succeed while memory remains omitted.
Only explicitly selected roots are restored; other `*.meta.json`,
`custom-title.json`, attachments, configuration and SQLite memory remain omitted.
If transcript recovery succeeds but selected memory restoration fails, its files
remain restored, the CLI reports incomplete recovery, exits unsuccessfully and
omits the resume command. It never overwrites an existing memory target or rolls
back transcript files that were already restored.

Encrypted remote viewers can enumerate opaque root ids in bounded authenticated
pages, decrypt head manifests locally to check session associations, and read a
verified selected file into memory without creating recovery files. Root ids,
hashes and session associations do not identify unselected harness locations;
viewers must restrict which sidecar paths they consume.
