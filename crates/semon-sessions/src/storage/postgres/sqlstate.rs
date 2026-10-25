use crate::storage::StorageError;

/// Operation-key conflicts use ON CONFLICT. Unexpected uniqueness violations
/// (23505) mean unavailable storage, never a valid receipt or replay permission.
pub(super) fn classify(code: Option<&str>, committing: bool) -> StorageError {
    if committing {
        return StorageError::CommitUncertain;
    }
    match code {
        Some("40001" | "40P01" | "55P03" | "57014") => StorageError::Busy,
        _ => StorageError::Unavailable,
    }
}

pub(super) fn error(error: super::pool::DriverError) -> StorageError {
    match error {
        super::pool::DriverError::Pg(error) => {
            #[cfg(test)]
            eprintln!("PG_DIAGNOSTIC sqlstate={}", error.code().map_or("none", |code| code.code()));
            classify(error.code().map(|code| code.code()), false)
        }
        super::pool::DriverError::Interrupted => StorageError::Busy,
    }
}
