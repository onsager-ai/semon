use super::{
    PgCancellation, PgConnectionFactory, PgPool, PoolLimits, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{LogicalSourceNamespace, StorageError};
use std::sync::Arc;

#[path = "native_schema.rs"]
mod native;

pub(super) const VERSION: i32 = 3;

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let rows = tx
        .driver
        .query("SELECT version FROM semon_pg_schema LIMIT 2", &[])
        .map_err(sqlstate::error)?;
    if rows.len() != 1
        || rows[0]
            .try_get::<_, i32>(0)
            .map_err(|_| StorageError::Unavailable)?
            != VERSION
    {
        return Err(StorageError::ScopeChanged);
    }
    native::verify(tx)
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
                CREATE TABLE semon_pg_schema(version integer NOT NULL PRIMARY KEY);
                INSERT INTO semon_pg_schema VALUES (3);
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
