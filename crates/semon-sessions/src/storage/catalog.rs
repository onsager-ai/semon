//! Semantic native Catalog snapshot boundary. No driver or SQL escapes here.
use crate::{
    catalog::Request,
    model::summary::{CatalogRow, CatalogSource},
};
use serde_json::Value;
use std::io;

pub(crate) type OriginalHeader = (String, u32, Option<String>, i64);

/// Physical trace effects in the publisher's existing fenced transaction.
/// Derivation, digests and ordering remain in the single trace producer.
pub(crate) trait TracePublicationStore {
    type Error;
    fn invalid(error: io::Error) -> Self::Error;
    fn matches_header(
        &self,
        key: &str,
        generation: &str,
        source: &str,
    ) -> Result<bool, Self::Error>;
    fn header(
        &self,
        key: &str,
        generation: &str,
        source: &str,
        metadata: &str,
    ) -> Result<bool, Self::Error>;
    fn clear(&self, key: &str) -> Result<(), Self::Error>;
    fn turn(
        &self,
        key: &str,
        id: Option<&str>,
        start: Option<&str>,
        metadata: &str,
    ) -> Result<(), Self::Error>;
    fn edge(&self, key: &str, id: Option<&str>, metadata: &str) -> Result<(), Self::Error>;
    fn bookmark(&self, key: &str, target: &str, edge: &str) -> Result<(), Self::Error>;
}

/// Current membership and native dependency effects within publication's CAS.
/// Source paths are typed row values, never statements supplied by a caller.
pub(crate) trait CurrentPublicationStore {
    type Error;
    fn current(&self, row: &CatalogRow, metadata: &str) -> Result<bool, Self::Error>;
    fn retire_sources(&self, key: &str) -> Result<(), Self::Error>;
    fn clear_dependencies(&self, source: &CatalogSource) -> Result<(), Self::Error>;
    fn dependency(
        &self,
        source: &CatalogSource,
        harness: &str,
        native: &str,
    ) -> Result<(), Self::Error>;
    fn source(&self, row: &CatalogRow, source: &CatalogSource) -> Result<(), Self::Error>;
}

/// Physical effects for the existing bounded metadata-search cohort algorithm.
pub(crate) trait SearchPublicationStore {
    fn begin(&self, rows: &[&CatalogRow]) -> Result<(), Self::Error>;
    type Error;
    fn data(error: serde_json::Error) -> Self::Error;
    fn validate(&self, key: &str) -> Result<(), Self::Error>;
    fn remove(&self, scope: crate::CatalogReadScope) -> Result<(), Self::Error>;
    fn document(
        &self,
        scope: crate::CatalogReadScope,
        row: &CatalogRow,
        fields: &str,
        complete: bool,
    ) -> Result<(), Self::Error>;
    fn postings(&self, rows: &[(&String, &CatalogRow)]) -> Result<(), Self::Error>;
    fn counts(&self, scope: crate::CatalogReadScope) -> Result<(), Self::Error>;
}

pub(crate) type HistoryRecipeHeader = (u32, Option<String>, bool);
pub(crate) trait CompositeHistoryStore {
    type Error;
    fn data(error: serde_json::Error) -> Self::Error;
    fn original(&self, key: &str) -> Result<bool, Self::Error>;
    fn previous(&self, key: &str) -> Result<Option<String>, Self::Error>;
    fn history_exists(&self, key: &str) -> Result<bool, Self::Error>;
    fn retain_source(&self, row: &CatalogRow, source: &CatalogSource) -> Result<(), Self::Error>;
    fn relocated(
        &self,
        old: &CatalogSource,
        current: &[CatalogSource],
    ) -> Result<bool, Self::Error>;
    fn clear(&self, key: &str) -> Result<(), Self::Error>;
    fn previous_recipe(
        &self,
        key: &str,
    ) -> Result<(bool, Option<HistoryRecipeHeader>), Self::Error>;
    fn recipe_slots(&self, key: &str, history: bool) -> Result<Vec<String>, Self::Error>;
    fn composed(
        &self,
        key: &str,
        generation: &str,
        sources: &str,
        metadata: &str,
        texts: &[String],
        incomplete: bool,
    ) -> Result<(), Self::Error>;
}

