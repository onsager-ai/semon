//! Transient, source-relative facts for the origin warehouse writer.
//!
//! Only native metadata and positions cross this boundary. The Index writer
//! will bind the origin, logical source and incarnation when it commits.

use super::{BillingUsage, Event, Kind, ModelTokens, Peer, Reply};
use crate::Tokens;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Position {
    pub(crate) offset: u64,
    pub(crate) block: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Observation {
    pub(crate) position: Position,
    pub(crate) ordinal: Option<u64>,
    /// With the payload kind and position, distinguishes observations from
    /// the same record. Restarts at each complete physical line.
    pub(crate) discriminator: usize,
    pub(crate) fact: Fact,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fact {
    Record {
        kind: Option<String>,
        uuid: Option<String>,
        message_id: Option<String>,
        time: Option<i64>,
    },
    Gap,
    SessionId(String),
    Relation {
        kind: &'static str,
        target: String,
    },
    Parent {
        harness: String,
        id: String,
        call_id: Option<String>,
    },
    LaunchCandidate {
        call_id: String,
        tool: String,
    },
    ClaudeUsage {
        message_id: String,
        model: Option<String>,
        tokens: Tokens,
        billing: BillingUsage,
    },
    CodexUsage {
        identity: String,
        model: Option<String>,
        tokens: Tokens,
        model_tokens: ModelTokens,
        time: Option<i64>,
        accepted: bool,
    },
    CodexTokenSnapshot {
        model: Option<String>,
        tokens: Tokens,
        time: Option<i64>,
    },
    CodexTurn {
        thread_id: String,
        turn_id: String,
        phase: crate::codex_lifecycle::TurnPhase,
    },
    Compact {
        time: Option<i64>,
        trigger: Option<String>,
        pre_tokens: Option<u64>,
    },
    ToolCall {
        id: Option<String>,
        tool: Option<String>,
        flags: u8,
        time: Option<i64>,
        target: Option<String>,
        task_name: Option<String>,
    },
    ToolResult {
        id: String,
        call: Option<Position>,
        reply: Reply,
        denial: Option<String>,
        exit_code: Option<i64>,
        target: Option<String>,
        kind: &'static str,
    },
    Event {
        kind: Kind,
        id: Option<String>,
        name: Option<String>,
        flags: u8,
        time: Option<i64>,
        peer: Option<Peer>,
        target: Option<String>,
    },
}

impl Fact {
    pub(super) fn event(event: &Event) -> Self {
        Self::Event {
            kind: event.k,
            id: event.id.clone(),
            name: event.n.clone(),
            flags: event.script,
            time: event.t,
            peer: event.peer.clone(),
            target: None,
        }
    }
}

/// Independent first-line witness. A provisional header is verified against
/// the source but does not own a complete-record ordinal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) native_id: String,
    pub(crate) metadata: crate::model::CodexMeta,
    pub(crate) complete: bool,
}

/// The sidecar owns its own revision. The batch carries only the projected
/// metadata, while the enclosing FileIndex supplies the verified witness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sidecar {
    Absent,
    Present(crate::native_sidecar::AgentMetadata),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Batch {
    pub(crate) harness: &'static str,
    pub(crate) observations: Vec<Observation>,
    pub(crate) header: Option<Header>,
    pub(crate) sidecar: Option<Sidecar>,
    ordinal: Option<u64>,
    record_start: usize,
}

impl Batch {
    pub(super) fn new(harness: &str) -> Option<Self> {
        let harness = match harness {
            "claude" => "claude",
            "codex" => "codex",
            _ => return None,
        };
        Some(Self {
            harness,
            observations: Vec::new(),
            header: None,
            sidecar: None,
            ordinal: None,
            record_start: 0,
        })
    }

    pub(super) fn begin_record(&mut self, ordinal: Option<u64>) {
        self.ordinal = ordinal;
        self.record_start = self.observations.len();
    }

    pub(super) fn push(&mut self, offset: u64, block: u32, fact: Fact) {
        self.observations.push(Observation {
            position: Position { offset, block },
            ordinal: self.ordinal,
            discriminator: self.observations.len() - self.record_start,
            fact,
        });
    }
}
