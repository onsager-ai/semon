//! Local, explicitly-started origin analytics publication.
//!
//! This service has no host integration or default enablement. Its namespace is
//! supplied by the caller, and each cycle opens a fresh read-only SQLite input.
//! The store root derived by [`analytics_store_root`] is a sibling of the SQLite
//! index, never a path inside the index file.
//!
//! Operational retries cover direct store I/O, Parquet `External` I/O, and
//! SQLite busy/locked/I/O/full/cannot-open errors. Parquet page-read I/O can
//! surface as an Arrow error after losing its typed I/O source; it fails safe
//! by latching `Refused` until the next notification, like structural errors.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::Duration,
};

use rusqlite::{Error as SqliteError, ErrorCode};
use semon_asi::{
    Clock, CompactionPolicy, Error as StoreError, InputBudget, PublishError, Store, Versions,
    publish_cycle,
};

use crate::{SqliteAnalyticsInput, events::session_activity::ACTIVITY_RULES_VERSION};

/// The minimum spacing between requested publication cycles by default.
pub const DEFAULT_ANALYTICS_WRITER_INTERVAL: Duration = Duration::from_secs(2);

/// A request refuses after three operational failures, with 1s/2s backoffs.
const REQUEST_FAILURE_LIMIT: u32 = 3;

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
    /// Input work per generation. Incomplete chunks continue at `interval`
    /// without requiring another external notification.
    pub input_budget: InputBudget,
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
            input_budget: InputBudget::default(),
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
    requests: BTreeMap<u64, Option<AnalyticsWriterStatus>>,
    #[cfg(test)]
    published: u64,
    #[cfg(test)]
    input_attempts: u64,
    #[cfg(test)]
    store_attempts: u64,
    #[cfg(test)]
    retire_attempts: u64,
    #[cfg(test)]
    idle_waits: u64,
    #[cfg(test)]
    waiting: bool,
    #[cfg(test)]
    hold_publish: bool,
    #[cfg(test)]
    hold_compact: bool,
    #[cfg(test)]
    compact_held: bool,
    #[cfg(test)]
    publish_held: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                stop: false,
                notifications: 0,
                consumed: 0,
                status: AnalyticsWriterStatus::Idle,
                requests: BTreeMap::new(),
                #[cfg(test)]
                published: 0,
                #[cfg(test)]
                input_attempts: 0,
                #[cfg(test)]
                store_attempts: 0,
                #[cfg(test)]
                retire_attempts: 0,
                #[cfg(test)]
                idle_waits: 0,
                #[cfg(test)]
                waiting: false,
                #[cfg(test)]
                hold_publish: false,
                #[cfg(test)]
                hold_compact: false,
                #[cfg(test)]
                compact_held: false,
                #[cfg(test)]
                publish_held: false,
            }),
            changed: Condvar::new(),
        }
    }
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
        state.hold_compact = false;
        self.shared.changed.notify_all();
    }
}

impl Handle {
    /// Records a successful catalog publication. Notifications coalesce.
    pub fn notify(&self) {
        let mut state = lock(&self.shared.state);
        state.notifications = state.notifications.wrapping_add(1);
        self.shared.changed.notify_all();
    }

    pub fn status(&self) -> AnalyticsWriterStatus {
        lock(&self.shared.state).status.clone()
    }

    /// Requests publication through input completion and waits for it to settle.
    ///
    /// A successful request includes the final chunk's bounded compaction step.
    /// Refused and no-writer states are returned to the caller rather than hidden
    /// behind a later notification.
    pub fn publish_and_wait(&self) -> AnalyticsWriterStatus {
        self.publish_and_wait_until(|| false)
    }

