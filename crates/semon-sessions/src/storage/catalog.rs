//! Semantic native Catalog snapshot boundary. No driver or SQL escapes here.
use crate::{
    catalog::Request,
    model::summary::{CatalogRow, CatalogSource},
};
use serde_json::Value;
use std::io;

/// Physical metadata owner selected by a snapshot, independent of display lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogObservationOwner {
    Catalog,
    History,
}

/// Physical selection provenance, including an immutable presentation's original
/// publication owner. This does not grant live observation authority.
pub(crate) struct SelectedCatalogRow {
    pub(crate) row: CatalogRow,
    pub(crate) owner: Option<CatalogObservationOwner>,
}
impl std::ops::Deref for SelectedCatalogRow {
    type Target = CatalogRow;
    fn deref(&self) -> &Self::Target {
        &self.row
    }
}

pub(crate) type OriginalHeader = (String, u32, Option<String>, i64);

/// Ephemeral proof that one indexed prefix advanced on the same file identity.
/// The ordinary Catalog publisher carries this beside its observations; it is
/// never stored as row metadata.
pub(crate) const SOURCE_CONTINUATION_LIMIT: usize = 64;

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct VerifiedSourceContinuation {
    path: std::path::PathBuf,
    prior: crate::events::Ledger,
    current: crate::events::Ledger,
    accepted: crate::source_change::AppendEvidence,
}

impl VerifiedSourceContinuation {
    pub(crate) fn from_scan(
        path: &std::path::Path,
        prior: &crate::events::Ledger,
        current: &crate::events::Ledger,
        accepted: &crate::source_change::AppendEvidence,
        physical: &crate::events::Stat,
    ) -> Option<Self> {
        if !accepted.qualifies(&prior.stat, physical)
            || prior.stat.size != prior.offset
            || current.stat != *physical
            || current.offset <= prior.offset
            || current.offset > physical.size
        {
            return None;
        }
        Some(Self {
            path: path.to_owned(),
            prior: prior.clone(),
            current: current.clone(),
            accepted: accepted.clone(),
        })
    }

