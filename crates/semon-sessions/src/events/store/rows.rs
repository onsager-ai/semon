use rusqlite::{Row, Statement, params, types::Type};
use serde::{Deserialize, Serialize};

use super::{BillingUsage, Event, MessageUsage, ModelTokens, Peer, Reply, Yield};
use super::{from_json, int, json, seq_int, seq_of, tag, uint, untag};
use crate::Tokens;

/// Event-local parser metadata kept in the existing `events.y` JSON slot.
/// The parser version invalidates older rows when this envelope is introduced.
#[derive(Debug, Deserialize, Serialize)]
struct EventState {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    yielded: Option<Box<Yield>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poll_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<Result<String, ()>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resumed_agent: Option<String>,
    #[serde(default)]
    result_source: bool,
    #[serde(default, skip_serializing_if = "crate::events::is_false")]
    native_direct_call: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activity_observation: Option<crate::events::session_activity::classify::Observation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activity_tool: Option<crate::events::session_activity::classify::ToolCallEvidence>,
}

fn event_state(column: usize, text: &str) -> rusqlite::Result<EventState> {
    let state: EventState = from_json(column, text)?;
    if state.version != 1 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported event-state version",
            )),
        ));
    }
    Ok(state)
}

/// One `events` row, selected as [`EVENT_COLUMNS`].
pub(super) fn read_event(row: &Row<'_>) -> rusqlite::Result<(bool, usize, Event)> {
    let state = row
        .get::<_, Option<String>>(12)?
        .map(|text| event_state(12, &text))
        .transpose()?;
    let (
        y,
        poll_target,
        task_id,
        resumed_agent,
        result_source,
        native_direct_call,
        activity_observation,
        activity_tool,
    ) = match state {
        Some(state) => (
            state.yielded,
            state.poll_target,
            state.task_id,
            state.resumed_agent,
            state.result_source,
            state.native_direct_call,
            state.activity_observation,
            state.activity_tool,
        ),
        None => (None, None, None, None, false, false, None, None),
    };
    let r = if row.get::<_, bool>(14)? {
        Some(Reply {
            o: uint(row.get(15)?),
            b: row.get(16)?,
            t: row.get(17)?,
            e: row.get(18)?,
            f: row.get(19)?,
            m: row.get(20)?,
        })
    } else {
        None
    };
    let peer = if row.get::<_, bool>(22)? {
        Some(Peer {
            sock: row.get(23)?,
            pid: row.get::<_, Option<i64>>(24)?.map(uint),
            start: row.get(25)?,
        })
    } else {
        None
    };
    let event = Event {
        k: untag(2, row.get(2)?)?,
        o: uint(row.get(3)?),
        b: row.get(4)?,
        t: row.get(5)?,
        id: row.get(6)?,
        n: row.get(7)?,
        code_mode: row.get(8)?,
        native_direct_call,
        code_mode_open: row.get(9)?,
        parent: row.get::<_, Option<i64>>(10)?.map(seq_of),
        script: row.get(11)?,
        poll_target,
        task_id,
        resumed_agent,
        result_source,
        y,
        poll: row.get::<_, Option<i64>>(13)?.map(seq_of),
        r,
        item: row.get::<_, Option<i64>>(21)?.map(uint),
        peer,
        u: row.get(26)?,
        activity_observation,
        activity_tool,
    };
    Ok((row.get(0)?, seq_of(row.get(1)?), event))
}

