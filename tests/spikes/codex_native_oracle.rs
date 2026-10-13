//! Semon-owned runner for the pinned Codex rollout-trace public reducer.
//!
//! CI copies this file into a disposable upstream checkout as an example. The
//! fixture is synthetic and no Codex provider or runtime is started.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::{ExecCommandBeginEvent, ExecCommandEndEvent};
use codex_rollout_trace::{
    ConversationItemKind, RawPayloadKind, RawTraceEventContext, RawTraceEventPayload, ToolCallKind,
    ToolCallRequester, ToolCallSummary, TraceWriter, replay_bundle,
};
use serde_json::{Value, json};

const UPSTREAM_COMMIT: &str = "a9b0815167c27ab13719dd12d13b63e02b70d221";
const MAX_FIXTURE_BYTES: u64 = 10 * 1024 * 1024;
const DIRECT_CALL_ID: &str = "start-26789";
// core/src/tools/context.rs response_text, not code_mode_result JSON.
const DIRECT_OUTPUT: &str =
    "Chunk ID: old\nWall time: 0.1000 seconds\nProcess running with session ID 26789\nOutput:\nold";
const CELL_HEADER: &str = "Script completed\nWall time 0.1 seconds\nOutput:\n";
const OUTER_CALL_ID: &str = "multi-output";
const OUTER_SOURCE: &str = r#"text(await tools.exec_command({cmd:"printf new"})); text(await tools.write_stdin({session_id:26789,chars:""}));"#;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let fixture_dir = PathBuf::from(
        args.next()
            .context("usage: semon_native_oracle <fixture-dir> <new-bundle-dir>")?,
    );
    let bundle_dir = PathBuf::from(
        args.next()
            .context("usage: semon_native_oracle <fixture-dir> <new-bundle-dir>")?,
    );
    ensure!(
        args.next().is_none(),
        "usage: semon_native_oracle <fixture-dir> <new-bundle-dir>"
    );
    ensure!(
        !bundle_dir.exists(),
        "bundle directory must be new and disposable: {}",
        bundle_dir.display()
    );

    let manifest: Value = serde_json::from_slice(
        &fs::read(fixture_dir.join("manifest.json")).context("read fixture manifest")?,
    )
    .context("parse fixture manifest")?;
    ensure!(
        manifest["source_origin"].as_str()
            == Some("source-shaped synthetic; not a runtime capture"),
        "fixture origin must remain explicitly synthetic"
    );
    ensure!(
        manifest["upstream_commit"].as_str() == Some(UPSTREAM_COMMIT),
        "fixture must name the exact reducer pin"
    );

    let legacy_rows = validate_rollout(&fixture_dir.join("rollout-legacy.jsonl"), false)?;
    let paginated_rows = validate_rollout(&fixture_dir.join("rollout-paginated.jsonl"), true)?;
    let trace_events = read_jsonl(&fixture_dir.join("trace-events.jsonl"))?;
    validate_fixture_pairing(&legacy_rows, &paginated_rows, &trace_events, &manifest)?;

    let bundle = &manifest["trace_bundle"];
    let trace_id = required_string(bundle, "trace_id")?;
    let rollout_id = required_string(bundle, "rollout_id")?;
    let root_thread_id = required_string(bundle, "root_thread_id")?;
    let writer = TraceWriter::create(
        &bundle_dir,
        trace_id.to_owned(),
        rollout_id.to_owned(),
        root_thread_id.to_owned(),
    )?;
    append_trace_events(&trace_events, &writer)?;
    drop(writer);

    let rollout = replay_bundle(&bundle_dir).context("replay synthetic trace bundle")?;
    validate_reduced_associations(&rollout)?;
    let actual = normalize_reducer_graph(&rollout)?;
    let expected = manifest["expected_upstream_reducer"].clone();
    ensure!(
        actual == expected,
        "pinned reducer graph differs from manifest\nexpected: {expected}\nactual: {actual}"
    );

    let terminal_ids = actual["terminal_sessions"]
        .as_array()
        .context("normalized terminal_sessions is not an array")?
        .iter()
        .map(|session| {
            session["terminal_id"]
                .as_str()
                .context("normalized terminal session has no id")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ensure!(
        terminal_ids == vec!["26789", "95583"],
        "the two synthetic terminal session IDs must remain distinct: {terminal_ids:?}"
    );

    println!(
        "matched Codex {UPSTREAM_COMMIT}: {} nested tool calls, {} terminal sessions",
        actual["nested_tool_call_ids"]
            .as_array()
            .map_or(0, Vec::len),
        terminal_ids.len()
    );
    Ok(())
}

fn validate_rollout(path: &Path, paginated: bool) -> anyhow::Result<Vec<Value>> {
    let rows = read_jsonl(path)?;
    ensure!(
        rows.first().and_then(|row| row["type"].as_str()) == Some("session_meta"),
        "{} must start with a session_meta record",
        path.display()
    );
    ensure!(
        rows.iter()
            .all(|row| { row.get("timestamp").is_none() && row.get("timestamp_ms").is_none() }),
        "{} must not add fabricated timestamps",
        path.display()
    );

    let completed_commands = rows
        .iter()
        .filter(|row| {
            row["type"].as_str() == Some("event_msg")
                && row["payload"]["type"].as_str() == Some("item_completed")
                && row["payload"]["item"]["type"].as_str() == Some("CommandExecution")
        })
        .count();
    if paginated {
        ensure!(
            completed_commands == 1,
            "the paginated carrier fixture must contain one completed CommandExecution"
        );
    } else {
        ensure!(
            completed_commands == 0,
            "Legacy persistence does not retain completed CommandExecution items"
        );
    }
    Ok(rows)
}

fn read_jsonl(path: &Path) -> anyhow::Result<Vec<Value>> {
    let size = fs::metadata(path)
        .with_context(|| format!("stat fixture {}", path.display()))?
        .len();
    ensure!(
        size <= MAX_FIXTURE_BYTES,
        "fixture exceeds the 10 MiB single-file bound: {}",
        path.display()
    );
    let file = File::open(path).with_context(|| format!("open fixture {}", path.display()))?;
    let mut rows = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read {} line {}", path.display(), index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        rows.push(
            serde_json::from_str(&line)
                .with_context(|| format!("parse {} line {}", path.display(), index + 1))?,
        );
    }
    ensure!(!rows.is_empty(), "fixture is empty: {}", path.display());
    Ok(rows)
}

