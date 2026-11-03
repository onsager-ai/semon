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
    pub(crate) contexts: std::collections::BTreeSet<String>,
    pub(crate) observations: Option<crate::storage::selected::PresentationObservations>,
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
    saved: (&str, &PresentationContexts),
) -> Result<(), StorageError> {
    verify_presentation_manifest_entry(key, metadata, slots, text, saved).map(|_| ())
}

pub(crate) fn verify_presentation_observations(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    base: &str,
    saved: (&str, &PresentationContexts),
    expected_digest: &str,
) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
    verify_presentation_digest(base, saved.0, expected_digest)?;
    Ok(verify_presentation_manifest_entry(key, metadata, slots, text, saved)?.observations)
}

fn verify_presentation_manifest_entry(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    saved: (&str, &PresentationContexts),
) -> Result<PresentationManifest, StorageError> {
    let manifest = decode_presentation_manifest(saved.0)?;
    verify_presentation_contexts(&manifest, saved.1)?;
    let entry = presentation_entry(metadata, slots, text)?;
    if manifest.entries.iter().filter(|(k, _)| k == key).count() != 1
        || !manifest
            .entries
            .iter()
            .any(|(k, digest)| k == key && *digest == entry)
    {
        return Err(StorageError::ScopeChanged);
    }
    resolve_presentation(text, saved.1)?;
    Ok(manifest)
}