    fn advances(&self, old: &CatalogSource, new: &CatalogSource) -> bool {
        let matches_ledger = |source: &CatalogSource, ledger: &crate::events::Ledger| {
            source.dev == ledger.stat.dev
                && source.ino == ledger.stat.ino
                && source.size == ledger.stat.size
                && source.modified_ns == ledger.stat.modified_ns
                && source.changed_ns == ledger.stat.changed_ns
                && source.offset == ledger.offset
                && source.prefix_sha256 == ledger.prefix
                && source.tail_sha256 == ledger.tail
        };
        old.path == self.path
            && new.path == self.path
            && old.native_id == new.native_id
            && old.native_role == new.native_role
            && matches_ledger(old, &self.prior)
            && matches_ledger(new, &self.current)
            && self.accepted.before == self.prior.stat
            && self.accepted.after == self.current.stat
            && self.current.offset > self.prior.offset
            && self.current.offset <= self.accepted.after.size
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod source_continuation_tests {
    use super::*;

    const GENERATION: &str = "0123456789abcdef0123456789abcdef";

    fn fixture() -> (
        std::path::PathBuf,
        crate::events::Ledger,
        crate::events::Ledger,
        crate::source_change::AppendEvidence,
        crate::events::Stat,
    ) {
        let path = std::path::PathBuf::from("/source.jsonl");
        let before = crate::events::Stat {
            dev: 1,
            ino: 2,
            size: 10,
            modified_ns: 3,
            changed_ns: Some(4),
        };
        let physical = crate::events::Stat {
            size: 20,
            modified_ns: 30,
            changed_ns: Some(40),
            ..before
        };
        let prior = crate::events::Ledger {
            stat: before,
            offset: before.size,
            prefix: [1; 32],
            tail: [2; 32],
        };
        let current = crate::events::Ledger {
            stat: physical,
            offset: 18,
            prefix: [3; 32],
            tail: [4; 32],
        };
        let accepted = crate::source_change::AppendEvidence {
            generation: GENERATION.into(),
            before,
            after: physical,
        };
        (path, prior, current, accepted, physical)
    }

    fn source(
        ledger: &crate::events::Ledger,
        path: &std::path::Path,
        native_id: &str,
        role: Option<serde_json::Value>,
    ) -> CatalogSource {
        let mut source = ledger.source(path, native_id);
        source.native_role = role;
        source
    }

    #[test]
    fn catalog_continuation_accepts_opaque_generation_and_changed_stat_times() {
        let (path, prior, current, accepted, physical) = fixture();
        assert_ne!(prior.stat.modified_ns, physical.modified_ns);
        assert_ne!(prior.stat.changed_ns, physical.changed_ns);

        let proof =
            VerifiedSourceContinuation::from_scan(&path, &prior, &current, &accepted, &physical)
                .expect(
                    "a qualified append may change mtime/ctime and keep an opaque custody token",
                );
        let role = Some(serde_json::json!({"kind": "owner"}));
        let old = source(&prior, &path, "native", role.clone());
        let new = source(&current, &path, "native", role);

        assert!(proof.advances(&old, &new));
    }

    #[test]
    fn catalog_continuation_refuses_replacement_truncation_and_no_advance() {
        let (path, prior, current, accepted, physical) = fixture();

        let replaced = crate::events::Stat {
            dev: 9,
            ino: 10,
            ..physical
        };
        let replaced_current = crate::events::Ledger {
            stat: replaced,
            ..current.clone()
        };
        let replaced_evidence = crate::source_change::AppendEvidence {
            after: replaced,
            ..accepted.clone()
        };
        assert!(
            VerifiedSourceContinuation::from_scan(
                &path,
                &prior,
                &replaced_current,
                &replaced_evidence,
                &replaced,
            )
            .is_none()
        );

        let truncated = crate::events::Stat {
            size: 8,
            modified_ns: 31,
            changed_ns: Some(41),
            ..prior.stat
        };
        let truncated_current = crate::events::Ledger {
            stat: truncated,
            offset: 8,
            ..current.clone()
        };
        let truncated_evidence = crate::source_change::AppendEvidence {
            after: truncated,
            ..accepted.clone()
        };
        assert!(
            VerifiedSourceContinuation::from_scan(
                &path,
                &prior,
                &truncated_current,
                &truncated_evidence,
                &truncated,
            )
            .is_none()
        );

        let incomplete = crate::events::Ledger {
            offset: prior.offset,
            ..current
        };
        assert!(VerifiedSourceContinuation::from_scan(
            &path,
            &prior,
            &incomplete,
            &accepted,
            &physical,
        )
        .is_none());
    }

    #[test]
    fn catalog_continuation_requires_exact_stats_identity_role_and_ledgers() {
        let (path, prior, current, accepted, physical) = fixture();
        let proof =
            VerifiedSourceContinuation::from_scan(&path, &prior, &current, &accepted, &physical)
                .unwrap();
        let role = Some(serde_json::json!({"kind": "owner"}));
        let old = source(&prior, &path, "native", role.clone());
        let new = source(&current, &path, "native", role.clone());

        let mut changed = old.clone();
        changed.path.push("-replacement");
        assert!(!proof.advances(&changed, &new));

        let mut changed = new.clone();
        changed.path.push("-replacement");
        assert!(!proof.advances(&old, &changed));

        let mut changed = new.clone();
        changed.native_id.push_str("-replacement");
        assert!(!proof.advances(&old, &changed));

        let mut changed = new.clone();
        changed.native_role = Some(serde_json::json!({"kind": "other"}));
        assert!(!proof.advances(&old, &changed));

        let mut changed = old.clone();
        changed.prefix_sha256[0] ^= 1;
        assert!(!proof.advances(&changed, &new));

        let mut changed = new.clone();
        changed.tail_sha256[0] ^= 1;
        assert!(!proof.advances(&old, &changed));

        let changed_before = crate::events::Stat {
            modified_ns: prior.stat.modified_ns + 1,
            ..prior.stat
        };
        let before_mismatch = crate::source_change::AppendEvidence {
            before: changed_before,
            ..accepted.clone()
        };
        assert!(
            VerifiedSourceContinuation::from_scan(
                &path,
                &prior,
                &current,
                &before_mismatch,
                &physical,
            )
            .is_none()
        );

        let after_mismatch = crate::events::Stat {
            changed_ns: Some(physical.changed_ns.unwrap() + 1),
            ..physical
        };
        let after_mismatch_evidence = crate::source_change::AppendEvidence {
            after: after_mismatch,
            ..accepted.clone()
        };
        assert!(
            VerifiedSourceContinuation::from_scan(
                &path,
                &prior,
                &current,
                &after_mismatch_evidence,
                &physical,
            )
            .is_none()
        );

        let current_mismatch = crate::events::Ledger {
            stat: after_mismatch,
            ..current
        };
        assert!(
            VerifiedSourceContinuation::from_scan(
                &path,
                &prior,
                &current_mismatch,
                &accepted,
                &physical,
            )
            .is_none()
        );

        let mut bad_old_identity = old;
        bad_old_identity.native_id.push_str("-other");
        assert!(!proof.advances(&bad_old_identity, &new));
    }

    #[test]
    fn catalog_continuation_refuses_malformed_custody_generation() {
        let (path, prior, current, accepted, physical) = fixture();
        let malformed = crate::source_change::AppendEvidence {
            generation: "untrusted".into(),
            ..accepted
        };
        assert!(
            VerifiedSourceContinuation::from_scan(&path, &prior, &current, &malformed, &physical,)
                .is_none()
        );
    }
}

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
    fn begin(&self, keys: &[String]) -> Result<(), Self::Error>;
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
    fn postings(
        &self,
        rows: &[crate::catalog_search::SearchPosting<'_>],
    ) -> Result<(), Self::Error>;
    fn counts(&self, scope: crate::CatalogReadScope) -> Result<(), Self::Error>;
}

pub(crate) type HistoryRecipeHeader = (u32, Option<String>, bool);

/// One local native publication receipt after its row, source ledgers, saved
/// slots, presentation digest, and CWD fingerprint have all been checked.
/// Imported references and row digests have no constructor on this path.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct VerifiedHistoryReceipt {
    pub(crate) id: String,
    pub(crate) source_generation: String,
    pub(crate) sources: Vec<CatalogSource>,
    pub(crate) slots: Vec<String>,
    pub(crate) cwd_fingerprint: String,
    pub(crate) cwds: std::collections::BTreeMap<std::path::PathBuf, Option<String>>,
}

impl VerifiedHistoryReceipt {
    pub(crate) fn verified(
        id: String,
        source_generation: String,
        sources: Vec<CatalogSource>,
        slots: Vec<String>,
        cwd_fingerprint: String,
        cwds: std::collections::BTreeMap<std::path::PathBuf, Option<String>>,
    ) -> Self {
        Self {
            id,
            source_generation,
            sources,
            slots,
            cwd_fingerprint,
            cwds,
        }
    }
}

/// Bounded facts used by the pure History planner. `receipts` contains only
/// evidence authenticated by the backend's retained local receipt verifier.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct HistoryCompositionSnapshot {
    pub(crate) key: String,
    pub(crate) original: bool,
    pub(crate) previous: Option<String>,
    pub(crate) history_exists: bool,
    pub(crate) recipe_history: bool,
    pub(crate) recipe_header: Option<HistoryRecipeHeader>,
    pub(crate) recipe_slots: Vec<String>,
    pub(crate) relocated: std::collections::BTreeSet<String>,
    pub(crate) receipts: Vec<VerifiedHistoryReceipt>,
    pub(crate) current_cwds: Option<std::collections::BTreeSet<String>>,
    pub(crate) unavailable: bool,
}

/// Inputs for the verified History projection and ownership write.
#[derive(Clone, Copy)]
pub(crate) struct VerifiedHistoryComposition<'a> {
    pub(crate) key: &'a str,
    pub(crate) generation: &'a str,
    pub(crate) sources: &'a str,
    pub(crate) metadata: &'a str,
    pub(crate) texts: &'a [String],
    pub(crate) incomplete: bool,
    pub(crate) receipt_ids: &'a [String],
    pub(crate) capture: &'a str,
}

/// The prewriter and writer derive this same deterministic plan. Slot texts
/// enter it only from `VerifiedHistoryReceipt`; a changed preimage therefore
/// prevents composition before any History owner or slot is replaced.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HistoryCompositionPlan {
    preimage: String,
    steps: Vec<HistoryCompositionStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
