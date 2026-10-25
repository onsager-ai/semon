use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct PgCancellation(Arc<AtomicBool>);

impl PgCancellation {
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }
}

/// One monitor per leased transaction. Scope joins it even on unwind; the
/// connection cannot be reused/dropped or its slot released while cancel runs.
pub(super) struct Monitor {
    stopped: Mutex<bool>,
    changed: Condvar,
    pub(super) attempted: AtomicBool,
}

impl Monitor {
    pub(super) fn new() -> Self {
        Self { stopped: Mutex::new(false), changed: Condvar::new(), attempted: AtomicBool::new(false) }
    }

    pub(super) fn run(&self, control: &PgCancellation, deadline: Instant,
        request: &(dyn Fn() -> Result<(), crate::storage::StorageError> + Send + Sync)) {
        let mut stopped = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        while !*stopped {
            if control.is_cancelled() || Instant::now() >= deadline {
                self.attempted.store(true, Ordering::SeqCst);
                drop(stopped);
                let _ = request(); // no ACK; only the real query outcome matters
                return;
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(5));
            stopped = self.changed.wait_timeout(stopped, wait).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    pub(super) fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_one();
    }
}

pub(super) struct StopOnDrop<'a>(pub(super) &'a Monitor);
impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) { self.0.stop(); }
}
