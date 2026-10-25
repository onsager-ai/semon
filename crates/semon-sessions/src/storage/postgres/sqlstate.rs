use crate::storage::StorageError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure { Storage(StorageError), Reconcile }

/// The one SQLSTATE policy table. No database diagnostics/credentials escape.
pub(super) fn classify(code: Option<&str>, committing: bool, operation_key: bool) -> Failure {
    if committing {
        return Failure::Storage(StorageError::CommitUncertain);
    }
    match code {
        Some("23505") if operation_key => Failure::Reconcile,
        Some("40001" | "40P01" | "55P03" | "57014") => Failure::Storage(StorageError::Busy),
        _ => Failure::Storage(StorageError::Unavailable),
    }
}

pub(super) fn error(error: ::postgres::Error) -> StorageError {
    match classify(error.code().map(|code| code.code()), false, false) {
        Failure::Storage(error) => error,
        Failure::Reconcile => StorageError::Conflict,
    }
}
