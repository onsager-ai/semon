use super::txn::TxnKind;
use super::*;
use crate::storage::{LogicalSourceNamespace, StorageError};
use serde_json::{Value, json};
use std::cell::Cell;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

thread_local! { static LOSE_ACK: Cell<bool> = const { Cell::new(false) }; }
thread_local! { static FAIL_CLEAN: Cell<bool> = const { Cell::new(false) }; }
pub(super) fn lose_ack() -> bool {
    LOSE_ACK.with(|flag| flag.replace(false))
}
pub(super) fn fail_clean() -> bool {
    FAIL_CLEAN.with(|flag| flag.replace(false))
}
static NEXT_SCHEMA: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Factory {
    pub(super) dsn: String,
    pub(super) schema: String,
}
impl Factory {
    pub(super) fn client(&self) -> ::postgres::Client {
        let mut config: ::postgres::Config = self.dsn.parse().expect("valid test DSN");
        config.options(&format!("-c search_path={}", self.schema));
        config.connect_timeout(Duration::from_secs(2));
        config
            .connect(::postgres::NoTls)
            .expect("real ephemeral PostgreSQL")
    }
}
impl PgConnectionFactory for Factory {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            let mut config: tokio_postgres::Config = self.dsn.parse().unwrap();
            config.options(format!("-c search_path={} -c idle_session_timeout=10000 -c idle_in_transaction_session_timeout=10000 -c statement_timeout=10000", self.schema));
            config.connect_timeout(Duration::from_secs(2));
            PgConnection::connect(config, tokio_postgres::NoTls).await
        })
    }
}

pub(super) fn limits() -> PoolLimits {
    PoolLimits {
        max_connections: 2,
        acquire_timeout: Duration::from_millis(250),
        statement_timeout: Duration::from_secs(2),
        lock_timeout: Duration::from_secs(1),
        idle_in_tx_timeout: Duration::from_secs(2),
        txn_deadline: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(60),
        max_lifetime: Duration::from_secs(3600),
        cancel_timeout: Duration::from_millis(250),
        cancel_grace: Duration::from_millis(250),
    }
}

pub(super) fn fixture(mut configure: impl FnMut(&mut PoolLimits)) -> (PgPool, Arc<Factory>) {
    let dsn = std::env::var("SEMON_PG_TEST_DSN")
        .expect("SEMON_PG_TEST_DSN required: PG tests may not silently skip");
    let schema = format!(
        "f1_{}_{}",
        std::process::id(),
        NEXT_SCHEMA.fetch_add(1, Ordering::SeqCst)
    );
    let factory = Arc::new(Factory { dsn, schema });
    let mut client = factory.client();
    client
        .batch_execute(&format!("CREATE SCHEMA {}", factory.schema))
        .unwrap();
    let row = client
        .query_one(
            "SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user",
            &[],
        )
        .unwrap();
    assert!(!row.get::<_, bool>(0));
    assert!(!row.get::<_, bool>(1));
    let mut bounds = limits();
    configure(&mut bounds);
    // Short operation deadlines qualify the operation under test, not unrelated
    // schema DDL on a contended runner. Provision with the ordinary bounds.
    drop(PgPool::create_schema(factory.clone(), limits()).unwrap());
    let pool = PgPool::new(factory.clone(), bounds).unwrap();
    factory
        .client()
        .batch_execute("CREATE TABLE effects(namespace bytea NOT NULL, value integer NOT NULL)")
        .unwrap();
    (pool, factory)
}

