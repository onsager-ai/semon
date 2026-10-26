//! Current native row restoration shared by physical adapters. No SQL or Raw I/O.
use super::*;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub(crate) struct DecodeError;

/// Adapters normalize physical integers, byte strings and nullable values here.
/// This is a fixed native row contract, never an arbitrary SQL interface.
pub(crate) struct Values(pub(crate) Vec<serde_json::Value>);

impl Values {
    pub(crate) fn get<T: DeserializeOwned>(&self, column: usize) -> Result<T, DecodeError> {
        serde_json::from_value(self.0.get(column).ok_or(DecodeError)?.clone())
            .map_err(|_| DecodeError)
    }
}

fn json<T: DeserializeOwned>(text: &str) -> Result<T, DecodeError> {
    serde_json::from_str(text).map_err(|_| DecodeError)
}

pub(crate) fn ledger(row: &Values) -> Result<(i64, Ledger), DecodeError> {
    let ledger = Ledger {
        stat: Stat {
            dev: row.get::<i64>(1)? as u64,
            ino: row.get::<i64>(2)? as u64,
            size: row.get::<i64>(3)? as u64,
            modified_ns: u128::from_be_bytes(row.get(4)?),
            changed_ns: row.get::<Option<[u8; 16]>>(8)?.map(i128::from_be_bytes),
        },
        offset: row.get::<i64>(5)? as u64,
        prefix: row.get(6)?,
        tail: row.get(7)?,
    };
    if ledger.offset > ledger.stat.size {
        return Err(DecodeError);
    }
    Ok((row.get(0)?, ledger))
}

pub(crate) fn event(row: &Values) -> Result<(bool, usize, Event), DecodeError> {
    let state = row.get::<Option<String>>(12)?.map(|s| json::<native_rows::EventState>(&s)).transpose()?;
    if state.as_ref().is_some_and(|s| s.version != 1) {
        return Err(DecodeError);
    }
    let (y, poll_target, result_source, native_direct_call) = match state {
        Some(s) => (s.yielded, s.poll_target, s.result_source, s.native_direct_call),
        None => (None, None, false, false),
    };
    let r = if row.get::<bool>(14)? {
        Some(Reply {
            o: row.get::<i64>(15)? as u64,
            b: row.get(16)?, t: row.get(17)?, e: row.get(18)?,
            f: row.get(19)?, m: row.get(20)?,
        })
    } else { None };
    let peer = if row.get::<bool>(22)? {
        Some(Peer { sock: row.get(23)?, pid: row.get::<Option<i64>>(24)?.map(|v| v as u64), start: row.get(25)? })
    } else { None };
    let ordinal = |v: i64| usize::try_from(v).map_err(|_| DecodeError);
    Ok((row.get(0)?, ordinal(row.get(1)?)?, Event {
        k: serde_json::from_value(serde_json::Value::String(row.get(2)?)).map_err(|_| DecodeError)?,
        o: row.get::<i64>(3)? as u64,
        b: row.get(4)?, t: row.get(5)?, id: row.get(6)?, n: row.get(7)?,
        code_mode: row.get(8)?, code_mode_open: row.get(9)?,
        parent: row.get::<Option<i64>>(10)?.map(ordinal).transpose()?,
        script: row.get(11)?, y, poll_target, result_source, native_direct_call,
        poll: row.get::<Option<i64>>(13)?.map(ordinal).transpose()?, r,
        item: row.get::<Option<i64>>(21)?.map(|v| v as u64), peer, u: row.get(26)?,
    }))
}

pub(crate) fn signal(row: &Values) -> Result<(usize, Signal), DecodeError> {
    Ok((usize::try_from(row.get::<i64>(0)?).map_err(|_| DecodeError)?, Signal {
        k: serde_json::from_value(serde_json::Value::String(row.get(1)?)).map_err(|_| DecodeError)?,
        o: row.get::<i64>(2)? as u64, t: row.get(3)?, at: row.get(4)?,
        n: row.get(5)?, v: row.get::<Option<i64>>(6)?.map(|v| v as u64),
    }))
}

pub(crate) fn usage(row: &Values) -> Result<(String, MessageUsage), DecodeError> {
    Ok((row.get(0)?, MessageUsage {
        record_ids: json(&row.get::<String>(22)?)?, model: row.get(1)?,
        tokens: Tokens {
            input: row.get::<i64>(2)? as u64, cached_input: row.get::<i64>(3)? as u64,
            output: row.get::<i64>(4)? as u64, reasoning_output: row.get::<i64>(5)? as u64,
            total: row.get::<i64>(6)? as u64,
        },
        model_tokens: ModelTokens {
            input: row.get::<i64>(7)? as u64, output: row.get::<i64>(8)? as u64,
            cache_write: row.get::<i64>(9)? as u64, cache_read: row.get::<i64>(10)? as u64,
        },
        billing: BillingUsage {
            input: row.get::<i64>(11)? as u64, output: row.get::<i64>(12)? as u64,
            cache_read: row.get::<i64>(13)? as u64, cache_write_5m: row.get::<i64>(14)? as u64,
            cache_write_1h: row.get::<i64>(15)? as u64, web_search_requests: row.get::<i64>(16)? as u64,
            speed: row.get(17)?, service_tier: row.get(18)?, prompt_size: row.get::<i64>(19)? as u64,
            timestamp: row.get(20)?, split_unknown: row.get(21)?,
        },
    }))
}

