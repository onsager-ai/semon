//! Durable Source work, independent of Catalog publication and physical IDs.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SourceJournalRecord {
    State {
        revision: i64,
        backfill: Option<String>,
    },
    Selected {
        value: String,
    },
    Claim {
        path: String,
        revision: i64,
        reason: String,
        observed_at: i64,
    },
    Member {
        path: String,
    },
    Intent {
        payload: String,
    },
    Receipt {
        operation: super::super::source_observations::SourceOperation,
        outcome: String,
        recovery_done: Option<bool>,
    },
}

impl SourceJournalRecord {
    pub(crate) fn category(&self) -> Category {
        match self {
            Self::State { .. } => Category::SourceState,
            Self::Selected { .. } => Category::StagedWork,
            Self::Claim { .. } => Category::Claims,
            Self::Member { .. } => Category::SourceMembers,
            Self::Intent { .. } => Category::SourceObservations,
            Self::Receipt { .. } => Category::Receipts,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), StorageError> {
        match self {
            Self::Intent { payload } => {
                let intent: super::super::source_observations::SourceIntent =
                    serde_json::from_str(payload).map_err(|_| StorageError::ScopeChanged)?;
                super::super::source_observations::validate_intent(
                    Some(&intent.operation().namespace), &intent,
                )?;
            }
            Self::Receipt { operation, outcome, .. } => {
                if !super::super::source_observations::operation_identity_matches(
                    operation, Some(&operation.namespace),
                )? || !["applied", "conflict"].contains(&outcome.as_str()) {
                    return Err(StorageError::ScopeChanged);
                }
            }
            Self::State { revision, backfill } => {
                if *revision < 0
                    || backfill
                        .as_deref()
                        .is_some_and(|v| !["discovering", "draining", "complete"].contains(&v))
                {
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

    pub(crate) fn validate_namespace(&self, namespace: &str) -> Result<(), StorageError> {
        self.validate()?;
        let valid = match self {
            Self::Intent { payload } => {
                let intent: super::super::source_observations::SourceIntent =
                    serde_json::from_str(payload).map_err(|_| StorageError::ScopeChanged)?;
                intent.operation().namespace == namespace
            }
            Self::Receipt { operation, .. } => operation.namespace == namespace,
            _ => true,
        };
        if valid { Ok(()) } else { Err(StorageError::ScopeChanged) }
    }
}
