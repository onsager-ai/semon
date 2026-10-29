#!/usr/bin/env python3
"""Small concurrent Jev burst probe over sanitized fixture user events."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import sys
import threading
import time
import urllib.error
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import benchmark

_status = threading.local()
_urlopen = urllib.request.urlopen


def _capture_status(*args, **kwargs):
    try:
        response = _urlopen(*args, **kwargs)
    except urllib.error.HTTPError as exc:
        _status.code = exc.code
        exc.close()
        raise
    _status.code = getattr(response, "status", response.getcode())
    return response


def _utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def _samples(events):
    found = {}
    ordered = sorted(events, key=lambda event: (event.timestamp if event.timestamp is not None else float("inf"), event.event_order, event.index))
    for event in ordered:
        kind = set(re.split(r"[^a-z]+", event.event_kind.lower()))
        if event.origin == "sanitized_fixture" and "user" in kind:
            found.setdefault(event.harness, event)
    if not found:
        raise benchmark.BenchmarkError("no sanitized fixture user events found")
    return list(found.values())


def _request(index, event, events, model, base, key, timeout):
    context = benchmark.format_context(event, events, "prefix", 8192)
    started = time.perf_counter()
    _status.code = None
    result = {
        "request": index, "event_id": event.event_id, "harness": event.harness,
        "native_sha256": event.native_sha256, "http_status": None,
        "error_code": None, "rtt_ms": None, "resolved_model": None,
        "input_tokens": None, "output_tokens": None, "usage_complete": False,
    }
    try:
        predictions, errors, _, usage = benchmark._request_batch(
            "jev", [event], {event.index: context}, model, base, key, timeout, "/v1/chat/completions"
        )
        result.update({
            "http_status": _status.code,
            "rtt_ms": usage["rtt_ms"],
            "resolved_model": usage["resolved_model"],
            "input_tokens": usage["input_tokens"],
            "output_tokens": usage["output_tokens"],
            "usage_complete": usage["usage_complete"],
        })
        if errors.get(event.index) or set(predictions.get(event.index, {})) != set(benchmark.DIMENSIONS):
            result["error_code"] = "invalid_answers"
    except benchmark.ProviderFailure as exc:
        result.update({"http_status": _status.code, "error_code": exc.code, "rtt_ms": exc.rtt_ms})
    except Exception as exc:
        result.update({"http_status": _status.code, "error_code": f"client_error_{type(exc).__name__}",
                       "rtt_ms": (time.perf_counter() - started) * 1000.0})
    finally:
        _status.code = None
    return result


def _level(concurrency, count, events, samples, model, base, key, timeout):
    started_utc, start = _utc(), time.perf_counter()
    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        futures = [pool.submit(_request, i + 1, samples[i % len(samples)], events, model, base, key, timeout)
                   for i in range(count)]
        results = [future.result() for future in as_completed(futures)]
    wall = max(time.perf_counter() - start, 1e-9)
    results.sort(key=lambda row: row["request"])
    successful = [row for row in results if row["error_code"] is None]
    priced = [row for row in successful if row["usage_complete"]]
    usage_complete = bool(successful) and len(priced) == len(successful)
    input_tokens = sum(row["input_tokens"] for row in priced)
    output_tokens = sum(row["output_tokens"] for row in priced)
    rtts = [row["rtt_ms"] for row in results if isinstance(row["rtt_ms"], (int, float))]
    return {
        "concurrency": concurrency, "started_at_utc": started_utc, "finished_at_utc": _utc(),
        "wall_seconds": wall, "attempted_requests": len(results), "successful_requests": len(successful),
        "failed_requests": len(results) - len(successful), "requests_per_second": len(results) / wall,
        "attempted_events_per_second": len(results) / wall, "successful_events_per_second": len(successful) / wall,
        "http_rtt_ms": benchmark._quantiles(rtts),
        "http_status_counts": dict(Counter(str(row["http_status"]) for row in results if row["http_status"] is not None)),
        "error_code_counts": dict(Counter(row["error_code"] for row in results if row["error_code"] is not None)),
        "usage": {"successful_input_tokens": input_tokens if usage_complete else None,
                  "successful_output_tokens": output_tokens if usage_complete else None,
                  "successful_calls_with_usage": len(priced), "successful_calls_missing_usage": len(successful) - len(priced),
                  "failed_request_cost_unknown": len(results) - len(successful)},
        "successful_subtotal_estimate_usd": (input_tokens * benchmark.JEV_LIST_PRICE_INPUT_PER_MILLION / 1_000_000
                                              if usage_complete else None),
        "requests": results,
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--events", type=Path, default=Path("experiments/asi-jev/data/events.jsonl"))
    parser.add_argument("--concurrency", type=int, nargs="+", default=[1, 2, 4])
    parser.add_argument("--requests-per-level", type=int, default=12)
    parser.add_argument("--out", type=Path, default=Path("experiments/asi-jev/results/load-probe.json"))
    parser.add_argument("--model", default="typesafe/jev-latest")
    parser.add_argument("--timeout", type=float, default=20.0)
    args = parser.parse_args(argv)
    if not args.concurrency or any(n < 1 for n in args.concurrency) or args.requests_per_level < 1 or args.timeout <= 0:
        parser.error("concurrency, requests-per-level, and timeout must be positive")
    try:
        events, corpus_sha = benchmark.load_events(args.events, "development")
        samples = _samples(events)
        base, key = benchmark._api_credentials()
    except benchmark.BenchmarkError as exc:
        parser.error(str(exc))

    urllib.request.urlopen = _capture_status
    try:
        levels = [_level(n, args.requests_per_level, events, samples, args.model, base, key, args.timeout)
                  for n in args.concurrency]
    finally:
        urllib.request.urlopen = _urlopen

    resolved = sorted({row["resolved_model"] for level in levels for row in level["requests"] if row["resolved_model"]})
    location = os.environ.get("ASI_CLIENT_LOCATION")
    location_source = "ASI_CLIENT_LOCATION" if location else "GITHUB_ACTIONS indicator" if os.environ.get("GITHUB_ACTIONS") == "true" else "unavailable"
    report = {
        "probe": "concurrent Jev burst; not sustained capacity testing",
        "requested_model": args.model, "resolved_models": resolved,
        "request_design": {"endpoint_path": "/v1/systemone", "helper": "benchmark._request_batch / make_jev_request",
                           "targets_per_request": 1, "dimensions": list(benchmark.DIMENSIONS),
                           "context": "sanitized fixture user event, causal prefix, 8192-byte cap",
                           "requests_per_level": args.requests_per_level, "timeout_seconds": args.timeout,
                           "samples": [{"harness": e.harness, "event_id": e.event_id, "native_sha256": e.native_sha256} for e in samples],
                           "automatic_retry": False, "fallback": False},
        "limitations": ["12 requests per level is a small burst, not sustained load capacity.",
                        "Repeated sanitized fixture prompts may benefit from provider caching.",
                        "Heldout synthetic challenge events are excluded."],
        "provenance": {"created_at_utc": _utc(), "python": sys.version.split()[0], "platform": platform.platform(),
                       "client_location": location or ("GitHub Actions hosted runner; region not exposed" if location_source == "GITHUB_ACTIONS indicator" else "not provided"),
                       "client_location_source": location_source,
                       "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                       "benchmark_sha256": hashlib.sha256(Path(benchmark.__file__).read_bytes()).hexdigest(),
                       "corpus_sha256": corpus_sha, "api_base_recorded": False, "api_credentials_recorded": False},
        "price_basis": {"input_usd_per_million": benchmark.JEV_LIST_PRICE_INPUT_PER_MILLION, "output_usd_per_million": 0.0,
                        "source": benchmark.JEV_PRICE_SOURCE, "failed_request_cost": "unknown"},
        "levels": levels,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
