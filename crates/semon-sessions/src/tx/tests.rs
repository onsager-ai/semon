//! Native rendering and extraction assertions on synthetic generation-bound sources.

use std::{
    env, fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{Value, json};

use super::*;
use crate::{
    Options,
    events::EventCache,
    model::{Texts, build_query},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub(crate) fn page(
    data: &model::QueryData,
    sid: &str,
    anchor: &Anchor,
    now: i64,
) -> io::Result<String> {
    page_limited(data, sid, anchor, now, PAGE_ENTRIES)
}

fn page_limited(
    data: &model::QueryData,
    sid: &str,
    anchor: &Anchor,
    now: i64,
    limit: usize,
) -> io::Result<String> {
    let transcript = data.tx.get(sid).ok_or(io::ErrorKind::NotFound)?;
    let records = data.sources.reader();
    let mut read = |path: &Path, offset| records.read(path, offset);
    let result = page_from_lines(
        &RenderContext {
            home: data.home.as_deref(),
            harness: data.sessions.get(sid).map(|s| s.harness),
            bounded: false,
        },
        transcript,
        sid,
        anchor,
        now,
        limit,
        &mut Lines::provider(&mut read),
    )?;
    records.finish()?;
    Ok(result)
}

// Focused parser assertions use the same native extraction, cut handling and
// byte limits as rendering/search. No retired endpoint or workspace transport
// is implemented here. Results are typed parser values.
fn extracted_part(
    data: &model::QueryData,
    sid: &str,
    index: usize,
    part: &str,
) -> io::Result<Value> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "native part unavailable");
    let slot = data
        .tx
        .get(sid)
        .and_then(|tx| tx.slots.get(index))
        .ok_or(io::ErrorKind::NotFound)?;
    let file = slot.file.as_deref().ok_or_else(invalid)?;
    let records = data.sources.reader();
    let mut read = |path: &Path, offset| records.read(path, offset);
    let mut lines = Lines::provider(&mut read);
    let value = match part {
        "script" => {
            let offset = match slot.kind {
                SlotKind::Operation {
                    script_offset: Some(offset),
                    ..
                } => offset,
                SlotKind::Yielded { .. } | SlotKind::Sent { .. } => slot.offset,
                _ => return Err(invalid()),
            };
            let script = script_at(&mut lines, &file.path, offset).ok_or_else(invalid)?;
            let (text, truncated) = clip(&script, FULL_MAX);
            json!({"text":text,"truncated":truncated})
        }
        "out" => {
            let (text, source, cut) = match &slot.kind {
                SlotKind::Tool {
                    reply,
                    item,
                    result_source,
                    ..
                } => {
                    let (text, source) = tool_output(
                        &mut lines,
                        &file.path,
                        reply.as_ref(),
                        *item,
                        *result_source,
                    )
                    .ok_or_else(invalid)?;
                    (text, source, false)
                }
                SlotKind::Operation { kind, .. } if kind == "CommandExecution" => {
                    let record = lines.get(&file.path, slot.offset).ok_or_else(invalid)?;
                    (
                        record
                            .pointer("/payload/item/aggregated_output")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .to_owned(),
                        Source::Collected,
                        false,
                    )
                }
                SlotKind::Yielded {
                    done,
                    first,
                    polls,
                    cut,
                    ..
                } => {
                    if let Some(output) = done
                        .as_ref()
                        .and_then(|done| lines.get(&file.path, done.o))
                        .and_then(|record| {
                            command_item(&record)
                                .and_then(|item| field(item, "aggregated_output"))
                                .map(str::to_owned)
                        })
                    {
                        (output, Source::Collected, false)
                    } else {
                        let (text, clipped) =
                            chunks(&mut lines, &file.path, *first, polls, *cut, FULL_MAX);
                        (text, Source::Model, clipped)
                    }
                }
                SlotKind::Sent { .. } => (
                    slot_texts(&mut lines, slot)
                        .into_iter()
                        .find(|(part, _)| *part == "out")
                        .ok_or_else(invalid)?
                        .1,
                    Source::Model,
                    false,
                ),
                _ => return Err(invalid()),
            };
            text_value(&text, source, cut, FULL_MAX)
        }
        "in" => {
            let text = match &slot.kind {
                SlotKind::Tool { .. } => {
                    let record = lines.get(&file.path, slot.offset).ok_or_else(invalid)?;
                    let name = tool_name(&record, slot.block as usize).ok_or_else(invalid)?;
                    let input = tool_input(&record, slot.block as usize).ok_or_else(invalid)?;
                    if SHELLS.contains(&name.as_str()) {
                        command(&input).ok_or_else(invalid)?
                    } else if let Value::String(text) = input {
                        text
                    } else {
                        serde_json::to_string_pretty(&input).unwrap()
                    }
                }
                SlotKind::Operation { kind, .. } if kind == "CommandExecution" => {
                    let record = lines.get(&file.path, slot.offset).ok_or_else(invalid)?;
                    item_command(record.pointer("/payload/item").ok_or_else(invalid)?)
                        .ok_or_else(invalid)?
                }
                _ => {
                    slot_texts(&mut lines, slot)
                        .into_iter()
                        .find(|(part, _)| *part == "in")
                        .ok_or_else(invalid)?
                        .1
                }
            };
            let (text, truncated) = clip(&text, FULL_MAX);
            json!({"text":text,"truncated":truncated})
        }
        "diff" => {
            let record = lines.get(&file.path, slot.offset).ok_or_else(invalid)?;
            let name = tool_name(&record, slot.block as usize).ok_or_else(invalid)?;
            let input = tool_input(&record, slot.block as usize).ok_or_else(invalid)?;
            let (diff, truncated) = diff_rows(&name, &input, FULL_MAX).ok_or_else(invalid)?;
            json!({"diff":diff,"truncated":truncated})
        }
        _ => return Err(invalid()),
    };
    records.finish()?;
    Ok(value)
}

/// 2026-09-24T00:00:00Z.
const BASE: i64 = 1_790_208_000_000;

fn ts(hour: i64, minute: i64, millis: i64) -> String {
    let at = BASE + (hour * 60 + minute) * 60_000 + millis;
    let seconds = at / 1000 % 60;
    format!(
        "2026-09-24T{hour:02}:{minute:02}:{seconds:02}.{:03}Z",
        at % 1000
    )
}

struct Home {
    root: PathBuf,
    options: Options,
}

impl Home {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "semon-tx-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let options = Options {
            source: crate::Options::default().source,
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        let home = Self { root, options };
        home.write("proc/locks", "");
        home.write("proc/sys/kernel/hostname", "testbox\n");
        home
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn lines(&self, relative: &str, records: &[Value]) -> PathBuf {
        let text: String = records.iter().map(|record| format!("{record}\n")).collect();
        self.write(relative, &text)
    }

    fn built(&self, now: i64) -> model::QueryData {
        let mut cache = EventCache::open_scoped(&self.options.cache, &self.options.source).unwrap();
        build_query(
            &self.options,
            &mut cache,
            &mut false,
            &mut Texts::default(),
            now,
        )
        .unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn claude(sid: &str, time: String, kind: &str, extra: Value) -> Value {
    let mut record =
        json!({"type": kind, "timestamp": time, "sessionId": sid, "cwd": "/work/proj"});
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    record
}

fn said(sid: &str, time: String, blocks: Value) -> Value {
    claude(
        sid,
        time,
        "assistant",
        json!({"message": {"role": "assistant", "model": "claude-opus-5-5", "content": blocks}}),
    )
}

fn result(sid: &str, time: String, id: &str, content: &str, error: bool) -> Value {
    claude(
        sid,
        time,
        "user",
        json!({"message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": content, "is_error": error}]}}),
    )
}

fn ask(sid: &str, time: String, text: &str) -> Value {
    claude(
        sid,
        time,
        "user",
        json!({"origin": {"kind": "human"}, "message": {"role": "user", "content": text}}),
    )
}

fn annotations(home: &Home, sid: &str, kind: &str) -> Value {
    let reply = crate::session_annotations(
        &home.options,
        "fixture",
        &format!("sid={sid}&scope=retained_history&kind={kind}"),
    );
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    serde_json::from_slice(&reply.body).unwrap()
}

fn page_of(built: &model::QueryData, sid: &str, anchor: &Anchor) -> Value {
    serde_json::from_str(&page(built, sid, anchor, BASE + 86_400_000).unwrap()).unwrap()
}

