#!/usr/bin/env python3
"""Local-only, bounded ingestion for private Claude Code and Codex sessions.

This module never sends data anywhere. ``prepare_one`` copies exact source
bytes into the gitignored private directory and writes a separate sanitized,
unlabelled projection for local review and independent annotation.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import io
import json
import math
import os
import re
import secrets
import stat
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable


MAX_SOURCE_BYTES = 10_000_000
MAX_LEADING_CONTROL_RECORDS = 128
MAX_TEXT_CHARS = 1_200
MIN_SOURCE_AGE_SECONDS = 24 * 60 * 60
THERMAL_LIMIT_MILLIDEGREES = 85_000
PRIVATE_ROOT = Path(__file__).resolve().parent / "private"
THERMAL_LOG = Path("/home/marvin/.cache/thermal-watch/temps.log")
HWMON_ROOT = Path("/sys/class/hwmon")

SAFE_STOP_REASONS = {
    "end_turn", "tool_use", "max_tokens", "stop_sequence", "refusal",
    "abort", "cancelled", "turn_complete",
}
SAFE_STATUSES = {
    "success", "error", "running", "pending", "complete", "cancelled",
    "blocked", "unknown",
}
STATUS_ALIASES = {
    "ok": "success", "succeeded": "success", "completed": "complete",
    "finished": "complete", "failed": "error", "failure": "error",
    "timeout": "error", "timed_out": "error", "queued": "pending",
    "started": "running", "in_progress": "running",
}
CLAUDE_CONTROL_TYPES = {
    "session_start", "file-history-snapshot", "queue-operation", "summary",
    "custom-title", "hook", "attachment", "progress", "system",
}
CODEX_ROOT_TYPES = {"session_meta", "response_item", "event_msg"}
CODEX_CONTROL_TYPES = {
    "turn_context", "token_count", "compaction", "compacted",
    "session_meta", "shutdown_complete", "deprecation_notice",
}
CODEX_PAYLOAD_TYPES = {
    "message", "user_message", "agent_message", "function_call", "tool_call",
    "tool_use", "function_call_output", "tool_result", "error",
    "reasoning", "agent_reasoning", "exec_command_begin", "exec_command_end",
    "command_execution", "task_complete", "turn_complete", "turn_started",
    "turn_aborted", "mcp_tool_call", "mcp_tool_result",
}

PEM_RE = re.compile(
    r"-----BEGIN [A-Z0-9 ]+-----.*?-----END [A-Z0-9 ]+-----", re.I | re.S
)
JWT_RE = re.compile(
    r"(?<![A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?![A-Za-z0-9_-])"
)
AUTH_HEADER_RE = re.compile(
    r"(?im)\b(?:authorization|proxy-authorization)\s*:\s*(?:bearer|basic)\s+[^\s,;]+"
)
SECRET_ASSIGN_RE = re.compile(
    r"(?i)\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|"
    r"auth(?:orization)?|secret|password|passwd|credential|client[_-]?secret)"
    r"\s*[:=]\s*(?:bearer\s+)?[^\s,;]+"
)
QUOTED_ASSIGN_RE = re.compile(
    r'''(?is)(?:"[^"\r\n]{1,96}"\s*:\s*"(?:\\.|[^"\\])*"|'''
    r'''\'[^\'\r\n]{1,96}\'\s*(?::|=)\s*\'(?:\\.|[^\'\\])*\')'''
)
ENV_OBJECT_RE = re.compile(
    r'''(?is)(?:"(?:env|environment|variables)"|'(?:env|environment|variables)')\s*:\s*\{[^{}]*\}'''
)
KNOWN_TOKEN_RE = re.compile(
    r"(?i)\b(?:AKIA[0-9A-Z]{16}|(?:sk|pk|rk|ghp|github_pat|xox[baprs])[-_][A-Za-z0-9_-]{8,})\b"
)
ASSIGNMENT_RE = re.compile(r"\b[A-Za-z_][A-Za-z0-9_]{0,63}\s*=\s*[^\s,;]+")
EMAIL_RE = re.compile(r"(?i)\b[A-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b")
URL_RE = re.compile(r"(?i)\bhttps?://[^\s<>\"']+")
WINDOWS_PATH_RE = re.compile(r"(?i)(?<![A-Za-z0-9])(?:[A-Z]:\\|\\\\)[^\s<>\"']+")
ABSOLUTE_PATH_RE = re.compile(r"(?<![A-Za-z0-9])/(?:[^\s<>\"']+/)*[^\s<>\"']+")
RELATIVE_PATH_RE = re.compile(
    r"(?<![A-Za-z0-9])(?:\.{1,2}/|(?:[A-Za-z0-9_.-]+/)+)[A-Za-z0-9_.-]+(?:\.[A-Za-z0-9_.-]+)?"
)
LONG_TOKEN_RE = re.compile(r"(?<![A-Za-z0-9])[A-Za-z0-9_+/=-]{32,}(?![A-Za-z0-9])")
CONTROL_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")
TOOL_NAME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9_.:-]{0,79}$")
PROMPT_TAG_RE = re.compile(
    r"(?is)<(?P<tag>system-reminder|environment_context|user_instructions|skills_instructions|"
    r"permissions\s+instructions|instructions?|system|developer|hook_context)\b[^>]*>.*?</(?P=tag)\s*>"
)
PROMPT_COMMENT_RE = re.compile(
    r"(?is)<!--\s*BEGIN\s+(?:agent-workbench|system|developer|hook)[^>]*-->.*?<!--\s*END\s+(?:agent-workbench|system|developer|hook)[^>]*-->"
)
CODE_FENCE_RE = re.compile(r"(?s)```.*?```|~~~.*?~~~")
OPEN_CODE_FENCE_RE = re.compile(r"(?ms)^\s*(?:```|~~~).*\Z")
PROMPT_HEADING_RE = re.compile(
    r"(?ims)^\s*#{1,6}\s*(?:AGENTS\.md(?:\s+instructions?)?|system prompt|system instructions|developer instructions|"
    r"user instructions|permissions instructions|skills instructions|instructions|environment context)\b.*\Z"
)
PLAIN_PROMPT_HEADER_RE = re.compile(
    r"(?ims)^\s*(?:SYSTEM PROMPT|SYSTEM INSTRUCTIONS|DEVELOPER INSTRUCTIONS|AGENT INSTRUCTIONS)\s*:.*\Z"
)
ENVIRONMENT_DUMP_RE = re.compile(
    r"(?ims)^\s*(?:environment (?:variables|context|dump)|exported environment)\s*:?.*\Z"
)
OPEN_PROMPT_TAG_RE = re.compile(
    r"(?is)<\s*(?:system-reminder|environment_context|user_instructions|skills_instructions|"
    r"permissions\s+instructions|instructions?|system|developer|hook_context)\b[^>]*>.*\Z"
)
SUSPICIOUS_RESIDUAL_RE = re.compile(
    r"(?i)(?:<\s*/?\s*(?:system-reminder|environment_context|user_instructions|skills_instructions|"
    r"permissions\s+instructions|instructions?|system|developer|hook_context)\b|"
    r"AGENTS\.md|-----BEGIN [A-Z0-9 ]+-----|"
    r"\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|authorization|password|passwd|secret)\s*[:=]|"
    r"\bbearer\s+[A-Za-z0-9._~+/-]{12,})"
)


class PipelineError(Exception):
    """Safe-to-report pipeline failure identified only by a short code."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


