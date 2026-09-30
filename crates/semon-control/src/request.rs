//! The pending-request model's types: what a request is, what can answer it,
//! and the states it moves through.

use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::{self, Read},
};

use serde_json::{Value, json};

use crate::canonical::{self, CanonicalError};

/// Semon's own id for a request: 128 random bits, written as 32 lowercase
/// hex digits. Never the harness's id, which can be short and guessable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId([u8; 16]);

impl RequestId {
    /// A fresh id from the kernel's random source.
    pub fn random() -> io::Result<Self> {
        let mut bytes = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// The id with these bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Parses exactly 32 lowercase hex digits; anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 32
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        let mut bytes = [0_u8; 16];
        hex::decode_to_slice(text, &mut bytes).ok()?;
        Some(Self(bytes))
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

/// The harness a request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Harness {
    /// Claude Code, through its hooks.
    Claude,
    /// Codex, through its app-server daemon (step 1b).
    Codex,
    /// OpenCode, through its TUI's server (step 1b).
    OpenCode,
}

impl Harness {
    /// The name the journal and the viewer use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }
}

/// What the agent is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A permission prompt: allow once or deny.
    Permission,
    /// A question with options.
    Question,
}

impl Kind {
    /// The name the journal and the viewer use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
        }
    }
}

/// Whether Semon holds the verbatim content the harness will act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answerable {
    /// It does; the request can be answered here.
    Yes,
    /// It doesn't, or can't be answered here for another reason; the request
    /// is shown read-only with this reason.
    No(String),
}

/// How a later report that a tool ran finds its request (Claude only):
/// Claude's `PermissionRequest` carries no tool-use id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchKey {
    /// The tool's name, as the hook reported it.
    pub tool_name: String,
    /// The lowercase hex SHA-256 of the tool input's canonical encoding.
    pub input_sha256: String,
}

impl MatchKey {
    /// The key for a Claude tool call. For `AskUserQuestion` the input's
    /// `answers` field is left out, since an answered call carries one and
    /// the request it answers doesn't.
    pub fn claude(tool_name: &str, tool_input: &Value) -> Result<Self, CanonicalError> {
        let input_sha256 = match tool_input {
            Value::Object(fields) if tool_name == "AskUserQuestion" => {
                let mut without_answers = fields.clone();
                without_answers.remove("answers");
                canonical::sha256_hex(&Value::Object(without_answers))?
            }
            _ => canonical::sha256_hex(tool_input)?,
        };
        Ok(Self {
            tool_name: tool_name.to_owned(),
            input_sha256,
        })
    }
}

/// Who sent an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The viewer window that sent it.
    pub window: String,
    /// The TCP peer's process id.
    pub peer_pid: u32,
    /// The TCP peer's user id.
    pub peer_uid: u32,
}

impl Source {
    pub(crate) fn to_json(&self) -> Value {
        json!({"window": self.window, "peer_pid": self.peer_pid, "peer_uid": self.peer_uid})
    }
}

/// An answer, as the human gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Allow once. Nothing broader exists in step 1.
    Allow,
    /// Deny, with the human's own words if they typed any.
    Deny {
        /// The human's message, if any.
        message: Option<String>,
    },
    /// Each question's text mapped to the label of the chosen option.
    Questions(BTreeMap<String, String>),
}

impl Answer {
    /// Whether this answer has the shape `kind` takes.
    pub fn fits(&self, kind: Kind) -> bool {
        match self {
            Self::Allow | Self::Deny { .. } => kind == Kind::Permission,
            Self::Questions(answers) => kind == Kind::Question && !answers.is_empty(),
        }
    }

    pub(crate) fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::Allow => json!({"decision": "allow"}),
            Self::Deny { message } => json!({"decision": "deny", "message": message}),
            Self::Questions(answers) => json!({"answers": answers}),
        }
    }
}

/// Why a request ended elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedReason {
    /// Claude: a report that the tool ran (a `PostToolUse` hook, or the
    /// transcript's tool result) matched the request while it was open.
    AllowedInTerminal,
    /// Claude: Claude ended the hook (No, Esc or an interrupt in the
    /// terminal), so its connection closed from Claude's side.
    DeniedOrInterruptedInTerminal,
    /// Another client answered it, as the harness reported.
    OtherClient,
}

impl ResolvedReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::AllowedInTerminal => "allowed_in_terminal",
            Self::DeniedOrInterruptedInTerminal => "denied_or_interrupted_in_terminal",
            Self::OtherClient => "other_client",
        }
    }
}