/// The original dependency derivation and statement ordering, shared by both
/// physical layouts. Lineage and bridge dependencies retain their own owners.
pub(crate) fn publish_current_sources<P: CurrentPublicationStore>(
    store: &P,
    row: &CatalogRow,
) -> Result<(), P::Error> {
    publish_current_sources_measured(
        store,
        row,
        &mut crate::native_timing::Measurements::new(false),
    )
}

pub(crate) fn publish_current_sources_measured<P: CurrentPublicationStore>(
    store: &P,
    row: &CatalogRow,
    measurements: &mut crate::native_timing::Measurements,
) -> Result<(), P::Error> {
    let started = measurements.start();
    store.retire_sources(&row.key)?;
    measurements.record("source_retirement", started, 1);
    for source in &row.sources {
        let started = measurements.start();
        store.clear_dependencies(source)?;
        let parent = source.native_role.as_ref().and_then(|role| {
            role.pointer("/Agent/parent")
                .and_then(Value::as_str)
                .or_else(|| {
                    (role
                        .pointer("/Codex/guardian_review")
                        .and_then(Value::as_bool)
                        != Some(true))
                    .then(|| role.pointer("/Codex/parent_thread").and_then(Value::as_str))
                    .flatten()
                })
        });
        if let Some(observations) = row
            .query
            .as_ref()
            .and_then(|query| query["rows"].as_object())
        {
            for observation in observations.values() {
                if let Some(pid) = observation["summary"]["pid"].as_u64() {
                    store.dependency(source, "runtime:pid", &pid.to_string())?;
                }
            }
        }
        if let Some(parent) = parent {
            store.dependency(source, &row.harness, parent)?;
        }
        measurements.record("native_dependencies", started, 1);
        let started = measurements.start();
        store.source(row, source)?;
        measurements.record("catalog_sources", started, 1);
    }
    Ok(())
}

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
    fn copy_slot(
        &self,
        key: &str,
        slot: i64,
        offset: i64,
        take: i64,
        split: bool,
        inspect: bool,
    ) -> Result<Option<String>, Self::Error>;
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
        Sha256::digest(
            serde_json::to_string(&(&metadata, &original_header))
                .map_err(P::data)?
                .as_bytes()
        )
    );
    let saved = store.transfer(&old.key)?;
    let prior = saved
        .as_deref()
        .map(serde_json::from_str::<HistoryTransfer>)
        .transpose()
        .map_err(P::data)?;
    if prior.as_ref().is_some_and(|prior| prior.witness != witness) {
        store.reset_transfer(&old.key)?;
        return Ok(false);
    }
    let mut transfer = prior.unwrap_or_else(|| HistoryTransfer {
        witness,
        last: None,
        copied: 0,
        partial: 0,
        complete: header
            .as_ref()
            .is_some_and(|(_, version, recorded, total)| {
                *version == crate::slot_projection::VERSION
                    && *total >= 0
                    && !old.sources.is_empty()
                    && old.sources.len() <= 64
                    && recorded.is_some()
                    && crate::retention::source_generation(&old.sources)
                        .ok()
                        .as_ref()
                        == recorded.as_ref()
            }),
    });
    let paths: BTreeSet<_> = old.sources.iter().map(|source| &source.path).collect();
    let mut bytes = 0usize;
    let mut processed = 0;
    while processed < 64 && bytes < 1048576 {
        let Some((slot, length)) = store.next_slot(&old.key, transfer.last)? else {
            break;
        };
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
            transfer.complete = text
                .as_deref()
                .and_then(|text| {
                    serde_json::from_str::<crate::slot_projection::SavedSlot>(text).ok()
                })
                .is_some_and(|slot| slot.source_paths().iter().all(|path| paths.contains(*path)));
        }
        transfer.last = Some(slot);
        transfer.copied += 1;
    }
    if store.more_slots(&old.key, transfer.last)? {
        store.checkpoint(
            &old.key,
            &serde_json::to_string(&transfer).map_err(P::data)?,
        )?;
        return Ok(false);
    }
    transfer.complete &= header
        .as_ref()
        .is_some_and(|(_, _, _, total)| usize::try_from(*total).ok() == Some(transfer.copied));
    store.complete(
        old,
        &metadata,
        &original_header,
        header.as_ref(),
        transfer.complete,
    )?;
    Ok(true)
}

/// Admission policy chooses a replacement; custody and its bounded progress are
/// shared. At most one original owner advances per publication attempt.
pub(crate) fn preserve_replacement_rows<P: OriginalHistoryStore>(
    store: &P,
    rows: &[CatalogRow],
) -> Result<bool, P::Error> {
    preserve_rows(store, rows, &[])
}

