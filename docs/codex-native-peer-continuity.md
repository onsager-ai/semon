# Native controller continuity — dependent on #310

The full native qualification run on Ubuntu 22.04 at `7b2dca0` passed the
standalone direct-tool shell probe, but failed the existing replacement-controller
exclusion assertion in `codex-local.py:724`:
https://github.com/onsager-ai/semon/actions/runs/37274265405 .

The old continuity guard compared only `/proc/PID/stat` start time and the
rendezvous target socket's device/inode. A killed child retained as a zombie still
has its old PID/start time; a new listener can reuse an inode. Those observations
cannot authenticate the live server accepting a newly connected transport.

The confined writable driver now rejects dead process states (`Z`, `X`, `x`) and
checks Linux `SO_PEERCRED` on the connected Unix transport **before** WebSocket
initialization or thread resume. Its PID must match the exact launcher-supplied
native controller and its UID must match the private socket owner. The binding is
obtained through rustix's safe API, preserving the crate's `forbid(unsafe_code)`.
Non-Linux writable peer qualification refuses; passive observation remains read-only.
Credentials, executor confinement and network policy are unchanged. Socket peer
identity is continuity evidence; it grants no SSH execution or product authority.

Focused regressions retain a killed child without reaping it, reject a wrong
kernel peer PID/UID, and verify that a same-owner replacement receives zero native
protocol bytes. Existing lost-write regression still retains Unknown without replay.

Local native qualification at the corrected tree passed:
- Existing local product probe, including the exact replacement-server rejection,
  reconnect/lost HTTP result, approvals and escalated executor isolation. Browser
  and tmux cases were unavailable locally.
- Fresh standalone operation `30700000000000000000000000000012`: actual direct
  `exec_command`, shell exit 0 and exact final `SEMON_EXECUTED_307:42`, private
  synthetic credential excluded from prompts/executor/files, one tool call despite
  duplicate inspection, native reaping and no controller/executor processes left.
- Root locked Rust checks and focused control tests.

The operation was not replayed. Its private native state was retained for
reconciliation. The native provider is synthetic; this does not establish live
OpenRouter access/execution or native SSH deployment/recovery. #307/#303, managed
sandbox/approval #309 and hosted integration Hub #102 retain their adoption gates.
Hub execution actions remain absent.

Reproduce using the exact same native package and commands documented in
[codex-openrouter-qualification.md](codex-openrouter-qualification.md), compiling
in a distinct CARGO_TARGET_DIR and choosing a fresh stable operation identity.
The full native workflow additionally supplies tmux and the browser dependency.
