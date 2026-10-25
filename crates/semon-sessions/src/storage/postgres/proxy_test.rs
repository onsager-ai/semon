//! Actual TCP reply withholding. Readers continue ACKing a live TCP connection;
//! notification events and pg_stat_activity, rather than elapsed sleeps, gate
//! each fault. All proxy children are aborted and joined on fixture drop.
use super::{tests::*, *};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::JoinSet,
};

struct Gate {
    frozen: AtomicBool,
    changed: Notify,
    observed: AtomicUsize,
}
impl Gate {
    fn new(frozen: bool) -> Arc<Self> {
        Arc::new(Self {
            frozen: AtomicBool::new(frozen),
            changed: Notify::new(),
            observed: AtomicUsize::new(0),
        })
    }
    fn set(&self, frozen: bool) {
        self.frozen.store(frozen, Ordering::SeqCst);
        self.changed.notify_waiters();
    }
    async fn wait(&self) {
        if self.frozen.load(Ordering::SeqCst) {
            self.observed.fetch_add(1, Ordering::SeqCst);
        }
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self.frozen.load(Ordering::SeqCst) {
                return;
            }
            changed.await;
        }
    }
    fn witness(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.observed.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "no proxy freeze event");
            std::thread::yield_now();
        }
    }
}
struct Proxy {
    addr: SocketAddr,
    replies: Arc<Gate>,
    commit: Arc<Gate>,
    cancels: Arc<Gate>,
    stop: Arc<Notify>,
    reset: Arc<Notify>,
    reset_done: Arc<AtomicUsize>,
    startups: Arc<AtomicUsize>,
    commits: Arc<AtomicUsize>,
    task: Option<tokio::task::JoinHandle<()>>,
    runtime: Option<tokio::runtime::Runtime>,
}
impl Proxy {
    fn new(target: SocketAddr, startup: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let replies = Gate::new(startup);
        let commit = Gate::new(false);
        let cancels = Gate::new(false);
        let stop = Arc::new(Notify::new());
        let reset = Arc::new(Notify::new());
        let reset_done = Arc::new(AtomicUsize::new(0));
        let startups = Arc::new(AtomicUsize::new(0));
        let commits = Arc::new(AtomicUsize::new(0));
        let (startup_events, commit_events) = (startups.clone(), commits.clone());
        let (r, c, s, arm, reset_event, reset_epoch) = (
            replies.clone(),
            cancels.clone(),
            stop.clone(),
            commit.clone(),
            reset.clone(),
            reset_done.clone(),
        );
        let task = runtime.spawn(async move {
            let mut children = JoinSet::new(); let mut ordinal = 0usize;
            loop {
                tokio::select! {
                    _ = s.notified() => break,
                    _ = reset_event.notified() => {
                        children.abort_all(); while children.join_next().await.is_some() {}
                        ordinal = 0; reset_epoch.fetch_add(1, Ordering::SeqCst);
                    },
                    Some(_) = children.join_next(), if !children.is_empty() => (),
                    accepted = listener.accept() => {
                        let (mut client, _) = accepted.unwrap(); let r = r.clone(); let c = c.clone(); let arm = arm.clone();
                        client.set_nodelay(true).unwrap();
                        let startups = startup_events.clone(); let commits = commit_events.clone();
                        let connection_ordinal = ordinal; ordinal += 1;
                        children.spawn(async move {
                            let mut server = TcpStream::connect(target).await.unwrap();
                            server.set_nodelay(true).unwrap();
                            let mut header = [0u8; 8];
                            if client.read_exact(&mut header).await.is_err() { return; }
                            let code = u32::from_be_bytes(header[4..8].try_into().unwrap());
                            // A TLS proxy cannot inspect encrypted CancelRequest.
                            // In the dedicated TLS test the first connection is
                            // the main client and the second is cancellation;
                            // withhold its SSL negotiation response explicitly.
                            if code == 80877102 || (code == 80877103 && connection_ordinal > 0) { c.wait().await; }
                            let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
                            if !(8..=65536).contains(&len) { return; }
                            let mut startup = vec![0; len]; startup[..8].copy_from_slice(&header);
                            if client.read_exact(&mut startup[8..]).await.is_err() || server.write_all(&startup).await.is_err() { return; }
                            if code == 80877102 { return; }
                            let (mut cr, mut cw) = client.into_split(); let (mut sr, mut sw) = server.into_split();
                            let upstream = async {
                                if code == 80877103 {
                                    let result = tokio::io::copy(&mut cr, &mut sw).await;
                                    if r.frozen.load(Ordering::SeqCst) { std::future::pending::<()>().await; }
                                    return result.map(|_| ());
                                }
                                loop {
                                    let tag = match cr.read_u8().await {
                                        Ok(tag) => tag,
                                        Err(error) => {
                                            // Deliberately keep the remote socket established
                                            // after the timed-out client closes. Recovery owns
                                            // teardown; this exposes remote residuals honestly.
                                            if r.frozen.load(Ordering::SeqCst) { std::future::pending::<()>().await; }
                                            return Err(error);
                                        }
                                    };
                                    let len = cr.read_u32().await?;
                                    if !(4..=1048576).contains(&len) { return std::io::Result::Ok(()); }
                                    let mut body = vec![0; len as usize - 4]; cr.read_exact(&mut body).await?;
                                    let commit = tag == b'Q' && body == b"COMMIT\0";
                                    let armed = commit && arm.frozen.load(Ordering::SeqCst);
                                    if armed { r.set(true); }
                                    let mut frame = Vec::with_capacity(1 + len as usize);
                                    frame.push(tag); frame.extend_from_slice(&len.to_be_bytes()); frame.extend_from_slice(&body);
                                    sw.write_all(&frame).await?;
                                    if commit { commits.fetch_add(1, Ordering::SeqCst); }
                                    // Witness only after the real COMMIT frame is forwarded.
                                    if armed { arm.observed.fetch_add(1, Ordering::SeqCst); }
                                }
                            };
                            let downstream = async {
                                // TLS stays opaque. Plain PostgreSQL frames let us
                                // witness the first actual ReadyForQuery, not accept().
                                if code != 80877103 {
                                    let mut startup = true;
                                    loop {
                                        let tag = sr.read_u8().await?;
                                        let len = sr.read_u32().await?;
                                        if !(4..=1048576).contains(&len) { return std::io::Result::Ok(()); }
                                        let mut frame = vec![0; 1 + len as usize];
                                        frame[0] = tag; frame[1..5].copy_from_slice(&len.to_be_bytes());
                                        sr.read_exact(&mut frame[5..]).await?;
                                        r.wait().await;
                                        cw.write_all(&frame).await?;
                                        if startup && tag == b'Z' {
                                            startup = false; startups.fetch_add(1, Ordering::SeqCst);
                                        }
                                    }
                                }
                                let mut bytes = [0u8; 8192];
                                loop {
                                    let n = sr.read(&mut bytes).await?; if n == 0 { return std::io::Result::Ok(()); }
                                    r.wait().await;
                                    cw.write_all(&bytes[..n]).await?;
                                }
                            };
                            tokio::select! { _ = upstream => (), _ = downstream => () }
                        });
                    }
                }
            }
            children.abort_all(); while children.join_next().await.is_some() {}
        });
        Self {
            addr,
            replies,
            commit,
            cancels,
            stop,
            reset,
            reset_done,
            startups,
            commits,
            task: Some(task),
            runtime: Some(runtime),
        }
    }
    fn recover(&self) {
        self.replies.set(false);
        self.cancels.set(false);
        self.commit.set(false);
        let before = self.reset_done.load(Ordering::SeqCst);
        self.reset.notify_one();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.reset_done.load(Ordering::SeqCst) == before {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.notify_one();
        self.runtime
            .as_ref()
            .unwrap()
            .block_on(self.task.take().unwrap())
            .unwrap();
        self.runtime
            .take()
            .unwrap()
            .shutdown_timeout(Duration::from_secs(1));
    }
}

fn target(factory: &Factory) -> SocketAddr {
    let config: tokio_postgres::Config = factory.dsn.parse().unwrap();
    let host = match &config.get_hosts()[0] {
        tokio_postgres::config::Host::Tcp(host) => host,
        _ => panic!("TCP fixture required"),
    };
    format!(
        "{}:{}",
        host,
        config.get_ports().first().copied().unwrap_or(5432)
    )
    .parse()
    .unwrap()
}
struct ProxyFactory {
    real: Arc<Factory>,
    addr: SocketAddr,
}
impl PgConnectionFactory for ProxyFactory {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            // Test DSN is a single TCP endpoint. Reconstruct it to avoid appending
            // another host and silently falling back around the frozen proxy.
            let mut config: tokio_postgres::Config = format!(
                "host=127.0.0.1 port={} user=semon_f1 password=ephemeral-f1 dbname=postgres",
                self.addr.port()
            )
            .parse()
            .unwrap();
            config.options(format!("-c search_path={} -c idle_in_transaction_session_timeout=10000 -c idle_session_timeout=10000 -c statement_timeout=10000", self.real.schema));
            PgConnection::connect(config, tokio_postgres::NoTls).await
        })
    }
}
fn proxy_pool(real: Arc<Factory>, proxy: &Proxy) -> PgPool {
    PgPool::new(
        Arc::new(ProxyFactory {
            real,
            addr: proxy.addr,
        }),
        PoolLimits {
            max_connections: 1,
            txn_deadline: Duration::from_millis(500),
            cancel_timeout: Duration::from_millis(100),
            cancel_grace: Duration::from_millis(100),
            ..limits()
        },
    )
    .unwrap()
}
fn residuals(real: &Factory, pid: i32) -> i64 {
    real.client()
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE pid=$1",
            &[&pid],
        )
        .unwrap()
        .get(0)
}
fn quiesce(real: &Factory, pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while residuals(real, pid) != 0 {
        assert!(
            Instant::now() < deadline,
            "remote backend did not quiesce after proxy recovery"
        );
        std::thread::yield_now();
    }
}

