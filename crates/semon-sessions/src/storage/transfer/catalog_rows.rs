//! Fixed logical Catalog records carried by the ordinary transfer checkpoint.
//! These records do not grant a publication ACK, reader lease or ownership.
use super::*;
pub(crate) const IMPORTED_GENERATION: &str = "native_transfer_v1:catalog-content-generation";

// Producer coordinates remain in the immutable receipt. These local records
// bind that receipt to the destination import; application alone grants nothing.
pub(crate) fn bind_imported_publication(
    tx: &mut impl InboxTransaction,
    generation: &str,
    kind: CatalogRowKind,
    values: &[serde_json::Value],
) -> Result<(), StorageError> {
    if kind == CatalogRowKind::PublicationReceipt {
        let publication = values[0].as_str().ok_or(StorageError::ScopeChanged)?;
        tx.put(
            &format!("native_transfer_v1:publication:{publication}"),
            generation.as_bytes(),
        )?;
    }
    Ok(())
}

pub(crate) fn admit_imported_publications(
    tx: &mut impl InboxTransaction,
    generation: &str,
    admission: u64,
    stream: &str,
) -> Result<(), StorageError> {
    tx.put(
        &format!("native_transfer_v1:catalog-owner:{generation}"),
        format!("{admission}:{stream}").as_bytes(),
    )
}

