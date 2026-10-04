//! Bounded private stdio transport to the pinned official-SDK inventory worker.
//! Authorization, credential resolution and durable claims belong to the host.
use super::{Inventory, Resource, reconcile_launch};
use crate::{OperationId, OwnerId, Session};
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::{Instant, timeout, timeout_at},
};
use zeroize::Zeroizing;

const MAX_OUTPUT: u64 = 2 * 1024 * 1024;
const MAX_RESOURCES: usize = 1000;
const LABEL_KEYS: [&str; 7] = [
    "semon_version",
    "semon_deployment",
    "semon_owner",
    "semon_workspace",
    "semon_session",
    "semon_operation",
    "semon_epoch",
];

pub enum OwnedInventory {
    Complete(Vec<Resource>),
    Incomplete,
    Unavailable,
}

/// Basic list-API access only; does not qualify create/template/lifecycle support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialValidation {
    Accepted,
    Rejected,
    Unavailable,
}
impl OwnedInventory {
    pub fn as_inventory(&self) -> Inventory<'_> {
        match self {
            Self::Complete(resources) => Inventory::Complete(resources),
            Self::Incomplete => Inventory::Incomplete,
            Self::Unavailable => Inventory::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ProcessError {
    #[error("invalid E2B inventory configuration or persisted launch")]
    Configuration,
    #[error("E2B inventory process unavailable")]
    Unavailable,
    #[error("E2B inventory process deadline exceeded")]
    Timeout,
    #[error("invalid E2B inventory process response")]
    Protocol,
}

// No Debug: the borrowed key and serialized request never enter diagnostics.
#[derive(Serialize)]
struct Request<'a> {
    version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<&'static str>,
    scope: std::collections::BTreeMap<&'static str, &'a str>,
    api_key: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u8,
    status: Status,
    resources: Option<Vec<Resource>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Complete,
    Incomplete,
    Unavailable,
    CredentialValid,
    CredentialRejected,
}

fn decode(response: Response) -> Result<OwnedInventory, ProcessError> {
    if response.version != 1 {
        return Err(ProcessError::Protocol);
    }
    match (response.status, response.resources) {
        (Status::Complete, Some(resources)) => {
            if resources.len() > MAX_RESOURCES
                || resources.iter().any(|resource| {
                    resource.labels.iter().any(|(key, value)| {
                        !LABEL_KEYS.contains(&key.as_str()) || OwnerId::new(value).is_err()
                    })
                })
            {
                return Err(ProcessError::Protocol);
            }
            Ok(OwnedInventory::Complete(resources))
        }
        (Status::Incomplete, None) => Ok(OwnedInventory::Incomplete),
        (Status::Unavailable, None) => Ok(OwnedInventory::Unavailable),
        _ => Err(ProcessError::Protocol),
    }
}

/// Inspect only. The host supplies its trusted absolute Python/worker paths,
/// freshly authorized persisted launch and owner-resolved key; it must recheck
/// authority before adopting a result or committing a create claim. A transport
/// error never establishes absence. No provider/guest mutation is performed.
pub struct InventoryWorker<'a> {
    pub python: &'a Path,
    pub worker: &'a Path,
    pub lifetime: Duration,
    pub environment_exclusions: &'a [OsString],
}