// A read-only control on the exact fault pool/proxy/namespace. It cannot create
// the target receipt or effect, so the later apply_operation must run its callback.
fn live_control(
    pool: &PgPool,
    proxy: &Proxy,
    label: &str,
    startups_before: usize,
    expected_receipts: i64,
) -> i32 {
    let calls = AtomicUsize::new(0);
    let commits_before = proxy.commits.load(Ordering::SeqCst);
    let start = Instant::now();
    let result = pool.transaction(
        &ns("a"),
        super::txn::TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            let pid = tx
                .driver
                .query_one("SELECT pg_backend_pid()", &[])
                .map_err(super::sqlstate::error)?
                .get::<_, i32>(0);
            let receipts = tx
                .driver
                .query_one(
                    "SELECT count(*) FROM semon_pg_operations WHERE operation_id=$1",
                    &[&identity().operation_id],
                )
                .map_err(super::sqlstate::error)?
                .get::<_, i64>(0);
            assert_eq!(receipts, expected_receipts, "{label}: target receipts");
            Ok(pid)
        },
    );
    let startups = proxy.startups.load(Ordering::SeqCst);
    let commits = proxy.commits.load(Ordering::SeqCst);
    eprintln!("{label}: result={result:?} callbacks={} startups={startups} commits={commits} elapsed={:?}",
        calls.load(Ordering::SeqCst), start.elapsed());
    assert!(result.is_ok(), "{label}: no-fault control failed: {result:?}");
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(startups > startups_before, "{label}: no fresh ReadyForQuery");
    assert_eq!(
        commits,
        commits_before + 1,
        "{label}: control COMMIT not forwarded"
    );
    assert_eq!(
        pool.accounted(),
        1,
        "{label}: clean control must remain reusable"
    );
    result.unwrap()
}

