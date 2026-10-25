use super::{
    PgCancellation, PgConnectionFactory, PgPool, PoolLimits, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{LogicalSourceNamespace, StorageError};
use std::sync::Arc;

pub(super) const VERSION: i32 = 2;

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
                CREATE TABLE semon_pg_schema(version integer NOT NULL PRIMARY KEY);
                INSERT INTO semon_pg_schema VALUES (2);
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
                Ok(())
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
