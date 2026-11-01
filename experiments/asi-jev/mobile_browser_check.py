#!/usr/bin/env python3
"""Synthetic mobile browser coverage for the standalone ASI / Jev viewer.

The fixture starts from public aggregate-only output and adds synthetic review
events and synthetic session projections. It never loads the private review
packet or private prediction files.
"""

from __future__ import annotations

import argparse
import functools
import json
import os
import shutil
import tempfile
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

import local_viewer
from real_pipeline import thermal_checkpoint


EXPERIMENT_ROOT = Path(__file__).resolve().parent
VIEWPORTS = (
    ("iphone-se", 375, 812),
    ("iphone-large", 414, 896),
    ("desktop", 1280, 900),
)
RESULT_VIEWS = (
    ("criteria", "acceptance"),
    ("tradeoff", "quality-cost"),
    ("classes", "per-class"),
    ("sessions", "sessions"),
    ("failures", "failure-explorer"),
    ("methodology", "58-vs-436"),
    ("caveats", "caveats"),
)
CHOICES = ("Agree", "Disagree", "Unsure")
SHARE_INIT_SCRIPT = """(() => {
  window.__shareCalls = [];
  window.__shareMode = 'success';
  Object.defineProperty(navigator, 'canShare', {
    configurable: true,
    value: (payload) => Boolean(payload && Array.isArray(payload.files) && payload.files.length)
  });
  Object.defineProperty(navigator, 'share', {
    configurable: true,
    value: async (payload) => {
      window.__shareCalls.push(payload);
      if (window.__shareMode === 'cancel') {
        throw new DOMException('The user cancelled the share sheet.', 'AbortError');
      }
      return undefined;
    }
  });
})();"""


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:
        pass


def synthetic_fixture() -> tuple[dict[str, object], dict[str, object]]:
    base_reference = {
        "activity": "tool_action",
        "failure_category": "none",
        "retry_loop": "none",
        "anomaly": False,
        "phase": "active",
    }
    long_context = (
        "<script>window.untrustedMarkupRan=true</script> SYNTHETIC ONLY. "
        + " ".join(
            f"Synthetic context phrase {index:03d} wraps naturally across the narrow viewer column."
            for index in range(1, 19)
        )
        + "\nSynthetic-unbroken-token-"
        + "x" * 160
    )
    events: list[dict[str, object]] = []
    for index in range(50):
        reference = base_reference
        if index == 0:
            reference = {
                "activity": "tool_action",
                "failure_category": "timeout",
                "retry_loop": "retry",
                "anomaly": True,
                "phase": "active",
            }
        elif index == 1:
            reference = {
                "activity": "tool_action",
                "failure_category": "none",
                "retry_loop": "loop",
                "anomaly": False,
                "phase": "active",
            }
        events.append({
            "id": f"synthetic-{index:02d}",
            "harness": "codex",
            "context": long_context,
            "reference": reference,
        })

    review = {
        "schema": 1,
        "aggregate_only": False,
        "packet_id": "mobile-browser-synthetic",
        "events": events,
    }

    rare_reference = {
        "activity": "tool_action",
        "failure_category": "timeout",
        "retry_loop": "retry",
        "anomaly": True,
        "phase": "active",
    }
    loop_reference = {
        "activity": "tool_action",
        "failure_category": "none",
        "retry_loop": "loop",
        "anomaly": False,
        "phase": "active",
    }
    # The public aggregate build supplies the complete condition rows. These
    # event projections mirror the shapes used by viewer_browser_check.py.
    return review, {
        "events": [
            {
                "id": "synthetic-result-01",
                "order": 1,
                "context": long_context,
                "reference": rare_reference,
                "causal_reference": rare_reference,
                "predictions": {},
            },
            {
                "id": "synthetic-result-02",
                "order": 2,
                "context": long_context,
                "reference": loop_reference,
                "causal_reference": loop_reference,
                "predictions": {},
            },
        ],
    }


