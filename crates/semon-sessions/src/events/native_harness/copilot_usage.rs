//! One native Copilot modelMetrics report. Object key ownership/order belongs
//! to its JSON owner; complete and Original reports share these scalar phases.
use super::record::RecordSource;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Row {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) read: u64,
    pub(crate) write: u64,
    pub(crate) reasoning: u64,
}
impl Row {
    pub(crate) fn model_tokens(self) -> crate::events::ModelTokens {
        crate::events::ModelTokens {
            input: self.input,
            output: self.output,
            cache_read: self.read,
            cache_write: self.write,
        }
    }
    /// Keep the native cumulative additions and overflow behavior, including
    /// input + output before adding it to total. Never clamp or skip a model.
    pub(crate) fn add_tokens(self, tokens: &mut crate::Tokens) {
        tokens.input += self.input;
        tokens.output += self.output;
        tokens.cached_input += self.read;
        tokens.reasoning_output += self.reasoning;
        tokens.total += self.input + self.output;
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Cursor<N> {
    Usage,
    Fields {
        usage: N,
        index: u8,
        values: [u64; 5],
    },
    Done(Option<Row>),
}
impl<N> Default for Cursor<N> {
    fn default() -> Self {
        Self::Usage
    }
}
impl<N> Cursor<N> {
    pub(crate) fn result(&self) -> Option<Option<Row>> {
        match self {
            Self::Done(row) => Some(*row),
            _ => None,
        }
    }
    pub(crate) fn valid(&self) -> bool {
        !matches!(self,Self::Fields{index,..} if *index>4)
    }
}
pub(crate) fn step<R: RecordSource>(
    record: &mut R,
    cursor: &Cursor<R::Node>,
) -> Result<Cursor<R::Node>, R::Error> {
    Ok(match cursor {
        Cursor::Usage => {
            let root = record.root();
            match record.member(&root, "usage")? {
                Some(usage) => Cursor::Fields {
                    usage,
                    index: 0,
                    values: [0; 5],
                },
                None => Cursor::Done(None),
            }
        }
        Cursor::Fields {
            usage,
            index,
            values,
        } => {
            let fields = [
                "inputTokens",
                "outputTokens",
                "cacheReadTokens",
                "cacheWriteTokens",
                "reasoningTokens",
            ];
            let number = match record.member(usage, fields[usize::from(*index)])? {
                Some(value) => record.unsigned(&value)?,
                None => None,
            };
            match number {
                None => Cursor::Done(None),
                Some(number) => {
                    let mut values = *values;
                    values[usize::from(*index)] = number;
                    if *index < 4 {
                        Cursor::Fields {
                            usage: usage.clone(),
                            index: *index + 1,
                            values,
                        }
                    } else {
                        Cursor::Done(Some(Row {
                            input: values[0],
                            output: values[1],
                            read: values[2],
                            write: values[3],
                            reasoning: values[4],
                        }))
                    }
                }
            }
        }
        Cursor::Done(row) => Cursor::Done(*row),
    })
}
