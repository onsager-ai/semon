use std::time::{SystemTime, UNIX_EPOCH};

use semon_store::RepoSource;
use serde_json::{Map, Number, Value};

/// Correlation and attribution carried between Codex session records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizeContext {
    pub(crate) session_id: String,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) repo: String,
    /// The basis for `repo`, maintained alongside it: set whenever `repo` is
    /// set from a git remote URL or a cwd basename, and reset to `None` when
    /// `repo` is empty. `--repo`/`SEMON_REPO` always wins independently of
    /// this field (see `normalize_record`'s final `repo`/`repo_source`
    /// computation), so this field only needs to track the two inferred
    /// bases.
    pub(crate) repo_source: RepoSource,
    pub(crate) cwd: String,
    pub(crate) calls: Map<String, Value>,
    /// Whether this session file has been seen to carry the `item_completed`
    /// stream, decided once per file (see `lib::file_uses_item_stream`) and
    /// then held fixed for the rest of that file's processing. When set, the
    /// legacy `response_item`/`message` mirror is no longer the source of
    /// message content for this file, so it no longer projects into the
    /// semantic region.
    pub(crate) has_item_stream: bool,
}

impl Default for NormalizeContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            parent_session_id: None,
            agent: None,
            repo: String::new(),
            repo_source: RepoSource::None,
            cwd: String::new(),
            calls: Map::new(),
            has_item_stream: false,
        }
    }
}

impl NormalizeContext {
    /// Returns the current Codex session identifier.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the subagent path, or its nickname when no path was supplied.
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// Returns the repository attribution inferred for the session.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Returns the basis for [`NormalizeContext::repo`].
    pub fn repo_source(&self) -> RepoSource {
        self.repo_source
    }

    pub(crate) fn set_session_meta_identity(&mut self, payload: &Map<String, Value>) {
        let parent = payload
            .get("session_id")
            .filter(|value| truthy(value))
            .or_else(|| payload.get("id").filter(|value| truthy(value)))
            .map(py_string)
            .unwrap_or_else(|| self.session_id.clone());
        let id = payload.get("id").and_then(Value::as_str);
        if payload.get("thread_source").and_then(Value::as_str) == Some("subagent")
            && let Some(id) = id
            && id != parent
        {
            self.session_id = format!("{parent}/agent-{id}");
            self.parent_session_id = Some(parent);
            self.agent = payload
                .get("agent_path")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| payload.get("agent_nickname").and_then(Value::as_str))
                .map(str::to_owned);
        } else {
            self.session_id = parent;
            self.parent_session_id = None;
            self.agent = None;
        }
    }
}

/// Parses a Unix timestamp in seconds/milliseconds or an ISO-8601 string to nanoseconds.
pub fn parse_timestamp(value: &Value) -> i64 {
    if let Some(number) = value.as_f64() {
        let seconds = if number > 10_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        return (seconds * 1_000_000_000.0) as i64;
    }
    if let Some(text) = value.as_str()
        && let Some(timestamp) = parse_iso_timestamp(text.trim())
    {
        return timestamp;
    }
    now_ns()
}

fn now_ns() -> i64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

// This accepts the calendar-date forms emitted by Codex, including a space or
// `T` separator, fractional seconds, `Z`, and numeric UTC offsets. Python's
// `datetime.fromisoformat` additionally accepts ISO week dates; implementing
// those without a time dependency would expand the carrier surface for a form
// Codex has never emitted, so unsupported forms retain the original fallback
// behavior (capture time).
fn parse_iso_timestamp(input: &str) -> Option<i64> {
    if input.len() < 10 {
        return None;
    }
    let year = parse_i64(input.get(0..4)?)?;
    if input.as_bytes().get(4) != Some(&b'-') || input.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    let month = parse_u32(input.get(5..7)?)?;
    let day = parse_u32(input.get(8..10)?)?;
    if !valid_date(year, month, day) {
        return None;
    }
    if input.len() == 10 {
        return days_from_civil(year, month, day).checked_mul(86_400_000_000_000);
    }
    if input.len() < 19 {
        return None;
    }
    let time = input.get(11..)?;
    if time.as_bytes().get(2) != Some(&b':') || time.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    let hour = parse_u32(time.get(0..2)?)?;
    let minute = parse_u32(time.get(3..5)?)?;
    let second = parse_u32(time.get(6..8)?)?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let remainder = time.get(8..)?;
    let zone_start = remainder
        .char_indices()
        .find(|(_, value)| matches!(value, 'Z' | 'z' | '+' | '-'))
        .map_or(remainder.len(), |(index, _)| index);
    let fraction = &remainder[..zone_start];
    let zone = &remainder[zone_start..];
    let fractional_ns = if fraction.is_empty() {
        0
    } else {
        let digits = fraction.strip_prefix(['.', ','])?;
        fraction_to_ns(digits)?
    };
    let offset_seconds = parse_offset(zone)?;
    let day_seconds = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    let seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(day_seconds)?
        .checked_sub(offset_seconds)?;
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(fractional_ns)
}

