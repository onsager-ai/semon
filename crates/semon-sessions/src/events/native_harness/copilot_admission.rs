//! Complete and Original admission drive the same property/action phases.
//! No timestamp, activity or body effect precedes the native id/start/seen gate.
use super::record::{RecordSource, member_text};
use super::{Admission, CopilotIdentityStore, copilot_admit, header};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Phase<N, T> {
    Identity,
    Start {
        identity: Option<T>,
    },
    Version {
        identity: T,
        data: Option<N>,
    },
    Session {
        identity: T,
        data: Option<N>,
        version: Option<T>,
    },
    Apply {
        identity: Option<T>,
        is_start: bool,
        version: Option<T>,
        session: Option<T>,
    },
    Done {
        admission: Admission,
        identity: Option<T>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Cursor<N, T> {
    pub(crate) phase: Phase<N, T>,
}
impl<N, T> Default for Cursor<N, T> {
    fn default() -> Self {
        Self {
            phase: Phase::Identity,
        }
    }
}
impl<N, T> Cursor<N, T> {
    pub(crate) fn result(&self) -> Option<(Admission, Option<&T>)> {
        match &self.phase {
            Phase::Done {
                admission,
                identity,
            } => Some((*admission, identity.as_ref())),
            _ => None,
        }
    }
}
pub(crate) struct Prepared<N, T> {
    next: Phase<N, T>,
    apply: bool,
}
pub(crate) fn prepare<R, S>(
    record: &mut R,
    store: &S,
    cursor: &Cursor<R::Node, R::Text>,
) -> Result<Prepared<R::Node, R::Text>, R::Error>
where
    R: RecordSource,
    S: CopilotIdentityStore<Text = R::Text>,
    R::Error: From<S::Error>,
{
    let mut apply = false;
    let next = match &cursor.phase {
        Phase::Identity => {
            let root = record.root();
            let identity = member_text(record, Some(&root), "id")?;
            let identity = match identity {
                Some(id) if record.text_nonempty(&id)? => Some(id),
                _ => None,
            };
            Phase::Start { identity }
        }
        Phase::Start { identity: None } => {
            apply = true;
            Phase::Apply {
                identity: None,
                is_start: false,
                version: None,
                session: None,
            }
        }
        Phase::Start {
            identity: Some(identity),
        } => {
            if store.copilot_started().map_err(R::Error::from)? {
                apply = true;
                Phase::Apply {
                    identity: Some(identity.clone()),
                    is_start: false,
                    version: None,
                    session: None,
                }
            } else {
                let root = record.root();
                let kind = record.member(&root, "type")?;
                let start = match kind {
                    Some(kind) => record.literal(&kind, "session.start")?,
                    None => false,
                };
                if start {
                    Phase::Version {
                        identity: identity.clone(),
                        data: record.member(&root, "data")?,
                    }
                } else {
                    apply = true;
                    Phase::Apply {
                        identity: Some(identity.clone()),
                        is_start: false,
                        version: None,
                        session: None,
                    }
                }
            }
        }
        Phase::Version { identity, data } => Phase::Session {
            identity: identity.clone(),
            data: data.clone(),
            version: member_text(record, data.as_ref(), "copilotVersion")?,
        },
        Phase::Session {
            identity,
            data,
            version,
        } => {
            let session = member_text(record, data.as_ref(), "sessionId")?;
            apply = true;
            Phase::Apply {
                identity: Some(identity.clone()),
                is_start: true,
                version: version.clone(),
                session,
            }
        }
        Phase::Apply { .. } => {
            apply = true;
            cursor.phase.clone()
        }
        Phase::Done { .. } => cursor.phase.clone(),
    };
    Ok(Prepared { next, apply })
}
pub(crate) fn apply<N: Clone, T: Clone, S: header::Store<Text = T>>(
    store: &mut S,
    cursor: &mut Cursor<N, T>,
    prepared: Prepared<N, T>,
) -> Result<(), S::Error> {
    let mut next = prepared.next;
    if prepared.apply {
        let Phase::Apply {
            identity,
            is_start,
            version,
            session,
        } = next
        else {
            unreachable!("prepared native admission")
        };
        let admission = copilot_admit(store, identity.as_ref(), is_start, |store| {
            store.initialize_copilot(version, session)
        })?;
        next = Phase::Done {
            admission,
            identity,
        };
    }
    cursor.phase = next;
    Ok(())
}