pub(crate) fn bind_imported_generation(
    tx: &mut impl InboxTransaction,
    generation: &str,
) -> Result<(), StorageError> {
    if let Some(old) = tx.get(IMPORTED_GENERATION, 64)? {
        if old != generation.as_bytes() {
            return Err(StorageError::Conflict);
        }
    } else {
        tx.put(IMPORTED_GENERATION, generation.as_bytes())?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogRowKind {
    Current,
    Recipe,
    Slot,
    History,
    HistoryRecipe,
    HistorySlot,
    HistoryOriginal,
    TraceHeader,
    TraceTurn,
    TraceEdge,
    TraceBookmark,
    SearchDocument,
    SearchCount,
    Dependency,
    Description,
    HistoryProgress,
    PublicationReceipt,
    PresentationRoot,
    ProofGeneration,
    Presentation,
    ProofSource,
    ProofRef,
}

impl CatalogRowKind {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Current,
        Self::Recipe,
        Self::Slot,
        Self::History,
        Self::HistoryRecipe,
        Self::HistorySlot,
        Self::HistoryOriginal,
        Self::TraceHeader,
        Self::TraceTurn,
        Self::TraceEdge,
        Self::TraceBookmark,
        Self::SearchDocument,
        Self::SearchCount,
        Self::Dependency,
        Self::Description,
        Self::HistoryProgress,
        Self::PublicationReceipt,
        Self::PresentationRoot,
        Self::ProofGeneration,
        Self::Presentation,
        Self::ProofSource,
        Self::ProofRef,
    ];
    pub(crate) fn category(self) -> Category {
        match self {
            Self::PublicationReceipt => Category::Operations,
            Self::PresentationRoot | Self::ProofRef => Category::ProofReferences,
            Self::ProofGeneration | Self::ProofSource => Category::Proofs,
            Self::Presentation => Category::Metadata,
            Self::Dependency | Self::Description => Category::Metadata,
            Self::HistoryProgress => Category::CatalogHistory,
            Self::Current => Category::CatalogCurrent,
            Self::History | Self::HistoryOriginal => Category::CatalogHistory,
            Self::Recipe | Self::HistoryRecipe => Category::Recipes,
            Self::Slot | Self::HistorySlot => Category::Slots,
            Self::SearchDocument | Self::SearchCount => Category::Search,
            _ => Category::Trace,
        }
    }
    pub(crate) fn keys(self) -> &'static [usize] {
        match self {
            Self::ProofGeneration | Self::Presentation => &[0, 1],
            Self::ProofSource | Self::ProofRef => &[0, 1, 2],
            Self::PublicationReceipt => &[0],
            Self::PresentationRoot => &[3],
            Self::Slot
            | Self::HistorySlot
            | Self::TraceTurn
            | Self::TraceEdge
            | Self::SearchDocument
            | Self::SearchCount => &[0, 1],
            Self::TraceBookmark | Self::Dependency => &[0, 1, 2],
            _ => &[0],
        }
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn namespace_owned(self) -> bool {
        matches!(
            self,
            Self::Description
                | Self::PublicationReceipt
                | Self::PresentationRoot
                | Self::ProofGeneration
                | Self::Presentation
                | Self::ProofSource
                | Self::ProofRef
        )
    }
}

pub(crate) fn validate(record: &TransferRecord) -> Result<(), StorageError> {
    let TransferRecord::CatalogRow {
        generation,
        kind,
        values,
    } = record
    else {
        return Err(StorageError::InvalidArguments);
    };
    if generation.len() != 64 || !generation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StorageError::ScopeChanged);
    }
    let v = serde_json::Value::Array(values.clone());
    let bad = |_| StorageError::ScopeChanged;
    use CatalogRowKind::*;
    match kind {
        PublicationReceipt => {
            let (publication, epoch, admission, base, revision, proof, claims, metadata): (
                String,
                i64,
                i64,
                Option<String>,
                i64,
                String,
                String,
                String,
            ) = serde_json::from_value(v).map_err(bad)?;
            if epoch <= 0
                || admission <= 0
                || revision < 0
                || !hex_digest(&publication)
                || !hex_digest(&proof)
                || base.as_deref().is_some_and(|s| !hex_digest(s))
            {
                return Err(StorageError::ScopeChanged);
            }
            let _: Vec<(String, Option<i64>)> = serde_json::from_str(&claims).map_err(bad)?;
            let _: Vec<String> = serde_json::from_str(&metadata).map_err(bad)?;
        }
        PresentationRoot => {
            let (publication, base, manifest, digest): (String, String, String, String) =
                serde_json::from_value(v).map_err(bad)?;
            if !hex_digest(&publication) || !hex_digest(&base) || manifest.len() > MAX_PAGE_BYTES {
                return Err(StorageError::ScopeChanged);
            }
            super::super::proof_codec::verify_presentation_digest(&base, &manifest, &digest)?;
        }
        ProofGeneration => {
            let (_, _, publication, proof, admission, _, metadata, query_recipe, slots, _): (
                String,
                String,
                String,
                String,
                i64,
                String,
                String,
                Option<String>,
                String,
                String,
            ) = serde_json::from_value(v).map_err(bad)?;
            if !hex_digest(&publication) || !hex_digest(&proof) || admission <= 0 {
                return Err(StorageError::ScopeChanged);
            }
            super::super::proof_codec::validate_query_recipe_cell(
                &metadata,
                query_recipe.as_deref(),
            )?;
            let _: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).map_err(bad)?;
            let _: Option<Vec<crate::slot_projection::SavedSlot>> =
                serde_json::from_str(&slots).map_err(bad)?;
        }
        Presentation => {
            let (_, _, metadata): (String, String, String) =
                serde_json::from_value(v).map_err(bad)?;
            let _: serde_json::Value = serde_json::from_str(&metadata).map_err(bad)?;
        }
        ProofSource => {
            let _: (String, String, String, String) = serde_json::from_value(v).map_err(bad)?;
        }
        ProofRef => {
            let (_, kind, _): (String, String, String) = serde_json::from_value(v).map_err(bad)?;
            if !["current", "history"].contains(&kind.as_str()) {
                return Err(StorageError::ScopeChanged);
            }
        }
        Current | History => {
            let (key, metadata, query_recipe): (String, String, Option<String>) =
                serde_json::from_value(v).map_err(bad)?;
            super::super::proof_codec::validate_query_recipe_cell(
                &metadata,
                query_recipe.as_deref(),
            )?;
            let row: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).map_err(bad)?;
            if row.key != key || row.sources.len() > 64 {
                return Err(StorageError::ScopeChanged);
            }
            if *kind == Current && row.lifecycle != crate::model::summary::CatalogLifecycle::Current
            {
                return Err(StorageError::ScopeChanged);
            }
        }
        HistoryOriginal => {
            let (_, metadata): (String, String) = serde_json::from_value(v).map_err(bad)?;
            let _: Option<crate::storage::catalog::OriginalHeader> =
                serde_json::from_str(&metadata).map_err(bad)?;
        }
        Recipe => {
            let (_, _, _, version, total): (String, String, Option<String>, u32, u64) =
                serde_json::from_value(v).map_err(bad)?;
            if version != crate::slot_projection::VERSION || total > i64::MAX as u64 {
                return Err(StorageError::ScopeChanged);
            }
        }
        HistoryRecipe => {
            let (_, _, _, _, total, _): (String, String, String, u32, u64, bool) =
                serde_json::from_value(v).map_err(bad)?;
            if total > i64::MAX as u64 {
                return Err(StorageError::ScopeChanged);
            }
        }
        Slot | HistorySlot => {
            let (_, _, metadata): (String, i64, String) = serde_json::from_value(v).map_err(bad)?;
            let _: crate::slot_projection::SavedSlot =
                serde_json::from_str(&metadata).map_err(bad)?;
        }
        TraceHeader => {
            let _: (String, String, String, String) = serde_json::from_value(v).map_err(bad)?;
        }
        TraceTurn => {
            let _: (String, String, Option<String>, String) =
                serde_json::from_value(v).map_err(bad)?;
        }
        TraceEdge => {
            let _: (String, String, String) = serde_json::from_value(v).map_err(bad)?;
        }
        TraceBookmark => {
            let _: (String, String, String) = serde_json::from_value(v).map_err(bad)?;
        }
        SearchDocument => {
            let _: (String, String, String, bool) = serde_json::from_value(v).map_err(bad)?;
        }
        Dependency => {
            let _: (String, String, String) = serde_json::from_value(v).map_err(bad)?;
        }
        Description => {
            let _: (String, u32, String, String) = serde_json::from_value(v).map_err(bad)?;
        }
        HistoryProgress => {
            let (_, metadata): (String, String) = serde_json::from_value(v).map_err(bad)?;
            let _: serde_json::Value = serde_json::from_str(&metadata).map_err(bad)?;
        }
        SearchCount => {
            let _: (String, String, u64) = serde_json::from_value(v).map_err(bad)?;
        }
    }
    Ok(())
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn proof_generation(
    namespace: &str,
    values: &[serde_json::Value],
) -> Result<super::super::proof_codec::GenerationRecord, StorageError> {
    use super::super::{
        proof_codec::{self, ExpectedGeneration, GenerationRecord},
        proof_lifetime::ProofGenerationId,
    };
    let (
        id,
        session_key,
        publication,
        proof,
        admission,
        stream,
        metadata,
        query_recipe,
        slots,
        prefix,
    ): (
        String,
        String,
        String,
        String,
        u64,
        String,
        String,
        Option<String>,
        String,
        String,
    ) = serde_json::from_value(serde_json::Value::Array(values.to_vec()))
        .map_err(|_| StorageError::ScopeChanged)?;
    proof_codec::validate_query_recipe_cell(&metadata, query_recipe.as_deref())?;
    let id = ProofGenerationId(id);
    let record = GenerationRecord {
        namespace: namespace.to_owned(),
        id: id.clone(),
        session_key,
        publication: publication.clone(),
        proof: proof.clone(),
        admission,
        stream: stream.clone(),
        metadata,
        slots,
        prefix: prefix.clone(),
    };
    let (record, _) = proof_codec::decode_generation(
        record,
        ExpectedGeneration {
            namespace,
            id: &id,
            admission,
            stream: &stream,
            proof: &proof,
            publication: &publication,
            prefix: &prefix,
        },
    )?;
    Ok(record)
}