#[test]
fn a_page_has_the_mockup_tx_shape() {
    let home = Home::new();
    let long = format!("{}\nend", "line\n".repeat(PREVIEW_MAX));
    home.lines(
        "claude/projects/-work-proj/lane.jsonl",
        &[
            ask("lane", ts(1, 0, 0), "Fix the <script>alert(1)</script> test"),
            said(
                "lane",
                ts(1, 0, 900),
                json!([{"type": "thinking", "thinking": "Where does it overflow?", "signature": "s"}]),
            ),
            said(
                "lane",
                ts(1, 1, 0),
                json!([{"type": "text", "text": "Running it."}]),
            ),
            said(
                "lane",
                ts(1, 1, 0),
                json!([{"type": "tool_use", "id": "b1", "name": "Bash", "input": {"command": "cd /work/proj && cargo test"}}]),
            ),
            result("lane", ts(1, 1, 1500), "b1", &long, true),
            said(
                "lane",
                ts(1, 2, 0),
                json!([{"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "/work/proj/src/a.rs", "old_string": "fn a() {\n    old();", "new_string": "fn a() {\n    new();"}}]),
            ),
            result("lane", ts(1, 2, 0), "e1", "updated", false),
            said(
                "lane",
                ts(1, 3, 0),
                json!([{"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "/work/proj/src/b.rs"}}]),
            ),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "lane", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    let kinds: Vec<&str> = entries
        .iter()
        .map(|entry| entry["k"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["h", "think", "a", "tool", "tool", "tool"]);
    assert!(
        entries[0]["turn"].is_string(),
        "the page's first entry names its turn"
    );
    assert_eq!(entries[1]["text"], "Where does it overflow?");
    let bash = &entries[3];
    assert_eq!(bash["arg"], "cd /work/proj && cargo test");
    assert_eq!(bash["ok"], false);
    assert_eq!(bash["secs"], "1.5s");
    assert!(
        bash.get("in").is_none(),
        "a command its summary already shows has no input preview"
    );
    let out = bash["out"].as_str().unwrap();
    assert!(out.len() <= PREVIEW_MAX + '…'.len_utf8() && out.ends_with('…'));
    assert_eq!(bash["more"], json!(["out"]));
    assert_eq!(bash["slot"], 3);
    let edit = &entries[4];
    assert_eq!(edit["arg"], "src/a.rs");
    assert_eq!(
        edit["diff"],
        json!([
            ["ctx", " fn a() {"],
            ["del", "-    old();"],
            ["add", "+    new();"]
        ])
    );
    let read = &entries[5];
    assert_eq!(read["ok"], false);
    assert_eq!(read["unfinished"], true);
    assert_eq!(read["secs"], "—");
    assert_eq!(
        (
            page["from"].as_u64(),
            page["to"].as_u64(),
            page["total"].as_u64()
        ),
        (Some(0), Some(6), Some(6))
    );
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(3), Some(2))
    );

    // View all reads each part in full, by the slot.
    let full_of =
        |slot: usize, part: &str| -> Value { extracted_part(&built, "lane", slot, part).unwrap() };
    assert_eq!(full_of(3, "out")["text"], long.as_str());
    assert_eq!(full_of(3, "out")["truncated"], false);
    assert_eq!(full_of(3, "in")["text"], "cd /work/proj && cargo test");
    assert_eq!(full_of(4, "diff")["diff"][2], json!(["add", "+    new();"]));
    assert!(extracted_part(&built, "lane", 3, "nope").is_err());
    assert!(
        extracted_part(&built, "lane", 2, "in").is_err(),
        "not a tool call"
    );
    assert!(
        extracted_part(&built, "lane", 5, "out").is_err(),
        "no result"
    );
    assert!(extracted_part(&built, "lane", 99, "in").is_err());
    assert!(extracted_part(&built, "nobody", 0, "in").is_err());
}

fn task_notice(sid: &str, time: String, id: &str, status: &str, summary: &str) -> Value {
    claude(
        sid,
        time,
        "user",
        json!({"message": {"role": "user", "content": format!(
            "<task-notification><task-id>task</task-id><tool-use-id>{id}</tool-use-id><status>{status}</status><summary>{summary}</summary></task-notification>"
        )}}),
    )
}

#[test]
fn background_bash_tracks_the_notification_without_changing_its_tool_result() {
    for (status, summary, state, exit) in [
        (
            "completed",
            "Background command completed (exit code 0)",
            "done",
            Some(0),
        ),
        (
            "completed",
            "Background command completed (exit code 2)",
            "failed",
            Some(2),
        ),
        (
            "failed",
            "Background command failed (exit code 0)",
            "failed",
            Some(0),
        ),
        (
            "killed",
            "Background command stopped (exit code 137)",
            "killed",
            Some(137),
        ),
        (
            "stopped",
            "Background command stopped (exit code 137)",
            "killed",
            Some(137),
        ),
        ("completed", "Background command completed", "done", None),
    ] {
        let home = Home::new();
        let output = "Command running in background with ID: task. Output is being written to: /tmp/task.output";
        home.lines("claude/projects/-work-proj/bg.jsonl", &[
            ask("bg", ts(1, 0, 0), "Fetch"),
            task_notice("bg", ts(1, 0, 100), "bash", "failed", "Before the call (exit code 9)"),
            said("bg", ts(1, 1, 0), json!([{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"git fetch","description":"Fetch changes","run_in_background":true}}])),
            result("bg", ts(1, 1, 500), "bash", output, false),
            task_notice("bg", ts(1, 1, 600), "bash", "running", "Still running"),
            task_notice("bg", ts(1, 1, 700), "other", "failed", "Wrong call (exit code 9)"),
            task_notice("bg", ts(1, 2, 250), "bash", status, summary),
            said("bg", ts(1, 3, 0), json!([{"type":"text","text":"After completion"}])),
            task_notice("bg", ts(1, 4, 0), "bash", "failed", "Later notification (exit code 9)"),
        ]);
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "bg", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| e["k"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["h", "tool", "bgend", "a"]
        );
        let step = &entries[1];
        assert_eq!(step["tid"], "bash");
        assert_eq!(step["ok"], true);
        assert_eq!(step["secs"], "0.5s");
        assert_eq!(step["out"], output);
        assert_eq!(step["bg"]["state"], state);
        assert_eq!(step["bg"]["status"], status);
        assert_eq!(step["bg"]["exit"], json!(exit));
        assert_eq!(step["bg"]["secs"], "1m 0s");
        assert_eq!(step["bg"]["summary"], summary);
        assert_eq!(entries[2]["call"], "bash");
        assert_eq!(entries[2]["state"], state);
        assert_eq!(entries[2]["exit"], json!(exit));
        assert_eq!(entries[2]["label"], "Fetch changes");
        assert_eq!(entries[2]["state"], step["bg"]["state"]);
        assert_eq!(entries[2]["exit"], step["bg"]["exit"]);
        // The finish row carries the step's whole lifecycle, so a viewer
        // holding the step from an earlier page updates it from the row.
        assert_eq!(entries[2]["bg"], step["bg"]);
        assert!(page.get("bg_running").is_none());
        let failed = state == "failed";
        assert_eq!(page["errors"], json!(usize::from(failed)));
        assert_eq!(built.sessions["bg"].errors, Some(usize::from(failed)));
        assert_eq!(built.tx["bg"].slots[1].failed_call(), Some(failed));
        // A later page retains the tool-use id even when the call is not on it.
        let tail: Value = serde_json::from_str(
            &page_limited(&built, "bg", &Anchor::After(2), BASE + 86_400_000, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(tail["entries"][0]["call"], "bash");
    }
}

#[test]
fn background_bash_without_an_end_runs_while_the_session_is_alive_after_its_turn() {
    for status in [None, Some("busy"), Some("idle"), Some("shell")] {
        let live = status.is_some();
        let home = Home::new();
        if live {
            home.write(
                "proc/1/stat",
                &format!("1 (claude) {}\n", ["0"; 19].join(" ") + " 7"),
            );
            home.write(
                "claude/sessions/1.json",
                &json!({"pid":1,"sessionId":"bg","procStart":7,"status":status}).to_string(),
            );
        }
        home.lines("claude/projects/-work-proj/bg.jsonl", &[
            ask("bg", ts(1, 0, 0), "Fetch"),
            said("bg", ts(1, 1, 0), json!([{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"git fetch","run_in_background":true}}])),
            result("bg", ts(1, 1, 500), "bash", "Command running in background with ID: task", false),
            task_notice("bg", ts(1, 1, 600), "different", "completed", "Wrong call (exit code 0)"),
            said("bg", ts(1, 1, 700), json!([{"type":"text","text":"Waiting for the background command"}])),
            ask("bg", ts(1, 1, 800), "Start another turn"),
        ]);
        let now = BASE + 3_710_000;
        let built = home.built(now);
        let page: Value =
            serde_json::from_str(&page(&built, "bg", &Anchor::Last, now).unwrap()).unwrap();
        let step = &page["entries"][1];
        assert_eq!(page["entries"].as_array().unwrap().len(), 4);
        assert_eq!(
            step["bg"]["state"],
            if live { "running" } else { "unknown" }
        );
        if live {
            assert_eq!(step["bg"]["secs"], "50s");
            assert_eq!(step["bg"]["since"], BASE + 3_660_000);
            if status != Some("busy") {
                assert_eq!(built.sessions["bg"].state, "idle");
            }
        } else {
            assert_eq!(step["bg"], json!({"state":"unknown"}));
        }
        // Every page lists the calls still running, whichever it holds.
        let tail: Value =
            serde_json::from_str(&page_limited(&built, "bg", &Anchor::After(3), now, 1).unwrap())
                .unwrap();
        assert_eq!(
            tail["bg_running"],
            if live { json!(["bash"]) } else { Value::Null }
        );
        assert_eq!(step["ok"], true);
        assert_eq!(step["secs"], "0.5s");
        if live {
            let before = page_of(&built, "bg", &Anchor::Last);
            fs::remove_file(home.root.join("proc/1/stat")).unwrap();
            let stopped = home.built(BASE + 86_400_000);
            let after = page_of(&stopped, "bg", &Anchor::Last);
            assert_ne!(
                before["bg_running"], after["bg_running"],
                "the transcript refreshes when its background call ceases to be live without a new log line"
            );
            assert_eq!(
                page_of(&stopped, "bg", &Anchor::Last)["entries"][1]["bg"]["state"],
                "unknown"
            );
        }
    }
}

/// A lane starts a background command, then restarts with `--resume`: the
/// new process writes a new file of the same session, and the old
/// process's shells died with it. Only a call in the file a live process
/// writes can still be running.
#[test]
fn a_background_call_runs_only_while_a_live_process_writes_its_own_file() {
    for in_new in [false, true] {
        let home = Home::new();
        home.write(
            "proc/1/stat",
            &format!("1 (claude) {}\n", ["0"; 19].join(" ") + " 7"),
        );
        home.write(
            "claude/sessions/1.json",
            &json!({"pid":1,"sessionId":"new","procStart":7,"status":"shell"}).to_string(),
        );
        let sid = if in_new { "new" } else { "old" };
        let call = [
            said(
                sid,
                ts(1, if in_new { 6 } else { 1 }, 0),
                json!([{"type":"tool_use","id":"watch","name":"Bash","input":{"command":"gh pr checks --watch","run_in_background":true}}]),
            ),
            result(
                sid,
                ts(1, if in_new { 6 } else { 1 }, 500),
                "watch",
                "Command running in background with ID: watch",
                false,
            ),
        ];
        let mut old = vec![ask("old", ts(1, 0, 0), "Watch the checks")];
        let mut new = vec![ask("new", ts(1, 5, 0), "Carry on")];
        if in_new {
            new.extend(call)
        } else {
            old.extend(call)
        }
        old.push(json!({"type":"continued-in","continuedInSessionId":"new","sessionId":"old","timestamp":ts(1, 2, 0)}));
        new.push(said(
            "new",
            ts(1, 7, 0),
            json!([{"type":"text","text":"Still here"}]),
        ));
        home.lines("claude/projects/-work-proj/old.jsonl", &old);
        home.lines("claude/projects/-work-proj/new.jsonl", &new);
        let now = BASE + 3 * 3_600_000;
        let built = home.built(now);
        // One session, stitched from both files, and alive (not "done").
        assert_eq!(built.tx.len(), 1);
        assert_eq!(built.sessions["old"].state, "idle");
        let page: Value =
            serde_json::from_str(&page(&built, "old", &Anchor::Last, now).unwrap()).unwrap();
        let step = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["tid"] == "watch")
            .unwrap();
        if in_new {
            assert_eq!(step["bg"]["state"], "running");
            assert_eq!(page["bg_running"], json!(["watch"]));
        } else {
            assert_eq!(step["bg"], json!({"state":"unknown"}));
            assert!(page.get("bg_running").is_none());
            // Nothing runs, so the mark does not count it either.
        }
    }
}

/// A live process for session `bg`, started at `started` (epoch ms), if given.
fn live_pid(home: &Home, started: Option<i64>) {
    home.write(
        "proc/1/stat",
        &format!("1 (claude) {}\n", ["0"; 19].join(" ") + " 7"),
    );
    let mut record = json!({"pid":1,"sessionId":"bg","procStart":7,"status":"shell"});
    if let Some(started) = started {
        record["startedAt"] = json!(started);
    }
    home.write("claude/sessions/1.json", &record.to_string());
}

/// A background launch that was denied or blocked (an error result)
/// started nothing, and one with no result yet is an ordinary unfinished
/// call: neither runs as background, both count once as failed.
#[test]
fn a_background_launch_that_failed_or_never_returned_does_not_run() {
    for failed in [true, false] {
        let home = Home::new();
        live_pid(&home, None);
        let mut records = vec![
            ask("bg", ts(1, 0, 0), "Watch the checks"),
            said(
                "bg",
                ts(1, 1, 0),
                json!([{"type":"tool_use","id":"watch","name":"Bash","input":{"command":"gh pr checks --watch","run_in_background":true}}]),
            ),
        ];
        if failed {
            records.push(result(
                "bg",
                ts(1, 1, 500),
                "watch",
                "Permission to use Bash has been denied.",
                true,
            ));
        }
        records.push(said(
            "bg",
            ts(1, 2, 0),
            json!([{"type":"text","text":"Could not start it"}]),
        ));
        records.push(ask("bg", ts(1, 3, 0), "Next"));
        home.lines("claude/projects/-work-proj/bg.jsonl", &records);
        let now = BASE + 3 * 3_600_000;
        let built = home.built(now);
        let page: Value =
            serde_json::from_str(&page(&built, "bg", &Anchor::Last, now).unwrap()).unwrap();
        let step = &page["entries"][1];
        assert_eq!(step["k"], "tool");
        assert!(step.get("bg").is_none(), "{step}");
        assert!(step.get("tid").is_none(), "{step}");
        assert!(step.get("live").is_none(), "{step}");
        if failed {
            assert_eq!(step["ok"], false);
        } else {
            assert_eq!(step["unfinished"], true);
        }
        assert!(page.get("bg_running").is_none());
        assert_eq!(page["errors"], 1);
        assert_eq!(built.sessions["bg"].errors, Some(1));
    }
}

/// A session resumed into its own file: the live process started after
/// an older background call, whose shell died with the process that
/// launched it. Only a call at or after the process's start can run.
#[test]
fn a_background_call_older_than_its_files_live_process_does_not_run() {
    let call = BASE + 3_660_000;
    for (started, running) in [
        (Some(call + 60_000), false),
        (Some(call), true),
        (Some(call - 60_000), true),
        (None, true),
    ] {
        let home = Home::new();
        live_pid(&home, started);
        home.lines("claude/projects/-work-proj/bg.jsonl", &[
            ask("bg", ts(1, 0, 0), "Watch the checks"),
            said("bg", ts(1, 1, 0), json!([{"type":"tool_use","id":"watch","name":"Bash","input":{"command":"gh pr checks --watch","run_in_background":true}}])),
            result("bg", ts(1, 1, 500), "watch", "Command running in background with ID: watch", false),
            ask("bg", ts(1, 5, 0), "Carry on"),
            said("bg", ts(1, 6, 0), json!([{"type":"text","text":"Still here"}])),
        ]);
        let now = BASE + 3 * 3_600_000;
        let built = home.built(now);
        assert_eq!(built.sessions["bg"].state, "idle");
        let page: Value =
            serde_json::from_str(&page(&built, "bg", &Anchor::Last, now).unwrap()).unwrap();
        let step = &page["entries"][1];
        assert_eq!(step["tid"], "watch");
        if running {
            assert_eq!(step["bg"]["state"], "running", "{started:?}");
            assert_eq!(page["bg_running"], json!(["watch"]));
        } else {
            assert_eq!(step["bg"], json!({"state":"unknown"}));
            assert!(page.get("bg_running").is_none());
        }
    }
}

#[test]
fn foreground_bash_has_no_background_fields_or_finish_row() {
    let home = Home::new();
    home.lines("claude/projects/-work-proj/fg.jsonl", &[
        ask("fg", ts(1, 0, 0), "Fetch"),
        said("fg", ts(1, 1, 0), json!([{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"git fetch","run_in_background":false}}])),
        result("fg", ts(1, 1, 500), "bash", "Fetched", false),
        task_notice("fg", ts(1, 2, 0), "bash", "completed", "Completed (exit code 0)"),
    ]);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "fg", &Anchor::Last);
    assert_eq!(page["entries"].as_array().unwrap().len(), 2);
    assert!(page["entries"][1].get("bg").is_none());
    assert!(page["entries"][1].get("tid").is_none());
}

#[test]
fn batched_background_notifications_use_each_calls_summary_and_label() {
    let home = Home::new();
    let long = format!("{} (exit code 2)", "é".repeat(PREVIEW_MAX));
    let one = task_notice("bg", ts(1, 2, 0), "one", "completed", "One (exit code 0)");
    let two = task_notice("bg", ts(1, 2, 0), "two", "completed", &long);
    let mut notice = one.clone();
    notice["message"]["content"] = json!(format!(
        "{}{}",
        one["message"]["content"].as_str().unwrap(),
        two["message"]["content"].as_str().unwrap()
    ));
    home.lines("claude/projects/-work-proj/bg.jsonl", &[
        ask("bg", ts(1, 0, 0), "Fetch"),
        said("bg", ts(1, 1, 0), json!([
            {"type":"tool_use","id":"one","name":"Bash","input":{"command":"git fetch one","run_in_background":true}},
            {"type":"tool_use","id":"two","name":"Bash","input":{"command":"git fetch two","run_in_background":true}}
        ])),
        result("bg", ts(1, 1, 500), "one", "Command running in background", false),
        result("bg", ts(1, 1, 500), "two", "Command running in background", false),
        notice,
    ]);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "bg", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[1]["bg"]["summary"], "One (exit code 0)");
    assert_eq!(entries[2]["bg"]["exit"], 2);
    assert_eq!(entries[2]["bg"]["summary"], clip(&long, PREVIEW_MAX).0);
    assert_eq!(entries[3]["label"], "git fetch one");
    assert_eq!(entries[4]["label"], "git fetch two");
    assert_eq!(entries[4]["state"], "failed");
}

#[test]
fn a_live_tool_entry_has_its_start_and_a_finished_entry_has_no_since() {
    let home = Home::new();
    let start = BASE + 60_000;
    let path = home.lines(
        "claude/projects/-work-proj/lane.jsonl",
        &[said(
            "lane",
            ts(0, 1, 0),
            json!([{"type":"tool_use","id":"live","name":"Bash","input":{"command":"sleep 30"}}]),
        )],
    );
    let file = SlotFile {
        prompts: Default::default(),
        path,
        cwd: Some("/work/proj".to_owned()),
    };
    let slot = |shown| Slot {
        kind: SlotKind::Tool {
            shown,
            name: "Bash".to_owned(),
            result_source: false,
            reply: None,
            item: None,
            bg: None,
        },
        file: Some(std::sync::Arc::new(file.clone())),
        offset: 0,
        block: 0,
        t: Some(start),
        turn: None,
        first: false,
    };
    let live = tool_entry(
        &mut Lines::default(),
        &file,
        &slot(Shown::Live),
        0,
        Shown::Live,
        "Bash",
        None,
        None,
        start + 10_000,
        None,
    );
    assert_eq!(live["since"], json!(start));

    let finished = tool_entry(
        &mut Lines::default(),
        &file,
        &slot(Shown::Ok),
        0,
        Shown::Ok,
        "Bash",
        None,
        None,
        start + 10_000,
        None,
    );
    assert!(finished.get("since").is_none());
}

#[test]
fn state_fields_include_since_only_for_live_steps() {
    let start = BASE + 60_000;
    let mut live = Map::new();
    state_fields(
        &mut live,
        Shown::Live,
        "—".to_owned(),
        Some(start),
        start + 10_000,
    );
    assert_eq!(live["since"], json!(start));

    let mut finished = Map::new();
    state_fields(
        &mut finished,
        Shown::Ok,
        "10.0s".to_owned(),
        Some(start),
        start + 10_000,
    );
    assert!(finished.get("since").is_none());
}

#[test]
fn bash_descriptions_are_titles_only_for_bash_calls() {
    let home = Home::new();
    let long = "x".repeat(300);
    home.lines(
        "claude/projects/-work-proj/titles.jsonl",
        &[
            ask("titles", ts(1, 0, 0), "Show titled tool calls"),
            said(
                "titles",
                ts(1, 0, 1),
                json!([
                    {"type": "tool_use", "id": "with-title", "name": "Bash", "input": {"command": "printf titled\nprintf again", "description": "Check  CI\n and merge"}},
                    {"type": "tool_use", "id": "without-title", "name": "Bash", "input": {"command": "printf untitled"}},
                    {"type": "tool_use", "id": "long-title", "name": "Bash", "input": {"command": "printf long", "description": long}},
                    {"type": "tool_use", "id": "agent-title", "name": "Agent", "input": {"description": "Agent title", "prompt": "delegate"}}
                ]),
            ),
        ],
    );

    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "titles", &Anchor::Last);
    let tools = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["k"] == "tool")
        .collect::<Vec<_>>();

    assert_eq!(tools.len(), 4);
    assert_eq!(tools[0]["title"], "Check CI and merge");
    assert_eq!(tools[0]["arg"], "printf titled printf again");
    assert_eq!(tools[0]["in"], "printf titled\nprintf again");
    assert!(tools[1].get("title").is_none());
    let clipped = tools[2]["title"].as_str().unwrap();
    assert_eq!(clipped.chars().count(), TITLE_MAX);
    assert!(clipped.ends_with('…'));
    assert!(tools[3].get("title").is_none());
}

#[test]
fn pages_are_bounded_and_page_back_without_gaps() {
    let home = Home::new();
    let mut records = Vec::new();
    for number in 0..450 {
        if number % 50 == 0 {
            records.push(ask(
                "many",
                ts(2, number / 60, (number % 60) * 1000),
                &format!("ask {number}"),
            ));
        }
        records.push(said(
            "many",
            ts(2, number / 60, (number % 60) * 1000 + 1),
            json!([{"type": "tool_use", "id": format!("t{number}"), "name": "Grep", "input": {"pattern": format!("p{number}")}}]),
        ));
        records.push(result(
            "many",
            ts(2, number / 60, (number % 60) * 1000 + 2),
            &format!("t{number}"),
            "hit",
            false,
        ));
    }
    home.lines("claude/projects/-work-proj/many.jsonl", &records);
    let built = home.built(BASE + 86_400_000);
    let total = built.tx["many"].slots.len();
    let mut seen = Vec::new();
    let mut anchor = Anchor::Last;
    loop {
        let page = page_of(&built, "many", &anchor);
        let entries = page["entries"].as_array().unwrap();
        assert!(!entries.is_empty() && entries.len() <= PAGE_ENTRIES);
        assert!(page.to_string().len() <= PAGE_BYTES + 1024);
        assert!(entries[0]["turn"].is_string());
        let from = page["from"].as_u64().unwrap() as usize;
        let to = page["to"].as_u64().unwrap() as usize;
        assert_eq!(to - from, entries.len());
        seen.splice(0..0, from..to);
        if from == 0 {
            break;
        }
        anchor = Anchor::Before(from);
    }
    assert_eq!(seen, (0..total).collect::<Vec<_>>());
    // A turn's page starts at its first entry, and later pages follow on.
    let turn = built.tx["many"].slots[250..]
        .iter()
        .find(|slot| slot.first)
        .and_then(|slot| slot.turn.clone())
        .unwrap();
    let page = page_of(&built, "many", &Anchor::Turn(turn.clone()));
    assert_eq!(page["entries"][0]["turn"], turn.as_str());
    let next = page_of(
        &built,
        "many",
        &Anchor::After(page["to"].as_u64().unwrap() as usize),
    );
    assert_eq!(next["from"], page["to"]);
    assert_eq!(
        self::page(&built, "many", &Anchor::Turn("missing".into()), 0)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(self::page(&built, "nobody", &Anchor::Last, 0).is_err());
}

#[test]
fn codex_harness_text_and_a_failed_return() {
    let home = Home::new();
    home.lines(
        "claude/projects/-work-proj/lead.jsonl",
        &[
            ask("lead", ts(3, 0, 0), "Hand it to Codex"),
            said(
                "lead",
                ts(3, 1, 0),
                json!([{"type": "tool_use", "id": "bash-1", "name": "Bash", "input": {"command": "codex exec < /tmp/p.md"}}]),
            ),
        ],
    );
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-run.jsonl",
        &[
            codex(ts(3, 1, 0), "session_meta", json!({"id": "run", "cwd": "/work/proj"})),
            codex(ts(3, 1, 0), "event_msg", json!({"type": "task_started", "turn_id": "failed-turn"})),
            codex(ts(3, 1, 0), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Semon-Parent: claude:lead:bash-1\nImplement it"}]})),
            codex(ts(3, 1, 1), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<recommended_plugins>x</recommended_plugins>"}]})),
            codex(ts(3, 1, 2), "response_item", json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": "Plan it"}]})),
            codex(ts(3, 2, 0), "response_item", json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "c1", "input": "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n*** End Patch"})),
            codex(ts(3, 2, 0), "response_item", json!({"type": "custom_tool_call_output", "call_id": "c1", "output": "{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"})),
            codex(ts(3, 3, 0), "response_item", json!({"type": "function_call", "name": "shell", "call_id": "c2", "arguments": "{\"command\":[\"make\"]}"})),
            codex(ts(3, 3, 500), "response_item", json!({"type": "function_call_output", "call_id": "c2", "output": "no status"})),
            codex(ts(3, 4, 0), "response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done"}]})),
            codex(ts(3, 4, 0), "event_msg", json!({"type": "task_complete", "turn_id": "failed-turn", "error": {"message": "failed"}})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "run", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    let kinds: Vec<&str> = entries
        .iter()
        .map(|entry| entry["k"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["h", "harness", "think", "tool", "tool", "a", "end"]);
    assert_eq!(entries[1]["label"], "recommended_plugins");
    assert_eq!(entries[2]["text"], "Plan it");
    assert_eq!(entries[3]["arg"], "src/a.rs");
    assert_eq!(
        entries[3]["diff"],
        json!([["del", "-old"], ["add", "+new"]])
    );
    assert_eq!(
        entries[4]["ok"],
        Value::Null,
        "no exit status: neither ok nor failed"
    );
    assert_eq!(entries[4]["out"], "no status");
    assert_eq!(entries[6]["ret"]["to"], "lead");
    assert_eq!(entries[6]["ret"]["failed"], true);
}

#[test]
fn code_mode_exec_renders_its_completed_operations() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    let script = "const r = await Promise.allSettled([\ntools.exec_command({cmd:\"git status\"}),\ntools.exec_command({cmd:\"sed -n '1,9p' a.rs\"}),\n]);";
    home.lines(
        "codex/sessions/2026/09/24/rollout-ops.jsonl",
        &[
            codex(
                ts(4, 0, 0),
                "session_meta",
                json!({"id":"ops","cwd":"/work/proj"}),
            ),
            codex(
                ts(4, 1, 0),
                "response_item",
                json!({"type":"custom_tool_call","call_id":"call_A","name":"exec","status":"completed","input":script}),
            ),
            codex(
                ts(4, 2, 0),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-1","command":["/bin/zsh","-lc","git status"],"cwd":"file:///work/proj","exit_code":0,"duration":{"secs":1,"nanos":200000000},"aggregated_output":"## main\n"}}),
            ),
            codex(
                ts(4, 2, 1),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-2","command":["/bin/zsh","-lc","sed -n '1,9p' a.rs"],"cwd":"file:///work/proj/src","exit_code":0,"duration":{"secs":0,"nanos":400000000},"aggregated_output":"a\n"}}),
            ),
            codex(
                ts(4, 2, 2),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"FileChange","id":"exec-3","changes":{"/work/proj/src/a.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\n-a\n+b\n","move_path":null}}}}),
            ),
            codex(
                ts(4, 3, 0),
                "response_item",
                json!({"type":"custom_tool_call_output","call_id":"call_A","output":[{"type":"input_text","text":"Script completed"}]}),
            ),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "ops", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["k"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["tool", "tool", "tool"]
    );
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(3), Some(0))
    );
    assert_eq!(entries[0]["arg"], "git status");
    assert_eq!(entries[0]["cwd"], ".");
    assert_eq!(entries[0]["secs"], "1.2s");
    assert_eq!(entries[0]["exit"], 0);
    assert_eq!(entries[0]["out"], "## main\n");
    assert_eq!(entries[1]["arg"], "sed -n '1,9p' a.rs");
    assert_eq!(entries[1]["cwd"], "src");
    assert_eq!(entries[2]["arg"], "src/a.rs");
    assert_eq!(
        entries[2]["changes"][0]["diff"],
        json!([["del", "-a"], ["add", "+b"]])
    );
    assert!(entries[0]["script"].is_number());
    let script_view: Value = extracted_part(&built, "ops", 0, "script").unwrap();
    assert_eq!(script_view["text"], script);
    assert_eq!(script_view["truncated"], false);
}

