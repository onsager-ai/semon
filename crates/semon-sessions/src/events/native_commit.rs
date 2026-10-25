//! Native commit ordering and relationship semantics shared by physical adapters.
//! The caller owns admission, ledger CAS, rollback and the durable operation.
//! None of these stages, individually, acknowledges a complete native ingest.
use std::path::Path;

use super::{DirtyRows, FileIndex, Ledger};

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
    fn relationships(&mut self, input: &Input<'_>) -> Result<(), Self::Error>;
    fn archive(&mut self, path: &str) -> Result<(), Self::Error>;
    fn invalidate(&mut self, path: &str, changed: bool) -> Result<(), Self::Error>;
    fn retire_mutation(&mut self, path: &str) -> Result<(), Self::Error>;
}

pub(crate) fn write<E: Effects>(effects: &mut E, input: Input<'_>) -> Result<(), E::Error> {
    effects.begin(&input)?;
    effects.planning_started();
    let full = DirtyRows {
        usage: if input.changes.is_none() {
            input.index.usage_by_id.keys().cloned().collect()
        } else {
            Default::default()
        },
        ..DirtyRows::default()
    };
    effects.planning_finished();
    effects.file(&input, input.changes.unwrap_or(&full))?;
    effects.usage(&input)?;
    effects.relationships(&input)?;
    effects.archive(input.path)?;
    effects.invalidate(input.path, input.semantic_change)?;
    effects.retire_mutation(input.path)
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
