//! E2B resource reconciliation before any compute or guest side effect.
//!
//! The embedding owns credentials, SDK transport, complete pagination and CAS.
//! This module plans only; a returned claim must be committed before create.
use crate::{Action, Compute, Desired, OperationId, OwnerId, Progress, RuntimeId, Session};
use std::collections::BTreeMap;
use thiserror::Error;

#[cfg(feature = "e2b-process")]
pub mod process;

/// Provider metadata contains stable identities, never prompts or credentials.
pub type Labels = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: RuntimeId,
    pub labels: Labels,
    pub state: ResourceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    Running,
    Paused,
    Unknown,
}

/// A complete query must include both running and paused resources and consume
/// every page. A page/retry limit or an unavailable API cannot establish absence.
pub enum Inventory<'a> {
    Complete(&'a [Resource]),
    Incomplete,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchPlan {
    /// CAS this operation from Prepared to InFlight at the supplied revision.
    /// Only the winner may make one create call with these labels. Never repeat
    /// it after timeout, restart, or an uncommitted/unknown provider response.
    ClaimCreate {
        revision: u64,
        epoch: u64,
        labels: Labels,
    },
    /// Reuse the exact owned resource; this does not imply bootstrap/model health.
    Adopt { id: RuntimeId, compute: Compute },
    /// Retain the durable operation and inspect again; do not create or destroy.
    InspectAgain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ReconcileError {
    #[error("invalid persisted launch or deployment identity")]
    InvalidLaunch,
    #[error("resource ownership differs from the persisted operation")]
    OwnershipMismatch,
    #[error("multiple resources match the persisted operation")]
    DuplicateResources,
}

/// Plan from a freshly authorized persisted aggregate and provider inventory.
/// Caller rechecks connection usability and commits with owner/revision/epoch
/// CAS. Metadata is provider evidence, not an authorization grant by itself.
pub fn reconcile_launch(
    deployment: &str,
    session: &Session,
    operation: &OperationId,
    inventory: Inventory<'_>,
) -> Result<LaunchPlan, ReconcileError> {
    OwnerId::new(deployment).map_err(|_| ReconcileError::InvalidLaunch)?;
    session
        .validate()
        .map_err(|_| ReconcileError::InvalidLaunch)?;
    let op = session
        .operations()
        .iter()
        .find(|op| &op.id == operation)
        .ok_or(ReconcileError::InvalidLaunch)?;
    let attempt = session
        .attempts()
        .last()
        .ok_or(ReconcileError::InvalidLaunch)?;
    if session.desired() != Desired::Active
        || op.action != Action::Launch
        || op.epoch != session.epoch()
        || attempt.epoch != op.epoch
        || &attempt.operation != operation
        || attempt.writer_excluded
        || !matches!(
            op.progress,
            Progress::Prepared | Progress::InFlight | Progress::OutcomeUnknown
        )
    {
        return Err(ReconcileError::InvalidLaunch);
    }
    let labels = ownership_labels(deployment, session, operation)?;
    let Inventory::Complete(resources) = inventory else {
        return Ok(LaunchPlan::InspectAgain);
    };
    let mut found = None;
    for resource in resources {
        let related = ["semon_deployment", "semon_session", "semon_operation"]
            .iter()
            .all(|key| resource.labels.get(*key) == labels.get(*key));
        let bound = attempt.runtime.as_ref() == Some(&resource.id);
        if !related && !bound {
            continue;
        }
        if !labels
            .iter()
            .all(|(key, value)| resource.labels.get(key) == Some(value))
        {
            return Err(ReconcileError::OwnershipMismatch);
        }
        if attempt
            .runtime
            .as_ref()
            .is_some_and(|id| id != &resource.id)
        {
            return Err(ReconcileError::OwnershipMismatch);
        }
        if found.replace(resource).is_some() {
            return Err(ReconcileError::DuplicateResources);
        }
    }
    if let Some(resource) = found {
        return Ok(match resource.state {
            ResourceState::Running => LaunchPlan::Adopt {
                id: resource.id.clone(),
                compute: Compute::Active,
            },
            ResourceState::Paused => LaunchPlan::Adopt {
                id: resource.id.clone(),
                compute: Compute::Paused,
            },
            ResourceState::Unknown => LaunchPlan::InspectAgain,
        });
    }
    // Absence never proves that an uncertain create had no effect. Also do not
    // replace a bound resource: qualified recovery/writer fencing is separate.
    if op.progress != Progress::Prepared || attempt.runtime.is_some() {
        return Ok(LaunchPlan::InspectAgain);
    }
    Ok(LaunchPlan::ClaimCreate {
        revision: session.revision(),
        epoch: op.epoch,
        labels,
    })
}

/// Exact provider ownership labels for an already persisted current attempt.
/// Useful for private guest calls after launch completes; labels grant no authority.
pub fn ownership_labels(
    deployment: &str,
    session: &Session,
    operation: &OperationId,
) -> Result<Labels, ReconcileError> {
    OwnerId::new(deployment).map_err(|_| ReconcileError::InvalidLaunch)?;
    session
        .validate()
        .map_err(|_| ReconcileError::InvalidLaunch)?;
    let attempt = session
        .attempts()
        .last()
        .ok_or(ReconcileError::InvalidLaunch)?;
    if &attempt.operation != operation
        || attempt.epoch != session.epoch()
        || attempt.writer_excluded
    {
        return Err(ReconcileError::InvalidLaunch);
    }
    Ok([
        ("semon_version", "1".to_owned()),
        ("semon_deployment", deployment.to_owned()),
        ("semon_owner", session.binding().owner.as_str().to_owned()),
        (
            "semon_workspace",
            session.binding().workspace.as_str().to_owned(),
        ),
        ("semon_session", session.id().as_str().to_owned()),
        ("semon_operation", operation.as_str().to_owned()),
        ("semon_epoch", session.epoch().to_string()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect())
}
