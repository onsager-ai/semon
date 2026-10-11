//! Content-free native tree facts observed by the Session Event Index parser.
//! Raw activity strings and historical tool/usage accounting retain the CLI contract.
use crate::{Marker, Tokens, ToolCall, claude_usage, events, field, sealed::LogFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::Arc,
};

/// Evidence used by this tree build, rechecked together before publication.
/// No prompts or tool inputs outlive their verified reads.
#[derive(Default)]
pub(crate) struct Evidence {
    sources: BTreeMap<
        PathBuf,
        (
            crate::model::summary::CatalogSource,
            Option<Arc<crate::source_chunks::Chunks>>,
        ),
    >,
    records: BTreeMap<PathBuf, BTreeSet<crate::native_record::Record>>,
}

impl Evidence {
    pub(crate) fn source(
        &mut self,
        source: crate::model::summary::CatalogSource,
        chunks: Option<Arc<crate::source_chunks::Chunks>>,
    ) -> io::Result<()> {
        if let Some((previous, _)) = self.sources.get(&source.path) {
            if previous != &source {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native tree source changed",
                ));
            }
        } else {
            self.sources.insert(source.path.clone(), (source, chunks));
        }
        Ok(())
    }

    pub(crate) fn finish(&self, options: &crate::Options) -> io::Result<()> {
        let sources =
            crate::query_source::Sources::capture_native(options, self.sources.values().cloned())?;
        let records = sources.reader();
        records.verify_all()?;
        for (path, used) in &self.records {
            for record in used {
                records.verify_evidence(path, record)?;
            }
        }
        records.finish()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(crate) malformed_lines: u64,
    pub(crate) line_count: u64,
    pub(crate) first: Option<String>,
    pub(crate) last: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) models: BTreeSet<String>,
    pub(crate) usage_by_id: BTreeMap<String, Tokens>,
    #[serde(default)]
    pub(crate) usage_records: claude_usage::Records,
    #[serde(default)]
    pub(crate) usage_models: BTreeMap<String, String>,
    pub(crate) tools: BTreeMap<String, String>,
    pub(crate) closed_tools: BTreeSet<String>,
    pub(crate) codex_tokens: Tokens,
    #[serde(default)]
    pub(crate) codex_native_usage: Option<events::CodexNativeUsage>,
    // Never persisted: a marker can carry a handoff prompt path, and the
    // on-disk cache must not retain session content (risk:secret).
    #[serde(skip)]
    pub(crate) marker: Option<Marker>,
}

impl Summary {
    pub(crate) fn tokens(&self, harness: &str) -> Tokens {
        if harness == "codex" {
            return self.codex_native_usage.as_ref().map_or_else(
                || self.codex_tokens.clone(),
                events::CodexNativeUsage::tokens,
            );
        }
        let mut total = Tokens::default();
        for usage in self.usage_by_id.values() {
            total.input += usage.input;
            total.cached_input += usage.cached_input;
            total.output += usage.output;
            total.reasoning_output += usage.reasoning_output;
            total.total += usage.total;
        }
        total
    }

    pub(crate) fn open_tools(&self) -> Vec<ToolCall> {
        self.tools
            .iter()
            .filter(|(id, _)| !self.closed_tools.contains(*id))
            .map(|(id, name)| ToolCall {
                id: id.clone(),
                name: name.clone(),
            })
            .collect()
    }
}

impl Summary {
    pub(crate) fn observe(&mut self, record: Option<&Value>, harness: &str) {
        self.line_count += 1;
        let Some(record) = record.filter(|record| record.is_object()) else {
            self.malformed_lines += 1;
            return;
        };
        if harness == "claude" {
            update_claude(self, record);
        } else {
            update_codex(self, record);
        }
    }