pub(super) fn ns(value: &str) -> LogicalSourceNamespace {
    LogicalSourceNamespace::new(value).unwrap()
}
pub(super) fn identity() -> OperationIdentity<'static> {
    OperationIdentity {
        operation_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        kind: "test-effect",
        request_digest: &[7; 32],
    }
}
pub(super) fn effect(tx: &mut super::txn::PgTxn<'_>) -> Result<Value, StorageError> {
    tx.driver
        .execute(
            "INSERT INTO effects VALUES ($1, 1)",
            &[&tx.namespace.as_str().as_bytes()],
        )
        .map_err(super::sqlstate::error)?;
    Ok(json!({"receipt": 1}))
}
pub(super) fn count(factory: &Factory) -> i64 {
    factory
        .client()
        .query_one("SELECT count(*) FROM effects", &[])
        .unwrap()
        .get(0)
}

#[test]
fn pg_operation_replay_namespace_rls_and_restricted_role() {
    let (pool, factory) = fixture(|_| {});
    let control = PgCancellation::default();
    let outcome = pool
        .apply_operation(&ns("a"), &identity(), &control, effect)
        .unwrap();
    assert_eq!(
        pool.apply_operation(&ns("a"), &identity(), &control, |_| panic!("must replay")),
        Ok(outcome.clone())
    );
    assert_eq!(
        pool.reconcile(&ns("a"), &identity(), &control),
        Ok(Reconciliation::Committed(outcome))
    );
    assert_eq!(
        pool.reconcile(&ns("b"), &identity(), &control),
        Ok(Reconciliation::Absent)
    );
    pool.apply_operation(&ns("b"), &identity(), &control, effect)
        .unwrap();
    assert_eq!(count(&factory), 2);
    let mismatch = OperationIdentity {
        kind: "other",
        ..identity()
    };
    assert_eq!(
        pool.reconcile(&ns("a"), &mismatch, &control),
        Err(StorageError::InvalidArguments)
    );
    let mismatch = OperationIdentity {
        request_digest: &[8; 32],
        ..identity()
    };
    assert_eq!(
        pool.reconcile(&ns("a"), &mismatch, &control),
        Err(StorageError::InvalidArguments)
    );
    // Forced RLS applies even to the restricted table-owning role. Outside a
    // namespaced transaction no operation from either source is visible.
    assert_eq!(
        factory
            .client()
            .query_one("SELECT count(*) FROM semon_pg_operations", &[])
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    pool.transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(
            tx.driver
                .query_one("SELECT count(*) FROM semon_pg_operations", &[])
                .unwrap()
                .get::<_, i64>(0),
            1
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn pg_commit_ack_loss_reconciles_exactly_one_effect() {
    let (pool, factory) = fixture(|_| {});
    LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(
        pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), effect),
        Err(StorageError::CommitUncertain)
    );
    assert_eq!(pool.accounted(), 0, "uncertain connection discarded");
    assert!(matches!(
        pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()),
        Ok(Reconciliation::Committed(_))
    ));
    pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |_| {
        panic!("must not re-execute")
    })
    .unwrap();
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
                pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), effect)
                    .unwrap();
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
    assert!(matches!(
        pool.acquire(&PgCancellation::default()),
        Err(StorageError::Busy)
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(pool.accounted(), 1);
    drop(lease);
    assert_eq!(pool.accounted(), 0);
}

struct ConnectEnded(Arc<AtomicUsize>);
impl Drop for ConnectEnded {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct BlockingFactory {
    started: mpsc::SyncSender<()>,
    calls: AtomicUsize,
    ended: Arc<AtomicUsize>,
}
impl PgConnectionFactory for BlockingFactory {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            let _ended = ConnectEnded(self.ended.clone());
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.send(()).unwrap();
            std::future::pending().await
        })
    }
}
#[test]
fn pg_pending_factory_is_dropped_before_reserved_slot_release() {
    let (send, receive) = mpsc::sync_channel(2);
    let factory = Arc::new(BlockingFactory {
        started: send,
        calls: AtomicUsize::new(0),
        ended: Arc::new(AtomicUsize::new(0)),
    });
    let pool = PgPool::new(
        factory.clone(),
        PoolLimits {
            max_connections: 1,
            ..limits()
        },
    )
    .unwrap();
    let start = Instant::now();
    assert!(matches!(
        pool.acquire(&PgCancellation::default()),
        Err(StorageError::Busy)
    ));
    receive.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(
        pool.accounted(),
        0,
        "connect future ended before capacity release"
    );
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        factory.ended.load(Ordering::SeqCst),
        1,
        "actual future drop preceded slot release"
    );
}