fn target_receipts(real: &Factory) -> i64 {
    let mut client = real.client();
    client
        .query_one(
            "SELECT set_config('semon.namespace', $1, false)",
            &[&super::namespace_context(&ns("a"))],
        )
        .unwrap();
    client
        .query_one(
            "SELECT count(*) FROM semon_pg_operations WHERE operation_id=$1",
            &[&identity().operation_id],
        )
        .unwrap()
        .get(0)
}

#[test]
fn pg_proxy_startup_reply_freeze_bounds_acquisition() {
    let (_, real) = fixture(|_| {});
    let proxy = Proxy::new(target(&real), true);
    let pool = proxy_pool(real.clone(), &proxy);
    let start = Instant::now();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| pool.acquire(&PgCancellation::default()));
        proxy.replies.witness();
        assert_eq!(pool.accounted(), 1, "startup I/O owns reserved capacity");
        assert!(matches!(
            worker.join().unwrap(),
            Err(crate::storage::StorageError::Busy)
        ));
    });
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(pool.accounted(), 0);
    proxy.replies.set(false);
    let lease = pool.acquire(&PgCancellation::default()).unwrap();
    assert_eq!(pool.accounted(), 1);
    drop(lease);
    assert_eq!(pool.accounted(), 0);
    proxy.recover();
}
#[test]
fn pg_proxy_read_and_write_reply_freeze_never_submits_commit() {
    for write in [false, true] {
        let (_, real) = fixture(|_| {});
        let proxy = Proxy::new(target(&real), false);
        let pool = proxy_pool(real.clone(), &proxy);
        let control_pid = live_control(&pool, &proxy, "precommit control", 0, 0);
        assert_eq!(count(&real), 0);
        let commits_before = proxy.commits.load(Ordering::SeqCst);
        let calls = AtomicUsize::new(0);
        let mut pid = 0;
        let start = Instant::now();
        let fault = |tx: &mut super::txn::PgTxn<'_>| {
            calls.fetch_add(1, Ordering::SeqCst);
            pid = tx
                .driver
                .query_one("SELECT pg_backend_pid()", &[])
                .unwrap()
                .get(0);
            assert_eq!(pool.accounted(), 1);
            if write {
                effect(tx)?;
            }
            proxy.replies.set(true);
            proxy.cancels.set(true);
            tx.driver
                .batch_execute("SELECT 42")
                .map_err(super::sqlstate::error)?;
            Ok(serde_json::json!(0))
        };
        let result = if write {
            pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), fault)
        } else {
            pool.transaction(
                &ns("a"),
                super::txn::TxnKind::ReadSnapshot,
                &PgCancellation::default(),
                fault,
            )
        };
        proxy.replies.witness();
        proxy.cancels.witness();
        eprintln!("precommit write={write}: result={result:?} callbacks={} commits={} elapsed={:?}",
            calls.load(Ordering::SeqCst), proxy.commits.load(Ordering::SeqCst), start.elapsed());
        assert_eq!(result, Err(crate::storage::StorageError::Busy));
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(pid, control_pid, "fault must use the controlled connection");
        assert_eq!(
            proxy.commits.load(Ordering::SeqCst),
            commits_before,
            "precommit fault must never forward COMMIT"
        );
        assert_eq!(pool.accounted(), 0);
        eprintln!(
            "frozen remote residual pid={pid}: {} (local bound excludes remote PIDs)",
            residuals(&real, pid)
        );
        let startups_before = proxy.startups.load(Ordering::SeqCst);
        proxy.recover();
        quiesce(&real, pid);
        assert_eq!(count(&real), 0);
        assert_eq!(target_receipts(&real), 0);
        let replacement = live_control(&pool, &proxy, "precommit recovered", startups_before, 0);
        assert_eq!(
            pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()),
            Ok(Reconciliation::Absent)
        );
        assert_ne!(replacement, pid);
        assert_eq!(pool.accounted(), 1);
        let reused = pool
            .transaction(
                &ns("a"),
                super::txn::TxnKind::ReadSnapshot,
                &PgCancellation::default(),
                |tx| {
                    Ok(tx
                        .driver
                        .query_one("SELECT pg_backend_pid()", &[])
                        .unwrap()
                        .get::<_, i32>(0))
                },
            )
            .unwrap();
        assert_eq!(replacement, reused);
        if write {
            let retry_calls = AtomicUsize::new(0);
            pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |tx| {
                retry_calls.fetch_add(1, Ordering::SeqCst);
                effect(tx)
            })
            .unwrap();
            pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |_| {
                panic!("explicit retry must apply exactly once")
            })
            .unwrap();
            assert_eq!(count(&real), 1);
            assert_eq!(target_receipts(&real), 1);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(retry_calls.load(Ordering::SeqCst), 1);
        }
    }
}
#[test]
fn pg_proxy_durable_commit_withheld_reply_reconciles_without_callback_replay() {
    let (_, real) = fixture(|_| {});
    let proxy = Proxy::new(target(&real), false);
    let pool = proxy_pool(real.clone(), &proxy);
    let control_pid = live_control(&pool, &proxy, "durable control", 0, 0);
    assert_eq!(count(&real), 0);
    assert_eq!(target_receipts(&real), 0);
    let commits_before = proxy.commits.load(Ordering::SeqCst);
    let calls = AtomicUsize::new(0);
    let mut pid = 0;
    let start = Instant::now();
    let result = pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |tx| {
        calls.fetch_add(1, Ordering::SeqCst);
        let value = effect(tx)?;
        pid = tx
            .driver
            .query_one("SELECT pg_backend_pid()", &[])
            .unwrap()
            .get(0);
        // Proxy parses the Q/COMMIT frame, freezes replies BEFORE forwarding it,
        // and reports a protocol event. No timing guess or synthetic ACK hook.
        proxy.commit.set(true);
        Ok(value)
    });
    proxy.commit.witness();
    eprintln!("durable armed COMMIT forwarded: {}; result={result:?} callbacks={} elapsed={:?}",
        proxy.commit.observed.load(Ordering::SeqCst), calls.load(Ordering::SeqCst), start.elapsed());
    assert_eq!(proxy.commit.observed.load(Ordering::SeqCst), 1);
    assert_eq!(proxy.commits.load(Ordering::SeqCst), commits_before + 1);
    assert_eq!(result, Err(crate::storage::StorageError::CommitUncertain));
    proxy.replies.witness();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(pid, control_pid, "fault must use the controlled connection");
    assert_eq!(pool.accounted(), 0);
    // Direct same-role witness: COMMIT is durable while its ACK is withheld.
    assert_eq!(count(&real), 1);
    assert_eq!(target_receipts(&real), 1);
    let row = real
        .client()
        .query_one(
            "SELECT state, query FROM pg_stat_activity WHERE pid=$1",
            &[&pid],
        )
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "idle");
    assert_eq!(row.get::<_, String>(1), "COMMIT");
    eprintln!(
        "frozen committed remote residual pid={pid}: {}",
        residuals(&real, pid)
    );
    let startups_before = proxy.startups.load(Ordering::SeqCst);
    proxy.recover();
    quiesce(&real, pid);
    let fresh = live_control(&pool, &proxy, "durable recovered", startups_before, 1);
    assert_ne!(pid, fresh);
    assert_eq!(
        pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()),
        Ok(Reconciliation::Committed(serde_json::json!({"receipt": 1})))
    );
    pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |_| {
        panic!("must reconcile without replay")
    })
    .unwrap();
    assert_eq!(count(&real), 1);
    assert_eq!(target_receipts(&real), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let replacement = pool
        .transaction(
            &ns("a"),
            super::txn::TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| {
                Ok(tx
                    .driver
                    .query_one("SELECT pg_backend_pid()", &[])
                    .unwrap()
                    .get::<_, i32>(0))
            },
        )
        .unwrap();
    assert_ne!(pid, replacement);
    assert_eq!(fresh, replacement);
    assert_eq!(pool.accounted(), 1);
}

