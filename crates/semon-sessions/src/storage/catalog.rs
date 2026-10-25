//! Semantic native Catalog snapshot boundary. No driver or SQL escapes here.
use crate::{
    catalog::Request,
    model::summary::{CatalogRow, CatalogSource},
};
use serde_json::Value;
use std::io;

/// One component-publication recipe, shared by the legacy and admitted writers.
pub(crate) fn component_generation(version: &str, previous: Option<&str>, epoch: &str, metadata: &[String]) -> String {
    use sha2::{Digest,Sha256};
    let mut digest=Sha256::new();
    digest.update(version.as_bytes());
    digest.update(previous.unwrap_or("").as_bytes());
    digest.update(epoch.as_bytes());
    for value in metadata { digest.update((value.len() as u64).to_le_bytes()); digest.update(value.as_bytes()); }
    format!("{:x}",digest.finalize())
}

pub(crate) struct SourceProofObservation {
    pub(crate) key: String,
    pub(crate) source: CatalogSource,
    pub(crate) generation: String,
}

/// Publication head operations share the native publisher's surrounding write
/// transaction. Row derivation, source/claim checks and ACK ordering stay in
/// that publisher until their whole dependency set is namespace-qualified.
pub(crate) trait CatalogPublicationHead {
    fn base_generation(&self) -> Result<Option<String>, super::StorageError>;
    fn publish_generation(
        &self,
        generation: &str,
        version: &str,
    ) -> Result<(), super::StorageError>;
}

pub(crate) trait CatalogSnapshot {
    fn require_current(&self) -> io::Result<()>;
    fn publication_value(&self, key: &str) -> io::Result<Option<String>>;
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>>;
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection>;
    fn has_history(&self, key: &str) -> io::Result<bool>;
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>>;
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool>;
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow) -> Value,
    ) -> io::Result<Option<Value>>;
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<Value>>;
}
