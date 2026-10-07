//! SSH onboarding primitives, independent of tenancy and agent execution.
//! The caller must resolve/authorize a destination before constructing a Target.
//! No credential type implements Debug or Serialize. Dropping a future kills its
//! child; stderr is classified locally and never returned as remote diagnostics.
pub mod controller;
pub mod execution;

use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::{net::IpAddr, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use zeroize::Zeroizing;

#[derive(Clone, Debug)]
pub struct Target {
    pub address: IpAddr,
    pub port: u16,
    pub username: String,
}
impl Target {
    pub fn validate(&self) -> Result<(), Error> {
        if self.port == 0
            || self.username.is_empty()
            || self.username.len() > 64
            || !self
                .username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.username.starts_with('-')
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostKey {
    pub algorithm: String,
    pub key: String,
}
impl HostKey {
    pub fn fingerprint(&self) -> Result<String, Error> {
        if !matches!(
            self.algorithm.as_str(),
            "ssh-ed25519" | "ssh-rsa" | "ecdsa-sha2-nistp256"
        ) {
            return Err(Error::Invalid);
        }
        let bytes = STANDARD.decode(&self.key).map_err(|_| Error::Invalid)?;
        if bytes.len() < 32 || bytes.len() > 8192 {
            return Err(Error::Invalid);
        }
        Ok(format!(
            "SHA256:{}",
            STANDARD.encode(Sha256::digest(bytes)).trim_end_matches('=')
        ))
    }
}
pub struct Credential(pub Zeroizing<String>);
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Enter a valid target and an unencrypted SSH private key.")]
    Invalid,
    #[error("Execution authority is missing, expired or revoked. Request separate authorization.")]
    Authority,
    #[error("The workspace writer is busy. Inspect the active operation before retrying.")]
    WriterBusy,
    #[error("Execution delivery is uncertain. Reconcile the existing operation; do not replay it.")]
    Execution,
    #[error("The server could not be reached. Check its address, SSH port and firewall.")]
    Unreachable,
    #[error("The operation timed out. Check reachability and retry or repair setup.")]
    Timeout,
    #[error("SSH rejected the key. Check the username and authorized_keys.")]
    Authentication,
    #[error("The host key changed. Verify it independently before replacing the pin.")]
    HostChanged,
    #[error("Linux, Python 3, flock and nohup are required on the server.")]
    Tooling,
    #[error("Setup is incomplete. Check disk space and home-directory permissions, then repair.")]
    Bootstrap,
    #[error("SSH tooling is unavailable on the caller.")]
    Transport,
    #[error("The server returned an unexpected or oversized response.")]
    Protocol,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Check {
    pub platform: String,
    pub architecture: String,
}

async fn run(
    mut cmd: Command,
    input: &[u8],
    limit: usize,
    lifetime: Duration,
) -> Result<Vec<u8>, Error> {
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .env("HOME", "/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|_| Error::Transport)?;
    let mut stdin = child.stdin.take().ok_or(Error::Transport)?;
    let stdout = child.stdout.take().ok_or(Error::Transport)?;
    let stderr = child.stderr.take().ok_or(Error::Transport)?;
    tokio::time::timeout(lifetime, async {
        let write = async {
            stdin
                .write_all(input)
                .await
                .map_err(|_| Error::Unreachable)?;
            drop(stdin);
            Ok::<_, Error>(())
        };
        let read = async {
            let mut out = Vec::new();
            stdout
                .take((limit + 1) as u64)
                .read_to_end(&mut out)
                .await
                .map_err(|_| Error::Transport)?;
            if out.len() > limit {
                return Err(Error::Protocol);
            }
            Ok(out)
        };
        let err = async {
            let mut out = Vec::new();
            stderr
                .take(8193)
                .read_to_end(&mut out)
                .await
                .map_err(|_| Error::Transport)?;
            if out.len() > 8192 {
                return Err(Error::Protocol);
            }
            Ok(out)
        };
        let (_, out, err) = tokio::try_join!(write, read, err)?;
        let status = child.wait().await.map_err(|_| Error::Transport)?;
        if status.success() {
            return Ok(out);
        }
        let err = String::from_utf8_lossy(&err);
        if err.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
            || err.contains("Host key verification failed")
        {
            Err(Error::HostChanged)
        } else if err.contains("invalid format")
            || err.contains("incorrect passphrase")
            || err.contains("error in libcrypto")
        {
            Err(Error::Invalid)
        } else if err.contains("Permission denied") {
            Err(Error::Authentication)
        } else if status.code() == Some(42) {
            Err(Error::Tooling)
        } else if status.code() == Some(44) {
            Err(Error::Authority)
        } else if status.code() == Some(45) {
            Err(Error::WriterBusy)
        } else if status.code() == Some(46) {
            Err(Error::Execution)
        } else if status.code() == Some(43) {
            Err(Error::Bootstrap)
        } else {
            Err(Error::Unreachable)
        }
    })
    .await
    .map_err(|_| Error::Timeout)?
}
/// Unauthenticated discovery only; this does not trust a key or transmit credentials.
pub async fn discover(target: &Target) -> Result<HostKey, Error> {
    target.validate()?;
    let mut cmd = Command::new("/usr/bin/ssh-keyscan");
    cmd.args([
        "-T",
        "5",
        "-t",
        "ed25519",
        "-p",
        &target.port.to_string(),
        &target.address.to_string(),
    ]);
    let out = run(cmd, &[], 16384, Duration::from_secs(8)).await?;
    let text = String::from_utf8(out).map_err(|_| Error::Protocol)?;
    let fields: Vec<_> = text
        .lines()
        .find(|l| !l.starts_with('#'))
        .ok_or(Error::Unreachable)?
        .split_whitespace()
        .collect();
    if fields.len() != 3 {
        return Err(Error::Protocol);
    }
    let key = HostKey {
        algorithm: fields[1].into(),
        key: fields[2].into(),
    };
    key.fingerprint()?;
    Ok(key)
}
struct PrivateFiles {
    dir: tempfile::TempDir,
}
impl PrivateFiles {
    fn new(key: &HostKey, credential: &Credential) -> Result<Self, Error> {
        use std::{
            fs,
            io::Write,
            os::unix::fs::{OpenOptionsExt, PermissionsExt},
        };
        key.fingerprint()?;
        if credential.0.len() > 16384 || !credential.0.starts_with("-----BEGIN ") {
            return Err(Error::Invalid);
        }
        let dir = tempfile::tempdir().map_err(|_| Error::Transport)?;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Transport)?;
        for (name, bytes) in [
            ("identity", credential.0.as_bytes()),
            (
                "hosts",
                format!("semon-target {} {}\n", key.algorithm, key.key).as_bytes(),
            ),
        ] {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.path().join(name))
                .map_err(|_| Error::Transport)?;
            f.write_all(bytes).map_err(|_| Error::Transport)?;
        }
        Ok(Self { dir })
    }
    fn command(&self, target: &Target, command: &str) -> Command {
        let mut cmd = Command::new("/usr/bin/ssh");
        cmd.args([
            "-F",
            "/dev/null",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "HostKeyAlias=semon-target",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ServerAliveInterval=3",
            "-o",
            "ServerAliveCountMax=2",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "HostKeyAlgorithms=ssh-ed25519,ecdsa-sha2-nistp256,rsa-sha2-512,rsa-sha2-256",
        ])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            self.dir.path().join("hosts").display()
        ))
        .arg("-i")
        .arg(self.dir.path().join("identity"))
        .arg("-p")
        .arg(target.port.to_string())
        .arg("-l")
        .arg(&target.username)
        .arg(target.address.to_string())
        .arg(command);
        cmd
    }
}
const CHECK: &str = "test \"$(uname -s)\" = Linux && command -v python3 >/dev/null && command -v flock >/dev/null && command -v nohup >/dev/null || exit 42; python3 -c 'import json,platform; print(json.dumps(dict(platform=platform.system(),architecture=platform.machine())))'";
/// Authentication + tooling only. No installation, enrollment, or agent launch.
pub async fn check(
    target: &Target,
    key: &HostKey,
    credential: &Credential,
) -> Result<Check, Error> {
    target.validate()?;
    let files = PrivateFiles::new(key, credential)?;
    let out = run(
        files.command(target, CHECK),
        &[],
        4096,
        Duration::from_secs(12),
    )
    .await?;
    let check: Check = serde_json::from_slice(&out).map_err(|_| Error::Protocol)?;
    if check.platform != "Linux" || !matches!(check.architecture.as_str(), "x86_64" | "aarch64") {
        return Err(Error::Tooling);
    }
    Ok(check)
}
/// Private enrollment material passed through stdin, never command arguments.
/// Supply a qualified Linux binary; installation does not download arbitrary code.
pub struct Bootstrap<'a> {
    pub operation: &'a str,
    pub destination: &'a str,
    pub token: &'a str,
    pub binary: &'a [u8],
}
#[derive(Debug, serde::Deserialize)]
pub struct Setup {
    pub running: bool,
}
pub async fn bootstrap(
    target: &Target,
    key: &HostKey,
    credential: &Credential,
    setup: Bootstrap<'_>,
) -> Result<Setup, Error> {
    bootstrap_inner(target, key, credential, setup, None, false).await
}
/// Scope mirror checkpoints to the stable logical receiver. Reuse this identity
/// for credential rotation/repair; choose a new identity for a new receiver even
/// when its URL is unchanged. No native harness is started or resumed.
pub async fn bootstrap_for_receiver(
    target: &Target,
    key: &HostKey,
    credential: &Credential,
    setup: Bootstrap<'_>,
    receiver_identity: &str,
) -> Result<Setup, Error> {
    if receiver_identity.is_empty()
        || receiver_identity.len() > 256
        || receiver_identity.bytes().any(|b| b.is_ascii_control())
    {
        return Err(Error::Invalid);
    }
    bootstrap_inner(
        target,
        key,
        credential,
        setup,
        Some(receiver_identity),
        false,
    )
    .await
}
/// Capture the already installed controller's fresh native home into its stable
/// receiver. This does not start, resume or authorize native execution. The
/// remote bootstrap requires an existing immutable controller installation.
pub async fn bootstrap_native_receiver(
    target: &Target,
    key: &HostKey,
    credential: &Credential,
    setup: Bootstrap<'_>,
    receiver_identity: &str,
) -> Result<Setup, Error> {
    if receiver_identity.is_empty()
        || receiver_identity.len() > 256
        || receiver_identity.bytes().any(|b| b.is_ascii_control())
    {
        return Err(Error::Invalid);
    }
    bootstrap_inner(
        target,
        key,
        credential,
        setup,
        Some(receiver_identity),
        true,
    )
    .await
}
async fn bootstrap_inner(
    target: &Target,
    key: &HostKey,
    credential: &Credential,
    setup: Bootstrap<'_>,
    receiver_identity: Option<&str>,
    capture_native: bool,
) -> Result<Setup, Error> {
    target.validate()?;
    if setup.operation.len() != 36
        || !setup
            .operation
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
        || setup.binary.len() > 128 * 1024 * 1024
        || setup.binary.is_empty()
        || setup.token.len() > 4096
    {
        return Err(Error::Invalid);
    }
    let files = PrivateFiles::new(key, credential)?;
    let input=Zeroizing::new(serde_json::to_vec(&serde_json::json!({"version":1,"operation":setup.operation,"receiver_identity":receiver_identity,"capture_native":capture_native,"destination":setup.destination,"token":setup.token,"binary":STANDARD.encode(setup.binary),"sha256":format!("{:x}",Sha256::digest(setup.binary))})).map_err(|_|Error::Invalid)?);
    let script = include_str!("bootstrap.py");
    let command = format!("python3 -c '{}'", script.replace('\'', "'\\''"));
    let out = run(
        files.command(target, &command),
        &input,
        4096,
        Duration::from_secs(60),
    )
    .await?;
    serde_json::from_slice(&out).map_err(|_| Error::Protocol)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_and_host_key_validation() {
        let mut t = Target {
            address: "127.0.0.1".parse().unwrap(),
            port: 22,
            username: "user".into(),
        };
        assert!(t.validate().is_ok());
        t.username = "-oProxyCommand=id".into();
        assert_eq!(t.validate(), Err(Error::Invalid));
        assert!(
            HostKey {
                algorithm: "ssh-ed25519\nmalicious".into(),
                key: STANDARD.encode([0; 32])
            }
            .fingerprint()
            .is_err()
        );
    }
    #[tokio::test]
    async fn missing_tools_and_remote_diagnostics_are_safe() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "echo remote-secret-do-not-return >&2; exit 42"]);
        let error = run(cmd, &[], 100, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error, Error::Tooling);
        assert!(!error.to_string().contains("remote-secret"));
    }
    #[tokio::test]
    async fn cancellation_terminates_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("pid");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo $$ > \"$1\"; exec sleep 30")
            .arg("fixture")
            .arg(&pid_path);
        let task = tokio::spawn(async move { run(cmd, &[], 100, Duration::from_secs(30)).await });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while !pid_path.exists() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = std::fs::read_to_string(pid_path).unwrap();
        task.abort();
        let _ = task.await;
        while std::path::Path::new(&format!("/proc/{}", pid.trim())).exists() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    #[tokio::test]
    async fn bounded_output_and_deadline() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "yes x"]);
        assert_eq!(
            run(cmd, &[], 16, Duration::from_secs(2)).await,
            Err(Error::Protocol)
        );
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("10");
        assert_eq!(
            run(cmd, &[], 16, Duration::from_millis(20)).await,
            Err(Error::Timeout)
        );
    }
}
