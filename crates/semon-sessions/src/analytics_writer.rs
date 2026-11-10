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

/// Starts a writer with an injected clock. Public for deterministic consumers
/// and tests; production callers normally use [`start_analytics_writer`].
pub fn start_with_clock<C>(config: AnalyticsWriterConfig, clock: Arc<C>) -> Handle
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

/// Descriptive alias for callers that prefer the expanded name.
pub type AnalyticsWriterHandle = Handle;

/// Descriptive crate-root spelling for callers that prefer it.
pub fn start_analytics_writer(config: AnalyticsWriterConfig) -> Handle {
    start(config)
}

#[derive(Clone, Copy)]
enum Work {
    Open,
    Publish,
    Compact,
    Retire,
    ReloadPublish,
    ReloadCompact,
    ReloadRetire,
}

fn run<C: Clock + Send + Sync + 'static>(
    config: AnalyticsWriterConfig,
    clock: Arc<C>,
    shared: Arc<Shared>,
) {
    let mut store = None;
    let mut retry = None;
    let mut retry_attempt: u32 = 0;
    let mut next_cycle = 0;
    let mut next_retirement = clock.now_ms() + duration_ms(RETIREMENT_CADENCE);
    let mut refused = false;

    loop {
        let now = clock.now_ms();
        let work = if refused {
            if has_notification(&shared) {
                refused = false;
                Some(if store.is_some() {
                    Work::Publish
                } else {
                    Work::Open
                })
            } else {
                None
            }
        } else if let Some((when, work)) = retry {
            (now >= when).then_some(work)
        } else if store.is_none() {
            Some(Work::Open)
        } else if has_notification(&shared) && now >= next_cycle {
            Some(Work::Publish)
        } else if now >= next_retirement {
            Some(Work::Retire)
        } else {
            None
        };

        let Some(work) = work else {
            let deadline = retry.map(|(when, _)| when).unwrap_or_else(|| {
                if refused {
                    return now.saturating_add(duration_ms(NO_WRITER_RETRY));
                }
                if store.is_none() {
                    now + duration_ms(NO_WRITER_RETRY)
                } else if has_notification(&shared) {
                    next_cycle
                } else {
                    next_retirement
                }
            });
            if wait_until(&shared, &clock, deadline) {
                break;
            }
            continue;
        };

        if stopped(&shared) {
            break;
        }
        set_status(&shared, AnalyticsWriterStatus::Running);
        retry = None;

        match work {
            Work::Open => match Store::open_with_clock(&config.store_root, Arc::clone(&clock)) {
                Ok(opened) => {
                    store = Some(opened);
                    retry_attempt = 0;
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(StoreError::WriterBusy) => {
                    set_status(&shared, AnalyticsWriterStatus::NoWriter);
                    retry = Some((now + duration_ms(NO_WRITER_RETRY), Work::Open));
                }
                Err(error) if is_refusal(&error) => {
                    set_status(
                        &shared,
                        AnalyticsWriterStatus::Refused {
                            reason: error.to_string(),
                        },
                    );
                    wait_for_notification(&shared);
                    refused = true;
                }
                Err(_error) => {
                    retry_attempt = retry_attempt.saturating_add(1);
                    set_status(
                        &shared,
                        AnalyticsWriterStatus::Retrying {
                            attempt: retry_attempt,
                        },
                    );
                    retry = Some((now + retry_delay(retry_attempt), Work::Open));
                }
            },
            Work::Publish => {
                #[cfg(test)]
                {
                    let mut state = lock(&shared.state);
                    state.input_attempts += 1;
                    shared.changed.notify_all();
                }
                let notification = notification_count(&shared);
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
                        #[cfg(test)]
                        {
                            let mut state = lock(&shared.state);
                            state.published += 1;
                            shared.changed.notify_all();
                        }
                        consume(&shared, notification);
                        retry_attempt = 0;
                        next_cycle = now + duration_ms(config.interval);
                        next_retirement = now + duration_ms(RETIREMENT_CADENCE);
                        set_status(&shared, AnalyticsWriterStatus::Idle);
                        retry = Some((now, Work::Compact));
                    }
                    Err(PublishError::Input(error)) if is_input_refusal(&error) => {
                        consume(&shared, notification);
                        set_status(
                            &shared,
                            AnalyticsWriterStatus::Refused {
                                reason: error.to_string(),
                            },
                        );
                        refused = true;
                    }
                    Err(PublishError::Store(error)) if is_refusal(&error) => {
                        set_status(
                            &shared,
                            AnalyticsWriterStatus::Refused {
                                reason: error.to_string(),
                            },
                        );
                        wait_for_notification(&shared);
                        refused = true;
                    }
                    Err(error) => {
                        retry_attempt = retry_attempt.saturating_add(1);
                        set_status(
                            &shared,
                            AnalyticsWriterStatus::Retrying {
                                attempt: retry_attempt,
                            },
                        );
                        let retry_work = if matches!(error, PublishError::Store(_)) {
                            Work::ReloadPublish
                        } else {
                            Work::Publish
                        };
                        retry = Some((now + retry_delay(retry_attempt), retry_work));
                    }
                }
            }
            Work::Compact => match store
                .as_mut()
                .expect("opened store")
                .compact(&config.compaction_policy)
            {
                Ok(_) => {
                    retry_attempt = 0;
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(error) if is_refusal(&error) => {
                    set_status(
                        &shared,
                        AnalyticsWriterStatus::Refused {
                            reason: error.to_string(),
                        },
                    );
                    wait_for_notification(&shared);
                    refused = true;
                }
                Err(_error) => {
                    retry_attempt = retry_attempt.saturating_add(1);
                    set_status(
                        &shared,
                        AnalyticsWriterStatus::Retrying {
                            attempt: retry_attempt,
                        },
                    );
                    retry = Some((now + retry_delay(retry_attempt), Work::ReloadCompact));
                }
            },
            Work::Retire => match store.as_mut().expect("opened store").retire() {
                Ok(_) => {
                    next_retirement = now + duration_ms(RETIREMENT_CADENCE);
                    retry_attempt = 0;
                    set_status(&shared, AnalyticsWriterStatus::Idle);
                }
                Err(error) if is_refusal(&error) => {
                    set_status(
                        &shared,
                        AnalyticsWriterStatus::Refused {
                            reason: error.to_string(),
                        },
                    );
                    wait_for_notification(&shared);
                    refused = true;
                }
                Err(_error) => {
                    retry_attempt = retry_attempt.saturating_add(1);
                    set_status(
                        &shared,
                        AnalyticsWriterStatus::Retrying {
                            attempt: retry_attempt,
                        },
                    );
                    retry = Some((now + retry_delay(retry_attempt), Work::ReloadRetire));
                }
            },
            Work::ReloadPublish | Work::ReloadCompact | Work::ReloadRetire => {
                let after_reload = match work {
                    Work::ReloadPublish => Work::Publish,
                    Work::ReloadCompact => Work::Compact,
                    Work::ReloadRetire => Work::Retire,
                    _ => unreachable!("matched reload work"),
                };
                match store.as_mut().expect("opened store").reload_manifest() {
                    Ok(()) => {
                        retry = Some((now, after_reload));
                    }
                    Err(error) if is_refusal(&error) => {
                        set_status(
                            &shared,
                            AnalyticsWriterStatus::Refused {
                                reason: error.to_string(),
                            },
                        );
                        wait_for_notification(&shared);
                        refused = true;
                    }
                    Err(_error) => {
                        retry_attempt = retry_attempt.saturating_add(1);
                        set_status(
                            &shared,
                            AnalyticsWriterStatus::Retrying {
                                attempt: retry_attempt,
                            },
                        );
                        retry = Some((now + retry_delay(retry_attempt), work));
                    }
                }
            }
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

fn is_input_refusal(error: &SqliteError) -> bool {
    matches!(error, SqliteError::InvalidQuery)
        || matches!(
            error.sqlite_error_code(),
            Some(code)
                if !matches!(
                    code,
                    ErrorCode::DatabaseBusy
                        | ErrorCode::DatabaseLocked
                        | ErrorCode::SystemIoFailure
                        | ErrorCode::CannotOpen
                )
        )
}

fn is_refusal(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::Invalid(_)
            | StoreError::Json(_)
            | StoreError::Parquet(_)
            | StoreError::Arrow(_)
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

fn stopped(shared: &Shared) -> bool {
    lock(&shared.state).stop
}

fn notification_count(shared: &Shared) -> u64 {
    lock(&shared.state).notifications
}

fn has_notification(shared: &Shared) -> bool {
    let state = lock(&shared.state);
    state.notifications != state.consumed
}

fn consume(shared: &Shared, notification: u64) {
    let mut state = lock(&shared.state);
    state.consumed = notification;
}

fn wait_for_notification(shared: &Shared) {
    let mut state = lock(&shared.state);
    state.consumed = state.notifications;
}

fn wait_until<C: Clock>(shared: &Shared, clock: &C, deadline: i64) -> bool {
    let mut state = lock(&shared.state);
    while !state.stop && clock.now_ms() < deadline {
        let remaining = (deadline - clock.now_ms()).max(1) as u64;
        let (next, _) = shared
            .changed
            .wait_timeout(state, Duration::from_millis(remaining))
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
    }
    state.stop
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
