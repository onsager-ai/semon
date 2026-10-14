//! Fixed native header properties retain qualified text references in Original
//! metadata. Complete FileIndex continues owning its native Strings/Index.
use super::{CopilotIdentityStore, FileIndex};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Field {
    CopilotVersion,
    CopilotSession,
    Cwd,
    Branch,
    LastModel,
    Title,
    Entrypoint,
    CopilotLifecycle,
    CopilotFirst,
    CopilotLast,
    First,
    Last,
}
impl Field {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::CopilotVersion => "copilot_version",
            Self::CopilotSession => "copilot_session",
            Self::Cwd => "cwd",
            Self::Branch => "branch",
            Self::LastModel => "last_model",
            Self::Title => "title",
            Self::Entrypoint => "entrypoint",
            Self::CopilotLifecycle => "copilot_lifecycle",
            Self::CopilotFirst => "copilot_first",
            Self::CopilotLast => "copilot_last",
            Self::First => "first",
            Self::Last => "last",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Value<T> {
    Native(T),
    Empty,
    CopilotCli,
    Start,
    Resume,
    Shutdown,
    Time(i64),
}
pub(crate) trait Store: CopilotIdentityStore {
    fn header(&self, field: Field) -> Result<Option<Value<Self::Text>>, Self::Error>;
    fn set_header(
        &mut self,
        field: Field,
        value: Option<Value<Self::Text>>,
    ) -> Result<(), Self::Error>;
    fn initialize_copilot(
        &mut self,
        version: Option<Self::Text>,
        session: Option<Self::Text>,
    ) -> Result<(), Self::Error>;
}
impl Store for FileIndex {
    fn header(&self, field: Field) -> Result<Option<Value<String>>, Infallible> {
        let text = match field {
            Field::CopilotVersion => self.copilot.as_ref().map(|c| c.version.clone()),
            Field::CopilotSession => self.copilot.as_ref().map(|c| c.session_id.clone()),
            Field::Cwd => self.cwd.clone(),
            Field::Branch => self.branch.clone(),
            Field::LastModel => self.last_model.clone(),
            Field::Title => self.title.clone(),
            Field::Entrypoint => self.entrypoint.clone(),
            Field::CopilotLifecycle => self.copilot.as_ref().and_then(|c| c.lifecycle.clone()),
            Field::CopilotFirst => self.copilot.as_ref().and_then(|c| c.first.clone()),
            Field::CopilotLast => self.copilot.as_ref().and_then(|c| c.last.clone()),
            Field::First => return Ok(self.first.map(Value::Time)),
            Field::Last => return Ok(self.last.map(Value::Time)),
        };
        Ok(text.map(Value::Native))
    }
    fn set_header(&mut self, field: Field, value: Option<Value<String>>) -> Result<(), Infallible> {
        if matches!(field, Field::First | Field::Last) {
            let value = value.map(|v| match v {
                Value::Time(t) => t,
                _ => unreachable!("native activity header is scalar"),
            });
            if field == Field::First {
                self.first = value;
            } else {
                self.last = value;
            }
            return Ok(());
        }
        let text = value.map(|v| match v {
            Value::Native(s) => s,
            Value::Empty => String::new(),
            Value::CopilotCli => "copilot-cli".into(),
            Value::Start => "session.start".into(),
            Value::Resume => "session.resume".into(),
            Value::Shutdown => "session.shutdown".into(),
            Value::Time(_) => unreachable!("native text header is qualified text"),
        });
        match field {
            Field::CopilotVersion => {
                self.copilot.as_mut().expect("initialized").version = text.unwrap_or_default()
            }
            Field::CopilotSession => {
                self.copilot.as_mut().expect("initialized").session_id = text.unwrap_or_default()
            }
            Field::Cwd => self.cwd = text,
            Field::Branch => self.branch = text,
            Field::LastModel => self.last_model = text,
            Field::Title => self.title = text,
            Field::Entrypoint => self.entrypoint = text,
            Field::CopilotLifecycle => self.copilot.as_mut().expect("initialized").lifecycle = text,
            Field::CopilotFirst => self.copilot.as_mut().expect("initialized").first = text,
            Field::CopilotLast => self.copilot.as_mut().expect("initialized").last = text,
            Field::First | Field::Last => unreachable!("scalar handled above"),
        };
        Ok(())
    }
    fn initialize_copilot(
        &mut self,
        version: Option<String>,
        session: Option<String>,
    ) -> Result<(), Infallible> {
        self.copilot = Some(crate::copilot::Index {
            version: version.unwrap_or_default(),
            session_id: session.unwrap_or_default(),
            ..Default::default()
        });
        Ok(())
    }
}
