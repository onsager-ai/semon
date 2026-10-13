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

Code-mode output arrays remain source-backed: the event index stores offsets, not copied output payloads. A yielded exec result is accepted only from one known text body, optionally preceded by the recognized Script completed header. Unknown block types, non-text blocks and multiple bodies keep the wrapper visible as its own step. The selected output expansion may add outputBlocks, retaining each block's ordinal, type and header/body role; text shares the existing 64 KiB selected-output budget, at most 128 blocks are returned, and image/base64 or unknown payload fields are omitted. Image and unknown blocks carry visible fallback labels.

Terminal attribution requires one unambiguous source operation with unique unquoted literal object keys and literal data whose exact awaited result is printed directly, optionally through JSON.stringify, or through one literal assignment followed by printing that same variable; argument expressions, quoted/computed keys, additional statements and control flow are not inferred. A literal write_stdin session id can narrow a poll to that exact previously observed session, and any session id in its single decoded result must match the literal target. A valid completed result without a session id can join only when the exact target's CommandExecution completion arrived during that poll. A variable target, mixed or repeated command calls, arbitrary printed JSON, or multi-body output does not create a relationship by order or by choosing the only open session. A CommandExecution process_id remains an exact terminal relationship when it matches a recorded yielded session, except when an explicit unified_exec_startup item overlaps the one open single-exec source, which identifies the current startup. Parser version 25 invalidates and rebuilds these derived associations on reopen; retained slot projection version 6 preserves this source qualification and rebuilds older recipes.

The codex-native-a9b0815 conformance fixture is source-shaped synthetic data, not a runtime capture. Its manifest pins the inspected upstream source revision and leaves version/build unverified; it also records that Legacy and Paginated durable-carrier behavior was not runtime-qualified by this fixture.

The pinned CodeMode success carrier inserts a `Script completed` header block before its output body; only one header followed by one text body qualifies for a yielded relation. The header uses either the normal wall-time line or the optional finite host/overhead timing line, whose overhead may be slightly negative due to millisecond quantization. `direct-81001` keeps an unwrapped scalar only as a compatibility case, and `future_text` is a deliberately unsupported synthetic block kind.
