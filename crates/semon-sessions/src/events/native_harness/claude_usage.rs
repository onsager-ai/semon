//! Claude's native message-id replacement and record-UUID union. Property
//! phases are shared by complete Value parsing and retained Original repair.
use super::record::{RecordSource, member_text};
use crate::Tokens;
use crate::events::{BillingUsage, FileIndex, MessageUsage, ModelTokens};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Billing<T> {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write_5m: u64,
    pub(crate) cache_write_1h: u64,
    pub(crate) web_search_requests: u64,
    pub(crate) speed: Option<T>,
    pub(crate) service_tier: Option<T>,
    pub(crate) prompt_size: u64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) split_unknown: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Observation<T> {
    pub(crate) identity: T,
    pub(crate) uuid: Option<T>,
    pub(crate) model: Option<T>,
    pub(crate) tokens: Tokens,
    pub(crate) model_tokens: ModelTokens,
    pub(crate) billing: Billing<T>,
}
pub(crate) trait Store {
    type Text;
    type Error;
    fn claude_report(&mut self, observation: Observation<Self::Text>) -> Result<(), Self::Error>;
}
impl Store for FileIndex {
    type Text = String;
    type Error = Infallible;
    fn claude_report(&mut self, report: Observation<String>) -> Result<(), Infallible> {
        if let Some(dirty) = &mut self.dirty_rows {
            dirty.usage.insert(report.identity.clone());
        }
        let mut record_ids = self
            .usage_by_id
            .get(&report.identity)
            .map(|usage| usage.record_ids.clone())
            .unwrap_or_default();
        record_ids.extend(report.uuid);
        let billing = report.billing;
        self.usage_by_id.insert(
            report.identity,
            MessageUsage {
                record_ids,
                model: report.model,
                tokens: report.tokens,
                model_tokens: report.model_tokens,
                billing: BillingUsage {
                    input: billing.input,
                    output: billing.output,
                    cache_read: billing.cache_read,
                    cache_write_5m: billing.cache_write_5m,
                    cache_write_1h: billing.cache_write_1h,
                    web_search_requests: billing.web_search_requests,
                    speed: billing.speed,
                    service_tier: billing.service_tier,
                    prompt_size: billing.prompt_size,
                    timestamp: billing.timestamp,
                    split_unknown: billing.split_unknown,
                },
            },
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Draft<N, T> {
    message: N,
    usage: N,
    identity: T,
    values: [Option<u64>; 7],
    model: Option<T>,
    speed: Option<T>,
    service: Option<T>,
    uuid: Option<T>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Cursor<N, T> {
    Message,
    Identity {
        message: N,
    },
    Usage {
        message: N,
        identity: T,
    },
    Numbers {
        draft: Draft<N, T>,
        index: u8,
    },
    Model(Draft<N, T>),
    Cache(Draft<N, T>),
    Five {
        draft: Draft<N, T>,
        creation: Option<N>,
    },
    One {
        draft: Draft<N, T>,
        creation: Option<N>,
    },
    Web(Draft<N, T>),
    Speed(Draft<N, T>),
    Service(Draft<N, T>),
    Assistant(Draft<N, T>),
    Uuid(Draft<N, T>),
    Done(Option<Observation<T>>),
}
impl<N, T> Default for Cursor<N, T> {
    fn default() -> Self {
        Self::Message
    }
}
impl<N, T> Cursor<N, T> {
    pub(crate) fn result(&self) -> Option<Option<&Observation<T>>> {
        match self {
            Self::Done(report) => Some(report.as_ref()),
            _ => None,
        }
    }
    pub(crate) fn valid(&self) -> bool {
        !matches!(self,Self::Numbers{index,..} if *index>3)
    }
}
fn report<N, T>(draft: Draft<N, T>, time: Option<i64>) -> Observation<T> {
    let [input, write, read, output, five, one, web] = draft.values;
    let (input, write, read, output) = (
        input.unwrap_or(0),
        write.unwrap_or(0),
        read.unwrap_or(0),
        output.unwrap_or(0),
    );
    let split = five
        .zip(one)
        .filter(|(five, one)| five.saturating_add(*one) == write);
    let (five, one, unknown) = split
        .map(|(five, one)| (five, one, false))
        .unwrap_or((write, 0, true));
    Observation {
        identity: draft.identity,
        uuid: draft.uuid,
        model: draft.model,
        tokens: Tokens {
            input: input + write + read,
            cached_input: read,
            output,
            reasoning_output: 0,
            total: input + write + read + output,
        },
        model_tokens: ModelTokens {
            input,
            output,
            cache_write: write,
            cache_read: read,
        },
        billing: Billing {
            input,
            output,
            cache_read: read,
            cache_write_5m: five,
            cache_write_1h: one,
            web_search_requests: web.unwrap_or(0),
            speed: draft.speed,
            service_tier: draft.service,
            prompt_size: input
                .saturating_add(read)
                .saturating_add(five)
                .saturating_add(one),
            timestamp: time,
            split_unknown: unknown,
        },
    }
}
pub(crate) fn step<R: RecordSource>(
    record: &mut R,
    cursor: &Cursor<R::Node, R::Text>,
    time: Option<i64>,
) -> Result<Cursor<R::Node, R::Text>, R::Error> {
    Ok(match cursor {
        Cursor::Message => {
            let root = record.root();
            match record.member(&root, "message")? {
                Some(message) => Cursor::Identity { message },
                None => Cursor::Done(None),
            }
        }
        Cursor::Identity { message } => match member_text(record, Some(message), "id")? {
            Some(identity) => Cursor::Usage {
                message: message.clone(),
                identity,
            },
            None => Cursor::Done(None),
        },
        Cursor::Usage { message, identity } => match record.member(message, "usage")? {
            Some(usage) => Cursor::Numbers {
                draft: Draft {
                    message: message.clone(),
                    usage,
                    identity: identity.clone(),
                    values: [None; 7],
                    model: None,
                    speed: None,
                    service: None,
                    uuid: None,
                },
                index: 0,
            },
            None => Cursor::Done(None),
        },
        Cursor::Numbers { draft, index } => {
            let mut draft = draft.clone();
            let key = [
                "input_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
                "output_tokens",
            ][usize::from(*index)];
            draft.values[usize::from(*index)] = match record.member(&draft.usage, key)? {
                Some(number) => record.unsigned(&number)?,
                None => None,
            };
            if *index < 3 {
                Cursor::Numbers {
                    draft,
                    index: *index + 1,
                }
            } else {
                Cursor::Model(draft)
            }
        }
        Cursor::Model(draft) => {
            let mut draft = draft.clone();
            draft.model = member_text(record, Some(&draft.message), "model")?;
            Cursor::Cache(draft)
        }
        Cursor::Cache(draft) => Cursor::Five {
            draft: draft.clone(),
            creation: record.member(&draft.usage, "cache_creation")?,
        },
        Cursor::Five { draft, creation } | Cursor::One { draft, creation } => {
            let five = matches!(cursor, Cursor::Five { .. });
            let mut draft = draft.clone();
            let value = match creation {
                Some(creation) => record.member(
                    creation,
                    if five {
                        "ephemeral_5m_input_tokens"
                    } else {
                        "ephemeral_1h_input_tokens"
                    },
                )?,
                None => None,
            };
            draft.values[if five { 4 } else { 5 }] = match value {
                Some(value) => record.unsigned(&value)?,
                None => None,
            };
            if five {
                Cursor::One {
                    draft,
                    creation: creation.clone(),
                }
            } else {
                Cursor::Web(draft)
            }
        }
        Cursor::Web(draft) => {
            let mut draft = draft.clone();
            let tools = record.member(&draft.usage, "server_tool_use")?;
            let web = match tools {
                Some(tools) => record.member(&tools, "web_search_requests")?,
                None => None,
            };
            draft.values[6] = match web {
                Some(web) => record.unsigned(&web)?,
                None => None,
            };
            Cursor::Speed(draft)
        }
        Cursor::Speed(draft) => {
            let mut draft = draft.clone();
            draft.speed = member_text(record, Some(&draft.usage), "speed")?;
            Cursor::Service(draft)
        }
        Cursor::Service(draft) => {
            let mut draft = draft.clone();
            draft.service = member_text(record, Some(&draft.usage), "service_tier")?;
            Cursor::Assistant(draft)
        }
        Cursor::Assistant(draft) => {
            let root = record.root();
            let kind = record.member(&root, "type")?;
            if match kind {
                Some(kind) => record.literal(&kind, "assistant")?,
                None => false,
            } {
                Cursor::Uuid(draft.clone())
            } else {
                Cursor::Done(Some(report(draft.clone(), time)))
            }
        }
        Cursor::Uuid(draft) => {
            let root = record.root();
            let mut draft = draft.clone();
            draft.uuid = member_text(record, Some(&root), "uuid")?;
            Cursor::Done(Some(report(draft, time)))
        }
        Cursor::Done(result) => Cursor::Done(result.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    // Frozen native producer calculation. This does not call the phased
    // parser or its report constructor, so a dropped field cannot self-pass.
    fn reference(record: &Value, time: Option<i64>) -> Option<MessageUsage> {
        let message = record.get("message")?;
        message.get("id")?.as_str()?;
        let usage = message.get("usage")?;
        let number = |key| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let (input, write, read, output) = (
            number("input_tokens"),
            number("cache_creation_input_tokens"),
            number("cache_read_input_tokens"),
            number("output_tokens"),
        );
        let split = usage
            .get("cache_creation")
            .and_then(Value::as_object)
            .and_then(|creation| {
                let five = creation.get("ephemeral_5m_input_tokens")?.as_u64()?;
                let one = creation.get("ephemeral_1h_input_tokens")?.as_u64()?;
                (five.saturating_add(one) == write).then_some((five, one))
            });
        let (five, one, unknown) = split
            .map(|(five, one)| (five, one, false))
            .unwrap_or((write, 0, true));
        Some(MessageUsage {
            record_ids: if record.get("type").and_then(Value::as_str) == Some("assistant") {
                record
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .into_iter()
                    .collect()
            } else {
                Default::default()
            },
            model: message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            tokens: Tokens {
                input: input + write + read,
                cached_input: read,
                output,
                reasoning_output: 0,
                total: input + write + read + output,
            },
            model_tokens: ModelTokens {
                input,
                output,
                cache_write: write,
                cache_read: read,
            },
            billing: BillingUsage {
                input,
                output,
                cache_read: read,
                cache_write_5m: five,
                cache_write_1h: one,
                web_search_requests: usage
                    .get("server_tool_use")
                    .and_then(|tools| tools.get("web_search_requests"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                speed: usage
                    .get("speed")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                service_tier: usage
                    .get("service_tier")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                prompt_size: input
                    .saturating_add(read)
                    .saturating_add(five)
                    .saturating_add(one),
                timestamp: time,
                split_unknown: unknown,
            },
        })
    }
    #[test]
    fn phased_reports_equal_independent_native_numbers_and_replace_without_losing_uuids() {
        for usage in [
            json!(null),
            json!(false),
            json!([]),
            json!({}),
            json!({"input_tokens":1.0,"output_tokens":-1,"cache_creation_input_tokens":3,"cache_read_input_tokens":2,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2},"server_tool_use":{"web_search_requests":7},"speed":"fast","service_tier":"tier"}),
            json!({"cache_creation_input_tokens":3,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":1}}),
            json!({"cache_creation_input_tokens":u64::MAX,"cache_creation":{"ephemeral_5m_input_tokens":u64::MAX,"ephemeral_1h_input_tokens":1}}),
        ] {
            let mut complete = FileIndex::default();
            let mut expected: Option<MessageUsage> = None;
            for (index, role) in ["assistant", "assistant", "user"].into_iter().enumerate() {
                let record = json!({"type":role,"uuid":format!("uuid-{index}"),"message":{"id":"","model":"model","usage":usage}});
                let mut cursor = Cursor::default();
                let mut source = super::super::record::ValueRecord { record: &record };
                let time = Some(index as i64);
                let mut phases = 0;
                while cursor.result().is_none() {
                    cursor =
                        crate::events::native_producer::complete(step(&mut source, &cursor, time));
                    phases += 1;
                    assert!(phases < 32);
                }
                complete
                    .claude_report(cursor.result().unwrap().unwrap().clone())
                    .unwrap();
                let mut next = reference(&record, time).unwrap();
                if let Some(previous) = expected {
                    next.record_ids.extend(previous.record_ids);
                }
                expected = Some(next);
                assert_eq!(complete.usage_by_id.get(""), expected.as_ref());
            }
        }
    }
}
