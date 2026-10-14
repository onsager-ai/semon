//! Native fixed Copilot metadata actions. Cumulative shutdown snapshot remains
//! a separate native producer prerequisite, indicated by snapshot_required.
use super::header::{self, Field, Value};
use super::record::{RecordSource, member_text};
use crate::events::SignalKind;
use crate::events::native_producer::{self, SignalStore};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum Mode {
    Start,
    Change,
    Assistant,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Phase<N> {
    Time,
    Dispatch,
    Kind {
        data: Option<N>,
        kind: Option<N>,
        index: u8,
    },
    Cwd {
        data: Option<N>,
    },
    Entrypoint {
        data: Option<N>,
    },
    Model {
        data: Option<N>,
        mode: Mode,
    },
    ModelTag {
        data: Option<N>,
        model: Option<N>,
        mode: Mode,
    },
    Effort {
        data: Option<N>,
    },
    Lifecycle(ValueKind),
    Abort {
        data: Option<N>,
    },
    Compact,
    Done {
        snapshot_required: bool,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum ValueKind {
    Start,
    Resume,
    Shutdown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Cursor<N> {
    pub(crate) phase: Phase<N>,
    pub(crate) offset: u64,
    pub(crate) time: Option<i64>,
}
impl<N> Cursor<N> {
    pub(crate) fn valid(&self) -> bool {
        !matches!(self.phase,Phase::Kind{index,..} if index>6)
    }
    pub(crate) fn new(offset: u64, time: Option<i64>) -> Self {
        Self {
            phase: Phase::Time,
            offset,
            time,
        }
    }
    pub(crate) fn result(&self) -> Option<bool> {
        match self.phase {
            Phase::Done { snapshot_required } => Some(snapshot_required),
            _ => None,
        }
    }
}
enum Action<T> {
    None,
    Header(Field, Option<Value<T>>),
    Times(T, bool),
    Setting(SignalKind, Option<String>),
    Compact,
}
pub(crate) struct Prepared<N, T> {
    next: Phase<N>,
    action: Action<T>,
}
pub(crate) fn prepare<R, S>(
    record: &mut R,
    store: &S,
    cursor: &Cursor<R::Node>,
) -> Result<Prepared<R::Node, R::Text>, R::Error>
where
    R: RecordSource,
    S: header::Store<Text = R::Text>,
    R::Error: From<S::Error>,
{
    let mut action = Action::None;
    let next = match &cursor.phase {
        Phase::Time => {
            if cursor.time.is_some() {
                let root = record.root();
                if let Some(timestamp) = record.member(&root, "timestamp")?
                    && let Some(timestamp) = record.text(&timestamp)?
                {
                    let first = store
                        .header(Field::CopilotFirst)
                        .map_err(R::Error::from)?
                        .is_none();
                    action = Action::Times(timestamp, first);
                }
            }
            Phase::Dispatch
        }
        Phase::Dispatch => {
            let root = record.root();
            Phase::Kind {
                data: record.member(&root, "data")?,
                kind: record.member(&root, "type")?,
                index: 0,
            }
        }
        Phase::Kind { data, kind, index } => {
            let literals = [
                "session.start",
                "session.resume",
                "session.model_change",
                "assistant.message",
                "abort",
                "session.compaction_complete",
                "session.shutdown",
            ];
            let found = match kind {
                Some(kind) => record.literal(kind, literals[usize::from(*index)])?,
                None => false,
            };
            if found {
                match index {
                    0 => Phase::Cwd { data: data.clone() },
                    1 => Phase::Lifecycle(ValueKind::Resume),
                    2 => Phase::Model {
                        data: data.clone(),
                        mode: Mode::Change,
                    },
                    3 => Phase::Model {
                        data: data.clone(),
                        mode: Mode::Assistant,
                    },
                    4 => Phase::Abort { data: data.clone() },
                    5 => Phase::Compact,
                    _ => Phase::Lifecycle(ValueKind::Shutdown),
                }
            } else if *index < 6 {
                Phase::Kind {
                    data: data.clone(),
                    kind: kind.clone(),
                    index: *index + 1,
                }
            } else {
                Phase::Done {
                    snapshot_required: false,
                }
            }
        }
        Phase::Cwd { data } => {
            let context = match data {
                Some(data) => record.member(data, "context")?,
                None => None,
            };
            let cwd = member_text(record, context.as_ref(), "cwd")?;
            action = Action::Header(Field::Cwd, cwd.map(Value::Native));
            Phase::Entrypoint { data: data.clone() }
        }
        Phase::Entrypoint { data } => {
            action = Action::Header(Field::Entrypoint, Some(Value::CopilotCli));
            Phase::Model {
                data: data.clone(),
                mode: Mode::Start,
            }
        }
        Phase::Model { data, mode } => {
            let mut model = match data {
                Some(data) => record.member(
                    data,
                    match mode {
                        Mode::Start => "selectedModel",
                        Mode::Change => "newModel",
                        Mode::Assistant => "model",
                    },
                )?,
                None => None,
            };
            let mut text = match &model {
                Some(model) => record.text(model)?,
                None => None,
            };
            if text.is_none() && matches!(mode, Mode::Change) {
                model = match data {
                    Some(data) => record.member(data, "model")?,
                    None => None,
                };
                text = match &model {
                    Some(model) => record.text(model)?,
                    None => None,
                };
            }
            if text.is_some() || matches!(mode, Mode::Start) {
                action = Action::Header(Field::LastModel, text.map(Value::Native));
            }
            Phase::ModelTag {
                data: data.clone(),
                model,
                mode: *mode,
            }
        }
        Phase::ModelTag { data, model, mode } => {
            action = Action::Setting(
                SignalKind::Model,
                match model {
                    Some(model) => record.tag(model)?,
                    None => None,
                },
            );
            if matches!(mode, Mode::Start) {
                Phase::Effort { data: data.clone() }
            } else {
                Phase::Done {
                    snapshot_required: false,
                }
            }
        }
        Phase::Effort { data } => {
            let effort = match data {
                Some(data) => record.member(data, "reasoningEffort")?,
                None => None,
            };
            action = Action::Setting(
                SignalKind::Effort,
                match effort {
                    Some(effort) => record.tag(&effort)?,
                    None => None,
                },
            );
            Phase::Lifecycle(ValueKind::Start)
        }
        Phase::Lifecycle(kind) => {
            action = Action::Header(
                Field::CopilotLifecycle,
                Some(match kind {
                    ValueKind::Start => Value::Start,
                    ValueKind::Resume => Value::Resume,
                    ValueKind::Shutdown => Value::Shutdown,
                }),
            );
            Phase::Done {
                snapshot_required: matches!(kind, ValueKind::Shutdown),
            }
        }
        Phase::Abort { data } => {
            let reason = match data {
                Some(data) => record.member(data, "reason")?,
                None => None,
            };
            action = Action::Setting(
                SignalKind::Interrupt,
                match reason {
                    Some(reason) => record.tag(&reason)?,
                    None => None,
                },
            );
            Phase::Done {
                snapshot_required: false,
            }
        }
        Phase::Compact => {
            action = Action::Compact;
            Phase::Done {
                snapshot_required: false,
            }
        }
        Phase::Done { .. } => cursor.phase.clone(),
    };
    Ok(Prepared { next, action })
}
pub(crate) fn apply<N: Clone, T: Clone, S>(
    store: &mut S,
    cursor: &mut Cursor<N>,
    prepared: Prepared<N, T>,
) -> Result<(), S::Error>
where
    S: header::Store<Text = T> + SignalStore,
{
    match prepared.action {
        Action::None => {}
        Action::Header(field, value) => store.set_header(field, value)?,
        Action::Times(timestamp, first) => {
            if first {
                store.set_header(Field::CopilotFirst, Some(Value::Native(timestamp.clone())))?;
            }
            store.set_header(Field::CopilotLast, Some(Value::Native(timestamp)))?;
        }
        Action::Setting(kind, value) => {
            native_producer::setting_tag(store, kind, cursor.offset, cursor.time, value)?
        }
        Action::Compact => native_producer::note(
            store,
            SignalKind::Compact,
            cursor.offset,
            cursor.time,
            "manual".into(),
            None,
        )?,
    };
    cursor.phase = prepared.next;
    Ok(())
}