#[cfg(feature = "postgres-test-tls")]
pub(super) fn tls_cancel_handshake_freeze(
    dsn: &str,
    schema: &str,
    tls: postgres_native_tls::MakeTlsConnector,
) {
    struct TlsProxyFactory {
        dsn: String,
        schema: String,
        tls: postgres_native_tls::MakeTlsConnector,
    }
    impl PgConnectionFactory for TlsProxyFactory {
        fn connect(&self) -> ConnectFuture<'_> {
            Box::pin(async move {
                let mut config: tokio_postgres::Config = self.dsn.parse().unwrap();
                config.options(format!("-c search_path={} -c idle_session_timeout=10000 -c idle_in_transaction_session_timeout=10000 -c statement_timeout=10000", self.schema));
                PgConnection::connect(config, self.tls.clone()).await
            })
        }
    }
    let config: tokio_postgres::Config = dsn.parse().unwrap();
    let port = config.get_ports()[0];
    let proxy = Proxy::new(format!("127.0.0.1:{port}").parse().unwrap(), false);
    let proxied = dsn.replace(
        &format!("port={port}"),
        &format!("port={} hostaddr=127.0.0.1", proxy.addr.port()),
    );
    let pool = PgPool::new(
        Arc::new(TlsProxyFactory {
            dsn: proxied,
            schema: schema.into(),
            tls: tls.clone(),
        }),
        PoolLimits {
            max_connections: 1,
            txn_deadline: Duration::from_millis(500),
            cancel_timeout: Duration::from_millis(100),
            cancel_grace: Duration::from_millis(100),
            ..limits()
        },
    )
    .unwrap();
    let start = Instant::now();
    let mut pid = 0;
    let result = pool.transaction(
        &ns("tls-freeze"),
        super::txn::TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            let row = tx
                .driver
                .query_one(
                    "SELECT pg_backend_pid(), ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()",
                    &[],
                )
                .unwrap();
            pid = row.get(0);
            assert!(row.get::<_, bool>(1));
            proxy.replies.set(true);
            proxy.cancels.set(true);
            tx.driver
                .batch_execute("SELECT 42")
                .map_err(super::sqlstate::error)
        },
    );
    assert_eq!(result, Err(crate::storage::StorageError::Busy));
    assert!(start.elapsed() < Duration::from_secs(3));
    proxy.replies.witness();
    proxy.cancels.witness();
    assert_eq!(pool.accounted(), 0);
    let config: ::postgres::Config = dsn.parse().unwrap();
    let mut direct = config.connect(tls.clone()).unwrap();
    let residual: i64 = direct
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE pid=$1",
            &[&pid],
        )
        .unwrap()
        .get(0);
    eprintln!("TLS frozen main/cancel remote residual pid={pid}: {residual}");
    proxy.recover();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if !direct
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)",
                &[&pid],
            )
            .unwrap()
            .get::<_, bool>(0)
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let replacement = pool
        .transaction(
            &ns("tls-freeze"),
            super::txn::TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| {
                Ok(tx
                    .driver
                    .query_one("SELECT pg_backend_pid()", &[])
                    .unwrap()
                    .get::<_, i32>(0))
            },
        )
        .unwrap();
    assert_ne!(pid, replacement, "aborted TLS client must never be reused");
}
