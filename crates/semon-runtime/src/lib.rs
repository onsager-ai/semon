//! Durable coordinator records, independent of provider compute and capture.
//!
//! An embedding server stores the entire [`Session`] atomically, checks the
//! authenticated owner and expected revision/epoch, and commits before invoking
//! a provider. Unknown side-effect outcomes require inspection, never replay.
//! No provider credentials, command transport, or persistence engine live here.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::future::Future;
use thiserror::Error;

/// Distinct identifiers cannot be substituted for one another.
macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, Error> {
                Self::try_from(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(value: String) -> Result<Self, Error> {
                if value.is_empty()
                    || matches!(value.as_str(), "." | "..")
                    || value.len() > 128
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                {
                    return Err(Error::InvalidIdentifier);
                }
                Ok(Self(value))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
    };
}
identifier!(SessionId);
identifier!(OwnerId);
identifier!(WorkspaceId);
identifier!(ThreadId);
identifier!(RuntimeId);
identifier!(OperationId);
identifier!(CheckpointId);
identifier!(CredentialId);

/// A coordinator-resolved name, never a secret value or a bearer token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub owner: OwnerId,
    pub name: CredentialId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub owner: OwnerId,
    pub workspace: WorkspaceId,
    pub cwd: String,
    pub harness_home: String,
    /// Receiver enrollment; independent of any provider resource identifier.
    pub mirror: String,
    pub provider_credential: CredentialRef,
    pub model_credential: CredentialRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Desired {
    Active,
    Paused,
    Ended,
}

/// Compute presence does not determine harness or logical-session lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compute {
    Absent,
    Provisioning,
    Active,
    Checkpointing,
    Suspending,
    Paused,
    Restoring,
    Blocked,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    NotStarted,
    Idle,
    Running,
    WaitingPermission,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Launch,
    Resume,
    SuspendMemory,
    SuspendFilesystem,
    Restore,
    Export,
    Destroy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Progress {
    Prepared,
    InFlight,
    OutcomeUnknown,
    Succeeded,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: OperationId,
    pub epoch: u64,
    pub action: Action,
    pub progress: Progress,
    /// Sensitive manual input retained before any provisioning. No dispatch yet.
    pub launch_input: Option<String>,
}

// Session Debug delegates here; durable manual input must not enter diagnostics.
impl std::fmt::Debug for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation")
            .field("id", &self.id)
            .field("epoch", &self.epoch)
            .field("action", &self.action)
            .field("progress", &self.progress)
            .field(
                "launch_input",
                &self.launch_input.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub epoch: u64,
    pub operation: OperationId,
    pub runtime: Option<RuntimeId>,
    /// Only set after observed shutdown or qualified storage-write exclusion.
    /// Epoch changes alone do not fence a harness/workspace writer.
    pub writer_excluded: bool,
}

/// Published, integrity-verified artifact references, not a file inventory or
/// a Push exit code. Later checkpoint implementation must verify coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub epoch: u64,
    pub input_revision: u64,
    pub intended: Action,
    pub thread: ThreadId,
    pub workspace: String,
    pub originals: String,
    pub harness: String,
    pub commands: String,
    pub path_mapping: String,
    /// Deletion remains disabled until independent storage is qualified (#255).
    pub independent: bool,
}

/// Atomically persisted coordinator aggregate; retained outside guest compute.
/// Private fields force normal mutations through checked lifecycle methods.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    id: SessionId,
    binding: Binding,
    thread: Option<ThreadId>,
    revision: u64,
    epoch: u64,
    input_revision: u64,
    desired: Desired,
    compute: Compute,
    harness: Harness,
    attempts: Vec<Attempt>,
    operations: Vec<Operation>,
    checkpoints: Vec<Checkpoint>,
    committed_checkpoint: Option<CheckpointId>,
}

