//! Semantic native Catalog snapshot boundary. No driver or SQL escapes here.
use crate::{
    catalog::Request,
    model::summary::{CatalogRow, CatalogSource},
};
use serde_json::Value;
use std::io;

pub(crate) type OriginalHeader = (String, u32, Option<String>, i64);

#[derive(serde::Serialize, serde::Deserialize)]
struct HistoryTransfer {
    witness: String,
    last: Option<i64>,
    copied: usize,
    partial: i64,
    complete: bool,
}

/// Typed original-recipe custody operations, inside the caller's fenced write
/// transaction. Adapters own all statements and namespace predicates.
pub(crate) trait OriginalHistoryStore {
    type Error;
    fn data(error: serde_json::Error) -> Self::Error;
    fn retained(&self, key: &str) -> Result<bool, Self::Error>;
    fn replacement(&self, row: &CatalogRow) -> Result<Option<CatalogRow>, Self::Error>;
    fn header(&self, key: &str) -> Result<Option<OriginalHeader>, Self::Error>;
    fn transfer(&self, key: &str) -> Result<Option<String>, Self::Error>;
    fn reset_transfer(&self, key: &str) -> Result<(), Self::Error>;
    fn next_slot(&self, key: &str, last: Option<i64>) -> Result<Option<(i64, i64)>, Self::Error>;
    fn copy_slot(&self, key: &str, slot: i64, offset: i64, take: i64, split: bool, inspect: bool)
    -> Result<Option<String>, Self::Error>;
    fn more_slots(&self, key: &str, last: Option<i64>) -> Result<bool, Self::Error>;
    fn checkpoint(&self, key: &str, transfer: &str) -> Result<(), Self::Error>;
    fn complete(
        &self,
        row: &CatalogRow,
        metadata: &str,
        original_header: &str,
        header: Option<&OriginalHeader>,
        complete: bool,
    ) -> Result<(), Self::Error>;
}

/// One shared bounded original-custody algorithm. Current remains the owner
/// until all original recipe bytes have been copied and the header is visible.
pub(crate) fn preserve_original<P: OriginalHistoryStore>(
    store: &P,
    old: &CatalogRow,
) -> Result<bool, P::Error> {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    if store.retained(&old.key)? {
        return Ok(true);
    }
    let mut history = old.clone();
    history.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
    let header = store.header(&old.key)?;
    let metadata = serde_json::to_string(&history).map_err(P::data)?;
    let original_header = serde_json::to_string(&header).map_err(P::data)?;
    let witness = format!(
        "{:x}",
        Sha256::digest(serde_json::to_string(&(&metadata, &original_header)).map_err(P::data)?.as_bytes())
    );
    let saved = store.transfer(&old.key)?;
    let prior = saved.as_deref().map(serde_json::from_str::<HistoryTransfer>).transpose().map_err(P::data)?;
    if prior.as_ref().is_some_and(|prior| prior.witness != witness) {
        store.reset_transfer(&old.key)?;
        return Ok(false);
    }
    let mut transfer = prior.unwrap_or_else(|| HistoryTransfer {
        witness,
        last: None,
        copied: 0,
        partial: 0,
        complete: header.as_ref().is_some_and(|(_, version, recorded, total)| {
            *version == crate::slot_projection::VERSION && *total >= 0
                && !old.sources.is_empty() && old.sources.len() <= 64
                && recorded.is_some()
                && crate::retention::source_generation(&old.sources).ok().as_ref() == recorded.as_ref()
        }),
    });
    let paths: BTreeSet<_> = old.sources.iter().map(|source| &source.path).collect();
    let mut bytes = 0usize;
    let mut processed = 0;
    while processed < 64 && bytes < 1048576 {
        let Some((slot, length)) = store.next_slot(&old.key, transfer.last)? else { break };
        let offset = transfer.partial;
        let take = length.saturating_sub(offset).min((1048576 - bytes) as i64);
        let split = length > 1048576;
        if !split && length as usize > 1048576 - bytes {
            break;
        }
        let text = store.copy_slot(&old.key, slot, offset, take, split, transfer.complete)?;
        if split {
            transfer.complete = false;
        }
        bytes += take as usize;
        processed += 1;
        if offset + take != length {
            transfer.partial += take;
            break;
        }
        transfer.partial = 0;
        transfer.complete &= usize::try_from(slot).ok() == Some(transfer.copied);
        if transfer.complete {
            transfer.complete = text.as_deref().and_then(|text| serde_json::from_str::<crate::slot_projection::SavedSlot>(text).ok()).is_some_and(|slot| slot.source_paths().iter().all(|path| paths.contains(*path)));
        }
        transfer.last = Some(slot);
        transfer.copied += 1;
    }
    if store.more_slots(&old.key, transfer.last)? {
        store.checkpoint(&old.key, &serde_json::to_string(&transfer).map_err(P::data)?)?;
        return Ok(false);
    }
    transfer.complete &= header.as_ref().is_some_and(|(_, _, _, total)| usize::try_from(*total).ok() == Some(transfer.copied));
    store.complete(old, &metadata, &original_header, header.as_ref(), transfer.complete)?;
    Ok(true)
}

