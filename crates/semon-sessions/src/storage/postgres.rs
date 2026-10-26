//! Optional PostgreSQL resource/transaction foundation, not a selectable native
//! session backend. #479 still needs the shared #478 ingest/read/publication ports.
//! Credentials, connect deadlines and TLS (including cancellation TLS) belong to
//! the authorized host. This module never reads environment credentials.
mod cancel;
mod catalog_effects;
mod native_bundle;
mod native_commit;
mod native_core;
mod native_pages;
mod native_usage;
mod native_usage_identity;
mod operations;
mod pool;
mod reported_facts;
mod schema;
mod source_journal;
mod sqlstate;
mod store_metadata;
pub(crate) mod tree_checkpoint;
mod tree_writer;
mod txn;

pub use cancel::PgCancellation;
pub use operations::{OperationIdentity, Reconciliation};
pub use pool::{ConnectFuture, PgConnection, PgConnectionFactory, PgPool, PoolLimits};
pub use source_journal::PgSourceJournal;
pub use store_metadata::PgStoreMetadata;

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