impl Session {
    /// Verify an untrusted successor is exactly one legal checked mutation.
    /// A store calls this under its CAS lock, including for decoded records.
    pub fn validate_successor(
        &self,
        next: &Self,
        owner: &OwnerId,
        epoch: u64,
    ) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        next.validate()?;
        if self.id != next.id || self.binding != next.binding {
            return Err(Error::IdentityChanged);
        }
        let matches = |candidate: Result<Self, Error>| candidate.is_ok_and(|s| s == *next);
        let mutate = |f: &dyn Fn(&mut Self) -> Result<(), Error>| {
            let mut s = self.clone();
            f(&mut s).map(|()| s)
        };
        if matches(mutate(&|s| s.set_desired(owner, epoch, next.desired)))
            || matches(mutate(&|s| s.set_compute(owner, epoch, next.compute)))
            || matches(mutate(&|s| s.set_harness(owner, epoch, next.harness)))
            || matches(mutate(&|s| s.exclude_writer(owner, epoch)))
        {
            return Ok(());
        }
        if let Some(thread) = &next.thread
            && matches(mutate(&|s| s.bind_thread(owner, epoch, thread.clone())))
        {
            return Ok(());
        }
        if let Some(runtime) = next.attempts.last().and_then(|a| a.runtime.as_ref())
            && matches(mutate(&|s| s.bind_runtime(owner, epoch, runtime.clone())))
        {
            return Ok(());
        }
        for operation in &next.operations {
            if matches(mutate(&|s| {
                s.operation_progress(owner, epoch, &operation.id, operation.progress)
            })) {
                return Ok(());
            }
        }
        if next.operations.len() == self.operations.len() + 1
            && let Some(operation) = next.operations.last()
            && let Some(input) = &operation.launch_input
            && matches(mutate(&|s| {
                s.prepare_launch(owner, epoch, operation.id.clone(), input.clone())
            }))
        {
            return Ok(());
        }
        Err(Error::IllegalTransition)
    }
    pub fn new(id: SessionId, binding: Binding) -> Result<Self, Error> {
        let value = Self {
            id,
            binding,
            thread: None,
            revision: 0,
            epoch: 0,
            input_revision: 0,
            desired: Desired::Active,
            compute: Compute::Absent,
            harness: Harness::NotStarted,
            attempts: vec![],
            operations: vec![],
            checkpoints: vec![],
            committed_checkpoint: None,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn id(&self) -> &SessionId {
        &self.id
    }
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    pub fn thread(&self) -> Option<&ThreadId> {
        self.thread.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn compute(&self) -> Compute {
        self.compute
    }
    pub fn harness(&self) -> Harness {
        self.harness
    }
    pub fn desired(&self) -> Desired {
        self.desired
    }
    /// Desired state is durable intent, not evidence that compute changed.
    pub fn set_desired(&mut self, owner: &OwnerId, epoch: u64, next: Desired) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        if next == Desired::Ended
            && (self.compute != Compute::Absent || self.attempts.iter().any(|a| !a.writer_excluded))
        {
            return Err(Error::WriterNotExcluded);
        }
        self.changed()?;
        self.desired = next;
        Ok(())
    }
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    pub fn validate(&self) -> Result<(), Error> {
        let b = &self.binding;
        if b.provider_credential.owner != b.owner || b.model_credential.owner != b.owner {
            return Err(Error::WrongOwner);
        }
        for path in [&b.cwd, &b.harness_home] {
            if !path.starts_with('/') || path.contains('\0') || path.split('/').any(|s| s == "..") {
                return Err(Error::InvalidPath);
            }
        }
        if b.cwd == b.harness_home || b.mirror.is_empty() || b.mirror.len() > 256 {
            return Err(Error::InvalidBinding);
        }
        if self.attempts.iter().filter(|a| !a.writer_excluded).count() > 1 {
            return Err(Error::WriterNotExcluded);
        }
        if self.operations.iter().any(|o| o.epoch > self.epoch)
            || self.attempts.iter().any(|a| a.epoch > self.epoch)
        {
            return Err(Error::StaleEpoch);
        }
        Ok(())
    }

    fn authorize(&self, owner: &OwnerId, epoch: u64) -> Result<(), Error> {
        if &self.binding.owner != owner {
            return Err(Error::WrongOwner);
        }
        if epoch != self.epoch {
            return Err(Error::StaleEpoch);
        }
        if self.desired == Desired::Ended {
            return Err(Error::Ended);
        }
        Ok(())
    }
    fn changed(&mut self) -> Result<(), Error> {
        self.revision = self.revision.checked_add(1).ok_or(Error::Exhausted)?;
        Ok(())
    }
    /// Persist the returned prepared operation/attempt before provider creation.
    pub fn prepare_launch(
        &mut self,
        owner: &OwnerId,
        epoch: u64,
        id: OperationId,
        input: String,
    ) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        if self.desired != Desired::Active {
            return Err(Error::IllegalTransition);
        }
        if !matches!(
            self.compute,
            Compute::Absent | Compute::Failed | Compute::Blocked
        ) || self.attempts.iter().any(|a| !a.writer_excluded)
        {
            return Err(Error::WriterNotExcluded);
        }
        if self.operations.iter().any(|o| o.id == id) {
            return Err(Error::DuplicateOperation);
        }
        if input.is_empty() || input.len() > 65536 {
            return Err(Error::InvalidInput);
        }
        let next = self.epoch.checked_add(1).ok_or(Error::Exhausted)?;
        self.changed()?;
        self.epoch = next;
        self.compute = Compute::Provisioning;
        self.operations.push(Operation {
            id: id.clone(),
            epoch: next,
            action: Action::Launch,
            progress: Progress::Prepared,
            launch_input: Some(input),
        });
        self.attempts.push(Attempt {
            epoch: next,
            operation: id,
            runtime: None,
            writer_excluded: false,
        });
        Ok(())
    }
    pub fn bind_runtime(
        &mut self,
        owner: &OwnerId,
        epoch: u64,
        runtime: RuntimeId,
    ) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        let attempt = self.attempts.last().ok_or(Error::IllegalTransition)?;
        if attempt.epoch != epoch || attempt.runtime.as_ref().is_some_and(|old| old != &runtime) {
            return Err(Error::IdentityChanged);
        }
        self.changed()?;
        self.attempts
            .last_mut()
            .ok_or(Error::IllegalTransition)?
            .runtime = Some(runtime);
        Ok(())
    }
    /// The embedding server must commit this immediately upon thread receipt.
    pub fn bind_thread(
        &mut self,
        owner: &OwnerId,
        epoch: u64,
        thread: ThreadId,
    ) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        if self.thread.as_ref().is_some_and(|old| old != &thread) {
            return Err(Error::IdentityChanged);
        }
        self.changed()?;
        self.thread = Some(thread);
        Ok(())
    }
    /// Unknown outcomes may progress only after external inspection; this method
    /// records that inspection's result and never performs a side effect.
    pub fn operation_progress(
        &mut self,
        owner: &OwnerId,
        epoch: u64,
        id: &OperationId,
        progress: Progress,
    ) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        let index = self
            .operations
            .iter()
            .position(|o| &o.id == id && o.epoch == epoch)
            .ok_or(Error::UnknownOperation)?;
        let old = self.operations[index].progress;
        if !matches!(
            (old, progress),
            (Progress::Prepared, Progress::InFlight | Progress::Failed)
                | (
                    Progress::InFlight,
                    Progress::Succeeded | Progress::Failed | Progress::OutcomeUnknown
                )
                | (
                    Progress::OutcomeUnknown,
                    Progress::Succeeded | Progress::Failed
                )
        ) {
            return Err(Error::IllegalTransition);
        }
        self.changed()?;
        self.operations[index].progress = progress;
        Ok(())
    }
    pub fn set_compute(&mut self, owner: &OwnerId, epoch: u64, next: Compute) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        if !matches!(
            (self.compute, next),
            (
                Compute::Provisioning | Compute::Restoring,
                Compute::Active | Compute::Blocked | Compute::Failed
            ) | (
                Compute::Active,
                Compute::Checkpointing | Compute::Blocked | Compute::Failed
            ) | (
                Compute::Checkpointing,
                Compute::Active | Compute::Suspending | Compute::Blocked | Compute::Failed
            ) | (
                Compute::Suspending,
                Compute::Paused | Compute::Blocked | Compute::Failed
            ) | (Compute::Paused, Compute::Restoring)
                | (Compute::Blocked | Compute::Failed, Compute::Absent)
        ) {
            return Err(Error::IllegalTransition);
        }
        if next == Compute::Active
            && self
                .attempts
                .last()
                .is_none_or(|a| a.runtime.is_none() || a.writer_excluded)
        {
            return Err(Error::IllegalTransition);
        }
        // Checkpoint verification and suspension eligibility arrive in #253.
        if next == Compute::Suspending {
            return Err(Error::NotQualified(Action::SuspendFilesystem));
        }
        self.changed()?;
        self.compute = next;
        Ok(())
    }
    pub fn set_harness(&mut self, owner: &OwnerId, epoch: u64, next: Harness) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        if matches!(next, Harness::Running | Harness::WaitingPermission)
            && self.compute != Compute::Active
        {
            return Err(Error::IllegalTransition);
        }
        self.changed()?;
        self.harness = next;
        Ok(())
    }
    /// Records externally verified shutdown/storage exclusion, not lease expiry.
    pub fn exclude_writer(&mut self, owner: &OwnerId, epoch: u64) -> Result<(), Error> {
        self.authorize(owner, epoch)?;
        if !matches!(
            self.compute,
            Compute::Blocked | Compute::Failed | Compute::Absent
        ) {
            return Err(Error::IllegalTransition);
        }
        if self.attempts.last().is_none() {
            return Err(Error::IllegalTransition);
        }
        self.changed()?;
        self.attempts
            .last_mut()
            .ok_or(Error::IllegalTransition)?
            .writer_excluded = true;
        Ok(())
    }
}