/// Admission policy chooses a replacement; custody and its bounded progress are
/// shared. At most one original owner advances per publication attempt.
pub(crate) fn preserve_replacement_rows<P: OriginalHistoryStore>(
    store: &P,
    rows: &[CatalogRow],
) -> Result<bool, P::Error> {
    let mut advanced = false;
    for row in rows {
        if store.retained(&row.key)? {
            continue;
        }
        if let Some(old) = store.replacement(row)? {
            if advanced || !preserve_original(store, &old)? {
                return Ok(false);
            }
            advanced = true;
        }
    }
    Ok(true)
}

pub(crate) struct SlotPublicationHeader<'a> {
    pub(crate) key: &'a str,
    pub(crate) generation: String,
    pub(crate) source_generation: String,
    pub(crate) total: i64,
}

pub(crate) trait SlotPublicationStore {
    type Error;
    fn data(error: serde_json::Error) -> Self::Error;
    fn capacity() -> Self::Error;
    fn header(&self, header: &SlotPublicationHeader<'_>) -> Result<bool, Self::Error>;
    fn replace(&self, key: &str, metadata: &[String]) -> Result<(), Self::Error>;
}

/// The native publisher's recipe, order and conditional replacement are shared
/// by both adapters. No body reads or parser/Query derivation enter this port.
pub(crate) fn publish_slot_rows<P: SlotPublicationStore>(
    store: &P,
    rows: &[CatalogRow],
    projections: &[crate::slot_projection::Projection],
) -> Result<(), P::Error> {
    let rows: std::collections::BTreeMap<_, _> = rows.iter().map(|row| (row.key.as_str(), row)).collect();
    for projection in projections {
        let Some(row) = rows.get(projection.key.as_str()) else { continue };
        let metadata = projection.slots.iter().map(serde_json::to_string).collect::<Result<Vec<_>, _>>().map_err(P::data)?;
        let header = SlotPublicationHeader {
            key: &projection.key,
            source_generation: crate::retention::source_generation(&row.sources).map_err(P::data)?,
            generation: crate::slot_projection::generation(&metadata, &row.sources).map_err(P::data)?,
            total: i64::try_from(metadata.len()).map_err(|_| P::capacity())?,
        };
        if store.header(&header)? || projection.repair {
            store.replace(&projection.key, &metadata)?;
        }
    }
    Ok(())
}

/// One component-publication recipe, shared by the legacy and admitted writers.
pub(crate) fn component_generation(
    version: &str,
    previous: Option<&str>,
    epoch: &str,
    metadata: &[String],
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(version.as_bytes());
    digest.update(previous.unwrap_or("").as_bytes());
    digest.update(epoch.as_bytes());
    for value in metadata {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
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
