//! Store interpretation authority is the provisioning database role, never a
//! Source namespace or a caller-supplied owner ID. All transactions acquire the
//! schema-scoped advisory lock BEFORE any SourceState/file/UsageSource lock.
//! Source work takes shared; the independently qualified host takes exclusive.
use super::{PgCancellation, PgPool, sqlstate, txn::{PgTxn, TxnKind}};
use crate::storage::{LogicalSourceNamespace, StorageError};
use sha2::{Digest, Sha256};

const TABLES: [(&str, &str, &str); 3] = [
    ("owner", "singleton boolean,owner name,contract integer,epoch bigint", "singleton"),
    ("state", "singleton boolean,owner name,revision bigint,pricing_target bytea?", "singleton"),
    ("operations", "operation_id bytea,owner name,request_digest bytea,revision bigint", "operation_id"),
];

pub(super) fn install(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.driver.batch_execute("
        CREATE TABLE semon_pg_store_owner(singleton boolean PRIMARY KEY CHECK(singleton),owner name NOT NULL,contract integer NOT NULL CHECK(contract=1),epoch bigint NOT NULL CHECK(epoch=1));
        CREATE TABLE semon_pg_store_state(singleton boolean PRIMARY KEY CHECK(singleton),owner name NOT NULL,revision bigint NOT NULL CHECK(revision>=0),pricing_target bytea CHECK(octet_length(pricing_target)<=1048576));
        CREATE TABLE semon_pg_store_operations(operation_id bytea PRIMARY KEY CHECK(octet_length(operation_id)=32),owner name NOT NULL,request_digest bytea NOT NULL CHECK(octet_length(request_digest)=32),revision bigint NOT NULL CHECK(revision>=0));
        INSERT INTO semon_pg_store_owner VALUES(true,CURRENT_USER,1,1);
        INSERT INTO semon_pg_store_state VALUES(true,CURRENT_USER,0,NULL);
    ").map_err(sqlstate::error)?;
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
    let role = tx.driver.query_one("SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname=CURRENT_USER", &[]).map_err(sqlstate::error)?;
    if role.get::<_, bool>(0) { return Err(StorageError::ScopeChanged); }
    for (table, columns, key) in TABLES {
        let name = format!("semon_pg_store_{table}");
        let rows = tx.driver.query("SELECT c.relrowsecurity AND c.relforcerowsecurity AND c.relkind='r' AND has_table_privilege(c.oid,'SELECT'),pg_get_userbyid(c.relowner)::text,
            ARRAY(SELECT a.attname::text||' '||format_type(a.atttypid,a.atttypmod)||CASE WHEN a.attnotnull THEN '' ELSE '?' END FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum),
            ARRAY(SELECT a.attname::text FROM pg_constraint k CROSS JOIN LATERAL unnest(k.conkey) WITH ORDINALITY x(num,ord) JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=x.num WHERE k.conrelid=c.oid AND k.contype='p' ORDER BY x.ord),
            ARRAY(SELECT p.polname::text||':'||p.polcmd::text||':'||p.polpermissive::text||':'||p.polroles::text||':'||coalesce(pg_get_expr(p.polqual,p.polrelid),'')||':'||coalesce(pg_get_expr(p.polwithcheck,p.polrelid),'') FROM pg_policy p WHERE p.polrelid=c.oid ORDER BY p.polname)
            FROM pg_class c WHERE c.relnamespace=current_schema()::regnamespace AND c.relname=$1", &[&name]).map_err(sqlstate::error)?;
        if rows.len()!=1 { return Err(StorageError::ScopeChanged); }
        let row=&rows[0];
        let mut policies=vec!["read_store:r:true:{0}:true:".to_owned()];
        if table=="state" { policies.push("write_store:w:true:{0}:(owner = CURRENT_USER):(owner = CURRENT_USER)".into()); }
        if table=="operations" {
            // Verify the policy structurally below as well as its exact role,
            // command and expression. The subquery cannot depend on Source GUCs.
            let expr = "((owner = CURRENT_USER) AND (owner = ( SELECT semon_pg_store_owner.owner\n   FROM semon_pg_store_owner\n  WHERE semon_pg_store_owner.singleton)))";
            policies.push(format!("write_store:a:true:{{0}}::{expr}"));
        }
        if !row.get::<_,bool>(0) || row.get::<_,Vec<String>>(2)!=columns.split(',').map(str::to_owned).collect::<Vec<_>>() || row.get::<_,Vec<String>>(3)!=vec![key.to_owned()] || row.get::<_,Vec<String>>(4)!=policies { return Err(StorageError::ScopeChanged); }
        let owners=tx.driver.query("SELECT owner::text,contract,epoch FROM semon_pg_store_owner WHERE singleton", &[]).map_err(sqlstate::error)?;
        if owners.len()!=1 || owners[0].get::<_,String>(0)!=row.get::<_,String>(1) || owners[0].get::<_,i32>(1)!=1 || owners[0].get::<_,i64>(2)!=1 { return Err(StorageError::ScopeChanged); }
    }
    Ok(())
}

pub(super) fn lock(tx: &mut PgTxn<'_>, exclusive: bool) -> Result<(), StorageError> {
    tx.check()?;
    // current_schema is the host-authorized search_path. Its OID only selects
    // a lock domain; it never grants ownership or store mutation authority.
    let sql=if exclusive { "SELECT pg_advisory_xact_lock(current_schema()::regnamespace::oid::int, 477110)" } else { "SELECT pg_advisory_xact_lock_shared(current_schema()::regnamespace::oid::int, 477110)" };
    tx.driver.query_one(sql,&[]).map_err(sqlstate::error)?;
    tx.check()
}

pub(super) fn read(tx: &mut PgTxn<'_>) -> Result<(i64, Option<String>), StorageError> {
    tx.check()?;
    let rows=tx.driver.query("SELECT s.revision,octet_length(s.pricing_target)::bigint,s.owner=o.owner AND o.contract=1 AND o.epoch=1 FROM semon_pg_store_state s CROSS JOIN semon_pg_store_owner o WHERE s.singleton AND o.singleton", &[]).map_err(sqlstate::error)?;
    if rows.len()!=1 || !rows[0].get::<_,bool>(2) || rows[0].get::<_,i64>(0)<0 { return Err(StorageError::ScopeChanged); }
    let bytes=rows[0].get::<_,Option<i64>>(1);
    if bytes.is_some_and(|n| !(0..=1048576).contains(&n)) { return Err(StorageError::ScopeChanged); }
    crate::selected_read::charge(64,bytes.unwrap_or(0) as usize).map_err(|_| StorageError::Busy)?;
    let value:Option<Vec<u8>>=tx.driver.query_one("SELECT pricing_target FROM semon_pg_store_state WHERE singleton", &[]).map_err(sqlstate::error)?.get(0);
    let value=value.map(String::from_utf8).transpose().map_err(|_| StorageError::ScopeChanged)?;
    Ok((rows[0].get(0),value))
}

/// Host control capability, bound to a qualified resource factory/pool. Opening
/// it reauthorizes the actual connection role; no identity string is accepted.
#[derive(Clone)]
pub struct PgStoreMetadata { pool: PgPool }
impl PgStoreMetadata {
    pub fn open(pool: PgPool, control: &PgCancellation) -> Result<Self, StorageError> {
        pool.transaction(&LogicalSourceNamespace::new("host-control")?,TxnKind::ReadSnapshot,control,|tx| authorize(tx))?;
        Ok(Self{pool})
    }
    /// Durable STORE operation identity; this is never a Source ingest ACK.
    pub fn set_pricing_target(&self, operation: &[u8;32], expected_revision: i64, target: Option<&str>, control: &PgCancellation) -> Result<i64,StorageError> {
        if expected_revision<0 || target.is_some_and(|v|v.len()>1048576) { return Err(StorageError::InvalidArguments); }
        let request=serde_json::to_vec(&(expected_revision,target)).map_err(|_|StorageError::InvalidArguments)?;
        let digest=Sha256::digest(request).to_vec();
        self.pool.transaction(&LogicalSourceNamespace::new("host-control")?,TxnKind::StoreWrite,control,|tx| {
            authorize(tx)?;
            let prior=tx.driver.query("SELECT request_digest,revision FROM semon_pg_store_operations WHERE operation_id=$1", &[&&operation[..]]).map_err(sqlstate::error)?;
            if let Some(row)=prior.first() { return if row.get::<_,Vec<u8>>(0)==digest { Ok(row.get(1)) } else { Err(StorageError::InvalidArguments) }; }
            let (revision,_) = read(tx)?;
            if revision!=expected_revision { return Err(StorageError::Conflict); }
            let next=revision.checked_add(1).ok_or(StorageError::Conflict)?;
            if tx.driver.execute("UPDATE semon_pg_store_state SET revision=$1,pricing_target=$2 WHERE singleton AND revision=$3", &[&next,&target.map(str::as_bytes),&revision]).map_err(sqlstate::error)?!=1 { return Err(StorageError::ScopeChanged); }
            if tx.driver.execute("INSERT INTO semon_pg_store_operations(operation_id,owner,request_digest,revision) VALUES($1,CURRENT_USER,$2,$3)", &[&&operation[..],&digest,&next]).map_err(sqlstate::error)?!=1 { return Err(StorageError::ScopeChanged); }
            tx.check()?;
            Ok(next)
        })
    }
}
fn authorize(tx: &mut PgTxn<'_>) -> Result<(),StorageError> {
    let allowed:bool=tx.driver.query_one("SELECT owner=CURRENT_USER AND has_table_privilege('semon_pg_store_state','UPDATE') AND has_table_privilege('semon_pg_store_operations','INSERT') FROM semon_pg_store_owner WHERE singleton", &[]).map_err(sqlstate::error)?.get(0);
    if !allowed { return Err(StorageError::ScopeChanged); }
    read(tx)?;
    Ok(())
}