impl InventoryWorker<'_> {
    pub async fn inspect(
        &self,
        api_key: &str,
        deployment: &str,
        session: &Session,
        operation: &OperationId,
    ) -> Result<OwnedInventory, ProcessError> {
        if !self.python.is_absolute()
            || !self.worker.is_absolute()
            || api_key.is_empty()
            || api_key.len() > 4096
            || self.lifetime.is_zero()
            || self.lifetime > Duration::from_secs(25)
            || reconcile_launch(deployment, session, operation, Inventory::Unavailable).is_err()
        {
            return Err(ProcessError::Configuration);
        }
        let request = Request {
            version: 1,
            method: None,
            api_key,
            scope: [
                ("semon_deployment", deployment),
                ("semon_session", session.id().as_str()),
                ("semon_operation", operation.as_str()),
            ]
            .into_iter()
            .collect(),
        };
        let inventory = decode(self.execute(&request).await?)?;
        if let OwnedInventory::Complete(resources) = &inventory
            && resources.iter().any(|resource| {
                request.scope.iter().any(|(key, value)| {
                    resource.labels.get(*key).map(String::as_str) != Some(*value)
                })
            })
        {
            return Err(ProcessError::Protocol);
        }
        Ok(inventory)
    }

    /// Validate an owner's connection with one read-only list request. No durable
    /// launch or compute is required. The host rechecks cookie, membership, scope
    /// and connection generation before committing this result or making use of
    /// the key. Account/template/create capabilities remain independently gated.
    pub async fn validate_credential(
        &self,
        api_key: &str,
        deployment: &str,
        owner: &OwnerId,
    ) -> Result<CredentialValidation, ProcessError> {
        OwnerId::new(deployment).map_err(|_| ProcessError::Configuration)?;
        let request = Request {
            version: 1,
            method: Some("validate_credential"),
            api_key,
            scope: [
                ("semon_deployment", deployment),
                ("semon_owner", owner.as_str()),
            ]
            .into_iter()
            .collect(),
        };
        let response = self.execute(&request).await?;
        if response.version != 1 || response.resources.is_some() {
            return Err(ProcessError::Protocol);
        }
        match response.status {
            Status::CredentialValid => Ok(CredentialValidation::Accepted),
            Status::CredentialRejected => Ok(CredentialValidation::Rejected),
            Status::Unavailable => Ok(CredentialValidation::Unavailable),
            _ => Err(ProcessError::Protocol),
        }
    }

    async fn execute(&self, request: &Request<'_>) -> Result<Response, ProcessError> {
        if !self.python.is_absolute()
            || !self.worker.is_absolute()
            || request.api_key.is_empty()
            || request.api_key.len() > 4096
            || self.lifetime.is_zero()
            || self.lifetime > Duration::from_secs(25)
        {
            return Err(ProcessError::Configuration);
        }
        let input =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| ProcessError::Configuration)?);
        let deadline = Instant::now() + self.lifetime;
        let mut command = Command::new(self.python);
        for name in self.environment_exclusions {
            command.env_remove(name);
        }
        // Isolate Python imports and SDK endpoint/billing selection. The embedding's
        // proxy/CA/runtime network guards remain inherited, with TLS verification.
        for name in [
            "E2B_API_KEY",
            "E2B_ACCESS_TOKEN",
            "E2B_API_URL",
            "E2B_DOMAIN",
            "E2B_DEBUG",
            "E2B_SANDBOX_URL",
            "OPENAI_API_KEY",
            "CODEX_ACCESS_TOKEN",
        ] {
            command.env_remove(name);
        }
        command
            .args(["-I", "-B"])
            .arg(self.worker)
            .current_dir(self.worker.parent().ok_or(ProcessError::Configuration)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| ProcessError::Unavailable)?;
        let mut stdin = child.stdin.take().ok_or(ProcessError::Unavailable)?;
        let stdout = child.stdout.take().ok_or(ProcessError::Unavailable)?;
        let result = timeout_at(deadline, async {
            let mut output = Zeroizing::new(Vec::new());
            let (_, _) = tokio::try_join!(
                async {
                    stdin.write_all(&input).await?;
                    stdin.shutdown().await?;
                    // ChildStdin's shutdown alone need not close the pipe. The
                    // one-request worker reads to EOF, so drop it explicitly.
                    drop(stdin);
                    Ok::<(), std::io::Error>(())
                },
                async {
                    stdout
                        .take(MAX_OUTPUT + 1)
                        .read_to_end(&mut output)
                        .await
                        .map(|_| ())
                },
            )
            .map_err(|_| ProcessError::Unavailable)?;
            if output.len() as u64 > MAX_OUTPUT {
                return Err(ProcessError::Protocol);
            }
            let status = child.wait().await.map_err(|_| ProcessError::Unavailable)?;
            if !status.success() {
                return Err(ProcessError::Unavailable);
            }
            serde_json::from_slice(&output).map_err(|_| ProcessError::Protocol)
        })
        .await
        .unwrap_or(Err(ProcessError::Timeout));
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.start_kill();
            // Bound explicit reaping too; kill_on_drop remains the cancellation guard.
            let _ = timeout(Duration::from_secs(1), child.wait()).await;
        }
        result
    }
}
