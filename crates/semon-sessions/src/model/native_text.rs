use serde_json::Value;

use crate::{attachments, events, field};

pub(crate) fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match field(part, "type") {
                Some("text") => field(part, "text").map(str::to_owned),
                Some("image") => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A prompt's text: a Claude user record, a queued command or a Codex user
/// message, with reminders removed. Images it attaches aren't text: the
/// transcript shows them apart ([`attachments::refs`]), so the harness's
/// `[Image #N]` placeholders for them, and Codex's `<image>` frames, go too.
pub(crate) fn prompt_text(record: &Value) -> Option<String> {
    if field(record, "type") == Some("user.message") {
        return record
            .get("data")
            .and_then(|data| field(data, "content"))
            .map(str::to_owned);
    }
    let images = attachments::parts(record).is_some_and(attachments::has_image);
    let placeholders = |text: String| {
        if images {
            attachments::strip_placeholders(&text)
        } else {
            text
        }
    };
    if let Some(attachment) = record.get("attachment") {
        let text = match attachment.get("prompt")? {
            Value::Array(parts) => parts
                .iter()
                .filter(|part| field(part, "type").is_none_or(|kind| kind == "text"))
                .filter_map(|part| field(part, "text"))
                .collect::<Vec<_>>()
                .join("\n"),
            other => content_text(other),
        };
        return Some(placeholders(events::clean_prompt(&text)));
    }
    if let Some(message) = record.get("message") {
        let text = match message.get("content")? {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|block| field(block, "type") == Some("text"))
                .filter_map(|block| field(block, "text"))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => return None,
        };
        return Some(placeholders(events::clean_prompt(&text)));
    }
    let payload = record.get("payload")?;
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| field(part, "text"))
        .filter(|text| !(images && attachments::is_image_tag(text)))
        .collect::<String>();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("Semon-Parent:") && !line.starts_with("Semon-Handoff:"))
        .collect();
    Some(placeholders(kept.join("\n").trim().to_owned()))
}

pub(super) fn block_of(record: &Value, block: usize) -> Option<&Value> {
    record
        .get("message")?
        .get("content")?
        .as_array()?
        .get(block)
}

pub(crate) fn assistant_text(record: &Value, block: usize) -> Option<String> {
    if field(record, "type") == Some("assistant.message") {
        return record
            .get("data")
            .and_then(|data| field(data, "content"))
            .map(str::to_owned);
    }
    if let Some(item) = block_of(record, block) {
        return field(item, "text").map(|text| text.trim().to_owned());
    }
    let payload = record.get("payload")?;
    Some(
        payload
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|part| field(part, "text"))
            .collect::<String>()
            .trim()
            .to_owned(),
    )
}

/// A tool call's input: a Claude `tool_use` block, or a Codex call's
/// arguments parsed as JSON.
pub(crate) fn tool_input(record: &Value, block: usize) -> Option<Value> {
    if let Some(item) = record.pointer("/payload/item")
        && matches!(field(item, "type"), Some("McpToolCall" | "DynamicToolCall"))
    {
        return item.get("arguments").cloned();
    }
    if field(record, "type") == Some("assistant.message") {
        return record
            .get("data")?
            .get("toolRequests")?
            .as_array()?
            .get(block)?
            .get("arguments")
            .cloned();
    }
    if let Some(item) = block_of(record, block) {
        return item.get("input").cloned();
    }
    let payload = record.get("payload")?;
    match payload.get("arguments").or_else(|| payload.get("input"))? {
        Value::String(text) => serde_json::from_str(text)
            .ok()
            .or_else(|| Some(Value::String(text.clone()))),
        other => Some(other.clone()),
    }
}

pub(crate) fn input_string(record: &Value, block: usize, keys: &[&str]) -> Option<String> {
    let input = tool_input(record, block)?;
    keys.iter()
        .find_map(|key| field(&input, key).map(str::to_owned))
}

pub(super) fn origin_of(record: &Value) -> Option<&Value> {
    record
        .get("origin")
        .filter(|origin| origin.is_object())
        .or_else(|| {
            record
                .get("attachment")
                .and_then(|attachment| attachment.get("origin"))
                .filter(|origin| origin.is_object())
        })
}

/// The body of a relay or hand-back: `origin.body`, else the tag's text.
/// With `from`, the `skip`th tag that agent sent (a record can batch
/// several).
pub(crate) fn received_body(
    record: &Value,
    tag: &str,
    from: Option<&str>,
    skip: usize,
) -> Option<String> {
    if let Some(body) = origin_of(record).and_then(|origin| field(origin, "body")) {
        return Some(body.trim().to_owned());
    }
    let text = if let Some(payload) = record.get("payload") {
        payload
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| field(part, "text"))
                    .collect::<String>()
            })
            .unwrap_or_default()
    } else {
        record
            .get("attachment")
            .and_then(|attachment| attachment.get("prompt"))
            .or_else(|| {
                record
                    .get("message")
                    .and_then(|message| message.get("content"))
            })
            .map(content_text)
            .unwrap_or_default()
    };
    if record.get("payload").is_some() {
        return Some(text.trim().to_owned());
    }
    let close = format!("</{tag}>");
    let (_, end) = events::tags(&text, tag)
        .into_iter()
        .filter(|(attributes, _)| {
            from.is_none()
                || events::attributes(attributes)
                    .get("from")
                    .map(String::as_str)
                    == from
        })
        .nth(skip)?;
    let rest = &text[end..];
    let body = rest.find(&close).map_or(rest, |close| &rest[..close]);
    Some(body.trim().to_owned())
}

/// The first line Claude Code puts before a parent's message to a running
/// subagent.
const COORDINATOR_PREAMBLE: &str = "The coordinator sent a message while you were working:";

/// The body of a parent's message as its subagent received it
/// (`origin.kind == "coordinator"`): the prompt without that preamble.
pub(super) fn coordinator_body(record: &Value) -> Option<String> {
    let text = prompt_text(record)?;
    let text = text.trim_start();
    let body = text
        .strip_prefix(COORDINATOR_PREAMBLE)
        .unwrap_or(text)
        .trim();
    Some(body.to_owned())
}

/// Whether a `SendMessage` result delivered the message to a subagent, and
/// to which: the pinned agent or the resumed one by id, or an empty string
/// for a send queued for an agent's next tool round that names no id (its
/// `to` names the agent). `None` for any other result.
pub(super) fn down_target(record: &Value, block: usize) -> Option<String> {
    let result = record
        .get("toolUseResult")
        .filter(|result| result.is_object());
    if let Some(id) = result.and_then(|result| {
        result
            .get("pin")
            .and_then(|pin| field(pin, "id"))
            .or_else(|| field(result, "resumedAgentId"))
    }) {
        return Some(id.to_owned());
    }
    let pinned = result.is_some_and(|result| result.get("pin").is_some());
    let text = result
        .and_then(|result| field(result, "message"))
        .map(str::to_owned)
        .or_else(|| tool_result_text(record, block))
        .unwrap_or_default();
    (pinned || text.contains("queued for delivery to")).then(String::new)
}

pub(crate) fn notification_result(record: &Value, id: &str, task: Option<&str>) -> Option<String> {
    let text = record
        .get("attachment")
        .and_then(|attachment| attachment.get("prompt"))
        .or_else(|| {
            record
                .get("message")
                .and_then(|message| message.get("content"))
        })
        .map(content_text)?;
    events::task_notification_result(&text, id, task)
}

pub(crate) fn tool_result_text(record: &Value, block: usize) -> Option<String> {
    block_of(record, block)
        .and_then(|item| item.get("content"))
        .map(content_text)
}