#[test]
fn codex_native_connector_tools_are_steps_between_thoughts() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp":time,"type":kind,"payload":payload});
    home.lines("codex/sessions/2026/09/24/rollout-native-tools.jsonl", &[
        codex(ts(4,0,0), "session_meta", json!({"id":"native-tools","cwd":"/work/proj"})),
        codex(ts(4,1,0), "response_item", json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Inspect tools"}]})),
        codex(ts(4,1,1), "response_item", json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Before tools"}]})),
        codex(ts(4,1,2), "response_item", json!({"type":"custom_tool_call","name":"exec","call_id":"wrapper","input":"await tools.search({query:'parser'});"})),
        codex(ts(4,1,3), "event_msg", json!({"type":"item_completed","item":{"type":"McpToolCall","id":"mcp","server":"github","tool":"search","arguments":{"query":"parser"},"status":"completed","result":{"content":[{"type":"text","text":"Tool result, not reasoning"}],"isError":false},"duration":{"secs":2,"nanos":0}}})),
        codex(ts(4,1,4), "event_msg", json!({"type":"item_completed","item":{"type":"DynamicToolCall","id":"dynamic","namespace":"functions","tool":"lookup","arguments":{"key":"parser"},"status":"completed","success":false,"content_items":[{"type":"input_text","text":"Lookup failed"}]}})),
        codex(ts(4,1,5), "response_item", json!({"type":"custom_tool_call_output","call_id":"wrapper","output":[{"type":"input_text","text":"Script completed"}]})),
        codex(ts(4,1,6), "response_item", json!({"type":"reasoning","summary":[{"type":"summary_text","text":"After tools"}]})),
        codex(ts(4,1,7), "response_item", json!({"type":"future_item","summary":[{"type":"summary_text","text":"Unknown is not thinking"}]})),
    ]);
    for _ in 0..2 {
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "native-tools", &Anchor::Last);
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry["k"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["u", "think", "tool", "tool", "think"]
        );
        assert_eq!(entries[2]["name"], "github.search");
        assert_eq!(entries[2]["out"], "Tool result, not reasoning");
        assert_eq!(entries[2]["ok"], true);
        assert_eq!(entries[2]["secs"], "2.0s");
        assert_eq!(entries[3]["name"], "functions.lookup");
        assert_eq!(entries[3]["out"], "Lookup failed");
        assert_eq!(entries[3]["ok"], false);
        let full: Value = extracted_part(
            &built,
            "native-tools",
            entries[2]["slot"].as_u64().unwrap() as usize,
            "out",
        )
        .unwrap();
        assert_eq!(full["text"], "Tool result, not reasoning");
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(2), Some(1))
        );
    }
}

#[test]
fn codex_tool_search_and_native_completion_keep_exact_call_identity() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp":time,"type":kind,"payload":payload});
    home.lines("codex/sessions/2026/09/24/rollout-tool-search.jsonl", &[
        codex(ts(4,0,0), "session_meta", json!({"id":"tool-search","cwd":"/work/proj"})),
        codex(ts(4,1,0), "response_item", json!({"type":"tool_search_call","id":"search","arguments":{"query":"repository"}})),
        codex(ts(4,1,1), "response_item", json!({"type":"tool_search_output","call_id":"search","tools":[{"name":"fetch"}]})),
        codex(ts(4,1,2), "response_item", json!({"type":"function_call","name":"lookup","call_id":"mirrored","arguments":"{\"key\":\"parser\"}"})),
        codex(ts(4,1,3), "event_msg", json!({"type":"item_completed","item":{"type":"DynamicToolCall","id":"mirrored","tool":"lookup","arguments":{"key":"parser"},"success":true,"status":"completed","content_items":[{"type":"input_text","text":"native answer"}]}})),
        codex(ts(4,1,4), "response_item", json!({"type":"function_call_output","call_id":"mirrored","output":"legacy answer"})),
    ]);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "tool-search", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["name"], "tool_search");
    assert!(entries[0]["out"].as_str().unwrap().contains("fetch"));
    assert_eq!(entries[1]["name"], "lookup");
    assert_eq!(entries[1]["out"], "native answer");
    assert_eq!(entries[1]["ok"], true);
    assert_eq!(page["calls"], 2);
    assert_eq!(page["errors"], 0);
}

#[test]
fn codex_failed_and_unfinished_wrappers_follow_their_native_tools() {
    for failed in [false, true] {
        let home = Home::new();
        let codex = |kind: &str, payload: Value| json!({"timestamp":ts(4,1,0),"type":kind,"payload":payload});
        let mut records = vec![
            codex("session_meta", json!({"id":"native-wrapper"})),
            codex(
                "response_item",
                json!({"type":"custom_tool_call","name":"exec","call_id":"wrapper","input":"await tools.lookup({key:'parser'});"}),
            ),
            codex(
                "event_msg",
                json!({"type":"item_completed","item":{"type":"DynamicToolCall","id":"native","namespace":"functions","tool":"lookup","arguments":{"key":"parser"},"status":"completed","success":true,"content_items":[{"type":"input_text","text":"answer"}]}}),
            ),
        ];
        if failed {
            records.push(codex("response_item", json!({"type":"custom_tool_call_output","call_id":"wrapper","output":[{"type":"input_text","text":"Script error: fixture failure"}]})));
        }
        home.lines(
            "codex/sessions/2026/09/24/rollout-native-wrapper.jsonl",
            &records,
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, "native-wrapper", &Anchor::Last);
        assert_eq!(page["entries"][0]["name"], "functions.lookup");
        assert_eq!(page["entries"][1]["name"], "exec");
        assert_eq!(page["entries"][1]["ok"], false);
        assert_eq!(page["calls"], 2);
        assert_eq!(page["errors"], 1);
    }
}

#[test]
fn code_mode_command_duration_falls_back_when_underreported_or_missing() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    let item = |id: &str, command: &str, duration: Option<Value>| {
        let mut item = json!({
            "type": "CommandExecution",
            "id": id,
            "command": ["/bin/zsh", "-lc", command],
            "exit_code": 0,
            "aggregated_output": "done\n"
        });
        if let Some(duration) = duration {
            item["duration"] = duration;
        }
        item
    };
    let completed = |time: String, item: Value| {
        codex(
            time,
            "event_msg",
            json!({"type":"item_completed","item":item}),
        )
    };
    let script = "const r = await Promise.allSettled([\ntools.exec_command({cmd: 'short'}),\ntools.exec_command({cmd: 'recorded'}),\ntools.exec_command({cmd: 'long'}),\ntools.exec_command({cmd: 'missing'}),\n]);";
    home.lines(
        "codex/sessions/2026/09/24/rollout-command-duration.jsonl",
        &[
            codex(
                ts(4, 0, 0),
                "session_meta",
                json!({"id":"command-duration","cwd":"/work/proj"}),
            ),
            codex(
                ts(4, 1, 0),
                "response_item",
                json!({"type":"custom_tool_call","call_id":"call","name":"exec","status":"completed","input":script}),
            ),
            completed(
                ts(4, 1, 142),
                item(
                    "short",
                    "short",
                    Some(json!({"secs":0,"nanos":4029})),
                ),
            ),
            completed(
                ts(4, 1, 800),
                item(
                    "recorded",
                    "recorded",
                    Some(json!({"secs":0,"nanos":562360489})),
                ),
            ),
            completed(
                ts(4, 1, 1800),
                item(
                    "long",
                    "long",
                    Some(json!({"secs":1,"nanos":710401325})),
                ),
            ),
            completed(ts(4, 1, 4142), item("missing", "missing", None)),
            codex(
                ts(4, 1, 6000),
                "response_item",
                json!({"type":"custom_tool_call_output","call_id":"call","output":"done"}),
            ),
        ],
    );

    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "command-duration", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["secs"], "~0.1s");
    assert_eq!(entries[1]["secs"], "0.6s");
    assert_eq!(entries[2]["secs"], "1.7s");
    // No prior completion is carried into this step, so the span starts at the call record.
    assert_eq!(entries[3]["secs"], "~4.1s");
}

#[test]
fn code_mode_command_longer_than_summary_has_a_full_command_preview() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    let command = format!("printf {}", "x".repeat(PREVIEW_MAX + 32));
    let script = "tools.exec_command({cmd: 'long command'})";
    home.lines(
        "codex/sessions/2026/09/24/rollout-long-command.jsonl",
        &[
            codex(
                ts(4, 0, 0),
                "session_meta",
                json!({"id":"long-command","cwd":"/work/proj"}),
            ),
            codex(
                ts(4, 1, 0),
                "response_item",
                json!({"type":"custom_tool_call","call_id":"call","name":"exec","status":"completed","input":script}),
            ),
            codex(
                ts(4, 2, 0),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc",command.clone()],"exit_code":0,"aggregated_output":"done\n"}}),
            ),
            codex(
                ts(4, 3, 0),
                "response_item",
                json!({"type":"custom_tool_call_output","call_id":"call","output":"done"}),
            ),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "long-command", &Anchor::Last);
    let entry = &page["entries"][0];
    assert!(entry["arg"].as_str().unwrap().ends_with('…'));
    assert!(entry["in"].as_str().unwrap().starts_with("printf "));
    assert!(entry["in"].as_str().unwrap().ends_with('…'));
    assert!(entry["more"].as_array().unwrap().contains(&json!("in")));
    let slot = entry["slot"].as_u64().unwrap() as usize;
    let full: Value = extracted_part(&built, "long-command", slot, "in").unwrap();
    assert_eq!(full["text"], command);
    assert_eq!(full["truncated"], false);
}

