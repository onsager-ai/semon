//! Projection of Claude Code transcript records into Semon's semantic region.
//!
//! Claude Code's `message.content` is a list of typed blocks, and (unlike
//! Codex) an assistant record can carry `text`, `thinking` and `tool_use`
//! blocks together in one record. The design doc
//! (`docs/design/trace-identity-and-occurrences.md`) settled that
//! classification is per block, not per record, and this module implements
//! that: [`classify_record`] returns zero or more projected blocks, each
//! carrying its own position (`block_index`) within the record's own content
//! array.
//!
//! The projection table mirrors every ruling already made for the Codex
//! adapter (see `crates/semon-codex/src/lib.rs::semantic_core`):
//!
//! | Source | Projects as | `authored_by` |
//! |---|---|---|
//! | `user`, string content, not `isSidechain`, not `isMeta`, not a known harness injection, not tag-shaped | `intent` | `human` |
//! | `user`, string content, `isSidechain` (and not a known harness injection) | `intent` | `agent` |
//! | `user`, string content, tag-shaped (`<name...`) but not a *known* harness injection | `intent` | `unknown` |
//! | `user`, list content, `text` blocks, not `isMeta`, not tag-shaped | `intent` per block | `human` |
//! | `user`, list content, `text` blocks, tag-shaped but not a known harness injection | `intent` per block | `unknown` |
//! | `user`, list content, `tool_result` blocks | not projected | — |
//! | `assistant`, `text` block | `outcome` per block | `agent` |
//! | `assistant`, `tool_use` block | `action`, structural only | `agent` |
//! | `assistant`, `thinking` block | not projected | — |
//! | `isMeta`, or content beginning `<system-reminder>`, `<command-name>`, `<local-command`, `Caveat:`, `<task-notification>` | not projected | — |
//!
//! **`human` requires positive evidence, not just "not a prefix we know
//! about".** The original version of this table treated anything that
//! wasn't one of four known harness prefixes as `human`, which
//! `docs/design/trace-identity-and-occurrences.md`'s authorship section had
//! already rejected for exactly this reason: a chain of exclusions "silently
//! widens every time the harness invents a new kind of non-human string."
//! Measured against the full real corpus: of the occurrences that first
//! version labeled `human`, 64% actually began with an XML-style tag the
//! four-prefix list did not know about (`<task-notification>` — now added to
//! the known list — plus `<command-message>`, `<bash-input>`,
//! `<bash-stdout>`, none of which are known yet). So a string that is
//! tag-shaped (`<` followed by an ASCII letter, per [`looks_like_tag`]) but
//! not one of the *known* harness injections is never silently folded into
//! `human`; it projects as [`semon_store::AuthoredBy::Unknown`] instead —
//! visible and countable, per the design doc's own rule. Refining those
//! specific tags is left to a follow-up, not this adapter.
//!
//! Whether `thinking` and harness content should ever be classified is a
//! genuinely open question tracked in #10; this adapter is deliberately
//! consistent with Codex rather than resolving it here.

use semon_store::AuthoredBy;
use serde_json::{Value, json};

use crate::AdapterError;

/// The maximum number of blocks a single record may project. `sequence` is
/// encoded as `line_ordinal * 1024 + block_index`, so a `block_index` at or
/// past this limit would collide with the next line's block 0 — see
/// [`crate::sequence_for`].
pub const MAX_BLOCKS_PER_RECORD: usize = semon_store::CLAUDE_MAX_BLOCKS_PER_RECORD as usize;

/// Harness-injected framing prefixes, identical in spirit to Codex's
/// `developer`-role exclusion: text the harness inserted, never the human's
/// or the agent's own words. Checked against any string that would otherwise
/// classify as `intent` (top-level string content and list-content `text`
/// blocks alike), since the harness injects into user turns, never assistant
/// ones.
///
/// This list is deliberately *not* the only test applied: measured against
/// the full real corpus, these four prefixes alone missed
/// `<task-notification>` (1,375 occurrences misclassified as `human` before
/// this was added) as well as other tag-shaped harness strings this build
/// does not yet know by name (`<command-message>`, `<bash-input>`,
/// `<bash-stdout>`). Rather than keep growing this list one census at a
/// time, [`looks_like_tag`] catches the *shape* and routes anything tag-like
/// but unrecognized to [`semon_store::AuthoredBy::Unknown`] instead of
/// `human` — see the module docs.
const HARNESS_INJECTION_PREFIXES: [&str; 5] = [
    "<system-reminder>",
    "<command-name>",
    "<local-command",
    "Caveat:",
    "<task-notification>",
];

