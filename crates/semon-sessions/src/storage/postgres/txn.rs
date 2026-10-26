use super::{PgCancellation, PgPool, pool::Driver, schema, sqlstate};
use crate::storage::{LogicalSourceNamespace, StorageError};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum TxnKind {
    ReadSnapshot,
    Write,
    StoreWrite,
    Transfer,
}

/// No driver values escape this adapter; callbacks are synchronous, borrowed,
/// nonblocking typed operations. Driver I/O itself is deadline bounded.
pub(super) struct PgTxn<'a> {
    pub(super) driver: &'a mut Driver,
    pub(super) namespace: &'a LogicalSourceNamespace,
    pub(super) deadline: Instant,
    pub(super) control: &'a PgCancellation,
}
impl PgTxn<'_> {
    pub(super) fn check(&self) -> Result<(), StorageError> {
        if self.control.is_cancelled() || Instant::now() >= self.deadline {
            Err(StorageError::Busy)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy)]
enum Phase {
    PreBegin,
    InTxn,
    CommitSent,
    Done,
}
impl PgPool {
    pub(super) fn transaction<T>(
        &self,
        namespace: &LogicalSourceNamespace,
        kind: TxnKind,
        control: &PgCancellation,
        operation: impl FnOnce(&mut PgTxn<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.transaction_inner(namespace, kind, control, true, operation)
    }
    pub(super) fn transaction_inner<T>(
        &self,
        namespace: &LogicalSourceNamespace,
        kind: TxnKind,
        control: &PgCancellation,
        verify_schema: bool,
        operation: impl FnOnce(&mut PgTxn<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        if namespace.as_str().is_empty() || namespace.as_str().len() > 4096 {
            return Err(StorageError::InvalidArguments);
        }
        if self
            .source_binding
            .as_ref()
            .is_some_and(|binding| &binding.namespace != namespace)
        {
            return Err(StorageError::NamespaceMismatch);
        }
        let mut lease = self.acquire(control)?;
        let limits = self.inner.limits;
        let deadline = control.deadline(Instant::now() + limits.txn_deadline);
        let connection = lease.connection.as_mut().ok_or(StorageError::Unavailable)?;
        connection.client.context(deadline, control, limits);
        let exclusive_admission = matches!(kind, TxnKind::StoreWrite);
        if verify_schema {
            let mut admission = PgTxn {
                driver: &mut connection.client,
                namespace,
                deadline,
                control,
            };
            // Acquire before BEGIN so a REPEATABLE READ transaction cannot
            // establish a stale snapshot while waiting for controlled DDL.
            super::store_metadata::lock_admission(&mut admission, exclusive_admission)?;
        }
        let schema_binding = &mut connection.schema_binding;
        let mut phase = Phase::PreBegin;
        let result = (|| {
            connection
                .client
                .batch_execute(match kind {
                    TxnKind::ReadSnapshot => "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
                    // Future effects require explicit locks/fences/unique keys.
                    TxnKind::Write | TxnKind::StoreWrite | TxnKind::Transfer => {
                        "BEGIN ISOLATION LEVEL READ COMMITTED"
                    }
                })
                .map_err(sqlstate::error)?;
            phase = Phase::InTxn;
            let mut tx = PgTxn {
                driver: &mut connection.client,
                namespace,
                deadline,
                control,
            };
            for (key, value) in [
                ("statement_timeout", limits.statement_timeout),
                ("lock_timeout", limits.lock_timeout),
                (
                    "idle_in_transaction_session_timeout",
                    limits.idle_in_tx_timeout,
                ),
            ] {
                let value = format!("{}ms", value.as_millis().max(1));
                tx.driver
                    .query_one("SELECT set_config($1, $2, true)", &[&key, &value])
                    .map_err(sqlstate::error)?;
            }
            tx.driver
                .query_one(
                    "SELECT set_config('semon.namespace', $1, true)",
                    &[&super::namespace_context(namespace)],
                )
                .map_err(sqlstate::error)?;
            tx.check()?;
            if verify_schema {
                let binding = schema::binding(&mut tx, namespace)?;
                if schema_binding.as_ref() != Some(&binding) {
                    schema::verify(&mut tx)?;
                    *schema_binding = Some(binding);
                }
                super::store_metadata::lock_state(&mut tx, exclusive_admission)?;
                if matches!(kind, TxnKind::Write | TxnKind::StoreWrite | TxnKind::Transfer) {
                    super::store_metadata::lock_publication(&mut tx)?;
                }
                if matches!(kind, TxnKind::ReadSnapshot) {
                    let incoming:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key='native_transfer_v1:incoming' AND octet_length(value)>0)", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0);
                    if incoming {
                        return Err(StorageError::NamespaceUpgradePending);
                    }
                }
                if matches!(kind, TxnKind::Write | TxnKind::StoreWrite) {
                    let blocked:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key IN ('native_transfer_v1:source-freeze','native_transfer_v1:incoming') AND octet_length(value)>0)", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0);
                    if blocked {
                        return Err(StorageError::ScopeChanged);
                    }
                }
            }
            if let Some(expected) = &self.source_binding {
                let actual = super::model_cache::binding(&mut tx)?;
                if actual.admission != expected.admission
                    || actual.stream_epoch != expected.stream_epoch
                {
                    return Err(StorageError::ScopeChanged);
                }
            }
            let value = operation(&mut tx)?;
            tx.check()?;
            // batch_execute constructs no protocol request until first poll.
            // Mark conservatively BEFORE that poll: any error from this point
            // lacks a COMMIT ACK and requires exact-operation reconciliation.
            phase = Phase::CommitSent;
            tx.driver
                .batch_execute("COMMIT")
                .map_err(|_| StorageError::CommitUncertain)?;
            phase = Phase::Done;
            #[cfg(test)]
            if super::tests::lose_ack() {
                return Err(StorageError::CommitUncertain);
            }
            Ok(value)
        })();
        if result.is_err() && matches!(phase, Phase::InTxn) && !connection.client.interrupted.get()
        {
            // COMMIT was never submitted. An absent rollback ACK leaves remote
            // cleanup unknown, but cannot make this transaction committed.
            let _ = connection.client.rollback();
        }
        // A known COMMIT ACK survives even a failed cleanup. Any interrupted
        // connection remains tainted and is never reused.
        if result.is_ok() {
            let _ = lease.check_clean();
        }
        result
    }

    /// Run host-controlled schema DDL under the existing exclusive schema
    /// lock and commit its epoch invalidation with the DDL. This remains an
    /// internal test capability until a public schema-evolution operation
    /// exists. The runtime role has no DDL privilege, and this must not become
    /// an accidental public backend capability.
    #[cfg(test)]
    pub(super) fn controlled_schema_change<T>(
        &self,
        control: &PgCancellation,
        operation: impl FnOnce(&mut PgTxn<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let namespace = LogicalSourceNamespace::new("schema")?;
        self.transaction_inner(&namespace, TxnKind::StoreWrite, control, true, |tx| {
            let value = operation(tx)?;
            schema::advance_epoch(tx)?;
            Ok(value)
        })
    }
}