#[test]
fn operation_output_can_be_opened_in_full() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    let output = format!("{}tail", "x".repeat(PREVIEW_MAX + 32));
    home.lines(
        "codex/sessions/2026/09/24/rollout-long-operation.jsonl",
        &[
            codex(ts(4, 0, 0), "session_meta", json!({"id":"long-operation","cwd":"/work/proj"})),
            codex(ts(4, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"tools.exec_command({cmd:'long'})"})),
            codex(ts(4, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","long"],"exit_code":0,"aggregated_output":output}})),
            codex(ts(4, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":"done"})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "long-operation", &Anchor::Last);
    let slot = page["entries"][0]["slot"].as_u64().unwrap() as usize;
    assert!(
        page["entries"][0]["more"]
            .as_array()
            .unwrap()
            .contains(&json!("out"))
    );
    let full: Value = extracted_part(&built, "long-operation", slot, "out").unwrap();
    assert_eq!(full["text"], output);
    assert_eq!(full["truncated"], false);
}

fn model(text: &str) -> (String, Option<Cut>) {
    split_cut(text, Source::Model)
}

fn text(part: &str) -> Part {
    Part::Text(part.to_owned())
}

fn gap(n: u64, unit: &'static str, of: Option<u64>) -> Part {
    Part::Gap(Gap { n, unit, of })
}

#[test]
fn a_token_cut_is_split_at_its_marker_and_its_header_stripped() {
    let (plain, cut) = model(
        "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\nline 2…19500 tokens truncated…line 899\nline 900\n",
    );
    assert_eq!(
        plain,
        "line 1\nline 2…19500 tokens truncated…line 899\nline 900\n"
    );
    assert_eq!(
        cut,
        Some(Cut {
            original_tokens: Some(24000),
            lines: Some(900),
            parts: vec![
                text("line 1\nline 2"),
                gap(19500, "tokens", None),
                text("line 899\nline 900\n")
            ],
        })
    );
}

#[test]
fn a_char_cut_is_split_at_its_marker() {
    let (_, cut) = model(
        "Warning: truncated output (original token count: 9000)\nTotal output lines: 40\n\nhead…12345 chars truncated…tail",
    );
    let cut = cut.unwrap();
    assert_eq!(
        cut.parts,
        vec![text("head"), gap(12345, "chars", None), text("tail")]
    );
    assert_eq!((cut.original_tokens, cut.lines), (Some(9000), Some(40)));
}

#[test]
fn the_collection_cap_marker_is_split_with_no_header() {
    let (plain, cut) = model("head\n... 1048576 bytes omitted ...\ntail\n");
    assert_eq!(plain, "head\n... 1048576 bytes omitted ...\ntail\n");
    assert_eq!(
        cut,
        Some(Cut {
            original_tokens: None,
            lines: None,
            parts: vec![text("head"), gap(1_048_576, "bytes", None), text("tail\n")],
        })
    );
}

#[test]
fn the_old_line_marker_is_split_and_gives_the_line_count() {
    let (_, cut) = model("a\nb\n[... omitted 3 of 9 lines ...]\n\nc\nd\n");
    assert_eq!(
        cut,
        Some(Cut {
            original_tokens: None,
            lines: Some(9),
            parts: vec![text("a\nb\n"), gap(3, "lines", Some(9)), text("c\nd\n")],
        })
    );
}

#[test]
fn text_that_only_looks_like_a_marker_is_not_split() {
    for output in [
        // The token marker is written mid-line, so only a header vouches for it.
        "echo says …5 tokens truncated… and goes on\n",
        "…5 chars truncated…",
        "see ... 5 bytes omitted ... here\n",
        "no newline after\n... 5 bytes omitted ...",
        "  [... omitted 3 of 9 lines ...]\n",
        "a [... omitted 3 of 9 lines ...] b\n",
        "[... omitted 3 of 9 lines ...] trailing\n",
        "Total output lines are 900\nplain\n",
        "Warning: truncated output is what Codex prints\n",
    ] {
        let (plain, cut) = model(output);
        assert_eq!((plain.as_str(), cut), (output, None), "{output:?}");
    }
}

#[test]
fn a_header_with_no_marker_keeps_the_cut_but_no_gap() {
    let (plain, cut) = model(
        "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nonly what is left\n",
    );
    assert_eq!(plain, "only what is left\n");
    assert_eq!(
        cut,
        Some(Cut {
            original_tokens: Some(24000),
            lines: Some(900),
            parts: Vec::new(),
        })
    );
}

#[test]
fn a_previewed_cut_keeps_the_head_and_the_end_of_the_tail() {
    let long = "x".repeat(PREVIEW_MAX);
    let (_, cut) = model(&format!(
        "Warning: truncated output (original token count: 1)\n{long}HEAD…9 tokens truncated…{long}TAIL"
    ));
    let (json, clipped) = cut.unwrap().json(PREVIEW_MAX);
    assert!(clipped);
    let parts = json["parts"].as_array().unwrap();
    assert!(parts[0]["text"].as_str().unwrap().ends_with('…'));
    assert_eq!(parts[1], json!({"gap": {"n": 9, "unit": "tokens"}}));
    let tail = parts[2]["text"].as_str().unwrap();
    assert!(tail.starts_with('…') && tail.ends_with("TAIL"));
    assert!(
        parts
            .iter()
            .all(|part| part.to_string().len() < PREVIEW_MAX)
    );
}

fn codex_line(time: String, kind: &str, payload: Value) -> Value {
    json!({"timestamp": time, "type": kind, "payload": payload})
}

#[test]
fn signals_keep_positions_turns_counts_and_setting_history_content_free() {
    let home = Home::new();
    home.lines("codex/sessions/2026/09/24/rollout-signals.jsonl", &[
        codex_line(ts(6,0,0), "session_meta", json!({"id":"signals","cwd":"/work/proj"})),
        codex_line(ts(6,0,1), "response_item", json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Work"}]})),
        codex_line(ts(6,0,2), "turn_context", json!({"model":"gpt-a","collaboration_mode":{"settings":{"reasoning_effort":"high"}}})),
        codex_line(ts(6,0,3), "response_item", json!({"type":"function_call","name":"shell","call_id":"c1","arguments":"{}"})),
        codex_line(ts(6,0,4), "event_msg", json!({"type":"context_compacted","private":"must-not-escape"})),
        codex_line(ts(6,0,5), "turn_context", json!({"model":"gpt-b","collaboration_mode":{"settings":{"reasoning_effort":"max"}}})),
        codex_line(ts(6,0,6), "response_item", json!({"type":"function_call_output","call_id":"c1","output":"ok"})),
        codex_line(ts(6,0,7), "response_item", json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]})),
    ]);
    let built = home.built(BASE + 6 * 3_600_000 + 60_000);
    let page: Value = serde_json::from_str(
        &page(
            &built,
            "signals",
            &Anchor::Last,
            BASE + 6 * 3_600_000 + 60_000,
        )
        .unwrap(),
    )
    .unwrap();
    let entries = page["entries"].as_array().unwrap();
    let signals: Vec<_> = entries.iter().filter(|e| e["k"] == "signal").collect();
    assert_eq!(signals.len(), 5);
    assert!(
        signals
            .iter()
            .all(|e| e["slot"].is_number() && e["turn"].is_string()),
        "{signals:?}"
    );
    let change = signals
        .iter()
        .find(|e| e["signal"]["kind"] == "effort" && e["signal"]["tag"] == "max")
        .unwrap();
    assert_eq!(change["signal"]["previous"], "high");
    assert_eq!(built.sessions["signals"].signals.values().sum::<usize>(), 5);
    let listed: Value = annotations(&home, "signals", "signals");
    assert_eq!(listed["total"], 5);
    assert_eq!(listed["entries"].as_array().unwrap().len(), 5);
    assert_eq!(page["calls"], 1);
    assert!(!page.to_string().contains("must-not-escape"));
}

#[test]
fn a_plain_call_with_a_cut_output_says_where_and_keeps_the_text() {
    let home = Home::new();
    let output = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
    home.lines(
        "codex/sessions/2026/09/24/rollout-plain-cut.jsonl",
        &[
            codex_line(ts(6, 0, 0), "session_meta", json!({"id":"plain-cut","cwd":"/work/proj"})),
            codex_line(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"cargo test\"}"})),
            codex_line(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"call","output":output})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "plain-cut", &Anchor::Last);
    let entry = &page["entries"][0];
    assert_eq!(entry["out"], "line 1\n…19500 tokens truncated…\nline 900\n");
    assert_eq!(
        entry["cut"],
        json!({"by":"codex","original_tokens":24000,"lines":900,"parts":[
            {"text":"line 1\n"},
            {"gap":{"n":19500,"unit":"tokens"}},
            {"text":"\nline 900\n"}
        ]})
    );
    assert!(entry.get("more").is_none());
    let slot = entry["slot"].as_u64().unwrap() as usize;
    let full: Value = extracted_part(&built, "plain-cut", slot, "out").unwrap();
    assert_eq!(full["text"], entry["out"]);
    assert_eq!(full["cut"], entry["cut"]);
    assert_eq!(full["truncated"], false);
}

#[test]
fn a_plain_call_shows_its_items_whole_output_instead_of_the_cut_one() {
    let home = Home::new();
    let cut = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
    home.lines(
        "codex/sessions/2026/09/24/rollout-plain-item.jsonl",
        &[
            codex_line(ts(6, 0, 0), "session_meta", json!({"id":"plain-item","cwd":"/work/proj"})),
            codex_line(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"cargo test\"}"})),
            codex_line(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"call","output":cut})),
            codex_line(ts(6, 1, 750), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"call","command":["/bin/zsh","-lc","cargo test"],"exit_code":0,"aggregated_output":"line 1\nline 2\nline 900\n"}})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "plain-item", &Anchor::Last);
    let entry = &page["entries"][0];
    assert_eq!(entry["out"], "line 1\nline 2\nline 900\n");
    assert!(entry.get("cut").is_none());
    let slot = entry["slot"].as_u64().unwrap() as usize;
    let full: Value = extracted_part(&built, "plain-item", slot, "out").unwrap();
    assert_eq!(full["text"], "line 1\nline 2\nline 900\n");
    assert!(full.get("cut").is_none());
}

