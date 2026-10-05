//! Separately authorized private-driver dispatch. Enrollment alone is insufficient.
//! The embedding commits coordinator intent before calls and owns current authority
//! and outbound policy. This adapter never launches compute or installs a harness.
use crate::{Credential, Error, HostKey, PrivateFiles, Target, run};
use semon_runtime::{OwnerId, Progress, Session};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub dispatch: bool,
    pub reconcile: bool,
    pub credential_repair: bool,
    pub revoke_execution: bool,
    pub pause: bool,
    pub snapshot: bool,
    pub restore: bool,
    pub replace: bool,
    pub delete_compute: bool,
}
/// Private-driver transport qualification is distinct from native/hosted adoption.
pub const CAPABILITIES: Capabilities = Capabilities {
    dispatch: true,
    reconcile: true,
    credential_repair: true,
    revoke_execution: true,
    pause: false,
    snapshot: false,
    restore: false,
    replace: false,
    delete_compute: false,
};

#[derive(Serialize)]
struct Scope {
    enrollment: String,
    authority: String,
    owner: String,
    workspace: String,
    connection: String,
    session: String,
    operation: String,
    epoch: u64,
    thread: String,
    model_connection: String,
    expires: u64,
}
/// Construct only after current owner/workspace authorization. No bearer secret.
/// A remote grant is separate from mirror bootstrap and permanently tombstoned
/// on revoke. Refreshing a lease cannot resurrect a revoked grant.
pub struct ModelCredential(pub Zeroizing<String>);

