//! Backend-neutral, body-free values for immutable proof publications.
//!
//! Physical stores own transactions, SQL dialects, row identifiers and RLS.
//! This module owns only the stable value representation and its validation.
use super::{StorageError, proof_lifetime::ProofGenerationId};
use crate::model::summary::CatalogRow;

pub(crate) const TTL_MAX: u32 = 60_000;
pub(crate) const LEASE_CAP: i64 = 64;
pub(crate) const METADATA_MAX: usize = 1_048_576;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresentationManifest {
    pub(crate) catalog_generation: String,
    pub(crate) entries: Vec<(String, String)>,
    pub(crate) observations: Option<crate::storage::selected::PresentationObservations>,
}

/// One authenticated root, retained only for the lifetime of a Catalog snapshot.
/// Every row still authenticates its own metadata, slots and presentation entry.
#[derive(Default)]
pub(crate) struct PresentationCache {
    root: Option<CachedPresentation>,
}

struct CachedPresentation {
    base: String,
    manifest: String,
    digest: String,
    decoded: PresentationManifest,
}

impl PresentationCache {
    /// Coordinates of a root authenticated in this same Catalog snapshot.
    pub(crate) fn coordinates(&self) -> Option<(String, String)> {
        self.root
            .as_ref()
            .map(|root| (root.base.clone(), root.digest.clone()))
    }

    pub(crate) fn cached_observations(
        &self,
        record: &GenerationRecord,
        presentation: &str,
        base: &str,
    ) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
        let root = self
            .root
            .as_ref()
            .filter(|root| root.base == base && root.digest == record.proof)
            .ok_or(StorageError::ScopeChanged)?;
        verify_decoded_presentation_entry(
            &record.session_key,
            &record.metadata,
            &record.slots,
            presentation,
            &root.decoded,
        )?;
        Ok(root.decoded.observations.clone())
    }

    pub(crate) fn observations(
        &mut self,
        record: &GenerationRecord,
        presentation: &str,
        base: &str,
        manifest: &str,
    ) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
        if self.root.as_ref().is_none_or(|root| {
            root.base != base || root.manifest != manifest || root.digest != record.proof
        }) {
            let decoded = decode_presentation_manifest(manifest)?;
            if presentation_digest(base, manifest)? != record.proof {
                return Err(StorageError::ScopeChanged);
            }
            self.root = Some(CachedPresentation {
                base: base.to_owned(),
                manifest: manifest.to_owned(),
                digest: record.proof.clone(),
                decoded,
            });
        }
        self.cached_observations(record, presentation, base)
    }
}

pub(crate) fn decode_presentation_manifest(
    manifest: &str,
) -> Result<PresentationManifest, StorageError> {
    let manifest: PresentationManifest =
        serde_json::from_str(manifest).map_err(|_| StorageError::Unavailable)?;
    validate_catalog_generation(&manifest.catalog_generation)?;
    Ok(manifest)
}

pub(crate) fn validate_catalog_generation(generation: &str) -> Result<(), StorageError> {
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(crate) fn verify_presentation_entry(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    manifest: &str,
) -> Result<(), StorageError> {
    verify_presentation_manifest_entry(key, metadata, slots, text, manifest).map(|_| ())
}

pub(crate) fn verify_presentation_observations(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    base: &str,
    manifest: &str,
    expected_digest: &str,
) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
    let decoded = decode_presentation_manifest(manifest)?;
    if presentation_digest(base, manifest)? != expected_digest {
        return Err(StorageError::ScopeChanged);
    }
    verify_decoded_presentation_entry(key, metadata, slots, text, &decoded)?;
    Ok(decoded.observations)
}

fn verify_presentation_manifest_entry(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    manifest: &str,
) -> Result<PresentationManifest, StorageError> {
    let manifest = decode_presentation_manifest(manifest)?;
    verify_decoded_presentation_entry(key, metadata, slots, text, &manifest)?;
    Ok(manifest)
}