#[test]
fn pg_statement_timeout_discards_connection_and_effect() {
    let (pool, factory) = fixture(|bounds| bounds.statement_timeout = Duration::from_millis(30));
    let result = pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |tx| {
        effect(tx)?;
        tx.driver
            .batch_execute("SELECT pg_sleep(1)")
            .map_err(super::sqlstate::error)?;
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
        let worker = scope.spawn(|| {
            pool.apply_operation(&ns("a"), &identity(), &control, |tx| {
                effect(tx)?;
                started_send.send(()).unwrap();
                tx.driver
                    .batch_execute("SELECT pg_sleep(2)")
                    .map_err(super::sqlstate::error)?;
                Ok(json!(1))
            })
        });
        started_receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
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
    release: Arc<tokio::sync::Notify>,
}
impl PgConnectionFactory for SlowCancelFactory {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            let mut connection = self.real.connect().await?;
            let request = connection.client.cancel.clone();
            let started = self.started.clone();
            let release = self.release.clone();
            connection.client.cancel = Arc::new(move || {
                let request = request.clone();
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    started.send(()).unwrap();
                    release.notified().await;
                    request().await
                })
            });
            Ok(connection)
        })
    }
}
#[test]
fn pg_cancel_helper_remains_accounted_after_query_returns() {
    let (_, real) = fixture(|_| {});
    let (cancel_send, cancel_receive) = mpsc::sync_channel(1);
    let release = Arc::new(tokio::sync::Notify::new());
    let factory = Arc::new(SlowCancelFactory {
        real: real.clone(),
        started: cancel_send,
        release: release.clone(),
    });
    let pool = PgPool::open(
        factory,
        PoolLimits {
            max_connections: 1,
            cancel_timeout: Duration::from_secs(2),
            ..limits()
        },
    )
    .unwrap();
    let control = PgCancellation::default();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            pool.apply_operation(&ns("a"), &identity(), &control, |tx| {
                effect(tx)?;
                let pid: i32 = tx
                    .driver
                    .query_one("SELECT pg_backend_pid()", &[])
                    .unwrap()
                    .get(0);
                send.send(pid).unwrap();
                tx.driver
                    .batch_execute("SELECT pg_sleep(0.2)")
                    .map_err(super::sqlstate::error)?;
                Ok(json!(1))
            })
        });
        let pid = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        control.cancel();
        cancel_receive.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut direct = real.client();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if direct.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND state='idle in transaction' AND query='SELECT pg_sleep(0.2)')", &[&pid]).unwrap().get::<_, bool>(0) { break; }
            assert!(
                Instant::now() < deadline,
                "query did not finish before cancel helper"
            );
            std::thread::yield_now();
        }
        // The query has finished while cancellation is still pending. The inline
        // cancel future continues owning the lease until completed/dropped.
        assert_eq!(pool.accounted(), 1);
        assert!(matches!(
            pool.acquire(&PgCancellation::default()),
            Err(StorageError::Busy)
        ));
        assert_eq!(pool.accounted(), 1);
        release.notify_one();
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
    assert_eq!(
        pool.apply_operation(&ns("a"), &identity(), &control, effect),
        Err(StorageError::Busy)
    );
    let result = pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |tx| {
        tx.driver
            .batch_execute("SELECT pg_sleep(1)")
            .map_err(super::sqlstate::error)?;
        effect(tx)
    });
    assert_eq!(result, Err(StorageError::Busy));
    assert_eq!(count(&factory), 0);
}