#[test]
fn an_operations_collection_cap_is_shown_as_a_gap_in_the_preview_and_in_full() {
    let home = Home::new();
    let head = "h".repeat(PREVIEW_MAX);
    let output = format!("{head}\n... 1048576 bytes omitted ...\nthe end\n");
    home.lines(
        "codex/sessions/2026/09/24/rollout-capped.jsonl",
        &[
            codex_line(ts(6, 0, 0), "session_meta", json!({"id":"capped","cwd":"/work/proj"})),
            codex_line(ts(6, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"tools.exec_command({cmd:'big'})"})),
            codex_line(ts(6, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","big"],"exit_code":0,"aggregated_output":output}})),
            codex_line(ts(6, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":"done"})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "capped", &Anchor::Last);
    let entry = &page["entries"][0];
    let parts = entry["cut"]["parts"].as_array().unwrap();
    assert_eq!(parts[1], json!({"gap": {"n": 1_048_576, "unit": "bytes"}}));
    assert_eq!(parts[2], json!({"text": "the end\n"}));
    assert!(entry["cut"].get("original_tokens").is_none());
    assert!(entry["more"].as_array().unwrap().contains(&json!("out")));
    let slot = entry["slot"].as_u64().unwrap() as usize;
    let full: Value = extracted_part(&built, "capped", slot, "out").unwrap();
    assert_eq!(full["text"], output);
    assert_eq!(full["cut"]["parts"][0], json!({"text": head}));
    assert_eq!(full["truncated"], false);
}

#[test]
fn a_code_mode_result_cut_in_its_json_falls_back_to_its_text_without_the_header() {
    let home = Home::new();
    let header =
        "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\n";
    // Source-shaped synthetic results use the pinned normal success header
    // and a qualifying direct awaited result print, not arbitrary JSON.
    // Whole JSON around the cut: the script's output field is read.
    let whole = format!(
        "{header}{{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"line 1\\nline 2…19500 tokens truncated…line 899\\n\"}}"
    );
    // Cut mid-string and never closed: the text is shown as it is.
    let broken = format!(
        "{header}{{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"line 1\\nline 2…19500 tokens truncated…lin"
    );
    let result = |call: &str, body: &str| json!({"type":"custom_tool_call_output","call_id":call,"output":[{"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},{"type":"input_text","text":body}]});
    let fake =
        json!({"wall_time_seconds":1.0,"session_id":26789,"output":"printed, not a native result"})
            .to_string();
    home.lines(
        "codex/sessions/2026/09/24/rollout-code-cut.jsonl",
        &[
            codex_line(
                ts(6, 0, 0),
                "session_meta",
                json!({"id":"code-cut","cwd":"/work/proj"}),
            ),
            codex_line(
                ts(6, 1, 0),
                "response_item",
                script_call("one", "text(await tools.exec_command({cmd:'one'}));"),
            ),
            codex_line(ts(6, 1, 500), "response_item", result("one", &whole)),
            codex_line(
                ts(6, 2, 0),
                "response_item",
                script_call("two", "text(await tools.exec_command({cmd:'two'}));"),
            ),
            codex_line(ts(6, 2, 500), "response_item", result("two", &broken)),
            codex_line(
                ts(6, 3, 0),
                "response_item",
                script_call("fake", "text('one')"),
            ),
            codex_line(ts(6, 3, 500), "response_item", result("fake", &fake)),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "code-cut", &Anchor::Last);
    let entries: Vec<&Value> = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["k"] == "tool")
        .collect();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[0]["out"],
        "line 1\nline 2…19500 tokens truncated…line 899\n"
    );
    assert_eq!(
        entries[0]["cut"]["parts"][1],
        json!({"gap": {"n": 19500, "unit": "tokens"}})
    );
    assert_eq!(entries[0]["cut"]["original_tokens"], 24000);
    let out = entries[1]["out"].as_str().unwrap();
    assert!(out.starts_with("{\"chunk_id\""), "{out}");
    assert!(!out.contains("Warning: truncated output"));
    assert_eq!(
        entries[1]["cut"]["parts"][1],
        json!({"gap": {"n": 19500, "unit": "tokens"}})
    );
    assert_eq!(entries[1]["cut"]["lines"], 900);
    assert_eq!(entries[2]["name"], "exec");
    assert_eq!(entries[2]["out"], fake);
    assert!(entries[2].get("cut").is_none());
    let slot = &built.tx["code-cut"].slots[entries[2]["slot"].as_u64().unwrap() as usize];
    assert!(
        matches!(&slot.kind, SlotKind::Tool { name, result_source: false, item: None, .. } if name == "exec")
    );
}

#[test]
fn collected_output_is_never_read_as_the_model_facing_cut() {
    // A command that printed a rollout's cut output.
    let printed = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…line 900\n[... omitted 3 of 9 lines ...]\n";
    assert_eq!(
        split_cut(printed, Source::Collected),
        (printed.to_owned(), None)
    );
    // The collection cap's own marker still splits it, and the printed
    // header stays in the text.
    let capped = format!("{printed}head\n... 2048 bytes omitted ...\ntail\n");
    let (plain, cut) = split_cut(&capped, Source::Collected);
    assert_eq!(plain, capped);
    let cut = cut.unwrap();
    assert_eq!((cut.original_tokens, cut.lines), (None, None));
    assert_eq!(cut.parts.len(), 3);
    assert_eq!(cut.parts[1], gap(2048, "bytes", None));
    assert!(
        matches!(&cut.parts[0], Part::Text(head) if head.starts_with("Warning: truncated") && head.ends_with("head"))
    );
    // No marker at the first line either: that form is the model's.
    let first = "... 5 bytes omitted ...\nwhat a command printed\n";
    assert_eq!(
        split_cut(first, Source::Collected),
        (first.to_owned(), None)
    );
}

#[test]
fn unified_execs_frame_comes_before_the_warning_header() {
    // codex-rs core/src/tools/context.rs: `{frame}\n{formatted_truncate_text(..)}`.
    let frame = "Chunk ID: 3f9a1c\nWall time: 4.2100 seconds\nProcess exited with code 0\nOriginal token count: 24000\nOutput:\n";
    let raw = format!(
        "{frame}Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…line 900\n"
    );
    let (plain, cut) = model(&raw);
    assert_eq!(
        plain,
        format!("{frame}line 1\n…19500 tokens truncated…line 900\n")
    );
    assert_eq!(
        cut,
        Some(Cut {
            original_tokens: Some(24000),
            lines: Some(900),
            parts: vec![
                text("line 1\n"),
                gap(19500, "tokens", None),
                text("line 900\n")
            ],
        })
    );
    // No cut: the frame alone is ordinary output, unchanged.
    let plain_run = format!("{frame}hello\n");
    assert_eq!(model(&plain_run), (plain_run.clone(), None));
    // A frame doesn't vouch for a token marker a command printed.
    let echoed = format!("{frame}echo …5 tokens truncated…\n");
    assert_eq!(model(&echoed), (echoed.clone(), None));
    // A frame needs its wall time.
    let bare = "Chunk ID: x\nOutput:\nWarning: truncated output (original token count: 1)\n\nx";
    assert_eq!(model(bare), (bare.to_owned(), None));
}

#[test]
fn a_cap_marker_first_then_a_token_cut_are_two_gaps() {
    // `truncated_output_with_policy`: the warning line, the omission
    // notice, a blank line, then the token-cut text.
    let raw = "Chunk ID: a\nWall time: 1.0000 seconds\nOriginal token count: 5000000\nOutput:\nWarning: truncated output (original token count: 5000000)\n... 1048576 bytes omitted ...\n\nhead…9000 tokens truncated…tail";
    let cut = model(raw).1.unwrap();
    assert_eq!(cut.original_tokens, Some(5_000_000));
    assert_eq!(
        cut.parts,
        vec![
            gap(1_048_576, "bytes", None),
            text("head"),
            gap(9000, "tokens", None),
            text("tail")
        ]
    );
    // The same notice with no token cut, as `format!("{marker}\n{text}")`.
    let notice =
        "Chunk ID: a\nWall time: 1.0000 seconds\nOutput:\n... 77 bytes omitted ...\nrest\n";
    assert_eq!(
        model(notice).1.unwrap().parts,
        vec![gap(77, "bytes", None), text("rest\n")]
    );
}

#[test]
fn a_header_inside_a_script_results_output_field_is_found() {
    let result = "{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"Warning: truncated output (original token count: 7)\\nTotal output lines: 9\\n\\nab…3 tokens truncated…cd\"}";
    let (plain, cut) = model(result);
    assert_eq!(plain, "ab…3 tokens truncated…cd");
    let cut = cut.unwrap();
    assert_eq!((cut.original_tokens, cut.lines), (Some(7), Some(9)));
    assert_eq!(
        cut.parts,
        vec![text("ab"), gap(3, "tokens", None), text("cd")]
    );
    // A result with nothing cut is left as it is.
    let whole = "{\"chunk_id\":\"c\",\"wall_time_seconds\":1.0,\"output\":\"fine\"}";
    assert_eq!(model(whole), (whole.to_owned(), None));
}

#[test]
fn a_multibyte_character_at_the_previews_split_is_kept_whole_or_dropped() {
    // Each side gets 768 bytes: 1 + 3 * 255 = 766 is the last boundary
    // of the head at or before 768, and the tail starts at a boundary
    // at or after len - 768.
    let head = format!("a{}", "€".repeat(400));
    let tail = format!("{}z", "€".repeat(400));
    let (_, cut) = model(&format!(
        "Warning: truncated output (original token count: 1)\n{head}…9 tokens truncated…{tail}"
    ));
    let (json, clipped) = cut.unwrap().json(PREVIEW_MAX);
    assert!(clipped);
    let parts = json["parts"].as_array().unwrap();
    let shown_head = parts[0]["text"]
        .as_str()
        .unwrap()
        .strip_suffix('…')
        .unwrap();
    assert!(
        head.starts_with(shown_head) && shown_head.len() == 766,
        "{}",
        shown_head.len()
    );
    let shown_tail = parts[2]["text"]
        .as_str()
        .unwrap()
        .strip_prefix('…')
        .unwrap();
    assert!(tail.ends_with(shown_tail) && shown_tail.ends_with('z'));
    assert!(
        shown_tail.len() <= 768 && shown_tail.len() >= 766,
        "{}",
        shown_tail.len()
    );
}

#[test]
fn view_all_never_says_semon_cut_a_gap_codex_made() {
    // Each side over half the cap, the whole under it.
    let side = FULL_MAX / 2 + 1024;
    let text = format!(
        "{}\n... 9 bytes omitted ...\n{}",
        "a".repeat(side),
        "b".repeat(1024)
    );
    let value: Value = text_value(&text, Source::Collected, false, FULL_MAX);
    assert_eq!(value["truncated"], false);
    assert_eq!(
        value["cut"]["parts"][0]["text"].as_str().unwrap().len(),
        side
    );
}

#[test]
fn an_item_that_printed_a_cut_output_shows_it_unchanged() {
    let home = Home::new();
    let printed = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
    home.lines(
        "codex/sessions/2026/09/24/rollout-printed.jsonl",
        &[
            codex_line(ts(6, 0, 0), "session_meta", json!({"id":"printed","cwd":"/work/proj"})),
            codex_line(ts(6, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"tools.exec_command({cmd:'jq'})"})),
            codex_line(ts(6, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","jq"],"exit_code":0,"aggregated_output":printed}})),
            codex_line(ts(6, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":"done"})),
            codex_line(ts(6, 4, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"plain","arguments":"{\"cmd\":\"jq\"}"})),
            codex_line(ts(6, 4, 500), "response_item", json!({"type":"function_call_output","call_id":"plain","output":"model saw this"})),
            codex_line(ts(6, 4, 750), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"plain","command":["/bin/zsh","-lc","jq"],"exit_code":0,"aggregated_output":printed}})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "printed", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry["out"], printed);
        assert!(entry.get("cut").is_none(), "{entry}");
        let slot = entry["slot"].as_u64().unwrap() as usize;
        let full: Value = extracted_part(&built, "printed", slot, "out").unwrap();
        assert_eq!(full["text"], printed);
        assert!(full.get("cut").is_none());
    }
}

#[test]
fn a_unified_exec_call_cut_by_codex_is_found_after_its_frame() {
    let home = Home::new();
    let output = "Chunk ID: 3f9a1c\nWall time: 4.2100 seconds\nProcess exited with code 0\nOriginal token count: 24000\nOutput:\nWarning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…\nline 900\n";
    home.lines(
        "codex/sessions/2026/09/24/rollout-framed.jsonl",
        &[
            codex_line(ts(6, 0, 0), "session_meta", json!({"id":"framed","cwd":"/work/proj"})),
            codex_line(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"cargo test\"}"})),
            codex_line(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"call","output":output})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "framed", &Anchor::Last);
    let entry = &page["entries"][0];
    assert_eq!(
        entry["cut"],
        json!({"by":"codex","original_tokens":24000,"lines":900,"parts":[
            {"text":"line 1\n"},
            {"gap":{"n":19500,"unit":"tokens"}},
            {"text":"\nline 900\n"}
        ]})
    );
    assert!(
        !entry["out"]
            .as_str()
            .unwrap()
            .contains("Warning: truncated")
    );
}

#[test]
fn a_printed_warning_line_behind_a_frame_is_kept_unless_its_count_is_the_frames() {
    // A command printed a rollout's cut output; Codex cut nothing.
    let printed = "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\nline 1\n…19500 tokens truncated…line 900\n";
    let with = |count: &str| {
        format!(
            "Chunk ID: a\nWall time: 1.0000 seconds\nProcess exited with code 0\n{count}Output:\n{printed}"
        )
    };
    // The frame's count is the whole output's, not the printed one's.
    let other = with("Original token count: 12\n");
    assert_eq!(model(&other), (other.clone(), None));
    // A frame with no count (an intercepted patch's) can't vouch for it.
    let none = with("");
    assert_eq!(model(&none), (none.clone(), None));
    // Equal counts are Codex's own header.
    let real = with("Original token count: 24000\n");
    let (plain, cut) = model(&real);
    assert!(!plain.contains("Warning: truncated output"));
    assert_eq!(cut.unwrap().original_tokens, Some(24000));
    // With no frame at all (legacy and code-mode forms) the header is
    // Codex's wherever it starts the text.
    assert_eq!(model(printed).1.unwrap().original_tokens, Some(24000));
}

#[test]
fn a_leading_cap_marker_takes_only_its_own_newline_without_a_warning_line() {
    // `format!("{marker}\n{text}")`: a kept output that starts blank.
    let raw = "Chunk ID: a\nWall time: 1.0000 seconds\nOutput:\n... 5 bytes omitted ...\n\nkept\n";
    assert_eq!(
        model(raw).1.unwrap().parts,
        vec![gap(5, "bytes", None), text("\nkept\n")]
    );
    // After a warning line the blank line is Codex's separator.
    let warned = "Chunk ID: a\nWall time: 1.0000 seconds\nOriginal token count: 9\nOutput:\nWarning: truncated output (original token count: 9)\n... 5 bytes omitted ...\n\nkept\n";
    assert_eq!(
        model(warned).1.unwrap().parts,
        vec![gap(5, "bytes", None), text("kept\n")]
    );
}

#[test]
fn codex_diff_keeps_deleted_lines_that_look_like_headers() {
    let (rows, cut, _) = codex_diff_rows("@@ -1 +0,0 @@\n--- note\n", FULL_MAX);
    assert_eq!(rows, json!([["del", "--- note"]]));
    assert!(!cut);
}

#[test]
fn code_mode_exec_errors_and_ambiguous_items_are_not_steps() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    let mut records = vec![codex(
        ts(5, 0, 0),
        "session_meta",
        json!({"id":"overlap","cwd":"/work/proj"}),
    )];
    for (id, script, minute) in [
        ("first", "const first = true;", 1),
        ("second", "const second = true;", 2),
    ] {
        records.push(codex(
            ts(5, minute, 0),
            "response_item",
            json!({"type":"custom_tool_call","call_id":id,"name":"exec","input":script}),
        ));
    }
    records.push(codex(
        ts(5, 3, 0),
        "event_msg",
        json!({"type":"item_completed","item":{"type":"CommandExecution","id":"unowned","command":["/bin/zsh","-lc","pwd"],"exit_code":0,"duration":{"secs":0,"nanos":100000000},"aggregated_output":"/work/proj\n"}}),
    ));
    records.push(codex(
        ts(5, 4, 0),
        "response_item",
        json!({"type":"custom_tool_call_output","call_id":"second","output":"done"}),
    ));
    records.push(codex(
        ts(5, 5, 0),
        "event_msg",
        json!({"type":"item_completed","item":{"type":"FileChange","id":"owned","changes":{"/work/proj/a.rs":{"unified_diff":"@@ -1 +1 @@\n-old\n+new\n"}}}}),
    ));
    records.push(codex(
        ts(5, 6, 0),
        "response_item",
        json!({"type":"custom_tool_call_output","call_id":"first","output":"done"}),
    ));
    records.push(codex(
        ts(5, 7, 0),
        "response_item",
        json!({"type":"custom_tool_call","call_id":"failed","name":"exec","input":"tools.exec_command({cmd:'false'})"}),
    ));
    records.push(codex(
        ts(5, 8, 0),
        "event_msg",
        json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-failed","command":["/bin/zsh","-lc","false"],"exit_code":1,"duration":{"secs":0,"nanos":200000000},"aggregated_output":"failed\n"}}),
    ));
    records.push(codex(
        ts(5, 9, 0),
        "response_item",
        json!({"type":"custom_tool_call_output","call_id":"failed","output":"done"}),
    ));
    records.push(codex(
        ts(5, 10, 0),
        "response_item",
        json!({"type":"custom_tool_call","call_id":"empty","name":"exec","input":"const empty = true;"}),
    ));
    records.push(codex(
        ts(5, 11, 0),
        "response_item",
        json!({"type":"custom_tool_call_output","call_id":"empty","output":"done"}),
    ));
    home.lines("codex/sessions/2026/09/24/rollout-overlap.jsonl", &records);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "overlap", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    // The item that completed while two calls were open has no exact
    // owner: it is no step of its own, so it can't repeat its wrapper.
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(4), Some(1))
    );
    assert!(!entries.iter().any(|entry| entry["arg"] == "pwd"));
    let owned = entries.iter().find(|entry| entry["arg"] == "a.rs").unwrap();
    assert!(owned["script"].is_number());
    let owned_script: Value = extracted_part(
        &built,
        "overlap",
        owned["slot"].as_u64().unwrap() as usize,
        "script",
    )
    .unwrap();
    assert_eq!(owned_script["text"], "const first = true;");
    let failed = entries
        .iter()
        .find(|entry| entry["arg"] == "false")
        .unwrap();
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["exit"], 1);
    assert_eq!(failed["secs"], "0.2s");
    assert_eq!(failed["out"], "failed\n");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["name"] == "exec")
            .count(),
        2
    );
    assert_eq!(entries.iter().filter(|entry| entry["arg"] == "").count(), 2);
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry["in"] == "const second = true;")
            .unwrap()["name"],
        "exec"
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry["in"] == "const empty = true;")
            .unwrap()["name"],
        "exec"
    );
}

#[test]
fn direct_function_call_exec_command_keeps_its_existing_entry() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-old-exec.jsonl",
        &[
            codex(
                ts(6, 0, 0),
                "session_meta",
                json!({"id":"old-exec","cwd":"/work/proj"}),
            ),
            codex(
                ts(6, 1, 0),
                "response_item",
                json!({"type":"function_call","name":"exec_command","call_id":"old-call","arguments":"{\"cmd\":\"git status\"}"}),
            ),
            codex(
                ts(6, 1, 500),
                "response_item",
                json!({"type":"function_call_output","call_id":"old-call","output":"{\"output\":\"## main\\n\",\"metadata\":{\"exit_code\":0}}"}),
            ),
            codex(
                ts(6, 1, 750),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"old-call","command":["/bin/zsh","-lc","git status"],"exit_code":1,"aggregated_output":"failed"}}),
            ),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "old-exec", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "exec_command");
    assert_eq!(entries[0]["arg"], "git status");
    // The item that completed the call holds its whole output (#52).
    assert_eq!(entries[0]["out"], "failed");
    assert_eq!(entries[0]["ok"], false);
    assert_eq!(entries[0]["secs"], "0.5s");
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(1), Some(1))
    );
}

#[test]
fn direct_function_call_apply_patch_keeps_one_entry_with_file_change_item() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-old-patch.jsonl",
        &[
            codex(ts(6, 0, 0), "session_meta", json!({"id":"old-patch","cwd":"/work/proj"})),
            codex(ts(6, 1, 0), "response_item", json!({"type":"function_call","name":"apply_patch","call_id":"patch-call","arguments":"*** Begin Patch\\n*** Update File: a.rs\\n@@ -1 +1 @@\\n-old\\n+new\\n*** End Patch"})),
            codex(ts(6, 1, 250), "event_msg", json!({"type":"item_completed","item":{"type":"FileChange","id":"patch-call","changes":{"/work/proj/a.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\\n-old\\n+new\\n"}}}})),
            codex(ts(6, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"patch-call","output":"{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "old-patch", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "apply_patch");
    assert_eq!(entries[0]["ok"], true);
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(1), Some(0))
    );
}

#[test]
fn an_aborted_code_mode_call_does_not_claim_the_next_turns_items() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-interrupted.jsonl",
        &[
            codex(ts(7, 0, 0), "session_meta", json!({"id":"interrupted","cwd":"/work/proj"})),
            codex(ts(7, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"old","name":"exec","input":"const old = true;"})),
            codex(ts(7, 2, 0), "event_msg", json!({"type":"turn_aborted"})),
            codex(ts(7, 3, 0), "response_item", json!({"type":"custom_tool_call","call_id":"new","name":"exec","input":"const next = true;"})),
            codex(ts(7, 4, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item-new","command":["/bin/zsh","-lc","next"],"exit_code":0,"aggregated_output":"ok"}})),
            codex(ts(7, 5, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"new","output":"done"})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "interrupted", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(2), Some(1))
    );
    let operation = entries.iter().find(|entry| entry["arg"] == "next").unwrap();
    assert_eq!(operation["name"], "exec_command");
    let script: Value = extracted_part(
        &built,
        "interrupted",
        operation["slot"].as_u64().unwrap() as usize,
        "script",
    )
    .unwrap();
    assert_eq!(script["text"], "const next = true;");
}