pub struct Execution {
    scope: Scope,
    input: Zeroizing<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Outcome {
    Unknown,
    Accepted,
    Rejected,
    Repaired,
    Reconnected,
    Revoked,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub thread: String,
    pub generation: String,
    pub connected: bool,
    pub writer_active: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub id: String,
    pub outcome: Outcome,
    pub writer_excluded: bool,
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
impl Execution {
    pub fn new(
        session: &Session,
        owner: &OwnerId,
        enrollment: &str,
        authority: &str,
        expires: u64,
    ) -> Result<Self, Error> {
        session.validate().map_err(|_| Error::Invalid)?;
        if session.desired() != semon_runtime::Desired::Active
            || session.attempts().last().is_none_or(|a| a.writer_excluded)
        {
            return Err(Error::Authority);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid)?
            .as_secs();
        if &session.binding().owner != owner
            || !uuid(enrollment)
            || !uuid(authority)
            || expires <= now
            || expires > now + 300
        {
            return Err(Error::Authority);
        }
        let operation = session.operations().last().ok_or(Error::Invalid)?;
        if !matches!(
            operation.progress,
            Progress::InFlight | Progress::OutcomeUnknown | Progress::Succeeded
        ) || operation.epoch != session.epoch()
        {
            return Err(Error::Invalid);
        }
        let thread = session.thread().ok_or(Error::Invalid)?;
        Ok(Self {
            scope: Scope {
                enrollment: enrollment.into(),
                authority: authority.into(),
                owner: owner.as_str().into(),
                workspace: session.binding().workspace.as_str().into(),
                connection: session.binding().provider_credential.name.as_str().into(),
                session: session.id().as_str().into(),
                operation: operation.id.as_str().into(),
                epoch: session.epoch(),
                thread: thread.as_str().into(),
                model_connection: session.binding().model_credential.name.as_str().into(),
                expires,
            },
            input: Zeroizing::new(operation.launch_input.clone().ok_or(Error::Invalid)?),
        })
    }
    /// Apply only inspected dispatch evidence, then CAS the Session. This performs
    /// no transport or replay and does not claim native writer shutdown.
    pub fn record_dispatch(
        &self,
        session: &mut Session,
        receipt: &Receipt,
    ) -> Result<(), semon_runtime::Error> {
        if session.id().as_str() != self.scope.session
            || session.epoch() != self.scope.epoch
            || receipt.id != self.dispatch_id()
            || session.binding().owner.as_str() != self.scope.owner
            || session.binding().workspace.as_str() != self.scope.workspace
            || session
                .operations()
                .last()
                .is_none_or(|o| o.id.as_str() != self.scope.operation)
        {
            return Err(semon_runtime::Error::IdentityChanged);
        }
        let progress = match receipt.outcome {
            Outcome::Accepted => Progress::Succeeded,
            Outcome::Rejected => Progress::Failed,
            Outcome::Unknown => Progress::OutcomeUnknown,
            _ => return Err(semon_runtime::Error::IllegalTransition),
        };
        let id = semon_runtime::OperationId::new(&self.scope.operation)?;
        if session
            .operations()
            .last()
            .is_some_and(|o| o.progress == progress)
        {
            return Ok(());
        }
        session.operation_progress(
            &OwnerId::new(&self.scope.owner)?,
            self.scope.epoch,
            &id,
            progress,
        )
    }

    pub fn dispatch_id(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!(
                "ssh-dispatch-v1/{}/{}/{}",
                self.scope.session, self.scope.epoch, self.scope.operation
            ))
        )[..32]
            .into()
    }
    async fn exchange(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        method: &str,
        id: &str,
        payload: serde_json::Value,
    ) -> Result<Vec<u8>, Error> {
        target.validate()?;
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid);
        }
        let files = PrivateFiles::new(pin, key)?;
        let input = Zeroizing::new(serde_json::to_vec(&serde_json::json!({"version":1,"scope":self.scope,"method":method,"id":id,"payload":payload})).map_err(|_| Error::Invalid)?);
        if input.len() > 131072 {
            return Err(Error::Invalid);
        }
        let command = format!(
            "python3 -c '{}'",
            include_str!("execution.py").replace('\'', "'\\''")
        );
        let out = run(
            files.command(target, &command),
            &input,
            4096,
            Duration::from_secs(40),
        )
        .await?;
        Ok(out)
    }
    async fn call(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        method: &str,
        id: &str,
        payload: serde_json::Value,
    ) -> Result<Receipt, Error> {
        let out = self.exchange(target, pin, key, method, id, payload).await?;
        let receipt: Receipt = serde_json::from_slice(&out).map_err(|_| Error::Protocol)?;
        if receipt.id != id {
            return Err(Error::Protocol);
        }
        Ok(receipt)
    }
    /// Explicit execution consent, never called by bootstrap/check/enrollment.
    pub async fn authorize(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
    ) -> Result<Receipt, Error> {
        self.call(
            target,
            pin,
            key,
            "authorize",
            &self.dispatch_id(),
            serde_json::Value::Null,
        )
        .await
    }
    pub async fn observe(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
    ) -> Result<Observation, Error> {
        let out = self
            .exchange(
                target,
                pin,
                key,
                "observe",
                &self.dispatch_id(),
                serde_json::Value::Null,
            )
            .await?;
        let observation: Observation = serde_json::from_slice(&out).map_err(|_| Error::Protocol)?;
        if observation.thread != self.scope.thread
            || observation.generation.is_empty()
            || observation.generation.len() > 256
            || observation.generation.bytes().any(|b| b.is_ascii_control())
        {
            return Err(Error::Protocol);
        }
        Ok(observation)
    }
    /// Reconnect the same native driver/thread without replaying input.
    pub async fn reconnect(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        id: &str,
    ) -> Result<Receipt, Error> {
        self.call(target, pin, key, "reconnect", id, serde_json::Value::Null)
            .await
    }

    /// The exact generation is observed from the qualified driver. Unknown replies
    /// must use reconcile with the same identity; this call never resends a claim.
    pub async fn dispatch(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        generation: &str,
    ) -> Result<Receipt, Error> {
        self.call(
            target,
            pin,
            key,
            "dispatch",
            &self.dispatch_id(),
            serde_json::json!({"generation":generation,"text":&*self.input}),
        )
        .await
    }
    pub async fn reconcile(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        id: &str,
    ) -> Result<Receipt, Error> {
        self.call(target, pin, key, "reconcile", id, serde_json::Value::Null)
            .await
    }
    /// An explicit stable repair operation; model secret is separate from SSH key,
    /// delivered only over private stdin/IPC, never retained in a receipt.
    pub async fn repair(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        id: &str,
        generation: u64,
        model: &ModelCredential,
    ) -> Result<Receipt, Error> {
        if model.0.is_empty()
            || model.0.len() > 4096
            || model.0.bytes().any(|b| b.is_ascii_control())
        {
            return Err(Error::Invalid);
        }
        self.call(
            target,
            pin,
            key,
            "repair",
            id,
            serde_json::json!({"generation":generation,"model_key":&*model.0}),
        )
        .await
    }
    /// Tombstone before attempting native shutdown. Unknown shutdown retains writer
    /// ownership; caller must not infer exclusion from revocation or lease expiry.
    pub async fn revoke(
        &self,
        target: &Target,
        pin: &HostKey,
        key: &Credential,
        id: &str,
    ) -> Result<Receipt, Error> {
        self.call(target, pin, key, "revoke", id, serde_json::Value::Null)
            .await
    }
}

#[cfg(test)]
mod tests;