/// Explicit semantics; unsupported capabilities fail before provider calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub retained_memory: bool,
    pub retained_filesystem: bool,
    pub restore: bool,
    pub export: bool,
}
impl Capabilities {
    pub fn require(self, action: Action) -> Result<(), Error> {
        let supported = match action {
            Action::Launch | Action::Resume => true,
            Action::SuspendMemory => self.retained_memory,
            Action::SuspendFilesystem => self.retained_filesystem,
            Action::Restore => self.restore,
            Action::Export => self.export,
            Action::Destroy => return Err(Error::NotQualified(action)),
        };
        if supported {
            Ok(())
        } else {
            Err(Error::Unsupported(action))
        }
    }
}

/// CAS is atomic across all four record kinds, scoped to authenticated ownership.
/// Implementations live in embedding servers; no guest-local store qualifies.
pub trait SessionStore {
    type Error;
    fn load(
        &self,
        owner: &OwnerId,
        id: &SessionId,
    ) -> impl Future<Output = Result<Option<Session>, Self::Error>> + Send;
    fn compare_and_swap(
        &self,
        owner: &OwnerId,
        expected_revision: Option<u64>,
        expected_epoch: u64,
        next: &Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// A provider resource observation. Unreachable is distinct from absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeObservation {
    Absent,
    Present { id: RuntimeId, compute: Compute },
    Unknown,
}

/// Provider calls take the already-persisted operation identity. Implementations
/// inspect its labeled resources before retrying an uncertain create. No destroy
/// entrypoint is exposed before independent recovery qualification.
pub trait Provider {
    type Error;
    fn capabilities(&self) -> Capabilities;
    fn inspect(
        &self,
        session: &SessionId,
        operation: &OperationId,
    ) -> Result<RuntimeObservation, Self::Error>;
    fn provision(
        &self,
        session: &Session,
        operation: &Operation,
    ) -> Result<RuntimeObservation, Self::Error>;
}

/// Harness inspection must use a live version-qualified driver, not PID facts.
pub trait HarnessDriver {
    type Error;
    fn inspect(&self, session: &SessionId, epoch: u64) -> Result<Harness, Self::Error>;
}

/// Independent publication/verification; checkpoint metadata is committed only
/// after verification. Confidentiality and credential handling belong to the
/// embedding deployment. Mirrors/ACKs are not automatically recovery artifacts.
pub trait RecoveryStorage {
    type Error;
    fn publish(&self, key: &str, protected_bytes: &[u8]) -> Result<(), Self::Error>;
    fn verify(&self, key: &str, sha256: &str, length: u64) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum Error {
    #[error("invalid identifier")]
    InvalidIdentifier,
    #[error("invalid stable guest path")]
    InvalidPath,
    #[error("invalid mirror/workspace binding")]
    InvalidBinding,
    #[error("credential/session owner mismatch")]
    WrongOwner,
    #[error("stale execution epoch")]
    StaleEpoch,
    #[error("logical session explicitly ended")]
    Ended,
    #[error("old workspace writer has not been excluded")]
    WriterNotExcluded,
    #[error("stable identity cannot be rebound")]
    IdentityChanged,
    #[error("illegal lifecycle transition")]
    IllegalTransition,
    #[error("operation already exists")]
    DuplicateOperation,
    #[error("unknown operation")]
    UnknownOperation,
    #[error("manual launch input is empty or exceeds 64 KiB")]
    InvalidInput,
    #[error("revision/epoch exhausted")]
    Exhausted,
    #[error("provider does not support {0:?}")]
    Unsupported(Action),
    #[error("lifecycle action is not qualified: {0:?}")]
    NotQualified(Action),
}