/// One block of one record that projects into the semantic region.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedBlock {
    /// This block's position within the record's own `message.content`
    /// array (or `0` for string content, which is treated as a single
    /// block).
    pub block_index: u32,
    /// The semantic core value, e.g. `{"kind": "intent", "content": "..."}`.
    pub semantic_core: Value,
    /// Who or what authored this block's content.
    pub authored_by: AuthoredBy,
}

fn is_harness_injection(text: &str) -> bool {
    HARNESS_INJECTION_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// Whether `text` has the shape of an XML/HTML-style tag: `<` immediately
/// followed by an ASCII letter. This is a shape test, not a membership test
/// against [`HARNESS_INJECTION_PREFIXES`] — it exists precisely to catch
/// harness-injected strings this build does not recognize by name yet (see
/// the module docs' authorship section). `<3` and `</foo>` are not
/// tag-shaped by this definition (the character after `<` is not a letter),
/// so they are not swept into `unknown` on the strength of a bare `<`.
fn looks_like_tag(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next() == Some('<') && chars.next().is_some_and(|next| next.is_ascii_alphabetic())
}

/// Classifies one string that would otherwise be a candidate for `intent`
/// content, returning `None` when it must not be projected at all.
///
/// `human` is asserted only on positive evidence (plain text, not
/// tag-shaped) — never as the fallback for "not a harness prefix we
/// happen to know about". A tag-shaped string that isn't a *known* harness
/// injection becomes [`AuthoredBy::Unknown`], visible and countable rather
/// than silently folded into `human` (see the module docs).
fn classify_intent_text(text: &str, is_sidechain: bool) -> Option<AuthoredBy> {
    if text.is_empty() || is_harness_injection(text) {
        return None;
    }
    if is_sidechain {
        // A subagent's own task prompt is authored by the orchestrating
        // (principal) agent, never the human — see the design doc's
        // decision record for #21. This holds regardless of the text's
        // shape: it is agent-authored by construction, not by inference
        // from its content.
        return Some(AuthoredBy::Agent);
    }
    if looks_like_tag(text) {
        return Some(AuthoredBy::Unknown);
    }
    Some(AuthoredBy::Human)
}

fn checked_block_index(index: usize) -> Result<u32, AdapterError> {
    if index >= MAX_BLOCKS_PER_RECORD {
        return Err(AdapterError::TooManyBlocks(index));
    }
    Ok(index as u32)
}

/// Extracts a `tool_use` block's file-path field, if it has one, and applies
/// the semantic region's path rule to it. Checked in this order: `file_path`,
/// `path`, `notebook_path` — the field names actually used by Claude Code's
/// built-in file-oriented tools. Anything else (e.g. `Bash`'s `command`) has
/// no path field, so `path` is `null`.
///
/// The tool's `input` body is otherwise never read here: this is the
/// structural-only projection the Codex work already settled — the command
/// string, file contents, and search queries stay in the forensic region
/// only.
fn tool_use_path(block: &Value, cwd: &str) -> Value {
    let Some(input) = block.get("input").and_then(Value::as_object) else {
        return Value::Null;
    };
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(raw_path) = input.get(key).and_then(Value::as_str) {
            return Value::String(semon_codex::apply_path_rule(raw_path, cwd));
        }
    }
    Value::Null
}