#[test]
fn pg_pool_context_cleared_after_success_and_error() {
    let (pool, _) = fixture(|bounds| bounds.max_connections = 1);
    let control = PgCancellation::default();
    pool.transaction(&ns("secret-a"), TxnKind::Write, &control, |_| Ok(()))
        .unwrap();
    {
        let mut lease = pool.acquire(&control).unwrap();
        lease.check_clean().unwrap();
    }
    assert_eq!(
        pool.transaction::<()>(&ns("secret-b"), TxnKind::Write, &control, |_| Err(
            StorageError::Conflict
        )),
        Err(StorageError::Conflict)
    );
    assert_eq!(pool.accounted(), 0, "error connections discarded");
    let mut lease = pool.acquire(&control).unwrap();
    lease.check_clean().unwrap();
    drop(lease);
    let mut lease = pool.acquire(&control).unwrap();
    lease
        .connection
        .as_mut()
        .unwrap()
        .client
        .batch_execute("BEGIN; SELECT set_config('semon.namespace', 'leaked', true)")
        .unwrap();
    assert_eq!(lease.check_clean(), Err(StorageError::NamespaceMismatch));
    drop(lease);
    assert_eq!(pool.accounted(), 0);
    let mut lease = pool.acquire(&control).unwrap();
    lease
        .connection
        .as_mut()
        .unwrap()
        .client
        .batch_execute("BEGIN")
        .unwrap();
    assert_eq!(
        lease.check_clean(),
        Err(StorageError::Unavailable),
        "DISCARD ALL must reject an open transaction"
    );
}

#[test]
fn pg_open_refuses_missing_or_mismatched_schema() {
    let (pool, factory) = fixture(|_| {});
    factory
        .client()
        .batch_execute("UPDATE semon_pg_schema SET version = 99")
        .unwrap();
    assert!(matches!(
        PgPool::open(factory.clone(), limits()),
        Err(StorageError::ScopeChanged)
    ));
    assert_eq!(
        pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()),
        Err(StorageError::ScopeChanged)
    );
    let missing = Arc::new(Factory {
        dsn: factory.dsn.clone(),
        schema: "missing_f1_schema".into(),
    });
    assert!(matches!(
        PgPool::open(missing, limits()),
        Err(StorageError::Unavailable)
    ));
}

#[test]
fn pg_sqlstate_policy_is_phase_sensitive() {
    use super::sqlstate::classify;
    for code in ["40001", "40P01", "55P03", "57014"] {
        assert_eq!(classify(Some(code), false), StorageError::Busy);
    }
    assert_eq!(classify(Some("23505"), false), StorageError::Unavailable);
    for code in [
        Some("08006"),
        Some("08003"),
        Some("57P01"),
        Some("57P02"),
        Some("57P03"),
        Some("42501"),
        None,
    ] {
        assert_eq!(classify(code, false), StorageError::Unavailable);
        assert_eq!(classify(code, true), StorageError::CommitUncertain);
    }
    assert_eq!(classify(Some("40001"), true), StorageError::CommitUncertain);
}

