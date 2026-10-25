use super::cancel::PgCancellation;
use crate::storage::StorageError;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    runtime::{Handle, Runtime},
    task::JoinHandle,
};
use tokio_postgres::{Row, types::ToSql};

pub type ConnectFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PgConnection, StorageError>> + Send + 'a>>;
type CancelFuture = Pin<Box<dyn Future<Output = Result<(), tokio_postgres::Error>> + Send>>;

/// Bounds local adapter leases and I/O, not remote PostgreSQL backend PIDs.
#[derive(Clone, Copy, Debug)]
pub struct PoolLimits {
    pub max_connections: usize,
    pub acquire_timeout: Duration,
    pub statement_timeout: Duration,
    pub lock_timeout: Duration,
    pub idle_in_tx_timeout: Duration,
    pub txn_deadline: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub cancel_timeout: Duration,
    pub cancel_grace: Duration,
}
impl PoolLimits {
    fn validate(self) -> Result<(), StorageError> {
        if self.max_connections == 0 || self.max_connections > 256 {
            return Err(StorageError::InvalidArguments);
        }
        for duration in [
            self.acquire_timeout,
            self.statement_timeout,
            self.lock_timeout,
            self.idle_in_tx_timeout,
            self.txn_deadline,
            self.idle_timeout,
            self.max_lifetime,
            self.cancel_timeout,
            self.cancel_grace,
        ] {
            if duration.is_zero() || duration > Duration::from_secs(86_400) {
                return Err(StorageError::InvalidArguments);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) enum DriverError {
    Pg(tokio_postgres::Error),
    Interrupted,
}

/// Private synchronous typed-port bridge. Every driver future is polled on the
/// pool runtime with a deadline; no borrowed callback outlives its caller.
pub(super) struct Driver {
    raw: tokio_postgres::Client,
    handle: Handle,
    pub(super) cancel: Arc<dyn Fn() -> CancelFuture + Send + Sync>,
    deadline: Instant,
    control: PgCancellation,
    limits: PoolLimits,
    pub(super) tainted: std::cell::Cell<bool>,
    pub(super) interrupted: std::cell::Cell<bool>,
}
impl Driver {
    pub(super) fn context(
        &mut self,
        deadline: Instant,
        control: &PgCancellation,
        limits: PoolLimits,
    ) {
        self.deadline = deadline;
        self.control = control.clone();
        self.limits = limits;
    }
    fn run<T>(
        &self,
        future: impl Future<Output = Result<T, tokio_postgres::Error>>,
    ) -> Result<T, DriverError> {
        if self.interrupted.get() {
            return Err(DriverError::Interrupted);
        }
        self.handle.block_on(async {
            tokio::pin!(future);
            tokio::select! {
                biased;
                result = &mut future => return result.map_err(DriverError::Pg),
                _ = tokio::time::sleep_until(self.deadline.into()) => (),
                _ = self.control.cancelled() => (),
            };
            self.tainted.set(true);
            // Inline, owned cancel future: dropping timeout closes its transport.
            // Even if the query finishes first, await/drop cancellation before
            // returning; no unaccounted helper survives the lease.
            let request = (self.cancel)();
            let cancel = tokio::time::timeout(self.limits.cancel_timeout, request);
            let grace = async {
                tokio::time::timeout(
                    self.limits.cancel_timeout + self.limits.cancel_grace,
                    &mut future,
                )
                .await
            };
            let (_, reply) = tokio::join!(cancel, grace);
            match reply {
                Ok(result) => result.map_err(DriverError::Pg),
                Err(_) => {
                    self.interrupted.set(true);
                    Err(DriverError::Interrupted)
                }
            }
        })
    }
    pub(super) fn batch_execute(&self, sql: &str) -> Result<(), DriverError> {
        self.run(self.raw.batch_execute(sql))
    }
    pub(super) fn query(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, DriverError> {
        self.run(self.raw.query(sql, params))
    }
    pub(super) fn query_one(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, DriverError> {
        self.run(self.raw.query_one(sql, params))
    }
    pub(super) fn execute(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<u64, DriverError> {
        self.run(self.raw.execute(sql, params))
    }
    pub(super) fn rollback(&mut self) -> Result<(), DriverError> {
        // A real rollback ACK may establish noncommit. Cancel alone never does.
        self.control = PgCancellation::default();
        self.deadline = Instant::now() + self.limits.cancel_grace;
        self.batch_execute("ROLLBACK")
    }
}

/// Host supplies config and TLS once. The same TLS connector is cloned for
/// cancellation. All connector futures must be cooperative, never blocking.
pub struct PgConnection {
    pub(super) client: Driver,
    io: Option<JoinHandle<()>>,
    pub(super) created_at: Instant,
    pub(super) returned_at: Instant,
}
impl PgConnection {
    pub async fn connect<T>(config: tokio_postgres::Config, tls: T) -> Result<Self, StorageError>
    where
        T: tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket>
            + Clone
            + Send
            + Sync
            + 'static,
        T::Stream: Send + 'static,
        T::TlsConnect: Send + 'static,
        <T::TlsConnect as tokio_postgres::tls::TlsConnect<tokio_postgres::Socket>>::Future:
            Send + 'static,
    {
        // Tokio DNS resolution may use an uncancellable blocking resolver.
        // Require host-owned numeric addresses; retain host for TLS validation.
        if config.get_user().is_none() {
            return Err(StorageError::InvalidArguments);
        }
        for (index, host) in config.get_hosts().iter().enumerate() {
            if let tokio_postgres::config::Host::Tcp(name) = host
                && config.get_hostaddrs().get(index).is_none()
                && name.parse::<std::net::IpAddr>().is_err()
            {
                return Err(StorageError::InvalidArguments);
            }
        }
        let (raw, connection) = config
            .connect(tls.clone())
            .await
            .map_err(|_| StorageError::Unavailable)?;
        let token = raw.cancel_token();
        let cancel = Arc::new(move || {
            let token = token.clone();
            let tls = tls.clone();
            Box::pin(async move { token.cancel_query(tls).await }) as CancelFuture
        });
        let io = tokio::spawn(async move {
            let _ = connection.await;
        });
        let now = Instant::now();
        // Context is replaced before first SQL use.
        let limits = PoolLimits {
            max_connections: 1,
            acquire_timeout: Duration::from_secs(1),
            statement_timeout: Duration::from_secs(1),
            lock_timeout: Duration::from_secs(1),
            idle_in_tx_timeout: Duration::from_secs(1),
            txn_deadline: Duration::from_secs(1),
            idle_timeout: Duration::from_secs(1),
            max_lifetime: Duration::from_secs(1),
            cancel_timeout: Duration::from_millis(100),
            cancel_grace: Duration::from_millis(100),
        };
        Ok(Self {
            client: Driver {
                raw,
                handle: Handle::current(),
                cancel,
                deadline: now + limits.acquire_timeout,
                control: PgCancellation::default(),
                limits,
                tainted: std::cell::Cell::new(false),
                interrupted: std::cell::Cell::new(false),
            },
            io: Some(io),
            created_at: now,
            returned_at: now,
        })
    }
    fn stop(&mut self) -> bool {
        if let Some(mut io) = self.io.take() {
            io.abort();
            let ended = self.client.handle.block_on(async {
                tokio::time::timeout(self.client.limits.cancel_grace, &mut io)
                    .await
                    .is_ok()
            });
            if !ended {
                self.io = Some(io);
                return false;
            }
        }
        true
    }
}
impl Drop for PgConnection {
    fn drop(&mut self) {
        // Ordinarily joined by Lease or Inner first. This fallback preserves
        // ownership even if a host violates the cooperative-I/O contract; it
        // never detaches a still-running task or falsely releases capacity.
        if !self.stop()
            && let Some(io) = self.io.take()
        {
            self.client.handle.block_on(async {
                let _ = io.await;
            });
        }
    }
}

pub trait PgConnectionFactory: Send + Sync + 'static {
    /// Must return cooperative async I/O. Pool timeout drops this future; the
    /// host must not spawn detached tasks or block a runtime worker.
    fn connect(&self) -> ConnectFuture<'_>;
}
struct State {
    count: usize,
    idle: Vec<PgConnection>,
    retired: Vec<PgConnection>,
}
pub(super) struct Inner {
    factory: Arc<dyn PgConnectionFactory>,
    state: Mutex<State>,
    changed: Condvar,
    pub(super) limits: PoolLimits,
    runtime: Option<Runtime>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        state.idle.clear();
        state.retired.clear();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(self.limits.cancel_grace);
        }
    }
}
#[derive(Clone)]
pub struct PgPool {
    pub(super) inner: Arc<Inner>,
}
impl PgPool {
    pub(super) fn new(
        factory: Arc<dyn PgConnectionFactory>,
        limits: PoolLimits,
    ) -> Result<Self, StorageError> {
        limits.validate()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| StorageError::Unavailable)?;
        Ok(Self {
            inner: Arc::new(Inner {
                factory,
                state: Mutex::new(State {
                    count: 0,
                    idle: Vec::new(),
                    retired: Vec::new(),
                }),
                changed: Condvar::new(),
                limits,
                runtime: Some(runtime),
            }),
        })
    }
    pub(super) fn acquire(&self, cancel: &PgCancellation) -> Result<Lease, StorageError> {
        let deadline = Instant::now() + self.inner.limits.acquire_timeout;
        loop {
            if cancel.is_cancelled() || Instant::now() >= deadline {
                return Err(StorageError::Busy);
            }
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| StorageError::Unavailable)?;
            // An unexpectedly slow abort remains owned and counted. Reap only
            // after its JoinHandle proves completion; never reuse it.
            let mut retired = std::mem::take(&mut state.retired);
            drop(state);
            let mut pending = Vec::new();
            let mut ended = 0;
            for mut connection in retired.drain(..) {
                if connection.io.as_ref().is_none_or(|io| io.is_finished()) && connection.stop() {
                    ended += 1;
                } else {
                    pending.push(connection);
                }
            }
            state = self
                .inner
                .state
                .lock()
                .map_err(|_| StorageError::Unavailable)?;
            state.count -= ended;
            state.retired.extend(pending);
            if let Some(connection) = state.idle.pop() {
                drop(state);
                let mut lease = Lease {
                    connection: Some(connection),
                    inner: self.inner.clone(),
                    reuse: false,
                };
                let connection = lease.connection.as_mut().ok_or(StorageError::Unavailable)?;
                connection
                    .client
                    .context(deadline, cancel, self.inner.limits);
                if connection.created_at.elapsed() >= self.inner.limits.max_lifetime {
                    drop(lease);
                    continue;
                }
                if connection.returned_at.elapsed() >= self.inner.limits.idle_timeout
                    && connection.client.batch_execute("SELECT 1").is_err()
                {
                    drop(lease);
                    continue;
                }
                if cancel.is_cancelled() || Instant::now() >= deadline {
                    return Err(StorageError::Busy);
                }
                return Ok(lease);
            }
            if state.count < self.inner.limits.max_connections {
                state.count += 1;
                drop(state);
                let mut lease = Lease {
                    connection: None,
                    inner: self.inner.clone(),
                    reuse: false,
                };
                let runtime = self
                    .inner
                    .runtime
                    .as_ref()
                    .ok_or(StorageError::Unavailable)?;
                let connection = runtime.block_on(async {
                    tokio::select! {
                        biased;
                        result = self.inner.factory.connect() => result,
                        _ = tokio::time::sleep_until(deadline.into()) => Err(StorageError::Busy),
                        _ = cancel.cancelled() => Err(StorageError::Busy),
                    }
                })?;
                lease.connection = Some(connection);
                lease.connection.as_mut().unwrap().client.context(
                    deadline,
                    cancel,
                    self.inner.limits,
                );
                return Ok(lease);
            }
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5));
            let _guard = self
                .inner
                .changed
                .wait_timeout(state, wait)
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
    pub(super) fn check_clean(&mut self) -> Result<(), StorageError> {
        #[cfg(test)]
        if super::tests::fail_clean() {
            return Err(StorageError::Unavailable);
        }
        let connection = self.connection.as_mut().ok_or(StorageError::Unavailable)?;
        if connection.client.tainted.get() {
            return Err(StorageError::Unavailable);
        }
        connection.client.context(
            Instant::now() + self.inner.limits.cancel_grace,
            &PgCancellation::default(),
            self.inner.limits,
        );
        let row = connection
            .client
            .query_one("SELECT current_setting('semon.namespace', true)", &[])
            .map_err(super::sqlstate::error)?;
        let namespace: Option<String> = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
        if namespace.is_some_and(|value| !value.is_empty()) {
            return Err(StorageError::NamespaceMismatch);
        }
        connection
            .client
            .batch_execute("DISCARD ALL")
            .map_err(super::sqlstate::error)?;
        connection.returned_at = Instant::now();
        self.reuse = true;
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.reuse
            && let Some(connection) = self.connection.take()
        {
            self.inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .idle
                .push(connection);
        } else {
            let mut connection = self.connection.take();
            let ended = connection.as_mut().is_none_or(PgConnection::stop);
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if ended {
                drop(connection);
                state.count -= 1;
            } else if let Some(connection) = connection {
                state.retired.push(connection);
            }
            // A missed join bound retains the slot and task in retired. It
            // cannot admit a replacement until actual completion is observed.
        }
        self.inner.changed.notify_one();
    }
}