/// Classifies one already-parsed JSONL record into zero or more projected
/// blocks, per the table on this module.
///
/// `cwd` is the record's own `cwd` field (each Claude Code record carries
/// one), used only for the `tool_use` path rule.
pub fn classify_record(record: &Value) -> Result<Vec<ProjectedBlock>, AdapterError> {
    // The blanket exclusion: `isMeta` is a record-level fact (see the
    // design doc's census), so it is checked once, before any per-block
    // logic, rather than threaded into every row below.
    if record
        .get("isMeta")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(Vec::new());
    }

    let record_type = record.get("type").and_then(Value::as_str).unwrap_or("");
    let Some(message) = record.get("message") else {
        return Ok(Vec::new());
    };
    let content = message.get("content").unwrap_or(&Value::Null);
    let cwd = record.get("cwd").and_then(Value::as_str).unwrap_or("");

    match (record_type, content) {
        ("user", Value::String(text)) => classify_user_string(record, text),
        ("user", Value::Array(blocks)) => classify_user_list(blocks),
        ("assistant", Value::Array(blocks)) => classify_assistant_list(blocks, cwd),
        _ => Ok(Vec::new()),
    }
}

fn classify_user_string(record: &Value, text: &str) -> Result<Vec<ProjectedBlock>, AdapterError> {
    let is_sidechain = record
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(authored_by) = classify_intent_text(text, is_sidechain) else {
        return Ok(Vec::new());
    };
    Ok(vec![ProjectedBlock {
        block_index: 0,
        semantic_core: json!({"kind": "intent", "content": text}),
        authored_by,
    }])
}

fn classify_user_list(blocks: &[Value]) -> Result<Vec<ProjectedBlock>, AdapterError> {
    let mut projected = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let block_index = checked_block_index(index)?;
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        // `tool_result` blocks, and anything else not listed in the
        // projection table (e.g. `image`), are deliberately not projected.
        if block_type != "text" {
            continue;
        }
        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
        // List-content text blocks have no sidechain-task-prompt carve-out
        // in the projection table (unlike top-level string content) — see
        // the module docs' table.
        let Some(authored_by) = classify_intent_text(text, false) else {
            continue;
        };
        projected.push(ProjectedBlock {
            block_index,
            semantic_core: json!({"kind": "intent", "content": text}),
            authored_by,
        });
    }
    Ok(projected)
}

