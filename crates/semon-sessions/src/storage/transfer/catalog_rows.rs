//! Fixed logical Catalog records carried by the ordinary transfer checkpoint.
//! These records do not grant a publication ACK, reader lease or ownership.
use super::*;
pub(crate) const IMPORTED_GENERATION: &str = "native_transfer_v1:catalog-content-generation";

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
    ];
    pub(crate) fn category(self) -> Category {
        match self {
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
            Self::Slot
            | Self::HistorySlot
            | Self::TraceTurn
            | Self::TraceEdge
            | Self::SearchDocument
            | Self::SearchCount => &[0, 1],
            Self::TraceBookmark => &[0, 1, 2],
            _ => &[0],
        }
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
        Current | History => {
            let (key, metadata): (String, String) = serde_json::from_value(v).map_err(bad)?;
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
        SearchCount => {
            let _: (String, String, u64) = serde_json::from_value(v).map_err(bad)?;
        }
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