/// One `usage` row, selected as [`USAGE_COLUMNS`].
pub(super) fn read_usage(row: &Row<'_>) -> rusqlite::Result<(String, MessageUsage)> {
    let usage = MessageUsage {
        offset: uint(row.get(23)?),
        record_ordinal: row.get::<_, Option<i64>>(24)?.map(uint),
        record_ids: from_json(22, &row.get::<_, String>(22)?)?,
        model: row.get(1)?,
        tokens: Tokens {
            input: uint(row.get(2)?),
            cached_input: uint(row.get(3)?),
            output: uint(row.get(4)?),
            reasoning_output: uint(row.get(5)?),
            total: uint(row.get(6)?),
        },
        model_tokens: ModelTokens {
            input: uint(row.get(7)?),
            output: uint(row.get(8)?),
            cache_write: uint(row.get(9)?),
            cache_read: uint(row.get(10)?),
        },
        billing: BillingUsage {
            input: uint(row.get(11)?),
            output: uint(row.get(12)?),
            cache_read: uint(row.get(13)?),
            cache_write_5m: uint(row.get(14)?),
            cache_write_1h: uint(row.get(15)?),
            web_search_requests: uint(row.get(16)?),
            speed: row.get(17)?,
            service_tier: row.get(18)?,
            prompt_size: uint(row.get(19)?),
            timestamp: row.get(20)?,
            split_unknown: row.get(21)?,
        },
    };
    Ok((row.get(0)?, usage))
}

pub(super) fn put_event(
    statement: &mut Statement<'_>,
    file_id: i64,
    extra: bool,
    seq: usize,
    event: &Event,
) -> rusqlite::Result<()> {
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
        task_id,
        resumed_agent,
        result_source,
        y,
        poll,
        r,
        item,
        peer,
        u,
        activity_observation,
        activity_tool,
    } = event;
    let y = if y.is_some()
        || poll_target.is_some()
        || task_id.is_some()
        || resumed_agent.is_some()
        || *result_source
        || *native_direct_call
        || activity_observation.is_some()
        || activity_tool.is_some()
    {
        Some(json(&EventState {
            version: 1,
            yielded: y.as_ref().map(|yielded| Box::new((**yielded).clone())),
            poll_target: poll_target.as_ref().cloned(),
            task_id: task_id.clone(),
            resumed_agent: resumed_agent.clone(),
            result_source: *result_source,
            native_direct_call: *native_direct_call,
            activity_observation: activity_observation.clone(),
            activity_tool: activity_tool.clone(),
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
            Some(int(*reply_o)),
            Some(*reply_b),
            *reply_t,
            Some(*reply_e),
            Some(*reply_f),
            reply_m.as_deref(),
        ),
        None => (false, None, None, None, None, None, None),
    };
    let (has_peer, peer_sock, peer_pid, peer_start) = match peer {
        Some(Peer { sock, pid, start }) => (true, sock.as_deref(), pid.map(int), start.as_deref()),
        None => (false, None, None, None),
    };
    statement.execute(params![
        file_id,
        extra,
        seq_int(seq),
        tag(k)?,
        int(*o),
        b,
        t,
        id,
        n,
        code_mode,
        code_mode_open,
        parent.map(seq_int),
        script,
        y,
        poll.map(seq_int),
        reply,
        reply_o,
        reply_b,
        reply_t,
        reply_e,
        reply_f,
        reply_m,
        item.map(int),
        has_peer,
        peer_sock,
        peer_pid,
        peer_start,
        u,
    ])?;
    Ok(())
}

pub(super) fn put_usage(
    statement: &mut Statement<'_>,
    file_id: i64,
    message_id: &str,
    usage: &MessageUsage,
) -> rusqlite::Result<()> {
    let MessageUsage {
        offset,
        record_ordinal,
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
    statement.execute(params![
        file_id,
        message_id,
        model,
        int(*input),
        int(*cached_input),
        int(*output),
        int(*reasoning_output),
        int(*total),
        int(*model_input),
        int(*model_output),
        int(*model_cache_write),
        int(*model_cache_read),
        int(*billing_input),
        int(*billing_output),
        int(*billing_cache_read),
        int(*cache_write_5m),
        int(*cache_write_1h),
        int(*web_search_requests),
        speed,
        service_tier,
        int(*prompt_size),
        timestamp,
        split_unknown,
        json(record_ids)?,
        int(*offset),
        record_ordinal.map(int),
    ])?;
    Ok(())
}