fn validate_fixture_pairing(
    legacy_rows: &[Value],
    paginated_rows: &[Value],
    trace_events: &[Value],
    manifest: &Value,
) -> anyhow::Result<()> {
    let direct_call = response_item(legacy_rows, "function_call", DIRECT_CALL_ID)?;
    ensure!(
        direct_call["name"].as_str() == Some("exec_command"),
        "the prior start must be a direct model exec_command call"
    );
    let direct_arguments: Value = serde_json::from_str(
        direct_call["arguments"]
            .as_str()
            .context("direct start arguments are not a JSON string")?,
    )
    .context("parse direct start arguments")?;
    ensure!(
        direct_arguments["cmd"].as_str() == Some("printf old"),
        "direct start command differs from the paired runtime evidence"
    );

    let direct_output = response_item(legacy_rows, "function_call_output", DIRECT_CALL_ID)?;
    ensure!(
        direct_output["output"].as_str() == Some(DIRECT_OUTPUT),
        "direct model result must retain the pinned formatter text, never Code Mode JSON"
    );

    let outer_call = response_item(legacy_rows, "custom_tool_call", OUTER_CALL_ID)?;
    ensure!(
        outer_call["input"].as_str() == Some(OUTER_SOURCE),
        "outer-cell source differs from the paired runtime fixture"
    );
    let outer_output = response_item(legacy_rows, "custom_tool_call_output", OUTER_CALL_ID)?;
    let outer_parts = text_parts(&outer_output["output"])?;
    ensure!(
        outer_parts.len() == 3 && outer_parts[0] == CELL_HEADER,
        "mixed outer-cell output must be one success header and two JSON results"
    );
    let new_result: Value =
        serde_json::from_str(outer_parts[1]).context("parse new command output")?;
    let poll_result: Value =
        serde_json::from_str(outer_parts[2]).context("parse completed poll output")?;
    ensure!(
        new_result["session_id"].as_u64() == Some(95583)
            && poll_result.get("session_id").is_none()
            && poll_result["exit_code"].as_i64() == Some(0),
        "outer output must print the new 95583 result followed by a completed poll without a session id"
    );

    let paginated_command_item = paginated_rows
        .iter()
        .find(|row| {
            row["type"].as_str() == Some("event_msg")
                && row["payload"]["type"].as_str() == Some("item_completed")
                && row["payload"]["item"]["type"].as_str() == Some("CommandExecution")
        })
        .context("Paginated fixture has no completed CommandExecution")?;
    let paginated_item: TurnItem =
        serde_json::from_value(paginated_command_item["payload"]["item"].clone())
            .context("deserialize Paginated item through pinned public TurnItem")?;
    let TurnItem::CommandExecution(paginated_command) = paginated_item else {
        bail!("Paginated fixture item did not deserialize as CommandExecution")
    };
    ensure!(
        serde_json::to_value(&paginated_command.source)? == json!("unified_exec_startup"),
        "Paginated CommandExecution source must deserialize as the pinned public unified-exec startup variant"
    );

    let scenario = &manifest["paired_default_and_trace_scenario"];
    ensure!(
        required_string(scenario, "direct_start_call_id")? == DIRECT_CALL_ID
            && required_string(scenario, "outer_cell_call_id")? == OUTER_CALL_ID
            && required_string(scenario, "outer_cell_source")? == OUTER_SOURCE
            && scenario["synthetic_shell_environment"]["shell"].as_str() == Some("/bin/sh")
            && scenario["synthetic_shell_environment"]["login"].as_bool() == Some(false)
            && scenario["synthetic_shell_environment"]["direct_process_source"]
                == json!("unified_exec_startup")
            && scenario["synthetic_shell_environment"]["nested_process_source"]
                == json!("unified_exec_startup")
            && scenario["synthetic_shell_environment"]["direct_process_command"]
                == json!(["/bin/sh", "-c", "printf old"])
            && scenario["synthetic_shell_environment"]["nested_process_command"]
                == json!(["/bin/sh", "-c", "printf new"]),
        "manifest scenario does not match the paired default rollout fixture"
    );

    assert_inference_output_contains(trace_events, "inference-1", direct_call)?;
    assert_inference_input_contains(trace_events, "inference-2", direct_output)?;
    assert_inference_output_contains(trace_events, "inference-2", outer_call)?;
    assert_inference_input_contains(trace_events, "inference-3", outer_output)?;

    let code_cell = trace_event(
        trace_events,
        "code_cell_started",
        "model_visible_call_id",
        OUTER_CALL_ID,
    )?;
    ensure!(
        code_cell["payload"]["source_js"].as_str() == Some(OUTER_SOURCE),
        "trace CodeCellStarted source differs from the default custom_tool_call input"
    );

    let direct_tool = trace_events
        .iter()
        .find(|event| {
            event["payload"]["type"].as_str() == Some("tool_call_started")
                && event["payload"]["model_visible_call_id"].as_str() == Some(DIRECT_CALL_ID)
        })
        .context("trace fixture has no direct Model-requested start")?;
    ensure!(
        direct_tool["payload"]["requester"]["type"].as_str() == Some("model")
            && direct_tool["payload"]["kind"]["type"].as_str() == Some("exec_command"),
        "start-26789 must be a direct Model-requested exec_command"
    );
    let direct_invocation = payload_data_value(direct_tool, "invocation_payload")?;
    ensure!(
        direct_invocation["tool_name"].as_str() == Some("exec_command")
            && direct_invocation["payload"]["arguments"].as_str()
                == direct_call["arguments"].as_str(),
        "direct trace invocation differs from the default function_call"
    );
    let direct_dispatch_end = trace_event(
        trace_events,
        "tool_call_ended",
        "tool_call_id",
        DIRECT_CALL_ID,
    )?;
    let direct_dispatch_result = payload_data_value(direct_dispatch_end, "result_payload")?;
    ensure!(
        direct_dispatch_result["type"] == "direct_response"
            && direct_dispatch_result["response_item"] == *direct_output,
        "direct dispatch result must preserve its model-facing formatter output"
    );
    let direct_end = trace_event(
        trace_events,
        "tool_call_runtime_ended",
        "tool_call_id",
        DIRECT_CALL_ID,
    )?;
    let direct_runtime = payload_data_value(direct_end, "runtime_payload")?;
    ensure!(
        direct_runtime["process_id"].as_str() == Some("26789")
            && direct_runtime["call_id"].as_str() == Some(DIRECT_CALL_ID)
            && direct_runtime["source"].as_str() == Some("unified_exec_startup")
            && direct_runtime["cwd"].as_str() == Some("/fixture/project")
            && direct_runtime["command"] == json!(["/bin/sh", "-c", "printf old"])
            && direct_runtime["aggregated_output"].as_str() == Some("old")
            && direct_runtime["exit_code"].as_i64() == Some(0),
        "direct trace operation must start process 26789 with the paired command"
    );

    let nested_starts: Vec<_> = trace_events
        .iter()
        .filter(|event| {
            event["payload"]["type"].as_str() == Some("tool_call_started")
                && event["payload"]["requester"]["type"].as_str() == Some("code_cell")
        })
        .collect();
    let nested_ids = nested_starts
        .iter()
        .map(|event| {
            event["payload"]["tool_call_id"]
                .as_str()
                .context("nested trace tool has no ID")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ensure!(
        nested_ids == vec!["tool-new", "tool-poll"],
        "the outer cell must have only the new exec and old-session poll: {nested_ids:?}"
    );
    ensure!(
        nested_starts[0]["payload"]["kind"]["type"].as_str() == Some("exec_command")
            && nested_starts[0]["payload"]["summary"]["input_preview"].as_str()
                == Some("printf new")
            && nested_starts[1]["payload"]["kind"]["type"].as_str() == Some("write_stdin")
            && nested_starts[1]["payload"]["summary"]["input_preview"].as_str()
                == Some("session_id=26789"),
        "outer-cell nested runtime operations do not match its source"
    );

    let new_end = trace_event(
        trace_events,
        "tool_call_runtime_ended",
        "tool_call_id",
        "tool-new",
    )?;
    let new_runtime = payload_data_value(new_end, "runtime_payload")?;
    let new_invocation = payload_data_value(nested_starts[0], "invocation_payload")?;
    let new_arguments: Value = serde_json::from_str(
        new_invocation["payload"]["arguments"]
            .as_str()
            .context("new invocation arguments missing")?,
    )?;
    let new_dispatch_end =
        trace_event(trace_events, "tool_call_ended", "tool_call_id", "tool-new")?;
    let new_dispatch_result = payload_data_value(new_dispatch_end, "result_payload")?;
    ensure!(
        new_invocation["tool_name"] == "exec_command"
            && new_invocation["payload"]["type"] == "function"
            && new_arguments == json!({"cmd":"printf new"})
            && new_dispatch_result["type"] == "code_mode_response"
            && new_dispatch_result["value"] == new_result,
        "new command dispatch must match the outer source and printed Code Mode result"
    );
    ensure!(
        new_runtime["process_id"].as_str() == Some("95583")
            && new_runtime["source"].as_str() == Some("unified_exec_startup")
            && new_runtime["cwd"].as_str() == Some("/fixture/project")
            && new_runtime["command"] == json!(["/bin/sh", "-c", "printf new"])
            && new_runtime["aggregated_output"].as_str() == Some("new")
            && new_runtime["exit_code"].as_i64() == Some(0),
        "the outer-cell exec operation must create process 95583"
    );
    let new_process_id = new_result["session_id"]
        .as_u64()
        .context("printed new-process result has no process ID")?
        .to_string();
    ensure!(
        new_result["output"].as_str() == Some("new")
            && paginated_command.process_id.as_deref() == Some(new_process_id.as_str())
            && paginated_command.command
                == vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf new".to_owned()
                ]
            && paginated_command.aggregated_output.as_deref() == new_result["output"].as_str()
            && new_runtime["command"] == json!(paginated_command.command)
            && new_runtime["aggregated_output"].as_str()
                == paginated_command.aggregated_output.as_deref(),
        "Paginated CommandExecution process, command, and output must match the paired runtime and printed result"
    );
    let poll_invocation = payload_data_value(nested_starts[1], "invocation_payload")?;
    let poll_arguments: Value = serde_json::from_str(
        poll_invocation["payload"]["arguments"]
            .as_str()
            .context("poll invocation arguments missing")?,
    )?;
    ensure!(
        poll_invocation["tool_name"] == "write_stdin"
            && poll_invocation["payload"]["type"] == "function"
            && poll_arguments == json!({"session_id":26789,"chars":""}),
        "canonical poll invocation must name old process 26789"
    );
    let poll_end = trace_event(trace_events, "tool_call_ended", "tool_call_id", "tool-poll")?;
    let poll_dispatch_result = payload_data_value(poll_end, "result_payload")?;
    ensure!(
        poll_dispatch_result["type"] == "code_mode_response"
            && poll_dispatch_result["value"] == poll_result,
        "canonical poll result must match its printed Code Mode result"
    );
    ensure!(
        !trace_events.iter().any(|event| {
            matches!(
                event["payload"]["type"].as_str(),
                Some("tool_call_runtime_started" | "tool_call_runtime_ended")
            ) && event["payload"]["tool_call_id"] == "tool-poll"
        }),
        "empty write_stdin polls do not emit hypothetical exec runtime begin/end"
    );
    ensure!(
        !trace_events.iter().any(|event| {
            event["payload"]["type"].as_str() == Some("tool_call_started")
                && matches!(
                    event["payload"]["model_visible_call_id"].as_str(),
                    Some("start-95583" | "poll-mismatch")
                )
        }),
        "nested operations must not masquerade as separate model-visible calls"
    );
    Ok(())
}

fn response_item<'a>(
    rows: &'a [Value],
    item_type: &str,
    call_id: &str,
) -> anyhow::Result<&'a Value> {
    rows.iter()
        .find(|row| {
            row["type"].as_str() == Some("response_item")
                && row["payload"]["type"].as_str() == Some(item_type)
                && row["payload"]["call_id"].as_str() == Some(call_id)
        })
        .map(|row| &row["payload"])
        .with_context(|| format!("missing {item_type} call_id {call_id}"))
}