/// Page admission checks the row binding and reference membership without
/// materializing a publication-sized store. Final admission resolves every row.
pub(crate) fn verify_presentation_references(
    key: &str,
    metadata: &str,
    slots: &str,
    text: &str,
    manifest: &str,
) -> Result<(), StorageError> {
    let manifest = decode_presentation_manifest(manifest)?;
    let entry = presentation_entry(metadata, slots, text)?;
    if manifest.entries.iter().filter(|(k, _)| k == key).count() != 1
        || !manifest.entries.iter().any(|(k, digest)| k == key && *digest == entry)
    {
        return Err(StorageError::ScopeChanged);
    }
    let witness: Option<serde_json::Value> =
        serde_json::from_str(text).map_err(|_| StorageError::ScopeChanged)?;
    if let Some(witness) = witness {
        let context = witness["context"].as_object().ok_or(StorageError::ScopeChanged)?;
        for values in context.values() {
            for reference in values.as_object().ok_or(StorageError::ScopeChanged)?.values() {
                if !manifest.contexts.contains(reference.as_str().ok_or(StorageError::ScopeChanged)?) {
                    return Err(StorageError::ScopeChanged);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn decode_presentation_context(
    reference: &str,
    text: &str,
) -> Result<(String, String, serde_json::Value), StorageError> {
    if text.len() > METADATA_MAX {
        return Err(StorageError::HistoryRetentionBounds);
    }
    if digest(text.as_bytes()) != reference {
        return Err(StorageError::ScopeChanged);
    }
    let entry = serde_json::from_str(text).map_err(|_| StorageError::ScopeChanged)?;
    // One canonical encoding binds the exact family, lookup key and saved value.
    if serde_json::to_string(&entry).map_err(|_| StorageError::Unavailable)? != text {
        return Err(StorageError::ScopeChanged);
    }
    Ok(entry)
}

pub(crate) fn verify_presentation_contexts(
    manifest: &PresentationManifest,
    contexts: &PresentationContexts,
) -> Result<(), StorageError> {
    if manifest.contexts.len() != contexts.len()
        || !manifest.contexts.iter().eq(contexts.keys())
    {
        return Err(StorageError::ScopeChanged);
    }
    let mut bytes = if contexts.is_empty() { 2 } else { 1 };
    for (reference, entry) in contexts {
        let text = serde_json::to_string(entry).map_err(|_| StorageError::Unavailable)?;
        decode_presentation_context(reference, &text)?;
        bytes += reference.len() + 4 + text.len();
        if bytes > 8 * METADATA_MAX {
            return Err(StorageError::HistoryRetentionBounds);
        }
    }
    Ok(())
}

/// Typed lookup identity includes the exact saved key and value, including null.
pub(crate) type PresentationContexts =
    std::collections::BTreeMap<String, (String, String, serde_json::Value)>;

#[derive(serde::Serialize)]
pub(crate) struct CapturedPresentations {
    pub(crate) rows: Vec<(String, String)>,
    pub(crate) contexts: PresentationContexts,
}

#[derive(Default)]
pub(crate) struct PresentationCapture {
    contexts: PresentationContexts,
    rows: Vec<(String, String)>,
    row_bytes: usize,
    context_bytes: usize,
}

impl PresentationCapture {
    pub(crate) fn push(
        &mut self,
        key: String,
        witness: crate::storage::selected::PresentationWitness,
    ) -> Result<(), StorageError> {
        let mut witness = serde_json::to_value(witness).map_err(|_| StorageError::Unavailable)?;
        let context = witness["context"]
            .as_object_mut()
            .ok_or(StorageError::Unavailable)?;
        for (kind, values) in context {
            let values = values.as_object_mut().ok_or(StorageError::Unavailable)?;
            for (key, value) in values {
                let entry = (kind.clone(), key.clone(), value.clone());
                let encoded = serde_json::to_vec(&entry).map_err(|_| StorageError::Unavailable)?;
                if encoded.len() > METADATA_MAX {
                    return Err(StorageError::HistoryRetentionBounds);
                }
                let digest = digest(&encoded);
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    self.contexts.entry(digest.clone())
                {
                    // Quoted digest, colon and comma. Closing brace replaces
                    // the final comma; include the opening brace below.
                    self.context_bytes += digest.len() + 4 + encoded.len();
                    slot.insert(entry);
                }
                *value = serde_json::Value::String(digest);
            }
        }
        let text = serde_json::to_string(&witness).map_err(|_| StorageError::Unavailable)?;
        self.row_bytes += text.len();
        self.rows.push((key, text));
        let store_bytes = if self.contexts.is_empty() {
            2
        } else {
            self.context_bytes + 1
        };
        if self.row_bytes + store_bytes > 8 * 1048576 {
            return Err(StorageError::HistoryRetentionBounds);
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> CapturedPresentations {
        CapturedPresentations {
            rows: self.rows,
            contexts: self.contexts,
        }
    }
}

/// Callers authenticate the original publication root and row entry before
/// resolving saved lookups. Resolution never consults live context.
pub(crate) fn resolve_presentation(
    text: &str,
    contexts: &PresentationContexts,
) -> Result<Option<crate::storage::selected::PresentationWitness>, StorageError> {
    let mut witness: Option<serde_json::Value> =
        serde_json::from_str(text).map_err(|_| StorageError::Unavailable)?;
    if let Some(witness) = witness.as_mut() {
        let context = witness["context"]
            .as_object_mut()
            .ok_or(StorageError::ScopeChanged)?;
        for (kind, values) in context {
            let values = values.as_object_mut().ok_or(StorageError::ScopeChanged)?;
            for (key, value) in values {
                let reference = value.as_str().ok_or(StorageError::ScopeChanged)?;
                let entry = contexts.get(reference).ok_or(StorageError::ScopeChanged)?;
                let encoded = serde_json::to_vec(entry).map_err(|_| StorageError::Unavailable)?;
                if entry.0 != *kind || entry.1 != *key || digest(&encoded) != reference {
                    return Err(StorageError::ScopeChanged);
                }
                *value = entry.2.clone();
            }
        }
    }
    serde_json::from_value(witness.unwrap_or(serde_json::Value::Null))
        .map_err(|_| StorageError::Unavailable)
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
        &serde_json::to_vec(&("selected-presentation-v4", base, manifest))
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
    fn codec89_digest_mismatch() {
        let manifest = serde_json::json!({
            "catalog_generation": "a".repeat(64),
            "entries": [],
            "observations": null,
            "contexts": [],
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
            serde_json::json!({"entries": [], "observations": null, "contexts": []}).to_string(),
            serde_json::json!({
                "catalog_generation": "not-a-generation",
                "entries": [],
                "observations": null,
            "contexts": [],
            })
            .to_string(),
        ] {
            let proof = presentation_digest("base", &manifest).unwrap();
            assert!(verify_presentation_digest("base", &manifest, &proof).is_err());
        }
    }

    fn presentation_fixture() -> crate::storage::selected::PresentationWitness {
        use serde_json::json;
        let key = serde_json::to_string(&("session",)).unwrap();
        let pair = serde_json::to_string(&("session", "turn")).unwrap();
        let mut context =
            serde_json::to_value(crate::storage::selected::CapturedContext::default()).unwrap();
        for (kind, lookup, value) in [
            ("details", pair.as_str(), json!([null, true])),
            ("relative", key.as_str(), json!(null)),
            ("descendants", key.as_str(), json!(["child"])),
            ("header", key.as_str(), json!(["generation", false])),
            ("turn", pair.as_str(), json!("saved turn")),
            ("edge", pair.as_str(), json!("saved edge")),
            ("target_turns", pair.as_str(), json!(["target"])),
            ("participant", key.as_str(), json!("saved participant")),
            ("preview_header", key.as_str(), json!(["generation", null])),
            ("preview_catalog", key.as_str(), json!("saved catalog")),
        ] {
            context[kind][lookup] = value;
        }
        crate::storage::selected::PresentationWitness {
            header_generation: "header".into(),
            version: crate::slot_projection::VERSION,
            total: 0,
            has_history: true,
            catalog: Default::default(),
            context: serde_json::from_value(context).unwrap(),
            relative_statuses: Default::default(),
            observations: Default::default(),
        }
    }

    fn presentation_capture() -> CapturedPresentations {
        let mut capture = PresentationCapture::default();
        capture
            .push("first".into(), presentation_fixture())
            .unwrap();
        capture
            .push("second".into(), presentation_fixture())
            .unwrap();
        capture.finish()
    }

    fn presentation_manifest(captured: &CapturedPresentations) -> String {
        serde_json::to_string(&PresentationManifest {
            catalog_generation: "a".repeat(64),
            entries: captured
                .rows
                .iter()
                .map(|(key, text)| {
                    (
                        key.clone(),
                        presentation_entry("metadata", "[]", text).unwrap(),
                    )
                })
                .collect(),
            contexts: captured.contexts.keys().cloned().collect(),
            observations: None,
        })
        .unwrap()
    }

    #[test]
    fn shared_context_round_trip_preserves_all_lookup_families_and_saved_null() {
        use crate::storage::selected::ContextLookup;
        let captured = presentation_capture();
        assert_eq!(captured.contexts.len(), 10);
        let expected = serde_json::to_value(presentation_fixture()).unwrap();
        for (_, text) in &captured.rows {
            let restored = resolve_presentation(text, &captured.contexts)
                .unwrap()
                .unwrap();
            assert_eq!(serde_json::to_value(&restored).unwrap(), expected);
            assert!(restored.context.relative("session").unwrap().is_none());
            assert!(restored.context.relative("uncaptured").is_err());
        }
        let context_bytes = serde_json::to_vec(&captured.contexts).unwrap().len();
        let mut capture = PresentationCapture::default();
        capture
            .push("first".into(), presentation_fixture())
            .unwrap();
        assert_eq!(capture.context_bytes + 1, context_bytes);
        assert!(
            resolve_presentation("null", &captured.contexts)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn shared_context_rejects_missing_changed_and_retargeted_entries() {
        let captured = presentation_capture();
        let text = &captured.rows[0].1;
        let reference = captured.contexts.keys().next().unwrap().clone();
        let mut missing = captured.contexts.clone();
        missing.remove(&reference);
        assert!(resolve_presentation(text, &missing).is_err());
        for field in 0..3 {
            let mut changed = captured.contexts.clone();
            let entry = changed.get_mut(&reference).unwrap();
            match field {
                0 => entry.0 = "different-family".into(),
                1 => entry.1 = "different-key".into(),
                _ => entry.2 = serde_json::json!("different-value"),
            }
            assert!(resolve_presentation(text, &changed).is_err());
            // Even a self-consistent digest cannot retarget a saved lookup.
            let entry = changed.remove(&reference).unwrap();
            let replacement = digest(&serde_json::to_vec(&entry).unwrap());
            changed.insert(replacement.clone(), entry);
            let retargeted = text.replace(&reference, &replacement);
            if field < 2 {
                assert!(resolve_presentation(&retargeted, &changed).is_err());
            }
        }
    }

    #[test]
    fn shared_context_authentication_binds_store_and_rejects_incomplete_rows() {
        let mut captured = presentation_capture();
        let manifest = presentation_manifest(&captured);
        let proof = presentation_digest("original receipt", &manifest).unwrap();
        let (key, text) = &captured.rows[0];
        assert!(
            verify_presentation_observations(
                key,
                "metadata",
                "[]",
                text,
                "original receipt",
                (&manifest, &captured.contexts),
                &proof,
            )
            .is_ok()
        );
        let mut changed: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        let reference = captured.contexts.keys().next().unwrap().clone();
        changed["contexts"][0] = serde_json::json!("changed");
        assert!(
            verify_presentation_observations(
                key,
                "metadata",
                "[]",
                text,
                "original receipt",
                (&changed.to_string(), &captured.contexts),
                &proof,
            )
            .is_err()
        );
        captured.contexts.remove(&reference);
        // A matching row entry alone does not make an incomplete store valid.
        let incomplete = presentation_manifest(&captured);
        assert!(verify_presentation_entry(key, "metadata", "[]", text, (&incomplete, &captured.contexts)).is_err());
        let mut inline: serde_json::Value = serde_json::from_str(text).unwrap();
        inline["context"]["relative"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .for_each(|value| *value = serde_json::Value::Null);
        assert!(resolve_presentation(&inline.to_string(), &captured.contexts).is_err());
    }

    #[test]
    fn shared_context_records_require_canonical_bytes_and_exact_membership() {
        let captured = presentation_capture();
        let manifest = decode_presentation_manifest(&presentation_manifest(&captured)).unwrap();
        assert!(verify_presentation_contexts(&manifest, &captured.contexts).is_ok());
        let (reference, entry) = captured.contexts.first_key_value().unwrap();
        let canonical = serde_json::to_string(entry).unwrap();
        assert_eq!(decode_presentation_context(reference, &canonical).unwrap(), *entry);
        // Hashing noncanonical bytes must not make them an admissible record.
        let noncanonical = format!(" {canonical}");
        assert!(decode_presentation_context(&digest(noncanonical.as_bytes()), &noncanonical).is_err());
        assert!(decode_presentation_context(reference, &format!("{canonical} ")).is_err());
        let mut missing = captured.contexts.clone();
        missing.remove(reference);
        assert!(verify_presentation_contexts(&manifest, &missing).is_err());
        let mut extra = captured.contexts.clone();
        let entry = ("relative".into(), "uncaptured".into(), serde_json::Value::Null);
        let reference = digest(&serde_json::to_vec(&entry).unwrap());
        extra.insert(reference, entry);
        assert!(verify_presentation_contexts(&manifest, &extra).is_err());
    }

    #[test]
    fn shared_context_record_bound_is_inclusive_and_never_splits_entries() {
        let mut entry = ("turn", "key", String::new());
        let overhead = serde_json::to_string(&entry).unwrap().len();
        entry.2 = "x".repeat(METADATA_MAX - overhead);
        let text = serde_json::to_string(&entry).unwrap();
        assert_eq!(text.len(), METADATA_MAX);
        assert!(decode_presentation_context(&digest(text.as_bytes()), &text).is_ok());
        entry.2.push('x');
        let text = serde_json::to_string(&entry).unwrap();
        assert_eq!(decode_presentation_context(&digest(text.as_bytes()), &text), Err(StorageError::HistoryRetentionBounds));
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
