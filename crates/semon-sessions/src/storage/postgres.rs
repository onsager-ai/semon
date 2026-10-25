//! Optional PostgreSQL resource/transaction foundation, not a selectable native
//! session backend. #479 still needs the shared #478 ingest/read/publication ports.
//! Credentials, connect deadlines and TLS (including cancellation TLS) belong to
//! the authorized host. This module never reads environment credentials.
mod cancel;
mod native_usage_identity;
mod operations;
mod pool;
mod schema;
mod source_journal;
mod sqlstate;
mod txn;

pub use cancel::PgCancellation;
pub use operations::{OperationIdentity, Reconciliation};
pub use pool::{ConnectFuture, PgConnection, PgConnectionFactory, PgPool, PoolLimits};
pub use source_journal::PgSourceJournal;

/// Lossless GUC transport for the common opaque UTF-8 bytes, including NUL.
fn namespace_context(namespace: &super::LogicalSourceNamespace) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(namespace.as_str().len() * 2);
    for byte in namespace.as_str().bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proxy_test;
