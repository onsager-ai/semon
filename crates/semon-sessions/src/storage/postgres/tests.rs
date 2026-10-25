use super::*;
use super::txn::TxnKind;
use crate::storage::{LogicalSourceNamespace, StorageError};
use serde_json::{Value, json};
use std::cell::Cell;
use std::sync::{Arc, Barrier, atomic::{AtomicUsize, Ordering}, mpsc};
use std::time::{Duration, Instant};

thread_local! { static LOSE_ACK: Cell<bool> = const { Cell::new(false) }; }
pub(super) fn lose_ack() -> bool { LOSE_ACK.with(|flag| flag.replace(false)) }
static NEXT_SCHEMA: AtomicUsize = AtomicUsize::new(0);

struct Factory { dsn: String, schema: String }
impl Factory {
    fn client(&self) -> ::postgres::Client {
        let mut config: ::postgres::Config = self.dsn.parse().expect("valid test DSN");
        config.options(&format!("-c search_path={}", self.schema));
        config.connect_timeout(Duration::from_secs(2));
        config.connect(::postgres::NoTls).expect("real ephemeral PostgreSQL")
    }
}
impl PgConnectionFactory for Factory {
    fn connect(&self, _: Instant, cancel: &PgCancellation) -> Result<PgConnection, StorageError> {
        if cancel.is_cancelled() { return Err(StorageError::Busy); }
        Ok(PgConnection::new(self.client(), |token| token.cancel_query(::postgres::NoTls).map_err(|_| StorageError::Unavailable)))
    }
}

fn limits() -> PoolLimits {
    PoolLimits {
        max_connections: 2, acquire_timeout: Duration::from_millis(100),
        statement_timeout: Duration::from_secs(2), lock_timeout: Duration::from_secs(1),
        idle_in_tx_timeout: Duration::from_secs(2), txn_deadline: Duration::from_secs(5),
    }
}

fn fixture(mut configure: impl FnMut(&mut PoolLimits)) -> (PgPool, Arc<Factory>) {
    let dsn = std::env::var("SEMON_PG_TEST_DSN").expect("SEMON_PG_TEST_DSN required: PG tests may not silently skip");
    let schema = format!("f1_{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::SeqCst));
    let factory = Arc::new(Factory { dsn, schema });
    let mut client = factory.client();
    client.batch_execute(&format!("CREATE SCHEMA {}", factory.schema)).unwrap();
    let row = client.query_one("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user", &[]).unwrap();
    assert!(!row.get::<_, bool>(0));
    assert!(!row.get::<_, bool>(1));
    let mut bounds = limits();
    configure(&mut bounds);
    let pool = PgPool::create_schema(factory.clone(), bounds).unwrap();
    factory.client().batch_execute("CREATE TABLE effects(namespace text NOT NULL, value integer NOT NULL)").unwrap();
    (pool, factory)
}

fn ns(value: &str) -> LogicalSourceNamespace { LogicalSourceNamespace::new(value).unwrap() }
fn identity() -> OperationIdentity<'static> {
    OperationIdentity { operation_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", kind: "test-effect", request_digest: &[7; 32] }
}
fn effect(tx: &mut super::txn::PgTxn<'_>) -> Result<Value, StorageError> {
    tx.driver.execute("INSERT INTO effects VALUES ($1, 1)", &[&tx.namespace.as_str()]).map_err(super::sqlstate::error)?;
    Ok(json!({"receipt": 1}))
}
fn count(factory: &Factory) -> i64 {
    factory.client().query_one("SELECT count(*) FROM effects", &[]).unwrap().get(0)
}

#[test]
fn pg_operation_replay_namespace_rls_and_restricted_role() {
    let (pool, factory) = fixture(|_| {});
    let control = PgCancellation::default();
    let outcome = pool.apply_operation(&ns("a"), &identity(), &control, effect).unwrap();
    assert_eq!(pool.apply_operation(&ns("a"), &identity(), &control, |_| panic!("must replay")), Ok(outcome.clone()));
    assert_eq!(pool.reconcile(&ns("a"), &identity(), &control), Ok(Reconciliation::Committed(outcome)));
    assert_eq!(pool.reconcile(&ns("b"), &identity(), &control), Ok(Reconciliation::Absent));
    pool.apply_operation(&ns("b"), &identity(), &control, effect).unwrap();
    assert_eq!(count(&factory), 2);
    let mismatch = OperationIdentity { kind: "other", ..identity() };
    assert_eq!(pool.reconcile(&ns("a"), &mismatch, &control), Err(StorageError::InvalidArguments));
    let mismatch = OperationIdentity { request_digest: &[8; 32], ..identity() };
    assert_eq!(pool.reconcile(&ns("a"), &mismatch, &control), Err(StorageError::InvalidArguments));
    // Forced RLS applies even to the restricted table-owning role. Outside a
    // namespaced transaction no operation from either source is visible.
    assert_eq!(factory.client().query_one("SELECT count(*) FROM semon_pg_operations", &[]).unwrap().get::<_, i64>(0), 0);
    pool.transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(tx.driver.query_one("SELECT count(*) FROM semon_pg_operations", &[]).unwrap().get::<_, i64>(0), 1);
        Ok(())
    }).unwrap();
}

