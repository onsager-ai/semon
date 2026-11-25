//! Extract only the typed Workbench trailer at a native final response.

use serde_json::Value;

use crate::field;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Outcome {
    pub(crate) disposition: &'static str,
    pub(crate) pr_url: Option<String>,
    pub(crate) head_sha: Option<String>,
    pub(crate) checks_state: Option<&'static str>,
}

impl Outcome {
    pub(super) fn parse(text: &str) -> Option<Self> {
        let mut parts = text
            .lines()
            .next_back()?
            .strip_prefix("Outcome: ")?
            .split(" · ");
        let disposition = match parts.next()? {
            "completed" => "completed",
            "blocked" => "blocked",
            "cancelled" => "cancelled",
            _ => return None,
        };
        let mut outcome = Self {
            disposition,
            pr_url: None,
            head_sha: None,
            checks_state: None,
        };
        if let Some(pr) = parts.next() {
            let url = pr.strip_prefix("PR ")?;
            let sha = parts.next()?.strip_prefix("head ")?;
            let checks = match parts.next()?.strip_prefix("checks ")? {
                "none" => "none",
                "red" => "red",
                "running" => "running",
                "green" => "green",
                _ => return None,
            };
            if parts.next().is_some()
                || sha.len() != 40
                || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !url.starts_with("https://")
                || !url.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return None;
            }
            let parsed = reqwest::Url::parse(url).ok()?;
            if !parsed.has_host()
                || parsed.path() == "/"
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return None;
            }
            outcome.pr_url = Some(url.to_owned());
            outcome.head_sha = Some(sha.to_owned());
            outcome.checks_state = Some(checks);
        }
        Some(outcome)
    }
}

pub(super) fn claude(record: &Value) -> Option<(u32, Outcome)> {
    let message = record.get("message")?;
    if field(record, "type") != Some("assistant")
        || field(message, "role") != Some("assistant")
        || field(message, "stop_reason") != Some("end_turn")
        || record.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let (block, text) = match message.get("content")? {
        Value::String(text) => (0, text.as_str()),
        Value::Array(blocks) => {
            let (block, value) = blocks
                .iter()
                .enumerate()
                .rev()
                .find(|(_, value)| field(value, "type") == Some("text"))?;
            (u32::try_from(block).ok()?, field(value, "text")?)
        }
        _ => return None,
    };
    Some((block, Outcome::parse(text)?))
}
