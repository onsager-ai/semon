# tiny_http, patched for semon

This is tiny_http 0.12.0 as published on crates.io (sha256 `389915df6413a2e74fb181895f933386023c71110878cd0825588928e64cdc82`, upstream commit `212b1c45852fef2093dc1374875a9393c55eb4b9`), with one change. The workspace uses it through `[patch.crates-io]` in the root `Cargo.toml`, so the `=0.12.0` pin and the features stay as they were. It is excluded from the workspace and from clippy (`--no-deps` in `.github/workflows/rust.yml`), so its code stays as published apart from the change below. Upstream's tests, examples, benches and CI files are not copied.

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
