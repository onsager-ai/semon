use super::*;
use crate::storage::postgres::{
    PgSourceJournal,
    tests::{Factory, LOSE_ACK, admin_factory, fixture, limits, ns, source_factory},
};
use crate::storage::source_observations::conformance::shared_source_observation_conformance;
use std::time::Duration;

fn snapshot(factory: &Factory) -> Vec<String> {
    let mut client = factory.client();
    let mut result = Vec::new();
    for table in ["owner", "state", "operations"] {
        let rows = client.query(&format!("SELECT to_jsonb(t)::text FROM semon_pg_store_{table} t ORDER BY to_jsonb(t)::text"), &[]).unwrap();
        result.extend(rows.iter().map(|r| r.get::<_, String>(0)));
    }
    result
}

// Only the isolated ephemeral CI administrator observes byte equality across
// every namespace, including intentionally broken RLS/ACL fixtures. This
// connection is never injected into the library or used as an application role.
fn full_snapshot(factory: &Factory) -> Vec<(String, Vec<String>)> {
    let mut client = admin_factory(factory).client();
    let tables = client.query("SELECT relname FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relkind='r' ORDER BY relname", &[]).unwrap();
    tables
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            let rows = client
                .query(
                    &format!("SELECT to_jsonb(t)::text FROM {name} t ORDER BY to_jsonb(t)::text"),
                    &[],
                )
                .unwrap();
            (name, rows.iter().map(|r| r.get(0)).collect())
        })
        .collect()
}

