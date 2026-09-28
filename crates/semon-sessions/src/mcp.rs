//! `semon mcp`: the agent read surface as a Model Context Protocol server on
//! stdio. JSON-RPC 2.0, one message per line on stdin and stdout; it answers
//! `initialize`, `ping`, `tools/list` and `tools/call`, with the tools and
//! input schemas of [`query_tools`]. A tool's result is text content holding
//! its JSON, and a tool's failure is a result with `isError`. Malformed
//! messages get JSON-RPC errors.
//!
//! No listener and no network: it reads stdin until it closes. Nothing goes
//! to stdout but protocol messages.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::query::{Query, query_tools};

/// The protocol versions this server speaks, newest first. A client that
/// asks for one of these gets it; any other gets the newest, as the protocol
/// says, and decides whether to go on.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Semon reads the local Claude Code and Codex session logs, read-only. \
    list_sessions and stalls give facts about sessions (state, last log line, process, parent and children); \
    get_session adds its turns and handoffs; read_transcript and find read what the sessions said. \
    Times are epoch milliseconds. A field Semon can't know exactly is null. \
    The tools read a window of recent logs (30 days unless the server was started with --since or --all); \
    every answer names its window_start, and a since before it is the error outside_window.";

/// Serves MCP over `input` and `output` until `input` ends.
pub fn serve_mcp(
    query: &mut Query,
    mut input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if let Some(reply) = handle(query, &line) {
            serde_json::to_writer(&mut output, &reply)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The answer to one line, or `None` for a notification, a response or a
/// blank line.
fn handle(query: &mut Query, line: &[u8]) -> Option<Value> {
    if line.trim_ascii().is_empty() {
        return None;
    }
    let Ok(message) = serde_json::from_slice::<Value>(line) else {
        return Some(error(Value::Null, PARSE_ERROR, "Parse error"));
    };
    // Batches aren't part of the protocol any more; neither is anything
    // that isn't an object.
    let Some(object) = message.as_object() else {
        return Some(error(Value::Null, INVALID_REQUEST, "Invalid Request"));
    };
    let id = object.get("id").cloned();
    let id_ok = matches!(id, None | Some(Value::String(_) | Value::Number(_)));
    let reply_id = if id_ok {
        id.clone().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !id_ok {
        return Some(error(reply_id, INVALID_REQUEST, "Invalid Request"));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response to a request of ours: we send none, so nothing to do.
        if id.is_some() && (object.contains_key("result") || object.contains_key("error")) {
            return None;
        }
        return Some(error(reply_id, INVALID_REQUEST, "Invalid Request"));
    };
    // Notifications (`notifications/initialized`, `…/cancelled`) need no answer.
    let id = id?;
    let params = match object.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(params @ Value::Object(_)) => params.clone(),
        Some(_) => return Some(error(id, INVALID_PARAMS, "params must be an object")),
    };
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => call(query, &params),
        _ => Err((METHOD_NOT_FOUND, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => error(id, code, &message),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .and_then(|asked| PROTOCOL_VERSIONS.into_iter().find(|known| *known == asked))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "semon", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

fn tools() -> Vec<Value> {
    query_tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
                "annotations": {"readOnlyHint": true, "openWorldHint": false},
            })
        })
        .collect()
}

fn call(query: &mut Query, params: &Value) -> Result<Value, (i64, String)> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err((INVALID_PARAMS, "tools/call needs a tool name".into()));
    };
    if !query_tools().iter().any(|tool| tool.name == name) {
        return Err((INVALID_PARAMS, format!("Unknown tool: {name}")));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let (value, failed) = match query.call(name, &arguments) {
        Ok(value) => (value, false),
        Err(error) => (error.to_json(), true),
    };
    Ok(json!({
        "content": [{"type": "text", "text": value.to_string()}],
        "isError": failed,
    }))
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, time::Duration};

    use super::*;
    use crate::Options;

    fn empty_query(label: &str) -> (Query, PathBuf) {
        let root = env::temp_dir().join(format!("semon-mcp-{label}-{}", std::process::id()));
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::write(root.join("proc/locks"), "").unwrap();
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
            codex_home: root.join("codex"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        (Query::new(options), root)
    }

    fn ask(query: &mut Query, line: &str) -> Option<Value> {
        handle(query, line.as_bytes())
    }

    #[test]
    fn requests_notifications_and_malformed_lines() {
        let (mut query, root) = empty_query("unit");
        let init = ask(
            &mut query,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        )
        .unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "semon");
        let newest = ask(
            &mut query,
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        )
        .unwrap();
        assert_eq!(newest["id"], "a");
        assert_eq!(newest["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert_eq!(
            ask(
                &mut query,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            ),
            None
        );
        assert_eq!(ask(&mut query, "  \n"), None);
        assert_eq!(
            ask(&mut query, r#"{"jsonrpc":"2.0","id":7,"result":{}}"#),
            None,
            "a response needs no answer"
        );
        let mut code = |line: &str| {
            let reply = handle(&mut query, line.as_bytes()).unwrap();
            (reply["id"].clone(), reply["error"]["code"].as_i64())
        };
        assert_eq!(code("{nope"), (Value::Null, Some(PARSE_ERROR)));
        assert_eq!(code("[]"), (Value::Null, Some(INVALID_REQUEST)));
        assert_eq!(code("42"), (Value::Null, Some(INVALID_REQUEST)));
        assert_eq!(
            code(r#"{"jsonrpc":"1.0","id":2,"method":"ping"}"#),
            (json!(2), Some(INVALID_REQUEST))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":{"x":1},"method":"ping"}"#),
            (Value::Null, Some(INVALID_REQUEST))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":3}"#),
            (json!(3), Some(INVALID_REQUEST))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#),
            (json!(4), Some(METHOD_NOT_FOUND))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#),
            (json!(5), Some(INVALID_PARAMS))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":[1]}"#),
            (json!(6), Some(INVALID_PARAMS))
        );
        let ping = ask(&mut query, r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#).unwrap();
        assert_eq!(ping, json!({"jsonrpc": "2.0", "id": 8, "result": {}}));

        // An empty home: empty results, and an unknown id is a tool error.
        let listed = ask(
            &mut query,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#,
        )
        .unwrap();
        assert_eq!(listed["result"]["isError"], false);
        let text: Value =
            serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            (&text["sessions"], &text["total"], &text["truncated"]),
            (&json!([]), &json!(0), &json!(false))
        );
        let missing = ask(
            &mut query,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"get_session","arguments":{"id":"nope"}}}"#,
        )
        .unwrap();
        assert_eq!(missing["result"]["isError"], true);
        let text: Value =
            serde_json::from_str(missing["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(text["error"]["code"], "unknown_session");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn serve_answers_each_line_in_order_until_input_ends() {
        let (mut query, root) = empty_query("serve");
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            "not json\n",
        );
        let mut output = Vec::new();
        serve_mcp(&mut query, input.as_bytes(), &mut output).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[1]["id"], 2);
        let names: Vec<&str> = replies[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_sessions",
                "get_session",
                "read_transcript",
                "find",
                "stalls"
            ]
        );
        assert_eq!(replies[2]["error"]["code"], PARSE_ERROR);
        let _ = fs::remove_dir_all(root);
    }
}