#[test]
fn pg_full_namespace_bytes_roundtrip_and_rls_isolation() {
    let (pool, factory) = fixture(|_| {});
    let control = PgCancellation::default();
    let mut bytes = "a".repeat(4096);
    bytes.replace_range(2048..2049, "\0");
    let a = ns(&bytes);
    let mut other = bytes.clone();
    other.replace_range(4095..4096, "b");
    let b = ns(&other);
    assert_eq!(
        &a.as_str().as_bytes()[..1024],
        &b.as_str().as_bytes()[..1024]
    );
    for namespace in [&a, &b] {
        pool.apply_operation(namespace, &identity(), &control, effect)
            .unwrap();
        pool.transaction(namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let rows = tx.driver.query("SELECT namespace, namespace_digest = sha256(namespace) FROM semon_pg_operations", &[]).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get::<_, Vec<u8>>(0).as_slice(), namespace.as_str().as_bytes());
            assert!(rows[0].get::<_, bool>(1));
            Ok(())
        }).unwrap();
        assert_eq!(
            pool.reconcile(namespace, &identity(), &control),
            Ok(Reconciliation::Committed(json!({"receipt": 1})))
        );
    }
    assert_eq!(count(&factory), 2);
    assert_eq!(
        LogicalSourceNamespace::new("x".repeat(4097)),
        Err(StorageError::InvalidArguments)
    );
    assert_eq!(
        factory
            .client()
            .query_one("SELECT count(*) FROM semon_pg_operations", &[])
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

fn session_witness(tx: &mut super::txn::PgTxn<'_>, schema: &str) -> i32 {
    let row = tx.driver.query_one("SELECT pg_backend_pid(), current_setting('search_path'), (SELECT version FROM semon_pg_schema)", &[]).unwrap();
    assert_eq!(row.get::<_, String>(1), schema);
    assert_eq!(row.get::<_, i32>(2), super::schema::VERSION);
    row.get(0)
}

#[test]
fn pg_schema_queries_survive_discard_all_and_error_replacement() {
    let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
    let control = PgCancellation::default();
    let mut write_pid = 0;
    pool.apply_operation(&ns("a"), &identity(), &control, |tx| {
        write_pid = session_witness(tx, &factory.schema);
        effect(tx)
    })
    .unwrap();
    let read_pid = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            let pid = session_witness(tx, &factory.schema);
            assert_eq!(
                tx.driver
                    .query_one("SELECT count(*) FROM semon_pg_operations", &[])
                    .unwrap()
                    .get::<_, i64>(0),
                1
            );
            Ok(pid)
        })
        .unwrap();
    assert_eq!(
        write_pid, read_pid,
        "successful connection was reused after DISCARD ALL"
    );
    let mut error_pid = 0;
    assert_eq!(
        pool.transaction::<()>(&ns("a"), TxnKind::Write, &control, |tx| {
            error_pid = session_witness(tx, &factory.schema);
            Err(StorageError::Conflict)
        }),
        Err(StorageError::Conflict)
    );
    assert_eq!(error_pid, read_pid);
    assert_eq!(
        pool.accounted(),
        0,
        "error connection is conservatively discarded"
    );
    let replacement = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            Ok(session_witness(tx, &factory.schema))
        })
        .unwrap();
    assert_ne!(replacement, error_pid);
    let reused = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            Ok(session_witness(tx, &factory.schema))
        })
        .unwrap();
    assert_eq!(reused, replacement);
}

#[test]
fn pg_known_commit_ack_survives_cleanup_failure() {
    let (pool, factory) = fixture(|_| {});
    FAIL_CLEAN.with(|flag| flag.set(true));
    assert_eq!(
        pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), effect),
        Ok(json!({"receipt": 1}))
    );
    assert_eq!(
        pool.accounted(),
        0,
        "cleanup failure discarded the known committed connection"
    );
    assert_eq!(count(&factory), 1);
    pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |_| {
        panic!("known commit must replay")
    })
    .unwrap();
}

#[test]
fn pg_f1_schema_is_refused_without_upgrade_or_reset() {
    let (_, factory) = fixture(|_| {});
    factory
        .client()
        .batch_execute("UPDATE semon_pg_schema SET version = 1")
        .unwrap();
    assert!(matches!(
        PgPool::open(factory.clone(), limits()),
        Err(StorageError::ScopeChanged)
    ));
    assert_eq!(
        factory
            .client()
            .query_one("SELECT version FROM semon_pg_schema", &[])
            .unwrap()
            .get::<_, i32>(0),
        1
    );
}