fn validate_reduced_associations(
    rollout: &codex_rollout_trace::RolloutTrace,
) -> anyhow::Result<()> {
    let cell = rollout
        .code_cells
        .values()
        .find(|cell| cell.model_visible_call_id == OUTER_CALL_ID)
        .context("reduced code cell does not reference the outer model-visible call")?;
    ensure!(
        cell.source_js == OUTER_SOURCE,
        "reduced code-cell source differs from the paired outer call"
    );

    let source_item = rollout
        .conversation_items
        .get(&cell.source_item_id)
        .context("reduced code-cell source_item_id does not resolve")?;
    ensure!(
        source_item.item_id == cell.source_item_id
            && matches!(&source_item.kind, ConversationItemKind::CustomToolCall)
            && source_item.call_id.as_deref() == Some(OUTER_CALL_ID),
        "reduced source_item_id must resolve to the exact outer custom_tool_call"
    );

    ensure!(
        cell.output_item_ids.len() == 1,
        "reduced code cell must link exactly one outer output item"
    );
    let output_item_id = &cell.output_item_ids[0];
    let output_item = rollout
        .conversation_items
        .get(output_item_id)
        .context("reduced code-cell output_item_id does not resolve")?;
    ensure!(
        output_item.item_id == *output_item_id
            && matches!(
                &output_item.kind,
                ConversationItemKind::CustomToolCallOutput
            )
            && output_item.call_id.as_deref() == Some(OUTER_CALL_ID),
        "reduced output_item_id must resolve to the exact outer custom_tool_call_output"
    );

    ensure!(
        cell.nested_tool_call_ids == vec!["tool-new".to_owned(), "tool-poll".to_owned()],
        "reduced code cell does not contain the expected nested calls"
    );
    for nested_tool_id in &cell.nested_tool_call_ids {
        let nested_tool = rollout.tool_calls.get(nested_tool_id).with_context(|| {
            format!("reduced nested tool call {nested_tool_id} does not resolve")
        })?;
        ensure!(
            matches!(
                &nested_tool.requester,
                ToolCallRequester::CodeCell { code_cell_id }
                    if code_cell_id == &cell.code_cell_id
            ),
            "nested tool call {nested_tool_id} requester does not reference the reduced code-cell ID"
        );
    }

    let direct_tool = rollout
        .tool_calls
        .values()
        .find(|tool| tool.model_visible_call_id.as_deref() == Some(DIRECT_CALL_ID))
        .context("reduced rollout has no direct tool for start-26789")?;
    ensure!(
        matches!(&direct_tool.requester, ToolCallRequester::Model),
        "start-26789 must remain a direct model-requested tool"
    );
    ensure!(
        direct_tool.model_visible_call_item_ids.len() == 1,
        "direct tool must link exactly one model-visible call item"
    );
    let direct_call_item_id = &direct_tool.model_visible_call_item_ids[0];
    let direct_call_item = rollout
        .conversation_items
        .get(direct_call_item_id)
        .context("direct tool call-item association does not resolve")?;
    ensure!(
        direct_call_item.item_id == *direct_call_item_id
            && matches!(&direct_call_item.kind, ConversationItemKind::FunctionCall)
            && direct_call_item.call_id.as_deref() == Some(DIRECT_CALL_ID),
        "direct tool call-item association must resolve to start-26789"
    );

    ensure!(
        direct_tool.model_visible_output_item_ids.len() == 1,
        "direct tool must link exactly one model-visible output item"
    );
    let direct_output_item_id = &direct_tool.model_visible_output_item_ids[0];
    let direct_output_item = rollout
        .conversation_items
        .get(direct_output_item_id)
        .context("direct tool output-item association does not resolve")?;
    ensure!(
        direct_output_item.item_id == *direct_output_item_id
            && matches!(
                &direct_output_item.kind,
                ConversationItemKind::FunctionCallOutput
            )
            && direct_output_item.call_id.as_deref() == Some(DIRECT_CALL_ID),
        "direct tool output-item association must resolve to start-26789"
    );
    Ok(())
}