pub(crate) fn verify_source_edge(
    metadata: &str,
    path: &str,
    ledger: &str,
) -> Result<(), StorageError> {
    let row: crate::model::summary::CatalogRow =
        serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
    if !row.sources.iter().any(|source| {
        source.path.to_str() == Some(path)
            && crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(source))
                == ledger
    }) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(crate) fn append(
    tx: &mut impl InboxTransaction,
    state: &mut reported::CaptureState,
    records: Vec<TransferRecord>,
) -> Result<(), StorageError> {
    let mut fitted = Vec::new();
    let mut bytes = 4096usize;
    for record in records {
        validate(&record)?;
        let size = encode_bounded(&record, MAX_PAGE_BYTES)?.len() + 1;
        if bytes.checked_add(size).is_none_or(|n| n > MAX_PAGE_BYTES) {
            break;
        }
        bytes += size;
        fitted.push(record);
    }
    let Some(TransferRecord::CatalogRow { kind, values, .. }) = fitted.last() else {
        return Err(StorageError::Busy);
    };
    let cursor = kind
        .keys()
        .iter()
        .map(|i| values[*i].clone())
        .collect::<Vec<_>>();
    let category = kind.category();
    state.native_key =
        Some(serde_json::to_string(&cursor).map_err(|_| StorageError::ScopeChanged)?);
    reported::append_capture(tx, state, category, fitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(name: String) -> String {
        let row = crate::model::summary::CatalogRow {
            overview: None,
            trace: None,
            query_recipe: None,
            query: None,
            lifecycle: Default::default(),
            key: "key".into(),
            name,
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
        serde_json::to_string(&row).unwrap()
    }

    fn row_record(kind: CatalogRowKind, metadata: String, recipe: Option<&str>) -> TransferRecord {
        TransferRecord::CatalogRow {
            generation: "a".repeat(64),
            kind,
            values: vec![
                serde_json::json!("key"),
                serde_json::json!(metadata),
                serde_json::json!(recipe),
            ],
        }
    }

    fn proof_generation_record(metadata: String, recipe: Option<&str>) -> TransferRecord {
        TransferRecord::CatalogRow {
            generation: "a".repeat(64),
            kind: CatalogRowKind::ProofGeneration,
            values: vec![
                serde_json::json!("f".repeat(64)),
                serde_json::json!("key"),
                serde_json::json!("b".repeat(64)),
                serde_json::json!("c".repeat(64)),
                serde_json::json!(1),
                serde_json::json!("d".repeat(64)),
                serde_json::json!(metadata),
                serde_json::json!(recipe),
                serde_json::json!("[]"),
                serde_json::json!("e".repeat(64)),
            ],
        }
    }

    #[test]
    fn recipe_cells_preserve_nullable_bytes_and_combined_bound() {
        let recipe = " \t{\"opaque\":\"漢字\"}\0\n ";
        for kind in [CatalogRowKind::Current, CatalogRowKind::History] {
            let record = row_record(kind, metadata(String::new()), Some(recipe));
            validate(&record).unwrap();
            let encoded = serde_json::to_vec(&record).unwrap();
            let decoded: TransferRecord = serde_json::from_slice(&encoded).unwrap();
            validate(&decoded).unwrap();
            let TransferRecord::CatalogRow { values, .. } = decoded else {
                unreachable!()
            };
            assert_eq!(values[2], serde_json::json!(recipe));

            let null = row_record(kind, metadata(String::new()), None);
            validate(&null).unwrap();
            let TransferRecord::CatalogRow { values, .. } = null else {
                unreachable!()
            };
            assert_eq!(values[2], serde_json::Value::Null);
        }

        let empty_name = metadata(String::new());
        let recipe_len = 1;
        let exact_name_len =
            crate::storage::proof_codec::METADATA_MAX - empty_name.len() - recipe_len;
        let exact = row_record(
            CatalogRowKind::Current,
            metadata("x".repeat(exact_name_len)),
            Some("q"),
        );
        validate(&exact).unwrap();

        let over = row_record(
            CatalogRowKind::Current,
            metadata("x".repeat(exact_name_len + 1)),
            Some("q"),
        );
        assert!(validate(&over).is_err());

        let proof_recipe = " {\"proof\": \"exact 汉字\"}\n";
        let proof = proof_generation_record(metadata(String::new()), Some(proof_recipe));
        validate(&proof).unwrap();
        let decoded: TransferRecord =
            serde_json::from_slice(&serde_json::to_vec(&proof).unwrap()).unwrap();
        let TransferRecord::CatalogRow { values, .. } = decoded else {
            unreachable!()
        };
        assert_eq!(values[7], serde_json::json!(proof_recipe));
        validate(&proof_generation_record(metadata(String::new()), None)).unwrap();
    }
}
