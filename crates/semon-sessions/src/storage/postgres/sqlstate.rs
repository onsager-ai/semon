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
            {
                use std::error::Error;
                let cause = error.source();
                eprintln!(
                    "PG_DIAGNOSTIC sqlstate={} closed={} wrong_type={} io_kind={:?}",
                    error.code().map_or("none", |code| code.code()),
                    error.is_closed(),
                    cause.is_some_and(|cause| cause.is::<tokio_postgres::types::WrongType>()),
                    cause
                        .and_then(|cause| cause.downcast_ref::<std::io::Error>())
                        .map(std::io::Error::kind),
                );
            }
            classify(error.code().map(|code| code.code()), false)
        }
        super::pool::DriverError::Interrupted => StorageError::Busy,
    }
}
