//! [`RefreshPool`]: the few threads that keep every background view fresh.
//!
//! A machine view in [`Refresh::Background`](crate::Refresh::Background) or
//! [`Refresh::OnInvalidate`](crate::Refresh::OnInvalidate) doesn't own a
//! thread. It puts one entry in a pool's queue, due when its next check is,
//! and one of the pool's workers takes the entry once it is due, checks the
//! view (and rebuilds what changed) and queues its next check. A view is in
//! the queue at most once, so the queue never holds more entries than there
//! are views, and a pool never runs more threads than its size, however many
//! machines and cores share it.

use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak},
    thread,
    time::{Duration, Instant},
};

use crate::viewer::{IDLE_AFTER, MachineView, lock};

/// A queued entry's key: when it is due, and a count that tells apart two
/// entries due at the same instant.
pub(crate) type Key = (Instant, u64);

/// A fixed-size pool of threads that check and rebuild background views'
/// models off the request path, shared by every [`ViewerCore`] given it.
///
/// Every core uses [`RefreshPool::shared`] unless
/// [`ViewerCore::set_refresh_pool`] gives it another, so a process with any
/// number of cores and machines runs at most that pool's size of refresh
/// threads. Threads start only when a view is queued and none is free, and
/// each stops after 30 s with nothing to run; a process that never serves a
/// background view starts none.
///
/// When the pool falls behind (every worker busy, entries waiting past their
/// time), a read of a view whose check is over a second late refreshes the
/// view itself before it answers, as the first read of a view does: reads
/// then wait for builds, but no answer is further behind the logs than that.
///
/// [`ViewerCore`]: crate::ViewerCore
/// [`ViewerCore::set_refresh_pool`]: crate::ViewerCore::set_refresh_pool
pub struct RefreshPool {
    size: usize,
    state: Mutex<PoolState>,
    /// Signalled when an entry becomes the first due.
    wake: Condvar,
    /// How long a worker waits with nothing to run before it stops.
    idle_after: Duration,
}

#[derive(Default)]
struct PoolState {
    /// Each queued view once, by when its check is due.
    queue: BTreeMap<Key, Weak<MachineView>>,
    /// The last key's count.
    count: u64,
    /// Worker threads alive.
    threads: usize,
    /// Of those, the ones waiting for an entry to come due.
    waiting: usize,
    /// The most threads alive at once, and how many were ever started.
    #[cfg(test)]
    peak: usize,
    #[cfg(test)]
    started: usize,
}

#[cfg(test)]
impl PoolState {
    fn note_start(&mut self) {
        self.started += 1;
        self.peak = self.peak.max(self.threads);
    }
}

impl RefreshPool {
    /// A pool of at most `size` threads (at least one).
    pub fn new(size: usize) -> Arc<Self> {
        Arc::new(Self {
            size: size.max(1),
            state: Mutex::default(),
            wake: Condvar::new(),
            idle_after: IDLE_AFTER,
        })
    }

    /// The process-wide pool every core uses unless given another: the
    /// number of cores the process may use, at most four.
    pub fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<RefreshPool>> = OnceLock::new();
        SHARED
            .get_or_init(|| {
                let cores = thread::available_parallelism().map_or(1, |cores| cores.get());
                Self::new(cores.min(4))
            })
            .clone()
    }

    /// The most threads this pool runs at once.
    pub fn size(&self) -> usize {
        self.size
    }

    /// How many of its threads are alive now.
    pub fn threads(&self) -> usize {
        lock(&self.state).threads
    }

    /// How many views are queued now: at most one entry each.
    pub fn queued(&self) -> usize {
        lock(&self.state).queue.len()
    }

    /// Queues `view`'s check at `at`, in place of its entry `replacing` if
    /// it has one (so a view is never queued twice), and starts a thread if
    /// none is free and the pool has room. The caller holds the view's
    /// `live` lock, which is always taken before this pool's.
    pub(crate) fn queue(
        self: &Arc<Self>,
        replacing: Option<Key>,
        at: Instant,
        view: Weak<MachineView>,
    ) -> Key {
        let mut state = lock(&self.state);
        if let Some(old) = replacing {
            state.queue.remove(&old);
        }
        state.count += 1;
        let key = (at, state.count);
        let first = state
            .queue
            .first_key_value()
            .is_none_or(|(head, _)| key < *head);
        state.queue.insert(key, view);
        if state.waiting == 0 && state.threads < self.size {
            self.start(&mut state);
        } else if first {
            // A waiting worker times its wait by the first entry.
            self.wake.notify_one();
        }
        key
    }

    /// Takes a view's entry out of the queue, if it is still there. The
    /// caller holds the view's `live` lock.
    pub(crate) fn remove(&self, key: Key) {
        lock(&self.state).queue.remove(&key);
    }

    /// Starts a worker. Without one (the system refused a thread), queued
    /// views are refreshed by their reads once their checks are overdue.
    fn start(self: &Arc<Self>, state: &mut PoolState) {
        let pool = self.clone();
        let started = thread::Builder::new()
            .name("semon-refresh".into())
            .spawn(move || worker(pool));
        match started {
            Ok(_) => {
                state.threads += 1;
                #[cfg(test)]
                state.note_start();
            }
            Err(error) => eprintln!("semon sessions viewer: no refresh thread: {error}"),
        }
    }

    /// How many entries the queue holds for `view`: never more than one.
    #[cfg(test)]
    pub(crate) fn entries_for(&self, view: &Arc<MachineView>) -> usize {
        let view = Arc::downgrade(view);
        lock(&self.state)
            .queue
            .values()
            .filter(|queued| queued.ptr_eq(&view))
            .count()
    }

    /// The most threads alive at once so far, and how many were started.
    #[cfg(test)]
    pub(crate) fn peak_and_started(&self) -> (usize, usize) {
        let state = lock(&self.state);
        (state.peak, state.started)
    }
}

/// A worker: takes each entry once it is due and runs it, until it has had
/// nothing to run for the pool's idle time. The pool's lock is never held
/// while an entry runs.
fn worker(pool: Arc<RefreshPool>) {
    /// A worker whose entry panicked is replaced, so the pool keeps its
    /// size: the view it ran keeps its last model, and is queued again.
    struct Replace(Arc<RefreshPool>);
    impl Drop for Replace {
        fn drop(&mut self) {
            if !thread::panicking() {
                return;
            }
            let mut state = lock(&self.0.state);
            state.threads -= 1;
            if !state.queue.is_empty() && state.threads < self.0.size {
                self.0.start(&mut state);
            }
        }
    }
    let replace = Replace(pool);
    let pool = &replace.0;
    let mut worked = Instant::now();
    loop {
        let (key, view) = {
            let mut state = lock(&pool.state);
            loop {
                let now = Instant::now();
                if let Some(entry) = state.queue.first_entry()
                    && entry.key().0 <= now
                {
                    break entry.remove_entry();
                }
                let idle = now.saturating_duration_since(worked);
                let head = state.queue.first_key_value().map(|(key, _)| key.0);
                // Stops when idle, unless it is the last worker and views
                // are queued: then it waits for the first of them.
                if idle >= pool.idle_after && (head.is_none() || state.threads > 1) {
                    state.threads -= 1;
                    return;
                }
                let mut wait = head.map_or(Duration::MAX, |due| due.saturating_duration_since(now));
                if idle < pool.idle_after {
                    wait = wait.min(pool.idle_after - idle);
                }
                state.waiting += 1;
                state = pool
                    .wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                state.waiting -= 1;
            }
        };
        if let Some(view) = view.upgrade() {
            view.run_queued(key);
        }
        worked = Instant::now();
    }
}