fn parse_offset(zone: &str) -> Option<i64> {
    if zone.is_empty() || zone.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let (sign, rest) = match zone.as_bytes().first()? {
        b'+' => (1_i64, &zone[1..]),
        b'-' => (-1_i64, &zone[1..]),
        _ => return None,
    };
    let compact = rest.replace(':', "");
    if compact.len() != 2 && compact.len() != 4 {
        return None;
    }
    let hour = parse_u32(compact.get(0..2)?)?;
    let minute = if compact.len() == 4 {
        parse_u32(compact.get(2..4)?)?
    } else {
        0
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * (i64::from(hour) * 3600 + i64::from(minute) * 60))
}

fn fraction_to_ns(digits: &str) -> Option<i64> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut padded = digits.bytes().take(9).collect::<Vec<_>>();
    padded.resize(9, b'0');
    std::str::from_utf8(&padded).ok()?.parse().ok()
}

fn valid_date(year: i64, month: u32, day: u32) -> bool {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let limit = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=limit).contains(&day)
}

// Howard Hinnant's civil-date conversion, shifted to the Unix epoch.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_i64(value: &str) -> Option<i64> {
    value.parse().ok()
}

fn parse_u32(value: &str) -> Option<u32> {
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{apply_path_rule, parse_iso_timestamp};

    #[test]
    fn path_rule_normalizes_relative_and_absolute_escapes() {
        assert_eq!(
            apply_path_rule("../../elsewhere/secrets.env", "/work/repo"),
            "<external>"
        );
        assert_eq!(
            apply_path_rule("src/../../secrets.env", "/work/repo"),
            "<external>"
        );
        assert_eq!(apply_path_rule("src/../main.rs", "/work/repo"), "main.rs");
        assert_eq!(
            apply_path_rule("/work/repo/../secrets.env", "/work/repo"),
            "<external>"
        );
        assert_eq!(
            apply_path_rule("/work/repo/src/../main.rs", "/work/repo"),
            "main.rs"
        );
    }

    #[test]
    fn rejects_fraction_without_decimal_separator() {
        assert_eq!(parse_iso_timestamp("2026-07-31T01:02:03xZ"), None);
    }
}

/// Renders a JSON value as compact UTF-8 text, preserving strings directly.
pub fn json_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).expect("serializing a parsed JSON value cannot fail"),
    }
}

/// Extracts human-readable text from Codex message/content variants.
pub fn message_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(value) => Some(value.clone()),
                Value::Object(object) => object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => ["text", "message", "content"]
            .into_iter()
            .find_map(|key| object.get(key).map(message_text))
            .unwrap_or_else(|| json_text(value)),
        _ => json_text(value),
    }
}

/// Infers a repository name from an HTTPS, SSH, or scp-style Git URL.
pub fn repo_from_url(value: &Value) -> String {
    let Some(url) = value.as_str().filter(|value| !value.is_empty()) else {
        return String::new();
    };
    let mut value = url.trim_end_matches('/');
    if let Some(without_git) = value.strip_suffix(".git") {
        value = without_git;
    }
    value
        .rsplit_once('/')
        .map_or(value, |(_, tail)| tail)
        .rsplit_once(':')
        .map_or_else(
            || value.rsplit('/').next().unwrap_or(value),
            |(_, tail)| tail,
        )
        .to_owned()
}

