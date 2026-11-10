//! Local, explicitly-started origin analytics publication.
//!
//! This service has no host integration or default enablement. Its namespace is
//! supplied by the caller, and each cycle opens a fresh read-only SQLite input.
//! The store root derived by [`analytics_store_root`] is a sibling of the SQLite
//! index, never a path inside the index file.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::Duration,
};

use rusqlite::{Error as SqliteError, ErrorCode};
use semon_asi::{
    Clock, CompactionPolicy, Error as StoreError, PublishError, Store, Versions, publish_cycle,
};

use crate::{SqliteAnalyticsInput, events::session_activity::ACTIVITY_RULES_VERSION};

/// The minimum spacing between requested publication cycles by default.
pub const DEFAULT_ANALYTICS_WRITER_INTERVAL: Duration = Duration::from_secs(2);

const NO_WRITER_RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(30);
const RETIREMENT_CADENCE: Duration = Duration::from_secs(10 * 60);

/// Configuration for one explicitly-enabled local origin writer.
#[derive(Clone, Debug)]
pub struct AnalyticsWriterConfig {
    /// Origin namespace. This is deliberately caller-supplied.
    pub namespace: String,
    /// Read-only native Event Index path.
    pub index_path: PathBuf,
    /// Root that contains `analytics.lock` and the `analytics/` directory.
    pub store_root: PathBuf,
    /// Minimum spacing between publication cycles.
    pub interval: Duration,
    /// Bounded policy used for one compaction attempt after each publication.
    pub compaction_policy: CompactionPolicy,
}

impl AnalyticsWriterConfig {
    pub fn new(
        namespace: impl Into<String>,
        index_path: impl Into<PathBuf>,
        store_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            index_path: index_path.into(),
            store_root: store_root.into(),
            interval: DEFAULT_ANALYTICS_WRITER_INTERVAL,
            compaction_policy: CompactionPolicy::default(),
        }
    }
}

/// Returns the conventional analytics store root beside an SQLite index.
///
/// For `/state/sessions-index.sqlite3` this is `/state/analytics-store`; it is
/// a sibling of the index file, not a child of (or replacement for) that file.
pub fn analytics_store_root(index_path: &Path) -> Option<PathBuf> {
    index_path
        .parent()
        .map(|parent| parent.join("analytics-store"))
}

/// A state suitable for a future host and for deterministic lifecycle tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalyticsWriterStatus {
    Idle,
    Running,
    NoWriter,
    Refused { reason: String },
    Retrying { attempt: u32 },
    Stopped,
}

struct State {
    stop: bool,
    notifications: u64,
    consumed: u64,
    status: AnalyticsWriterStatus,
    #[cfg(test)]
    published: u64,
    #[cfg(test)]
    input_attempts: u64,
    #[cfg(test)]
    retire_attempts: u64,
    #[cfg(test)]
    idle_waits: u64,
    #[cfg(test)]
    waiting: bool,
    #[cfg(test)]
    hold_publish: bool,
    #[cfg(test)]
    publish_held: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// An owned local writer thread. Dropping it stops and joins the thread.
pub struct Handle {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(test)]
pub(crate) struct PublishHold {
    shared: Arc<Shared>,
}

#[cfg(test)]
impl Drop for PublishHold {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        state.hold_publish = false;
        self.shared.changed.notify_all();
    }
}

impl Handle {
    /// Records a successful catalog publication. Notifications coalesce.
    pub fn notify(&self) {
        let mut state = lock(&self.shared.state);
        state.notifications = state.notifications.wrapping_add(1);
        self.shared.changed.notify_one();
    }

    pub fn status(&self) -> AnalyticsWriterStatus {
        lock(&self.shared.state).status.clone()
    }

    /// Stops future scheduling and joins without flushing, retirement, or a
    /// partially-started generation.
    pub fn stop_and_join(mut self) {
        self.shutdown();
    }

    #[cfg(test)]
    pub(crate) fn wait_for_publishes(&self, expected: u64) {
        let mut state = lock(&self.shared.state);
        while state.published < expected && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(
            state.published >= expected,
            "writer stopped before publishing"
        );
    }

    #[cfg(test)]
    pub(crate) fn published(&self) -> u64 {
        lock(&self.shared.state).published
    }