fn wait_for_blocked_sql(factory: &Factory, blocker: i32) {
    let mut observer = admin_factory(factory).client();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let waiting: bool = observer
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
                &[&blocker],
            )
            .unwrap()
            .get(0);
        if waiting {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual SQL lock wait observed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn pg_store_metadata_distinct_source_principal_journal_cold_warm_cannot_control() {
    let (host_pool, factory) = fixture(|b| b.max_connections = 1);
    let control = PgCancellation::default();
    let host = PgStoreMetadata::open(host_pool, &control).unwrap();
    assert_eq!(
        host.set_pricing_target(&[31; 32], 0, Some("global\0🦀"), &control),
        Ok(1)
    );
    let source = source_factory(&factory);
    let forced: bool = source.client().query_one("SELECT bool_and(relrowsecurity AND relforcerowsecurity) FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relname LIKE 'semon_pg_%' AND relname<>'semon_pg_schema' AND relkind='r'", &[]).unwrap().get(0);
    assert!(forced);
    let before = snapshot(&factory);
    for cycle in 0..2 {
        let namespace = format!("source\0🦀-{cycle}");
        let pool = PgPool::open(source.clone(), limits()).unwrap();
        shared_source_observation_conformance(
            || {
                Box::new(PgSourceJournal {
                    pool: pool.clone(),
                    namespace: ns(&namespace),
                    control: control.clone(),
                })
            },
            &namespace,
        );
        assert!(PgStoreMetadata::open(pool.clone(), &control).is_err());
        let pricing_in_source = pool.transaction(&ns(&namespace), TxnKind::Write, &control, |tx| {
            tx.driver
                .execute(
                    "INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3)",
                    &[
                        &tx.namespace.as_str().as_bytes(),
                        &b"pricing_recompute_target".as_slice(),
                        &b"forged".as_slice(),
                    ],
                )
                .map_err(sqlstate::error)?;
            Ok(())
        });
        assert!(
            pricing_in_source.is_err(),
            "STORE key is illegal in Source metadata"
        );
        let operation_id = "1f".repeat(32);
        let source_operation = crate::storage::postgres::OperationIdentity {
            operation_id: &operation_id,
            kind: "store-pricing",
            request_digest: &[31; 32],
        };
        pool.apply_operation(&ns(&namespace), &source_operation, &control, |_| {
            Ok(serde_json::json!({"source":true}))
        })
        .unwrap();
        assert!(
            snapshot(&factory) == before,
            "Source receipt cannot masquerade as STORE receipt"
        );
        for name in ["host-control", "schema", "semon_f1", "source\0🦀", ""] {
            let logical = if name.is_empty() {
                ns("empty-guc")
            } else {
                ns(name)
            };
            pool.transaction(&logical, TxnKind::Write, &control, |tx| {
                assert_eq!(read(tx)?, (1, Some("global\0🦀".into())));
                Ok(())
            })
            .unwrap();
            // Direct SQL cannot turn caller-controlled GUCs or owner strings
            // into a control capability, even outside the typed API.
            let mut client = source.client();
            // Match the production GUC transport: PostgreSQL text cannot carry
            // the NUL in an opaque namespace. Empty attacker context stays empty.
            let context = if name.is_empty() {
                String::new()
            } else {
                crate::storage::postgres::namespace_context(&ns(name))
            };
            client
                .query_one("SELECT set_config('semon.namespace',$1,false)", &[&context])
                .unwrap();
            for sql in [
                "UPDATE semon_pg_store_state SET revision=revision+1",
                "INSERT INTO semon_pg_store_owner VALUES(true,CURRENT_USER,1,1)",
                "INSERT INTO semon_pg_store_operations VALUES(decode(repeat('ab',32),'hex'),CURRENT_USER,decode(repeat('cd',32),'hex'),2)",
            ] {
                assert!(
                    client.batch_execute(sql).is_err(),
                    "Source STORE mutation refused"
                );
            }
        }
        drop(pool);
    }
    assert!(
        snapshot(&factory) == before,
        "STORE bytes unchanged by Source"
    );
}

#[test]
fn pg_store_metadata_fault_boundaries_restart_replay_and_value_domain() {
    let (pool, factory) = fixture(|b| b.max_connections = 1);
    let control = PgCancellation::default();
    let host = PgStoreMetadata::open(pool.clone(), &control).unwrap();
    for point in 1..=3 {
        let before = full_snapshot(&factory);
        FAIL_MUTATION.with(|p| p.set(point));
        let result = host.set_pricing_target(&[point; 32], 0, Some("fault"), &control);
        FAIL_MUTATION.with(|p| p.set(0));
        assert_eq!(result, Err(StorageError::Busy));
        assert!(
            full_snapshot(&factory) == before,
            "state and receipt rollback"
        );
    }
    let too_large = "x".repeat(1048577);
    let before = full_snapshot(&factory);
    assert_eq!(
        host.set_pricing_target(&[10; 32], 0, Some(&too_large), &control),
        Err(StorageError::InvalidArguments)
    );
    assert!(full_snapshot(&factory) == before);
    let maximum = "🦀".repeat(1048576 / 4);
    for (index, target) in [Some(""), Some("\0🦀\n\""), Some(maximum.as_str()), None]
        .into_iter()
        .enumerate()
    {
        let revision = index as i64;
        let operation = [20 + index as u8; 32];
        LOSE_ACK.with(|p| p.set(true));
        assert_eq!(
            host.set_pricing_target(&operation, revision, target, &control),
            Err(StorageError::CommitUncertain)
        );
        let reopened = PgPool::open(factory.clone(), limits()).unwrap();
        let next_host = PgStoreMetadata::open(reopened.clone(), &control).unwrap();
        assert_eq!(
            next_host.set_pricing_target(&operation, revision, target, &control),
            Ok(revision + 1)
        );
        assert_eq!(
            next_host.set_pricing_target(&operation, revision, Some("changed"), &control),
            Err(StorageError::InvalidArguments)
        );
        reopened
            .transaction(&ns("foreign"), TxnKind::ReadSnapshot, &control, |tx| {
                let (actual_revision, actual) = read(tx)?;
                assert_eq!(actual_revision, revision + 1);
                assert!(actual.as_deref() == target, "optional UTF8 bytes preserved");
                Ok(())
            })
            .unwrap();
    }
    drop(host);
    drop(pool);
    let reopened = PgPool::open(factory, limits()).unwrap();
    assert_eq!(
        PgStoreMetadata::open(reopened, &control)
            .unwrap()
            .set_pricing_target(&[23; 32], 3, None, &control),
        Ok(4)
    );
}

#[test]
fn pg_store_metadata_warm_corruption_matrix_refuses_without_mutation() {
    for corrupt in [
        "ALTER TABLE semon_pg_store_owner NO FORCE ROW LEVEL SECURITY",
        "ALTER TABLE semon_pg_store_state DISABLE ROW LEVEL SECURITY",
        "DROP POLICY read_store ON semon_pg_store_owner",
        "ALTER POLICY read_store ON semon_pg_store_state USING(false)",
        "ALTER TABLE semon_pg_store_state ADD COLUMN extra boolean",
        "ALTER TABLE semon_pg_store_state ALTER COLUMN revision SET DEFAULT 0",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_contract_check",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_epoch_check",
        "ALTER TABLE semon_pg_store_state DROP CONSTRAINT semon_pg_store_state_pricing_target_check",
        "ALTER TABLE semon_pg_store_operations DROP CONSTRAINT semon_pg_store_operations_request_digest_check",
        "ALTER TABLE semon_pg_store_operations DROP CONSTRAINT semon_pg_store_operations_pkey",
        "ALTER TABLE semon_pg_store_state NO FORCE ROW LEVEL SECURITY; UPDATE semon_pg_store_state SET owner='semon_source'; ALTER TABLE semon_pg_store_state FORCE ROW LEVEL SECURITY",
        "ALTER TABLE semon_pg_store_owner NO FORCE ROW LEVEL SECURITY; DELETE FROM semon_pg_store_owner; ALTER TABLE semon_pg_store_owner FORCE ROW LEVEL SECURITY",
        "ALTER TABLE semon_pg_store_owner NO FORCE ROW LEVEL SECURITY; UPDATE semon_pg_store_owner SET owner='semon_source'; ALTER TABLE semon_pg_store_owner FORCE ROW LEVEL SECURITY",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_epoch_check; ALTER TABLE semon_pg_store_owner NO FORCE ROW LEVEL SECURITY; UPDATE semon_pg_store_owner SET epoch=2; ALTER TABLE semon_pg_store_owner FORCE ROW LEVEL SECURITY",
        "REVOKE SELECT ON semon_pg_store_state FROM semon_f1",
        "REVOKE UPDATE ON semon_pg_store_state FROM semon_f1",
        "REVOKE INSERT ON semon_pg_store_operations FROM semon_f1",
        "REVOKE SELECT ON semon_pg_store_owner FROM semon_f1",
        "ALTER TABLE semon_pg_store_state OWNER TO semon_source",
        "ALTER TABLE semon_pg_store_operations OWNER TO semon_source",
        "UPDATE semon_pg_store_state SET owner='semon_source'",
        "DELETE FROM semon_pg_store_state",
        "INSERT INTO semon_pg_store_operations VALUES(decode(repeat('de',32),'hex'),'semon_f1',decode(repeat('ad',32),'hex'),0)",
        "INSERT INTO semon_pg_store_operations VALUES(decode(repeat('de',32),'hex'),'semon_f1',decode(repeat('ad',32),'hex'),100)",
        "INSERT INTO semon_pg_store_operations VALUES(decode(repeat('de',32),'hex'),'semon_source',decode(repeat('ad',32),'hex'),0)",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_pkey; INSERT INTO semon_pg_store_owner VALUES(true,'semon_f1',1,1)",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_contract_check; UPDATE semon_pg_store_owner SET contract=2",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_epoch_check; UPDATE semon_pg_store_owner SET epoch=2",
        "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_contract_check; ALTER TABLE semon_pg_store_owner ADD CHECK(contract>0)",
    ] {
        let (pool, factory) = fixture(|b| b.max_connections = 1);
        let control = PgCancellation::default();
        let host = PgStoreMetadata::open(pool.clone(), &control).unwrap();
        let controlled = matches!(
            corrupt,
            "ALTER TABLE semon_pg_store_owner NO FORCE ROW LEVEL SECURITY"
                | "ALTER TABLE semon_pg_store_state DISABLE ROW LEVEL SECURITY"
                | "DROP POLICY read_store ON semon_pg_store_owner"
                | "ALTER POLICY read_store ON semon_pg_store_state USING(false)"
                | "ALTER TABLE semon_pg_store_state ADD COLUMN extra boolean"
                | "ALTER TABLE semon_pg_store_state ALTER COLUMN revision SET DEFAULT 0"
                | "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_contract_check"
                | "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_epoch_check"
                | "ALTER TABLE semon_pg_store_state DROP CONSTRAINT semon_pg_store_state_pricing_target_check"
                | "ALTER TABLE semon_pg_store_operations DROP CONSTRAINT semon_pg_store_operations_request_digest_check"
                | "ALTER TABLE semon_pg_store_operations DROP CONSTRAINT semon_pg_store_operations_pkey"
                | "ALTER TABLE semon_pg_store_owner DROP CONSTRAINT semon_pg_store_owner_contract_check; ALTER TABLE semon_pg_store_owner ADD CHECK(contract>0)"
        );
        if controlled {
            pool.controlled_schema_change(&control, |tx| {
                tx.driver.batch_execute(corrupt).map_err(sqlstate::error)
            })
            .unwrap();
        } else {
            admin_factory(&factory)
                .client()
                .batch_execute(corrupt)
                .unwrap();
        }
        let before = full_snapshot(&factory);
        assert!(
            host.set_pricing_target(&[40; 32], 0, Some("refused"), &control)
                .is_err(),
            "warm corruption refused"
        );
        assert!(
            before == full_snapshot(&factory),
            "all namespace/store bytes unchanged after refusal"
        );
    }
}

#[test]
fn pg_store_metadata_competing_hosts_exact_revision_one_winner() {
    let (pool, factory) = fixture(|_| {});
    let control = PgCancellation::default();
    let a = PgStoreMetadata::open(pool, &control).unwrap();
    let b = PgStoreMetadata::open(PgPool::open(factory, limits()).unwrap(), &control).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            a.set_pricing_target(&[51; 32], 0, Some("a"), &control)
        });
        let second = scope.spawn(|| {
            barrier.wait();
            b.set_pricing_target(&[52; 32], 0, Some("b"), &control)
        });
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|r| **r == Ok(1)).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| **r == Err(StorageError::Conflict))
                .count(),
            1
        );
    });
}

