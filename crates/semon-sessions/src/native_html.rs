//! Shared native/no-JavaScript presentation. Hosts retain authentication, source
//! admission and read lifetimes; HTML consumes the same bounded native replies.
use serde_json::Value;

pub fn path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("writing a String");
        }
    }
    encoded
}

pub fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", path_segment(name), path_segment(value)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn link(path: &str, pairs: &[(&str, &str)], label: &str) -> String {
    format!(
        "<a href=\"{}?{}\">{}</a>",
        crate::shell::escape(path),
        crate::shell::escape(&query(pairs)),
        crate::shell::escape(label)
    )
}

fn paragraphs(text: &str) -> String {
    format!("<p class=\"hub-name\">{}</p>", crate::shell::escape(text))
}

/// A selected source overview; authentication and source admission stay with
/// the host. The bounded recent rows link to the same native history reader.
pub fn machine_overview(body: &mut String, value: &Value, key: &str) -> bool {
    let Some(machines) = value["machines"].as_array() else {
        return false;
    };
    let [machine] = machines.as_slice() else {
        return false;
    };
    if machine["key"] != key {
        return false;
    }
    let label = machine["label"].as_str().unwrap_or(key);
    body.push_str(&format!(
        "<h1>{}</h1><p>{} · {} working · {} sessions</p>",
        crate::shell::escape(label),
        if machine["up"] == true {
            "Up"
        } else {
            "Not responding"
        },
        machine["working"].as_u64().unwrap_or(0),
        machine["sessions"].as_u64().unwrap_or(0)
    ));
    overview_sessions(body, &value["recent"]);
    body.push_str(&link(
        "/sessions",
        &[("machine", key), ("native", "1")],
        "All sessions",
    ));
    true
}

pub fn overview_sessions(body: &mut String, rows: &Value) {
    body.push_str("<ul>");
    for row in rows.as_array().into_iter().flatten() {
        let (Some(machine), Some(key), Some(harness)) = (
            row["source_key"].as_str(),
            row["catalog_key"].as_str(),
            row["session"]["harness"].as_str(),
        ) else {
            continue;
        };
        let path = format!("/s/{}/{}", path_segment(harness), path_segment(key));
        body.push_str(&format!(
            "<li>{} <span>{}</span></li>",
            link(
                &path,
                &[("machine", machine), ("native", "1")],
                row["session"]["name"].as_str().unwrap_or(key)
            ),
            crate::shell::escape(row["session"]["state"].as_str().unwrap_or("off"))
        ));
    }
    body.push_str("</ul>");
}

fn render_family(body: &mut String, session: &Value, machine: &str) {
    let family = &session["details"]["family"];
    let relatives: Vec<_> = ["ancestors", "runs"]
        .into_iter()
        .flat_map(|field| family[field].as_array().into_iter().flatten())
        .collect();
    if relatives.is_empty() && family["state"] != "incomplete" {
        return;
    }
    body.push_str("<details class=\"sh-disclosure\"><summary>Related sessions</summary><ul>");
    for relative in relatives {
        let (Some(key), Some(harness)) = (relative["key"].as_str(), relative["harness"].as_str())
        else {
            continue;
        };
        let path = format!("/s/{}/{}", path_segment(harness), path_segment(key));
        body.push_str(&format!(
            "<li>{}</li>",
            link(
                &path,
                &[("machine", machine), ("native", "1")],
                relative["name"].as_str().unwrap_or(key)
            )
        ));
    }
    body.push_str("</ul>");
    if family["state"] == "incomplete" {
        body.push_str("<p>Some recorded relationships are unavailable.</p>");
    }
    body.push_str("</details>");
}

