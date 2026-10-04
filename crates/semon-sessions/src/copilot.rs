//! Pinned persisted Copilot CLI contract. No native CLI execution or recovery.
use crate::{
    Tokens,
    events::{CodexUsageEvent, ModelTokens},
    field,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader},
    path::Path,
};

/// A complete cumulative shutdown usage observation, never a per-message delta.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CopilotUsage {
    /// Native totals; missing observations are represented by an absent snapshot.
    pub tokens: Tokens,
    /// Exact native model identities and their categories.
    pub by_model: BTreeMap<String, ModelTokens>,
    /// Native reasoning counts, separate from billed output.
    pub reasoning_by_model: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Index {
    pub version: String,
    pub session_id: String,
    pub seen: BTreeSet<String>,
    pub usage: Option<CopilotUsage>,
    pub usage_source: Option<Observation>,
    pub usage_events: Vec<CodexUsageEvent>,
    pub lifecycle: Option<String>,
    pub first: Option<String>,
    pub last: Option<String>,
}

/// Identity and source location of a persisted usage observation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    /// Exact native event id.
    pub event_id: String,
    /// Source byte offset within this session generation.
    pub offset: u64,
    /// Persisted timestamp in epoch milliseconds, or unknown.
    pub timestamp: Option<i64>,
}

/// Source-supported facts; null values explicitly represent unknown evidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopilotEvidence {
    /// Native CLI release from session.start.
    pub version: String,
    /// Native saved-state schema version.
    pub schema: u32,
    /// Latest complete cumulative shutdown snapshot, or unknown.
    pub usage: Option<CopilotUsage>,
    /// Exact source of the latest shutdown observation, including incomplete ones.
    pub usage_source: Option<Observation>,
    /// Last persisted session lifecycle observation (not process liveness).
    pub lifecycle: Option<String>,
    /// This contract establishes no logical session parent.
    pub logical_parent: Option<String>,
    /// The pinned CLI does not persist successful approval records.
    pub approvals: Option<String>,
}
impl Index {
    pub fn evidence(&self) -> CopilotEvidence {
        CopilotEvidence {
            version: self.version.clone(),
            schema: 1,
            usage: self.usage.clone(),
            usage_source: self.usage_source.clone(),
            lifecycle: self.lifecycle.clone(),
            logical_parent: None,
            approvals: None,
        }
    }
}

/// Validate a native session.start against the two fixture-backed releases.
/// Returns the recorded session identity; unsupported formats are diagnostics.
pub fn validate_start(record: &Value, expected: &str) -> io::Result<String> {
    let data = &record["data"];
    if field(record, "type") != Some("session.start")
        || data["version"].as_u64() != Some(1)
        || !matches!(field(data, "copilotVersion"), Some("1.0.90" | "1.0.91"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported Copilot persisted format: require session.start schema 1, CLI 1.0.90 or 1.0.91",
        ));
    }
    if field(data, "sessionId") != Some(expected) || field(record, "id").is_none_or(str::is_empty) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Copilot session.start identity does not match its session-state directory",
        ));
    }
    Ok(field(data, "copilotVersion")
        .expect("validated version")
        .to_owned())
}

pub(crate) fn header(path: &Path, expected: &str) -> io::Result<Option<String>> {
    let file = crate::open_input(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    if !line.ends_with(b"\n") {
        return Ok(None);
    }
    let record: Value =
        serde_json::from_slice(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    validate_start(&record, expected).map(Some)
}

pub(crate) fn snapshot(data: &Value) -> Option<CopilotUsage> {
    let models = data.get("modelMetrics")?.as_object()?;
    if models.is_empty() {
        return None;
    }
    let mut snapshot = CopilotUsage::default();
    for (model, report) in models {
        let usage = report.get("usage")?;
        let [
            Some(input),
            Some(output),
            Some(read),
            Some(write),
            Some(reasoning),
        ] = [
            "inputTokens",
            "outputTokens",
            "cacheReadTokens",
            "cacheWriteTokens",
            "reasoningTokens",
        ]
        .map(|key| usage.get(key).and_then(Value::as_u64))
        else {
            return None;
        };
        snapshot.tokens.input += input;
        snapshot.tokens.output += output;
        snapshot.tokens.cached_input += read;
        snapshot.tokens.reasoning_output += reasoning;
        snapshot.tokens.total += input + output;
        snapshot.by_model.insert(
            model.clone(),
            ModelTokens {
                input,
                output,
                cache_read: read,
                cache_write: write,
            },
        );
        snapshot.reasoning_by_model.insert(model.clone(), reasoning);
    }
    Some(snapshot)
}

/// Native ISO8601 timestamp as store nanoseconds, or unknown. Never a join key.
pub fn timestamp_ns(text: &str) -> Option<i64> {
    crate::events::parse_ms(text)?.checked_mul(1_000_000)
}
