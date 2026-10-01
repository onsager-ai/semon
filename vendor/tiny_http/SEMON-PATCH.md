# tiny_http, patched for semon

This is tiny_http 0.12.0 as published on crates.io (sha256 `389915df6413a2e74fb181895f933386023c71110878cd0825588928e64cdc82`, upstream commit `212b1c45852fef2093dc1374875a9393c55eb4b9`), with the changes below. The workspace uses it through `[patch.crates-io]` in the root `Cargo.toml`, so the `=0.12.0` pin and the features stay as they were. It is excluded from the workspace and from clippy (`--no-deps` in `.github/workflows/rust.yml`), so its code stays as published apart from the changes below. Upstream's tests, examples, benches and CI files are not copied.

## The change

`src/util/task_pool.rs`, `TaskPool::spawn`:

```diff
-        if self.sharing.waiting_tasks.load(Ordering::Acquire) == 0 {
+        if self.sharing.waiting_tasks.load(Ordering::Acquire) <= queue.len() {
             self.add_thread(Some(code));
         } else {
             queue.push_back(code);
             self.sharing.condvar.notify_one();
         }
```

plus a comment above it.

## Why

The server's accept thread hands each new connection to `spawn`. A connection's task runs for as long as the client keeps the connection alive. Upstream queues the task whenever `waiting_tasks > 0`. A worker registers in `waiting_tasks` while it holds the queue lock, and it leaves only after it has been woken and has taken the lock again. When connections arrive in a burst, `spawn` runs again before the woken workers get the lock back, so it still counts them as idle. It queues the connection, `notify_one` has nobody left to wake, and no thread is added. The connection is read only when some other connection closes, which a browser may put off for about 10 s. That was the viewer's stalled page load in CI (onsager-ai/semon#68, #74).

Every change to `waiting_tasks` and to the queue happens under the queue lock. Each queued task has a waiting worker that has already been woken for it, or that will pop it when it next takes the lock. So "more waiting workers than queued tasks" means a worker is free for this one. Otherwise `spawn` starts a thread, just as upstream does when nobody is waiting.

`crates/semon-sessions/tests/viewer_server.rs` reproduces the stall: without this change, 4 of 8 connections in a burst waited until other connections closed.

## When to remove it

Remove it when the viewer moves to semon's own HTTP code, with SSE. If the relay's receiver still uses tiny_http by then (onsager-ai/semon#75), keep the patch until that moves as well. Then delete this directory, the `[patch.crates-io]` entry and the workspace `exclude`, and drop `--no-deps` from the clippy step unless something else needs it.

The two upstream rustc warnings (`unused import: SequentialWriter` and `trait MustBeShareDummy is never used`) are expected. Do not run `cargo clippy --fix` on the vendored crate; keep unrelated upstream code unchanged.

## Bounded receiver connections (#75)

`Server::from_listener_with_limits` is an additive semon API. It acquires a
connection permit in the accept loop before dispatching work, closes excess
connections, and applies read/write socket timeouts before a TLS handshake.
TLS handshakes now execute in connection workers rather than blocking the
accept loop. A permit stays alive through the kept-alive connection task and
is returned on timeout, handshake failure or disconnect. Existing constructors
use an unlimited policy; the relay opts into 128 connections and ten-second
I/O deadlines. Deadlines apply to each socket operation, rather than imposing a
maximum duration on an actively transferring request.

`crates/semon-relay/tests/security.rs` covers excess-peer rejection, idle-slot
recovery, incomplete TLS handshakes alongside an authenticated sender, and the
existing burst and signed-header/body-limit checks.
