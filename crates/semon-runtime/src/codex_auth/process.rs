//! Private stdio device-login process. No WebSocket listener or task execution.
//!
//! The embedding supplies a fresh protected home and owns credential custody,
//! durable claims, authorization and cleanup. Losing this process requires a
//! fresh explicit login; it must not be replayed against an old home.

use super::{
    AccountRead, CODEX_VERSION, DeviceChallenge, LoginCompleted, LoginFlow, LoginResponse, Method,
    State,
};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Instant, timeout, timeout_at},
};

const MAX_FRAME: u64 = 65536;
const MAX_PENDING: usize = 8;

/// No Debug implementation: messages and challenge contain owner-only material.
pub struct DeviceLogin {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    flow: LoginFlow,
    completions: VecDeque<LoginCompleted>,
    deadline: Instant,
    next_id: u64,
}

fn command(binary: &Path, home: &Path) -> Command {
    let mut command = Command::new(binary);
    // Preserve system networking/proxy/CA settings, but never give the auth
    // subprocess Hub's database, vault, GitHub or compute credentials.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| {
            key.starts_with("SEMON_HUB_")
                || matches!(
                    key,
                    "GH_TOKEN"
                        | "GITHUB_TOKEN"
                        | "E2B_API_KEY"
                        | "AWS_ACCESS_KEY_ID"
                        | "AWS_SECRET_ACCESS_KEY"
                        | "AWS_SESSION_TOKEN"
                )
        }) {
            command.env_remove(key);
        }
    }
    command
        .current_dir(home)
        .env("CODEX_HOME", home)
        // Subscription onboarding never picks an inherited billing source.
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_ACCESS_TOKEN")
        .args([
            "-c",
            "cli_auth_credentials_store=\"file\"",
            "-c",
            "model_provider=\"openai\"",
        ])
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

