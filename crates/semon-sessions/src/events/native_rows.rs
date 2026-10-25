//! Pure native child-row selection shared by physical storage writers.
//!
//! Values borrow the shared parser's index. No connection, SQL, serialization,
//! source reads or operation identity is created here. The caller must apply
//! every family together with its ledger, proofs and receipt in one transaction.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    BillingUsage, CodexUsageEvent, DirtyRows, Event, FileIndex, MessageUsage, ModelTokens, Peer,
    Reply, Signal, Yield,
};
use crate::Tokens;

pub(crate) struct NativeRowPlan<'a> {
    pub(crate) events: RowChanges<'a, Event>,
    pub(crate) extras: RowChanges<'a, Event>,
    pub(crate) signals: RowChanges<'a, Signal>,
    pub(crate) codex_usage: RowChanges<'a, CodexUsageEvent>,
}

impl<'a> NativeRowPlan<'a> {
    pub(crate) fn new(index: &'a FileIndex, changes: &'a DirtyRows) -> Self {
        Self {
            events: RowChanges::new(changes.before_events, &changes.events, &index.events),
            extras: RowChanges::new(changes.before_extras, &changes.extras, &index.extras),
            signals: RowChanges::new(changes.before_signals, &changes.signals, &index.signals),
            codex_usage: RowChanges::new(
                changes.before_codex_usage,
                &changes.codex_usage,
                &index.codex_usage_events,
            ),
        }
    }
}

/// Delete identities first, then replace this message's usage and identities,
/// or delete the usage when absent. The owner transaction determines the SQL.
pub(crate) struct UsageChanges<'a> {
    touched: &'a BTreeSet<String>,
    rows: &'a BTreeMap<String, MessageUsage>,
}

impl<'a> UsageChanges<'a> {
    pub(crate) fn new(changes: &'a DirtyRows, rows: &'a BTreeMap<String, MessageUsage>) -> Self {
        Self {
            touched: &changes.usage,
            rows,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&'a String, Option<&'a MessageUsage>)> + '_ {
        self.touched.iter().map(|id| (id, self.rows.get(id)))
    }
}

pub(crate) struct RowChanges<'a, T> {
    before: usize,
    touched: &'a BTreeSet<usize>,
    rows: &'a [T],
}

impl<'a, T> RowChanges<'a, T> {
    fn new(before: usize, touched: &'a BTreeSet<usize>, rows: &'a [T]) -> Self {
        Self {
            before,
            touched,
            rows,
        }
    }

    /// Original ascending, deduplicated write order: changed prefix, then tail.
    /// An out-of-range dirty ordinal does not manufacture a row.
    pub(crate) fn upserts(&self) -> impl Iterator<Item = (usize, &'a T)> + '_ {
        self.touched
            .range(..self.before)
            .copied()
            .chain(self.before..self.rows.len())
            .filter_map(|seq| self.rows.get(seq).map(|row| (seq, row)))
    }

    /// The append parser never cuts rows; synthetic truncation deltas can.
    pub(crate) fn delete_from(&self) -> Option<usize> {
        (self.rows.len() < self.before).then_some(self.rows.len())
    }

    pub(crate) fn apply<E>(
        &self,
        mut put: impl FnMut(usize, &'a T) -> Result<(), E>,
        cut: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<(), E> {
        for (seq, row) in self.upserts() {
            put(seq, row)?;
        }
        if let Some(from) = self.delete_from() {
            cut(from)?;
        }
        Ok(())
    }
}

/// Event-local parser metadata kept in the existing `events.y` JSON slot.
/// The parser version invalidates older rows when this envelope is introduced.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct EventState {
    pub(crate) version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) yielded: Option<Box<Yield>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) poll_target: Option<String>,
    #[serde(default)]
    pub(crate) result_source: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub(crate) native_direct_call: bool,
}

/// Backend-neutral event values in the current persisted column contract.
/// Unsigned native offsets/IDs retain their bits in signed database integers.
pub(crate) struct EventRow<'a> {
    pub(crate) k: String,
    pub(crate) o: i64,
    pub(crate) b: u32,
    pub(crate) t: Option<i64>,
    pub(crate) id: Option<&'a str>,
    pub(crate) n: Option<&'a str>,
    pub(crate) code_mode: bool,
    pub(crate) code_mode_open: bool,
    pub(crate) parent: Option<i64>,
    pub(crate) script: u8,
    pub(crate) y: Option<String>,
    pub(crate) poll: Option<i64>,
    pub(crate) reply: bool,
    pub(crate) reply_o: Option<i64>,
    pub(crate) reply_b: Option<u32>,
    pub(crate) reply_t: Option<i64>,
    pub(crate) reply_e: Option<bool>,
    pub(crate) reply_f: Option<u8>,
    pub(crate) reply_m: Option<&'a str>,
    pub(crate) item: Option<i64>,
    pub(crate) has_peer: bool,
    pub(crate) peer_sock: Option<&'a str>,
    pub(crate) peer_pid: Option<i64>,
    pub(crate) peer_start: Option<&'a str>,
    pub(crate) u: Option<&'a str>,
}

