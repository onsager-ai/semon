//! Store interpretation authority is the provisioning database role, never a
//! Source namespace or a caller-supplied owner ID. All transactions acquire the
//! schema-scoped advisory lock BEFORE any SourceState/file/UsageSource lock.
//! Source work takes shared; the independently qualified host takes exclusive.
use super::{
    PgCancellation, PgPool, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{LogicalSourceNamespace, StorageError};
use sha2::{Digest, Sha256};

const TABLES: [(&str, &str, &str); 3] = [
    (
        "owner",
        "singleton boolean,owner name,contract integer,epoch bigint",
        "singleton",
    ),
    (
        "state",
        "singleton boolean,owner name,revision bigint,pricing_target bytea?",
        "singleton",
    ),
    (
        "operations",
        "operation_id bytea,owner name,request_digest bytea,revision bigint",
        "operation_id",
    ),
];

pub(super) fn install(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.driver.batch_execute("
        CREATE TABLE semon_pg_store_owner(singleton boolean PRIMARY KEY CHECK(singleton),owner name NOT NULL,contract integer NOT NULL CHECK(contract=1),epoch bigint NOT NULL CHECK(epoch=1));
        CREATE TABLE semon_pg_store_state(singleton boolean PRIMARY KEY CHECK(singleton),owner name NOT NULL,revision bigint NOT NULL CHECK(revision>=0),pricing_target bytea CHECK(octet_length(pricing_target)<=1048576));
        CREATE TABLE semon_pg_store_operations(operation_id bytea PRIMARY KEY CHECK(octet_length(operation_id)=32),owner name NOT NULL,request_digest bytea NOT NULL CHECK(octet_length(request_digest)=32),revision bigint NOT NULL CHECK(revision>=0));
        INSERT INTO semon_pg_store_owner VALUES(true,CURRENT_USER,1,1);
        INSERT INTO semon_pg_store_state VALUES(true,CURRENT_USER,0,NULL);
    ").map_err(sqlstate::error)?;
    tx.driver.batch_execute("ALTER TABLE semon_pg_native_meta ADD CONSTRAINT semon_pg_meta_source_owner CHECK(key<>convert_to('pricing_recompute_target','UTF8'))").map_err(sqlstate::error)?;
    for (table, _, _) in TABLES {
        let name = format!("semon_pg_store_{table}");
        tx.driver.batch_execute(&format!("ALTER TABLE {name} ENABLE ROW LEVEL SECURITY; ALTER TABLE {name} FORCE ROW LEVEL SECURITY; CREATE POLICY read_store ON {name} FOR SELECT USING(true);")).map_err(sqlstate::error)?;
        if table == "state" {
            tx.driver.batch_execute(&format!("CREATE POLICY write_store ON {name} FOR UPDATE USING(owner=CURRENT_USER) WITH CHECK(owner=CURRENT_USER);")).map_err(sqlstate::error)?;
        } else if table == "operations" {
            tx.driver.batch_execute(&format!("CREATE POLICY write_store ON {name} FOR INSERT WITH CHECK(owner=CURRENT_USER AND owner=(SELECT owner FROM semon_pg_store_owner WHERE singleton));")).map_err(sqlstate::error)?;
        }
    }
    Ok(())
}

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.check()?;
    let role = tx
        .driver
        .query_one(
            "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname=CURRENT_USER",
            &[],
        )
        .map_err(sqlstate::error)?;
    if role.get::<_, bool>(0) {
        return Err(StorageError::ScopeChanged);
    }
    let ownership: bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='semon_pg_native_meta'::regclass AND conname='semon_pg_meta_source_owner' AND contype='c' AND convalidated AND pg_get_expr(conbin,conrelid)=$1)", &[&"(key <> convert_to('pricing_recompute_target'::text, 'UTF8'::name))"]).map_err(sqlstate::error)?.get(0);
    if !ownership {
        return Err(StorageError::ScopeChanged);
    }
    for (table, columns, key) in TABLES {
        let name = format!("semon_pg_store_{table}");
        let rows = tx.driver.query("SELECT c.relrowsecurity AND c.relforcerowsecurity AND c.relkind='r' AND has_table_privilege(c.oid,'SELECT'),pg_get_userbyid(c.relowner)::text,
            ARRAY(SELECT a.attname::text||' '||format_type(a.atttypid,a.atttypmod)||CASE WHEN a.attnotnull THEN '' ELSE '?' END FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum),
            ARRAY(SELECT a.attname::text FROM pg_constraint k CROSS JOIN LATERAL unnest(k.conkey) WITH ORDINALITY x(num,ord) JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=x.num WHERE k.conrelid=c.oid AND k.contype='p' ORDER BY x.ord),
            ARRAY(SELECT p.polname::text||':'||p.polcmd::text||':'||p.polpermissive::text||':'||p.polroles::text||':'||coalesce(pg_get_expr(p.polqual,p.polrelid),'')||':'||coalesce(pg_get_expr(p.polwithcheck,p.polrelid),'') FROM pg_policy p WHERE p.polrelid=c.oid ORDER BY p.polname)
            FROM pg_class c WHERE c.relnamespace=current_schema()::regnamespace AND c.relname=$1", &[&name]).map_err(sqlstate::error)?;
        if rows.len() != 1 {
            return Err(StorageError::ScopeChanged);
        }
        let row = &rows[0];
        let mut policies = vec!["read_store:r:true:{0}:true:".to_owned()];
        if table == "state" {
            policies.push(
                "write_store:w:true:{0}:(owner = CURRENT_USER):(owner = CURRENT_USER)".into(),
            );
        }
        if table == "operations" {
            // Verify the policy structurally below as well as its exact role,
            // command and expression. The subquery cannot depend on Source GUCs.
            let expr = "((owner = CURRENT_USER) AND (owner = ( SELECT semon_pg_store_owner.owner\n   FROM semon_pg_store_owner\n  WHERE semon_pg_store_owner.singleton)))";
            policies.push(format!("write_store:a:true:{{0}}::{expr}"));
        }
        if !row.get::<_, bool>(0)
            || row.get::<_, Vec<String>>(2)
                != columns.split(',').map(str::to_owned).collect::<Vec<_>>()
            || row.get::<_, Vec<String>>(3) != vec![key.to_owned()]
            || row.get::<_, Vec<String>>(4) != policies
        {
            return Err(StorageError::ScopeChanged);
        }
        let checks: Vec<String> = tx.driver.query(
            "SELECT pg_get_expr(conbin,conrelid) FROM pg_constraint WHERE conrelid=$1::text::regclass AND contype='c' AND convalidated ORDER BY 1", &[&name]
        ).map_err(sqlstate::error)?.iter().map(|r| r.get(0)).collect();
        let mut expected = match table {
            "owner" => vec!["singleton", "(contract = 1)", "(epoch = 1)"],
            "state" => vec![
                "singleton",
                "(revision >= 0)",
                "(octet_length(pricing_target) <= 1048576)",
            ],
            _ => vec![
                "(octet_length(operation_id) = 32)",
                "(octet_length(request_digest) = 32)",
                "(revision >= 0)",
            ],
        };
        expected.sort_unstable();
        let shape: bool = tx.driver.query_one(
            "SELECT NOT EXISTS(SELECT 1 FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=$1::text::regclass AND a.attnum>0 AND NOT a.attisdropped AND (a.attidentity<>'' OR a.attgenerated<>'' OR d.oid IS NOT NULL)) AND NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid=$1::text::regclass AND (NOT convalidated OR contype NOT IN ('p','c'))) AND NOT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid=$1::text::regclass AND NOT tgisinternal) AND NOT EXISTS(SELECT 1 FROM pg_rewrite WHERE ev_class=$1::text::regclass)", &[&name]
        ).map_err(sqlstate::error)?.get(0);
        if checks != expected || !shape {
            return Err(StorageError::ScopeChanged);
        }
        let owners = tx
            .driver
            .query(
                "SELECT owner::text,contract,epoch FROM semon_pg_store_owner LIMIT 2",
                &[],
            )
            .map_err(sqlstate::error)?;
        if owners.len() != 1
            || owners[0].get::<_, String>(0) != row.get::<_, String>(1)
            || owners[0].get::<_, i32>(1) != 1
            || owners[0].get::<_, i64>(2) != 1
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

pub(super) fn lock_admission(tx: &mut PgTxn<'_>, exclusive: bool) -> Result<(), StorageError> {
    tx.check()?;
    // current_schema is the host-authorized search_path. Its OID only selects
    // a lock domain; it never grants ownership or store mutation authority.
    // This is session-scoped so it can be acquired before BEGIN. A repeatable-
    // read snapshot taken before waiting here could otherwise miss a DDL epoch
    // committed while the lock was held by another connection.
    let sql = if exclusive {
        "SELECT pg_advisory_lock(current_schema()::regnamespace::oid::int, 477110)"
    } else {
        "SELECT pg_advisory_lock_shared(current_schema()::regnamespace::oid::int, 477110)"
    };
    tx.driver.query_one(sql, &[]).map_err(sqlstate::error)?;
    tx.check()
}

pub(super) fn lock_state(tx: &mut PgTxn<'_>, exclusive: bool) -> Result<(), StorageError> {
    tx.check()?;
    // Real row locking also fences independently authorized SQL writers that
    // obey PostgreSQL row locks without using this adapter's advisory lock.
    // Read-only snapshots cannot take row locks; repeatable-read remains their
    // snapshot lifetime. Every publication/control transaction is writable.
    if exclusive {
        let rows = tx
            .driver
            .query(
                "SELECT revision FROM semon_pg_store_state WHERE singleton FOR UPDATE",
                &[],
            )
            .map_err(sqlstate::error)?;
        if rows.len() != 1 {
            return Err(StorageError::ScopeChanged);
        }
    }
    tx.check()
}

pub(super) fn lock_publication(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    // SELECT FOR SHARE requires UPDATE privilege in PostgreSQL. Source roles
    // deliberately have SELECT only on STORE tables. Their shared advisory
    // lock (acquired before this call) fences every authorized host mutation;
    // the exclusive host path also locks the state row. No definer function
    // or UPDATE grant is used to turn a Source into a STORE controller.
    read(tx)?;
    tx.check()
}

pub(super) fn read(tx: &mut PgTxn<'_>) -> Result<(i64, Option<String>), StorageError> {
    tx.check()?;
    let rows=tx.driver.query("SELECT s.revision,octet_length(s.pricing_target)::bigint,s.owner=o.owner AND o.contract=1 AND o.epoch=1 FROM semon_pg_store_state s CROSS JOIN semon_pg_store_owner o WHERE s.singleton AND o.singleton", &[]).map_err(sqlstate::error)?;
    if rows.len() != 1 || !rows[0].get::<_, bool>(2) || rows[0].get::<_, i64>(0) < 0 {
        return Err(StorageError::ScopeChanged);
    }
    let bad_operation: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_store_operations p CROSS JOIN semon_pg_store_owner o CROSS JOIN semon_pg_store_state s WHERE p.owner<>o.owner OR p.revision<1 OR p.revision>s.revision)", &[]).map_err(sqlstate::error)?.get(0);
    if bad_operation {
        return Err(StorageError::ScopeChanged);
    }
    let bytes = rows[0].get::<_, Option<i64>>(1);
    if bytes.is_some_and(|n| !(0..=1048576).contains(&n)) {
        return Err(StorageError::ScopeChanged);
    }
    crate::selected_read::charge(64, bytes.unwrap_or(0) as usize)
        .map_err(|_| StorageError::Busy)?;
    let value: Option<Vec<u8>> = tx
        .driver
        .query_one(
            "SELECT pricing_target FROM semon_pg_store_state WHERE singleton",
            &[],
        )
        .map_err(sqlstate::error)?
        .get(0);
    let value = value
        .map(String::from_utf8)
        .transpose()
        .map_err(|_| StorageError::ScopeChanged)?;
    Ok((rows[0].get(0), value))
}

/// Host control capability, bound to a qualified resource factory/pool. Opening
/// it reauthorizes the actual connection role; no identity string is accepted.
#[derive(Clone)]
pub struct PgStoreMetadata {
    pool: PgPool,
}
impl PgStoreMetadata {
    pub fn open(pool: PgPool, control: &PgCancellation) -> Result<Self, StorageError> {
        pool.transaction(
            &LogicalSourceNamespace::new("host-control")?,
            TxnKind::ReadSnapshot,
            control,
            authorize,
        )?;
        Ok(Self { pool })
    }
    /// Durable STORE operation identity; this is never a Source ingest ACK.
    pub fn set_pricing_target(
        &self,
        operation: &[u8; 32],
        expected_revision: i64,
        target: Option<&str>,
        control: &PgCancellation,
    ) -> Result<i64, StorageError> {
        if expected_revision < 0 || target.is_some_and(|v| v.len() > 1048576) {
            return Err(StorageError::InvalidArguments);
        }
        let request = serde_json::to_vec(&(expected_revision, target))
            .map_err(|_| StorageError::InvalidArguments)?;
        let digest = Sha256::digest(request).to_vec();
        self.pool.transaction(&LogicalSourceNamespace::new("host-control")?,TxnKind::StoreWrite,control,|tx| {
            authorize(tx)?;
            let prior=tx.driver.query("SELECT request_digest,revision FROM semon_pg_store_operations WHERE operation_id=$1", &[&&operation[..]]).map_err(sqlstate::error)?;
            if let Some(row)=prior.first() { return if row.get::<_,Vec<u8>>(0)==digest { Ok(row.get(1)) } else { Err(StorageError::InvalidArguments) }; }
            let (revision,_) = read(tx)?;
            if revision!=expected_revision { return Err(StorageError::Conflict); }
            let next=revision.checked_add(1).ok_or(StorageError::Conflict)?;
            mutation_boundary(1)?;
            if tx.driver.execute("UPDATE semon_pg_store_state SET revision=$1,pricing_target=$2 WHERE singleton AND revision=$3 AND owner=CURRENT_USER", &[&next,&target.map(str::as_bytes),&revision]).map_err(sqlstate::error)?!=1 { return Err(StorageError::ScopeChanged); }
            mutation_boundary(2)?;
            if tx.driver.execute("INSERT INTO semon_pg_store_operations(operation_id,owner,request_digest,revision) VALUES($1,CURRENT_USER,$2,$3)", &[&&operation[..],&digest,&next]).map_err(sqlstate::error)?!=1 { return Err(StorageError::ScopeChanged); }
            mutation_boundary(3)?;
            tx.check()?;
            Ok(next)
        })
    }
}
#[cfg(test)]
thread_local! { static FAIL_MUTATION: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
fn mutation_boundary(_point: u8) -> Result<(), StorageError> {
    #[cfg(test)]
    if FAIL_MUTATION.with(|p| p.get() == _point) {
        return Err(StorageError::Busy);
    }
    Ok(())
}
pub(super) fn authorize(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let allowed:bool=tx.driver.query_one("SELECT owner=CURRENT_USER AND has_table_privilege('semon_pg_store_state','UPDATE') AND has_table_privilege('semon_pg_store_operations','INSERT') FROM semon_pg_store_owner WHERE singleton", &[]).map_err(sqlstate::error)?.get(0);
    if !allowed {
        return Err(StorageError::ScopeChanged);
    }
    read(tx)?;
    Ok(())
}

#[cfg(test)]
#[path = "store_metadata/tests.rs"]
mod authority_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::tests::{LOSE_ACK, fixture, ns};

    #[test]
    fn pg_store_metadata_optional_owner_qualified_ack_replay_and_namespace_cannot_mint_owner() {
        let (pool, _) = fixture(|b| b.max_connections = 1);
        let control = PgCancellation::default();
        let host = PgStoreMetadata::open(pool.clone(), &control).unwrap();
        pool.transaction(&ns("source\0🦀"), TxnKind::ReadSnapshot, &control, |tx| {
            assert_eq!(read(tx)?, (0, None));
            Ok(())
        })
        .unwrap();
        LOSE_ACK.with(|p| p.set(true));
        assert_eq!(
            host.set_pricing_target(&[9; 32], 0, Some("p\0🦀"), &control),
            Err(StorageError::CommitUncertain)
        );
        assert_eq!(
            host.set_pricing_target(&[9; 32], 0, Some("p\0🦀"), &control),
            Ok(1)
        );
        assert_eq!(
            host.set_pricing_target(&[9; 32], 0, Some("different"), &control),
            Err(StorageError::InvalidArguments)
        );
        assert_eq!(
            host.set_pricing_target(&[8; 32], 0, None, &control),
            Err(StorageError::Conflict)
        );
        for source in ["host-control", "schema", "source\0🦀", "other"] {
            let result = pool.transaction(&ns(source), TxnKind::Write, &control, |tx| {
                tx.driver
                    .execute(
                        "INSERT INTO semon_pg_store_owner VALUES(true,CURRENT_USER,1,1)",
                        &[],
                    )
                    .map_err(sqlstate::error)?;
                Ok(())
            });
            assert!(result.is_err(), "GUC cannot mint store owner");
            pool.transaction(&ns(source), TxnKind::ReadSnapshot, &control, |tx| {
                assert_eq!(read(tx)?, (1, Some("p\0🦀".into())));
                Ok(())
            })
            .unwrap();
        }
        let cancelled = PgCancellation::default();
        cancelled.cancel();
        assert!(
            host.set_pricing_target(&[7; 32], 1, None, &cancelled)
                .is_err()
        );
        assert_eq!(host.set_pricing_target(&[7; 32], 1, None, &control), Ok(2));
    }

    #[test]
    fn pg_store_metadata_warm_contract_drift_refuses_before_source_effect() {
        for corrupt in [
            "ALTER TABLE semon_pg_store_owner NO FORCE ROW LEVEL SECURITY",
            "ALTER TABLE semon_pg_store_state ADD COLUMN extra boolean",
            "DROP POLICY write_store ON semon_pg_store_state",
        ] {
            let (pool, _factory) = fixture(|b| b.max_connections = 1);
            let control = PgCancellation::default();
            assert!(PgStoreMetadata::open(pool.clone(), &control).is_ok());
            pool.controlled_schema_change(&control, |tx| {
                tx.driver.batch_execute(corrupt).map_err(sqlstate::error)
            })
            .unwrap();
            let mut entered = false;
            assert!(
                pool.transaction(&ns("source"), TxnKind::Write, &control, |_| {
                    entered = true;
                    Ok(())
                })
                .is_err()
            );
            assert!(!entered);
        }
    }

    #[test]
    fn pg_store_metadata_pricing_waits_for_source_publication_lock() {
        let (pool, factory) = fixture(|_| {});
        let control = PgCancellation::default();
        let host = PgStoreMetadata::open(pool.clone(), &control).unwrap();
        let source_pool = PgPool::open(
            crate::storage::postgres::tests::source_factory(&factory),
            crate::storage::postgres::tests::limits(),
        )
        .unwrap();
        let (captured_tx, captured_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader_pool = &source_pool;
            let reader_control = &control;
            let reader = scope.spawn(move || {
                reader_pool.transaction(&ns("source"), TxnKind::Write, reader_control, |tx| {
                    assert_eq!(read(tx)?, (0, None));
                    captured_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    assert_eq!(read(tx)?, (0, None));
                    Ok(())
                })
            });
            captured_rx.recv().unwrap();
            let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
            let host_ref = &host;
            let writer_control = &control;
            let writer = scope.spawn(move || {
                let result = host_ref.set_pricing_target(&[1; 32], 0, Some("next"), writer_control);
                done_tx.send(()).unwrap();
                result
            });
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_millis(50))
                    .is_err()
            );
            release_tx.send(()).unwrap();
            reader.join().unwrap().unwrap();
            assert_eq!(writer.join().unwrap(), Ok(1));
        });
    }
}