pub(crate) fn codex_usage(row: &Values) -> Result<(usize, CodexUsageEvent), DecodeError> {
    Ok((usize::try_from(row.get::<i64>(0)?).map_err(|_| DecodeError)?, CodexUsageEvent {
        model: row.get(1)?, tokens: ModelTokens {
            input: row.get::<i64>(2)? as u64, output: row.get::<i64>(3)? as u64,
            cache_write: row.get::<i64>(4)? as u64, cache_read: row.get::<i64>(5)? as u64,
        }, timestamp: row.get(6)?,
    }))
}

/// All independently qualified children; absence is explicit, never fabricated.
pub(crate) struct Children {
    pub(crate) events: Vec<Event>,
    pub(crate) extras: Vec<Event>,
    pub(crate) signals: Vec<Signal>,
    pub(crate) usage_by_id: BTreeMap<String, MessageUsage>,
    pub(crate) codex_usage_events: Vec<CodexUsageEvent>,
    pub(crate) native_usage_observations: Option<Vec<store::native_usage::Observation>>,
    pub(crate) source_chunks: Option<Arc<crate::source_chunks::Chunks>>,
    pub(crate) marker_index: Option<crate::native_marker::Index>,
    pub(crate) tree_summary: Option<crate::native_tree::Summary>,
    pub(crate) handoff_index: Option<crate::native_handoff::Index>,
    pub(crate) codex_header: Option<crate::native_header::CodexHeader>,
    pub(crate) claude_sidecar: Option<crate::native_sidecar::ClaudeSidecar>,
    pub(crate) codex_native_usage: Option<CodexNativeUsage>,
}

pub(crate) fn file(row: &Values, c: Children) -> Result<FileIndex, DecodeError> {
    Ok(FileIndex {
        dirty_rows: None,
        native_usage_observations: c.native_usage_observations,
        source_chunks: c.source_chunks, marker_index: c.marker_index,
        tree_summary: c.tree_summary, handoff_index: c.handoff_index,
        codex_header: c.codex_header, claude_sidecar: c.claude_sidecar,
        events: c.events, extras: c.extras, signals: c.signals,
        usage_by_id: c.usage_by_id, codex_usage_events: c.codex_usage_events,
        codex_native_usage: c.codex_native_usage,
        pending: json(&row.get::<String>(18)?)?, tool_ids: json(&row.get::<String>(19)?)?,
        yields: json(&row.get::<String>(20)?)?, busy: json(&row.get::<String>(21)?)?,
        links: json(&row.get::<String>(22)?)?,
        entrypoint: row.get(9)?, title: row.get(10)?, agent_name: row.get(11)?, last_model: row.get(12)?,
        failed: row.get(13)?, first: row.get(14)?, last: row.get(15)?, cwd: row.get(16)?, branch: row.get(17)?,
        codex_tokens: json(&row.get::<String>(23)?)?, codex_tokens_by_model: json(&row.get::<String>(24)?)?,
        rate_limits: row.get::<Option<String>>(25)?.map(|s| json(&s)).transpose()?,
        copilot: row.get::<Option<String>>(27)?.map(|s| json(&s)).transpose()?,
    })
}

pub(crate) fn usage_root(encoded: &str) -> Result<CodexNativeUsage, DecodeError> {
    #[derive(Deserialize)]
    struct Root { projection_version: u32, tokens: Tokens, cost: crate::pricing::CostBuild }
    let root: Root = json(encoded)?;
    if root.projection_version != 1 { return Err(DecodeError); }
    Ok(CodexNativeUsage { tokens: root.tokens, cost: Some(root.cost), ..Default::default() })
}

pub(crate) fn usage_dimension(native: &mut CodexNativeUsage, kind: &str, key: String, value: &str) -> Result<(), DecodeError> {
    let cost = native.cost.as_mut().ok_or(DecodeError)?;
    match kind {
        "model" => { native.by_model.insert(key, json(value)?); }
        "cost_model" => { cost.models.insert(key, json(value)?); }
        "cost_day" => { cost.by_day.insert(key, json(value)?); }
        _ => return Err(DecodeError),
    }
    Ok(())
}

pub(crate) fn finish_usage(native: &mut CodexNativeUsage) -> Result<(), DecodeError> {
    let cost = native.cost.as_mut().ok_or(DecodeError)?;
    cost.unpriced_models = cost.models.iter().filter(|(_, m)| m.unpriced).map(|(k, _)| k.clone()).collect();
    Ok(())
}