/// Why a request was handed back to the harness's own prompt. It never means
/// the terminal answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftReason {
    /// The request expired before anyone answered it here.
    TimedOut,
    /// An answer was claimed but its delivery didn't end by the deadline.
    DeliveryUnknown,
}

impl LeftReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::TimedOut => "timed_out",
            Self::DeliveryUnknown => "delivery_unknown",
        }
    }
}

/// Where a request is: `Open`, then `Claimed` while an answer is being
/// delivered, then exactly one final state, which never changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Waiting for an answer.
    Open,
    /// An answer won the claim and is being delivered.
    Claimed {
        /// The answer being delivered.
        answer: Answer,
        /// Who sent it.
        source: Source,
        /// When the claim was made.
        at_ms: u64,
    },
    /// Semon delivered the answer, as far as the harness lets it see.
    Answered {
        /// The answer delivered.
        answer: Answer,
        /// Who sent it.
        source: Source,
    },
    /// It ended elsewhere.
    Resolved(ResolvedReason),
    /// The harness refused a delivered answer with this status (OpenCode).
    Rejected {
        /// The answer refused.
        answer: Answer,
        /// Who sent it.
        source: Source,
        /// The harness's status code.
        status: u16,
    },
    /// Handed back to the harness's own prompt.
    Left(LeftReason),
    /// The session or the adapter's connection to the harness ended.
    Gone,
}

impl State {
    /// Whether this is one of the final states.
    pub fn is_final(&self) -> bool {
        !matches!(self, Self::Open | Self::Claimed { .. })
    }

    /// The state's name: `open`, `claimed`, `answered`, `resolved`,
    /// `rejected`, `left` or `gone`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claimed { .. } => "claimed",
            Self::Answered { .. } => "answered",
            Self::Resolved(_) => "resolved",
            Self::Rejected { .. } => "rejected",
            Self::Left(_) => "left",
            Self::Gone => "gone",
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::Resolved(reason) => json!({"state": self.name(), "reason": reason.as_str()}),
            Self::Left(reason) => json!({"state": self.name(), "reason": reason.as_str()}),
            Self::Rejected { status, .. } => json!({"state": self.name(), "status": status}),
            _ => json!({"state": self.name()}),
        }
    }
}

/// A request as an adapter registers it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRequest {
    /// The Semon session key the request belongs to.
    pub session: String,
    /// The harness it came from.
    pub harness: Harness,
    /// The harness's own reference (a JSON-RPC id, a request id, a hook
    /// connection), for the adapter's use only; never shown.
    pub harness_ref: String,
    /// Permission or question.
    pub kind: Kind,
    /// What the human is shown, verbatim from the harness.
    pub payload: Value,
    /// The adapter's reason the request can't be answered here, if any.
    pub read_only: Option<String>,
    /// Claude: how a later tool-run report finds this request.
    pub match_key: Option<MatchKey>,
    /// Whether a Claude hook connection waits on this request.
    pub hook_wait: bool,
    /// The earliest of the harness's deadline and Semon's.
    pub expires_ms: u64,
}

/// One question or permission prompt the store has seen.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRequest {
    /// Semon's id for it.
    pub id: RequestId,
    /// The Semon session key.
    pub session: String,
    /// The harness it came from.
    pub harness: Harness,
    /// The harness's own reference; never shown.
    pub harness_ref: String,
    /// Permission or question.
    pub kind: Kind,
    /// What the human is shown, verbatim.
    pub payload: Value,
    /// Whether it can be answered here.
    pub answerable: Answerable,
    /// The SHA-256 of the payload's canonical encoding; `None` when it has
    /// none, in which case the request is read-only.
    pub payload_sha256: Option<String>,
    /// Claude: how a later tool-run report finds it.
    pub match_key: Option<MatchKey>,
    /// Whether a Claude hook connection waits on it.
    pub hook_wait: bool,
    /// When the store first saw it.
    pub created_ms: u64,
    /// After this, answers are refused and it becomes `left`, timed out.
    pub expires_ms: u64,
    /// Where it is.
    pub state: State,
    /// When it reached its final state.
    pub ended_ms: Option<u64>,
    /// Whether a tool-run report has been matched to it.
    pub tool_run_matched: bool,
    /// Whether the tool ran although a deny from the viewer was delivered:
    /// the terminal allowed it first.
    pub tool_ran_after_deny: bool,
}