/// Infers a repository name from the final component of a working directory.
pub fn repo_from_cwd(value: &Value) -> String {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .and_then(|value| std::path::Path::new(value).file_name())
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Extracts input and output counts from Codex's most recent usage object.
pub fn token_usage(payload: &Map<String, Value>) -> (i64, i64) {
    let Some(info) = payload.get("info").and_then(Value::as_object) else {
        return (0, 0);
    };
    let usage = info
        .get("last_token_usage")
        .filter(|value| truthy(value))
        .or_else(|| info.get("last_usage").filter(|value| truthy(value)))
        .and_then(Value::as_object);
    let Some(usage) = usage else {
        return (0, 0);
    };
    let tokens_in = int_value(usage.get("input_tokens")).unwrap_or(0);
    let tokens_out = int_value(usage.get("output_tokens")).unwrap_or(0)
        + int_value(usage.get("reasoning_output_tokens")).unwrap_or(0);
    (tokens_in, tokens_out)
}

/// Infers success from Codex result/status/exit-code variants.
pub fn infer_success(value: &Value, default: bool) -> bool {
    if let Value::Object(object) = value {
        if let Some(success) = object.get("success").and_then(Value::as_bool) {
            return success;
        }
        if let Some(status) = object.get("status").and_then(Value::as_str) {
            match status.to_ascii_lowercase().as_str() {
                "failed" | "error" | "cancelled" | "rejected" => return false,
                "success" | "succeeded" | "completed" | "ok" => return true,
                _ => {}
            }
        }
        for key in ["exit_code", "exitCode"] {
            if let Some(code) = object.get(key).and_then(Value::as_i64) {
                return code == 0;
            }
        }
        if let Some(metadata) = object.get("metadata").filter(|value| value.is_object()) {
            return infer_success(metadata, default);
        }
    }
    if let Some(encoded) = value.as_str()
        && let Ok(decoded) = serde_json::from_str(encoded)
    {
        return infer_success(&decoded, default);
    }
    default
}

#[derive(Default)]
struct EventFields {
    tool: String,
    decision: String,
    tokens_in: i64,
    tokens_out: i64,
    duration_ms: i64,
    prompt: String,
    response: String,
    tool_input: String,
    tool_output: String,
    success: Option<bool>,
}

fn normalized_event(
    timestamp: &Value,
    session_id: &str,
    repo: &str,
    repo_source: RepoSource,
    kind: &str,
    source: &Value,
    fields: EventFields,
) -> Value {
    let mut extras = source.as_object().cloned().unwrap_or_default();
    if let Some(success) = fields.success {
        extras.insert("success".into(), Value::String(success.to_string()));
    }
    let extras = sort_json(Value::Object(extras));
    let mut event = Map::new();
    event.insert(
        "ts".into(),
        Value::Number(parse_timestamp(timestamp).into()),
    );
    event.insert("session_id".into(), Value::String(session_id.to_owned()));
    event.insert("repo".into(), Value::String(repo.to_owned()));
    event.insert(
        "repo_source".into(),
        Value::String(repo_source.as_str().to_owned()),
    );
    event.insert("kind".into(), Value::String(kind.to_owned()));
    event.insert("tool".into(), Value::String(fields.tool));
    event.insert("decision".into(), Value::String(fields.decision));
    event.insert(
        "tokens_in".into(),
        Value::Number(fields.tokens_in.max(0).into()),
    );
    event.insert(
        "tokens_out".into(),
        Value::Number(fields.tokens_out.max(0).into()),
    );
    event.insert("cost_usd".into(), Value::Number(Number::from(0)));
    event.insert(
        "duration_ms".into(),
        Value::Number(fields.duration_ms.max(0).into()),
    );
    event.insert(
        "extras".into(),
        Value::String(
            serde_json::to_string(&extras).expect("JSON value serialization cannot fail"),
        ),
    );
    event.insert("prompt".into(), Value::String(fields.prompt));
    event.insert("response".into(), Value::String(fields.response));
    event.insert("tool_input".into(), Value::String(fields.tool_input));
    event.insert("tool_output".into(), Value::String(fields.tool_output));
    Value::Object(event)
}

/// Normalizes one Codex session JSON record and updates correlation context.
pub fn normalize_record(
    record: &Value,
    context: &mut NormalizeContext,
    repo_override: &str,
) -> Value {
    let empty = Map::new();
    let record_object = record.as_object().unwrap_or(&empty);
    let top_type = record_object
        .get("type")
        .filter(|value| truthy(value))
        .map(py_string)
        .unwrap_or_else(|| "unknown".to_owned());
    let payload = record_object
        .get("payload")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let payload_type = payload
        .get("type")
        .filter(|value| truthy(value))
        .map(py_string)
        .unwrap_or_else(|| top_type.clone());
    let timestamp = record_object
        .get("timestamp")
        .filter(|value| truthy(value))
        .or_else(|| payload.get("timestamp"))
        .unwrap_or(&Value::Null);

    if top_type == "session_meta" {
        context.set_session_meta_identity(payload);
        context.cwd = payload
            .get("cwd")
            .filter(|value| truthy(value))
            .map(py_string)
            .or_else(|| (!context.cwd.is_empty()).then(|| context.cwd.clone()))
            .unwrap_or_default();
        let git_url = payload
            .get("git")
            .and_then(Value::as_object)
            .and_then(|git| git.get("repository_url"))
            .unwrap_or(&Value::Null);
        if repo_override.is_empty() {
            let from_url = repo_from_url(git_url);
            if !from_url.is_empty() {
                context.repo = from_url;
                context.repo_source = RepoSource::GitRemote;
            } else {
                let from_cwd = repo_from_cwd(&Value::String(context.cwd.clone()));
                if !from_cwd.is_empty() {
                    context.repo = from_cwd;
                    context.repo_source = RepoSource::CwdBasename;
                }
                // Otherwise `context.repo`/`context.repo_source` are left
                // exactly as they were: neither source yielded anything new.
            }
        } else {
            context.repo = repo_override.to_owned();
            context.repo_source = RepoSource::ExplicitOverride;
        }
    } else if top_type == "turn_context" {
        context.cwd = payload
            .get("cwd")
            .filter(|value| truthy(value))
            .map(py_string)
            .or_else(|| (!context.cwd.is_empty()).then(|| context.cwd.clone()))
            .unwrap_or_default();
        if repo_override.is_empty() && context.repo.is_empty() {
            let from_cwd = repo_from_cwd(&Value::String(context.cwd.clone()));
            if !from_cwd.is_empty() {
                context.repo = from_cwd;
                context.repo_source = RepoSource::CwdBasename;
            }
        }
    }

    let session_id = context.session_id.clone();
    let (repo, repo_source) = if !repo_override.is_empty() {
        (repo_override.to_owned(), RepoSource::ExplicitOverride)
    } else if context.repo.is_empty() {
        (String::new(), RepoSource::None)
    } else {
        (context.repo.clone(), context.repo_source)
    };
    let base = |kind: &str, fields: EventFields| {
        normalized_event(
            timestamp,
            &session_id,
            &repo,
            repo_source,
            kind,
            record,
            fields,
        )
    };

    if top_type == "event_msg" && payload_type == "user_message" {
        return base(
            "user_prompt",
            EventFields {
                prompt: message_text(payload.get("message").unwrap_or(&Value::Null)),
                ..EventFields::default()
            },
        );
    }
    if top_type == "event_msg" && payload_type == "agent_message" {
        return base(
            "assistant_response",
            EventFields {
                response: message_text(payload.get("message").unwrap_or(&Value::Null)),
                ..EventFields::default()
            },
        );
    }
    if top_type == "event_msg" && payload_type == "token_count" {
        let (tokens_in, tokens_out) = token_usage(payload);
        return base(
            "api_request",
            EventFields {
                tokens_in,
                tokens_out,
                ..EventFields::default()
            },
        );
    }
    if top_type == "event_msg" && payload_type == "task_complete" {
        return base(
            "turn_complete",
            EventFields {
                duration_ms: int_value(payload.get("duration_ms")).unwrap_or(0),
                response: message_text(payload.get("last_agent_message").unwrap_or(&Value::Null)),
                ..EventFields::default()
            },
        );
    }
    if top_type == "event_msg" && payload_type == "patch_apply_end" {
        return base(
            "tool_result",
            EventFields {
                tool: "apply_patch".into(),
                duration_ms: int_value(payload.get("duration_ms")).unwrap_or(0),
                tool_output: json_text(&Value::Object(Map::from_iter([
                    (
                        "stdout".into(),
                        payload.get("stdout").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "stderr".into(),
                        payload.get("stderr").cloned().unwrap_or(Value::Null),
                    ),
                ]))),
                success: Some(infer_success(&Value::Object(payload.clone()), true)),
                ..EventFields::default()
            },
        );
    }
    if top_type == "event_msg" && payload_type == "mcp_tool_call_end" {
        let tool = ["app_name", "action_name"]
            .into_iter()
            .filter_map(|key| {
                payload
                    .get(key)
                    .filter(|value| truthy(value))
                    .map(py_string)
            })
            .collect::<Vec<_>>()
            .join(".");
        let result = payload.get("result").unwrap_or(&Value::Null);
        return base(
            "tool_result",
            EventFields {
                tool: if tool.is_empty() {
                    "mcp_tool".into()
                } else {
                    tool
                },
                duration_ms: float_value(payload.get("duration")).unwrap_or(0.0) as i64,
                tool_input: json_text(payload.get("invocation").unwrap_or(&Value::Null)),
                tool_output: json_text(result),
                success: Some(infer_success(result, true)),
                ..EventFields::default()
            },
        );
    }
    if top_type == "event_msg"
        && matches!(
            payload_type.as_str(),
            "web_search_end" | "image_generation_end"
        )
    {
        let (tool, input_key) = if payload_type == "web_search_end" {
            ("web_search", "query")
        } else {
            ("image_generation", "prompt")
        };
        return base(
            "tool_result",
            EventFields {
                tool: tool.into(),
                tool_input: json_text(payload.get(input_key).unwrap_or(&Value::Null)),
                tool_output: json_text(
                    payload
                        .get("results")
                        .or_else(|| payload.get("result"))
                        .unwrap_or(&Value::Null),
                ),
                success: Some(infer_success(&Value::Object(payload.clone()), true)),
                ..EventFields::default()
            },
        );
    }

    if top_type == "response_item"
        && matches!(
            payload_type.as_str(),
            "function_call" | "custom_tool_call" | "tool_search_call"
        )
    {
        let call_id = payload
            .get("call_id")
            .filter(|value| truthy(value))
            .or_else(|| payload.get("id").filter(|value| truthy(value)))
            .map(py_string)
            .unwrap_or_default();
        let mut tool = payload
            .get("name")
            .filter(|value| truthy(value))
            .map(py_string)
            .unwrap_or_else(|| payload_type.trim_end_matches("_call").to_owned());
        if let Some(namespace) = payload.get("namespace").filter(|value| truthy(value)) {
            tool = format!("{}.{}", py_string(namespace), tool);
        }
        let tool_input = json_text(
            payload
                .get("arguments")
                .or_else(|| payload.get("input"))
                .unwrap_or(&Value::Null),
        );
        if !call_id.is_empty() {
            context.calls.insert(
                call_id,
                Value::Object(Map::from_iter([
                    ("tool".into(), Value::String(tool.clone())),
                    ("input".into(), Value::String(tool_input.clone())),
                ])),
            );
        }
        return base(
            "tool_call",
            EventFields {
                tool,
                tool_input,
                ..EventFields::default()
            },
        );
    }

    if top_type == "response_item"
        && matches!(
            payload_type.as_str(),
            "function_call_output" | "custom_tool_call_output" | "tool_search_output"
        )
    {
        let call_id = payload.get("call_id").map(py_string).unwrap_or_default();
        let call = if call_id.is_empty() {
            Map::new()
        } else {
            context
                .calls
                .remove(&call_id)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default()
        };
        let output = payload
            .get("output")
            .or_else(|| payload.get("tools"))
            .unwrap_or(&Value::Null);
        let default_success = infer_success(output, true);
        return base(
            "tool_result",
            EventFields {
                tool: call
                    .get("tool")
                    .filter(|value| truthy(value))
                    .map(py_string)
                    .unwrap_or_else(|| payload_type.trim_end_matches("_output").to_owned()),
                tool_input: call
                    .get("input")
                    .filter(|value| truthy(value))
                    .map(py_string)
                    .unwrap_or_default(),
                tool_output: json_text(output),
                success: Some(infer_success(
                    &Value::Object(payload.clone()),
                    default_success,
                )),
                ..EventFields::default()
            },
        );
    }

    if top_type == "response_item" && payload_type == "message" {
        let role = payload.get("role").map(py_string).unwrap_or_default();
        // Harness boilerplate is never transferable work: `developer`-role
        // messages are Codex's own label for framing it injects, never the
        // agent's or the user's. And once a session file is known to carry
        // the `item_completed` item stream, that stream — not this legacy
        // mirror — is the source of message content, so the mirror (which
        // also carries injections, such as `<recommended_plugins>`, that
        // never appear in the item stream at all) is no longer projected.
        if !matches!(role.as_str(), "user" | "assistant") || context.has_item_stream {
            return base("legacy_message", EventFields::default());
        }
        let content = message_text(payload.get("content").unwrap_or(&Value::Null));
        if role == "user" {
            if is_harness_prompt(&content) {
                return base("legacy_message", EventFields::default());
            }
            return base(
                "user_prompt",
                EventFields {
                    prompt: content,
                    ..EventFields::default()
                },
            );
        }
        return base(
            "assistant_response",
            EventFields {
                response: content,
                ..EventFields::default()
            },
        );
    }

    if top_type == "event_msg" && payload_type == "item_completed" {
        let empty_item = Map::new();
        let item = payload
            .get("item")
            .and_then(Value::as_object)
            .unwrap_or(&empty_item);
        return item_completed_event(
            timestamp,
            &session_id,
            &repo,
            repo_source,
            &context.cwd,
            item,
        );
    }

    base(&payload_type, EventFields::default())
}

/// Builds a normalized event from one `event_msg`/`item_completed` item.
///
/// Current Codex sessions carry real work almost entirely through this
/// stream (see issue #9's two-day census). Only `UserMessage` and
/// `AgentMessage` project text content; `CommandExecution` and `FileChange`
/// project a machine-independent `action` core; `Reasoning`, `Extension`, and
/// `ContextCompaction` are deliberately not projected (consumed for cursor
/// purposes only, exactly like any other unrecognized kind falling through
/// to the generic `base` branch).
fn item_completed_event(
    timestamp: &Value,
    session_id: &str,
    repo: &str,
    repo_source: RepoSource,
    cwd: &str,
    item: &Map<String, Value>,
) -> Value {
    let item_type = item
        .get("type")
        .filter(|value| truthy(value))
        .map(py_string)
        .unwrap_or_default();

    let mut event = Map::new();
    event.insert(
        "ts".into(),
        Value::Number(parse_timestamp(timestamp).into()),
    );
    event.insert("session_id".into(), Value::String(session_id.to_owned()));
    event.insert("repo".into(), Value::String(repo.to_owned()));
    event.insert(
        "repo_source".into(),
        Value::String(repo_source.as_str().to_owned()),
    );

    match item_type.as_str() {
        "UserMessage" => {
            event.insert("kind".into(), Value::String("item_user_message".into()));
            event.insert(
                "content".into(),
                Value::String(message_text(item.get("content").unwrap_or(&Value::Null))),
            );
        }
        "AgentMessage" => {
            event.insert("kind".into(), Value::String("item_agent_message".into()));
            event.insert(
                "content".into(),
                Value::String(message_text(item.get("content").unwrap_or(&Value::Null))),
            );
        }
        "CommandExecution" => {
            event.insert(
                "kind".into(),
                Value::String("item_command_execution".into()),
            );
            let first_parsed = item
                .get("parsed_cmd")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(Value::as_object);
            let action = first_parsed
                .and_then(|entry| entry.get("type"))
                .filter(|value| truthy(value))
                .map(py_string)
                .unwrap_or_else(|| "unknown".to_owned());
            let path = first_parsed
                .and_then(|entry| entry.get("path"))
                .and_then(Value::as_str)
                .map(|raw_path| Value::String(apply_path_rule(raw_path, cwd)))
                .unwrap_or(Value::Null);
            event.insert("action".into(), Value::String(action));
            event.insert("path".into(), path);
            if let Some(parsed) = item.get("parsed_cmd").and_then(Value::as_array)
                && parsed.len() > 1
            {
                // Preserve parser order without storing raw commands. The
                // overall exit code applies to the compound command, not to
                // any individual component's execution status.
                let components = parsed
                    .iter()
                    .map(|entry| {
                        let action = entry
                            .get("type")
                            .filter(|value| truthy(value))
                            .map(py_string)
                            .unwrap_or_else(|| "unknown".into());
                        let path = entry
                            .get("path")
                            .and_then(Value::as_str)
                            .map(|path| Value::String(apply_path_rule(path, cwd)))
                            .unwrap_or(Value::Null);
                        serde_json::json!({"action":action,"path":path})
                    })
                    .collect();
                event.insert("components".into(), Value::Array(components));
            }
            // Codex has emitted `exit_code` on every observed CommandExecution
            // item; -1 marks the (unobserved) absent case distinctly from a
            // genuine zero (success) exit code.
            event.insert(
                "exit_code".into(),
                Value::Number(int_value(item.get("exit_code")).unwrap_or(-1).into()),
            );
        }
        "FileChange" => {
            event.insert("kind".into(), Value::String("item_file_change".into()));
            let changes = item
                .get("changes")
                .and_then(Value::as_object)
                .map(|changes| {
                    changes
                        .iter()
                        .map(|(raw_path, change)| {
                            let change_kind = change
                                .get("type")
                                .and_then(Value::as_str)
                                .map(map_file_change_kind)
                                .unwrap_or_else(|| "modify".to_owned());
                            Value::Object(Map::from_iter([
                                ("path".into(), Value::String(apply_path_rule(raw_path, cwd))),
                                ("change".into(), Value::String(change_kind)),
                            ]))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            event.insert("changes".into(), Value::Array(changes));
        }
        "Reasoning" => {
            event.insert("kind".into(), Value::String("item_reasoning".into()));
        }
        "Extension" => {
            event.insert("kind".into(), Value::String("item_extension".into()));
        }
        "ContextCompaction" => {
            event.insert(
                "kind".into(),
                Value::String("item_context_compaction".into()),
            );
        }
        _ => {
            event.insert("kind".into(), Value::String("item_unrecognized".into()));
        }
    }

    Value::Object(event)
}

/// Maps a Codex `FileChange` entry's own `type` word to the semantic
/// region's vocabulary. Codex spells an edit `"update"`; the semantic core
/// spells it `"modify"`. Unrecognized words pass through unchanged rather
/// than being silently discarded.
fn map_file_change_kind(value: &str) -> String {
    match value {
        "update" => "modify",
        other => other,
    }
    .to_owned()
}

/// Known carrier framing is not a work prompt, including on older streams
/// without typed items. Ordinary prose and unknown XML remain user content.
fn is_harness_prompt(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix('<') else {
        return false;
    };
    [
        "environment_context",
        "user_instructions",
        "recommended_plugins",
        "skills_instructions",
        "multi_agent_mode",
        "multi_agent_role",
        "user_shell_command",
        "turn_aborted",
    ]
    .iter()
    .any(|tag| {
        rest.strip_prefix(tag)
            .is_some_and(|tail| tail.starts_with(['>', ' ', '\n']))
    })
}

/// Applies the semantic region's path rule to one absolute or relative path.
///
/// The semantic region is content-addressed and must stay machine
/// independent, so an absolute path can never enter it as-is:
///
/// - Relative paths are normalized by their text; a `..` that leaves the
///   working directory becomes `"<external>"`. No filesystem is consulted.
/// - An absolute path under `cwd` is rewritten relative to `cwd`.
/// - An absolute path outside `cwd` (or when `cwd` itself is unknown) becomes
///   the literal string `"<external>"` — never a basename, which would still
///   leak the file's name.
pub fn apply_path_rule(raw_path: &str, cwd: &str) -> String {
    fn normalized(path: &str) -> Option<String> {
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                other => parts.push(other),
            }
        }
        Some(parts.join("/"))
    }
    let Some(path) = normalized(raw_path) else {
        return "<external>".to_owned();
    };
    if !raw_path.starts_with('/') {
        return path;
    }
    if !cwd.starts_with('/') {
        return "<external>".to_owned();
    }
    let Some(base) = normalized(cwd) else {
        return "<external>".to_owned();
    };
    if path == base {
        return String::new();
    }
    if base.is_empty() {
        return path;
    }
    path.strip_prefix(&format!("{base}/"))
        .map(str::to_owned)
        .unwrap_or_else(|| "<external>".to_owned())
}

/// Normalizes one entry from Codex's separate history file.
pub fn history_event(
    record: &Value,
    session_repos: &std::collections::BTreeMap<String, (String, RepoSource)>,
    repo_override: &str,
) -> Value {
    let object = record.as_object();
    let session_id = object
        .and_then(|value| value.get("session_id"))
        .map(py_string)
        .unwrap_or_default();
    let (repo, repo_source) = if !repo_override.is_empty() {
        (repo_override.to_owned(), RepoSource::ExplicitOverride)
    } else {
        session_repos
            .get(&session_id)
            .cloned()
            .unwrap_or((String::new(), RepoSource::None))
    };
    normalized_event(
        object
            .and_then(|value| value.get("ts"))
            .unwrap_or(&Value::Null),
        &session_id,
        &repo,
        repo_source,
        "history_entry",
        record,
        EventFields {
            prompt: message_text(
                object
                    .and_then(|value| value.get("text"))
                    .unwrap_or(&Value::Null),
            ),
            ..EventFields::default()
        },
    )
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(Map::from_iter(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json(value))),
            ))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_json).collect()),
        value => value,
    }
}

