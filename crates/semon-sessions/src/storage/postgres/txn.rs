use super::{
    PgCancellation, PgPool,
    cancel::{Monitor, StopOnDrop},
    schema, sqlstate,
};
use crate::storage::{LogicalSourceNamespace, StorageError};
use std::sync::atomic::Ordering;
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum TxnKind {
    ReadSnapshot,
    Write,
}

/// Private adapter context, not a public arbitrary-SQL facade. Future #478 ports
/// add typed helpers here; driver values never cross the adapter boundary.
pub(super) struct PgTxn<'a> {
    pub(super) driver: ::postgres::Transaction<'a>,
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
        let request = connection.cancel.clone();
        let monitor = Monitor::new();
        let result = std::thread::scope(|scope| {
            scope.spawn(|| monitor.run(control, deadline, request.as_ref()));
            let _stop = StopOnDrop(&monitor);
            let mut builder = connection.client.build_transaction();
            builder = match kind {
                TxnKind::ReadSnapshot => builder
                    .isolation_level(::postgres::IsolationLevel::RepeatableRead)
                    .read_only(true),
                // Ledger reservation uses the database unique-key lock. Future
                // effects must explicitly lock their own publication/claim keys.
                TxnKind::Write => {
                    builder.isolation_level(::postgres::IsolationLevel::ReadCommitted)
                }
            };
            let driver = builder.start().map_err(sqlstate::error)?;
            let mut tx = PgTxn {
                driver,
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
            }
            let value = match operation(&mut tx) {
                Ok(value) => value,
                Err(error) => {
                    // Explicit rollback ACK (or connection discard on failure).
                    tx.driver.rollback().map_err(sqlstate::error)?;
                    return Err(error);
                }
            };
            if let Err(error) = tx.check() {
                tx.driver.rollback().map_err(sqlstate::error)?;
                return Err(error); // COMMIT has not been sent
            }
            tx.driver
                .commit()
                .map_err(|error| sqlstate::classify(error.code().map(|code| code.code()), true))?;
            #[cfg(test)]
            if super::tests::lose_ack() {
                return Err(StorageError::CommitUncertain);
            }
            Ok(value)
        }); // joins any in-flight cancel before touching the lease
        if result.is_ok() && monitor.attempted.load(Ordering::SeqCst) {
            return Err(StorageError::CommitUncertain);
        }
        // A cleanup failure discards the connection, but cannot undo a known
        // successful COMMIT or turn its acknowledged result into a refusal.
        if result.is_ok() {
            let _ = lease.check_clean();
        }
        result
    }
}
