//! [`Stop`]: how a caller ends a push, `--watch` included, from another
//! thread.

use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

/// How often a wait on work running on another thread (a request to the
/// receiver, facts) looks at the stop. A stop waits there at most two of
/// these: one to be seen, one for an answer racing it.
const POLL: Duration = Duration::from_millis(20);

/// A stop handle for [`crate::push_until`]. Clones share one flag; once
/// stopped it stays stopped.
///
/// A stop is seen within about 40 ms at every point a push blocks: the sleep
/// between `--watch` passes wakes at once, a pass checks it before each file
/// and each chunk it reads, and a request to the receiver or a facts
/// collection is waited on from here, so the push returns without waiting
/// for it. Work left running that way finishes on its own thread, holding
/// the state lock until it ends ([`crate::StateLock`]), and its result is
/// dropped: the state file only ever records what the receiver acknowledged
/// before the stop, and no other push with that state runs until the work
/// is done.
#[derive(Clone, Default)]
pub struct Stop {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Stop")
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

impl Stop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the push to end. It returns `Ok(())` promptly, from wherever it
    /// was.
    pub fn stop(&self) {
        *self
            .inner
            .stopped
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = true;
        self.inner.wake.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        *self
            .inner
            .stopped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Sleeps for `duration`, or until stopped. True when stopped.
    pub fn sleep(&self, duration: Duration) -> bool {
        let stopped = self
            .inner
            .stopped
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (stopped, _) = self
            .inner
            .wake
            .wait_timeout_while(stopped, duration, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        *stopped
    }

    /// Runs `work` on a thread of its own and waits for it, or for the stop,
    /// whichever comes first: `Ok(None)` when stopped first. Nothing starts
    /// when already stopped.
    pub(crate) fn run<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> crate::Result<Option<T>> {
        if self.is_stopped() {
            return Ok(None);
        }
        let (answer, answers) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name(format!("semon-push-{name}"))
            .spawn(move || {
                // The waiter may be gone after a stop; the answer is dropped.
                let _ = answer.send(work());
            })
            .map_err(|error| format!("{name}: {error}"))?;
        self.wait(name, &answers)
    }

    /// Waits for one answer from work on another thread, or for the stop:
    /// `Ok(None)` when stopped first. An answer already there wins over a
    /// stop, and one racing it gets one more poll, so what the receiver
    /// acknowledged, or refused, just as the stop came is still seen.
    pub(crate) fn wait<T>(
        &self,
        name: &str,
        answers: &mpsc::Receiver<T>,
    ) -> crate::Result<Option<T>> {
        loop {
            match answers.recv_timeout(POLL) {
                Ok(answer) => return Ok(Some(answer)),
                Err(RecvTimeoutError::Timeout) if self.is_stopped() => {
                    return Ok(answers.recv_timeout(POLL).ok());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("{name}: its thread ended without an answer"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_stop_wakes_a_sleep_at_once() {
        let stop = Stop::new();
        let other = stop.clone();
        let sleeper = thread::spawn(move || {
            let started = Instant::now();
            (other.sleep(Duration::from_secs(30)), started.elapsed())
        });
        thread::sleep(Duration::from_millis(50));
        stop.stop();
        let (stopped, slept) = sleeper.join().unwrap();
        assert!(stopped);
        assert!(slept < Duration::from_secs(2), "slept {slept:?}");
        assert!(stop.sleep(Duration::from_secs(30)), "stays stopped");
    }

    #[test]
    fn a_sleep_without_a_stop_runs_its_time() {
        let started = Instant::now();
        assert!(!Stop::new().sleep(Duration::from_millis(30)));
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn run_returns_the_answer_or_leaves_on_a_stop() {
        let stop = Stop::new();
        assert_eq!(stop.run("test", || 7).unwrap(), Some(7));

        let (release, released) = mpsc::channel::<()>();
        let other = stop.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            other.stop();
        });
        let started = Instant::now();
        let answer = stop
            .run("test", move || {
                let _ = released.recv_timeout(Duration::from_secs(30));
                8
            })
            .unwrap();
        assert_eq!(answer, None);
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(release);
        assert_eq!(stop.run("test", || 9).unwrap(), None, "nothing starts");
    }
}
