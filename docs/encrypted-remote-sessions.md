# Encrypted remote session trees

Encrypted Relay is being retired under [#380](https://github.com/onsager-ai/semon/issues/380).
These commands remain available to inspect existing encrypted data during migration.
Core and hosted source readers do not depend on Relay. Library users of
`collect_remote` must explicitly enable `semon-sessions/encrypted-relay`; the
existing `semon` CLI enables it while historical decryption/export is retained.
This transitional feature is scheduled for removal with
[#386](https://github.com/onsager-ai/semon/issues/386), after the deployment
inventory and legacy-data migration path are qualified. Do not delete identities,
keys, ciphertext, snapshots or recovery artifacts as part of upgrading.

`semon sessions --remote ENDPOINT` builds the metadata tree from the encrypted
relay, using the local machine's enrolled signing and age identities. This is a
local CLI view: the receiver stores ciphertext and does not build or receive the
decrypted tree. It uses the relay protocol, independently of a hub's plaintext
mirror view.

```sh
semon sessions --remote https://receiver.example \
  --remote-config ~/.config/semon --remote-ca receiver.pem --json
semon sessions --remote https://receiver.example \
  --remote-ca receiver.pem --session SESSION --watch
```

The identity directory defaults to `$XDG_CONFIG_HOME/semon` or
`~/.config/semon`. HTTPS requires a pinned CA (`--remote-ca`, also spelled
`--tls-ca`). Signed loopback HTTP is supported for local receivers; remote
plaintext HTTP is refused. The reader must be an age recipient of the existing
session envelopes and snapshots. Enrolling a signing key alone does not grant
decryption access; use the relay's recipient enrollment and envelope rewrap
commands where necessary.

The usual metadata flags apply: `--all`, `--since DURATION`, `--session ID`,
`--json`, and `--watch`. The default window is 24 hours. A selected session
includes its descendant tree, and recent or live descendants keep their parent
visible. Remote sources are exclusive with local machine roots, `--serve`, and
`--model-json`. Watch polls every two seconds and reports connection errors while
waiting for the receiver to return.

Machine roots identify the current lease holder. Every session node also carries
its machine, so a linked child on another machine remains identifiable. The
`lease` object reports holder, epoch, renewal time, expiry and whether the lease
was active at the receiver's observation time. The running state means the relay
lease is active; it does not assert a remote PID is alive. Expired leases are
ended, and absent leases are unknown. Subagent state also uses its exact parent
Task/Agent call and result. Parent markers and native Codex parent identifiers
link sessions; cwd and timing never establish a parent.

The client requests bounded frame pages, authenticates each encrypted frame,
checks sequence and hash-chain continuity, and checks the final receiver tip.
It parses one decrypted record at a time. Transcript text, reasoning, tool inputs
and results, data keys and ciphertext are never persisted in the view cache.
Only allowlisted metadata, exact parent/call identifiers, and verification
watermarks are cached. Files are mode 0600, separately scoped to receiver origin
and identity beside the configured `--cache` path. Deleting them is safe. An
unchanged stream needs no frame download; an append resumes after its verified
watermark. Older stream generations remain archived on the receiver while the
view follows each stream's latest generation.

A server deletion revision invalidates summaries. A stream with forgotten or
missing frames becomes unknown with its transcript-derived metadata omitted;
the client does not invent a verified prefix across a gap. Authentication,
chain, identity and tip substitution failures reject the update, leaving the
previous cache untouched.

Claude sidecars use encrypted snapshots with an explicit session association:

```sh
semon-relay snapshot --root ~/.claude/projects/PROJECT/SESSION \
  --session SESSION --endpoint https://receiver.example --tls-ca receiver.pem
```

The view enumerates opaque root IDs, downloads current head metadata, and reads
only recognized agent metadata, custom titles and numeric runtime JSON records
in memory. It accepts labels, agent type, worktree path/branch, Task call ID and
Claude bridge links. Unknown fields and `*.key` files are never read by the
viewer. Unchanged snapshot heads need no blob reads. Divergent heads make the
associated label/state unknown instead of silently asserting one branch.

Cloud-only conversations without local carrier files still need their vendor's
API. The remote view does not claim to enumerate those conversations.
