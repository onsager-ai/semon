#!/usr/bin/env python3
"""Normalize Codex JSONL session records and export them as OTLP logs."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Iterable

VERSION = 1


def default_state_path() -> Path:
    root = os.environ.get("XDG_STATE_HOME")
    if root:
        return Path(root) / "devlog" / "codex-tailer.json"
    return Path.home() / ".local" / "state" / "devlog" / "codex-tailer.json"


def load_state(path: Path) -> dict[str, Any]:
    try:
        state = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {"version": VERSION, "files": {}, "session_repos": {}}
    except (OSError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"cannot read state file {path}: {exc}") from exc
    if state.get("version") != VERSION:
        raise RuntimeError(
            f"unsupported state version in {path}: {state.get('version')!r}"
        )
    state.setdefault("files", {})
    state.setdefault("session_repos", {})
    return state


def save_state(path: Path, state: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(state, handle, sort_keys=True, separators=(",", ":"))
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def parse_timestamp(value: Any) -> int:
    if isinstance(value, (int, float)):
        seconds = float(value)
        if seconds > 10_000_000_000:
            seconds /= 1000
        return int(seconds * 1_000_000_000)
    if isinstance(value, str):
        candidate = value.strip().replace("Z", "+00:00")
        try:
            parsed = dt.datetime.fromisoformat(candidate)
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=dt.timezone.utc)
            return int(parsed.timestamp() * 1_000_000_000)
        except ValueError:
            pass
    return time.time_ns()


def json_text(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def message_text(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        parts: list[str] = []
        for item in value:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                text = item.get("text")
                if isinstance(text, str):
                    parts.append(text)
        return "\n".join(parts)
    if isinstance(value, dict):
        for key in ("text", "message", "content"):
            if key in value:
                return message_text(value[key])
    return json_text(value)


def repo_from_url(url: Any) -> str:
    if not isinstance(url, str) or not url:
        return ""
    value = url.rstrip("/")
    if value.endswith(".git"):
        value = value[:-4]
    return value.rsplit("/", 1)[-1].rsplit(":", 1)[-1]


def repo_from_cwd(cwd: Any) -> str:
    if not isinstance(cwd, str) or not cwd:
        return ""
    return Path(cwd).name


def token_usage(payload: dict[str, Any]) -> tuple[int, int]:
    info = payload.get("info")
    if not isinstance(info, dict):
        return 0, 0
    usage = info.get("last_token_usage") or info.get("last_usage") or {}
    if not isinstance(usage, dict):
        return 0, 0
    tokens_in = int(usage.get("input_tokens") or 0)
    tokens_out = int(usage.get("output_tokens") or 0)
    tokens_out += int(usage.get("reasoning_output_tokens") or 0)
    return tokens_in, tokens_out


def infer_success(value: Any, default: bool = True) -> bool:
    if isinstance(value, dict):
        if isinstance(value.get("success"), bool):
            return value["success"]
        status = value.get("status")
        if isinstance(status, str):
            if status.lower() in {"failed", "error", "cancelled", "rejected"}:
                return False
            if status.lower() in {"success", "succeeded", "completed", "ok"}:
                return True
        for key in ("exit_code", "exitCode"):
            if isinstance(value.get(key), int):
                return value[key] == 0
        metadata = value.get("metadata")
        if isinstance(metadata, dict):
            return infer_success(metadata, default)
    if isinstance(value, str):
        try:
            decoded = json.loads(value)
        except json.JSONDecodeError:
            return default
        return infer_success(decoded, default)
    return default


def normalized_event(
    *,
    timestamp: Any,
    session_id: str,
    repo: str,
    kind: str,
    source: dict[str, Any],
    tool: str = "",
    decision: str = "",
    tokens_in: int = 0,
    tokens_out: int = 0,
    duration_ms: int = 0,
    prompt: str = "",
    response: str = "",
    tool_input: str = "",
    tool_output: str = "",
    success: bool | None = None,
) -> dict[str, Any]:
    extras = dict(source)
    if success is not None:
        extras["success"] = str(success).lower()
    return {
        "ts": parse_timestamp(timestamp),
        "session_id": session_id,
        "repo": repo,
        "kind": kind,
        "tool": tool,
        "decision": decision,
        "tokens_in": max(0, tokens_in),
        "tokens_out": max(0, tokens_out),
        "cost_usd": 0,
        "duration_ms": max(0, duration_ms),
        "extras": json.dumps(
            extras, ensure_ascii=False, separators=(",", ":"), sort_keys=True
        ),
        "prompt": prompt,
        "response": response,
        "tool_input": tool_input,
        "tool_output": tool_output,
    }


def normalize_record(
    record: dict[str, Any],
    context: dict[str, Any],
    repo_override: str,
) -> dict[str, Any]:
    top_type = str(record.get("type") or "unknown")
    payload = record.get("payload")
    if not isinstance(payload, dict):
        payload = {}
    payload_type = str(payload.get("type") or top_type)
    timestamp = record.get("timestamp") or payload.get("timestamp")

    if top_type == "session_meta":
        context["session_id"] = str(
            payload.get("session_id") or payload.get("id") or context["session_id"]
        )
        context["cwd"] = str(payload.get("cwd") or context.get("cwd") or "")
        git = payload.get("git")
        git_url = git.get("repository_url") if isinstance(git, dict) else ""
        context["repo"] = (
            repo_override
            or repo_from_url(git_url)
            or repo_from_cwd(context["cwd"])
            or context["repo"]
        )
    elif top_type == "turn_context":
        context["cwd"] = str(payload.get("cwd") or context.get("cwd") or "")
        if not repo_override and not context.get("repo"):
            context["repo"] = repo_from_cwd(context["cwd"])

    session_id = str(context.get("session_id") or "")
    repo = repo_override or str(context.get("repo") or "")
    calls = context.setdefault("calls", {})

    common = {
        "timestamp": timestamp,
        "session_id": session_id,
        "repo": repo,
        "source": record,
    }

    if top_type == "event_msg" and payload_type == "user_message":
        return normalized_event(
            **common,
            kind="user_prompt",
            prompt=message_text(payload.get("message")),
        )
    if top_type == "event_msg" and payload_type == "agent_message":
        return normalized_event(
            **common,
            kind="assistant_response",
            response=message_text(payload.get("message")),
        )
    if top_type == "event_msg" and payload_type == "token_count":
        tokens_in, tokens_out = token_usage(payload)
        return normalized_event(
            **common,
            kind="api_request",
            tokens_in=tokens_in,
            tokens_out=tokens_out,
        )
    if top_type == "event_msg" and payload_type == "task_complete":
        return normalized_event(
            **common,
            kind="turn_complete",
            duration_ms=int(payload.get("duration_ms") or 0),
            response=message_text(payload.get("last_agent_message")),
        )
    if top_type == "event_msg" and payload_type == "patch_apply_end":
        success = infer_success(payload)
        return normalized_event(
            **common,
            kind="tool_result",
            tool="apply_patch",
            duration_ms=int(payload.get("duration_ms") or 0),
            tool_output=json_text(
                {"stdout": payload.get("stdout"), "stderr": payload.get("stderr")}
            ),
            success=success,
        )
    if top_type == "event_msg" and payload_type == "mcp_tool_call_end":
        tool = ".".join(
            str(part)
            for part in (payload.get("app_name"), payload.get("action_name"))
            if part
        )
        return normalized_event(
            **common,
            kind="tool_result",
            tool=tool or "mcp_tool",
            duration_ms=int(float(payload.get("duration") or 0)),
            tool_input=json_text(payload.get("invocation")),
            tool_output=json_text(payload.get("result")),
            success=infer_success(payload.get("result")),
        )
    if top_type == "event_msg" and payload_type == "web_search_end":
        return normalized_event(
            **common,
            kind="tool_result",
            tool="web_search",
            tool_input=json_text(payload.get("query")),
            tool_output=json_text(payload.get("results")),
            success=infer_success(payload),
        )
    if top_type == "event_msg" and payload_type == "image_generation_end":
        return normalized_event(
            **common,
            kind="tool_result",
            tool="image_generation",
            tool_input=json_text(payload.get("prompt")),
            tool_output=json_text(payload.get("result")),
            success=infer_success(payload),
        )

    call_types = {"function_call", "custom_tool_call", "tool_search_call"}
    if top_type == "response_item" and payload_type in call_types:
        call_id = str(payload.get("call_id") or payload.get("id") or "")
        tool = str(payload.get("name") or payload_type.replace("_call", ""))
        namespace = payload.get("namespace")
        if namespace:
            tool = f"{namespace}.{tool}"
        tool_input = json_text(payload.get("arguments", payload.get("input")))
        if call_id:
            calls[call_id] = {"tool": tool, "input": tool_input}
        return normalized_event(
            **common,
            kind="tool_call",
            tool=tool,
            tool_input=tool_input,
        )

    output_types = {
        "function_call_output",
        "custom_tool_call_output",
        "tool_search_output",
    }
    if top_type == "response_item" and payload_type in output_types:
        call_id = str(payload.get("call_id") or "")
        call = calls.pop(call_id, {}) if call_id else {}
        output = payload.get("output", payload.get("tools"))
        success = infer_success(payload, infer_success(output))
        return normalized_event(
            **common,
            kind="tool_result",
            tool=str(call.get("tool") or payload_type.replace("_output", "")),
            tool_input=str(call.get("input") or ""),
            tool_output=json_text(output),
            success=success,
        )

    if top_type == "response_item" and payload_type == "message":
        role = str(payload.get("role") or "")
        content = message_text(payload.get("content"))
        if role == "user":
            return normalized_event(
                **common, kind="user_prompt", prompt=content
            )
        return normalized_event(
            **common, kind="assistant_response", response=content
        )

    return normalized_event(**common, kind=payload_type)


def history_event(
    record: dict[str, Any],
    session_repos: dict[str, str],
    repo_override: str,
) -> dict[str, Any]:
    session_id = str(record.get("session_id") or "")
    return normalized_event(
        timestamp=record.get("ts"),
        session_id=session_id,
        repo=repo_override or session_repos.get(session_id, ""),
        kind="history_entry",
        source=record,
        prompt=message_text(record.get("text")),
    )


def any_value(value: Any) -> dict[str, str]:
    if isinstance(value, bool):
        return {"boolValue": value}
    if isinstance(value, int):
        return {"intValue": str(value)}
    if isinstance(value, float):
        return {"doubleValue": value}
    return {"stringValue": str(value)}


def otlp_payload(events: Iterable[dict[str, Any]]) -> bytes:
    records = []
    for event in events:
        attributes = {
            "devlog.harness": "codex",
            "devlog.session_id": event["session_id"],
            "session.id": event["session_id"],
            "devlog.repo": event["repo"],
            "devlog.tool": event["tool"],
            "devlog.decision": event["decision"],
            "devlog.tokens_in": event["tokens_in"],
            "devlog.tokens_out": event["tokens_out"],
            "devlog.cost_usd": event["cost_usd"],
            "devlog.duration_ms": event["duration_ms"],
            "devlog.extras": event["extras"],
            "devlog.prompt": event["prompt"],
            "devlog.response": event["response"],
            "devlog.tool_input": event["tool_input"],
            "devlog.tool_output": event["tool_output"],
            "event.name": event["kind"],
        }
        records.append(
            {
                "timeUnixNano": str(event["ts"]),
                "observedTimeUnixNano": str(time.time_ns()),
                "eventName": event["kind"],
                "body": {"stringValue": event["kind"]},
                "attributes": [
                    {"key": key, "value": any_value(value)}
                    for key, value in attributes.items()
                ],
            }
        )
    document = {
        "resourceLogs": [
            {
                "resource": {
                    "attributes": [
                        {
                            "key": "service.name",
                            "value": {"stringValue": "devlog-codex-tailer"},
                        },
                        {
                            "key": "devlog.harness",
                            "value": {"stringValue": "codex"},
                        },
                    ]
                },
                "scopeLogs": [
                    {
                        "scope": {
                            "name": "onsager-ai.devlog.codex-tailer",
                            "version": str(VERSION),
                        },
                        "logRecords": records,
                    }
                ],
            }
        ]
    }
    return json.dumps(document, ensure_ascii=False, separators=(",", ":")).encode()


def export(endpoint: str, events: list[dict[str, Any]], timeout: float) -> None:
    if not events:
        return
    request = urllib.request.Request(
        endpoint,
        data=otlp_payload(events),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            if response.status < 200 or response.status >= 300:
                raise RuntimeError(f"OTLP endpoint returned HTTP {response.status}")
            body = response.read()
            if body:
                result = json.loads(body)
                partial = result.get("partialSuccess", {})
                rejected = int(partial.get("rejectedLogRecords") or 0)
                if rejected:
                    message = partial.get("errorMessage") or "no reason given"
                    raise RuntimeError(
                        f"OTLP endpoint rejected {rejected} log record(s): {message}"
                    )
    except (OSError, ValueError, urllib.error.HTTPError) as exc:
        raise RuntimeError(f"OTLP export failed: {exc}") from exc


def candidate_files(sessions: Path, history: Path) -> list[Path]:
    files = sorted(sessions.glob("**/*.jsonl")) if sessions.exists() else []
    if history.exists():
        files.append(history)
    return files


def process_file(
    path: Path,
    state: dict[str, Any],
    endpoint: str,
    timeout: float,
    batch_size: int,
    max_batch_bytes: int,
    repo_override: str,
    history_path: Path,
) -> int:
    key = str(path.resolve())
    saved = state["files"].setdefault(
        key,
        {"offset": 0, "session_id": "", "repo": "", "cwd": "", "calls": {}},
    )
    size = path.stat().st_size
    if int(saved.get("offset", 0)) > size:
        saved.clear()
        saved.update(
            {"offset": 0, "session_id": "", "repo": "", "cwd": "", "calls": {}}
        )
    context = {
        "session_id": str(saved.get("session_id") or ""),
        "repo": str(saved.get("repo") or ""),
        "cwd": str(saved.get("cwd") or ""),
        "calls": dict(saved.get("calls") or {}),
    }
    emitted = 0
    with path.open("rb") as handle:
        handle.seek(int(saved.get("offset", 0)))
        while True:
            events: list[dict[str, Any]] = []
            pending_bytes = 0
            batch_end = handle.tell()
            while len(events) < batch_size and pending_bytes < max_batch_bytes:
                start = handle.tell()
                line = handle.readline()
                if not line:
                    break
                if not line.endswith(b"\n"):
                    handle.seek(start)
                    break
                batch_end = handle.tell()
                pending_bytes += len(line)
                try:
                    record = json.loads(line)
                    if not isinstance(record, dict):
                        raise ValueError("record is not a JSON object")
                    if path.resolve() == history_path.resolve():
                        event = history_event(
                            record, state["session_repos"], repo_override
                        )
                    else:
                        event = normalize_record(record, context, repo_override)
                    events.append(event)
                except (json.JSONDecodeError, ValueError) as exc:
                    events.append(
                        normalized_event(
                            timestamp=None,
                            session_id=str(context.get("session_id") or ""),
                            repo=repo_override
                            or str(context.get("repo") or ""),
                            kind="parse_error",
                            source={
                                "error": str(exc),
                                "raw": line.decode("utf-8", errors="replace"),
                            },
                        )
                    )
            if batch_end == int(saved.get("offset", 0)):
                break
            export(endpoint, events, timeout)
            saved.update(
                {
                    "offset": batch_end,
                    "session_id": context["session_id"],
                    "repo": context["repo"],
                    "cwd": context["cwd"],
                    "calls": context["calls"],
                }
            )
            if context["session_id"] and context["repo"]:
                state["session_repos"][context["session_id"]] = context["repo"]
            save_state(Path(state["_state_path"]), {
                key: value for key, value in state.items() if key != "_state_path"
            })
            emitted += len(events)
            if not events:
                break
    return emitted


def run_once(args: argparse.Namespace) -> int:
    state = load_state(args.state)
    state["_state_path"] = str(args.state)
    total = 0
    for path in candidate_files(args.sessions, args.history):
        total += process_file(
            path,
            state,
            args.endpoint,
            args.timeout,
            args.batch_size,
            args.max_batch_bytes,
            args.repo,
            args.history,
        )
    if args.verbose:
        print(f"exported {total} record(s)", file=sys.stderr)
    return total


def positive_int(value: str) -> int:
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return number


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument(
        "--sessions",
        type=Path,
        default=Path.home() / ".codex" / "sessions",
        help="Codex sessions directory",
    )
    result.add_argument(
        "--history",
        type=Path,
        default=Path.home() / ".codex" / "history.jsonl",
        help="Codex history JSONL",
    )
    result.add_argument(
        "--state",
        type=Path,
        default=default_state_path(),
        help="durable per-file offset state",
    )
    result.add_argument(
        "--endpoint",
        default=os.environ.get(
            "DEVLOG_OTLP_LOGS_ENDPOINT", "http://127.0.0.1:4318/v1/logs"
        ),
        help="OTLP/HTTP JSON logs endpoint",
    )
    result.add_argument(
        "--repo",
        default=os.environ.get("DEVLOG_REPO", ""),
        help="override repository attribution",
    )
    result.add_argument("--batch-size", type=positive_int, default=100)
    result.add_argument("--max-batch-bytes", type=positive_int, default=1_000_000)
    result.add_argument("--timeout", type=float, default=10.0)
    result.add_argument(
        "--watch",
        action="store_true",
        help="poll continuously instead of making one timer-friendly pass",
    )
    result.add_argument("--interval", type=positive_int, default=10)
    result.add_argument("-v", "--verbose", action="store_true")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        while True:
            run_once(args)
            if not args.watch:
                return 0
            time.sleep(args.interval)
    except KeyboardInterrupt:
        return 130
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"codex_tailer: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