    /// Repair metadata for a preexisting native ledger, without replaying events.
    pub(crate) fn repair(
        file: &mut LogFile,
        consumed: u64,
        harness: &str,
    ) -> io::Result<(Self, [u8; 32])> {
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(file.take(consumed));
        let mut summary = Self::default();
        let mut digest = Sha256::new();
        let mut read = 0;
        let mut bytes = Vec::new();
        while reader.read_until(b'\n', &mut bytes)? != 0 {
            if !bytes.ends_with(b"\n") {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            digest.update(&bytes);
            read += bytes.len() as u64;
            let record = serde_json::from_slice::<Value>(&bytes).ok();
            summary.observe(record.as_ref(), harness);
            bytes.clear();
        }
        if read != consumed {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok((summary, digest.finalize().into()))
    }
}
fn update_activity(summary: &mut Summary, record: &Value) {
    if let Some(timestamp) = field(record, "timestamp").or_else(|| {
        record
            .get("payload")
            .and_then(|payload| field(payload, "timestamp"))
    }) {
        if summary.first.is_none() {
            summary.first = Some(timestamp.into());
        }
        summary.last = Some(timestamp.into());
    }
}

fn update_claude(summary: &mut Summary, record: &Value) {
    update_activity(summary, record);
    if let Some(cwd) = field(record, "cwd") {
        summary.cwd = Some(cwd.into());
    }
    if let Some(branch) = field(record, "gitBranch") {
        summary.branch = Some(branch.into());
    }
    let Some(message) = record.get("message") else {
        return;
    };
    if let Some(model) = field(message, "model") {
        summary.models.insert(model.into());
    }
    if let (Some(id), Some(usage)) = (field(message, "id"), message.get("usage")) {
        if field(record, "type") == Some("assistant")
            && let Some(uuid) = field(record, "uuid")
        {
            summary
                .usage_records
                .entry(id.into())
                .or_default()
                .insert(uuid.into());
        }
        if let Some(model) = field(message, "model") {
            summary.usage_models.insert(id.into(), model.into());
        }
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let creation = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let read = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        summary.usage_by_id.insert(
            id.into(),
            Tokens {
                input: input + creation + read,
                cached_input: read,
                output,
                reasoning_output: 0,
                total: input + creation + read + output,
            },
        );
    }
    if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        for block in blocks {
            match field(block, "type") {
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (field(block, "id"), field(block, "name")) {
                        summary.tools.insert(id.into(), name.into());
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = field(block, "tool_use_id") {
                        summary.closed_tools.insert(id.into());
                    }
                }
                _ => {}
            }
        }
    }
}

fn update_codex(summary: &mut Summary, record: &Value) {
    update_activity(summary, record);
    let payload = &record["payload"];
    match field(record, "type") {
        Some("session_meta") => {
            if field(payload, "forked_from_id").is_some() {
                summary
                    .codex_native_usage
                    .get_or_insert_with(Default::default);
            }
            if let Some(cwd) = field(payload, "cwd") {
                summary.cwd = Some(cwd.into());
            }
            if let Some(branch) = payload.get("git").and_then(|git| field(git, "branch")) {
                summary.branch = Some(branch.into());
            }
        }
        Some("turn_context") => {
            if let Some(model) = field(payload, "model") {
                summary.models.insert(model.into());
            }
        }
        Some("token_usage_record") => {
            // Tree summaries preserve available request counts without asserting
            // model attribution; their native model labels are tracked separately.
            events::CodexNativeUsage::observe(
                &mut summary.codex_native_usage,
                payload,
                "unknown".to_owned(),
                None,
            );
        }
        Some("event_msg") if field(payload, "type") == Some("token_count") => {
            if let Some(usage) = payload
                .get("info")
                .and_then(|info| info.get("total_token_usage"))
            {
                summary.codex_tokens = Tokens {
                    input: usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cached_input: usage
                        .get("cached_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    output: usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    reasoning_output: usage
                        .get("reasoning_output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    total: usage
                        .get("total_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                };
            }
        }
        _ => {}
    }
}

pub(crate) fn read(
    options: &crate::Options,
    path: &std::path::Path,
    harness: &str,
    native_id: &str,
    cache: &mut events::EventCache,
    dirty: &mut bool,
    evidence: &mut Evidence,
) -> io::Result<Summary> {
    let index = events::scan_file(path, harness, cache, dirty)?;
    let mut summary = index
        .tree_summary
        .clone()
        .ok_or_else(|| io::Error::other("native tree metadata unavailable"))?;
    let source = cache
        .source(path, native_id)
        .ok_or_else(|| io::Error::other("native tree source unavailable"))?;
    let sources = crate::query_source::Sources::capture_native(
        options,
        std::iter::once((source.clone(), index.source_chunks.clone())),
    )?;
    let records = sources.reader();
    records.verify_all()?;
    summary.marker = records.marker(
        path,
        index
            .marker_index
            .as_ref()
            .ok_or_else(|| io::Error::other("native tree marker unavailable"))?,
    )?;
    records.finish()?;
    evidence.source(source, index.source_chunks.clone())?;
    if let Some(record) = index
        .marker_index
        .as_ref()
        .and_then(|marker| marker.record())
    {
        evidence
            .records
            .entry(path.to_owned())
            .or_default()
            .insert(record.clone());
    }
    Ok(summary)
}

pub(crate) fn launch(
    options: &crate::Options,
    cache: &events::EventCache,
    path: &std::path::Path,
    prompt: &str,
    cwd: Option<&str>,
    start: Option<i64>,
    evidence: &mut Evidence,
) -> io::Result<Option<ToolCall>> {
    let index = cache
        .file_index(path)
        .ok_or_else(|| io::Error::other("native tree parent unavailable"))?;
    let source = evidence
        .sources
        .get(path)
        .map(|(source, _)| source.clone())
        .ok_or_else(|| io::Error::other("native tree parent source unavailable"))?;
    let sources = crate::query_source::Sources::capture_native(
        options,
        std::iter::once((source, index.source_chunks.clone())),
    )?;
    let records = sources.reader();
    records.verify_all()?;
    let calls = index
        .handoff_index
        .as_ref()
        .ok_or_else(|| io::Error::other("native tree launch metadata unavailable"))?;
    let mut matcher = crate::handoff::Matcher::new(prompt, cwd, start);
    let mut selected = None;
    for location in calls.calls() {
        let call = records.launch(path, location)?;
        evidence
            .records
            .entry(path.to_owned())
            .or_default()
            .insert(location.record().clone());
        if let Some(tool) = matcher.consider(&call) {
            selected = Some(tool);
            break;
        }
    }
    let selected = selected.or_else(|| matcher.finish());
    records.finish()?;
    Ok(selected)
}

#[cfg(test)]
mod tests;
