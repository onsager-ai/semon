//! Standalone native HTML consumes the same focused read routes as the browser.
//! The local HTTP owner authenticates before handing a request to ViewerCore.
use crate::{
    ViewerReply,
    native_html::{link, path_segment, query},
};
use serde_json::Value;
use std::collections::BTreeMap;

fn response(status: u16, body: &str) -> ViewerReply {
    let navigation = crate::shell::NAV
        .iter()
        .map(|item| link(item.path, &[("native", "1")], item.label))
        .collect::<Vec<_>>()
        .join(" ");
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Semon Sessions</title><link rel=\"stylesheet\" href=\"/viewer.css\"><link rel=\"stylesheet\" href=\"/shell.css\"></head><body><nav aria-label=\"Pages\">{navigation}</nav><main id=\"page\" class=\"page\">{body}</main></body></html>"
    );
    ViewerReply {
        status,
        content_type: "text/html; charset=utf-8",
        body: html.into_bytes(),
        etag: None,
    }
}

pub(crate) fn reply(
    path: &str,
    arguments: &str,
    mut read: impl FnMut(&str, &str) -> ViewerReply,
) -> ViewerReply {
    if arguments.len() > 8192 {
        return response(400, "<p>Native read arguments exceed their size limit.</p>");
    }
    let mut args = BTreeMap::new();
    for pair in arguments.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(name), Some(value)) =
            (crate::viewer::decoded(name), crate::viewer::decoded(value))
        else {
            return response(400, "<p>Invalid native read argument.</p>");
        };
        if args.insert(name, value).is_some() {
            return response(400, "<p>Duplicate native read argument.</p>");
        }
    }
    // Bootstrap secrets never enter generated links or the HTML document.
    args.remove("t");
    let get = |name: &str| args.get(name).map(String::as_str);
    let decoded = |reply: ViewerReply| {
        (
            reply.status,
            serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null),
        )
    };
    let mut body = String::new();
    let mut status = 200;
    if let Some(encoded) = path.strip_prefix("/machines/") {
        let Some(key) = crate::viewer::decoded(encoded) else {
            return response(400, "<p>Invalid machine address.</p>");
        };
        if get("machine").is_some_and(|machine| machine != key) {
            return response(400, "<p>Machine address and source differ.</p>");
        }
        let (code, value) = decoded(read("/api/viewer/home", &query(&[("source", &key)])));
        status = code;
        if code == 200 && !crate::native_html::machine_overview(&mut body, &value, &key) {
            return response(
                409,
                "<p>Machine observation changed. Reload to refresh.</p>",
            );
        }
    } else if matches!(path, "/" | "/analytics" | "/timeline") {
        let home = path == "/";
        let names: &[&str] = if home {
            &["inbox_offset", "working_offset", "answered_offset"]
        } else {
            &["range", "repo", "machine", "harness", "model"]
        };
        let mut pairs: Vec<_> = names
            .iter()
            .filter_map(|name| get(name).map(|value| (*name, value)))
            .collect();
        if !home && get("range").is_none() {
            pairs.push(("range", "24h"));
        }
        let (code, value) = decoded(read(
            if home {
                "/api/viewer/home"
            } else {
                "/api/viewer/analytics"
            },
            &query(&pairs),
        ));
        status = code;
        if code == 200 {
            body.push_str(if home {
                "<h1>Home</h1>"
            } else {
                "<h1>Analytics</h1>"
            });
            if home {
                for (field, title) in [
                    ("inbox", "Needs You"),
                    ("working", "Working Now"),
                    ("answered", "Answered"),
                ] {
                    body.push_str(&format!("<section><h2>{title}</h2>"));
                    crate::native_html::overview_sessions(&mut body, &value[field]);
                    if let Some(offset) = value["next"][field].as_u64() {
                        let name = format!("{field}_offset");
                        let offset = offset.to_string();
                        body.push_str(&link(
                            "/",
                            &[("native", "1"), (&name, &offset)],
                            "More sessions",
                        ));
                    }
                    body.push_str("</section>");
                }
            } else {
                body.push_str("<nav aria-label=\"Usage range\">");
                for range in ["24h", "7d", "30d"] {
                    body.push_str(&link(
                        "/analytics",
                        &[("native", "1"), ("range", range)],
                        range,
                    ));
                }
                body.push_str("</nav><dl>");
                for (field, label) in [
                    ("started", "Sessions started"),
                    ("turns", "Turns"),
                    ("tools", "Tool calls"),
                    ("errors", "Errors"),
                    ("agent_ms", "Agent time (ms)"),
                    ("wait_ms", "Wait time (ms)"),
                ] {
                    if let Some(number) = value["current"][field].as_i64() {
                        body.push_str(&format!("<dt>{label}</dt><dd>{number}</dd>"));
                    }
                }
                body.push_str("</dl>");
            }
            body.push_str("<h2>Recent sessions</h2>");
            crate::native_html::overview_sessions(&mut body, &value["recent"]);
        }
    } else {
        let mut source_pairs = vec![("limit", "100")];
        if let Some(cursor) = get("source_cursor") {
            source_pairs.push(("cursor", cursor));
        }
        let (code, sources) = decoded(read("/api/session-sources", &query(&source_pairs)));
        let next_source = sources["next_cursor"].as_str().map(str::to_owned);
        let sources: Vec<_> = sources["items"].as_array().into_iter().flatten().collect();
        let machine = get("machine").or_else(|| {
            (sources.len() == 1)
                .then(|| sources[0]["source_key"].as_str())
                .flatten()
        });
        if code != 200 {
            status = code;
        } else if machine.is_none() || path == "/machines" {
            body.push_str("<h1>Session sources</h1><ul>");
            for source in sources {
                let Some(key) = source["source_key"].as_str() else {
                    continue;
                };
                let destination = if path == "/machines" {
                    "/sessions"
                } else {
                    path
                };
                let mut pairs: Vec<_> = args
                    .iter()
                    .filter(|(name, _)| !matches!(name.as_str(), "native" | "machine" | "compat"))
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();
                pairs.extend([("machine", key), ("native", "1")]);
                body.push_str(&format!(
                    "<li>{}</li>",
                    link(destination, &pairs, source["label"].as_str().unwrap_or(key))
                ));
            }
            body.push_str("</ul>");
            if let Some(cursor) = next_source {
                let mut pairs: Vec<_> = args
                    .iter()
                    .filter(|(name, _)| !matches!(name.as_str(), "native" | "source_cursor"))
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();
                pairs.extend([("native", "1"), ("source_cursor", &cursor)]);
                body.push_str(&link(path, &pairs, "More sources"));
            }
        } else if let Some(machine) = machine {
            if path == "/sessions" {
                let mut pairs = vec![
                    ("machine", machine),
                    ("scope", "retained_history"),
                    ("limit", "60"),
                ];
                for name in ["cursor", "harness", "repo", "model", "q"] {
                    if let Some(value) = get(name) {
                        pairs.push((name, value));
                    }
                }
                let (code, value) = decoded(read("/api/sessions", &query(&pairs)));
                status = code;
                if code == 200 {
                    body.push_str("<h1>Sessions</h1><ul aria-label=\"Sessions\">");
                    for item in value["items"].as_array().into_iter().flatten() {
                        let (Some(key), Some(harness)) =
                            (item["key"].as_str(), item["harness"].as_str())
                        else {
                            continue;
                        };
                        let route = format!("/s/{}/{}", path_segment(harness), path_segment(key));
                        body.push_str(&format!(
                            "<li>{}</li>",
                            link(
                                &route,
                                &[("machine", machine), ("native", "1")],
                                item["name"].as_str().unwrap_or(key)
                            )
                        ));
                    }
                    body.push_str("</ul>");
                    if let Some(cursor) = value["next_cursor"].as_str() {
                        let mut next: Vec<_> = pairs
                            .into_iter()
                            .filter(|(name, _)| !matches!(*name, "scope" | "limit" | "cursor"))
                            .collect();
                        next.extend([("native", "1"), ("cursor", cursor)]);
                        body.push_str(&link("/sessions", &next, "More sessions"));
                    }
                }
            } else {
                let parts = crate::viewer::page_parts(path).unwrap_or_default();
                match parts.as_slice() {
                    [kind, harness, key] | [kind, harness, key, _]
                        if matches!(kind.as_str(), "s" | "trace") =>
                    {
                        let (code, located) = decoded(read(
                            "/api/session-locate",
                            &query(&[("machine", machine), ("sid", key)]),
                        ));
                        let matches: Vec<_> = located["matches"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|item| item["harness"] == *harness)
                            .collect();
                        if code != 200 || matches.len() != 1 {
                            status = if code != 200 {
                                code
                            } else if located["unavailable"].as_u64().unwrap_or(0) > 0 {
                                503
                            } else {
                                404
                            };
                        } else {
                            let key = matches[0]["catalog_key"].as_str().unwrap_or(key);
                            let mut after = get("after").map(str::to_owned);
                            let mut generation = get("generation").map(str::to_owned);
                            if parts.len() == 4 && after.is_none() {
                                let (code, anchor) = decoded(read(
                                    "/api/session-anchor",
                                    &query(&[
                                        ("machine", machine),
                                        ("sid", key),
                                        ("turn", &parts[3]),
                                        ("scope", "retained_history"),
                                    ]),
                                ));
                                if code != 200
                                    || !anchor["slot"].is_number()
                                    || !anchor["generation"].is_string()
                                {
                                    status = if code == 200 { 404 } else { code };
                                } else {
                                    after = Some(anchor["slot"].to_string());
                                    generation = anchor["generation"].as_str().map(str::to_owned);
                                }
                            }
                            if status == 200 {
                                let mut pairs = vec![
                                    ("machine", machine),
                                    ("sid", key),
                                    ("scope", "retained_history"),
                                ];
                                if let Some(after) = after.as_deref() {
                                    pairs.push(("after", after));
                                }
                                if let Some(generation) = generation.as_deref() {
                                    pairs.push(("generation", generation));
                                }
                                let endpoint = if let Some(chunk) = get("field_chunk") {
                                    pairs.extend([("field_chunk", chunk), ("limit", "1")]);
                                    "/api/session-entry"
                                } else if let Some(part) = get("part") {
                                    pairs.extend([("part", part), ("limit", "1")]);
                                    "/api/session-entry"
                                } else {
                                    pairs.push(("limit", "60"));
                                    "/api/session-transcript"
                                };
                                let (code, value) = decoded(read(endpoint, &query(&pairs)));
                                status = code;
                                if code == 200 {
                                    body.push_str(&crate::native_html::transcript(
                                        &value, path, machine, harness, key,
                                    ));
                                }
                            }
                        }
                    }
                    [kind, _] if kind == "machines" => {
                        body.push_str("<h1>Machine sessions</h1>");
                        body.push_str(&link(
                            "/sessions",
                            &[("machine", machine), ("native", "1")],
                            "Browse this source",
                        ));
                    }
                    _ => status = 404,
                }
            }
        }
    }
    if status != 200 {
        body.push_str("<p>Selected native history is unavailable. Reload this page to retry.</p>");
    }
    response(status, &body)
}
