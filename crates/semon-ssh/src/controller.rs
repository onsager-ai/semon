//! Explicit native controller bootstrap, separate from mirror enrollment.
//! The embedding owns execution consent, current authority, outbound policy,
//! artifact selection and durable launch claims before invoking installation.
use crate::{Credential, Error, HostKey, PrivateFiles, Target, run};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

/// Stable authorized application identity. Credentials never occur here.
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Owner of both runtime and model custody.
    pub owner: String,
    /// Authorized workspace.
    pub workspace: String,
    /// Selected SSH connection.
    pub connection: String,
    /// Durable coordinator session.
    pub session: String,
    /// Selected model connection, distinct from SSH credential custody.
    pub model_connection: String,
    /// Claimed model credential generation.
    pub model_generation: u64,
    /// Claimed coordinator epoch.
    pub epoch: u64,
}
/// Private bootstrap; the application must explicitly authorize execution.
/// A repeated identity only observes existing state and never creates a thread.
pub struct Install<'a> {
    /// Previously established stable mirror enrollment UUID.
    pub enrollment: &'a str,
    /// Persisted fresh native launch identity,32 lowercase hex characters.
    pub launch: &'a str,
    /// Absolute UTC expiry, at most five minutes ahead. No implicit renewal.
    pub expires: u64,
    /// Current scoped application binding.
    pub binding: Binding,
    /// Preinstalled complete official native package directory. Installation
    /// verifies every file against the pinned upstream0.160.0 musl artifact.
    pub native_directory: &'a str,
    /// Reviewed controller artifact. Host policy must bind its selected version
    /// and digest before this call; transport integrity is checked remotely.
    pub controller: &'a [u8],
    /// Native model settings only; executor policy cannot be overridden.
    pub model_config: &'a str,
    /// Private model credential, sent only over protected stdin/IPC.
    pub model_key: &'a str,
}
/// Sanitized installation result. Unknown means inspect this same identity.
#[derive(Debug, Deserialize)]
pub struct Launch {
    /// Stable launch identity.
    pub launch: String,
    /// Claimed, running or unknown; existence does not prove a completed turn.
    pub state: String,
    /// Native thread, only after the controller has committed it.
    pub thread: Option<String>,
    /// True only after independent supervisor descendant-reaping proof.
    #[serde(default)]
    pub writer_excluded: bool,
}
/// Install the controller into an existing mirror enrollment and start its
/// independent supervisor. No compute is created, replaced, paused or deleted.
/// The native package is a separately provisioned, integrity-pinned prerequisite.
pub async fn install(
    target: &Target,
    pin: &HostKey,
    key: &Credential,
    setup: Install<'_>,
) -> Result<Launch, Error> {
    target.validate()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Invalid)?
        .as_secs();
    if setup.controller.is_empty()
        || setup.controller.len() > 16 * 1024 * 1024
        || setup.expires <= now
        || setup.expires > now + 300
        || setup.model_key.is_empty()
        || setup.model_key.len() > 4096
        || setup.model_config.len() > 32768
    {
        return Err(Error::Invalid);
    }
    let input=Zeroizing::new(serde_json::to_vec(&serde_json::json!({"version":1,"method":"openai_api_key","authorize_execution":true,"enrollment":setup.enrollment,"launch":setup.launch,"expires":setup.expires,"binding":setup.binding,"native_directory":setup.native_directory,"controller":STANDARD.encode(setup.controller),"controller_sha256":hex_sha(setup.controller),"model_config":setup.model_config,"model_key":setup.model_key})).map_err(|_|Error::Invalid)?);
    let script = format!(
        "NATIVE_MANIFEST={}\nSUPERVISOR={}\n{}\n{}",
        include_str!("codex-0.160.0-musl.json"),
        serde_json::to_string(include_str!("controller-supervisor.py"))
            .map_err(|_| Error::Invalid)?,
        include_str!("controller-observe.py"),
        include_str!("controller-bootstrap.py")
    );
    let command = format!("python3 -c '{}'", script.replace('\'', "'\\''"));
    let files = PrivateFiles::new(pin, key)?;
    let output = run(
        files.command(target, &command),
        &input,
        4096,
        Duration::from_secs(40),
    )
    .await?;
    let result: Launch = serde_json::from_slice(&output).map_err(|_| Error::Protocol)?;
    if result.launch != setup.launch
        || !matches!(
            result.state.as_str(),
            "claimed" | "running" | "unknown" | "ended"
        )
    {
        return Err(Error::Protocol);
    }
    Ok(result)
}
fn hex_sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Read current launch state without model custody or any native write. The host
/// still rechecks current owner/workspace/SSH authority before calling this.
pub async fn observe(
    target: &Target,
    pin: &HostKey,
    key: &Credential,
    enrollment: &str,
    launch: &str,
    binding: &Binding,
) -> Result<Launch, Error> {
    observe_inner(target, pin, key, enrollment, launch, binding, false).await
}
/// Permanently stop the exact installed launch. This cannot install, authorize
/// execution, extend a lease or replay input. Exclusion still requires reaping.
pub async fn stop(
    target: &Target,
    pin: &HostKey,
    key: &Credential,
    enrollment: &str,
    launch: &str,
    binding: &Binding,
) -> Result<Launch, Error> {
    observe_inner(target, pin, key, enrollment, launch, binding, true).await
}
async fn observe_inner(
    target: &Target,
    pin: &HostKey,
    key: &Credential,
    enrollment: &str,
    launch: &str,
    binding: &Binding,
    stop: bool,
) -> Result<Launch, Error> {
    target.validate()?;
    let input=serde_json::to_vec(&serde_json::json!({"version":1,"method":if stop{"stop"}else{"observe"},"authorize_stop":stop,"enrollment":enrollment,"launch":launch,"binding":binding})).map_err(|_|Error::Invalid)?;
    let script = format!(
        "{}\n{}",
        include_str!("controller-observe.py"),
        include_str!("controller-bootstrap.py")
    );
    let command = format!("python3 -c '{}'", script.replace('\'', "'\\''"));
    let files = PrivateFiles::new(pin, key)?;
    let output = run(
        files.command(target, &command),
        &input,
        4096,
        Duration::from_secs(10),
    )
    .await?;
    let result: Launch = serde_json::from_slice(&output).map_err(|_| Error::Protocol)?;
    if result.launch != launch || !matches!(result.state.as_str(), "unknown" | "running" | "ended")
    {
        return Err(Error::Protocol);
    }
    Ok(result)
}