impl DeviceLogin {
    /// The deadline covers startup, device polling and account confirmation.
    pub async fn start(
        binary: &Path,
        home: &Path,
        lifetime: Duration,
    ) -> Result<Self, ProcessError> {
        if !binary.is_absolute()
            || !home.is_absolute()
            || lifetime.is_zero()
            || lifetime > Duration::from_secs(900)
        {
            return Err(ProcessError::Configuration);
        }
        let deadline = Instant::now() + lifetime;
        let version = timeout(
            Duration::from_secs(5),
            command(binary, home).arg("--version").output(),
        )
        .await
        .map_err(|_| ProcessError::Timeout)?
        .map_err(|_| ProcessError::Unavailable)?;
        if !version.status.success()
            || version.stdout != format!("codex-cli {CODEX_VERSION}\n").as_bytes()
        {
            return Err(ProcessError::VersionMismatch);
        }
        let mut child = command(binary, home)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| ProcessError::Unavailable)?;
        let input = child.stdin.take().ok_or(ProcessError::Unavailable)?;
        let output = BufReader::new(child.stdout.take().ok_or(ProcessError::Unavailable)?);
        let mut login = Self {
            child,
            input,
            output,
            flow: LoginFlow::new(Method::ChatGptDeviceCode, CODEX_VERSION)
                .map_err(|_| ProcessError::VersionMismatch)?,
            completions: VecDeque::new(),
            deadline,
            next_id: 1,
        };
        login
            .rpc(
                "initialize",
                json!({"clientInfo": {"name": "semon-runtime", "version": "0.1.0"}}),
            )
            .await?;
        login
            .send(json!({"method":"initialized", "params":{}}))
            .await?;
        let response = login
            .rpc("account/login/start", json!({"type":"chatgptDeviceCode"}))
            .await?;
        let response: LoginResponse =
            serde_json::from_value(response).map_err(|_| ProcessError::Protocol)?;
        login
            .flow
            .started(response)
            .map_err(|_| ProcessError::Protocol)?;
        Ok(login)
    }

    pub fn challenge(&self) -> Option<&DeviceChallenge> {
        self.flow.challenge()
    }

    async fn send(&mut self, message: Value) -> Result<(), ProcessError> {
        let mut bytes = serde_json::to_vec(&message).map_err(|_| ProcessError::Protocol)?;
        bytes.push(b'\n');
        timeout_at(self.deadline, async {
            self.input.write_all(&bytes).await?;
            self.input.flush().await
        })
        .await
        .map_err(|_| ProcessError::Timeout)?
        .map_err(|_| ProcessError::Unavailable)
    }

    async fn receive(&mut self) -> Result<Value, ProcessError> {
        let mut bytes = Vec::new();
        let length = timeout_at(
            self.deadline,
            (&mut self.output)
                .take(MAX_FRAME + 1)
                .read_until(b'\n', &mut bytes),
        )
        .await
        .map_err(|_| ProcessError::Timeout)?
        .map_err(|_| ProcessError::Unavailable)?;
        if length == 0 || length as u64 > MAX_FRAME || bytes.last() != Some(&b'\n') {
            return Err(ProcessError::Protocol);
        }
        serde_json::from_slice(&bytes).map_err(|_| ProcessError::Protocol)
    }

    fn remember_completion(&mut self, message: &Value) -> Result<(), ProcessError> {
        if message.get("method").and_then(Value::as_str) == Some("account/login/completed") {
            if self.completions.len() >= MAX_PENDING {
                return Err(ProcessError::Protocol);
            }
            let event = serde_json::from_value(
                message
                    .get("params")
                    .cloned()
                    .ok_or(ProcessError::Protocol)?,
            )
            .map_err(|_| ProcessError::Protocol)?;
            self.completions.push_back(event);
        }
        Ok(())
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, ProcessError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"id":id, "method":method, "params":params}))
            .await?;
        loop {
            let message = self.receive().await?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if message.get("error").is_some() {
                    // Never return vendor error text: it may include secrets.
                    return Err(ProcessError::Rejected);
                }
                return message.get("result").cloned().ok_or(ProcessError::Protocol);
            }
            self.remember_completion(&message)?;
        }
    }

    /// The app-server performs the official polling. Success also requires an
    /// account/read matching subscription auth; no model request is made.
    pub async fn wait(&mut self) -> Result<(), ProcessError> {
        while self.flow.state() == State::AwaitingUser {
            if let Some(event) = self.completions.pop_front() {
                match self.flow.completed(event) {
                    Ok(()) => {}
                    Err(super::AuthError::UnrelatedCompletion) => continue,
                    Err(_) => return Err(ProcessError::Protocol),
                }
            } else {
                let message = self.receive().await?;
                self.remember_completion(&message)?;
            }
        }
        if self.flow.state() != State::AwaitingAccountRead {
            return Err(ProcessError::Rejected);
        }
        let account: AccountRead = serde_json::from_value(
            self.rpc("account/read", json!({"refreshToken":false}))
                .await?,
        )
        .map_err(|_| ProcessError::Protocol)?;
        self.flow
            .account_read(account)
            .map_err(|_| ProcessError::AccountMismatch)?;
        self.shutdown().await
    }

    /// Kill and reap before the embedding reads or removes the managed home.
    pub async fn shutdown(&mut self) -> Result<(), ProcessError> {
        self.flow.cancel();
        if self
            .child
            .try_wait()
            .map_err(|_| ProcessError::Unavailable)?
            .is_none()
        {
            self.child
                .kill()
                .await
                .map_err(|_| ProcessError::Unavailable)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ProcessError {
    #[error("Codex login process configuration is invalid")]
    Configuration,
    #[error("pinned Codex executable is unavailable")]
    Unavailable,
    #[error("Codex executable version differs from the protocol pin")]
    VersionMismatch,
    #[error("Codex login expired; reconnect ChatGPT")]
    Timeout,
    #[error("Codex rejected sign-in; check device-login permission and reconnect")]
    Rejected,
    #[error("Codex login protocol failed; reconnect ChatGPT")]
    Protocol,
    #[error("Codex account does not match subscription sign-in")]
    AccountMismatch,
}