enum HistoryCompositionStep {
    Preserve {
        key: String,
    },
    Clear {
        key: String,
        metadata: String,
        capture_cwds: Option<std::collections::BTreeSet<String>>,
    },
    Retain {
        key: String,
        previous: String,
        sources: Vec<CatalogSource>,
    },
    Compose {
        key: String,
        previous: Option<String>,
        generation: String,
        sources: String,
        metadata: String,
        slots: Vec<String>,
        incomplete: bool,
        retained: Vec<CatalogSource>,
        receipt_ids: Vec<String>,
        capture_cwds: Option<std::collections::BTreeSet<String>>,
    },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HistoryCompositionCaptures {
    values: std::collections::BTreeMap<String, String>,
}

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

/// Verified composition is limited to adapters that can authenticate local
/// publication receipts and charge their snapshot reads against the caller's
/// dynamic request budget. Other adapters keep their established publisher
/// until their receipt contract is reviewed.
pub(crate) trait VerifiedHistoryCompositionStore: CompositeHistoryStore {
    fn composition_snapshot(
        &self,
        current: &CatalogRow,
        budget: &mut crate::overview::SummaryPublicationBudget,
    ) -> Result<HistoryCompositionSnapshot, Self::Error>;
    fn composed_verified(
        &self,
        composition: VerifiedHistoryComposition<'_>,
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
    fn previous(&self, row: &CatalogRow) -> Result<Option<CatalogRow>, Self::Error>;
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

/// Sessions a publication withdraws from Current keep original custody the
/// same way replaced rows do. At most one original owner advances per attempt.
pub(crate) fn preserve_rows<P: OriginalHistoryStore>(
    store: &P,
    rows: &[CatalogRow],
    withdrawn: &[CatalogRow],
    continuations: &[VerifiedSourceContinuation],
) -> Result<bool, P::Error> {
    let mut advanced = false;
    for row in rows {
        if store.retained(&row.key)? {
            continue;
        }
        if let Some(old) = store.previous(row)?
            && replaces_original(&old, row, continuations)
        {
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

/// Shared source-revision classification for SQLite and PostgreSQL History.
/// Every changed incoming source must have one exact scanner proof; any
/// addition, rewrite, role change, or ambiguous proof keeps original custody.
fn replaces_original(
    old: &CatalogRow,
    current: &CatalogRow,
    continuations: &[VerifiedSourceContinuation],
) -> bool {
    if old.harness != current.harness {
        return true;
    }
    let mut old_paths = std::collections::BTreeSet::new();
    if old
        .sources
        .iter()
        .any(|source| !old_paths.insert(source.path.clone()))
    {
        return true;
    }
    let mut paths = std::collections::BTreeSet::new();
    current.sources.iter().any(|source| {
        if !paths.insert(source.path.clone()) {
            return true;
        }
        if old.sources.contains(source) {
            return false;
        }
        let mut previous_matches = old
            .sources
            .iter()
            .filter(|prior| prior.path == source.path && prior.native_id == source.native_id);
        let Some(previous) = previous_matches.next() else {
            return true;
        };
        if previous_matches.next().is_some() {
            return true;
        }
        continuations
            .iter()
            .filter(|proof| proof.advances(previous, source))
            .count()
            != 1
    })
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

#[cfg(test)]
#[test]
fn catalog_history_classification_rejects_duplicate_current_source_paths() {
    let source = CatalogSource {
        path: std::path::PathBuf::from("/source.jsonl"),
        native_id: "native".into(),
        dev: 1,
        ino: 2,
        size: 10,
        modified_ns: 3,
        changed_ns: Some(4),
        offset: 10,
        prefix_sha256: [1; 32],
        tail_sha256: [2; 32],
        native_role: None,
    };
    let old = CatalogRow {
        overview: None,
        trace: None,
        query_recipe: None,
        query: None,
        lifecycle: Default::default(),
        key: "session".into(),
        name: "session".into(),
        harness: "claude".into(),
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
        sources: vec![source.clone()],
    };
    let mut appended = source.clone();
    appended.size = 20;
    appended.offset = 18;
    appended.prefix_sha256 = [3; 32];
    let mut current = old.clone();
    current.sources.push(appended);

    assert!(replaces_original(&old, &current, &[]));
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogPublicationBlocker {
    PublicationTooLarge,
}

impl CatalogPublicationBlocker {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::PublicationTooLarge => "publication_too_large",
        }
    }

    pub(crate) const fn retryable(self) -> bool {
        false
    }
}

pub(crate) trait CatalogSnapshot {
    fn runtime_generation(&self, options: &crate::Options) -> io::Result<String> {
        crate::native_evidence::query_runtime_generation(options)
    }
    fn facts_authority<'a>(
        &'a self,
        path: &std::path::Path,
    ) -> io::Result<Box<dyn crate::facts::source_authority::SourceAuthority + 'a>>;
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
    /// Current per-session Facts/PID dependencies under this snapshot's
    /// namespace, exact source ledgers and qualified observation provenance.
    fn runtime_observation(
        &self,
        _options: &crate::Options,
        _row: &CatalogRow,
        _owner: CatalogObservationOwner,
        _runtime: &str,
    ) -> io::Result<Option<String>> {
        Err(io::Error::other(super::StorageError::Unavailable))
    }
    /// Imported observations retain their producer's exact binding. Adapters
    /// must qualify that provenance under the current destination admission.
    fn row_runtime_binding(
        &self,
        _row: &CatalogRow,
    ) -> io::Result<Option<super::QueryRuntimeBinding>> {
        Ok(self.runtime_binding())
    }
    fn require_current(&self) -> io::Result<()>;
    fn publication_value(&self, key: &str) -> io::Result<Option<String>>;
    /// A bounded, typed explanation for the current partial publication.
    /// Adapters without an equivalent durable stage refusal return `None`.
    fn publication_blocker(&self) -> io::Result<Option<CatalogPublicationBlocker>> {
        Ok(None)
    }
    fn catalog_search_paths(
        &self,
        lookup: &crate::events::CatalogSearchLookup<'_>,
        limit: usize,
    ) -> io::Result<Vec<String>>;
    fn rows(&self, request: &Request) -> io::Result<Vec<SelectedCatalogRow>>;
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection>;
    fn has_history(&self, key: &str) -> io::Result<bool>;
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>>;
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool>;
    /// Qualify compact identity for an incomplete, non-original composite
    /// History row. The selected row must be its same-key, same-harness
    /// Current Catalog owner. This does not qualify History recipe contents.
    fn composite_history_identity_qualified(
        &self,
        current: &SelectedCatalogRow,
        history: &CatalogRow,
    ) -> io::Result<bool>;
    /// Read only the payload belonging to the store-issued exact observation.
    /// This never selects a cohort from a display lifecycle or latest header.
    #[allow(dead_code)] // Inactive until whole-cohort selected Query wiring is ready.
    fn selected_query_recipe(
        &self,
        _observation: &super::query::QueryRecipeObservation,
    ) -> io::Result<crate::query_projection::SelectedRecipe> {
        Err(io::Error::other(super::StorageError::Unavailable))
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow, Option<CatalogObservationOwner>) -> Value,
    ) -> io::Result<Option<Value>>;
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<Value>>;
}

fn history_snapshots<P: VerifiedHistoryCompositionStore>(
    store: &P,
    rows: &[CatalogRow],
    projections: Option<&[crate::slot_projection::Projection]>,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<Vec<HistoryCompositionSnapshot>, P::Error> {
    let Some(projections) = projections else {
        return Ok(Vec::new());
    };
    let rows: std::collections::BTreeMap<_, _> =
        rows.iter().map(|row| (row.key.as_str(), row)).collect();
    let mut snapshots = Vec::new();
    for projection in projections {
        let Some(current) = rows.get(projection.key.as_str()).copied() else {
            continue;
        };
        let snapshot = if store.original(&current.key)? {
            HistoryCompositionSnapshot {
                key: current.key.clone(),
                original: true,
                previous: None,
                history_exists: true,
                recipe_history: false,
                recipe_header: None,
                recipe_slots: Vec::new(),
                relocated: std::collections::BTreeSet::new(),
                receipts: Vec::new(),
                current_cwds: None,
                unavailable: false,
            }
        } else {
            store.composition_snapshot(current, budget)?
        };
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}

fn plan_history_rows(
    rows: &[CatalogRow],
    projections: Option<&[crate::slot_projection::Projection]>,
    snapshots: &[HistoryCompositionSnapshot],
) -> Result<HistoryCompositionPlan, serde_json::Error> {
    use crate::slot_projection::{SavedSlot, VERSION};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};

    let Some(projections) = projections else {
        return Ok(HistoryCompositionPlan {
            preimage: String::new(),
            steps: Vec::new(),
        });
    };
    let rows: BTreeMap<_, _> = rows.iter().map(|row| (row.key.as_str(), row)).collect();
    let snapshots: BTreeMap<_, _> = snapshots
        .iter()
        .map(|snapshot| (snapshot.key.as_str(), snapshot))
        .collect();
    let mut steps = Vec::new();
    for projection in projections {
        let Some(current) = rows.get(projection.key.as_str()).copied() else {
            continue;
        };
        let Some(snapshot) = snapshots.get(current.key.as_str()).copied() else {
            steps.push(HistoryCompositionStep::Preserve {
                key: current.key.clone(),
            });
            continue;
        };
        if snapshot.unavailable || snapshot.original {
            steps.push(HistoryCompositionStep::Preserve {
                key: current.key.clone(),
            });
            continue;
        }
        let old_text = snapshot.previous.as_deref();
        let old = old_text.and_then(|text| serde_json::from_str::<CatalogRow>(text).ok());
        if old_text.is_some() && old.is_none() {
            steps.push(HistoryCompositionStep::Preserve {
                key: current.key.clone(),
            });
            continue;
        }
        if snapshot.history_exists && old.is_none() {
            steps.push(HistoryCompositionStep::Preserve {
                key: current.key.clone(),
            });
            continue;
        }
        if let Some(old) = old.as_ref()
            && old.harness != current.harness
        {
            steps.push(HistoryCompositionStep::Retain {
                key: current.key.clone(),
                previous: old_text.unwrap_or_default().to_owned(),
                sources: old.sources.clone(),
            });
            continue;
        }

        let paths: BTreeSet<_> = current.sources.iter().map(|source| &source.path).collect();
        let mut missing = BTreeMap::new();
        for source in old.as_ref().into_iter().flat_map(|row| &row.sources) {
            if !paths.contains(&source.path)
                && !snapshot
                    .relocated
                    .contains(&source.path.to_string_lossy().into_owned())
            {
                missing.insert(source.path.clone(), source.clone());
            }
        }
        if missing.is_empty() {
            if snapshot.history_exists {
                steps.push(HistoryCompositionStep::Clear {
                    key: current.key.clone(),
                    metadata: serde_json::to_string(current)?,
                    capture_cwds: snapshot.current_cwds.clone(),
                });
            } else {
                steps.push(HistoryCompositionStep::Preserve {
                    key: current.key.clone(),
                });
            }
            continue;
        }

        let old = old
            .as_ref()
            .expect("missing sources require a previous row");
        let mut history = current.clone();
        history.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        history.query = None;
        history.sources.extend(missing.values().cloned());
        history.native_ids = history
            .sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let mut slots = projection.slots.clone();
        let mut incomplete;
        let mut invalid_predecessor = false;
        let mut receipt_ids = BTreeSet::new();
        let mut capture_cwds = snapshot.current_cwds.clone();
        let qualified = snapshot
            .recipe_header
            .as_ref()
            .is_some_and(|(version, generation, _)| {
                *version == VERSION
                    && generation.as_deref()
                        == crate::retention::source_generation(&old.sources)
                            .ok()
                            .as_deref()
            });
        if qualified {
            incomplete = snapshot
                .recipe_header
                .as_ref()
                .is_some_and(|(_, _, incomplete)| *incomplete);
            let old_sources: BTreeMap<_, _> = old
                .sources
                .iter()
                .map(|source| (&source.path, source))
                .collect();
            let current_sources: BTreeMap<_, _> = current
                .sources
                .iter()
                .map(|source| (&source.path, source))
                .collect();
            for text in &snapshot.recipe_slots {
                let slot: SavedSlot = match serde_json::from_str(text) {
                    Ok(slot) => slot,
                    Err(_) => {
                        incomplete = true;
                        invalid_predecessor = true;
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
                        invalid_predecessor = true;
                    }
                    continue;
                }
                let authenticated = snapshot.receipts.iter().find(|receipt| {
                    receipt.slots.iter().any(|saved| saved == text)
                        && crate::retention::source_generation(&receipt.sources)
                            .ok()
                            .as_deref()
                            == Some(receipt.source_generation.as_str())
                        && slot.source_paths().iter().all(|path| {
                            old_sources.get(*path).copied()
                                == receipt.sources.iter().find(|source| &source.path == *path)
                        })
                });
                let Some(authenticated) = authenticated else {
                    incomplete = true;
                    invalid_predecessor = true;
                    continue;
                };
                receipt_ids.insert(authenticated.id.clone());
                let foreign_dependencies_match = slot.source_paths().iter().all(|path| {
                    missing.contains_key(*path)
                        || old_sources.get(*path) == current_sources.get(*path)
                });
                if !foreign_dependencies_match {
                    incomplete = true;
                    invalid_predecessor = true;
                    continue;
                }
                let at = slot
                    .t
                    .and_then(|time| {
                        slots
                            .iter()
                            .position(|current| current.t.is_some_and(|t| t > time))
                    })
                    .unwrap_or(slots.len());
                slots.insert(at, slot);
            }
        } else {
            incomplete = true;
            invalid_predecessor = true;
        }
        if let Some(cwds) = capture_cwds.as_mut() {
            for source in missing.values() {
                let mut retained_cwd = None;
                let mut ambiguous = false;
                let mut cwd_receipt_ids = BTreeSet::new();
                for receipt in snapshot.receipts.iter().filter(|receipt| {
                    receipt.sources.iter().any(|saved| saved == source)
                        && crate::retention::source_generation(&receipt.sources)
                            .ok()
                            .as_deref()
                            == Some(receipt.source_generation.as_str())
                        && crate::native_evidence::runtime::cwd_fingerprint(
                            &receipt.cwds.values().filter_map(Clone::clone).collect(),
                        ) == receipt.cwd_fingerprint
                }) {
                    let Some(value) = receipt.cwds.get(&source.path) else {
                        ambiguous = true;
                        break;
                    };
                    if retained_cwd.as_ref().is_some_and(|saved| saved != value) {
                        ambiguous = true;
                        break;
                    }
                    retained_cwd = Some(value.clone());
                    cwd_receipt_ids.insert(receipt.id.clone());
                }
                match (retained_cwd, ambiguous) {
                    (Some(Some(cwd)), false) => {
                        cwds.insert(cwd);
                        receipt_ids.extend(cwd_receipt_ids);
                    }
                    (Some(None), false) => receipt_ids.extend(cwd_receipt_ids),
                    _ => {
                        invalid_predecessor = true;
                        capture_cwds = None;
                        break;
                    }
                }
            }
        }
        if snapshot.history_exists && invalid_predecessor {
            steps.push(HistoryCompositionStep::Preserve {
                key: current.key.clone(),
            });
            continue;
        }
        if invalid_predecessor {
            slots = projection.slots.clone();
            receipt_ids.clear();
            incomplete = true;
        }
        let mut seen = BTreeSet::new();
        slots.retain(|slot| seen.insert(slot.identity()));
        let texts: Vec<String> = slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()?;
        let sources = crate::retention::source_generation(&history.sources)
            .map_err(|error| serde_json::Error::io(std::io::Error::other(error)))?;
        let metadata = serde_json::to_string(&history)?;
        let generation = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_string(&(VERSION, &sources, &texts, incomplete))?.as_bytes()
            )
        );
        steps.push(HistoryCompositionStep::Compose {
            key: current.key.clone(),
            previous: old_text.map(str::to_owned),
            generation,
            sources,
            metadata,
            slots: texts,
            incomplete,
            retained: missing.into_values().collect(),
            receipt_ids: receipt_ids.into_iter().collect(),
            capture_cwds,
        });
    }
    let preimage = serde_json::to_vec(&(
        rows.values()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?,
        projections
            .iter()
            .map(|projection| {
                Ok((
                    projection.key.clone(),
                    projection
                        .slots
                        .iter()
                        .map(serde_json::to_string)
                        .collect::<Result<Vec<_>, serde_json::Error>>()?,
                ))
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?,
        snapshots,
    ))?;
    Ok(HistoryCompositionPlan {
        preimage: format!("{:x}", Sha256::digest(preimage)),
        steps,
    })
}

pub(crate) fn prepare_history_composition<P: VerifiedHistoryCompositionStore>(
    store: &P,
    rows: &[CatalogRow],
    projections: Option<&[crate::slot_projection::Projection]>,
    observations: Option<&crate::storage::selected::PresentationObservations>,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<(HistoryCompositionPlan, HistoryCompositionCaptures), P::Error> {
    let snapshots = history_snapshots(store, rows, projections, budget)?;
    let plan = plan_history_rows(rows, projections, &snapshots).map_err(P::data)?;
    let captures = prepare_history_captures(&plan, observations, budget).map_err(P::data)?;
    Ok((plan, captures))
}

fn prepare_history_captures(
    plan: &HistoryCompositionPlan,
    observations: Option<&crate::storage::selected::PresentationObservations>,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<HistoryCompositionCaptures, serde_json::Error> {
    let Some(observations) = observations else {
        return Ok(HistoryCompositionCaptures::default());
    };
    let mut captures = HistoryCompositionCaptures::default();
    for step in &plan.steps {
        let (key, metadata, cwds) = match step {
            HistoryCompositionStep::Compose {
                key,
                metadata,
                capture_cwds,
                ..
            }
            | HistoryCompositionStep::Clear {
                key,
                metadata,
                capture_cwds,
            } => (key, metadata, capture_cwds.as_ref()),
            _ => continue,
        };
        let Some(cwds) = cwds else { continue };
        let row: CatalogRow = serde_json::from_str(metadata)?;
        let mut observed = observations.clone();
        let runtime = match observed.runtime_observation(&row, cwds) {
            Ok(runtime) => runtime,
            Err(_) => continue,
        };
        if observed.capture_runtime(&row, cwds, runtime).is_err() {
            continue;
        }
        let Some(capture) = observed.runtime_capture_for(&row) else {
            continue;
        };
        if !capture.is_runtime_capture_for(&row) {
            continue;
        }
        let encoded = serde_json::to_string(&capture)?;
        if budget.charge(encoded.len()).is_err() {
            continue;
        }
        captures.values.insert(key.clone(), encoded);
    }
    Ok(captures)
}

fn apply_history_composition<P: VerifiedHistoryCompositionStore>(
    store: &P,
    plan: &HistoryCompositionPlan,
    captures: &HistoryCompositionCaptures,
) -> Result<(), P::Error> {
    for step in &plan.steps {
        let key = match step {
            HistoryCompositionStep::Preserve { .. } => continue,
            HistoryCompositionStep::Clear { key, .. }
            | HistoryCompositionStep::Retain { key, .. }
            | HistoryCompositionStep::Compose { key, .. } => key,
        };
        if store.original(key)? {
            continue;
        }
        match step {
            HistoryCompositionStep::Preserve { .. } => unreachable!(),
            HistoryCompositionStep::Clear { key, .. } => {
                if captures.values.contains_key(key) {
                    store.clear(key)?;
                }
            }
            HistoryCompositionStep::Retain {
                key: _,
                previous,
                sources,
            } => {
                let row: CatalogRow = serde_json::from_str(previous).map_err(P::data)?;
                for source in sources {
                    store.retain_source(&row, source)?;
                }
            }
            HistoryCompositionStep::Compose {
                key,
                previous,
                generation,
                sources,
                metadata,
                slots,
                incomplete,
                retained,
                receipt_ids,
                ..
            } => {
                let Some(capture) = captures.values.get(key) else {
                    continue;
                };
                if let Some(previous) = previous {
                    let row: CatalogRow = serde_json::from_str(previous).map_err(P::data)?;
                    for source in retained {
                        store.retain_source(&row, source)?;
                    }
                }
                store.composed_verified(VerifiedHistoryComposition {
                    key,
                    generation,
                    sources,
                    metadata,
                    texts: slots,
                    incomplete: *incomplete,
                    receipt_ids,
                    capture,
                })?;
            }
        }
    }
    Ok(())
}

/// Build the approved plan before the SQL write transaction.
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
        if store.original(&current.key)? {
            continue;
        }
        let old_text = store.previous(&current.key)?;
        let old = old_text
            .as_deref()
            .and_then(|text| serde_json::from_str::<CatalogRow>(text).ok());
        if old
            .as_ref()
            .is_some_and(|old| old.harness != current.harness)
        {
            let old = old.as_ref().unwrap();
            for source in &old.sources {
                store.retain_source(old, source)?;
            }
            continue;
        }
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
        let Some(old) = old.as_ref() else {
            continue;
        };
        let mut history = current.clone();
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
            store.retain_source(old, source)?;
        }
        let mut slots = projection.slots.clone();
        let (recipe_history, header) = store.previous_recipe(&current.key)?;
        let qualified = header.as_ref().is_some_and(|(version, hash, _)| {
            *version == VERSION
                && hash.as_deref()
                    == crate::retention::source_generation(&old.sources)
                        .ok()
                        .as_deref()
        });
        let mut incomplete = !qualified;
        if qualified {
            incomplete = header.is_some_and(|(_, _, incomplete)| incomplete);
            let old_sources: BTreeMap<_, _> = old
                .sources
                .iter()
                .map(|source| (&source.path, source))
                .collect();
            let current_sources: BTreeMap<_, _> = current
                .sources
                .iter()
                .map(|source| (&source.path, source))
                .collect();
            for text in store.recipe_slots(&current.key, recipe_history)? {
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
        }
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

/// Recompute the deterministic plan inside the writer fence without changing
/// custody. Callers that have other custody work can compare first, complete
/// that work, and apply the prepared plan only after it succeeds.
pub(crate) fn history_composition_matches<P: VerifiedHistoryCompositionStore>(
    store: &P,
    rows: &[CatalogRow],
    projections: Option<&[crate::slot_projection::Projection]>,
    expected: &HistoryCompositionPlan,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<bool, P::Error> {
    let snapshots = history_snapshots(store, rows, projections, budget)?;
    let actual = plan_history_rows(rows, projections, &snapshots).map_err(P::data)?;
    Ok(&actual == expected)
}

pub(crate) fn apply_prepared_history_composition<P: VerifiedHistoryCompositionStore>(
    store: &P,
    plan: &HistoryCompositionPlan,
    captures: &HistoryCompositionCaptures,
) -> Result<(), P::Error> {
    apply_history_composition(store, plan, captures)
}

/// Recompute the deterministic plan inside the writer fence. A mismatch leaves
/// every existing History owner and slot untouched; Current can still publish.
#[cfg(test)]
pub(crate) fn publish_history_composition<P: VerifiedHistoryCompositionStore>(
    store: &P,
    rows: &[CatalogRow],
    projections: Option<&[crate::slot_projection::Projection]>,
    expected: &HistoryCompositionPlan,
    captures: &HistoryCompositionCaptures,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<bool, P::Error> {
    if !history_composition_matches(store, rows, projections, expected, budget)? {
        return Ok(false);
    }
    apply_history_composition(store, expected, captures)?;
    Ok(true)
}

#[cfg(test)]
mod history_composition_tests {
    use super::*;
    use crate::slot_projection::{File, Projection, Recipe, SavedSlot, VERSION};
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    fn source(path: &str, native_id: &str) -> CatalogSource {
        CatalogSource {
            path: PathBuf::from(path),
            native_id: native_id.to_owned(),
            dev: 1,
            ino: 1,
            size: 1,
            modified_ns: 1,
            changed_ns: None,
            offset: 1,
            prefix_sha256: [0; 32],
            tail_sha256: [0; 32],
            native_role: None,
        }
    }

    fn row(name: &str, sources: Vec<CatalogSource>) -> CatalogRow {
        let native_ids = sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        CatalogRow {
            overview: None,
            trace: None,
            query_recipe: None,
            query: None,
            lifecycle: crate::model::summary::CatalogLifecycle::Current,
            key: "lineage".into(),
            name: name.into(),
            harness: "codex".into(),
            kind: "session".into(),
            presentation_kind: None,
            native_ids,
            parent: None,
            parent_source: None,
            repo: None,
            branch: None,
            model: String::new(),
            effort: None,
            start: None,
            last: None,
            tokens: [0.0; 3],
            cost: crate::pricing::Cost::default(),
            sources,
        }
    }

    fn slot(path: &str, cwd: &str, time: i64) -> SavedSlot {
        SavedSlot {
            recipe: Recipe::U,
            file: Some(File {
                path: PathBuf::from(path),
                cwd: Some(cwd.to_owned()),
            }),
            offset: time as u64,
            block: 0,
            t: Some(time),
            turn: Some(format!("turn-{time}")),
            first: true,
            field: None,
            native_event_id: Some(format!("event-{time}")),
            images: Vec::new(),
        }
    }

    fn receipt(
        id: &str,
        sources: &[CatalogSource],
        slots: &[String],
        cwd_by_path: BTreeMap<PathBuf, Option<String>>,
    ) -> VerifiedHistoryReceipt {
        let cwds = cwd_by_path.values().filter_map(Clone::clone).collect();
        VerifiedHistoryReceipt::verified(
            id.to_owned(),
            crate::retention::source_generation(sources).unwrap(),
            sources.to_vec(),
            slots.to_vec(),
            crate::native_evidence::runtime::cwd_fingerprint(&cwds),
            cwd_by_path,
        )
    }

    fn compose_step(plan: &HistoryCompositionPlan) -> (&[String], &[String], &BTreeSet<String>) {
        let HistoryCompositionStep::Compose {
            receipt_ids,
            slots,
            capture_cwds: Some(cwds),
            ..
        } = &plan.steps[0]
        else {
            panic!("expected a capturable History composition")
        };
        (receipt_ids, slots, cwds)
    }

    struct PlanStore {
        snapshot: RefCell<HistoryCompositionSnapshot>,
        original: Cell<bool>,
        effects: Cell<usize>,
    }

    impl CompositeHistoryStore for PlanStore {
        type Error = ();
        fn data(_: serde_json::Error) {}
        fn original(&self, _: &str) -> Result<bool, Self::Error> {
            Ok(self.original.get())
        }
        fn previous(&self, _: &str) -> Result<Option<String>, Self::Error> {
            Ok(None)
        }
        fn history_exists(&self, _: &str) -> Result<bool, Self::Error> {
            Ok(false)
        }
        fn retain_source(&self, _: &CatalogRow, _: &CatalogSource) -> Result<(), Self::Error> {
            self.effects.set(self.effects.get() + 1);
            Ok(())
        }
        fn relocated(&self, _: &CatalogSource, _: &[CatalogSource]) -> Result<bool, Self::Error> {
            Ok(false)
        }
        fn clear(&self, _: &str) -> Result<(), Self::Error> {
            self.effects.set(self.effects.get() + 1);
            Ok(())
        }
        fn previous_recipe(
            &self,
            _: &str,
        ) -> Result<(bool, Option<HistoryRecipeHeader>), Self::Error> {
            Ok((false, None))
        }
        fn recipe_slots(&self, _: &str, _: bool) -> Result<Vec<String>, Self::Error> {
            Ok(Vec::new())
        }
        fn composed(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
            _: bool,
        ) -> Result<(), Self::Error> {
            self.effects.set(self.effects.get() + 1);
            Ok(())
        }
    }

    impl VerifiedHistoryCompositionStore for PlanStore {
        fn composition_snapshot(
            &self,
            _: &CatalogRow,
            _: &mut crate::overview::SummaryPublicationBudget,
        ) -> Result<HistoryCompositionSnapshot, Self::Error> {
            Ok(self.snapshot.borrow().clone())
        }
        fn composed_verified(&self, _: VerifiedHistoryComposition<'_>) -> Result<(), Self::Error> {
            self.effects.set(self.effects.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn repeated_disappearance_keeps_exact_slot_and_cwd_receipts() {
        let a = source("/sources/a.jsonl", "a");
        let b = source("/sources/b.jsonl", "b");
        let c = source("/sources/c.jsonl", "c");
        let original = row("original", vec![a.clone(), b.clone(), c.clone()]);
        let slot_a = slot("/sources/a.jsonl", "/cwd/a", 1);
        let slot_b = slot("/sources/b.jsonl", "/cwd/b", 2);
        let slot_c = slot("/sources/c.jsonl", "/cwd/c", 3);
        let slot_a_text = serde_json::to_string(&slot_a).unwrap();
        let slot_b_text = serde_json::to_string(&slot_b).unwrap();
        let slot_c_text = serde_json::to_string(&slot_c).unwrap();
        let cwd_by_path = BTreeMap::from([
            (a.path.clone(), Some("/cwd/a".into())),
            (b.path.clone(), Some("/cwd/b".into())),
            (c.path.clone(), Some("/cwd/c".into())),
        ]);
        let slot_receipt = receipt(
            "slot-receipt",
            &original.sources,
            &[slot_a_text.clone(), slot_b_text.clone()],
            cwd_by_path.clone(),
        );
        let cwd_only_receipt = receipt(
            "cwd-only-receipt",
            &original.sources,
            &[],
            cwd_by_path.clone(),
        );
        let receipt_ids = vec![slot_receipt, cwd_only_receipt];

        let first_current = row("first-current", vec![b.clone(), c.clone()]);
        let first_projection = Projection {
            key: first_current.key.clone(),
            slots: vec![slot_b.clone(), slot_c.clone()],
            repair: false,
        };
        let first_snapshot = HistoryCompositionSnapshot {
            key: first_current.key.clone(),
            original: false,
            previous: Some(serde_json::to_string(&original).unwrap()),
            history_exists: false,
            recipe_history: false,
            recipe_header: Some((
                VERSION,
                Some(crate::retention::source_generation(&original.sources).unwrap()),
                false,
            )),
            recipe_slots: vec![slot_a_text.clone(), slot_b_text.clone()],
            relocated: BTreeSet::new(),
            receipts: receipt_ids.clone(),
            current_cwds: Some(BTreeSet::from(["/cwd/b".into(), "/cwd/c".into()])),
            unavailable: false,
        };
        let first_plan = plan_history_rows(
            std::slice::from_ref(&first_current),
            Some(std::slice::from_ref(&first_projection)),
            std::slice::from_ref(&first_snapshot),
        )
        .unwrap();
        let (first_ids, first_slots, first_cwds) = compose_step(&first_plan);
        assert_eq!(
            first_ids,
            &["cwd-only-receipt".to_owned(), "slot-receipt".to_owned()]
        );
        assert_eq!(
            first_slots,
            &[
                slot_a_text.clone(),
                slot_b_text.clone(),
                slot_c_text.clone()
            ]
        );
        assert_eq!(
            first_cwds,
            &BTreeSet::from(["/cwd/a".into(), "/cwd/b".into(), "/cwd/c".into()])
        );
        let HistoryCompositionStep::Compose {
            metadata,
            generation,
            ..
        } = &first_plan.steps[0]
        else {
            unreachable!()
        };
        let first_history: CatalogRow = serde_json::from_str(metadata).unwrap();
        let first_generation = generation.clone();

        let metadata_only_current = row("metadata-only", vec![b.clone(), c.clone()]);
        let metadata_only_plan = plan_history_rows(
            std::slice::from_ref(&metadata_only_current),
            Some(std::slice::from_ref(&first_projection)),
            std::slice::from_ref(&first_snapshot),
        )
        .unwrap();
        let HistoryCompositionStep::Compose {
            metadata: metadata_only_metadata,
            generation: metadata_only_generation,
            ..
        } = &metadata_only_plan.steps[0]
        else {
            unreachable!()
        };
        assert_eq!(metadata_only_generation, &first_generation);
        assert_ne!(metadata_only_metadata, metadata);

        let second_current = row("second-current", vec![c.clone()]);
        let second_projection = Projection {
            key: second_current.key.clone(),
            slots: vec![slot_c],
            repair: false,
        };
        let second_snapshot = HistoryCompositionSnapshot {
            key: second_current.key.clone(),
            original: false,
            previous: Some(serde_json::to_string(&first_history).unwrap()),
            history_exists: true,
            recipe_history: true,
            recipe_header: Some((
                VERSION,
                Some(crate::retention::source_generation(&first_history.sources).unwrap()),
                false,
            )),
            recipe_slots: first_slots.to_vec(),
            relocated: BTreeSet::new(),
            receipts: receipt_ids,
            current_cwds: Some(BTreeSet::from(["/cwd/c".into()])),
            unavailable: false,
        };
        let second_plan = plan_history_rows(
            std::slice::from_ref(&second_current),
            Some(std::slice::from_ref(&second_projection)),
            &[second_snapshot],
        )
        .unwrap();
        let (second_ids, second_slots, second_cwds) = compose_step(&second_plan);
        assert_eq!(
            second_ids,
            &["cwd-only-receipt".to_owned(), "slot-receipt".to_owned()]
        );
        assert_eq!(second_slots, &[slot_a_text, slot_b_text, slot_c_text]);
        assert_eq!(
            second_cwds,
            &BTreeSet::from(["/cwd/a".into(), "/cwd/b".into(), "/cwd/c".into()])
        );
        let HistoryCompositionStep::Compose { generation, .. } = &second_plan.steps[0] else {
            unreachable!()
        };
        assert_ne!(generation, &first_generation);
    }

    #[test]
    fn unsupported_history_without_local_cwd_receipt_is_preserved() {
        let a = source("/sources/a.jsonl", "a");
        let b = source("/sources/b.jsonl", "b");
        let original = row("original", vec![a.clone(), b.clone()]);
        let previous = serde_json::to_string(&original).unwrap();
        let slot_a = slot("/sources/a.jsonl", "/cwd/a", 1);
        let current = row("current", vec![b.clone()]);
        let projection = Projection {
            key: current.key.clone(),
            slots: Vec::new(),
            repair: false,
        };
        let slot_text = serde_json::to_string(&slot_a).unwrap();
        let snapshot = HistoryCompositionSnapshot {
            key: current.key.clone(),
            original: false,
            previous: Some(previous),
            history_exists: true,
            recipe_history: true,
            recipe_header: Some((
                VERSION,
                Some(crate::retention::source_generation(&original.sources).unwrap()),
                false,
            )),
            recipe_slots: vec![slot_text.clone()],
            relocated: BTreeSet::new(),
            receipts: Vec::new(),
            current_cwds: Some(BTreeSet::from(["/cwd/b".into()])),
            unavailable: false,
        };
        let mut wrong_receipt = receipt(
            "wrong-cwd-fingerprint",
            &original.sources,
            &[slot_text],
            BTreeMap::from([
                (a.path.clone(), Some("/cwd/a".into())),
                (b.path.clone(), Some("/cwd/b".into())),
            ]),
        );
        wrong_receipt.cwd_fingerprint = "wrong fingerprint".into();
        for receipts in [Vec::new(), vec![wrong_receipt]] {
            let mut candidate = snapshot.clone();
            candidate.receipts = receipts;
            let plan = plan_history_rows(
                std::slice::from_ref(&current),
                Some(std::slice::from_ref(&projection)),
                &[candidate],
            )
            .unwrap();
            assert!(matches!(
                plan.steps.as_slice(),
                [HistoryCompositionStep::Preserve { .. }]
            ));
        }
    }

    #[test]
    fn stale_preimage_and_new_original_marker_have_no_history_effects() {
        let a = source("/sources/a.jsonl", "a");
        let b = source("/sources/b.jsonl", "b");
        let original = row("original", vec![a.clone(), b.clone()]);
        let current = row("current", vec![b.clone()]);
        let slot_a = slot("/sources/a.jsonl", "/cwd/a", 1);
        let slot_b = slot("/sources/b.jsonl", "/cwd/b", 2);
        let receipt = receipt(
            "receipt-a-b",
            &original.sources,
            &[serde_json::to_string(&slot_a).unwrap()],
            BTreeMap::from([
                (a.path.clone(), Some("/cwd/a".into())),
                (b.path.clone(), Some("/cwd/b".into())),
            ]),
        );
        let snapshot = HistoryCompositionSnapshot {
            key: current.key.clone(),
            original: false,
            previous: Some(serde_json::to_string(&original).unwrap()),
            history_exists: false,
            recipe_history: false,
            recipe_header: Some((
                VERSION,
                Some(crate::retention::source_generation(&original.sources).unwrap()),
                false,
            )),
            recipe_slots: vec![serde_json::to_string(&slot_a).unwrap()],
            relocated: BTreeSet::new(),
            receipts: vec![receipt],
            current_cwds: Some(BTreeSet::from(["/cwd/b".into()])),
            unavailable: false,
        };
        let projection = Projection {
            key: current.key.clone(),
            slots: vec![slot_b],
            repair: false,
        };
        let store = PlanStore {
            snapshot: RefCell::new(snapshot.clone()),
            original: Cell::new(false),
            effects: Cell::new(0),
        };
        let mut budget = crate::overview::SummaryPublicationBudget::after_catalog_metadata(0);
        let (plan, mut captures) = prepare_history_composition(
            &store,
            std::slice::from_ref(&current),
            Some(std::slice::from_ref(&projection)),
            None,
            &mut budget,
        )
        .unwrap();
        captures
            .values
            .insert(current.key.clone(), "capture".into());
        assert!(matches!(
            plan.steps.as_slice(),
            [HistoryCompositionStep::Compose { .. }]
        ));

        store.snapshot.borrow_mut().receipts.clear();
        let mut writer_budget =
            crate::overview::SummaryPublicationBudget::after_catalog_metadata(0);
        assert!(
            !publish_history_composition(
                &store,
                std::slice::from_ref(&current),
                Some(std::slice::from_ref(&projection)),
                &plan,
                &captures,
                &mut writer_budget,
            )
            .unwrap()
        );
        assert_eq!(store.effects.get(), 0, "stale plans cannot change History");

        let original_store = PlanStore {
            snapshot: RefCell::new(snapshot),
            original: Cell::new(true),
            effects: Cell::new(0),
        };
        apply_prepared_history_composition(&original_store, &plan, &captures).unwrap();
        assert_eq!(
            original_store.effects.get(),
            0,
            "custody completed after planning must keep original History immutable"
        );
    }
}