#[test]
fn code_mode_wrapper_script_error_is_a_failed_tool() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-script-error.jsonl",
        &[
            codex(ts(8, 0, 0), "session_meta", json!({"id":"script-error","cwd":"/work/proj"})),
            codex(ts(8, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"throw new Error('boom')"})),
            codex(ts(8, 2, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script error: boom"}]})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "script-error", &Anchor::Last);
    let entry = &page["entries"][0];
    assert_eq!(entry["name"], "exec");
    assert_eq!(entry["ok"], false);
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(1), Some(1))
    );
}

#[test]
fn an_item_with_no_open_code_mode_call_is_not_a_step() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-unowned.jsonl",
        &[
            codex(ts(9, 0, 0), "session_meta", json!({"id":"unowned","cwd":"/work/proj"})),
            codex(ts(9, 1, 0), "response_item", json!({"type":"function_call","name":"apply_patch","call_id":"patch-call","arguments":"*** Begin Patch\\n*** Update File: a.rs\\n@@ -1 +1 @@\\n-old\\n+new\\n*** End Patch"})),
            codex(ts(9, 1, 250), "event_msg", json!({"type":"item_completed","item":{"type":"FileChange","id":"other-id","changes":{"/work/proj/a.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\\n-old\\n+new\\n"}}}})),
            codex(ts(9, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"patch-call","output":"{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"})),
            codex(ts(9, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"stray","command":["/bin/zsh","-lc","pwd"],"exit_code":1,"aggregated_output":"/work/proj\n"}})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "unowned", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "apply_patch");
    assert_eq!(entries[0]["ok"], true);
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(1), Some(0))
    );
}

#[test]
fn a_script_error_after_its_operations_is_a_failed_step() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-late-error.jsonl",
        &[
            codex(ts(9, 0, 0), "session_meta", json!({"id":"late-error","cwd":"/work/proj"})),
            codex(ts(9, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"await tools.exec_command({cmd:'ls'}); throw new Error('boom')"})),
            codex(ts(9, 2, 0), "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"item","command":["/bin/zsh","-lc","ls"],"exit_code":0,"aggregated_output":"a.rs\n"}})),
            codex(ts(9, 3, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script error: boom"}]})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "late-error", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["arg"], "ls");
    assert_eq!(entries[0]["ok"], true);
    assert_eq!(entries[1]["name"], "exec");
    assert_eq!(entries[1]["ok"], false);
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(2), Some(1))
    );
}

#[test]
fn a_script_error_is_read_from_the_harness_header_only() {
    let home = Home::new();
    let codex = |time: String, kind: &str, payload: Value| json!({"timestamp": time, "type": kind, "payload": payload});
    home.lines(
        "codex/sessions/2026/09/24/rollout-printed-error.jsonl",
        &[
            codex(ts(9, 0, 0), "session_meta", json!({"id":"printed-error","cwd":"/work/proj"})),
            codex(ts(9, 1, 0), "response_item", json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"text('Script error: printed')"})),
            codex(ts(9, 2, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"call","output":[{"type":"input_text","text":"Script completed"},{"type":"input_text","text":"Script error: printed"}]})),
            codex(ts(9, 3, 0), "response_item", json!({"type":"custom_tool_call","call_id":"plain","name":"exec","input":"text('x')"})),
            codex(ts(9, 4, 0), "response_item", json!({"type":"custom_tool_call_output","call_id":"plain","output":"Script error: a plain string is no header"})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "printed-error", &Anchor::Last);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry["ok"] != false));
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(2), Some(0))
    );
}

fn script_call(id: &str, script: &str) -> Value {
    json!({"type":"custom_tool_call","call_id":id,"name":"exec","status":"completed","input":script})
}

/// A script's output that printed one exec result, as
/// `text(JSON.stringify(r))` does.
fn exec_result(id: &str, session: Option<u64>, exit: Option<i64>, output: &str) -> Value {
    let mut result = json!({"chunk_id":"c0ffee","wall_time_seconds":1.0,"original_token_count":1,"output":output});
    if let Some(session) = session {
        result["session_id"] = json!(session);
    }
    if let Some(exit) = exit {
        result["exit_code"] = json!(exit);
    }
    json!({"type":"custom_tool_call_output","call_id":id,"output":[
        {"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},
        {"type":"input_text","text":result.to_string()},
    ]})
}

const WATCH: &str = "const r = await tools.exec_command({cmd:\"make watch\",workdir:\"/work/proj\",yield_time_ms:1000});\ntext(JSON.stringify(r));\n";

fn poll(session: u64, chars: &str) -> String {
    format!(
        "const r = await tools.write_stdin({{session_id:{session},chars:{},yield_time_ms:1000}});\ntext(JSON.stringify(r));\n",
        json!(chars)
    )
}

/// `make watch` outlives its yield as session 4242 and prints "one";
/// three polls with no input print "two", nothing and "three".
fn yielded(sid: &str) -> Vec<Value> {
    let mut records = vec![
        codex_line(
            ts(10, 0, 0),
            "session_meta",
            json!({"id":sid,"cwd":"/work/proj"}),
        ),
        codex_line(ts(10, 1, 0), "response_item", script_call("start", WATCH)),
        codex_line(
            ts(10, 1, 1000),
            "response_item",
            exec_result("start", Some(4242), None, "one\n"),
        ),
    ];
    for (minute, output) in [(2, "two\n"), (3, ""), (4, "three\n")] {
        let id = format!("poll-{minute}");
        records.push(codex_line(
            ts(10, minute, 0),
            "response_item",
            script_call(&id, &poll(4242, "")),
        ));
        records.push(codex_line(
            ts(10, minute, 1000),
            "response_item",
            exec_result(&id, Some(4242), None, output),
        ));
    }
    records
}

/// The last poll: the command exits while it waits, so its completion
/// arrives inside it and its output has an exit code and no session.
fn completed(records: &mut Vec<Value>, chars: &str, aggregated: &str) {
    records.push(codex_line(
        ts(10, 5, 0),
        "response_item",
        script_call("poll-last", &poll(4242, chars)),
    ));
    records.push(codex_line(
        ts(10, 5, 500),
        "event_msg",
        json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-watch","process_id":"4242","command":["/bin/zsh","-lc","make watch"],"cwd":"file:///work/proj","status":"completed","exit_code":0,"duration":{"secs":5,"nanos":0},"aggregated_output":aggregated}}),
    ));
    records.push(codex_line(
        ts(10, 5, 1000),
        "response_item",
        exec_result("poll-last", None, Some(0), "four\n"),
    ));
}

fn tools(page: &Value) -> Vec<Value> {
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["k"] == "tool")
        .cloned()
        .collect()
}

#[test]
fn a_yielded_command_and_its_polls_are_one_step() {
    let home = Home::new();
    let mut records = yielded("watch");
    completed(&mut records, "", "one\ntwo\nthree\nfour\n");
    records.push(codex_line(
        ts(10, 6, 0),
        "response_item",
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"It passed."}]}),
    ));
    home.lines("codex/sessions/2026/09/24/rollout-watch.jsonl", &records);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "watch", &Anchor::Last);
    let steps = tools(&page);
    assert_eq!(steps.len(), 1, "{steps:?}");
    let step = &steps[0];
    assert_eq!(step["name"], "exec_command");
    assert_eq!(step["arg"], "make watch");
    assert_eq!(step["ok"], true);
    assert_eq!(step["exit"], 0);
    assert_eq!(step["secs"], "5.0s");
    assert_eq!(step["cwd"], ".");
    assert_eq!(step["out"], "one\ntwo\nthree\nfour\n");
    assert!(step.get("in").is_none());
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(1), Some(0))
    );
    let slot = step["slot"].as_u64().unwrap() as usize;
    let script: Value = extracted_part(&built, "watch", slot, "script").unwrap();
    assert_eq!(script["text"], WATCH);
    let out: Value = extracted_part(&built, "watch", slot, "out").unwrap();
    assert_eq!(out["text"], "one\ntwo\nthree\nfour\n");
    assert_eq!(out["truncated"], false);
}

#[test]
fn a_yielded_command_that_never_completes_is_one_unfinished_step() {
    let endings: [(&str, Vec<Value>); 4] = [
        ("ends", Vec::new()),
        (
            "aborted",
            vec![codex_line(
                ts(10, 5, 0),
                "event_msg",
                json!({"type":"turn_aborted"}),
            )],
        ),
        (
            "complete",
            vec![codex_line(
                ts(10, 5, 0),
                "event_msg",
                json!({"type":"task_complete"}),
            )],
        ),
        (
            "mid-poll",
            vec![
                codex_line(
                    ts(10, 5, 0),
                    "response_item",
                    script_call("poll-5", &poll(4242, "")),
                ),
                codex_line(ts(10, 5, 1000), "event_msg", json!({"type":"turn_aborted"})),
            ],
        ),
    ];
    for (sid, ending) in endings {
        let home = Home::new();
        let mut records = yielded(sid);
        records.extend(ending);
        home.lines(
            &format!("codex/sessions/2026/09/24/rollout-{sid}.jsonl"),
            &records,
        );
        let built = home.built(BASE + 86_400_000);
        let page = page_of(&built, sid, &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 1, "{sid}: {steps:?}");
        let step = &steps[0];
        assert_eq!(step["name"], "exec_command", "{sid}");
        assert_eq!(step["arg"], "make watch", "{sid}");
        assert_eq!(step["unfinished"], true, "{sid}");
        assert_eq!(step["ok"], false, "{sid}");
        assert_eq!(step["out"], "one\ntwo\nthree\n", "{sid}");
        assert_eq!(
            (page["calls"].as_u64(), page["errors"].as_u64()),
            (Some(1), Some(1)),
            "{sid}"
        );
    }
}

#[test]
fn a_poll_that_sends_input_is_a_step_of_its_own() {
    let home = Home::new();
    let mut records = vec![
        codex_line(
            ts(10, 0, 0),
            "session_meta",
            json!({"id":"sent","cwd":"/work/proj"}),
        ),
        codex_line(ts(10, 1, 0), "response_item", script_call("start", WATCH)),
        codex_line(
            ts(10, 1, 1000),
            "response_item",
            exec_result("start", Some(4242), None, "one\n"),
        ),
        codex_line(
            ts(10, 2, 0),
            "response_item",
            script_call("answer", &poll(4242, "y\n")),
        ),
        codex_line(
            ts(10, 2, 1500),
            "response_item",
            exec_result("answer", Some(4242), None, "ok\n"),
        ),
    ];
    completed(&mut records, "", "one\nok\n");
    home.lines("codex/sessions/2026/09/24/rollout-sent.jsonl", &records);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "sent", &Anchor::Last);
    let steps = tools(&page);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0]["name"], "exec_command");
    assert_eq!(steps[0]["out"], "one\nok\n");
    assert_eq!(steps[0]["exit"], 0);
    assert_eq!(steps[1]["name"], "write_stdin");
    assert_eq!(steps[1]["arg"], "make watch");
    assert_eq!(steps[1]["in"], "y\n");
    assert_eq!(steps[1]["out"], "ok\n");
    assert_eq!(steps[1]["ok"], true);
    assert_eq!(steps[1]["secs"], "1.5s");
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(2), Some(0))
    );
}

#[test]
fn a_script_with_no_operations_shows_its_text_output() {
    let home = Home::new();
    home.lines(
        "codex/sessions/2026/09/24/rollout-printed.jsonl",
        &[
            codex_line(ts(10, 0, 0), "session_meta", json!({"id":"printed","cwd":"/work/proj"})),
            codex_line(ts(10, 1, 0), "response_item", script_call("plain", "text('hello')")),
            codex_line(
                ts(10, 1, 500),
                "response_item",
                json!({"type":"custom_tool_call_output","call_id":"plain","output":[
                    {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
                    {"type":"input_text","text":"hello"},
                ]}),
            ),
            // An orphan poll stays on its native outer exec cell: no
            // observed start exists to establish a yielded/poll relation.
            codex_line(ts(10, 2, 0), "response_item", script_call("orphan", &poll(777, ""))),
            codex_line(
                ts(10, 2, 500),
                "response_item",
                exec_result("orphan", Some(777), None, "still running\n"),
            ),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "printed", &Anchor::Last);
    let steps = tools(&page);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0]["name"], "exec");
    assert_eq!(steps[0]["out"], "hello");
    assert_eq!(steps[1]["name"], "exec");
    assert!(steps[1].get("unfinished").is_none());
    assert_eq!(steps[1]["out"], "still running\n");
    let transcript = &built.tx["printed"];
    assert!(
        transcript
            .slots
            .iter()
            .all(|slot| !matches!(&slot.kind, SlotKind::Yielded { .. } | SlotKind::Sent { .. }))
    );
    let slot = &transcript.slots[steps[1]["slot"].as_u64().unwrap() as usize];
    assert!(matches!(&slot.kind, SlotKind::Tool { name, item: None, .. } if name == "exec"));
    let record = Lines::default()
        .get(&slot.file.as_ref().unwrap().path, slot.offset)
        .unwrap();
    assert_eq!(record["payload"]["call_id"], "orphan");
    assert_eq!(record["payload"]["input"], poll(777, ""));
}

#[test]
fn an_interrupted_code_mode_call_shows_unfinished_after_its_operations() {
    let home = Home::new();
    home.lines(
        "codex/sessions/2026/09/24/rollout-stopped.jsonl",
        &[
            codex_line(ts(11, 0, 0), "session_meta", json!({"id":"stopped","cwd":"/work/proj"})),
            codex_line(
                ts(11, 1, 0),
                "response_item",
                script_call("call", "await tools.exec_command({cmd:'ls'});\nawait tools.exec_command({cmd:'pwd'});\nawait tools.exec_command({cmd:'sleep 600'});"),
            ),
            codex_line(
                ts(11, 2, 0),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"ls","command":["/bin/zsh","-lc","ls"],"exit_code":0,"aggregated_output":"a.rs\n"}}),
            ),
            codex_line(
                ts(11, 3, 0),
                "event_msg",
                json!({"type":"item_completed","item":{"type":"CommandExecution","id":"pwd","command":["/bin/zsh","-lc","pwd"],"exit_code":0,"aggregated_output":"/work/proj\n"}}),
            ),
            codex_line(ts(11, 4, 0), "event_msg", json!({"type":"turn_aborted"})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "stopped", &Anchor::Last);
    let steps = tools(&page);
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert_eq!(steps[0]["arg"], "ls");
    assert_eq!(steps[1]["arg"], "pwd");
    assert_eq!(steps[2]["name"], "exec");
    assert_eq!(steps[2]["unfinished"], true);
    assert_eq!(steps[2]["ok"], false);
    assert_eq!(
        (page["calls"].as_u64(), page["errors"].as_u64()),
        (Some(3), Some(1))
    );
    let turn = built
        .turns
        .iter()
        .rfind(|turn| turn.sid == "stopped")
        .unwrap();
    assert_eq!((turn.end.st, turn.end.why), ("err", "unfinished_step"));
}

