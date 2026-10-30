//! Native commit ordering and relationship semantics shared by physical adapters.
//! The caller owns admission, ledger CAS, rollback and the durable operation.
//! None of these stages, individually, acknowledges a complete native ingest.
use std::path::Path;

use super::{DirtyRows, FileIndex, Ledger};

/// Shared cold-publication plan: all entries are complete, distinct sources
/// and still name the physical witness parsed by the ordinary scanner. Both
/// engines apply this before any family effect, in their writer transaction.
pub(crate) fn initial_batch(
    files: &[(String, Ledger, std::sync::Arc<FileIndex>)],
) -> Result<bool, &'static str> {
    if files.len() > 64 {
        return Err("initial index batch exceeds source budget");
    }
    let mut paths = std::collections::BTreeSet::new();
    for (path, ledger, index) in files {
        if !paths.insert(path) {
            return Err("initial index batch repeats a source");
        }
        let physical = index
            .native_path
            .as_deref()
            .unwrap_or_else(|| Path::new(path));
        if physical.to_string_lossy() != path.as_str()
            || ledger.offset != ledger.stat.size
            || super::Stat::of(physical).ok().as_ref() != Some(&ledger.stat)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) struct Input<'a> {
    pub(crate) path: &'a str,
    pub(crate) ledger: &'a Ledger,
    pub(crate) index: &'a FileIndex,
    pub(crate) changes: Option<&'a DirtyRows>,
    pub(crate) expected: Option<&'a Ledger>,
    pub(crate) semantic_change: bool,
}