impl<'a> EventRow<'a> {
    pub(crate) fn encode(event: &'a Event) -> serde_json::Result<Self> {
        let Event {
            k,
            o,
            b,
            t,
            id,
            n,
            code_mode,
            native_direct_call,
            code_mode_open,
            parent,
            script,
            poll_target,
            result_source,
            y,
            poll,
            r,
            item,
            peer,
            u,
        } = event;
        let y = if y.is_some() || poll_target.is_some() || *result_source || *native_direct_call {
            Some(serde_json::to_string(&EventState {
                version: 1,
                yielded: y.as_ref().map(|yielded| Box::new((**yielded).clone())),
                poll_target: poll_target.as_ref().cloned(),
                result_source: *result_source,
                native_direct_call: *native_direct_call,
            })?)
        } else {
            None
        };
        let (reply, reply_o, reply_b, reply_t, reply_e, reply_f, reply_m) = match r {
            Some(Reply {
                o: reply_o,
                b: reply_b,
                t: reply_t,
                e: reply_e,
                f: reply_f,
                m: reply_m,
            }) => (
                true,
                Some(*reply_o as i64),
                Some(*reply_b),
                *reply_t,
                Some(*reply_e),
                Some(*reply_f),
                reply_m.as_deref(),
            ),
            None => (false, None, None, None, None, None, None),
        };
        let (has_peer, peer_sock, peer_pid, peer_start) = match peer {
            Some(Peer { sock, pid, start }) => (
                true,
                sock.as_deref(),
                pid.map(|value| value as i64),
                start.as_deref(),
            ),
            None => (false, None, None, None),
        };
        Ok(Self {
            k: kind_name(k)?,
            o: *o as i64,
            b: *b,
            t: *t,
            id: id.as_deref(),
            n: n.as_deref(),
            code_mode: *code_mode,
            code_mode_open: *code_mode_open,
            parent: parent.map(|value| value as i64),
            script: *script,
            y,
            poll: poll.map(|value| value as i64),
            reply,
            reply_o,
            reply_b,
            reply_t,
            reply_e,
            reply_f,
            reply_m,
            item: item.map(|value| value as i64),
            has_peer,
            peer_sock,
            peer_pid,
            peer_start,
            u: u.as_deref(),
        })
    }
}

fn kind_name(value: &super::Kind) -> serde_json::Result<String> {
    use serde::ser::Error;
    match serde_json::to_value(value)? {
        serde_json::Value::String(text) => Ok(text),
        _ => Err(serde_json::Error::custom("not a unit variant")),
    }
}

/// Current copied-usage columns; identities remain separate typed rows.
pub(crate) struct UsageRow<'a> {
    pub(crate) model: Option<&'a str>,
    pub(crate) input: i64,
    pub(crate) cached_input: i64,
    pub(crate) output: i64,
    pub(crate) reasoning_output: i64,
    pub(crate) total: i64,
    pub(crate) model_input: i64,
    pub(crate) model_output: i64,
    pub(crate) model_cache_write: i64,
    pub(crate) model_cache_read: i64,
    pub(crate) billing_input: i64,
    pub(crate) billing_output: i64,
    pub(crate) billing_cache_read: i64,
    pub(crate) cache_write_5m: i64,
    pub(crate) cache_write_1h: i64,
    pub(crate) web_search_requests: i64,
    pub(crate) speed: Option<&'a str>,
    pub(crate) service_tier: Option<&'a str>,
    pub(crate) prompt_size: i64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) split_unknown: bool,
    pub(crate) record_ids: String,
}

impl<'a> UsageRow<'a> {
    pub(crate) fn encode(usage: &'a MessageUsage) -> serde_json::Result<Self> {
        let MessageUsage {
            record_ids,
            model,
            tokens:
                Tokens {
                    input,
                    cached_input,
                    output,
                    reasoning_output,
                    total,
                },
            model_tokens:
                ModelTokens {
                    input: model_input,
                    output: model_output,
                    cache_write: model_cache_write,
                    cache_read: model_cache_read,
                },
            billing:
                BillingUsage {
                    input: billing_input,
                    output: billing_output,
                    cache_read: billing_cache_read,
                    cache_write_5m,
                    cache_write_1h,
                    web_search_requests,
                    speed,
                    service_tier,
                    prompt_size,
                    timestamp,
                    split_unknown,
                },
        } = usage;
        Ok(Self {
            model: model.as_deref(),
            input: *input as i64,
            cached_input: *cached_input as i64,
            output: *output as i64,
            reasoning_output: *reasoning_output as i64,
            total: *total as i64,
            model_input: *model_input as i64,
            model_output: *model_output as i64,
            model_cache_write: *model_cache_write as i64,
            model_cache_read: *model_cache_read as i64,
            billing_input: *billing_input as i64,
            billing_output: *billing_output as i64,
            billing_cache_read: *billing_cache_read as i64,
            cache_write_5m: *cache_write_5m as i64,
            cache_write_1h: *cache_write_1h as i64,
            web_search_requests: *web_search_requests as i64,
            speed: speed.as_deref(),
            service_tier: service_tier.as_deref(),
            prompt_size: *prompt_size as i64,
            timestamp: *timestamp,
            split_unknown: *split_unknown,
            record_ids: serde_json::to_string(record_ids)?,
        })
    }
}
