use super::{
    PgCancellation, PgPool, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{LogicalSourceNamespace, StorageError};
use serde_json::Value;

/// Retain this exact identity across uncertain replies. Digests bind the full
/// typed request, not merely an operation kind. Fresh work needs a fresh ID.
pub struct OperationIdentity<'a> {
    pub operation_id: &'a str,
    pub kind: &'a str,
    pub request_digest: &'a [u8; 32],
}

impl OperationIdentity<'_> {
    fn validate(&self) -> Result<(), StorageError> {
        if self.operation_id.len() != 64
            || !self.operation_id.bytes().all(|b| b.is_ascii_hexdigit())
            || self.kind.is_empty()
            || self.kind.len() > 128
            || self.kind.contains('\0')
        {
            return Err(StorageError::InvalidArguments);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum Reconciliation {
    Committed(Value),
    Absent,
}

impl PgTxn<'_> {
    fn lookup_operation(
        &mut self,
        id: &OperationIdentity<'_>,
    ) -> Result<Reconciliation, StorageError> {
        self.check()?;
        let rows = self.driver.query("SELECT kind, request_digest, outcome FROM semon_pg_operations WHERE namespace = $1 AND operation_id = $2",
            &[&self.namespace.as_str(), &id.operation_id]).map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            return Ok(Reconciliation::Absent);
        };
        let kind: String = row.try_get(0).map_err(|_| StorageError::Unavailable)?;
        let digest: Vec<u8> = row.try_get(1).map_err(|_| StorageError::Unavailable)?;
        if kind != id.kind || digest.as_slice() != &id.request_digest[..] {
            return Err(StorageError::InvalidArguments);
        }
        Ok(Reconciliation::Committed(
            row.try_get(2).map_err(|_| StorageError::Unavailable)?,
        ))
    }
}

impl PgPool {
    pub fn reconcile(
        &self,
        namespace: &LogicalSourceNamespace,
        identity: &OperationIdentity<'_>,
        control: &PgCancellation,
    ) -> Result<Reconciliation, StorageError> {
        identity.validate()?;
        self.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
            tx.lookup_operation(identity)
        })
    }

    // Intentionally private until #478 supplies the typed native effects. This
    // primitive is exercised against real PG, but is not a public SQL callback.
    #[allow(dead_code)]
    pub(super) fn apply_operation(
        &self,
        namespace: &LogicalSourceNamespace,
        id: &OperationIdentity<'_>,
        control: &PgCancellation,
        effect: impl FnOnce(&mut PgTxn<'_>) -> Result<Value, StorageError>,
    ) -> Result<Value, StorageError> {
        id.validate()?;
        self.transaction(namespace, TxnKind::Write, control, |tx| {
            tx.check()?;
            // The unique key serializes competing identical operation IDs. On
            // conflict the next READ COMMITTED statement sees the winner's row.
            let inserted = tx.driver.execute("INSERT INTO semon_pg_operations(namespace, operation_id, kind, request_digest, outcome) VALUES ($1,$2,$3,$4,'null'::jsonb) ON CONFLICT (namespace, operation_id) DO NOTHING",
                &[&namespace.as_str(), &id.operation_id, &id.kind, &&id.request_digest[..]]).map_err(sqlstate::error)?;
            if inserted == 0 {
                return match tx.lookup_operation(id)? {
                    Reconciliation::Committed(outcome) => Ok(outcome),
                    Reconciliation::Absent => Err(StorageError::Conflict),
                };
            }
            let outcome = effect(tx)?;
            // Outcomes are metadata receipts, not prompt/tool bodies. Bound the
            // stored/decoded receipt; callers must choose compact typed output.
            if outcome.to_string().len() > 1024 * 1024 { return Err(StorageError::InvalidArguments); }
            tx.check()?;
            tx.driver.execute("UPDATE semon_pg_operations SET outcome = $3 WHERE namespace = $1 AND operation_id = $2",
                &[&namespace.as_str(), &id.operation_id, &outcome]).map_err(sqlstate::error)?;
            Ok(outcome)
        })
    }
}
