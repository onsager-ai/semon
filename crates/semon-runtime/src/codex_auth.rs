//! Authentication protocol subset for Codex CLI 0.159.0-alpha.3.
//!
//! This is an ephemeral guest-side login exchange, not credential storage or
//! hosted-use qualification. The embedding server authenticates the owner,
//! isolates the Codex home, retains task/connection references, and transports
//! these messages over a protected app-server connection. Exec/resume is unchanged.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

#[cfg(feature = "codex-process")]
pub mod process;

pub const CODEX_VERSION: &str = "0.159.0-alpha.3";
pub const LOGIN_START_METHOD: &str = "account/login/start";
pub const LOGIN_COMPLETED_METHOD: &str = "account/login/completed";

/// The user's selection is immutable for an exchange; there is no fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    OpenAiApiKey,
    ChatGptDeviceCode,
}

/// Secret-bearing requests are neither deserializable nor persisted here.
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum LoginRequest<'a> {
    #[serde(rename = "apiKey")]
    ApiKey {
        #[serde(rename = "apiKey")]
        api_key: &'a str,
    },
    #[serde(rename = "chatgptDeviceCode")]
    DeviceCode,
}

impl fmt::Debug for LoginRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey { .. } => f.write_str("ApiKey([REDACTED])"),
            Self::DeviceCode => f.write_str("DeviceCode"),
        }
    }
}

/// Decode only the two selected public routes. In particular, the pinned
/// protocol's internal chatgptAuthTokens route is deliberately unsupported.
#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum LoginResponse {
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "chatgptDeviceCode", rename_all = "camelCase")]
    DeviceCode {
        login_id: String,
        verification_url: String,
        user_code: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginCompleted {
    login_id: Option<String>,
    success: bool,
    // Vendor error strings are ignored: they can contain sensitive context.
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRead {
    account: Option<Account>,
    requires_openai_auth: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Account {
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "chatgpt")]
    ChatGpt,
}

/// Safe to display only to the authenticated connection owner. Never log it.
/// Codex owns the polling and managed credential refresh; Hub owns timeout,
/// cancellation and protected custody of the resulting isolated Codex home.
pub struct DeviceChallenge {
    login_id: String,
    verification_url: String,
    user_code: String,
}

impl DeviceChallenge {
    pub fn login_id(&self) -> &str {
        &self.login_id
    }
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }
    pub fn user_code(&self) -> &str {
        &self.user_code
    }
}

impl fmt::Debug for DeviceChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceChallenge([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotStarted,
    AwaitingUser,
    AwaitingAccountRead,
    /// A local credential is present, not proof of model access/hosted suitability.
    AccountPresent,
    Failed,
    Cancelled,
}

/// Retained in memory only; reconstructing a lost exchange requires reconnect,
/// not replaying a login or treating an unrelated account notification as success.
pub struct LoginFlow {
    method: Method,
    state: State,
    challenge: Option<DeviceChallenge>,
}

impl fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginFlow")
            .field("method", &self.method)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl LoginFlow {
    pub fn new(method: Method, observed_version: &str) -> Result<Self, AuthError> {
        if observed_version != CODEX_VERSION {
            return Err(AuthError::VersionMismatch);
        }
        Ok(Self {
            method,
            state: State::NotStarted,
            challenge: None,
        })
    }

    pub fn method(&self) -> Method {
        self.method
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn challenge(&self) -> Option<&DeviceChallenge> {
        self.challenge.as_ref()
    }

    /// Call once for the correlated account/login/start response. The transport
    /// must commit/claim its exchange identity before issuing the request.
    pub fn started(&mut self, response: LoginResponse) -> Result<(), AuthError> {
        if self.state != State::NotStarted {
            return Err(AuthError::UnexpectedState);
        }
        match (self.method, response) {
            (Method::OpenAiApiKey, LoginResponse::ApiKey) => {
                self.state = State::AwaitingAccountRead;
            }
            (
                Method::ChatGptDeviceCode,
                LoginResponse::DeviceCode {
                    login_id,
                    verification_url,
                    user_code,
                },
            ) => {
                // Fail closed on a protocol change instead of rendering an
                // untrusted URL, userinfo, alternate port or redirect target.
                if verification_url != "https://auth.openai.com/codex/device"
                    || login_id.is_empty()
                    || login_id.len() > 256
                    || login_id.chars().any(char::is_control)
                    || user_code.is_empty()
                    || user_code.len() > 128
                    || user_code.chars().any(char::is_control)
                {
                    self.state = State::Failed;
                    return Err(AuthError::InvalidChallenge);
                }
                self.challenge = Some(DeviceChallenge {
                    login_id,
                    verification_url,
                    user_code,
                });
                self.state = State::AwaitingUser;
            }
            _ => {
                self.state = State::Failed;
                return Err(AuthError::BillingMethodMismatch);
            }
        }
        Ok(())
    }

    /// Unrelated/late completions never consume the live exchange. Success
    /// requires a subsequent account/read with refreshToken=false in this home.
    pub fn completed(&mut self, event: LoginCompleted) -> Result<(), AuthError> {
        if self.state != State::AwaitingUser {
            return Err(AuthError::UnexpectedState);
        }
        if event.login_id.as_deref() != self.challenge.as_ref().map(|c| c.login_id.as_str()) {
            return Err(AuthError::UnrelatedCompletion);
        }
        self.challenge = None;
        self.state = if event.success {
            State::AwaitingAccountRead
        } else {
            State::Failed
        };
        Ok(())
    }

    pub fn account_read(&mut self, response: AccountRead) -> Result<(), AuthError> {
        if self.state != State::AwaitingAccountRead {
            return Err(AuthError::UnexpectedState);
        }
        if !response.requires_openai_auth {
            self.state = State::Failed;
            return Err(AuthError::BillingMethodMismatch);
        }
        match (self.method, response.account) {
            (Method::OpenAiApiKey, Some(Account::ApiKey))
            | (Method::ChatGptDeviceCode, Some(Account::ChatGpt)) => {
                self.state = State::AccountPresent;
                Ok(())
            }
            (_, None) => {
                self.state = State::Failed;
                Err(AuthError::AccountMissing)
            }
            _ => {
                self.state = State::Failed;
                Err(AuthError::BillingMethodMismatch)
            }
        }
    }

    /// The embedding must also request account/login/cancel for a pending
    /// loginId, or terminate the isolated process, before dropping custody.
    pub fn cancel(&mut self) {
        self.challenge = None;
        self.state = State::Cancelled;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum AuthError {
    #[error("Codex version differs from the qualified protocol pin")]
    VersionMismatch,
    #[error("unexpected login state")]
    UnexpectedState,
    #[error("invalid official device challenge")]
    InvalidChallenge,
    #[error("completion does not belong to the current login")]
    UnrelatedCompletion,
    #[error("selected authentication/billing method does not match Codex")]
    BillingMethodMismatch,
    #[error("Codex account is absent; reconnect the selected method")]
    AccountMissing,
}
