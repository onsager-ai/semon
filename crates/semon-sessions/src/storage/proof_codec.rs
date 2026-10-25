//! Backend-neutral, body-free values for immutable proof publications.
//!
//! Physical stores own transactions, SQL dialects, row identifiers and RLS.
//! This module owns only the stable value representation and its validation.
use super::{StorageError, proof_lifetime::ProofGenerationId};
use crate::model::summary::CatalogRow;

pub(crate) const TTL_MAX: u32 = 60_000;
pub(crate) const LEASE_CAP: i64 = 64;
pub(crate) const METADATA_MAX: usize = 1_048_576;
pub(crate) const PRESENTATION_MAX: usize = 8 * 1_048_576;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GenerationRecord {
    pub(crate) namespace: String,
    pub(crate) id: ProofGenerationId,
    pub(crate) session_key: String,
    pub(crate) publication: String,
    pub(crate) proof: String,
    pub(crate) admission: u64,
    pub(crate) stream: String,
    pub(crate) metadata: String,
    pub(crate) slots: String,
    pub(crate) prefix: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefKind {
    Current,
    History,
}
impl RefKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::History => "history",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "current" => Ok(Self::Current),
            "history" => Ok(Self::History),
            _ => Err(StorageError::ScopeChanged),
        }
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn presentation_digest(base: &str, manifest: &str) -> Result<String, StorageError> {
    Ok(digest(
        &serde_json::to_vec(&("selected-presentation-v2", base, manifest))
            .map_err(|_| StorageError::Unavailable)?,
    ))
}

pub(crate) fn presentation_entry(
    metadata: &str,
    slots: &str,
    text: &str,
) -> Result<String, StorageError> {
    if metadata.len() > METADATA_MAX || slots.len() > METADATA_MAX || text.len() > PRESENTATION_MAX
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(digest(
        &serde_json::to_vec(&(metadata, slots, text)).map_err(|_| StorageError::Unavailable)?,
    ))
}

/// Coordinates supplied by the owner transaction, rather than by the record
/// being decoded. A record may prove it is self-consistent, but cannot grant
/// itself a namespace, admission, stream, proof, or generation identity.
pub(crate) struct ExpectedGeneration<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) id: &'a ProofGenerationId,
    pub(crate) admission: u64,
    pub(crate) stream: &'a str,
    pub(crate) proof: &'a str,
}

pub(crate) fn decode_generation(
    record: GenerationRecord,
    expected: ExpectedGeneration<'_>,
) -> Result<(GenerationRecord, CatalogRow), StorageError> {
    if record.namespace != expected.namespace
        || record.id != *expected.id
        || record.admission != expected.admission
        || record.stream != expected.stream
        || record.proof != expected.proof
        || record.metadata.len() > METADATA_MAX
        || record.slots.len() > METADATA_MAX
    {
        return Err(StorageError::ScopeChanged);
    }
    if ProofGenerationId::derive(
        &record.namespace,
        record.admission,
        &record.stream,
        &record.proof,
    ) != record.id
    {
        return Err(StorageError::ScopeChanged);
    }
    let row: CatalogRow =
        serde_json::from_str(&record.metadata).map_err(|_| StorageError::Unavailable)?;
    let _: Option<Vec<crate::slot_projection::SavedSlot>> =
        serde_json::from_str(&record.slots).map_err(|_| StorageError::Unavailable)?;
    if row.key != record.session_key
        || crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::Unavailable)?
            != record.prefix
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok((record, row))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presentation_digest_is_bound_to_both_values() {
        assert_ne!(
            presentation_digest("a", "b").unwrap(),
            presentation_digest("a", "c").unwrap()
        );
        assert!(RefKind::parse("other").is_err());
        assert_eq!(RefKind::History.as_str(), "history");
    }
    #[test]
    fn presentation_entry_refuses_oversized_values() {
        assert!(presentation_entry(&"x".repeat(METADATA_MAX + 1), "[]", "null").is_err());
    }
}
