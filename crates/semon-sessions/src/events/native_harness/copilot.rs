//! Copilot event body phases. Header/lifecycle/usage facts remain at their
//! qualified native metadata owner. Complete Value and Original records drive
//! these same phases, including every historical occurrence reset.

use super::record::{RecordSource, member_bool, member_literal, member_text};
use super::{
    CopilotStore, ExtraStore, Reset, copilot_completion_reply, copilot_completion_target,
    copilot_reset_reply_step,
};
use crate::events::native_event::NativeEvent;
use crate::events::native_producer::{self, SignalStore};
use crate::events::{Kind, SignalKind};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Phase<N, T> {
    Dispatch,
    DispatchKind {
        data: Option<N>,
        kind: Option<N>,
        index: u8,
    },
    User {
        data: Option<N>,
    },
    Assistant {
        data: Option<N>,
    },
    Requests {
        array: N,
        after: Option<u64>,
    },
    Reset {
        array: N,
        index: u64,
        event: NativeEvent<T>,
        after: Option<usize>,
    },
    Append {
        array: N,
        index: u64,
        event: NativeEvent<T>,
    },
    Completion {
        data: Option<N>,
    },
    Subagent {
        data: Option<N>,
        name: T,
    },
    Done,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Cursor<N, T> {
    pub(crate) phase: Phase<N, T>,
    pub(crate) base: NativeEvent<T>,
}
impl<N, T> Cursor<N, T> {
    pub(crate) fn new(base: NativeEvent<T>) -> Self {
        Self {
            phase: Phase::Dispatch,
            base,
        }
    }
    pub(crate) fn complete(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Action<T> {
    None,
    Append(NativeEvent<T>),
    Tool(NativeEvent<T>),
    Extra(NativeEvent<T>),
    Reset {
        id: T,
        after: Option<usize>,
    },
    Completion {
        id: T,
        target: usize,
        denied: bool,
        reply: crate::events::native_event::NativeReply<T>,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct Prepared<N, T> {
    next: Phase<N, T>,
    action: Action<T>,
}

/// A phase may qualify several fixed properties, or mutate one native event.
/// Original checkpoints the resulting cursor with the event transaction and
/// all source/publication fences; Deferred never advances this cursor.
pub(crate) fn prepare<R, S>(
    record: &mut R,
    store: &S,
    cursor: &Cursor<R::Node, R::Text>,
) -> Result<Prepared<R::Node, R::Text>, R::Error>
where
    R: RecordSource,
    S: CopilotStore<Text = R::Text>,
    R::Error: From<S::Error>,
{
    let mut action = Action::None;
    let next = match &cursor.phase {
        Phase::Dispatch => {
            let root = record.root();
            let data = record.member(&root, "data")?;
            let kind = record.member(&root, "type")?;
            Phase::DispatchKind {
                data,
                kind,
                index: 0,
            }
        }
        Phase::DispatchKind { data, kind, index } => {
            let literals = [
                "user.message",
                "assistant.message",
                "tool.execution_complete",
                "subagent.started",
                "subagent.completed",
            ];
            let matches = match kind {
                Some(kind) => record.literal(kind, literals[usize::from(*index)])?,
                None => false,
            };
            if matches && *index == 0 {
                Phase::User { data: data.clone() }
            } else if matches && *index == 1 {
                Phase::Assistant { data: data.clone() }
            } else if matches && *index == 2 {
                Phase::Completion { data: data.clone() }
            } else if matches {
                match kind {
                    Some(kind) => match record.text(kind)? {
                        Some(name) => Phase::Subagent {
                            data: data.clone(),
                            name,
                        },
                        None => Phase::Done,
                    },
                    None => Phase::Done,
                }
            } else if *index < 4 {
                Phase::DispatchKind {
                    data: data.clone(),
                    kind: kind.clone(),
                    index: *index + 1,
                }
            } else {
                Phase::Done
            }
        }
        Phase::User { data } => {
            let content = match data {
                Some(data) => record.member(data, "content")?,
                None => None,
            };
            if let Some(content) = content
                && record.string_present(&content)?
            {
                let id = member_text(record, data.as_ref(), "messageId")?;
                action = Action::Append(NativeEvent {
                    k: Kind::U,
                    id,
                    ..cursor.base.clone()
                });
            }
            Phase::Done
        }
        Phase::Assistant { data } => {
            let content = match data {
                Some(data) => record.member(data, "content")?,
                None => None,
            };
            let id = if let Some(content) = &content
                && record.string_nonempty(content)?
            {
                member_text(record, data.as_ref(), "messageId")?
            } else {
                None
            };
            let array = match data {
                Some(data) => record.member(data, "toolRequests")?,
                None => None,
            };
            // Qualify every property before event I/O so a Pending property
            // cannot accidentally commit a duplicate assistant event.
            let emit = match content {
                Some(content) => record.string_nonempty(&content)?,
                None => false,
            };
            if emit {
                action = Action::Append(NativeEvent {
                    k: Kind::A,
                    id,
                    ..cursor.base.clone()
                });
            }
            match array {
                Some(array) => Phase::Requests { array, after: None },
                None => Phase::Done,
            }
        }
        Phase::Requests { array, after } => match record.array_next(array, *after)? {
            None => Phase::Done,
            Some((index, request)) => {
                let id = member_text(record, Some(&request), "toolCallId")?;
                let id = match id {
                    Some(id) if record.text_nonempty(&id)? => Some(id),
                    _ => None,
                };
                let name = member_text(record, Some(&request), "name")?;
                let event = NativeEvent {
                    k: Kind::Tool,
                    b: index as u32,
                    id,
                    n: name,
                    ..cursor.base.clone()
                };
                Phase::Reset {
                    array: array.clone(),
                    index,
                    event,
                    after: None,
                }
            }
        },
        Phase::Reset {
            array,
            index,
            event,
            after,
        } => {
            if let Some(id) = &event.id {
                action = Action::Reset {
                    id: id.clone(),
                    after: *after,
                };
                cursor.phase.clone()
            } else {
                Phase::Append {
                    array: array.clone(),
                    index: *index,
                    event: event.clone(),
                }
            }
        }
        Phase::Append {
            array,
            index,
            event,
        } => {
            action = Action::Tool(event.clone());
            Phase::Requests {
                array: array.clone(),
                after: Some(*index),
            }
        }
        Phase::Completion { data } => {
            let id = member_text(record, data.as_ref(), "toolCallId")?;
            if let Some(id) = id
                && let Some(target) =
                    copilot_completion_target(store, id.borrow()).map_err(R::Error::from)?
            {
                let success = member_bool(record, data.as_ref(), "success")?;
                let shell = match data {
                    Some(data) => record.member(data, "shellExecution")?,
                    None => None,
                };
                let exit = match shell {
                    Some(shell) => match record.member(&shell, "exitCode")? {
                        Some(exit) => record.integer(&exit)?,
                        None => None,
                    },
                    None => None,
                };
                let error = match data {
                    Some(data) => record.member(data, "error")?,
                    None => None,
                };
                let denied = member_literal(record, error.as_ref(), "code", "denied")?;
                let name = store
                    .event(target)
                    .map_err(R::Error::from)?
                    .and_then(|event| event.n);
                let bash = match name {
                    Some(name) => record.text_literal(&name, "bash")?,
                    None => false,
                };
                let reply = copilot_completion_reply::<R::Text>(
                    cursor.base.o,
                    cursor.base.t,
                    success,
                    exit,
                    denied,
                    bash,
                );
                action = Action::Completion {
                    id,
                    target,
                    denied,
                    reply,
                };
            }
            Phase::Done
        }
        Phase::Subagent { data, name } => {
            let id = member_text(record, data.as_ref(), "toolCallId")?;
            action = Action::Extra(NativeEvent {
                k: Kind::Harness,
                n: Some(name.clone()),
                id,
                ..cursor.base.clone()
            });
            Phase::Done
        }
        Phase::Done => Phase::Done,
    };
    Ok(Prepared { next, action })
}

/// Apply only after Original property/source I/O finishes and the native owner
/// rechecks all publication/source/semantic-cursor witnesses in its transaction.
/// This function performs no Record/provider I/O; the complete producer calls
/// it immediately after preparing each same phase.
pub(crate) fn apply<N: Clone, T: Clone, S>(
    store: &mut S,
    cursor: &mut Cursor<N, T>,
    prepared: Prepared<N, T>,
) -> Result<(), S::Error>
where
    S: CopilotStore<Text = T> + SignalStore + ExtraStore,
{
    let mut next = prepared.next;
    match prepared.action {
        Action::None => {}
        Action::Append(event) => {
            store.append_event(event)?;
        }
        Action::Tool(event) => native_producer::tool_event(store, event)?,
        Action::Extra(event) => store.append_extra(event)?,
        Action::Reset { id, after } => match copilot_reset_reply_step(store, id.borrow(), after)? {
            Reset::Progress(sequence) => {
                let Phase::Reset { after, .. } = &mut next else {
                    unreachable!("prepared reset phase")
                };
                *after = Some(sequence);
            }
            Reset::Complete => {
                let Phase::Reset {
                    array,
                    index,
                    event,
                    ..
                } = next
                else {
                    unreachable!("prepared reset phase")
                };
                next = Phase::Append {
                    array,
                    index,
                    event,
                };
            }
        },
        Action::Completion {
            id,
            target,
            denied,
            reply,
        } => {
            if denied {
                native_producer::note(
                    store,
                    SignalKind::Denial,
                    cursor.base.o,
                    cursor.base.t,
                    "permission_denied".into(),
                    u64::try_from(target).ok(),
                )?;
            }
            native_producer::resolve(store, id.borrow(), |_| reply)?;
        }
    }
    cursor.phase = next;
    Ok(())
}

pub(crate) fn step<R, S>(
    record: &mut R,
    store: &mut S,
    cursor: &mut Cursor<R::Node, R::Text>,
) -> Result<(), R::Error>
where
    R: RecordSource,
    S: CopilotStore<Text = R::Text, Error = R::Error> + SignalStore + ExtraStore,
{
    let prepared = prepare(record, store, cursor)?;
    apply(store, cursor, prepared)
}
