//! The per-file event index behind the session model.
//!
//! Every event is metadata and byte offsets only: its kind, line offset,
//! block, time and the ids that join it to other files. It never holds
//! message text, briefs, tool input or output, or answers (risk:secret).
//! The model reads text back from the source line by offset and keeps it in
//! memory only.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Tokens, attachments, field, sealed::LogFile};

pub(crate) mod file_rows;
pub(crate) mod native_aux;
pub(crate) mod native_commit;
pub(crate) mod native_decode;
pub(crate) mod native_pages;
pub(crate) mod native_rows;
pub(crate) mod reported_rows;
pub(crate) mod store;
pub(crate) mod tree_rows;
pub(crate) use store::open_regular;
pub(crate) use store::{archive_index_retained, retain_archive_index};

#[cfg(test)]
thread_local! {
    /// Reads of the persisted event cache on this thread.
    pub(crate) static CACHE_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Full per-file event/signal row decodes, including exact-path loads.
    pub(crate) static FILE_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static CODEX_HEADER_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static CLAUDE_SIDECAR_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static AFTER_CLAUDE_SIDECAR: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_CODEX_HEADER: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_codex_header_hook(hook: impl FnOnce() + 'static) {
    AFTER_CODEX_HEADER.with(|value| *value.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
pub(crate) fn set_claude_sidecar_hook(hook: impl FnOnce() + 'static) {
    AFTER_CLAUDE_SIDECAR.with(|value| *value.borrow_mut() = Some(Box::new(hook)));
}

/// The parser's version: what the fold functions derive from a line. The
/// index store keeps it and empties its derived tables when it differs, so
/// every file is read again.
/// v3: every assistant text is its own event (v2 collapsed adjacent ones).
/// v4: thinking blocks and Codex harness text are indexed apart, as `extras`.
/// v5: completed Codex items carry their exact code-mode call parent, if known.
/// v6: code-mode calls stop accepting item attribution at a turn boundary.
/// v7: tool ids index all tool events for exact legacy item matching.
/// v8: an item is an operation only when exactly one code-mode call owns it,
/// and a script error is read from the harness header alone.
/// v9: token usage is retained by model and Codex rate limits are indexed.
/// v10: assistant billing splits, timestamps and Codex token-count deltas.
/// v11: a Codex command that outlived its yield links its polls and its
/// completion to the script that started it.
/// v12: thinking extras and the thinking filters follow the viewer rewrite (#38).
/// v13: a plain Codex call links its `CommandExecution` item, which holds the
/// whole collected output (#52).
/// v14: compaction and interrupt signals are indexed apart, as `signals` (PR 1 of the dropped-signals plan).
/// v15: a prompt that attaches only images, and no text, is indexed.
/// v16: run settings, hook runs and permission denials are indexed as `signals`
/// (PR 2 of the dropped-signals plan).
/// v17: Claude assistant lines index their reasoning effort (`effort`, or `perTurnEffort` when set) as an `Effort` signal.
/// v18: background Claude Bash calls retain their launch flag and terminal
/// notifications retain their failure outcome.
/// v19: ledgers verify the full consumed prefix instead of just two windows.
/// v23: native connector completions and tool search are tools, including
/// their exact code-mode parent; only explicitly typed reasoning is thinking.
/// v25: agent messages carry their direction; coordinator messages and unknown origin kinds are indexed; relay tags count only at the start of a prompt.
/// v26: a subagent's non-final send (`senderTaskId`, no `handback`) is an up agent message flagged [`AGM_PROGRESS`], not a relay; relay tags nested in another relay block are not their own events.
/// v27: rebuild derived billing usage after a pricing-table restoration.
/// v28: source-qualified code-mode output/terminal attribution and direct formatter
/// outcomes; event-state envelopes preserve bounded selected output evidence.
/// v29: durable source-owned Codex turn activity, separate from writer liveness.
pub(crate) const CACHE_VERSION: u32 = 29;

/// The four token categories the model serves for an exact model id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokens {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl ModelTokens {
    pub(crate) fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }
}

/// A Claude assistant message's token report. `tokens` preserves the legacy
/// triple's input/cache-read accounting; `model_tokens` keeps cache writes
/// separate for the model JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MessageUsage {
    #[serde(default)]
    pub(crate) record_ids: BTreeSet<String>,
    pub(crate) model: Option<String>,
    pub(crate) tokens: Tokens,
    pub(crate) model_tokens: ModelTokens,
    pub(crate) billing: BillingUsage,
}

/// Per-message facts used only for price calculation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BillingUsage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write_5m: u64,
    pub(crate) cache_write_1h: u64,
    pub(crate) web_search_requests: u64,
    pub(crate) speed: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) prompt_size: u64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) split_unknown: bool,
}

/// One Codex cumulative `token_count` event converted to the delta since the
/// preceding event in its file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CodexUsageEvent {
    pub(crate) model: String,
    pub(crate) tokens: ModelTokens,
    pub(crate) timestamp: Option<i64>,
}

/// Explicit per-response usage, kept separate from legacy cumulative counters.
/// Native thread counters can include a fork's inherited context.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct CodexNativeUsage {
    #[serde(default)]
    seen: BTreeSet<String>,
    tokens: Tokens,
    #[serde(default)]
    by_model: BTreeMap<String, ModelTokens>,
    #[serde(default)]
    events: Vec<CodexUsageEvent>,
    #[serde(skip)]
    cost: Option<crate::pricing::CostBuild>,
}

impl CodexNativeUsage {
    pub(crate) fn tokens(&self) -> Tokens {
        self.tokens.clone()
    }

    pub(crate) fn compact(&self) -> Self {
        Self {
            tokens: self.tokens.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn observe(
        slot: &mut Option<Self>,
        payload: &Value,
        model: String,
        time: Option<i64>,
    ) {
        let Some((key, tokens, counts)) = Self::decode(payload) else {
            return;
        };
        let native = slot.get_or_insert_with(Default::default);
        if !native.seen.insert(key) {
            return;
        }
        native.tokens.input += tokens.input;
        native.tokens.cached_input += tokens.cached_input;
        native.tokens.output += tokens.output;
        native.tokens.total += tokens.total;
        native.tokens.reasoning_output += tokens.reasoning_output;
        native
            .by_model
            .entry(model.clone())
            .or_default()
            .add(&counts);
        let event = CodexUsageEvent {
            model,
            tokens: counts,
            timestamp: time,
        };
        if let Some(cost) = &mut native.cost {
            cost.codex(&event);
        } else {
            native.events.push(event);
        }
    }

    /// Shared validation for indexing and receipt-checked representation migration.
    pub(crate) fn decode(payload: &Value) -> Option<(String, Tokens, ModelTokens)> {
        let usage = payload.get("usage")?;
        let identity =
            ["thread_id", "session_id", "turn_id", "response_id"].map(|key| field(payload, key));
        if identity.iter().any(|id| id.is_none_or(str::is_empty)) {
            return None;
        }
        let [
            Some(input),
            Some(output),
            Some(total),
            Some(cache_read),
            Some(cache_write),
            Some(reasoning),
        ] = [
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "reasoning_output_tokens",
        ]
        .map(|key| usage.get(key).and_then(Value::as_u64))
        else {
            return None;
        };
        // Serialize the exact tuple, avoiding delimiter collisions or timing joins.
        let key = serde_json::to_string(&identity).expect("string tuple serializes");
        let counts = ModelTokens {
            input: input.saturating_sub(cache_read),
            output,
            cache_read,
            cache_write,
        };
        Some((
            key,
            Tokens {
                input,
                cached_input: cache_read,
                output,
                total,
                reasoning_output: reasoning,
            },
            counts,
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RateLimitWindow {
    pub(crate) minutes: u64,
    pub(crate) used_percent: f64,
    pub(crate) resets_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RateLimits {
    pub(crate) recorded_at: i64,
    pub(crate) windows: Vec<RateLimitWindow>,
}

pub(crate) type FileRevision = (u64, u64, u64, [u8; 32], [u8; 32], Option<i128>);

/// Rows touched while parsing, plus the append boundary of each table.
/// This is transient write metadata, never persisted with the index.
#[derive(Clone, Debug, Default)]
pub(crate) struct DirtyRows {
    pub(crate) events: BTreeSet<usize>,
    pub(crate) extras: BTreeSet<usize>,
    pub(crate) signals: BTreeSet<usize>,
    pub(crate) codex_usage: BTreeSet<usize>,
    pub(crate) usage: BTreeSet<String>,
    pub(crate) before_events: usize,
    pub(crate) before_extras: usize,
    pub(crate) before_signals: usize,
    pub(crate) before_codex_usage: usize,
}

impl DirtyRows {
    fn begin(index: &FileIndex) -> Self {
        Self {
            before_events: index.events.len(),
            before_extras: index.extras.len(),
            before_signals: index.signals.len(),
            before_codex_usage: index.codex_usage_events.len(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn between(old: &FileIndex, new: &FileIndex) -> Self {
        fn changed<T: PartialEq>(old: &[T], new: &[T]) -> BTreeSet<usize> {
            new.iter()
                .enumerate()
                .filter_map(|(seq, row)| (old.get(seq) != Some(row)).then_some(seq))
                .collect()
        }
        Self {
            events: changed(&old.events, &new.events),
            extras: changed(&old.extras, &new.extras),
            signals: changed(&old.signals, &new.signals),
            codex_usage: changed(&old.codex_usage_events, &new.codex_usage_events),
            usage: old
                .usage_by_id
                .keys()
                .chain(new.usage_by_id.keys())
                .filter(|id| old.usage_by_id.get(*id) != new.usage_by_id.get(*id))
                .cloned()
                .collect(),
            ..Self::begin(old)
        }
    }
}

/// One file's event index and the facts the model needs about it. Metadata
/// only (risk:secret).
#[derive(Clone, Debug, Default)]
pub(crate) struct FileIndex {
    /// Exact local carrier when the private text Index key cannot encode it.
    pub(crate) native_path: Option<PathBuf>,
    pub(crate) dirty_rows: Option<DirtyRows>,
    /// Only this parse batch; durable facts and identities stay in the Event Index.
    pub(crate) native_usage_observations: Option<Vec<store::native_usage::Observation>>,
    pub(crate) tree_summary: Option<crate::native_tree::Summary>,
    pub(crate) source_chunks: Option<Arc<crate::source_chunks::Chunks>>,
    pub(crate) marker_index: Option<crate::native_marker::Index>,
    pub(crate) handoff_index: Option<crate::native_handoff::Index>,
    /// Allowlisted first-record metadata observed on the same native generation.
    pub(crate) codex_header: Option<crate::native_header::CodexHeader>,
    /// A subagent sidecar has its own generation, separate from the native log.
    pub(crate) claude_sidecar: Option<crate::native_sidecar::ClaudeSidecar>,
    pub(crate) events: Vec<Event>,
    /// Transcript-only items the model's rules never read: thinking blocks,
    /// harness text added before a Codex prompt, and completed Codex operations.
    /// Kept apart from `events` so they can't change how events collapse or
    /// turns split.
    pub(crate) extras: Vec<Event>,
    /// Harness signals indexed for analysis, never shown in the transcript.
    pub(crate) signals: Vec<Signal>,
    /// Tool calls still waiting for a result: call id -> event index.
    pub(crate) pending: BTreeMap<String, usize>,
    /// Tool call id -> all event indices with that exact id.
    tool_ids: BTreeMap<String, Vec<usize>>,
    /// Codex commands still running after their yield: the exec session id
    /// the yielded output named -> the event index of the script that
    /// started it. At most [`YIELDS_MAX`].
    pub(crate) yields: BTreeMap<String, usize>,
    /// Line timestamps clustered into busy intervals (epoch ms).
    pub(crate) busy: Vec<(i64, i64)>,
    pub(crate) links: Links,
    /// Claude `entrypoint` or Codex `originator`.
    pub(crate) entrypoint: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) last_model: Option<String>,
    /// Codex: the last `task_complete` carried an error.
    pub(crate) failed: bool,
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
    pub(crate) cwd: Option<String>,
    pub(crate) branch: Option<String>,
    usage_by_id: BTreeMap<String, MessageUsage>,
    codex_tokens: Tokens,
    codex_tokens_by_model: BTreeMap<String, ModelTokens>,
    codex_usage_events: Vec<CodexUsageEvent>,
    codex_native_usage: Option<CodexNativeUsage>,
    pub(crate) copilot: Option<crate::copilot::Index>,
    pub(crate) rate_limits: Option<RateLimits>,
}

/// A harness signal that isn't a transcript event: indexed for analysis only,
/// never shown in the transcript. Content-free like the rest of the cache:
/// `n` holds only a short enum-like tag, never log text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Signal {
    pub(crate) k: SignalKind,
    /// Byte offset of the line.
    pub(crate) o: u64,
    /// Timestamp (same unit and source as `Event::t`).
    pub(crate) t: Option<i64>,
    /// `events.len()` when the signal was pushed: its place among the file's events.
    pub(crate) at: u32,
    /// A short tag: the compaction trigger, the interrupt kind or reason, a
    /// run setting's value, a hook's `event:outcome`, or a denial's source.
    /// Compact and interrupt tags are at most 32 bytes, the others at most
    /// [`TAG_MAX`] characters.
    pub(crate) n: Option<String>,
    /// A number the harness reported with it (tokens before a compaction), or,
    /// for a denial or a blocking hook, the index in `events` of the tool call
    /// it refused.
    pub(crate) v: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SignalKind {
    Compact,
    Interrupt,
    /// A Codex turn's model, when it changes (`n` is the model id).
    Model,
    /// A reported reasoning effort, when it changes.
    Effort,
    /// A Codex turn's approval policy, when it changes.
    Approval,
    /// A Codex turn's sandbox policy, when it changes.
    Sandbox,
    /// A Claude Code permission mode, when it changes.
    Permission,
    /// A hook run: `n` is `event:outcome`.
    Hook,
    /// A refused tool call: `n` is `user`, `policy` or `declined`.
    Denial,
}

impl SignalKind {
    /// Every kind but compactions and interrupts only annotates the log, so a
    /// compaction's dedupe looks past it.
    fn annotates(self) -> bool {
        !matches!(self, Self::Compact | Self::Interrupt)
    }
}

/// The longest `n` a run-setting, hook or denial signal keeps, in characters.
pub(crate) const TAG_MAX: usize = 64;

/// Eight hex digits of the SHA-256 of `text`: a stable short name that tells
/// two long values, or two runs, apart without keeping either.
pub(crate) fn short_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A value fit to cache as a signal's `n`: enum-like text of letters, digits
/// and `_ - . : / [ ] +` or a space, at most [`TAG_MAX`] characters. Text with
/// anything else in it (a newline, a quote, a sentence) is not kept at all. A
/// longer value is cut and ends in `#` and a hash of the whole value, so two
/// values that share a long prefix stay different.
fn bounded(text: &str) -> Option<String> {
    let safe = !text.is_empty()
        && text.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'-' | b'.' | b':' | b'/' | b'[' | b']' | b'+' | b' '
                )
        });
    if !safe {
        return None;
    }
    // The text is ASCII, so bytes and characters agree.
    Some(if text.len() > TAG_MAX {
        format!("{}#{}", &text[..TAG_MAX - 9], short_hash(text))
    } else {
        text.to_owned()
    })
}

/// Pushes a signal whose tag is already bounded, without the compaction dedupe.
fn note(summary: &mut FileIndex, k: SignalKind, o: u64, t: Option<i64>, n: String, v: Option<u64>) {
    let at = u32::try_from(summary.events.len()).unwrap_or(u32::MAX);
    summary.signals.push(Signal {
        k,
        o,
        t,
        at,
        n: Some(n),
        v,
    });
}

/// Records a run setting when `value` is the first seen or differs from the
/// last one recorded for `k`. A missing value changes nothing.
fn setting(summary: &mut FileIndex, k: SignalKind, o: u64, t: Option<i64>, value: Option<&str>) {
    let Some(value) = value.and_then(bounded) else {
        return;
    };
    let last = summary
        .signals
        .iter()
        .rev()
        .find(|signal| signal.k == k)
        .and_then(|signal| signal.n.as_deref());
    if last != Some(value.as_str()) {
        note(summary, k, o, t, value, None);
    }
}

/// A policy as Codex writes it: a name, or an object naming its type or mode
/// (a single-key object such as a granular policy names itself).
fn policy_name(value: &Value) -> Option<&str> {
    match value {
        Value::String(name) => Some(name),
        Value::Object(map) => field(value, "type")
            .or_else(|| field(value, "mode"))
            .or_else(|| {
                (map.len() == 1)
                    .then(|| map.keys().next().map(String::as_str))
                    .flatten()
            }),
        _ => None,
    }
}

/// The index in `events` of the latest tool call with this id.
fn call_index(summary: &FileIndex, id: &str) -> Option<u64> {
    summary
        .tool_ids
        .get(id)
        .and_then(|indices| indices.last())
        .and_then(|index| u64::try_from(*index).ok())
}

/// A Codex `turn_context`: each setting is a signal only when it differs from
/// the last one recorded, so the first turn records them all.
fn codex_settings(summary: &mut FileIndex, payload: &Value, o: u64, t: Option<i64>) {
    setting(summary, SignalKind::Model, o, t, field(payload, "model"));
    let effort = field(payload, "effort").or_else(|| {
        payload
            .get("collaboration_mode")
            .and_then(|mode| mode.get("settings"))
            .and_then(|settings| field(settings, "reasoning_effort"))
    });
    setting(summary, SignalKind::Effort, o, t, effort);
    let approval = payload.get("approval_policy").and_then(policy_name);
    setting(summary, SignalKind::Approval, o, t, approval);
    let sandbox = payload.get("sandbox_policy").and_then(policy_name);
    setting(summary, SignalKind::Sandbox, o, t, sandbox);
}

/// A Codex command or patch whose approval was declined. One call ends as an
/// event and as an item: it counts once.
fn codex_declined(
    summary: &mut FileIndex,
    id: Option<&str>,
    status: Option<&str>,
    o: u64,
    t: Option<i64>,
) {
    if !status.is_some_and(|status| status.eq_ignore_ascii_case("declined")) {
        return;
    }
    let v = id.and_then(|id| call_index(summary, id));
    let at = u32::try_from(summary.events.len()).unwrap_or(u32::MAX);
    let seen = summary.signals.iter().rev().take(16).any(|signal| {
        signal.k == SignalKind::Denial && signal.v == v && (v.is_some() || signal.at == at)
    });
    if !seen {
        note(summary, SignalKind::Denial, o, t, "declined".to_owned(), v);
    }
}

/// How much of a tool result's text is read to tell a refusal: the harness's
/// fixed wording is at the start.
const DENIAL_PREFIX: usize = 160;

/// The start of a tool result's text, borrowed: the whole result is never
/// copied.
fn text_prefix(content: Option<&Value>) -> &str {
    let text = match content {
        Some(Value::String(text)) => text.as_str(),
        Some(Value::Array(parts)) => parts
            .iter()
            .find_map(|part| field(part, "text"))
            .unwrap_or(""),
        _ => "",
    }
    .trim_start();
    let end = text
        .char_indices()
        .nth(DENIAL_PREFIX)
        .map_or(text.len(), |(end, _)| end);
    &text[..end]
}

/// The source of a Claude Code tool refusal: `user`, `policy` or `classifier`.
/// The record's own `toolDenialKind` decides when it is there; otherwise the
/// harness's fixed wording at the start of the result does. None of the text
/// is kept.
fn denial_source(
    kind: Option<&str>,
    content: Option<&Value>,
    result: Option<&Value>,
) -> Option<&'static str> {
    if let Some(kind) = kind {
        let kind = kind
            .chars()
            .take(32)
            .collect::<String>()
            .to_ascii_lowercase();
        // `automode-blocked` is the value real logs carry for a classifier
        // denial; the others are guesses.
        return Some(if kind.contains("automode") || kind.contains("classif") {
            "classifier"
        } else if kind.contains("user") || kind.contains("reject") {
            "user"
        } else {
            "policy"
        });
    }
    let text = text_prefix(content);
    if text.starts_with("The user doesn't want to")
        || result.and_then(Value::as_str) == Some("User rejected tool use")
    {
        Some("user")
    } else if text.starts_with("Permission for this action was denied") {
        Some(if text.contains("classifier") {
            "classifier"
        } else {
            "policy"
        })
    } else if (text.starts_with("Permission to use ") && text.contains(" has been denied"))
        || text.starts_with("Permission for this action has been denied")
    {
        Some("policy")
    } else {
        None
    }
}

/// How much a hook outcome says, so one run logged twice keeps the strongest.
fn outcome_rank(outcome: &str) -> u8 {
    match outcome {
        "blocked" => 5,
        "error" => 4,
        "stopped" => 3,
        "cancelled" => 2,
        "context" => 1,
        _ => 0,
    }
}

/// How far back a hook run looks for its other log line: the lines of one run
/// are adjacent.
const HOOK_LOOKBACK: usize = 32;

/// A hook run that did something: `event:outcome`, with `#` and a hash of its
/// `toolUseID` when the harness gave one, never its output. A hook that
/// blocked a tool call carries that call's index. A plain success is not a
/// signal, which bounds the list to the runs that mattered. One run is logged
/// on more than one line (a blocking Stop hook and its summary, a success and
/// its added context, sharing a `toolUseID`): the lines fold into one signal
/// with the strongest outcome.
fn hook(
    summary: &mut FileIndex,
    event: Option<&str>,
    outcome: &str,
    tool: Option<&str>,
    o: u64,
    t: Option<i64>,
) {
    if outcome == "allowed" {
        return;
    }
    let event: String = event
        .and_then(bounded)
        .unwrap_or_else(|| "unknown".to_owned())
        .chars()
        .take(TAG_MAX - 24)
        .collect();
    let v = tool
        .filter(|_| outcome == "blocked")
        .and_then(|id| call_index(summary, id));
    let key = tool.map(|id| format!("#{}", short_hash(id)));
    if let Some(key) = &key {
        let head = format!("{event}:");
        let same = summary
            .signals
            .iter()
            .enumerate()
            .rev()
            .take(HOOK_LOOKBACK)
            .find(|(_, signal)| {
                signal.k == SignalKind::Hook
                    && signal
                        .n
                        .as_deref()
                        .is_some_and(|n| n.starts_with(&head) && n.ends_with(key.as_str()))
            })
            .map(|(index, _)| index);
        if let Some(signal) = same.and_then(|index| summary.signal_mut(index)) {
            let earlier = signal
                .n
                .as_deref()
                .and_then(|n| n.strip_prefix(&head))
                .and_then(|n| n.strip_suffix(key.as_str()))
                .unwrap_or("");
            if outcome_rank(outcome) > outcome_rank(earlier) {
                signal.n = Some(format!("{head}{outcome}{key}"));
                signal.v = signal.v.or(v);
            }
            return;
        }
    }
    let key = key.unwrap_or_default();
    note(
        summary,
        SignalKind::Hook,
        o,
        t,
        format!("{event}:{outcome}{key}"),
        v,
    );
}

/// A Claude Code hook attachment: what the hook did, by the harness's own type.
fn claude_hook(summary: &mut FileIndex, attachment: &Value, o: u64, t: Option<i64>) {
    let outcome = match field(attachment, "type") {
        Some("hook_success") => "allowed",
        Some("hook_blocking_error") => "blocked",
        Some("hook_non_blocking_error") => "error",
        Some("hook_additional_context") => "context",
        Some("hook_cancelled") => "cancelled",
        Some("hook_stopped_continuation") => "stopped",
        _ => return,
    };
    let event = field(attachment, "hookEvent")
        .or_else(|| field(attachment, "hookName").and_then(|name| name.split(':').next()));
    hook(
        summary,
        event,
        outcome,
        field(attachment, "toolUseID"),
        o,
        t,
    );
}

impl FileIndex {
    fn signal_mut(&mut self, index: usize) -> Option<&mut Signal> {
        if let Some(dirty) = &mut self.dirty_rows {
            dirty.signals.insert(index);
        }
        self.signals.get_mut(index)
    }

    fn event_mut(&mut self, index: usize) -> Option<&mut Event> {
        if let Some(dirty) = &mut self.dirty_rows {
            dirty.events.insert(index);
        }
        self.events.get_mut(index)
    }

    /// Claude usage by message id, for merging across a lineage's files.
    pub(crate) fn usage(&self) -> &BTreeMap<String, MessageUsage> {
        &self.usage_by_id
    }

    pub(crate) fn codex_usage(&self) -> &BTreeMap<String, ModelTokens> {
        self.codex_native_usage
            .as_ref()
            .map_or(&self.codex_tokens_by_model, |usage| &usage.by_model)
    }

    pub(crate) fn codex_usage_events(&self) -> &[CodexUsageEvent] {
        self.codex_native_usage
            .as_ref()
            .map_or(&self.codex_usage_events, |usage| &usage.events)
    }

    #[cfg(test)]
    pub(crate) fn native_usage_history_sizes(&self) -> Option<(usize, usize)> {
        self.codex_native_usage
            .as_ref()
            .map(|native| (native.seen.len(), native.events.len()))
    }

    pub(crate) fn codex_cost(&self) -> crate::pricing::CostBuild {
        if let Some(native) = &self.codex_native_usage
            && let Some(cost) = &native.cost
        {
            return cost.clone();
        }
        let mut cost = crate::pricing::CostBuild::default();
        for event in self.codex_usage_events() {
            cost.codex(event);
        }
        cost
    }

    pub(crate) fn tokens(&self, harness: &str) -> Tokens {
        if harness == "copilot" {
            return self
                .copilot
                .as_ref()
                .and_then(|index| index.usage.as_ref())
                .map_or_else(Tokens::default, |usage| usage.tokens.clone());
        }
        if harness == "codex" {
            return self
                .codex_native_usage
                .as_ref()
                .map_or_else(|| self.codex_tokens.clone(), |usage| usage.tokens.clone());
        }
        let mut total = Tokens::default();
        for usage in self.usage_by_id.values() {
            total.input += usage.tokens.input;
            total.cached_input += usage.tokens.cached_input;
            total.output += usage.tokens.output;
            total.total += usage.tokens.total;
        }
        total
    }
}

/// A file as last stat'ed: its identity, size and modified time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stat {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) size: u64,
    pub(crate) modified_ns: u128,
    pub(crate) changed_ns: Option<i128>,
}

impl Stat {
    pub(crate) fn of(path: &Path) -> io::Result<Self> {
        Self::from_metadata(&fs::metadata(path)?)
    }

    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> io::Result<Self> {
        let modified_ns = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        #[cfg(unix)]
        let (dev, ino) = (metadata.dev(), metadata.ino());
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);
        Ok(Self {
            dev,
            ino,
            size: metadata.len(),
            modified_ns,
            changed_ns: change_time_ns(metadata),
        })
    }
}

/// Filesystem change time fences metadata-identical rewrites without reading
/// source bodies. None means this platform/legacy observation cannot prove it.
pub(crate) fn change_time_ns(metadata: &fs::Metadata) -> Option<i128> {
    #[cfg(test)]
    if let Some(value) = FIXED_CHANGE_TIME.with(std::cell::Cell::get) {
        return Some(value);
    }
    #[cfg(unix)]
    {
        Some(i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

#[cfg(test)]
thread_local! {
    static FIXED_CHANGE_TIME: std::cell::Cell<Option<i128>> = const { std::cell::Cell::new(None) };
}

/// Model indistinguishable change stamps deterministically across filesystems.
#[cfg(test)]
pub(crate) fn fixed_change_time_for_test(value: i128) -> impl Drop {
    struct Restore(Option<i128>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FIXED_CHANGE_TIME.with(|time| time.set(self.0));
        }
    }
    Restore(FIXED_CHANGE_TIME.with(|time| time.replace(Some(value))))
}

/// How many bytes the ledger's tail hash covers before its offset.
const WINDOW: u64 = 4096;

#[cfg(test)]
thread_local! {
    pub(crate) static BEFORE_CATALOG_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static AFTER_CATALOG_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static CATALOG_DERIVED_SOURCES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static CATALOG_PUBLISHED_ROWS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static INDEX_WRITTEN_ROWS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static SOURCE_READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static SOURCE_HASHED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static HASHED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// What the index records about a file it has read: the stat it was read
/// at, the offset its complete lines end at, and SHA-256 hashes of the
/// complete consumed prefix and of the tail window before that offset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ledger {
    pub(crate) stat: Stat,
    pub(crate) offset: u64,
    pub(crate) prefix: [u8; 32],
    pub(crate) tail: [u8; 32],
}

impl Ledger {
    pub(crate) fn source(
        &self,
        path: &Path,
        native_id: &str,
    ) -> crate::model::summary::CatalogSource {
        crate::model::summary::CatalogSource {
            path: path.to_owned(),
            native_id: native_id.to_owned(),
            dev: self.stat.dev,
            ino: self.stat.ino,
            size: self.stat.size,
            modified_ns: self.stat.modified_ns,
            changed_ns: self.stat.changed_ns,
            offset: self.offset,
            prefix_sha256: self.prefix,
            tail_sha256: self.tail,
            native_role: None,
        }
    }

    /// The file still holds what was read, so reading resumes at the
    /// offset: the same identity, at least as long, and the consumed prefix
    /// unchanged. That covers a file that grew and one only touched; any
    /// other change (a rewrite, a truncation, a new inode) rereads it.
    pub(crate) fn resumes(&self, stat: &Stat, file: &mut LogFile) -> bool {
        self.stat.dev == stat.dev
            && self.stat.ino == stat.ino
            && self.offset <= stat.size
            && consumed_hashes(file, self.offset)
                .is_ok_and(|(prefix, tail)| prefix == self.prefix && tail == self.tail)
    }
}

/// Hash the complete consumed prefix and its tail window. Idle scans retain
/// the stat shortcut; changed files must verify all consumed bytes because
/// unchanged end windows cannot establish an unchanged interior.
pub(crate) fn consumed_hashes(file: &mut LogFile, offset: u64) -> io::Result<([u8; 32], [u8; 32])> {
    file.seek(SeekFrom::Start(0))?;
    let mut prefix = Sha256::new();
    let bytes = io::copy(&mut (&mut *file).take(offset), &mut prefix)?;
    #[cfg(test)]
    {
        HASHED_BYTES.with(|count| count.set(count.get() + bytes + offset.min(WINDOW)));
        SOURCE_HASHED_BYTES.with(|count| count.set(count.get() + bytes + offset.min(WINDOW)));
    }
    if bytes != offset {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "consumed prefix shortened",
        ));
    }
    let width = offset.min(WINDOW);
    let mut tail = vec![0; width as usize];
    file.seek(SeekFrom::Start(offset - width))?;
    file.read_exact(&mut tail)?;
    Ok((prefix.finalize().into(), Sha256::digest(&tail).into()))
}

/// Why a store call failed.
#[derive(Debug)]
pub(crate) enum StoreError {
    /// Explicit backend errors never detach a configured producer or fork it
    /// into an in-memory/unbound writer.
    Configured(crate::storage::StorageError),
    /// Another process held the store past its bounded wait. The configured
    /// producer propagates Busy without publishing an in-memory replacement.
    Busy(String),
    /// A stored row did not decode under the current contract.
    Data(String),
    /// The store is unavailable; a configured producer refuses the operation.
    Failed(String),
}

pub(crate) const SESSION_DESCRIPTION_BATCH_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionDescriptionRequest {
    pub(crate) key: String,
    pub(crate) version: u32,
    pub(crate) inputs: String,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configured(error) => std::fmt::Display::fmt(error, formatter),
            Self::Busy(message) | Self::Data(message) | Self::Failed(message) => {
                formatter.write_str(message)
            }
        }
    }
}

/// What a store write did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Written,
    /// The ledger row wasn't the one the change was based on: another
    /// process moved the file on first. Nothing was written.
    Conflict,
    /// A bounded original-custody checkpoint committed. Current publication and
    /// its durable source claims remain pending for the next producer step.
    Deferred,
    /// The captured store or parser contract is no longer current. Nothing
    /// was written.
    Stale,
}

/// Everything a store holds, as read when it opens.
#[derive(Default)]
pub(crate) struct Loaded {
    pub(crate) files: Vec<(String, Ledger, FileIndex)>,
    pub(crate) reported_runs: Vec<crate::facts::ReportedRunSnapshot>,
    pub(crate) stamp: Option<ReportedFileStamp>,
}

/// Validated Catalog search demand changes scheduling, never native identity.
pub(crate) struct CatalogSearchInterest<'a> {
    pub(crate) query: &'a str,
    pub(crate) harness: Option<&'a str>,
}

pub(crate) use crate::storage::source_observations::{
    CATALOG_SEARCH_LIMIT, CatalogPendingBatch, CatalogPendingSearch, CatalogPendingTier,
    CatalogSearchLookup,
};

/// Where the event index persists between runs: the narrow interface the
/// cache needs, so the storage behind it (SQLite today) stays in one place.
///
/// Every write is atomic, and every file's write is conditional on the
/// ledger row it was based on, so two processes sharing one store never
/// apply the same lines twice.
pub(crate) trait IndexStore: Send {
    fn prepare_facts_authority(&self, _options: &crate::Options) -> io::Result<()> {
        Ok(())
    }
    fn facts_authority(&self, path: &Path) -> io::Result<crate::facts::source_authority::Sources>;

    /// Backend-specific borrowed port; native SQLite retains its owner
    /// connection and committed discovery witness for the full producer call.
    fn source_observations(
        &self,
    ) -> Option<&dyn crate::storage::source_observations::SourceObservationStore> {
        None
    }

    fn source_observations_mut(
        &mut self,
    ) -> Option<&mut dyn crate::storage::source_observations::SourceObservationStore> {
        None
    }

    fn event_receipt(
        &self,
        _path: &str,
    ) -> Result<crate::storage::native_events::EventReceipt, StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::NamespaceUpgradePending,
        ))
    }

    fn reconcile_event(
        &self,
        _operation_id: &str,
        _path: &str,
        _harness: &str,
    ) -> Result<Option<crate::storage::native_events::EventReceipt>, StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::NamespaceUpgradePending,
        ))
    }

    /// Durable read/parse intent, before opening Raw; never a worker lease.
    fn prepare_file(
        &mut self,
        _path: &str,
        _expected: Option<&Ledger>,
        _operation_id: &str,
        _harness: &str,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    /// At most 64 compatible metadata-only descriptions, in request order. The
    /// model checks dependencies after a successful source scan, before lookup.
    fn session_descriptions(
        &self,
        requests: &[SessionDescriptionRequest],
    ) -> Result<Vec<Option<String>>, StoreError>;

    /// One complete description replaces its predecessor atomically.
    fn save_session_descriptions(
        &mut self,
        descriptions: &[(String, u32, String, String)],
    ) -> Result<Outcome, StoreError>;

    /// The committed catalog observed before a model build starts.
    fn session_catalog_generation(&self) -> Result<Option<String>, StoreError>;
    /// Acknowledge a bounded observation only while its exact publication is
    /// current and no source invalidation or semantic stage remains.
    fn note_catalog_audit(&mut self, expected: Option<&str>, now: i64) -> Result<bool, StoreError>;

    /// Existing scalar recipes at the exact source and catalog generations.
    /// Optional reuse never grants source access or changes publication CAS.
    fn session_slots(
        &self,
        row: &crate::model::summary::CatalogRow,
        expected_generation: Option<&str>,
        max: usize,
    ) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError>;

    /// Replaces one complete source-validated metadata catalog atomically.
    fn publish_session_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<(Outcome, Option<String>), StoreError>;

    fn publish_partial_catalog(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<(Outcome, Option<String>), StoreError>;
    fn publish_partial_catalog_with_model_witness(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
        _model_witness: &dyn Fn() -> io::Result<String>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.publish_partial_catalog(publication, expected_generation, claims)
    }
    fn semantic_origin_pending(&self) -> Result<bool, StoreError> {
        Ok(false)
    }
    fn publish_closed_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<Option<(Outcome, Option<String>)>, StoreError>;
    fn facts_write_begin(
        &mut self,
        _options: &crate::Options,
        _intent: &crate::source_change::FactsIntent,
    ) -> Result<(), StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::Unavailable,
        ))
    }
    fn facts_write_complete(
        &mut self,
        _options: &crate::Options,
        _intent: &crate::source_change::FactsIntent,
    ) -> Result<(), StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::Unavailable,
        ))
    }
    fn source_write_prepare(
        &self,
        _options: &crate::Options,
        _input: &crate::Input,
        _intent: &mut crate::source_change::WriteIntent,
    ) -> Result<(), StoreError> {
        Ok(())
    }
    fn source_write_proofs(&self) -> bool {
        false
    }
    fn source_write_begin(
        &mut self,
        _options: &crate::Options,
        _input: &crate::Input,
        _intent: &crate::source_change::WriteIntent,
    ) -> Result<(i64, Option<crate::source_change::AppendEvidence>), StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::Unavailable,
        ))
    }
    fn source_write_complete(
        &mut self,
        _options: &crate::Options,
        _input: &crate::Input,
        _intent: &crate::source_change::WriteIntent,
        _revision: i64,
        _completion: &crate::source_change::WriteCompletion,
    ) -> Result<(), StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::Unavailable,
        ))
    }
    fn resume_runtime_input(&mut self, _: &crate::Options, _: &str) -> Result<(), StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::Unavailable,
        ))
    }
    fn resolve_catalog_input(
        &mut self,
        path: &str,
        revision: i64,
        native: Option<(&str, &str)>,
        log: Option<&str>,
    ) -> Result<Outcome, StoreError>;
    fn relocate_codex(
        &mut self,
        path: &str,
        stat: &Stat,
        id: &str,
        file: &mut LogFile,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Result<Option<String>, StoreError>;
    fn append_evidence(
        &self,
        path: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError>;
    fn qualified_catalog(&self) -> Result<bool, StoreError>;
    fn catalog_component_paths(
        &self,
        paths: &[String],
        keys: &[String],
        native_parents: &[(String, String)],
    ) -> Result<Option<Vec<String>>, StoreError>;

    fn query_runtime_binding(
        &self,
    ) -> Result<Option<crate::storage::QueryRuntimeBinding>, StoreError> {
        Ok(None)
    }
    fn runtime_source_cwds(
        &self,
        _sources: &[crate::model::summary::CatalogSource],
    ) -> Result<std::collections::BTreeSet<String>, StoreError> {
        Err(StoreError::Configured(
            crate::storage::StorageError::Unavailable,
        ))
    }
    fn begin_semantic_stage(
        &mut self,
        paths: &[String],
        runtime: &str,
        journaled_runtime: bool,
    ) -> Result<Outcome, StoreError> {
        let _ = (paths, runtime, journaled_runtime);
        Err(StoreError::Data("semantic staging unavailable".into()))
    }
    fn semantic_batch(
        &mut self,
        limit: usize,
    ) -> Result<Option<crate::model::relation_stage::Batch>, StoreError> {
        let _ = limit;
        Err(StoreError::Data("semantic staging unavailable".into()))
    }
    fn record_semantic_batch(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
        let _ = (batch, prepared);
        Err(StoreError::Data("semantic staging unavailable".into()))
    }
    /// Constant, content-free reason from the last semantic write attempt.
    fn semantic_rejection(&self) -> Option<&'static str> {
        None
    }
    fn publish_semantic_catalog(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        let _ = (batch, prepared);
        Err(StoreError::Data("semantic staging unavailable".into()))
    }
    fn finish_semantic_stage(&mut self, epoch: i64) -> Result<Outcome, StoreError> {
        let _ = epoch;
        Err(StoreError::Data("semantic staging unavailable".into()))
    }

    fn semantic_select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        let _ = (epoch, selector);
        Err(StoreError::Data("semantic staging unavailable".into()))
    }

    fn semantic_replay_page(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<crate::model::relation_stage::Context, StoreError> {
        let _ = (epoch, owners, after);
        Err(StoreError::Data("semantic replay unavailable".into()))
    }

    /// `path`'s ledger as committed now.
    fn ledger(&self, path: &str) -> Result<Option<Ledger>, StoreError>;

    /// Reads up to 64 requested ledgers from one validated backend snapshot.
    /// The returned entries align with `paths`, including `None` for absent rows.
    fn ledger_batch(&self, paths: &[String]) -> Result<Vec<Option<Ledger>>, StoreError>;

    /// `path`'s committed ledger and index, read together.
    fn load_file(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError>;

    fn native_usage_seen(
        &self,
        _path: &str,
        _ledger: &Ledger,
        _identity: &str,
    ) -> Result<store::native_usage::Seen, StoreError> {
        Ok(store::native_usage::Seen::default())
    }

    fn advance_tree_repair(
        &mut self,
        _options: &crate::Options,
    ) -> io::Result<(bool, Option<String>)> {
        Ok((false, None))
    }
    fn tree_repair_pending(&self) -> Result<bool, StoreError> {
        Ok(false)
    }

    fn queue_tree_repair(
        &mut self,
        _path: &str,
        _ledger: &Ledger,
        _harness: &str,
    ) -> io::Result<()> {
        Err(io::Error::other(
            "native tree repair requires a durable store",
        ))
    }

    /// A committed tree summary and its evidence, without event/signal rows.
    fn load_tree(
        &self,
        path: &str,
        expected: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StoreError>;

    fn commit_initial_files(
        &mut self,
        files: &[(String, Ledger, Arc<FileIndex>)],
    ) -> Result<Outcome, StoreError>;

    /// Commits `path`'s new ledger and index if its ledger is still
    /// `expected`. `physical_stat` is the full file metadata captured before
    /// parsing; the persisted `ledger.stat` may name only a bounded prefix.
    /// `changes` names rows the append parser touched. Without it the file's
    /// rows are replaced; no previous index is needed.
    fn commit_file(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        changes: Option<&DirtyRows>,
        ledger: &Ledger,
        physical_stat: &Stat,
        index: &FileIndex,
    ) -> Result<Outcome, StoreError>;

    /// Exact native identities whose historical reported runs a scoped build needs.
    fn select_reported_runs(
        &mut self,
        ids: &BTreeSet<String>,
    ) -> Result<Vec<crate::facts::ReportedRunSnapshot>, StoreError>;

    fn file_ledgers(&self) -> Result<Vec<(String, Option<Ledger>)>, StoreError>;

    /// Drops files' rows. A file given with a ledger loses its rows only if
    /// they are still at that ledger, so a file this process failed to read
    /// once keeps the rows another process keeps up; one given without a
    /// ledger (its row didn't decode) loses them only if they still don't.
    fn remove_files(&mut self, files: &[(String, Option<Ledger>)]) -> Result<Outcome, StoreError>;

    /// Saves reported runs and the `~/.claude.json` stamp they were read at,
    /// and returns every reported run the store holds (other processes may
    /// have added some). `None` when the store is stale.
    fn save_runs(
        &mut self,
        runs: &[crate::facts::ReportedRunSnapshot],
        stamp: &ReportedFileStamp,
    ) -> Result<Option<Vec<crate::facts::ReportedRunSnapshot>>, StoreError>;
}

impl crate::storage::native_events::NativeEventStore for EventCache {
    fn ingest(
        &mut self,
        operation_id: &str,
        path: &Path,
        harness: &str,
    ) -> Result<crate::storage::native_events::EventReceipt, crate::storage::StorageError> {
        if !matches!(harness, "claude" | "codex" | "copilot") {
            return Err(crate::storage::StorageError::InvalidArguments);
        }
        self.require_current_store().map_err(|error| {
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<crate::storage::StorageError>())
                .copied()
                .unwrap_or(crate::storage::StorageError::Unavailable)
        })?;
        if let Some(receipt) = self
            .store
            .as_ref()
            .ok_or(crate::storage::StorageError::Unavailable)?
            .reconcile_event(
                operation_id,
                path.to_str()
                    .ok_or(crate::storage::StorageError::InvalidArguments)?,
                harness,
            )
            .map_err(configured_error)?
        {
            return Ok(receipt);
        }
        self.event_operation = Some(operation_id.to_owned());
        let mut dirty = false;
        let scanned = scan_file(path, harness, self, &mut dirty);
        // Operation identity is request-scoped. Clear it on both scan outcomes
        // before propagating an error or performing receipt reads.
        self.event_operation = None;
        scanned.map_err(|error| {
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<crate::storage::StorageError>())
                .copied()
                .unwrap_or(crate::storage::StorageError::Unavailable)
        })?;
        let key = path.to_string_lossy();
        if self.unpersisted.contains(key.as_ref()) {
            return Err(crate::storage::StorageError::Unavailable);
        }
        self.store
            .as_ref()
            .ok_or(crate::storage::StorageError::Unavailable)?
            .event_receipt(&key)
            .map_err(configured_error)
    }
}

fn store_io_error(error: StoreError) -> io::Error {
    match error {
        StoreError::Configured(error) => io::Error::other(error),
        other => io::Error::other(other.to_string()),
    }
}

fn configured_error(error: StoreError) -> crate::storage::StorageError {
    match error {
        StoreError::Configured(error) => error,
        StoreError::Busy(_) => crate::storage::StorageError::Busy,
        StoreError::Data(_) | StoreError::Failed(_) => crate::storage::StorageError::Unavailable,
    }
}

/// One file's index in memory, and the ledger it was read at.
struct CachedFile {
    ledger: Ledger,
    // A published scoped producer keeps source identity, not every transcript
    // payload. The existing durable index reloads a selected generation lazily.
    index: Option<Arc<FileIndex>>,
}

/// The admitted event index: selected [`FileIndex`] values in memory, written
/// through the explicitly owned [`IndexStore`].
#[derive(Default)]
pub(crate) struct EventCache {
    files: BTreeMap<String, CachedFile>,
    reported_runs: BTreeMap<String, BTreeMap<i64, crate::facts::ReportedRunSnapshot>>,
    claude_json_stamp: Option<ReportedFileStamp>,
    store: Option<Box<dyn IndexStore>>,
    source_observations:
        Option<Box<dyn crate::storage::source_observations::SourceObservationStore>>,
    observation_namespace: String,
    observation_path: Option<PathBuf>,
    pending_source_operation: Option<PendingSourceOperation>,
    reconciled_recovery: Option<bool>,
    deferred_source_operations: bool,
    scoped: bool,
    host_source: Option<crate::storage::HostSource>,
    event_operation: Option<String>,
    retention_base: Vec<(String, Option<Ledger>)>,
    /// Source fences for the admitted native producer.
    lock_cache: Option<PathBuf>,
    /// Cold ingestion advances complete records by a bounded byte slice.
    pub(crate) scan_byte_budget: Option<u64>,
    cold_writes: Vec<(String, Ledger, Arc<FileIndex>)>,
    atomic_cold: bool,
    /// The V1 cache the store sits beside, when this index has one.
    v1_cache: Option<PathBuf>,
    /// The store was busy in this refresh (reported runs, then the scan):
    /// its other writes wait for the next, so a refresh waits at most once.
    busy: bool,
    /// Gone files whose rows the store still holds: dropped when it can.
    gone: Vec<(String, Option<Ledger>)>,
    /// Time spent cloning a cached index during the current scan.
    index_clone: Duration,
    scan_timing: bool,
    parse_duration: Duration,
    index_write_duration: Duration,
    scan_measurements: crate::native_timing::Measurements,
    /// Files reread from byte zero this scan: offsets in cached text may
    /// now refer to different records, even if their new size is larger.
    replaced: BTreeSet<String>,
    /// Reported runs the store didn't take: saved at the next refresh.
    runs_unsaved: bool,
    descriptions_pending: Vec<(String, u32, String, String)>,
    /// Files whose change is in memory only (the store was busy, or other
    /// processes kept moving them on): read again at the next scan, which
    /// writes them through.
    unpersisted: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportedFileStamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified_ns: u128,
}

/// Only the named fields deserialize. Other account and project values are
/// consumed as ignored JSON and never enter the session model or facts.
#[derive(Deserialize)]
struct ClaudeJson {
    #[serde(default)]
    projects: BTreeMap<String, LastProjectRun>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastProjectRun {
    last_session_id: Option<String>,
    last_start_time: Option<i64>,
    last_cost: Option<f64>,
    last_duration: Option<u64>,
    last_api_duration: Option<u64>,
    last_tool_duration: Option<u64>,
    last_lines_added: Option<u64>,
    last_lines_removed: Option<u64>,
    last_model_usage: Option<BTreeMap<String, crate::facts::ReportedModelUsage>>,
}

impl LastProjectRun {
    fn snapshot(self, capture_at: i64) -> Option<crate::facts::ReportedRunSnapshot> {
        Some(crate::facts::ReportedRunSnapshot {
            last_session_id: self.last_session_id?,
            last_start_time: self.last_start_time?,
            last_cost: self.last_cost,
            last_duration: self.last_duration,
            last_api_duration: self.last_api_duration,
            last_tool_duration: self.last_tool_duration,
            last_lines_added: self.last_lines_added,
            last_lines_removed: self.last_lines_removed,
            last_model_usage: self.last_model_usage.unwrap_or_default(),
            capture_at,
        })
    }
}

// Reuse the existing allowlisted parser for a content-free runtime observation.
// Account/configuration fields never enter this value or persisted fingerprints.
pub(crate) fn current_reported_runs(path: &Path) -> Vec<crate::facts::ReportedRunSnapshot> {
    crate::read_regular_at_most(path, 64 * 1024 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ClaudeJson>(&bytes).ok())
        .map(|root| {
            root.projects
                .into_values()
                .filter_map(|project| project.snapshot(0))
                .collect()
        })
        .unwrap_or_default()
}

/// Where a file's unchanged index comes from.
enum Base {
    Found(Arc<FileIndex>),
    /// The store moved on between two reads: look again.
    Moved,
    /// Its rows can't be read: read the file from its start.
    Unreadable,
}

/// Exact typed payload shared by the producer and its durable backend journal.
type PendingSourceOperation = crate::storage::source_observations::SourceIntent;

impl EventCache {
    fn observation_store(
        &self,
    ) -> Option<&dyn crate::storage::source_observations::SourceObservationStore> {
        if let Some(store) = &self.source_observations {
            Some(store.as_ref())
        } else {
            self.store.as_ref()?.source_observations()
        }
    }

    fn observation_store_mut(
        &mut self,
    ) -> Option<&mut dyn crate::storage::source_observations::SourceObservationStore> {
        if let Some(ref mut store) = self.source_observations {
            Some(store.as_mut())
        } else {
            self.store.as_mut()?.source_observations_mut()
        }
    }

    /// Test-only binding of the real producer without a native IndexStore.
    /// This is not a public PG backend factory or qualification.
    #[cfg(all(test, feature = "postgres"))]
    pub(crate) fn source_observation_test_producer(
        namespace: &str,
        path: &Path,
        store: Box<dyn crate::storage::source_observations::SourceObservationStore>,
    ) -> io::Result<Self> {
        let mut cache = Self {
            observation_namespace: namespace.to_owned(),
            observation_path: Some(path.to_owned()),
            source_observations: Some(store),
            ..Self::default()
        };
        cache.reconcile_pending_source_operation()?;
        Ok(cache)
    }

    #[cfg(test)]
    pub(crate) fn inject_source_observations(
        &mut self,
        store: Box<dyn crate::storage::source_observations::SourceObservationStore>,
    ) {
        self.source_observations = Some(store);
    }

    #[cfg(test)]
    pub(crate) fn acknowledge_source_for_test(
        &mut self,
        path: &str,
    ) -> Result<Outcome, StoreError> {
        let observations = self
            .observation_store()
            .expect("test source observation store");
        let revision = observations
            .claims(&[path.to_owned()])
            .map_err(StoreError::Configured)?[0]
            .1
            .expect("test source claim");
        let store = self.store.as_mut().expect("test producer store");
        store.resolve_catalog_input(path, revision, None, None)
    }

    /// Reopen and replay the durable queue before capturing any new identity.
    /// A failed apply/ack leaves the original payload available to the next open.
    pub(crate) fn reconcile_pending_source_operation(&mut self) -> io::Result<()> {
        let pending_source_operation = self.pending_source_operation.clone();
        let mut recovered = self.reconciled_recovery;
        let Some(store) = self.observation_store_mut() else {
            return if pending_source_operation.is_none() {
                Ok(())
            } else {
                Err(io::Error::other("source observation producer unavailable"))
            };
        };
        if let Some(pending) = &pending_source_operation {
            store
                .persist_intent(pending)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        let pending = store
            .pending_intents()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let mut deferred = false;
        for intent in pending {
            if let PendingSourceOperation::Recover { operation, cursor } = &intent {
                // Keep later captured pages durable for a later API call. An
                // unconsumed result must never be overwritten by another page.
                if recovered.is_some() {
                    deferred = true;
                    continue;
                }
                let (_, done) = store
                    .recover(operation, cursor.as_deref())
                    .map_err(|error| io::Error::other(error.to_string()))?;
                recovered = Some(done);
            } else {
                intent
                    .apply(store)
                    .map_err(|error| io::Error::other(error.to_string()))?;
            }
            store
                .acknowledge_intent(intent.operation())
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        self.pending_source_operation = None;
        self.reconciled_recovery = recovered;
        self.deferred_source_operations = deferred;
        Ok(())
    }

    fn persist_source_operation(
        &mut self,
        intent: PendingSourceOperation,
    ) -> io::Result<crate::storage::source_observations::SourceOperation> {
        let operation = intent.operation().clone();
        // Keep this copy even if committing the intent itself is uncertain.
        self.pending_source_operation = Some(intent.clone());
        self.observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .persist_intent(&intent)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(operation)
    }

    fn capture_discover_operation(
        &mut self,
        paths: &[String],
        complete: bool,
    ) -> io::Result<crate::storage::source_observations::SourceOperation> {
        self.reconcile_pending_source_operation()?;
        let payload = serde_json::to_vec(&(paths, complete)).map_err(io::Error::other)?;
        let operation = crate::storage::source_observations::SourceOperation::capture(
            &self.observation_namespace,
            "discover",
            &payload,
        );
        self.persist_source_operation(PendingSourceOperation::Discover {
            operation,
            paths: paths.to_vec(),
            complete,
        })
    }

    fn capture_finish_backfill_operation(
        &mut self,
        expected_generation: Option<&str>,
    ) -> io::Result<crate::storage::source_observations::SourceOperation> {
        self.reconcile_pending_source_operation()?;
        let payload = serde_json::to_vec(&expected_generation).map_err(io::Error::other)?;
        let operation = crate::storage::source_observations::SourceOperation::capture(
            &self.observation_namespace,
            "finish_backfill",
            &payload,
        );
        self.persist_source_operation(PendingSourceOperation::FinishBackfill {
            operation,
            expected_generation: expected_generation.map(str::to_owned),
        })
    }

    fn finish_source_operation(
        &mut self,
        operation: &crate::storage::source_observations::SourceOperation,
    ) -> io::Result<()> {
        self.observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .acknowledge_intent(operation)
            .map_err(|error| io::Error::other(error.to_string()))?;
        if self
            .pending_source_operation
            .as_ref()
            .is_some_and(|pending| pending.operation().id == operation.id)
        {
            self.pending_source_operation = None;
        }
        Ok(())
    }

    pub(crate) fn prepare_facts_authority(&self, options: &crate::Options) -> io::Result<()> {
        self.store
            .as_ref()
            .ok_or_else(|| io::Error::other("Facts authority store unavailable"))?
            .prepare_facts_authority(options)
    }
    pub(crate) fn facts_authority(
        &self,
        path: &Path,
    ) -> io::Result<crate::facts::source_authority::Sources> {
        self.store
            .as_ref()
            .ok_or_else(|| io::Error::other("Facts authority store unavailable"))?
            .facts_authority(path)
    }
    pub(crate) fn query_runtime_binding(
        &self,
    ) -> io::Result<Option<crate::storage::QueryRuntimeBinding>> {
        self.store
            .as_ref()
            .map(|store| store.query_runtime_binding())
            .transpose()
            .map(Option::flatten)
            .map_err(|error| io::Error::other(error.to_string()))
    }
    pub(crate) fn runtime_source_cwds(
        &self,
        sources: &[crate::model::summary::CatalogSource],
    ) -> io::Result<std::collections::BTreeSet<String>> {
        self.store
            .as_ref()
            .ok_or_else(|| io::Error::other("runtime metadata store unavailable"))?
            .runtime_source_cwds(sources)
            .map_err(|error| io::Error::other(error.to_string()))
    }
    #[allow(dead_code)]
    pub(crate) fn open_semantic(
        capability: crate::storage::sqlite::semantic::SemanticStaging,
    ) -> Result<Self, crate::storage::StorageError> {
        let source_journal = capability.journal();
        let observation_namespace = source_journal.namespace().to_owned();
        let observation_path = Some(source_journal.database_path().clone());
        let (store, claude_json_stamp) =
            crate::storage::sqlite::configured_index::open_semantic_with_stamp(capability)?;
        let mut cache = Self {
            store: Some(store),
            claude_json_stamp,
            source_observations: Some(Box::new(source_journal)),
            observation_namespace,
            observation_path,
            scoped: true,
            ..Self::default()
        };
        cache
            .reconcile_pending_source_operation()
            .map_err(|_| crate::storage::StorageError::Unavailable)?;
        Ok(cache)
    }
    pub(crate) fn open_configured(
        config: &crate::storage::StoreConfig,
    ) -> Result<Self, crate::storage::StorageError> {
        #[cfg(feature = "postgres")]
        if let crate::storage::StoreBackend::Postgres { pool, control } = &config.backend {
            crate::storage::postgres::require_configured(config, pool, control)?;
            let pool = pool.bind_source(
                &config.namespace,
                config.expected_stream_epoch.as_deref(),
                control,
            )?;
            return Self::open_pg_native(pool, config.namespace.clone(), control.clone());
        }
        let store = crate::storage::sqlite::open_configured_index(config)?;
        let database = config.backend.sqlite_database()?;
        let mut cache = Self {
            store: Some(store),
            source_observations: Some(crate::storage::sqlite::source_journal::configured(config)),
            observation_namespace: config.namespace.as_str().to_owned(),
            observation_path: Some(database.clone()),
            scoped: true,
            ..Self::default()
        };
        cache
            .reconcile_pending_source_operation()
            .map_err(|_| crate::storage::StorageError::Unavailable)?;
        Ok(cache)
    }
    /// Explicit authorized PG native capability. No public backend readiness,
    /// SQLite coordinates, Options-derived path or alternate-engine fallback.
    #[cfg(feature = "postgres")]
    #[allow(dead_code)] // Private integration until complete backend qualification.
    pub(crate) fn open_pg_native(
        pool: crate::storage::postgres::PgPool,
        namespace: crate::storage::LogicalSourceNamespace,
        control: crate::storage::postgres::PgCancellation,
    ) -> Result<Self, crate::storage::StorageError> {
        let observation_namespace = namespace.as_str().to_owned();
        let (store, claude_json_stamp) =
            crate::storage::postgres::index_store::open(pool, namespace, control)?;
        let mut cache = Self {
            store: Some(store),
            claude_json_stamp,
            observation_namespace,
            scoped: true,
            ..Self::default()
        };
        cache
            .reconcile_pending_source_operation()
            .map_err(|_| crate::storage::StorageError::Unavailable)?;
        Ok(cache)
    }
    /// A read-only request-local cache of selected source generations. The
    /// caller keeps catalog metadata and these ledgers in one SQLite snapshot.
    /// No store is attached, so native projection cannot mutate global rows.
    pub(crate) fn from_selected(
        connection: &rusqlite::Connection,
        sources: &[crate::model::summary::CatalogSource],
        native_ids: &BTreeSet<String>,
    ) -> rusqlite::Result<Self> {
        store::read_selected(connection, sources, native_ids).map(Self::from_loaded)
    }

    pub(crate) fn selected_index(&self, path: &Path) -> Option<Arc<FileIndex>> {
        self.files
            .get(path.to_str()?)
            .and_then(|file| file.index.as_ref().map(Arc::clone))
    }

    /// Current ownership needs ledger and independent role evidence only.
    /// Keep this inside the caller's Catalog snapshot without loading events,
    /// transcript slots, byte-proof pages or unrelated source metadata.
    pub(crate) fn selected_roles_current(
        connection: &rusqlite::Connection,
        sources: &[crate::model::summary::CatalogSource],
    ) -> rusqlite::Result<bool> {
        store::selected_roles_current(connection, sources)
    }

    /// The shared Session Event Index. Preserve the configured cache stem:
    /// `sessions-index.json` gives `sessions-index.sqlite3`. The retired tree
    /// JSON file is left untouched; tree and focused reads use this store.
    pub(crate) fn path(v1_cache: &Path) -> PathBuf {
        v1_cache.with_extension("sqlite3")
    }

    /// Opens the durable index beside `v1_cache`. An unavailable or refused
    /// store is an error; admission failures must never become an empty cache.
    pub(crate) fn open(
        v1_cache: &Path,
        source: &crate::storage::HostSource,
    ) -> Result<Self, String> {
        if let Some(config) = source.selected_store() {
            return Self::open_configured(config).map_err(|error| error.to_string());
        }
        #[cfg(test)]
        CACHE_READS.with(|reads| reads.set(reads.get() + 1));
        let path = Self::path(v1_cache);
        let mut cache = match store::open(&path, source) {
            Ok((store, loaded, journal)) => {
                let mut cache = Self::from_loaded(loaded);
                cache.store = Some(store);
                cache.source_observations = Some(Box::new(journal));
                cache.observation_path = Some(path.clone());
                cache
            }
            Err(error) => return Err(error.to_string()),
        };
        cache.v1_cache = Some(v1_cache.to_owned());
        cache.host_source = Some(source.clone());
        cache.scoped = true;
        cache.observation_namespace = source
            .namespace()
            .map_err(|error| error.to_string())?
            .as_str()
            .to_owned();
        cache
            .reconcile_pending_source_operation()
            .map_err(|error| error.to_string())?;
        Ok(cache)
    }

    /// Writable producer cache with lazy, exact-path FileIndex loads. Never falls
    /// back to a full restore or an in-memory producer after an open failure.
    pub(crate) fn open_scoped(
        v1_cache: &Path,
        source: &crate::storage::HostSource,
    ) -> Result<Self, String> {
        if let Some(config) = source.selected_store() {
            return Self::open_configured(config).map_err(|error| error.to_string());
        }
        let (store, loaded, journal) = store::open_scoped(&Self::path(v1_cache), source)?;
        let mut cache = Self::from_loaded(loaded);
        cache.store = Some(store);
        // Recovery/discovery use the admitted namespace journal, never the
        // store's unscoped physical one.
        cache.source_observations = Some(Box::new(journal));
        cache.observation_path = Some(Self::path(v1_cache));
        cache.scoped = true;
        cache.observation_namespace = source
            .namespace()
            .map_err(|error| error.to_string())?
            .as_str()
            .to_owned();
        cache.lock_cache = Some(v1_cache.to_owned());
        cache.host_source = Some(source.clone());
        cache
            .reconcile_pending_source_operation()
            .map_err(|error| error.to_string())?;
        Ok(cache)
    }

    /// Only the complete native background scan owns disappearance
    /// reconciliation. Capture ledger metadata before discovery so a concurrent
    /// newly committed source cannot be mistaken for a disappeared source.
    pub(crate) fn begin_full_scan(&mut self) -> io::Result<()> {
        if self.scoped {
            self.retention_base = self
                .store
                .as_ref()
                .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
                .file_ledgers()
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn scoped_store_lost(&self) -> bool {
        self.scoped && self.store.is_none()
    }

    pub(crate) fn is_scoped(&self) -> bool {
        self.scoped
    }

    pub(crate) fn select_reported_runs(&mut self, ids: &BTreeSet<String>) -> io::Result<()> {
        let runs = self
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .select_reported_runs(ids)
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.set_reported_runs(runs);
        Ok(())
    }

    pub(crate) fn resolve_catalog_input(
        &mut self,
        options: &crate::Options,
        path: &str,
    ) -> io::Result<()> {
        let input = crate::native_evidence::source_input(Path::new(path), options)
            .ok_or_else(|| io::Error::other("input journal outside allowlist"))?;
        let _lock = crate::source_change::SourceLock::acquire(
            options,
            &crate::source_change::lock_key(options, &input),
            false,
        )?;
        if options.facts.is_some() {
            self.store
                .as_mut()
                .ok_or_else(|| io::Error::other("native producer unavailable"))?
                .resume_runtime_input(options, path)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        let revision = self.catalog_claims(&[path.to_owned()])?[0]
            .1
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "input intent changed"))?;
        let log = input.path.ends_with(".meta.json").then(|| {
            input
                .full_path(options)
                .with_extension("")
                .with_extension("jsonl")
                .to_string_lossy()
                .into_owned()
        });
        let native =
            if input.root == crate::InputRoot::Claude && input.path.starts_with("sessions/") {
                let value = crate::open_input(&input.full_path(options))
                    .ok()
                    .and_then(|file| {
                        let mut bytes = Vec::new();
                        file.take(65537).read_to_end(&mut bytes).ok()?;
                        (bytes.len() <= 65536)
                            .then(|| serde_json::from_slice::<Value>(&bytes).ok())
                            .flatten()
                    });
                value.and_then(|value| value["sessionId"].as_str().map(str::to_owned))
            } else {
                None
            };
        match self
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other("native producer unavailable"))?
            .resolve_catalog_input(
                path,
                revision,
                native.as_deref().map(|id| ("claude", id)),
                log.as_deref(),
            ) {
            Ok(Outcome::Written) => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "input publication changed",
            )),
            Err(error) => Err(io::Error::other(error.to_string())),
        }
    }

    pub(crate) fn backfill_state(&self) -> io::Result<Option<String>> {
        self.observation_store()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .backfill_state()
            .map_err(|error| io::Error::other(error.to_string()))
    }
    pub(crate) fn discover_recovery_sources(&mut self) -> io::Result<bool> {
        // Test-only static step labels; never values, paths or SQL.
        fn step(step: &'static str) -> impl Fn(crate::storage::StorageError) -> io::Error {
            move |error| {
                #[cfg(test)]
                eprintln!("source recovery step={step} error={error:?}");
                #[cfg(not(test))]
                let _ = step;
                io::Error::other(error.to_string())
            }
        }
        // A database path is an optional SQLite timing coordinate, never the
        // identity or admission requirement of a logical Source journal.
        let _probe = self
            .observation_path
            .as_deref()
            .map(|path| crate::native_timing::DiscoveryProbe::new(path, "recovery"));
        // A failed/unknown ACK retains the exact identity before any new capture.
        if self.reconciled_recovery.is_none()
            && (self.pending_source_operation.is_some() || self.deferred_source_operations)
        {
            self.reconcile_pending_source_operation().inspect_err(|_| {
                #[cfg(test)]
                eprintln!("source recovery step=reconcile");
            })?;
        }
        if let Some(done) = self.reconciled_recovery.take() {
            return Ok(done);
        }
        let store = self
            .observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?;
        if store.recovery_is_current().map_err(step("is_current"))? {
            return Ok(true);
        }
        let cursor = store.recovery_cursor().map_err(step("cursor"))?;
        let operation = crate::storage::source_observations::SourceOperation::capture(
            &self.observation_namespace,
            "recover",
            &serde_json::to_vec(&cursor).map_err(io::Error::other)?,
        );
        let operation = self
            .persist_source_operation(PendingSourceOperation::Recover {
                operation,
                cursor: cursor.clone(),
            })
            .inspect_err(|_| {
                #[cfg(test)]
                eprintln!("source recovery step=intent");
            })?;
        let (_, done) = self
            .observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .recover(&operation, cursor.as_deref())
            .map_err(step("recover"))?;
        // The native port has already committed its matching ACK with Done.
        self.finish_source_operation(&operation).inspect_err(|_| {
            #[cfg(test)]
            eprintln!("source recovery step=ack");
        })?;
        Ok(done)
    }
    pub(crate) fn discover_catalog_sources(
        &mut self,
        options: &crate::Options,
        inputs: &[crate::Input],
        complete: bool,
    ) -> io::Result<()> {
        let observation_path = self.observation_path.clone();
        let mut probe = observation_path
            .as_deref()
            .map(|path| crate::native_timing::DiscoveryProbe::new(path, "publication"));
        let timing = crate::native_timing::enabled();
        let mut measurements = crate::native_timing::Measurements::new(timing);
        let paths = inputs
            .iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .map(|input| input.full_path(options).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if self
            .observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .discovery_is_current(&paths, complete)
            .map_err(|error| io::Error::other(error.to_string()))?
        {
            return Ok(());
        }
        let operation = self.capture_discover_operation(&paths, complete)?;
        let started = measurements.start();
        let thread_cpu_started_ns = crate::native_timing::thread_cpu_time_ns(timing);
        let result = self
            .observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .discover(&operation, &paths, complete);
        let receipt = match result {
            Ok(receipt) => {
                self.finish_source_operation(&operation)?;
                receipt
            }
            Err(error) => {
                return Err(io::Error::other(error.to_string()));
            }
        };
        let thread_cpu_ended_ns = crate::native_timing::thread_cpu_time_ns(timing);
        measurements.record_cpu(
            "source_journal_inclusive",
            thread_cpu_started_ns,
            thread_cpu_ended_ns,
            paths.len(),
        );
        measurements.record("source_journal_inclusive", started, paths.len());
        if timing && let Some(observation_path) = observation_path.as_deref() {
            // This measures the actual breakdown emit call; source-journal, probe,
            // and phase log writes remain in their enclosing wall/CPU scopes.
            let diagnostic_emit_started = std::time::Instant::now();
            measurements.emit(
                &crate::native_timing::id(observation_path),
                "discovery_publication_work",
            );
            if let Some(probe) = &mut probe {
                probe.record_diagnostic_emit(diagnostic_emit_started);
            }
        }
        match receipt {
            crate::storage::source_observations::SourceMutationReceipt::Applied
            | crate::storage::source_observations::SourceMutationReceipt::AlreadyApplied => Ok(()),
            crate::storage::source_observations::SourceMutationReceipt::Conflict => {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "source observation operation changed",
                ))
            }
        }
    }
    pub(crate) fn finish_catalog_backfill(&mut self, generation: &str) -> io::Result<Outcome> {
        let expected_generation = Some(generation);
        let operation = self.capture_finish_backfill_operation(expected_generation)?;
        let result = self
            .observation_store_mut()
            .ok_or_else(|| io::Error::other("source observation producer unavailable"))?
            .finish_backfill(&operation, expected_generation);
        let receipt = match result {
            Ok(receipt) => {
                self.finish_source_operation(&operation)?;
                receipt
            }
            Err(error) => {
                return Err(io::Error::other(error.to_string()));
            }
        };
        match receipt {
            crate::storage::source_observations::SourceMutationReceipt::Applied
            | crate::storage::source_observations::SourceMutationReceipt::AlreadyApplied => {
                Ok(Outcome::Written)
            }
            crate::storage::source_observations::SourceMutationReceipt::Conflict => {
                Ok(Outcome::Conflict)
            }
        }
    }

    pub(crate) fn retire_scoped_sources(
        &mut self,
        options: &crate::Options,
        inputs: &[crate::Input],
        seen: &BTreeSet<String>,
        machine: &crate::facts::MachineFacts,
    ) -> io::Result<()> {
        for input in inputs {
            let path = input.full_path(options);
            let key = path.to_string_lossy().into_owned();
            if seen.contains(&key) {
                continue;
            }
            let absent = fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
            let deselected = input.root == crate::InputRoot::Codex
                && !machine.codex_rollout_is_current(options, &path);
            if !absent && !deselected {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "selected source cannot be qualified",
                ));
            }
            let recorded = self.recorded(&key)?;
            if recorded.is_some() {
                match self
                    .store
                    .as_mut()
                    .ok_or_else(|| io::Error::other("native producer unavailable"))?
                    .remove_files(&[(key.clone(), recorded)])
                {
                    Ok(Outcome::Written) => {
                        self.files.remove(&key);
                    }
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "source disappearance changed",
                        ));
                    }
                    Err(error) => return Err(io::Error::other(error.to_string())),
                }
            }
        }
        Ok(())
    }

    pub(crate) fn pending_catalog_sources(&self, limit: usize) -> io::Result<Vec<String>> {
        self.prioritized_catalog_sources(limit, None)
            .map(|batch| batch.paths)
    }

    pub(crate) fn prioritized_catalog_sources(
        &self,
        limit: usize,
        search: Option<&CatalogPendingSearch<'_>>,
    ) -> io::Result<CatalogPendingBatch> {
        self.observation_store()
            .ok_or_else(|| io::Error::other("source journal unavailable"))?
            .pending_catalog_sources(limit, search)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn catalog_search_ranges(
        options: &crate::Options,
        harness: Option<&str>,
    ) -> [(String, String); 3] {
        [
            crate::InputRoot::Claude,
            crate::InputRoot::Codex,
            crate::InputRoot::Copilot,
        ]
        .map(|root| {
            if harness.is_some_and(|harness| harness != root.as_str()) {
                return (String::new(), String::new());
            }
            let home = root.home(options).to_string_lossy();
            let home = home.trim_end_matches('/');
            (format!("{home}/"), format!("{home}0"))
        })
    }

    fn qualified_catalog_search_inputs(
        options: &crate::Options,
        paths: &[String],
        harness: Option<&str>,
        limit: usize,
    ) -> Option<Vec<crate::Input>> {
        let inputs: Vec<_> = paths
            .iter()
            .filter_map(|path| {
                crate::native_evidence::catalog_input(Path::new(path), options)
                    .filter(|input| harness.is_none_or(|harness| input.root.as_str() == harness))
            })
            .collect();
        if paths.len() == limit.min(CATALOG_SEARCH_LIMIT) && inputs.len() < paths.len() {
            return None;
        }
        Some(inputs)
    }

    pub(crate) fn catalog_search_sources(
        snapshot: &dyn crate::storage::catalog::CatalogSnapshot,
        options: &crate::Options,
        interest: CatalogSearchInterest<'_>,
        limit: usize,
    ) -> io::Result<Vec<String>> {
        // The shortcut is ASCII only; Catalog's Unicode substring semantics
        // remain independent and all other identities continue through FIFO.
        if !interest.query.is_ascii() {
            return Ok(Vec::new());
        }
        let ranges = Self::catalog_search_ranges(options, interest.harness);
        let lookup = CatalogSearchLookup {
            query: interest.query,
            ranges: &ranges,
        };
        let paths = snapshot.catalog_search_paths(&lookup, limit)?;
        Ok(
            Self::qualified_catalog_search_inputs(options, &paths, interest.harness, limit)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Catalog search candidates exceed qualified source bounds",
                    )
                })?
                .into_iter()
                .map(|input| input.full_path(options).to_string_lossy().into_owned())
                .collect(),
        )
    }

    pub(crate) fn catalog_pending_inputs(
        options: &crate::Options,
        mut batch: CatalogPendingBatch,
        interest: Option<CatalogSearchInterest<'_>>,
    ) -> io::Result<(CatalogPendingTier, Vec<crate::Input>, Option<Vec<String>>)> {
        let mut checked = batch.search_checked.then(Vec::new);
        if batch.tier == CatalogPendingTier::SearchHint {
            let inputs = Self::qualified_catalog_search_inputs(
                options,
                &batch.paths,
                interest.as_ref().and_then(|interest| interest.harness),
                CATALOG_SEARCH_LIMIT,
            );
            if let Some(inputs) = inputs
                && !inputs.is_empty()
            {
                checked = Some(
                    inputs
                        .iter()
                        .map(|input| input.full_path(options).to_string_lossy().into_owned())
                        .collect(),
                );
                return Ok((batch.tier, inputs, checked));
            }
            // A physical identity match does not grant source authority. A
            // rejected hint must still allow the ordinary producer to drain.
            batch.paths = batch.fallback;
            batch.tier = CatalogPendingTier::Fifo;
        }
        if let Some(path) = batch.paths.iter().find(|path| path.ends_with(".json")) {
            // The selected tier may have changed while qualifying its hint.
            // Return a whole JSON intent for resolution before native mapping.
            let input = crate::native_evidence::source_input(Path::new(path), options)
                .ok_or_else(|| io::Error::other("input journal outside allowlist"))?;
            return Ok((batch.tier, vec![input], checked));
        }
        let inputs = batch
            .paths
            .into_iter()
            .map(|path| {
                crate::native_evidence::catalog_input(Path::new(&path), options).ok_or_else(|| {
                    io::Error::other("Catalog source is outside the native allowlist")
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok((batch.tier, inputs, checked))
    }

    pub(crate) fn qualified_catalog(&self) -> io::Result<bool> {
        self.store
            .as_ref()
            .ok_or_else(|| io::Error::other("native producer unavailable"))?
            .qualified_catalog()
            .inspect_err(|_| {
                #[cfg(test)]
                eprintln!("native catalog phase=qualification");
            })
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn catalog_claims(
        &self,
        paths: &[String],
    ) -> io::Result<Vec<(String, Option<i64>)>> {
        self.observation_store()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .claims(paths)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn catalog_component_paths(
        &self,
        inputs: &[crate::inputs::Input],
        keys: &[String],
        native_parents: &[(String, String)],
        options: &crate::Options,
    ) -> io::Result<Option<Vec<crate::inputs::Input>>> {
        let paths = inputs
            .iter()
            .map(|input| input.full_path(options).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let paths = self
            .store
            .as_ref()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .catalog_component_paths(&paths, keys, native_parents)
            .map_err(|error| io::Error::other(error.to_string()))?;
        paths
            .map(|paths| {
                paths
                    .into_iter()
                    .map(|path| {
                        crate::native_evidence::catalog_input(Path::new(&path), options).ok_or_else(
                            || {
                                io::Error::other(
                                    "Catalog component source is outside the native allowlist",
                                )
                            },
                        )
                    })
                    .collect()
            })
            .transpose()
    }

    pub(crate) fn semantic_select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> io::Result<crate::model::relation_stage::Selection> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .semantic_select(epoch, selector)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn semantic_replay_page(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> io::Result<crate::model::relation_stage::Context> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .semantic_replay_page(epoch, owners, after)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn begin_semantic_stage(
        &mut self,
        paths: &[String],
        runtime: &str,
    ) -> io::Result<Outcome> {
        self.begin_semantic_stage_with_journal(paths, runtime, false)
    }

    pub(crate) fn begin_semantic_stage_with_journal(
        &mut self,
        paths: &[String],
        runtime: &str,
        journaled_runtime: bool,
    ) -> io::Result<Outcome> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .begin_semantic_stage(paths, runtime, journaled_runtime)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn semantic_batch(
        &mut self,
        limit: usize,
    ) -> io::Result<Option<crate::model::relation_stage::Batch>> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .semantic_batch(limit)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn record_semantic_batch(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> io::Result<Outcome> {
        if self.busy || !self.unpersisted.is_empty() {
            return Err(io::Error::other(
                "native producer sources not durably observed",
            ));
        }
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .record_semantic_batch(batch, prepared)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn semantic_rejection(&self) -> Option<&'static str> {
        self.store
            .as_deref()
            .and_then(|store| store.semantic_rejection())
    }

    /// Prepare with the shared native algorithm, then persist its exact staged
    /// publication witness before exposing the payload to the publisher.
    #[allow(dead_code)] // Private admitted runtime; public qualification is zero.
    pub(crate) fn prepare_semantic_model_batch(
        &mut self,
        options: &crate::Options,
        inputs: &[crate::Input],
        batch: &crate::model::relation_stage::Batch,
        now: i64,
    ) -> io::Result<crate::model::SemanticModelBatch> {
        let model = crate::model::prepare_semantic_batch(options, self, inputs, batch, now)?;
        if self.record_semantic_batch(batch, model.prepared())? != Outcome::Written {
            return Err(io::Error::other("semantic model stage changed"));
        }
        Ok(model)
    }

    pub(crate) fn publish_semantic_catalog(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> io::Result<(Outcome, Option<String>)> {
        if self.busy || !self.unpersisted.is_empty() {
            return Err(io::Error::other(
                "native producer sources not durably observed",
            ));
        }
        let result = self
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .publish_semantic_catalog(batch, prepared)
            .map_err(|error| io::Error::other(error.to_string()))?;
        #[cfg(test)]
        if result.0 == Outcome::Written {
            CATALOG_PUBLISHED_ROWS
                .with(|count| count.set(count.get() + prepared.rows.len() as u64));
            if let Some(hook) = AFTER_CATALOG_PUBLICATION.with(|hook| hook.borrow_mut().take()) {
                hook();
            }
        }
        Ok(result)
    }

    pub(crate) fn finish_semantic_stage(&mut self, epoch: i64) -> io::Result<Outcome> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .finish_semantic_stage(epoch)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    #[cfg(test)]
    pub(crate) fn publish_partial_catalog(
        &mut self,
        prepared: &crate::model::PreparedSources,
    ) -> io::Result<Outcome> {
        self.publish_partial_catalog_fenced(prepared)
            .map(|(outcome, _)| outcome)
    }

    /// Return this transaction's generation, rather than reading a generation
    /// another producer may already have replaced after the commit.
    /// An exact retry retains the same PreparedSources, including its original
    /// base head, claims, rows, slot projections and presentation observations.
    /// Rebuilding the model is a new request and cannot consume an old receipt.
    pub(crate) fn publish_partial_catalog_fenced(
        &mut self,
        prepared: &crate::model::PreparedSources,
    ) -> io::Result<(Outcome, Option<String>)> {
        if self.busy || !self.unpersisted.is_empty() {
            return Err(io::Error::other(
                "native producer sources not durably observed",
            ));
        }
        #[cfg(test)]
        if let Some(hook) = BEFORE_CATALOG_PUBLICATION.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        let model_witness = || prepared.staged_model_witness();
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .publish_partial_catalog_with_model_witness(
                &crate::slot_projection::Publication {
                    rows: &prepared.rows,
                    transcripts: Some(&prepared.transcripts),
                    observations: prepared.presentation_observations(),
                },
                prepared.base_generation.as_deref(),
                &prepared.claims,
                &model_witness,
            )
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn semantic_origin_pending(&self) -> io::Result<bool> {
        self.store
            .as_ref()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .semantic_origin_pending()
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn publish_closed_catalog(
        &mut self,
        prepared: &crate::model::PreparedSources,
    ) -> io::Result<Option<Outcome>> {
        self.publish_closed_catalog_fenced(prepared)
            .map(|result| result.map(|(outcome, _)| outcome))
    }

    /// Carry the committed generation through the source-closure check.
    pub(crate) fn publish_closed_catalog_fenced(
        &mut self,
        prepared: &crate::model::PreparedSources,
    ) -> io::Result<Option<(Outcome, Option<String>)>> {
        if self.busy || !self.unpersisted.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "native producer sources not durably observed",
            ));
        }
        #[cfg(test)]
        if let Some(hook) = BEFORE_CATALOG_PUBLICATION.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        let outcome = self
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .publish_closed_catalog_fenced(
                &crate::slot_projection::Publication {
                    rows: &prepared.rows,
                    transcripts: Some(&prepared.transcripts),
                    observations: prepared.presentation_observations(),
                },
                prepared.base_generation.as_deref(),
                &prepared.claims,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
        #[cfg(test)]
        if matches!(outcome, Some((Outcome::Written, _))) {
            CATALOG_PUBLISHED_ROWS
                .with(|count| count.set(count.get() + prepared.rows.len() as u64));
            if let Some(hook) = AFTER_CATALOG_PUBLICATION.with(|hook| hook.borrow_mut().take()) {
                hook();
            }
        }
        Ok(outcome)
    }

    /// Ends a scan, and with it the refresh: the store gets its writes again.
    pub(crate) fn end_scan(&mut self) {
        self.busy = false;
    }

    /// Drop only durably committed producer payloads after Catalog publication.
    /// Ledgers and membership remain available to incremental change detection
    /// and disappearance reconciliation. In-memory-only/busy evidence stays.
    #[cfg(test)]
    pub(crate) fn resident_index_payloads(&self) -> usize {
        self.files
            .values()
            .filter(|file| file.index.is_some())
            .count()
    }

    pub(crate) fn release_published_indexes(&mut self) {
        if self.scoped && self.store.is_some() && !self.busy {
            for (path, file) in &mut self.files {
                if !self.unpersisted.contains(path) {
                    file.index = None;
                }
            }
        }
    }

    /// Time spent cloning cached file indexes in the current scan.
    pub(crate) fn index_clone_duration(&self) -> Duration {
        self.index_clone
    }

    pub(crate) fn scan_durations(&self) -> (Duration, Duration) {
        (self.parse_duration, self.index_write_duration)
    }

    pub(crate) fn emit_scan_measurements(&self, producer: &str) {
        self.scan_measurements.emit(producer, "scan");
    }

    pub(crate) fn revision(&self, path: &Path) -> Option<FileRevision> {
        self.files
            .get(path.to_string_lossy().as_ref())
            .map(|entry| {
                let ledger = &entry.ledger;
                (
                    ledger.stat.dev,
                    ledger.stat.ino,
                    ledger.offset,
                    ledger.prefix,
                    ledger.tail,
                    ledger.stat.changed_ns,
                )
            })
    }

    pub(crate) fn session_descriptions(
        &self,
        requests: &[SessionDescriptionRequest],
    ) -> Vec<Option<String>> {
        if requests.is_empty() {
            return Vec::new();
        }
        self.store
            .as_ref()
            .and_then(|store| store.session_descriptions(requests).ok())
            .unwrap_or_else(|| vec![None; requests.len()])
    }

    pub(crate) fn save_session_description(
        &mut self,
        key: &str,
        version: u32,
        inputs: &str,
        description: &str,
    ) {
        self.descriptions_pending.push((
            key.to_owned(),
            version,
            inputs.to_owned(),
            description.to_owned(),
        ));
    }

    pub(crate) fn publish_session_descriptions(&mut self) {
        let descriptions = std::mem::take(&mut self.descriptions_pending);
        if !descriptions.is_empty()
            && !self.busy
            && self.unpersisted.is_empty()
            && let Some(store) = self.store.as_mut()
        {
            // A failed cache publication never changes the serving snapshot.
            let _ = store.save_session_descriptions(&descriptions);
        }
    }

    pub(crate) fn session_catalog_generation(&self) -> Option<String> {
        self.store
            .as_ref()?
            .session_catalog_generation()
            .ok()
            .flatten()
    }

    pub(crate) fn note_catalog_audit(
        &mut self,
        expected: Option<&str>,
        now: i64,
    ) -> io::Result<bool> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("producer store unavailable"))?
            .note_catalog_audit(expected, now)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(crate) fn session_slots(
        &self,
        row: &crate::model::summary::CatalogRow,
        expected_generation: Option<&str>,
        max: usize,
    ) -> Option<Vec<crate::slot_projection::SavedSlot>> {
        self.store
            .as_ref()?
            .session_slots(row, expected_generation, max)
            .ok()
            .flatten()
    }

    #[cfg(test)]
    pub(crate) fn publish_session_catalog(
        &mut self,
        rows: &[crate::model::summary::CatalogRow],
        expected_generation: Option<&str>,
    ) {
        self.publish_session_projection(
            &crate::slot_projection::Publication {
                rows,
                transcripts: None,
                observations: None,
            },
            expected_generation,
        );
    }

    pub(crate) fn publish_session_projection(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) {
        let _ = self.publish_catalog_projection(publication, expected_generation);
    }

    /// A complete native observation is acknowledged only after its source
    /// evidence and Catalog publication are durable in the same generation.
    pub(crate) fn publish_catalog_projection(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> io::Result<()> {
        self.publish_catalog_projection_fenced(publication, expected_generation)
            .map(|_| ())
    }

    pub(crate) fn publish_catalog_projection_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> io::Result<Option<String>> {
        if self.busy || !self.unpersisted.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "native producer sources not durably observed",
            ));
        }
        let (outcome, generation) = self
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other("scoped producer store unavailable"))?
            .publish_session_catalog_fenced(publication, expected_generation)
            .map_err(|error| io::Error::other(error.to_string()))?;
        match outcome {
            Outcome::Written => {
                #[cfg(test)]
                if let Some(hook) = AFTER_CATALOG_PUBLICATION.with(|hook| hook.borrow_mut().take())
                {
                    hook();
                }
                Ok(generation)
            }
            Outcome::Deferred | Outcome::Conflict | Outcome::Stale => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "native Catalog publication generation changed",
            )),
        }
    }

    pub(crate) fn replaced(&self, path: &Path) -> bool {
        self.replaced.contains(path.to_string_lossy().as_ref())
    }

    fn require_current_store(&self) -> io::Result<()> {
        if self.store.is_none() {
            return Err(io::Error::other(crate::storage::StorageError::Unavailable));
        }
        if self.busy {
            return Err(io::Error::other(crate::storage::StorageError::Busy));
        }
        Ok(())
    }

    pub(crate) fn begin_scan(&mut self) -> io::Result<()> {
        self.require_current_store()?;
        self.cold_writes.clear();
        self.index_clone = Duration::ZERO;
        self.scan_timing = crate::native_timing::enabled();
        self.parse_duration = Duration::ZERO;
        self.index_write_duration = Duration::ZERO;
        self.scan_measurements = crate::native_timing::Measurements::new(self.scan_timing);
        self.replaced.clear();
        Ok(())
    }

    fn from_loaded(loaded: Loaded) -> Self {
        let Loaded {
            files,
            reported_runs,
            stamp,
        } = loaded;
        let mut cache = Self {
            files: files
                .into_iter()
                .map(|(path, ledger, index)| {
                    (
                        path,
                        CachedFile {
                            ledger,
                            index: Some(Arc::new(index)),
                        },
                    )
                })
                .collect(),
            claude_json_stamp: stamp,
            ..Self::default()
        };
        cache.set_reported_runs(reported_runs);
        cache
    }

    fn set_reported_runs(&mut self, runs: Vec<crate::facts::ReportedRunSnapshot>) {
        self.reported_runs.clear();
        for run in runs {
            self.reported_runs
                .entry(run.last_session_id.clone())
                .or_default()
                .insert(run.last_start_time, run);
        }
    }

    /// Maps a failed configured store call to its storage refusal.
    fn store_failed(&mut self, error: &StoreError) -> io::Result<()> {
        Err(io::Error::other(match error {
            StoreError::Configured(error) => *error,
            StoreError::Busy(_) => crate::storage::StorageError::Busy,
            StoreError::Data(_) | StoreError::Failed(_) => {
                crate::storage::StorageError::Unavailable
            }
        }))
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub(crate) fn advance_tree_repair(&mut self, options: &crate::Options) -> io::Result<bool> {
        let Some(store) = self.store.as_mut() else {
            return Ok(false);
        };
        let (pending, changed) = store.advance_tree_repair(options)?;
        if let Some(path) = changed {
            self.files.remove(&path);
        }
        Ok(pending)
    }

    pub(crate) fn tree_repair_pending(&self) -> bool {
        self.store
            .as_ref()
            .is_some_and(|store| store.tree_repair_pending().unwrap_or(true))
    }

    pub(crate) fn indexed_stat(&self, path: &Path) -> Option<Stat> {
        self.files
            .get(path.to_string_lossy().as_ref())
            .map(|file| file.ledger.stat)
    }

    pub(crate) fn source(
        &self,
        path: &Path,
        native_id: &str,
    ) -> Option<crate::model::summary::CatalogSource> {
        let file = self.files.get(path.to_string_lossy().as_ref())?;
        Some(file.ledger.source(path, native_id))
    }

    /// Warm trees read the existing metadata regions only. Changed sources or
    /// incomplete metadata use the ordinary parser/ledger CAS; never put a
    /// partial FileIndex in the producer cache.
    pub(crate) fn tree(
        &mut self,
        path: &Path,
        harness: &str,
        native_id: &str,
        dirty: &mut bool,
    ) -> io::Result<(
        crate::model::summary::CatalogSource,
        crate::native_tree::Metadata,
    )> {
        self.require_current_store()?;
        let key = path.to_string_lossy();
        let stat = Stat::of(path)?;
        if !self.unpersisted.contains(key.as_ref()) {
            match self
                .store
                .as_ref()
                .map(|store| store.load_tree(&key, &stat))
            {
                Some(Ok(Some((ledger, metadata)))) if ledger.stat == stat => {
                    return Ok((ledger.source(path, native_id), metadata));
                }
                Some(Err(error)) => {
                    return Err(io::Error::other(configured_error(error)));
                }
                Some(Ok(_)) | None => {}
            }
        }
        let index = scan_file(path, harness, self, dirty)?;
        let metadata = crate::native_tree::Metadata::from_index(&index)
            .ok_or_else(|| io::Error::other("native tree metadata unavailable"))?;
        let source = self
            .source(path, native_id)
            .ok_or_else(|| io::Error::other("native tree source unavailable"))?;
        Ok((source, metadata))
    }

    #[cfg(test)]
    pub(crate) fn file_index(&self, path: &Path) -> Option<&FileIndex> {
        self.files
            .get(path.to_string_lossy().as_ref())
            .and_then(|file| file.index.as_deref())
    }

    pub(crate) fn reported_runs(&self) -> impl Iterator<Item = &crate::facts::ReportedRunSnapshot> {
        self.reported_runs.values().flat_map(|runs| runs.values())
    }

    /// Reads only selected fields from the sibling state file when its
    /// identity, size or modified time changes. A missing/unreadable file is
    /// an empty input and leaves prior snapshots intact.
    pub(crate) fn refresh_reported_runs(
        &mut self,
        path: &Path,
        capture_at: i64,
        dirty: &mut bool,
    ) -> io::Result<()> {
        self.require_current_store()?;
        let Ok(metadata) = fs::metadata(path) else {
            return Ok(());
        };
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos());
        #[cfg(unix)]
        let (dev, ino) = (metadata.dev(), metadata.ino());
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);
        let stamp = ReportedFileStamp {
            dev,
            ino,
            size: metadata.len(),
            modified_ns,
        };
        if self.claude_json_stamp.as_ref() == Some(&stamp) {
            if self.runs_unsaved {
                let runs: Vec<_> = self.reported_runs().cloned().collect();
                self.save_runs(&runs, &stamp)?;
            }
            return Ok(());
        }
        let Ok(file) = crate::open_input(path) else {
            return Ok(());
        };
        let Ok(root) = serde_json::from_reader::<_, ClaudeJson>(file) else {
            return Ok(());
        };
        self.claude_json_stamp = Some(stamp.clone());
        *dirty = true;
        let mut read = Vec::new();
        for project in root.projects.into_values() {
            let Some(snapshot) = project.snapshot(capture_at) else {
                continue;
            };
            let session_id = snapshot.last_session_id.clone();
            let start = snapshot.last_start_time;
            read.push(snapshot.clone());
            self.reported_runs
                .entry(session_id)
                .or_default()
                .insert(start, snapshot);
        }
        self.save_runs(&read, &stamp)
    }

    /// Writes reported runs through to the store, and takes back every run
    /// it holds.
    fn save_runs(
        &mut self,
        runs: &[crate::facts::ReportedRunSnapshot],
        stamp: &ReportedFileStamp,
    ) -> io::Result<()> {
        self.require_current_store()?;
        if self.store.is_none() {
            return Ok(());
        }
        self.runs_unsaved = true;
        if self.busy {
            return Ok(());
        }
        let saved = self
            .store
            .as_mut()
            .map(|store| store.save_runs(runs, stamp));
        match saved {
            Some(Ok(None)) => {
                return Err(io::Error::other(crate::storage::StorageError::ScopeChanged));
            }
            Some(Ok(Some(all))) => {
                self.runs_unsaved = false;
                self.set_reported_runs(all);
            }
            Some(Err(error)) => self.store_failed(&error)?,
            None => {}
        }
        Ok(())
    }

    /// Drops files that are gone, so the index doesn't grow without bound.
    pub(crate) fn retain(&mut self, seen: &BTreeSet<String>, dirty: &mut bool) -> io::Result<()> {
        self.require_current_store()?;
        let mut gone: Vec<(String, Option<Ledger>)> = self
            .files
            .iter()
            .filter(|(path, _)| !seen.contains(*path))
            .map(|(path, entry)| (path.clone(), Some(entry.ledger.clone())))
            .collect();
        gone.extend(
            std::mem::take(&mut self.retention_base)
                .into_iter()
                .filter(|(path, _)| !seen.contains(path)),
        );
        gone.sort_by(|a, b| a.0.cmp(&b.0));
        gone.dedup_by(|a, b| a.0 == b.0);
        for (path, _) in &gone {
            self.files.remove(path);
            self.unpersisted.remove(path);
        }
        *dirty |= !gone.is_empty();
        if self.store.is_none() {
            return Ok(());
        }
        self.gone.extend(gone);
        if self.gone.is_empty() || self.busy {
            return Ok(());
        }
        let removed = self
            .store
            .as_mut()
            .map(|store| store.remove_files(&self.gone));
        match removed {
            Some(Ok(Outcome::Stale)) => {
                return Err(io::Error::other(crate::storage::StorageError::ScopeChanged));
            }
            Some(Ok(Outcome::Written | Outcome::Conflict | Outcome::Deferred)) => self.gone.clear(),
            Some(Err(error)) => self.store_failed(&error)?,
            None => {}
        }
        Ok(())
    }

    /// What the index last recorded for `path`: the store's ledger, which
    /// another process may have moved on, or this process's without one.
    fn recorded(&mut self, path: &str) -> io::Result<Option<Ledger>> {
        let stored = self.store.as_ref().map(|store| store.ledger(path));
        match stored {
            Some(Ok(ledger)) => return Ok(ledger),
            Some(Err(error)) => {
                return Err(io::Error::other(configured_error(error)));
            }
            None => {}
        }
        Ok(self.files.get(path).map(|entry| entry.ledger.clone()))
    }

    fn recorded_batch(&self, paths: &[String]) -> io::Result<Vec<Option<Ledger>>> {
        if paths.len() > 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native ledger batch exceeds 64 paths",
            ));
        }
        let Some(store) = &self.store else {
            return Ok(paths
                .iter()
                .map(|path| self.files.get(path).map(|entry| entry.ledger.clone()))
                .collect());
        };
        let ledgers = store
            .ledger_batch(paths)
            .map_err(|error| io::Error::other(configured_error(error)))?;
        if ledgers.len() != paths.len() {
            return Err(io::Error::other("native ledger batch result size changed"));
        }
        Ok(ledgers)
    }

    /// Selected source preparation may replace this snapshot with per-source
    /// `prepare_file` CAS checks before parsing. Explicit ingest keeps its own
    /// caller supplied operation and continues to read each ledger directly.
    pub(crate) fn host_scan_ledger_batch(
        &self,
        paths: &[String],
    ) -> io::Result<Option<Vec<Option<Ledger>>>> {
        if self.store.is_none() || self.host_source.is_none() || self.event_operation.is_some() {
            return Ok(None);
        }
        let mut ledgers = Vec::with_capacity(paths.len());
        for batch in paths.chunks(64) {
            ledgers.extend(self.recorded_batch(batch)?);
        }
        Ok(Some(ledgers))
    }

    /// The index `ledger` recorded for `path`: this process's when it is at
    /// that ledger, else the store's.
    fn base(&mut self, path: &str, ledger: &Ledger) -> io::Result<Base> {
        if let Some(entry) = self.files.get(path)
            && entry.ledger == *ledger
            && let Some(index) = &entry.index
            && index
                .codex_native_usage
                .as_ref()
                .and_then(|native| native.cost.as_ref())
                .is_none_or(|cost| {
                    cost.pricing_fingerprint == crate::pricing::snapshot().fingerprint
                })
        {
            return Ok(Base::Found(Arc::clone(index)));
        }
        // A pricing-only fingerprint mismatch falls through to the committed
        // SQLite index below. `load_file` restores retained facts/dimensions;
        // it does not open the native log body. Ordinary ledger movement still
        // follows the existing bounded append/rebuild path.
        let loaded = self.store.as_ref().map(|store| store.load_file(path));
        Ok(match loaded {
            Some(Ok(Some((stored, index)))) if stored == *ledger => Base::Found(Arc::new(index)),
            Some(Ok(_)) => Base::Moved,
            Some(Err(error)) => {
                return Err(io::Error::other(configured_error(error)));
            }
            None => Base::Unreadable,
        })
    }

    fn require_usage_store(&self, index: &FileIndex) -> io::Result<()> {
        // Either consumer can accept usage first: Tree also accepts it before
        // the first model. Their compact checkpoints keep historical identities
        // only in SQLite, so neither can become an in-memory append base.
        let tree_checkpoint = index.native_usage_observations.is_some()
            && index
                .tree_summary
                .as_ref()
                .is_some_and(|tree| tree.stored.is_some());
        if self.store.is_none()
            && (tree_checkpoint
                || index
                    .codex_native_usage
                    .as_ref()
                    .is_some_and(|native| native.cost.is_some()))
        {
            return Err(io::Error::other(
                "normalized native usage store unavailable",
            ));
        }
        Ok(())
    }

    /// Parsing happens before the write reservation. Cold complete new files
    /// share one bounded commit; no offset or dirty revision can escape alone.
    pub(crate) fn flush_cold_writes(&mut self) -> io::Result<()> {
        let files = std::mem::take(&mut self.cold_writes);
        if files.is_empty() {
            return Ok(());
        }
        let started = self.scan_timing.then(Instant::now);
        let outcome = self
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other("cold batch store unavailable"))?
            .commit_initial_files(&files)
            .map_err(store_io_error)?;
        if let Some(started) = started {
            self.index_write_duration += started.elapsed();
        }
        if outcome != Outcome::Written {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "cold index batch generation changed",
            ));
        }
        for (path, _, _) in files {
            self.unpersisted.remove(&path);
        }
        Ok(())
    }

    /// Request-owned cold Source batch. Parsing still uses scan_file, while
    /// every durable family effect waits for the single final store commit.
    /// Reconcile all identities before Raw, including after a lost batch ACK.
    #[cfg(feature = "postgres")]
    #[allow(dead_code)] // Private native integration, not a public backend.
    pub(crate) fn ingest_initial_batch(
        &mut self,
        requests: &[(&str, &Path, &str)],
    ) -> Result<Vec<crate::storage::native_events::EventReceipt>, crate::storage::StorageError>
    {
        use crate::storage::StorageError;
        if requests.is_empty()
            || requests.len() > 64
            || !self.cold_writes.is_empty()
            || self.event_operation.is_some()
        {
            return Err(StorageError::InvalidArguments);
        }
        let mut paths = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let store = self.store.as_ref().ok_or(StorageError::Unavailable)?;
        let mut replay = Vec::new();
        for (id, path, harness) in requests {
            let path = path.to_str().ok_or(StorageError::InvalidArguments)?;
            if !paths.insert(path) || !ids.insert(id) {
                return Err(StorageError::InvalidArguments);
            }
            if let Some(receipt) = store
                .reconcile_event(id, path, harness)
                .map_err(configured_error)?
            {
                replay.push(receipt);
            }
        }
        if replay.len() == requests.len() {
            return Ok(replay);
        }
        if !replay.is_empty() {
            return Err(StorageError::Conflict);
        }
        let prior_budget = self.scan_byte_budget;
        self.scan_byte_budget = Some(prior_budget.unwrap_or(1024 * 1024).min(1024 * 1024));
        self.atomic_cold = true;
        let scanned = (|| {
            for (id, path, harness) in requests {
                self.event_operation = Some((*id).to_owned());
                scan_file(path, harness, self, &mut false)?;
            }
            self.flush_cold_writes()
        })();
        self.atomic_cold = false;
        self.event_operation = None;
        self.scan_byte_budget = prior_budget;
        if let Err(error) = scanned {
            self.cold_writes.clear();
            return Err(error
                .get_ref()
                .and_then(|e| e.downcast_ref::<StorageError>())
                .copied()
                .unwrap_or(StorageError::Unavailable));
        }
        requests
            .iter()
            .map(|(_, path, _)| {
                self.store
                    .as_ref()
                    .ok_or(StorageError::Unavailable)?
                    .event_receipt(path.to_str().ok_or(StorageError::InvalidArguments)?)
                    .map_err(configured_error)
            })
            .collect()
    }

    /// Writes a file's change through to the store. `true` when another
    /// process moved the file on first, so the change must be redone. A
    /// change that isn't written is noted, and read again next scan.
    /// Commit against the scanner's full open-file witness, which can be
    /// larger than the persisted prefix ledger on a bounded cold scan.
    fn commit(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        changes: Option<&DirtyRows>,
        ledger: &Ledger,
        physical_stat: &Stat,
        index: &FileIndex,
    ) -> io::Result<bool> {
        self.require_current_store()?;
        if self.store.is_none() {
            return Ok(false);
        }
        self.unpersisted.insert(path.to_owned());
        if self.busy {
            return Ok(false);
        }
        let started = self.scan_timing.then(Instant::now);
        let committed = self
            .store
            .as_mut()
            .map(|store| store.commit_file(path, expected, changes, ledger, physical_stat, index));
        if let Some(started) = started {
            self.index_write_duration += started.elapsed();
        }
        match committed {
            Some(Ok(Outcome::Written)) => {
                self.unpersisted.remove(path);
                Ok(false)
            }
            Some(Ok(Outcome::Conflict)) => Ok(true),
            Some(Ok(Outcome::Deferred)) => {
                Err(io::Error::other(crate::storage::StorageError::Conflict))
            }
            Some(Ok(Outcome::Stale)) => {
                Err(io::Error::other(crate::storage::StorageError::ScopeChanged))
            }
            Some(Err(StoreError::Configured(crate::storage::StorageError::Conflict))) => Ok(true),
            Some(Err(error)) => Err(io::Error::other(configured_error(error))),
            None => Err(io::Error::other(crate::storage::StorageError::Unavailable)),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Lines parsed by [`scan_file`] on this thread: a warm build parses none.
    pub(crate) static PARSED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static PARSED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// What [`scan_file`] did with each file on this thread: `unchanged`,
    /// `append`, `replace` or `conflict`.
    pub(crate) static LEDGER: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Runs once, after a file is parsed and before its change commits.
    pub(crate) static BEFORE_COMMIT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

fn trace(_what: &'static str) {
    #[cfg(test)]
    LEDGER.with(|ledger| ledger.borrow_mut().push(_what));
}

/// Maximum attempts to reconcile a file moved by another process. Exhaustion
/// refuses the scan; an uncommitted in-memory index is never returned.
const COMMIT_ATTEMPTS: usize = 3;

/// Indexes one file: shares the recorded index when the file didn't change,
/// resumes from the recorded offset when it only grew (or was only
/// touched), and reads it from the start otherwise. The change commits to
/// the store only if no other process recorded the file meanwhile; if one
/// did, it is redone from what that process recorded.
pub(crate) fn crate_id_for_relocation(path: &Path) -> Option<String> {
    crate::codex_native_id_prefix(path, Stat::of(path).ok()?.size)
        .ok()
        .flatten()
}

pub(crate) fn scan_file(
    path: &Path,
    harness: &str,
    cache: &mut EventCache,
    dirty: &mut bool,
) -> io::Result<Arc<FileIndex>> {
    scan_file_with_mode(path, harness, cache, dirty, false, None)
}

pub(crate) fn scan_file_with_ledger_hint(
    path: &Path,
    harness: &str,
    cache: &mut EventCache,
    dirty: &mut bool,
    ledger_hint: Option<Ledger>,
) -> io::Result<Arc<FileIndex>> {
    scan_file_with_mode(path, harness, cache, dirty, false, Some(ledger_hint))
}

/// Semantic derivation consumes a committed Index generation even when its
/// cache also owns ordinary host ingestion. It never creates an ingest intent.
pub(crate) fn scan_committed_file(
    path: &Path,
    harness: &str,
    cache: &mut EventCache,
    dirty: &mut bool,
) -> io::Result<Arc<FileIndex>> {
    scan_file_with_mode(path, harness, cache, dirty, true, None)
}

#[derive(Debug)]
pub(crate) struct TreeProjectionRepair {
    path: String,
    ledger: Ledger,
    harness: String,
}

impl std::fmt::Display for TreeProjectionRepair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native tree projection repair pending")
    }
}

impl std::error::Error for TreeProjectionRepair {}

impl TreeProjectionRepair {
    pub(crate) fn queue(&self, cache: &mut EventCache) -> io::Result<()> {
        let _source_lock = cache
            .lock_cache
            .as_ref()
            .or(cache.v1_cache.as_ref())
            .filter(|_| cache.store.is_some())
            .map(|stem| crate::source_change::SourceLock::acquire_cache(stem, &self.path, false))
            .transpose()?;
        if Stat::of(Path::new(&self.path))? != self.ledger.stat {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        cache
            .store
            .as_mut()
            .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?
            .queue_tree_repair(&self.path, &self.ledger, &self.harness)
    }
}

fn scan_file_with_mode(
    path: &Path,
    harness: &str,
    cache: &mut EventCache,
    dirty: &mut bool,
    committed_only: bool,
    initial_recorded: Option<Option<Ledger>>,
) -> io::Result<Arc<FileIndex>> {
    cache.require_current_store()?;
    // Host-owned ordinary producers capture their own request identity. The
    // configured writer persists it with the observed ledger and Source claim
    // in prepare_file before any Raw parsing or publication decision. Explicit
    // ingest retains its caller's identity; staged model readers remain reads.
    let host_operation =
        if !committed_only && cache.host_source.is_some() && cache.event_operation.is_none() {
            Some(
                crate::storage::source_observations::SourceOperation::capture(
                    &cache.observation_namespace,
                    "native_ingest",
                    path.as_os_str().as_encoded_bytes(),
                ),
            )
        } else {
            None
        };
    let key = path.to_string_lossy().into_owned();
    let mut initial_recorded = host_operation.as_ref().and(initial_recorded);
    if let Some(index) = cache.files.get(&key).and_then(|entry| entry.index.as_ref()) {
        cache.require_usage_store(index)?;
    }
    let started = cache.scan_measurements.start();
    let _source_lock = cache
        .lock_cache
        .as_ref()
        .or(cache.v1_cache.as_ref())
        .filter(|_| cache.store.is_some())
        .map(|v1| crate::source_change::SourceLock::acquire_cache(v1, &key, false))
        .transpose()?;
    cache.scan_measurements.record("source_lock", started, 1);
    let agent = harness == "claude" && crate::native_sidecar::agent_path(path);
    // Model preparation consumes the committed typed Index/proof binding. It
    // has no ingestion operation ID and must neither create an intent nor
    // reparse changed Raw behind the staged claim. load_file validates the
    // configured current schema, admission and namespace in its read snapshot.
    if committed_only || (cache.event_operation.is_none() && host_operation.is_none()) {
        let (ledger, index) = cache
            .store
            .as_ref()
            .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?
            .load_file(&key)
            .map_err(store_io_error)?
            .ok_or_else(|| io::Error::other(crate::storage::StorageError::Conflict))?;
        if ledger.stat != Stat::of(path)?
            || !index
                .source_chunks
                .as_ref()
                .is_some_and(|chunks| chunks.valid(ledger.offset))
            || (harness != "copilot" && index.marker_index.is_none())
            || index.handoff_index.is_none()
            || (harness == "codex" && index.codex_header.is_none())
            || (agent
                && !index
                    .claude_sidecar
                    .as_ref()
                    .is_some_and(|sidecar| sidecar.current(path)))
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "committed model source changed or incomplete",
            ));
        }
        cache.require_usage_store(&index)?;
        if harness != "copilot" && index.tree_summary.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                TreeProjectionRepair {
                    path: key,
                    ledger,
                    harness: harness.to_owned(),
                },
            ));
        }
        let index = Arc::new(index);
        cache.files.insert(
            key,
            CachedFile {
                ledger,
                index: Some(Arc::clone(&index)),
            },
        );
        // The existing model captures these exact chunks, verifies every used
        // range and finishes its source/sidecar fences before record/publish.
        return Ok(index);
    }
    *dirty = true;
    let mut attempt = 0;
    let mut force_replace = false;
    let mut verify_prefix = false;
    loop {
        attempt += 1;
        let last = attempt >= COMMIT_ATTEMPTS;
        let mut recorded = match initial_recorded.take() {
            Some(recorded) => recorded,
            None => cache.recorded(&key)?,
        };
        let started = cache.scan_measurements.start();
        let mut file = LogFile::open(path)?;
        let stat = Stat::from_metadata(&file.metadata()?)?;
        cache.scan_measurements.record("open_stat", started, 1);
        if recorded.is_none()
            && harness == "codex"
            && let Some(id) = crate_id_for_relocation(path)
        {
            let started = cache.scan_measurements.start();
            if let Some(old) = cache
                .store
                .as_mut()
                .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?
                .relocate_codex(&key, &stat, &id, &mut file, &mut cache.scan_measurements)
                .map_err(store_io_error)?
            {
                cache.files.remove(&old);
                recorded = cache.recorded(&key)?;
            }
            cache
                .scan_measurements
                .record("relocation_lookup", started, 1);
        }
        // Resolve a known deferred representation before journaling ingestion.
        // The projection worker repairs this same committed Ledger; it does
        // not own, complete, or acknowledge a native ingest operation.
        let mut existing_base = None;
        if let Some(ledger) = &recorded
            && harness != "copilot"
            && ledger.offset > 1024 * 1024
            && ledger.stat == stat
            && let Base::Found(index) = cache.base(&key, ledger)?
        {
            if index.tree_summary.is_none() {
                cache
                    .store
                    .as_mut()
                    .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?
                    .queue_tree_repair(&key, ledger, harness)?;
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "native tree projection repair pending",
                ));
            }
            existing_base = Some(index);
        }
        {
            let expected = recorded.as_ref();
            // Each bounded host attempt owns a distinct immutable intent.
            // Explicit ingest keeps its caller's exact replay identity.
            let operation = host_operation.as_ref().map(|operation| {
                crate::storage::source_observations::SourceOperation::digest(
                    "native_ingest_identity",
                    format!("{}:{attempt}", operation.id).as_bytes(),
                )
            });
            let prepare_started = cache.scan_measurements.start();
            let prepared = cache
                .store
                .as_mut()
                .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?
                .prepare_file(
                    &key,
                    expected,
                    operation
                        .as_deref()
                        .or(cache.event_operation.as_deref())
                        .ok_or_else(|| {
                            io::Error::other(crate::storage::StorageError::InvalidArguments)
                        })?,
                    harness,
                );
            cache
                .scan_measurements
                .record("prepare_file", prepare_started, 1);
            match prepared {
                Err(StoreError::Configured(crate::storage::StorageError::Conflict))
                    if host_operation.is_some() && !last =>
                {
                    trace("prepare_conflict");
                    continue;
                }
                result => result.map_err(store_io_error)?,
            }
        }
        let started = cache.scan_measurements.start();
        let trusted_receiver = match recorded.as_ref() {
            Some(ledger) if !verify_prefix => cache
                .store
                .as_ref()
                .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?
                .append_evidence(&key)
                .map_err(store_io_error)?
                .is_some_and(|evidence| evidence.qualifies(&ledger.stat, &stat)),
            _ => false,
        };
        let mut trusted_append = false;
        let mut base = None;
        if let Some(ledger) = &recorded
            && !force_replace
            && (ledger.stat == stat || trusted_receiver || ledger.resumes(&stat, &mut file))
        {
            let restored = match existing_base.take() {
                Some(index) => Base::Found(index),
                None => cache.base(&key, ledger)?,
            };
            match restored {
                Base::Found(index) => {
                    cache.require_usage_store(&index)?;
                    trusted_append = trusted_receiver
                        && index
                            .source_chunks
                            .as_ref()
                            .is_some_and(|chunks| chunks.resume.is_some());
                    if ledger.stat == stat || trusted_append || ledger.resumes(&stat, &mut file) {
                        base = Some(index);
                    }
                }
                Base::Moved if !last => continue,
                Base::Moved | Base::Unreadable => {}
            }
        }
        cache.scan_measurements.record("ledger_base", started, 1);
        let started = agent.then(|| cache.scan_measurements.start()).flatten();
        let sidecar = if agent {
            match crate::native_sidecar::ClaudeSidecar::observe(
                path,
                base.as_ref()
                    .and_then(|index| index.claude_sidecar.as_ref()),
            ) {
                Ok(sidecar) => sidecar,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    if last {
                        None
                    } else {
                        continue;
                    }
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        cache.scan_measurements.record("sidecar", started, 1);
        // Another process already read it as it is now.
        if let Some(ledger) = &recorded
            && ledger.stat == stat
            && let Some(index) = &base
            && (harness == "copilot" || index.tree_summary.is_some())
            && index.source_chunks.is_some()
            && (harness == "copilot" || index.marker_index.is_some())
            && index.handoff_index.is_some()
            && (harness != "codex" || index.codex_header.is_some())
            && (!agent
                || sidecar
                    .as_ref()
                    .is_some_and(|observed| index.claude_sidecar.as_ref() == Some(observed)))
        {
            trace("unchanged");
            {
                let ledger = ledger.clone();
                let mut index = Arc::clone(index);
                if path.to_str().is_none() {
                    Arc::make_mut(&mut index).native_path = Some(path.to_owned());
                }
                if cache.commit(&key, Some(&ledger), None, &ledger, &stat, &index)? {
                    trace("conflict");
                    if last {
                        return Err(io::Error::other(crate::storage::StorageError::Conflict));
                    }
                    continue;
                }
            }
            // The store holds it as it is now: nothing waits to be written.
            cache.unpersisted.remove(&key);
            cache.files.insert(
                key,
                CachedFile {
                    ledger: ledger.clone(),
                    index: Some(Arc::clone(index)),
                },
            );
            return Ok(Arc::clone(index));
        }
        let from = match (&recorded, &base) {
            (Some(ledger), Some(_)) => ledger.offset,
            _ => 0,
        };
        trace(if base.is_some() { "append" } else { "replace" });
        if base.is_none() {
            cache.replaced.insert(key.clone());
        }
        let append = base.is_some();
        // The cache owns the previous index, and a served model owns only
        // stable transcript metadata. Drop that cache owner before COW so
        // ordinary appends mutate the sole Arc without cloning the index.
        cache.files.remove(&key);
        let mut index = base.unwrap_or_else(|| Arc::new(FileIndex::default()));
        let clone_started = Instant::now();
        let shared = Arc::strong_count(&index) > 1;
        let writable = Arc::make_mut(&mut index);
        if !append && harness == "codex" {
            writable.native_usage_observations = Some(Vec::new());
        }
        if shared {
            cache.index_clone += clone_started.elapsed();
        }
        let started = (harness == "codex")
            .then(|| cache.scan_measurements.start())
            .flatten();
        let header_bytes = if harness == "codex" {
            // Preserve first-physical-line and unfinished-header semantics.
            // The same open native handle supplies the event ledger and header.
            file.seek(SeekFrom::Start(0))?;
            let mut line = Vec::new();
            BufReader::new(&mut file).read_until(b'\n', &mut line)?;
            #[cfg(test)]
            CODEX_HEADER_READS.with(|reads| reads.set(reads.get() + 1));
            writable.codex_header = Some(
                serde_json::from_slice::<Value>(&line)
                    .ok()
                    .as_ref()
                    .map(crate::native_header::CodexHeader::from_record)
                    .unwrap_or_default(),
            );
            #[cfg(test)]
            if let Some(hook) = AFTER_CODEX_HEADER.with(|hook| hook.borrow_mut().take()) {
                hook();
            }
            Some(line)
        } else {
            None
        };
        cache.scan_measurements.record("native_header", started, 1);
        writable.claude_sidecar = sidecar;
        #[cfg(test)]
        if agent && let Some(hook) = AFTER_CLAUDE_SIDECAR.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        let deferred_tree =
            harness != "copilot" && writable.tree_summary.is_none() && from > 1024 * 1024;
        if deferred_tree {
            // Small representation repairs have a fixed byte ceiling. Large
            // retained sources resume through the verified projection worker;
            // never rebuild a historical Summary in the foreground reader.
            if let (Some(store), Some(ledger)) = (cache.store.as_mut(), recorded.as_ref()) {
                store.queue_tree_repair(&key, ledger, harness)?;
            }
            if recorded.as_ref().is_some_and(|ledger| ledger.stat == stat) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "native tree projection repair pending",
                ));
            }
            // Advancing the Index receipt for a real append lets the worker
            // resume its verified prefix. Tree stays explicitly unavailable
            // until that worker commits a complete replacement projection.
        }
        if harness != "copilot" && writable.tree_summary.is_none() && !deferred_tree {
            let started = cache.scan_measurements.start();
            let (summary, observed) =
                crate::native_tree::Summary::repair(&mut file, from, harness)?;
            cache.scan_measurements.record("tree_repair", started, 1);
            if append
                && recorded
                    .as_ref()
                    .is_some_and(|ledger| observed != ledger.prefix)
            {
                if last {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native tree metadata generation changed",
                    ));
                }
                force_replace = true;
                continue;
            }
            writable.tree_summary = Some(summary);
        }
        writable.dirty_rows = Some(DirtyRows::begin(writable));
        if harness != "copilot" && writable.marker_index.is_none() {
            let started = cache.scan_measurements.start();
            writable.marker_index = Some(crate::native_marker::Index::repair(&mut file, from)?);
            cache.scan_measurements.record("marker_repair", started, 1);
        }
        if let Some(markers) = &mut writable.marker_index {
            markers.begin_append();
        }
        if writable.handoff_index.is_none() {
            let started = cache.scan_measurements.start();
            writable.handoff_index = Some(crate::native_handoff::Index::repair(&mut file, from)?);
            cache.scan_measurements.record("handoff_repair", started, 1);
        }
        if let Some(handoffs) = &mut writable.handoff_index {
            handoffs.begin_append();
        }
        let parse_started = cache.scan_timing.then(Instant::now);
        let (offset, parsed, bounded) = parse(
            &mut file,
            from,
            harness,
            writable,
            cache.scan_byte_budget,
            append
                .then(|| {
                    cache
                        .store
                        .as_deref()
                        .zip(recorded.as_ref())
                        .map(|(store, ledger)| (store, key.as_str(), ledger))
                })
                .flatten(),
            &mut cache.scan_measurements,
        )?;
        if let Some(started) = parse_started {
            cache.parse_duration += started.elapsed();
        }
        let bounded = bounded && offset < stat.size;
        let changes = writable
            .dirty_rows
            .take()
            .expect("parser tracks dirty rows");
        let started = cache.scan_measurements.start();
        let captured = if trusted_append {
            let previous = writable
                .source_chunks
                .as_ref()
                .expect("trusted chunks exist");
            match crate::source_chunks::capture_append(
                &mut file,
                offset,
                from,
                previous,
                recorded.as_ref().expect("trusted ledger exists").prefix,
            ) {
                Ok(captured) => captured,
                Err(error)
                    if !last
                        && error.get_ref().and_then(|error| {
                            error.downcast_ref::<crate::storage::StorageError>()
                        }) == Some(&crate::storage::StorageError::Conflict) =>
                {
                    verify_prefix = true;
                    continue;
                }
                Err(error) => return Err(error),
            }
        } else {
            crate::source_chunks::capture(&mut file, offset, from)?
        };
        cache.scan_measurements.record("chunk_capture", started, 1);
        if captured.parsed != parsed
            || (append
                && recorded
                    .as_ref()
                    .is_some_and(|ledger| captured.before != ledger.prefix))
        {
            if last {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native source changed while indexing",
                ));
            }
            force_replace = true;
            continue;
        }
        let (prefix, tail) = (captured.prefix, captured.tail);
        writable.source_chunks = Some(Arc::new(captured.chunks));
        writable.native_path = path.to_str().is_none().then(|| path.to_owned());
        if let Some(header) = header_bytes {
            let started = cache.scan_measurements.start();
            file.seek(SeekFrom::Start(0))?;
            let mut verified = Vec::new();
            BufReader::new(&mut file).read_until(b'\n', &mut verified)?;
            #[cfg(test)]
            CODEX_HEADER_READS.with(|reads| reads.set(reads.get() + 1));
            cache.scan_measurements.record("native_header", started, 1);
            if verified != header {
                if last {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native Codex header changed while indexing",
                    ));
                }
                continue;
            }
        }
        if writable
            .claude_sidecar
            .as_ref()
            .is_some_and(|sidecar| !sidecar.current(path))
        {
            if last {
                // Keep the native session under bounded sidecar churn, using
                // its existing metadata fallback without an eviction proof.
                writable.claude_sidecar = None;
            } else {
                continue;
            }
        }
        let ledger = Ledger {
            stat: if bounded {
                Stat {
                    size: offset,
                    ..stat
                }
            } else {
                stat
            },
            offset,
            prefix,
            tail,
        };
        // A byte slice must retain one exact physical generation across its
        // checkpoint. Untrusted changes never qualify the continuation proof.
        if bounded && Stat::of(path).ok().as_ref() != Some(&stat) {
            if last {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "cold source changed during checkpoint",
                ));
            }
            continue;
        }
        let started = writable
            .marker_index
            .as_ref()
            .and_then(|_| cache.scan_measurements.start());
        let invalid_markers = writable
            .marker_index
            .as_ref()
            .is_some_and(|markers| !markers.valid(&ledger) || markers.verify(&mut file).is_err());
        cache.scan_measurements.record("marker_verify", started, 1);
        if invalid_markers {
            if last {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native parent marker changed while indexing",
                ));
            }
            force_replace = true;
            continue;
        }
        let started = writable
            .handoff_index
            .as_ref()
            .and_then(|_| cache.scan_measurements.start());
        let invalid_handoffs = writable.handoff_index.as_ref().is_some_and(|handoffs| {
            !handoffs.valid(&ledger) || handoffs.verify(&mut file).is_err()
        });
        cache.scan_measurements.record("handoff_verify", started, 1);
        if invalid_handoffs {
            if last {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native launch evidence changed while indexing",
                ));
            }
            force_replace = true;
            continue;
        }
        #[cfg(test)]
        {
            if let Some(hook) = BEFORE_COMMIT.with(|hook| hook.borrow_mut().take()) {
                hook();
            }
        }
        if cache.atomic_cold && (recorded.is_some() || bounded || ledger.offset != ledger.stat.size)
        {
            return Err(io::Error::other(crate::storage::StorageError::Conflict));
        }
        if cache.scan_byte_budget.is_some()
            && recorded.is_none()
            && !bounded
            && ledger.offset == ledger.stat.size
            && cache.scoped
            && cache.store.is_some()
        {
            if cache.cold_writes.len() >= 64 {
                return Err(io::Error::other("cold index batch exceeds source budget"));
            }
            cache.unpersisted.insert(key.clone());
            cache
                .cold_writes
                .push((key.clone(), ledger.clone(), Arc::clone(&index)));
        } else if cache.commit(
            &key,
            recorded.as_ref(),
            append.then_some(&changes),
            &ledger,
            &stat,
            &index,
        )? {
            trace("conflict");
            if last {
                return Err(io::Error::other(crate::storage::StorageError::Conflict));
            }
            continue;
        }
        if bounded && !cache.unpersisted.contains(&key) {
            let stem = cache
                .lock_cache
                .as_ref()
                .or(cache.v1_cache.as_ref())
                .ok_or_else(|| io::Error::other("bounded source requires a durable store"))?;
            let source = cache
                .host_source
                .as_ref()
                .ok_or_else(|| io::Error::other("bounded source requires host Source admission"))?;
            super::events::store::checkpoint_source_scan(stem, source, &key, &ledger, &stat)?;
        }
        if cache.store.is_some() && !cache.unpersisted.contains(&key) {
            store::native_usage::checkpoint(Arc::make_mut(&mut index));
            if let Some(store) = cache.store.as_ref()
                && let Some((stored, metadata)) = store
                    .load_tree(&key, &ledger.stat)
                    .map_err(store_io_error)?
                && stored == ledger
            {
                Arc::make_mut(&mut index).tree_summary = Some(metadata.summary);
            }
        }
        cache.files.insert(
            key,
            CachedFile {
                ledger,
                index: Some(Arc::clone(&index)),
            },
        );
        return Ok(index);
    }
}

/// Folds the complete lines of `file` from `from` into `index`, and returns
/// the offset they end at. A last line without its newline is left for the
/// next read.
fn parse(
    file: &mut LogFile,
    from: u64,
    harness: &str,
    index: &mut FileIndex,
    budget: Option<u64>,
    stored: Option<(&dyn IndexStore, &str, &Ledger)>,
    measurements: &mut crate::native_timing::Measurements,
) -> io::Result<(u64, [u8; 32], bool)> {
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(from))?;
    let mut offset = from;
    let mut line = Vec::new();
    let mut parsed = Sha256::new();
    let mut bounded = false;
    loop {
        if budget.is_some_and(|budget| offset.saturating_sub(from) >= budget) {
            bounded = true;
            break;
        }
        line.clear();
        let started = measurements.start();
        let length = reader.read_until(b'\n', &mut line)?;
        // Buffered reader time includes refill I/O; this is not physical I/O
        // latency or a count of unique source bytes.
        measurements.record("body_buffered_read", started, length);
        if length == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            if let Some(markers) = &mut index.marker_index
                && let Ok(value) = serde_json::from_slice(&line)
            {
                markers.observe(&line, offset, false, &value);
            }
            if let Some(handoffs) = &mut index.handoff_index
                && let Ok(value) = serde_json::from_slice(&line)
            {
                handoffs.observe(&line, offset, false, &value)?;
            }
            break;
        }
        let line_offset = offset;
        offset += length as u64;
        let started = measurements.start();
        parsed.update(&line);
        measurements.record("body_record_hash", started, length);
        #[cfg(test)]
        {
            PARSED.with(|parsed| parsed.set(parsed.get() + 1));
            PARSED_BYTES.with(|bytes| bytes.set(bytes.get() + length as u64));
            SOURCE_HASHED_BYTES.with(|bytes| bytes.set(bytes.get() + length as u64));
        }
        let started = measurements.start();
        let record = serde_json::from_slice::<Value>(&line);
        // Only a touched message needs its old report for the Tree delta.
        if harness == "claude"
            && let Some(summary) = &mut index.tree_summary
            && summary.usage_total.is_some()
            && let Some(message) = record.as_ref().ok().and_then(|r| r.get("message"))
            && message.get("usage").is_some()
            && let Some(id) = field(message, "id")
            && !summary.usage_by_id.contains_key(id)
            && let Some(old) = index.usage_by_id.get(id)
        {
            summary
                .usage_by_id
                .insert(id.to_owned(), old.tokens.clone());
        }
        let observation = if harness == "codex" {
            record.as_ref().ok().and_then(|record| {
                store::native_usage::Observation::capture(
                    record,
                    &line,
                    line_offset,
                    index.last_model.as_deref(),
                )
            })
        } else {
            None
        };
        let known = if let (Some(observation), Some((store, path, ledger))) = (&observation, stored)
        {
            store
                .native_usage_seen(path, ledger, observation.identity())
                .map_err(|error| io::Error::other(error.to_string()))?
        } else {
            store::native_usage::Seen::default()
        };
        if let (Some(observation), Some(observations)) =
            (observation, &mut index.native_usage_observations)
        {
            observations.push(observation);
        }
        if let Some(summary) = &mut index.tree_summary {
            if harness == "codex"
                && let Ok(record) = &record
            {
                summary.codex_turn.observe(record, line_offset);
            }
            summary.observe_counted(record.as_ref().ok(), harness, !known.tree);
        }
        match record {
            Ok(record) if record.is_object() => {
                if let Some(markers) = &mut index.marker_index {
                    markers.observe(&line, line_offset, true, &record);
                }
                if let Some(handoffs) = &mut index.handoff_index {
                    handoffs.observe(&line, line_offset, true, &record)?;
                }
                if harness == "copilot" {
                    copilot(index, &record, line_offset);
                } else if harness == "claude" {
                    claude(index, &record, line_offset);
                } else if !known.model {
                    codex(index, &record, line_offset);
                } else {
                    activity(index, record_time(&record));
                }
            }
            _ => gap(index, line_offset),
        }
        measurements.record("body_record_projection", started, 1);
    }
    Ok((offset, parsed.finalize().into(), bounded))
}

/// Busy clustering: consecutive line timestamps no more than this far apart
/// form one interval.
pub(crate) const BUSY_GAP_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// Your message: a human-origin user record or queued command.
    Ask,
    /// A user prompt that isn't yours: a brief, an sdk-cli prompt, or a
    /// Codex user message.
    #[default]
    U,
    /// Assistant text: one event per text block, so back-to-back texts all
    /// stay (where the last reply matters, the model takes it explicitly).
    A,
    /// A tool call, resolved in place when its result arrives.
    Tool,
    /// A Codex command or file change completed by an `item_completed` event.
    Operation,
    /// A relay received (`origin.kind == "peer"`, or a
    /// `<cross-session-message>` block).
    Xsm,
    /// A message between a session and its own agent: a subagent's hand-back
    /// or `<agent-message>` (up), a coordinator message (down), or a Codex
    /// `agent_message`, its direction in `script`.
    Agm,
    /// A `<task-notification>` block.
    Tn,
    /// Lines the log lost: a run of malformed lines.
    Gap,
    /// A thinking block or Codex reasoning item (in `extras` only).
    Think,
    /// Harness text added before a Codex prompt, `n` its tag (in `extras`
    /// only).
    Harness,
    /// A user-side record whose `origin.kind` semon doesn't classify, `n` that kind (in `extras` only).
    Other,
}

/// Result flags on a tool call.
pub(crate) const ASYNC: u8 = 1;
pub(crate) const DENIED: u8 = 2;
pub(crate) const PIN: u8 = 4;
pub(crate) const ANSWERED: u8 = 8;
pub(crate) const SEND_FAILED: u8 = 16;
/// A Codex call's output is only an acknowledgement (`{"accepted": …}`), not
/// an answer.
pub(crate) const ACK: u8 = 32;
/// A Codex call finished without a structured exit code: its outcome is
/// unknown, neither success nor failure.
pub(crate) const UNKNOWN: u8 = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reply {
    /// Offset of the line holding the result, and its block.
    pub(crate) o: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) b: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) t: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) e: bool,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub(crate) f: u8,
    /// A SendMessage result's `msg_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) m: Option<String>,
}

/// A code-mode script calls `tools.write_stdin`...
pub(crate) const STDIN: u8 = 1;
/// ...with `chars` that aren't an empty string: it sends input.
pub(crate) const SENDS: u8 = 2;
/// A Claude Bash call started with `run_in_background: true`.
pub(crate) const BACKGROUND: u8 = 4;
/// A task notification reports failure by status or a nonzero summary exit code.
pub(crate) const BACKGROUND_FAILED: u8 = 8;
/// An agent message sent by an agent to the session that receives it: a
/// subagent's hand-back or send, or a Codex child's message to its parent.
pub(crate) const AGM_UP: u8 = 16;
/// An agent message sent down: a parent's message to its running or
/// resumed subagent (`origin.kind == "coordinator"`), or a Codex parent's
/// message to its child.
pub(crate) const AGM_DOWN: u8 = 32;
/// A Codex agent message between agents where neither is the other's ancestor.
pub(crate) const AGM_SIDE: u8 = 64;
/// The agent's final report for its run (`origin.handback == true`).
pub(crate) const AGM_FINAL: u8 = 128;
/// Agm only (`script` bits are per kind; [`STDIN`] shares the value on tool
/// calls): a Claude subagent's send that isn't its hand-back (`senderTaskId`
/// without `handback`), always with [`AGM_UP`]. An up message with neither
/// this nor [`AGM_FINAL`] is a legacy `<agent-message>` tag: which it is, the
/// log doesn't say.
pub(crate) const AGM_PROGRESS: u8 = 1;
/// A code-mode script calls `tools.exec_command`.
const EXEC_COMMAND: u8 = 16;
/// The source contains multiple or mixed command operations whose outputs
/// have no exact nested call identity.
const AMBIGUOUS_COMMANDS: u8 = 32;
/// A code-mode poll names its target with a literal session id.
const LITERAL_SESSION: u8 = 64;

/// Open yielded commands kept per file; past this the oldest is dropped.
const YIELDS_MAX: usize = 64;
/// Poll outputs kept per yielded command; later polls still fold in.
pub(crate) const POLLS_MAX: usize = 256;

/// A Codex command that outlived its `yield_time_ms`: the script that
/// started it answered with an exec session id and no exit code. Offsets
/// only, like the rest of the index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Yield {
    /// The output lines of the polls that folded into it, in order: at most
    /// [`POLLS_MAX`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) polls: Vec<u64>,
    /// More polls folded in than were kept.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) cut: bool,
    /// The `CommandExecution` item whose `process_id` is the session id:
    /// the process's end, its line and outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) done: Option<Reply>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Peer {
    /// `origin.from`: the sender's socket address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sock: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Event {
    pub(crate) k: Kind,
    /// Byte offset of the source line.
    pub(crate) o: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) b: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) t: Option<i64>,
    /// tool: the tool_use / call id. xsm: `msg_id`. agm: Claude, the sender's
    /// `origin.senderTaskId` (the agent id or its spawning call's id), else
    /// the tag's `from`; none for a coordinator message. Codex, the
    /// `author` path. tn: the tool-use id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    /// tool: the tool name. xsm: the sender's `origin.name`. agm (Claude):
    /// `origin.from` when it differs from `id`. tn: the status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) n: Option<String>,
    /// A Codex `custom_tool_call` named `exec`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) code_mode: bool,
    /// An unnamespaced Codex `function_call` named `exec_command` or
    /// `write_stdin`, eligible for direct formatter outcome evidence.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) native_direct_call: bool,
    /// Still open for operation attribution within the current turn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) code_mode_open: bool,
    /// Operation or native connector tool: the exact index of its parent
    /// code-mode call, the only one open when the item completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<usize>,
    /// Call flags: code-mode source calls ([`STDIN`], [`SENDS`],
    /// [`EXEC_COMMAND`], [`AMBIGUOUS_COMMANDS`], [`LITERAL_SESSION`]), or a
    /// Claude Bash call launched in the [`BACKGROUND`], or a task notification
    /// with a [`BACKGROUND_FAILED`] outcome; Agm direction/finality uses
    /// [`AGM_UP`], [`AGM_DOWN`], [`AGM_SIDE`], [`AGM_FINAL`] and [`AGM_PROGRESS`].
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub(crate) script: u8,
    /// A bounded literal code-mode `write_stdin` target. Retained so an
    /// output arriving after the terminal event can still be checked exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) poll_target: Option<String>,
    /// The one awaited command result is printed directly in a recognized
    /// straight-line form; arbitrary text output cannot claim its identity.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) result_source: bool,
    /// Code-mode call: a command it started outlived its yield.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) y: Option<Box<Yield>>,
    /// Code-mode call: a poll of the yielded command the call at this event
    /// index started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) poll: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) r: Option<Reply>,
    /// A plain Codex call: the line of the `CommandExecution` item with its
    /// id, whose `aggregated_output` is the command's whole output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) item: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer: Option<Peer>,
    /// The Claude record's `uuid`: an id, used to find the lines a
    /// copy-resume duplicated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) u: Option<String>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The exact links between transcript files of one lineage (M0).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Links {
    /// `session_id` values naming another file: the lineage's first file.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) session_ids: BTreeSet<String>,
    /// `continued-in` records: the file this one was copied into.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) continued_in: BTreeSet<String>,
    /// `bridge-session` records: the Remote Control session (D5).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) bridges: BTreeSet<String>,
}

/// Parses an RFC 3339 timestamp to epoch milliseconds.
pub(crate) fn parse_ms(value: &str) -> Option<i64> {
    let seconds = i64::try_from(crate::parse_rfc3339(value)?).ok()?;
    let fraction = value.get(19..).and_then(|tail| tail.strip_prefix('.'));
    let millis = fraction
        .map(|digits| {
            let digits: String = digits
                .chars()
                .take_while(char::is_ascii_digit)
                .chain("000".chars())
                .take(3)
                .collect();
            digits.parse::<i64>().unwrap_or(0)
        })
        .unwrap_or(0);
    Some(seconds * 1000 + millis)
}

/// Adds one timestamp to a sorted interval list whose intervals are more than
/// [`BUSY_GAP_MS`] apart. Clustering per file and merging later is the same
/// as clustering every stamp of a lineage at once.
pub(crate) fn busy_add(busy: &mut Vec<(i64, i64)>, time: i64) {
    if let Some(last) = busy.last_mut()
        && time >= last.0
    {
        if time - last.1 <= BUSY_GAP_MS {
            last.1 = last.1.max(time);
            return;
        }
        busy.push((time, time));
        return;
    }
    busy.push((time, time));
    busy.sort_unstable();
    let merged = busy_merge(std::mem::take(busy));
    *busy = merged;
}

/// Merges sorted intervals that are no more than [`BUSY_GAP_MS`] apart.
pub(crate) fn busy_merge(mut intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    intervals.sort_unstable();
    let mut result: Vec<(i64, i64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = result.last_mut()
            && start - last.1 <= BUSY_GAP_MS
        {
            last.1 = last.1.max(end);
        } else {
            result.push((start, end));
        }
    }
    result
}

fn push(summary: &mut FileIndex, event: Event) {
    if event.k == Kind::Gap
        && summary
            .events
            .last()
            .is_some_and(|last| last.k == Kind::Gap)
    {
        return;
    }
    summary.events.push(event);
}

/// Compaction dedupe has two limits: an interrupt between lines of one
/// compaction counts it twice, while two compactions with no event between
/// them (such as `/compact` twice in a row) fold into one.
fn signal(
    summary: &mut FileIndex,
    k: SignalKind,
    o: u64,
    t: Option<i64>,
    n: Option<&str>,
    v: Option<u64>,
) {
    let n = n
        .filter(|tag| {
            tag.len() <= 32
                && tag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b' '))
        })
        .map(str::to_owned);
    let at = u32::try_from(summary.events.len()).unwrap_or(u32::MAX);
    if k == SignalKind::Compact
        && let Some(index) = summary
            .signals
            .iter()
            .enumerate()
            .rev()
            .find(|(_, signal)| !signal.k.annotates())
            .map(|(index, _)| index)
        && let Some(last) = summary.signal_mut(index)
        && last.k == SignalKind::Compact
        && last.at == at
    {
        if last.n.is_none() {
            last.n = n;
        }
        if last.v.is_none() {
            last.v = v;
        }
        if last.t.is_none() {
            last.t = t;
        }
        return;
    }
    summary.signals.push(Signal { k, o, t, at, n, v });
}

/// Adds a transcript-only marker. The viewer groups consecutive thinking
/// entries, retaining each time so a masked run can show its measured length.
fn extra(summary: &mut FileIndex, event: Event) {
    summary.extras.push(event);
}

pub(crate) fn gap(summary: &mut FileIndex, offset: u64) {
    push(
        summary,
        Event {
            k: Kind::Gap,
            o: offset,
            ..Event::default()
        },
    );
}

pub(crate) fn record_time(record: &Value) -> Option<i64> {
    field(record, "timestamp")
        .or_else(|| {
            record
                .get("payload")
                .and_then(|payload| field(payload, "timestamp"))
        })
        .and_then(parse_ms)
}

fn absolute_reset_ms(value: &Value) -> Option<i64> {
    if let Some(seconds) = value.as_i64() {
        return if seconds.unsigned_abs() >= 100_000_000_000 {
            Some(seconds)
        } else {
            seconds.checked_mul(1000)
        };
    }
    if let Some(seconds) = value.as_u64() {
        return if seconds >= 100_000_000_000 {
            i64::try_from(seconds).ok()
        } else {
            i64::try_from(seconds).ok()?.checked_mul(1000)
        };
    }
    if let Some(seconds) = value.as_f64() {
        if !seconds.is_finite() {
            return None;
        }
        let millis = if seconds.abs() >= 100_000_000_000.0 {
            seconds
        } else {
            seconds * 1000.0
        };
        return (millis >= i64::MIN as f64 && millis <= i64::MAX as f64)
            .then(|| millis.round() as i64);
    }
    value.as_str().and_then(parse_ms)
}

fn rate_limits(info: &Value, recorded_at: i64) -> Option<RateLimits> {
    let data = info.get("rate_limits")?;
    let mut by_minutes = BTreeMap::new();
    for name in ["primary", "secondary"] {
        let window = data.get(name)?;
        let minutes = window.get("window_minutes")?.as_u64()?;
        let used_percent = window.get("used_percent")?.as_f64()?;
        let resets_at = if let Some(seconds) = window.get("resets_in_seconds") {
            let seconds = seconds.as_f64()?;
            if !seconds.is_finite() {
                continue;
            }
            let millis = seconds * 1000.0;
            if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
                continue;
            }
            recorded_at.checked_add(millis.round() as i64)?
        } else {
            absolute_reset_ms(window.get("resets_at")?)?
        };
        by_minutes.insert(
            minutes,
            RateLimitWindow {
                minutes,
                used_percent,
                resets_at,
            },
        );
    }
    (!by_minutes.is_empty()).then_some(RateLimits {
        recorded_at,
        windows: by_minutes.into_values().collect(),
    })
}

fn strip_reminders(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<system-reminder>") {
        output.push_str(&rest[..start]);
        match rest[start..].find("</system-reminder>") {
            Some(end) => rest = &rest[start + end + "</system-reminder>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    output.push_str(rest);
    output.trim().to_owned()
}

pub(crate) fn clean_prompt(text: &str) -> String {
    strip_reminders(text)
}

/// `/compact` as a whole command, typed or as a command tag.
fn is_compact_command(text: &str) -> bool {
    text.strip_prefix("/compact")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        || text.contains("<command-name>/compact</command-name>")
}

/// `name="value"` attributes of an opening tag, as the harness writes them.
pub(crate) fn attributes(tag: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut rest = tag;
    while let Some(equals) = rest.find("=\"") {
        // Byte offsets from `char_indices`, so a multi-byte character in the
        // tag can't split a slice.
        let start = rest[..equals]
            .char_indices()
            .rev()
            .find(|(_, character)| !(character.is_ascii_alphanumeric() || *character == '-'))
            .map_or(0, |(at, character)| at + character.len_utf8());
        let name = rest[start..equals].to_owned();
        let value_start = equals + 2;
        let Some(length) = rest[value_start..].find('"') else {
            break;
        };
        if !name.is_empty() {
            result.insert(name, rest[value_start..value_start + length].to_owned());
        }
        rest = &rest[value_start + length + 1..];
    }
    result
}

/// The block kinds a relay or notification prompt is made of.
const RELAY_TAGS: [&str; 3] = [
    "cross-session-message",
    "agent-message",
    "task-notification",
];

/// Every `<name ...>` opening tag in `text`: where its `<` is, its attribute
/// string, and where the tag ends.
fn opening_tags<'a>(text: &'a str, name: &str) -> Vec<(usize, &'a str, usize)> {
    let open = format!("<{name}");
    let mut result = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(&open) {
        let at = from + found;
        let start = at + open.len();
        let next = text[start..].chars().next();
        if !matches!(next, Some(' ' | '>' | '\n' | '\t')) {
            from = start;
            continue;
        }
        let Some(close) = text[start..].find('>') else {
            break;
        };
        result.push((at, &text[start..start + close], start + close + 1));
        from = start + close + 1;
    }
    result
}

/// The span of every relay block in `text`, from its `<` to the end of its
/// closing tag; an opening tag with no later close spans only its opening tag,
/// so it hides nothing after it.
fn relay_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for name in RELAY_TAGS {
        let length = format!("</{name}>").len();
        let opens = opening_tags(text, name);
        for ((at, _, end), close) in opens.iter().zip(closes(text, name, &opens)) {
            spans.push((*at, close.map_or(*end, |close| close + length)));
        }
    }
    spans
}

/// Where each of `opens` (`<name ...>` tags in `text`) is closed: the start
/// of the `</name>` that balances it, counting `<name>` blocks quoted inside
/// it, so a quoted block's close doesn't end the one around it. If nested
/// openings leave it unbalanced, fall back to the first close after its
/// opening tag. `None` only when no close follows the opening tag.
fn closes(text: &str, name: &str, opens: &[(usize, &str, usize)]) -> Vec<Option<usize>> {
    let close = format!("</{name}>");
    let mut marks: Vec<(usize, bool)> = opens.iter().map(|(at, _, _)| (*at, true)).collect();
    let mut from = 0;
    while let Some(found) = text[from..].find(&close) {
        marks.push((from + found, false));
        from += found + close.len();
    }
    marks.sort_unstable();
    opens
        .iter()
        .map(|(at, _, end)| {
            let first = marks.partition_point(|(position, _)| position < at);
            let mut depth = 0usize;
            for (position, open) in &marks[first..] {
                if *open {
                    depth += 1;
                } else {
                    depth -= 1;
                    if depth == 0 {
                        return Some(*position);
                    }
                }
            }
            text[*end..].find(&close).map(|found| *end + found)
        })
        .collect()
}

/// Whether a block starting at `at` lies inside another block's span: quoted
/// in its body, not a block of the prompt.
fn nested(spans: &[(usize, usize)], at: usize) -> bool {
    spans.iter().any(|(start, end)| *start < at && at < *end)
}

/// The opening-tag attribute strings of every top-level `<name ...>` block in
/// `text`, each with where its tag ends. A block quoted inside another relay
/// block (a notification's `<result>` quoting an `<agent-message>`, say)
/// isn't one.
pub(crate) fn tags<'a>(text: &'a str, name: &str) -> Vec<(&'a str, usize)> {
    let spans = relay_spans(text);
    opening_tags(text, name)
        .into_iter()
        .filter(|(at, _, _)| !nested(&spans, *at))
        .map(|(_, attributes, end)| (attributes, end))
        .collect()
}

fn starts_with_relay_tag(text: &str) -> bool {
    let head = strip_reminders(text);
    let head = match head.split_once('\n') {
        Some((line, rest)) if line.trim() == "Another Claude session sent a message:" => rest,
        _ if head.trim() == "Another Claude session sent a message:" => "",
        _ => head.as_str(),
    }
    .trim_start();
    head.starts_with("<cross-session-message")
        || head.starts_with("<agent-message")
        || head.starts_with("<task-notification>")
}

/// The text inside `<name>…</name>`, first occurrence.
pub(crate) fn inner<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

/// The exit code in a background task notification's summary.
pub(crate) fn notification_exit(summary: &str) -> Option<i64> {
    summary
        .split_once("(exit code ")?
        .1
        .split_once(')')?
        .0
        .parse()
        .ok()
}

/// Top-level task-notification blocks: `(tool-use id, status, body)`. A
/// block quoted inside another relay block isn't one.
pub(crate) fn notifications(text: &str) -> Vec<(Option<&str>, Option<&str>, &str)> {
    let spans = relay_spans(text);
    let opens = opening_tags(text, "task-notification");
    let ends = closes(text, "task-notification", &opens);
    let mut result = Vec::new();
    for ((at, _, start), close) in opens.iter().zip(ends) {
        let Some(close) = close else {
            continue;
        };
        if nested(&spans, *at) {
            continue;
        }
        let body = &text[*start..close];
        result.push((inner(body, "tool-use-id"), inner(body, "status"), body));
    }
    result
}

fn peer_event(origin: &Value, offset: u64, time: Option<i64>) -> Event {
    if origin.get("handback").and_then(Value::as_bool) == Some(true) {
        // `senderTaskId` first; `from` is kept too when it differs, and the
        // model matches either against the agent id or the spawning call.
        let task = field(origin, "senderTaskId");
        let from = field(origin, "from");
        return Event {
            k: Kind::Agm,
            o: offset,
            t: time,
            id: task.or(from).map(str::to_owned),
            n: from
                .filter(|from| task.is_some_and(|task| task != *from))
                .map(str::to_owned),
            script: AGM_UP | AGM_FINAL,
            ..Event::default()
        };
    }
    let peer = Peer {
        sock: field(origin, "from").map(str::to_owned),
        pid: origin.get("verifiedPeerPid").and_then(Value::as_u64),
        start: origin
            .get("verifiedPeerProcStart")
            .and_then(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            }),
    };
    if let Some(task) = field(origin, "senderTaskId")
        && field(origin, "msg_id").is_none()
    {
        // A subagent's send that isn't its hand-back: to its parent (a
        // progress receipt) or to another agent. Which one, the model
        // decides from the tree. `peer.sock` keeps `from` (the agent id), as
        // when this was a relay, so the sender resolves the same way. One
        // with a `msg_id` went over the peer network and joins its send by
        // that id: it stays a relay.
        let from = field(origin, "from");
        return Event {
            k: Kind::Agm,
            o: offset,
            t: time,
            id: Some(task.to_owned()),
            n: from.filter(|from| *from != task).map(str::to_owned),
            script: AGM_UP | AGM_PROGRESS,
            peer: Some(peer),
            ..Event::default()
        };
    }
    Event {
        k: Kind::Xsm,
        o: offset,
        t: time,
        id: field(origin, "msg_id").map(str::to_owned),
        n: field(origin, "name").map(str::to_owned),
        peer: Some(peer),
        ..Event::default()
    }
}

/// Classifies one user-side prompt (a user record's text or a queued
/// command's prompt) into events. `images`: the prompt attaches an image,
/// so it is one even without text.
fn prompt_events(
    summary: &mut FileIndex,
    text: &str,
    images: bool,
    origin: Option<&Value>,
    meta: bool,
    offset: u64,
    time: Option<i64>,
) {
    match origin.and_then(|origin| field(origin, "kind")) {
        Some("peer") => {
            push(
                summary,
                peer_event(origin.expect("peer origin"), offset, time),
            );
            return;
        }
        Some("human") => {
            let text = strip_reminders(text);
            if !meta && (images || !text.is_empty()) && !is_compact_command(&text) {
                push(
                    summary,
                    Event {
                        k: Kind::Ask,
                        o: offset,
                        t: time,
                        ..Event::default()
                    },
                );
            }
            return;
        }
        Some("coordinator") => {
            push(
                summary,
                Event {
                    k: Kind::Agm,
                    o: offset,
                    t: time,
                    script: AGM_DOWN,
                    ..Event::default()
                },
            );
            return;
        }
        Some("task-notification") => {}
        Some(kind) => {
            extra(
                summary,
                Event {
                    k: Kind::Other,
                    o: offset,
                    t: time,
                    n: Some(kind.to_owned()),
                    ..Event::default()
                },
            );
            return;
        }
        _ => {}
    }
    let mut found = false;
    if starts_with_relay_tag(text) {
        for (tag, _) in tags(text, "cross-session-message") {
            let attributes = attributes(tag);
            push(
                summary,
                Event {
                    k: Kind::Xsm,
                    o: offset,
                    t: time,
                    n: attributes.get("from-name").cloned(),
                    peer: Some(Peer {
                        sock: attributes.get("from").cloned(),
                        ..Peer::default()
                    }),
                    ..Event::default()
                },
            );
            found = true;
        }
        for (tag, _) in tags(text, "agent-message") {
            push(
                summary,
                Event {
                    k: Kind::Agm,
                    o: offset,
                    t: time,
                    id: attributes(tag).get("from").cloned(),
                    script: AGM_UP,
                    ..Event::default()
                },
            );
            found = true;
        }
        for (id, status, body) in notifications(text) {
            push(
                summary,
                Event {
                    k: Kind::Tn,
                    o: offset,
                    t: time,
                    id: id.map(str::to_owned),
                    n: status.map(str::to_owned),
                    script: if !matches!(status, Some("killed" | "stopped"))
                        && (status == Some("failed")
                            || inner(body, "summary")
                                .and_then(notification_exit)
                                .is_some_and(|exit| exit != 0))
                    {
                        BACKGROUND_FAILED
                    } else {
                        0
                    },
                    ..Event::default()
                },
            );
            found = true;
        }
    }
    if found || meta {
        return;
    }
    let text = strip_reminders(text);
    if text.starts_with("[Request interrupted") {
        let kind = if text.starts_with("[Request interrupted by user for tool use") {
            "tool"
        } else {
            "user"
        };
        signal(
            summary,
            SignalKind::Interrupt,
            offset,
            time,
            Some(kind),
            None,
        );
        return;
    }
    if (text.is_empty() && !images) || text.starts_with('<') || text.starts_with('/') {
        return;
    }
    push(
        summary,
        Event {
            k: Kind::U,
            o: offset,
            t: time,
            ..Event::default()
        },
    );
}

fn text_parts(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| field(part, "type").is_none_or(|kind| kind == "text"))
            .filter_map(|part| field(part, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn result_flags(name: Option<&str>, result: Option<&Value>) -> (u8, Option<String>) {
    let mut flags = 0;
    let mut msg_id = None;
    match name {
        Some("Agent" | "Task") => {
            if result.and_then(|value| field(value, "status")) == Some("async_launched") {
                flags |= ASYNC;
            }
        }
        Some("SendMessage") => match result {
            Some(Value::String(_)) => flags |= DENIED,
            Some(value @ Value::Object(_)) => {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    flags |= SEND_FAILED;
                }
                if value.get("pin").is_some() || value.get("resumedAgentId").is_some() {
                    flags |= PIN;
                }
                msg_id = field(value, "msg_id").map(str::to_owned);
            }
            _ => {}
        },
        Some("AskUserQuestion") => match result {
            Some(Value::String(_)) => flags |= DENIED,
            Some(value) if value.get("answers").is_some_and(Value::is_object) => {
                flags |= ANSWERED;
            }
            _ => {}
        },
        _ => {}
    }
    (flags, msg_id)
}

fn resolve(summary: &mut FileIndex, id: &str, reply: impl FnOnce(Option<&str>) -> Reply) {
    let Some(index) = summary.pending.remove(id) else {
        return;
    };
    if let Some(event) = summary.event_mut(index)
        && event.k == Kind::Tool
    {
        event.r = Some(reply(event.n.as_deref()));
        event.code_mode_open = false;
    }
}

/// Stops unresolved code-mode calls from claiming items in a later turn.
fn close_code_mode(summary: &mut FileIndex) {
    for index in summary.pending.values().copied().collect::<Vec<_>>() {
        if let Some(event) = summary.event_mut(index)
            && event.k == Kind::Tool
            && event.code_mode
        {
            event.code_mode_open = false;
        }
    }
}

fn non_code_mode_tool(summary: &FileIndex, id: &str) -> bool {
    summary.tool_ids.get(id).is_some_and(|indices| {
        indices.iter().any(|index| {
            summary
                .events
                .get(*index)
                .is_some_and(|event| event.k == Kind::Tool && !event.code_mode)
        })
    })
}

/// The one code-mode call open for item attribution, if exactly one is.
fn open_code_mode_call(summary: &FileIndex) -> Option<usize> {
    let mut open = summary.pending.values().copied().filter(|index| {
        summary
            .events
            .get(*index)
            .is_some_and(|event| event.k == Kind::Tool && event.code_mode && event.code_mode_open)
    });
    let first = open.next()?;
    open.next().is_none().then_some(first)
}

/// A code-mode script failed: the harness's header, the output's first
/// element, says so. Text in later elements is the script's own output and is
/// never read as a failure.
fn script_error(output: Option<&Value>) -> bool {
    output
        .and_then(Value::as_array)
        .and_then(|elements| elements.first())
        .and_then(|header| field(header, "text"))
        .is_some_and(|text| text.trim_start().starts_with("Script error"))
}

/// What a code-mode script gives one key of a `tools` call's object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScriptArg {
    /// The object has no such key.
    Absent,
    /// A plain string literal, unescaped.
    Literal(String),
    /// A nonnegative integer literal, as used by numeric exec session ids.
    Number(u64),
    /// Anything else: a variable, an expression, a template with `${…}`, or
    /// a call whose argument isn't an object literal this reader can follow.
    Other,
}

/// One native text block, with its source-array ordinal and type retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputBlock<'a> {
    pub(crate) ordinal: usize,
    pub(crate) kind: Option<&'a str>,
    pub(crate) content: OutputBlockContent<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputBlockContent<'a> {
    Text { text: &'a str, header: bool },
    Image,
    Unknown,
}

/// The native output's original scalar or ordered content blocks. It keeps
/// boundaries until a caller explicitly needs a text rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OutputContentView<'a> {
    Text(&'a str),
    Blocks(Vec<OutputBlock<'a>>),
}

fn text_content_type(kind: Option<&str>) -> bool {
    matches!(kind, Some("text" | "input_text" | "output_text"))
}

fn successful_script_header(text: &str) -> bool {
    let text = text.trim_start();
    if text == "Script completed" {
        return true;
    }
    let Some(rest) = text.strip_prefix("Script completed\n") else {
        return false;
    };
    let Some((wall_time, rest)) = rest.split_once('\n') else {
        return false;
    };
    let Some(wall_time) = wall_time.strip_prefix("Wall time ") else {
        return false;
    };
    let success_header = if let Some(seconds) = wall_time.strip_suffix(" seconds") {
        finite_seconds(seconds, true)
    } else if let Some((total, timings)) = wall_time.split_once(" seconds (code-mode ") {
        let Some((host, overhead)) = timings.split_once(" seconds; overhead ") else {
            return false;
        };
        let Some(overhead) = overhead.strip_suffix(" seconds)") else {
            return false;
        };
        finite_seconds(total, true) && finite_seconds(host, true) && finite_seconds(overhead, false)
    } else {
        false
    };
    success_header && (rest == "Output:" || rest == "Output:\n")
}

fn finite_seconds(text: &str, nonnegative: bool) -> bool {
    text.parse::<f64>()
        .is_ok_and(|seconds| seconds.is_finite() && (!nonnegative || seconds >= 0.0))
}

pub(crate) fn output_content_view(output: &Value) -> Option<OutputContentView<'_>> {
    match output {
        Value::String(text) => Some(OutputContentView::Text(text)),
        Value::Array(parts) => Some(OutputContentView::Blocks(
            parts
                .iter()
                .enumerate()
                .map(|(ordinal, part)| {
                    let kind = field(part, "type");
                    let content = if text_content_type(kind) {
                        match field(part, "text") {
                            Some(text) => OutputBlockContent::Text {
                                text,
                                header: ordinal == 0 && successful_script_header(text),
                            },
                            None => OutputBlockContent::Unknown,
                        }
                    } else if matches!(kind, Some("image" | "input_image")) {
                        OutputBlockContent::Image
                    } else {
                        OutputBlockContent::Unknown
                    };
                    OutputBlock {
                        ordinal,
                        kind,
                        content,
                    }
                })
                .collect(),
        )),
        _ => None,
    }
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The JavaScript string literal opening at `start` (a quote): its text,
/// unescaped, or `None` for a template with `${…}`; and where it ends. An
/// unterminated literal ends at the end of `text`.
fn read_literal(text: &str, start: usize) -> (Option<String>, usize) {
    let mut chars = text[start..].char_indices();
    let Some((_, quote)) = chars.next() else {
        return (None, text.len());
    };
    let mut value = String::new();
    let mut plain = true;
    // Hex digits only: `from_str_radix` would also take a leading `+`.
    let hex = |digits: &str| {
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok()
    };
    while let Some((at, c)) = chars.next() {
        match c {
            _ if c == quote => return (plain.then_some(value), start + at + c.len_utf8()),
            '$' if quote == '`' && text[start + at..].starts_with("${") => plain = false,
            '\\' => {
                let Some((_, escaped)) = chars.next() else {
                    break;
                };
                let rest = chars.as_str();
                let code = match escaped {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'b' => Some('\u{8}'),
                    'f' => Some('\u{c}'),
                    'v' => Some('\u{b}'),
                    '0' => Some('\0'),
                    '\n' => None,
                    'x' => {
                        let code = rest.get(..2).and_then(hex).and_then(char::from_u32);
                        if code.is_some() {
                            chars.nth(1);
                        }
                        code.or(Some('x'))
                    }
                    'u' if rest.starts_with('{') => {
                        // `{`, at most six digits and `}`: never a scan
                        // of the rest of the script.
                        let close = rest
                            .char_indices()
                            .take(8)
                            .find(|(_, c)| *c == '}')
                            .map(|(close, _)| close);
                        let code = close
                            .and_then(|close| hex(&rest[1..close]))
                            .map(|code| char::from_u32(code).unwrap_or('\u{fffd}'));
                        if let (Some(close), Some(_)) = (close, code) {
                            chars.nth(close);
                        }
                        code.or(Some('u'))
                    }
                    'u' => {
                        let code = rest
                            .get(..4)
                            .and_then(hex)
                            .map(|code| char::from_u32(code).unwrap_or('\u{fffd}'));
                        if code.is_some() {
                            chars.nth(3);
                        }
                        code.or(Some('u'))
                    }
                    other => Some(other),
                };
                value.extend(code);
            }
            _ => value.push(c),
        }
    }
    (None, text.len())
}

/// The value at `start`, after a key's colon.
fn script_value(text: &str, start: usize) -> ScriptArg {
    let rest = text[start..].trim_start();
    if !rest.starts_with(['"', '\'', '`']) {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let boundary = rest[digits..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || matches!(c, ',' | '}' | ']'));
        if digits > 0
            && boundary
            && let Ok(number) = rest[..digits].parse::<u64>()
        {
            return ScriptArg::Number(number);
        }
        return ScriptArg::Other;
    }
    match read_literal(text, text.len() - rest.len()).0 {
        Some(value) => ScriptArg::Literal(value),
        None => ScriptArg::Other,
    }
}

#[derive(Default)]
struct CallMatches {
    first: Option<usize>,
    multiple: bool,
}

/// The first one or two source calls to `tool`, outside comments and string
/// literals. The scanner only distinguishes zero, one and multiple calls.
fn call_matches(script: &str, tool: &str) -> CallMatches {
    if tool.is_empty() {
        return CallMatches::default();
    }
    let mut found = CallMatches::default();
    let mut at = 0;
    while let Some(c) = script[at..].chars().next() {
        match c {
            '"' | '\'' | '`' => {
                at = read_literal(script, at).1;
                continue;
            }
            '/' if script[at..].starts_with("//") => {
                at = script[at..].find('\n').map_or(script.len(), |end| at + end);
                continue;
            }
            '/' if script[at..].starts_with("/*") => {
                at = script[at + 2..]
                    .find("*/")
                    .map_or(script.len(), |end| at + end + 4);
                continue;
            }
            _ if ident_char(c) => {
                let end = script[at..]
                    .find(|c: char| !ident_char(c))
                    .map_or(script.len(), |end| at + end);
                let word = &script[at..end];
                at = end;
                let rest = script[at..].trim_start();
                if word == tool && rest.starts_with('(') {
                    if found.first.is_some() {
                        found.multiple = true;
                        return found;
                    }
                    found.first = Some(script.len() - rest.len() + 1);
                }
                continue;
            }
            _ => {}
        }
        at += c.len_utf8();
    }
    found
}

/// Where the source's first call to `tool` opens its arguments: `tool` as a
/// whole identifier followed by `(`, outside comments and string literals.
fn call_open(script: &str, tool: &str) -> Option<usize> {
    call_matches(script, tool).first
}

/// What a code-mode script gives `key` in its first call to `tool`
/// (`tools.<tool>({key: …})`); `None` when the source never calls it. Read
/// only for what the structured output doesn't carry: a yielded command's
/// line before it completes, and the input a poll sends.
pub(crate) fn script_arg(script: &str, tool: &str, key: &str) -> Option<ScriptArg> {
    let open = call_open(script, tool)?;
    let text = &script[open..];
    if !text.trim_start().starts_with('{') {
        return Some(ScriptArg::Other);
    }
    // Depth 0 is the argument list, 1 the object literal.
    let mut depth = 0usize;
    let mut expect_key = false;
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        match c {
            '"' | '\'' | '`' => {
                let (literal, end) = read_literal(text, at);
                at = end;
                let rest = text[at..].trim_start();
                if depth == 1 && expect_key && rest.starts_with(':') {
                    if literal.as_deref() == Some(key) {
                        return Some(script_value(text, text.len() - rest.len() + 1));
                    }
                    expect_key = false;
                }
                continue;
            }
            '/' if text[at..].starts_with("//") => {
                at = text[at..].find('\n').map_or(text.len(), |end| at + end);
                continue;
            }
            '/' if text[at..].starts_with("/*") => {
                at = text[at + 2..]
                    .find("*/")
                    .map_or(text.len(), |end| at + end + 4);
                continue;
            }
            '(' | '[' | '{' => {
                depth += 1;
                expect_key = depth == 1 && c == '{';
            }
            ')' | ']' | '}' => {
                if depth == 0 {
                    return Some(ScriptArg::Absent);
                }
                depth -= 1;
                expect_key = false;
            }
            ',' if depth == 1 => expect_key = true,
            _ if depth == 1 && expect_key && ident_char(c) => {
                let end = text[at..]
                    .find(|c: char| !ident_char(c))
                    .map_or(text.len(), |end| at + end);
                let word = &text[at..end];
                at = end;
                let rest = text[at..].trim_start();
                if word == key {
                    return Some(if rest.starts_with(':') {
                        script_value(text, text.len() - rest.len() + 1)
                    } else {
                        // Shorthand `{chars}`: a variable.
                        ScriptArg::Other
                    });
                }
                expect_key = false;
                continue;
            }
            _ if depth == 1 && !c.is_whitespace() => expect_key = false,
            _ => {}
        }
        at += c.len_utf8();
    }
    // The source ends inside the call: it can't be read.
    Some(ScriptArg::Other)
}

/// The command line a code-mode script starts with `tools.exec_command`.
pub(crate) fn script_command(script: &str) -> Option<String> {
    match script_arg(script, "exec_command", "cmd")? {
        ScriptArg::Literal(command) if !command.trim().is_empty() => Some(command),
        _ => None,
    }
}

fn script_session_target(script: &str) -> Option<String> {
    match script_arg(script, "write_stdin", "session_id")? {
        ScriptArg::Literal(session) if !session.is_empty() && session.len() <= 64 => Some(session),
        ScriptArg::Number(session) => Some(session.to_string()),
        _ => None,
    }
}

/// Code-mode command operations visible in a script. Poll links require one
/// literal `write_stdin` target; mixed or multiple operations stay unowned.
fn script_flags(script: &str) -> u8 {
    let exec = call_matches(script, "exec_command");
    let stdin = call_matches(script, "write_stdin");
    let mut flags = 0;
    if exec.first.is_some() {
        flags |= EXEC_COMMAND;
    }
    if stdin.first.is_some() {
        flags |= STDIN;
    }
    if exec.multiple || stdin.multiple || (exec.first.is_some() && stdin.first.is_some()) {
        flags |= AMBIGUOUS_COMMANDS;
    }
    if stdin.first.is_some() {
        match script_arg(script, "write_stdin", "chars") {
            Some(ScriptArg::Absent) => {}
            Some(ScriptArg::Literal(chars)) if chars.is_empty() => {}
            _ => flags |= SENDS,
        }
        if matches!(
            script_arg(script, "write_stdin", "session_id"),
            Some(ScriptArg::Literal(_) | ScriptArg::Number(_))
        ) {
            flags |= LITERAL_SESSION;
        }
    }
    flags
}

const RESULT_SOURCE_BYTES_MAX: usize = 64 * 1024;
const RESULT_SOURCE_TOKENS_MAX: usize = 4096;

fn source_tokens(script: &str) -> Option<Vec<String>> {
    if script.len() > RESULT_SOURCE_BYTES_MAX {
        return None;
    }
    let bytes = script.as_bytes();
    let mut at = 0;
    let mut tokens = Vec::new();
    while at < bytes.len() {
        let c = script[at..].chars().next()?;
        if c.is_whitespace() {
            at += c.len_utf8();
            continue;
        }
        if bytes[at] == b'/' && bytes.get(at + 1) == Some(&b'/') {
            at += 2;
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        if bytes[at] == b'/' && bytes.get(at + 1) == Some(&b'*') {
            let end = script[at + 2..].find("*/")?;
            at += end + 4;
            continue;
        }
        if matches!(bytes[at], b'\'' | b'"') {
            let quote = bytes[at];
            at += 1;
            let mut closed = false;
            while at < bytes.len() {
                match bytes[at] {
                    b'\\' => at = (at + 2).min(bytes.len()),
                    byte if byte == quote => {
                        at += 1;
                        closed = true;
                        break;
                    }
                    _ => at += 1,
                }
            }
            if !closed {
                return None;
            }
            push_source_token(&mut tokens, "s".to_owned(), RESULT_SOURCE_TOKENS_MAX)?;
            continue;
        }
        if bytes[at] == b'`' {
            // Template literals may interpolate arbitrary code; this narrow
            // source recognizer does not try to model them.
            return None;
        }
        if c.is_alphabetic() || matches!(c, '_' | '$') {
            let start = at;
            at += c.len_utf8();
            while at < bytes.len() {
                let next = script[at..].chars().next()?;
                if !ident_char(next) {
                    break;
                }
                at += next.len_utf8();
            }
            push_source_token(
                &mut tokens,
                format!("i:{}", &script[start..at]),
                RESULT_SOURCE_TOKENS_MAX,
            )?;
            continue;
        }
        if c.is_ascii_digit() {
            let start = at;
            at += c.len_utf8();
            while at < bytes.len() && bytes[at].is_ascii_digit() {
                at += 1;
            }
            push_source_token(
                &mut tokens,
                format!("n:{}", &script[start..at]),
                RESULT_SOURCE_TOKENS_MAX,
            )?;
            continue;
        }
        push_source_token(&mut tokens, format!("p:{c}"), RESULT_SOURCE_TOKENS_MAX)?;
        at += c.len_utf8();
    }
    Some(tokens)
}

fn push_source_token(tokens: &mut Vec<String>, token: String, max: usize) -> Option<()> {
    if tokens.len() >= max {
        return None;
    }
    tokens.push(token);
    Some(())
}

fn matching_call_end(tokens: &[String], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.as_str() {
            "p:(" => depth += 1,
            "p:)" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn exact_statement(tokens: &[String], mut at: usize) -> bool {
    if tokens.get(at).is_some_and(|token| token == "p:;") {
        at += 1;
    }
    at == tokens.len()
}

/// A literal value in the narrow call-source grammar. Identifiers, calls,
/// spreads and operators are rejected; only JSON-like scalar/object/array
/// data can be shown to have no evaluation side effects.
fn literal_value(tokens: &[String], at: &mut usize, depth: usize) -> bool {
    if depth > 24 {
        return false;
    }
    match tokens.get(*at).map(String::as_str) {
        Some("s") | Some("i:true" | "i:false" | "i:null") => {
            *at += 1;
            true
        }
        Some(token) if token.starts_with("n:") => {
            *at += 1;
            true
        }
        Some("p:{") => {
            *at += 1;
            if tokens.get(*at).map(String::as_str) == Some("p:}") {
                *at += 1;
                return true;
            }
            let mut keys = BTreeSet::new();
            loop {
                // Quoted, escaped and computed keys are outside this source
                // grammar; duplicate unquoted keys are rejected because JS
                // evaluates the last value while the lexical scanner sees the
                // first one.
                let Some(key) = tokens
                    .get(*at)
                    .and_then(|key| key.strip_prefix("i:"))
                    .filter(|key| !key.is_empty())
                else {
                    return false;
                };
                if !keys.insert(key.to_owned()) {
                    return false;
                }
                *at += 1;
                if tokens.get(*at).map(String::as_str) != Some("p::") {
                    return false;
                }
                *at += 1;
                if !literal_value(tokens, at, depth + 1) {
                    return false;
                }
                match tokens.get(*at).map(String::as_str) {
                    Some("p:}") => {
                        *at += 1;
                        return true;
                    }
                    Some("p:,") => {
                        *at += 1;
                        if tokens.get(*at).map(String::as_str) == Some("p:}") {
                            *at += 1;
                            return true;
                        }
                    }
                    _ => return false,
                }
            }
        }
        Some("p:[") => {
            *at += 1;
            if tokens.get(*at).map(String::as_str) == Some("p:]") {
                *at += 1;
                return true;
            }
            loop {
                if !literal_value(tokens, at, depth + 1) {
                    return false;
                }
                match tokens.get(*at).map(String::as_str) {
                    Some("p:]") => {
                        *at += 1;
                        return true;
                    }
                    Some("p:,") => {
                        *at += 1;
                        if tokens.get(*at).map(String::as_str) == Some("p:]") {
                            *at += 1;
                            return true;
                        }
                    }
                    _ => return false,
                }
            }
        }
        _ => false,
    }
}

/// The call receives one statically literal object, with no argument
/// expression that can print or replace the awaited result.
fn literal_call_arguments(tokens: &[String], open: usize) -> bool {
    if tokens.get(open).map(String::as_str) != Some("p:(") {
        return false;
    }
    let Some(close) = matching_call_end(tokens, open) else {
        return false;
    };
    let mut at = open + 1;
    literal_value(tokens, &mut at, 0) && at == close
}

fn directly_prints_call(tokens: &[String], operation: &str, stringify: bool) -> bool {
    let mut prefix = vec!["i:text", "p:("];
    if stringify {
        prefix.extend(["i:JSON", "p:.", "i:stringify", "p:("]);
    }
    prefix.extend(["i:await", "i:tools", "p:."]);
    if tokens.len() < prefix.len() + 2 {
        return false;
    }
    if tokens[..prefix.len()]
        .iter()
        .map(String::as_str)
        .ne(prefix.iter().copied())
    {
        return false;
    }
    let operation_at = prefix.len();
    if tokens[operation_at] != format!("i:{operation}")
        || tokens.get(operation_at + 1).map(String::as_str) != Some("p:(")
        || !literal_call_arguments(tokens, operation_at + 1)
    {
        return false;
    }
    let Some(close) = matching_call_end(tokens, operation_at + 1) else {
        return false;
    };
    let suffix = if stringify { 2 } else { 1 };
    let end = close + 1 + suffix;
    if end > tokens.len() {
        return false;
    }
    (0..suffix).all(|index| tokens[close + 1 + index] == "p:)") && exact_statement(tokens, end)
}

fn assigned_result_is_printed(tokens: &[String], operation: &str, stringify: bool) -> bool {
    if tokens.len() < 10
        || !matches!(tokens[0].as_str(), "i:const" | "i:let")
        || !tokens[1].starts_with("i:")
        || ["p:=", "i:await", "i:tools", "p:."]
            .iter()
            .enumerate()
            .any(|(offset, expected)| tokens.get(offset + 2).map(String::as_str) != Some(*expected))
    {
        return false;
    }
    let variable = &tokens[1];
    let operation_at = 6;
    if tokens[operation_at] != format!("i:{operation}")
        || tokens.get(operation_at + 1).map(String::as_str) != Some("p:(")
        || !literal_call_arguments(tokens, operation_at + 1)
    {
        return false;
    }
    let Some(close) = matching_call_end(tokens, operation_at + 1) else {
        return false;
    };
    let mut at = close + 1;
    if tokens.get(at).map(String::as_str) != Some("p:;") {
        return false;
    }
    at += 1;
    if tokens.get(at).map(String::as_str) != Some("i:text")
        || tokens.get(at + 1).map(String::as_str) != Some("p:(")
    {
        return false;
    }
    at += 2;
    if stringify {
        if tokens.get(at).map(String::as_str) != Some("i:JSON")
            || tokens.get(at + 1).map(String::as_str) != Some("p:.")
            || tokens.get(at + 2).map(String::as_str) != Some("i:stringify")
            || tokens.get(at + 3).map(String::as_str) != Some("p:(")
        {
            return false;
        }
        at += 4;
    }
    if tokens.get(at) != Some(variable) {
        return false;
    }
    at += 1;
    let closes = if stringify { 2 } else { 1 };
    for _ in 0..closes {
        if tokens.get(at).map(String::as_str) != Some("p:)") {
            return false;
        }
        at += 1;
    }
    exact_statement(tokens, at)
}

fn result_source(script: &str) -> bool {
    if script.len() > RESULT_SOURCE_BYTES_MAX {
        return false;
    }
    let flags = script_flags(script);
    if flags & AMBIGUOUS_COMMANDS != 0 {
        return false;
    }
    let operation = match flags & (EXEC_COMMAND | STDIN) {
        EXEC_COMMAND => {
            if script_command(script).is_none() {
                return false;
            }
            "exec_command"
        }
        STDIN => {
            if script_session_target(script).is_none() {
                return false;
            }
            "write_stdin"
        }
        _ => return false,
    };
    let Some(tokens) = source_tokens(script) else {
        return false;
    };
    directly_prints_call(&tokens, operation, false)
        || directly_prints_call(&tokens, operation, true)
        || assigned_result_is_printed(&tokens, operation, false)
        || assigned_result_is_printed(&tokens, operation, true)
}

/// A code-mode script's output that is one exec result. Unknown and non-text
/// blocks make the result ambiguous instead of disappearing from the parse.
pub(crate) fn yield_json(output: Option<&Value>) -> Option<Value> {
    let text = match output_content_view(output?)? {
        OutputContentView::Text(text) => text,
        OutputContentView::Blocks(parts) => match parts.as_slice() {
            [
                OutputBlock {
                    content:
                        OutputBlockContent::Text {
                            text,
                            header: false,
                        },
                    ..
                },
            ] => *text,
            [
                OutputBlock {
                    content: OutputBlockContent::Text { header: true, .. },
                    ..
                },
                OutputBlock {
                    content:
                        OutputBlockContent::Text {
                            text,
                            header: false,
                        },
                    ..
                },
            ] => *text,
            _ => return None,
        },
    };
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    (value.get("output").is_some_and(Value::is_string)
        && value.get("wall_time_seconds").is_some_and(Value::is_number))
    .then_some(value)
}

/// An exec session id, from a yielded output's `session_id` or an item's
/// `process_id`: a number or a short string.
fn exec_session(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(number) => number.as_u64().map(|number| number.to_string()),
        Value::String(text) if !text.is_empty() && text.len() <= 64 => Some(text.clone()),
        _ => None,
    }
}

/// The exec session a code-mode script's output leaves running: the
/// command outlived its yield, so there is a session id and no exit code.
#[cfg(test)]
fn yielded_session(output: Option<&Value>) -> Option<String> {
    let value = yield_json(output)?;
    if value.get("exit_code").is_some_and(|code| !code.is_null()) {
        return None;
    }
    exec_session(value.get("session_id"))
}

/// Links one source-qualified code-mode command result to a yielded command.
/// Mixed/multiple operations and printed output without a matching source
/// operation stay visible and unowned.
fn fold_yield(summary: &mut FileIndex, index: usize, output: Option<&Value>, offset: u64) {
    let Some(event) = summary.events.get(index) else {
        return;
    };
    let (call_offset, flags, linked, result_source) =
        (event.o, event.script, event.poll, event.result_source);
    let failed = event.r.as_ref().is_some_and(|reply| reply.e);
    let starts_command = result_source && flags & EXEC_COMMAND != 0 && flags & STDIN == 0;
    let polls_command = flags & STDIN != 0
        && flags & (EXEC_COMMAND | AMBIGUOUS_COMMANDS | LITERAL_SESSION) == LITERAL_SESSION
        && result_source;
    if flags & AMBIGUOUS_COMMANDS != 0 || (!starts_command && !polls_command) {
        summary.event_mut(index).expect("event exists").poll = None;
        return;
    }
    let operations = summary
        .extras
        .iter()
        .rev()
        .take_while(|extra| extra.o > call_offset)
        .any(|extra| extra.k == Kind::Operation && extra.parent == Some(index));
    let decoded = yield_json(output);
    let output_session = decoded
        .as_ref()
        .and_then(|value| exec_session(value.get("session_id")));
    let session = decoded.as_ref().and_then(|value| {
        (!value.get("exit_code").is_some_and(|code| !code.is_null()))
            .then(|| exec_session(value.get("session_id")))
            .flatten()
    });
    if polls_command {
        // A completed poll can end a previously matched command, but only a
        // single qualifying result block may join to that command. Opaque or
        // multi-body output stays on its wrapper, even if a terminal item
        // arrived while it ran.
        if decoded.is_none() {
            summary.event_mut(index).expect("event exists").poll = None;
            return;
        }
        if output_session
            .as_deref()
            .is_some_and(|session| Some(session) != summary.events[index].poll_target.as_deref())
        {
            summary.event_mut(index).expect("event exists").poll = None;
            return;
        }
    }
    // A poll is owned only when the literal source target and the exact
    // output session id resolve to the same previously observed command.
    let known_session = session
        .as_ref()
        .filter(|_| polls_command)
        .and_then(|session| summary.yields.get(session).copied())
        .filter(|start| *start < index);
    let known = linked.filter(|start| Some(*start) == known_session);
    if operations || failed {
        summary.event_mut(index).expect("event exists").poll = None;
        if let Some(start) = known {
            push_poll(summary, start, offset);
        }
        return;
    }
    // The command's completion arrived while this poll ran: its last poll.
    let ended_here = linked.filter(|start| {
        summary
            .events
            .get(*start)
            .and_then(|found| found.y.as_ref())
            .and_then(|yielded| yielded.done.as_ref())
            .is_some_and(|done| done.o > call_offset)
    });
    let start = match session {
        None => ended_here,
        Some(_) if known.is_some() => known,
        Some(_) if ended_here.is_some() => ended_here,
        Some(_) if polls_command => None,
        // A command these logs haven't seen yet: a new start, whether this
        // single source call started it.
        Some(session) => {
            if summary.yields.len() >= YIELDS_MAX
                && !summary.yields.contains_key(&session)
                && let Some(oldest) = summary
                    .yields
                    .iter()
                    .min_by_key(|(_, start)| **start)
                    .map(|(session, _)| session.clone())
            {
                summary.yields.remove(&oldest);
            }
            summary.yields.insert(session, index);
            summary.event_mut(index).expect("event exists").poll = None;
            summary.event_mut(index).expect("event exists").y = Some(Box::default());
            return;
        }
    };
    let Some(start) = start.filter(|start| *start < index) else {
        summary.event_mut(index).expect("event exists").poll = None;
        return;
    };
    summary.event_mut(index).expect("event exists").poll = Some(start);
    push_poll(summary, start, offset);
}

/// Adds a poll's output line to the yielded command it polled, up to
/// [`POLLS_MAX`]; past that the command's output is marked cut.
fn push_poll(summary: &mut FileIndex, start: usize, offset: u64) {
    if let Some(found) = summary.event_mut(start)
        && let Some(yielded) = found.y.as_mut()
    {
        if yielded.polls.len() < POLLS_MAX {
            yielded.polls.push(offset);
        } else {
            yielded.cut = true;
        }
    }
}

/// A poll's literal source target can narrow the open set, but never create
/// an identity or choose the only unrelated session by elimination.
fn open_poll(
    summary: &FileIndex,
    flags: u8,
    target: Option<String>,
    result_source: bool,
) -> Option<usize> {
    if !result_source
        || flags & STDIN == 0
        || flags & (EXEC_COMMAND | AMBIGUOUS_COMMANDS | LITERAL_SESSION) != LITERAL_SESSION
    {
        return None;
    }
    summary.yields.get(&target?).copied()
}

fn exit_code(summary: &mut FileIndex, id: &str, code: i64, offset: u64, time: Option<i64>) {
    if summary.pending.contains_key(id) {
        resolve(summary, id, |_| Reply {
            o: offset,
            t: time,
            e: code != 0,
            ..Reply::default()
        });
        return;
    }
    // A later structured exit code settles an unknown outcome.
    if let Some(index) = summary
        .events
        .iter()
        .enumerate()
        .rev()
        .take(512)
        .find(|(_, event)| event.k == Kind::Tool && event.id.as_deref() == Some(id))
        .map(|(index, _)| index)
        && let Some(reply) = summary.event_mut(index).and_then(|event| event.r.as_mut())
    {
        reply.e = code != 0;
        reply.f &= !UNKNOWN;
    }
}

/// Links the plain call `id` to the item at `offset` that completed it.
fn link_item(summary: &mut FileIndex, id: &str, offset: u64) {
    let Some(index) = summary.tool_ids.get(id).and_then(|indices| {
        indices.iter().rev().copied().find(|index| {
            summary
                .events
                .get(*index)
                .is_some_and(|event| event.k == Kind::Tool && !event.code_mode)
        })
    }) else {
        return;
    };
    summary.event_mut(index).expect("event exists").item = Some(offset);
}

fn tool_event(summary: &mut FileIndex, event: Event) {
    if let Some(id) = &event.id {
        let index = summary.events.len();
        summary.tool_ids.entry(id.clone()).or_default().push(index);
        if event.r.is_none() {
            summary.pending.insert(id.clone(), index);
        }
    }
    summary.events.push(event);
}

/// A completed native connector call is a tool even when no legacy call was
/// emitted. Match mirrors only by their exact call id, never by tool name or
/// text. Index offsets and metadata; keep arguments/results in the source.
fn codex_connector_tool(summary: &mut FileIndex, item: &Value, offset: u64, time: Option<i64>) {
    let Some(id) = field(item, "id").filter(|id| !id.is_empty()) else {
        return;
    };
    let Some(tool) = field(item, "tool").filter(|name| !name.is_empty()) else {
        return;
    };
    let scope = field(item, "namespace").or_else(|| field(item, "server"));
    let name = scope
        .filter(|scope| !scope.is_empty())
        .map_or_else(|| tool.to_owned(), |scope| format!("{scope}.{tool}"));
    let result_error = item
        .get("result")
        .and_then(|result| result.get("isError"))
        .and_then(Value::as_bool);
    let success = item.get("success").and_then(Value::as_bool);
    let status = field(item, "status");
    let failed = success == Some(false)
        || result_error == Some(true)
        || item.get("error").is_some_and(|error| !error.is_null())
        || matches!(status, Some("failed" | "declined" | "cancelled"));
    let known =
        failed || success.is_some() || result_error.is_some() || status == Some("completed");
    let reply = Reply {
        o: offset,
        t: time,
        e: failed,
        f: if known { 0 } else { UNKNOWN },
        ..Reply::default()
    };
    if let Some(index) = summary.tool_ids.get(id).and_then(|indices| {
        indices.iter().rev().copied().find(|index| {
            summary
                .events
                .get(*index)
                .is_some_and(|event| !event.code_mode)
        })
    }) {
        summary.pending.remove(id);
        if let Some(event) = summary.event_mut(index) {
            event.r = Some(reply);
            event.code_mode_open = false;
        }
        return;
    }
    let parent = open_code_mode_call(summary);
    tool_event(
        summary,
        Event {
            k: Kind::Tool,
            o: offset,
            t: time,
            id: Some(id.to_owned()),
            n: Some(name),
            parent,
            r: Some(reply),
            ..Event::default()
        },
    );
}

/// Claude Code lines: `offset` is the line's byte offset.
fn activity(summary: &mut FileIndex, time: Option<i64>) {
    if let Some(time) = time {
        busy_add(&mut summary.busy, time);
        // Earliest and latest, not first and last line: lines aren't always
        // written in time order.
        summary.first = Some(summary.first.map_or(time, |first| first.min(time)));
        summary.last = Some(summary.last.map_or(time, |last| last.max(time)));
    }
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn claude(summary: &mut FileIndex, record: &Value, offset: u64) {
    claude_events(summary, record, offset);
    if let Some(uuid) = field(record, "uuid") {
        let indices: Vec<_> = summary
            .events
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, event)| event.o == offset)
            .map(|(index, _)| index)
            .collect();
        for index in indices {
            summary.event_mut(index).expect("event exists").u = Some(uuid.to_owned());
        }
    }
}

fn claude_events(summary: &mut FileIndex, record: &Value, offset: u64) {
    let time = record_time(record);
    activity(summary, time);
    if let Some(cwd) = field(record, "cwd") {
        summary.cwd = Some(cwd.to_owned());
    }
    if let Some(branch) = field(record, "gitBranch") {
        summary.branch = Some(branch.to_owned());
    }
    // Tokens de-duplicated by message id, as V1 counts them.
    if let Some(message) = record.get("message")
        && let (Some(id), Some(usage)) = (field(message, "id"), message.get("usage"))
    {
        let input = number(usage, "input_tokens");
        let cache_write = number(usage, "cache_creation_input_tokens");
        let cache_read = number(usage, "cache_read_input_tokens");
        let output = number(usage, "output_tokens");
        let split = usage
            .get("cache_creation")
            .and_then(Value::as_object)
            .and_then(|cache_creation| {
                let five = cache_creation
                    .get("ephemeral_5m_input_tokens")
                    .and_then(Value::as_u64)?;
                let one = cache_creation
                    .get("ephemeral_1h_input_tokens")
                    .and_then(Value::as_u64)?;
                (five.saturating_add(one) == cache_write).then_some((five, one))
            });
        let (cache_write_5m, cache_write_1h, split_unknown) = split
            .map(|(five, one)| (five, one, false))
            .unwrap_or((cache_write, 0, true));
        let web_search_requests = usage
            .get("server_tool_use")
            .and_then(|tools| tools.get("web_search_requests"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let timestamp = time;
        if let Some(dirty) = &mut summary.dirty_rows {
            dirty.usage.insert(id.to_owned());
        }
        let mut record_ids = summary
            .usage_by_id
            .get(id)
            .map(|usage| usage.record_ids.clone())
            .unwrap_or_default();
        if field(record, "type") == Some("assistant")
            && let Some(uuid) = field(record, "uuid")
        {
            record_ids.insert(uuid.to_owned());
        }
        summary.usage_by_id.insert(
            id.to_owned(),
            MessageUsage {
                record_ids,
                model: field(message, "model").map(str::to_owned),
                tokens: Tokens {
                    input: input + cache_write + cache_read,
                    cached_input: cache_read,
                    output,
                    reasoning_output: 0,
                    total: input + cache_write + cache_read + output,
                },
                model_tokens: ModelTokens {
                    input,
                    output,
                    cache_write,
                    cache_read,
                },
                billing: BillingUsage {
                    input,
                    output,
                    cache_read,
                    cache_write_5m,
                    cache_write_1h,
                    web_search_requests,
                    speed: field(usage, "speed").map(str::to_owned),
                    service_tier: field(usage, "service_tier").map(str::to_owned),
                    prompt_size: input
                        .saturating_add(cache_read)
                        .saturating_add(cache_write_5m)
                        .saturating_add(cache_write_1h),
                    timestamp,
                    split_unknown,
                },
            },
        );
    }
    if let Some(id) = field(record, "session_id")
        && Some(id) != field(record, "sessionId")
    {
        summary.links.session_ids.insert(id.to_owned());
    }
    match field(record, "type") {
        Some("continued-in") => {
            if let Some(id) = field(record, "continuedInSessionId") {
                summary.links.continued_in.insert(id.to_owned());
            }
            return;
        }
        Some("bridge-session") => {
            if let Some(id) = field(record, "bridgeSessionId") {
                summary.links.bridges.insert(id.to_owned());
            }
            return;
        }
        Some("permission-mode") => {
            setting(
                summary,
                SignalKind::Permission,
                offset,
                time,
                field(record, "permissionMode"),
            );
            return;
        }
        Some("custom-title") => {
            summary.title = field(record, "customTitle").map(str::to_owned);
            return;
        }
        Some("agent-name") => {
            summary.agent_name = field(record, "agentName").map(str::to_owned);
            return;
        }
        Some("attachment") => {
            if let Some(attachment) = record.get("attachment") {
                claude_hook(summary, attachment, offset, time);
            }
            if let Some(attachment) = record.get("attachment")
                && field(attachment, "type") == Some("queued_command")
            {
                let text = attachment.get("prompt").map(text_parts).unwrap_or_default();
                let images = attachments::parts(record).is_some_and(attachments::has_image);
                let meta = attachment.get("isMeta").and_then(Value::as_bool) == Some(true)
                    || record.get("isMeta").and_then(Value::as_bool) == Some(true);
                prompt_events(
                    summary,
                    &text,
                    images,
                    attachment.get("origin").filter(|origin| origin.is_object()),
                    meta,
                    offset,
                    time,
                );
            }
            return;
        }
        _ => {}
    }
    if summary.entrypoint.is_none() {
        summary.entrypoint = field(record, "entrypoint").map(str::to_owned);
    }
    let role = field(record, "type");
    if role == Some("system") && field(record, "subtype") == Some("compact_boundary") {
        let metadata = record.get("compactMetadata");
        signal(
            summary,
            SignalKind::Compact,
            offset,
            time,
            metadata.and_then(|metadata| field(metadata, "trigger")),
            metadata
                .and_then(|metadata| metadata.get("preTokens"))
                .and_then(Value::as_u64),
        );
    }
    if role == Some("system") && field(record, "subtype") == Some("stop_hook_summary") {
        let prevented = record.get("preventedContinuation").and_then(Value::as_bool) == Some(true);
        let errors = record
            .get("hookErrors")
            .and_then(Value::as_array)
            .is_some_and(|errors| !errors.is_empty());
        let outcome = if prevented {
            "blocked"
        } else if errors {
            "error"
        } else {
            "allowed"
        };
        hook(
            summary,
            Some("Stop"),
            outcome,
            field(record, "toolUseID"),
            offset,
            time,
        );
    }
    let Some(message) = record.get("message") else {
        return;
    };
    if role == Some("assistant")
        && let Some(model) = field(message, "model")
    {
        summary.last_model = Some(model.to_owned());
    }
    if role == Some("assistant") {
        setting(
            summary,
            SignalKind::Effort,
            offset,
            time,
            field(record, "perTurnEffort").or_else(|| field(record, "effort")),
        );
    }
    if record.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
        signal(summary, SignalKind::Compact, offset, time, None, None);
        return;
    }
    let tool_result = record.get("toolUseResult");
    let mut texts = Vec::new();
    match message.get("content") {
        Some(Value::String(text)) => texts.push(text.as_str()),
        Some(Value::Array(blocks)) => {
            for (block, item) in blocks.iter().enumerate() {
                let block = u32::try_from(block).unwrap_or(u32::MAX);
                match field(item, "type") {
                    Some("text") => {
                        let text = field(item, "text").unwrap_or("");
                        if role == Some("assistant") {
                            if !text.trim().is_empty() {
                                push(
                                    summary,
                                    Event {
                                        k: Kind::A,
                                        o: offset,
                                        b: block,
                                        t: time,
                                        ..Event::default()
                                    },
                                );
                            }
                        } else {
                            texts.push(text);
                        }
                    }
                    // Retain empty blocks too: Claude often redacts the text, but
                    // the viewer still shows a masked thought marker.
                    Some("thinking") if role == Some("assistant") => {
                        extra(
                            summary,
                            Event {
                                k: Kind::Think,
                                o: offset,
                                b: block,
                                t: time,
                                ..Event::default()
                            },
                        );
                    }
                    Some("tool_use") => tool_event(
                        summary,
                        Event {
                            k: Kind::Tool,
                            o: offset,
                            b: block,
                            t: time,
                            id: field(item, "id").map(str::to_owned),
                            n: field(item, "name").map(str::to_owned),
                            script: if field(item, "name") == Some("Bash")
                                && item
                                    .get("input")
                                    .and_then(|input| input.get("run_in_background"))
                                    .and_then(Value::as_bool)
                                    == Some(true)
                            {
                                BACKGROUND
                            } else {
                                0
                            },
                            ..Event::default()
                        },
                    ),
                    Some("tool_result") => {
                        if let Some(id) = field(item, "tool_use_id") {
                            let error = item.get("is_error").and_then(Value::as_bool) == Some(true);
                            let kind = field(record, "toolDenialKind");
                            if (error || kind.is_some())
                                && let Some(source) =
                                    denial_source(kind, item.get("content"), tool_result)
                            {
                                let call = call_index(summary, id);
                                note(
                                    summary,
                                    SignalKind::Denial,
                                    offset,
                                    time,
                                    source.to_owned(),
                                    call,
                                );
                            }
                            resolve(summary, id, |name| {
                                let (flags, msg_id) = result_flags(name, tool_result);
                                Reply {
                                    o: offset,
                                    b: block,
                                    t: time,
                                    e: error,
                                    f: flags,
                                    m: msg_id,
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let images =
        role == Some("user") && attachments::parts(record).is_some_and(attachments::has_image);
    if role == Some("user") && (images || !texts.is_empty()) {
        let text = texts.join("\n");
        let meta = record.get("isMeta").and_then(Value::as_bool) == Some(true);
        prompt_events(
            summary,
            &text,
            images,
            record.get("origin").filter(|origin| origin.is_object()),
            meta,
            offset,
            time,
        );
    }
}

/// A Codex call's structured exit code: `metadata.exit_code` of its output.
/// Output text is never read for it, so `rg exit_code` or a printed
/// "Exit code: 1" can't mark a call failed; without the field it is unknown.
/// An output that only acknowledges the call, as `request_user_input_async`
/// answers at once.
fn acknowledgement(output: Option<&Value>) -> bool {
    let parsed;
    let value = match output {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(value) => {
                parsed = value;
                &parsed
            }
            Err(_) => return false,
        },
        Some(other) => other,
        None => return false,
    };
    value
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key("accepted"))
}

fn codex_exit(output: Option<&Value>) -> Option<i64> {
    let parsed;
    let value = match output? {
        Value::String(text) => {
            parsed = serde_json::from_str::<Value>(text).ok()?;
            &parsed
        }
        other => other,
    };
    value.get("metadata")?.get("exit_code")?.as_i64()
}

/// A formatted direct-exec exit is evidence only for the exact native
/// function-call pair that owns this output. The shared formatter decoder
/// qualifies the header; a session ID by itself never establishes a result.
fn codex_direct_exec_exit(index: &FileIndex, call_id: &str, output: Option<&Value>) -> Option<i64> {
    // A repeated source ID has no unique owner, even if `pending` currently
    // points at one of its calls. Keep this guard scoped to formatter evidence;
    // legacy structured exit metadata retains its existing matching behavior.
    if !index
        .tool_ids
        .get(call_id)
        .is_some_and(|indices| indices.len() == 1)
    {
        return None;
    }
    let call = index
        .pending
        .get(call_id)
        .and_then(|at| index.events.get(*at))?;
    if call.k != Kind::Tool
        || !call.native_direct_call
        || !matches!(call.n.as_deref(), Some("exec_command" | "write_stdin"))
    {
        return None;
    }
    match crate::exec_response::decode_exec_response(output?.as_str()?)?.native_outcome()? {
        crate::exec_response::ExecResponseOutcome::Exited(code) => Some(i64::from(code)),
        crate::exec_response::ExecResponseOutcome::SessionOnly
        | crate::exec_response::ExecResponseOutcome::NoExit => None,
    }
}

/// Text Codex adds before a prompt, by its tag. Any other text is a prompt,
/// even one that starts with `<`.
const HARNESS_TAGS: [&str; 5] = [
    "environment_context",
    "user_instructions",
    "recommended_plugins",
    "user_shell_command",
    "turn_aborted",
];

fn harness_tag(text: &str) -> Option<&'static str> {
    let rest = text.trim_start().strip_prefix('<')?;
    HARNESS_TAGS.into_iter().find(|tag| {
        rest.strip_prefix(tag)
            .is_some_and(|after| after.starts_with(['>', ' ', '\n']))
    })
}

/// Codex rollout lines.
pub(crate) fn codex(summary: &mut FileIndex, record: &Value, offset: u64) {
    let time = record_time(record);
    activity(summary, time);
    let payload = &record["payload"];
    match field(record, "type") {
        Some("compacted") => signal(summary, SignalKind::Compact, offset, time, None, None),
        Some("session_meta") => {
            if field(payload, "forked_from_id").is_some() {
                // A fork's cumulative counters include inherited history. Until
                // a valid per-response report arrives its fresh usage is unknown.
                summary
                    .codex_native_usage
                    .get_or_insert_with(Default::default);
            }
            summary.entrypoint = field(payload, "originator").map(str::to_owned);
            summary.cwd = field(payload, "cwd").map(str::to_owned);
            summary.branch = payload
                .get("git")
                .and_then(|git| field(git, "branch"))
                .map(str::to_owned);
        }
        Some("event_msg") if field(payload, "type") == Some("token_count") => {
            if let Some(info) = payload.get("info") {
                if let Some(usage) = info.get("total_token_usage") {
                    let current = Tokens {
                        input: number(usage, "input_tokens"),
                        cached_input: number(usage, "cached_input_tokens"),
                        output: number(usage, "output_tokens"),
                        reasoning_output: number(usage, "reasoning_output_tokens"),
                        total: number(usage, "total_tokens"),
                    };
                    if let Some(model) = &summary.last_model {
                        let input_delta = current.input.saturating_sub(summary.codex_tokens.input);
                        let cache_read = current
                            .cached_input
                            .saturating_sub(summary.codex_tokens.cached_input);
                        let counts = ModelTokens {
                            input: input_delta.saturating_sub(cache_read),
                            output: current.output.saturating_sub(summary.codex_tokens.output),
                            cache_write: 0,
                            cache_read,
                        };
                        summary
                            .codex_tokens_by_model
                            .entry(model.clone())
                            .or_default()
                            .add(&counts);
                        summary.codex_usage_events.push(CodexUsageEvent {
                            model: model.clone(),
                            tokens: counts,
                            timestamp: time,
                        });
                    }
                    summary.codex_tokens = current;
                }
                if let Some(time) = time
                    && let Some(latest) = rate_limits(info, time)
                    && summary
                        .rate_limits
                        .as_ref()
                        .is_none_or(|previous| latest.recorded_at >= previous.recorded_at)
                {
                    summary.rate_limits = Some(latest);
                }
            }
        }
        Some("token_usage_record") => {
            if let Some(model) = summary.last_model.clone() {
                CodexNativeUsage::observe(&mut summary.codex_native_usage, payload, model, time);
            }
        }
        Some("event_msg") if field(payload, "type") == Some("context_compacted") => {
            signal(summary, SignalKind::Compact, offset, time, None, None);
        }
        // A finished command's structured exit code, and the completed
        // operations emitted by code-mode `exec`.
        Some("event_msg") if field(payload, "type") == Some("item_completed") => {
            let item = &payload["item"];
            if matches!(field(item, "type"), Some("McpToolCall" | "DynamicToolCall")) {
                codex_connector_tool(summary, item, offset, time);
            }
            if field(item, "type") == Some("ContextCompaction") {
                signal(summary, SignalKind::Compact, offset, time, None, None);
            }
            if matches!(field(item, "type"), Some("CommandExecution" | "FileChange")) {
                codex_declined(
                    summary,
                    field(item, "id"),
                    field(item, "status"),
                    offset,
                    time,
                );
            }
            let operation = matches!(field(item, "type"), Some("CommandExecution" | "FileChange"));
            let legacy_tool = field(item, "id").is_some_and(|id| non_code_mode_tool(summary, id));
            let open = open_code_mode_call(summary);
            let code = item.get("exit_code").and_then(Value::as_i64);
            let failed =
                code.is_some_and(|code| code != 0) || field(item, "status") == Some("failed");
            let known = code.is_some()
                || field(item, "status") == Some("failed")
                || field(item, "type") == Some("FileChange");
            // A yielded command's end: its `process_id` is the session id
            // the starting script answered with. A startup source and an open
            // exec wrapper do not identify a new native invocation: the old
            // startup's exit watcher can finish during a different wrapper.
            // Preserve the exact process-id join without guessing reuse.
            let yielded = if field(item, "type") == Some("CommandExecution") && !legacy_tool {
                exec_session(item.get("process_id"))
                    .and_then(|session| summary.yields.remove(&session))
            } else {
                None
            };
            if let Some(start) = yielded
                && let Some(found) = summary.event_mut(start)
                && let Some(started) = found.y.as_mut()
            {
                started.done = Some(Reply {
                    o: offset,
                    t: time,
                    e: failed,
                    f: if known { 0 } else { UNKNOWN },
                    ..Reply::default()
                });
            }
            // An item is a step of its own only when exactly one code-mode
            // call is open to own it. Otherwise it is a plain call's item, or
            // its owner is ambiguous: its wrapper, or the plain call, is the
            // step, and the item adds nothing but the exit code below.
            let parent = if operation && !legacy_tool && yielded.is_none() {
                open
            } else {
                None
            };
            if let Some(parent) = parent {
                extra(
                    summary,
                    Event {
                        k: Kind::Operation,
                        o: offset,
                        t: time,
                        id: field(item, "id").map(str::to_owned),
                        n: field(item, "type").map(str::to_owned),
                        parent: Some(parent),
                        r: Some(Reply {
                            o: offset,
                            t: time,
                            e: failed,
                            f: if known { 0 } else { UNKNOWN },
                            ..Reply::default()
                        }),
                        ..Event::default()
                    },
                );
            }
            if legacy_tool
                && field(item, "type") == Some("CommandExecution")
                && let Some(id) = field(item, "id")
            {
                link_item(summary, id, offset);
            }
            if let (Some(id), Some(code)) = (
                field(item, "id"),
                item.get("exit_code").and_then(Value::as_i64),
            ) {
                let code_mode_call = summary
                    .pending
                    .get(id)
                    .and_then(|index| summary.events.get(*index))
                    .is_some_and(|event| event.code_mode);
                if !code_mode_call {
                    exit_code(summary, id, code, offset, time);
                }
            }
        }
        Some("turn_context") => {
            if let Some(model) = field(payload, "model") {
                summary.last_model = Some(model.to_owned());
            }
            codex_settings(summary, payload, offset, time);
        }
        Some("event_msg")
            if matches!(
                field(payload, "type"),
                Some("exec_command_end" | "patch_apply_end")
            ) =>
        {
            codex_declined(
                summary,
                field(payload, "call_id"),
                field(payload, "status"),
                offset,
                time,
            );
        }
        Some("event_msg") if field(payload, "type") == Some("task_complete") => {
            close_code_mode(summary);
            summary.failed = payload.get("error").is_some_and(|error| !error.is_null());
        }
        Some("event_msg") if field(payload, "type") == Some("turn_aborted") => {
            signal(
                summary,
                SignalKind::Interrupt,
                offset,
                time,
                field(payload, "reason"),
                None,
            );
            close_code_mode(summary);
        }
        Some("response_item") => match field(payload, "type") {
            Some("message") => {
                let text = payload
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| field(part, "text"))
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                match field(payload, "role") {
                    Some("user") => {
                        close_code_mode(summary);
                        if let Some(tag) = harness_tag(&text) {
                            extra(
                                summary,
                                Event {
                                    k: Kind::Harness,
                                    o: offset,
                                    t: time,
                                    n: Some(tag.to_owned()),
                                    ..Event::default()
                                },
                            );
                        } else if !text.trim().is_empty()
                            || attachments::parts(record).is_some_and(attachments::has_image)
                        {
                            push(
                                summary,
                                Event {
                                    k: Kind::U,
                                    o: offset,
                                    t: time,
                                    ..Event::default()
                                },
                            );
                        }
                    }
                    Some("assistant") if !text.trim().is_empty() => push(
                        summary,
                        Event {
                            k: Kind::A,
                            o: offset,
                            t: time,
                            ..Event::default()
                        },
                    ),
                    _ => {}
                }
            }
            Some(
                "function_call" | "custom_tool_call" | "local_shell_call" | "tool_search_call",
            ) => {
                let code_mode = field(payload, "type") == Some("custom_tool_call")
                    && field(payload, "name") == Some("exec");
                let script_text = if code_mode {
                    field(payload, "input")
                } else {
                    None
                };
                let script = script_text.map_or(0, script_flags);
                let poll_target = script_text.and_then(script_session_target);
                let printed_result = script_text.is_some_and(result_source);
                let poll = open_poll(summary, script, poll_target.clone(), printed_result);
                tool_event(
                    summary,
                    Event {
                        k: Kind::Tool,
                        o: offset,
                        t: time,
                        id: field(payload, "call_id")
                            .or_else(|| field(payload, "id"))
                            .map(str::to_owned),
                        n: Some(
                            field(payload, "name")
                                .unwrap_or(if field(payload, "type") == Some("tool_search_call") {
                                    "tool_search"
                                } else {
                                    "shell"
                                })
                                .to_owned(),
                        ),
                        code_mode,
                        native_direct_call: field(payload, "type") == Some("function_call")
                            && field(payload, "call_id").is_some()
                            && matches!(
                                field(payload, "name"),
                                Some("exec_command" | "write_stdin")
                            )
                            && payload.get("namespace").is_none_or(Value::is_null),
                        code_mode_open: code_mode,
                        script,
                        poll_target,
                        result_source: printed_result,
                        poll,
                        ..Event::default()
                    },
                );
            }
            Some("function_call_output" | "custom_tool_call_output" | "tool_search_output") => {
                if let Some(id) = field(payload, "call_id") {
                    let output = payload.get("output").or_else(|| payload.get("tools"));
                    let direct_code = if field(payload, "type") == Some("function_call_output") {
                        codex_direct_exec_exit(summary, id, output)
                    } else {
                        None
                    };
                    let code = codex_exit(output).or(direct_code);
                    let mut flags = if code.is_none() { UNKNOWN } else { 0 };
                    if acknowledgement(output) {
                        flags |= ACK;
                    }
                    let code_mode_index = summary.pending.get(id).copied().filter(|index| {
                        summary
                            .events
                            .get(*index)
                            .is_some_and(|event| event.code_mode)
                    });
                    let code_mode_call = code_mode_index.is_some();
                    let script_output = field(payload, "type") == Some("custom_tool_call_output");
                    if script_output || !code_mode_call {
                        resolve(summary, id, |_| Reply {
                            o: offset,
                            t: time,
                            e: code.is_some_and(|code| code != 0)
                                || (code_mode_call && script_error(output)),
                            f: flags,
                            ..Reply::default()
                        });
                    }
                    if let Some(index) = code_mode_index
                        && script_output
                    {
                        fold_yield(summary, index, output, offset);
                    }
                }
            }
            Some("reasoning")
                if payload
                    .get("summary")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| field(part, "text").is_some())
                    }) =>
            {
                extra(
                    summary,
                    Event {
                        k: Kind::Think,
                        o: offset,
                        t: time,
                        ..Event::default()
                    },
                );
            }
            Some("agent_message") => {
                let author = field(payload, "author");
                let recipient = field(payload, "recipient");
                let script = match (author, recipient) {
                    (Some(author), Some(recipient)) => {
                        let up_prefix = format!("{recipient}/");
                        let down_prefix = format!("{author}/");
                        if author.starts_with(up_prefix.as_str()) {
                            AGM_UP
                        } else if recipient.starts_with(down_prefix.as_str()) {
                            AGM_DOWN
                        } else {
                            AGM_SIDE
                        }
                    }
                    _ => 0,
                };
                push(
                    summary,
                    Event {
                        k: Kind::Agm,
                        o: offset,
                        t: time,
                        id: author.map(str::to_owned),
                        script,
                        ..Event::default()
                    },
                );
            }
            _ => {}
        },
        _ => {}
    }
}

/// Native Copilot records preserve exact physical event and call identities.
pub(crate) fn copilot(summary: &mut FileIndex, record: &Value, offset: u64) {
    let Some(identity) = field(record, "id").filter(|id| !id.is_empty()) else {
        gap(summary, offset);
        return;
    };
    let data = &record["data"];
    if summary.copilot.is_none() {
        if field(record, "type") != Some("session.start") {
            return;
        }
        summary.copilot = Some(crate::copilot::Index {
            version: field(data, "copilotVersion").unwrap_or("").to_owned(),
            session_id: field(data, "sessionId").unwrap_or("").to_owned(),
            ..Default::default()
        });
    }
    if !summary
        .copilot
        .as_mut()
        .expect("initialized")
        .seen
        .insert(identity.to_owned())
    {
        return;
    }
    let time = record_time(record);
    if let Some(timestamp) = field(record, "timestamp")
        && time.is_some()
    {
        let index = summary.copilot.as_mut().expect("initialized");
        if index.first.is_none() {
            index.first = Some(timestamp.into());
        }
        index.last = Some(timestamp.into());
    }
    activity(summary, time);
    let event = Event {
        o: offset,
        t: time,
        u: Some(identity.to_owned()),
        ..Default::default()
    };
    match field(record, "type").unwrap_or("") {
        "session.start" => {
            summary.cwd = data
                .get("context")
                .and_then(|c| field(c, "cwd"))
                .map(str::to_owned);
            summary.entrypoint = Some("copilot-cli".into());
            summary.last_model = field(data, "selectedModel").map(str::to_owned);
            setting(
                summary,
                SignalKind::Model,
                offset,
                time,
                field(data, "selectedModel"),
            );
            setting(
                summary,
                SignalKind::Effort,
                offset,
                time,
                field(data, "reasoningEffort"),
            );
            summary.copilot.as_mut().expect("initialized").lifecycle = Some("session.start".into());
        }
        "session.resume" => {
            summary.copilot.as_mut().expect("initialized").lifecycle = Some("session.resume".into())
        }
        "session.model_change" => {
            let model = field(data, "newModel").or_else(|| field(data, "model"));
            setting(summary, SignalKind::Model, offset, time, model);
            if let Some(model) = model {
                summary.last_model = Some(model.to_owned());
            }
        }
        "user.message" => {
            if field(data, "content").is_some() {
                summary.events.push(Event {
                    k: Kind::U,
                    id: field(data, "messageId").map(str::to_owned),
                    ..event
                });
            }
        }
        "assistant.message" => {
            if let Some(model) = field(data, "model") {
                summary.last_model = Some(model.into());
                setting(summary, SignalKind::Model, offset, time, Some(model));
            }
            if field(data, "content").is_some_and(|text| !text.is_empty()) {
                summary.events.push(Event {
                    k: Kind::A,
                    id: field(data, "messageId").map(str::to_owned),
                    ..event.clone()
                });
            }
            for (block, request) in data
                .get("toolRequests")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let id = field(request, "toolCallId").filter(|id| !id.is_empty());
                if let Some(indices) = id.and_then(|id| summary.tool_ids.get(id)).cloned() {
                    for index in indices {
                        if let Some(event) = summary.event_mut(index) {
                            event.r = None;
                        }
                    }
                }
                // Repeated native call ids are ambiguous, never resolved by proximity.
                tool_event(
                    summary,
                    Event {
                        k: Kind::Tool,
                        b: block as u32,
                        id: id.map(str::to_owned),
                        n: field(request, "name").map(str::to_owned),
                        ..event.clone()
                    },
                );
            }
        }
        "tool.execution_start" => {
            // A start is metadata for the assistant's exact request; no fabricated
            // call from an orphan start or name/timing attribution.
        }
        "tool.execution_complete" => {
            let Some(id) = field(data, "toolCallId") else {
                return;
            };
            if summary
                .tool_ids
                .get(id)
                .is_none_or(|indices| indices.len() != 1)
            {
                return;
            }
            let success = data.get("success").and_then(Value::as_bool);
            let exit = data
                .get("shellExecution")
                .and_then(|s| s.get("exitCode"))
                .and_then(Value::as_i64);
            let denied = data.get("error").and_then(|e| field(e, "code")) == Some("denied");
            let mut flags = if denied { DENIED } else { 0 };
            if success.is_none()
                || (summary
                    .tool_ids
                    .get(id)
                    .and_then(|v| v.first())
                    .and_then(|i| summary.events.get(*i))
                    .and_then(|e| e.n.as_deref())
                    == Some("bash")
                    && exit.is_none()
                    && success == Some(true))
            {
                flags |= UNKNOWN;
            }
            if denied {
                note(
                    summary,
                    SignalKind::Denial,
                    offset,
                    time,
                    "permission_denied".into(),
                    call_index(summary, id),
                );
            }
            resolve(summary, id, |_| Reply {
                o: offset,
                t: time,
                e: success == Some(false) || exit.is_some_and(|code| code != 0),
                f: flags,
                ..Default::default()
            });
        }
        "abort" => {
            // No toolCallId: retain cancellation; pending calls remain unknown.
            setting(
                summary,
                SignalKind::Interrupt,
                offset,
                time,
                field(data, "reason"),
            );
        }
        "session.compaction_complete" => {
            note(
                summary,
                SignalKind::Compact,
                offset,
                time,
                "manual".into(),
                None,
            );
        }
        "subagent.started" | "subagent.completed" => {
            summary.extras.push(Event {
                k: Kind::Harness,
                n: field(record, "type").map(str::to_owned),
                id: field(data, "toolCallId").map(str::to_owned),
                ..event
            });
        }
        "session.shutdown" => {
            let index = summary.copilot.as_mut().expect("initialized");
            index.lifecycle = Some("session.shutdown".into());
            // Each shutdown replaces the preceding cumulative snapshot, including
            // child usage. Never add agentMetrics or compaction metrics to it.
            index.usage = crate::copilot::snapshot(data);
            index.usage_source = Some(crate::copilot::Observation {
                event_id: identity.into(),
                offset,
                timestamp: time,
            });
            index.usage_events = index
                .usage
                .as_ref()
                .map(|usage| {
                    usage
                        .by_model
                        .iter()
                        .map(|(model, tokens)| CodexUsageEvent {
                            model: model.clone(),
                            tokens: tokens.clone(),
                            timestamp: time,
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::Lines;

    #[test]
    fn configured_refusal_never_becomes_busy_or_memory_success() {
        let mut cache = EventCache {
            ..EventCache::default()
        };
        let refusal = cache
            .store_failed(&StoreError::Configured(
                crate::storage::StorageError::NamespaceUpgradePending,
            ))
            .unwrap_err();
        assert_eq!(
            refusal
                .get_ref()
                .and_then(|error| error.downcast_ref::<crate::storage::StorageError>()),
            Some(&crate::storage::StorageError::NamespaceUpgradePending)
        );
        assert!(!cache.busy);
        let mut dirty = false;
        let error = scan_file(
            Path::new("absent-raw-must-not-be-opened"),
            "codex",
            &mut cache,
            &mut dirty,
        )
        .unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<crate::storage::StorageError>()),
            Some(&crate::storage::StorageError::Unavailable)
        );
        assert!(cache.files.is_empty());
        assert!(!dirty);
        let error = cache.require_current_store().unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<crate::storage::StorageError>()),
            Some(&crate::storage::StorageError::Unavailable)
        );
        assert!(EventCache::default().require_current_store().is_err());
    }

    #[test]
    fn busy_clusters_within_five_minutes_in_any_order() {
        let mut busy = Vec::new();
        for time in [0, 60_000, 300_000 + 60_000, 1_000_000, 700_000, 2_000_000] {
            busy_add(&mut busy, time);
        }
        // Sorted: gaps of 1, 5, 5.7, 5 and 16.7 minutes.
        assert_eq!(
            busy,
            [(0, 360_000), (700_000, 1_000_000), (2_000_000, 2_000_000)]
        );
        assert_eq!(
            busy_merge(vec![(10, 20), (0, 5), (400_000, 500_000)]),
            [(0, 20), (400_000, 500_000)]
        );
    }

    #[test]
    fn timestamps_keep_milliseconds() {
        assert_eq!(
            parse_ms("2026-09-24T00:00:00.123Z"),
            Some(1_790_208_000_123)
        );
        assert_eq!(parse_ms("2026-09-24T00:00:00Z"), Some(1_790_208_000_000));
        assert_eq!(
            parse_ms("2026-09-24T08:00:00.5+08:00"),
            Some(1_790_208_000_500)
        );
    }

    #[test]
    fn attributes_and_notifications_parse() {
        let text = "<cross-session-message from=\"uds:/x.sock\" from-name=\"peer-a1\">hi</cross-session-message>";
        let found = tags(text, "cross-session-message");
        assert_eq!(found.len(), 1);
        assert_eq!(attributes(found[0].0)["from-name"], "peer-a1");
        let notes = notifications(
            "<task-notification><tool-use-id>t1</tool-use-id><status>completed</status><result>ok</result></task-notification>",
        );
        assert_eq!(notes[0].0, Some("t1"));
        assert_eq!(notes[0].1, Some("completed"));
        assert_eq!(inner(notes[0].2, "result"), Some("ok"));
    }
    #[test]
    fn non_ascii_attributes_never_split_a_character() {
        for tag in [
            " 名前=\"x\" from=\"y\"",
            " fröm=\"ü\" from-name=\"名前\"",
            " ü=\"\" from=\"z\"",
        ] {
            let parsed = attributes(tag);
            assert!(parsed.values().all(|value| !value.contains('"')));
        }
        assert_eq!(attributes(" 名前=\"x\" from=\"y\"")["from"], "y");
        assert_eq!(attributes(" x名=\"1\"").get("名"), None);
        assert_eq!(
            attributes(" fröm=\"ü\" from-name=\"名前\"")["from-name"],
            "名前"
        );
    }

    /// Each call's outcome: `Some(true)` failed, `Some(false)` succeeded,
    /// `None` unknown.
    fn codex_reply(records: &[Value]) -> Vec<Option<bool>> {
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }
        index
            .events
            .iter()
            .filter(|event| event.k == Kind::Tool)
            .map(|event| {
                let reply = event.r.as_ref().expect("resolved");
                (reply.f & UNKNOWN == 0).then_some(reply.e)
            })
            .collect()
    }

    fn call(id: &str) -> Value {
        serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":id,"arguments":"{}"}})
    }

    fn direct_call(id: &str, name: &str) -> Value {
        let arguments = match name {
            "exec_command" => serde_json::json!({"cmd":"printf fixture"}).to_string(),
            "write_stdin" => {
                let session_id = match id {
                    "negative" => 5151,
                    "running" => 6151,
                    _ => 1,
                };
                serde_json::json!({"session_id":session_id,"chars":""}).to_string()
            }
            _ => "{}".to_owned(),
        };
        serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":name,"call_id":id,"arguments":arguments}})
    }

    fn output(id: &str, output: Value) -> Value {
        serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":id,"output":output}})
    }

    #[test]
    fn codex_failure_comes_only_from_a_structured_exit_code() {
        let replies = codex_reply(&[
            call("rg"),
            output(
                "rg",
                serde_json::json!(
                    "{\"output\":\"src/a.rs:3: exit_code: 1\",\"metadata\":{\"exit_code\":0}}"
                ),
            ),
            call("text"),
            output(
                "text",
                serde_json::json!("Exit code: 1\nWall time: 0.2 seconds\nOutput:\nboom"),
            ),
            call("failed"),
            output(
                "failed",
                serde_json::json!({"output":"x","metadata":{"exit_code":2}}),
            ),
            call("item"),
            output("item", serde_json::json!("Process exited with code 1")),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"item","exit_code":1}}}),
        ]);
        assert_eq!(replies, [Some(false), None, Some(true), Some(true)]);
    }

    #[test]
    fn direct_formatter_outcomes_need_a_qualified_matching_native_call() {
        let valid_zero = "Chunk ID: a\nWall time: 0.1000 seconds\nProcess exited with code 0\nOriginal token count: 7\nOutput:\nfinished";
        let valid_nonzero =
            "Wall time: 0.2000 seconds\nProcess exited with code 3\nOutput:\nfailed";
        let valid_negative_and_running = "Chunk ID: b\nWall time: 0.3000 seconds\nProcess exited with code -7\nProcess running with session ID 5151\nOutput:\nfailed";
        let running_only =
            "Wall time: 0.4000 seconds\nProcess running with session ID 6151\nOutput:\nrunning";
        let no_process_fields =
            "Wall time: 0.4500 seconds\nOriginal token count: 2\nOutput:\nunknown";
        let both_optionals = "Wall time: 0.5000 seconds\nProcess exited with code 0\nProcess running with session ID 6161\nOutput:\nfinished";
        let body_headers = "Wall time: 0.6000 seconds\nProcess exited with code 0\nOutput:\nbody\nProcess exited with code 99\nProcess running with session ID 8888\nWall time: 9.0000 seconds\nOutput:\nopaque";
        let malformed = [
            "Wall time: 0.7000 seconds\nOriginal token count: 8\nProcess exited with code 0\nOutput:\nignored",
            "Wall time: 0.8000 seconds\nProcess exited with code 0\nProcess exited with code 1\nOutput:\nignored",
            "Wall time: 0.9000 seconds\nProcess exited with code nope\nOutput:\nignored",
            "Wall time: NaN seconds\nProcess exited with code 0\nOutput:\nignored",
            "Wall time: 1.0000 seconds\nProcess exited with code 0\nmissing boundary",
            "Wall time: 1.1000 seconds\nProcess exited with code 2147483648\nOutput:\nignored",
            "Process exited with code 0\nOutput:\nignored",
        ];
        let mut records = Vec::new();
        for (id, name, formatted) in [
            ("zero", "exec_command", valid_zero),
            ("nonzero", "exec_command", valid_nonzero),
            ("negative", "write_stdin", valid_negative_and_running),
            ("running", "write_stdin", running_only),
            ("no-fields", "exec_command", no_process_fields),
            ("both", "exec_command", both_optionals),
            ("opaque", "exec_command", body_headers),
        ] {
            records.push(direct_call(id, name));
            records.push(output(id, serde_json::json!(formatted)));
        }
        let mut null_namespace = direct_call("null-namespace", "exec_command");
        null_namespace["payload"]["namespace"] = serde_json::Value::Null;
        records.push(null_namespace);
        records.push(output("null-namespace", serde_json::json!(valid_zero)));
        for (ordinal, formatted) in malformed.iter().enumerate() {
            let id = format!("malformed-{ordinal}");
            records.push(direct_call(&id, "exec_command"));
            records.push(output(&id, serde_json::json!(formatted)));
        }
        let oversized_prefix = format!(
            "Chunk ID: {}\nWall time: 0.1000 seconds\nProcess exited with code 0\nOutput:\nignored",
            "x".repeat(2048)
        );
        records.push(direct_call("oversized-prefix", "exec_command"));
        records.push(output(
            "oversized-prefix",
            serde_json::json!(oversized_prefix),
        ));
        records.push(direct_call("unrelated", "shell"));
        records.push(output("unrelated", serde_json::json!(valid_nonzero)));
        records.push(direct_call("wrong-carrier", "exec_command"));
        records.push(serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"wrong-carrier","output":valid_nonzero}}));
        records.push(serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"code-mode","input":"text('fixture')"}}));
        records.push(serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"code-mode","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":valid_nonzero}]}}));
        let mut namespaced = direct_call("namespaced", "exec_command");
        namespaced["payload"]["namespace"] = serde_json::json!("mcp__fixture__exec");
        records.push(namespaced);
        records.push(output("namespaced", serde_json::json!(valid_zero)));

        let mut expected = vec![
            Some(false),
            Some(true),
            Some(true),
            None,
            None,
            Some(false),
            Some(false),
            Some(false),
        ];
        expected.extend(std::iter::repeat_n(None, malformed.len() + 5));
        assert_eq!(codex_reply(&records), expected);
    }

    #[test]
    fn direct_formatter_refuses_a_reused_pending_call_id() {
        let mut index = FileIndex::default();
        let records = [
            direct_call("reused", "exec_command"),
            direct_call("reused", "exec_command"),
            output(
                "reused",
                serde_json::json!(
                    "Wall time: 0.1000 seconds\nProcess exited with code 0\nOutput:\nfinished"
                ),
            ),
        ];
        for (offset, record) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }
        assert_eq!(index.events.len(), 2);
        assert!(index.events.iter().all(|event| event.native_direct_call));
        assert!(index.events[0].r.is_none());
        let reply = index.events[1]
            .r
            .as_ref()
            .expect("the output resolves the latest call");
        assert_eq!(reply.o, 2);
        assert_ne!(reply.f & UNKNOWN, 0, "the duplicate ID has no unique owner");
    }

    #[test]
    fn an_acknowledgement_is_not_an_answer() {
        let mut index = FileIndex::default();
        for (offset, record) in [
            call("ask"),
            output("ask", serde_json::json!("{\"accepted\":true}")),
            call("real"),
            output(
                "real",
                serde_json::json!("{\"accepted\":true,\"answer\":\"yes\"}"),
            ),
        ]
        .iter()
        .enumerate()
        {
            codex(&mut index, record, offset as u64);
        }
        let flags: Vec<bool> = index
            .events
            .iter()
            .map(|event| event.r.as_ref().unwrap().f & ACK != 0)
            .collect();
        assert_eq!(flags, [true, false]);
    }

    fn texts(records: &[Value]) -> Vec<(u64, u32)> {
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64 * 100);
        }
        index
            .events
            .iter()
            .filter(|event| event.k == Kind::A)
            .map(|event| (event.o, event.b))
            .collect()
    }

    fn said(id: &str, blocks: &[&str]) -> Value {
        let content: Vec<Value> = blocks
            .iter()
            .map(|text| serde_json::json!({"type":"text","text":text}))
            .collect();
        serde_json::json!({"type":"assistant","uuid":format!("u-{id}"),"message":{"id":id,"role":"assistant","content":content}})
    }

    #[test]
    fn claude_compact_boundary_and_summary_make_one_signal() {
        let records = [
            serde_json::json!({
                "type":"system",
                "subtype":"compact_boundary",
                "compactMetadata":{"trigger":"auto","preTokens":155000}
            }),
            serde_json::json!({
                "type":"user",
                "isCompactSummary":true,
                "message":{"role":"user","content":[{"type":"text","text":"summary"}]}
            }),
        ];
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
        assert_eq!(index.signals[0].o, 0);
        assert_eq!(index.signals[0].at, 0);
        assert_eq!(index.signals[0].n.as_deref(), Some("auto"));
        assert_eq!(index.signals[0].v, Some(155000));
        assert!(index.events.is_empty());
    }

    #[test]
    fn claude_compact_summary_from_old_logs_makes_a_signal() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({
                "type":"user",
                "isCompactSummary":true,
                "message":{"role":"user","content":[{"type":"text","text":"summary"}]}
            }),
            12,
        );

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
        assert_eq!(index.signals[0].o, 12);
        assert_eq!(index.signals[0].n, None);
        assert_eq!(index.signals[0].v, None);
    }

    #[test]
    fn claude_compactions_separated_by_a_prompt_make_two_signals() {
        let records = [
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":155000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":[]}}),
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"continue"}]}}),
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"manual","preTokens":140000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":[]}}),
        ];
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 2);
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| signal.at)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(index.signals[0].n.as_deref(), Some("auto"));
        assert_eq!(index.signals[1].n.as_deref(), Some("manual"));
    }

    #[test]
    fn claude_interrupts_are_signals_without_user_events() {
        for (text, expected) in [
            ("[Request interrupted by user]", "user"),
            ("[Request interrupted by user for tool use]", "tool"),
        ] {
            let mut index = FileIndex::default();
            claude(
                &mut index,
                &serde_json::json!({
                    "type":"user",
                    "message":{"role":"user","content":[{"type":"text","text":"keep this prompt"}]}
                }),
                10,
            );
            let events_before_interrupt = index.events.clone();
            claude(
                &mut index,
                &serde_json::json!({
                    "type":"user",
                    "message":{"role":"user","content":[{"type":"text","text":text}]}
                }),
                20,
            );

            assert_eq!(index.events, events_before_interrupt);
            assert_eq!(index.signals.len(), 1);
            assert_eq!(index.signals[0].k, SignalKind::Interrupt);
            assert_eq!(index.signals[0].o, 20);
            assert_eq!(index.signals[0].n.as_deref(), Some(expected));
        }
    }

    #[test]
    fn codex_compaction_records_dedupe_and_context_compaction_items_index() {
        let records = [
            serde_json::json!({"type":"compacted"}),
            serde_json::json!({"type":"event_msg","payload":{"type":"context_compacted"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"ContextCompaction"}}}),
        ];
        let mut index = FileIndex::default();
        for (offset, record) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
        assert_eq!(index.signals[0].at, 0);
        assert!(index.events.is_empty());
    }

    fn assert_one_codex_compact(record: Value) {
        let mut index = FileIndex::default();
        codex(&mut index, &record, 0);

        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].k, SignalKind::Compact);
    }

    #[test]
    fn lone_codex_compacted_record_indexes_one_compact_signal() {
        assert_one_codex_compact(serde_json::json!({"type":"compacted"}));
    }

    #[test]
    fn lone_codex_context_compacted_event_indexes_one_compact_signal() {
        assert_one_codex_compact(
            serde_json::json!({"type":"event_msg","payload":{"type":"context_compacted"}}),
        );
    }

    #[test]
    fn lone_codex_context_compaction_item_indexes_one_compact_signal() {
        assert_one_codex_compact(serde_json::json!({
            "type":"event_msg",
            "payload":{"type":"item_completed","item":{"type":"ContextCompaction"}}
        }));
    }

    #[test]
    fn codex_turn_aborts_keep_only_safe_reason_tags() {
        let long_reason = "x".repeat(33);
        let records = [
            (
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}),
                Some("interrupted"),
            ),
            (
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"bad\nreason"}}),
                None,
            ),
            (
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":long_reason}}),
                None,
            ),
        ];
        let mut index = FileIndex::default();
        for (offset, (record, _)) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }

        assert_eq!(index.signals.len(), 3);
        assert!(
            index
                .signals
                .iter()
                .all(|signal| signal.k == SignalKind::Interrupt)
        );
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| signal.n.as_deref())
                .collect::<Vec<_>>(),
            records.map(|(_, expected)| expected)
        );
    }

    fn turn_context(model: &str, effort: &str, approval: &str, sandbox: &str) -> Value {
        serde_json::json!({
            "timestamp":"2026-09-29T00:00:01Z",
            "type":"turn_context",
            "payload":{
                "model":model,
                "approval_policy":approval,
                "sandbox_policy":{"type":sandbox,"network_access":false},
                "collaboration_mode":{"mode":"default","settings":{"model":model,"reasoning_effort":effort}}
            }
        })
    }

    /// The `n` of each signal of kind `k`, in order.
    fn signal_tags(index: &FileIndex, k: SignalKind) -> Vec<&str> {
        index
            .signals
            .iter()
            .filter(|signal| signal.k == k)
            .map(|signal| signal.n.as_deref().unwrap_or(""))
            .collect()
    }

    fn attachment(body: Value) -> Value {
        serde_json::json!({"type":"attachment","timestamp":"2026-09-29T00:00:03Z","attachment":body})
    }

    fn refusal(id: &str, content: Value, is_error: bool, result: Value) -> Value {
        serde_json::json!({"type":"user","timestamp":"2026-09-29T00:00:02Z","toolUseResult":result,
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":content,"is_error":is_error}]}})
    }

    #[test]
    fn codex_turn_context_records_initial_settings_then_only_changes() {
        let mut index = FileIndex::default();
        codex(
            &mut index,
            &turn_context("gpt-a", "high", "on-request", "workspace-write"),
            10,
        );
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| (signal.k, signal.at, signal.o, signal.t.is_some()))
                .collect::<Vec<_>>(),
            [
                (SignalKind::Model, 0, 10, true),
                (SignalKind::Effort, 0, 10, true),
                (SignalKind::Approval, 0, 10, true),
                (SignalKind::Sandbox, 0, 10, true),
            ]
        );

        codex(&mut index, &call("c1"), 20);
        codex(
            &mut index,
            &turn_context("gpt-a", "high", "on-request", "workspace-write"),
            30,
        );
        assert_eq!(index.signals.len(), 4, "nothing changed");

        codex(
            &mut index,
            &turn_context("gpt-a", "medium", "on-request", "read-only"),
            40,
        );
        assert_eq!(index.signals.len(), 6);
        assert_eq!(signal_tags(&index, SignalKind::Model), ["gpt-a"]);
        assert_eq!(signal_tags(&index, SignalKind::Effort), ["high", "medium"]);
        assert_eq!(signal_tags(&index, SignalKind::Approval), ["on-request"]);
        assert_eq!(
            signal_tags(&index, SignalKind::Sandbox),
            ["workspace-write", "read-only"]
        );
        let changed = &index.signals[4..];
        assert!(
            changed
                .iter()
                .all(|signal| signal.at == 1 && signal.o == 40)
        );
    }

    #[test]
    fn codex_turn_context_missing_values_change_nothing() {
        let mut index = FileIndex::default();
        codex(
            &mut index,
            &turn_context("gpt-a", "high", "never", "read-only"),
            0,
        );
        codex(
            &mut index,
            &serde_json::json!({"type":"turn_context","payload":{"model":"gpt-a","effort":null}}),
            1,
        );
        codex(
            &mut index,
            &serde_json::json!({"type":"turn_context","payload":{}}),
            2,
        );
        assert_eq!(index.signals.len(), 4);
    }

    #[test]
    fn codex_policies_name_their_type_mode_or_single_key() {
        let mut index = FileIndex::default();
        codex(
            &mut index,
            &serde_json::json!({"type":"turn_context","payload":{
                "effort":"low",
                "approval_policy":{"granular":{"rules":true}},
                "sandbox_policy":{"mode":"danger-full-access"}
            }}),
            0,
        );
        assert_eq!(signal_tags(&index, SignalKind::Effort), ["low"]);
        assert_eq!(signal_tags(&index, SignalKind::Approval), ["granular"]);
        assert_eq!(
            signal_tags(&index, SignalKind::Sandbox),
            ["danger-full-access"]
        );
    }

    #[test]
    fn claude_assistant_records_effort_and_per_turn_overrides() {
        let assistant = |settings: Value| {
            let mut record = serde_json::json!({
                "type":"assistant",
                "timestamp":"2026-09-29T00:00:01Z",
                "message":{"role":"assistant","model":"claude-opus-5-5","content":[]}
            });
            record
                .as_object_mut()
                .unwrap()
                .extend(settings.as_object().unwrap().clone());
            record
        };
        let mut index = FileIndex::default();
        for (offset, settings) in [
            serde_json::json!({"effort":"high","perTurnEffort":null}),
            serde_json::json!({"effort":"high","perTurnEffort":"max"}),
            serde_json::json!({"effort":"high","perTurnEffort":"max"}),
        ]
        .iter()
        .enumerate()
        {
            claude(&mut index, &assistant(settings.clone()), offset as u64);
        }
        assert_eq!(signal_tags(&index, SignalKind::Effort), ["high", "max"]);
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| signal.o)
                .collect::<Vec<_>>(),
            [0, 1]
        );

        let mut missing = FileIndex::default();
        claude(&mut missing, &assistant(serde_json::json!({})), 0);
        assert!(signal_tags(&missing, SignalKind::Effort).is_empty());
    }

    #[test]
    fn claude_permission_mode_records_the_first_value_and_each_change() {
        let mode = |mode: &str| serde_json::json!({"type":"permission-mode","permissionMode":mode,"sessionId":"s"});
        let mut index = FileIndex::default();
        for (offset, record) in [
            mode("default"),
            mode("default"),
            mode("acceptEdits"),
            mode("acceptEdits"),
            mode("default"),
        ]
        .iter()
        .enumerate()
        {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(
            signal_tags(&index, SignalKind::Permission),
            ["default", "acceptEdits", "default"]
        );
        assert_eq!(index.signals.len(), 3);
        assert_eq!(
            index
                .signals
                .iter()
                .map(|signal| signal.o)
                .collect::<Vec<_>>(),
            [0, 2, 4]
        );
        assert!(index.events.is_empty());
    }

    #[test]
    fn claude_hooks_record_event_and_outcome_and_the_tool_a_block_stopped() {
        let mut index = FileIndex::default();
        let records = [
            claude_tool("toolu_1"),
            attachment(
                serde_json::json!({"type":"hook_blocking_error","hookEvent":"PreToolUse",
                "hookName":"PreToolUse:Bash","toolUseID":"toolu_1",
                "blockingError":{"blockingError":"SECRET-TEXT","command":"/x/secret.sh"}}),
            ),
            attachment(
                serde_json::json!({"type":"hook_success","hookEvent":"PreToolUse",
                "toolUseID":"toolu_1","stdout":"SECRET-TEXT","exitCode":0}),
            ),
            attachment(serde_json::json!({"type":"hook_additional_context",
                "hookEvent":"UserPromptSubmit","content":["SECRET-TEXT"]})),
            attachment(serde_json::json!({"type":"hook_non_blocking_error",
                "hookName":"PostToolUse:Edit","stderr":"SECRET-TEXT"})),
            attachment(serde_json::json!({"type":"hook_progress","hookEvent":"Stop"})),
            serde_json::json!({"type":"system","subtype":"stop_hook_summary","hookCount":1,
                "hookInfos":[{"command":"/x/secret.sh"}],"hookErrors":[],
                "preventedContinuation":true,"stopReason":"SECRET-TEXT"}),
            serde_json::json!({"type":"system","subtype":"stop_hook_summary","hookCount":1,
                "hookErrors":[],"preventedContinuation":false}),
        ];
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        // A plain success and a Stop summary that blocked nothing are not
        // signals; a run with a `toolUseID` carries its hash.
        assert_eq!(
            signal_tags(&index, SignalKind::Hook),
            [
                format!("PreToolUse:blocked#{}", short_hash("toolu_1")),
                "UserPromptSubmit:context".to_owned(),
                "PostToolUse:error".to_owned(),
                "Stop:blocked".to_owned(),
            ]
        );
        let linked = index
            .signals
            .iter()
            .map(|signal| signal.v)
            .collect::<Vec<_>>();
        assert_eq!(
            linked,
            [Some(0), None, None, None],
            "only the blocking hook names the call it stopped"
        );
        assert_eq!(index.events.len(), 1, "hooks add no transcript events");
        let text = format!("{:?}", index.signals);
        assert!(!text.contains("SECRET") && !text.contains("secret.sh"));
    }

    #[test]
    fn one_hook_run_logged_twice_is_one_signal_with_the_strongest_outcome() {
        let stop_summary = |id: &str| {
            serde_json::json!({"type":"system","subtype":"stop_hook_summary","hookCount":1,
                "hookErrors":["e"],"preventedContinuation":false,"toolUseID":id})
        };
        let mut index = FileIndex::default();
        let records = [
            // A blocking Stop hook and its summary share a `toolUseID`.
            attachment(
                serde_json::json!({"type":"hook_blocking_error","hookEvent":"Stop","toolUseID":"u-1"}),
            ),
            stop_summary("u-1"),
            // A PostToolUse success then its added context.
            attachment(
                serde_json::json!({"type":"hook_success","hookEvent":"PostToolUse","toolUseID":"toolu_2"}),
            ),
            attachment(
                serde_json::json!({"type":"hook_additional_context","hookEvent":"PostToolUse","toolUseID":"toolu_2","content":[]}),
            ),
            // The stronger line first: the weaker one adds nothing.
            attachment(
                serde_json::json!({"type":"hook_non_blocking_error","hookEvent":"PostToolUse","toolUseID":"toolu_3"}),
            ),
            attachment(
                serde_json::json!({"type":"hook_additional_context","hookEvent":"PostToolUse","toolUseID":"toolu_3","content":[]}),
            ),
            // The weaker line first: the stronger one replaces it.
            attachment(
                serde_json::json!({"type":"hook_additional_context","hookEvent":"PostToolUse","toolUseID":"toolu_5","content":[]}),
            ),
            attachment(
                serde_json::json!({"type":"hook_non_blocking_error","hookEvent":"PostToolUse","toolUseID":"toolu_5"}),
            ),
            // Another run of the same event is its own signal.
            attachment(
                serde_json::json!({"type":"hook_additional_context","hookEvent":"PostToolUse","toolUseID":"toolu_4","content":[]}),
            ),
            // The same id under another event is not the same run.
            attachment(
                serde_json::json!({"type":"hook_additional_context","hookEvent":"PreToolUse","toolUseID":"toolu_4","content":[]}),
            ),
        ];
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(
            signal_tags(&index, SignalKind::Hook),
            [
                format!("Stop:blocked#{}", short_hash("u-1")),
                format!("PostToolUse:context#{}", short_hash("toolu_2")),
                format!("PostToolUse:error#{}", short_hash("toolu_3")),
                format!("PostToolUse:error#{}", short_hash("toolu_5")),
                format!("PostToolUse:context#{}", short_hash("toolu_4")),
                format!("PreToolUse:context#{}", short_hash("toolu_4")),
            ]
        );
    }

    #[test]
    fn plain_successful_hooks_are_not_signals() {
        let mut index = FileIndex::default();
        for offset in 0..200 {
            claude(
                &mut index,
                &attachment(
                    serde_json::json!({"type":"hook_success","hookEvent":"Stop","toolUseID":format!("u-{offset}")}),
                ),
                offset,
            );
        }
        assert!(index.signals.is_empty());
    }

    #[test]
    fn claude_denied_tools_link_to_their_call() {
        let user =
            "The user doesn't want to proceed with this tool use. The tool use was rejected.";
        let mut index = FileIndex::default();
        let records = [
            claude_tool("toolu_a"),
            refusal(
                "toolu_a",
                serde_json::json!(user),
                true,
                serde_json::json!("x"),
            ),
            claude_tool("toolu_b"),
            refusal(
                "toolu_b",
                serde_json::json!([{"type":"text","text":"Permission to use Bash has been denied."}]),
                true,
                serde_json::json!("x"),
            ),
            claude_tool("toolu_c"),
            // The same words on a result that did not fail are not a denial.
            refusal("toolu_c", serde_json::json!(user), false, Value::Null),
            claude_tool("toolu_d"),
            refusal("toolu_d", serde_json::json!("exit 1"), true, Value::Null),
            claude_tool("toolu_e"),
            refusal(
                "toolu_e",
                serde_json::json!("blocked"),
                true,
                serde_json::json!("User rejected tool use"),
            ),
            // A classifier denial by wording alone, as the harness words it.
            claude_tool("toolu_f"),
            refusal(
                "toolu_f",
                serde_json::json!(
                    "Permission for this action was denied by the Claude Code auto mode classifier. Reason: made up for the test"
                ),
                true,
                serde_json::json!("Error: made up for the test"),
            ),
            // The record's own kind decides, whatever the wording.
            claude_tool("toolu_g"),
            {
                let mut record = refusal(
                    "toolu_g",
                    serde_json::json!("some other words"),
                    true,
                    Value::Null,
                );
                record["toolDenialKind"] = serde_json::json!("classifier");
                record
            },
            // The kind real logs carry, on the real wording.
            claude_tool("toolu_i"),
            {
                let mut record = refusal(
                    "toolu_i",
                    serde_json::json!(
                        "Permission for this action was denied by the Claude Code auto mode classifier."
                    ),
                    true,
                    Value::Null,
                );
                record["toolDenialKind"] = serde_json::json!("automode-blocked");
                record
            },
            claude_tool("toolu_h"),
            {
                let mut record = refusal(
                    "toolu_h",
                    serde_json::json!(
                        "Permission for this action was denied by the Claude Code auto mode classifier."
                    ),
                    true,
                    Value::Null,
                );
                record["toolDenialKind"] = serde_json::json!("user");
                record
            },
        ];
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        let denials = index
            .signals
            .iter()
            .filter(|signal| signal.k == SignalKind::Denial)
            .map(|signal| (signal.n.as_deref(), signal.v))
            .collect::<Vec<_>>();
        assert_eq!(
            denials,
            [
                (Some("user"), Some(0)),
                (Some("policy"), Some(1)),
                (Some("user"), Some(4)),
                (Some("classifier"), Some(5)),
                (Some("classifier"), Some(6)),
                (Some("classifier"), Some(7)),
                (Some("user"), Some(8)),
            ]
        );
        assert_eq!(index.events[0].id.as_deref(), Some("toolu_a"));
        assert!(index.events[0].r.as_ref().is_some_and(|reply| reply.e));
        assert!(!format!("{:?}", index.signals).contains("rejected"));
    }

    #[test]
    fn codex_declined_approval_is_one_denial_linked_to_its_call() {
        let mut index = FileIndex::default();
        let records = [
            call("c1"),
            serde_json::json!({"type":"event_msg","payload":{"type":"exec_approval_request","call_id":"c1","command":["ls"]}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"exec_command_end","call_id":"c1","status":"declined"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"c1","status":"declined"}}}),
            call("c2"),
            serde_json::json!({"type":"event_msg","payload":{"type":"exec_command_end","call_id":"c2","status":"completed"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"patch_apply_end","call_id":"gone","status":"declined"}}),
        ];
        for (offset, record) in records.iter().enumerate() {
            codex(&mut index, record, offset as u64);
        }

        let denials = index
            .signals
            .iter()
            .map(|signal| (signal.k, signal.n.as_deref(), signal.v, signal.o))
            .collect::<Vec<_>>();
        assert_eq!(
            denials,
            [
                (SignalKind::Denial, Some("declined"), Some(0), 2),
                (SignalKind::Denial, Some("declined"), None, 6),
            ]
        );
    }

    #[test]
    fn run_signal_tags_are_bounded() {
        assert_eq!(bounded(&"x".repeat(TAG_MAX + 1)).unwrap().len(), TAG_MAX);
        // Values that differ only after the cut stay different.
        let (a, b) = (
            format!("{}1", "x".repeat(80)),
            format!("{}2", "x".repeat(80)),
        );
        assert_ne!(bounded(&a), bounded(&b));
        assert!(bounded(&a).unwrap().ends_with(&short_hash(&a)));
        assert_eq!(bounded(&"y".repeat(TAG_MAX)).unwrap(), "y".repeat(TAG_MAX));
        assert_eq!(
            bounded("claude-opus-5[1m]").as_deref(),
            Some("claude-opus-5[1m]")
        );
        assert_eq!(bounded("bad\nvalue"), None);
        assert_eq!(bounded("say \"hi\""), None);
        assert_eq!(bounded(""), None);

        let mut index = FileIndex::default();
        let long = "m".repeat(200);
        codex(
            &mut index,
            &turn_context(&long, &"e".repeat(200), &"a".repeat(200), &"s".repeat(200)),
            0,
        );
        codex(
            &mut index,
            &turn_context("bad\nmodel", "high", "never", "read-only"),
            1,
        );
        claude(
            &mut index,
            &serde_json::json!({"type":"permission-mode","permissionMode":"p".repeat(200)}),
            2,
        );
        claude(
            &mut index,
            &attachment(
                serde_json::json!({"type":"hook_blocking_error","hookEvent":"E".repeat(200)}),
            ),
            3,
        );
        claude(
            &mut index,
            &attachment(serde_json::json!({"type":"hook_success","hookEvent":"with\nnewline"})),
            4,
        );

        let model = signal_tags(&index, SignalKind::Model);
        assert_eq!(model, [bounded(&long).unwrap().as_str()]);
        assert!(model[0].starts_with(&"m".repeat(TAG_MAX - 9)));
        let permission = signal_tags(&index, SignalKind::Permission);
        assert_eq!(permission.len(), 1);
        assert_eq!(permission[0].len(), TAG_MAX);
        let expected_hash = short_hash(&"p".repeat(200));
        assert!(permission[0].starts_with(&"p".repeat(TAG_MAX - 9)));
        assert!(permission[0].ends_with(&format!("#{expected_hash}")));
        let hooks = signal_tags(&index, SignalKind::Hook);
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].ends_with(":blocked"));
        for signal in &index.signals {
            assert!(signal.n.as_deref().is_some_and(|n| n.len() <= TAG_MAX));
        }
    }

    #[test]
    fn run_signals_between_a_compaction_boundary_and_its_summary_still_make_one() {
        let mut index = FileIndex::default();
        let records = [
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":9}}),
            serde_json::json!({"type":"permission-mode","permissionMode":"default"}),
            attachment(
                serde_json::json!({"type":"hook_additional_context","hookEvent":"SessionStart","content":[]}),
            ),
            serde_json::json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":[]}}),
        ];
        for (offset, record) in records.iter().enumerate() {
            claude(&mut index, record, offset as u64);
        }

        assert_eq!(signal_tags(&index, SignalKind::Compact).len(), 1);
        assert_eq!(index.signals.len(), 3);
    }

    #[test]
    fn run_signals_round_trip_through_the_index() {
        let root = scratch("run-signals");
        let v1 = root.join("index.json");
        let log = root.join("session.jsonl");
        write_lines(
            &log,
            &[
                serde_json::json!({"type":"permission-mode","permissionMode":"acceptEdits"}),
                claude_tool("toolu_1"),
                attachment(
                    serde_json::json!({"type":"hook_blocking_error","hookEvent":"PreToolUse","toolUseID":"toolu_1"}),
                ),
                refusal(
                    "toolu_1",
                    serde_json::json!("The user doesn't want to proceed with this tool use."),
                    true,
                    Value::Null,
                ),
            ],
        );
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        let index = scan_file(&log, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(signal_tags(&index, SignalKind::Permission), ["acceptEdits"]);
        assert_eq!(
            signal_tags(&index, SignalKind::Hook),
            [format!("PreToolUse:blocked#{}", short_hash("toolu_1"))]
        );
        assert_eq!(signal_tags(&index, SignalKind::Denial), ["user"]);
        assert_eq!(stored(&v1, &log), cold(&log));

        let rollout = root.join("rollout.jsonl");
        write_lines(
            &rollout,
            &[
                turn_context("gpt-a", "high", "on-request", "workspace-write"),
                call("c1"),
                serde_json::json!({"type":"event_msg","payload":{"type":"exec_command_end","call_id":"c1","status":"declined"}}),
            ],
        );
        let index = scan_file(&rollout, "codex", &mut cache, &mut false).unwrap();
        assert_eq!(index.signals.len(), 5);
        let mut fresh =
            EventCache::open(&root.join("fresh.json"), &crate::Options::default().source).unwrap();
        let fresh_index = scan_file(&rollout, "codex", &mut fresh, &mut false).unwrap();
        let reopened = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        let key = rollout.to_string_lossy();
        let (_, persisted) = reopened
            .store
            .as_ref()
            .unwrap()
            .load_file(&key)
            .unwrap()
            .expect("the rollout is stored");
        assert_eq!(index_snapshot(&persisted), index_snapshot(&fresh_index));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn signals_round_trip_through_the_index() {
        let root = scratch("signals");
        let v1 = root.join("index.json");
        let log = root.join("session.jsonl");
        write_lines(
            &log,
            &[
                serde_json::json!({
                    "type":"system",
                    "subtype":"compact_boundary",
                    "compactMetadata":{"trigger":"auto","preTokens":155000}
                }),
                said("m1", &["after the compaction"]),
            ],
        );
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        let index = scan_file(&log, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(index.signals.len(), 1);
        assert_eq!(index.signals[0].n.as_deref(), Some("auto"));
        assert_eq!(stored(&v1, &log), cold(&log));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_assistant_text_is_its_own_event() {
        // Two text blocks in one message.
        assert_eq!(texts(&[said("m1", &["first", "second"])]), [(0, 0), (0, 1)]);
        // Two consecutive messages, and one message streamed over two lines
        // (Claude Code writes a line per block, with the same message id).
        assert_eq!(
            texts(&[
                said("m1", &["one"]),
                said("m2", &["two"]),
                said("m3", &["three"]),
                said("m3", &["four"]),
            ]),
            [(0, 0), (100, 0), (200, 0), (300, 0)]
        );
    }

    #[test]
    fn claude_events_carry_their_record_uuid() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"assistant","uuid":"u-1","message":{"role":"assistant","content":[
                {"type":"text","text":"hi"},{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}),
            0,
        );
        assert!(
            index
                .events
                .iter()
                .all(|event| event.u.as_deref() == Some("u-1"))
        );
        assert_eq!(index.events.len(), 2);
    }

    #[test]
    fn assistant_billing_facts_split_cache_writes_and_keep_thinking_in_output_once() {
        let mut index = FileIndex::default();
        let record = serde_json::json!({
            "type":"assistant",
            "timestamp":"2026-09-28T12:00:00.000Z",
            "message":{
                "id":"billing-1",
                "model":"claude-opus-5",
                "usage":{
                    "input_tokens":2,
                    "cache_creation_input_tokens":15,
                    "cache_read_input_tokens":20,
                    "output_tokens":7,
                    "output_tokens_details":{"thinking_tokens":6},
                    "server_tool_use":{"web_search_requests":3},
                    "service_tier":"standard",
                    "speed":"fast",
                    "cache_creation":{"ephemeral_1h_input_tokens":10,"ephemeral_5m_input_tokens":5}
                }
            }
        });
        claude(&mut index, &record, 0);
        let fact = index.usage().get("billing-1").unwrap();
        assert_eq!(fact.model_tokens.output, 7);
        assert_eq!(fact.billing.cache_write_5m, 5);
        assert_eq!(fact.billing.cache_write_1h, 10);
        assert_eq!(fact.billing.prompt_size, 37);
        assert_eq!(fact.billing.web_search_requests, 3);
        assert_eq!(fact.billing.speed.as_deref(), Some("fast"));
        assert_eq!(fact.billing.service_tier.as_deref(), Some("standard"));
        assert!(!fact.billing.split_unknown);

        let fallback = serde_json::json!({
            "type":"assistant",
            "timestamp":"2026-09-28T12:01:00.000Z",
            "message":{"id":"billing-2","model":"claude-opus-5","usage":{
                "input_tokens":1,"cache_creation_input_tokens":9,"cache_read_input_tokens":0,"output_tokens":2
            }}
        });
        claude(&mut index, &fallback, 1);
        let fact = index.usage().get("billing-2").unwrap();
        assert_eq!(fact.billing.cache_write_5m, 9);
        assert_eq!(fact.billing.cache_write_1h, 0);
        assert!(fact.billing.split_unknown);
    }

    #[test]
    fn claude_state_reader_keeps_allowlisted_snapshots_across_overwrites() {
        let root = scratch("reported-runs");
        let claude_json = root.join(".claude.json");
        let v1 = root.join("index.json");
        let secrets = ["fixture-secret@example.invalid", "fixture-account-uuid"];
        let first = serde_json::json!({
            "oauthAccount":{"emailAddress":secrets[0],"accountUuid":secrets[1]},
            "projects":{"/private/project/path":{
                "lastSessionId":"run-one","lastStartTime":1000,"lastCost":1.25,
                "lastDuration":2000,"lastAPIDuration":1500,"lastToolDuration":300,
                "lastLinesAdded":4,"lastLinesRemoved":2,"projectSecret":"unlisted-project-value",
                "lastModelUsage":{"claude-opus-5[1m]":{
                    "inputTokens":10,"outputTokens":5,"thinkingTokens":3,
                    "cacheReadInputTokens":20,"cacheCreationInputTokens":4,
                    "webSearchRequests":1,"costUSD":1.25,"accountSecret":"unlisted-model-value"
                }}
            }},
            "unlistedRootSecret":"unlisted-root-value"
        });
        fs::write(&claude_json, first.to_string()).unwrap();
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        let mut dirty = false;
        cache
            .refresh_reported_runs(&claude_json, 2000, &mut dirty)
            .unwrap();
        assert!(dirty);
        drop(cache);
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        cache
            .select_reported_runs(&BTreeSet::from(["run-one".to_owned()]))
            .unwrap();
        assert_eq!(cache.reported_runs().count(), 1);
        let bytes = store_text(&v1);
        for secret in secrets.into_iter().chain([
            "unlisted-project-value",
            "unlisted-model-value",
            "unlisted-root-value",
            "/private/project/path",
        ]) {
            assert!(!bytes.contains(secret));
        }
        let snapshot = cache.reported_runs().next().unwrap();
        assert_eq!(
            snapshot.last_model_usage["claude-opus-5[1m]"].thinking_tokens,
            3
        );
        assert_eq!(snapshot.last_cost, Some(1.25));

        let second = serde_json::json!({
            "projects":{"/private/project/path":{
                "lastSessionId":"run-two","lastStartTime":3000,"lastCost":2.0,
                "lastDuration":4000,"lastAPIDuration":3000,"lastToolDuration":600,
                "lastLinesAdded":8,"lastLinesRemoved":1,"lastModelUsage":{}
            }},
            "oauthAccount":{"emailAddress":secrets[0],"accountUuid":secrets[1]}
        });
        fs::write(&claude_json, second.to_string()).unwrap();
        cache
            .refresh_reported_runs(&claude_json, 5000, &mut dirty)
            .unwrap();
        cache
            .select_reported_runs(&BTreeSet::from([
                "run-one".to_owned(),
                "run-two".to_owned(),
            ]))
            .unwrap();
        assert_eq!(cache.reported_runs().count(), 2);
        drop(cache);
        let mut reopened = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        reopened
            .select_reported_runs(&BTreeSet::from([
                "run-one".to_owned(),
                "run-two".to_owned(),
            ]))
            .unwrap();
        let ids: BTreeSet<_> = reopened
            .reported_runs()
            .map(|snapshot| snapshot.last_session_id.as_str())
            .collect();
        assert_eq!(ids, ["run-one", "run-two"].into_iter().collect());
        // The stamp is kept too: an unchanged file isn't read again.
        assert!(reopened.claude_json_stamp.is_some());
        let bytes = store_text(&v1);
        assert!(!bytes.contains(secrets[0]));
        assert!(!bytes.contains(secrets[1]));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_unreadable_claude_state_is_an_empty_input() {
        let root = scratch("reported-runs-missing");
        let mut cache =
            EventCache::open(&root.join("index.json"), &crate::Options::default().source).unwrap();
        let mut dirty = false;
        cache
            .refresh_reported_runs(&root.join("missing.json"), 1000, &mut dirty)
            .unwrap();
        assert!(cache.reported_runs().next().is_none());
        fs::create_dir(root.join("unreadable.json")).unwrap();
        cache
            .refresh_reported_runs(&root.join("unreadable.json"), 2000, &mut dirty)
            .unwrap();
        assert!(cache.reported_runs().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_hand_back_keeps_its_sender_task_id_and_from() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","origin":{"kind":"peer","handback":true,"from":"lead","senderTaskId":"a1","body":"done"},"message":{"role":"user","content":"x"}}),
            0,
        );
        assert_eq!(index.events[0].id.as_deref(), Some("a1"));
        assert_eq!(index.events[0].n.as_deref(), Some("lead"));
    }

    #[test]
    fn a_hand_back_is_up_and_final() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","origin":{"kind":"peer","handback":true,"from":"lead","senderTaskId":"a1","body":"done"},"message":{"role":"user","content":"x"}}),
            0,
        );
        assert_eq!(index.events.len(), 1);
        assert_eq!(index.events[0].script, AGM_UP | AGM_FINAL);
    }

    #[test]
    fn a_coordinator_message_is_a_down_agent_message() {
        for record in [
            serde_json::json!({"type":"user","isMeta":true,"origin":{"kind":"coordinator"},"message":{"role":"user","content":"The coordinator sent a message while you were working:\nrebase and push"}}),
            serde_json::json!({"type":"user","origin":{"kind":"coordinator"},"message":{"role":"user","content":"The coordinator sent a message while you were working:\nrebase and push"}}),
        ] {
            let mut index = FileIndex::default();
            claude(&mut index, &record, 0);
            assert_eq!(index.events.len(), 1);
            assert_eq!(index.events[0].k, Kind::Agm);
            assert_eq!(index.events[0].script, AGM_DOWN);
        }
    }

    #[test]
    fn an_unknown_origin_kind_is_kept_as_other() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","isMeta":true,"origin":{"kind":"future-thing"},"message":{"role":"user","content":"synthetic unknown-origin record"}}),
            0,
        );
        assert!(index.events.is_empty());
        assert_eq!(index.extras.len(), 1);
        assert_eq!(index.extras[0].k, Kind::Other);
        assert_eq!(index.extras[0].n.as_deref(), Some("future-thing"));
    }

    #[test]
    fn a_task_notification_origin_still_parses_its_block() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification><tool-use-id>t1</tool-use-id><status>completed</status></task-notification>"}}),
            0,
        );
        assert_eq!(index.events.len(), 1);
        assert_eq!(index.events[0].k, Kind::Tn);
        assert_eq!(index.events[0].id.as_deref(), Some("t1"));
    }

    #[test]
    fn a_prompt_quoting_relay_tags_stays_a_prompt() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","message":{"role":"user","content":"Review this log:\n<task-notification><tool-use-id>t1</tool-use-id><status>completed</status></task-notification>\n<agent-message from=\"a1\">x</agent-message>"}}),
            0,
        );
        assert_eq!(index.events.len(), 1);
        assert_eq!(index.events[0].k, Kind::U);
        assert!(
            !index
                .events
                .iter()
                .any(|event| matches!(event.k, Kind::Tn | Kind::Agm))
        );
    }

    #[test]
    fn a_prompt_starting_with_tags_still_classifies() {
        let mut agent = FileIndex::default();
        claude(
            &mut agent,
            &serde_json::json!({"type":"user","message":{"role":"user","content":"Another Claude session sent a message:\n<agent-message from=\"a1\">done</agent-message>"}}),
            0,
        );
        assert_eq!(agent.events.len(), 1);
        assert_eq!(agent.events[0].k, Kind::Agm);
        assert_eq!(agent.events[0].script, AGM_UP);

        let mut notifications = FileIndex::default();
        claude(
            &mut notifications,
            &serde_json::json!({"type":"user","message":{"role":"user","content":"<task-notification><tool-use-id>t1</tool-use-id><status>completed</status></task-notification><task-notification><tool-use-id>t2</tool-use-id><status>failed</status></task-notification>"}}),
            0,
        );
        assert_eq!(notifications.events.len(), 2);
        assert!(notifications.events.iter().all(|event| event.k == Kind::Tn));
    }

    #[test]
    fn a_subagent_send_that_is_not_its_hand_back_is_up_progress() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","origin":{"kind":"peer","from":"agent-7","senderTaskId":"agent-7","name":"general-purpose"},"message":{"role":"user","content":"<agent-message from=\"agent-7\">halfway</agent-message>"}}),
            0,
        );
        assert_eq!(index.events.len(), 1);
        let event = &index.events[0];
        assert_eq!(event.k, Kind::Agm);
        assert_eq!(event.script, AGM_UP | AGM_PROGRESS);
        assert_eq!(event.id.as_deref(), Some("agent-7"));
        // `from` is kept only when it differs from `senderTaskId`.
        assert_eq!(event.n, None);
        assert_eq!(
            event.peer.as_ref().and_then(|peer| peer.sock.as_deref()),
            Some("agent-7")
        );

        let mut differs = FileIndex::default();
        claude(
            &mut differs,
            &serde_json::json!({"type":"user","origin":{"kind":"peer","from":"agent-7","senderTaskId":"call-7"},"message":{"role":"user","content":"x"}}),
            0,
        );
        assert_eq!(differs.events[0].k, Kind::Agm);
        assert_eq!(differs.events[0].script, AGM_UP | AGM_PROGRESS);
        assert_eq!(differs.events[0].id.as_deref(), Some("call-7"));
        assert_eq!(differs.events[0].n.as_deref(), Some("agent-7"));

        // A socket peer without `senderTaskId` is still a relay.
        let mut relay = FileIndex::default();
        claude(
            &mut relay,
            &serde_json::json!({"type":"user","origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/5.sock","name":"peer","msg_id":"m5"},"message":{"role":"user","content":"x"}}),
            0,
        );
        assert_eq!(relay.events[0].k, Kind::Xsm);
        assert_eq!(relay.events[0].id.as_deref(), Some("m5"));
        assert_eq!(relay.events[0].script, 0);
    }

    #[test]
    fn a_relay_tag_quoted_inside_another_block_is_not_its_own_event() {
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","message":{"role":"user","content":"<task-notification><tool-use-id>t1</tool-use-id><status>completed</status><result>The log said <agent-message from=\"x\">quoted</agent-message> and <task-notification><tool-use-id>t9</tool-use-id><status>failed</status></task-notification></result></task-notification>"}}),
            0,
        );
        assert_eq!(index.events.len(), 1, "{:?}", index.events);
        assert_eq!(index.events[0].k, Kind::Tn);
        assert_eq!(index.events[0].id.as_deref(), Some("t1"));
        assert!(index.events.iter().all(|event| event.k != Kind::Agm));

        // Top-level blocks side by side are each their own.
        let text = "<agent-message from=\"a\">one</agent-message>\n<agent-message from=\"b\">two <cross-session-message from=\"s\">quoted</cross-session-message></agent-message>";
        let found: Vec<_> = tags(text, "agent-message")
            .into_iter()
            .map(|(tag, _)| attributes(tag)["from"].clone())
            .collect();
        assert_eq!(found, ["a", "b"]);
        assert!(tags(text, "cross-session-message").is_empty());
    }

    #[test]
    fn a_block_quoted_after_a_same_name_block_inside_a_notification_stays_quoted() {
        // The inner notification's close doesn't end the outer one, so the
        // agent message quoted after it is still inside.
        let text = "<task-notification><tool-use-id>t1</tool-use-id><status>completed</status><result>quoted <task-notification><tool-use-id>t9</tool-use-id><status>failed</status></task-notification> then <agent-message from=\"x\">leak</agent-message></result></task-notification>";
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","message":{"role":"user","content":text}}),
            0,
        );
        assert_eq!(index.events.len(), 1, "{:?}", index.events);
        assert_eq!(index.events[0].k, Kind::Tn);
        assert_eq!(index.events[0].id.as_deref(), Some("t1"));
        let found = notifications(text);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].0, found[0].1), (Some("t1"), Some("completed")));
        assert!(found[0].2.ends_with("</agent-message></result>"));
    }

    #[test]
    fn an_unclosed_quoted_notification_keeps_the_outer_notification() {
        let text = "<task-notification><tool-use-id>t1</tool-use-id><status>completed</status><result>quoted <task-notification></result></task-notification>";
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","message":{"role":"user","content":text}}),
            0,
        );
        assert_eq!(index.events.len(), 1, "{:?}", index.events);
        assert_eq!(index.events[0].k, Kind::Tn);
        assert_eq!(index.events[0].id.as_deref(), Some("t1"));
        assert_eq!(index.events[0].n.as_deref(), Some("completed"));
    }

    #[test]
    fn an_unclosed_quoted_agent_message_keeps_the_outer_sender() {
        let text = "<agent-message from=\"a1\">quoted <agent-message from=\"x\">unfinished</agent-message>";
        let mut index = FileIndex::default();
        claude(
            &mut index,
            &serde_json::json!({"type":"user","message":{"role":"user","content":text}}),
            0,
        );
        assert_eq!(index.events.len(), 1, "{:?}", index.events);
        assert_eq!(index.events[0].k, Kind::Agm);
        assert_eq!(index.events[0].id.as_deref(), Some("a1"));
    }

    #[test]
    fn a_codex_agent_message_has_a_direction() {
        for (record, direction, author) in [
            (
                serde_json::json!({"type":"response_item","payload":{"type":"agent_message","author":"root/worker","recipient":"root","message":"done"}}),
                AGM_UP,
                "root/worker",
            ),
            (
                serde_json::json!({"type":"response_item","payload":{"type":"agent_message","author":"root","recipient":"root/worker","message":"continue"}}),
                AGM_DOWN,
                "root",
            ),
            (
                serde_json::json!({"type":"response_item","payload":{"type":"agent_message","author":"root/a","recipient":"root/b","message":"side note"}}),
                AGM_SIDE,
                "root/a",
            ),
        ] {
            let mut index = FileIndex::default();
            codex(&mut index, &record, 0);
            assert_eq!(index.events.len(), 1);
            assert_eq!(index.events[0].k, Kind::Agm);
            assert_eq!(index.events[0].script, direction);
            assert_eq!(index.events[0].id.as_deref(), Some(author));
        }
    }

    #[test]
    fn only_a_whole_compact_command_is_skipped() {
        for (text, skipped) in [
            ("/compact", true),
            ("/compact keep tests", true),
            ("/compactor", false),
            ("<command-name>/compact</command-name>", true),
        ] {
            assert_eq!(is_compact_command(text), skipped, "{text}");
        }
    }

    #[test]
    fn a_script_argument_is_read_only_from_a_plain_literal() {
        let literal = |text: &str| Some(ScriptArg::Literal(text.to_owned()));
        for (script, tool, key, expected) in [
            (
                "await tools.write_stdin({session_id:1,chars:\"\"})",
                "write_stdin",
                "chars",
                literal(""),
            ),
            (
                "tools.write_stdin({session_id: 1, \"chars\": 'y\\n'})",
                "write_stdin",
                "chars",
                literal("y\n"),
            ),
            (
                "tools.write_stdin({session_id: 26789, chars: \"\"})",
                "write_stdin",
                "session_id",
                Some(ScriptArg::Number(26789)),
            ),
            (
                "tools.write_stdin({session_id:1})",
                "write_stdin",
                "chars",
                Some(ScriptArg::Absent),
            ),
            (
                "tools.write_stdin({session_id:1, chars: input})",
                "write_stdin",
                "chars",
                Some(ScriptArg::Other),
            ),
            (
                "tools.write_stdin({chars})",
                "write_stdin",
                "chars",
                Some(ScriptArg::Other),
            ),
            (
                "tools.write_stdin(args)",
                "write_stdin",
                "chars",
                Some(ScriptArg::Other),
            ),
            (
                "tools.exec_command({cmd:\"echo \\\"hi\\\" \\u0041\\x42\\u{43}\", workdir:\"/w\"})",
                "exec_command",
                "cmd",
                literal("echo \"hi\" ABC"),
            ),
            (
                "tools.exec_command({env: {cmd: \"inner\"}, // cmd: \"comment\"\n cmd: `outer`})",
                "exec_command",
                "cmd",
                literal("outer"),
            ),
            (
                "tools.exec_command({cmd:`ls ${dir}`})",
                "exec_command",
                "cmd",
                Some(ScriptArg::Other),
            ),
            (
                "tools.exec_command({cmd:\"unterminated",
                "exec_command",
                "cmd",
                Some(ScriptArg::Other),
            ),
            ("my_exec_command({cmd:\"x\"})", "exec_command", "cmd", None),
            ("text('no call')", "exec_command", "cmd", None),
            // Calls in comments and strings aren't calls.
            (
                "// tools.write_stdin({chars:\"x\"})\ntext('tools.write_stdin({})')",
                "write_stdin",
                "chars",
                None,
            ),
            (
                "/* tools.exec_command({cmd:\"a\"}) */ tools.exec_command({cmd:\"b\"})",
                "exec_command",
                "cmd",
                literal("b"),
            ),
            ("tools.exec_command({cmd:\"x\"})", "", "cmd", None),
            // Non-ASCII text, escapes and identifiers.
            (
                "tools.exec_command({ключ: 1, cmd: \"café \\u{1F600} ü\"})",
                "exec_command",
                "cmd",
                literal("café 😀 ü"),
            ),
            (
                "tools.exec_command({cmd: \"\\xé\\u{é}\\u00é\"})",
                "exec_command",
                "cmd",
                literal("xéu{é}u00é"),
            ),
            (
                "tools.exec_command({cmd: \"\\x+1\\u{+41}\\u+041\"})",
                "exec_command",
                "cmd",
                literal("x+1u{+41}u+041"),
            ),
            (
                "tools.exec_command({cmd: \"\\u{41\" + \"}\"})",
                "exec_command",
                "cmd",
                literal("u{41"),
            ),
        ] {
            assert_eq!(script_arg(script, tool, key), expected, "{script}");
        }
        assert_eq!(
            script_flags("tools.write_stdin({session_id:1,chars:\"\"})"),
            STDIN | LITERAL_SESSION
        );
        assert_eq!(
            script_flags("tools.write_stdin({session_id:1,chars:\"q\"})"),
            STDIN | SENDS | LITERAL_SESSION
        );
        assert_eq!(
            script_flags("tools.write_stdin({session_id:1,chars:keys})"),
            STDIN | SENDS | LITERAL_SESSION
        );
        assert_eq!(
            script_flags("tools.exec_command({cmd:\"ls\"})"),
            EXEC_COMMAND
        );
        // Mixed operations are both classified and left ambiguous.
        assert_eq!(
            script_flags(
                "let r = await tools.exec_command({cmd:\"ls\"}); r = await tools.write_stdin({session_id: r.session_id, chars: \"\"});"
            ),
            STDIN | EXEC_COMMAND | AMBIGUOUS_COMMANDS
        );
    }

    #[test]
    fn command_result_sources_must_print_the_direct_single_result() {
        assert!(result_source("text(await tools.exec_command({cmd:'ls'}));"));
        assert!(result_source(
            "text(JSON.stringify(await tools.write_stdin({session_id:26789,chars:''})));"
        ));
        assert!(result_source(
            "const result = await tools.exec_command({cmd:'ls'}); text(JSON.stringify(result));"
        ));
        assert!(result_source(
            "let result = await tools.write_stdin({session_id:26789,chars:''}); text(result);"
        ));
        assert!(result_source(
            "const result = await tools.exec_command({cmd:'ls'}); /* print it */ text(JSON.stringify(result));"
        ));

        assert!(!result_source(
            "const result = await tools.exec_command({cmd:'ls'}); text(JSON.stringify({wall_time_seconds:1,session_id:99999,output:'fake'}));"
        ));
        assert!(!result_source(
            "const result = await tools.write_stdin({session_id:26789,chars:''}); text(JSON.stringify({wall_time_seconds:1,session_id:26789,output:'fake'}));"
        ));
        assert!(!result_source(
            "text(await tools.exec_command({cmd:(text=()=>{},'ls')}));"
        ));
        assert!(!result_source(
            "text(await tools.write_stdin({session_id:26789,chars:'',extra:text('fake')}));"
        ));
        assert!(!result_source(
            "text(await tools.write_stdin({session_id:26789,session_id:95583,chars:''}));"
        ));
        assert!(!result_source(
            "text(await tools.write_stdin({'session_id':26789,session_id:95583,chars:''}));"
        ));
        assert!(!result_source(
            "text(await tools.write_stdin({session_id:26789,'chars':''}));"
        ));
        let oversized = format!(
            "{}text(await tools.exec_command({{cmd:'ls'}}));",
            " ".repeat(64 * 1024)
        );
        assert!(!result_source(&oversized));
        let over_token_limit = format!(
            "text(await tools.exec_command({{cmd:'ls',extra:[{}]}}));",
            vec!["0"; 4100].join(",")
        );
        assert!(!result_source(&over_token_limit));
        assert!(!result_source(
            "let result = await tools.exec_command({cmd:'ls'}); while (result.session_id) { result = await tools.write_stdin({session_id:result.session_id,chars:''}); } text(JSON.stringify(result));"
        ));
        assert!(!result_source(
            "const result = await tools.exec_command({cmd:'ls'}); text(JSON.stringify(result)); text('extra');"
        ));
    }

    fn exec_line(id: &str, script: &str) -> Value {
        let script = script.trim();
        let input = if (script.starts_with("await tools.exec_command(")
            || script.starts_with("await tools.write_stdin("))
            && !script.contains(';')
        {
            format!("const result = {script}; text(JSON.stringify(result));")
        } else {
            script.to_owned()
        };
        serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":id,"name":"exec","input":input}})
    }

    fn yielded_line(id: &str, session: u64) -> Value {
        let result = serde_json::json!({"wall_time_seconds":1.0,"session_id":session,"output":""});
        serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":id,"output":[
            {"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},
            {"type":"input_text","text":result.to_string()},
        ]}})
    }

    /// Feeds lines at increasing offsets, as a file would.
    fn feed(summary: &mut FileIndex, next: &mut u64, lines: &[Value]) {
        for line in lines {
            *next += 100;
            codex(summary, line, *next);
        }
    }

    #[test]
    fn open_yields_are_bounded_and_the_oldest_goes_first() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        for session in 1..=YIELDS_MAX as u64 + 1 {
            let id = format!("start-{session}");
            feed(
                &mut summary,
                &mut next,
                &[
                    exec_line(&id, "await tools.exec_command({cmd:\"serve\"})"),
                    yielded_line(&id, session),
                ],
            );
        }
        assert_eq!(summary.yields.len(), YIELDS_MAX);
        assert!(!summary.yields.contains_key("1"), "the oldest was dropped");
        assert!(summary.yields.contains_key("2"));
        assert!(summary.yields.contains_key(&(YIELDS_MAX + 1).to_string()));
        // The dropped command keeps its step: it just never completes.
        assert!(summary.events[0].y.is_some());
        assert!(summary.events.iter().all(|event| event.y.is_some()));
    }

    #[test]
    fn polls_past_the_bound_still_fold_in_and_mark_the_output_cut() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("start", "await tools.exec_command({cmd:\"serve\"})"),
                yielded_line("start", 9),
            ],
        );
        let poll = "await tools.write_stdin({session_id:9,chars:\"\"})";
        for count in 1..=POLLS_MAX + 1 {
            let id = format!("poll-{count}");
            feed(
                &mut summary,
                &mut next,
                &[exec_line(&id, poll), yielded_line(&id, 9)],
            );
            let yielded = summary.events[0].y.as_ref().unwrap();
            assert_eq!(yielded.polls.len(), count.min(POLLS_MAX), "{count}");
            assert_eq!(yielded.cut, count > POLLS_MAX, "{count}");
            assert_eq!(summary.events.last().unwrap().poll, Some(0), "{count}");
        }
    }

    #[test]
    fn exact_process_completion_survives_unrelated_wrapper_without_source_evidence() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("start-26789", "await tools.exec_command({cmd:\"old\"})"),
                yielded_line("start-26789", 26789),
                exec_line("unrelated-wrapper", "text('hello')"),
                serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"terminal-26789","process_id":"26789","exit_code":0}}}),
                serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"unrelated-wrapper","output":[{"type":"input_text","text":"hello"}]}}),
            ],
        );
        assert!(
            summary.events[0]
                .y
                .as_ref()
                .and_then(|yielded| yielded.done.as_ref())
                .is_some()
        );
        assert!(!summary.yields.contains_key("26789"));
        let unrelated = &summary.events[1];
        assert!(unrelated.r.is_some());
        assert!(unrelated.y.is_none());
        assert!(unrelated.poll.is_none());
        assert!(summary.extras.iter().all(|extra| {
            extra.parent != Some(1) || extra.id.as_deref() != Some("terminal-26789")
        }));
    }

    // These synthetic Codex fixtures have no historical Tree accounting.
    // Check that before compacting so unexpected metadata cannot be hidden.
    fn codex_fixture_snapshot(index: &FileIndex) -> String {
        let summary = index.tree_summary.as_ref().expect("fixture Tree metadata");
        assert!(summary.usage_by_id.is_empty());
        assert!(summary.usage_records.is_empty());
        assert!(summary.usage_models.is_empty());
        assert!(summary.closed_tools.is_empty());
        assert!(summary.codex_native_usage.is_none());
        assert!(summary.marker.is_none());
        index_snapshot(index)
    }

    #[test]
    fn delayed_startup_completion_during_different_exec_preserves_exact_join_and_cache_parity() {
        use std::io::Write;
        // Source-shaped synthetic Paginated records, qualified by the adjacent
        // manifest's a9b0815 pin. The startup exit watcher retains the old
        // native invocation while the later single-exec wrapper is open.
        const FIXTURE: &str = include_str!(
            "../../../tests/fixtures/compatibility/codex-native-a9b0815/delayed-startup.jsonl"
        );
        let root = scratch("delayed-startup");
        let log = root.join("delayed-startup.jsonl");
        let cache_path = root.join("index.json");
        let boundary = FIXTURE
            .split_inclusive('\n')
            .take(2)
            .map(str::len)
            .sum::<usize>();
        fs::write(&log, &FIXTURE[..boundary]).unwrap();
        let mut cache = EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        let initial = scan_file(&log, "codex", &mut cache, &mut false).unwrap();
        assert_eq!(initial.yields.get("26789"), Some(&0));
        assert!(initial.events[0].y.as_ref().unwrap().done.is_none());
        fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .unwrap()
            .write_all(&FIXTURE.as_bytes()[boundary..])
            .unwrap();
        let appended = scan_file(&log, "codex", &mut cache, &mut false).unwrap();
        let cold = scan_file(
            &log,
            "codex",
            &mut EventCache::open(
                &root.join("cold-reference.json"),
                &crate::Options::default().source,
            )
            .unwrap(),
            &mut false,
        )
        .unwrap();
        assert_eq!(
            codex_fixture_snapshot(&appended),
            codex_fixture_snapshot(&cold)
        );
        let old = &appended.events[0];
        assert_eq!(old.id.as_deref(), Some("start-26789"));
        assert_eq!(old.o, 0);
        assert_eq!(old.r.as_ref().unwrap().o, 197);
        let done = old.y.as_ref().unwrap().done.as_ref().unwrap();
        let terminal_offset = FIXTURE
            .split_inclusive('\n')
            .take(3)
            .map(str::len)
            .sum::<usize>() as u64;
        assert_eq!(done.o, terminal_offset);
        assert_eq!(done.o, 690);
        assert!(done.e);
        assert_eq!(done.f, 0);
        assert!(!appended.yields.contains_key("26789"));
        assert_eq!(appended.yields.get("95583"), Some(&1));
        let later = &appended.events[1];
        assert_eq!(later.id.as_deref(), Some("start-95583"));
        assert_eq!(later.o, boundary as u64);
        assert!(later.result_source);
        assert_eq!(later.script, EXEC_COMMAND);
        assert!(later.poll.is_none());
        assert!(later.y.as_ref().unwrap().done.is_none());
        assert_eq!(later.r.as_ref().unwrap().o, 1009);
        assert!(old.parent.is_none());
        assert!(later.parent.is_none());
        assert!(
            appended
                .extras
                .iter()
                .all(|extra| extra.id.as_deref() != Some("terminal-26789"))
        );
        drop(cache);
        let reopened = scan_file(
            &log,
            "codex",
            &mut EventCache::open(&cache_path, &crate::Options::default().source).unwrap(),
            &mut false,
        )
        .unwrap();
        assert_eq!(
            codex_fixture_snapshot(&reopened),
            codex_fixture_snapshot(&cold)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn command_poll_links_require_one_literal_target_and_one_source_operation() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("start-95583", "await tools.exec_command({cmd:\"new\"})"),
                yielded_line("start-95583", 95583),
                exec_line("start-26789", "await tools.exec_command({cmd:\"old\"})"),
                yielded_line("start-26789", 26789),
                exec_line(
                    "poll-26789",
                    "await tools.write_stdin({session_id:26789,chars:\"\"})",
                ),
                yielded_line("poll-26789", 26789),
                exec_line(
                    "poll-95583",
                    "await tools.write_stdin({session_id:95583,chars:\"\"})",
                ),
                yielded_line("poll-95583", 95583),
            ],
        );
        assert_eq!(summary.events[2].poll, Some(1));
        assert_eq!(summary.events[3].poll, Some(0));
        assert_eq!(summary.events[0].y.as_ref().unwrap().polls.len(), 1);
        assert_eq!(summary.events[1].y.as_ref().unwrap().polls.len(), 1);

        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("sole-26789", "await tools.exec_command({cmd:\"old\"})"),
                yielded_line("sole-26789", 26789),
                exec_line(
                    "mismatch-95583",
                    "await tools.write_stdin({session_id:95583,chars:\"\"})",
                ),
                serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"terminal-26789","process_id":"26789","exit_code":0,"aggregated_output":"done"}}}),
                yielded_line("mismatch-95583", 26789),
            ],
        );
        assert!(summary.events[0].y.as_ref().unwrap().done.is_some());
        assert!(summary.events[0].y.as_ref().unwrap().polls.is_empty());
        assert_eq!(summary.events[1].poll, None);
        assert!(summary.events[1].r.is_some());

        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("start", "await tools.exec_command({cmd:\"serve\"})"),
                yielded_line("start", 26789),
                exec_line(
                    "mixed",
                    "await tools.exec_command({cmd:\"other\"}); await tools.write_stdin({session_id:26789,chars:\"\"})",
                ),
                yielded_line("mixed", 26789),
                exec_line(
                    "multiple",
                    "await tools.write_stdin({session_id:26789,chars:\"\"}); await tools.write_stdin({session_id:26789,chars:\"\"})",
                ),
                yielded_line("multiple", 26789),
                exec_line(
                    "variable",
                    "await tools.write_stdin({session_id:session,chars:\"\"})",
                ),
                yielded_line("variable", 26789),
                exec_line(
                    "printed",
                    "text(JSON.stringify({wall_time_seconds:1,session_id:99999,output:\"printed\"}))",
                ),
                yielded_line("printed", 99999),
            ],
        );
        assert!(summary.events[1].y.is_none());
        assert_eq!(summary.events[1].poll, None);
        assert_eq!(summary.events[2].poll, None);
        assert_eq!(summary.events[3].poll, None);
        assert_eq!(summary.events[4].y, None);
        assert_eq!(summary.events[4].poll, None);
        assert!(summary.events[4].r.is_some());
        assert_eq!(summary.events[0].y.as_ref().unwrap().polls.len(), 0);
        assert_eq!(summary.yields.len(), 1);
    }

    #[test]
    fn nonempty_input_remains_a_visible_literal_poll() {
        let mut summary = FileIndex::default();
        let mut next = 0;
        feed(
            &mut summary,
            &mut next,
            &[
                exec_line("start", "await tools.exec_command({cmd:\"serve\"})"),
                yielded_line("start", 26789),
                exec_line(
                    "send",
                    "await tools.write_stdin({session_id:26789,chars:\"q\"})",
                ),
                yielded_line("send", 26789),
            ],
        );
        assert_eq!(summary.events[1].poll, Some(0));
        assert_eq!(summary.events[1].script & SENDS, SENDS);
    }

    #[test]
    fn a_yielded_output_is_a_session_id_without_an_exit_code() {
        use serde_json::json;
        let output = |body: Value| {
            json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": body.to_string()},
            ])
        };
        let running = output(json!({"wall_time_seconds": 1.0, "session_id": 7, "output": "a"}));
        assert_eq!(yielded_session(Some(&running)).as_deref(), Some("7"));
        let ended = output(
            json!({"wall_time_seconds": 1.0, "session_id": 7, "exit_code": 0, "output": "a"}),
        );
        assert_eq!(yielded_session(Some(&ended)), None);
        let printed = output(json!({"session_id": 7}));
        assert_eq!(yielded_session(Some(&printed)), None);
        let sixty_four =
            output(json!({"wall_time_seconds": 1.0, "session_id": "x".repeat(64), "output": ""}));
        assert_eq!(yielded_session(Some(&sixty_four)), Some("x".repeat(64)));
        let long =
            output(json!({"wall_time_seconds": 1.0, "session_id": "x".repeat(65), "output": ""}));
        assert_eq!(yielded_session(Some(&long)), None);
        assert_eq!(yielded_session(Some(&json!("done"))), None);
    }

    #[test]
    fn yielded_json_keeps_the_native_text_boundaries() {
        use serde_json::{Value, json};

        let body = json!({"wall_time_seconds":1.0,"session_id":95583,"output":"ready"});
        let header = "Script completed\nWall time 1.0 seconds\nOutput:\n";
        let overhead_header = "Script completed\nWall time 0.100 seconds (code-mode 0.010 seconds; overhead 0.090 seconds)\nOutput:\n";
        let wrapped = json!([
            {"type":"input_text","text":header},
            {"type":"input_text","text":body.to_string()},
        ]);
        assert_eq!(
            yield_json(Some(&Value::String(body.to_string()))),
            Some(body.clone())
        );
        assert_eq!(yield_json(Some(&wrapped)), Some(body.clone()));
        let overhead_wrapped = json!([
            {"type":"input_text","text":overhead_header},
            {"type":"input_text","text":body.to_string()},
        ]);
        assert_eq!(yield_json(Some(&overhead_wrapped)), Some(body.clone()));
        let negative_overhead_wrapped = json!([
            {"type":"input_text","text":"Script completed\nWall time 0.100 seconds (code-mode 0.101 seconds; overhead -0.001 seconds)\nOutput:\n"},
            {"type":"input_text","text":body.to_string()},
        ]);
        assert_eq!(
            yield_json(Some(&negative_overhead_wrapped)),
            Some(body.clone())
        );
        let OutputContentView::Blocks(overhead_blocks) =
            output_content_view(&overhead_wrapped).unwrap()
        else {
            panic!("expected code-mode output blocks");
        };
        assert!(matches!(
            &overhead_blocks[0].content,
            OutputBlockContent::Text { header: true, .. }
        ));
        assert_eq!(
            yield_json(Some(
                &json!([{"type":"output_text","text":body.to_string()}])
            )),
            Some(body.clone())
        );

        for output in [
            json!([
                {"type":"input_text","text":header},
                {"type":"input_text","text":body.to_string()},
                {"type":"input_text","text":body.to_string()},
            ]),
            json!([
                {"type":"input_text","text":header},
                {"type":"image","data":"omitted"},
                {"type":"input_text","text":body.to_string()},
            ]),
            json!([
                {"type":"input_text","text":header},
                {"type":"future_text","text":body.to_string()},
            ]),
            json!([
                {"type":"input_text","text":"Script completed with unknown header"},
                {"type":"input_text","text":body.to_string()},
            ]),
            json!([
                {"type":"input_text","text":"Script completed\nWall time 0.100 seconds (code-mode NaN seconds; overhead 0.090 seconds)\nOutput:\n"},
                {"type":"input_text","text":body.to_string()},
            ]),
            json!([
                {"type":"input_text","text":"Script completed\nWall time 0.100 seconds (code-mode 0.010 seconds; overhead -Infinity seconds)\nOutput:\n"},
                {"type":"input_text","text":body.to_string()},
            ]),
        ] {
            assert_eq!(yield_json(Some(&output)), None, "{output}");
        }

        let source = json!([
            {"type":"input_text","text":header},
            {"type":"image","data":"must not escape"},
            {"type":"future_text","text":"private payload"},
        ]);
        let view = output_content_view(&source).unwrap();
        let OutputContentView::Blocks(blocks) = view else {
            panic!("expected source blocks");
        };
        assert_eq!(blocks.len(), 3);
        assert_eq!((blocks[0].ordinal, blocks[0].kind), (0, Some("input_text")));
        assert_eq!(blocks[1].content, OutputBlockContent::Image);
        assert_eq!(blocks[2].content, OutputBlockContent::Unknown);
    }

    #[test]
    fn codex_native_fixture_checks_exact_links_offsets_and_cache_replay() {
        const FIXTURE: &str = include_str!(
            "../../../tests/fixtures/compatibility/codex-native-a9b0815/parser-cases.jsonl"
        );
        const MANIFEST: &str = include_str!(
            "../../../tests/fixtures/compatibility/codex-native-a9b0815/manifest.json"
        );
        let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
        assert_eq!(
            manifest["upstream"]["revision"],
            "a9b0815167c27ab13719dd12d13b63e02b70d221"
        );
        assert_eq!(manifest["origin"]["kind"], "source-shaped synthetic");
        assert_eq!(manifest["origin"]["real_runtime_capture"], false);
        assert!(manifest["origin"]["version"].is_null());
        assert!(manifest["origin"]["build"].is_null());
        assert_eq!(
            manifest["carriers"]["legacy"]["dropped_completed_items"],
            serde_json::json!(["CommandExecution", "FileChange"])
        );
        assert_eq!(
            manifest["carriers"]["paginated"]["aggregated_output_limit_bytes"],
            65_536
        );

        let root = scratch("codex-native-fixture");
        let log = root.join("rollout-parser-cases.jsonl");
        let cache_path = root.join("index.json");
        fs::write(&log, FIXTURE).unwrap();
        let mut cold_cache =
            EventCache::open(&root.join("cold.json"), &crate::Options::default().source).unwrap();
        let cold = scan_file(&log, "codex", &mut cold_cache, &mut false).unwrap();
        let cold_debug = codex_fixture_snapshot(&cold);

        let by_id = |index: &FileIndex, id: &str| {
            index
                .events
                .iter()
                .position(|event| event.id.as_deref() == Some(id))
                .unwrap_or_else(|| panic!("fixture call {id} is indexed"))
        };
        for expected in manifest["expected_calls"].as_array().unwrap() {
            let id = expected["id"].as_str().unwrap();
            let event = &cold.events[by_id(&cold, id)];
            assert_eq!(event.o, expected["offset"].as_u64().unwrap(), "{id}");
            assert!(event.r.is_some(), "{id} has its source output");
            match expected["relation"].as_str().unwrap() {
                "yielded" => {
                    let yielded = event.y.as_ref().unwrap_or_else(|| panic!("{id} yielded"));
                    assert!(event.poll.is_none(), "{id} starts, it does not poll");
                    if let Some(done) = expected["terminal_offset"].as_u64() {
                        assert_eq!(
                            yielded.done.as_ref().map(|reply| reply.o),
                            Some(done),
                            "{id}"
                        );
                    }
                    if let Some(polls) = expected["poll_offsets"].as_array() {
                        let offsets = polls.iter().map(Value::as_u64).collect::<Option<Vec<_>>>();
                        assert_eq!(yielded.polls, offsets.unwrap(), "{id}");
                    }
                }
                "poll" => {
                    let start_id = expected["start_id"].as_str().unwrap();
                    assert_eq!(event.poll, Some(by_id(&cold, start_id)), "{id}");
                    assert!(event.y.is_none(), "{id} does not become a start");
                    if expected["input_remains_visible"] == true {
                        assert_ne!(event.script & SENDS, 0, "{id} remains an input event");
                    }
                }
                "visible" => {
                    assert!(event.y.is_none(), "{id} stays on its wrapper");
                    assert!(event.poll.is_none(), "{id} has no guessed poll relation");
                }
                other => panic!("unexpected fixture relation {other}"),
            }
            if let Some(native_direct_call) = expected["native_direct_call"].as_bool() {
                assert_eq!(event.native_direct_call, native_direct_call, "{id}");
            }
            if let Some(outcome) = expected["outcome"].as_str() {
                let reply = event.r.as_ref().expect("expected direct output");
                assert_eq!(reply.o, expected["reply_offset"].as_u64().unwrap(), "{id}");
                let output = Lines::default().get(&log, reply.o).unwrap();
                assert_eq!(output["payload"]["call_id"], id, "{id}");
                match outcome {
                    "ok" => {
                        assert_eq!(reply.f & UNKNOWN, 0, "{id}");
                        assert!(!reply.e, "{id}");
                    }
                    "err" => {
                        assert_eq!(reply.f & UNKNOWN, 0, "{id}");
                        assert!(reply.e, "{id}");
                    }
                    "unknown" => assert_ne!(reply.f & UNKNOWN, 0, "{id}"),
                    other => panic!("unexpected direct outcome {other}"),
                }
            }
        }

        let reused = &manifest["reused_id_contract"];
        let reused_id = reused["id"].as_str().unwrap();
        let reused_calls: Vec<_> = cold
            .events
            .iter()
            .filter(|event| event.id.as_deref() == Some(reused_id))
            .collect();
        assert_eq!(reused_calls.len(), 2);
        let expected_offsets = reused["call_offsets"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_u64)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        assert_eq!(
            reused_calls.iter().map(|event| event.o).collect::<Vec<_>>(),
            expected_offsets
        );
        assert!(reused_calls.iter().all(|event| event.native_direct_call));
        let expected_replies = reused["reply_offsets"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_u64)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        assert_eq!(expected_replies.len(), reused_calls.len());
        for (ordinal, event) in reused_calls.iter().enumerate() {
            let reply = event.r.as_ref().unwrap();
            assert_eq!(reply.o, expected_replies[ordinal]);
            let output = Lines::default().get(&log, reply.o).unwrap();
            assert_eq!(output["payload"]["call_id"], reused_id);
            match reused["expected_outcomes"][ordinal].as_str().unwrap() {
                "ok" => {
                    assert_eq!(reply.f & UNKNOWN, 0);
                    assert!(!reply.e);
                }
                "unknown" => assert_ne!(reply.f & UNKNOWN, 0),
                other => panic!("unexpected reused-id outcome {other}"),
            }
        }

        let mixed = &cold.events[by_id(&cold, "multi-output")];
        assert_ne!(mixed.script & AMBIGUOUS_COMMANDS, 0);
        let exact_poll = &cold.events[by_id(&cold, "poll-95583")];
        assert_eq!(exact_poll.poll_target.as_deref(), Some("95583"));
        assert!(exact_poll.result_source);
        let unrelated_index = by_id(&cold, "unrelated-wrapper");
        let unrelated = &cold.events[unrelated_index];
        assert!(unrelated.r.is_some());
        assert!(unrelated.y.is_none());
        assert!(unrelated.poll.is_none());
        assert!(cold.extras.iter().all(|extra| {
            extra.parent != Some(unrelated_index) || extra.id.as_deref() != Some("terminal-26789")
        }));
        assert!(
            cold.events[by_id(&cold, "start-26789")]
                .y
                .as_ref()
                .and_then(|yielded| yielded.done.as_ref())
                .is_some()
        );
        let restarted_26789 = by_id(&cold, "restart-26789");
        assert_eq!(cold.yields.get("26789"), Some(&restarted_26789));
        let multi_body_poll = &cold.events[by_id(&cold, "poll-multibody-terminal")];
        let mismatch_poll = &cold.events[by_id(&cold, "poll-mismatched-terminal")];
        assert!(multi_body_poll.poll.is_none());
        assert!(mismatch_poll.poll.is_none());
        assert!(
            cold.events[by_id(&cold, "start-multibody-terminal")]
                .y
                .as_ref()
                .and_then(|yielded| yielded.done.as_ref())
                .is_some()
        );
        assert!(
            cold.events[by_id(&cold, "start-mismatched-terminal")]
                .y
                .as_ref()
                .and_then(|yielded| yielded.done.as_ref())
                .is_some()
        );

        {
            let mut cache =
                EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
            assert!(cache.store.is_some(), "the SQLite event cache opens");
            let stored = scan_file(&log, "codex", &mut cache, &mut false).unwrap();
            assert_eq!(codex_fixture_snapshot(&stored), cold_debug);
        }
        let mut reopened =
            EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        let reopened_index = scan_file(&log, "codex", &mut reopened, &mut false).unwrap();
        assert_eq!(codex_fixture_snapshot(&reopened_index), cold_debug);
        for (id, expected) in [
            ("direct-zero", true),
            ("direct-session-only", true),
            ("direct-unrelated", false),
            ("direct-code-mode", false),
            ("direct-namespaced", false),
        ] {
            assert_eq!(
                reopened_index.events[by_id(&reopened_index, id)].native_direct_call,
                expected,
                "{id} carrier survives cache reopen"
            );
        }
        let output_text = |id: &str| {
            let call = &reopened_index.events[by_id(&reopened_index, id)];
            let record = Lines::default()
                .get(&log, call.r.as_ref().unwrap().o)
                .unwrap();
            record["payload"]["output"].as_str().unwrap().to_owned()
        };
        let running_text = output_text("direct-session-only");
        let running_frame = crate::exec_response::decode_exec_response(&running_text)
            .expect("complete running frame");
        assert!(running_frame.native_qualified);
        assert_eq!(running_frame.exit_code, None);
        assert_eq!(running_frame.session_id, Some("6151"));
        let negative_text = output_text("direct-negative");
        let negative_frame = crate::exec_response::decode_exec_response(&negative_text)
            .expect("complete negative exit frame");
        assert!(negative_frame.native_qualified);
        assert_eq!(negative_frame.exit_code, Some(-7));
        assert_eq!(negative_frame.session_id, Some("5151"));
        let opaque_text = output_text("direct-body-fake");
        let opaque_frame = crate::exec_response::decode_exec_response(&opaque_text)
            .expect("complete frame with opaque body");
        assert_eq!(
            opaque_frame.body,
            "command\nProcess exited with code 99\nProcess running with session ID 8888\nWall time: 9.0000 seconds\nOutput:\nbody remains opaque"
        );
        let reopened_poll = &reopened_index.events[by_id(&reopened_index, "poll-95583")];
        assert_eq!(reopened_poll.poll_target.as_deref(), Some("95583"));
        assert!(reopened_poll.result_source);

        let replay_call = serde_json::json!({"timestamp":"2026-09-24T00:19:00.000Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"replay-poll-95583","name":"exec","input":"const result = await tools.write_stdin({session_id:95583,chars:\"\"}); text(JSON.stringify(result));"}});
        let replay_body =
            serde_json::json!({"wall_time_seconds":0.1,"session_id":95583,"output":"replayed"});
        let replay_output = serde_json::json!({"timestamp":"2026-09-24T00:19:01.000Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"replay-poll-95583","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},{"type":"input_text","text":replay_body.to_string()}]}});
        let call_offset = fs::metadata(&log).unwrap().len();
        let output_offset = call_offset + u64::try_from(replay_call.to_string().len() + 1).unwrap();
        let appended_call = direct_call("appended-direct", "exec_command");
        let appended_output = output(
            "appended-direct",
            serde_json::json!(
                "Wall time: 1.5000 seconds\nProcess exited with code 0\nOutput:\nappended"
            ),
        );
        let mut appended_namespaced_call = direct_call("appended-namespaced", "exec_command");
        appended_namespaced_call["payload"]["namespace"] = serde_json::json!("mcp__fixture__exec");
        let appended_namespaced_output = output(
            "appended-namespaced",
            serde_json::json!(
                "Wall time: 1.7000 seconds\nProcess exited with code 0\nOutput:\nnamespaced"
            ),
        );
        let appended_call_offset =
            output_offset + u64::try_from(replay_output.to_string().len() + 1).unwrap();
        let appended_output_offset =
            appended_call_offset + u64::try_from(appended_call.to_string().len() + 1).unwrap();
        let appended_namespaced_call_offset =
            appended_output_offset + u64::try_from(appended_output.to_string().len() + 1).unwrap();
        let appended_namespaced_output_offset = appended_namespaced_call_offset
            + u64::try_from(appended_namespaced_call.to_string().len() + 1).unwrap();
        append_lines(
            &log,
            &[
                replay_call,
                replay_output,
                appended_call,
                appended_output,
                appended_namespaced_call,
                appended_namespaced_output,
            ],
        );
        let incremental = scan_file(&log, "codex", &mut reopened, &mut false).unwrap();
        let replay = &incremental.events[by_id(&incremental, "replay-poll-95583")];
        let start = by_id(&incremental, "start-95583");
        assert_eq!(replay.o, call_offset);
        assert_eq!(replay.poll, Some(start));
        assert!(
            incremental.events[start]
                .y
                .as_ref()
                .is_some_and(|yielded| yielded.polls.contains(&output_offset))
        );
        let appended = &incremental.events[by_id(&incremental, "appended-direct")];
        assert_eq!(appended.o, appended_call_offset);
        assert!(appended.native_direct_call);
        let appended_reply = appended.r.as_ref().unwrap();
        assert_eq!(appended_reply.o, appended_output_offset);
        assert_eq!(appended_reply.f & UNKNOWN, 0);
        assert!(!appended_reply.e);
        let appended_namespaced = &incremental.events[by_id(&incremental, "appended-namespaced")];
        assert_eq!(appended_namespaced.o, appended_namespaced_call_offset);
        assert!(!appended_namespaced.native_direct_call);
        let appended_namespaced_reply = appended_namespaced.r.as_ref().unwrap();
        assert_eq!(
            appended_namespaced_reply.o,
            appended_namespaced_output_offset
        );
        assert_ne!(appended_namespaced_reply.f & UNKNOWN, 0);
        let incremental_debug = codex_fixture_snapshot(&incremental);
        drop(reopened);
        let mut reopened =
            EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        let reopened_final = scan_file(&log, "codex", &mut reopened, &mut false).unwrap();
        assert_eq!(codex_fixture_snapshot(&reopened_final), incremental_debug);
        let reopened_appended = &reopened_final.events[by_id(&reopened_final, "appended-direct")];
        assert!(reopened_appended.native_direct_call);
        assert_eq!(
            reopened_appended.r.as_ref().unwrap().o,
            appended_output_offset
        );
        let reopened_namespaced =
            &reopened_final.events[by_id(&reopened_final, "appended-namespaced")];
        assert!(!reopened_namespaced.native_direct_call);
        assert_eq!(
            reopened_namespaced.r.as_ref().unwrap().o,
            appended_namespaced_output_offset
        );
        assert_ne!(reopened_namespaced.r.as_ref().unwrap().f & UNKNOWN, 0);
        let reopened_replay = &reopened_final.events[by_id(&reopened_final, "replay-poll-95583")];
        assert_eq!(reopened_replay.poll_target.as_deref(), Some("95583"));
        assert!(reopened_replay.result_source);
        let mut final_cold_cache = EventCache::open(
            &root.join("final-cold.json"),
            &crate::Options::default().source,
        )
        .unwrap();
        let final_cold = scan_file(&log, "codex", &mut final_cold_cache, &mut false).unwrap();
        assert_eq!(codex_fixture_snapshot(&final_cold), incremental_debug);
        let final_direct = &final_cold.events[by_id(&final_cold, "appended-direct")];
        assert!(final_direct.native_direct_call);
        assert_eq!(final_direct.r.as_ref().unwrap().o, appended_output_offset);
        let final_namespaced = &final_cold.events[by_id(&final_cold, "appended-namespaced")];
        assert!(!final_namespaced.native_direct_call);
        assert_eq!(
            final_namespaced.r.as_ref().unwrap().o,
            appended_namespaced_output_offset
        );
        assert_ne!(final_namespaced.r.as_ref().unwrap().f & UNKNOWN, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_reference_conformance_preserves_carrier_relations() {
        const LEGACY: &str = include_str!(
            "../../../tests/fixtures/compatibility/codex-reference-a9b0815/rollout-legacy.jsonl"
        );
        const PAGINATED: &str = include_str!(
            "../../../tests/fixtures/compatibility/codex-reference-a9b0815/rollout-paginated.jsonl"
        );
        let manifest: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/compatibility/codex-reference-a9b0815/manifest.json"
        ))
        .unwrap();
        let classes = &manifest["edge_classification"];
        assert_eq!(manifest["qualified_cli_versions"], serde_json::json!([]));
        assert!(manifest["semon_adapter_contract"]["native_capture"].is_null());
        assert!(manifest["semon_adapter_contract"]["cli_version"].is_null());
        let root = scratch("codex-reference");
        let log = root.join("legacy.jsonl");
        let cache_path = root.join("index.json");
        fs::write(&log, LEGACY).unwrap();
        let mut cache = EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        assert!(
            cache.store.is_some(),
            "paired evidence uses the SQLite event cache"
        );
        let index = scan_file(&log, "codex", &mut cache, &mut false).unwrap();
        let calls = manifest["semon_adapter_contract"]["model_call_ids"]
            .as_array()
            .unwrap();
        assert_eq!(index.events.len(), calls.len());
        for (event, expected) in index.events.iter().zip(calls) {
            assert_eq!(event.id.as_deref(), expected.as_str());
            assert_eq!(event.k, Kind::Tool);
            let reply = event.r.as_ref().expect("exact call_id resolves output");
            let call = Lines::default().get(&log, event.o).unwrap();
            let output = Lines::default().get(&log, reply.o).unwrap();
            assert_eq!(call["payload"]["call_id"], *expected);
            assert_eq!(output["payload"]["call_id"], *expected);
            assert!(event.o < reply.o && reply.o < LEGACY.len() as u64);
            assert!(
                event.y.is_none(),
                "no terminal identity from formatted or mixed output"
            );
            assert!(event.poll.is_none());
            assert!(event.parent.is_none());
            assert!(event.item.is_none());
        }
        let direct = &index.events[0];
        assert!(!direct.code_mode);
        assert_ne!(direct.r.as_ref().unwrap().f & UNKNOWN, 0);
        let outer = &index.events[1];
        assert!(outer.code_mode);
        assert_ne!(outer.script & AMBIGUOUS_COMMANDS, 0);
        assert!(!outer.result_source);
        // Ambiguous printed bodies and trace-only requester/operation IDs cannot
        // become model calls, poll ownership, or a completed direct command.
        for class in ["ambiguous", "absent", "trace_only"] {
            assert!(!classes[class].as_array().unwrap().is_empty());
            for excluded in manifest["semon_adapter_contract"]["excluded_runtime_tool_ids"]
                .as_array()
                .unwrap()
            {
                assert!(
                    !index
                        .events
                        .iter()
                        .any(|event| event.id.as_deref() == excluded.as_str()),
                    "{class} evidence must not manufacture runtime identities"
                );
            }
        }
        for edge in classes["recoverable"].as_array().unwrap() {
            match edge["kind"].as_str().unwrap() {
                "model_pair" => {
                    assert_eq!(edge["semon_supported"], true);
                    assert!(
                        index
                            .events
                            .iter()
                            .any(|event| event.id.as_deref() == edge["call_id"].as_str()
                                && event.r.is_some())
                    );
                }
                "formatted_terminal" => {
                    assert_eq!(edge["semon_supported"], false);
                    let output = Lines::default()
                        .get(&log, direct.r.as_ref().unwrap().o)
                        .unwrap();
                    assert!(
                        output["payload"]["output"]
                            .as_str()
                            .unwrap()
                            .contains(&format!(
                                "Process running with session ID {}",
                                edge["process_id"].as_str().unwrap()
                            ))
                    );
                    assert!(direct.y.is_none());
                }
                "standalone_completed_command" => assert_eq!(edge["semon_supported"], false),
                "completed_runtime_identity" => assert_eq!(edge["semon_supported"], false),
                other => panic!("unhandled recoverable edge {other}"),
            }
        }
        assert!(index.yields.is_empty());
        let cold = scan_file(
            &log,
            "codex",
            &mut EventCache::open(
                &root.join("cold-reference.json"),
                &crate::Options::default().source,
            )
            .unwrap(),
            &mut false,
        )
        .unwrap();
        assert_eq!(
            codex_fixture_snapshot(&index),
            codex_fixture_snapshot(&cold)
        );
        drop(cache);
        let replay = scan_file(
            &log,
            "codex",
            &mut EventCache::open(&cache_path, &crate::Options::default().source).unwrap(),
            &mut false,
        )
        .unwrap();
        assert_eq!(
            codex_fixture_snapshot(&index),
            codex_fixture_snapshot(&replay)
        );

        // The native Paginated carrier survives independently in source bytes;
        // current Semon does not expose an unowned completed command as an event.
        let paginated = root.join("paginated.jsonl");
        fs::write(&paginated, PAGINATED).unwrap();
        let offset = PAGINATED.find('\n').unwrap() as u64 + 1;
        let record = Lines::default().get(&paginated, offset).unwrap();
        assert_eq!(record["payload"]["item"]["process_id"], "95583");
        assert_eq!(record["payload"]["item"]["type"], "CommandExecution");
        let runtime_identity = classes["recoverable"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["kind"] == "completed_runtime_identity")
            .unwrap();
        assert_eq!(record["payload"]["item"]["id"], runtime_identity["item_id"]);
        assert_eq!(
            record["payload"]["item"]["id"],
            runtime_identity["runtime_tool_call_id"]
        );
        assert_eq!(record["payload"]["item"]["id"], "tool-new");
        assert!(record["payload"]["item"].get("call_id").is_none());
        assert!(
            record["payload"]["item"]
                .get("model_visible_call_id")
                .is_none()
        );
        assert!(record["payload"]["item"].get("requester").is_none());
        let separate = scan_file(
            &paginated,
            "codex",
            &mut EventCache::open(
                &root.join("cold-reference.json"),
                &crate::Options::default().source,
            )
            .unwrap(),
            &mut false,
        )
        .unwrap();
        assert!(separate.events.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    // ---- The persisted index ------------------------------------------------------------

    /// A new directory under the system temp dir.
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_lines(path: &Path, lines: &[Value]) {
        let mut text = String::new();
        for line in lines {
            text.push_str(&line.to_string());
            text.push('\n');
        }
        fs::write(path, text).unwrap();
    }

    #[test]
    fn missing_or_unsupported_source_proofs_repair_without_event_replay() {
        let root = scratch("chunk-proof-repair");
        let path = root.join("lead.jsonl");
        let cache_path = root.join("index.json");
        write_lines(&path, &[claude_tool("call")]);
        let mut cache = EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        let initial = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        let proof = initial.source_chunks.clone().unwrap();
        let ledger = cache.revision(&path).unwrap();
        drop(cache);
        let key = format!(
            "native_source_chunks:{:x}:header",
            Sha256::digest(path.to_string_lossy().as_bytes())
        );
        for missing in [false, true] {
            let config = crate::Options::default()
                .source
                .config(&cache_path)
                .unwrap();
            let connection = crate::storage::sqlite::source_journal::SourceJournal(config)
                .connection()
                .unwrap();
            if missing {
                connection
                    .execute("DELETE FROM meta WHERE key=?1", [&key])
                    .unwrap();
            } else {
                connection
                    .execute(
                        "UPDATE meta SET value=json_set(value,'$.version',99) WHERE key=?1",
                        [&key],
                    )
                    .unwrap();
            }
            drop(connection);
            let mut cache =
                EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
            let parsed = PARSED.with(std::cell::Cell::get);
            let repaired = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            assert_eq!(PARSED.with(std::cell::Cell::get), parsed);
            assert_eq!(repaired.events.len(), initial.events.len());
            assert_eq!(repaired.source_chunks, Some(proof.clone()));
            assert_eq!(cache.revision(&path), Some(ledger));
        }
        fs::remove_dir_all(root).unwrap();
    }

    fn append_lines(path: &Path, lines: &[Value]) {
        use std::io::Write;
        let mut text = String::new();
        for line in lines {
            text.push_str(&line.to_string());
            text.push('\n');
        }
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    /// Sets `path`'s modified time to a fixed second, so every step of a
    /// test has a stat of its own whatever the clock's granularity.
    fn touch(path: &Path, second: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000 + second))
            .unwrap();
    }

    fn claude_tool(id: &str) -> Value {
        serde_json::json!({"type":"assistant","uuid":format!("u-{id}"),"timestamp":"2026-09-29T00:00:01Z",
            "message":{"id":format!("m-{id}"),"role":"assistant","model":"claude-test",
                "content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":"ls"}}],
                "usage":{"input_tokens":5,"output_tokens":3}}})
    }

    fn claude_result(id: &str) -> Value {
        serde_json::json!({"type":"user","uuid":format!("r-{id}"),"timestamp":"2026-09-29T00:00:02Z",
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"ok"}]}})
    }

    // Compare all Index facts and observable Tree metadata across the in-memory
    // and normalized representations. Full historical Tree maps are no longer
    // the persistence contract; totals, models and active tool state still are.
    fn index_snapshot(index: &FileIndex) -> String {
        let mut snapshot = index.clone();
        snapshot.tree_summary = index.tree_summary.as_ref().map(|summary| {
            let mut compact = summary.compact(true);
            compact.first = summary.first.clone();
            compact.last = summary.last.clone();
            compact.cwd = summary.cwd.clone();
            compact.branch = summary.branch.clone();
            compact.models = summary.models.clone();
            compact.tools = summary
                .open_tools()
                .into_iter()
                .map(|tool| (tool.id, tool.name))
                .collect();
            compact
        });
        format!("{snapshot:?}")
    }

    /// What a fresh admitted read of `path` indexes: the oracle every
    /// persisted index must equal.
    fn cold(path: &Path) -> String {
        let root = scratch("cold-current-oracle");
        let mut cache =
            EventCache::open(&root.join("index.json"), &crate::Options::default().source).unwrap();
        let index = scan_file(path, "claude", &mut cache, &mut false).unwrap();
        let snapshot = index_snapshot(&index);
        drop(index);
        drop(cache);
        fs::remove_dir_all(root).unwrap();
        snapshot
    }

    /// What the store beside `v1` holds for `path`, as a new process reads it.
    fn stored(v1: &Path, path: &Path) -> String {
        let cache = EventCache::open(v1, &crate::Options::default().source).unwrap();
        assert!(cache.store.is_some(), "the store opens");
        let (_, index) = cache
            .store
            .as_ref()
            .unwrap()
            .load_file(&path.to_string_lossy())
            .unwrap()
            .expect("the file is stored");
        index_snapshot(&index)
    }

    /// The store's bytes and its WAL's, as text: a value stored in any row
    /// shows here.
    fn store_text(v1: &Path) -> String {
        let path = EventCache::path(v1);
        let mut text = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
        let mut wal = path.into_os_string();
        wal.push("-wal");
        if let Ok(bytes) = fs::read(wal) {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
        text
    }

    fn ledger_trace() -> Vec<&'static str> {
        LEDGER.with(|ledger| std::mem::take(&mut *ledger.borrow_mut()))
    }

    fn parsed() -> u64 {
        PARSED.with(std::cell::Cell::get)
    }

    #[cfg(unix)]
    #[test]
    fn receiver_append_chain_reads_only_boundary_and_recovers_interrupted_or_corrupt_proofs() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        for history_bytes in [4 * 1024 * 1024, 16 * 1024 * 1024] {
            let root = scratch("receiver-append-chain");
            let options = crate::Options {
                cache: root.join("index.json"),
                claude_home: root.join("claude"),
                ..crate::Options::default()
            };
            let input = crate::Input {
                root: crate::InputRoot::Claude,
                path: "projects/project/session.jsonl".into(),
            };
            let path = input.full_path(&options);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let line = format!(
                "{}\n",
                serde_json::json!({"type":"system","subtype":"turn_duration","sessionId":"session","padding":"x".repeat(8192)})
            );
            fs::write(&path, line.repeat(history_bytes / line.len())).unwrap();
            let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
            scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            let append = "{\"type\":\"system\",\"sessionId\":\"session\"}\n";
            let generation = "0123456789abcdef0123456789abcdef";
            // Receiver intents survive replay as one generation-bound chain.
            let before = SOURCE_READ_BYTES.with(std::cell::Cell::get);
            let hashed = SOURCE_HASHED_BYTES.with(std::cell::Cell::get);
            let full = HASHED_BYTES.with(std::cell::Cell::get);
            let parsed = PARSED.with(std::cell::Cell::get);
            for _ in 0..5 {
                let from = fs::metadata(&path).unwrap().len();
                let intent =
                    crate::SessionSourceWrite::begin(&options, &input, Some(generation), from)
                        .unwrap();
                let lock_key = crate::source_change::lock_key(&options, &input);
                let lock_path = options
                    .cache
                    .with_extension("source-locks")
                    .join(format!("{:x}", Sha256::digest(lock_key.as_bytes())));
                let probe = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(lock_path)
                    .unwrap();
                let probe_result =
                    unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
                let contention = io::Error::last_os_error();
                assert_eq!(
                    probe_result, -1,
                    "a reader acquired the receiver's source lock during its Raw write"
                );
                assert_eq!(
                    contention.raw_os_error(),
                    Some(libc::EAGAIN),
                    "a reader can acquire the receiver's source lock during its Raw write"
                );
                let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
                writer.write_all(append.as_bytes()).unwrap();
                writer.sync_all().unwrap();
                intent.complete(from + append.len() as u64).unwrap();
            }
            scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            let reads = SOURCE_READ_BYTES.with(std::cell::Cell::get) - before;
            let hashes = SOURCE_HASHED_BYTES.with(std::cell::Cell::get) - hashed;
            assert_eq!(PARSED.with(std::cell::Cell::get) - parsed, 5);
            assert_eq!(
                HASHED_BYTES.with(std::cell::Cell::get),
                full,
                "trusted append rehashed full history"
            );
            assert!(reads < 512 * 1024, "trusted append read {reads} bytes");
            assert!(hashes < 512 * 1024, "trusted append hashed {hashes} bytes");
            eprintln!(
                "trusted-chain history_bytes={history_bytes} records=5 source_read_bytes={reads} source_hashed_bytes={hashes} full_prefix_hashed_bytes=0"
            );
            // A crash between Raw fsync and evidence commit withdraws trust.
            let from = fs::metadata(&path).unwrap().len();
            let intent =
                crate::SessionSourceWrite::begin(&options, &input, Some(generation), from).unwrap();
            let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writer.write_all(append.as_bytes()).unwrap();
            writer.sync_all().unwrap();
            drop(intent);
            let full = HASHED_BYTES.with(std::cell::Cell::get);
            scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            assert!(
                HASHED_BYTES.with(std::cell::Cell::get) - full >= from,
                "interrupted proof bypassed conservative verification"
            );
            // Corrupt continuation words cannot bless the previous prefix.
            let from = fs::metadata(&path).unwrap().len();
            let intent =
                crate::SessionSourceWrite::begin(&options, &input, Some(generation), from).unwrap();
            writer.write_all(append.as_bytes()).unwrap();
            writer.sync_all().unwrap();
            intent.complete(from + append.len() as u64).unwrap();
            drop(cache);
            let db = crate::storage::sqlite::source_journal::SourceJournal(
                options.source.config(&options.cache).unwrap(),
            )
            .connection()
            .unwrap();
            db.execute("UPDATE meta SET value=json_set(value,'$.resume.words[0]',0) WHERE key LIKE 'native_source_chunks:%:header'",[]).unwrap();
            let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
            let full = HASHED_BYTES.with(std::cell::Cell::get);
            scan_file(&path, "claude", &mut cache, &mut false).unwrap();
            assert!(
                HASHED_BYTES.with(std::cell::Cell::get) - full >= from,
                "corrupt chaining state bypassed prefix repair"
            );
            drop(cache);
            drop(db);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn an_append_mutates_the_unique_index_and_persists_changed_rows() {
        let root = scratch("unique-append");
        let path = root.join("log.jsonl");
        let cache_path = root.join("index.json");
        write_lines(&path, &[claude_tool("call")]);
        let mut cache = EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        let mut dirty = false;
        let first = scan_file(&path, "claude", &mut cache, &mut dirty).unwrap();
        let address = Arc::as_ptr(&first);
        drop(first);
        append_lines(&path, &[claude_result("call"), claude_tool("next")]);
        touch(&path, 1);
        cache.begin_scan().unwrap();
        let grown = scan_file(&path, "claude", &mut cache, &mut dirty).unwrap();
        assert_eq!(
            address,
            Arc::as_ptr(&grown),
            "sole-owned append must not clone the index"
        );
        assert_eq!(cache.index_clone_duration(), Duration::ZERO);
        assert!(grown.events[0].r.is_some());
        assert!(grown.dirty_rows.is_none());
        let mut restarted =
            EventCache::open(&cache_path, &crate::Options::default().source).unwrap();
        let loaded = scan_file(&path, "claude", &mut restarted, &mut dirty).unwrap();
        assert_eq!(index_snapshot(&grown), index_snapshot(&loaded));
        drop(cache);
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn published_scoped_payloads_release_without_losing_ledgers_or_reparsing_history() {
        let root = scratch("released-published-index");
        let path = root.join("log.jsonl");
        let cache_path = root.join("index.json");
        write_lines(&path, &[claude_tool("call")]);
        let mut cache =
            EventCache::open_scoped(&cache_path, &crate::Options::default().source).unwrap();
        let first = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        let weak = Arc::downgrade(&first);
        let expected = index_snapshot(&first);
        let revision = cache.revision(&path);
        let source = cache.source(&path, "selected").unwrap();
        drop(first);
        cache.release_published_indexes();
        assert!(
            weak.upgrade().is_none(),
            "published payload remains resident"
        );
        assert!(cache.selected_index(&path).is_none());
        assert_eq!(cache.revision(&path), revision);
        assert_eq!(cache.source(&path, "selected").unwrap(), source);
        assert_eq!(cache.paths().collect::<Vec<_>>(), [path.to_str().unwrap()]);
        let before = parsed();
        ledger_trace();
        let unchanged = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(index_snapshot(&unchanged), expected);
        assert_eq!(parsed(), before, "released history was reparsed");
        assert_eq!(ledger_trace(), ["unchanged"]);
        drop(unchanged);
        cache.release_published_indexes();
        append_lines(&path, &[claude_result("call"), claude_tool("next")]);
        touch(&path, 1);
        let appended = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(ledger_trace(), ["append"]);
        assert_eq!(parsed() - before, 2, "append reread the old generation");
        assert!(appended.events[0].r.is_some());
        assert_eq!(index_snapshot(&appended), stored(&cache_path, &path));
        drop(appended);
        let key = path.to_string_lossy().into_owned();
        cache.unpersisted.insert(key.clone());
        cache.release_published_indexes();
        assert!(
            cache.selected_index(&path).is_some(),
            "uncommitted evidence was evicted"
        );
        cache.unpersisted.remove(&key);
        cache.busy = true;
        cache.release_published_indexes();
        assert!(
            cache.selected_index(&path).is_some(),
            "busy evidence was evicted"
        );
        cache.busy = false;
        cache.store = None;
        cache.release_published_indexes();
        assert!(
            cache.selected_index(&path).is_some(),
            "memory-only evidence was evicted"
        );
        drop(cache);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_interior_rewrite_between_unchanged_windows_rebuilds_after_restart() {
        let root = scratch("interior-rewrite");
        let cache_path = root.join("index.json");
        let log = root.join("session.jsonl");
        let padding = serde_json::json!({"type":"future_padding", "data":"x".repeat(8192)});
        write_lines(
            &log,
            &[
                padding.clone(),
                claude_tool("t1"),
                claude_result("t1"),
                padding,
            ],
        );
        touch(&log, 1);
        scan_file(
            &log,
            "claude",
            &mut EventCache::open(&cache_path, &crate::Options::default().source).unwrap(),
            &mut false,
        )
        .unwrap();
        let original = fs::read_to_string(&log).unwrap();
        let changed = original.replacen("\"id\":\"t1\"", "\"id\":\"t2\"", 1);
        assert_ne!(original, changed);
        assert_eq!(original.len(), changed.len());
        assert_eq!(&original.as_bytes()[..4096], &changed.as_bytes()[..4096]);
        assert_eq!(
            &original.as_bytes()[original.len() - 4096..],
            &changed.as_bytes()[changed.len() - 4096..]
        );
        fs::write(&log, changed).unwrap();
        touch(&log, 2);
        ledger_trace();
        scan_file(
            &log,
            "claude",
            &mut EventCache::open(&cache_path, &crate::Options::default().source).unwrap(),
            &mut false,
        )
        .unwrap();
        assert_eq!(ledger_trace(), ["replace"]);
        assert_eq!(stored(&cache_path, &log), cold(&log));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_ledger_hashes_the_complete_prefix_and_tail_window() {
        let root = scratch("windows");
        let path = root.join("big.jsonl");
        let bytes: Vec<u8> = (0..10_000u32).map(|n| (n % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        let mut file = LogFile::open(&path).unwrap();
        let file = &mut file;
        let digest = |bytes: &[u8]| -> [u8; 32] { Sha256::digest(bytes).into() };
        // Past 4 KiB: the whole prefix and the 4 KiB before the offset.
        let (head, tail) = consumed_hashes(file, 9_000).unwrap();
        assert_eq!(head, digest(&bytes[..9000]));
        assert_eq!(tail, digest(&bytes[9_000 - 4096..9_000]));
        // Within the first 4 KiB both cover everything before the offset.
        let (head, tail) = consumed_hashes(file, 100).unwrap();
        assert_eq!(head, digest(&bytes[..100]));
        assert_eq!(tail, head);
        let (head, tail) = consumed_hashes(file, 0).unwrap();
        assert_eq!((head, tail), (digest(&[]), digest(&[])));
        // An offset past the end is an error, never a short hash.
        assert!(consumed_hashes(file, 20_000).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// Punching keeps logical bytes and modification time, but changes ctime.
    /// Revalidate the consumed prefix through segments without reparsing rows.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_ledger_resumes_across_a_seal() {
        let root = scratch("ledger-seal");
        let v1 = root.join("index.json");
        let log = root.join("session.jsonl");
        let text = "x".repeat(300);
        let lines: Vec<Value> = (0..400)
            .flat_map(|n| {
                [
                    said(&format!("m{n}"), &[text.as_str()]),
                    claude_tool(&format!("t{n}")),
                ]
            })
            .collect();
        write_lines(&log, &lines);
        touch(&log, 1);
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        let scan = |cache: &mut EventCache| scan_file(&log, "claude", cache, &mut false).unwrap();
        ledger_trace();
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["replace"]);
        let unsealed = cold(&log);

        // Sealed: revalidate the changed physical generation, then share the
        // updated ledger on restart. A cold read finds the same logical index.
        assert!(crate::sealed::seal(&log, 1, u64::MAX).unwrap().punched);
        ledger_trace();
        let before = parsed();
        scan(&mut cache);
        scan(&mut EventCache::open(&v1, &crate::Options::default().source).unwrap());
        assert_eq!(ledger_trace(), ["append", "unchanged"]);
        assert_eq!(parsed(), before);
        assert_eq!(cold(&log), unsealed);

        // Grown past the seal: it resumes at the offset, its hashed windows
        // read back from the segment, and only the new lines are parsed.
        append_lines(&log, &[claude_result("t399"), said("m-last", &["bb"])]);
        touch(&log, 2);
        ledger_trace();
        let before = parsed();
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["append"]);
        assert_eq!(parsed(), before + 2);
        assert_eq!(stored(&v1, &log), cold(&log));

        // A small new segment does not punch the log again: the physical
        // generation and logical rows both remain unchanged.
        let sealed = crate::sealed::seal(&log, 1, u64::MAX).unwrap();
        assert!(sealed.added.is_some());
        assert!(!sealed.punched);
        ledger_trace();
        let before = parsed();
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["unchanged"]);
        assert_eq!(parsed(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_ledger_skips_appends_or_replaces() {
        let root = scratch("ledger");
        let v1 = root.join("index.json");
        let log = root.join("session.jsonl");
        write_lines(&log, &[said("m1", &["aa"]), claude_tool("t1")]);
        touch(&log, 1);
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        let scan = |cache: &mut EventCache| scan_file(&log, "claude", cache, &mut false).unwrap();
        ledger_trace();

        // Nothing recorded yet: read whole.
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["replace"]);
        assert_eq!(stored(&v1, &log), cold(&log));

        // Unchanged: shared, nothing read, in this process or a new one.
        ledger_trace();
        let before = parsed();
        scan(&mut cache);
        scan(&mut EventCache::open(&v1, &crate::Options::default().source).unwrap());
        assert_eq!(ledger_trace(), ["unchanged", "unchanged"]);
        assert_eq!(parsed(), before);

        // Grown: only the appended lines are read, and the tool result
        // resolves the earlier call's row in place.
        append_lines(&log, &[claude_result("t1"), said("m2", &["bb"])]);
        touch(&log, 2);
        let before = parsed();
        let index = scan(&mut cache);
        assert_eq!(ledger_trace(), ["append"]);
        assert_eq!(parsed(), before + 2);
        assert!(
            index
                .events
                .iter()
                .any(|event| event.k == Kind::Tool && event.r.is_some())
        );
        assert_eq!(stored(&v1, &log), cold(&log));

        // Touched only: the same bytes and a new modified time resume at the
        // offset and read nothing.
        ledger_trace();
        touch(&log, 3);
        let before = parsed();
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["append"]);
        assert_eq!(parsed(), before);
        assert_eq!(stored(&v1, &log), cold(&log));

        // Rewritten in place at the same size: the head hash differs.
        ledger_trace();
        let text = fs::read_to_string(&log).unwrap();
        let rewritten = text.replace("\"aa\"", "\"zz\"");
        assert_eq!(rewritten.len(), text.len());
        fs::write(&log, rewritten).unwrap();
        touch(&log, 4);
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["replace"]);
        assert_eq!(stored(&v1, &log), cold(&log));

        // Truncated: shorter than the offset.
        ledger_trace();
        write_lines(&log, &[said("m1", &["aa"])]);
        touch(&log, 5);
        scan(&mut cache);
        assert_eq!(ledger_trace(), ["replace"]);
        assert_eq!(stored(&v1, &log), cold(&log));

        // Rotated: another file renamed over the path, a new inode.
        ledger_trace();
        let next = root.join("next.jsonl");
        write_lines(
            &next,
            &[said("m9", &["rotated"]), said("m10", &["and longer"])],
        );
        fs::rename(&next, &log).unwrap();
        touch(&log, 6);
        let index = scan(&mut cache);
        assert_eq!(ledger_trace(), ["replace"]);
        assert_eq!(index_snapshot(&index), cold(&log));
        assert_eq!(stored(&v1, &log), cold(&log));

        // Gone: its rows go with it.
        fs::remove_file(&log).unwrap();
        cache.retain(&BTreeSet::new(), &mut false).unwrap();
        assert_eq!(
            EventCache::open(&v1, &crate::Options::default().source)
                .unwrap()
                .paths()
                .count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_change_another_process_committed_first_is_redone_from_its_rows() {
        let root = scratch("conflict");
        let v1 = root.join("index.json");
        let log = root.join("session.jsonl");
        write_lines(&log, &[said("m1", &["aa"]), claude_tool("t1")]);
        touch(&log, 1);
        let mut first = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        scan_file(&log, "claude", &mut first, &mut false).unwrap();
        append_lines(&log, &[claude_result("t1")]);
        touch(&log, 2);
        // Between this process's read and its commit, another process reads
        // a later line too and commits first.
        let (other_v1, other_log) = (v1.clone(), log.clone());
        BEFORE_COMMIT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                append_lines(&other_log, &[said("m2", &["bb"])]);
                touch(&other_log, 3);
                let mut second =
                    EventCache::open(&other_v1, &crate::Options::default().source).unwrap();
                scan_file(&other_log, "claude", &mut second, &mut false).unwrap();
            }));
        });
        ledger_trace();
        let index = scan_file(&log, "claude", &mut first, &mut false).unwrap();
        // The first commit is refused, and the retry finds the other
        // process's rows already current: nothing is written twice.
        assert_eq!(
            ledger_trace(),
            ["append", "append", "conflict", "unchanged"]
        );
        assert_eq!(index_snapshot(&index), cold(&log));
        assert_eq!(stored(&v1, &log), cold(&log));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_processes_indexing_at_once_end_equal_to_a_cold_read() {
        let root = scratch("writers");
        let v1 = root.join("index.json");
        let logs: Vec<PathBuf> = (0..3)
            .map(|n| {
                let log = root.join(format!("s{n}.jsonl"));
                write_lines(&log, &[said(&format!("m{n}"), &["start"])]);
                log
            })
            .collect();
        drop(EventCache::open(&v1, &crate::Options::default().source).unwrap());
        let first_commit = Arc::new(std::sync::Barrier::new(2));
        let writers: Vec<_> = (0..2)
            .map(|writer| {
                let (v1, logs) = (v1.clone(), logs.clone());
                let first_commit = Arc::clone(&first_commit);
                std::thread::spawn(move || {
                    let mut cache =
                        EventCache::open(&v1, &crate::Options::default().source).unwrap();
                    assert!(cache.store.is_some());
                    let mut scans = 0usize;
                    for round in 0..20 {
                        let id = format!("t{writer}-{round}");
                        append_lines(
                            &logs[writer],
                            &[said(&format!("m-{id}"), &["line"]), claude_tool(&id)],
                        );
                        append_lines(&logs[writer], &[claude_result(&id)]);
                        if round == 0 {
                            // Both producers have appended. Keep the peer in
                            // this phase until one new source is durably indexed,
                            // so progress cannot depend on scheduling luck or
                            // the final single-threaded catch-up.
                            first_commit.wait();
                            if writer == 0 {
                                let index =
                                    scan_file(&logs[0], "claude", &mut cache, &mut false).unwrap();
                                assert!(!index.events.is_empty());
                                assert_eq!(stored(&v1, &logs[0]), cold(&logs[0]));
                                scans += 1;
                            }
                            first_commit.wait();
                        }
                        for log in &logs {
                            if let Err(error) = scan_file(log, "claude", &mut cache, &mut false) {
                                // Busy exhausts the bounded writer wait; Conflict
                                // exhausts the three CAS attempts. The ordinary
                                // viewer records either error and schedules its
                                // next check (MachineView::tick/checked).
                                assert!(
                                    matches!(
                                        error
                                            .get_ref()
                                            .and_then(|error| error
                                                .downcast_ref::<crate::storage::StorageError>()),
                                        Some(
                                            crate::storage::StorageError::Busy
                                                | crate::storage::StorageError::Conflict
                                        )
                                    ),
                                    "{error:?}: {:?}",
                                    ledger_trace()
                                );
                            } else {
                                scans += 1;
                            }
                        }
                    }
                    scans
                })
            })
            .collect();
        let scans: usize = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .sum();
        assert!(
            scans > 0,
            "concurrent scans must make progress before catch-up"
        );
        eprintln!("concurrent successful scans before catch-up={scans}; coordinated commits=1");
        // A fresh process resumes from committed rows. Any row written twice
        // or lost under the concurrent writers shows in the persisted oracle.
        let mut last = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        for log in &logs {
            scan_file(log, "claude", &mut last, &mut false).unwrap();
        }
        for log in &logs {
            assert_eq!(stored(&v1, log), cold(log));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_usage_lost_store_rejects_compact_totals_until_reopened() {
        for known_model in [false, true] {
            let root = scratch("native-usage-store-loss");
            let v1 = root.join("index.json");
            let log = root.join("session.jsonl");
            let usage = serde_json::json!({"type":"token_usage_record","payload":{
                "thread_id":"native","session_id":"session","turn_id":"turn",
                "response_id":"response","usage":{"input_tokens":7,"output_tokens":0,
                "total_tokens":7,"cached_input_tokens":0,"cache_write_input_tokens":0,
                "reasoning_output_tokens":0}}});
            let mut records =
                vec![serde_json::json!({"type":"session_meta","payload":{"id":"native"}})];
            if known_model {
                records.push(
                    serde_json::json!({"type":"turn_context","payload":{"model":"gpt-6-sol"}}),
                );
            }
            records.push(usage.clone());
            write_lines(&log, &records);
            let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
            let index = scan_file(&log, "codex", &mut cache, &mut false).unwrap();
            assert_eq!(index.tokens("codex").input, if known_model { 7 } else { 0 });
            assert_eq!(
                index.tree_summary.as_ref().unwrap().tokens("codex").input,
                7
            );
            drop(index);
            cache.store = None;
            assert!(
                scan_file(&log, "codex", &mut cache, &mut false)
                    .unwrap_err()
                    .get_ref()
                    .and_then(|error| error.downcast_ref::<crate::storage::StorageError>())
                    == Some(&crate::storage::StorageError::Unavailable)
            );
            append_lines(&log, &[usage]);
            assert!(
                scan_file(&log, "codex", &mut cache, &mut false)
                    .unwrap_err()
                    .get_ref()
                    .and_then(|error| error.downcast_ref::<crate::storage::StorageError>())
                    == Some(&crate::storage::StorageError::Unavailable)
            );
            let mut reopened = EventCache::open(&v1, &crate::Options::default().source).unwrap();
            let index = scan_file(&log, "codex", &mut reopened, &mut false).unwrap();
            assert_eq!(index.tokens("codex").input, if known_model { 7 } else { 0 });
            assert_eq!(
                index.tree_summary.as_ref().unwrap().tokens("codex").input,
                7
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn a_damaged_store_is_refused_without_replacement() {
        let root = scratch("damaged");
        let v1 = root.join("index.json");
        let garbage = vec![b'x'; 8192];
        fs::write(EventCache::path(&v1), &garbage).unwrap();
        let log = root.join("session.jsonl");
        write_lines(&log, &[said("m1", &["aa"])]);
        assert!(EventCache::open(&v1, &crate::Options::default().source).is_err());
        assert_eq!(fs::read(EventCache::path(&v1)).unwrap(), garbage);
        assert!(!root.join("index.sqlite3.corrupt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_store_and_its_wal_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let loose = |path: &Path| {
            fs::create_dir_all(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let root = scratch("private");
        // A machine's `.semon`, left readable: tightened, and nothing above.
        let machine = root.join("machine");
        let own = machine.join(".semon");
        loose(&own);
        loose(&machine);
        let v1 = own.join("index.json");
        let log = root.join("session.jsonl");
        write_lines(&log, &[said("m1", &["aa"])]);
        let mut cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        scan_file(&log, "claude", &mut cache, &mut false).unwrap();
        let path = EventCache::path(&v1);
        assert_eq!(mode(&own), 0o700);
        assert_eq!(mode(&machine), 0o755);
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.clone().into_os_string();
            file.push(suffix);
            assert_eq!(mode(Path::new(&file)), 0o600, "{suffix}");
        }
        drop(cache);
        // Loosened by something else: tightened when opened.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let cache = EventCache::open(&v1, &crate::Options::default().source).unwrap();
        assert!(cache.store.is_some());
        assert_eq!(mode(&path), 0o600);
        drop(cache);

        // An absolute `--cache` in a user's own tree, even a checkout named
        // `semon`: no existing directory changes; a missing one is created
        // owner-only.
        let checkout = root.join("projects/semon");
        loose(&checkout);
        let direct = EventCache::open(
            &checkout.join("idx.json"),
            &crate::Options::default().source,
        )
        .unwrap();
        assert!(direct.store.is_some());
        let nested = EventCache::open(
            &checkout.join("tmp/idx.json"),
            &crate::Options::default().source,
        )
        .unwrap();
        assert!(nested.store.is_some());
        assert_eq!(mode(&checkout), 0o755);
        assert_eq!(mode(&checkout.join("tmp")), 0o700);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_store_path_is_refused() {
        let root = scratch("symlink");
        let v1 = root.join("index.json");
        let target = root.join("elsewhere.sqlite3");
        fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, EventCache::path(&v1)).unwrap();
        assert!(EventCache::open(&v1, &crate::Options::default().source).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"", "the target is untouched");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_directory_refuses_without_memory_fallback() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("read-only");
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
        let restore = || fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        if fs::write(state.join("probe"), b"").is_ok() {
            // A privileged user writes anyway: nothing to test here.
            restore();
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let v1 = state.join("index.json");
        let log = root.join("session.jsonl");
        write_lines(&log, &[said("m1", &["aa"]), claude_tool("t1")]);
        assert!(EventCache::open(&v1, &crate::Options::default().source).is_err());
        assert!(!EventCache::path(&v1).exists());
        restore();
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn native_connector_tool_classification_refuses_incomplete_identity_and_keeps_unknown_outcomes() {
    let mut index = FileIndex::default();
    for item in [
        serde_json::json!({"type":"McpToolCall","tool":"search","result":{"content":[]}}),
        serde_json::json!({"type":"DynamicToolCall","id":"missing-tool","summary":[{"text":"not thinking"}]}),
        serde_json::json!({"type":"FutureTool","id":"future","tool":"search","summary":[{"text":"not thinking"}]}),
    ] {
        codex(
            &mut index,
            &serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":item}}),
            0,
        );
    }
    assert!(index.events.is_empty());
    assert!(index.extras.is_empty());
    codex(
        &mut index,
        &serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"DynamicToolCall","id":"unknown","tool":"search","arguments":{}}}}),
        1,
    );
    assert_eq!(index.events.len(), 1);
    assert_eq!(index.events[0].k, Kind::Tool);
    assert_eq!(index.events[0].r.as_ref().unwrap().f, UNKNOWN);
    assert!(index.pending.is_empty());
}
