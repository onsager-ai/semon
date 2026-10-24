//! Hosts must authorize each Raw path before ingestion. No bytes are modified.
use super::StorageError;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventReceipt {
    pub operation_id: String,
    pub consumed_bytes: u64,
    /// Observation revision, not a worker lease or an acknowledged Catalog claim.
    pub observed_revision: Option<i64>,
}

pub trait NativeEventStore: Send {
    /// Caller retains a 64-character hex operation ID across uncertain replies.
    /// Reusing that ID reconciles its durable completion before any Raw I/O.
    /// Reuses the native parser and identity/projection algorithm. A successful
    /// return requires a durable, reconciled Event/ledger completion.
    fn ingest(&mut self, operation_id: &str, path: &Path, harness: &str) -> Result<EventReceipt, StorageError>;
}
