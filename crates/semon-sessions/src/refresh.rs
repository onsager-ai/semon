//! [`RefreshPool`]: the few threads that keep every background view fresh.
//!
//! A machine view in [`Refresh::Background`](crate::Refresh::Background) or
//! [`Refresh::OnInvalidate`](crate::Refresh::OnInvalidate) doesn't own a
//! thread. It puts one entry in a pool's queue, due when its next check is,
//! and one of the pool's workers takes the entry once it is due, checks the
//! view (and rebuilds what changed) and queues its next check. A view is in
//! the queue at most once, so the queue never holds more entries than there
//! are views, and a pool never runs more threads than its size, however many
//! machines and cores share it, unless builds hang: then it may run up to
//! twice its size ([`OVERFLOW_AFTER`]).

use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak},
    thread,
    time::{Duration, Instant},
};

use crate::viewer::{IDLE_AFTER, MachineView, lock};

/// A check this late, with every worker of its pool busy, lets a read's
/// nudge start a thread past the pool's size, up to twice it: builds that
/// hang on as many machines as the pool has threads then don't freeze every
/// other machine's checks.
pub(crate) const OVERFLOW_AFTER: Duration = Duration::from_secs(5);

/// A queued entry's key: when it is due, and a count that tells apart two
/// entries due at the same instant.
pub(crate) type Key = (Instant, u64);

