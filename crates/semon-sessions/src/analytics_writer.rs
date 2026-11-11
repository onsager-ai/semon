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
    WriterLock, publish_cycle,
};

use crate::{SqliteAnalyticsInput, events::session_activity::ACTIVITY_RULES_VERSION};

/// The minimum spacing between requested publication cycles by default.
pub const DEFAULT_ANALYTICS_WRITER_INTERVAL: Duration = Duration::from_secs(2);

/// Each active chain refuses after three operational failures, with 1s/2s backoffs.
/// Successful operations within the chain never reset these counters.
const CHAIN_FAILURE_LIMIT: u32 = 3;

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
    cancelled: Vec<u64>,
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
                cancelled: Vec::new(),
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
        // Exhaustion at >1.8e19 requests is unreachable; settlement then is unspecified.
        state.notifications = state.notifications.saturating_add(1);
        self.shared.changed.notify_all();
    }

    pub fn status(&self) -> AnalyticsWriterStatus {
        lock(&self.shared.state).status.clone()
    }

    /// Requests publication through input completion and waits for it to settle.
    ///
    /// A successful request includes the final chunk's bounded compaction step.
    /// Later notifications require their own publication. A replay drain has a
    /// fixed endpoint; ongoing source changes belong to the next request.
    /// Refused and no-writer states are returned to the caller rather than hidden
    /// behind a later notification.
    pub fn publish_and_wait(&self) -> AnalyticsWriterStatus {
        self.publish_and_wait_until(|| false)
    }

    /// Cancellation is checked every 20 ms while waiting and withdraws the
    /// owning chain before another operation is selected. An operation already
    /// in progress finishes first. The caller can then stop and join the writer.
    pub fn publish_and_wait_until(&self, cancelled: impl Fn() -> bool) -> AnalyticsWriterStatus {
        let mut state = lock(&self.shared.state);
        state.notifications = state.notifications.saturating_add(1);
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
                state.cancelled.push(target);
                self.shared.changed.notify_all();
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
        wait_for_test_state(
            &self.shared,
            |state| state.published >= expected,
            "publication",
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
    pub(crate) fn wait_for_requests(&self, expected: usize) {
        wait_for_test_state(
            &self.shared,
            |state| state.requests.len() == expected,
            "requests",
        );
    }

    #[cfg(test)]
    pub(crate) fn hold_compact(&self) -> PublishHold {
        lock(&self.shared.state).hold_compact = true;
        PublishHold {
            shared: Arc::clone(&self.shared),
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_for_compact_hold(&self) {
        wait_for_test_state(&self.shared, |state| state.compact_held, "compaction hold");
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
        wait_for_test_state(
            &self.shared,
            |state| state.retire_attempts >= expected,
            "retirement",
        );
    }

    #[cfg(test)]
    pub(crate) fn wait_for_idle_wait(&self) {
        wait_for_test_state(
            &self.shared,
            |state| state.waiting && state.status == AnalyticsWriterStatus::Idle,
            "idle wait",
        );
    }

    #[cfg(test)]
    pub(crate) fn idle_waits(&self) -> u64 {
        lock(&self.shared.state).idle_waits
    }

    #[cfg(test)]
    pub(crate) fn wait_for_idle_wait_after(&self, previous: u64) {
        wait_for_test_state(
            &self.shared,
            |state| state.idle_waits > previous,
            "next idle wait",
        );
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
    lock: WriterLock,
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
        let lock = WriterLock::acquire(root).map_err(|error| match error {
            StoreError::WriterBusy => "analytics writer busy".to_owned(),
            error => format!("analytics writer refused: {error}"),
        })?;
        Ok(Self { lock, clock })
    }

    pub fn start(self, config: AnalyticsWriterConfig) -> Result<Handle, String> {
        let store = Store::open_with_lock(self.lock, Arc::clone(&self.clock))
            .map_err(|error| format!("analytics writer refused: {error}"))?;
        Ok(start_thread(config, self.clock, Some(store)))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChainKind {
    Publication,
    Maintenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EndpointOrigin {
    Uninspected,
    Inherited,
    Fresh,
}

/// The only owner of request identity, progress and recovery counters.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Chain {
    captured_generation: u64,
    kind: ChainKind,
    attempt: u32,
    failures: u32,
    endpoint_origin: EndpointOrigin,
    operation: Operation,
    retry_at: Option<i64>,
    drain_complete: bool,
}

impl Chain {
    fn new(captured_generation: u64, kind: ChainKind, operation: Operation) -> Self {
        Self {
            captured_generation,
            kind,
            attempt: 0,
            failures: 0,
            endpoint_origin: EndpointOrigin::Uninspected,
            operation,
            retry_at: None,
            drain_complete: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ChainEvent {
    Capture {
        generation: u64,
        kind: ChainKind,
        operation: Operation,
    },
    Endpoint {
        inherited: bool,
    },
    Published {
        complete: bool,
    },
    Succeeded(Operation),
    Failed {
        resume: Operation,
        operational: bool,
    },
    NoWriter,
    SuspendForRetirement,
    Cancel {
        generation: u64,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChainEffect {
    Unchanged,
    Idle,
    Progress(u64),
    Retrying(u32),
    Success(u64),
    Refused {
        generation: u64,
        exhausted: Option<u32>,
    },
    NoWriter(u64),
    Cancelled(u64),
    Stopped,
}

#[derive(Clone, Debug)]
struct Scheduler {
    active: Option<Chain>,
    suspended: Option<Chain>,
    // A refused/cancelled operation can require a reload barrier for the next
    // request. This latch carries no identity, progress or recovery counters.
    refused: Option<Operation>,
    cooldown_until: Option<i64>,
    next_cycle: i64,
    next_retirement: i64,
}

impl Scheduler {
    fn new(now: i64) -> Self {
        Self {
            active: None,
            suspended: None,
            refused: None,
            cooldown_until: None,
            next_cycle: now,
            next_retirement: now.saturating_add(duration_ms(RETIREMENT_CADENCE)),
        }
    }

    /// Pure transition table; no I/O, locks or request-map mutation. Successful
    /// operations preserve counters. Only dropping a chain ends its budget.
    fn transition(&mut self, event: ChainEvent, now: i64) -> ChainEffect {
        match event {
            ChainEvent::Capture {
                generation,
                kind,
                operation,
            } => {
                if self.active.is_some() {
                    return ChainEffect::Unchanged;
                }
                self.active = Some(Chain::new(generation, kind, operation));
                ChainEffect::Idle
            }
            ChainEvent::Endpoint { inherited } => {
                let chain = self.active.as_mut().expect("active input chain");
                if chain.endpoint_origin == EndpointOrigin::Uninspected {
                    chain.endpoint_origin = if inherited {
                        EndpointOrigin::Inherited
                    } else {
                        EndpointOrigin::Fresh
                    };
                }
                ChainEffect::Idle
            }
            ChainEvent::Published { complete } => {
                let chain = self.active.as_mut().expect("active publication chain");
                chain.drain_complete = complete;
                chain.operation = Operation::Compact;
                chain.retry_at = None;
                ChainEffect::Progress(chain.captured_generation)
            }
            ChainEvent::Succeeded(operation) => {
                let chain = self.active.as_mut().expect("active operation chain");
                chain.retry_at = None;
                match operation {
                    Operation::Open if chain.kind == ChainKind::Publication => {
                        chain.operation = Operation::Publish;
                    }
                    Operation::Open => {
                        self.active = None;
                    }
                    Operation::Reload(resume) => {
                        chain.operation = resume.operation();
                    }
                    Operation::Compact => {
                        if chain.drain_complete && chain.endpoint_origin == EndpointOrigin::Fresh {
                            let generation = chain.captured_generation;
                            self.active = None;
                            return ChainEffect::Success(generation);
                        }
                        if chain.drain_complete {
                            // A previous chain captured that endpoint before this
                            // request registered. Finish it, then capture our own.
                            chain.endpoint_origin = EndpointOrigin::Fresh;
                        }
                        chain.drain_complete = false;
                        chain.operation = Operation::Publish;
                    }
                    Operation::Retire => {
                        self.active = self.suspended.take();
                        self.next_retirement = now.saturating_add(duration_ms(RETIREMENT_CADENCE));
                    }
                    Operation::Publish => unreachable!("publication carries completion"),
                }
                ChainEffect::Idle
            }
            ChainEvent::Failed {
                resume,
                operational,
            } => {
                let chain = self.active.as_mut().expect("active failed chain");
                chain.operation = resume;
                if operational {
                    chain.attempt += 1;
                    chain.failures += 1;
                    if chain.failures < CHAIN_FAILURE_LIMIT {
                        chain.retry_at = Some(now.saturating_add(retry_delay(chain.attempt)));
                        return ChainEffect::Retrying(chain.attempt);
                    }
                }
                let chain = self.active.take().expect("terminal failed chain");
                // A failed retirement also ends the suspended request explicitly;
                // it must never later revive with its previous counters.
                self.suspended = None;
                self.refused = Some(resume);
                ChainEffect::Refused {
                    generation: chain.captured_generation,
                    exhausted: operational.then_some(chain.failures),
                }
            }
            ChainEvent::NoWriter => {
                let chain = self.active.take().expect("open chain");
                self.suspended = None;
                self.refused = None;
                self.cooldown_until = Some(now.saturating_add(duration_ms(NO_WRITER_RETRY)));
                ChainEffect::NoWriter(chain.captured_generation)
            }
            ChainEvent::SuspendForRetirement => {
                let chain = self.active.take().expect("publication to suspend");
                let generation = chain.captured_generation;
                self.suspended = Some(chain);
                self.active = Some(Chain::new(
                    generation,
                    ChainKind::Maintenance,
                    Operation::Retire,
                ));
                ChainEffect::Idle
            }
            ChainEvent::Cancel { generation } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|chain| chain.captured_generation == generation)
                {
                    let chain = self.active.take().expect("cancelled active chain");
                    // Only a store error leaves a manifest reload barrier.
                    // Clean pending work must not block maintenance without demand.
                    self.refused =
                        matches!(chain.operation, Operation::Reload(_)).then_some(chain.operation);
                    self.suspended = None;
                    return ChainEffect::Cancelled(generation);
                }
                ChainEffect::Unchanged
            }
            ChainEvent::Shutdown => {
                self.active = None;
                self.suspended = None;
                ChainEffect::Stopped
            }
        }
    }

    fn operation_due(
        &mut self,
        now: i64,
        has_notification: bool,
        has_store: bool,
        generation: u64,
    ) -> Option<Operation> {
        if self.cooldown_until.is_some_and(|deadline| now < deadline) {
            return None;
        }
        self.cooldown_until = None;
        if self.active.is_none() {
            if self.refused.is_some() && !has_notification {
                return None;
            }
            let operation = if let Some(operation) = self.refused.take() {
                operation
            } else if !has_store {
                Operation::Open
            } else if now >= self.next_retirement {
                Operation::Retire
            } else if has_notification && now >= self.next_cycle {
                Operation::Publish
            } else {
                return None;
            };
            let kind = if has_notification
                && !matches!(
                    operation,
                    Operation::Retire | Operation::Reload(Resume::Retire)
                ) {
                ChainKind::Publication
            } else {
                ChainKind::Maintenance
            };
            self.transition(
                ChainEvent::Capture {
                    generation,
                    kind,
                    operation,
                },
                now,
            );
        }
        let chain = self.active.as_ref().expect("selected chain");
        if chain.kind == ChainKind::Publication
            && chain.operation == Operation::Publish
            && now < self.next_cycle
            && now >= self.next_retirement
            && chain.retry_at.is_none_or(|deadline| now >= deadline)
        {
            self.transition(ChainEvent::SuspendForRetirement, now);
        }
        let chain = self.active.as_ref().expect("due chain");
        let due = if chain.operation == Operation::Publish {
            chain.retry_at.unwrap_or(now).max(self.next_cycle)
        } else {
            chain.retry_at.unwrap_or(now)
        };
        (now >= due).then_some(chain.operation)
    }

    fn deadline(&self, has_notification: bool, has_store: bool) -> Option<i64> {
        if let Some(deadline) = self.cooldown_until {
            return Some(deadline);
        }
        if let Some(chain) = &self.active {
            return Some(if chain.operation == Operation::Publish {
                chain
                    .retry_at
                    .unwrap_or(self.next_cycle)
                    .max(self.next_cycle)
                    .min(self.next_retirement.max(chain.retry_at.unwrap_or(0)))
            } else {
                chain.retry_at.unwrap_or(0)
            });
        }
        if self.refused.is_some() && !has_notification {
            return None;
        }
        if !has_store {
            return Some(0);
        }
        Some(if has_notification {
            self.next_cycle.min(self.next_retirement)
        } else {
            self.next_retirement
        })
    }

    fn publication_attempted(&mut self, now: i64, interval: Duration) {
        // Even an error may follow a visible manifest rename.
        self.next_cycle = now.saturating_add(duration_ms(interval));
    }
}

fn run<C: Clock + Send + Sync + 'static>(
    config: AnalyticsWriterConfig,
    clock: Arc<C>,
    shared: Arc<Shared>,
    mut store: Option<Store<Arc<C>>>,
) {
    let mut scheduler = Scheduler::new(clock.now_ms());
    while let Some(operation) = next_operation(&shared, &clock, &mut scheduler, store.is_some()) {
        let (event, reason) = match operation {
            Operation::Open => match Store::open_with_clock(&config.store_root, Arc::clone(&clock))
            {
                Ok(opened) => {
                    store = Some(opened);
                    (ChainEvent::Succeeded(operation), None)
                }
                Err(StoreError::WriterBusy) => (ChainEvent::NoWriter, None),
                Err(error) => (
                    ChainEvent::Failed {
                        resume: operation,
                        operational: error.is_operational(),
                    },
                    Some(error.to_string()),
                ),
            },
            Operation::Publish => {
                #[cfg(test)]
                publish_started(&shared);
                let result = (|| {
                    let mut input =
                        SqliteAnalyticsInput::open(&config.index_path, config.namespace.clone())
                            .map_err(PublishError::Input)?;
                    if scheduler
                        .active
                        .as_ref()
                        .expect("input chain")
                        .endpoint_origin
                        == EndpointOrigin::Uninspected
                    {
                        let inherited = SqliteAnalyticsInput::resumes_replay(
                            store
                                .as_ref()
                                .expect("opened store")
                                .manifest()
                                .checkpoints
                                .get(&config.namespace)
                                .map(String::as_str),
                        )
                        .map_err(PublishError::Input)?;
                        scheduler.transition(ChainEvent::Endpoint { inherited }, clock.now_ms());
                    }
                    publish_cycle(
                        store.as_mut().expect("opened store"),
                        &mut input,
                        &config.namespace,
                        target_versions(),
                        config.input_budget,
                    )
                })();
                scheduler.publication_attempted(clock.now_ms(), config.interval);
                match result {
                    Ok(progress) => {
                        #[cfg(test)]
                        published(&shared);
                        (
                            ChainEvent::Published {
                                complete: progress.complete,
                            },
                            None,
                        )
                    }
                    Err(PublishError::Input(error)) => (
                        ChainEvent::Failed {
                            resume: operation,
                            operational: is_input_operational(&error),
                        },
                        Some(error.to_string()),
                    ),
                    Err(PublishError::Store(error)) => (
                        ChainEvent::Failed {
                            resume: Operation::Reload(Resume::Publish),
                            operational: error.is_operational(),
                        },
                        Some(error.to_string()),
                    ),
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
                    Ok(_) => (ChainEvent::Succeeded(operation), None),
                    Err(error) => (
                        ChainEvent::Failed {
                            resume: Operation::Reload(Resume::Compact),
                            operational: error.is_operational(),
                        },
                        Some(error.to_string()),
                    ),
                }
            }
            Operation::Retire => {
                #[cfg(test)]
                retirement_started(&shared);
                match store.as_mut().expect("opened store").retire() {
                    Ok(_) => (ChainEvent::Succeeded(operation), None),
                    Err(error) => (
                        ChainEvent::Failed {
                            resume: Operation::Reload(Resume::Retire),
                            operational: error.is_operational(),
                        },
                        Some(error.to_string()),
                    ),
                }
            }
            Operation::Reload(_) => match store.as_mut().expect("opened store").reload_manifest() {
                Ok(()) => (ChainEvent::Succeeded(operation), None),
                Err(error) => (
                    ChainEvent::Failed {
                        resume: operation,
                        operational: error.is_operational(),
                    },
                    Some(error.to_string()),
                ),
            },
        };
        let effect = scheduler.transition(event, clock.now_ms());
        apply_effect(&mut lock(&shared.state), effect, reason.as_deref());
        shared.changed.notify_all();
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
) -> Option<Operation> {
    let mut state = lock(&shared.state);
    loop {
        if state.stop {
            let effect = scheduler.transition(ChainEvent::Shutdown, clock.now_ms());
            apply_effect(&mut state, effect, None);
            return None;
        }
        for generation in std::mem::take(&mut state.cancelled) {
            let effect = scheduler.transition(ChainEvent::Cancel { generation }, clock.now_ms());
            apply_effect(&mut state, effect, None);
        }
        let now = clock.now_ms();
        let has_notification = state.notifications != state.consumed;
        let generation = capture_generation(state.consumed, state.notifications, &state.requests);
        if let Some(operation) =
            scheduler.operation_due(now, has_notification, has_store, generation)
        {
            #[cfg(test)]
            {
                state.waiting = false;
                if matches!(operation, Operation::Open | Operation::Reload(_)) {
                    state.store_attempts += 1;
                }
            }
            state.status = AnalyticsWriterStatus::Running;
            shared.changed.notify_all();
            return Some(operation);
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
                let (next, _) = shared
                    .changed
                    .wait_timeout(state, Duration::from_millis((deadline - now).max(1) as u64))
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

// A chain owns (consumed, captured], ending at the earliest unsettled waiter;
// notification-only work coalesces up to the latest notification.
fn capture_generation(
    consumed: u64,
    notifications: u64,
    requests: &BTreeMap<u64, Option<AnalyticsWriterStatus>>,
) -> u64 {
    requests
        .range((
            std::ops::Bound::Excluded(consumed),
            std::ops::Bound::Unbounded,
        ))
        .find_map(|(&generation, outcome)| outcome.is_none().then_some(generation))
        .unwrap_or(notifications)
}

fn apply_effect(state: &mut State, effect: ChainEffect, reason: Option<&str>) {
    let (generation, outcome) = match effect {
        ChainEffect::Unchanged => return,
        ChainEffect::Idle => {
            state.status = AnalyticsWriterStatus::Idle;
            return;
        }
        ChainEffect::Progress(generation) => {
            state.consumed = state.consumed.max(generation);
            state.status = AnalyticsWriterStatus::Idle;
            return;
        }
        ChainEffect::Retrying(attempt) => {
            state.status = AnalyticsWriterStatus::Retrying { attempt };
            return;
        }
        ChainEffect::Success(generation) => (generation, AnalyticsWriterStatus::Idle),
        ChainEffect::Refused {
            generation,
            exhausted,
        } => {
            let reason = reason.expect("failed operation reason");
            let reason = match exhausted {
                Some(failures) => format!(
                    "operational retry budget exhausted after {failures} failures: {reason}"
                ),
                None => reason.to_owned(),
            };
            (generation, AnalyticsWriterStatus::Refused { reason })
        }
        ChainEffect::NoWriter(generation) => (generation, AnalyticsWriterStatus::NoWriter),
        ChainEffect::Cancelled(generation) => {
            state.consumed = state.consumed.max(generation);
            state.status = AnalyticsWriterStatus::Idle;
            return;
        }
        ChainEffect::Stopped => {
            state.status = AnalyticsWriterStatus::Stopped;
            return;
        }
    };
    state.consumed = state.consumed.max(generation);
    for (_, request) in state.requests.range_mut(..=generation) {
        if request.is_none() {
            *request = Some(outcome.clone());
        }
    }
    state.status = outcome;
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

#[cfg(test)]
fn wait_for_test_state(shared: &Shared, ready: impl Fn(&State) -> bool, description: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut state = lock(&shared.state);
    while !ready(&state) && !state.stop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            // Unblock the writer and any scoped request waiter before failing.
            state.stop = true;
            shared.changed.notify_all();
            panic!("writer test timed out waiting for {description}");
        }
        state = shared
            .changed
            .wait_timeout(state, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
    assert!(ready(&state), "writer stopped before {description}");
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
    fn chain_capture_ends_at_the_earliest_unsettled_waiter() {
        let mut requests = BTreeMap::from([(1, None), (2, None)]);
        assert_eq!(capture_generation(0, 2, &requests), 1);
        requests.insert(1, Some(AnalyticsWriterStatus::Idle));
        assert_eq!(capture_generation(1, 2, &requests), 2);
        // Notification-only work beyond a waiter remains for the next chain.
        assert_eq!(capture_generation(1, 7, &requests), 2);
        requests.insert(2, Some(AnalyticsWriterStatus::Stopped));
        assert_eq!(capture_generation(1, 7, &requests), 7);
        requests.insert(4, None);
        assert_eq!(capture_generation(1, 7, &requests), 4);
        requests.remove(&4); // Cancelled waiters no longer split notification work.
        assert_eq!(capture_generation(1, 7, &requests), 7);
        assert_eq!(capture_generation(7, 7, &requests), 7);
    }

    #[test]
    fn active_chain_transition_table() {
        struct Case {
            name: &'static str,
            active: Option<Chain>,
            suspended: Option<Chain>,
            event: ChainEvent,
            expected_active: Option<Chain>,
            expected_suspended: Option<Chain>,
            effect: ChainEffect,
        }
        let open = Chain::new(1, ChainKind::Publication, Operation::Open);
        let publish = Chain {
            operation: Operation::Publish,
            endpoint_origin: EndpointOrigin::Fresh,
            ..open.clone()
        };
        let failed = Chain {
            attempt: 2,
            failures: 2,
            retry_at: Some(2_000),
            ..publish.clone()
        };
        let compact = Chain {
            operation: Operation::Compact,
            retry_at: None,
            drain_complete: true,
            ..failed.clone()
        };
        let inherited = Chain {
            endpoint_origin: EndpointOrigin::Inherited,
            ..compact.clone()
        };
        let maintenance = Chain::new(1, ChainKind::Maintenance, Operation::Retire);
        let maintenance_failed = Chain {
            attempt: 2,
            failures: 2,
            retry_at: Some(2_000),
            ..maintenance.clone()
        };
        let failed_open = Chain {
            operation: Operation::Open,
            ..failed.clone()
        };
        let reload = Chain {
            operation: Operation::Reload(Resume::Publish),
            ..failed.clone()
        };
        let cases = vec![
            Case {
                name: "capture starts at zero",
                active: None,
                suspended: None,
                event: ChainEvent::Capture {
                    generation: 1,
                    kind: ChainKind::Publication,
                    operation: Operation::Open,
                },
                expected_active: Some(open.clone()),
                expected_suspended: None,
                effect: ChainEffect::Idle,
            },
            Case {
                name: "arrival does not extend captured ownership",
                active: Some(failed.clone()),
                suspended: None,
                event: ChainEvent::Capture {
                    generation: 2,
                    kind: ChainKind::Publication,
                    operation: Operation::Publish,
                },
                expected_active: Some(failed.clone()),
                expected_suspended: None,
                effect: ChainEffect::Unchanged,
            },
            Case {
                name: "first operational failure",
                active: Some(publish.clone()),
                suspended: None,
                event: ChainEvent::Failed {
                    resume: Operation::Reload(Resume::Publish),
                    operational: true,
                },
                expected_active: Some(Chain {
                    attempt: 1,
                    failures: 1,
                    retry_at: Some(1_000),
                    operation: Operation::Reload(Resume::Publish),
                    ..publish.clone()
                }),
                expected_suspended: None,
                effect: ChainEffect::Retrying(1),
            },
            Case {
                name: "third failure drops the record",
                active: Some(failed.clone()),
                suspended: None,
                event: ChainEvent::Failed {
                    resume: Operation::Publish,
                    operational: true,
                },
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::Refused {
                    generation: 1,
                    exhausted: Some(3),
                },
            },
            Case {
                name: "structural refusal drops the record",
                active: Some(publish.clone()),
                suspended: None,
                event: ChainEvent::Failed {
                    resume: Operation::Publish,
                    operational: false,
                },
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::Refused {
                    generation: 1,
                    exhausted: None,
                },
            },
            Case {
                name: "open success preserves publication budget",
                active: Some(failed_open.clone()),
                suspended: None,
                event: ChainEvent::Succeeded(Operation::Open),
                expected_active: Some(Chain {
                    retry_at: None,
                    ..failed.clone()
                }),
                expected_suspended: None,
                effect: ChainEffect::Idle,
            },
            Case {
                name: "reload barrier preserves publication budget",
                active: Some(reload),
                suspended: None,
                event: ChainEvent::Succeeded(Operation::Reload(Resume::Publish)),
                expected_active: Some(Chain {
                    retry_at: None,
                    ..failed.clone()
                }),
                expected_suspended: None,
                effect: ChainEffect::Idle,
            },
            Case {
                name: "chunk advances progress but keeps budget",
                active: Some(failed.clone()),
                suspended: None,
                event: ChainEvent::Published { complete: false },
                expected_active: Some(Chain {
                    operation: Operation::Compact,
                    retry_at: None,
                    ..failed.clone()
                }),
                expected_suspended: None,
                effect: ChainEffect::Progress(1),
            },
            Case {
                name: "partial compaction continues same chain",
                active: Some(Chain {
                    drain_complete: false,
                    ..compact.clone()
                }),
                suspended: None,
                event: ChainEvent::Succeeded(Operation::Compact),
                expected_active: Some(Chain {
                    retry_at: None,
                    ..failed.clone()
                }),
                expected_suspended: None,
                effect: ChainEffect::Idle,
            },
            Case {
                name: "final compaction drops successful chain",
                active: Some(compact.clone()),
                suspended: None,
                event: ChainEvent::Succeeded(Operation::Compact),
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::Success(1),
            },
            Case {
                name: "inherited endpoint requires a fresh drain",
                active: Some(inherited),
                suspended: None,
                event: ChainEvent::Succeeded(Operation::Compact),
                expected_active: Some(Chain {
                    retry_at: None,
                    ..failed.clone()
                }),
                expected_suspended: None,
                effect: ChainEffect::Idle,
            },
            Case {
                name: "no writer drops failed opens",
                active: Some(failed_open),
                suspended: None,
                event: ChainEvent::NoWriter,
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::NoWriter(1),
            },
            Case {
                name: "retirement suspends the whole record",
                active: Some(failed.clone()),
                suspended: None,
                event: ChainEvent::SuspendForRetirement,
                expected_active: Some(maintenance.clone()),
                expected_suspended: Some(failed.clone()),
                effect: ChainEffect::Idle,
            },
            Case {
                name: "retirement restores exactly the saved record",
                active: Some(maintenance_failed.clone()),
                suspended: Some(failed.clone()),
                event: ChainEvent::Succeeded(Operation::Retire),
                expected_active: Some(failed.clone()),
                expected_suspended: None,
                effect: ChainEffect::Idle,
            },
            Case {
                name: "retirement refusal terminates the suspended request",
                active: Some(maintenance_failed),
                suspended: Some(failed.clone()),
                event: ChainEvent::Failed {
                    resume: Operation::Reload(Resume::Retire),
                    operational: true,
                },
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::Refused {
                    generation: 1,
                    exhausted: Some(3),
                },
            },
            Case {
                name: "owning waiter cancellation drops both records",
                active: Some(maintenance.clone()),
                suspended: Some(failed.clone()),
                event: ChainEvent::Cancel { generation: 1 },
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::Cancelled(1),
            },
            Case {
                name: "other waiter cancellation leaves ownership alone",
                active: Some(failed.clone()),
                suspended: None,
                event: ChainEvent::Cancel { generation: 2 },
                expected_active: Some(failed.clone()),
                expected_suspended: None,
                effect: ChainEffect::Unchanged,
            },
            Case {
                name: "shutdown drops active and suspended records",
                active: Some(maintenance),
                suspended: Some(failed),
                event: ChainEvent::Shutdown,
                expected_active: None,
                expected_suspended: None,
                effect: ChainEffect::Stopped,
            },
        ];
        for case in cases {
            let mut scheduler = Scheduler::new(0);
            scheduler.active = case.active;
            scheduler.suspended = case.suspended;
            assert_eq!(
                scheduler.transition(case.event, 0),
                case.effect,
                "{}",
                case.name
            );
            assert_eq!(scheduler.active, case.expected_active, "{}", case.name);
            assert_eq!(
                scheduler.suspended, case.expected_suspended,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn chunk_watermark_defers_settlement_and_terminal_outcomes_preserve_later_waiters() {
        let shared = Shared::new();
        let mut state = lock(&shared.state);
        state.notifications = 2;
        state.requests = BTreeMap::from([(1, None), (2, None)]);
        apply_effect(&mut state, ChainEffect::Progress(1), None);
        assert_eq!(state.consumed, 1);
        assert!(state.requests.values().all(Option::is_none));
        apply_effect(
            &mut state,
            ChainEffect::Refused {
                generation: 1,
                exhausted: Some(3),
            },
            Some("last operation"),
        );
        assert!(
            matches!(state.requests[&1].as_ref(), Some(AnalyticsWriterStatus::Refused { reason }) if reason.contains("after 3 failures: last operation"))
        );
        assert!(state.requests[&2].is_none());
        let first_outcome = state.requests[&1].clone();
        apply_effect(&mut state, ChainEffect::Success(2), None);
        assert_eq!(state.requests[&1], first_outcome);
        assert_eq!(state.requests[&2], Some(AnalyticsWriterStatus::Idle));
        assert_eq!(state.consumed, 2);
    }

    #[test]
    fn cancellation_retains_only_manifest_reload_barriers() {
        for operation in [
            Operation::Open,
            Operation::Publish,
            Operation::Compact,
            Operation::Retire,
            Operation::Reload(Resume::Publish),
            Operation::Reload(Resume::Compact),
            Operation::Reload(Resume::Retire),
        ] {
            let mut scheduler = Scheduler::new(0);
            scheduler.active = Some(Chain::new(1, ChainKind::Publication, operation));
            assert_eq!(
                scheduler.transition(ChainEvent::Cancel { generation: 1 }, 0),
                ChainEffect::Cancelled(1)
            );
            assert!(scheduler.active.is_none());
            assert!(scheduler.suspended.is_none());
            if matches!(operation, Operation::Reload(_)) {
                assert_eq!(scheduler.refused, Some(operation));
                assert_eq!(scheduler.operation_due(600_000, false, true, 1), None);
                assert_eq!(scheduler.deadline(false, true), None);
                assert_eq!(
                    scheduler.operation_due(600_000, true, true, 2),
                    Some(operation)
                );
                assert_eq!(scheduler.active.as_ref().unwrap().captured_generation, 2);
            } else {
                assert_eq!(scheduler.refused, None);
                assert_eq!(scheduler.deadline(false, true), Some(600_000));
                assert_eq!(
                    scheduler.operation_due(600_000, false, true, 1),
                    Some(Operation::Retire)
                );
            }
        }
    }

    #[test]
    fn terminal_transitions_give_the_next_chain_zero_counters() {
        for terminal in [
            ChainEvent::Failed {
                resume: Operation::Open,
                operational: true,
            },
            ChainEvent::Failed {
                resume: Operation::Open,
                operational: false,
            },
            ChainEvent::NoWriter,
            ChainEvent::Cancel { generation: 1 },
            ChainEvent::Shutdown,
        ] {
            let mut scheduler = Scheduler::new(0);
            scheduler.active = Some(Chain {
                attempt: 2,
                failures: 2,
                ..Chain::new(1, ChainKind::Publication, Operation::Open)
            });
            scheduler.transition(terminal, 0);
            assert!(scheduler.active.is_none());
            scheduler.transition(
                ChainEvent::Capture {
                    generation: 2,
                    kind: ChainKind::Publication,
                    operation: Operation::Open,
                },
                30_000,
            );
            assert_eq!(
                scheduler.transition(
                    ChainEvent::Failed {
                        resume: Operation::Open,
                        operational: true
                    },
                    30_000
                ),
                ChainEffect::Retrying(1)
            );
            let chain = scheduler.active.unwrap();
            assert_eq!(
                (
                    chain.captured_generation,
                    chain.attempt,
                    chain.failures,
                    chain.retry_at
                ),
                (2, 1, 1, Some(31_000))
            );
        }
    }

    #[test]
    fn successful_open_and_chunks_do_not_discard_two_publication_failures() {
        let mut scheduler = Scheduler::new(0);
        scheduler.transition(
            ChainEvent::Capture {
                generation: 1,
                kind: ChainKind::Publication,
                operation: Operation::Open,
            },
            0,
        );
        for now in [0, 1_000] {
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Open,
                    operational: true,
                },
                now,
            );
        }
        scheduler.transition(ChainEvent::Succeeded(Operation::Open), 3_000);
        scheduler.transition(ChainEvent::Endpoint { inherited: false }, 3_000);
        scheduler.transition(ChainEvent::Published { complete: false }, 3_000);
        scheduler.transition(ChainEvent::Succeeded(Operation::Compact), 3_000);
        assert_eq!(
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Reload(Resume::Publish),
                    operational: true
                },
                5_000
            ),
            ChainEffect::Refused {
                generation: 1,
                exhausted: Some(3)
            }
        );
        assert!(scheduler.active.is_none());
    }

    fn wait_until(handle: &Handle, predicate: impl Fn(&State) -> bool) {
        wait_for_test_state(&handle.shared, predicate, "expected lifecycle state");
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
        let handle = owner
            .start(AnalyticsWriterConfig::new("machine", &index, &store_root))
            .unwrap();
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

    // Register A and B before the worker exists, removing thread-start races
    // from tests of initial capture rather than just requests during backoff.
    fn start_with_two_queued_requests(
        config: AnalyticsWriterConfig,
        clock: Arc<ManualClock>,
    ) -> Handle {
        let shared = Arc::new(Shared::new());
        {
            let mut state = lock(&shared.state);
            state.notifications = 2;
            state.requests = BTreeMap::from([(1, None), (2, None)]);
        }
        let worker_shared = Arc::clone(&shared);
        let thread = thread::spawn(move || run(config, clock, worker_shared, None));
        Handle {
            shared,
            thread: Some(thread),
        }
    }

    #[test]
    fn requests_queued_before_first_operation_have_independent_failure_budgets() {
        let root = temp_root("requests-already-queued-failure");
        let cache = root.join("index.json");
        let clock = Arc::new(ManualClock(std::sync::atomic::AtomicI64::new(0)));
        let handle = start_with_two_queued_requests(
            AnalyticsWriterConfig::new(
                "machine",
                crate::events::EventCache::path(&cache),
                root.join("store"),
            ),
            Arc::clone(&clock),
        );
        for (attempt, now) in [(1, 2_000), (2, 4_000)] {
            wait_until(&handle, |state| {
                state.input_attempts == attempt
                    && state.status
                        == AnalyticsWriterStatus::Retrying {
                            attempt: attempt as u32,
                        }
            });
            clock.0.store(now, Ordering::Release);
            handle.wake_for_clock();
        }
        wait_until(&handle, |state| {
            matches!(
                state.requests[&1].as_ref(),
                Some(AnalyticsWriterStatus::Refused { .. })
            )
        });
        {
            let state = lock(&handle.shared.state);
            assert_eq!(state.input_attempts, 3);
            assert_eq!(state.consumed, 1);
            let Some(AnalyticsWriterStatus::Refused { reason }) = &state.requests[&1] else {
                panic!("A must exhaust its own budget");
            };
            assert!(reason.contains("after 3 failures"));
            assert!(state.requests[&2].is_none(), "B did not initiate A's chain");
        }
        for (attempts, attempt, now) in [(4, 1, 6_000), (5, 2, 8_000)] {
            clock.0.store(now, Ordering::Release);
            handle.wake_for_clock();
            wait_until(&handle, |state| {
                state.input_attempts == attempts
                    && state.status == AnalyticsWriterStatus::Retrying { attempt }
            });
            assert!(lock(&handle.shared.state).requests[&2].is_none());
        }
        drop(crate::events::EventCache::open_scoped(&cache).unwrap());
        clock.0.store(10_000, Ordering::Release);
        handle.wake_for_clock();
        wait_until(&handle, |state| {
            state.requests[&2] == Some(AnalyticsWriterStatus::Idle)
        });
        {
            let state = lock(&handle.shared.state);
            assert!(matches!(
                state.requests[&1].as_ref(),
                Some(AnalyticsWriterStatus::Refused { .. })
            ));
            assert_eq!(state.consumed, 2);
            assert_eq!(state.input_attempts, 6);
            assert_eq!(state.published, 1);
        }
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn requests_queued_before_first_operation_require_separate_successful_publications() {
        let root = temp_root("requests-already-queued-success");
        let cache = root.join("index.json");
        drop(crate::events::EventCache::open_scoped(&cache).unwrap());
        let clock = Arc::new(ManualClock(std::sync::atomic::AtomicI64::new(0)));
        let handle = start_with_two_queued_requests(
            AnalyticsWriterConfig::new(
                "machine",
                crate::events::EventCache::path(&cache),
                root.join("store"),
            ),
            Arc::clone(&clock),
        );
        handle.wait_for_publishes(1);
        handle.wait_for_idle_wait();
        {
            let state = lock(&handle.shared.state);
            assert_eq!(state.requests[&1], Some(AnalyticsWriterStatus::Idle));
            assert!(
                state.requests[&2].is_none(),
                "A's publication must not settle B"
            );
            assert_eq!(state.consumed, 1);
        }
        clock.0.store(2_000, Ordering::Release);
        handle.wake_for_clock();
        wait_until(&handle, |state| {
            state.requests[&2] == Some(AnalyticsWriterStatus::Idle)
        });
        assert_eq!(handle.published(), 2);
        handle.stop_and_join();
        fs::remove_dir_all(root).unwrap();
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
    fn request_arriving_during_backoff_gets_its_own_full_failure_budget() {
        let root = temp_root("request-backoff-generation");
        let cache = root.join("index.json");
        let clock = Arc::new(ManualClock(std::sync::atomic::AtomicI64::new(0)));
        let handle = start_with_clock(
            AnalyticsWriterConfig::new(
                "machine",
                crate::events::EventCache::path(&cache),
                root.join("store"),
            ),
            Arc::clone(&clock),
        );
        handle.wait_for_idle_wait();
        thread::scope(|scope| {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let handle_ref = &handle;
            let first = scope.spawn(move || {
                handle_ref.publish_and_wait_until(|| std::time::Instant::now() >= deadline)
            });
            wait_until(&handle, |state| {
                state.input_attempts == 1
                    && state.status == AnalyticsWriterStatus::Retrying { attempt: 1 }
            });
            clock.0.store(2_000, Ordering::Release);
            handle.wake_for_clock();
            wait_until(&handle, |state| {
                state.input_attempts == 2
                    && state.status == AnalyticsWriterStatus::Retrying { attempt: 2 }
            });
            let second = scope.spawn(move || {
                handle_ref.publish_and_wait_until(|| std::time::Instant::now() >= deadline)
            });
            handle.wait_for_requests(2);
            clock.0.store(4_000, Ordering::Release);
            handle.wake_for_clock();
            let AnalyticsWriterStatus::Refused { reason } = first.join().unwrap() else {
                panic!("request A must exhaust its own budget");
            };
            assert!(reason.contains("after 3 failures"));
            assert_eq!(
                handle.unsettled_requests(),
                1,
                "request B did not start A's recovery chain"
            );
            for (attempts, attempt, now) in [(4, 1, 6_000), (5, 2, 8_000)] {
                clock.0.store(now, Ordering::Release);
                handle.wake_for_clock();
                wait_until(&handle, |state| {
                    state.input_attempts == attempts
                        && state.status == AnalyticsWriterStatus::Retrying { attempt }
                });
                assert_eq!(handle.unsettled_requests(), 1);
            }
            // B has survived two failures of its own and can still succeed.
            drop(crate::events::EventCache::open_scoped(&cache).unwrap());
            clock.0.store(10_000, Ordering::Release);
            handle.wake_for_clock();
            assert_eq!(second.join().unwrap(), AnalyticsWriterStatus::Idle);
        });
        assert_eq!(handle.input_attempts(), 6);
        assert_eq!(handle.published(), 1);
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
    fn scheduler_spaces_recovery_and_preserves_retirement_deadlines() {
        let mut scheduler = Scheduler::new(0);
        assert_eq!(
            scheduler.operation_due(0, true, true, 1),
            Some(Operation::Publish)
        );
        scheduler.publication_attempted(0, Duration::from_secs(3_600));
        assert_eq!(
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Reload(Resume::Publish),
                    operational: true
                },
                0
            ),
            ChainEffect::Retrying(1)
        );
        assert_eq!(scheduler.operation_due(999, true, true, 2), None);
        assert_eq!(
            scheduler.operation_due(1_000, true, true, 2),
            Some(Operation::Reload(Resume::Publish))
        );
        scheduler.transition(
            ChainEvent::Succeeded(Operation::Reload(Resume::Publish)),
            1_000,
        );
        let publication = scheduler.active.clone().unwrap();
        assert_eq!(scheduler.operation_due(1_000, true, true, 2), None);
        assert_eq!(scheduler.deadline(true, true), Some(600_000));
        assert_eq!(
            scheduler.operation_due(600_000, true, true, 2),
            Some(Operation::Retire)
        );
        assert_eq!(scheduler.suspended, Some(publication.clone()));
        for now in [600_000, 601_000] {
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Reload(Resume::Retire),
                    operational: true,
                },
                now,
            );
        }
        scheduler.transition(
            ChainEvent::Succeeded(Operation::Reload(Resume::Retire)),
            603_000,
        );
        scheduler.transition(ChainEvent::Succeeded(Operation::Retire), 603_000);
        assert_eq!(scheduler.active, Some(publication));
        assert_eq!(scheduler.deadline(true, true), Some(1_203_000));
        for now in [1_203_000, 1_803_000, 2_403_000, 3_003_000] {
            assert_eq!(
                scheduler.operation_due(now, true, true, 2),
                Some(Operation::Retire)
            );
            scheduler.transition(ChainEvent::Succeeded(Operation::Retire), now);
        }
        assert_eq!(
            scheduler.operation_due(3_600_000, true, true, 2),
            Some(Operation::Publish)
        );
        assert_eq!(scheduler.active.as_ref().unwrap().captured_generation, 1);
        assert_eq!(
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Publish,
                    operational: true
                },
                3_600_000
            ),
            ChainEffect::Retrying(2)
        );
        assert_eq!(
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Publish,
                    operational: true
                },
                3_602_000
            ),
            ChainEffect::Refused {
                generation: 1,
                exhausted: Some(3)
            }
        );
    }

    #[test]
    fn no_writer_cooldown_and_refusal_cannot_share_a_chain_budget() {
        let mut scheduler = Scheduler::new(0);
        assert_eq!(
            scheduler.operation_due(0, true, false, 1),
            Some(Operation::Open)
        );
        for now in [0, 1_000] {
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Open,
                    operational: true,
                },
                now,
            );
        }
        assert_eq!(
            scheduler.transition(ChainEvent::NoWriter, 3_000),
            ChainEffect::NoWriter(1)
        );
        assert_eq!(scheduler.operation_due(32_999, true, false, 2), None);
        assert_eq!(scheduler.deadline(true, false), Some(33_000));
        assert_eq!(
            scheduler.operation_due(33_000, true, false, 2),
            Some(Operation::Open)
        );
        assert_eq!(
            scheduler.transition(
                ChainEvent::Failed {
                    resume: Operation::Open,
                    operational: true
                },
                33_000
            ),
            ChainEffect::Retrying(1)
        );
        assert_eq!(scheduler.active.as_ref().unwrap().failures, 1);
    }

    #[test]
    fn scheduler_deadlines_saturate_and_waits_remain_in_the_future() {
        let mut scheduler = Scheduler::new(i64::MAX - 1);
        scheduler.publication_attempted(i64::MAX - 1, Duration::MAX);
        assert_eq!(scheduler.next_cycle, i64::MAX);
        assert_eq!(scheduler.operation_due(i64::MAX - 1, true, true, 1), None);
        for operation in [
            Operation::Open,
            Operation::Publish,
            Operation::Compact,
            Operation::Retire,
            Operation::Reload(Resume::Publish),
            Operation::Reload(Resume::Compact),
            Operation::Reload(Resume::Retire),
        ] {
            for retry_at in [None, Some(900_000)] {
                for has_notification in [false, true] {
                    for now in [0, 599_999, 600_000, 899_999, 900_000, 3_599_999, 3_600_000] {
                        let mut scheduler = Scheduler::new(0);
                        scheduler.next_cycle = 3_600_000;
                        let kind = if matches!(
                            operation,
                            Operation::Retire | Operation::Reload(Resume::Retire)
                        ) {
                            ChainKind::Maintenance
                        } else {
                            ChainKind::Publication
                        };
                        scheduler.active = Some(Chain {
                            retry_at,
                            ..Chain::new(1, kind, operation)
                        });
                        if scheduler
                            .operation_due(now, has_notification, true, 2)
                            .is_none()
                        {
                            assert!(
                                scheduler
                                    .deadline(has_notification, true)
                                    .is_none_or(|deadline| deadline > now),
                                "{scheduler:?} at {now}"
                            );
                        }
                    }
                }
            }
        }
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
