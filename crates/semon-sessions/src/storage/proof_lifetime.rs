//! Private generation-bound selected reads. SQL never grants Raw access.
#![allow(dead_code)] // Public qualification stays closed pending CI and parity.
use super::{LogicalSourceNamespace, StorageError};
use crate::model::summary::CatalogRow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProofGenerationId(pub(crate) String);
impl ProofGenerationId {
    pub(crate) fn derive(namespace: &str, admission: u64, stream: &str, proof: &str) -> Self {
        use sha2::{Digest, Sha256};
        let payload = serde_json::to_vec(&(
            "semon-proof-generation-v1",
            namespace,
            admission,
            stream,
            proof,
        ))
        .expect("generation tuple serializes");
        Self(format!("{:x}", Sha256::digest(payload)))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReaderSnapshot {
    pub(crate) namespace: LogicalSourceNamespace,
    pub(crate) admission: u64,
    pub(crate) stream_epoch: String,
    pub(crate) publication_generation: String,
    pub(crate) proof_generation: ProofGenerationId,
    pub(crate) consumed_prefix: String,
}

/// A private native recipe page carries the exact pinned response proof root.
/// Presentation continues to use the existing SavedSlot/record algorithms.
pub(crate) struct ReaderPage {
    pub(crate) snapshot: ReaderSnapshot,
    pub(crate) records: Vec<(crate::slot_projection::SavedSlot, Option<serde_json::Value>)>,
}

/// Host retains its own source authority; this port contains metadata only.
pub(crate) trait ProofReaderLease {
    fn snapshot(&self) -> &ReaderSnapshot;
    fn renew(&mut self, ttl_ms: u32) -> Result<(), StorageError>;
    fn release(&mut self) -> Result<(), StorageError>;
    fn read_record(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        path: &std::path::Path,
        offset: u64,
    ) -> std::io::Result<Option<serde_json::Value>> {
        self.read_record_with_control(
            options,
            provider,
            path,
            offset,
            &crate::SelectedReadControl::default(),
        )
    }
    fn read_record_with_control(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        path: &std::path::Path,
        offset: u64,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<Option<serde_json::Value>>;
    fn transcript_records(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        start: usize,
        limit: usize,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<ReaderPage>;
    fn row(&self) -> &CatalogRow;
}