#[test]
fn pg_digest_collision_refuses_without_alias_or_effect() {
    let (pool, factory) = fixture(|_| {});
    // No SHA-256 collision is manufactured. Test-only schema corruption creates
    // the same digest/full-bytes mismatch that the defensive RLS guard handles.
    factory
        .client()
        .batch_execute(
            "ALTER TABLE semon_pg_operations ALTER COLUMN namespace_digest DROP EXPRESSION",
        )
        .unwrap();
    pool.transaction(&ns("b"), TxnKind::Write, &PgCancellation::default(), |tx| {
        tx.driver.execute("INSERT INTO semon_pg_operations(namespace, namespace_digest, operation_id, kind, request_digest, outcome) VALUES ($1, sha256($2::bytea), $3, $4, $5, '1')",
            &[&b"b".as_slice(), &b"a".as_slice(), &identity().operation_id, &identity().kind, &&identity().request_digest[..]]).unwrap();
        Ok(())
    }).unwrap();
    assert_eq!(
        pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()),
        Err(StorageError::Unavailable)
    );
    assert!(matches!(
        pool.apply_operation(
            &ns("a"),
            &identity(),
            &PgCancellation::default(),
            |_| panic!("collision must not execute")
        ),
        Err(StorageError::Unavailable | StorageError::ScopeChanged)
    ));
    assert_eq!(count(&factory), 0);
}

fn deferred_commit_fixture() -> (PgPool, Arc<Factory>) {
    let (pool, factory) = fixture(|bounds| {
        bounds.max_connections = 1;
        bounds.statement_timeout = Duration::from_secs(10);
        bounds.txn_deadline = Duration::from_secs(15);
    });
    factory
        .client()
        .batch_execute(
            "
        CREATE TABLE commit_delays(value integer NOT NULL);
        CREATE FUNCTION commit_pause() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_sleep(5); RETURN NEW; END; $$;
        CREATE CONSTRAINT TRIGGER pause_during_commit AFTER INSERT ON commit_delays
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION commit_pause();
    ",
        )
        .unwrap();
    (pool, factory)
}

fn wait_for_commit_sleep(client: &mut ::postgres::Client, pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let row = client.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND state = 'active' AND query = 'COMMIT' AND wait_event = 'PgSleep')", &[&pid]).unwrap();
        if row.get::<_, bool>(0) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "backend did not enter the deferred COMMIT trigger"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn commit_phase_fault(terminate: bool) {
    let (pool, factory) = deferred_commit_fixture();
    let control = PgCancellation::default();
    let calls = AtomicUsize::new(0);
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            pool.apply_operation(&ns("a"), &identity(), &control, |tx| {
                calls.fetch_add(1, Ordering::SeqCst);
                let outcome = effect(tx)?;
                tx.driver
                    .execute("INSERT INTO commit_delays VALUES (1)", &[])
                    .map_err(super::sqlstate::error)?;
                let pid: i32 = tx
                    .driver
                    .query_one("SELECT pg_backend_pid()", &[])
                    .unwrap()
                    .get(0);
                send.send(pid).unwrap();
                Ok(outcome)
            })
        });
        let pid = receive.recv_timeout(Duration::from_secs(3)).unwrap();
        let mut controller = factory.client();
        wait_for_commit_sleep(&mut controller, pid);
        assert_eq!(pool.accounted(), 1, "COMMIT still owns its slot");
        if terminate {
            // The restricted role may terminate its own backend. No superuser
            // credentials or pg_signal_backend grant is needed in the fixture.
            assert!(
                controller
                    .query_one("SELECT pg_terminate_backend($1, 2000)", &[&pid])
                    .unwrap()
                    .get::<_, bool>(0)
            );
        } else {
            control.cancel();
        }
        assert_eq!(worker.join().unwrap(), Err(StorageError::CommitUncertain));
    });
    assert_eq!(
        pool.accounted(),
        0,
        "discard released the slot after driver completion"
    );
    // The interrupted deferred trigger prevented the server from committing.
    // Reconciliation observes the real rollback without executing the effect.
    assert_eq!(
        pool.reconcile(&ns("a"), &identity(), &PgCancellation::default()),
        Ok(Reconciliation::Absent)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(&factory), 0);
    assert_eq!(
        factory
            .client()
            .query_one("SELECT count(*) FROM commit_delays", &[])
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), effect)
        .unwrap();
    pool.apply_operation(&ns("a"), &identity(), &PgCancellation::default(), |_| {
        panic!("replay must not duplicate")
    })
    .unwrap();
    assert_eq!(count(&factory), 1);
}