class ExcludedRecord(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _now() -> datetime:
    return datetime.now(timezone.utc)


def _canonical_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _ensure_private_root(private_root: Path = PRIVATE_ROOT) -> Path:
    root = Path(private_root)
    try:
        if root.is_symlink():
            raise PipelineError("private_root_symlink")
        if root.exists() and not root.is_dir():
            raise PipelineError("private_root_not_directory")
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(root, 0o700)
        if not stat.S_ISDIR(root.stat().st_mode):
            raise PipelineError("private_root_not_directory")
    except PipelineError:
        raise
    except OSError:
        raise PipelineError("private_root_unavailable") from None
    return root


def _private_key(private_root: Path) -> bytes:
    key_path = private_root / ".pseudonym-key"
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(key_path, flags)
    except FileNotFoundError:
        try:
            fd = os.open(
                key_path,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
                0o600,
            )
        except FileExistsError:
            fd = os.open(key_path, flags)
        else:
            key = secrets.token_bytes(32)
            with os.fdopen(fd, "wb") as stream:
                stream.write(key)
                stream.flush()
                os.fsync(stream.fileno())
            os.chmod(key_path, 0o600)
            return key
    except OSError:
        raise PipelineError("pseudonym_key_unavailable") from None
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size != 32:
            raise PipelineError("pseudonym_key_invalid")
        os.chmod(key_path, 0o600)
        with os.fdopen(fd, "rb") as stream:
            key = stream.read(33)
    except OSError:
        os.close(fd)
        raise PipelineError("pseudonym_key_unavailable") from None
    if len(key) != 32:
        raise PipelineError("pseudonym_key_invalid")
    return key


def _opaque_id(key: bytes, namespace: bytes, value: str) -> str:
    digest = hmac.new(key, namespace + b"\0" + value.encode("utf-8"), hashlib.sha256).hexdigest()
    return digest[:32]


def _path_id(path: Path, key: bytes) -> str:
    try:
        identity = os.fsencode(path.absolute())
    except OSError:
        raise PipelineError("path_unavailable") from None
    digest = hmac.new(key, b"inventory\0" + identity, hashlib.sha256).hexdigest()
    return digest[:32]


def _iter_jsonl_paths(source: Path) -> Iterable[Path]:
    try:
        info = source.lstat()
    except OSError:
        raise PipelineError("source_unavailable") from None
    if stat.S_ISREG(info.st_mode):
        yield source
        return
    if not stat.S_ISDIR(info.st_mode):
        raise PipelineError("source_not_regular")
    for base, dirs, files in os.walk(source, followlinks=False):
        dirs[:] = [name for name in dirs if not (Path(base) / name).is_symlink()]
        for name in files:
            candidate = Path(base) / name
            if candidate.suffix.lower() == ".jsonl":
                yield candidate


def inventory(source: str | Path, *, private_root: Path = PRIVATE_ROOT) -> dict[str, Any]:
    """Inspect file metadata only; never open or print native contents."""
    root = _ensure_private_root(private_root)
    key = _private_key(root)
    now_ts = _now().timestamp()
    rows: list[dict[str, Any]] = []
    exclusions: Counter[str] = Counter()
    for path in _iter_jsonl_paths(Path(source)):
        try:
            info = path.lstat()
        except OSError:
            exclusions["source_unavailable"] += 1
            continue
        codes: list[str] = []
        if not stat.S_ISREG(info.st_mode):
            codes.append("source_not_regular")
        if info.st_size >= MAX_SOURCE_BYTES:
            codes.append("source_too_large")
        age_seconds = max(0, int(now_ts - info.st_mtime))
        if age_seconds < MIN_SOURCE_AGE_SECONDS:
            codes.append("source_not_24h_stable")
        if "subagents" in {part.lower() for part in path.parts}:
            codes.append("subagent_path")
        exclusions.update(codes)
        rows.append({
            "candidate_ref": _path_id(path, key),
            "bytes": info.st_size,
            "age_seconds": age_seconds,
            "eligible_by_metadata": not codes,
            "exclusion_codes": codes,
        })
    return {"candidates": rows, "exclusion_counts": dict(sorted(exclusions.items()))}


def _entropy(value: str) -> float:
    if not value:
        return 0.0
    counts = Counter(value)
    size = len(value)
    return -sum((count / size) * math.log2(count / size) for count in counts.values())


def _replace(pattern: re.Pattern[str], text: str, replacement: str, counts: Counter[str], code: str) -> str:
    updated, count = pattern.subn(replacement, text)
    counts[code] += count
    return updated


def _sanitize_url(match: re.Match[str], counts: Counter[str]) -> str:
    from urllib.parse import urlsplit

    raw = match.group(0).rstrip(".,);]")
    try:
        parsed = urlsplit(raw)
        host = parsed.hostname
        if not host:
            counts["url"] += 1
            return "[redacted-url]"
        port = f":{parsed.port}" if parsed.port else ""
        counts["url_path"] += bool(parsed.path and parsed.path != "/")
        counts["url_query"] += bool(parsed.query or parsed.fragment)
        counts["url_credentials"] += bool(parsed.username or parsed.password)
        return f"{parsed.scheme.lower()}://{host.lower()}{port}" + ("/[redacted]" if parsed.path and parsed.path != "/" else "")
    except (ValueError, UnicodeError):
        counts["url"] += 1
        return "[redacted-url]"


def _sanitize_text(value: Any, counts: Counter[str]) -> str:
    if not isinstance(value, str):
        raise ExcludedRecord("unsafe_text_shape")
    text = value
    for pattern, code in (
        (PROMPT_TAG_RE, "prompt_block"),
        (PROMPT_COMMENT_RE, "prompt_block"),
        (CODE_FENCE_RE, "code_fence"),
        (OPEN_CODE_FENCE_RE, "code_fence"),
        (PROMPT_HEADING_RE, "prompt_block"),
        (PLAIN_PROMPT_HEADER_RE, "prompt_block"),
        (ENVIRONMENT_DUMP_RE, "environment_dump"),
    ):
        text, count = pattern.subn("[redacted-block]", text)
        counts[code] += count
    text, count = OPEN_PROMPT_TAG_RE.subn("[redacted-block]", text)
    counts["prompt_block"] += count
    text = CONTROL_RE.sub(" ", text)
    counts["control_character"] += len(CONTROL_RE.findall(value))
    text = _replace(PEM_RE, text, "[redacted-key]", counts, "pem")
    text = _replace(AUTH_HEADER_RE, text, "[redacted-auth]", counts, "auth_header")
    text = _replace(JWT_RE, text, "[redacted-token]", counts, "jwt")
    text = _replace(SECRET_ASSIGN_RE, text, "[redacted-secret]", counts, "secret_assignment")
    text = _replace(ENV_OBJECT_RE, text, "[redacted-environment]", counts, "environment_dump")
    text = _replace(QUOTED_ASSIGN_RE, text, "[redacted-assignment]", counts, "quoted_assignment")
    text = _replace(ASSIGNMENT_RE, text, "[redacted-assignment]", counts, "assignment")
    text = _replace(KNOWN_TOKEN_RE, text, "[redacted-token]", counts, "known_token")
    text = _replace(EMAIL_RE, text, "[redacted-email]", counts, "email")
    text = URL_RE.sub(lambda match: _sanitize_url(match, counts), text)
    text = _replace(WINDOWS_PATH_RE, text, "[redacted-path]", counts, "path")
    text = _replace(ABSOLUTE_PATH_RE, text, "[redacted-path]", counts, "path")
    text = _replace(RELATIVE_PATH_RE, text, "[redacted-path]", counts, "path")

    def redact_entropy(match: re.Match[str]) -> str:
        token = match.group(0)
        if _entropy(token) >= 4.0:
            counts["high_entropy"] += 1
            return "[redacted-token]"
        return token

    text = LONG_TOKEN_RE.sub(redact_entropy, text)
    text = re.sub(r"[ \t]+", " ", text).strip()
    if SUSPICIOUS_RESIDUAL_RE.search(text):
        raise ExcludedRecord("residual_sensitive_marker")
    if len(text) > MAX_TEXT_CHARS:
        text = text[:MAX_TEXT_CHARS]
        counts["truncated_text"] += 1
    return text


def _safe_tool_name(value: Any) -> str:
    if not isinstance(value, str) or not TOOL_NAME_RE.fullmatch(value):
        raise ExcludedRecord("unsafe_tool_name")
    if value.lower() in {"authorization", "password", "token", "secret"}:
        raise ExcludedRecord("unsafe_tool_name")
    return value


def _safe_status(value: Any) -> str:
    if isinstance(value, bool):
        return "success" if value else "error"
    if not isinstance(value, str):
        raise ExcludedRecord("unsafe_status")
    normalized = STATUS_ALIASES.get(value.lower(), value.lower())
    if normalized not in SAFE_STATUSES:
        raise ExcludedRecord("unsafe_status")
    return normalized


def _safe_exit_status(value: Any) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or not -255 <= value <= 255:
        raise ExcludedRecord("unsafe_exit_status")
    return value


def _combine_result_fields(
    first_status: str | None,
    first_exit: int | None,
    second_status: str | None,
    second_exit: int | None,
) -> tuple[str | None, int | None]:
    exits = [value for value in (first_exit, second_exit) if value is not None]
    failing_exits = [value for value in exits if value != 0]
    exit_status = failing_exits[0] if failing_exits else (0 if exits else None)
    statuses = {value for value in (first_status, second_status) if value is not None}
    if "error" in statuses or failing_exits or ("success" in statuses and len(statuses) > 1):
        return "error", exit_status
    if "success" in statuses or exit_status == 0:
        return "success", exit_status
    if statuses:
        return sorted(statuses)[0], exit_status
    return None, exit_status


def _extract_text(
    content: Any,
    counts: Counter[str],
) -> tuple[list[str], list[dict[str, Any]], set[str], list[str], str | None, int | None]:
    """Read only native text/tool blocks; discard attachment and metadata blocks."""
    if isinstance(content, str):
        text = _sanitize_text(content, counts)
        return ([text] if text else []), [{"type": "text", "text": text}] if text else [], set(), [], None, None
    if content is None:
        return [], [], set(), [], None, None
    if not isinstance(content, list):
        raise ExcludedRecord("unsafe_content_shape")
    visible: list[str] = []
    safe_blocks: list[dict[str, Any]] = []
    kinds: set[str] = set()
    tool_names: list[str] = []
    result_status: str | None = None
    exit_status: int | None = None
    for block in content:
        if not isinstance(block, dict) or not isinstance(block.get("type"), str):
            raise ExcludedRecord("unsafe_content_block")
        block_type = block["type"].lower()
        if block_type in {"text", "input_text", "output_text"}:
            text = _sanitize_text(block.get("text"), counts)
            if text:
                visible.append(text)
                safe_blocks.append({"type": "text", "text": text})
        elif block_type in {"tool_use", "tool_call"}:
            name = _safe_tool_name(block.get("name") or block.get("tool_name"))
            kinds.add("tool_use")
            tool_names.append(name)
            safe_blocks.append({"type": "tool_use", "name": name})
        elif block_type in {"tool_result", "function_call_output"}:
            kinds.add("tool_result")
            nested: list[dict[str, Any]] = []
            for result_key in ("content", "output", "stdout", "stderr", "text"):
                if result_key not in block:
                    continue
                result_text, result_blocks, _, _, nested_status, nested_exit = _extract_text(
                    block[result_key], counts,
                )
                visible.extend(result_text)
                nested.extend(result_blocks)
                result_status, exit_status = _combine_result_fields(
                    result_status, exit_status, nested_status, nested_exit,
                )
            block_status, block_exit = _result_fields(block, counts)
            result_status, exit_status = _combine_result_fields(
                result_status, exit_status, block_status, block_exit,
            )
            safe_blocks.append({"type": "tool_result", "content": nested})
        elif block_type in {"image", "document", "attachment", "file", "thinking", "redacted_thinking"}:
            counts["excluded_attachment_or_internal_block"] += 1
        else:
            raise ExcludedRecord("unknown_content_block")
    return visible, safe_blocks, kinds, tool_names, result_status, exit_status


def _result_fields(payload: dict[str, Any], counts: Counter[str]) -> tuple[str | None, int | None]:
    status: str | None = None
    exit_status: int | None = None
    if "exit_code" in payload:
        exit_status = _safe_exit_status(payload["exit_code"])
        status = "success" if exit_status == 0 else "error"
    elif "exit_status" in payload:
        exit_status = _safe_exit_status(payload["exit_status"])
        status = "success" if exit_status == 0 else "error"
    elif "is_error" in payload or "isError" in payload:
        value = payload.get("is_error", payload.get("isError"))
        if not isinstance(value, bool):
            raise ExcludedRecord("unsafe_status")
        status = "error" if value else "success"
    elif "success" in payload:
        if not isinstance(payload["success"], bool):
            raise ExcludedRecord("unsafe_status")
        status = "success" if payload["success"] else "error"
    elif "status" in payload:
        status = _safe_status(payload["status"])
    return status, exit_status


def _claude_projection(root: dict[str, Any], counts: Counter[str]) -> dict[str, Any]:
    root_type = root.get("type")
    if not isinstance(root_type, str):
        raise ExcludedRecord("unrecognized_envelope")
    root_type = root_type.lower()
    if root_type in CLAUDE_CONTROL_TYPES:
        if root_type == "system":
            raise ExcludedRecord("system_prompt")
        raise ExcludedRecord("non_event_metadata")
    if root_type not in {"user", "assistant", "tool_result"}:
        raise ExcludedRecord("unrecognized_envelope")
    if _subagent_or_forked(root):
        raise ExcludedRecord("subagent_record")

    message = root.get("message") if isinstance(root.get("message"), dict) else {}
    role = message.get("role") or (root_type if root_type in {"user", "assistant"} else None)
    if role not in {"user", "assistant"}:
        raise ExcludedRecord("unrecognized_role")
    if root_type in {"user", "assistant"} and role != root_type:
        raise ExcludedRecord("role_mismatch")

    content = message.get("content", root.get("content"))
    visible, safe_blocks, block_kinds, names, block_status, block_exit = _extract_text(content, counts)
    kind = "user_message" if role == "user" else "assistant_message"
    if "tool_use" in block_kinds:
        kind = "tool_use"
    elif "tool_result" in block_kinds or root_type == "tool_result":
        kind = "tool_result"
    tool = names[0] if names else None
    if len(set(names)) > 1:
        tool = ",".join(dict.fromkeys(names))[:MAX_TEXT_CHARS]

    stop_reason = None
    if "stop_reason" in message:
        value = message["stop_reason"]
        if value is not None:
            if not isinstance(value, str) or value not in SAFE_STOP_REASONS:
                raise ExcludedRecord("unsafe_stop_reason")
            stop_reason = value

    status, exit_status = _result_fields(root, counts)
    status, exit_status = _combine_result_fields(status, exit_status, block_status, block_exit)
    return {
        "event_kind": kind,
        "native_record_kind": root_type,
        "role": role,
        "tool_name": tool,
        "result_status": status,
        "exit_status": exit_status,
        "stop_reason": stop_reason,
        "reason": None,
        "error_message": None,
        "visible_text": "\n".join(visible)[:MAX_TEXT_CHARS],
        "_safe_content": safe_blocks,
    }


def _codex_projection(root: dict[str, Any], counts: Counter[str]) -> dict[str, Any]:
    root_type = root.get("type")
    payload = root.get("payload")
    if not isinstance(root_type, str) or root_type not in CODEX_ROOT_TYPES or not isinstance(payload, dict):
        raise ExcludedRecord("unrecognized_envelope")
    if root_type in CODEX_CONTROL_TYPES:
        raise ExcludedRecord("non_event_metadata")
    if _subagent_or_forked(root):
        raise ExcludedRecord("subagent_record")
    payload_type = payload.get("type")
    if not isinstance(payload_type, str) or payload_type not in CODEX_PAYLOAD_TYPES:
        raise ExcludedRecord("unrecognized_envelope")
    if payload_type in {"reasoning", "agent_reasoning"}:
        raise ExcludedRecord("internal_reasoning")

    message = payload.get("message")
    role = message.get("role") if isinstance(message, dict) else payload.get("role")
    if role is not None and role not in {"user", "assistant"}:
        raise ExcludedRecord("unrecognized_role")
    if payload_type == "user_message":
        role = "user"
    elif payload_type == "agent_message":
        role = "assistant"
    result_event = payload_type in {
        "function_call_output", "tool_result", "mcp_tool_result",
    }
    if result_event:
        visible: list[str] = []
        safe_blocks: list[dict[str, Any]] = []
        block_kinds: set[str] = {"tool_result"}
        names: list[str] = []
        block_status: str | None = None
        block_exit: int | None = None
        for result_key in ("content", "output", "stdout", "stderr", "text"):
            if result_key not in payload:
                continue
            result_text, result_blocks, nested_kinds, nested_names, nested_status, nested_exit = _extract_text(
                payload[result_key], counts,
            )
            visible.extend(result_text)
            safe_blocks.extend(result_blocks)
            block_kinds.update(nested_kinds)
            names.extend(nested_names)
            block_status, block_exit = _combine_result_fields(
                block_status, block_exit, nested_status, nested_exit,
            )
    else:
        if isinstance(message, dict):
            content = message.get("content", payload.get("content"))
        elif isinstance(message, str):
            content = message
        else:
            content = payload.get("content")
        visible, safe_blocks, block_kinds, names, block_status, block_exit = _extract_text(content, counts)

    tool = None
    if payload_type in {"function_call", "tool_call", "tool_use", "mcp_tool_call"}:
        tool = _safe_tool_name(payload.get("name") or payload.get("tool_name"))
        block_kinds.add("tool_use")
    elif result_event and (payload.get("name") or payload.get("tool_name")):
        tool = _safe_tool_name(payload.get("name") or payload.get("tool_name"))
    if names:
        if tool is not None and any(name != tool for name in names):
            raise ExcludedRecord("conflicting_tool_names")
        tool = tool or names[0]

    reason = None
    if "reason" in payload:
        reason = _sanitize_text(payload["reason"], counts)
    error_message = None
    error = payload.get("error")
    if error is not None:
        if not isinstance(error, dict) or not isinstance(error.get("message"), str):
            raise ExcludedRecord("unsafe_error_shape")
        error_message = _sanitize_text(error["message"], counts)

    status, exit_status = _result_fields(payload, counts)
    status, exit_status = _combine_result_fields(status, exit_status, block_status, block_exit)
    if error_message:
        status, exit_status = _combine_result_fields(status, exit_status, "error", None)
    stop_reason = payload.get("stop_reason")
    if stop_reason is not None:
        if not isinstance(stop_reason, str) or stop_reason not in SAFE_STOP_REASONS:
            raise ExcludedRecord("unsafe_stop_reason")

    if "tool_use" in block_kinds:
        kind = "tool_use"
    elif payload_type in {"function_call_output", "tool_result", "mcp_tool_result"} or "tool_result" in block_kinds:
        kind = "tool_result"
    elif payload_type == "error" or error_message is not None:
        kind = "error"
    elif payload_type in {"reasoning", "agent_reasoning"} or reason is not None:
        kind = "reasoning"
    elif role == "user":
        kind = "user_message"
    elif role == "assistant":
        kind = "assistant_message"
    elif payload_type in {"exec_command_begin", "exec_command_end", "command_execution"}:
        kind = "tool_status"
    elif payload_type in {"task_complete", "turn_complete", "turn_started", "turn_aborted"}:
        kind = "turn_status"
    else:
        kind = "status"

    return {
        "event_kind": kind,
        "native_record_kind": f"{root_type}/{payload_type}",
        "role": role,
        "tool_name": tool,
        "result_status": status,
        "exit_status": exit_status,
        "stop_reason": stop_reason,
        "reason": reason,
        "error_message": error_message,
        "visible_text": "\n".join(visible)[:MAX_TEXT_CHARS],
        "_safe_content": safe_blocks,
    }


def _native_session_id(record: dict[str, Any], harness: str) -> str | None:
    if harness == "claude":
        value = record.get("sessionId")
        return value if isinstance(value, str) and value else None
    payload = record.get("payload")
    if isinstance(payload, dict) and record.get("type") == "session_meta":
        value = payload.get("id")
        return value if isinstance(value, str) and value else None
    return None


def _native_timestamp_seconds(record: Any) -> float | None:
    if not isinstance(record, dict):
        return None
    value = record.get("timestamp")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        try:
            numeric = float(value)
        except OverflowError:
            return None
        return numeric if math.isfinite(numeric) else None
    if isinstance(value, str) and value.strip():
        try:
            parsed = datetime.fromisoformat(value.strip().replace("Z", "+00:00"))
        except ValueError:
            return None
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=timezone.utc)
        try:
            return parsed.timestamp()
        except (OverflowError, OSError, ValueError):
            return None
    return None


