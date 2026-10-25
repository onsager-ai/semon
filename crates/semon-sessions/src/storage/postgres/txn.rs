use super::{PgCancellation, PgPool, pool::Driver, schema, sqlstate};
use crate::storage::{LogicalSourceNamespace, StorageError};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum TxnKind {
    ReadSnapshot,
    Write,
    StoreWrite,
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
        let mut lease = self.acquire(control)?;
        let limits = self.inner.limits;
        let deadline = Instant::now() + limits.txn_deadline;
        let connection = lease.connection.as_mut().ok_or(StorageError::Unavailable)?;
        connection.client.context(deadline, control, limits);
        let mut phase = Phase::PreBegin;
        let result = (|| {
            connection
                .client
                .batch_execute(match kind {
                    TxnKind::ReadSnapshot => "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
                    // Future effects require explicit locks/fences/unique keys.
                    TxnKind::Write | TxnKind::StoreWrite => "BEGIN ISOLATION LEVEL READ COMMITTED",
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
                schema::verify(&mut tx)?;
                super::store_metadata::lock(&mut tx, matches!(kind, TxnKind::StoreWrite))?;
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
}