fn verify_decoded_presentation_entry(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    manifest: &PresentationManifest,
) -> Result<(), StorageError> {
    let entry = presentation_entry(metadata, slots, text)?;
    if manifest.entries.iter().filter(|(k, _)| k == key).count() != 1
        || !manifest
            .entries
            .iter()
            .any(|(k, digest)| k == key && *digest == entry)
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

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

pub(crate) fn validate_query_recipe_cell(
    metadata: &str,
    query_recipe: Option<&str>,
) -> Result<(), StorageError> {
    if metadata.len() + query_recipe.map_or(0, str::len) > METADATA_MAX {
        return Err(StorageError::HistoryRetentionBounds);
    }
    Ok(())
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn presentation_digest(base: &str, manifest: &str) -> Result<String, StorageError> {
    Ok(digest(
        &serde_json::to_vec(&("selected-presentation-v3", base, manifest))
            .map_err(|_| StorageError::Unavailable)?,
    ))
}

pub(crate) fn verify_presentation_digest(
    base: &str,
    manifest: &str,
    expected: &str,
) -> Result<(), StorageError> {
    decode_presentation_manifest(manifest)?;
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

/// The adapter has checked the original receipt/presentation and the current
/// destination's import grant. A row cannot select an arbitrary old admission.
pub(crate) fn observation_binding(
    record: GenerationRecord,
    current: &super::QueryRuntimeBinding,
    selected: &CatalogRow,
) -> Result<super::QueryRuntimeBinding, StorageError> {
    let id = record.id.clone();
    let admission = record.admission;
    let proof = record.proof.clone();
    let publication = record.publication.clone();
    let prefix = record.prefix.clone();
    let (record, original) = decode_generation(
        record,
        ExpectedGeneration {
            namespace: current.namespace.as_str(),
            id: &id,
            admission,
            stream: &current.stream_epoch,
            proof: &proof,
            publication: &publication,
            prefix: &prefix,
        },
    )?;
    let mut selected = selected.clone();
    // Catalog's retained view may retag the immutable original row.
    if selected.lifecycle != original.lifecycle
        && !(original.lifecycle == crate::model::summary::CatalogLifecycle::Current
            && selected.lifecycle == crate::model::summary::CatalogLifecycle::Retained)
    {
        return Err(StorageError::ScopeChanged);
    }
    selected.lifecycle = original.lifecycle;
    if record.admission == 0
        || serde_json::to_value(selected).map_err(|_| StorageError::Unavailable)?
            != serde_json::to_value(original).map_err(|_| StorageError::Unavailable)?
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(super::QueryRuntimeBinding {
        namespace: current.namespace.clone(),
        admission: record.admission,
        stream_epoch: current.stream_epoch.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> GenerationRecord {
        let row = CatalogRow {
            overview: None,
            trace: None,
            query_recipe: None,
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

    #[test]
    fn codec89_observation_binding_keeps_original_coordinates_and_exact_row() {
        let record = fixture();
        let row: CatalogRow = serde_json::from_str(&record.metadata).unwrap();
        let current = super::super::QueryRuntimeBinding {
            namespace: super::super::LogicalSourceNamespace::new("source-a").unwrap(),
            admission: 2,
            stream_epoch: "stream".into(),
        };
        assert_eq!(
            observation_binding(record.clone(), &current, &row)
                .unwrap()
                .admission,
            1
        );
        let mut retained = row.clone();
        retained.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        assert!(observation_binding(record.clone(), &current, &retained).is_ok());
        retained.name = "forged".into();
        assert!(observation_binding(record.clone(), &current, &retained).is_err());
        let mut retired_record = record;
        let mut retired_row = row.clone();
        retired_row.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        retired_record.metadata = serde_json::to_string(&retired_row).unwrap();
        assert!(observation_binding(retired_record, &current, &row).is_err());
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
        // Adjacent native pricing values must keep the same exact owner digest
        // after storage decoding, rather than acquiring a neighbouring float.
        for base in [0.0001_f64, 0.01, 0.1, 1.0] {
            for offset in 0..64 {
                let cost = f64::from_bits(base.to_bits() + offset);
                let mut original = row.clone();
                original.cost.usd = Some(cost);
                let mut priced = record.clone();
                priced.metadata = serde_json::to_string(&original).unwrap();
                let (_, restored) = decode(priced.clone()).unwrap();
                assert_eq!(restored.cost.usd.unwrap().to_bits(), cost.to_bits());
                assert_eq!(
                    digest(serde_json::to_string(&restored).unwrap().as_bytes()),
                    digest(priced.metadata.as_bytes())
                );
            }
        }
    }

    fn oversized(mut json: String) -> String {
        // Keep valid typed JSON so only the byte limit can reject this case.
        json.push_str(&" ".repeat(METADATA_MAX + 1 - json.len()));
        json
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
        oversized(fixture().metadata)
    );
    refuses!(codec89_malformed_slots, slots, "{".into());
    refuses!(codec89_oversized_slots, slots, oversized("[]".into()));
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
    fn cached_presentation_rechecks_each_entry_and_exact_root() {
        let mut first = fixture();
        let mut second = first.clone();
        second.session_key = "other".into();
        let entry = presentation_entry(&first.metadata, &first.slots, "null").unwrap();
        let mut manifest = serde_json::json!({
            "catalog_generation": "a".repeat(64),
            "entries": [[first.session_key, entry], [second.session_key, entry]],
            "observations": null,
        });
        let encoded = manifest.to_string();
        first.proof = presentation_digest("base", &encoded).unwrap();
        second.proof = first.proof.clone();
        let mut cache = PresentationCache::default();
        assert!(cache.cached_observations(&first, "null", "base").is_err());
        assert!(
            cache
                .observations(&first, "null", "base", &encoded)
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .observations(&second, "null", "base", &encoded)
                .unwrap()
                .is_none()
        );

        assert!(
            cache
                .cached_observations(&second, "null", "base")
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .cached_observations(&second, "null", "other-base")
                .is_err()
        );
        assert!(cache.cached_observations(&second, "{}", "base").is_err());
        let mut changed = second.clone();
        changed.metadata.push(' ');
        assert!(cache.cached_observations(&changed, "null", "base").is_err());
        assert!(
            cache
                .observations(&changed, "null", "base", &encoded)
                .is_err()
        );
        changed = second.clone();
        changed.slots = "[1]".into();
        assert!(
            cache
                .observations(&changed, "null", "base", &encoded)
                .is_err()
        );
        assert!(cache.observations(&second, "{}", "base", &encoded).is_err());
        changed = second.clone();
        changed.session_key = "unlisted".into();
        assert!(
            cache
                .observations(&changed, "null", "base", &encoded)
                .is_err()
        );
        changed = second.clone();
        changed.proof = "0".repeat(64);
        assert!(
            cache
                .observations(&changed, "null", "base", &encoded)
                .is_err()
        );
        assert!(
            cache
                .observations(&second, "null", "other-base", &encoded)
                .is_err()
        );

        manifest["catalog_generation"] = serde_json::json!("b".repeat(64));
        let replacement = manifest.to_string();
        assert!(
            cache
                .observations(&second, "null", "base", &replacement)
                .is_err()
        );
        second.proof = presentation_digest("base", &replacement).unwrap();
        assert!(
            cache
                .observations(&second, "null", "base", &replacement)
                .is_ok()
        );
        manifest["entries"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!([second.session_key, entry]));
        let duplicate = manifest.to_string();
        second.proof = presentation_digest("base", &duplicate).unwrap();
        assert!(
            cache
                .observations(&second, "null", "base", &duplicate)
                .is_err()
        );
        manifest["catalog_generation"] = serde_json::json!("invalid");
        let invalid = manifest.to_string();
        second.proof = presentation_digest("base", &invalid).unwrap();
        assert!(
            cache
                .observations(&second, "null", "base", &invalid)
                .is_err()
        );
    }

    #[test]
    fn codec89_digest_mismatch() {
        let manifest = serde_json::json!({
            "catalog_generation": "a".repeat(64),
            "entries": [],
            "observations": null,
        })
        .to_string();
        let proof = presentation_digest("base", &manifest).unwrap();
        assert!(verify_presentation_digest("base", &manifest, &proof).is_ok());
        assert!(verify_presentation_digest("Base", &manifest, &proof).is_err());
        assert!(verify_presentation_digest("base", "Manifest", &proof).is_err());
    }

    #[test]
    fn presentation_manifest_requires_a_valid_catalog_generation() {
        for manifest in [
            serde_json::json!({"entries": [], "observations": null}).to_string(),
            serde_json::json!({
                "catalog_generation": "not-a-generation",
                "entries": [],
                "observations": null,
            })
            .to_string(),
        ] {
            let proof = presentation_digest("base", &manifest).unwrap();
            assert!(verify_presentation_digest("base", &manifest, &proof).is_err());
        }
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
