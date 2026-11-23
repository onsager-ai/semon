//! Transient, source-relative facts for the origin warehouse writer.
//!
//! Only native metadata and positions cross this boundary. The Index writer
//! will bind the origin, logical source and incarnation when it commits.

use super::{BillingUsage, Event, Kind, ModelTokens, Peer, Reply};
use crate::Tokens;
type RuntimeParent = (String, String, Option<String>);

pub(crate) struct RuntimeSource<'a> {
    pub(crate) path: &'a str,
    pub(crate) harness: &'a str,
    pub(crate) native_id: &'a str,
    pub(crate) revision: super::FileRevision,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeBinding {
    pub(crate) index: String,
    pub(crate) namespace: String,
    pub(crate) source: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeCapture {
    pub(crate) pid: u32,
    pub(crate) start: u64,
    pub(crate) clock: i64,
    pub(crate) environment: Option<RuntimeParent>,
    pub(crate) ancestors: Vec<u32>,
}

impl RuntimeCapture {
    pub(crate) fn read(
        options: &crate::Options,
        facts: &crate::facts::MachineFacts,
        pid: u32,
        start: u64,
    ) -> Option<Self> {
        Self::acquire(
            pid,
            start,
            || facts.proc_start(options, pid),
            || {
                let environment = facts
                    .run(options, pid, Some(start))
                    .and_then(|run| crate::parent_links::parse(run.get("SEMON_PARENT")?))
                    .and_then(|(parent, call)| {
                        let (harness, id) = parent.split_once(':')?;
                        Some((harness.to_owned(), id.to_owned(), call))
                    });
                let ancestors = facts.ancestors(options, pid).unwrap_or_default();
                (environment, ancestors)
            },
        )
    }

    fn acquire(
        pid: u32,
        start: u64,
        process_start: impl Fn() -> Option<u64>,
        observe: impl FnOnce() -> (Option<RuntimeParent>, Vec<u32>),
    ) -> Option<Self> {
        if process_start() != Some(start) || pid == 0 || i64::try_from(start).is_err() {
            return None;
        }
        let (environment, mut ancestors) = observe();
        ancestors.truncate(64);
        (process_start() == Some(start)).then(|| Self {
            pid,
            start,
            clock: crate::model::now_ms(),
            environment,
            ancestors,
        })
    }
}

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
        parent_id: Option<String>,
        time: Option<i64>,
    },
    Gap,
    PromptImages(crate::attachments::PromptMetadata),
    NativeFields(crate::native_field::SparseRecord),
    SessionId(String),
    Title {
        present: bool,
    },
    Model(String),
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
    CopilotShutdown {
        event_id: String,
        time: Option<i64>,
        usage: Option<crate::copilot::CopilotUsage>,
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
    Operation {
        parent: Position,
        tool: &'static str,
        ok: Option<bool>,
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
    pub(crate) eof: Vec<Observation>,
    ordinal: Option<u64>,
    record_start: usize,
}

impl Batch {
    pub(super) fn new(harness: &str) -> Option<Self> {
        let harness = match harness {
            "claude" => "claude",
            "codex" => "codex",
            "copilot" => "copilot",
            _ => return None,
        };
        Some(Self {
            harness,
            observations: Vec::new(),
            header: None,
            sidecar: None,
            eof: Vec::new(),
            ordinal: None,
            record_start: 0,
        })
    }

    pub(super) fn begin_record(&mut self, ordinal: Option<u64>) {
        self.ordinal = ordinal;
        self.record_start = self.observations.len();
    }

    pub(super) fn push_eof(&mut self, offset: u64, block: u32, fact: Fact) {
        self.eof.push(Observation {
            position: Position { offset, block },
            ordinal: None,
            discriminator: self.eof.len(),
            fact,
        });
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

#[cfg(test)]
mod runtime_tests {
    use std::cell::Cell;

    use super::RuntimeCapture;

    #[test]
    fn reused_pid_cannot_supply_environment_or_ancestry() {
        let process_start = Cell::new(Some(10));
        assert!(
            RuntimeCapture::acquire(
                100,
                10,
                || process_start.get(),
                || {
                    process_start.set(Some(11));
                    (Some(("claude".into(), "parent".into(), None)), vec![1])
                }
            )
            .is_none()
        );
        assert!(
            RuntimeCapture::acquire(
                100,
                10,
                || process_start.get(),
                || { panic!("a reused PID must be rejected before reading its environment") }
            )
            .is_none()
        );
        let capture =
            RuntimeCapture::acquire(100, 11, || process_start.get(), || (None, vec![1])).unwrap();
        assert_eq!(capture.start, 11);
    }
}
