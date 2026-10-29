#!/usr/bin/env python3
"""Compare parser evidence with selected Codex error payload fields in Jev."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import benchmark


ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "experiments" / "asi-jev" / "data" / "events.jsonl"
OUTPUT = ROOT / "experiments" / "asi-jev" / "results" / "projection-probe.json"
REQUESTED_MODEL = "typesafe/jev-latest"
TIMEOUT_SECONDS = 20.0
REPETITIONS = 3
MAX_CONTEXT_BYTES = 8192

TARGETS = {
    "syn-model-error-01": {
        "failure_category": "model",
        "raw_sha256": "3006ee9fce029cf8ae8dc7d3a5e3ee200ecc57b5e19050e6409aed1cea1f0419",
        "source_line_sha256": "d740a0984aef4dd43320dfc5a525c06a26ed5b23c167da7b2480e7d0f852af25",
    },
    "syn-transport-error-01": {
        "failure_category": "transport",
        "raw_sha256": "c134c50dcb5d9752b7807091d4e3438b181e04e8dcca9d9f9f5bdc8ccd24734b",
        "source_line_sha256": "8fa4d97c367db72934e2515d6b520c96d8392d7607f4359f7515fcc37e3acc81",
    },
}


@dataclass(frozen=True)
class ProbeTarget:
    event: benchmark.Event
    expected_weak_labels: dict[str, str]
    source_line_sha256: str
    reason: str | None
    error_message: str


def _utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def _sanitize_field(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    collapsed = " ".join(value.split())
    printable = "".join(character for character in collapsed if character.isprintable())
    sanitized = benchmark._truncate_utf8(printable, 1200, keep_tail=False).strip()
    return sanitized or None


def _load_targets(path: Path, events: list[benchmark.Event]) -> list[ProbeTarget]:
    events_by_id = {event.event_id: event for event in events}
    found: dict[str, ProbeTarget] = {}

    try:
        stream = path.open(encoding="utf-8")
    except OSError as exc:
        raise benchmark.BenchmarkError(f"cannot open events file ({type(exc).__name__})") from None

    with stream:
        for line_number, line in enumerate(stream, 1):
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                raise benchmark.BenchmarkError(f"row {line_number}: invalid JSONL record") from None
            if not isinstance(row, dict) or row.get("id") not in TARGETS:
                continue

            event_id = row["id"]
            expected = TARGETS[event_id]
            event = events_by_id.get(event_id)
            native = row.get("native")
            source = row.get("source")
            if event is None or not isinstance(native, dict) or not isinstance(source, dict):
                raise benchmark.BenchmarkError(f"{event_id}: missing parsed event, native data, or source data")
            raw_sha256 = native.get("raw_sha256")
            source_line_sha256 = source.get("line_sha256")
            if raw_sha256 != expected["raw_sha256"] or event.native_sha256 != expected["raw_sha256"]:
                raise benchmark.BenchmarkError(f"{event_id}: raw hash differs from the pinned probe input")
            if source_line_sha256 != expected["source_line_sha256"]:
                raise benchmark.BenchmarkError(f"{event_id}: source line hash differs from the pinned probe input")
            if event.harness.lower() != "codex" or "turn_aborted" not in event.event_kind.lower():
                raise benchmark.BenchmarkError(f"{event_id}: expected a Codex turn_aborted event")

            raw = native.get("raw")
            try:
                native_record = json.loads(raw) if isinstance(raw, str) else None
            except json.JSONDecodeError:
                native_record = None
            payload = native_record.get("payload") if isinstance(native_record, dict) else None
            if not isinstance(payload, dict):
                raise benchmark.BenchmarkError(f"{event_id}: missing native payload")
            error = payload.get("error")
            error_message = _sanitize_field(error.get("message") if isinstance(error, dict) else None)
            if not error_message:
                raise benchmark.BenchmarkError(f"{event_id}: expected a string payload.error.message")
            reason = _sanitize_field(payload.get("reason"))
            gold = row.get("gold")
            streaming_gold = gold.get("streaming") if isinstance(gold, dict) else None
            if not isinstance(streaming_gold, dict) or streaming_gold.get("failure_category") != expected["failure_category"]:
                raise benchmark.BenchmarkError(f"{event_id}: streaming label differs from the expected weak label")

            found[event_id] = ProbeTarget(
                event=event,
                expected_weak_labels={"failure_category": expected["failure_category"]},
                source_line_sha256=source_line_sha256,
                reason=reason,
                error_message=error_message,
            )

    missing = [event_id for event_id in TARGETS if event_id not in found]
    if missing:
        raise benchmark.BenchmarkError("probe events not found: " + ", ".join(missing))
    return [found[event_id] for event_id in TARGETS]


def _variants(target: ProbeTarget) -> tuple[tuple[str, benchmark.Event], ...]:
    augmented = [target.event.evidence_text]
    if target.reason:
        augmented.append(f"payload.reason: {target.reason}")
    augmented.append(f"payload.error.message: {target.error_message}")
    return (
        ("parser_evidence", target.event),
        ("payload_reason_error_message", replace(target.event, evidence_text="\n".join(augmented))),
    )


def _run_call(
    call_number: int,
    target: ProbeTarget,
    variant: str,
    event: benchmark.Event,
    repetition: int,
    base: str,
    api_key: str,
) -> dict[str, Any]:
    started_at = _utc()
    started = time.perf_counter()
    row: dict[str, Any] = {
        "call": call_number,
        "event_id": event.event_id,
        "raw_sha256": event.native_sha256,
        "source_line_sha256": target.source_line_sha256,
        "expected_weak_labels": target.expected_weak_labels,
        "variant": variant,
        "repetition": repetition,
        "requested_model": REQUESTED_MODEL,
        "resolved_model": None,
        "predictions": {dimension: None for dimension in benchmark.DIMENSIONS},
        "confidence": {dimension: None for dimension in benchmark.DIMENSIONS},
        "answer_details": {},
        "usage": {"input_tokens": None, "output_tokens": None, "complete": False},
        "rtt_ms": None,
        "errors": {"request": None, "dimensions": {}},
        "started_at_utc": started_at,
        "finished_at_utc": None,
    }
    try:
        context = benchmark.format_context(event, [event], "prefix", MAX_CONTEXT_BYTES)
        predictions, errors, details, telemetry = benchmark._request_batch(
            "jev",
            [event],
            {event.index: context},
            REQUESTED_MODEL,
            base,
            api_key,
            TIMEOUT_SECONDS,
            "/v1/chat/completions",
        )
        event_predictions = predictions.get(event.index, {})
        event_details = details.get(event.index, {})
        row.update({
            "resolved_model": telemetry["resolved_model"],
            "predictions": {dimension: event_predictions.get(dimension) for dimension in benchmark.DIMENSIONS},
            "confidence": {
                dimension: event_details.get(dimension, {}).get("confidence")
                for dimension in benchmark.DIMENSIONS
            },
            "answer_details": event_details,
            "usage": {
                "input_tokens": telemetry["input_tokens"],
                "output_tokens": telemetry["output_tokens"],
                "complete": telemetry["usage_complete"],
            },
            "rtt_ms": telemetry["rtt_ms"],
            "errors": {"request": None, "dimensions": errors.get(event.index, {})},
        })
    except benchmark.ProviderFailure as exc:
        row["errors"]["request"] = exc.code
        row["rtt_ms"] = exc.rtt_ms
    except Exception as exc:
        row["errors"]["request"] = f"client_error_{type(exc).__name__}"
        row["rtt_ms"] = (time.perf_counter() - started) * 1000.0
    finally:
        row["finished_at_utc"] = _utc()
    return row


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Running this probe sends exactly 12 sequential Jev requests; it does not retry.",
    )
    parser.add_argument("--out", type=Path, default=OUTPUT)
    args = parser.parse_args(argv)

    try:
        events, corpus_sha256 = benchmark.load_events(CORPUS, "all")
        targets = _load_targets(CORPUS, events)
        base, api_key = benchmark._api_credentials()
    except benchmark.BenchmarkError as exc:
        parser.error(str(exc))

    calls = []
    call_number = 0
    for target in targets:
        for variant, event in _variants(target):
            for repetition in range(1, REPETITIONS + 1):
                call_number += 1
                calls.append(_run_call(call_number, target, variant, event, repetition, base, api_key))

    report = {
        "probe": "Codex turn_aborted payload field projection",
        "request_design": {
            "requested_model": REQUESTED_MODEL,
            "targets_per_call": 1,
            "variants": ["parser_evidence", "payload_reason_error_message"],
            "repetitions_per_event_variant": REPETITIONS,
            "timeout_seconds": TIMEOUT_SECONDS,
            "automatic_retry": False,
            "context": "single target event, causal prefix, 8192-byte cap",
        },
        "provenance": {
            "created_at_utc": _utc(),
            "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "benchmark_sha256": hashlib.sha256(Path(benchmark.__file__).read_bytes()).hexdigest(),
            "corpus_sha256": corpus_sha256,
        },
        "calls": calls,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