def _subagent_or_forked(record: dict[str, Any]) -> bool:
    payload = record.get("payload") if isinstance(record.get("payload"), dict) else {}
    for container in (record, payload):
        if container.get("isSidechain") is True or container.get("is_subagent") is True:
            return True
        if any(container.get(key) for key in (
            "agentId", "agent_id", "parent_session_id", "parentSessionId",
            "parent_thread_id", "parentThreadId", "forked_from", "forkedFrom",
            "forked_from_id", "forked_from_session_id",
        )):
            return True
        source = container.get("source")
        if isinstance(source, str) and source.lower() in {"subagent", "fork", "forked"}:
            return True
        if isinstance(source, dict):
            if any(key in source for key in ("subagent", "fork", "forked")):
                return True
            kind = source.get("kind") or source.get("type")
            if isinstance(kind, str) and kind.lower() in {"subagent", "fork", "forked"}:
                return True
    return False


def _locate_native_start(records: list[tuple[Any, bytes]], harness: str) -> tuple[str, int]:
    if harness == "codex":
        first = records[0][0] if records else None
        if not isinstance(first, dict) or first.get("type") != "session_meta":
            raise PipelineError("native_session_start_required")
        session_id = _native_session_id(first, harness)
        if not session_id:
            raise PipelineError("native_session_identity_missing")
        if _subagent_or_forked(first):
            raise PipelineError("subagent_session")
        return session_id, 0

    leading_controls = {
        "session_start", "file-history-snapshot", "queue-operation", "summary",
        "custom-title", "progress", "system", "hook", "attachment",
    }
    for index, (record, _line_bytes) in enumerate(records[:MAX_LEADING_CONTROL_RECORDS + 1]):
        if not isinstance(record, dict):
            raise PipelineError("native_session_start_required")
        if _subagent_or_forked(record):
            raise PipelineError("subagent_session")
        if record.get("type") == "user" and _native_session_id(record, harness):
            if record.get("parentUuid") is not None:
                raise PipelineError("forked_or_partial_session")
            if record.get("isSidechain") not in (None, False):
                raise PipelineError("subagent_session")
            return record["sessionId"], index
        if record.get("type") == "user":
            raise PipelineError("native_session_identity_missing")
        if record.get("type") not in leading_controls:
            raise PipelineError("native_session_start_required")
    raise PipelineError("native_session_start_required")