fn text_parts(output: &Value) -> anyhow::Result<Vec<&str>> {
    output
        .as_array()
        .context("output is not an ordered text-part array")?
        .iter()
        .map(|part| part["text"].as_str().context("output part has no text"))
        .collect()
}

fn assert_inference_output_contains(
    events: &[Value],
    inference_id: &str,
    expected: &Value,
) -> anyhow::Result<()> {
    let inference = trace_event(
        events,
        "inference_completed",
        "inference_call_id",
        inference_id,
    )?;
    let items = payload_data_value(inference, "response_payload")?["output_items"]
        .as_array()
        .context("inference response has no output_items array")?;
    ensure!(
        items.iter().any(|item| item == expected),
        "trace inference {inference_id} output does not contain its paired rollout item"
    );
    Ok(())
}

fn assert_inference_input_contains(
    events: &[Value],
    inference_id: &str,
    expected: &Value,
) -> anyhow::Result<()> {
    let inference = trace_event(
        events,
        "inference_started",
        "inference_call_id",
        inference_id,
    )?;
    let items = payload_data_value(inference, "request_payload")?["input"]
        .as_array()
        .context("inference request has no input array")?;
    ensure!(
        items.iter().any(|item| item == expected),
        "trace inference {inference_id} input does not contain its paired rollout result"
    );
    Ok(())
}

