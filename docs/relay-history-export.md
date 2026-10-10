# Offline access to historical Encrypted Relay evidence

Tracking: [Relay retirement #386](https://github.com/onsager-ai/semon/issues/386).
The [retirement contract](encrypted-relay-retirement.md) owns deployment inventory
and authority. First create a complete private [custody artifact](relay-custody-export.md)
from explicitly inventoried, quiescent receiver/config/sender trees and original
recovery identities. Keep the originals and custody artifact unchanged.

`semon-relay-history` is a Linux/Unix offline migration reader. It builds
**outside the product workspace**, against the clean historical source pin
[`0648a99f977971fb177a5dda7a445c4941d64b57`](https://github.com/onsager-ai/semon/tree/0648a99f977971fb177a5dda7a445c4941d64b57).
It reuses that revision's unchanged frame, key, receiver, state and snapshot
verification code. Its age dependency and all registry versions/checksums come
from the historical lockfile. The normal `semon` binary and independent custody
exporter gain no historical cryptographic or transport dependencies.

```sh
python3 scripts/export-relay-history.py \
  --legacy-source /path/to/clean/pinned-semon \
  --custody /private/relay-custody \
  --out /private/new-relay-history
```

Building may fetch locked Cargo dependencies. The resulting reader performs no
network requests, enrollment, lease acquisition, queue flush, service operation,
native-home restore or deletion. It accesses only the declared custody artifact
and a new output directory whose parent already exists. Existing outputs and
outputs inside the artifact are refused. No default installation paths are scanned.

The output includes plaintext, credentials, keys, metadata and complete ciphertext
copies; a warning precedes access. Files are 0600 and directories 0700. Stdout
contains only a generic result, never decrypted evidence. Treat the entire output,
including both manifests and source paths, as private data.

## Coverage and integrity

The reader validates the artifact's version, exact inventory, lengths, hashes,
types and safe paths before creating output, then rechecks its generations and
every byte after decoding. Historical source paths in the manifest are never
followed. These checks detect changes; they do not establish an independently
signed provenance claim or replace quiescing writers.

All declared bytes, keys and empty directories remain in `custody/`. Historical
APIs that can tighten permissions operate exclusively on that new private copy.
Original files, modes and modification times remain unchanged.

- Every supported live generation/epoch and orphan branch is enumerated. Frame
  identities must match their storage paths. Encrypted records authenticate their
  original wrapped key, associated data and content tag. Plaintext records retain
  exact bytes. Chain checks include each retained generation and orphan prefix;
  gaps, conflicting live sequences and mismatched/missing checkpoint tips fail
  verification. An intentionally deleted predecessor can leave continuity
  unprovable; that remains a reported failure rather than a reconstructed line.
- Every retained snapshot manifest and divergent head is inspected. Manifests
  authenticate their version, identity, public header, relative paths, references
  and modes. Blob ciphertext checksums, age authentication and plaintext hashes
  must match. All present blobs are inspected, subject to deletion state. Missing
  parents/references and unavailable original recipients are explicit failures.
- Every pending snapshot packet under `snapshot-outbox`, including the actual
  `<root-id>.json` layout and all its blobs, is inspected without publication.
  Standard commented/multiple x25519 recovery-key files are supported. New
  recipients cannot substitute for original recipients.
- Body-free deletion receipts, pruned snapshots and deletion cutoffs remain
  recorded as unavailable. Coexisting crash-interrupted bodies/manifests are not
  decoded. Unreferenced blobs under a root with deletion state remain opaque.
  Pending packets covered by deletion state in any declared receiver are also
  suppressed. Invalid deletion state blocks plaintext for the affected scope.
- Sender state versions 1–3 and pending deletion queues are checked and retained.
  Leases, takeover/rewrap records, enrollment, generation checkpoints and native
  timing metadata remain inspectable in custody. Unknown or unsupported records
  are preserved and prevent successful full verification of the declared scope.

## Version 1 result and failures

The final `manifest.json` uses `semon.relay-history-export`, version 1, binds the
input custody manifest's SHA-256 and records the fixed historical source pin.
It is published last after file/directory fsync and original re-verification.
`frames`, `snapshots`, `blobs` and `metadata` identify source and output paths;
`unavailable` records deletion boundaries and `failures` records unsuccessful
checks. Snapshot manifests preserve original modes as metadata; exported files
remain private and are never restored into native homes.

Exit 0 means `verification_complete` and `decryption_verified` are true for the
**declared supported scope**, with no recorded failure. Exit 1 can retain a private
partial report with both fields false. A fatal custody, output or I/O failure can
leave private incomplete output without a final manifest. Do not treat a failed
command or missing manifest as a verified result. Exit 2 means invalid arguments.

`recovery_verified` is always false. Successful synthetic or declared-scope
decryption does not establish complete deployment inventory, workspace/harness
compatibility, current credential authority, writer exclusion, durable operation
reconciliation or safe recovery. Future recovery belongs to the explicit,
verified, versioned artifact contract in
[#255](https://github.com/onsager-ai/semon/issues/255).

## Retain a usable reader before protocol removal

```sh
python3 scripts/export-relay-history.py \
  --legacy-source /path/to/clean/pinned-semon \
  --build-only --artifact /private/tools/semon-relay-history

/private/tools/semon-relay-history \
  --custody /private/relay-custody --out /private/new-relay-history

python3 scripts/qualify-relay-custody.py \
  --legacy-source /path/to/clean/pinned-semon \
  --history-binary /private/tools/semon-relay-history
```

The build writes a separate receipt with source versions, binary/lockfile hashes
and a build-only qualification limit. Retain the binary, receipt, historical
source and locked dependencies beyond temporary CI artifact expiry. Release CI
builds the standalone reader and runs the actual binary against private synthetic
fixtures, including negative and deletion-boundary cases. This qualification
never scans existing installations. Deployment-specific inventory and original
recipient/read-back qualification still precede uninstall or loss of working
access; no data or key removal is part of this tool.