#[test]
fn pg_commit_ack_loss_reconciles_exactly_one_effect() {
    let (pool, factory) = fixture(|_| {});
    LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), effect), Err(StorageError::CommitUncertain));
    assert_eq!(pool.accounted(), 0, "uncertain connection discarded");
    assert!(matches!(pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()), Ok(Reconciliation::Committed(_))));
    pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |_| panic!("must not re-execute")).unwrap();
    assert_eq!(count(&factory), 1);
}

#[test]
fn pg_concurrent_operation_writers_apply_once() {
    let (pool, factory) = fixture(|_| {});
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let barrier = barrier.clone();
            let pool = &pool;
            scope.spawn(move || {
                barrier.wait();
                pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), effect).unwrap();
            });
        }
    });
    assert_eq!(count(&factory), 1);
}

#[test]
fn pg_pool_saturation_and_deadline_keep_capacity() {
    let (pool, _) = fixture(|bounds| bounds.max_connections = 1);
    let lease = pool.acquire(&PgCancellation::default()).unwrap();
    let start = Instant::now();
    assert!(matches!(pool.acquire(&PgCancellation::default()), Err(StorageError::Busy)));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(pool.accounted(), 1);
    drop(lease);
    assert_eq!(pool.accounted(), 0);
}

struct BlockingFactory { started: mpsc::SyncSender<()>, release: std::sync::Mutex<mpsc::Receiver<()>>, calls: AtomicUsize }
impl PgConnectionFactory for BlockingFactory {
    fn connect(&self, _: Instant, _: &PgCancellation) -> Result<PgConnection, StorageError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.send(()).unwrap();
        self.release.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
        Err(StorageError::Unavailable)
    }
}

#[test]
fn pg_blocking_factory_retains_reserved_slot_after_acquire_timeout() {
    let (started_send, started_receive) = mpsc::sync_channel(1);
    let (release_send, release_receive) = mpsc::sync_channel(1);
    let factory = Arc::new(BlockingFactory { started: started_send, release: std::sync::Mutex::new(release_receive), calls: AtomicUsize::new(0) });
    let pool = PgPool::new(factory.clone(), PoolLimits { max_connections: 1, ..limits() }).unwrap();
    assert!(matches!(pool.acquire(&PgCancellation::default()), Err(StorageError::Busy)));
    started_receive.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(pool.accounted(), 1);
    assert!(matches!(pool.acquire(&PgCancellation::default()), Err(StorageError::Busy)));
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    release_send.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while pool.accounted() != 0 && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(5)); }
    assert_eq!(pool.accounted(), 0);
}

#[test]
fn pg_statement_timeout_discards_connection_and_effect() {
    let (pool, factory) = fixture(|bounds| bounds.statement_timeout = Duration::from_millis(30));
    let result = pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |tx| {
        effect(tx)?;
        tx.driver.batch_execute("SELECT pg_sleep(1)").map_err(super::sqlstate::error)?;
        Ok(json!(1))
    });
    assert_eq!(result, Err(StorageError::Busy));
    assert_eq!(count(&factory), 0);
    assert_eq!(pool.accounted(), 0);
}

#[test]
fn pg_cancellation_before_commit_rolls_back_and_holds_lease() {
    let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
    let control = PgCancellation::default();
    let (started_send, started_receive) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| pool.apply_operation(&ns("a"), &identity(), &control, |tx| {
            effect(tx)?;
            started_send.send(()).unwrap();
            tx.driver.batch_execute("SELECT pg_sleep(2)").map_err(super::sqlstate::error)?;
            Ok(json!(1))
        }));
        started_receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(pool.accounted(), 1);
        control.cancel();
        assert_eq!(worker.join().unwrap(), Err(StorageError::Busy));
    });
    assert_eq!(count(&factory), 0);
    assert_eq!(pool.accounted(), 0);
}

struct SlowCancelFactory {
    real: Arc<Factory>,
    started: mpsc::SyncSender<()>,
    release: Arc<std::sync::Mutex<mpsc::Receiver<()>>>,
}
impl PgConnectionFactory for SlowCancelFactory {
    fn connect(&self, _: Instant, _: &PgCancellation) -> Result<PgConnection, StorageError> {
        let started = self.started.clone();
        let release = self.release.clone();
        Ok(PgConnection::new(self.real.client(), move |token| {
            started.send(()).unwrap();
            release.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
            token.cancel_query(::postgres::NoTls).map_err(|_| StorageError::Unavailable)
        }))
    }
}