/// A fixed-size pool of threads that check and rebuild background views'
/// models off the request path, shared by every [`ViewerCore`] given it.
///
/// Every core uses [`RefreshPool::shared`] unless
/// [`ViewerCore::set_refresh_pool`] gives it another, so a process with any
/// number of cores and machines runs at most that pool's size of refresh
/// threads (twice it while builds hang, see below). Threads start only when
/// a view is queued and none is free, and each stops after 30 s with nothing
/// to run; a process that never serves a background view starts none.
///
/// When the pool falls behind (every worker busy, entries waiting past their
/// time), reads still answer from each view's last model at once: none waits
/// for a build once a model is built. A read of a view whose check is over a
/// second late nudges the pool, which starts a worker if it has room for
/// one; with every worker busy, the check waits its turn, and answers are as
/// far behind the logs as the queue is. A check 5 s late lets the nudge
/// start threads past the pool's size, up to twice it, so builds that hang
/// can't hold every worker; a view whose check is a minute late answers an
/// error (500) instead of an ever older model.
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
    /// [`OVERFLOW_AFTER`], but for tests.
    overflow_after: Duration,
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
            overflow_after: OVERFLOW_AFTER,
        })
    }

    /// [`RefreshPool::new`], starting threads past its size for checks
    /// `overflow_after` late.
    #[cfg(test)]
    pub(crate) fn with_overflow_after(size: usize, overflow_after: Duration) -> Arc<Self> {
        Arc::new(Self {
            size: size.max(1),
            state: Mutex::default(),
            wake: Condvar::new(),
            idle_after: IDLE_AFTER,
            overflow_after,
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
    /// it has one (so a view is never queued twice). With no worker
    /// waiting, a thread starts if the pool has room, unless the caller is
    /// a worker queueing the view it just checked (`by_worker`: it is free
    /// again at once); otherwise the waiting workers are woken when the
    /// entry is due now or comes first (each times its wait by the first
    /// entry). The caller holds the view's `live` lock, which is always
    /// taken before this pool's.
    pub(crate) fn queue(
        self: &Arc<Self>,
        replacing: Option<Key>,
        at: Instant,
        view: Weak<MachineView>,
        by_worker: bool,
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
        let start = if state.waiting == 0 {
            !by_worker && self.reserve(&mut state)
        } else {
            if first || at <= Instant::now() {
                self.wake.notify_all();
            }
            false
        };
        drop(state);
        if start {
            self.start();
        }
        key
    }

    /// Takes a view's entry out of the queue, if it is still there. The
    /// caller holds the view's `live` lock.
    pub(crate) fn remove(&self, key: Key) {
        lock(&self.state).queue.remove(&key);
    }

    /// Counts a thread about to start, if the pool has room for one: the
    /// caller then calls [`RefreshPool::start`] once the lock is released.
    fn reserve(&self, state: &mut PoolState) -> bool {
        self.reserve_up_to(state, self.size)
    }

    /// [`RefreshPool::reserve`], with room for `limit` threads.
    fn reserve_up_to(&self, state: &mut PoolState, limit: usize) -> bool {
        if state.threads >= limit {
            return false;
        }
        state.threads += 1;
        #[cfg(test)]
        state.note_start();
        true
    }

    /// A read found its view's check `late`, past its time: the waiting
    /// workers are woken, or with none waiting a thread starts if the pool
    /// has room (as when the system refused one before). With every worker
    /// busy the check waits its turn, unless it is [`OVERFLOW_AFTER`] late:
    /// then a thread starts past the pool's size, up to twice it, since the
    /// busy workers may be held by builds that hang. Extra threads stop,
    /// like any, after the pool's idle time with nothing to run. The caller
    /// holds the view's `live` lock, which is always taken before this
    /// pool's, as in [`RefreshPool::queue`].
    pub(crate) fn nudge(self: &Arc<Self>, late: Duration) {
        let mut state = lock(&self.state);
        let start = if state.waiting > 0 {
            self.wake.notify_all();
            false
        } else if late >= self.overflow_after {
            self.reserve_up_to(&mut state, self.size * 2)
        } else {
            self.reserve(&mut state)
        };
        drop(state);
        if start {
            self.start();
        }
    }

    /// Starts the worker [`RefreshPool::reserve`] counted, without the
    /// pool's lock held. Without it (the system refused a thread), the next
    /// queue or a read that finds its check overdue tries again.
    fn start(self: &Arc<Self>) {
        let pool = self.clone();
        let started = thread::Builder::new()
            .name("semon-refresh".into())
            .spawn(move || worker(pool));
        if let Err(error) = started {
            lock(&self.state).threads -= 1;
            eprintln!("semon sessions viewer: no refresh thread: {error}");
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

    /// How many of its threads wait for an entry to come due.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        lock(&self.state).waiting
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
    /// The replacement starts while this thread still unwinds, so for that
    /// moment one OS thread more than the pool's size is alive; the count
    /// never is.
    struct Replace(Arc<RefreshPool>);
    impl Drop for Replace {
        fn drop(&mut self) {
            if !thread::panicking() {
                return;
            }
            let mut state = lock(&self.0.state);
            state.threads -= 1;
            let start = !state.queue.is_empty() && self.0.reserve(&mut state);
            drop(state);
            if start {
                self.0.start();
            }
        }
    }
    let replace = Replace(pool);
    let pool = &replace.0;
    let mut worked = Instant::now();
    loop {
        let (key, view, start) = {
            let mut state = lock(&pool.state);
            loop {
                let now = Instant::now();
                if let Some(entry) = state.queue.first_entry()
                    && entry.key().0 <= now
                {
                    let (key, view) = entry.remove_entry();
                    // A burst: the next entry is due too. The wake that
                    // brought this worker passes on to the waiting ones
                    // (each takes one, and passes it on again), and once
                    // none waits, the pool grows to its size.
                    let more = state
                        .queue
                        .first_key_value()
                        .is_some_and(|(next, _)| next.0 <= now);
                    let mut start = false;
                    if more {
                        if state.waiting > 0 {
                            pool.wake.notify_all();
                        } else {
                            start = pool.reserve(&mut state);
                        }
                    }
                    break (key, view, start);
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
        if start {
            pool.start();
        }
        if let Some(view) = view.upgrade() {
            view.run_queued(key);
        }
        worked = Instant::now();
    }
}