def _project_record(record: Any, harness: str, counts: Counter[str]) -> dict[str, Any]:
    if not isinstance(record, dict):
        raise ExcludedRecord("unrecognized_envelope")
    if harness == "claude":
        return _claude_projection(record, counts)
    if harness == "codex":
        return _codex_projection(record, counts)
    raise ExcludedRecord("unsupported_harness")


def _load_source(path: Path) -> tuple[bytes, os.stat_result]:
    try:
        before_path = path.lstat()
    except OSError:
        raise PipelineError("source_unavailable") from None
    if not stat.S_ISREG(before_path.st_mode):
        raise PipelineError("source_not_regular")
    if before_path.st_size >= MAX_SOURCE_BYTES:
        raise PipelineError("source_too_large")
    if "subagents" in {part.lower() for part in path.parts}:
        raise PipelineError("subagent_path")
    age = _now().timestamp() - before_path.st_mtime
    if age < MIN_SOURCE_AGE_SECONDS:
        raise PipelineError("source_not_24h_stable")

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    if not hasattr(os, "O_NOATIME") or not hasattr(os, "O_NOFOLLOW"):
        raise PipelineError("source_safe_open_unavailable")
    flags |= getattr(os, "O_NOATIME", 0)
    try:
        fd = os.open(path, flags)
    except OSError:
        raise PipelineError("source_open_failed") from None
    try:
        with os.fdopen(fd, "rb", closefd=True) as stream:
            before = os.fstat(stream.fileno())
            if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
                before_path.st_dev, before_path.st_ino, before_path.st_size, before_path.st_mtime_ns,
            ):
                raise PipelineError("source_changed")
            if before.st_size >= MAX_SOURCE_BYTES:
                raise PipelineError("source_too_large")
            chunks = bytearray()
            while len(chunks) < before.st_size:
                chunk = stream.read(min(64 * 1024, before.st_size - len(chunks)))
                if not chunk:
                    raise PipelineError("source_changed")
                chunks.extend(chunk)
            after = os.fstat(stream.fileno())
            after_path = path.lstat()
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
            after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns,
        ) or (after.st_dev, after.st_ino) != (after_path.st_dev, after_path.st_ino):
            raise PipelineError("source_changed")
    except OSError:
        raise PipelineError("source_read_failed") from None
    return bytes(chunks), before