fn trace_event<'a>(
    events: &'a [Value],
    event_type: &str,
    field: &str,
    value: &str,
) -> anyhow::Result<&'a Value> {
    events
        .iter()
        .find(|event| {
            event["payload"]["type"].as_str() == Some(event_type)
                && event["payload"][field].as_str() == Some(value)
        })
        .with_context(|| format!("missing trace event {event_type} with {field}={value}"))
}

fn payload_data_value<'a>(event: &'a Value, field: &str) -> anyhow::Result<&'a Value> {
    let value = event["payload_data"][field]
        .get("value")
        .context("raw payload has no value")?;
    ensure!(
        value.is_object(),
        "trace event payload_data.{field}.value is not an object"
    );
    Ok(value)
}

fn append_trace_events(records: &[Value], writer: &TraceWriter) -> anyhow::Result<()> {
    for (index, record) in records.iter().enumerate() {
        if matches!(
            record["payload"]["type"].as_str(),
            Some("tool_call_runtime_started" | "tool_call_runtime_ended")
        ) {
            let mut value = payload_data_value(record, "runtime_payload")?.clone();
            ensure!(
                value["call_id"] == record["payload"]["tool_call_id"],
                "native runtime envelope and payload must use the same emitted call_id"
            );
            ensure!(
                value["cwd"] == "/fixture/project",
                "trace cwd must be a native path"
            );
            // protocol_event.rs serializes PathUri as an inferred native path.
            // Normalize only this fixture's known cwd for public live-event decoding.
            value["cwd"] = json!("file:///fixture/project");
            if record["payload"]["type"] == "tool_call_runtime_started" {
                let _: ExecCommandBeginEvent = serde_json::from_value(value)
                    .context("decode required public exec begin fields")?;
            } else {
                let _: ExecCommandEndEvent = serde_json::from_value(value)
                    .context("decode required public exec end fields")?;
            }
        }
        // Check the real public model-item shape before the reducer sees it.
        let inference = match record["payload"]["type"].as_str() {
            Some("inference_started") => Some(("request_payload", "input")),
            Some("inference_completed") => Some(("response_payload", "output_items")),
            _ => None,
        };
        if let Some((field, items)) = inference {
            for item in payload_data_value(record, field)?[items]
                .as_array()
                .context("inference payload has no model-item array")?
            {
                let _: ResponseItem = serde_json::from_value(item.clone())
                    .context("deserialize inference model item through pinned protocol")?;
            }
        }
        let context_value = record.get("context").cloned().unwrap_or(Value::Null);
        let context = RawTraceEventContext {
            thread_id: optional_string(&context_value, "thread_id")?,
            codex_turn_id: optional_string(&context_value, "codex_turn_id")?,
        };
        let mut payload = record
            .get("payload")
            .cloned()
            .context("trace event is missing its upstream payload")?;

        if let Some(payload_data) = record.get("payload_data") {
            let payload_data = payload_data
                .as_object()
                .context("payload_data must be an object keyed by upstream payload field")?;
            let payload_fields = payload
                .as_object_mut()
                .context("upstream event payload must be an object")?;
            for (field, data) in payload_data {
                let kind: RawPayloadKind = serde_json::from_value(
                    data.get("kind")
                        .cloned()
                        .with_context(|| format!("{field} payload is missing kind"))?,
                )
                .with_context(|| format!("decode {field} payload kind"))?;
                let value = data
                    .get("value")
                    .with_context(|| format!("{field} payload is missing value"))?;
                let raw_payload = writer
                    .write_json_payload(kind, value)
                    .with_context(|| format!("write raw payload for {field}"))?;
                payload_fields.insert(field.clone(), serde_json::to_value(raw_payload)?);
            }
        }

        hydrate_tool_call_started(&mut payload)?;
        let payload: RawTraceEventPayload =
            serde_json::from_value(payload).context("decode upstream raw trace event")?;
        writer
            .append_with_context(context, payload)
            .with_context(|| format!("append trace fixture line {}", index + 1))?;
    }
    ensure!(!records.is_empty(), "trace fixture is empty");
    Ok(())
}

