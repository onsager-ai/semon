# Private custody of historical Encrypted Relay data

Tracking: [Relay retirement #386](https://github.com/onsager-ai/semon/issues/386).
This is a migration tool for existing data. The
[retirement contract](encrypted-relay-retirement.md) owns deployment inventory,
operator authority, historical access and recovery limits.

`semon-relay-export` copies every file and empty directory in explicitly selected
receiver, configuration and sender directories, plus individually selected
recovery-key files. It builds independently of Relay, Sessions, Runtime and
capture crates. It never discovers installations, initializes identities,
starts receivers, sends frames, restores native homes or invokes deletion.
The main `semon` binary has no dependency on this exporter.

Inventory and quiesce **all** writers before exporting. Include every receiver
and its retained generations/epochs, orphan frames, envelopes, leases, takeover
records, snapshots/forks and deletion receipts. Include the configuration's
original age/signing keys, recipient/machine records and snapshot outboxes;
include complete sender state directories with pending deletion queues and
separately held recovery identities. Inputs may be repeated. A sender watermark,
metadata tree or apparent latest snapshot is not a complete custody inventory.

```sh
cargo build --locked --release -p semon-relay-export
./target/release/semon-relay-export \
  --receiver-dir /private/receiver \
  --config-dir /private/identity-and-outboxes \
  --sender-dir /private/sender-state \
  --recovery-key /offline/recovery.key \
  --out /private/new-relay-custody
```

The destination's parent must already exist. The new destination must be outside
every input tree; an existing destination is refused. No default paths are
scanned. A warning precedes access because the export can contain private keys,
credentials, plaintext evidence, ciphertext, machine paths and pending deletion
state. Newly created directories are 0700 and files 0600 on Unix; original bytes,
permissions and modification times remain unchanged. Preserve the artifact as
private data, including its manifest's source paths.

Symlinks, special files, non-UTF-8 paths, missing inputs and mismatched input
roles fail explicitly. Source inventory, inode/generation, mode or byte changes
reject completion; these observations do not replace quiescing writers. Failed
exports retain private incomplete output. Do not treat output without a complete
manifest or a failed command as a verified artifact. Originals are never removed.
Unknown files are preserved byte-for-byte rather than silently filtered out.

## Version 1 artifact

`manifest.json` is written last, after file fsync, destination read-back and
source re-verification. Its format is `semon.relay-custody-export`, version 1.
Each declared input records its resolved source path, role, separate artifact
location and all relative directory/file entries. Files record byte length and
SHA-256; empty directories remain represented. A recovery-key input's root entry
has an empty relative path and its artifact is a file, rather than a directory.

`may_include_private_keys` is true. **`decryption_verified` is false.** Copy
integrity establishes custody of the declared bytes, not complete inventory,
recipient coverage, authenticated plaintext, stream continuity or recoverability.
Do not rewrite this field based on a metadata tree or one successful decryption.
Keep separate results for each version-qualified read-back scope and failure.

## Historical read-back qualification

Retain a clean source checkout at
[`0648a99f977971fb177a5dda7a445c4941d64b57`](https://github.com/onsager-ai/semon/tree/0648a99f977971fb177a5dda7a445c4941d64b57),
its locked dependencies/vendor patch and matching binaries/configuration. Record
their exact versions and checksums, and preserve them beyond temporary CI artifact
expiry. New recipients do not grant access to all historical envelopes/blobs.

The reproducible **synthetic** qualification is:

```sh
python3 scripts/qualify-relay-custody.py --legacy-source /path/to/clean/pinned-semon
```

The runner verifies the source pin and cleanliness, preserves its registry pins,
and compiles unchanged historical receiver/crypto/snapshot code with the current
exporter. Building may fetch locked Cargo dependencies. Tests create only private
disposable fixtures and authenticated loopback receivers; they never scan user
installations. They cover live retained generations across epochs, a rewritten
generation, orphan ciphertext, body-free deletion receipts, lease/takeover state,
all snapshot history including divergent heads, original and separately held
recovery identities, pending snapshots and opaque pending sender state. Artifact
hashes and originals are checked before and after read-back. Missing recipients,
frame substitution/corruption/gaps, snapshot substitution/checksum failure and
unsupported sender versions refuse successful verification.

This fixture qualification does not certify an existing deployment. Use the
matching historical reader and original recipients against a **separate private
working copy**: opening a historical Receiver or serving snapshot reads can write
metadata and tighten permissions. Keep the custody artifact itself unchanged.
`Receiver::list_frames` covers all live retained generations/epochs; enumerate
orphans separately. Current metadata trees alone do not establish this coverage.
Snapshot history includes every retained manifest, not only fork heads. Preserve
deletion receipts and report intentionally removed payloads as unavailable;
never reconstruct or replay them. Record missing keys, gaps, unsupported versions,
divergent heads and integrity failures explicitly before retiring working access.

An all-scope operator-facing offline decrypted export remains a separate #386
deliverable. Protocol/reader removal must preserve a usable qualified historical
access path. No service uninstall, identity replacement, data pruning or live
recovery is authorized by running this custody exporter or synthetic qualification.