#[test]
fn exact_completion_without_a_poll_is_preserved_without_inferred_reuse() {
    let home = Home::new();
    // An open exec wrapper cannot prove reuse of 4242. The old startup
    // can finish during it. Preserve distinct old/new commands and outputs;
    // independent native invocation identity would be needed to prove reuse.
    let mut records = yielded("reused");
    records.extend([
        codex_line(
            ts(10, 6, 0),
            "response_item",
            script_call("date", "const r = await tools.exec_command({cmd:\"date\"});\ntext(JSON.stringify(r));"),
        ),
        codex_line(
            ts(10, 6, 500),
            "event_msg",
            json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-old","process_id":"4242","command":["/bin/zsh","-lc","make watch"],"exit_code":0,"aggregated_output":"one\ntwo\nthree\n"}}),
        ),
        codex_line(
            ts(10, 6, 1000),
            "response_item",
            exec_result("date", None, Some(0), "Thu\n"),
        ),
    ]);
    home.lines("codex/sessions/2026/09/24/rollout-reused.jsonl", &records);
    // A poll during which some other process completes: that item is
    // the poll's own operation, and the poll isn't folded, but what it
    // printed of 4242 is 4242's.
    let mut stray = yielded("stray");
    stray.extend([
        codex_line(ts(10, 6, 0), "response_item", script_call("poll-6", &poll(4242, ""))),
        codex_line(
            ts(10, 6, 500),
            "event_msg",
            json!({"type":"item_completed","item":{"type":"CommandExecution","id":"exec-other","process_id":"999","command":["/bin/zsh","-lc","echo other"],"exit_code":0,"aggregated_output":"other\n"}}),
        ),
        codex_line(
            ts(10, 6, 1000),
            "response_item",
            exec_result("poll-6", Some(4242), None, "five\n"),
        ),
    ]);
    home.lines("codex/sessions/2026/09/24/rollout-stray.jsonl", &stray);
    let built = home.built(BASE + 86_400_000);
    // The stray poll's own chunk is still the command's output.
    for (sid, arg, other, out, unfinished) in [
        ("reused", "make watch", "date", "one\ntwo\nthree\n", false),
        (
            "stray",
            "make watch",
            "echo other",
            "one\ntwo\nthree\nfive\n",
            true,
        ),
    ] {
        let page = page_of(&built, sid, &Anchor::Last);
        let steps = tools(&page);
        assert_eq!(steps.len(), 2, "{sid}: {steps:?}");
        assert_eq!(steps[0]["arg"], arg, "{sid}");
        assert_eq!(steps[0]["unfinished"] == true, unfinished, "{sid}");
        if !unfinished {
            assert_eq!(steps[0]["ok"], true, "{sid}");
        }
        assert_eq!(steps[0]["out"], out, "{sid}");
        if sid == "reused" {
            // No independent nested native identity exists for date.
            // Its distinct source/result stays on the outer exec cell.
            assert_eq!(steps[1]["name"], "exec");
            assert!(steps[1]["ok"].is_null());
            assert_eq!(steps[1]["out"], "Thu\n");
            let slot = &built.tx[sid].slots[steps[1]["slot"].as_u64().unwrap() as usize];
            assert!(
                matches!(&slot.kind, SlotKind::Tool { name, item: None, .. } if name == "exec")
            );
            let record = Lines::default()
                .get(&slot.file.as_ref().unwrap().path, slot.offset)
                .unwrap();
            assert_eq!(record["payload"]["call_id"], "date");
            assert_eq!(
                record["payload"]["input"],
                "const r = await tools.exec_command({cmd:\"date\"});\ntext(JSON.stringify(r));"
            );
        } else {
            assert_eq!(steps[1]["ok"], true, "{sid}");
            assert_eq!(steps[1]["name"], "exec_command", "{sid}");
            assert_eq!(steps[1]["arg"], other, "{sid}");
        }
    }
}

#[test]
fn a_script_with_looped_operations_keeps_its_outer_cells_visible() {
    let home = Home::new();
    // It starts and polls in a loop with reassignment. The printed result
    // cannot be assigned to one nested operation, so the wrapper stays
    // visible with its native output. A comment names another call.
    let script = "let r = await tools.exec_command({cmd:\"make watch\",yield_time_ms:1000});\n// then tools.write_stdin({session_id, chars:\"q\"}) to quit\nwhile (r.session_id && Date.now() < end) {\n  r = await tools.write_stdin({session_id: r.session_id, chars: \"\"});\n}\ntext(JSON.stringify(r));\n";
    let mut records = vec![
        codex_line(
            ts(10, 0, 0),
            "session_meta",
            json!({"id":"looped","cwd":"/work/proj"}),
        ),
        codex_line(ts(10, 1, 0), "response_item", script_call("start", script)),
        codex_line(
            ts(10, 1, 1000),
            "response_item",
            exec_result("start", Some(4242), None, "one\n"),
        ),
        codex_line(
            ts(10, 2, 0),
            "response_item",
            script_call("poll-2", &poll(4242, "")),
        ),
        codex_line(
            ts(10, 2, 1000),
            "response_item",
            exec_result("poll-2", Some(4242), None, "two\n"),
        ),
    ];
    completed(&mut records, "", "one\ntwo\nfour\n");
    home.lines("codex/sessions/2026/09/24/rollout-looped.jsonl", &records);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "looped", &Anchor::Last);
    let steps = tools(&page);
    let transcript = built.tx.get("looped").unwrap();
    assert!(steps.len() > 1, "{steps:?}");
    assert!(transcript.slots.iter().any(|slot| matches!(
        &slot.kind,
        SlotKind::Tool { name, .. } if name == "exec"
    )));
    assert!(steps.iter().any(|step| {
        step["name"] == "exec"
            && step["out"].as_str().is_some_and(|out| {
                out.contains("\"session_id\":4242") && out.contains("\"output\":\"one\\n\"")
            })
    }));
    assert!(
        transcript
            .slots
            .iter()
            .all(|slot| !matches!(&slot.kind, SlotKind::Yielded { .. }))
    );
}

#[test]
fn polls_past_the_bound_fold_in_and_mark_the_output_cut() {
    let home = Home::new();
    let mut records = vec![
        codex_line(
            ts(12, 0, 0),
            "session_meta",
            json!({"id":"many","cwd":"/work/proj"}),
        ),
        codex_line(ts(12, 1, 0), "response_item", script_call("start", WATCH)),
        codex_line(
            ts(12, 1, 10),
            "response_item",
            exec_result("start", Some(4242), None, "one\n"),
        ),
    ];
    for poll_index in 0..=crate::events::POLLS_MAX {
        let id = format!("poll-{poll_index}");
        let at = 20 + poll_index as i64 * 20;
        records.push(codex_line(
            ts(12, 2, at),
            "response_item",
            script_call(&id, &poll(4242, "")),
        ));
        records.push(codex_line(
            ts(12, 2, at + 10),
            "response_item",
            exec_result(&id, Some(4242), None, ""),
        ));
    }
    home.lines("codex/sessions/2026/09/24/rollout-many.jsonl", &records);
    let built = home.built(BASE + 86_400_000);
    let page = page_of(&built, "many", &Anchor::Last);
    let steps = tools(&page);
    assert_eq!(steps.len(), 1, "{} steps", steps.len());
    assert_eq!(steps[0]["out"], "one\n\n…");
    assert!(steps[0]["more"].as_array().unwrap().contains(&json!("out")));
    let slot = steps[0]["slot"].as_u64().unwrap() as usize;
    let out: Value = extracted_part(&built, "many", slot, "out").unwrap();
    assert_eq!(out["text"], "one\n\n…");
    assert_eq!(out["truncated"], true);
}

#[test]
fn a_character_across_the_output_bound_is_never_split() {
    let home = Home::new();
    for (sid, head) in [("wide", PREVIEW_MAX - 1), ("huge", FULL_MAX - 1)] {
        home.lines(
            &format!("codex/sessions/2026/09/24/rollout-{sid}.jsonl"),
            &[
                codex_line(
                    ts(13, 0, 0),
                    "session_meta",
                    json!({"id":sid,"cwd":"/work/proj"}),
                ),
                codex_line(ts(13, 1, 0), "response_item", script_call("start", WATCH)),
                codex_line(
                    ts(13, 1, 10),
                    "response_item",
                    exec_result("start", Some(4242), None, &"a".repeat(head)),
                ),
                codex_line(
                    ts(13, 2, 0),
                    "response_item",
                    script_call("poll", &poll(4242, "")),
                ),
                codex_line(
                    ts(13, 2, 10),
                    "response_item",
                    exec_result("poll", Some(4242), None, "é tail"),
                ),
            ],
        );
    }
    let built = home.built(BASE + 86_400_000);
    let out_of = |sid: &str| {
        let page = page_of(&built, sid, &Anchor::Last);
        let step = tools(&page).remove(0);
        let slot = step["slot"].as_u64().unwrap() as usize;
        let full: Value = extracted_part(&built, sid, slot, "out").unwrap();
        (step, full)
    };
    // The two bytes of "é" straddle PREVIEW_MAX: the preview stops
    // before it; the whole output is under FULL_MAX.
    let (step, full) = out_of("wide");
    assert_eq!(step["out"], format!("{}…", "a".repeat(PREVIEW_MAX - 1)));
    assert!(step["more"].as_array().unwrap().contains(&json!("out")));
    assert_eq!(
        full["text"],
        format!("{}é tail", "a".repeat(PREVIEW_MAX - 1))
    );
    assert_eq!(full["truncated"], false);
    // They straddle FULL_MAX: "View all" stops before it.
    let (step, full) = out_of("huge");
    assert_eq!(step["out"], format!("{}…", "a".repeat(PREVIEW_MAX)));
    assert_eq!(full["text"], format!("{}…", "a".repeat(FULL_MAX - 1)));
    assert_eq!(full["truncated"], true);
}