def _atomic_private_write(path: Path, payload: bytes) -> None:
    temporary = path.with_name(f".{path.name}.{secrets.token_hex(8)}.tmp")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
    try:
        fd = os.open(temporary, flags, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o600)
        os.link(temporary, path)
        temporary.unlink()
        os.chmod(path, 0o600)
    except FileExistsError:
        temporary.unlink(missing_ok=True)
        raise PipelineError("duplicate_session") from None
    except OSError:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        raise PipelineError("private_output_failed") from None


def _iter_jsonl(raw: bytes) -> Iterable[tuple[Any, bytes]]:
    """Decode one bounded source incrementally, without retaining parsed rows."""
    found = False
    for line in io.BytesIO(raw):
        if not line.strip():
            continue
        found = True
        try:
            yield json.loads(line.decode("utf-8")), line
        except UnicodeDecodeError:
            raise PipelineError("source_invalid_utf8") from None
        except (json.JSONDecodeError, RecursionError):
            yield {"__invalid_jsonl_record__": True}, line
    if not found:
        raise PipelineError("source_empty")


def _manifest_path(root: Path, session_id: str) -> Path:
    return root / "manifests" / f"{session_id}.json"


def _prepare_one_impl(
    input_path: str | Path,
    *,
    harness: str,
    closed_confirmed: bool = False,
    private_root: Path = PRIVATE_ROOT,
) -> dict[str, Any]:
    """Prepare exactly one explicitly named source file for local annotation.

    ``closed_confirmed`` is an operator attestation. The implementation also
    requires a 24-hour-old mtime and a stable inode/size/mtime snapshot.
    """
    if not closed_confirmed:
        raise PipelineError("closed_source_confirmation_required")
    harness_key = harness.strip().lower().replace("-code", "").replace("_code", "")
    if harness_key not in {"claude", "codex"}:
        raise PipelineError("unsupported_harness")
    root = _ensure_private_root(private_root)
    raw_dir = root / "raw"
    projection_dir = root / "projections"
    manifest_dir = root / "manifests"
    for directory in (raw_dir, projection_dir, manifest_dir):
        directory.mkdir(mode=0o700, exist_ok=True)
        os.chmod(directory, 0o700)
    key = _private_key(root)
    source_path = Path(input_path).absolute()
    raw_bytes, source_stat = _load_source(source_path)
    records = list(_iter_jsonl(raw_bytes))
    first_timestamp = next(
        (timestamp for record, _line_bytes in records
         if (timestamp := _native_timestamp_seconds(record)) is not None),
        None,
    )

    native_session_id, native_start_index = _locate_native_start(records, harness_key)
    for record, _line_bytes in records:
        found_id = _native_session_id(record, harness_key) if isinstance(record, dict) else None
        if found_id and found_id != native_session_id:
            raise PipelineError("mixed_native_sessions")
        if isinstance(record, dict) and record.get("type") == "session_meta" and _subagent_or_forked(record):
            raise PipelineError("subagent_session")

    session_id = _opaque_id(key, b"session", native_session_id)
    raw_path = raw_dir / f"{session_id}.jsonl"
    projection_path = projection_dir / f"{session_id}.jsonl"
    manifest_path = _manifest_path(root, session_id)
    if raw_path.exists() or projection_path.exists() or manifest_path.exists():
        raise PipelineError("duplicate_session")

    exclusions: Counter[str] = Counter()
    redactions: Counter[str] = Counter()
    tool_identity_counts: Counter[str] = Counter()
    event_kind_counts: Counter[str] = Counter()
    native_error_count = 0
    result_failure_count = 0
    projected: list[dict[str, Any]] = []
    seen_records: set[str] = set()
    event_order = 0
    for record_index, (record, line_bytes) in enumerate(records):
        if record_index < native_start_index:
            exclusions["leading_native_metadata"] += 1
            continue
        if harness_key == "codex" and record_index == native_start_index:
            exclusions["native_session_start"] += 1
            continue
        if isinstance(record, dict) and record.get("__invalid_jsonl_record__"):
            exclusions["malformed_jsonl_record"] += 1
            continue
        fingerprint = hashlib.sha256(line_bytes).hexdigest()
        if fingerprint in seen_records:
            exclusions["duplicate_record"] += 1
            continue
        seen_records.add(fingerprint)
        record_counts: Counter[str] = Counter()
        try:
            projection = _project_record(record, harness_key, record_counts)
        except ExcludedRecord as exc:
            exclusions[exc.code] += 1
            if exc.code in {
                "residual_sensitive_marker", "unsafe_text_shape", "unsafe_content_shape",
                "unsafe_content_block", "unknown_content_block", "unsafe_error_shape",
                "unsafe_tool_name", "conflicting_tool_names",
            }:
                raise PipelineError("session_unsanitizable") from None
            continue
        redactions.update(record_counts)
        event_kind_counts[projection["event_kind"]] += 1
        if projection["event_kind"] == "error" or projection["error_message"]:
            native_error_count += 1
        if projection["result_status"] == "error":
            result_failure_count += 1
        if projection["tool_name"]:
            for tool_name in projection["tool_name"].split(","):
                tool_identity_counts[_opaque_id(key, b"tool", tool_name)] += 1
        clean_projection = {key_name: value for key_name, value in projection.items() if not key_name.startswith("_")}
        projection_text = _canonical_json(clean_projection)
        projection_hash = hashlib.sha256(projection_text.encode("utf-8")).hexdigest()
        event_seed = None
        if isinstance(record, dict):
            candidate_seed = record.get("uuid") or record.get("id")
            if isinstance(candidate_seed, str) and candidate_seed:
                event_seed = candidate_seed
        event_key = f"{native_session_id}\0{event_seed or ''}\0{event_order}"
        event_id = _opaque_id(key, b"event", event_key)
        native_hash = hashlib.sha256(line_bytes).hexdigest()
        native_timestamp = _native_timestamp_seconds(record)
        arrival_ms = (
            round((native_timestamp - first_timestamp) * 1_000, 3)
            if native_timestamp is not None and first_timestamp is not None
            else None
        )
        projected.append({
            "id": event_id,
            "origin": "real_private_candidate",
            "harness": harness_key,
            "native_format": f"{harness_key}-jsonl-v1",
            "native_sha256": native_hash,
            "session_id": session_id,
            "event_order": event_order,
            "arrival_ms": arrival_ms,
            "native": {
                "harness": harness_key,
                "format": f"{harness_key}-jsonl-v1",
            },
            "context": {"session_id": session_id, "event_order": event_order},
            "event_kind": projection["event_kind"],
            "evidence_text": projection["visible_text"],
            "projection": clean_projection,
            "projection_sha256": projection_hash,
            "redaction_counts": dict(sorted(record_counts.items())),
        })
        event_order += 1

    if not projected:
        raise PipelineError("no_eligible_events")
    raw_hash = hashlib.sha256(raw_bytes).hexdigest()
    projection_bytes = b"".join(
        (_canonical_json(row) + "\n").encode("utf-8") for row in projected
    )
    combined_projection_hash = hashlib.sha256(projection_bytes).hexdigest()
    ordinary_omissions = {
        "native_session_start", "non_event_metadata", "system_prompt",
        "excluded_attachment_or_internal_block", "duplicate_record",
    }
    unusual_record_count = sum(
        count for code, count in exclusions.items() if code not in ordinary_omissions
    )
    selection_cues = {
        "eligible_projected_rows": len(projected),
        "native_error_count": native_error_count,
        "result_failure_count": result_failure_count,
        "repeated_tool_identity_counts": {
            identity: count for identity, count in sorted(tool_identity_counts.items()) if count > 1
        },
        "event_kind_counts": dict(sorted(event_kind_counts.items())),
        "unusual_record_count": unusual_record_count,
        "omitted_record_count": sum(exclusions.values()),
        "redaction_count": sum(redactions.values()),
        "truncation_count": redactions["truncated_text"],
        "native_session_start_valid": True,
        "native_session_identity_valid": bool(native_session_id),
    }
    manifest = {
        "session_id": session_id,
        "source_path": str(source_path),
        "source_bytes": source_stat.st_size,
        "source_mtime_ns": source_stat.st_mtime_ns,
        "source_sha256": raw_hash,
        "closed_source_confirmed": True,
        "raw_copy": str(raw_path),
        "projection_copy": str(projection_path),
        "record_count": len(records),
        "event_count": len(projected),
        "projection_sha256": combined_projection_hash,
        "selection_cues": selection_cues,
        "exclusion_counts": dict(sorted(exclusions.items())),
        "redaction_counts": dict(sorted(redactions.items())),
        "labels": None,
    }
    owned_outputs: list[Path] = []
    try:
        _atomic_private_write(raw_path, raw_bytes)
        owned_outputs.append(raw_path)
        _atomic_private_write(projection_path, projection_bytes)
        owned_outputs.append(projection_path)
        _atomic_private_write(manifest_path, (_canonical_json(manifest) + "\n").encode("utf-8"))
        owned_outputs.append(manifest_path)
    except PipelineError:
        for output in owned_outputs:
            try:
                output.unlink(missing_ok=True)
            except OSError:
                pass
        raise
    return {
        "session_id": session_id,
        "event_count": len(projected),
        "selection_cues": selection_cues,
        "exclusion_counts": dict(sorted(exclusions.items())),
        "redaction_counts": dict(sorted(redactions.items())),
        "projection_sha256": combined_projection_hash,
        "raw_copy": str(raw_path),
        "projection_copy": str(projection_path),
        "manifest": str(manifest_path),
    }