fn classify_assistant_list(
    blocks: &[Value],
    cwd: &str,
) -> Result<Vec<ProjectedBlock>, AdapterError> {
    let mut projected = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let block_index = checked_block_index(index)?;
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                projected.push(ProjectedBlock {
                    block_index,
                    semantic_core: json!({"kind": "outcome", "content": text}),
                    authored_by: AuthoredBy::Agent,
                });
            }
            "tool_use" => {
                let action = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let path = tool_use_path(block, cwd);
                projected.push(ProjectedBlock {
                    block_index,
                    semantic_core: json!({"kind": "action", "action": action, "path": path}),
                    authored_by: AuthoredBy::Agent,
                });
            }
            // `thinking` is deliberately not projected — consistent with
            // Codex, not a new decision here. See #10 for the open question
            // of whether thinking/harness content should ever be captured.
            _ => {}
        }
    }
    Ok(projected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_string_prompt_projects_as_intent() {
        let record = json!({
            "type": "user",
            "isSidechain": false,
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "do the thing"},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_index, 0);
        assert_eq!(
            blocks[0].semantic_core,
            json!({"kind": "intent", "content": "do the thing"})
        );
        assert_eq!(blocks[0].authored_by, AuthoredBy::Human);
    }

    #[test]
    fn sidechain_string_prompt_is_agent_authored() {
        let record = json!({
            "type": "user",
            "isSidechain": true,
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "run the subtask"},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].authored_by, AuthoredBy::Agent);
    }

    #[test]
    fn is_meta_string_is_never_projected() {
        let record = json!({
            "type": "user",
            "isMeta": true,
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "some meta framing"},
        });
        assert!(classify_record(&record).unwrap().is_empty());
    }

    #[test]
    fn harness_injection_prefixes_are_never_projected() {
        for prefix in HARNESS_INJECTION_PREFIXES {
            let record = json!({
                "type": "user",
                "cwd": "/work/repo",
                "message": {"role": "user", "content": format!("{prefix} rest of the text")},
            });
            assert!(
                classify_record(&record).unwrap().is_empty(),
                "expected no projection for prefix {prefix}"
            );
        }
    }

    #[test]
    fn task_notification_is_a_known_harness_injection_not_human() {
        let record = json!({
            "type": "user",
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "<task-notification>\n<task-id>abc</task-id>\n</task-notification>"},
        });
        assert!(classify_record(&record).unwrap().is_empty());
    }

    #[test]
    fn unrecognized_tag_shaped_content_projects_as_unknown_not_human() {
        let record = json!({
            "type": "user",
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "<some-new-tag>surprise harness shape</some-new-tag>"},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].authored_by, AuthoredBy::Unknown);
    }

    #[test]
    fn plain_text_starting_with_angle_bracket_non_letter_is_still_human() {
        // "<3" and similar are not tag-shaped (no letter immediately after
        // `<`), so a bare `<` must not sweep ordinary text into `unknown`.
        let record = json!({
            "type": "user",
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "<3 this refactor, thanks!"},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].authored_by, AuthoredBy::Human);
    }

    #[test]
    fn tag_shaped_list_content_text_block_projects_as_unknown() {
        let record = json!({
            "type": "user",
            "cwd": "/work/repo",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "<bash-stdout>some captured output</bash-stdout>"}
            ]},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].authored_by, AuthoredBy::Unknown);
    }

    #[test]
    fn sidechain_task_prompt_stays_agent_even_if_it_were_tag_shaped() {
        // Sidechain authorship is asserted by construction (the orchestrator
        // wrote it), not inferred from shape, so it must not be diverted to
        // `unknown` just because it happens to look tag-shaped.
        let record = json!({
            "type": "user",
            "isSidechain": true,
            "cwd": "/work/repo",
            "message": {"role": "user", "content": "<not-actually-a-harness-tag>do the task</not-actually-a-harness-tag>"},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].authored_by, AuthoredBy::Agent);
    }

    #[test]
    fn tool_result_list_blocks_are_not_projected() {
        let record = json!({
            "type": "user",
            "cwd": "/work/repo",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": []}
            ]},
        });
        assert!(classify_record(&record).unwrap().is_empty());
    }

    #[test]
    fn assistant_text_and_tool_use_project_with_sequential_block_indices() {
        let record = json!({
            "type": "assistant",
            "cwd": "/work/repo",
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "doing it"},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}},
            ]},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_index, 0);
        assert_eq!(
            blocks[0].semantic_core,
            json!({"kind": "outcome", "content": "doing it"})
        );
        assert_eq!(blocks[1].block_index, 1);
        assert_eq!(
            blocks[1].semantic_core,
            json!({"kind": "action", "action": "Bash", "path": null})
        );
        assert!(blocks.iter().all(|b| b.authored_by == AuthoredBy::Agent));
    }

    #[test]
    fn assistant_thinking_block_is_not_projected() {
        let record = json!({
            "type": "assistant",
            "cwd": "/work/repo",
            "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "let me consider..."},
            ]},
        });
        assert!(classify_record(&record).unwrap().is_empty());
    }

    #[test]
    fn tool_use_path_rewrites_under_cwd_and_marks_external() {
        let record = json!({
            "type": "assistant",
            "cwd": "/work/repo",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read",
                 "input": {"file_path": "/work/repo/src/main.rs"}},
                {"type": "tool_use", "id": "t2", "name": "Read",
                 "input": {"file_path": "/etc/secrets"}},
            ]},
        });
        let blocks = classify_record(&record).unwrap();
        assert_eq!(
            blocks[0].semantic_core,
            json!({"kind": "action", "action": "Read", "path": "src/main.rs"})
        );
        assert_eq!(
            blocks[1].semantic_core,
            json!({"kind": "action", "action": "Read", "path": "<external>"})
        );
    }

    #[test]
    fn too_many_blocks_in_one_record_is_an_error() {
        let blocks: Vec<Value> = (0..MAX_BLOCKS_PER_RECORD + 1)
            .map(|i| json!({"type": "text", "text": format!("block {i}")}))
            .collect();
        let record = json!({
            "type": "assistant",
            "cwd": "/work/repo",
            "message": {"role": "assistant", "content": blocks},
        });
        assert!(matches!(
            classify_record(&record),
            Err(AdapterError::TooManyBlocks(_))
        ));
    }
}
