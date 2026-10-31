//! Fixed logical Catalog records carried by the ordinary transfer checkpoint.
//! These records do not grant a publication ACK, reader lease or ownership.
use super::*;
pub(crate) const IMPORTED_GENERATION: &str = "native_transfer_v1:catalog-content-generation";

// ProofGeneration has the largest column count (ten). Ten 1 MiB cells cover
// its bounded metadata/recipe/slots and typed coordinates; Presentation instead
// has two keys and an 8 MiB cell. This is a whole-row transfer bound, not a new
// per-cell limit: unbounded/unsupported original-custody shapes refuse intact.
// JSON escapes each input byte to at most six bytes (e.g. NUL -> \u0000).
// The fixed typed envelope, numeric cells and punctuation fit in 4 KiB.
const MAX_CATALOG_CELLS: usize = 10;
pub(crate) const MAX_CATALOG_ROW_BYTES: usize = MAX_CATALOG_CELLS * MAX_PAGE_BYTES * 6 + 4096;
pub(crate) const CATALOG_ROW_FRAGMENT_BYTES: usize = 64 * 1024;

#[cfg(test)]
pub(crate) mod tests;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogRowData {
    pub(crate) generation: String,
    pub(crate) kind: CatalogRowKind,
    pub(crate) values: Vec<serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureCursor {
    pub(crate) key: Vec<serde_json::Value>,
    pub(crate) next_part: u32,
    pub(crate) parts: u32,
    pub(crate) total_bytes: u64,
    pub(crate) row_digest: [u8; 32],
}

impl CaptureCursor {
    pub(crate) fn pending(&self) -> bool {
        self.next_part < self.parts
    }
}

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
        snapshot: _,
        generation,
        kind,
        key,
        part,
        parts,
        total_bytes,
        row_digest: _,
        bytes,
    } = record
    else {
        return Err(StorageError::InvalidArguments);
    };
    let total_bytes = usize::try_from(*total_bytes).map_err(|_| StorageError::ScopeChanged)?;
    if generation.len() != 64
        || !generation.bytes().all(|b| b.is_ascii_hexdigit())
        || key.len() != kind.keys().len()
        || total_bytes == 0
        || total_bytes > MAX_CATALOG_ROW_BYTES
        || *parts == 0
        || usize::try_from(*parts).ok() != Some(total_bytes.div_ceil(CATALOG_ROW_FRAGMENT_BYTES))
        || *part >= *parts
    {
        return Err(StorageError::ScopeChanged);
    }
    let expected_len = (total_bytes - (*part as usize * CATALOG_ROW_FRAGMENT_BYTES))
        .min(CATALOG_ROW_FRAGMENT_BYTES);
    if bytes.len() != expected_len {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(crate) fn validate_row(row: &CatalogRowData) -> Result<(), StorageError> {
    let CatalogRowData {
        generation,
        kind,
        values,
    } = row;
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

pub(crate) fn capture_cursor(value: Option<&str>) -> Result<Option<CaptureCursor>, StorageError> {
    let cursor: Option<CaptureCursor> = value
        .map(|text| serde_json::from_str(text).map_err(|_| StorageError::ScopeChanged))
        .transpose()?;
    if cursor.as_ref().is_some_and(|cursor| {
        cursor.parts == 0
            || cursor.next_part == 0
            || cursor.next_part > cursor.parts
            || cursor.total_bytes == 0
            || cursor.total_bytes > MAX_CATALOG_ROW_BYTES as u64
            || cursor.parts as u64
                != cursor
                    .total_bytes
                    .div_ceil(CATALOG_ROW_FRAGMENT_BYTES as u64)
    }) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(cursor)
}

pub(crate) fn resume_fragments(
    mut fragments: Vec<TransferRecord>,
    cursor: Option<&CaptureCursor>,
) -> Result<Vec<TransferRecord>, StorageError> {
    if let Some(cursor) = cursor.filter(|cursor| cursor.pending()) {
        let Some(TransferRecord::CatalogRow {
            key,
            parts,
            total_bytes,
            row_digest,
            ..
        }) = fragments.first()
        else {
            return Err(StorageError::ScopeChanged);
        };
        if cursor.key != *key
            || cursor.parts != *parts
            || cursor.total_bytes != *total_bytes
            || cursor.row_digest != *row_digest
            || cursor.next_part as usize >= fragments.len()
        {
            return Err(StorageError::ScopeChanged);
        }
        fragments.drain(..cursor.next_part as usize);
    }
    Ok(fragments)
}

pub(crate) fn row_key(row: &CatalogRowData) -> Result<Vec<serde_json::Value>, StorageError> {
    row.kind
        .keys()
        .iter()
        .map(|index| {
            row.values
                .get(*index)
                .cloned()
                .ok_or(StorageError::ScopeChanged)
        })
        .collect()
}

pub(crate) fn fragments(
    snapshot: [u8; 32],
    row: CatalogRowData,
) -> Result<Vec<TransferRecord>, StorageError> {
    validate_row(&row)?;
    let key = row_key(&row)?;
    let bytes = encode_bounded(&row, MAX_CATALOG_ROW_BYTES)
        .map_err(|_| StorageError::HistoryRetentionBounds)?;
    let total_bytes = bytes.len();
    let row_digest: [u8; 32] = Sha256::digest(&bytes).into();
    let parts = u32::try_from(total_bytes.div_ceil(CATALOG_ROW_FRAGMENT_BYTES))
        .map_err(|_| StorageError::InvalidArguments)?;
    // Capture has always persisted its typed cursor in a bounded metadata
    // envelope. Refuse an uncheckpointable key before cloning it per part.
    encode_bounded(
        &CaptureCursor {
            key: key.clone(),
            next_part: 1,
            parts,
            total_bytes: total_bytes as u64,
            row_digest,
        },
        MAX_MANIFEST_BYTES,
    )?;
    bytes
        .chunks(CATALOG_ROW_FRAGMENT_BYTES)
        .enumerate()
        .map(|(part, bytes)| {
            let record = TransferRecord::CatalogRow {
                snapshot,
                generation: row.generation.clone(),
                kind: row.kind,
                key: key.clone(),
                part: u32::try_from(part).map_err(|_| StorageError::InvalidArguments)?,
                parts,
                total_bytes: u64::try_from(total_bytes)
                    .map_err(|_| StorageError::InvalidArguments)?,
                row_digest,
                bytes: bytes.to_vec(),
            };
            validate(&record)?;
            Ok(record)
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentState {
    snapshot: [u8; 32],
    generation: String,
    kind: CatalogRowKind,
    key_digest: [u8; 32],
    parts: u32,
    total_bytes: u64,
    row_digest: [u8; 32],
    next_part: u32,
}

fn fragment_token(
    snapshot: &[u8; 32],
    generation: &str,
    kind: CatalogRowKind,
    key: &[serde_json::Value],
) -> Result<String, StorageError> {
    let bytes = serde_json::to_vec(&(snapshot, generation, kind, key))
        .map_err(|_| StorageError::InvalidArguments)?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn key_digest(key: &[serde_json::Value]) -> Result<[u8; 32], StorageError> {
    let bytes = encode_bounded(&key, MAX_PAGE_BYTES)?;
    Ok(Sha256::digest(bytes).into())
}

pub(crate) fn apply_fragment(
    tx: &mut impl InboxTransaction,
    record: &TransferRecord,
    expected_snapshot: &[u8; 32],
) -> Result<Option<CatalogRowData>, StorageError> {
    validate(record)?;
    let TransferRecord::CatalogRow {
        snapshot,
        generation,
        kind,
        key,
        part,
        parts,
        total_bytes,
        row_digest,
        bytes,
    } = record
    else {
        return Err(StorageError::InvalidArguments);
    };
    if snapshot != expected_snapshot {
        return Err(StorageError::Conflict);
    }
    let token = fragment_token(snapshot, generation, *kind, key)?;
    let key_digest = key_digest(key)?;
    // One logical row at a time: neither interleaving another row nor finishing
    // a manifest may hide an unfinished row in this snapshot's inbox.
    let descriptor_key = pending_key(snapshot);
    let part_key = |index| super::key(snapshot, &format!("catalog-row:{token}:part:{index:08}"));
    let mut state = match tx.get(&descriptor_key, MAX_MANIFEST_BYTES)? {
        Some(value) => {
            let state: FragmentState =
                serde_json::from_slice(&value).map_err(|_| StorageError::ScopeChanged)?;
            if state.snapshot != *snapshot
                || state.generation != *generation
                || state.kind != *kind
                || state.key_digest != key_digest
                || state.parts != *parts
                || state.total_bytes != *total_bytes
                || state.row_digest != *row_digest
            {
                return Err(StorageError::Conflict);
            }
            state
        }
        None => {
            if *part != 0 {
                return Err(StorageError::Conflict);
            }
            FragmentState {
                snapshot: *snapshot,
                generation: generation.clone(),
                kind: *kind,
                key_digest,
                parts: *parts,
                total_bytes: *total_bytes,
                row_digest: *row_digest,
                next_part: 0,
            }
        }
    };
    if state.next_part != *part || tx.get(&part_key(*part), MAX_PAGE_BYTES)?.is_some() {
        return Err(StorageError::Conflict);
    }
    let encoded_part = serde_json::to_vec(bytes).map_err(|_| StorageError::InvalidArguments)?;
    tx.put(&part_key(*part), &encoded_part)?;
    state.next_part += 1;
    tx.put(
        &descriptor_key,
        &encode_bounded(&state, MAX_MANIFEST_BYTES)?,
    )?;
    if state.next_part != state.parts {
        return Ok(None);
    }

    let total_bytes = usize::try_from(state.total_bytes).map_err(|_| StorageError::ScopeChanged)?;
    if total_bytes > MAX_CATALOG_ROW_BYTES {
        return Err(StorageError::ScopeChanged);
    }
    let mut joined = Vec::new();
    joined
        .try_reserve_exact(total_bytes)
        .map_err(|_| StorageError::Busy)?;
    for index in 0..state.parts {
        let bytes = tx
            .get(&part_key(index), MAX_PAGE_BYTES)?
            .ok_or(StorageError::ScopeChanged)?;
        let fragment: Vec<u8> =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        let expected_len = (total_bytes - (index as usize * CATALOG_ROW_FRAGMENT_BYTES))
            .min(CATALOG_ROW_FRAGMENT_BYTES);
        if fragment.len() != expected_len
            || joined
                .len()
                .checked_add(fragment.len())
                .is_none_or(|length| length > total_bytes)
        {
            return Err(StorageError::ScopeChanged);
        }
        joined.extend_from_slice(&fragment);
    }
    let joined_digest: [u8; 32] = Sha256::digest(&joined).into();
    if joined.len() != total_bytes || joined_digest != state.row_digest {
        return Err(StorageError::ScopeChanged);
    }
    let row: CatalogRowData =
        serde_json::from_slice(&joined).map_err(|_| StorageError::ScopeChanged)?;
    validate_row(&row)?;
    if row.generation != state.generation
        || row.kind != state.kind
        || self::key_digest(&row_key(&row)?)? != state.key_digest
    {
        return Err(StorageError::ScopeChanged);
    }
    tx.delete(&descriptor_key)?;
    for index in 0..state.parts {
        tx.delete(&part_key(index))?;
    }
    Ok(Some(row))
}

fn pending_key(snapshot: &[u8; 32]) -> String {
    super::key(snapshot, "catalog-row:pending")
}

pub(crate) fn require_complete(
    tx: &mut impl InboxTransaction,
    snapshot: &[u8; 32],
) -> Result<(), StorageError> {
    if tx
        .get(&pending_key(snapshot), MAX_MANIFEST_BYTES)?
        .is_some()
    {
        return Err(StorageError::Conflict);
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
        if fitted.len() == MAX_PAGE_ROWS {
            break;
        }
        validate(&record)?;
        if let TransferRecord::CatalogRow { snapshot, .. } = &record
            && *snapshot != state.manifest.identity()
        {
            return Err(StorageError::ScopeChanged);
        }
        let size = encode_bounded(&record, MAX_PAGE_BYTES)?.len() + 1;
        if bytes.checked_add(size).is_none_or(|n| n > MAX_PAGE_BYTES) {
            break;
        }
        bytes += size;
        fitted.push(record);
    }
    let Some(TransferRecord::CatalogRow {
        kind,
        key,
        part,
        parts,
        total_bytes,
        row_digest,
        ..
    }) = fitted.last()
    else {
        return Err(StorageError::Busy);
    };
    let category = kind.category();
    state.native_key = Some(
        serde_json::to_string(&CaptureCursor {
            key: key.clone(),
            next_part: *part + 1,
            parts: *parts,
            total_bytes: *total_bytes,
            row_digest: *row_digest,
        })
        .map_err(|_| StorageError::ScopeChanged)?,
    );
    reported::append_capture(tx, state, category, fitted)
}

#[cfg(test)]
mod row_validation_tests {
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

    fn row_record(kind: CatalogRowKind, metadata: String, recipe: Option<&str>) -> CatalogRowData {
        CatalogRowData {
            generation: "a".repeat(64),
            kind,
            values: vec![
                serde_json::json!("key"),
                serde_json::json!(metadata),
                serde_json::json!(recipe),
            ],
        }
    }

    fn proof_generation_record(metadata: String, recipe: Option<&str>) -> CatalogRowData {
        CatalogRowData {
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
    fn current_transfer225_recipe_cells_preserve_nullable_bytes_and_combined_bound() {
        let recipe = " \t{\"opaque\":\"漢字\"}\0\n ";
        for kind in [CatalogRowKind::Current, CatalogRowKind::History] {
            let record = row_record(kind, metadata(String::new()), Some(recipe));
            validate_row(&record).unwrap();
            let encoded = serde_json::to_vec(&record).unwrap();
            let decoded: CatalogRowData = serde_json::from_slice(&encoded).unwrap();
            validate_row(&decoded).unwrap();
            let CatalogRowData { values, .. } = decoded;
            assert_eq!(values[2], serde_json::json!(recipe));

            let null = row_record(kind, metadata(String::new()), None);
            validate_row(&null).unwrap();
            let CatalogRowData { values, .. } = null;
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
        validate_row(&exact).unwrap();
        let expected = exact.values.clone();
        let parts = fragments([11; 32], exact).unwrap();
        assert!(parts.len() > 1);
        let mut inbox = tests::MemoryInbox::default();
        let mut result = None;
        for part in parts {
            validate(&part).unwrap();
            result = apply_fragment(&mut inbox, &part, &[11; 32]).unwrap();
        }
        assert_eq!(result.unwrap().values, expected);

        let over = row_record(
            CatalogRowKind::Current,
            metadata("x".repeat(exact_name_len + 1)),
            Some("q"),
        );
        assert!(validate_row(&over).is_err());

        let proof_recipe = " {\"proof\": \"exact 汉字\"}\n";
        let proof = proof_generation_record(metadata(String::new()), Some(proof_recipe));
        validate_row(&proof).unwrap();
        let decoded: CatalogRowData =
            serde_json::from_slice(&serde_json::to_vec(&proof).unwrap()).unwrap();
        let CatalogRowData { values, .. } = decoded;
        assert_eq!(values[7], serde_json::json!(proof_recipe));
        validate_row(&proof_generation_record(metadata(String::new()), None)).unwrap();
    }

    #[test]
    fn current_transfer225_proof_metadata_and_slots_exact_independent_bounds() {
        let mut metadata = metadata(String::new());
        metadata.push_str(&" ".repeat(MAX_PAGE_BYTES - metadata.len()));
        let slots = format!("[]{}", " ".repeat(MAX_PAGE_BYTES - 2));
        let mut row = proof_generation_record(metadata, None);
        row.values[8] = slots.clone().into();
        let expected = row.values.clone();
        let parts = fragments([12; 32], row).unwrap();
        assert!(parts.len() > 1);
        let mut inbox = tests::MemoryInbox::default();
        let mut result = None;
        for part in parts {
            validate(&part).unwrap();
            result = apply_fragment(&mut inbox, &part, &[12; 32]).unwrap();
        }
        assert_eq!(result.unwrap().values, expected);
    }
}