    /// Cancellation is checked every 20 ms while waiting. The caller then stops
    /// and joins the writer; an operation already in progress finishes first.
    pub fn publish_and_wait_until(&self, cancelled: impl Fn() -> bool) -> AnalyticsWriterStatus {
        let mut state = lock(&self.shared.state);
        state.notifications += 1;
        let target = state.notifications;
        state.requests.insert(target, None);
        self.shared.changed.notify_all();
        loop {
            if let Some(Some(outcome)) = state.requests.get(&target) {
                let outcome = outcome.clone();
                state.requests.remove(&target);
                return outcome;
            }
            if state.stop || cancelled() {
                state.requests.remove(&target);
                return AnalyticsWriterStatus::Stopped;
            }
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
        }
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
    pub(crate) fn unsettled_requests(&self) -> usize {
        lock(&self.shared.state)
            .requests
            .values()
            .filter(|outcome| outcome.is_none())
            .count()
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
        // Serialize the wake with predicate evaluation and entering the wait,
        // just like notify(), even though the clock itself is independent.
        let _state = lock(&self.shared.state);
        self.shared.changed.notify_all();
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
    pub(crate) fn wait_for_store_refusal(&self, expected: u64) {
        let mut state = lock(&self.shared.state);
        while !(state.store_attempts >= expected
            && state.waiting
            && matches!(state.status, AnalyticsWriterStatus::Refused { .. }))
            && !state.stop
        {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        assert_eq!(state.store_attempts, expected);
        assert!(state.waiting);
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
/// push/receive, or ordinary CLI read starts it. Materialize enables it explicitly.
pub fn start(config: AnalyticsWriterConfig) -> Handle {
    start_with_clock(config, Arc::new(semon_asi::SystemClock))
}

/// Starts a writer with an injected clock for deterministic crate tests.
pub(crate) fn start_with_clock<C>(config: AnalyticsWriterConfig, clock: Arc<C>) -> Handle
where
    C: Clock + Send + Sync + 'static,
{
    start_thread(config, clock, None)
}

/// Ownership acquired before namespace creation or native Catalog mutation.
pub struct Owner {
    store: Store<Arc<semon_asi::SystemClock>>,
    clock: Arc<semon_asi::SystemClock>,
}

impl Owner {
    /// Validate existing SQLite bytes read-only before creating analytics state.
    /// Store opening takes its lock before touching the analytics directory.
    pub fn acquire(index: &Path, root: &Path) -> Result<Self, String> {
        match std::fs::symlink_metadata(index) {
            Ok(_) => {
                SqliteAnalyticsInput::open(index, "validation")
                    .map_err(|error| format!("analytics writer refused: {error}"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("analytics writer refused: {error}")),
        }
        let clock = Arc::new(semon_asi::SystemClock);
        let store =
            Store::open_with_clock(root, Arc::clone(&clock)).map_err(|error| match error {
                StoreError::WriterBusy => "analytics writer busy".to_owned(),
                error => format!("analytics writer refused: {error}"),
            })?;
        Ok(Self { store, clock })
    }

    pub fn start(self, config: AnalyticsWriterConfig) -> Handle {
        start_thread(config, self.clock, Some(self.store))
    }
}

fn start_thread<C: Clock + Send + Sync + 'static>(
    config: AnalyticsWriterConfig,
    clock: Arc<C>,
    store: Option<Store<Arc<C>>>,
) -> Handle {
    let shared = Arc::new(Shared::new());
    let worker_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        // Rust's standard library has no portable thread-priority API.
        .name("semon-analytics-writer".into())
        .spawn(move || run(config, clock, worker_shared, store))
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
    // A paused publication owns its retry attempt and total failure budget.
    publication_after_retirement: Option<(u32, u32)>,
    refused: Option<Operation>,
    next_cycle: i64,
    next_retirement: i64,
    retry_at: Option<i64>,
    attempt: u32,
    failures: u32,
}

impl Scheduler {
    fn new(now: i64) -> Self {
        Self {
            pending: None,
            publication_after_retirement: None,
            refused: None,
            next_cycle: now,
            next_retirement: now.saturating_add(duration_ms(RETIREMENT_CADENCE)),
            retry_at: None,
            attempt: 0,
            failures: 0,
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
            self.failures = 0;
            self.pending = Some(refused);
        }
        // Leave fresh publication demand in the notification count until its
        // interval is due, so it cannot hide an earlier retirement deadline.
        // An already pending operation belongs to a required recovery chain.
        if self.pending.is_none() {
            self.pending = Some(if !has_store {
                Operation::Open
            } else if now >= self.next_retirement {
                Operation::Retire
            } else if has_notification && now >= self.next_cycle {
                Operation::Publish
            } else {
                return None;
            });
        }
        let operation = self.pending.expect("selected pending operation");
        if operation == Operation::Publish
            && now < self.next_cycle
            && now >= self.next_retirement
            && self.retry_at.is_none_or(|deadline| now >= deadline)
        {
            self.publication_after_retirement = Some((self.attempt, self.failures));
            self.attempt = 0;
            self.failures = 0;
            self.retry_at = None;
            self.pending = Some(Operation::Retire);
            return Some(Operation::Retire);
        }
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
                    .max(self.next_cycle)
                    .min(self.retry_at.unwrap_or(0).max(self.next_retirement)),
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

    fn succeeded(&mut self, operation: Operation, now: i64) {
        self.pending = match operation {
            Operation::Open | Operation::Compact | Operation::Retire => None,
            Operation::Reload(after) => Some(after.operation()),
            Operation::Publish => Some(Operation::Compact),
        };
        if operation == Operation::Retire {
            self.next_retirement = now.saturating_add(duration_ms(RETIREMENT_CADENCE));
        }
        self.retry_at = None;
        // Reload is only a barrier: the operation it resumes has not recovered.
        if !matches!(operation, Operation::Reload(_)) {
            self.attempt = 0;
        }
        // These complete their operation chain; publication success still has
        // compaction to settle. A paused publication restores its own counters.
        if matches!(
            operation,
            Operation::Open | Operation::Retire | Operation::Compact
        ) {
            self.failures = 0;
        }
        if operation == Operation::Retire
            && let Some((attempt, failures)) = self.publication_after_retirement.take()
        {
            self.pending = Some(Operation::Publish);
            self.attempt = attempt;
            self.failures = failures;
        }
    }

    fn publication_attempted(&mut self, now: i64, interval: Duration) {
        // Even an error may follow a visible manifest rename. Conservatively
        // space every attempt, including input failures, before any reload.
        self.next_cycle = now.saturating_add(duration_ms(interval));
    }

    fn retry(&mut self, operation: Operation, now: i64) -> u32 {
        self.pending = Some(operation);
        self.refused = None;
        self.attempt = self.attempt.saturating_add(1);
        self.retry_at = Some(now.saturating_add(retry_delay(self.attempt)));
        self.attempt
    }

    fn refuse(&mut self, operation: Operation) {
        self.pending = None;
        self.refused = Some(operation);
        self.retry_at = None;
    }

    fn no_writer(&mut self, now: i64) {
        self.pending = Some(Operation::Open);
        self.retry_at = Some(now.saturating_add(duration_ms(NO_WRITER_RETRY)));
        self.attempt = 0;
    }
}

fn run<C: Clock + Send + Sync + 'static>(
    config: AnalyticsWriterConfig,
    clock: Arc<C>,
    shared: Arc<Shared>,
    mut store: Option<Store<Arc<C>>>,
) {
    let mut publication = 0;
    let mut publication_complete = false;
    let mut scheduler = Scheduler::new(clock.now_ms());

    while let Some((operation, notification)) =
        next_operation(&shared, &clock, &mut scheduler, store.is_some())
    {
        match operation {
            Operation::Open => match Store::open_with_clock(&config.store_root, Arc::clone(&clock))
            {
                Ok(opened) => {
                    store = Some(opened);
                    scheduler.succeeded(operation, clock.now_ms());
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(StoreError::WriterBusy) => {
                    scheduler.no_writer(clock.now_ms());
                    consume_notification(&shared, notification);
                    settle(&shared, notification, AnalyticsWriterStatus::NoWriter);
                    set_status(&shared, AnalyticsWriterStatus::NoWriter);
                }
                Err(error) if error.is_operational() => {
                    retry_operation(
                        &shared,
                        &mut scheduler,
                        operation,
                        clock.now_ms(),
                        notification,
                        error.to_string(),
                    );
                }
                Err(error) => refuse_operation(
                    &shared,
                    &mut scheduler,
                    operation,
                    notification,
                    error.to_string(),
                ),
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
                        config.input_budget,
                    ),
                    Err(error) => Err(PublishError::Input(error)),
                };
                publication = notification;
                scheduler.publication_attempted(clock.now_ms(), config.interval);
                consume_notification(&shared, notification);
                match result {
                    Ok(progress) => {
                        publication_complete = progress.complete;
                        if !progress.complete {
                            // Also resume after a retry consumed the original
                            // notification; each chunk still observes interval.
                            let mut state = lock(&shared.state);
                            state.notifications = state.notifications.wrapping_add(1);
                        }
                        scheduler.succeeded(operation, clock.now_ms());
                        #[cfg(test)]
                        published(&shared);
                        set_status(&shared, AnalyticsWriterStatus::Idle);
                    }
                    Err(PublishError::Input(error)) if is_input_operational(&error) => {
                        retry_operation(
                            &shared,
                            &mut scheduler,
                            operation,
                            clock.now_ms(),
                            notification,
                            error.to_string(),
                        );
                    }
                    Err(PublishError::Input(error)) => {
                        refuse_operation(
                            &shared,
                            &mut scheduler,
                            operation,
                            notification,
                            error.to_string(),
                        );
                    }
                    Err(PublishError::Store(error)) if error.is_operational() => {
                        retry_operation(
                            &shared,
                            &mut scheduler,
                            Operation::Reload(Resume::Publish),
                            clock.now_ms(),
                            notification,
                            error.to_string(),
                        );
                    }
                    Err(PublishError::Store(error)) => {
                        refuse_operation(
                            &shared,
                            &mut scheduler,
                            Operation::Reload(Resume::Publish),
                            notification,
                            error.to_string(),
                        );
                    }
                }
            }
            Operation::Compact => {
                #[cfg(test)]
                compact_started(&shared);
                match store
                    .as_mut()
                    .expect("opened store")
                    .compact(&config.compaction_policy)
                {
                    Ok(_) => {
                        scheduler.succeeded(operation, clock.now_ms());
                        if publication_complete {
                            settle(&shared, publication, AnalyticsWriterStatus::Idle);
                        }
                        set_status(&shared, AnalyticsWriterStatus::Idle);
                    }
                    Err(error) if error.is_operational() => retry_operation(
                        &shared,
                        &mut scheduler,
                        Operation::Reload(Resume::Compact),
                        clock.now_ms(),
                        notification,
                        error.to_string(),
                    ),
                    Err(error) => refuse_operation(
                        &shared,
                        &mut scheduler,
                        Operation::Reload(Resume::Compact),
                        notification,
                        error.to_string(),
                    ),
                }
            }
            Operation::Retire => {
                #[cfg(test)]
                retirement_started(&shared);
                match store.as_mut().expect("opened store").retire() {
                    Ok(_) => {
                        scheduler.succeeded(operation, clock.now_ms());
                        set_status(&shared, AnalyticsWriterStatus::Idle);
                    }
                    Err(error) if error.is_operational() => retry_operation(
                        &shared,
                        &mut scheduler,
                        Operation::Reload(Resume::Retire),
                        clock.now_ms(),
                        notification,
                        error.to_string(),
                    ),
                    Err(error) => refuse_operation(
                        &shared,
                        &mut scheduler,
                        Operation::Reload(Resume::Retire),
                        notification,
                        error.to_string(),
                    ),
                }
            }
            Operation::Reload(_) => match store.as_mut().expect("opened store").reload_manifest() {
                Ok(()) => {
                    scheduler.succeeded(operation, clock.now_ms());
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(error) if error.is_operational() => {
                    retry_operation(
                        &shared,
                        &mut scheduler,
                        operation,
                        clock.now_ms(),
                        notification,
                        error.to_string(),
                    );
                }
                Err(error) => refuse_operation(
                    &shared,
                    &mut scheduler,
                    operation,
                    notification,
                    error.to_string(),
                ),
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
                if matches!(operation, Operation::Open | Operation::Reload(_)) {
                    state.store_attempts += 1;
                }
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

fn retry_operation(
    shared: &Shared,
    scheduler: &mut Scheduler,
    operation: Operation,
    now: i64,
    notification: u64,
    reason: String,
) {
    scheduler.failures += 1;
    if scheduler.failures >= REQUEST_FAILURE_LIMIT {
        refuse_operation(
            shared,
            scheduler,
            operation,
            notification,
            format!(
                "operational retry budget exhausted after {} failures: {reason}",
                scheduler.failures
            ),
        );
    } else {
        let attempt = scheduler.retry(operation, now);
        set_status(shared, AnalyticsWriterStatus::Retrying { attempt });
    }
}

fn refuse_operation(
    shared: &Shared,
    scheduler: &mut Scheduler,
    operation: Operation,
    notification: u64,
    reason: String,
) {
    consume_notification(shared, notification);
    scheduler.refuse(operation);
    let outcome = AnalyticsWriterStatus::Refused { reason };
    settle(shared, notification, outcome.clone());
    set_status(shared, outcome);
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

fn settle(shared: &Shared, generation: u64, outcome: AnalyticsWriterStatus) {
    let mut state = lock(&shared.state);
    for (_, request) in state.requests.range_mut(..=generation) {
        if request.is_none() {
            *request = Some(outcome.clone());
        }
    }
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
fn compact_started(shared: &Shared) {
    let mut state = lock(&shared.state);
    state.compact_held = state.hold_compact;
    shared.changed.notify_all();
    while state.hold_compact && !state.stop {
        state = shared
            .changed
            .wait(state)
            .unwrap_or_else(|error| error.into_inner());
    }
    state.compact_held = false;
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
    fn scheduler_refusals_consume_only_the_selected_notification() {
        for operation in [
            Operation::Open,
            Operation::Publish,
            Operation::Reload(Resume::Publish),
            Operation::Reload(Resume::Compact),
            Operation::Reload(Resume::Retire),
        ] {
            for notified_during_work in [false, true] {
                let shared = Shared::new();
                let mut scheduler = Scheduler::new(0);
                lock(&shared.state).notifications = 1;
                let snapshot = lock(&shared.state).notifications;
                if notified_during_work {
                    lock(&shared.state).notifications += 1;
                }
                refuse_operation(
                    &shared,
                    &mut scheduler,
                    operation,
                    snapshot,
                    "invalid".into(),
                );
                let state = lock(&shared.state);
                assert_eq!(state.consumed, snapshot);
                let has_notification = state.notifications != state.consumed;
                assert_eq!(has_notification, notified_during_work);
                assert_eq!(
                    scheduler.operation_due(
                        1_000_000,
                        has_notification,
                        operation != Operation::Open
                    ),
                    notified_during_work.then_some(operation),
                );
                if !notified_during_work {
                    assert_eq!(scheduler.deadline(false, true), None);
                    assert!(matches!(
                        state.status,
                        AnalyticsWriterStatus::Refused { .. }
                    ));
                }
            }
        }
    }

    #[test]
    fn scheduler_backoff_survives_successful_reloads_until_the_operation_succeeds() {
        for resume in [Resume::Publish, Resume::Compact, Resume::Retire] {
            let shared = Shared::new();
            let mut scheduler = Scheduler::new(0);
            let mut now = 0;
            let reload = Operation::Reload(resume);
            for (index, delay) in [1_000, 2_000].into_iter().enumerate() {
                let attempt = index as u32 + 1;
                retry_operation(
                    &shared,
                    &mut scheduler,
                    reload,
                    now,
                    0,
                    "synthetic I/O".into(),
                );
                assert_eq!(
                    lock(&shared.state).status,
                    AnalyticsWriterStatus::Retrying { attempt }
                );
                assert_eq!(scheduler.deadline(true, true), Some(now + delay));
                assert_eq!(scheduler.operation_due(now + delay - 1, true, true), None);
                now += delay;
                assert_eq!(scheduler.operation_due(now, true, true), Some(reload));
                scheduler.succeeded(reload, now);
                assert_eq!(scheduler.attempt, attempt);
                assert_eq!(
                    scheduler.operation_due(now, false, true),
                    Some(resume.operation())
                );
            }
            scheduler.succeeded(resume.operation(), now);
            assert_eq!(scheduler.attempt, 0);
            assert_eq!(scheduler.retry(reload, now), 1);
        }
    }

    #[test]
    fn operational_budget_exhausts_across_reloads_with_last_error() {
        let shared = Shared::new();
        lock(&shared.state).requests.insert(1, None);
        let mut scheduler = Scheduler::new(0);
        for attempt in 1..=REQUEST_FAILURE_LIMIT {
            retry_operation(
                &shared,
                &mut scheduler,
                Operation::Reload(Resume::Publish),
                0,
                1,
                format!("synthetic failure {attempt}"),
            );
            if attempt < REQUEST_FAILURE_LIMIT {
                scheduler.succeeded(Operation::Reload(Resume::Publish), 0);
                assert!(lock(&shared.state).requests[&1].is_none());
            }
        }
        let state = lock(&shared.state);
        let Some(AnalyticsWriterStatus::Refused { reason }) = &state.requests[&1] else {
            panic!("request did not fail");
        };
        assert!(reason.contains("budget exhausted"));
        assert!(reason.contains("synthetic failure 3"));
        assert!(scheduler.refused.is_some());
        assert_eq!(state.consumed, 1);
    }

    #[test]
    fn successful_maintenance_gives_the_next_request_a_fresh_failure_budget() {
        for operation in [Operation::Open, Operation::Retire, Operation::Compact] {
            let shared = Shared::new();
            let mut scheduler = Scheduler::new(0);
            let recovery = match operation {
                Operation::Open => Operation::Open,
                Operation::Retire => Operation::Reload(Resume::Retire),
                Operation::Compact => Operation::Reload(Resume::Compact),
                _ => unreachable!(),
            };
            for _ in 0..2 {
                retry_operation(
                    &shared,
                    &mut scheduler,
                    recovery,
                    0,
                    0,
                    "maintenance I/O".into(),
                );
                if matches!(recovery, Operation::Reload(_)) {
                    scheduler.succeeded(recovery, 0);
                }
            }
            assert_eq!(scheduler.failures, 2);
            scheduler.succeeded(operation, 0);
            assert_eq!(scheduler.failures, 0);
            lock(&shared.state).requests.insert(1, None);
            for failure in 1..=REQUEST_FAILURE_LIMIT {
                retry_operation(
                    &shared,
                    &mut scheduler,
                    Operation::Reload(Resume::Publish),
                    0,
                    1,
                    format!("request failure {failure}"),
                );
                if failure < REQUEST_FAILURE_LIMIT {
                    assert!(lock(&shared.state).requests[&1].is_none());
                    scheduler.succeeded(Operation::Reload(Resume::Publish), 0);
                }
            }
            let state = lock(&shared.state);
            let Some(AnalyticsWriterStatus::Refused { reason }) = &state.requests[&1] else {
                panic!("request did not exhaust its fresh budget");
            };
            assert!(reason.contains("after 3 failures: request failure 3"));
        }
    }

    #[test]
    fn interrupted_publication_restores_its_own_failure_budget_after_retirement() {
        let shared = Shared::new();
        lock(&shared.state).requests.insert(1, None);
        let mut scheduler = Scheduler::new(0);
        let reload_publish = Operation::Reload(Resume::Publish);
        scheduler.publication_attempted(0, Duration::from_secs(3_600));
        retry_operation(
            &shared,
            &mut scheduler,
            reload_publish,
            0,
            1,
            "publication I/O".into(),
        );
        scheduler.succeeded(reload_publish, 1_000);
        assert_eq!(
            scheduler.operation_due(600_000, false, true),
            Some(Operation::Retire)
        );
        assert_eq!(scheduler.publication_after_retirement, Some((1, 1)));
        assert_eq!(scheduler.failures, 0);
        for _ in 0..2 {
            retry_operation(
                &shared,
                &mut scheduler,
                Operation::Reload(Resume::Retire),
                600_000,
                1,
                "retirement I/O".into(),
            );
            scheduler.succeeded(Operation::Reload(Resume::Retire), 600_000);
        }
        scheduler.succeeded(Operation::Retire, 600_000);
        assert_eq!(scheduler.pending, Some(Operation::Publish));
        assert_eq!(scheduler.attempt, 1);
        assert_eq!(scheduler.failures, 1);
        retry_operation(
            &shared,
            &mut scheduler,
            reload_publish,
            3_600_000,
            1,
            "publication I/O again".into(),
        );
        assert_eq!(scheduler.failures, 2);
        assert!(lock(&shared.state).requests[&1].is_none());
        scheduler.succeeded(reload_publish, 3_602_000);
        retry_operation(
            &shared,
            &mut scheduler,
            reload_publish,
            3_602_000,
            1,
            "publication I/O last".into(),
        );
        let state = lock(&shared.state);
        let Some(AnalyticsWriterStatus::Refused { reason }) = &state.requests[&1] else {
            panic!("publication lost its earlier failure");
        };
        assert!(reason.contains("after 3 failures: publication I/O last"));
    }

    fn wait_until(handle: &Handle, predicate: impl Fn(&State) -> bool) {
        let mut state = lock(&handle.shared.state);
        while !predicate(&state) {
            let (next, timeout) = handle
                .shared
                .changed
                .wait_timeout(state, Duration::from_secs(10))
                .unwrap_or_else(|error| error.into_inner());
            state = next;
            assert!(
                !timeout.timed_out(),
                "writer did not reach expected state: {:?}",
                state.status
            );
        }
    }

    #[test]
    fn request_waits_for_compaction_and_a_later_request_needs_its_own_publication() {
        let root = temp_root("request-completion");
        let cache = root.join("index.json");
        drop(crate::events::EventCache::open_scoped(&cache).unwrap());
        let index = crate::events::EventCache::path(&cache);
        let mut config = AnalyticsWriterConfig::new("machine", index, root.join("store"));
        config.interval = Duration::ZERO;
        let handle = start(config);
        lock(&handle.shared.state).hold_compact = true;
        let hold = PublishHold {
            shared: Arc::clone(&handle.shared),
        };
        thread::scope(|scope| {
            let (send, receive) = std::sync::mpsc::channel();
            let handle_ref = &handle;
            scope.spawn(move || send.send(handle_ref.publish_and_wait()).unwrap());
            wait_until(&handle, |state| state.compact_held);
            assert!(
                receive.try_recv().is_err(),
                "publication alone must not settle success"
            );
            let second = scope.spawn(|| handle.publish_and_wait());
            wait_until(&handle, |state| state.requests.len() == 2);
            assert!(
                lock(&handle.shared.state)
                    .requests
                    .values()
                    .all(Option::is_none)
            );
            drop(hold);
            assert_eq!(
                receive.recv_timeout(Duration::from_secs(10)).unwrap(),
                AnalyticsWriterStatus::Idle
            );
            assert_eq!(second.join().unwrap(), AnalyticsWriterStatus::Idle);
        });
        assert_eq!(handle.published(), 2);
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writer_refuses_an_index_symlink_swapped_after_validation() {
        let root = temp_root("index-symlink-swap");
        let cache = root.join("index.json");
        drop(crate::events::EventCache::open_scoped(&cache).unwrap());
        let index = crate::events::EventCache::path(&cache);
        let store_root = root.join("store");
        let owner = Owner::acquire(&index, &store_root).unwrap();
        let target = root.join("target.sqlite3");
        fs::rename(&index, &target).unwrap();
        let before = fs::read(&target).unwrap();
        std::os::unix::fs::symlink(&target, &index).unwrap();
        let handle = owner.start(AnalyticsWriterConfig::new("machine", &index, &store_root));
        assert!(matches!(
            handle.publish_and_wait(),
            AnalyticsWriterStatus::Refused { .. }
        ));
        handle.stop_and_join();
        assert_eq!(fs::read(target).unwrap(), before);
        assert!(semon_asi::read_manifest(&store_root).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_wait_retries_refused_input_instead_of_returning_the_old_refusal() {
        let root = temp_root("request-refusal");
        let index = root.join("index.sqlite3");
        rusqlite::Connection::open(&index).unwrap();
        let mut config = AnalyticsWriterConfig::new("machine", &index, root.join("store"));
        config.interval = Duration::ZERO;
        let handle = start(config);
        assert!(matches!(
            handle.publish_and_wait(),
            AnalyticsWriterStatus::Refused { .. }
        ));
        let attempts = handle.input_attempts();
        assert!(matches!(
            handle.publish_and_wait(),
            AnalyticsWriterStatus::Refused { .. }
        ));
        assert_eq!(handle.input_attempts(), attempts + 1);
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    struct ManualClock(std::sync::atomic::AtomicI64);

    impl Clock for ManualClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::Acquire)
        }
    }

    #[test]
    fn a_new_wait_does_not_return_a_previous_no_writer_outcome() {
        let root = temp_root("request-no-writer");
        let cache = root.join("index.json");
        drop(crate::events::EventCache::open_scoped(&cache).unwrap());
        let store_root = root.join("store");
        let held = Store::open(&store_root).unwrap();
        let clock = Arc::new(ManualClock(std::sync::atomic::AtomicI64::new(0)));
        let handle = start_with_clock(
            AnalyticsWriterConfig::new(
                "machine",
                crate::events::EventCache::path(&cache),
                &store_root,
            ),
            Arc::clone(&clock),
        );
        handle.wait_for_status(AnalyticsWriterStatus::NoWriter);
        drop(held);
        thread::scope(|scope| {
            let waiter = scope.spawn(|| handle.publish_and_wait());
            wait_until(&handle, |state| state.requests.len() == 1);
            assert!(
                lock(&handle.shared.state)
                    .requests
                    .values()
                    .all(Option::is_none)
            );
            clock.0.store(30_000, Ordering::Release);
            handle.wake_for_clock();
            assert_eq!(waiter.join().unwrap(), AnalyticsWriterStatus::Idle);
        });
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_input_io_failure_settles_the_request_after_three_attempts() {
        let root = temp_root("request-io-budget");
        let clock = Arc::new(ManualClock(std::sync::atomic::AtomicI64::new(0)));
        let handle = start_with_clock(
            AnalyticsWriterConfig::new("machine", root.join("missing.sqlite3"), root.join("store")),
            Arc::clone(&clock),
        );
        thread::scope(|scope| {
            let waiter = scope.spawn(|| handle.publish_and_wait());
            for (attempt, now) in [(1, 2_000), (2, 4_000)] {
                wait_until(&handle, |state| {
                    state.input_attempts == attempt
                        && matches!(state.status, AnalyticsWriterStatus::Retrying { .. })
                });
                clock.0.store(now, Ordering::Release);
                handle.wake_for_clock();
            }
            let AnalyticsWriterStatus::Refused { reason } = waiter.join().unwrap() else {
                panic!("request did not fail");
            };
            assert!(reason.contains("budget exhausted after 3 failures"));
            assert!(reason.contains("unable to open database"));
        });
        assert_eq!(handle.input_attempts(), 3);
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_interrupts_a_wait_during_operational_backoff() {
        let root = temp_root("request-cancel");
        let handle = start(AnalyticsWriterConfig::new(
            "machine",
            root.join("missing.sqlite3"),
            root.join("store"),
        ));
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        thread::scope(|scope| {
            let waiter =
                scope.spawn(|| handle.publish_and_wait_until(|| cancelled.load(Ordering::Acquire)));
            wait_until(&handle, |state| {
                matches!(state.status, AnalyticsWriterStatus::Retrying { .. })
            });
            cancelled.store(true, Ordering::Release);
            assert_eq!(waiter.join().unwrap(), AnalyticsWriterStatus::Stopped);
        });
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_deadlines_saturate_for_an_extreme_interval() {
        let mut scheduler = Scheduler::new(i64::MAX - 1);
        scheduler.publication_attempted(i64::MAX - 1, Duration::MAX);
        assert_eq!(scheduler.next_cycle, i64::MAX);
        assert_eq!(scheduler.operation_due(i64::MAX - 1, true, true), None);
    }

    #[test]
    fn scheduler_spaces_a_publish_that_fails_after_installing_a_visible_head() {
        // Store's cfg(test) fault hook is private to semon-asi. Model its
        // rename-visible error contract without files, threads or wall time.
        struct StoreDouble(u64);
        impl StoreDouble {
            fn publish(&mut self) -> Result<u64, StoreError> {
                self.0 += 1;
                Err(StoreError::Io(std::io::Error::other("post-rename fsync")))
            }
        }
        let mut store = StoreDouble(0);
        let mut scheduler = Scheduler::new(0);
        assert_eq!(
            scheduler.operation_due(0, true, true),
            Some(Operation::Publish)
        );
        let error = store.publish().unwrap_err();
        scheduler.publication_attempted(0, DEFAULT_ANALYTICS_WRITER_INTERVAL);
        assert!(error.is_operational());
        let reload = Operation::Reload(Resume::Publish);
        scheduler.retry(reload, 0);
        assert_eq!(store.0, 1, "the failed attempt installed a visible head");
        assert_eq!(scheduler.operation_due(999, true, true), None);
        assert_eq!(scheduler.operation_due(1_000, false, true), Some(reload));
        scheduler.succeeded(reload, 1_000);
        assert_eq!(scheduler.operation_due(1_000, false, true), None);
        assert_eq!(scheduler.deadline(false, true), Some(2_000));
        assert_eq!(scheduler.operation_due(1_999, false, true), None);
        assert_eq!(
            scheduler.operation_due(2_000, false, true),
            Some(Operation::Publish)
        );
    }

    #[test]
    fn scheduler_retires_before_a_notified_publication_with_a_one_hour_interval() {
        let mut scheduler = Scheduler::new(0);
        scheduler.publication_attempted(0, Duration::from_secs(3_600));
        scheduler.succeeded(Operation::Publish, 0);
        scheduler.succeeded(Operation::Compact, 0);
        // Demand stays in the shell's notification count while publication is
        // interval-gated; do not commit pending to Publish and hide retirement.
        assert_eq!(scheduler.operation_due(1_000, true, true), None);
        assert_eq!(scheduler.deadline(true, true), Some(600_000));
        assert_eq!(scheduler.operation_due(599_999, true, true), None);
        assert_eq!(
            scheduler.operation_due(600_000, true, true),
            Some(Operation::Retire)
        );
        scheduler.succeeded(Operation::Retire, 600_000);
        assert_eq!(scheduler.next_cycle, 3_600_000);
        assert_eq!(scheduler.operation_due(600_000, true, true), None);
        assert_eq!(scheduler.deadline(true, true), Some(1_200_000));
        for now in (1_200_000..=3_600_000).step_by(600_000) {
            assert_eq!(
                scheduler.operation_due(now, true, true),
                Some(Operation::Retire)
            );
            scheduler.succeeded(Operation::Retire, now);
        }
        assert_eq!(
            scheduler.operation_due(3_600_000, true, true),
            Some(Operation::Publish)
        );
    }

    #[test]
    fn scheduler_preserves_reload_refusal_and_retry_barriers_when_retirement_is_due() {
        for resume in [Resume::Publish, Resume::Compact, Resume::Retire] {
            let mut scheduler = Scheduler::new(0);
            let reload = Operation::Reload(resume);
            scheduler.refuse(reload);
            assert_eq!(scheduler.operation_due(600_000, false, true), None);
            assert_eq!(scheduler.deadline(false, true), None);
            assert_eq!(scheduler.operation_due(600_000, true, true), Some(reload));
            scheduler.retry(reload, 600_000);
            assert_eq!(scheduler.operation_due(600_999, true, true), None);
            assert_eq!(scheduler.deadline(true, true), Some(601_000));
            assert_eq!(scheduler.operation_due(601_000, true, true), Some(reload));
            scheduler.succeeded(reload, 601_000);
            assert_eq!(
                scheduler.operation_due(601_000, false, true),
                Some(resume.operation())
            );
        }
    }

    #[test]
    fn scheduler_successful_open_and_maintenance_keep_publication_demand() {
        for operation in [Operation::Open, Operation::Compact, Operation::Retire] {
            let mut scheduler = Scheduler::new(0);
            scheduler.succeeded(operation, 0);
            assert_eq!(
                scheduler.operation_due(0, true, true),
                Some(Operation::Publish)
            );
        }
        for resume in [Resume::Publish, Resume::Compact, Resume::Retire] {
            let mut scheduler = Scheduler::new(0);
            scheduler.succeeded(Operation::Reload(resume), 0);
            assert_eq!(
                scheduler.operation_due(0, true, true),
                Some(resume.operation())
            );
            scheduler.succeeded(resume.operation(), 0);
            if resume == Resume::Publish {
                scheduler.succeeded(Operation::Compact, 0);
            }
            assert_eq!(
                scheduler.operation_due(0, true, true),
                Some(Operation::Publish)
            );
        }
    }

    #[test]
    fn scheduler_retirement_preserves_a_waiting_recovery_publication_and_its_backoff() {
        let mut scheduler = Scheduler::new(0);
        scheduler.publication_attempted(0, Duration::from_secs(3_600));
        let reload_publish = Operation::Reload(Resume::Publish);
        assert_eq!(scheduler.retry(reload_publish, 0), 1);
        assert_eq!(
            scheduler.operation_due(1_000, false, true),
            Some(reload_publish)
        );
        scheduler.succeeded(reload_publish, 1_000);
        assert_eq!(scheduler.operation_due(1_000, false, true), None);
        assert_eq!(scheduler.deadline(false, true), Some(600_000));
        assert_eq!(
            scheduler.operation_due(600_000, false, true),
            Some(Operation::Retire)
        );
        assert_eq!(scheduler.publication_after_retirement, Some((1, 0)));
        let reload_retire = Operation::Reload(Resume::Retire);
        assert_eq!(scheduler.retry(reload_retire, 600_000), 1);
        scheduler.refuse(reload_retire);
        assert_eq!(scheduler.operation_due(700_000, false, true), None);
        assert_eq!(scheduler.deadline(false, true), None);
        assert_eq!(
            scheduler.operation_due(700_000, true, true),
            Some(reload_retire)
        );
        scheduler.succeeded(reload_retire, 700_000);
        assert_eq!(
            scheduler.operation_due(700_000, false, true),
            Some(Operation::Retire)
        );
        scheduler.succeeded(Operation::Retire, 700_000);
        assert_eq!(scheduler.pending, Some(Operation::Publish));
        assert_eq!(scheduler.attempt, 1);
        assert_eq!(scheduler.operation_due(700_000, false, true), None);
        assert_eq!(scheduler.deadline(false, true), Some(1_300_000));
        // Repeated retirement never forgets the publication or resets its retry
        // progression. Complete the later maintenance cycles before publishing.
        for now in [1_300_000, 1_900_000, 2_500_000, 3_100_000] {
            assert_eq!(
                scheduler.operation_due(now, false, true),
                Some(Operation::Retire)
            );
            scheduler.succeeded(Operation::Retire, now);
        }
        assert_eq!(
            scheduler.operation_due(3_600_000, false, true),
            Some(Operation::Publish)
        );
        scheduler.publication_attempted(3_600_000, Duration::from_secs(3_600));
        assert_eq!(scheduler.retry(reload_publish, 3_600_000), 2);
        assert_eq!(scheduler.retry_at, Some(3_602_000));
    }

    #[test]
    fn scheduler_wait_deadlines_are_future_or_absent_after_selecting_no_work() {
        let operations = [
            None,
            Some(Operation::Open),
            Some(Operation::Publish),
            Some(Operation::Compact),
            Some(Operation::Retire),
            Some(Operation::Reload(Resume::Publish)),
            Some(Operation::Reload(Resume::Compact)),
            Some(Operation::Reload(Resume::Retire)),
        ];
        for operation in operations {
            for refused in [false, true] {
                for retry_at in [None, Some(900_000)] {
                    for has_notification in [false, true] {
                        for has_store in [false, true] {
                            for now in [0, 599_999, 600_000, 899_999, 900_000, 3_599_999, 3_600_000]
                            {
                                let mut scheduler = Scheduler::new(0);
                                scheduler.next_cycle = 3_600_000;
                                scheduler.retry_at = retry_at;
                                if refused {
                                    scheduler.refused = operation;
                                } else {
                                    scheduler.pending = operation;
                                }
                                if scheduler
                                    .operation_due(now, has_notification, has_store)
                                    .is_none()
                                {
                                    assert!(
                                        scheduler
                                            .deadline(has_notification, has_store)
                                            .is_none_or(|deadline| deadline > now),
                                        "{scheduler:?} at {now}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scheduler_no_writer_deadline_cannot_be_bypassed_by_notifications() {
        let mut scheduler = Scheduler::new(0);
        scheduler.no_writer(10);
        assert_eq!(scheduler.operation_due(30_009, true, false), None);
        assert_eq!(scheduler.deadline(true, false), Some(30_010));
        assert_eq!(
            scheduler.operation_due(30_010, true, false),
            Some(Operation::Open)
        );
    }

    #[test]
    fn corrupt_or_unsupported_manifest_open_refuses_once_per_notification() {
        for unsupported in [false, true] {
            let root = temp_root(if unsupported {
                "unsupported-manifest"
            } else {
                "corrupt-manifest"
            });
            fs::create_dir_all(root.join("analytics")).unwrap();
            let bytes = if unsupported {
                let mut manifest = semon_asi::Manifest::empty();
                manifest.schema_version += 1;
                serde_json::to_vec(&manifest).unwrap()
            } else {
                b"invalid synthetic manifest".to_vec()
            };
            fs::write(root.join("analytics/manifest.json"), &bytes).unwrap();
            let handle = start(AnalyticsWriterConfig::new(
                "machine",
                root.join("index.sqlite3"),
                &root,
            ));
            handle.wait_for_store_refusal(1);
            handle.notify();
            handle.wait_for_store_refusal(2);
            assert!(matches!(
                handle.status(),
                AnalyticsWriterStatus::Refused { .. }
            ));
            assert_eq!(
                fs::read(root.join("analytics/manifest.json")).unwrap(),
                bytes
            );
            handle.stop_and_join();
            fs::remove_dir_all(root).unwrap();
        }
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
