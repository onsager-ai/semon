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
    fn initialize_copilot(
        &mut self,
        version: Option<Self::Text>,
        session: Option<Self::Text>,
    ) -> Result<(), Self::Error>;
}
impl Store for FileIndex {
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