fn render_handoff(body: &mut String, entry: &Value, context: &Value, machine: &str) {
    let Some(edge) = context["handoffs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|edge| edge["id"] == entry["id"])
    else {
        body.push_str("<p>Recorded handoff context is unavailable.</p>");
        return;
    };
    body.push_str("<dl>");
    for (field, label) in [("from", "From"), ("to", "To")] {
        let Some(key) = edge[field].as_str() else {
            body.push_str(&format!("<dt>{label}</dt><dd>Unresolved destination</dd>"));
            continue;
        };
        let session = &context["sessions"][key];
        let destination = match session["harness"].as_str() {
            Some(harness) => {
                let path = if field == "to"
                    && let Some(turn) = edge["target_turn"].as_str()
                {
                    format!(
                        "/trace/{}/{}/{}",
                        path_segment(harness),
                        path_segment(key),
                        path_segment(turn)
                    )
                } else {
                    format!("/s/{}/{}", path_segment(harness), path_segment(key))
                };
                link(
                    &path,
                    &[("machine", machine), ("native", "1")],
                    session["name"].as_str().unwrap_or(key),
                )
            }
            None if key == "you" => "You".into(),
            None => crate::shell::escape(key),
        };
        body.push_str(&format!("<dt>{label}</dt><dd>{destination}</dd>"));
    }
    if let Some(status) = edge["status"].as_str() {
        body.push_str(&format!(
            "<dt>Status</dt><dd>{}</dd>",
            crate::shell::escape(status)
        ));
    }
    body.push_str("</dl>");
}

/// Render an already authorized selected Transcript or Entry reply. Generation
/// anchors, attachment URLs, collapsed Thinking and tool expansions stay shared.
pub fn transcript(value: &Value, route: &str, machine: &str, harness: &str, key: &str) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<h1>{}</h1>",
        crate::shell::escape(value["session"]["name"].as_str().unwrap_or(key))
    ));
    body.push_str(&link(
        "/sessions",
        &[("machine", machine), ("native", "1")],
        "Back to sessions",
    ));
    render_family(&mut body, &value["session"], machine);
    body.push_str("<section aria-label=\"Session transcript\">");
    if let Some(text) = value["text"].as_str() {
        body.push_str(&paragraphs(text));
        if let Some(next) = value["field"]["next"].as_u64() {
            body.push_str(&link(
                route,
                &[
                    ("machine", machine),
                    ("native", "1"),
                    ("after", &value["slot"].to_string()),
                    (
                        "generation",
                        value["projection"]["generation"].as_str().unwrap_or(""),
                    ),
                    ("field_chunk", &next.to_string()),
                ],
                "Continue full record",
            ));
        }
    }
    for entry in value["entries"].as_array().into_iter().flatten() {
        let kind = entry["k"].as_str().unwrap_or("record");
        let text = entry["text"]
            .as_str()
            .or_else(|| entry["brief"].as_str())
            .unwrap_or("");
        if matches!(kind, "think" | "thinking" | "tool") {
            let label = if kind == "tool" {
                entry["name"].as_str().unwrap_or("Tool call")
            } else {
                "Thinking"
            };
            body.push_str(&format!(
                "<details class=\"sh-disclosure\"><summary>{}</summary>{}",
                crate::shell::escape(label),
                paragraphs(text)
            ));
            for field in [
                "arg",
                "in",
                "out",
                "scriptText",
                "input",
                "output",
                "result",
            ] {
                if let Some(text) = entry[field].as_str() {
                    body.push_str(&paragraphs(text));
                }
            }
            if entry["field"].is_object() && entry["field"]["complete"] == false {
                body.push_str(&link(
                    route,
                    &[
                        ("machine", machine),
                        ("native", "1"),
                        ("after", &entry["slot"].to_string()),
                        (
                            "generation",
                            value["projection"]["generation"].as_str().unwrap_or(""),
                        ),
                        ("field_chunk", "0"),
                    ],
                    "Read full record",
                ));
            }
            for (field, part, label) in [
                ("in", "script", "Read full input"),
                ("out", "output", "Read full output"),
            ] {
                if entry["more"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value == field))
                {
                    body.push_str(&link(
                        route,
                        &[
                            ("machine", machine),
                            ("native", "1"),
                            ("after", &entry["slot"].to_string()),
                            (
                                "generation",
                                value["projection"]["generation"].as_str().unwrap_or(""),
                            ),
                            ("part", part),
                        ],
                        label,
                    ));
                }
            }
            body.push_str("</details>");
        } else {
            body.push_str(&format!(
                "<article class=\"msg assistant\"><h2>{}</h2><div class=\"body\">{}</div>",
                match kind {
                    "a" => "Assistant",
                    "u" => "User",
                    "h" => "Handoff",
                    "harness" => "Harness context",
                    "end" => "Session status",
                    _ => "Session event",
                },
                paragraphs(text)
            ));
            if entry["field"].is_object() && entry["field"]["complete"] == false {
                body.push_str(&link(
                    route,
                    &[
                        ("machine", machine),
                        ("native", "1"),
                        ("after", &entry["slot"].to_string()),
                        (
                            "generation",
                            value["projection"]["generation"].as_str().unwrap_or(""),
                        ),
                        ("field_chunk", "0"),
                    ],
                    "Read full record",
                ));
            }
            if kind == "h" {
                render_handoff(&mut body, entry, &value["relationship_context"], machine);
            }
            for (index, _) in entry["img"]
                .as_array()
                .into_iter()
                .flatten()
                .take(64)
                .enumerate()
            {
                let image_query = query(&[
                    ("machine", machine),
                    ("sid", key),
                    ("scope", "retained_history"),
                    ("after", &entry["slot"].to_string()),
                    ("limit", "1"),
                    (
                        "generation",
                        value["projection"]["generation"].as_str().unwrap_or(""),
                    ),
                    ("part", "attachment"),
                    ("image", &index.to_string()),
                ]);
                body.push_str(&format!("<img class=\"attach-img\" src=\"/api/session-entry?{}\" alt=\"Recorded image attachment {}\" loading=\"lazy\" decoding=\"async\">", crate::shell::escape(&image_query), index + 1));
            }
            body.push_str("</article>");
        }
    }
    body.push_str("</section><nav aria-label=\"Transcript pages\">");
    let first = value["range"]["first"].as_u64().unwrap_or(0);
    let path = format!("/s/{}/{}", path_segment(harness), path_segment(key));
    if first > 0 {
        body.push_str(&link(
            &path,
            &[
                ("machine", machine),
                ("native", "1"),
                ("after", &first.saturating_sub(60).to_string()),
                (
                    "generation",
                    value["projection"]["generation"].as_str().unwrap_or(""),
                ),
            ],
            "Earlier records",
        ));
    }
    if let Some(next) = value["range"]["next"].as_u64() {
        body.push_str(&link(
            &path,
            &[
                ("machine", machine),
                ("native", "1"),
                ("after", &next.to_string()),
                (
                    "generation",
                    value["projection"]["generation"].as_str().unwrap_or(""),
                ),
            ],
            "Later records",
        ));
    }
    body.push_str("</nav>");
    if value["freshness"]["state"] == "incomplete" {
        body.push_str("<p>Some selected history is unavailable. Reload to retry.</p>");
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recorded_relationship_links_keep_source_and_turn_without_html_injection() {
        let mut body = String::new();
        render_handoff(
            &mut body,
            &json!({"k":"h","id":"edge"}),
            &json!({"handoffs":[{"id":"edge","from":"you","to":"session/key","target_turn":"turn+one","status":"wait"}],
                "sessions":{"session/key":{"harness":"claude","name":"<script>name</script>"}}}),
            "source&key",
        );
        assert!(
            body.contains(
                "/trace/claude/session%2Fkey/turn%2Bone?machine=source%26key&amp;native=1"
            ),
            "{body}"
        );
        assert!(body.contains("&lt;script&gt;name&lt;/script&gt;"), "{body}");
        assert!(!body.contains("<script>"));
        assert!(body.contains("<dd>You</dd>"));
        assert!(body.contains("<dd>wait</dd>"));
    }
}