def create_synthetic_site(site: Path, *, mock_thermal: bool) -> None:
    site.mkdir(parents=True, exist_ok=True)
    os.chmod(site, 0o700)
    with tempfile.TemporaryDirectory(prefix="asi-jev-mobile-aggregate-") as temporary:
        aggregate = Path(temporary) / "aggregate"
        if mock_thermal:
            with patch.object(local_viewer, "thermal_checkpoint", return_value=35_000):
                local_viewer.build(aggregate, aggregate_only=True)
        else:
            local_viewer.build(aggregate, aggregate_only=True)

        for name in ("index.html", "app.js", "style.css"):
            shutil.copyfile(aggregate / name, site / name)
        results = json.loads((aggregate / "results.json").read_text(encoding="utf-8"))
        # Public aggregates still reveal the real model ranking. Use their
        # schema only: phone screenshots must remain blind to actual results.
        def illustrative(value, key=""):
            if isinstance(value, dict):
                return {name: ("insufficient" if name == "status" else illustrative(item, name))
                        for name, item in value.items()}
            if isinstance(value, list):
                if len(value) == 2 and all(isinstance(item, (int, float)) for item in value) and any(part in key for part in ("ci", "interval")):
                    return [0.4, 0.6]
                return [illustrative(item, key) for item in value]
            if isinstance(value, bool):
                return value
            if isinstance(value, (int, float)):
                if key in {"schema", "threshold", "minimum_unique_positive_events"}:
                    return value
                if key in {"lower", "lower_bound", "ci_lower"}:
                    return 0.4
                if key in {"upper", "upper_bound", "ci_upper"}:
                    return 0.6
                if key in {"estimate", "accuracy", "macro_f1", "precision", "recall", "f1", "bound"}:
                    return 0.5
                if "cost" in key:
                    return 0.001
                if key.endswith("_ms"):
                    return 1000
                return 20
            return value

        results = illustrative(results)
        results["counts"] = {"events": 2, "sessions": 1}
        results["first_run"]["events"] = 4
        results["first_run"]["sessions"] = 2
        for index, condition in enumerate(results["conditions"], 1):
            condition["label"] = f"Example condition {index}"
        for index, condition in enumerate(results["first_run"]["conditions"], 1):
            condition["label"] = f"Example condition {index}"
        index_path = site / "index.html"
        index_path.write_text(index_path.read_text().replace("Private evaluation", "Synthetic UI preview"))
        app_path = site / "app.js"
        app_path.write_text(app_path.read_text().replace(
            "The measured conditions did not qualify for production ASI integration.",
            "Synthetic preview: these values are illustrative.",
        ))
        review, synthetic_projection = synthetic_fixture()
        conditions = results["conditions"]
        synthetic_events = synthetic_projection["events"]
        for event in synthetic_events:
            event_predictions: dict[str, list[dict[str, object]]] = {}
            for condition in conditions:
                predicted = dict(event["reference"])
                predicted["phase"] = "blocked" if event["order"] == 1 else "recovery"
                event_predictions[condition["id"]] = [{
                    "repetition": 1,
                    "labels": predicted,
                }]
            event["predictions"] = event_predictions
        results["sessions"] = [{
            "key": "Synthetic session",
            "harness": "codex",
            "context": "prefix",
            "events": synthetic_events,
        }]
        results["confusion"] = {
            condition["id"]: {
                "activity": {
                    "labels": [
                        "tool_action", "failure", "wait", "recovery", "completion",
                        "handoff", "planning", "unknown", "<abstain>",
                    ],
                    "row_labels": [
                        "tool_action", "failure", "wait", "recovery", "completion",
                        "handoff", "planning", "unknown",
                    ],
                    "matrix": [
                        [1 / 3 if row == 0 and column == 0 else 2 / 3 if row == 0 and column == 1 else 0
                         for column in range(9)]
                        for row in range(8)
                    ],
                },
            }
            for condition in conditions
        }
        results["confusion_supported"] = True
        (site / "review.json").write_text(json.dumps(review), encoding="utf-8")
        (site / "results.json").write_text(json.dumps(results), encoding="utf-8")