#[test]
fn every_field_passes_content_as_data() {
    let home = Home::new();
    let payload =
        "<img src=x onerror=alert(1)><script>alert(2)</script><iframe src=javascript:alert(3)>";
    // A session key comes from a file name, which can't hold a slash.
    let key = "<img src=x onerror=alert(4)>";
    home.write(
        "proc/1/stat",
        &format!("1 (claude) {}\n", ["0"; 19].join(" ") + " 7"),
    );
    home.write(
        "claude/sessions/1.json",
        &json!({"pid": 1, "sessionId": key, "procStart": 7, "status": "busy", "name": payload})
            .to_string(),
    );
    home.lines(
        &format!("claude/projects/-work-proj/{key}.jsonl"),
        &[
            claude(key, ts(4, 0, 0), "user", json!({"origin": {"kind": "human"}, "gitBranch": payload, "message": {"role": "user", "content": payload}})),
            claude(key, ts(4, 0, 1), "user", json!({"message": {"role": "user", "content": format!("note {payload}")}})),
            said(key, ts(4, 0, 2), json!([{"type": "thinking", "thinking": payload, "signature": "s"}])),
            said(key, ts(4, 1, 0), json!([{"type": "text", "text": payload}])),
            said(key, ts(4, 2, 0), json!([{"type": "tool_use", "id": "x1", "name": payload, "input": {"a": payload, "b": payload}}])),
            result(key, ts(4, 2, 1), "x1", payload, false),
            said(key, ts(4, 3, 0), json!([{"type": "tool_use", "id": "x2", "name": "Edit", "input": {"file_path": "/work/proj/a.rs", "old_string": payload, "new_string": format!("{payload}!")}}])),
            result(key, ts(4, 3, 1), "x2", "ok", false),
            said(key, ts(4, 4, 0), json!([{"type": "tool_use", "id": "x3", "name": "SendMessage", "input": {"to": payload, "message": payload}}])),
            claude(key, ts(4, 4, 1), "user", json!({"toolUseResult": {"success": false}, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x3", "content": "no such peer"}]}})),
            said(key, ts(4, 5, 0), json!([{"type": "tool_use", "id": "x4", "name": "AskUserQuestion", "input": {"questions": [{"question": payload, "header": "h", "multiSelect": false, "options": [{"label": payload}, {"label": "b"}]}]}}])),
            claude(key, ts(4, 5, 1), "user", json!({"toolUseResult": {"answers": {payload.to_string(): payload}}, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x4", "content": "answered"}]}})),
            said(key, ts(4, 6, 0), json!([{"type": "tool_use", "id": "x5", "name": "Agent", "input": {"description": payload, "prompt": payload}}])),
            result(key, ts(4, 7, 0), "x5", payload, false),
            said(key, ts(4, 8, 0), json!([{"type": "tool_use", "id": "x6", "name": "Bash", "input": {"command": payload}}])),
        ],
    );
    home.lines(
        &format!("claude/projects/-work-proj/{key}/subagents/agent-sub.jsonl"),
        &[
            claude(
                "sub",
                ts(4, 6, 0),
                "user",
                json!({"message": {"role": "user", "content": format!("brief {payload}")}}),
            ),
            said(
                "sub",
                ts(4, 6, 30),
                json!([{"type": "text", "text": payload}]),
            ),
        ],
    );
    home.write(
        &format!("claude/projects/-work-proj/{key}/subagents/agent-sub.meta.json"),
        &json!({"description": payload, "toolUseId": "x5"}).to_string(),
    );
    home.lines(
        "codex/sessions/2026/09/24/rollout-cx.jsonl",
        &[
            json!({"timestamp": ts(4, 0, 0), "type": "session_meta", "payload": {"id": "cx", "cwd": "/work/proj", "git": {"branch": payload}}}),
            json!({"timestamp": ts(4, 0, 1), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": payload}]}}),
            json!({"timestamp": ts(4, 0, 2), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": format!("<recommended_plugins>{payload}</recommended_plugins>")}]}}),
            json!({"timestamp": ts(4, 0, 3), "type": "response_item", "payload": {"type": "reasoning", "summary": [{"type": "summary_text", "text": payload}]}}),
        ],
    );
    let now = BASE + 4 * 3_600_000 + 9 * 60_000;
    let built = home.built(now);
    let entries = |sid: &str| {
        page_of(&built, sid, &Anchor::Last)["entries"]
            .as_array()
            .unwrap()
            .clone()
    };

    let lane = entries(key);
    let find = |kind: &str| {
        lane.iter()
            .filter(|entry| entry["k"] == kind)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(find("u")[0]["text"], format!("note {payload}"));
    assert_eq!(find("think")[0]["text"], payload);
    assert_eq!(find("a")[0]["text"], payload);
    let tools = find("tool");
    assert_eq!(tools[0]["name"], payload);
    assert_eq!(tools[0]["arg"], payload);
    assert!(tools[0]["in"].as_str().unwrap().contains(payload));
    assert_eq!(tools[0]["out"], payload);
    assert_eq!(
        tools[1]["diff"],
        json!([
            ["del", format!("-{payload}")],
            ["add", format!("+{payload}!")]
        ])
    );
    assert_eq!(tools[2]["arg"], payload, "a running tool's summary");

    let sub = entries("sub");
    assert_eq!(sub.last().unwrap()["ret"]["to"], key);
    let codex = entries("cx");
    assert_eq!(
        codex[0]["text"], payload,
        "a Codex prompt that starts with < is a prompt"
    );
    assert_eq!(
        codex[1]["label"], "recommended_plugins",
        "a harness label is only a known tag"
    );
    assert_eq!(codex[2]["text"], payload);

    // The failed send's addressee has its own transcript: its send.
    let unsent = entries(&format!("unsent:{payload}"));
    assert_eq!(unsent[0]["text"], "No activity in these logs");
    assert_eq!(unsent[1]["k"], "h");

    // Native semantic strings remain data, including source-backed asks.
    let records = built.sources.reader();
    let mut handoffs = serde_json::to_value(&built.handoffs).unwrap();
    for h in handoffs.as_array_mut().unwrap() {
        if let Some(brief) = built.ask_briefs.get(h["id"].as_str().unwrap()) {
            h["brief"] = json!(brief.read(&records).unwrap());
        }
    }
    records.finish().unwrap();
    let model = json!({"sessions":built.sessions,"handoffs":handoffs});
    let session = &model["sessions"][key];
    assert_eq!(session["name"], payload);
    // A subagent's name is its description on one line, cut at 80.
    assert!(
        model["sessions"]["sub"]["name"]
            .as_str()
            .unwrap()
            .starts_with("<img src=x onerror=alert(1)><script>")
    );
    assert_eq!(
        model["sessions"]["cx"]["branch"],
        Value::Null,
        "no repo, no branch"
    );
    assert_eq!(session["activity"][1], payload);
    let handoff = |kind: &str| {
        model["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|handoff| handoff["kind"] == kind)
            .unwrap()
            .clone()
    };
    assert_eq!(handoff("ask")["brief"], payload);
    let relay = handoff("relay");
    assert_eq!(
        (relay["target"].as_str(), relay["brief"].as_str()),
        (Some(payload), Some(payload))
    );
    let question = handoff("toyou");
    assert_eq!(
        (question["brief"].as_str(), question["answer"][0].as_str()),
        (Some(payload), Some(payload))
    );
    let spawn = handoff("spawn");
    assert_eq!(
        (spawn["brief"].as_str(), spawn["result"].as_str()),
        (Some(payload), Some(payload))
    );
}

#[test]
fn a_command_longer_than_its_summary_shows_as_input() {
    let long = format!("cd /work/proj && {}", "cargo test ".repeat(20));
    let input = json!({"command": long});
    let arg = arg_summary("Bash", &input, None, None);
    assert!(arg.ends_with('…'));
    assert_eq!(tool_in("Bash", &input, &arg), Some(long));
    let multi = json!({"command": "set -e\nmake"});
    assert_eq!(
        tool_in("Bash", &multi, &arg_summary("Bash", &multi, None, None)).as_deref(),
        Some("set -e\nmake")
    );
    let short = json!({"command": "ls -la"});
    assert_eq!(tool_in("Bash", &short, "ls -la"), None);
}

#[test]
fn selected_output_preserves_claude_and_codex_results_without_optional_blocks() {
    let home = Home::new();
    let text = "selected native result".repeat(100);
    home.lines(
        "claude/projects/-work-proj/selected-claude.jsonl",
        &[
            said(
                "selected-claude",
                ts(1, 0, 0),
                json!([{
                    "type":"tool_use","id":"plain-call","name":"Bash","input":{"command":"plain"}
                }]),
            ),
            result("selected-claude", ts(1, 0, 500), "plain-call", &text, false),
        ],
    );
    home.lines(
        "codex/sessions/2026/09/24/rollout-selected-codex.jsonl",
        &[
            codex_line(ts(1, 0, 0), "session_meta", json!({"id":"selected-codex","cwd":"/work/proj"})),
            codex_line(ts(1, 1, 0), "response_item", json!({"type":"function_call","call_id":"plain-call","name":"exec_command","arguments":"{\"cmd\":\"plain\"}"})),
            codex_line(ts(1, 1, 500), "response_item", json!({"type":"function_call_output","call_id":"plain-call","output":text})),
        ],
    );
    let built = home.built(BASE + 86_400_000);
    for sid in ["selected-claude", "selected-codex"] {
        let transcript = &built.tx[sid];
        let slot = transcript
            .slots
            .iter()
            .find(|slot| matches!(&slot.kind, SlotKind::Tool { .. }))
            .unwrap();
        let output = selected_output(&mut Lines::default(), slot).unwrap();
        assert_eq!(output["text"], text, "{sid}");
        assert_eq!(output["truncated"], false, "{sid}");
        assert!(output.get("blocks").is_none(), "{sid}");
        assert!(output.get("blocks_truncated").is_none(), "{sid}");
        assert!(output.get("blocks_omitted").is_none(), "{sid}");
    }
}

#[test]
fn codex_native_fixture_selected_output_keeps_bounded_native_blocks() {
    const FIXTURE: &str = include_str!(
        "../../../../tests/fixtures/compatibility/codex-native-a9b0815/parser-cases.jsonl"
    );
    const MANIFEST: &str =
        include_str!("../../../../tests/fixtures/compatibility/codex-native-a9b0815/manifest.json");
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let expected = manifest["expected_blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["call_id"] == "image-unknown")
        .unwrap();
    let home = Home::new();
    home.write(
        "codex/sessions/2026/09/24/rollout-parser-cases.jsonl",
        FIXTURE,
    );
    let built = home.built(BASE + 86_400_000);
    let transcript = built.tx.get("parser-cases").unwrap();
    let call_id = expected["call_id"].as_str().unwrap();
    let call_offset = manifest["expected_calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == call_id)
        .unwrap()["offset"]
        .as_u64()
        .unwrap();
    let slot = transcript
        .slots
        .iter()
        .find(|slot| slot.offset == call_offset)
        .unwrap_or_else(|| panic!("visible image-unknown wrapper at {call_offset}"));
    assert!(matches!(&slot.kind, SlotKind::Tool { .. }));
    let output = selected_output(&mut Lines::default(), slot).unwrap();
    let blocks = output["blocks"].as_array().unwrap();
    let expected_blocks = expected["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), expected_blocks.len());
    for (actual, expected) in blocks.iter().zip(expected_blocks) {
        assert_eq!(actual["ordinal"], expected["ordinal"]);
        assert_eq!(actual["type"], expected["type"]);
        assert_eq!(actual["role"], expected["role"]);
        if let Some(fallback) = expected.get("fallback") {
            assert_eq!(actual["fallback"], *fallback);
        }
    }
    assert_eq!(blocks[1]["fallback"], "[image]");
    assert_eq!(blocks[2]["fallback"], "[unsupported output block]");
    let text_bytes = output["text"].as_str().unwrap().len()
        + blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .map(str::len)
            .sum::<usize>();
    assert!(text_bytes <= 64 * 1024);
    let serialized = output.to_string();
    assert!(!serialized.contains("synthetic-base64-secret"));
    assert!(!serialized.contains("synthetic private payload"));
    assert!(output.get("blocks_truncated").is_none());
    assert!(output.get("blocks_omitted").is_none());

    for (call_id, printed_text, printed_session) in [
        ("fake-exec-json", "printed object, not result", 7171),
        ("fake-poll-json", "printed object, not poll result", 95583),
    ] {
        let call_offset = manifest["expected_calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == call_id)
            .unwrap()["offset"]
            .as_u64()
            .unwrap();
        let slot = transcript
            .slots
            .iter()
            .find(|slot| slot.offset == call_offset)
            .unwrap();
        assert!(matches!(
            &slot.kind,
            SlotKind::Tool {
                result_source: false,
                ..
            }
        ));
        let printed = selected_output(&mut Lines::default(), slot).unwrap();
        let text = printed["text"].as_str().unwrap();
        assert!(text.contains(printed_text));
        assert!(text.contains(&format!("\"session_id\":{printed_session}")));
        assert_ne!(text, printed_text);
    }
}

#[test]
fn codex_reference_conformance_selected_output_keeps_native_coordinates() {
    const FIXTURE: &str = include_str!(
        "../../../../tests/fixtures/compatibility/codex-reference-a9b0815/rollout-legacy.jsonl"
    );
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/compatibility/codex-reference-a9b0815/manifest.json"
    ))
    .unwrap();
    let home = Home::new();
    let path = home.write("codex/sessions/2026/09/24/rollout-reference.jsonl", FIXTURE);
    let built = home.built(BASE + 86_400_000);
    let transcript = built.tx.get("legacy-reference").unwrap();
    let mut lines = Lines::default();
    let mut offset = 0_u64;
    let mut tested = 0;
    for native_line in FIXTURE.split_inclusive('\n') {
        let record = Lines::default().get(&path, offset).unwrap();
        let kind = record["payload"]["type"].as_str();
        if matches!(kind, Some("function_call" | "custom_tool_call")) {
            let call_id = record["payload"]["call_id"].as_str().unwrap();
            assert_eq!(
                manifest["semon_adapter_contract"]["model_call_ids"][tested],
                call_id
            );
            let slot = transcript
                .slots
                .iter()
                .find(|slot| slot.offset == offset)
                .unwrap();
            assert_eq!(slot.file.as_ref().unwrap().path, path);
            assert_eq!(slot.block, 0);
            let SlotKind::Tool {
                reply: Some(reply),
                result_source,
                item,
                ..
            } = &slot.kind
            else {
                panic!("{call_id} must remain a visible tool wrapper");
            };
            assert!(!result_source);
            assert!(item.is_none());
            assert_eq!(reply.o, offset + native_line.len() as u64);
            assert_eq!(reply.b, 0);
            assert!(reply.o < FIXTURE.len() as u64);
            let native_output = Lines::default().get(&path, reply.o).unwrap();
            assert_eq!(native_output["payload"]["call_id"], call_id);
            let selected = selected_output(&mut lines, slot).unwrap();
            if call_id == "start-26789" {
                assert_eq!(selected["text"], native_output["payload"]["output"]);
                assert!(selected.get("blocks").is_none());
            } else {
                let blocks = selected["blocks"].as_array().unwrap();
                let parts = native_output["payload"]["output"].as_array().unwrap();
                assert_eq!(blocks.len(), 3);
                for (ordinal, (block, part)) in blocks.iter().zip(parts).enumerate() {
                    assert_eq!(block["ordinal"], ordinal);
                    assert_eq!(block["type"], part["type"]);
                    assert_eq!(block["text"], part["text"]);
                    assert_eq!(block["role"], if ordinal == 0 { "header" } else { "body" });
                }
                assert_eq!(
                    blocks[0]["text"],
                    "Script completed\nWall time 0.1 seconds\nOutput:\n"
                );
                let new: Value = serde_json::from_str(blocks[1]["text"].as_str().unwrap()).unwrap();
                let poll: Value =
                    serde_json::from_str(blocks[2]["text"].as_str().unwrap()).unwrap();
                assert_eq!(new["session_id"], 95583);
                assert!(poll.get("session_id").is_none());
                assert_eq!(poll["output"], "old poll");
                assert_eq!(
                    selected["text"],
                    format!(
                        "{}\n{}",
                        parts[1]["text"].as_str().unwrap(),
                        parts[2]["text"].as_str().unwrap()
                    )
                );
                assert!(selected.get("blocks_truncated").is_none());
                assert!(selected.get("blocks_omitted").is_none());
            }
            assert!(string_value_bytes(&selected) <= 64 * 1024);
            tested += 1;
        }
        offset += native_line.len() as u64;
    }
    assert_eq!(tested, 2);
    assert_eq!(offset, FIXTURE.len() as u64);
    assert_eq!(
        transcript
            .slots
            .iter()
            .filter(|slot| matches!(&slot.kind, SlotKind::Tool { .. }))
            .count(),
        2
    );
    assert!(!transcript.slots.iter().any(|slot| matches!(
        &slot.kind,
        SlotKind::Yielded { .. } | SlotKind::Sent { .. } | SlotKind::Operation { .. }
    )));
    assert!(lines.bytes <= FIXTURE.len() as u64);
}

#[test]
fn selected_output_blocks_obey_count_and_text_budgets() {
    let output = Value::Array(
        (0..OUTPUT_BLOCKS_MAX + 2)
            .map(|ordinal| json!({"type":"input_text","text":format!("block-{ordinal}")}))
            .collect(),
    );
    let (blocks, truncated, omitted) = selected_output_blocks(&output, 5).unwrap();
    let blocks = blocks.as_array().unwrap();
    assert_eq!(blocks.len(), OUTPUT_BLOCKS_MAX);
    assert!(truncated);
    assert_eq!(omitted, 2);
    let text_bytes = blocks
        .iter()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .map(str::len)
        .sum::<usize>();
    assert!(text_bytes <= 5);
}

#[test]
fn selected_output_text_stays_within_shared_utf8_budget() {
    let ascii = "a".repeat(SELECTED_OUTPUT_MAX + 1);
    let selected = selected_text_value(&ascii, Source::Plain, false, SELECTED_OUTPUT_MAX);
    let flat = selected["text"].as_str().unwrap();
    assert_eq!(flat.len(), SELECTED_OUTPUT_MAX);
    assert!(flat.ends_with('…'));
    assert_eq!(selected["truncated"], true);

    let multibyte = "é".repeat(SELECTED_OUTPUT_MAX / 2 + 1);
    let selected = selected_text_value(&multibyte, Source::Plain, false, SELECTED_OUTPUT_MAX);
    let flat = selected["text"].as_str().unwrap();
    assert!(flat.len() <= SELECTED_OUTPUT_MAX);
    assert!(flat.is_char_boundary(flat.len()));
    assert!(flat.ends_with('…'));

    let selected = selected_text_value("flat", Source::Plain, false, SELECTED_OUTPUT_MAX);
    let remaining = SELECTED_OUTPUT_MAX.saturating_sub(string_value_bytes(&selected));
    let output = json!([{"type":"input_text","text":"block".repeat(SELECTED_OUTPUT_MAX)}]);
    let (blocks, truncated, _) = selected_output_blocks(&output, remaining).unwrap();
    let block_text_bytes = blocks
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .map(str::len)
        .sum::<usize>();
    assert!(truncated);
    assert!(selected["text"].as_str().unwrap().len() + block_text_bytes <= SELECTED_OUTPUT_MAX);
}

#[test]
fn selected_native_cut_parts_report_local_ascii_and_utf8_clipping() {
    for unit in ["a", "é"] {
        let side = unit.repeat(20 * 1024 / unit.len());
        let body = format!("{side}…9 tokens truncated…{side}");
        let native = format!(
            "Warning: truncated output (original token count: 24000)\nTotal output lines: 900\n\n{body}"
        );
        // The flat native text fits. Its duplicated structured parts need
        // Semon clipping under the shared selected-output budget.
        assert!(body.len() < SELECTED_OUTPUT_MAX);
        let selected = selected_text_value(&native, Source::Model, false, SELECTED_OUTPUT_MAX);
        assert_eq!(selected["text"], body);
        assert_eq!(selected["truncated"], true);
        assert_eq!(selected["cut"]["by"], "codex");
        assert_eq!(selected["cut"]["original_tokens"], 24000);
        assert_eq!(selected["cut"]["lines"], 900);
        assert_eq!(
            selected["cut"]["parts"][1],
            json!({"gap":{"n":9,"unit":"tokens"}})
        );
        let parts = selected["cut"]["parts"].as_array().unwrap();
        let part_bytes = parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .map(str::len)
            .sum::<usize>();
        assert!(selected["text"].as_str().unwrap().len() + part_bytes <= SELECTED_OUTPUT_MAX);
        assert!(string_value_bytes(&selected) <= SELECTED_OUTPUT_MAX);
        for part in parts.iter().filter_map(|part| part["text"].as_str()) {
            assert!(part.ends_with('…'));
            assert!(part.len() < side.len());
        }
    }
}

#[test]
fn selected_native_gap_without_local_clipping_is_not_truncated() {
    let native = "Warning: truncated output (original token count: 24)\nTotal output lines: 9\n\nhead…9 tokens truncated…tail";
    let selected = selected_text_value(native, Source::Model, false, SELECTED_OUTPUT_MAX);
    assert_eq!(selected["text"], "head…9 tokens truncated…tail");
    assert_eq!(selected["truncated"], false);
    assert_eq!(selected["cut"]["by"], "codex");
    assert_eq!(
        selected["cut"]["parts"],
        json!([
            {"text":"head"}, {"gap":{"n":9,"unit":"tokens"}}, {"text":"tail"}
        ])
    );
}

#[test]
fn selected_large_native_cut_reserves_metadata_in_ascii_and_utf8_budget() {
    for unit in ["a", "é"] {
        let side = unit.repeat(SELECTED_OUTPUT_MAX / unit.len());
        let native = format!(
            "Warning: truncated output (original token count: 50000)\nTotal output lines: 900\n\n{side}…9 tokens truncated…{side}"
        );
        let selected = selected_text_value(&native, Source::Model, false, SELECTED_OUTPUT_MAX);
        let flat = selected["text"].as_str().unwrap();
        assert!(flat.ends_with('…'));
        assert!(flat.len() < SELECTED_OUTPUT_MAX);
        assert!(string_value_bytes(&selected) <= SELECTED_OUTPUT_MAX);
        assert_eq!(selected["truncated"], true);
        assert_eq!(selected["cut"]["by"], "codex");
        assert_eq!(selected["cut"]["original_tokens"], 50000);
        assert_eq!(
            selected["cut"]["parts"][1],
            json!({"gap":{"n":9,"unit":"tokens"}})
        );
        let parts = selected["cut"]["parts"].as_array().unwrap();
        let part_bytes = parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .map(str::len)
            .sum::<usize>();
        assert!(flat.len() + part_bytes <= SELECTED_OUTPUT_MAX);
    }
}

/// Prints every session's last page of a fixture home written by
/// `tests/ui/fixture.mjs`, to check the fixture without a browser.
#[test]
#[ignore = "reads SEMON_SAMPLE_HOME, written by tests/ui/fixture.mjs"]
fn print_sample_tx() {
    let root = PathBuf::from(env::var("SEMON_SAMPLE_HOME").unwrap());
    let now: i64 = env::var("SEMON_SAMPLE_NOW").unwrap().parse().unwrap();
    let options = Options {
            source: crate::Options::default().source,
        claude_home: root.join("claude"),
        claude_json: root.join(".claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join("tx-index.json"),
        all: false,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    let mut cache = EventCache::default();
    let built = build_query(&options, &mut cache, &mut false, &mut Texts::default(), now).unwrap();
    let mut all = serde_json::Map::new();
    for sid in built.tx.keys() {
        let page: Value =
            serde_json::from_str(&page(&built, sid, &Anchor::Last, now).unwrap()).unwrap();
        all.insert(sid.clone(), page);
    }
    println!("{}", Value::Object(all));
}