def prepare_one(
    input_path: str | Path,
    *,
    harness: str,
    closed_confirmed: bool = False,
    private_root: Path = PRIVATE_ROOT,
) -> dict[str, Any]:
    """Prepare one explicit source, checking real telemetry before and after."""
    if not closed_confirmed:
        raise PipelineError("closed_source_confirmation_required")
    before = thermal_checkpoint()
    if before >= THERMAL_LIMIT_MILLIDEGREES:
        raise PipelineError("thermal_limit")
    try:
        return _prepare_one_impl(
            input_path,
            harness=harness,
            closed_confirmed=True,
            private_root=private_root,
        )
    finally:
        thermal_checkpoint()


def _read_tctl_millidegrees() -> int:
    try:
        devices = sorted(HWMON_ROOT.glob("hwmon*"))
    except OSError:
        raise PipelineError("thermal_unavailable") from None
    for device in devices:
        try:
            if (device / "name").read_text(encoding="ascii").strip() != "k10temp":
                continue
            for label_path in sorted(device.glob("temp*_label")):
                if label_path.read_text(encoding="ascii").strip() != "Tctl":
                    continue
                input_path = device / f"{label_path.name[:-6]}_input"
                raw = input_path.read_text(encoding="ascii").strip()
                value = int(raw)
                if value <= 0:
                    raise ValueError("invalid sensor value")
                return value
        except (OSError, UnicodeError, ValueError):
            continue
    raise PipelineError("thermal_unavailable")