#[test]
fn pg_cancel_helper_remains_accounted_after_query_returns() {
    let (_, real) = fixture(|_| {});
    let (cancel_send, cancel_receive) = mpsc::sync_channel(1);
    let (release_send, release_receive) = mpsc::sync_channel(1);
    let factory = Arc::new(SlowCancelFactory { real: real.clone(), started: cancel_send,
        release: Arc::new(std::sync::Mutex::new(release_receive)) });
    let pool = PgPool::open(factory, PoolLimits { max_connections: 1, ..limits() }).unwrap();
    let control = PgCancellation::default();
    let (started_send, started_receive) = mpsc::sync_channel(1);
    let (query_done_send, query_done_receive) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| pool.apply_operation(&ns("a"), &identity(), &control, |tx| {
            effect(tx)?;
            started_send.send(()).unwrap();
            tx.driver.batch_execute("SELECT pg_sleep(0.2)").map_err(super::sqlstate::error)?;
            query_done_send.send(()).unwrap();
            Ok(json!(1))
        }));
        started_receive.recv_timeout(Duration::from_secs(2)).unwrap();
        control.cancel();
        cancel_receive.recv_timeout(Duration::from_secs(2)).unwrap();
        query_done_receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(pool.accounted(), 1);
        assert!(matches!(pool.acquire(&PgCancellation::default()), Err(StorageError::Busy)));
        release_send.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Err(StorageError::Busy));
    });
    assert_eq!(pool.accounted(), 0);
    assert_eq!(count(&real), 0);
}

#[test]
fn pg_transaction_deadline_and_precancel_refuse_effects() {
    let (pool, factory) = fixture(|bounds| bounds.txn_deadline = Duration::from_millis(50));
    let control = PgCancellation::default();
    control.cancel();
    assert_eq!(pool.apply_operation(&ns("a"), &identity(), &control, effect), Err(StorageError::Busy));
    let result = pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |tx| {
        tx.driver.batch_execute("SELECT pg_sleep(1)").map_err(super::sqlstate::error)?;
        effect(tx)
    });
    assert_eq!(result, Err(StorageError::Busy));
    assert_eq!(count(&factory), 0);
}

#[test]
fn pg_pool_context_cleared_after_success_and_error() {
    let (pool, _) = fixture(|bounds| bounds.max_connections = 1);
    let control = PgCancellation::default();
    pool.transaction(&ns("secret-a"), TxnKind::Write, &control, |_| Ok(())).unwrap();
    {
        let mut lease = pool.acquire(&control).unwrap();
        lease.check_clean().unwrap();
    }
    assert_eq!(pool.transaction::<()>(&ns("secret-b"), TxnKind::Write, &control, |_| Err(StorageError::Conflict)), Err(StorageError::Conflict));
    assert_eq!(pool.accounted(), 0, "error connections discarded");
    let mut lease = pool.acquire(&control).unwrap();
    lease.check_clean().unwrap();
    drop(lease);
    let mut lease = pool.acquire(&control).unwrap();
    lease.connection.as_mut().unwrap().client.batch_execute("BEGIN; SELECT set_config('semon.namespace', 'leaked', true)").unwrap();
    assert_eq!(lease.check_clean(), Err(StorageError::NamespaceMismatch));
    drop(lease);
    assert_eq!(pool.accounted(), 0);
    let mut lease = pool.acquire(&control).unwrap();
    lease.connection.as_mut().unwrap().client.batch_execute("BEGIN").unwrap();
    assert_eq!(lease.check_clean(), Err(StorageError::Unavailable), "DISCARD ALL must reject an open transaction");
}

#[test]
fn pg_open_refuses_missing_or_mismatched_schema() {
    let (pool, factory) = fixture(|_| {});
    factory.client().batch_execute("UPDATE semon_pg_schema SET version = 99").unwrap();
    assert!(matches!(PgPool::open(factory.clone(), limits()), Err(StorageError::ScopeChanged)));
    assert_eq!(pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()), Err(StorageError::ScopeChanged));
    let missing = Arc::new(Factory { dsn: factory.dsn.clone(), schema: "missing_f1_schema".into() });
    assert!(matches!(PgPool::open(missing, limits()), Err(StorageError::Unavailable)));
}

#[test]
fn pg_sqlstate_policy_is_phase_sensitive() {
    use super::sqlstate::{classify, Failure};
    for code in ["40001", "40P01", "55P03", "57014"] {
        assert_eq!(classify(Some(code), false, false), Failure::Storage(StorageError::Busy));
    }
    assert_eq!(classify(Some("23505"), false, true), Failure::Reconcile);
    for code in [Some("08006"), Some("08003"), Some("57P01"), Some("57P02"), Some("57P03"), Some("42501"), None] {
        assert_eq!(classify(code, false, false), Failure::Storage(StorageError::Unavailable));
        assert_eq!(classify(code, true, false), Failure::Storage(StorageError::CommitUncertain));
    }
    assert_eq!(classify(Some("40001"), true, false), Failure::Storage(StorageError::CommitUncertain));
}
