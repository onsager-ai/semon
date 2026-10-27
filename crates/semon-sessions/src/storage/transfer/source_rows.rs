//! Durable Source work, independent of Catalog publication and physical IDs.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SourceJournalRecord {
    State { revision: i64, backfill: Option<String> },
    Selected { value: String },
    Claim { path: String, revision: i64, reason: String, observed_at: i64 },
    Member { path: String },
}

impl SourceJournalRecord {
    pub(crate) fn category(&self) -> Category {
        match self {
            Self::State { .. } => Category::SourceState,
            Self::Selected { .. } => Category::StagedWork,
            Self::Claim { .. } => Category::Claims,
            Self::Member { .. } => Category::SourceMembers,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), StorageError> {
        match self {
            Self::State { revision, backfill } => {
                if *revision < 0 || backfill.as_deref().is_some_and(|v| !["discovering", "draining", "complete"].contains(&v)) {
                    return Err(StorageError::ScopeChanged);
                }
            }
            Self::Selected { value } => {
                super::super::source_observations::selected_projection_paths(value.as_bytes())?;
            }
            Self::Claim { path, revision, .. } => {
                if path.is_empty() || *revision <= 0 {
                    return Err(StorageError::ScopeChanged);
                }
            }
            Self::Member { path } => {
                if path.is_empty() {
                    return Err(StorageError::ScopeChanged);
                }
            }
        }
        Ok(())
    }
}