#[test]
fn pg_backend_termination_during_commit_requires_reconciliation() {
    commit_phase_fault(true);
}

#[test]
fn pg_cancel_racing_commit_requires_reconciliation() {
    commit_phase_fault(false);
}

#[test]
fn pg_dead_idle_connection_is_validated_discarded_and_replaced() {
    let (pool, factory) = fixture(|bounds| {
        bounds.max_connections = 1;
        bounds.idle_timeout = Duration::from_millis(1);
    });
    let control = PgCancellation::default();
    let old_pid = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            Ok(session_witness(tx, &factory.schema))
        })
        .unwrap();
    assert_eq!(pool.accounted(), 1);
    assert!(
        factory
            .client()
            .query_one("SELECT pg_terminate_backend($1, 2000)", &[&old_pid])
            .unwrap()
            .get::<_, bool>(0)
    );
    std::thread::sleep(Duration::from_millis(5));
    let fresh_pid = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            Ok(session_witness(tx, &factory.schema))
        })
        .unwrap();
    assert_ne!(fresh_pid, old_pid);
    assert_eq!(pool.accounted(), 1);
    let lease = pool.acquire(&control).unwrap();
    drop(lease);
    assert_eq!(
        pool.accounted(),
        0,
        "no over-release after validation and replacement"
    );
}

#[test]
fn pg_connection_max_lifetime_retires_healthy_idle_client() {
    let (pool, factory) = fixture(|bounds| {
        bounds.max_connections = 1;
        bounds.max_lifetime = Duration::from_millis(5);
    });
    let control = PgCancellation::default();
    let old_pid = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            Ok(session_witness(tx, &factory.schema))
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    let fresh_pid = pool
        .transaction(&ns("a"), TxnKind::ReadSnapshot, &control, |tx| {
            Ok(session_witness(tx, &factory.schema))
        })
        .unwrap();
    assert_ne!(fresh_pid, old_pid);
    assert_eq!(pool.accounted(), 1);
}

