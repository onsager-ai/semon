# Encrypted Relay retirement and existing data

Tracking: [Session foundation #380](https://github.com/onsager-ai/semon/issues/380)
and [Relay retirement #386](https://github.com/onsager-ai/semon/issues/386).

New Relay service setup is retired. The repository no longer supplies
`semon-relay.service`, `semon-relay.timer` or `semon-relay-follow.service`
templates. Removing source templates does not stop installed units, change
configuration, revoke identities or remove data. External deployment locations
are not yet known; checked-in units are not evidence that installations are absent.

The current product workspace no longer includes `semon-relay`, its transport,
identity/enrollment, sender/receiver, snapshot, lease, takeover or restore code.
Sessions removes `collect_remote`, the `encrypted-relay` feature and its exclusive
lease metadata; the CLI rejects retired encrypted-view flags before discovering
sources or opening keys. Normal builds no longer resolve the Relay crypto stack.
The independent custody exporter and offline history reader remain available;
the latter builds against a clean historical source outside this workspace.
Hub's independent credential-vault encryption remains supported. Generic
Push/receive, hosted synchronization, SSH, native agent handoff/peer events and
runtime lifecycle remain supported.

## Inventory before changing an installation

Record private locations and exact versions, without publishing key contents,
tokens, plaintext session evidence or ciphertext archives in issues or PRs.
Command-line/configuration overrides are authoritative; defaults are only leads.

| Custody | Locate and retain |
| --- | --- |
| Installed services | All hosts and receiver/sender/snapshot processes, unit files, drop-ins, launch arguments, endpoints and TLS configuration; include manual/one-shot jobs |
| Machine and recipient identity | Config directory (`$XDG_CONFIG_HOME/semon` or `~/.config/semon` by default), `identity.age`, `signing.key`, `recipients.txt`, receiver `machines.txt`, and separately held recovery-recipient identities |
| Sender/deletion state | `--state` path (`$XDG_STATE_HOME/semon/relay.json` or `~/.local/state/semon/relay.json` by default), identity-scoped watermarks, generations, pending deletion state and private caches |
| Receiver evidence | Actual `--dir` tree, encrypted frames and wrapped key envelopes, receipts/hash-chain tips, orphan generations, deletion tombstones, lease/epoch metadata and snapshot storage |
| Snapshot publication | Config-scoped `snapshot-outbox`, every root/session association, manifest/head/fork and encrypted blob; pending publication is distinct from an acknowledged snapshot |
| Restore/recovery use | Native homes or workspaces already restored, external copies, exact harness/protocol versions, and which recovery identities can decrypt historical data |

Preserve a pinned legacy binary/source and its configuration alongside any
private backup. The [historical deployment reference](history/encrypted-relay-deployment.md)
records the retired setup and protocol behavior; it is not a current new-install
recommendation. Do not regenerate an existing identity as a substitute for finding
its key or assume that new recipient enrollment covers historical ciphertext.

## Retained inspection and migration limits

Retain the legacy source at commit
`0648a99f977971fb177a5dda7a445c4941d64b57` and a working legacy binary before
upgrading an installation that uses Relay. Build `semon-relay` and `semon-cli`
with `cargo build --locked --release -p semon-relay -p semon-cli --bins` in that
clean historical checkout, never in the current product workspace. Its
`semon-relay keys show --config PATH` reports public identity/fingerprint values.
[Historical encrypted remote trees](history/encrypted-remote-sessions.md) and
`semon-relay verify --session SESSION --endpoint URL --config PATH` preserve the
old authenticated decryption, continuity and tip checks. Snapshot listing/restore
and explicit deletion likewise belong to those retained legacy tools and their
original authorization boundaries. A metadata tree or stream verification does not prove that every
historical blob, orphan generation or divergent snapshot is recoverable.

The independent [Relay custody exporter](relay-custody-export.md) preserves every
file/directory in explicitly declared inputs with private permissions and
checksummed read-back, without depending on the Relay protocol. Its pinned
synthetic qualification covers retained frames, orphans, snapshot history/forks,
recovery identities and pending state. Copy integrity is explicitly separate from
decryption. The [offline history reader](relay-history-export.md) supplies
operator-facing decrypted access using the fixed historical implementation
outside the product workspace. It checks all declared supported scopes, preserves
opaque originals and reports unsupported data, missing keys, gaps and deletion
boundaries. Its synthetic qualification does not qualify an existing deployment.
Before removing writers/readers, inventory and quiesce all related writers,
preserve consistent private copies of their complete custody trees, and qualify
read-back/decryption using the retained versions and original recipients. Record
missing keys, gaps, divergent heads, unsupported versions and integrity failures
as unresolved failures. Keep original data and working legacy access until this
is verified. Do not copy only a sender watermark or one apparent latest snapshot
and call that a complete export.

No upgrade, source cleanup or normal read automatically stops a service, removes
an installed binary/unit, prunes ciphertext, clears a deletion outbox or deletes
a key/snapshot/recovery artifact. Existing deletion commands retain their explicit
operator authorization and original semantics; retirement invokes none of them.

## Supported synchronization and recovery

Use the [generic mirror protocol](mirror-protocol.md) for source synchronization.
Its receiver can read the redacted content; it does not provide end-to-end
encryption. It is not a complete workspace/native harness backup.

`semon-runtime` has no implemented Relay dependency. The former independent
recovery roadmap is now [#255](https://github.com/onsager-ai/semon/issues/255):
explicit verified/versioned artifacts binding source generations, workspace and
native state, path/thread metadata and durable operation journals where needed.
Publication/read-back verification precedes checkpoint commit; restoration
verifies scope, completeness, hashes and compatibility before launch. Revalidate
current credential authority and writer exclusion, and do not replay unknown
external writes. A Relay lease or epoch alone establishes none of these guarantees.