// Private domain stages, not an arbitrary SQL or public backend interface.
pub(crate) trait Effects {
    type Error;
    fn begin(&mut self, input: &Input<'_>) -> Result<(), Self::Error>;
    fn planning_started(&mut self);
    fn planning_finished(&mut self);
    fn file(&mut self, input: &Input<'_>, changes: &DirtyRows) -> Result<(), Self::Error>;
    fn usage(&mut self, input: &Input<'_>) -> Result<(), Self::Error>;
    fn tree(&mut self, input: &Input<'_>) -> Result<(), Self::Error>;
    fn auxiliary(&mut self, input: &Input<'_>, stage: Auxiliary) -> Result<(), Self::Error>;
    fn relationships(&mut self, input: &Input<'_>) -> Result<(), Self::Error>;
    fn archive(&mut self, path: &str) -> Result<(), Self::Error>;
    fn invalidate(&mut self, path: &str, changed: bool) -> Result<(), Self::Error>;
    fn retire_mutation(&mut self, path: &str) -> Result<(), Self::Error>;
}

pub(crate) fn write<E: Effects>(effects: &mut E, input: Input<'_>) -> Result<(), E::Error> {
    effects.begin(&input)?;
    effects.planning_started();
    let full = if input.changes.is_none() {
        super::native_rows::full_changes(input.index)
    } else {
        DirtyRows::default()
    };
    effects.planning_finished();
    effects.file(&input, input.changes.unwrap_or(&full))?;
    effects.usage(&input)?;
    effects.tree(&input)?;
    for stage in AUXILIARY_ORDER {
        effects.auxiliary(&input, stage)?;
    }
    effects.invalidate(input.path, input.semantic_change)?;
    effects.retire_mutation(input.path)?;
    effects.relationships(&input)?;
    effects.archive(input.path)
}

/// Physical families have one order in both adapters. Their effects remain
/// private to the caller's transaction; none publishes a complete ingest.
#[derive(Clone, Copy)]
pub(crate) enum Auxiliary {
    Header,
    Sidecar,
    Chunks,
    Parent,
    Launch,
}

pub(crate) const AUXILIARY_ORDER: [Auxiliary; 5] = [
    Auxiliary::Header,
    Auxiliary::Sidecar,
    Auxiliary::Chunks,
    Auxiliary::Parent,
    Auxiliary::Launch,
];

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogRelocation {
    pub(crate) version: u32,
    pub(crate) new_path: String,
    pub(crate) native_id: String,
    pub(crate) prior: Ledger,
    pub(crate) catalog_before: Option<Ledger>,
    pub(crate) after: Ledger,
}

/// Advance only evidence already qualified by the relocation producer. This
/// does not qualify a move, read Raw, or infer identity from a path/inode.
pub(crate) fn advance_relocation(
    encoded: Option<&str>,
    expected: Option<&Ledger>,
    next: &Ledger,
    append: bool,
) -> serde_json::Result<Option<String>> {
    let proof = encoded.and_then(|value| serde_json::from_str::<CatalogRelocation>(value).ok());
    let Some(mut proof) =
        proof.filter(|proof| proof.version == 1 && Some(&proof.after) == expected)
    else {
        return Ok(None);
    };
    if !append {
        return Ok(if next == &proof.after {
            encoded.map(str::to_owned)
        } else {
            None
        });
    }
    proof.after = next.clone();
    serde_json::to_string(&proof).map(Some)
}

#[derive(Clone, Copy)]
pub(crate) enum RelationshipGroup {
    Lineage,
    Semantic,
}

pub(crate) trait Relationships {
    type Error;
    fn metadata_error(error: serde_json::Error) -> Self::Error;
    fn clear(&mut self, path: &str, group: RelationshipGroup) -> Result<(), Self::Error>;
    fn put(&mut self, path: &str, harness: &str, id: &str) -> Result<(), Self::Error>;
    fn finished(&mut self, group: RelationshipGroup) -> Result<(), Self::Error>;
}

/// One recipe for Claude lineage/bridge and Codex native/parent dependencies.
/// Physical adapters preserve unrelated dependency families and deduplicate keys.
pub(crate) fn relationships<E: Relationships>(
    effects: &mut E,
    path: &str,
    index: &FileIndex,
) -> Result<(), E::Error> {
    effects.clear(path, RelationshipGroup::Lineage)?;
    for id in index
        .links
        .session_ids
        .iter()
        .chain(&index.links.continued_in)
    {
        effects.put(path, "lineage:claude", id)?;
    }
    for id in &index.links.bridges {
        effects.put(path, "bridge:claude", id)?;
    }
    effects.finished(RelationshipGroup::Lineage)?;
    effects.clear(path, RelationshipGroup::Semantic)?;
    if let Some(header) = &index.codex_header {
        if let Some(id) = header.native_id(Path::new(path)) {
            effects.put(path, "semantic-native:codex", &id)?;
        }
        let metadata = serde_json::to_value(&header.metadata).map_err(E::metadata_error)?;
        if metadata
            .get("guardian_review")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
            && let Some(parent) = metadata
                .get("parent_thread")
                .and_then(serde_json::Value::as_str)
        {
            effects.put(path, "semantic-parent:codex", parent)?;
        }
    }
    effects.finished(RelationshipGroup::Semantic)
}

/// Native archive relocation is restricted to one harness home and opposite
/// active/archive collections. Both physical adapters use this contract.
pub(crate) fn relocation_namespace(path: &str) -> Option<(&str, bool)> {
    path.rsplit_once("/archived_sessions/")
        .map(|(root, _)| (root, true))
        .or_else(|| {
            path.rsplit_once("/sessions/")
                .map(|(root, _)| (root, false))
        })
}

#[cfg(test)]
mod relocation_tests {
    use super::{CatalogRelocation, Ledger, advance_relocation};
    use crate::events::Stat;

    fn ledger(n: u8) -> Ledger {
        Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: 3 + u64::from(n),
                modified_ns: 10 + u128::from(n),
                changed_ns: Some(20 + i128::from(n)),
            },
            offset: 3 + u64::from(n),
            prefix: [n; 32],
            tail: [n.wrapping_add(1); 32],
        }
    }

    fn encoded(after: Ledger) -> String {
        serde_json::to_string(&CatalogRelocation {
            version: 1,
            new_path: "archived/session.jsonl".into(),
            native_id: "session".into(),
            prior: ledger(0),
            catalog_before: None,
            after,
        })
        .unwrap()
    }

    #[test]
    fn unchanged_relocation_replay_retains_the_qualified_witness() {
        let after = ledger(1);
        let encoded = encoded(after.clone());

        let retained = advance_relocation(Some(&encoded), Some(&after), &after, false).unwrap();

        assert_eq!(retained.as_deref(), Some(encoded.as_str()));
    }

    #[test]
    fn relocation_witness_mismatches_clear_and_valid_appends_advance() {
        let after = ledger(1);
        let changed = ledger(2);
        let encoded = encoded(after.clone());

        assert!(
            advance_relocation(Some(&encoded), None, &after, false)
                .unwrap()
                .is_none()
        );
        assert!(
            advance_relocation(Some(&encoded), Some(&changed), &after, false)
                .unwrap()
                .is_none()
        );
        assert!(
            advance_relocation(Some(&encoded), Some(&after), &changed, false)
                .unwrap()
                .is_none()
        );
        assert!(
            advance_relocation(Some(&encoded), Some(&changed), &changed, true)
                .unwrap()
                .is_none()
        );

        let advanced = advance_relocation(Some(&encoded), Some(&after), &changed, true)
            .unwrap()
            .unwrap();
        let advanced: CatalogRelocation = serde_json::from_str(&advanced).unwrap();
        assert_eq!(advanced.after, changed);
        assert_eq!(advanced.new_path, "archived/session.jsonl");
        assert_eq!(advanced.native_id, "session");
    }
}