#[cfg(feature = "postgres-test-tls")]
struct TlsFactory {
    dsn: String,
    schema: String,
    tls: postgres_native_tls::MakeTlsConnector,
}
#[cfg(feature = "postgres-test-tls")]
impl PgConnectionFactory for TlsFactory {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            let mut config: tokio_postgres::Config = self.dsn.parse().unwrap();
            config.options(format!("-c search_path={} -c idle_in_transaction_session_timeout=10000 -c idle_session_timeout=10000 -c statement_timeout=10000", self.schema));
            config.ssl_mode(tokio_postgres::config::SslMode::Require);
            if config.get_hostaddrs().is_empty() {
                config.hostaddr("127.0.0.1".parse().unwrap());
            }
            PgConnection::connect(config, self.tls.clone()).await
        })
    }
}
#[cfg(feature = "postgres-test-tls")]
fn test_tls(ca: &str) -> postgres_native_tls::MakeTlsConnector {
    let cert = native_tls::Certificate::from_pem(&std::fs::read(ca).unwrap()).unwrap();
    let tls = native_tls::TlsConnector::builder()
        .disable_built_in_roots(true)
        .add_root_certificate(cert)
        .build()
        .unwrap();
    postgres_native_tls::MakeTlsConnector::new(tls)
}
#[cfg(feature = "postgres-test-tls")]
#[test]
fn pg_tls_verified_connect_and_cancel_and_certificate_refusals() {
    let dsn = std::env::var("SEMON_PG_TLS_DSN").expect("TLS service mandatory");
    let ca = std::env::var("SEMON_PG_TLS_CA").expect("ephemeral CA mandatory");
    let wrong_ca = std::env::var("SEMON_PG_TLS_WRONG_CA").expect("wrong CA mandatory");
    let schema = format!(
        "tls_{}_{}",
        std::process::id(),
        NEXT_SCHEMA.fetch_add(1, Ordering::SeqCst)
    );
    let tls = test_tls(&ca);
    let mut config: ::postgres::Config = dsn.parse().unwrap();
    config.ssl_mode(::postgres::config::SslMode::Require);
    let mut admin = config.connect(tls.clone()).unwrap();
    assert!(
        admin
            .query_one(
                "SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()",
                &[]
            )
            .unwrap()
            .get::<_, bool>(0)
    );
    admin
        .batch_execute(&format!("CREATE SCHEMA {schema}"))
        .unwrap();
    let factory = Arc::new(TlsFactory {
        dsn: dsn.clone(),
        schema: schema.clone(),
        tls,
    });
    let pool = PgPool::create_schema(factory, limits()).unwrap();
    let control = PgCancellation::default();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| pool.transaction::<()>(&ns("tls"), TxnKind::Write, &control, |tx| {
            let row = tx.driver.query_one("SELECT pg_backend_pid(), ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()", &[]).unwrap();
            assert!(row.get::<_, bool>(1)); let pid: i32 = row.get(0);
            send.send(pid).unwrap();
            let error = tx.driver.batch_execute("SELECT pg_sleep(5)").expect_err("TLS cancel must reach server");
            assert!(matches!(&error, super::pool::DriverError::Pg(error) if error.code().is_some_and(|code| code.code() == "57014")), "a local timeout is not TLS cancel proof: {error:?}");
            Err(super::sqlstate::error(error))
        }));
        let pid = receive.recv_timeout(Duration::from_secs(3)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if admin.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event='PgSleep')", &[&pid]).unwrap().get::<_, bool>(0) { break; }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        control.cancel();
        // Busy is the real 57014 server response via the TLS cancel connection.
        assert_eq!(worker.join().unwrap(), Err(StorageError::Busy));
    });
    assert_eq!(pool.accounted(), 0);
    super::proxy_test::tls_cancel_handshake_freeze(&dsn, &schema, test_tls(&ca));
    for (bad_dsn, bad_tls) in [
        (dsn.clone(), test_tls(&wrong_ca)),
        (
            dsn.replace("host=localhost", "host=wrong.invalid hostaddr=127.0.0.1"),
            test_tls(&ca),
        ),
    ] {
        let factory = Arc::new(TlsFactory {
            dsn: bad_dsn,
            schema: schema.clone(),
            tls: bad_tls,
        });
        let pool = PgPool::new(factory, limits()).unwrap();
        assert!(matches!(
            pool.acquire(&PgCancellation::default()),
            Err(StorageError::Unavailable)
        ));
        assert_eq!(pool.accounted(), 0);
    }
}

#[test]
fn pg_unresolved_hostname_is_refused_without_unaccounted_dns() {
    struct Unresolved;
    impl PgConnectionFactory for Unresolved {
        fn connect(&self) -> ConnectFuture<'_> {
            Box::pin(async {
                let config = "host=unresolved.invalid user=test dbname=test"
                    .parse()
                    .unwrap();
                PgConnection::connect(config, tokio_postgres::NoTls).await
            })
        }
    }
    let pool = PgPool::new(Arc::new(Unresolved), limits()).unwrap();
    assert!(matches!(
        pool.acquire(&PgCancellation::default()),
        Err(StorageError::InvalidArguments)
    ));
    assert_eq!(pool.accounted(), 0);
}