/// Sessions a publication withdraws from Current keep original custody the
/// same way replaced rows do, under the same one-advance bound.
pub(crate) fn preserve_rows<P: OriginalHistoryStore>(
    store: &P,
    rows: &[CatalogRow],
    withdrawn: &[CatalogRow],
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
    for old in withdrawn {
        if store.retained(&old.key)? {
            continue;
        }
        if advanced || !preserve_original(store, old)? {
            return Ok(false);
        }
        advanced = true;
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
    let rows: std::collections::BTreeMap<_, _> =
        rows.iter().map(|row| (row.key.as_str(), row)).collect();
    for projection in projections {
        let Some(row) = rows.get(projection.key.as_str()) else {
            continue;
        };
        let metadata = projection
            .slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(P::data)?;
        let header = SlotPublicationHeader {
            key: &projection.key,
            source_generation: crate::retention::source_generation(&row.sources)
                .map_err(P::data)?,
            generation: crate::slot_projection::generation(&metadata, &row.sources)
                .map_err(P::data)?,
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

/// The publisher's consumed-prefix identity test, shared by both adapters.
/// A Catalog source intentionally omits unconsumed size/mtime. Those fields
/// remain bound by the native proof's full Ledger rather than invented here.
pub(crate) fn source_matches_ledger(
    source: &CatalogSource,
    ledger: &crate::events::Ledger,
) -> bool {
    (
        ledger.stat.dev,
        ledger.stat.ino,
        ledger.offset,
        ledger.prefix,
        ledger.tail,
        ledger.stat.changed_ns,
    ) == (
        source.dev,
        source.ino,
        source.offset,
        source.prefix_sha256,
        source.tail_sha256,
        source.changed_ns,
    )
}

#[cfg(test)]
#[test]
fn catalog135_source_prefix_identity_keeps_nullable_and_unsigned_semantics() {
    let ledger = crate::events::Ledger {
        stat: crate::events::Stat {
            dev: u64::MAX,
            ino: u64::MAX - 1,
            size: u64::MAX,
            modified_ns: u128::MAX,
            changed_ns: None,
        },
        offset: u64::MAX - 2,
        prefix: [7; 32],
        tail: [9; 32],
    };
    let source = ledger.source(std::path::Path::new("logical\0🦀"), "native\0🦀");
    assert!(source_matches_ledger(&source, &ledger));
    let mut changed = ledger.clone();
    changed.stat.size -= 1;
    changed.stat.modified_ns -= 1;
    // These are unconsumed observations, not consumed-prefix identity fields.
    assert!(source_matches_ledger(&source, &changed));
    for field in 0..6 {
        let mut changed = ledger.clone();
        match field {
            0 => changed.stat.dev -= 1,
            1 => changed.stat.ino -= 1,
            2 => changed.offset -= 1,
            3 => changed.prefix[0] ^= 1,
            4 => changed.tail[0] ^= 1,
            _ => changed.stat.changed_ns = Some(i128::MIN),
        }
        assert!(!source_matches_ledger(&source, &changed));
    }
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
    fn presentation_observations(
        &self,
    ) -> Option<&crate::storage::selected::PresentationObservations> {
        None
    }
    fn source_state(&self, _source: &CatalogSource) -> Option<&'static str> {
        None
    }
    fn runtime_binding(&self) -> Option<super::QueryRuntimeBinding> {
        None
    }
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

/// The original missing-source History composition, shared without SQL or a
/// second recipe derivation. Physical reads/effects remain adapter-owned.
pub(crate) fn publish_history_rows<P: CompositeHistoryStore>(
    store: &P,
    rows: &[CatalogRow],
    projections: Option<&[crate::slot_projection::Projection]>,
) -> Result<(), P::Error> {
    use crate::slot_projection::{SavedSlot, VERSION};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};
    let Some(projections) = projections else {
        return Ok(());
    };
    let rows: BTreeMap<_, _> = rows.iter().map(|row| (row.key.as_str(), row)).collect();
    for projection in projections {
        let Some(current) = rows.get(projection.key.as_str()).copied() else {
            continue;
        };
        // A completed cross-harness original is immutable custody even if a
        // later Current publication reuses its original harness again.
        let original = store.original(&current.key)?;
        if original {
            continue;
        }
        let old_text = store.previous(&current.key)?;
        let old = old_text
            .as_deref()
            .and_then(|text| serde_json::from_str::<CatalogRow>(text).ok());
        let separate_harness = old
            .as_ref()
            .is_some_and(|old| old.harness != current.harness);
        if separate_harness {
            let old = old.as_ref().unwrap();
            for source in &old.sources {
                store.retain_source(old, source)?;
            }
            // The owning publication transaction completed custody preflight.
            debug_assert!(store.history_exists(&old.key)?);
            continue;
        }
        // A reused viewer key does not join two native harness identities.
        // Retain the original owner and recipes independently of Current.
        let mut history = current.clone();
        let paths: BTreeSet<_> = current.sources.iter().map(|source| &source.path).collect();
        let mut missing = BTreeMap::new();
        for source in old.as_ref().into_iter().flat_map(|row| &row.sources) {
            if !paths.contains(&source.path) && !store.relocated(source, &current.sources)? {
                missing.insert(source.path.clone(), source.clone());
            }
        }
        if missing.is_empty() {
            store.clear(&current.key)?;
            continue;
        }
        // A composite recipe is custody, not an exact aggregate of its native
        // records. The existing selected producer must qualify full Query facts.
        history.query = None;
        history.sources.extend(missing.values().cloned());
        history.native_ids = history
            .sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for source in missing.values() {
            store.retain_source(old.as_ref().unwrap(), source)?;
        }

        let mut slots = projection.slots.clone();
        let mut incomplete = false;
        if !missing.is_empty() {
            let (history_recipe, header) = store.previous_recipe(&current.key)?;
            let qualified =
                old.as_ref()
                    .zip(header.as_ref())
                    .is_some_and(|(old, (version, hash, _))| {
                        *version == VERSION
                            && hash.is_some()
                            && crate::retention::source_generation(&old.sources)
                                .ok()
                                .as_ref()
                                == hash.as_ref()
                    });
            if qualified {
                incomplete = header.is_some_and(|(_, _, incomplete)| incomplete);
                let old_sources: BTreeMap<_, _> = old
                    .as_ref()
                    .unwrap()
                    .sources
                    .iter()
                    .map(|source| (&source.path, source))
                    .collect();
                let current_sources: BTreeMap<_, _> = current
                    .sources
                    .iter()
                    .map(|source| (&source.path, source))
                    .collect();
                for text in store.recipe_slots(&current.key, history_recipe)? {
                    let slot: SavedSlot = match serde_json::from_str(&text) {
                        Ok(slot) => slot,
                        Err(_) => {
                            incomplete = true;
                            continue;
                        }
                    };
                    let Some(file) = &slot.file else {
                        continue;
                    };
                    if !missing.contains_key(&file.path) {
                        if slot
                            .source_paths()
                            .iter()
                            .any(|path| missing.contains_key(*path))
                        {
                            incomplete = true;
                        }
                        continue;
                    }
                    // Every foreign-file dependency must retain the same original
                    // consumed generation. Changed foreign paths need a future
                    // multi-generation recipe, never an implicit current rebind.
                    if slot.source_paths().iter().all(|path| {
                        missing.contains_key(*path)
                            || old_sources.get(*path) == current_sources.get(*path)
                    }) {
                        let at = slot
                            .t
                            .and_then(|time| {
                                slots
                                    .iter()
                                    .position(|current| current.t.is_some_and(|t| t > time))
                            })
                            .unwrap_or(slots.len());
                        slots.insert(at, slot);
                    } else {
                        incomplete = true;
                    }
                }
            } else {
                incomplete = true;
            }
        }
        // Native source/event/block/kind identity, not text equality. Existing
        // current slots win. Stable timestamp ordering preserves old records
        // while allowing current continuation to advance after a disappearance.
        let mut seen = BTreeSet::new();
        slots.retain(|slot| seen.insert(slot.identity()));
        let texts: Vec<String> = slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()
            .map_err(P::data)?;
        let sources = crate::retention::source_generation(&history.sources).map_err(P::data)?;
        let metadata = serde_json::to_string(&history).map_err(P::data)?;
        let generation = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_string(&(VERSION, &sources, &texts, incomplete))
                    .map_err(P::data)?
                    .as_bytes()
            )
        );
        store.composed(
            &current.key,
            &generation,
            &sources,
            &metadata,
            &texts,
            incomplete,
        )?;
    }
    Ok(())
}
