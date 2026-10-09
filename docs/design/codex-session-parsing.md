# Codex session display and event classification

Session display names are separate from native session identity. An ordinary
Codex subagent uses a recorded title, then the task segment of `agent_path`,
then its first non-harness user prompt (one line, at most 80 characters).
Without those facts it is named `Codex subagent`. `agent_nickname` is agent
metadata, not a task title. Approval reviews retain their explicit kind and
`Approval review` name. These display rules do not change trace identity,
session keys, occurrence ordering or retained source bytes.

A task path also takes precedence over a nickname when matching a parent's
`spawn_agent` call. Legacy logs with no path can still use their nickname for
that relationship. A match must remain unique among both calls and children;
ambiguous relationships stay unplaced. No parent is inferred from a title.

The viewer classifies events by their native type, independently of text or
channel. `function_call`, `custom_tool_call`, `local_shell_call` and
`tool_search_call` are tools. `tool_search_output` resolves its exact call id
and displays its returned tool definitions.

Completed `McpToolCall` and `DynamicToolCall` items are tools even without a
legacy call record. They retain their source arguments/results, duration and
structured outcome. Exact call ids correlate legacy mirrors without duplicate
steps. A uniquely open code-mode wrapper owns its nested calls; its successful
wrapper is represented by those steps. Failure and unfinished wrappers remain
visible. Unknown ownership does not create a guessed parent.

Only explicit Codex `response_item/reasoning` records and Claude `thinking`
blocks become thinking slots. Unknown item kinds and misplaced annotations
are not interpreted as thinking based on a `summary` field or a fallback.
Incomplete native connector identities are ignored, and absent outcome
evidence remains unknown. Native tool errors use structured error/status
fields, never words found in the output.

The event index and cached descriptions are versioned so existing viewers
rebuild derived metadata on reopening. Regression tests use source-shaped
synthetic records, with private payloads replaced. They establish parser
behavior, not a new native harness version or runtime lifecycle guarantee.