def assert_no_horizontal_overflow(page, label: str) -> None:
    metrics = page.evaluate("""() => {
      const visible = (element) => {
        const style = getComputedStyle(element);
        const rect = element.getBoundingClientRect();
        return style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.height > 0;
      };
      const targeted = [
        '#spot-context', '.context-frame', '#spot-reference',
        '#results-nav',
        '.result-view:not([hidden]) table', '.result-view:not([hidden]) svg',
        '.result-view:not([hidden]) canvas', '.result-view:not([hidden]) pre',
        '.result-view:not([hidden]) .chart', '.result-view:not([hidden]) .heatmap-grid',
        '.result-view:not([hidden]) .timeline-scroll', '.result-view:not([hidden]) .timeline-track'
      ].flatMap((selector) => [...document.querySelectorAll(selector)]);
      const overflow = [...new Set(targeted)].filter((element) => visible(element) && element.scrollWidth > element.clientWidth)
        .map((element) => ({
          selector: element.id ? `#${element.id}` : element.className?.baseVal || element.className || element.tagName,
          scrollWidth: element.scrollWidth,
          clientWidth: element.clientWidth,
          overflowX: getComputedStyle(element).overflowX
        }));
      return {
        documentScrollWidth: document.documentElement.scrollWidth,
        innerWidth: window.innerWidth,
        overflow
      };
    }""")
    assert metrics["documentScrollWidth"] <= metrics["innerWidth"], (
        f"{label}: document horizontal overflow {metrics}"
    )
    assert not metrics["overflow"], f"{label}: inner horizontal overflow {metrics['overflow']}"


def assert_reference_prominent(page, viewport_height: int, *, first_phone_screen: bool = False) -> None:
    frame = page.locator(".reference-frame")
    frame.wait_for(state="visible")
    assert "frozen causal reference" in frame.inner_text().lower()
    assert "LOCKED" in frame.inner_text()
    labels = set(page.locator("#spot-reference dt").all_text_contents())
    assert labels == {"Activity", "Failure category", "Retry / loop", "Anomaly", "Phase"}, labels
    values = [value.lower() for value in page.locator("#spot-reference dd").all_text_contents()]
    assert {"timeout", "retry", "true"}.issubset(set(values)), f"rare reference labels are missing: {values}"
    reference_box = page.locator("#spot-reference").bounding_box()
    decision_box = page.locator("#decision-options").bounding_box()
    assert reference_box and decision_box and reference_box["height"] > 0
    if first_phone_screen:
        dock = page.locator(".mobile-dock").bounding_box()
        assert reference_box["y"] >= 0 and reference_box["y"] + reference_box["height"] <= min(viewport_height, dock["y"]), (
            f"frozen reference must fit above the phone dock: {reference_box}"
        )


def assert_decision_targets(page, label: str) -> None:
    boxes = page.locator("#decision-options button").evaluate_all("""buttons => buttons.map(button => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height, text: button.textContent.trim() };
    })""")
    assert len(boxes) == 3, f"{label}: expected three decision targets, got {boxes}"
    assert all(box["width"] >= 44 and box["height"] >= 44 for box in boxes), (
        f"{label}: decision target below 44 by 44 CSS pixels: {boxes}"
    )


