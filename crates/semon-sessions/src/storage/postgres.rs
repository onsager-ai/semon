//! Optional PostgreSQL resource/transaction foundation, not a selectable native
//! session backend. #479 still needs the shared #478 ingest/read/publication ports.
//! Credentials, connect deadlines and TLS (including cancellation TLS) belong to
//! the authorized host. This module never reads environment credentials.
mod cancel;
mod operations;
mod pool;
mod schema;
mod sqlstate;
mod txn;

pub use cancel::PgCancellation;
pub use operations::{OperationIdentity, Reconciliation};
pub use pool::{PgConnection, PgConnectionFactory, PgPool, PoolLimits};

#[cfg(test)]
mod tests;