#[test]
fn pg_store_metadata_held_lock_cancel_and_deadline_release_single_pool_slot() {
    let (pool, factory) = fixture(|b| {
        b.max_connections = 1;
        b.lock_timeout = Duration::from_secs(2);
    });
    let control = PgCancellation::default();
    let host = PgStoreMetadata::open(pool.clone(), &control).unwrap();
    let before = snapshot(&factory);
    let mut blocker = factory.client();
    blocker.batch_execute("BEGIN; SELECT pg_advisory_xact_lock_shared(current_schema()::regnamespace::oid::int,477110)").unwrap();
    let blocker_pid: i32 = blocker
        .query_one("SELECT pg_backend_pid()", &[])
        .unwrap()
        .get(0);
    let cancelled = PgCancellation::default();
    std::thread::scope(|scope| {
        let cancel = &cancelled;
        let writer = scope.spawn(|| host.set_pricing_target(&[61; 32], 0, Some("blocked"), cancel));
        wait_for_blocked_sql(&factory, blocker_pid);
        cancelled.cancel();
        assert!(writer.join().unwrap().is_err());
    });
    assert!(snapshot(&factory) == before);
    blocker.batch_execute("ROLLBACK").unwrap();
    assert_eq!(
        host.set_pricing_target(&[61; 32], 0, Some("after"), &control),
        Ok(1)
    );
    let mut short = limits();
    short.txn_deadline = Duration::from_millis(250);
    short.lock_timeout = Duration::from_secs(2);
    let short_pool = PgPool::new(factory.clone(), short).unwrap();
    let short_host = PgStoreMetadata::open(short_pool, &control).unwrap();
    blocker.batch_execute("BEGIN; SELECT pg_advisory_xact_lock_shared(current_schema()::regnamespace::oid::int,477110)").unwrap();
    assert!(
        short_host
            .set_pricing_target(&[62; 32], 1, None, &control)
            .is_err()
    );
    blocker.batch_execute("ROLLBACK").unwrap();
    assert_eq!(
        short_host.set_pricing_target(&[62; 32], 1, None, &control),
        Ok(2)
    );
}

