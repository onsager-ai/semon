//! Backend-neutral, body-free values for immutable proof publications.
//!
//! Physical stores own transactions, SQL dialects, row identifiers and RLS.
//! This module owns only the stable value representation and its validation.
use super::{StorageError, proof_lifetime::ProofGenerationId};
use crate::model::summary::CatalogRow;

pub(crate) const TTL_MAX: u32 = 60_000;
pub(crate) const LEASE_CAP: i64 = 64;
pub(crate) const METADATA_MAX: usize = 1_048_576;

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

pub(crate) fn verify_presentation_digest(
    base: &str,
    manifest: &str,
    expected: &str,
) -> Result<(), StorageError> {
    if presentation_digest(base, manifest)? != expected {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(crate) fn presentation_entry(
    metadata: &str,
    slots: &str,
    text: &str,
) -> Result<String, StorageError> {
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
    pub(crate) publication: &'a str,
    pub(crate) prefix: &'a str,
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
        || record.publication != expected.publication
        || record.prefix != expected.prefix
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

    fn fixture() -> GenerationRecord {
        let row = CatalogRow {
            overview: None,
            trace: None,
            query: None,
            lifecycle: Default::default(),
            key: "key".into(),
            name: "session".into(),
            harness: "codex".into(),
            kind: "session".into(),
            presentation_kind: None,
            native_ids: vec!["native".into()],
            parent: None,
            parent_source: None,
            repo: None,
            branch: None,
            model: "model".into(),
            effort: None,
            start: None,
            last: None,
            tokens: [0.0; 3],
            cost: Default::default(),
            sources: vec![],
        };
        GenerationRecord {
            namespace: "source-a".into(),
            id: ProofGenerationId::derive("source-a", 1, "stream", "proof"),
            session_key: row.key.clone(),
            publication: "publication".into(),
            proof: "proof".into(),
            admission: 1,
            stream: "stream".into(),
            metadata: serde_json::to_string(&row).unwrap(),
            slots: "[]".into(),
            prefix: crate::retention::source_generation(&row.sources).unwrap(),
        }
    }

    fn decode(record: GenerationRecord) -> Result<(GenerationRecord, CatalogRow), StorageError> {
        let trusted = fixture();
        decode_generation(
            record,
            ExpectedGeneration {
                namespace: &trusted.namespace,
                id: &trusted.id,
                admission: trusted.admission,
                stream: &trusted.stream,
                proof: &trusted.proof,
                publication: &trusted.publication,
                prefix: &trusted.prefix,
            },
        )
    }

    #[test]
    fn codec89_generation_round_trip() {
        let record = fixture();
        let (decoded, row) = decode(record.clone()).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(serde_json::to_string(&row).unwrap(), record.metadata);
    }

    macro_rules! refuses {
        ($name:ident, $field:ident, $value:expr) => {
            #[test]
            fn $name() {
                let mut record = fixture();
                record.$field = $value;
                assert!(decode(record).is_err());
            }
        };
    }
    refuses!(codec89_wrong_namespace, namespace, "foreign".into());
    refuses!(codec89_wrong_admission, admission, 2);
    refuses!(codec89_wrong_stream, stream, "other".into());
    refuses!(codec89_wrong_proof, proof, "other".into());
    refuses!(
        codec89_wrong_generation,
        id,
        ProofGenerationId("other".into())
    );
    refuses!(codec89_wrong_publication, publication, "other".into());
    refuses!(codec89_wrong_prefix, prefix, "other".into());
    refuses!(codec89_wrong_session_key, session_key, "other".into());
    refuses!(codec89_malformed_metadata, metadata, "{".into());
    refuses!(
        codec89_oversized_metadata,
        metadata,
        " ".repeat(METADATA_MAX + 1)
    );
    refuses!(codec89_malformed_slots, slots, "{".into());
    refuses!(codec89_oversized_slots, slots, " ".repeat(METADATA_MAX + 1));
    // Valid JSON is insufficient: every slot must be a native SavedSlot.
    refuses!(codec89_wrong_slot_type, slots, "[{}]".into());

    #[test]
    fn codec89_forged_matching_derive_is_not_authority() {
        let mut record = fixture();
        record.namespace = "foreign".into();
        record.id = ProofGenerationId::derive(
            &record.namespace,
            record.admission,
            &record.stream,
            &record.proof,
        );
        assert!(decode(record).is_err());
    }

    #[test]
    fn codec89_derive_mismatch_with_matching_expected_id() {
        let mut record = fixture();
        record.id = ProofGenerationId("forged".into());
        let trusted = record.clone();
        assert!(
            decode_generation(
                record,
                ExpectedGeneration {
                    namespace: &trusted.namespace,
                    id: &trusted.id,
                    admission: trusted.admission,
                    stream: &trusted.stream,
                    proof: &trusted.proof,
                    publication: &trusted.publication,
                    prefix: &trusted.prefix,
                }
            )
            .is_err()
        );
    }

    #[test]
    fn codec89_existing_metadata_and_slots_limits_are_inclusive() {
        let mut record = fixture();
        record
            .metadata
            .push_str(&" ".repeat(METADATA_MAX - record.metadata.len()));
        record
            .slots
            .push_str(&" ".repeat(METADATA_MAX - record.slots.len()));
        assert!(decode(record).is_ok());
    }

    #[test]
    fn presentation_digest_is_bound_to_both_values() {
        assert_ne!(
            presentation_digest("a", "b").unwrap(),
            presentation_digest("a", "c").unwrap()
        );
    }

    #[test]
    fn codec89_digest_mismatch() {
        let proof = presentation_digest("base", "manifest").unwrap();
        assert!(verify_presentation_digest("base", "manifest", &proof).is_ok());
        assert!(verify_presentation_digest("Base", "manifest", &proof).is_err());
        assert!(verify_presentation_digest("base", "Manifest", &proof).is_err());
    }

    #[test]
    fn codec89_publisher_digest_adds_no_reader_caps() {
        // The writer accepted these bytes before codec extraction. Bounds
        // remain at the existing persisted-reader and capture guards.
        let metadata = "x".repeat(METADATA_MAX + 1);
        assert_eq!(
            presentation_entry(&metadata, "[]", "null").unwrap(),
            digest(&serde_json::to_vec(&(&metadata, "[]", "null")).unwrap())
        );
    }
}
