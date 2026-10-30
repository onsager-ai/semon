#!/usr/bin/env python3
"""Compare a common selected transcript tail read on identical native fixtures.

The baseline and candidate receive byte-identical native files and separate
empty indexes. Catalog readiness is measured separately from warm selected-tail
reads. Candidate browser paint belongs to catalog-journey.py.
"""
import argparse
import hashlib
import importlib.util
import json
import shutil
import tempfile
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import urlopen


TAIL_TEXT = "Fixed selected tail."
SELECTED_SAMPLES = 5
READINESS_TIMEOUT_SECONDS = 60
HTTP_TIMEOUT_SECONDS = 60
ERROR_BODY_LIMIT = 1024


def load_catalog_journey():
    path = Path(__file__).with_name("catalog-journey.py")
    spec = importlib.util.spec_from_file_location("catalog_journey", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("Cannot load the shared Catalog fixture builder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fixture_digest(home):
    digest = hashlib.sha256()
    for path in sorted(path for path in home.rglob("*") if path.is_file()):
        digest.update(path.relative_to(home).as_posix().encode())
        digest.update(b"\0")
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
    return digest.hexdigest()


def body_excerpt(raw):
    try:
        rendered = json.dumps(json.loads(raw), ensure_ascii=False, separators=(",", ":"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        rendered = raw.decode("utf-8", errors="replace")
    if len(rendered) > ERROR_BODY_LIMIT:
        return rendered[:ERROR_BODY_LIMIT] + "…"
    return rendered


def http_json(base, token, path, timeout=HTTP_TIMEOUT_SECONDS):
    separator = "&" if "?" in path else "?"
    url = f"{base}{path}{separator}{urlencode({'t': token})}"
    started = time.perf_counter()
    error_body_truncated = False
    try:
        with urlopen(url, timeout=timeout) as response:
            status = response.status
            raw = response.read()
    except HTTPError as response:
        status = response.code
        raw = response.read(ERROR_BODY_LIMIT + 1)
        error_body_truncated = len(raw) > ERROR_BODY_LIMIT
        raw = raw[:ERROR_BODY_LIMIT]
    elapsed_ms = (time.perf_counter() - started) * 1000
    timing = {"status": status, "elapsed_ms": elapsed_ms}
    if status == 200:
        timing["response_bytes"] = len(raw)
    try:
        body = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        if status == 200:
            raise RuntimeError(
                f"{path.split('?')[0]} returned non-JSON HTTP 200; "
                f"body={body_excerpt(raw)!r}"
            ) from error
        body = None
    if status != 200:
        timing.update(
            error_body_excerpt=body_excerpt(raw),
            error_body_bytes_captured=len(raw),
            error_body_truncated=error_body_truncated,
        )
    return timing, body


def io_delta(before, after):
    keys = ("rchar", "wchar", "read_bytes", "write_bytes", "cpu_ticks")
    return {key: after.get(key, 0) - before.get(key, 0) for key in keys}


def normalized_entries(entries):
    """Keep only common transcript content; omit slot, IDs and provenance."""
    return [{"kind": entry.get("k"), "text": entry.get("text")} for entry in entries]


def require_tail_response(label, timing, body):
    if timing["status"] != 200:
        detail = timing.get("error_body_excerpt", "<no body>")
        raise RuntimeError(
            f"{label} selected transcript failed: HTTP {timing['status']}; body={detail}"
        )
    entries = body.get("entries") if isinstance(body, dict) else None
    if not isinstance(entries, list):
        raise RuntimeError(f"{label} selected transcript returned no entries array")
    normalized = normalized_entries(entries)
    expected = [{"kind": "a", "text": TAIL_TEXT}]
    if normalized != expected:
        raise RuntimeError(
            f"{label} selected tail differs from the fixed fixture: "
            f"{json.dumps(normalized, ensure_ascii=False)}"
        )
    return normalized


def wait_for_catalog(journey, server, auth, label):
    query = "/api/sessions?machine=local&limit=60"
    deadline = time.monotonic() + READINESS_TIMEOUT_SECONDS
    announced_at = time.perf_counter()
    polls = []
    while time.monotonic() < deadline:
        before = journey.process_sample(server.pid)
        remaining = max(0.1, deadline - time.monotonic())
        timing, body = http_json(
            auth["base"], auth["token"], query,
            timeout=min(HTTP_TIMEOUT_SECONDS, remaining),
        )
        after = journey.process_sample(server.pid)
        poll = {"status": timing["status"], "elapsed_ms": timing["elapsed_ms"]}
        items = body.get("items") if isinstance(body, dict) else None
        if timing["status"] == 200 and isinstance(items, list):
            poll["selected_session_present"] = any(
                item.get("key") == "backlog" for item in items if isinstance(item, dict)
            )
        if timing["status"] != 200:
            poll["error_body_excerpt"] = timing.get("error_body_excerpt", "<no body>")
            poll["error_body_truncated"] = timing.get("error_body_truncated", False)
        polls.append(poll)
        if timing["status"] == 200:
            if not isinstance(items, list):
                raise RuntimeError(
                    f"{label} Catalog readiness returned HTTP 200 without items; "
                    f"body={body_excerpt(json.dumps(body).encode())!r}"
                )
            if not poll["selected_session_present"]:
                time.sleep(min(0.05, max(0, deadline - time.monotonic())))
                continue
            return {
                "ready_after_listener_announcement_ms": (time.perf_counter() - announced_at) * 1000,
                "poll_count": len(polls),
                "polls": polls,
                "catalog_request": {
                    **timing,
                    "process_io_delta": io_delta(before, after),
                    "catalog_page_items": len(items),
                },
            }
        if timing["status"] != 503:
            raise RuntimeError(
                f"{label} Catalog readiness failed: HTTP {timing['status']}; "
                f"body={timing.get('error_body_excerpt', '<no body>')}"
            )
        time.sleep(min(0.05, max(0, deadline - time.monotonic())))

    last = polls[-1] if polls else {"status": "no response"}
    raise RuntimeError(
        f"{label} Catalog did not become ready within {READINESS_TIMEOUT_SECONDS}s; "
        f"polls={len(polls)} last={last}"
    )


def measure_phase(journey, binary, home, now, revision, label, additional, phase):
    server, auth = journey.start_server(binary, home, now)
    try:
        readiness = wait_for_catalog(journey, server, auth, label)
        query = "/api/session-transcript?machine=local&sid=backlog&limit=1"

        # A separate unmeasured read qualifies the selected tail as warm before
        # collecting fixed samples. Focused samples are never retried.
        warmup_timing, warmup_body = http_json(auth["base"], auth["token"], query)
        warmup_entries = require_tail_response(label, warmup_timing, warmup_body)

        selected = []
        for sample in range(SELECTED_SAMPLES):
            sample_before = journey.process_sample(server.pid)
            timing, body = http_json(auth["base"], auth["token"], query)
            sample_after = journey.process_sample(server.pid)
            entries = require_tail_response(label, timing, body)
            selected.append({
                "sample": sample + 1,
                **timing,
                "process_io_delta": io_delta(sample_before, sample_after),
                "entry_count": len(entries),
                "entries": entries,
            })

        return {
            "revision_label": label,
            "source_revision": revision,
            "phase": phase,
            "unrelated_sessions": additional,
            "catalog_readiness": readiness,
            "focused_read": {
                "query_scope": "default selected tail, limit=1",
                "warmup": warmup_timing,
                "warmup_entries": warmup_entries,
                "samples": selected,
                "sample_count": len(selected),
            },
            "process_after": journey.process_sample(server.pid),
            "process_memory_scope": "whole server process; includes producer work, not per-request peak memory",
            "source_range_provider_calls": None,
        }
    finally:
        server.terminate()
        server.wait(timeout=10)


def write_results(path, results, baseline_revision, candidate_revision):
    path.write_text(json.dumps({
        "baseline_revision": baseline_revision,
        "candidate_revision": candidate_revision,
        "comparison_scope": "whole-revision context; no causal speedup attribution",
        "limits": [
            "identical generated native files and fixed clock are cloned into separate empty indexes",
            "process-cold with warm OS file caches; Catalog readiness is reported separately",
            "five selected transcript samples follow one unmeasured selected-tail warmup",
        "focused reads are not retried; Catalog readiness polls stop when backlog appears or after 60 seconds",
            "process I/O and RSS are whole-process observations and include background work",
            "internal native source-range provider call counts are not exposed by this HTTP API",
            "per-request peak memory and production latency are not measured",
        ],
        "results": results,
    }, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-binary", required=True)
    parser.add_argument("--baseline-revision", required=True)
    parser.add_argument("--candidate-binary", required=True)
    parser.add_argument("--candidate-revision", required=True)
    parser.add_argument("--public-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--counts", default="0,256,2048")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    output_path = args.output / "focused-read-comparison.json"
    journey = load_catalog_journey()
    variants = [
        ("baseline", args.baseline_binary, args.baseline_revision),
        ("candidate", args.candidate_binary, args.candidate_revision),
    ]
    binary_hashes = {
        label: hashlib.sha256(Path(binary).read_bytes()).hexdigest()
        for label, binary, _ in variants
    }
    results = []
    with tempfile.TemporaryDirectory(prefix="semon-focused-read-") as temporary:
        work = Path(temporary)
        for additional in map(int, args.counts.split(",")):
            if additional < 0:
                raise ValueError("History counts must be nonnegative")
            fixture = work / f"history-{additional}-fixture"
            fixture.mkdir()
            now = journey.seed(fixture, args.public_root, additional, TAIL_TEXT)
            digest = fixture_digest(fixture)
            for label, binary, revision in variants:
                home = work / f"history-{additional}-{label}"
                shutil.copytree(fixture, home)
                for phase in ("first-import", "persisted-restart"):
                    result = measure_phase(
                        journey, binary, home, now, revision, label, additional, phase
                    )
                    result["fixture_sha256"] = digest
                    result["binary_sha256"] = binary_hashes[label]
                    results.append(result)
                    write_results(
                        output_path, results, args.baseline_revision, args.candidate_revision
                    )

        by_fixture_phase = {}
        for result in results:
            key = (result["unrelated_sessions"], result["phase"])
            by_fixture_phase.setdefault(key, {})[result["revision_label"]] = result
        for key, pair in by_fixture_phase.items():
            baseline = pair["baseline"]["focused_read"]["samples"][0]["entries"]
            candidate = pair["candidate"]["focused_read"]["samples"][0]["entries"]
            equivalent = baseline == candidate == [{"kind": "a", "text": TAIL_TEXT}]
            pair["baseline"]["output_equivalent_to_candidate"] = equivalent
            pair["candidate"]["output_equivalent_to_baseline"] = equivalent
            if not equivalent:
                raise RuntimeError(f"Selected transcript content differs for fixture/phase {key}")

    write_results(output_path, results, args.baseline_revision, args.candidate_revision)
    print(json.dumps({"output": str(output_path), "result_count": len(results)}, indent=2))


if __name__ == "__main__":
    main()