def assert_mobile_dock_visible(page, viewport_height: int, label: str) -> None:
    metrics = page.evaluate("""() => {
      const selectors = ['#previous-event', '#spot-position', '#next-event', ...
        [...document.querySelectorAll('#decision-options button')].map((_, index) =>
          `#decision-options button:nth-child(${index + 1})`)];
      return selectors.map((selector) => {
        const element = document.querySelector(selector);
        if (!element) return {selector, missing: true};
        const rect = element.getBoundingClientRect();
        const x = rect.left + rect.width / 2;
        const y = rect.top + rect.height / 2;
        const hit = document.elementFromPoint(x, y);
        return {
          selector,
          top: rect.top,
          bottom: rect.bottom,
          visible: element.getClientRects().length > 0,
          unobscured: hit === element || element.contains(hit),
        };
      });
    }""")
    assert len(metrics) == 6 and all(
        row.get("visible")
        and row["top"] >= 0
        and row["bottom"] <= viewport_height
        and row["unobscured"]
        for row in metrics
    ), f"{label}: fixed decision/navigation dock is clipped or covered: {metrics}"


def record_page_errors(page, errors: list[str], requests: list[str]) -> None:
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: requests.append(request.url))


def browser_thermal_checkpoint(label: str, *, mocked: bool) -> None:
    if mocked:
        return
    temperature = thermal_checkpoint()
    print(f"{label}_millidegrees_c={temperature}", flush=True)