def thermal_checkpoint() -> int:
    """Append one actual k10temp/Tctl reading and stop at 85 C or above."""
    millidegrees = _read_tctl_millidegrees()
    timestamp = _now().strftime("%Y-%m-%dT%H:%M:%SZ")
    record = f"{timestamp} k10temp/Tctl millidegrees_c={millidegrees}\n".encode("ascii")
    flags = os.O_WRONLY | os.O_APPEND | os.O_CREAT | getattr(os, "O_CLOEXEC", 0)
    try:
        fd = os.open(THERMAL_LOG, flags, 0o600)
        try:
            os.chmod(THERMAL_LOG, 0o600)
            os.write(fd, record)
            os.fsync(fd)
        finally:
            os.close(fd)
    except OSError:
        raise PipelineError("thermal_log_unavailable") from None
    if millidegrees >= THERMAL_LIMIT_MILLIDEGREES:
        raise PipelineError("thermal_limit")
    return millidegrees


def _main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    inventory_parser = subparsers.add_parser("inventory", help="inspect metadata only")
    inventory_parser.add_argument("path")
    prepare_parser = subparsers.add_parser("prepare-one", help="prepare one explicit source file")
    prepare_parser.add_argument("path")
    prepare_parser.add_argument("--harness", choices=("claude", "codex"), required=True)
    prepare_parser.add_argument("--closed", action="store_true", dest="closed_confirmed")
    subparsers.add_parser("thermal-check", help="record one actual k10temp/Tctl reading")
    args = parser.parse_args(argv)
    try:
        if args.command == "inventory":
            result = inventory(args.path)
        elif args.command == "prepare-one":
            result = prepare_one(
                args.path, harness=args.harness, closed_confirmed=args.closed_confirmed,
            )
        else:
            result = {"millidegrees_c": thermal_checkpoint()}
    except PipelineError as exc:
        print(_canonical_json({"error_code": exc.code}), file=sys.stderr)
        return 2
    print(_canonical_json(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