fn hydrate_tool_call_started(payload: &mut Value) -> anyhow::Result<()> {
    if payload["type"].as_str() != Some("tool_call_started") {
        return Ok(());
    }
    let fields = payload
        .as_object_mut()
        .context("tool_call_started payload must be an object")?;
    let kind = match fields.get("kind").and_then(|value| value["type"].as_str()) {
        Some("exec_command") => ToolCallKind::ExecCommand,
        Some("write_stdin") => ToolCallKind::WriteStdin,
        Some(other) => bail!("unsupported synthetic tool kind {other}"),
        None => bail!("tool_call_started is missing its kind"),
    };
    fields.insert("kind".to_string(), serde_json::to_value(kind)?);

    let summary_data = fields
        .get("summary")
        .cloned()
        .context("tool_call_started is missing its summary")?;
    let summary = ToolCallSummary::Generic {
        label: required_string(&summary_data, "label")?.to_owned(),
        input_preview: optional_string(&summary_data, "input_preview")?,
        output_preview: optional_string(&summary_data, "output_preview")?,
    };
    fields.insert("summary".to_string(), serde_json::to_value(summary)?);
    Ok(())
}

fn normalize_reducer_graph(rollout: &codex_rollout_trace::RolloutTrace) -> anyhow::Result<Value> {
    ensure!(
        rollout.code_cells.len() == 1,
        "expected exactly one reduced code cell"
    );
    let cell = rollout
        .code_cells
        .values()
        .next()
        .context("reducer returned no code cell")?;

    let mut nested_tool_edges = Vec::new();
    for tool_call_id in &cell.nested_tool_call_ids {
        let tool_call = rollout
            .tool_calls
            .get(tool_call_id)
            .with_context(|| format!("nested tool call {tool_call_id} is missing"))?;
        let operation_id = tool_call.terminal_operation_id.as_ref().with_context(|| {
            format!("nested tool call {tool_call_id} has no terminal operation")
        })?;
        let operation = rollout
            .terminal_operations
            .get(operation_id)
            .with_context(|| format!("terminal operation {operation_id} is missing"))?;
        let terminal_id = operation
            .terminal_id
            .as_deref()
            .with_context(|| format!("terminal operation {operation_id} has no session id"))?;
        nested_tool_edges.push(json!({
            "tool_call_id": tool_call_id,
            "terminal_operation_id": operation_id,
            "terminal_id": terminal_id
        }));
    }

    let mut terminal_sessions = rollout
        .terminal_sessions
        .iter()
        .map(|(terminal_id, session)| {
            json!({
                "terminal_id": terminal_id,
                "operation_ids": session.operation_ids.clone()
            })
        })
        .collect::<Vec<_>>();
    terminal_sessions.sort_by(|left, right| {
        left["terminal_id"]
            .as_str()
            .cmp(&right["terminal_id"].as_str())
    });

    Ok(json!({
        "nested_tool_call_ids": cell.nested_tool_call_ids.clone(),
        "nested_tool_edges": nested_tool_edges,
        "terminal_sessions": terminal_sessions
    }))
}

fn required_string<'a>(value: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("manifest is missing string {key}"))
}

fn optional_string(value: &Value, key: &str) -> anyhow::Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => bail!("trace context {key} must be a string or null"),
    }
}
