//! Durable source observation revisions, consumed as publication claims.
//! Recording observations grants no Raw access and does not publish a Catalog.
use super::StorageError;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub type SourceClaims = Vec<(String, Option<i64>)>;

/// Stable identity for one typed source-observation mutation. The namespace is
/// bound into the ID; the digest binds the operation kind and payload. Callers
/// retain this value across retries so an uncertain commit can be reconciled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceOperation {
    pub namespace: String,
    pub id: String,
    pub payload_digest: String,
}

impl SourceOperation {
    /// Capture one operation key. Keep the returned value and pass it again on
    /// every retry; a separate call captures a distinct logical operation.
    /// The payload is encoded by the typed producer, not supplied as SQL.
    pub fn capture(namespace: &str, operation: &str, payload: &[u8]) -> Self {
        let mut payload_hash = Sha256::new();
        payload_hash.update(b"semon-source-observation-payload-v1\0");
        payload_hash.update((operation.len() as u64).to_be_bytes());
        payload_hash.update(operation.as_bytes());
        payload_hash.update((payload.len() as u64).to_be_bytes());
        payload_hash.update(payload);
        let payload_digest = format!("{:x}", payload_hash.finalize());

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let id = format!(
            "source-observation-v1:{}:{:x}:{timestamp:x}:{sequence:x}",
            Self::namespace_tag(namespace),
            std::process::id(),
        );

        Self {
            namespace: namespace.to_owned(),
            id,
            payload_digest,
        }
    }

    pub(crate) fn namespace_tag(namespace: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"semon-source-observation-namespace-v1\0");
        digest.update((namespace.len() as u64).to_be_bytes());
        digest.update(namespace.as_bytes());
        format!("{:x}", digest.finalize())
    }
}

/// Result of committing or reconciling one source operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMutationReceipt {
    Applied,
    AlreadyApplied,
    Conflict,
}

pub trait SourceObservationStore: Send {
    /// Record newly discovered paths atomically with their invalidations. An
    /// exact retry returns `AlreadyApplied`; a reused key with another payload
    /// or namespace returns `Conflict` without changing observations.
    fn discover(
        &mut self,
        operation: &SourceOperation,
        paths: &[String],
        complete: bool,
    ) -> Result<SourceMutationReceipt, StorageError>;
    /// Advance distinct paths only if every captured claim is still current.
    /// A conflict changes no row; this never acknowledges or publishes work.
    fn invalidate(
        &mut self,
        operation: &SourceOperation,
        expected: &[(String, Option<i64>)],
    ) -> Result<SourceMutationReceipt, StorageError>;
    /// Revisions are durable CAS witnesses, not worker leases.
    fn claims(&self, paths: &[String]) -> Result<SourceClaims, StorageError>;
    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError>;
    fn backfill_state(&self) -> Result<Option<String>, StorageError>;
    /// Finish a source backfill only at the captured catalog generation.
    fn finish_backfill(
        &mut self,
        operation: &SourceOperation,
        expected_generation: Option<&str>,
    ) -> Result<SourceMutationReceipt, StorageError>;
}