    #[cfg(test)]
    pub(crate) fn wait_for_input_attempts(&self, expected: u64) {
        let mut state = lock(&self.shared.state);
        while state.input_attempts < expected && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(
            state.input_attempts >= expected,
            "writer stopped before opening input"
        );
    }

    #[cfg(test)]
    pub(crate) fn input_attempts(&self) -> u64 {
        lock(&self.shared.state).input_attempts
    }

    #[cfg(test)]
    pub(crate) fn wait_for_retire_attempts(&self, expected: u64) {
        let mut state = lock(&self.shared.state);
        while state.retire_attempts < expected && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(
            state.retire_attempts >= expected,
            "writer stopped before retirement"
        );
    }

    #[cfg(test)]
    pub(crate) fn wait_for_idle_wait(&self) {
        let mut state = lock(&self.shared.state);
        while !(state.waiting && state.status == AnalyticsWriterStatus::Idle) && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(state.waiting && state.status == AnalyticsWriterStatus::Idle);
    }

    #[cfg(test)]
    pub(crate) fn idle_waits(&self) -> u64 {
        lock(&self.shared.state).idle_waits
    }

    #[cfg(test)]
    pub(crate) fn wait_for_idle_wait_after(&self, previous: u64) {
        let mut state = lock(&self.shared.state);
        while state.idle_waits <= previous && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(state.idle_waits > previous);
    }