def run_viewport(browser, engine: str, viewport_name: str, width: int, height: int,
                 origin: str, output: Path, errors: list[str], requests: list[str]) -> list[str]:
    is_phone = width < 500
    user_agent = None
    if engine == "webkit" and is_phone:
        user_agent = (
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) "
            "AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 "
            "Mobile/15E148 Safari/604.1"
        )
    context_options = {
        "viewport": {"width": width, "height": height},
        "device_scale_factor": 1,
        "is_mobile": is_phone,
        "has_touch": is_phone,
        "reduced_motion": "reduce",
    }
    if user_agent:
        context_options["user_agent"] = user_agent
    context = browser.new_context(**context_options)
    context.set_default_timeout(7000)
    context.add_init_script(SHARE_INIT_SCRIPT)
    page = context.new_page()
    record_page_errors(page, errors, requests)
    shots = output / "screens" / engine / f"{width}x{height}"
    shots.mkdir(parents=True, exist_ok=True)
    captured: list[str] = []
    try:
        page.goto(origin + "/", wait_until="domcontentloaded")
        page.locator("#review-panel").wait_for(state="visible")
        assert not page.locator("#results-app").is_visible()
        assert page.locator("#reveal-results").is_disabled()
        assert page.locator("#review-count").inner_text() == "0 / 50"
        assert page.evaluate("window.untrustedMarkupRan") is None
        assert_no_horizontal_overflow(page, f"{engine}/{width} spotcheck collapsed")
        if is_phone:
            assert_decision_targets(page, f"{engine}/{width}")
        assert_reference_prominent(page, height, first_phone_screen=is_phone)
        if is_phone:
            assert_mobile_dock_visible(page, height, f"{engine}/{width}")
        page.screenshot(path=str(shots / "spotcheck.png"), animations="disabled")
        captured.append(str(shots / "spotcheck.png"))

        context_text = page.locator("#spot-context")
        toggle = page.locator("#context-toggle")
        assert toggle.count() == 1, "spot context disclosure control is missing"
        assert toggle.get_attribute("aria-expanded") == "false", "spot context must start collapsed"
        toggle.click()
        assert toggle.get_attribute("aria-expanded") == "true"
        page.locator("#context-frame.is-open").wait_for(state="attached")
        context_text.wait_for(state="visible")
        assert context_text.is_visible()
        content_length = context_text.evaluate("element => element.textContent.length")
        context_box = None
        for _ in range(40):
            context_box = context_text.bounding_box()
            if context_box and context_box["height"] > 50:
                break
            page.wait_for_timeout(25)
        assert content_length > 400 and context_box and context_box["height"] > 50, (
            f"expanded context did not render and wrap: length={content_length}, box={context_box}"
        )
        assert_reference_prominent(page, height)
        assert_no_horizontal_overflow(page, f"{engine}/{width} spotcheck expanded")
        expanded_path = shots / "spotcheck-context-expanded.png"
        page.screenshot(path=str(expanded_path), full_page=True, animations="disabled")
        captured.append(str(expanded_path))

        assert page.locator("#previous-event").is_disabled()
        page.locator("#next-event").click()
        assert page.locator("#spot-position").inner_text().endswith("Event 2")
        page.locator("#previous-event").click()
        assert page.locator("#spot-position").inner_text().endswith("Event 1")

        for index in range(3):
            if index:
                page.locator("#next-event").click()
            if index == 0:
                page.locator("#spot-note").fill("Synthetic autosave note")
            page.get_by_role("button", name=CHOICES[index % len(CHOICES)], exact=True).click()
            assert page.locator("#review-count").inner_text() == f"{index + 1} / 50"
        assert "Draft saved in this browser" in page.locator("#cache-status").inner_text()
        page.reload(wait_until="domcontentloaded")
        page.locator("#review-panel").wait_for(state="visible")
        assert page.locator("#review-count").inner_text() == "3 / 50"
        assert page.locator("#spot-position").inner_text().endswith("Event 4")
        page.locator("#previous-event").click()
        assert page.locator("#spot-position").inner_text().endswith("Event 3")
        assert page.get_by_role("button", name=CHOICES[2], exact=True).get_attribute("aria-pressed") == "true"
        page.locator("#next-event").click()

        for index in range(3, 50):
            page.get_by_role("button", name=CHOICES[index % len(CHOICES)], exact=True).click()
            assert page.locator("#review-count").inner_text() == f"{index + 1} / 50"
            if index < 49:
                page.locator("#next-event").click()
        assert page.locator("#next-event").is_disabled()
        assert page.locator("#reveal-results").is_disabled()

        share_button = page.locator("#share-review")
        if is_phone:
            assert share_button.count() == 1, "mobile share control is missing"
            assert share_button.is_visible() and not share_button.is_disabled()
            share_button.click()
            success_calls = page.evaluate("window.__shareCalls.length")
            assert success_calls == 1, "supported share success branch did not call navigator.share"
            share_file = page.evaluate("""() => {
              const file = window.__shareCalls[0]?.files?.[0];
              return file ? { name: file.name, type: file.type, size: file.size } : null;
            }""")
            assert share_file and share_file["name"].endswith(".jsonl")
            assert share_file["type"] == "application/json"
            assert page.locator("#reveal-results").is_disabled(), "share must not bypass the saved-file gate"

        with page.expect_download() as download_info:
            page.locator("#download-review").click()
        download_path = output / "downloads" / engine / f"{width}x{height}" / "answers.jsonl"
        download_path.parent.mkdir(parents=True, exist_ok=True)
        download_info.value.save_as(download_path)
        lines = download_path.read_text(encoding="utf-8").splitlines()
        answers = [json.loads(line) for line in lines]
        assert len(answers) == 50
        assert all(answer["packet_id"] == "mobile-browser-synthetic" for answer in answers)
        assert all(answer["review_stage"] == "blind" for answer in answers)
        assert [answer["id"] for answer in answers] == [f"synthetic-{index:02d}" for index in range(50)]
        assert [answer["decision"] for answer in answers] == [
            ("agree", "disagree", "unsure")[index % 3] for index in range(50)
        ]
        assert all("predictions" not in answer and "context" not in answer for answer in answers)
        assert page.locator("#reveal-results").is_disabled(), "download alone must not unlock results"
        page.locator("#saved-file-confirm").check()
        assert page.locator("#reveal-results").is_disabled(), "saved-file confirmation alone must not unlock results"
        page.locator("#reveal-ack").check()
        assert not page.locator("#reveal-results").is_disabled(), "all review gates should now unlock results"
        page.locator("#reveal-results").click()
        page.locator("#results-app").wait_for(state="visible")
        assert any(url.endswith("/results.json") for url in requests)
        assert len(requests) == len([url for url in requests if url.startswith(origin + "/")]), (
            f"external request observed: {requests}"
        )
        nav_buttons = page.locator("#results-nav button")
        assert nav_buttons.count() == 7, f"expected seven result views, got {nav_buttons.count()}"
        for view_id, screenshot_name in RESULT_VIEWS:
            nav_button = page.locator(f'#results-nav [data-view="{view_id}"]')
            if is_phone:
                page.locator("#result-view-select").select_option(view_id)
            else:
                nav_button.click()
            section = page.locator(f"#view-{view_id}")
            section.wait_for(state="visible")
            assert section.inner_text().strip(), f"empty result view: {view_id}"
            assert_no_horizontal_overflow(page, f"{engine}/{width} {view_id}")
            page.evaluate("window.scrollTo(0, 0)")
            screenshot_path = shots / f"{screenshot_name}.png"
            page.screenshot(path=str(screenshot_path), animations="disabled")
            captured.append(str(screenshot_path))
            if view_id == "sessions":
                assert page.locator(".marker-anomaly").count() > 0
                assert page.locator(".marker-retry").count() > 0
                assert page.locator(".marker-loop").count() > 0
                assert page.locator(".marker-failure").count() > 0
            if view_id == "failures":
                if page.locator("#failure-filters-open").is_visible():
                    page.locator("#failure-filters-open").click()
                    page.locator("#failure-filter-sheet").wait_for(state="visible")
                page.locator("#failure-condition").select_option("jev-prefix")
                page.locator("#failure-dimension").select_option("phase")
                if page.locator("#failure-filters-apply").is_visible():
                    page.locator("#failure-filters-apply").click()
                assert page.locator(".failure-row").count() == 2
                page.locator(".failure-open").first.click()
                assert page.locator("#event-dialog").is_visible()
                assert "SYNTHETIC ONLY" in page.locator("#dialog-body").inner_text()
                page.locator("#dialog-close").click()
    finally:
        context.close()
    return captured


