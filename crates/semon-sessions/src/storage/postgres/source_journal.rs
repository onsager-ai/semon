//! Typed Source observation family only. There is deliberately no StoreConfig
//! selector or public constructor: native ledger/publication TX ports are not PG
//! yet. Tests inject this exact port into the normal EventCache producer.
use super::{
    PgCancellation, PgPool, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{
    LogicalSourceNamespace, StorageError,
    source_observations::{
        SourceClaims, SourceIntent, SourceMutationReceipt, SourceObservationStore, SourceOperation,
        validate_intent, validate_paths,
    },
};

/// PostgreSQL implementation of the shared typed observation contract. It owns
/// no SQLite handle, parser, Catalog publisher, credential or backend selector.
pub struct PgSourceJournal {
    pub(super) pool: PgPool,
    pub(super) namespace: LogicalSourceNamespace,
    pub(super) control: PgCancellation,
}

impl PgSourceJournal {
    fn read<T>(
        &self,
        f: impl FnOnce(&mut PgTxn<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.pool
            .transaction(&self.namespace, TxnKind::ReadSnapshot, &self.control, f)
    }
    fn write<T>(
        &self,
        f: impl FnOnce(&mut PgTxn<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.pool.transaction(&self.namespace, TxnKind::Write, &self.control, |tx| {
            let namespace = tx.namespace.as_str().as_bytes();
            tx.driver.execute("INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT(namespace_digest) DO NOTHING", &[&namespace]).map_err(sqlstate::error)?;
            // This namespace row serializes queue capacity, CAS checks, effects,
            // receipts and future publication fences. Full bytes qualify the
            // digest key even under an impossible-but-refused hash collision.
            let row = tx.driver.query_one("SELECT namespace FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) FOR UPDATE", &[&namespace]).map_err(sqlstate::error)?;
            if row.try_get::<_, Vec<u8>>(0).map_err(|_| StorageError::Unavailable)?.as_slice() != namespace {
                return Err(StorageError::ScopeChanged);
            }
            f(tx)
        })
    }
    fn apply(&self, intent: &SourceIntent) -> Result<SourceMutationReceipt, StorageError> {
        // Shared validation binds the namespace, kind, payload and operation ID.
        // Foreign/rebound operations return Conflict without acquiring a writer.
        if let Err(error) = validate_intent(Some(self.namespace.as_str()), intent) {
            return if error == StorageError::Conflict {
                Ok(SourceMutationReceipt::Conflict)
            } else {
                Err(error)
            };
        }
        self.write(|tx| {
            let operation = intent.operation();
            if let Some(receipt) = receipt(tx, operation)? { return Ok(receipt); }
            let applied = match intent {
                SourceIntent::Discover { paths, complete, .. } => {
                    for path in paths {
                        let inserted = tx.driver.execute("INSERT INTO semon_pg_source_members(namespace,source_path) VALUES($1,$2) ON CONFLICT(namespace_digest,source_path) DO NOTHING", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?;
                        if inserted == 1 { invalidate(tx, path)?; }
                    }
                    let state = if *complete { "draining" } else { "discovering" };
                    tx.driver.execute("UPDATE semon_pg_source_state SET backfill_state=$2 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&tx.namespace.as_str().as_bytes(), &state]).map_err(sqlstate::error)?;
                    true
                }
                SourceIntent::Invalidate { expected, .. } => {
                    let paths = expected.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>();
                    if claims(tx, &paths)? != *expected { false } else {
                        for path in paths { invalidate(tx, &path)?; }
                        true
                    }
                }
                SourceIntent::FinishBackfill { expected_generation, .. } => {
                    // Publication head is persisted backend state, never a
                    // caller-supplied replacement for a Catalog publisher.
                    // F4c must write this head and drain claims in its ledger TX.
                    let row = tx.driver.query_one("SELECT catalog_generation, semantic_pending, EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea)) FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
                    let generation: Option<String> = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
                    let semantic: bool = row.try_get(1).map_err(|_| StorageError::Unavailable)?;
                    let pending: bool = row.try_get(2).map_err(|_| StorageError::Unavailable)?;
                    // No publication port exists yet: an unbound head refuses,
                    // rather than inventing a successful empty PG Catalog.
                    if generation.is_none() || generation != *expected_generation || semantic || pending { false } else {
                        tx.driver.execute("UPDATE semon_pg_source_state SET backfill_state='complete',catalog_completeness='complete',search_backfill_complete=true,recovery_reason=NULL WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
                        true
                    }
                }
            };
            let outcome = if applied { "applied" } else { "conflict" };
            tx.driver.execute("INSERT INTO semon_pg_source_receipts(namespace,operation_id,payload_digest,outcome) VALUES($1,$2,$3,$4)", &[&tx.namespace.as_str().as_bytes(), &operation.id, &operation.payload_digest, &outcome]).map_err(sqlstate::error)?;
            Ok(if applied { SourceMutationReceipt::Applied } else { SourceMutationReceipt::Conflict })
        })
    }
}

fn receipt(
    tx: &mut PgTxn<'_>,
    operation: &SourceOperation,
) -> Result<Option<SourceMutationReceipt>, StorageError> {
    let namespace = tx.namespace.as_str().as_bytes();
    let rows = tx.driver.query("SELECT payload_digest FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND operation_id=$2", &[&namespace, &operation.id]).map_err(sqlstate::error)?;
    if let Some(row) = rows.first() {
        let digest: String = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
        if digest != operation.payload_digest {
            return Ok(Some(SourceMutationReceipt::Conflict));
        }
    }
    let rows = tx.driver.query("SELECT payload_digest,outcome FROM semon_pg_source_receipts WHERE namespace_digest=sha256($1::bytea) AND operation_id=$2", &[&namespace, &operation.id]).map_err(sqlstate::error)?;
    rows.first()
        .map(|row| {
            let digest: String = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
            let outcome: String = row.try_get(1).map_err(|_| StorageError::Unavailable)?;
            Ok(
                if digest == operation.payload_digest && outcome == "applied" {
                    SourceMutationReceipt::AlreadyApplied
                } else {
                    SourceMutationReceipt::Conflict
                },
            )
        })
        .transpose()
}

fn claims(tx: &mut PgTxn<'_>, paths: &[String]) -> Result<SourceClaims, StorageError> {
    paths.iter().map(|path| {
        let rows = tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND source_path=$2", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?;
        Ok((path.clone(), rows.first().map(|r| r.try_get(0).map_err(|_| StorageError::Unavailable)).transpose()?))
    }).collect()
}

fn invalidate(tx: &mut PgTxn<'_>, path: &str) -> Result<(), StorageError> {
    let namespace = tx.namespace.as_str().as_bytes();
    let rows = tx.driver.query("UPDATE semon_pg_source_state SET revision=revision+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND revision<9223372036854775807 RETURNING revision", &[&namespace]).map_err(sqlstate::error)?;
    let revision: i64 = rows
        .first()
        .ok_or(StorageError::Unavailable)?
        .try_get(0)
        .map_err(|_| StorageError::Unavailable)?;
    tx.driver.execute("INSERT INTO semon_pg_source_invalidations(namespace,source_path,revision,reason,observed_at) VALUES($1,$2,$3,'changed',$4) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET revision=excluded.revision,reason=excluded.reason,observed_at=excluded.observed_at", &[&namespace, &path.as_bytes(), &revision, &crate::model::now_ms()]).map_err(sqlstate::error)?;
    Ok(())
}

impl SourceObservationStore for PgSourceJournal {
    fn persist_intent(&mut self, intent: &SourceIntent) -> Result<(), StorageError> {
        validate_intent(Some(self.namespace.as_str()), intent)?;
        let payload = serde_json::to_string(intent).map_err(|_| StorageError::InvalidArguments)?;
        if payload.len() > 2 * 1024 * 1024 {
            return Err(StorageError::InvalidArguments);
        }
        self.write(|tx| {
            let operation = intent.operation();
            let namespace = tx.namespace.as_str().as_bytes();
            let existing = tx.driver.query("SELECT payload FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND operation_id=$2", &[&namespace, &operation.id]).map_err(sqlstate::error)?;
            if let Some(row) = existing.first() {
                return if row.try_get::<_, String>(0).map_err(|_| StorageError::Unavailable)? == payload { Ok(()) } else { Err(StorageError::Conflict) };
            }
            let count: i64 = tx.driver.query_one("SELECT count(*) FROM (SELECT 1 FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) LIMIT 256) bounded", &[&namespace]).map_err(sqlstate::error)?.try_get(0).map_err(|_| StorageError::Unavailable)?;
            if count >= 256 { return Err(StorageError::Busy); }
            let receipts = tx.driver.query("SELECT payload_digest FROM semon_pg_source_receipts WHERE namespace_digest=sha256($1::bytea) AND operation_id=$2", &[&namespace, &operation.id]).map_err(sqlstate::error)?;
            let digest = receipts.first().map(|row| row.try_get::<_, String>(0).map_err(|_| StorageError::Unavailable)).transpose()?;
            if digest.is_some_and(|digest| digest != operation.payload_digest) { return Err(StorageError::Conflict); }
            tx.driver.execute("INSERT INTO semon_pg_source_intents(namespace,operation_id,payload_digest,payload) VALUES($1,$2,$3,$4)", &[&namespace, &operation.id, &operation.payload_digest, &payload]).map_err(sqlstate::error)?;
            Ok(())
        })
    }
    fn pending_intents(&self) -> Result<Vec<SourceIntent>, StorageError> {
        self.read(|tx| {
            let rows = tx.driver.query("SELECT operation_id,payload_digest,payload FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) ORDER BY sequence LIMIT 257", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
            if rows.len() > 256 { return Err(StorageError::Unavailable); }
            rows.iter().map(|row| {
                let id: String = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
                let digest: String = row.try_get(1).map_err(|_| StorageError::Unavailable)?;
                let payload: String = row.try_get(2).map_err(|_| StorageError::Unavailable)?;
                if payload.len() > 2 * 1024 * 1024 { return Err(StorageError::Unavailable); }
                let intent: SourceIntent = serde_json::from_str(&payload).map_err(|_| StorageError::Unavailable)?;
                validate_intent(Some(tx.namespace.as_str()), &intent)?;
                if intent.operation().id != id || intent.operation().payload_digest != digest { return Err(StorageError::Conflict); }
                Ok(intent)
            }).collect()
        })
    }
    fn acknowledge_intent(&mut self, operation: &SourceOperation) -> Result<(), StorageError> {
        if operation.namespace != self.namespace.as_str() {
            return Err(StorageError::Conflict);
        }
        self.write(|tx| {
            let namespace = tx.namespace.as_str().as_bytes();
            let rows = tx.driver.query("SELECT 1 FROM semon_pg_source_receipts WHERE namespace_digest=sha256($1::bytea) AND operation_id=$2 AND payload_digest=$3 AND outcome IN ('applied','conflict')", &[&namespace, &operation.id, &operation.payload_digest]).map_err(sqlstate::error)?;
            if rows.len() != 1 { return Err(StorageError::Conflict); }
            tx.driver.execute("DELETE FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND operation_id=$2 AND payload_digest=$3", &[&namespace, &operation.id, &operation.payload_digest]).map_err(sqlstate::error)?;
            Ok(())
        })
    }
    fn discover(
        &mut self,
        operation: &SourceOperation,
        paths: &[String],
        complete: bool,
    ) -> Result<SourceMutationReceipt, StorageError> {
        validate_paths(paths)?;
        self.apply(&SourceIntent::Discover {
            operation: operation.clone(),
            paths: paths.to_vec(),
            complete,
        })
    }
    fn invalidate(
        &mut self,
        operation: &SourceOperation,
        expected: &[(String, Option<i64>)],
    ) -> Result<SourceMutationReceipt, StorageError> {
        if expected.len() > 256 {
            return Err(StorageError::InvalidArguments);
        }
        self.apply(&SourceIntent::Invalidate {
            operation: operation.clone(),
            expected: expected.to_vec(),
        })
    }
    fn claims(&self, paths: &[String]) -> Result<SourceClaims, StorageError> {
        validate_paths(paths)?;
        self.read(|tx| claims(tx, paths))
    }
    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError> {
        if limit == 0 || limit > 256 {
            return Err(StorageError::InvalidArguments);
        }
        self.read(|tx| {
            let rows = tx.driver.query("SELECT source_path FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) ORDER BY revision,source_path LIMIT $2", &[&tx.namespace.as_str().as_bytes(), &(limit as i64)]).map_err(sqlstate::error)?;
            rows.iter().map(|row| {
                String::from_utf8(row.try_get(0).map_err(|_| StorageError::Unavailable)?).map_err(|_| StorageError::Unavailable)
            }).collect()
        })
    }
    fn backfill_state(&self) -> Result<Option<String>, StorageError> {
        self.read(|tx| {
            let rows = tx.driver.query("SELECT backfill_state FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea)", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
            Ok(rows.first().map(|row| row.try_get::<_, Option<String>>(0).map_err(|_| StorageError::Unavailable)).transpose()?.flatten())
        })
    }
    fn finish_backfill(
        &mut self,
        operation: &SourceOperation,
        expected_generation: Option<&str>,
    ) -> Result<SourceMutationReceipt, StorageError> {
        if expected_generation.is_some_and(|value| value.len() > 4096) {
            return Err(StorageError::InvalidArguments);
        }
        self.apply(&SourceIntent::FinishBackfill {
            operation: operation.clone(),
            expected_generation: expected_generation.map(str::to_owned),
        })
    }
}

#[cfg(test)]
mod tests;