    #[cfg(test)]
    /// Lets an injected test clock wake a deadline wait after advancing time.
    pub(crate) fn wake_for_clock(&self) {
        self.shared.changed.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn hold_publish(&self) -> PublishHold {
        let mut state = lock(&self.shared.state);
        state.hold_publish = true;
        PublishHold {
            shared: Arc::clone(&self.shared),
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_for_publish_hold(&self) {
        let mut state = lock(&self.shared.state);
        while !state.publish_held && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(state.publish_held);
    }

    #[cfg(test)]
    pub(crate) fn wait_for_refusal(&self) {
        let mut state = lock(&self.shared.state);
        while !matches!(state.status, AnalyticsWriterStatus::Refused { .. }) && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert!(matches!(
            state.status,
            AnalyticsWriterStatus::Refused { .. }
        ));
    }

    #[cfg(test)]
    pub(crate) fn wait_for_status(&self, expected: AnalyticsWriterStatus) {
        let mut state = lock(&self.shared.state);
        while state.status != expected && !state.stop {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert_eq!(state.status, expected);
    }

    fn shutdown(&mut self) {
        {
            let mut state = lock(&self.shared.state);
            state.stop = true;
            self.shared.changed.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts a dedicated local analytics writer using the system clock.
///
/// This is intentionally an explicit lifecycle primitive: no Viewer, MCP,
/// push/receive, or CLI entry point starts it.
pub fn start(config: AnalyticsWriterConfig) -> Handle {
    start_with_clock(config, Arc::new(semon_asi::SystemClock))
}

/// Starts a writer with an injected clock for deterministic crate tests.
pub(crate) fn start_with_clock<C>(config: AnalyticsWriterConfig, clock: Arc<C>) -> Handle
where
    C: Clock + Send + Sync + 'static,
{
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            stop: false,
            notifications: 0,
            consumed: 0,
            status: AnalyticsWriterStatus::Idle,
            #[cfg(test)]
            published: 0,
            #[cfg(test)]
            input_attempts: 0,
            #[cfg(test)]
            retire_attempts: 0,
            #[cfg(test)]
            idle_waits: 0,
            #[cfg(test)]
            waiting: false,
            #[cfg(test)]
            hold_publish: false,
            #[cfg(test)]
            publish_held: false,
        }),
        changed: Condvar::new(),
    });
    let worker_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        // Rust's standard library has no portable thread-priority API.
        .name("semon-analytics-writer".into())
        .spawn(move || run(config, clock, worker_shared))
        .expect("start analytics writer thread");
    Handle {
        shared,
        thread: Some(thread),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Open,
    Reload(Resume),
    Publish,
    Compact,
    Retire,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Resume {
    Publish,
    Compact,
    Retire,
}

impl Resume {
    fn operation(self) -> Operation {
        match self {
            Self::Publish => Operation::Publish,
            Self::Compact => Operation::Compact,
            Self::Retire => Operation::Retire,
        }
    }
}

#[derive(Debug)]
struct Scheduler {
    pending: Option<Operation>,
    refused: Option<Operation>,
    next_cycle: i64,
    next_retirement: i64,
    retry_at: Option<i64>,
    attempt: u32,
}

impl Scheduler {
    fn new(now: i64) -> Self {
        Self {
            pending: None,
            refused: None,
            next_cycle: now,
            next_retirement: now + duration_ms(RETIREMENT_CADENCE),
            retry_at: None,
            attempt: 0,
        }
    }

    fn operation_due(
        &mut self,
        now: i64,
        has_notification: bool,
        has_store: bool,
    ) -> Option<Operation> {
        if let Some(refused) = self.refused {
            if !has_notification {
                return None;
            }
            self.refused = None;
            self.pending = Some(refused);
        }
        if self.pending.is_none() {
            self.pending = Some(if !has_store {
                Operation::Open
            } else if now >= self.next_retirement {
                Operation::Retire
            } else if has_notification {
                Operation::Publish
            } else {
                return None;
            });
        }
        let operation = self.pending.expect("selected pending operation");
        let due = match operation {
            Operation::Publish => self.retry_at.unwrap_or(now).max(self.next_cycle),
            _ => self.retry_at.unwrap_or(now),
        };
        (now >= due).then_some(operation)
    }

    fn deadline(&self, has_notification: bool, has_store: bool) -> Option<i64> {
        if self.refused.is_some() && !has_notification {
            return None;
        }
        if let Some(operation) = self.pending {
            return Some(match operation {
                Operation::Publish => self
                    .retry_at
                    .unwrap_or(self.next_cycle)
                    .max(self.next_cycle),
                _ => self.retry_at.unwrap_or(0),
            });
        }
        if !has_store {
            return Some(0);
        }
        if has_notification {
            Some(self.next_cycle.min(self.next_retirement))
        } else {
            Some(self.next_retirement)
        }
    }

    fn succeeded(&mut self, operation: Operation, now: i64, interval: Duration) {
        self.pending = match operation {
            Operation::Open | Operation::Compact | Operation::Retire => None,
            Operation::Reload(after) => Some(after.operation()),
            Operation::Publish => Some(Operation::Compact),
        };
        if operation == Operation::Publish {
            self.next_cycle = now + duration_ms(interval);
        }
        if operation == Operation::Retire {
            self.next_retirement = now + duration_ms(RETIREMENT_CADENCE);
        }
        self.retry_at = None;
        self.attempt = 0;
    }

    fn retry(&mut self, operation: Operation, now: i64) -> u32 {
        self.pending = Some(operation);
        self.refused = None;
        self.attempt = self.attempt.saturating_add(1);
        self.retry_at = Some(now + retry_delay(self.attempt));
        self.attempt
    }

    fn refuse(&mut self, operation: Operation) {
        self.pending = None;
        self.refused = Some(operation);
        self.retry_at = None;
    }

    fn no_writer(&mut self, now: i64) {
        self.pending = Some(Operation::Open);
        self.retry_at = Some(now + duration_ms(NO_WRITER_RETRY));
        self.attempt = 0;
    }
}

fn run<C: Clock + Send + Sync + 'static>(
    config: AnalyticsWriterConfig,
    clock: Arc<C>,
    shared: Arc<Shared>,
) {
    let mut store = None;
    let mut scheduler = Scheduler::new(clock.now_ms());

    while let Some((operation, notification)) =
        next_operation(&shared, &clock, &mut scheduler, store.is_some())
    {
        match operation {
            Operation::Open => match Store::open_with_clock(&config.store_root, Arc::clone(&clock))
            {
                Ok(opened) => {
                    store = Some(opened);
                    scheduler.succeeded(operation, clock.now_ms(), config.interval);
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(StoreError::WriterBusy) => {
                    scheduler.no_writer(clock.now_ms());
                    set_status(&shared, AnalyticsWriterStatus::NoWriter);
                }
                Err(error) if error.is_operational() => {
                    retry_operation(&shared, &mut scheduler, operation, clock.now_ms());
                }
                Err(error) => {
                    refuse_operation(&shared, &mut scheduler, operation, error.to_string())
                }
            },
            Operation::Publish => {
                #[cfg(test)]
                publish_started(&shared);
                let result = match SqliteAnalyticsInput::open(
                    &config.index_path,
                    config.namespace.clone(),
                ) {
                    Ok(mut input) => publish_cycle(
                        store.as_mut().expect("opened store"),
                        &mut input,
                        &config.namespace,
                        target_versions(),
                    ),
                    Err(error) => Err(PublishError::Input(error)),
                };
                match result {
                    Ok(_) => {
                        consume_notification(&shared, notification);
                        #[cfg(test)]
                        published(&shared);
                        scheduler.succeeded(operation, clock.now_ms(), config.interval);
                        set_status(&shared, AnalyticsWriterStatus::Idle);
                    }
                    Err(PublishError::Input(error)) if is_input_operational(&error) => {
                        consume_notification(&shared, notification);
                        retry_operation(&shared, &mut scheduler, operation, clock.now_ms());
                    }
                    Err(PublishError::Input(error)) => {
                        consume_notification(&shared, notification);
                        refuse_operation(&shared, &mut scheduler, operation, error.to_string());
                    }
                    Err(PublishError::Store(error)) if error.is_operational() => {
                        consume_notification(&shared, notification);
                        retry_operation(
                            &shared,
                            &mut scheduler,
                            Operation::Reload(Resume::Publish),
                            clock.now_ms(),
                        );
                    }
                    Err(PublishError::Store(error)) => {
                        consume_notification(&shared, notification);
                        refuse_operation(
                            &shared,
                            &mut scheduler,
                            Operation::Reload(Resume::Publish),
                            error.to_string(),
                        );
                    }
                }
            }
            Operation::Compact => match store
                .as_mut()
                .expect("opened store")
                .compact(&config.compaction_policy)
            {
                Ok(_) => {
                    scheduler.succeeded(operation, clock.now_ms(), config.interval);
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(error) if error.is_operational() => retry_operation(
                    &shared,
                    &mut scheduler,
                    Operation::Reload(Resume::Compact),
                    clock.now_ms(),
                ),
                Err(error) => refuse_operation(
                    &shared,
                    &mut scheduler,
                    Operation::Reload(Resume::Compact),
                    error.to_string(),
                ),
            },
            Operation::Retire => {
                #[cfg(test)]
                retirement_started(&shared);
                match store.as_mut().expect("opened store").retire() {
                    Ok(_) => {
                        scheduler.succeeded(operation, clock.now_ms(), config.interval);
                        set_status(&shared, AnalyticsWriterStatus::Idle);
                    }
                    Err(error) if error.is_operational() => retry_operation(
                        &shared,
                        &mut scheduler,
                        Operation::Reload(Resume::Retire),
                        clock.now_ms(),
                    ),
                    Err(error) => refuse_operation(
                        &shared,
                        &mut scheduler,
                        Operation::Reload(Resume::Retire),
                        error.to_string(),
                    ),
                }
            }
            Operation::Reload(_) => match store.as_mut().expect("opened store").reload_manifest() {
                Ok(()) => {
                    scheduler.succeeded(operation, clock.now_ms(), config.interval);
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(error) if error.is_operational() => {
                    retry_operation(&shared, &mut scheduler, operation, clock.now_ms());
                }
                Err(error) => {
                    refuse_operation(&shared, &mut scheduler, operation, error.to_string())
                }
            },
        }
    }
    set_status(&shared, AnalyticsWriterStatus::Stopped);
}

fn target_versions() -> Versions {
    Versions {
        transform: semon_asi::ANALYTICS_TRANSFORM_VERSION,
        rules: ACTIVITY_RULES_VERSION,
        price: crate::pricing::snapshot().fingerprint.clone(),
    }
}

fn next_operation<C: Clock>(
    shared: &Shared,
    clock: &C,
    scheduler: &mut Scheduler,
    has_store: bool,
) -> Option<(Operation, u64)> {
    let mut state = lock(&shared.state);
    loop {
        if state.stop {
            return None;
        }
        let now = clock.now_ms();
        let has_notification = state.notifications != state.consumed;
        if let Some(operation) = scheduler.operation_due(now, has_notification, has_store) {
            #[cfg(test)]
            {
                state.waiting = false;
            }
            state.status = AnalyticsWriterStatus::Running;
            shared.changed.notify_all();
            return Some((operation, state.notifications));
        }
        #[cfg(test)]
        {
            state.waiting = true;
            if state.status == AnalyticsWriterStatus::Idle {
                state.idle_waits += 1;
            }
            shared.changed.notify_all();
        }
        match scheduler.deadline(has_notification, has_store) {
            Some(deadline) if deadline > now => {
                let remaining = (deadline - now).max(1) as u64;
                let (next, _) = shared
                    .changed
                    .wait_timeout(state, Duration::from_millis(remaining))
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state = next;
            }
            Some(_) => continue,
            None => {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }
    }
}

fn retry_operation(shared: &Shared, scheduler: &mut Scheduler, operation: Operation, now: i64) {
    let attempt = scheduler.retry(operation, now);
    set_status(shared, AnalyticsWriterStatus::Retrying { attempt });
}

fn refuse_operation(
    shared: &Shared,
    scheduler: &mut Scheduler,
    operation: Operation,
    reason: String,
) {
    scheduler.refuse(operation);
    set_status(shared, AnalyticsWriterStatus::Refused { reason });
}

fn is_input_operational(error: &SqliteError) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(
            ErrorCode::DatabaseBusy
                | ErrorCode::DatabaseLocked
                | ErrorCode::SystemIoFailure
                | ErrorCode::DiskFull
                | ErrorCode::CannotOpen
        )
    )
}

fn retry_delay(attempt: u32) -> i64 {
    let shift = attempt.saturating_sub(1).min(5);
    ((1_i64 << shift) * 1_000).min(duration_ms(MAX_RETRY))
}

fn duration_ms(duration: Duration) -> i64 {
    duration.as_millis().min(i64::MAX as u128) as i64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_status(shared: &Shared, status: AnalyticsWriterStatus) {
    lock(&shared.state).status = status;
    shared.changed.notify_all();
}

fn consume_notification(shared: &Shared, notification: u64) {
    let mut state = lock(&shared.state);
    state.consumed = notification;
}

#[cfg(test)]
fn publish_started(shared: &Shared) {
    let mut state = lock(&shared.state);
    state.input_attempts += 1;
    state.publish_held = state.hold_publish;
    shared.changed.notify_all();
    while state.hold_publish && !state.stop {
        state = shared
            .changed
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    state.publish_held = false;
    shared.changed.notify_all();
}

#[cfg(test)]
fn published(shared: &Shared) {
    let mut state = lock(&shared.state);
    state.published += 1;
    shared.changed.notify_all();
}

#[cfg(test)]
fn retirement_started(shared: &Shared) {
    let mut state = lock(&shared.state);
    state.retire_attempts += 1;
    shared.changed.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> PathBuf {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "semon-analytics-writer-{name}-{}-{serial}",
            std::process::id(),
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn store_root_is_a_sibling_of_the_index() {
        assert_eq!(
            analytics_store_root(Path::new("/state/sessions-index.sqlite3")),
            Some(PathBuf::from("/state/analytics-store")),
        );
    }

    #[test]
    fn unsupported_index_refuses_without_mutating_it_or_checkpointing() {
        let root = temp_root("unsupported");
        let index = root.join("old.sqlite3");
        rusqlite::Connection::open(&index).unwrap();
        let bytes = fs::read(&index).unwrap();
        let store_root = root.join("store");
        let handle = start(AnalyticsWriterConfig::new("machine", &index, &store_root));
        handle.notify();
        handle.wait_for_input_attempts(1);
        handle.wait_for_refusal();
        assert_eq!(fs::read(&index).unwrap(), bytes);
        assert!(semon_asi::read_manifest(&store_root).unwrap().is_none());
        handle.notify();
        handle.wait_for_input_attempts(2);
        handle.wait_for_refusal();
        assert_eq!(handle.input_attempts(), 2);
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn second_writer_reports_no_writer_and_stops() {
        let root = temp_root("writer-lock");
        let held = Store::open(&root).unwrap();
        let handle = start(AnalyticsWriterConfig::new(
            "machine",
            root.join("index.sqlite3"),
            &root,
        ));
        handle.wait_for_status(AnalyticsWriterStatus::NoWriter);
        handle.stop_and_join();
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }
}