def run_share_cancel_fresh_page(browser, engine: str, origin: str,
                                errors: list[str], requests: list[str]) -> None:
    """Prove a first-export share cancellation leaves the save gate locked."""
    viewport = {"width": 375, "height": 812}
    user_agent = None
    if engine == "webkit":
        user_agent = (
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) "
            "AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 "
            "Mobile/15E148 Safari/604.1"
        )
    context_options = {
        "viewport": viewport,
        "device_scale_factor": 1,
        "is_mobile": True,
        "has_touch": True,
    }
    if user_agent:
        context_options["user_agent"] = user_agent
    context = browser.new_context(**context_options)
    context.set_default_timeout(7000)
    context.add_init_script(SHARE_INIT_SCRIPT)
    page = context.new_page()
    record_page_errors(page, errors, requests)
    downloads: list[str] = []
    page.on("download", lambda download: downloads.append(download.suggested_filename))
    try:
        page.goto(origin + "/", wait_until="domcontentloaded")
        page.locator("#review-panel").wait_for(state="visible")
        for index in range(50):
            page.get_by_role("button", name=CHOICES[index % len(CHOICES)], exact=True).click()
            if index < 49:
                page.locator("#next-event").click()
        assert page.locator("#review-count").inner_text() == "50 / 50"
        assert page.locator("#reveal-results").is_disabled()
        assert not page.locator("#saved-file-confirm-wrap").is_visible()
        assert page.evaluate("window.__shareCalls.length") == 0
        page.evaluate("window.__shareMode = 'cancel'")
        page.locator("#share-review").click()
        for _ in range(20):
            if page.evaluate("window.__shareCalls.length") == 1:
                break
            page.wait_for_timeout(25)
        assert page.evaluate("window.__shareCalls.length") == 1
        assert page.locator("#reveal-results").is_disabled(), "share cancellation must not mark a saved export"
        assert not page.locator("#saved-file-confirm-wrap").is_visible()
        assert not downloads, f"first export cancellation unexpectedly downloaded a file: {downloads}"
    finally:
        context.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=EXPERIMENT_ROOT / "private" / "mobile-qa",
        help="site and screenshot output directory (default: ignored private/mobile-qa)",
    )
    parser.add_argument(
        "--engines",
        choices=("all", "chromium", "webkit"),
        default="all",
        help="browser engines to cover; CI should use both Chromium and WebKit",
    )
    parser.add_argument(
        "--mock-thermal",
        action="store_true",
        help="mock generator thermal checkpoints for CI machines without Tctl hardware",
    )
    parser.add_argument(
        "--browser-cache",
        type=Path,
        help="optional task-private Playwright browser cache directory",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    output = args.output_dir.resolve()
    site = output / "site"
    output.mkdir(parents=True, exist_ok=True)
    if args.browser_cache:
        os.environ["PLAYWRIGHT_BROWSERS_PATH"] = str(args.browser_cache.resolve())
    create_synthetic_site(site, mock_thermal=args.mock_thermal)

    browser_thermal_checkpoint("pre_browser", mocked=args.mock_thermal)

    from playwright.sync_api import sync_playwright

    handler = functools.partial(QuietHandler, directory=str(site))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f"http://127.0.0.1:{server.server_port}"
    engines = ("chromium", "webkit") if args.engines == "all" else (args.engines,)
    captured: list[str] = []
    errors: list[str] = []
    requests: list[str] = []
    try:
        with sync_playwright() as playwright:
            for engine in engines:
                browser_type = getattr(playwright, engine)
                browser_thermal_checkpoint(f"pre_{engine}_launch", mocked=args.mock_thermal)
                browser = browser_type.launch()
                browser_thermal_checkpoint(f"post_{engine}_launch", mocked=args.mock_thermal)
                try:
                    for viewport_name, width, height in VIEWPORTS:
                        browser_thermal_checkpoint(
                            f"pre_{engine}_{width}x{height}", mocked=args.mock_thermal
                        )
                        captured.extend(run_viewport(
                            browser, engine, viewport_name, width, height,
                            origin, output, errors, requests,
                        ))
                        browser_thermal_checkpoint(
                            f"post_{engine}_{width}x{height}", mocked=args.mock_thermal
                        )
                    browser_thermal_checkpoint(f"pre_{engine}_share_cancel", mocked=args.mock_thermal)
                    run_share_cancel_fresh_page(browser, engine, origin, errors, requests)
                    browser_thermal_checkpoint(f"post_{engine}_share_cancel", mocked=args.mock_thermal)
                finally:
                    browser.close()
    finally:
        server.shutdown()
        server.server_close()
        if not args.mock_thermal:
            browser_thermal_checkpoint("post_browser", mocked=False)

    assert not errors, f"browser JavaScript errors: {errors}"
    print(json.dumps({
        "engines": list(engines),
        "viewports": [{"name": name, "width": width, "height": height} for name, width, height in VIEWPORTS],
        "screenshots": captured,
        "assertions": [
            "document horizontal overflow checked with no tolerance on all 8 views",
            "context, tables, charts and timeline inner overflow checked",
            "44 by 44 CSS pixel decision targets",
            "collapsed and expanded wrapped synthetic context; visible reference labels",
            "rare anomaly, retry, loop and failure markers",
            "progress, navigation, local autosave and reload recovery",
            "reveal gate remains locked through 50 choices, download, save confirmation and acknowledgement",
            "downloaded JSONL parsed and checked for 50 blind answers",
            "supported navigator.canShare/share success shim on phone viewports",
            "first-export cancellation shim on a separate fresh phone page leaves save gate locked",
        ],
        "native_ios_share_sheet": "not available in this Linux browser environment; WebKit iPhone Safari emulation and navigator share shims only",
    }, indent=2))


if __name__ == "__main__":
    main()
