use super::cancel::PgCancellation;
use crate::storage::StorageError;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

/// All durations must be nonzero and at most one day. A host must additionally
/// bound DNS/TLS/connect and cancellation I/O in its factory implementation.
#[derive(Clone, Copy, Debug)]
pub struct PoolLimits {
    pub max_connections: usize,
    pub acquire_timeout: Duration,
    pub statement_timeout: Duration,
    pub lock_timeout: Duration,
    pub idle_in_tx_timeout: Duration,
    pub txn_deadline: Duration,
}

impl PoolLimits {
    fn validate(self) -> Result<(), StorageError> {
        if self.max_connections == 0 || self.max_connections > 256 {
            return Err(StorageError::InvalidArguments);
        }
        for duration in [self.acquire_timeout, self.statement_timeout, self.lock_timeout,
            self.idle_in_tx_timeout, self.txn_deadline] {
            if duration.is_zero() || duration > Duration::from_secs(86_400) {
                return Err(StorageError::InvalidArguments);
            }
        }
        Ok(())
    }
}

/// An opaque owned connection. Driver objects enter only at host construction;
/// consumers never receive a Client, Transaction, Row or CancelToken.
pub struct PgConnection {
    pub(super) client: ::postgres::Client,
    pub(super) cancel: Arc<dyn Fn() -> Result<(), StorageError> + Send + Sync>,
}

impl PgConnection {
    /// `cancel` must use the same host TLS/endpoint policy as `client`, with a
    /// bounded connect timeout. Its success is only a request, never a server ACK.
    pub fn new(
        client: ::postgres::Client,
        cancel: impl Fn(::postgres::CancelToken) -> Result<(), StorageError> + Send + Sync + 'static,
    ) -> Self {
        let token = client.cancel_token();
        Self { client, cancel: Arc::new(move || cancel(token.clone())) }
    }
}

pub trait PgConnectionFactory: Send + Sync + 'static {
    /// Honor the supplied deadline/cancellation AND a bounded driver connect
    /// timeout. A noncompliant blocking factory keeps its slot until it returns;
    /// an acquire timeout cannot terminate arbitrary host code.
    fn connect(&self, deadline: Instant, cancel: &PgCancellation)
        -> Result<PgConnection, StorageError>;
}

struct State {
    count: usize,
    idle: Vec<PgConnection>,
}

pub(super) struct Inner {
    factory: Arc<dyn PgConnectionFactory>,
    state: Mutex<State>,
    changed: Condvar,
    pub(super) limits: PoolLimits,
}

#[derive(Clone)]
pub struct PgPool {
    pub(super) inner: Arc<Inner>,
}

impl PgPool {
    pub(super) fn new(factory: Arc<dyn PgConnectionFactory>, limits: PoolLimits)
        -> Result<Self, StorageError> {
        limits.validate()?;
        Ok(Self { inner: Arc::new(Inner {
            factory, state: Mutex::new(State { count: 0, idle: Vec::new() }),
            changed: Condvar::new(), limits,
        }) })
    }

    pub(super) fn acquire(&self, cancel: &PgCancellation) -> Result<Lease, StorageError> {
        let deadline = Instant::now() + self.inner.limits.acquire_timeout;
        loop {
            if cancel.is_cancelled() || Instant::now() >= deadline {
                return Err(StorageError::Busy);
            }
            let mut state = self.inner.state.lock().map_err(|_| StorageError::Unavailable)?;
            if let Some(connection) = state.idle.pop() {
                return Ok(Lease { connection: Some(connection), inner: self.inner.clone(), reuse: false });
            }
            if state.count < self.inner.limits.max_connections {
                state.count += 1; // reserve BEFORE starting any host work
                drop(state);
                let lease = Lease { connection: None, inner: self.inner.clone(), reuse: false };
                let factory = self.inner.factory.clone();
                let control = cancel.clone();
                let (send, receive) = mpsc::sync_channel(1);
                let opening = std::thread::Builder::new().name("semon-pg-connect".into()).spawn(move || {
                    let mut lease = lease;
                    let result = factory.connect(deadline, &control).map(|connection| {
                        lease.connection = Some(connection);
                        lease
                    });
                    // On timeout/disconnection the returned Lease drops here,
                    // after Client drop. Failed factories also release via RAII.
                    let _ = send.send(result);
                }).map_err(|_| StorageError::Unavailable)?;
                loop {
                    if cancel.is_cancelled() || Instant::now() >= deadline {
                        return Err(StorageError::Busy);
                    }
                    let wait = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(10));
                    match receive.recv_timeout(wait) {
                        Ok(result) => {
                            // No factory work remains after delivery. Join the
                            // successful helper before admitting the lease.
                            opening.join().map_err(|_| StorageError::Unavailable)?;
                            if cancel.is_cancelled() || Instant::now() >= deadline {
                                drop(result);
                                return Err(StorageError::Busy);
                            }
                            return result;
                        },
                        Err(mpsc::RecvTimeoutError::Timeout) => {},
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Err(StorageError::Unavailable),
                    }
                }
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(10));
            let _guard = self.inner.changed.wait_timeout(state, wait)
                .map_err(|_| StorageError::Unavailable)?;
        }
    }

    #[cfg(test)]
    pub(super) fn accounted(&self) -> usize {
        self.inner.state.lock().unwrap().count
    }
}

pub(super) struct Lease {
    pub(super) connection: Option<PgConnection>,
    inner: Arc<Inner>,
    pub(super) reuse: bool,
}

impl Lease {
    /// A successful DISCARD ALL is PostgreSQL's idle-state check: the command
    /// refuses inside any transaction block. Verify local namespace clearance
    /// first, so DISCARD cannot hide a leaked namespace.
    pub(super) fn check_clean(&mut self) -> Result<(), StorageError> {
        let connection = self.connection.as_mut().ok_or(StorageError::Unavailable)?;
        let row = connection.client.query_one("SELECT current_setting('semon.namespace', true)", &[])
            .map_err(|_| StorageError::Unavailable)?;
        let namespace: Option<String> = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
        if namespace.is_some_and(|value| !value.is_empty()) {
            return Err(StorageError::NamespaceMismatch);
        }
        connection.client.batch_execute("DISCARD ALL").map_err(|_| StorageError::Unavailable)?;
        self.reuse = true;
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.reuse {
            if let Some(connection) = self.connection.take() {
                self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).idle.push(connection);
                self.inner.changed.notify_one();
                return;
            }
        }
        // Capacity includes cancellation, rollback and Client/runtime teardown.
        drop(self.connection.take());
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).count -= 1;
        self.inner.changed.notify_one();
    }
}