fn py_string(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn int_value(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(value) => value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64)),
        Value::String(value) => value.parse().ok(),
        Value::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
}

fn float_value(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

#[cfg(test)]
mod item_stream_tests {
    use serde_json::json;

    use super::{NormalizeContext, apply_path_rule, normalize_record};

    #[test]
    fn path_rule_rewrites_a_path_under_cwd_as_relative() {
        assert_eq!(
            apply_path_rule("/work/repo/src/main.rs", "/work/repo"),
            "src/main.rs"
        );
    }

    #[test]
    fn path_rule_marks_a_path_outside_cwd_as_external() {
        assert_eq!(
            apply_path_rule("/tmp/scratch.cpp", "/work/repo"),
            "<external>"
        );
    }

    #[test]
    fn path_rule_rejects_a_same_prefix_sibling_directory() {
        // "/work/repo-other" is not under "/work/repo": a naive string
        // prefix check without a separator boundary would wrongly accept it.
        assert_eq!(
            apply_path_rule("/work/repo-other/file.rs", "/work/repo"),
            "<external>"
        );
    }

    #[test]
    fn path_rule_leaves_an_already_relative_path_unchanged() {
        assert_eq!(
            apply_path_rule("src/tolmap/multi.py", "/work/repo"),
            "src/tolmap/multi.py"
        );
    }

    #[test]
    fn path_rule_treats_an_unknown_cwd_as_external() {
        assert_eq!(apply_path_rule("/work/repo/file.rs", ""), "<external>");
    }

    fn context_with_cwd(cwd: &str) -> NormalizeContext {
        NormalizeContext {
            cwd: cwd.to_owned(),
            ..NormalizeContext::default()
        }
    }

    #[test]
    fn user_message_item_projects_as_intent() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "UserMessage",
                        "content": [{"type": "text", "text": "do the thing"}]
                    }
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "item_user_message");
        assert_eq!(event["content"], "do the thing");
    }

    #[test]
    fn agent_message_item_projects_as_outcome() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "AgentMessage",
                        "content": [{"type": "Text", "text": "done"}],
                        "phase": "commentary"
                    }
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "item_agent_message");
        assert_eq!(event["content"], "done");
    }

    #[test]
    fn command_execution_item_projects_action_path_and_exit_code() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "CommandExecution",
                        "command": ["/bin/zsh", "-lc", "sed -n '1,10p' src/main.rs"],
                        "cwd": "file:///work/repo",
                        "parsed_cmd": [
                            {"type": "read", "cmd": "sed -n '1,10p' src/main.rs", "name": "main.rs", "path": "src/main.rs"}
                        ],
                        "status": "completed",
                        "stdout": "fn main() {}\n",
                        "stderr": "",
                        "exit_code": 0
                    }
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "item_command_execution");
        assert_eq!(event["action"], "read");
        assert_eq!(event["path"], "src/main.rs");
        assert_eq!(event["exit_code"], 0);
    }

    #[test]
    fn command_execution_item_without_parsed_cmd_yields_unknown_and_no_path() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "CommandExecution",
                        "command": ["/bin/zsh", "-lc", "some || compound && thing"],
                        "cwd": "file:///work/repo",
                        "parsed_cmd": [{"type": "unknown", "cmd": "some || compound && thing"}],
                        "status": "completed",
                        "stdout": "",
                        "stderr": "",
                        "exit_code": 1
                    }
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "item_command_execution");
        assert_eq!(event["action"], "unknown");
        assert!(event["path"].is_null());
        assert_eq!(event["exit_code"], 1);
    }

    #[test]
    fn file_change_item_projects_add_and_modify_with_relative_paths() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "event_msg",
                "payload": {
                    "type": "item_completed",
                    "item": {
                        "type": "FileChange",
                        "changes": {
                            "/work/repo/src/new.rs": {"type": "add", "content": "fn a() {}\n"},
                            "/work/repo/src/old.rs": {"type": "update", "content": "fn b() {}\n"},
                            "/tmp/scratch.rs": {"type": "add", "content": "fn c() {}\n"}
                        },
                        "status": "completed",
                        "stdout": "",
                        "stderr": ""
                    }
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "item_file_change");
        let changes = event["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 3);
        let by_path = |path: &str| {
            changes
                .iter()
                .find(|entry| entry["path"] == path)
                .unwrap_or_else(|| panic!("missing change for {path}"))
        };
        assert_eq!(by_path("src/new.rs")["change"], "add");
        assert_eq!(by_path("src/old.rs")["change"], "modify");
        assert_eq!(by_path("<external>")["change"], "add");
    }

    #[test]
    fn reasoning_extension_and_compaction_items_do_not_project() {
        for (item_type, extra) in [
            ("Reasoning", json!({"summary_text": [], "raw_content": []})),
            (
                "Extension",
                json!({"kind": "web.search", "query": "q", "results": []}),
            ),
            ("ContextCompaction", json!({})),
        ] {
            let mut item = extra.as_object().cloned().unwrap();
            item.insert("type".into(), json!(item_type));
            let mut context = context_with_cwd("/work/repo");
            let event = normalize_record(
                &json!({
                    "type": "event_msg",
                    "payload": {"type": "item_completed", "item": item}
                }),
                &mut context,
                "",
            );
            let kind = event["kind"].as_str().unwrap().to_owned();
            assert!(
                kind.starts_with("item_") && kind != "item_user_message",
                "unexpected kind for {item_type}: {kind}"
            );
        }
    }

    #[test]
    fn legacy_harness_user_and_system_messages_are_never_projected() {
        for (role, text) in [
            (
                "user",
                "<recommended_plugins>injected</recommended_plugins>",
            ),
            ("user", "<environment_context>machine</environment_context>"),
            ("system", "system framing"),
        ] {
            let mut context = context_with_cwd("/work/repo");
            let event = normalize_record(
                &json!({"type":"response_item", "payload": {"type":"message", "role":role,
                "content":[{"type":"input_text", "text":text}]}}),
                &mut context,
                "",
            );
            assert_eq!(event["kind"], "legacy_message");
        }
    }

    #[test]
    fn developer_role_message_is_never_projected() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "<skills_instructions>secret framing</skills_instructions>"}]
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "legacy_message");
    }

    #[test]
    fn legacy_message_mirror_is_suppressed_once_item_stream_is_flagged() {
        let mut context = context_with_cwd("/work/repo");
        context.has_item_stream = true;
        let event = normalize_record(
            &json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "<recommended_plugins>..."}]
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "legacy_message");
    }

    #[test]
    fn legacy_message_mirror_still_projects_without_an_item_stream() {
        let mut context = context_with_cwd("/work/repo");
        let event = normalize_record(
            &json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hello"}]
                }
            }),
            &mut context,
            "",
        );
        assert_eq!(event["kind"], "user_prompt");
        assert_eq!(event["prompt"], "hello");
    }
}
