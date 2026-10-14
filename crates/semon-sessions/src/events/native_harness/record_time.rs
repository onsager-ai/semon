//! Native record_time precedence. An invalid top-level string does not fall
//! through to payload.timestamp; a missing/non-string field does.
use super::record::RecordSource;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Cursor<N> {
    Top,
    Payload,
    Parse(Option<N>),
    Done(Option<i64>),
}
impl<N> Default for Cursor<N> {
    fn default() -> Self {
        Self::Top
    }
}
impl<N> Cursor<N> {
    pub(crate) fn result(&self) -> Option<Option<i64>> {
        match self {
            Self::Done(time) => Some(*time),
            _ => None,
        }
    }
}
pub(crate) fn step<R: RecordSource>(
    record: &mut R,
    cursor: &Cursor<R::Node>,
) -> Result<Cursor<R::Node>, R::Error> {
    Ok(match cursor {
        Cursor::Top => {
            let root = record.root();
            match record.member(&root, "timestamp")? {
                Some(node) if record.string_present(&node)? => Cursor::Parse(Some(node)),
                _ => Cursor::Payload,
            }
        }
        Cursor::Payload => {
            let root = record.root();
            let payload = record.member(&root, "payload")?;
            Cursor::Parse(match payload {
                Some(payload) => record.member(&payload, "timestamp")?,
                None => None,
            })
        }
        Cursor::Parse(node) => Cursor::Done(match node {
            Some(node) => record.timestamp(node)?,
            None => None,
        }),
        Cursor::Done(time) => Cursor::Done(*time),
    })
}
