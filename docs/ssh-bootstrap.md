# SSH transport and mirror bootstrap

`semon-ssh` is a reusable Linux/OpenSSH transport for existing servers. It has
no Hub URL, tenancy, provider policy or agent-control capabilities. It supports
unencrypted OpenSSH/PEM private keys accepted by OpenSSH, with Ed25519 host-key
discovery. Passwords, key passphrases, SSH agents and jump hosts are not supported
in this initial slice. Remote requirements are Linux x86_64/aarch64, Python 3,
`flock`, `nohup`, a writable private home and a matching qualified Semon binary.

An embedding application must authorize a destination, resolve and reject
unauthorized addresses, then supply a literal-IP `Target`. Transport never resolves
an independently supplied hostname or reads user SSH configuration. Discover
is unauthenticated and sends no credential. Display `HostKey::fingerprint()` and
require independent, explicit acceptance before calling `check`. Pinned keys are
mandatory for authentication and bootstrap; changed keys fail closed. `check`
only authenticates and inspects tooling. It does not install, enroll or launch.

After explicit authorization, `bootstrap` takes a stable UUID operation identity,
a caller-supplied binary, generic mirror destination and narrow enrollment token.
Secrets travel through SSH stdin, never arguments or returned diagnostics. The
private payload and credential types have no Debug/Serialize lifecycle contract.
OpenSSH uses temporary 0700 directories / 0600 key files removed when futures
finish or are cancelled. Processes inherit only fixed non-secret environment
values; dropping a future kills the local child. Discovery/check/setup deadlines
are 8/12/60 seconds. Output is capped, stderr is classified into fixed errors,
and host output never becomes an error string. Check results are limited to
supported platform/architecture values.

Bootstrap v1 installs in `~/.local/state/semon-ssh/<operation>` using owner-only
files, rejects symlink ancestors, atomically writes files and recovers interrupted
`.new` writes. A setup lock serializes operations and a watch lock excludes duplicate
pushers. Repeat setup leaves a running mirror intact; token replacement after
re-enrollment validates a PID/start-time/boot receipt before restarting its watch
group. It never signals an agent. A caller must persist its enrollment identity
and protected token *before* remote mutation so lost replies can be repaired.
Bootstrap starts a detached background mirror; after server reboot the caller
must test and explicitly repair. No reboot service or compute lifecycle is implied.

`semon push` and its documented mirror protocol remain the only ingestion path.
Enrollment does not grant dispatch, pause, snapshot, restore or server deletion.
Model authentication remains independent. The separately authorized
[execution receipt broker](ssh-execution.md) provides private-driver dispatch and
inspection; native launch and hosted execution adoption remain qualification gates.

Qualification:

```sh
cargo test -p semon-ssh --locked
scripts/ssh-fixture.sh cargo test -p semon-ssh --locked --features real-ssh-tests
```

The second command requires Docker and OpenSSH and fails when its fixture cannot
start. Hub's companion qualification additionally installs this binary, pushes a
synthetic Codex session, reads it through Hub, and exercises repair/disconnect and
native browser forms. Neither test uses a real user's logs or a public SSH host.