#[test]
fn pg_store_metadata_held_row_lock_sql_cancel_rolls_back_and_reuses_pool() {
    let (pool, factory) = fixture(|b| b.max_connections = 1);
    let control = PgCancellation::default();
    let host = PgStoreMetadata::open(pool, &control).unwrap();
    let before = full_snapshot(&factory);
    let mut blocker = factory.client();
    blocker
        .batch_execute(
            "BEGIN; SELECT revision FROM semon_pg_store_state WHERE singleton FOR UPDATE",
        )
        .unwrap();
    let blocker_pid: i32 = blocker
        .query_one("SELECT pg_backend_pid()", &[])
        .unwrap()
        .get(0);
    let cancelled = PgCancellation::default();
    std::thread::scope(|scope| {
        let cancel = &cancelled;
        let writer =
            scope.spawn(|| host.set_pricing_target(&[71; 32], 0, Some("blocked-row"), cancel));
        wait_for_blocked_sql(&factory, blocker_pid);
        cancelled.cancel();
        assert!(writer.join().unwrap().is_err());
    });
    assert!(full_snapshot(&factory) == before);
    blocker.batch_execute("ROLLBACK").unwrap();
    assert_eq!(
        host.set_pricing_target(&[71; 32], 0, Some("after-row"), &control),
        Ok(1)
    );
}
