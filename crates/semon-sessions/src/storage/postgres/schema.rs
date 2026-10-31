use super::{
    PgCancellation, PgConnectionFactory, PgPool, PoolLimits, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{LogicalSourceNamespace, StorageError};
use std::sync::Arc;

#[path = "native_schema.rs"]
mod native;

pub(super) const VERSION: i32 = 11;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Binding {
    database_oid: i64,
    database_name: String,
    session_role_oid: i64,
    role_oid: i64,
    schema_oid: i64,
    version: i32,
    epoch: i64,
    native_contract: Vec<u8>,
}

#[cfg(test)]
thread_local! {
    static FULL_VERIFY_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_verify_count() {
    FULL_VERIFY_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn verify_count() -> usize {
    FULL_VERIFY_COUNT.with(std::cell::Cell::get)
}

/// Read the cheap per-transaction admission tuple while holding the existing
/// schema-scoped advisory lock. Namespace context is checked on every call but
/// is deliberately not part of the connection's reusable schema identity.
pub(super) fn binding(
    tx: &mut PgTxn<'_>,
    namespace: &LogicalSourceNamespace,
) -> Result<Binding, StorageError> {
    let rows = tx
        .driver
        .query(
            "SELECT d.oid::bigint,d.datname::text,sr.oid::bigint,cr.oid::bigint,n.oid::bigint,marker.version,marker.epoch,marker.native_contract,cr.rolsuper,cr.rolbypassrls,(owner.owner=CURRENT_USER),has_schema_privilege(CURRENT_USER,n.oid,'CREATE'),(has_table_privilege(CURRENT_USER,marker_table.oid,'SELECT') AND (owner.owner=CURRENT_USER OR NOT (has_table_privilege(CURRENT_USER,marker_table.oid,'INSERT') OR has_table_privilege(CURRENT_USER,marker_table.oid,'UPDATE') OR has_table_privilege(CURRENT_USER,marker_table.oid,'DELETE')))),COALESCE((SELECT bool_and(CASE WHEN t.relname='semon_pg_schema' OR t.relname LIKE 'semon_pg_store_%' THEN has_table_privilege(CURRENT_USER,t.oid,'SELECT') AND (owner.owner=CURRENT_USER OR NOT (has_table_privilege(CURRENT_USER,t.oid,'INSERT') OR has_table_privilege(CURRENT_USER,t.oid,'UPDATE') OR has_table_privilege(CURRENT_USER,t.oid,'DELETE'))) ELSE (has_table_privilege(CURRENT_USER,t.oid,'SELECT') AND has_table_privilege(CURRENT_USER,t.oid,'INSERT') AND has_table_privilege(CURRENT_USER,t.oid,'UPDATE') AND has_table_privilege(CURRENT_USER,t.oid,'DELETE')) END) FROM pg_class t WHERE t.relnamespace=n.oid AND t.relkind='r' AND t.relname LIKE 'semon_pg_%'),false),current_setting('semon.namespace',true) FROM pg_database d JOIN pg_roles sr ON sr.rolname=session_user JOIN pg_roles cr ON cr.rolname=current_user JOIN pg_namespace n ON n.nspname=current_schema() JOIN pg_class marker_table ON marker_table.relnamespace=n.oid AND marker_table.relname='semon_pg_schema' AND marker_table.relkind='r' JOIN semon_pg_schema marker ON marker.tableoid=marker_table.oid JOIN pg_class owner_table ON owner_table.relnamespace=n.oid AND owner_table.relname='semon_pg_store_owner' AND owner_table.relkind='r' JOIN semon_pg_store_owner owner ON owner.tableoid=owner_table.oid AND owner.singleton WHERE d.datname=current_database()",
            &[],
        )
        .map_err(sqlstate::error)?;
    if rows.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let row = &rows[0];
    let is_store_owner: bool = row.get(10);
    if row.get::<_, bool>(8)
        || row.get::<_, bool>(9)
        || (!is_store_owner && row.get::<_, bool>(11))
        || !row.get::<_, bool>(12)
        || !row.get::<_, bool>(13)
        || row.get::<_, Option<String>>(14).as_deref()
            != Some(super::namespace_context(namespace).as_str())
        || row.get::<_, i64>(6) < 1
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(Binding {
        database_oid: row.get(0),
        database_name: row.get(1),
        session_role_oid: row.get(2),
        role_oid: row.get(3),
        schema_oid: row.get(4),
        version: row.get(5),
        epoch: row.get(6),
        native_contract: row.get(7),
    })
}

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    #[cfg(test)]
    FULL_VERIFY_COUNT.with(|count| count.set(count.get() + 1));
    let rows = tx
        .driver
        .query(
            "SELECT marker.version,marker.epoch FROM semon_pg_schema marker JOIN pg_class c ON c.oid=marker.tableoid AND c.relnamespace=current_schema()::regnamespace AND c.relname='semon_pg_schema' AND c.relkind='r' LIMIT 2",
            &[],
        )
        .map_err(sqlstate::error)?;
    if rows.len() != 1
        || rows[0]
            .try_get::<_, i32>(0)
            .map_err(|_| StorageError::Unavailable)?
            != VERSION
        || rows[0]
            .try_get::<_, i64>(1)
            .map_err(|_| StorageError::ScopeChanged)?
            < 1
    {
        return Err(StorageError::ScopeChanged);
    }
    let marker_shape = tx
        .driver
        .query_one(
            "SELECT c.relkind='r' AND pg_get_userbyid(c.relowner)::name=o.owner AND ARRAY(SELECT a.attname::text||' '||format_type(a.atttypid,a.atttypmod)||CASE WHEN a.attnotnull THEN '' ELSE '?' END FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum)=ARRAY['version integer','epoch bigint','native_contract bytea']::text[] AND ARRAY(SELECT a.attname::text FROM pg_constraint k CROSS JOIN LATERAL unnest(k.conkey) WITH ORDINALITY x(num,ord) JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=x.num WHERE k.conrelid=c.oid AND k.contype='p' ORDER BY x.ord)=ARRAY['version']::text[] AND ARRAY(SELECT pg_get_expr(k.conbin,k.conrelid) FROM pg_constraint k WHERE k.conrelid=c.oid AND k.contype='c' AND k.convalidated ORDER BY 1)=ARRAY['(epoch > 0)']::text[] AND NOT EXISTS(SELECT 1 FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND (a.attidentity<>'' OR a.attgenerated<>'' OR d.oid IS NOT NULL)) AND NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid=c.oid AND (NOT convalidated OR contype NOT IN ('p','c'))) AND NOT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid=c.oid AND NOT tgisinternal) AND NOT EXISTS(SELECT 1 FROM pg_rewrite WHERE ev_class=c.oid) AND has_table_privilege(CURRENT_USER,c.oid,'SELECT') AND (o.owner=CURRENT_USER OR NOT (has_table_privilege(CURRENT_USER,c.oid,'INSERT') OR has_table_privilege(CURRENT_USER,c.oid,'UPDATE') OR has_table_privilege(CURRENT_USER,c.oid,'DELETE'))) FROM pg_class c JOIN pg_class owner_table ON owner_table.relnamespace=current_schema()::regnamespace AND owner_table.relname='semon_pg_store_owner' AND owner_table.relkind='r' JOIN semon_pg_store_owner o ON o.tableoid=owner_table.oid AND o.singleton WHERE c.relnamespace=current_schema()::regnamespace AND c.relname='semon_pg_schema'",
            &[],
        )
        .map_err(sqlstate::error)?
        .get::<_, bool>(0);
    if !marker_shape {
        return Err(StorageError::ScopeChanged);
    }
    native::verify(tx)?;
    super::store_metadata::verify(tx)
}

/// Must be called after controlled DDL while holding the exclusive schema
/// advisory lock. The epoch and DDL commit atomically in the caller's txn.
///
/// Production exposes no schema-evolution capability yet: the runtime role is
/// intentionally denied DDL. Keep this test-only host fixture rather than
/// leaving a dormant production capability that could be mistaken for a
/// qualified schema migration API.
#[cfg(test)]
pub(super) fn advance_epoch(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let rows = tx
        .driver
        .query(
            "UPDATE semon_pg_schema marker SET epoch=marker.epoch+1 FROM pg_class c WHERE marker.tableoid=c.oid AND c.relnamespace=current_schema()::regnamespace AND c.relname='semon_pg_schema' AND c.relkind='r' AND marker.version=$1 AND marker.epoch>0 RETURNING marker.epoch",
            &[&VERSION],
        )
        .map_err(sqlstate::error)?;
    if rows.len() != 1 || rows[0].get::<_, i64>(0) < 2 {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

impl PgPool {
    /// Explicit creation of a fresh, current foundation schema. Existing tables
    /// cause refusal; this never resets a store or runs historical upgrade chains.
    /// Hosts authorize DDL separately and choose the session-data search_path.
    pub fn create_schema(
        factory: Arc<dyn PgConnectionFactory>,
        limits: PoolLimits,
    ) -> Result<Self, StorageError> {
        let pool = Self::new(factory, limits)?;
        let namespace = LogicalSourceNamespace::new("schema")?;
        pool.transaction_inner(
            &namespace,
            TxnKind::Write,
            &PgCancellation::default(),
            false,
            |tx| {
                tx.driver
                    .batch_execute(
                        "
                CREATE TABLE semon_pg_schema(version integer NOT NULL PRIMARY KEY,epoch bigint NOT NULL CHECK(epoch>0));
                INSERT INTO semon_pg_schema VALUES (11,1);
                CREATE TABLE semon_pg_operations(
                    namespace bytea NOT NULL CHECK (octet_length(namespace) BETWEEN 1 AND 4096),
                    namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,
                    operation_id char(64) NOT NULL CHECK (operation_id ~ '^[0-9A-Fa-f]{64}$'),
                    kind text NOT NULL CHECK (octet_length(kind) BETWEEN 1 AND 128),
                    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
                    outcome jsonb NOT NULL CHECK (octet_length(outcome::text) <= 1048576),
                    committed_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
                    PRIMARY KEY(namespace_digest, operation_id)
                );
                ALTER TABLE semon_pg_operations ENABLE ROW LEVEL SECURITY;
                ALTER TABLE semon_pg_operations FORCE ROW LEVEL SECURITY;
                CREATE FUNCTION semon_pg_namespace_matches(stored bytea, digest bytea, expected bytea)
                RETURNS boolean LANGUAGE plpgsql AS $$
                BEGIN
                    IF expected IS NULL OR digest <> sha256(expected) THEN RETURN false; END IF;
                    IF stored <> expected THEN
                        RAISE EXCEPTION 'Logical namespace digest collision' USING ERRCODE = '55000';
                    END IF;
                    RETURN stored = expected;
                END; $$;
                CREATE POLICY semon_pg_operation_namespace ON semon_pg_operations
                    USING (semon_pg_namespace_matches(namespace, namespace_digest,
                        decode(current_setting('semon.namespace', true), 'hex')))
                    WITH CHECK (namespace = decode(current_setting('semon.namespace', true), 'hex'));
            ",
                    )
                    .map_err(sqlstate::error)?;
                tx.driver.batch_execute("
                CREATE TABLE semon_pg_source_state(
                    namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),
                    namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED PRIMARY KEY,
                    revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0),
                    backfill_state text CHECK(backfill_state IN ('discovering','draining','complete')),
                    catalog_generation text CHECK(octet_length(catalog_generation)<=4096),
                    semantic_pending boolean NOT NULL DEFAULT false,
                    catalog_completeness text,
                    search_backfill_complete boolean NOT NULL DEFAULT false,
                    recovery_reason text
                );
                CREATE TABLE semon_pg_source_members(
                    namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),
                    namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,
                    source_path bytea NOT NULL CHECK(octet_length(source_path) BETWEEN 1 AND 4096),
                    PRIMARY KEY(namespace_digest,source_path)
                );
                CREATE TABLE semon_pg_source_invalidations(
                    namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),
                    namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,
                    source_path bytea NOT NULL CHECK(octet_length(source_path) BETWEEN 1 AND 4096),
                    revision bigint NOT NULL CHECK(revision>0),
                    reason text NOT NULL,
                    observed_at bigint NOT NULL,
                    search_path_digest bytea GENERATED ALWAYS AS (sha256(source_path)) STORED NOT NULL,
                    search_filename_digest bytea GENERATED ALWAYS AS (sha256(decode(translate(regexp_replace(encode(source_path, 'escape'::text), '^.*/'::text, ''::text), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text), 'escape'::text))) STORED NOT NULL,
                    search_copilot_digest bytea GENERATED ALWAYS AS (sha256(decode(translate(regexp_replace(regexp_replace(encode(source_path, 'escape'::text), '/events[.]jsonl$'::text, ''::text), '^.*/'::text, ''::text), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text), 'escape'::text))) STORED NOT NULL,
                    PRIMARY KEY(namespace_digest,source_path)
                );
                CREATE INDEX semon_pg_source_pending ON semon_pg_source_invalidations(namespace_digest,revision,source_path);
                CREATE TABLE semon_pg_source_receipts(
                    namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),
                    namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,
                    operation_id text NOT NULL CHECK(octet_length(operation_id) BETWEEN 1 AND 256),
                    payload_digest text NOT NULL CHECK(payload_digest ~ '^[0-9A-Fa-f]{64}$'),
                    outcome text NOT NULL CHECK(outcome IN ('applied','conflict')),
                    PRIMARY KEY(namespace_digest,operation_id)
                );
                CREATE TABLE semon_pg_source_intents(
                    namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),
                    namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,
                    operation_id text NOT NULL CHECK(octet_length(operation_id) BETWEEN 1 AND 256),
                    payload_digest text NOT NULL CHECK(payload_digest ~ '^[0-9A-Fa-f]{64}$'),
                    payload text NOT NULL CHECK(octet_length(payload)<=2097152),
                    sequence bigint GENERATED ALWAYS AS IDENTITY,
                    PRIMARY KEY(namespace_digest,operation_id)
                );
                CREATE INDEX semon_pg_source_intent_order ON semon_pg_source_intents(namespace_digest,sequence);
                ").map_err(sqlstate::error)?;
                for table in ["semon_pg_source_state", "semon_pg_source_members", "semon_pg_source_invalidations", "semon_pg_source_receipts", "semon_pg_source_intents"] {
                    tx.driver.batch_execute(&format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY; ALTER TABLE {table} FORCE ROW LEVEL SECURITY; CREATE POLICY {table}_namespace ON {table} USING (semon_pg_namespace_matches(namespace,namespace_digest,decode(current_setting('semon.namespace',true),'hex'))) WITH CHECK (semon_pg_namespace_matches(namespace,namespace_digest,decode(current_setting('semon.namespace',true),'hex')));")).map_err(sqlstate::error)?;
                }
                native::install(tx)?;
                super::store_metadata::install(tx)?;
                verify(tx)
            },
        )?;
        Ok(pool)
    }

    /// Open only an explicitly provisioned matching schema. No automatic DDL,
    /// service startup, credentials, SQLite access or fallback is performed.
    pub fn open(
        factory: Arc<dyn PgConnectionFactory>,
        limits: PoolLimits,
    ) -> Result<Self, StorageError> {
        let pool = Self::new(factory, limits)?;
        let namespace = LogicalSourceNamespace::new("schema")?;
        pool.transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |_| Ok(()),
        )?;
        Ok(pool)
    }
}
